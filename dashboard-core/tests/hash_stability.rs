// P0-B hash-stability 测试（审计对账 2026-07-06）
// 保证同一 git 状态下 `snapshot_hash()` 输出稳定（排除时间戳），跳过 events + commit + push 的 spec 意图能生效

use chrono::{TimeZone, Utc};
use dashboard_core::sync::{
    Event, DashboardStateRepo, Snapshot, SnapshotProject, Trigger, CURRENT_SNAPSHOT_SCHEMA_VERSION,
};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-hash-{}-{}-{}",
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

fn base_project(name: &str, head: &str) -> SnapshotProject {
    SnapshotProject {
        project_id: Some(Uuid::new_v4()),
        name: name.to_string(),
        branch: Some("main".into()),
        head: Some(head.into()),
        head_full: None,
        upstream: Some("origin/main".into()),
        ahead: Some(0),
        behind: Some(0),
        dirty: 0,
        untracked: 0,
        last_commit_at: None,
        last_fs_change_at: None,
        collected_at: Utc::now(),
        progress: None,
        dangerous_state: None,
        recent_commits: None,
    }
}

#[test]
fn hash_stable_when_only_timestamps_differ() {
    // 相同 git 状态（相同 head / dirty / untracked），仅 updated_at 时间戳变化 —— hash 必须相等
    let mut snap1 = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap(),
        projects: vec![base_project("proj-a", "abc123"), base_project("proj-b", "def456")],
    };
    let h1 = DashboardStateRepo::hash_of(&snap1).unwrap();

    // 各种时间戳都推进
    snap1.updated_at = Utc.with_ymd_and_hms(2026, 7, 6, 23, 59, 59).single().unwrap();
    for p in snap1.projects.iter_mut() {
        p.collected_at = Utc.with_ymd_and_hms(2026, 7, 6, 23, 59, 59).single().unwrap();
        p.last_fs_change_at = Some(Utc.with_ymd_and_hms(2026, 7, 6, 23, 59, 59).single().unwrap());
    }
    let h2 = DashboardStateRepo::hash_of(&snap1).unwrap();

    assert_eq!(h1, h2, "只时间戳变化，semantic hash 必须稳定");
}

#[test]
fn hash_changes_when_head_changes() {
    let snap1 = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("proj-a", "abc123")],
    };
    let mut snap2 = snap1.clone();
    snap2.projects[0].head = Some("xyz789".into());
    let h1 = DashboardStateRepo::hash_of(&snap1).unwrap();
    let h2 = DashboardStateRepo::hash_of(&snap2).unwrap();
    assert_ne!(h1, h2, "head 变化必须改变 hash");
}

#[test]
fn hash_changes_when_dirty_changes() {
    let snap1 = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("proj-a", "abc123")],
    };
    let mut snap2 = snap1.clone();
    snap2.projects[0].dirty = 3;
    let h1 = DashboardStateRepo::hash_of(&snap1).unwrap();
    let h2 = DashboardStateRepo::hash_of(&snap2).unwrap();
    assert_ne!(h1, h2, "dirty 变化必须改变 hash");
}

#[test]
fn write_snapshot_read_hash_roundtrip_stable() {
    // 落盘一次 → read → hash 与 in-memory hash 相等（保证读老 snapshot 的 hash 和新 snapshot 稳定投影一致）
    let tmp = make_tempdir("rt");
    let repo = DashboardStateRepo::new(tmp, "M1");
    let snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap(),
        projects: vec![base_project("proj-a", "abc123")],
    };
    repo.write_snapshot(&snap).unwrap();
    let disk_hash = repo.snapshot_hash().unwrap();
    let memory_hash = DashboardStateRepo::hash_of(&snap).unwrap();
    assert_eq!(disk_hash, memory_hash);
}

#[test]
fn sync_cycle_does_not_write_when_semantic_unchanged() {
    // P0-I（2026-07-06）：sync_cycle 只时间戳变化时**不能**覆盖 snapshot.json，
    // 否则本地 dashboard-state 工作区常态化 dirty，影响 git pull --ff-only 和排障。
    let tmp = make_tempdir("no-write");
    let dashboard_state = tmp.join("dashboard-state");
    fs::create_dir_all(&dashboard_state).unwrap();
    // 提前建成 git 仓（sync_cycle 里要 add/commit —— 但没 remote，push 会失败）
    let _ = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@project-dashboard"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "project-dashboard-test"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "commit.gpgsign", "false"])
        .current_dir(&dashboard_state)
        .output();

    let repo = DashboardStateRepo::new(&dashboard_state, "M1");
    let t0 = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).single().unwrap();
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: t0,
        projects: vec![base_project("proj-a", "abc123")],
    };
    // 第一次同步：落盘 + commit（push 会失败，但落盘 + commit 成功即可）
    let _ = repo.sync_cycle(snap.clone(), vec![], Trigger::Manual, false, true);

    // 第一次后确认 snapshot.json 存在
    let snap_path = repo.snapshot_path();
    assert!(snap_path.exists(), "首次 sync_cycle 应该落盘 snapshot.json");
    let bytes_before = fs::read(&snap_path).unwrap();

    // 只推进时间戳 —— 语义 hash 不变
    snap.updated_at = Utc.with_ymd_and_hms(2026, 7, 6, 23, 59, 59).single().unwrap();
    for p in snap.projects.iter_mut() {
        p.collected_at = Utc.with_ymd_and_hms(2026, 7, 6, 23, 59, 59).single().unwrap();
    }

    let result = repo.sync_cycle(snap.clone(), vec![], Trigger::Tick, false, true).unwrap();
    assert!(!result.changed, "语义未变时 sync_cycle 应返 changed=false");
    let bytes_after = fs::read(&snap_path).unwrap();
    assert_eq!(
        bytes_before, bytes_after,
        "语义未变时 snapshot.json 文件字节必须完全不变 —— 否则会污染本地工作区"
    );
}

#[test]
fn sync_cycle_writes_when_head_changes() {
    // 反向验证：head 变化必须落盘 —— 排除"永远不写"的过修正
    let tmp = make_tempdir("write-head");
    let dashboard_state = tmp.join("dashboard-state");
    fs::create_dir_all(&dashboard_state).unwrap();
    let _ = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@project-dashboard"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "project-dashboard-test"])
        .current_dir(&dashboard_state)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "commit.gpgsign", "false"])
        .current_dir(&dashboard_state)
        .output();

    let repo = DashboardStateRepo::new(&dashboard_state, "M1");
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("proj-a", "abc123")],
    };
    let _ = repo.sync_cycle(snap.clone(), vec![], Trigger::Manual, false, true);
    let bytes_before = fs::read(repo.snapshot_path()).unwrap();

    // 改 head → 语义 hash 变
    snap.projects[0].head = Some("xyz789".into());
    let event = Event {
        ts: Utc::now(),
        trigger: Trigger::Hook,
        project_id: snap.projects[0].project_id,
        action: dashboard_core::sync::Action::Collect,
        from_head: Some("abc123".into()),
        to_head: Some("xyz789".into()),
        dirty: 0,
        untracked: 0,
        dangerous_state: None,
        manual: None,
        git_remote_mismatch: None,
        git_remote_failed: None,
    };
    // push 会失败（无 remote），但 write_snapshot + append_event 是可观察的
    let _ = repo.sync_cycle(snap.clone(), vec![event], Trigger::Hook, false, true);
    let bytes_after = fs::read(repo.snapshot_path()).unwrap();
    assert_ne!(bytes_before, bytes_after, "head 变化必须落盘新 snapshot");
}

// ============================================================================
// v4.3 §1.3 R2-P0-A：稳定 hash 投影包含 head_full + recent_commits.sha_full
// ============================================================================

use dashboard_core::tracker::CommitEntry;

fn v43_project_with_head_full(name: &str, head: &str, head_full: Option<&str>) -> SnapshotProject {
    let mut p = base_project(name, head);
    p.head_full = head_full.map(String::from);
    p
}

fn v43_commit(sha: &str, subject: &str) -> CommitEntry {
    CommitEntry {
        sha_full: sha.to_string(),
        subject: subject.to_string(),
        author: "example".into(),
        time: Utc.with_ymd_and_hms(2026, 7, 11, 10, 0, 0).single().unwrap(),
        files: None,
    }
}

/// R2-P0-A 核心：首次升级到 v4.3（head_full 从 None → Some）→ 稳定 hash 必变
///
/// 若不变，spec §"snapshot 未变时跳过 events + commit + push" 会永远压制新字段落盘。
#[test]
fn v43_hash_changes_when_head_full_added_first_time() {
    let mut snap_v42 = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc.with_ymd_and_hms(2026, 7, 11, 10, 0, 0).single().unwrap(),
        projects: vec![v43_project_with_head_full("proj-a", "abc1234", None)],
    };
    let h1 = DashboardStateRepo::hash_of(&snap_v42).unwrap();

    // 首次升级：head_full 从 None → Some（同一逻辑 HEAD 的完整 sha）
    snap_v42.projects[0].head_full = Some("abc1234567890abcdef1234567890abcdef123456".into());
    let h2 = DashboardStateRepo::hash_of(&snap_v42).unwrap();

    assert_ne!(h1, h2, "head_full 从 None → Some 稳定 hash 必须变（否则首次升级永远写不出）");
}

/// R2-P0-A：head_full 严格值变化 → hash 变
#[test]
fn v43_hash_changes_when_head_full_value_changes() {
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![v43_project_with_head_full("p", "abc", Some(&"a".repeat(40)))],
    };
    let h1 = DashboardStateRepo::hash_of(&snap).unwrap();
    snap.projects[0].head_full = Some("b".repeat(40));
    let h2 = DashboardStateRepo::hash_of(&snap).unwrap();
    assert_ne!(h1, h2);
}

/// R2-P0-A：recent_commits None → Some(vec![]) 稳定 hash 必变（三态语义边界）
#[test]
fn v43_hash_changes_when_recent_commits_none_to_empty() {
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("p", "abc")],
    };
    let h1 = DashboardStateRepo::hash_of(&snap).unwrap();
    snap.projects[0].recent_commits = Some(vec![]);
    let h2 = DashboardStateRepo::hash_of(&snap).unwrap();
    assert_ne!(
        h1, h2,
        "None（不可用）→ Some(vec![])（有效空仓）是两态跃迁，稳定 hash 必须能捕获"
    );
}

/// R2-P0-A：recent_commits.sha_full 序列变化 → hash 变
#[test]
fn v43_hash_changes_when_recent_commits_sha_sequence_changes() {
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("p", "abc")],
    };
    snap.projects[0].recent_commits = Some(vec![
        v43_commit(&"a".repeat(40), "first"),
        v43_commit(&"b".repeat(40), "second"),
    ]);
    let h1 = DashboardStateRepo::hash_of(&snap).unwrap();

    // 加一条新 commit（sha 序列变了）
    snap.projects[0].recent_commits = Some(vec![
        v43_commit(&"c".repeat(40), "third"),
        v43_commit(&"a".repeat(40), "first"),
        v43_commit(&"b".repeat(40), "second"),
    ]);
    let h2 = DashboardStateRepo::hash_of(&snap).unwrap();
    assert_ne!(h1, h2, "sha 序列变化必须触发 hash 变（新 commit 落盘）");
}

/// R2-P0-A 契约：recent_commits 里只 subject 变化（sha 一样）→ hash 不变
///
/// 避免 amend commit 只改 subject / author.time 变动触发不必要的 push（spec 明说
/// snapshot 未变时跳过）。sha_full 是同一提交的稳定 id，subject/author/time 只是元数据。
#[test]
fn v43_hash_stable_when_only_subject_changes_but_sha_same() {
    let mut snap = Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M1".into(),
        hostname: String::new(),
        updated_at: Utc::now(),
        projects: vec![base_project("p", "abc")],
    };
    snap.projects[0].recent_commits = Some(vec![v43_commit(&"a".repeat(40), "orig")]);
    let h1 = DashboardStateRepo::hash_of(&snap).unwrap();

    // subject 变，sha 不变
    snap.projects[0].recent_commits = Some(vec![v43_commit(&"a".repeat(40), "amended msg")]);
    let h2 = DashboardStateRepo::hash_of(&snap).unwrap();
    assert_eq!(
        h1, h2,
        "subject 变但 sha 不变时稳定 hash 必须不变（防 amend 触发 push 噪音）"
    );
}
