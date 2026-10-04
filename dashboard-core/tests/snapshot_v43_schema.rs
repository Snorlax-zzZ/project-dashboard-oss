// v4.3 §1.1 schema 层集成测试 —— roundtrip + 三态语义 + v4.2 向后兼容。
//
// 覆盖 tasks §1.1.5（snapshot_roundtrip_v42_old_shape）+ §1.1.6（三态 roundtrip）。
// 采集侧 head_full / recent_commits 的真采集逻辑在 §1.2/§1.3 实现，本文件只测数据模型层。

use chrono::{TimeZone, Utc};
use dashboard_core::schema::{check_sensitive_fields, validate_snapshot};
use dashboard_core::sync::{Snapshot, SnapshotProject, CURRENT_SNAPSHOT_SCHEMA_VERSION};
use dashboard_core::tracker::CommitEntry;
use serde_json::json;
use uuid::Uuid;

fn v43_base_snapshot() -> Snapshot {
    Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: "M-test".into(),
        hostname: "host-test".into(),
        updated_at: Utc.with_ymd_and_hms(2026, 7, 11, 15, 0, 0).single().unwrap(),
        projects: vec![],
    }
}

fn v43_base_project(name: &str) -> SnapshotProject {
    SnapshotProject {
        project_id: Some(Uuid::new_v4()),
        name: name.to_string(),
        branch: Some("main".into()),
        head: Some("abc1234".into()),
        head_full: None,
        upstream: Some("origin/main".into()),
        ahead: Some(0),
        behind: Some(0),
        dirty: 0,
        untracked: 0,
        last_commit_at: None,
        last_fs_change_at: None,
        collected_at: Utc.with_ymd_and_hms(2026, 7, 11, 15, 0, 0).single().unwrap(),
        progress: None,
        dangerous_state: None,
        recent_commits: None,
    }
}

/// §1.1.5：v4.2 老 shape JSON 反序列化后新字段全 None，不 panic。
#[test]
fn snapshot_roundtrip_v42_old_shape() {
    // 严格模拟 v4.2 序列化产物：不含 head_full / recent_commits 两字段
    let v42_json = json!({
        "schema_version": 1,
        "machine_id": "M-legacy",
        "updated_at": "2026-06-01T00:00:00Z",
        "projects": [
            {
                "project_id": "8a8f0a52-3a6c-4a55-9f2f-1234567890ab",
                "name": "legacy-proj",
                "branch": "main",
                "head": "abc1234",
                "upstream": "origin/main",
                "ahead": 0,
                "behind": 0,
                "dirty": 0,
                "untracked": 0,
                "last_commit_at": null,
                "last_fs_change_at": null,
                "collected_at": "2026-06-01T00:00:00Z",
                "progress": null,
                "dangerous_state": null
            }
        ]
    });

    let snap: Snapshot = serde_json::from_value(v42_json).expect("v4.2 老 shape 必须能被新版本反序列化");
    assert_eq!(snap.projects.len(), 1);
    let p = &snap.projects[0];
    assert!(p.head_full.is_none(), "v4.2 老 snapshot 新字段 head_full 必须为 None");
    assert!(p.recent_commits.is_none(), "v4.2 老 snapshot 新字段 recent_commits 必须为 None（不可用信号）");
}

/// §1.1.6-a：recent_commits = None 三态之一（老 snapshot / 采集失败）
#[test]
fn snapshot_roundtrip_recent_commits_state_none() {
    let mut snap = v43_base_snapshot();
    let mut proj = v43_base_project("proj-none");
    proj.recent_commits = None;
    snap.projects.push(proj);

    let ser = serde_json::to_value(&snap).unwrap();
    let de: Snapshot = serde_json::from_value(ser).unwrap();
    assert!(de.projects[0].recent_commits.is_none());
}

/// §1.1.6-b：recent_commits = Some(vec![]) 三态之二（有效空仓 unborn repo）
#[test]
fn snapshot_roundtrip_recent_commits_state_empty_vec() {
    let mut snap = v43_base_snapshot();
    let mut proj = v43_base_project("proj-unborn");
    proj.recent_commits = Some(vec![]);
    snap.projects.push(proj);

    let ser = serde_json::to_value(&snap).unwrap();
    let de: Snapshot = serde_json::from_value(ser).unwrap();
    // 关键契约：Some(vec![]) 不能被降级成 None，也不能被降级成非空
    assert_eq!(de.projects[0].recent_commits.as_ref().map(|v| v.len()), Some(0));
}

/// §1.1.6-c：recent_commits = Some(非空) 三态之三（有效证据）
#[test]
fn snapshot_roundtrip_recent_commits_state_populated() {
    let mut snap = v43_base_snapshot();
    let mut proj = v43_base_project("proj-populated");
    proj.head_full = Some("a".repeat(40));
    proj.recent_commits = Some(vec![
        CommitEntry {
            sha_full: "a".repeat(40),
            subject: "first commit".into(),
            author: "example".into(),
            time: Utc.with_ymd_and_hms(2026, 7, 11, 10, 0, 0).single().unwrap(),
            files: None,
        },
        CommitEntry {
            sha_full: "b".repeat(40),
            subject: "second commit".into(),
            author: "example".into(),
            time: Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).single().unwrap(),
            files: None,
        },
    ]);
    snap.projects.push(proj);

    let ser = serde_json::to_value(&snap).unwrap();
    let de: Snapshot = serde_json::from_value(ser).unwrap();
    let commits = de.projects[0].recent_commits.as_ref().expect("有效证据不可能是 None");
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].sha_full.len(), 40);
    assert_eq!(commits[0].subject, "first commit");
}

/// §1.1.6-d：CommitEntry 字段 required —— 缺 sha_full 必须反序列化失败（`deny_unknown_fields` 保守契约）
#[test]
fn commit_entry_rejects_missing_required_field() {
    let bad = json!({
        "subject": "no sha",
        "author": "example",
        "time": "2026-07-11T00:00:00Z"
    });
    let result: Result<CommitEntry, _> = serde_json::from_value(bad);
    assert!(result.is_err(), "缺 sha_full 必须失败");
}

/// §1.1.6-e：CommitEntry 拒收未知字段（防未来演进 schema drift）
#[test]
fn commit_entry_rejects_unknown_field() {
    let bad = json!({
        "sha_full": "a".repeat(40),
        "subject": "ok",
        "author": "example",
        "time": "2026-07-11T00:00:00Z",
        "future_field": "should be rejected"
    });
    let result: Result<CommitEntry, _> = serde_json::from_value(bad);
    assert!(result.is_err(), "未知字段必须被 deny_unknown_fields 拒收");
}

/// §1.1.4：JSON Schema 校验 v4.3 snapshot（含新字段）不 panic。
///
/// 校验器契约：v4.2 老 shape 也必须过（不 required 新字段），v4.3 新 shape 也必须过。
#[test]
fn snapshot_json_schema_accepts_v42_and_v43_shapes() {
    // v4.2 老 shape
    let v42_json = json!({
        "schema_version": 1,
        "machine_id": "M-legacy",
        "updated_at": "2026-06-01T00:00:00Z",
        "projects": [{
            "project_id": "8a8f0a52-3a6c-4a55-9f2f-1234567890ab",
            "name": "legacy-proj",
            "branch": "main",
            "head": "abc1234",
            "upstream": "origin/main",
            "ahead": 0,
            "behind": 0,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-06-01T00:00:00Z",
            "progress": null,
            "dangerous_state": null
        }]
    });
    assert!(validate_snapshot(&v42_json).is_ok(), "v4.2 老 shape 必须通过校验");

    // v4.3 新 shape 含 head_full + recent_commits Some(vec)
    let v43_json = json!({
        "schema_version": 1,
        "machine_id": "M-new",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": "8a8f0a52-3a6c-4a55-9f2f-1234567890ab",
            "name": "new-proj",
            "branch": "main",
            "head": "abc1234",
            "head_full": "abc1234abc1234abc1234abc1234abc1234abc12",
            "upstream": "origin/main",
            "ahead": 0,
            "behind": 0,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": [
                {
                    "sha_full": "abc1234abc1234abc1234abc1234abc1234abc12",
                    "subject": "first",
                    "author": "example",
                    "time": "2026-07-11T10:00:00Z"
                }
            ]
        }]
    });
    assert!(validate_snapshot(&v43_json).is_ok(), "v4.3 新 shape 必须通过校验");
}

/// §1.1.4：JSON Schema 拒收 head_full 长度不为 40 的坏值
#[test]
fn snapshot_json_schema_rejects_bad_head_full_length() {
    let bad = json!({
        "schema_version": 1,
        "machine_id": "M-bad",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null,
            "name": "p",
            "branch": null,
            "head": "abc",
            "head_full": "abc",
            "upstream": null,
            "ahead": null,
            "behind": null,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": null
        }]
    });
    assert!(validate_snapshot(&bad).is_err(), "head_full 长度非 40 必须被拒");
}

/// §1.1.4：JSON Schema 拒收 recent_commits 超过 20 条
#[test]
fn snapshot_json_schema_rejects_recent_commits_over_20() {
    let mut commits = Vec::new();
    for i in 0..21u32 {
        commits.push(json!({
            "sha_full": format!("{:040x}", i),
            "subject": format!("c{i}"),
            "author": "example",
            "time": "2026-07-11T10:00:00Z"
        }));
    }
    let bad = json!({
        "schema_version": 1,
        "machine_id": "M-over",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null,
            "name": "p",
            "branch": null,
            "head": null,
            "head_full": null,
            "upstream": null,
            "ahead": null,
            "behind": null,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": commits
        }]
    });
    assert!(validate_snapshot(&bad).is_err(), "recent_commits > 20 条必须被拒");
}

/// P0-4：commit subject/author 是用户 git 自由文本，含 diff/hunk/prompt 等
/// git 语义词属正常，绝不能因此拒收整份 snapshot 导致同步挂死（用户部署即撞的实际阻塞）。
#[test]
fn snapshot_commit_freetext_with_git_keywords_passes() {
    let snap = json!({
        "schema_version": 1,
        "machine_id": "M-freetext",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null,
            "name": "p",
            "branch": "main",
            "head": "abc1234",
            "head_full": "abc1234abc1234abc1234abc1234abc1234abc12",
            "upstream": null,
            "ahead": null,
            "behind": null,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": [{
                "sha_full": "abc1234abc1234abc1234abc1234abc1234abc12",
                "subject": "fix diff rendering, squash hunk, tweak prompt",
                "author": "diff-hunk-dev",
                "time": "2026-07-11T10:00:00Z"
            }]
        }]
    });
    assert!(
        validate_snapshot(&snap).is_ok(),
        "commit subject/author 含 diff/hunk/prompt 是正常 git 文本，必须通过校验"
    );
}

/// P0-4 反向兜底：即便对 subject/author 豁免关键词黑名单，SECRET_PREFIXES 高熵密钥前缀
/// 仍须拦截——有人把 token 贴进 commit message 时能兜住。
#[test]
fn snapshot_commit_subject_with_secret_prefix_still_rejected() {
    let snap = json!({
        "schema_version": 1,
        "machine_id": "M-secret",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null,
            "name": "p",
            "branch": "main",
            "head": null,
            "head_full": null,
            "upstream": null,
            "ahead": null,
            "behind": null,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": [{
                "sha_full": "abc1234abc1234abc1234abc1234abc1234abc12",
                "subject": "oops leaked sk-ant-api03-abcdefghijklmnop token",
                "author": "example",
                "time": "2026-07-11T10:00:00Z"
            }]
        }]
    });
    assert!(
        validate_snapshot(&snap).is_err(),
        "commit message 里的真实密钥前缀必须仍被 SECRET_PREFIXES 兜底拦截"
    );
}

/// P0-4 豁免边界：关键词豁免只作用于 recent_commits[*].subject/author，不得泄漏到其他字段
/// （如 project name 含 diff 仍须拦截，防止 allowlist 意外放宽）。
#[test]
fn snapshot_non_commit_field_keyword_still_rejected() {
    let snap = json!({
        "schema_version": 1,
        "machine_id": "M-name",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null,
            "name": "diff-viewer",
            "branch": "main",
            "head": null,
            "head_full": null,
            "upstream": null,
            "ahead": null,
            "behind": null,
            "dirty": 0,
            "untracked": 0,
            "last_commit_at": null,
            "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z",
            "progress": null,
            "dangerous_state": null,
            "recent_commits": null
        }]
    });
    assert!(
        validate_snapshot(&snap).is_err(),
        "project name 含 diff 属非 commit 自由文本字段，值层扫描必须仍拦截"
    );
}

/// P0-4-b：伪造的 machine key（如 `xrecent_commits[0].subject`——registry
/// machines 允许任意动态 key）不得被豁免。精确锚定只认 `projects[N].recent_commits[M].
/// subject|author` 整条结构，其余一律走值层关键词扫描。
#[test]
fn fake_recent_commits_path_not_exempted_from_keyword_scan() {
    // 模拟攻击者在 registry machines map 里构造的 key，其 value 含 diff 值层关键词
    let forged = json!({
        "machines": { "xrecent_commits[0].subject": "path with diff inside" }
    });
    assert!(
        check_sensitive_fields(&forged).is_err(),
        "伪造的 xrecent_commits machine key 不得豁免，diff 值层扫描必须拦"
    );
    // 对照：真 recent_commits 结构才豁免关键词扫描
    let real = json!({
        "projects": [{ "recent_commits": [{ "subject": "fix diff bug" }] }]
    });
    assert!(
        check_sensitive_fields(&real).is_ok(),
        "真 projects[N].recent_commits[M].subject 含 diff 应豁免通过"
    );
}

/// P0-3-b：Snapshot deny_unknown_fields —— 未来版本写入的顶层未知字段必须
/// 反序列化失败（fail-closed），而非被静默忽略、再由 collect_single 重写时丢弃别机新数据。
#[test]
fn snapshot_rejects_unknown_top_level_field() {
    let with_extra = json!({
        "schema_version": 1,
        "machine_id": "M",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [],
        "future_field": "should be rejected"
    });
    let r: Result<dashboard_core::sync::Snapshot, _> = serde_json::from_value(with_extra);
    assert!(r.is_err(), "deny_unknown_fields：顶层未知字段必须反序列化失败");
}

/// P0-3-b：SnapshotProject deny_unknown_fields —— project 层未知字段同样 fail-closed。
#[test]
fn snapshot_project_rejects_unknown_field() {
    let with_extra = json!({
        "schema_version": 1,
        "machine_id": "M",
        "updated_at": "2026-07-11T15:00:00Z",
        "projects": [{
            "project_id": null, "name": "p", "branch": null, "head": null,
            "upstream": null, "ahead": null, "behind": null, "dirty": 0, "untracked": 0,
            "last_commit_at": null, "last_fs_change_at": null,
            "collected_at": "2026-07-11T15:00:00Z", "progress": null, "dangerous_state": null,
            "future_project_field": "x"
        }]
    });
    let r: Result<dashboard_core::sync::Snapshot, _> = serde_json::from_value(with_extra);
    assert!(r.is_err(), "deny_unknown_fields：project 层未知字段必须反序列化失败");
}

/// hostname 字段与 `additionalProperties:false` 的兼容性。
///
/// snapshot schema 是 Allowlist（`additionalProperties:false`）且每次写盘都跑 validator，
/// 所以给 Rust struct 加字段而忘了同步 schema，会让**所有同步静默写失败**。
/// 本测试同时钉住两侧：新 snapshot 带 hostname 要过、老 snapshot 无 hostname 也要过。
#[test]
fn snapshot_hostname_field_passes_allowlist_both_with_and_without() {
    let with_hostname = json!({
        "schema_version": 1,
        "machine_id": "mbp-00000001",
        "hostname": "demo-MacBook-Pro-4",
        "updated_at": "2026-07-25T10:00:00Z",
        "projects": []
    });
    validate_snapshot(&with_hostname).expect("带 hostname 的新 snapshot 必须过 schema");

    // 老 snapshot 无 hostname —— hostname 不在 required，必须照样过（否则存量数据全废）。
    let without_hostname = json!({
        "schema_version": 1,
        "machine_id": "demo-MacBook-Pro-3",
        "updated_at": "2026-07-23T06:28:17.547178Z",
        "projects": []
    });
    validate_snapshot(&without_hostname).expect("无 hostname 的老 snapshot 必须过 schema");

    // 反序列化侧：老 JSON 读进来 hostname 应为空串而非 panic。
    let parsed: Snapshot = serde_json::from_value(without_hostname).unwrap();
    assert_eq!(parsed.hostname, "");

    // 真正的未知字段仍须被拒 —— 别把 Allowlist 放宽成什么都收。
    let bogus = json!({
        "schema_version": 1,
        "machine_id": "mbp-00000001",
        "totally_unknown_field": "x",
        "updated_at": "2026-07-25T10:00:00Z",
        "projects": []
    });
    assert!(
        validate_snapshot(&bogus).is_err(),
        "未知字段必须仍被 Allowlist 拒绝"
    );
}
