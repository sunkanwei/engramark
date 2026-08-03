//! Data root layout and project directory detection.
//! Paths must be losslessly representable as Unicode; to_string_lossy is never
//! used for persistent identifiers or configuration.

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::normalize::normalize_text;

pub const PROJECT_MARKERS: [&str; 10] = [
    ".git",
    ".hg",
    ".svn",
    ".codex",
    "pyproject.toml",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "build-profile.json5",
    "oh-package.json5",
];

#[derive(Clone, Debug)]
pub struct Layout {
    pub home: PathBuf,
}

impl Layout {
    pub fn discover() -> Layout {
        let home = std::env::var_os("ENGRAMARK_HOME")
            .filter(|value| !value.is_empty())
            .map(|value| match value.to_str() {
                Some(value) => expand_user(value),
                None => PathBuf::from(value),
            })
            .unwrap_or_else(|| home_dir().join("engramark"));
        Layout { home }
    }

    pub fn cards(&self) -> PathBuf {
        self.home.join("cards")
    }

    pub fn candidates(&self) -> PathBuf {
        self.home.join("candidates")
    }

    pub fn state(&self) -> PathBuf {
        self.home.join("state")
    }

    pub fn transactions(&self) -> PathBuf {
        self.state().join("transactions")
    }

    pub fn locks(&self) -> PathBuf {
        self.state().join("locks")
    }

    pub fn migration_backups(&self) -> PathBuf {
        self.state().join("migration-backups")
    }

    pub fn rollback_backups(&self) -> PathBuf {
        self.state().join("rollback-backups")
    }

    pub fn install_backups(&self) -> PathBuf {
        self.state().join("install-backups")
    }

    pub fn cache(&self) -> PathBuf {
        self.home.join("cache")
    }

    pub fn logs(&self) -> PathBuf {
        self.home.join("logs")
    }

    pub fn index(&self) -> PathBuf {
        self.cache().join("memory.mcache")
    }

    pub fn config(&self) -> PathBuf {
        self.home.join("engramark.json")
    }

    pub fn id_sequence(&self) -> PathBuf {
        self.state().join("id-sequence")
    }

    pub fn feedback_state(&self) -> PathBuf {
        self.state().join("feedback")
    }

    pub fn card_path(&self, id: i64) -> PathBuf {
        self.cards().join(format!("{id:04}.mem"))
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        require_unicode(&self.home)?;
        if !self.home.exists() {
            crate::durable_fs::create_dir_all_private(&self.home)?;
        }
        for directory in [
            self.home.clone(),
            self.cards(),
            self.state(),
            self.transactions(),
            self.locks(),
            self.migration_backups(),
            self.cache(),
            self.logs(),
        ] {
            match std::fs::symlink_metadata(&directory) {
                Ok(_) if is_link_like(&directory) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Engramark 目录不能是符号链接：{}", directory.display()),
                    ));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Engramark 路径不是目录：{}", directory.display()),
                    ));
                }
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    crate::durable_fs::create_dir_all_private(&directory)?;
                }
                Err(err) => return Err(err),
            }
            crate::durable_fs::chmod_private(&directory, true)?;
        }
        for file in [self.config(), self.id_sequence(), self.index()] {
            if is_link_like(&file) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Engramark 控制文件不能是符号链接：{}", file.display()),
                ));
            }
        }
        Ok(())
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn require_unicode(path: &Path) -> std::io::Result<&str> {
    path.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "路径必须能够无损表示为 Unicode",
        )
    })
}

pub fn is_link_like(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0;
    }
    #[cfg(not(windows))]
    false
}

/// pathlib expanduser for a leading "~" component.
pub fn expand_user(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(value)
}

/// pathlib resolve(strict=False): canonicalize the longest existing prefix,
/// append the remainder verbatim.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => {
            let mut missing: Vec<std::ffi::OsString> = Vec::new();
            let mut cursor = path;
            loop {
                match std::fs::canonicalize(cursor) {
                    Ok(resolved) => {
                        let mut out = resolved;
                        for part in missing.iter().rev() {
                            out.push(part);
                        }
                        return out;
                    }
                    Err(_) => match (cursor.file_name(), cursor.parent()) {
                        (Some(name), Some(parent)) => {
                            missing.push(name.to_os_string());
                            cursor = parent;
                        }
                        _ => return cursor.to_path_buf(),
                    },
                }
            }
        }
    }
}

fn resolve_strict(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

pub fn temp_dir() -> PathBuf {
    resolve_lenient(&std::env::temp_dir())
}

/// The program root (parent of the directory holding the executable), as the
/// Python reference derives from __file__.
pub fn program_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = resolve_lenient(&exe);
    exe.parent()?.parent().map(Path::to_path_buf)
}

pub fn broad_project_directories(layout: &Layout) -> Vec<PathBuf> {
    let user_home = resolve_lenient(&home_dir());
    let mut broad = vec![
        PathBuf::from("/"),
        user_home.clone(),
        user_home.join("Desktop"),
        user_home.join("Documents"),
        user_home.join("Downloads"),
        temp_dir(),
        resolve_lenient(&layout.home),
    ];
    if let Some(root) = program_root() {
        broad.push(root);
    }
    broad
}

/// project_id: name-sha1(canonical)[:6], or "global" for broad/invalid roots.
pub fn project_id(cwd: Option<&str>, layout: &Layout) -> String {
    let Some(cwd) = cwd else {
        return "global".into();
    };
    if cwd.is_empty() || cwd == "global" {
        return "global".into();
    }
    let expanded = expand_user(cwd);
    let path = resolve_lenient(&expanded);
    let user_home = resolve_lenient(&home_dir());
    let broad = [
        PathBuf::from("/"),
        user_home.clone(),
        user_home.join("Desktop"),
        user_home.join("Documents"),
        user_home.join("Downloads"),
        temp_dir(),
        resolve_lenient(&layout.home),
    ];
    if broad.iter().any(|b| b == &path) {
        return "global".into();
    }
    let Some(canonical) = path.to_str() else {
        // Persistent identifiers must be lossless Unicode (no surrogate lossy).
        return "global".into();
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let digest = Sha1::digest(canonical.as_bytes());
    let short: String = digest[..3].iter().map(|b| format!("{b:02x}")).collect();
    format!("{name}-{short}")
}

/// project_directory: walk up to a marker directory; authoritative mode
/// returns the resolved path itself when no marker matches.
pub fn project_directory(
    cwd: Option<&str>,
    authoritative: bool,
    layout: &Layout,
) -> Option<PathBuf> {
    let cwd = cwd?;
    if cwd.is_empty() {
        return None;
    }
    let expanded = expand_user(cwd);
    if !expanded.is_absolute() {
        return None;
    }
    let path = resolve_strict(&expanded)?;
    let broad = broad_project_directories(layout);
    let data_home = resolve_lenient(&layout.home);
    if !path.is_dir() || broad.iter().any(|b| b == &path) {
        return None;
    }
    if path == data_home || path.starts_with(&data_home) {
        return None;
    }
    if let Some(root) = program_root() {
        if path == root || path.starts_with(&root) {
            return None;
        }
    }
    let mut candidate = Some(path.as_path());
    while let Some(dir) = candidate {
        if broad.iter().any(|b| b == dir) {
            break;
        }
        if PROJECT_MARKERS
            .iter()
            .any(|marker| dir.join(marker).exists())
        {
            return Some(dir.to_path_buf());
        }
        candidate = dir.parent();
    }
    if authoritative {
        Some(path)
    } else {
        None
    }
}

pub fn project_context_id(cwd: Option<&str>, authoritative: bool, layout: &Layout) -> String {
    match project_directory(cwd, authoritative, layout) {
        Some(root) => project_id(root.to_str(), layout),
        None => "global".into(),
    }
}

pub fn normalize_for_match(value: &str) -> String {
    normalize_text(value)
}
