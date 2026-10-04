//! 本机会话存储：所有路径均为独立临时夹具，绝不迁移真实用户目录。
use dashboard_core::session_storage::SessionStorage;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn fixture() -> (PathBuf, SessionStorage) {
    let root = std::env::temp_dir().join(format!("dashboard-storage-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let storage = SessionStorage::new(root.join("sessions"), root.join("legacy"));
    (root, storage)
}
fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}
fn cached(source: &str, sid: &str) -> String {
    let meta = json!({"session_id":sid,"path":"/fixture/source.jsonl","mtime":1.0,"size":20,"projects":"historic-project","source":source});
    match source {
        "claude-cli" => meta.to_string(),
        "codex" => json!({"version":2,"meta":meta}).to_string(),
        _ => json!({"version":1,"fingerprint":"old","meta":meta}).to_string(),
    }
}
#[test]
fn fresh_install_initializes_once_and_never_reimports_old_favorites() {
    let (root, store) = fixture();
    store.ensure_initialized().unwrap();
    assert_eq!(store.load_favorites().unwrap().len(), 0);
    assert!(store.root().join(".layout-v1.json").is_file());
    assert!(!store.root().join("hidden.json").exists());
    write(&root.join("legacy/favorites.json"), r#"{"old":true}"#);
    let restarted = SessionStorage::new(store.root().to_owned(), root.join("legacy"));
    restarted.ensure_initialized().unwrap();
    assert!(!restarted.load_favorites().unwrap().contains_key("old"));
    fs::remove_file(store.root().join("favorites.json")).unwrap();
    assert!(restarted.load_favorites().is_err());
    assert!(
        SessionStorage::new(store.root().to_owned(), root.join("legacy"))
            .ensure_initialized()
            .is_err()
    );
}
#[test]
fn imports_four_actual_layouts_without_changing_cache_bytes_or_legacy() {
    let (root, store) = fixture();
    write(&root.join("legacy/favorites.json"), r#"{"same":true}"#);
    for source in ["claude-cli", "codex", "pi", "grok"] {
        let dir = if source == "claude-cli" {
            root.join("legacy")
        } else {
            root.join("legacy").join(source)
        };
        let bytes = cached(source, "same");
        write(&dir.join("same.json"), &bytes);
    }
    let report = store.ensure_initialized().unwrap();
    assert!(report.warnings.is_empty());
    assert_eq!(store.load_favorites().unwrap().get("same"), Some(&true));
    for source in ["claude-cli", "codex", "pi", "grok"] {
        assert_eq!(
            fs::read_to_string(store.cache_dir(source).join("same.json")).unwrap(),
            cached(source, "same")
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("legacy/favorites.json")).unwrap(),
        r#"{"same":true}"#
    );
    fs::remove_dir_all(store.root().join("cache")).unwrap();
    assert_eq!(store.load_favorites().unwrap().get("same"), Some(&true));
}
#[test]
fn interrupted_import_preserves_new_choices_and_existing_valid_cache() {
    let (root, store) = fixture();
    write(&root.join("legacy/favorites.json"), r#"{"old":true}"#);
    write(&store.root().join("favorites.json"), r#"{"new":true}"#);
    write(
        &root.join("legacy/same.json"),
        &cached("claude-cli", "same"),
    );
    let newer = cached("claude-cli", "same").replace("historic-project", "new-project");
    write(&store.cache_dir("claude-cli").join("same.json"), &newer);
    store.ensure_initialized().unwrap();
    assert_eq!(store.load_favorites().unwrap().len(), 1);
    assert_eq!(store.load_favorites().unwrap().get("new"), Some(&true));
    assert_eq!(
        fs::read_to_string(store.cache_dir("claude-cli").join("same.json")).unwrap(),
        newer
    );
}
#[test]
fn broken_new_or_old_favorites_abort_without_overwrite_and_can_retry() {
    for broken_new in [false, true] {
        let (root, store) = fixture();
        let path = if broken_new {
            store.root().join("favorites.json")
        } else {
            root.join("legacy/favorites.json")
        };
        write(&path, "{broken");
        assert!(store.ensure_initialized().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
        assert!(!store.root().join(".layout-v1.json").exists());
        write(&path, r#"{"repaired":true}"#);
        store.ensure_initialized().unwrap();
        assert_eq!(store.load_favorites().unwrap().get("repaired"), Some(&true));
    }
}
#[test]
fn unknown_or_broken_marker_never_triggers_reimport() {
    for value in ["{broken", r#"{"version":99}"#] {
        let (root, store) = fixture();
        write(&store.root().join(".layout-v1.json"), value);
        write(&root.join("legacy/favorites.json"), r#"{"old":true}"#);
        assert!(store.ensure_initialized().is_err());
        assert!(!store.root().join("favorites.json").exists());
        assert_eq!(
            fs::read_to_string(store.root().join(".layout-v1.json")).unwrap(),
            value
        );
    }
}
#[test]
fn bad_cache_warns_but_does_not_block_valid_preferences() {
    let (root, store) = fixture();
    write(&root.join("legacy/bad.json"), "{broken");
    let report = store.ensure_initialized().unwrap();
    assert!(!report.warnings.is_empty());
    assert_eq!(store.load_favorites().unwrap().len(), 0);
    assert!(!store.cache_dir("claude-cli").join("bad.json").exists());
    assert_eq!(
        fs::read_to_string(root.join("legacy/bad.json")).unwrap(),
        "{broken"
    );
}
#[cfg(unix)]
#[test]
fn import_does_not_follow_directory_or_file_symlinks() {
    use std::os::unix::fs::symlink;
    let (root, store) = fixture();
    write(
        &root.join("outside/file.json"),
        &cached("claude-cli", "file"),
    );
    fs::create_dir_all(root.join("legacy")).unwrap();
    symlink(
        root.join("outside/file.json"),
        root.join("legacy/file.json"),
    )
    .unwrap();
    symlink(root.join("outside"), root.join("legacy/codex")).unwrap();
    store.ensure_initialized().unwrap();
    assert!(!store.cache_dir("claude-cli").join("file.json").exists());
    assert!(!store.cache_dir("codex").join("file.json").exists());
}
#[test]
fn concurrent_processes_initialize_and_toggle_without_lost_updates() {
    let (root, store) = fixture();
    let mut children = vec![];
    for i in 0..6 {
        children.push(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "favorite_worker", "--nocapture"])
                .env("DASHBOARD_STORAGE_TEST_ROOT", &root)
                .env("DASHBOARD_STORAGE_TEST_ID", format!("session-{i}"))
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    store.ensure_initialized().unwrap();
    assert_eq!(store.load_favorites().unwrap().len(), 6);
}
#[test]
fn favorite_worker() {
    if let Ok(root) = std::env::var("DASHBOARD_STORAGE_TEST_ROOT") {
        let root = PathBuf::from(root);
        let store = SessionStorage::new(root.join("sessions"), root.join("legacy"));
        assert!(store
            .toggle_favorite(&std::env::var("DASHBOARD_STORAGE_TEST_ID").unwrap())
            .unwrap());
    }
}
#[test]
fn broken_favorites_cannot_be_overwritten_by_toggle() {
    let (_, store) = fixture();
    store.ensure_initialized().unwrap();
    write(&store.root().join("favorites.json"), "broken");
    let error = store.toggle_favorite("valid-id").unwrap_err().to_string();
    assert!(error.contains("SESSION_PREFERENCE_READ_FAILED"));
    assert_eq!(
        fs::read_to_string(store.root().join("favorites.json")).unwrap(),
        "broken"
    );
}

#[cfg(unix)]
#[test]
fn failed_atomic_write_preserves_favorites_and_reports_failure() {
    use std::os::unix::fs::PermissionsExt;
    let (_, store) = fixture();
    store.toggle_favorite("kept").unwrap();
    let before = fs::read(store.favorites_file()).unwrap();
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = store.toggle_favorite("new");
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("SESSION_PREFERENCE_WRITE_FAILED"));
    assert_eq!(fs::read(store.favorites_file()).unwrap(), before);
}

#[test]
fn lock_timeout_is_local_and_does_not_overwrite_preferences() {
    let (_, store) = fixture();
    store.ensure_initialized().unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.root().join(".write.lock"))
        .unwrap();
    lock.lock().unwrap();
    let before = fs::read(store.favorites_file()).unwrap();
    let error = store.toggle_favorite("blocked").unwrap_err().to_string();
    assert!(error.contains("SESSION_PREFERENCE_BUSY"));
    assert!(!error.contains("sync:"));
    assert_eq!(fs::read(store.favorites_file()).unwrap(), before);
}
