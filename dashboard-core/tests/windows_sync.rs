#![cfg(all(windows, feature = "process-fixture"))]
//! 经真实 120 秒同步预算验证超时、状态记录和锁释放；进程环境只影响本测试二进制。
use dashboard_core::{
    commands,
    config::LocalConfig,
    sync::{DashboardStateRepo, Trigger},
    sync_status::{SyncOutcome, SyncStatusStore},
};
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
fn real_sync_timeout_preserves_last_success_and_releases_repo_lock() {
    let base = std::env::temp_dir().join(format!("pd-win-sync-{}", uuid::Uuid::new_v4()));
    let root = base.join("state");
    let bin = base.join("bin");
    fs::create_dir_all(root.join("registry/projects")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_process-fixture"), bin.join("git.exe")).unwrap();
    let remote = "https://example.invalid/synthetic-state.git";
    let local_path = base.join("local.yaml");
    let mut local = LocalConfig::new("win-test");
    local.dashboard_state_remote = Some(remote.into());
    fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();
    let store = SyncStatusStore::new(&root, "win-test", Some(remote)).unwrap();
    let successful_at = chrono::Utc::now() - chrono::Duration::hours(1);
    store.record(SyncOutcome::Succeeded, successful_at).unwrap();
    std::env::set_var("PATH", &bin);
    std::env::set_var("DASHBOARD_FAKE_GIT", "1");
    std::env::set_var("FAKE_GIT_ORIGIN", remote);
    let start = Instant::now();
    let error =
        commands::manual_sync(&root, &local_path, "win-test", Trigger::Manual, true).unwrap_err();
    assert!(error.to_string().contains("TIMEOUT"), "{error}");
    assert!(start.elapsed() >= Duration::from_secs(119));
    assert!(
        start.elapsed() < Duration::from_secs(135),
        "耗时 {:?}",
        start.elapsed()
    );
    let history = store.read().unwrap();
    assert_eq!(history.last_success_at, Some(successful_at));
    assert_eq!(history.last_attempt.unwrap().outcome, SyncOutcome::Failed);
    let repo = DashboardStateRepo::new(&root, "win-test");
    let lock = repo
        .acquire_lock_allow_uninit(Duration::from_millis(100))
        .unwrap();
    drop(lock);
    assert!(!root.join("machines/win-test/snapshot.json").exists());
}
