use chrono::{Duration, Utc};
use dashboard_core::sync_status::{SyncOutcome, SyncStatus, SyncStatusStore};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("dashboard-sync-status-{}", uuid::Uuid::new_v4())))
    }
    fn root(&self) -> PathBuf {
        self.0.join("state")
    }
    fn store(&self) -> SyncStatusStore {
        SyncStatusStore::new(&self.root(), "machine", Some("remote")).unwrap()
    }
    fn file(&self) -> PathBuf {
        self.0.join("state.sync-status.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_history_reads_without_creating_any_file() {
    let f = Fixture::new();
    assert_eq!(f.store().read().unwrap(), SyncStatus::default());
    assert!(!f.0.exists());
}

#[test]
fn new_reader_recovers_success_and_later_failure_without_losing_success() {
    let f = Fixture::new();
    let first = Utc::now();
    f.store().record(SyncOutcome::Succeeded, first).unwrap();
    assert_eq!(f.store().read().unwrap().last_success_at, Some(first));
    f.store()
        .record(SyncOutcome::PushFailed, first + Duration::seconds(1))
        .unwrap();
    let status = f.store().read().unwrap();
    assert_eq!(status.last_success_at, Some(first));
    assert_eq!(
        status.last_attempt.unwrap().outcome,
        SyncOutcome::PushFailed
    );
    assert!(!f.root().exists(), "记录必须在数据仓之外");
}

#[test]
fn wrong_machine_remote_or_root_never_reuses_history() {
    let f = Fixture::new();
    f.store()
        .record(SyncOutcome::Succeeded, Utc::now())
        .unwrap();
    for (root, machine, remote) in [
        (f.root(), "other", Some("remote")),
        (f.root(), "machine", Some("other")),
        (f.root(), "machine", None),
        (f.0.join("other"), "machine", Some("remote")),
    ] {
        assert_eq!(
            SyncStatusStore::new(&root, machine, remote)
                .unwrap()
                .read()
                .unwrap(),
            SyncStatus::default()
        );
    }
}

#[test]
fn delayed_success_cannot_replace_a_newer_failure() {
    let f = Fixture::new();
    let at = Utc::now();
    f.store()
        .record(SyncOutcome::Failed, at + Duration::seconds(2))
        .unwrap();
    f.store().record(SyncOutcome::Succeeded, at).unwrap();
    let status = f.store().read().unwrap();
    assert_eq!(status.last_success_at, Some(at));
    assert_eq!(status.last_attempt.unwrap().outcome, SyncOutcome::Failed);
}

#[test]
fn concurrent_writers_preserve_latest_attempt_and_success() {
    let f = Fixture::new();
    let at = Utc::now();
    let workers: Vec<_> = (0..12)
        .map(|i| {
            let store = f.store();
            std::thread::spawn(move || {
                store
                    .record(
                        if i % 2 == 0 {
                            SyncOutcome::Succeeded
                        } else {
                            SyncOutcome::Failed
                        },
                        at + Duration::seconds(i),
                    )
                    .unwrap()
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let status = f.store().read().unwrap();
    assert_eq!(status.last_success_at, Some(at + Duration::seconds(10)));
    assert_eq!(
        status.last_attempt.unwrap().completed_at,
        at + Duration::seconds(11)
    );
}

#[test]
fn corrupt_and_future_version_records_fail_without_overwrite() {
    let f = Fixture::new();
    f.store()
        .record(SyncOutcome::Succeeded, Utc::now())
        .unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.file()).unwrap()).unwrap();
    let mut future = saved;
    future["schema_version"] = 99.into();
    for bytes in [b"{broken".to_vec(), serde_json::to_vec(&future).unwrap()] {
        std::fs::write(f.file(), &bytes).unwrap();
        assert!(f.store().read().is_err());
        assert!(f
            .store()
            .record(SyncOutcome::Succeeded, Utc::now())
            .is_err());
        assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
    }
}
