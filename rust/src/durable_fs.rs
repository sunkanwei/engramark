//! Durable filesystem primitives: private creation, fsync, atomic replace,
//! durable unlink. Temporary files are created in the target directory with
//! private permissions from the start (no create-then-chmod window).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
fn reject_extended_acl(path: &Path) -> io::Result<()> {
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn acl_get_file(path: *const c_char, acl_type: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_free(value: *mut c_void) -> c_int;
    }

    const ACL_TYPE_EXTENDED: c_int = 0x100;
    const ACL_FIRST_ENTRY: c_int = 0;
    let raw = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径包含 NUL 字节"))?;
    let acl = unsafe { acl_get_file(raw.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(());
        }
        return Err(error);
    }
    let mut entry = std::ptr::null_mut();
    let has_entry = unsafe { acl_get_entry(acl, ACL_FIRST_ENTRY, &mut entry) } == 0;
    unsafe {
        acl_free(acl);
    }
    if has_entry {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "路径包含扩展 ACL，拒绝静默修改；请先人工移除额外授权：{}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn reject_extended_acl(path: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let raw = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径包含 NUL 字节"))?;
    let name = c"system.posix_acl_access";
    let size = unsafe { libc::getxattr(raw.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if size > 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "路径包含扩展 ACL，拒绝静默修改；请先人工移除额外授权：{}",
                path.display()
            ),
        ));
    }
    if size < 0 {
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(code) if code == libc::ENODATA || code == libc::ENOTSUP)
        {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn reject_extended_acl(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn chmod_private(path: &Path, directory: bool) -> io::Result<()> {
    if crate::paths::is_link_like(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("拒绝修改符号链接或重解析点权限：{}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("路径不属于当前用户：{}", path.display()),
            ));
        }
        reject_extended_acl(path)?;
        let mode = if directory { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        if fs::symlink_metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("无法收紧路径权限：{}", path.display()),
            ));
        }
    }
    #[cfg(windows)]
    {
        let _ = directory;
        crate::durable_fs::windows::apply_private_dacl(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> io::Result<File> {
    crate::durable_fs::windows::create_private_file(path)
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    crate::durable_fs::windows::create_private_directory(path)
}

#[cfg(windows)]
pub mod windows {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::path::Path;

    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        SECURITY_ATTRIBUTES,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FlushFileBuffers, MoveFileExW, CREATE_NEW, FILE_APPEND_DATA,
        FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH, OPEN_ALWAYS, OPEN_EXISTING,
    };

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn private_descriptor() -> io::Result<*mut c_void> {
        let descriptor_text: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)\0"
            .encode_utf16()
            .collect();
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor_text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(descriptor)
    }

    /// Protected DACL granting full control only to the owner, SYSTEM and the
    /// built-in Administrators group. The file owner is the current user for
    /// every path Engramark creates.
    pub fn apply_private_dacl(path: &Path) -> io::Result<()> {
        let descriptor = private_descriptor()?;
        let path_wide = wide(path.as_os_str());
        let applied = unsafe {
            SetFileSecurityW(
                path_wide.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if applied == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn create_private_file(path: &Path) -> io::Result<std::fs::File> {
        let descriptor = private_descriptor()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path_wide = wide(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_WRITE,
                0,
                &mut attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle as RawHandle) })
    }

    pub fn create_private_directory(path: &Path) -> io::Result<()> {
        let descriptor = private_descriptor()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path_wide = wide(path.as_os_str());
        let created = unsafe { CreateDirectoryW(path_wide.as_ptr(), &mut attributes) };
        unsafe {
            LocalFree(descriptor);
        }
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn open_private_append(path: &Path) -> io::Result<std::fs::File> {
        let descriptor = private_descriptor()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path_wide = wide(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                FILE_APPEND_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &mut attributes,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle as RawHandle) })
    }

    pub fn replace_file(tmp: &Path, path: &Path) -> io::Result<()> {
        let tmp_wide = wide(tmp.as_os_str());
        let path_wide = wide(path.as_os_str());
        let moved = unsafe {
            MoveFileExW(
                tmp_wide.as_ptr(),
                path_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn sync_directory(path: &Path) -> io::Result<()> {
        let path_wide = wide(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let flushed = unsafe { FlushFileBuffers(handle) };
        unsafe {
            CloseHandle(handle);
        }
        if flushed == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Open a private append-only log without a create-then-chmod window and
/// without following a link or Windows reparse point.
pub fn open_private_append(path: &Path) -> io::Result<File> {
    if crate::paths::is_link_like(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("日志文件不能是符号链接或重解析点：{}", path.display()),
        ));
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
    };
    #[cfg(windows)]
    let file = crate::durable_fs::windows::open_private_append(path)?;
    if crate::paths::is_link_like(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("日志文件不能是符号链接或重解析点：{}", path.display()),
        ));
    }
    chmod_private(path, false)?;
    Ok(file)
}

fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn fsync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let dir = fs::File::open(path)?;
        dir.sync_all()?;
    }
    #[cfg(windows)]
    {
        crate::durable_fs::windows::sync_directory(path)?;
    }
    Ok(())
}

/// Create missing directory components with private permissions from the first
/// observable instant. Existing ancestors are preserved unchanged.
pub fn create_dir_all_private(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无法确定目录父级"))?;
    }
    if !cursor.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("父路径不是目录：{}", cursor.display()),
        ));
    }
    for directory in missing.iter().rev() {
        match create_private_directory(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // Another process may be performing the same first-run setup.
                // Accept only a real directory; links and reparse points must
                // never win this race.
                if crate::paths::is_link_like(directory) || !directory.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("并发创建得到的路径不是安全目录：{}", directory.display()),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn temp_path_in(dir: &Path, suffix: &std::ffi::OsStr) -> PathBuf {
    let token = crate::clock::clock()
        .urlsafe_token()
        .replace(['-', '_'], "x");
    let mut name = std::ffi::OsString::from(format!(".tmp-{token}"));
    name.push(suffix);
    dir.join(name)
}

pub fn atomic_write_bytes(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent_existed = parent.is_dir();
    if !parent_existed {
        create_dir_all_private(parent)?;
    }
    let mut suffix = std::ffi::OsString::new();
    if let Some(extension) = path.extension() {
        suffix.push(".");
        suffix.push(extension);
    }
    for _ in 0..100 {
        let tmp = temp_path_in(parent, &suffix);
        let result = create_private_file(&tmp).and_then(|mut file| {
            chmod_private(&tmp, false)?;
            file.write_all(data)?;
            sync_file(&file)?;
            Ok(())
        });
        match result {
            Ok(()) => {
                return replace_durable(&tmp, path).inspect_err(|_| {
                    let _ = fs::remove_file(&tmp);
                });
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "临时文件冲突"))
}

/// Atomically replace a same-directory file without a remove-before-rename
/// gap. Windows uses MoveFileExW with replace + write-through semantics.
pub fn replace_durable(tmp: &Path, path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::durable_fs::windows::replace_file(tmp, path)?;
    }
    #[cfg(not(windows))]
    {
        fs::rename(tmp, path)?;
    }
    chmod_private(path, false)?;
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

pub fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    atomic_write_bytes(path, text.as_bytes())
}

pub fn durable_unlink(path: &Path) -> io::Result<()> {
    if path.try_exists().unwrap_or(false) {
        fs::remove_file(path)?;
        if let Some(parent) = path.parent() {
            fsync_dir(parent)?;
        }
    }
    Ok(())
}
