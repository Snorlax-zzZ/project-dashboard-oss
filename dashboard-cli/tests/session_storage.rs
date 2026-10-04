// Windows 的 dirs::home_dir 读取 Known Folder，不接受 HOME 覆盖，不能这样隔离用户数据。
#![cfg(unix)]

use std::{fs, process::Command};
#[test]
fn cli_initializes_shared_storage_but_keeps_hidden_sessions_visible() {
    let home = std::env::temp_dir().join(format!("dashboard-cli-storage-{}", uuid::Uuid::new_v4()));
    let source = home.join(".claude/projects/example");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("same.jsonl"),
        "{\"type\":\"ai-title\",\"aiTitle\":\"synthetic session\"}\n",
    )
    .unwrap();
    let storage = home.join(".project-dashboard/sessions");
    fs::create_dir_all(&storage).unwrap();
    fs::write(
        storage.join("hidden.json"),
        r#"{"version":1,"hidden":{"claude-cli":["same"]}}"#,
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_fleet"))
        .arg("--json")
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let items: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert!(storage.join("favorites.json").is_file());
    assert!(storage.join(".layout-v1.json").is_file());
    assert!(storage.join("cache/claude-cli/same.json").is_file());
    fs::remove_dir_all(home).unwrap();
}
