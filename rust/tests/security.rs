#[cfg(unix)]
#[test]
fn atomic_write_preserves_existing_parent_mode() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("tempdir");
    let parent = temporary.path().join("public-parent");
    std::fs::create_dir(&parent).expect("mkdir");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755))
        .expect("set parent mode");
    engramark::durable_fs::atomic_write(&parent.join("value.txt"), "value\n")
        .expect("atomic write");
    let mode = std::fs::metadata(&parent)
        .expect("parent metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
}

#[cfg(unix)]
#[test]
fn private_log_append_is_private_and_rejects_symlinks() {
    use std::io::Write;
    use std::os::unix::fs::{symlink, PermissionsExt};

    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("private.log");
    let mut file = engramark::durable_fs::open_private_append(&path).expect("open private log");
    file.write_all(b"entry\n").expect("append log");
    drop(file);
    let mode = std::fs::metadata(&path)
        .expect("log metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    let link = temporary.path().join("linked.log");
    symlink(&path, &link).expect("log symlink");
    assert!(engramark::durable_fs::open_private_append(&link).is_err());
    assert_eq!(std::fs::read_to_string(path).expect("read log"), "entry\n");
}

#[cfg(unix)]
#[test]
fn layout_rejects_symlinked_data_root() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().expect("tempdir");
    let real = temporary.path().join("real");
    std::fs::create_dir(&real).expect("real root");
    let linked = temporary.path().join("linked");
    symlink(&real, &linked).expect("symlink");
    let layout = engramark::paths::Layout { home: linked };
    let error = layout.ensure().expect_err("symlink root must fail");
    assert!(error.to_string().contains("符号链接"));
}

#[cfg(unix)]
#[test]
fn layout_rejects_non_unicode_data_root_before_creation() {
    use std::os::unix::ffi::OsStringExt;

    let temporary = tempfile::tempdir().expect("tempdir");
    let invalid = std::ffi::OsString::from_vec(vec![b'n', b'o', b'n', 0xff]);
    let home = temporary.path().join(invalid);
    let layout = engramark::paths::Layout { home: home.clone() };
    let error = layout.ensure().expect_err("non-Unicode root must fail");
    assert!(error.to_string().contains("Unicode"));
    assert!(!home.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn private_permissions_reject_extended_acl() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("private.txt");
    std::fs::write(&path, b"private\n").expect("write fixture");
    let status = std::process::Command::new("chmod")
        .args(["+a", "everyone allow read"])
        .arg(&path)
        .status()
        .expect("run chmod");
    assert!(status.success());
    let error = engramark::durable_fs::chmod_private(&path, false)
        .expect_err("extended ACL must be rejected");
    assert!(error.to_string().contains("扩展 ACL"));
}

#[test]
fn write_connection_enforces_cache_pragmas() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("cache.sqlite");
    let conn = engramark::cache::open_write(&path, std::time::Duration::from_secs(1))
        .expect("open write cache");
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .expect("synchronous");
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("foreign keys");
    let trusted_schema: i64 = conn
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .expect("trusted schema");
    assert_eq!(journal, "delete");
    assert_eq!(synchronous, 2);
    assert_eq!(foreign_keys, 1);
    assert_eq!(trusted_schema, 0);
}
