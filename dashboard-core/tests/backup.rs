// §13 灾恢集成测试 —— 备份 / 4 周滚动清理 / 重建

use chrono::{TimeZone, Utc};
use dashboard_core::backup::{latest_backup, prune_old_backups, rebuild_from_local_backup, snapshot_backup};
use std::fs;
use std::path::PathBuf;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-backup-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn seed_dashboard_state(root: &std::path::Path) {
    let projects_dir = root.join("registry").join("projects");
    fs::create_dir_all(&projects_dir).unwrap();
    fs::write(projects_dir.join("proj-a.yaml"), "schema_version: 1\n").unwrap();
    let m1 = root.join("machines").join("M1");
    fs::create_dir_all(&m1).unwrap();
    fs::write(m1.join("snapshot.json"), r#"{"machine_id":"M1"}"#).unwrap();
    fs::write(m1.join("events.ndjson"), "").unwrap();
}

#[test]
fn snapshot_backup_creates_day_dir_and_copies_files() {
    let root = make_tempdir("backup-basic");
    let dashboard_state = root.join("dashboard-state");
    let backups = root.join("backups");
    seed_dashboard_state(&dashboard_state);
    let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap();
    let dst = snapshot_backup(&dashboard_state, &backups, now).unwrap();
    assert!(dst.ends_with("2026-07-06"));
    assert!(dst.join("registry").join("projects").join("proj-a.yaml").is_file());
    assert!(dst.join("machines").join("M1").join("snapshot.json").is_file());
}

#[test]
fn prune_removes_backups_older_than_28_days() {
    let root = make_tempdir("prune");
    let backups = root.join("backups");
    fs::create_dir_all(&backups).unwrap();
    // 30 天前的备份 → 应被清
    fs::create_dir_all(backups.join("2026-06-01")).unwrap();
    fs::write(backups.join("2026-06-01").join("_"), "x").unwrap();
    // 3 天前 → 保留
    fs::create_dir_all(backups.join("2026-07-03")).unwrap();
    // 今天 → 保留
    fs::create_dir_all(backups.join("2026-07-06")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap();
    let removed = prune_old_backups(&backups, now).unwrap();
    assert_eq!(removed, 1);
    assert!(!backups.join("2026-06-01").exists());
    assert!(backups.join("2026-07-03").exists());
    assert!(backups.join("2026-07-06").exists());
}

#[test]
fn latest_backup_picks_most_recent_by_date() {
    let root = make_tempdir("latest");
    let backups = root.join("backups");
    fs::create_dir_all(&backups).unwrap();
    for day in ["2026-06-20", "2026-07-01", "2026-07-05", "not-a-date"] {
        fs::create_dir_all(backups.join(day)).unwrap();
    }
    let latest = latest_backup(&backups).unwrap().unwrap();
    assert!(latest.ends_with("2026-07-05"));
}

#[test]
fn rebuild_copies_latest_backup_to_target() {
    let root = make_tempdir("rebuild");
    let backups = root.join("backups");
    seed_dashboard_state(&backups.join("2026-07-01"));
    let target = root.join("new-dashboard-state");
    let result = rebuild_from_local_backup(&backups, &target).unwrap();
    assert_eq!(result, target);
    assert!(target.join("registry").join("projects").join("proj-a.yaml").is_file());
    assert!(target.join("machines").join("M1").join("snapshot.json").is_file());
}

#[test]
fn rebuild_errors_when_no_backup_present() {
    let root = make_tempdir("rebuild-none");
    let backups = root.join("backups");
    fs::create_dir_all(&backups).unwrap();
    let target = root.join("new-fs");
    let err = rebuild_from_local_backup(&backups, &target).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn snapshot_backup_incremental_days_coexist() {
    let root = make_tempdir("multiple-days");
    let dashboard_state = root.join("dashboard-state");
    let backups = root.join("backups");
    seed_dashboard_state(&dashboard_state);
    for (day, _) in [("2026-07-04", "a"), ("2026-07-05", "b"), ("2026-07-06", "c")] {
        let now = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        snapshot_backup(&dashboard_state, &backups, now).unwrap();
    }
    for day in ["2026-07-04", "2026-07-05", "2026-07-06"] {
        assert!(backups.join(day).is_dir(), "{day} should exist");
    }
    let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap();
    let removed = prune_old_backups(&backups, now).unwrap();
    assert_eq!(removed, 0);
}

// ===== Q2-C 数据目录可配：backups 跟随 state_root（覆盖 AC-8）=====

#[test]
fn backups_root_follows_state_root() {
    // 数据目录挪到自定义位置 → backups 跟着走（与 state_root 同父的兄弟目录，
    // 不放 state_root 内部以免被 dashboard-state 仓 git 跟踪）
    let got = dashboard_core::backup::default_backups_root(std::path::Path::new(
        "/data/pd/dashboard-state-clone",
    ));
    assert_eq!(got, PathBuf::from("/data/pd/backups"));
}

#[test]
fn backups_root_default_matches_legacy() {
    // 默认数据目录下 backups 根仍是 ~/.project-dashboard/backups（向后兼容，路径不变）
    let default_root = dashboard_core::config::default_dashboard_state_root_fallback();
    let got = dashboard_core::backup::default_backups_root(&default_root);
    assert!(
        got.ends_with(std::path::Path::new(".project-dashboard").join("backups")),
        "默认 backups 应保持 ~/.project-dashboard/backups，实际 {got:?}"
    );
}
