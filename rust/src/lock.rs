//! Timed cross-process shared/exclusive file locks with a globally fixed
//! order: mutation → cache.swap → radar-state/audit. Same-process re-entry in
//! the wrong order is a programming error, exactly as in the Python runtime.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::paths::Layout;
use crate::{Error, Result};

pub const LOCK_TIMEOUT: f64 = 5.0;

fn lock_timeout_env() -> f64 {
    std::env::var("ENGRAMARK_LOCK_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 3_600.0)
        .unwrap_or(LOCK_TIMEOUT)
}

fn order_of(name: &str) -> i32 {
    match name {
        "mutation" => 10,
        "cache.swap" => 20,
        "radar-state" | "audit" => 30,
        _ => -1,
    }
}

thread_local! {
    static LOCK_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub struct FileLock {
    name: String,
    file: Option<File>,
}

impl FileLock {
    pub fn acquire(
        layout: &Layout,
        name: &str,
        shared: bool,
        timeout: Option<f64>,
    ) -> Result<FileLock> {
        LOCK_STACK.with(|stack| {
            let stack = stack.borrow();
            let order = order_of(name);
            if order >= 0 && stack.iter().any(|held| order_of(held) > order) {
                return Err(Error::core(format!(
                    "非法锁顺序：{} → {name}",
                    stack.last().map(String::as_str).unwrap_or("")
                )));
            }
            Ok(())
        })?;
        let locks = layout.locks();
        crate::durable_fs::create_dir_all_private(&locks)
            .map_err(|err| Error::core(format!("无法创建锁目录：{err}")))?;
        crate::durable_fs::chmod_private(&locks, true)
            .map_err(|err| Error::core(format!("锁目录权限不安全：{err}")))?;
        let path: PathBuf = locks.join(format!("{name}.lock"));
        if crate::paths::is_link_like(&path) {
            return Err(Error::core(format!(
                "锁文件不能是符号链接：{}",
                path.display()
            )));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(&path)
            .map_err(|err| Error::core(format!("无法打开锁文件：{err}")))?;
        crate::durable_fs::chmod_private(&path, false)
            .map_err(|err| Error::core(format!("锁文件权限不安全：{err}")))?;
        let timeout = timeout.unwrap_or_else(lock_timeout_env);
        let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
        loop {
            let attempt = if shared {
                file.try_lock_shared()
            } else {
                file.try_lock()
            };
            match attempt {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(Error::lock_timeout(name));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    return Err(Error::core(format!("锁定 {name} 失败：{err}")));
                }
            }
        }
        LOCK_STACK.with(|stack| stack.borrow_mut().push(name.to_string()));
        Ok(FileLock {
            name: name.to_string(),
            file: Some(file),
        })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
        LOCK_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(position) = stack.iter().rposition(|held| *held == self.name) {
                stack.remove(position);
            }
        });
    }
}
