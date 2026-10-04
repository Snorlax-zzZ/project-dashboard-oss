// 旧身份认领（主机名身份 → 硬件身份）。
//
// 承重不变量：
// 1. 合并**绝不**按整份 snapshot 的 updated_at 取胜者 —— 改名后新开的空壳快照时间戳更新但
//    projects 为空，那样合并会把真实历史整份抹掉（真机数据就是这个形状，见 P1 测试）。
// 2. 干跑不写盘；真改前先整仓备份；旧目录一个都不删。
// 3. 认领判定保守：只认本机自己写下的证据，别的机器目录一律不动。

use dashboard_core::config::{LocalConfig, Registry, RegistryProject};
use dashboard_core::identity_migration::{self, ClaimReason};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const TARGET: &str = "mbp-00000001";
const LEGACY_OLD: &str = "demo-MacBook-Pro-3";
const LEGACY_NEW: &str = "demo-MacBook-Pro-4";
const OTHER_MACHINE: &str = "DESKTOP-DEMO01";

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-identity-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// 写一份 snapshot.json。`projects` 传 (name, collected_at) 列表。
fn write_snapshot(
    machines_dir: &Path,
    machine_id: &str,
    updated_at: &str,
    projects: &[(Uuid, &str, &str)],
) {
    let dir = machines_dir.join(machine_id);
    std::fs::create_dir_all(&dir).unwrap();
    let items: Vec<String> = projects
        .iter()
        .map(|(id, name, collected_at)| {
            format!(
                r#"{{"project_id":"{id}","name":"{name}","branch":"main","head":"abc1234",
                "upstream":null,"ahead":0,"behind":0,"dirty":0,"untracked":0,
                "last_commit_at":null,"last_fs_change_at":null,"collected_at":"{collected_at}",
                "progress":null,"dangerous_state":null}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"schema_version":1,"machine_id":"{machine_id}","updated_at":"{updated_at}","projects":[{}]}}"#,
        items.join(",")
    );
    std::fs::write(dir.join("snapshot.json"), json).unwrap();
}

fn write_events(machines_dir: &Path, machine_id: &str, lines: &[&str]) {
    let dir = machines_dir.join(machine_id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(dir.join("events.ndjson"), body).unwrap();
}

fn write_registry_project(state_root: &Path, id: Uuid, name: &str, machines: &[(&str, &str)]) {
    let dir = state_root.join("registry").join("projects");
    std::fs::create_dir_all(&dir).unwrap();
    let entries: Vec<String> = machines
        .iter()
        .map(|(key, path)| format!("  {key}: {path}"))
        .collect();
    let yaml = format!(
        "schema_version: 1\nproject_id: {id}\nname: {name}\n\
         git_url: https://github.com/owner/{name}\ngit_url_aliases: []\nsync: true\n\
         machines:\n{}\ncreated_at: 2026-07-10T02:03:56.913339700Z\n\
         updated_at: 2026-07-11T03:05:49.930111Z\n",
        entries.join("\n")
    );
    std::fs::write(dir.join(format!("{id}.yaml")), yaml).unwrap();
}

fn write_local_config(local_path: &Path, machine_id: &str, history: &[&str]) {
    let mut cfg = LocalConfig::new(machine_id);
    cfg.hostname_history = history.iter().map(|s| s.to_string()).collect();
    let yaml = serde_yaml::to_string(&cfg).unwrap();
    std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    std::fs::write(local_path, yaml).unwrap();
}

/// 合成迁移场景：Pro-3 有 两个合成项目的历史快照，改名后的 Pro-4 是空壳但 updated_at 更新。
/// 按整份 updated_at 取胜者会丢掉全部历史 —— 本测试就是钉死这一点。
#[test]
fn merge_keeps_real_history_when_newer_snapshot_is_an_empty_shell() {
    let tmp = make_tempdir("empty-shell");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();
    let board = Uuid::new_v4();

    // 旧身份：2 个项目，时间较早
    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T06:28:17.547178Z",
        &[
            (kb, "project-epsilon", "2026-07-23T06:28:17.547178Z"),
            (board, "project-dashboard", "2026-07-23T06:28:17.547178Z"),
        ],
    );
    // 改名后新身份：0 个项目，但时间更新（真机就是这个形状）
    write_snapshot(&machines, LEGACY_NEW, "2026-07-24T10:24:35.237530Z", &[]);
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let plan = identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    let merge = plan.snapshot_merge.expect("应产出合并计划");

    assert_eq!(
        merge.project_count, 2,
        "空壳快照时间戳更新，但合并后必须保住 两个合成项目的历史快照"
    );
    // 各来源贡献数要暴露出来，让用户一眼看出哪份是空壳。
    assert_eq!(
        merge.source_project_counts,
        vec![
            (LEGACY_OLD.to_string(), 2),
            (LEGACY_NEW.to_string(), 0),
        ]
    );
}

/// 同一项目在两份快照都有时，取项目级 collected_at 更新的那条。
#[test]
fn merge_prefers_project_entry_with_newer_collected_at() {
    let tmp = make_tempdir("newer-entry");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let shared = Uuid::new_v4();

    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-20T00:00:00Z",
        &[(shared, "old-entry", "2026-07-20T00:00:00Z")],
    );
    write_snapshot(
        &machines,
        LEGACY_NEW,
        "2026-07-21T00:00:00Z",
        &[(shared, "new-entry", "2026-07-21T00:00:00Z")],
    );
    write_local_config(&local_path, LEGACY_OLD, &[]);

    identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    let backups = tmp.join("backups");
    let report =
        identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();
    assert!(report.snapshot_written);

    let merged: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(machines.join(TARGET).join("snapshot.json")).unwrap(),
    )
    .unwrap();
    let projects = merged["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1, "同一 project_id 只该留一条");
    assert_eq!(
        projects[0]["name"].as_str(),
        Some("new-entry"),
        "应保留 collected_at 更新的那条"
    );
}

/// 认领判定：只认本机证据；别的机器目录必须出现在 skipped 而不是被改。
#[test]
fn claims_only_local_evidence_and_reports_skipped_machines() {
    let tmp = make_tempdir("claims");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");

    write_snapshot(&machines, LEGACY_OLD, "2026-07-23T00:00:00Z", &[]);
    write_snapshot(&machines, LEGACY_NEW, "2026-07-24T00:00:00Z", &[]);
    write_snapshot(&machines, OTHER_MACHINE, "2026-07-24T00:00:00Z", &[]);
    write_snapshot(&machines, "unknown-machine", "2026-07-11T00:00:00Z", &[]);
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let plan = identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();

    let claimed: Vec<&str> = plan.claims.iter().map(|c| c.machine_id.as_str()).collect();
    assert_eq!(claimed, vec![LEGACY_OLD, LEGACY_NEW]);
    assert_eq!(plan.claims[0].reason, ClaimReason::FrozenInLocalConfig);
    assert_eq!(plan.claims[1].reason, ClaimReason::CurrentHostname);

    let skipped: Vec<&str> = plan.skipped.iter().map(|s| s.machine_id.as_str()).collect();
    assert!(skipped.contains(&OTHER_MACHINE), "对方机不得被认领");
    assert!(
        skipped.contains(&"unknown-machine"),
        "无证据的孤儿身份不得被认领"
    );
}

/// 注册表改写：只换键名，路径值一个字节不动；未认领的键原样保留。
#[test]
fn registry_rewrites_key_only_and_leaves_other_machines_untouched() {
    let tmp = make_tempdir("registry");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let kb = Uuid::new_v4();
    let mac_path = "/Users/example/Documents/project-delta";
    let win_path = r"F:\project-delta";

    write_registry_project(
        &state_root,
        kb,
        "project-epsilon",
        &[(OTHER_MACHINE, win_path), (LEGACY_OLD, mac_path)],
    );
    std::fs::create_dir_all(state_root.join("machines")).unwrap();
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let plan = identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    assert_eq!(plan.registry_edits.len(), 1);
    assert_eq!(plan.registry_edits[0].from_key, LEGACY_OLD);
    assert_eq!(plan.registry_edits[0].to_key, TARGET);
    assert_eq!(plan.registry_edits[0].path, mac_path);

    let backups = tmp.join("backups");
    identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();

    let registry = Registry::load_at_read_only(&state_root, &local_path).unwrap();
    let project: &RegistryProject = registry.projects.get(&kb).unwrap();
    let machines: &BTreeMap<String, String> = &project.machines;
    assert_eq!(
        machines.get(TARGET).map(String::as_str),
        Some(mac_path),
        "新键必须承接原路径值"
    );
    assert!(!machines.contains_key(LEGACY_OLD), "旧键应已改名");
    assert_eq!(
        machines.get(OTHER_MACHINE).map(String::as_str),
        Some(win_path),
        "对方机条目必须一个字节不动"
    );
}

/// 干跑不写盘；真改前先备份；旧目录一个都不删。
#[test]
fn plan_writes_nothing_and_apply_backs_up_and_keeps_legacy_dirs() {
    let tmp = make_tempdir("noop-and-backup");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();

    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T00:00:00Z",
        &[(kb, "project-epsilon", "2026-07-23T00:00:00Z")],
    );
    write_events(&machines, LEGACY_OLD, &[r#"{"e":"a"}"#, r#"{"e":"b"}"#]);
    write_registry_project(&state_root, kb, "project-epsilon", &[(LEGACY_OLD, "/tmp/kb")]);
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let before = std::fs::read_to_string(local_path.as_path()).unwrap();
    identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    assert_eq!(
        std::fs::read_to_string(local_path.as_path()).unwrap(),
        before,
        "干跑不得改动 local.yaml"
    );
    assert!(
        !machines.join(TARGET).exists(),
        "干跑不得创建目标身份目录"
    );

    let backups = tmp.join("backups");
    let report =
        identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();

    assert!(report.backup_path.exists(), "真改前必须留下备份");
    assert!(
        report.backup_path.join("machines").join(LEGACY_OLD).exists(),
        "备份必须含旧身份数据，才能回滚"
    );
    assert!(
        machines.join(LEGACY_OLD).exists(),
        "旧目录必须原样保留（只加不删）"
    );
    assert_eq!(report.legacy_dirs_kept, vec![LEGACY_OLD.to_string()]);
    // 事件流搬到新身份下且逐行去重后保序。
    let events =
        std::fs::read_to_string(machines.join(TARGET).join("events.ndjson")).unwrap();
    assert_eq!(events.lines().collect::<Vec<_>>(), vec![r#"{"e":"a"}"#, r#"{"e":"b"}"#]);
    // 身份已冻结进 local.yaml，且当次主机名留痕。
    let saved = Registry::load_at_read_only(&state_root, &local_path)
        .unwrap()
        .local
        .unwrap();
    assert_eq!(saved.machine_id, TARGET);
    assert_eq!(saved.hostname, LEGACY_NEW);
    assert!(saved.hostname_history.contains(&LEGACY_NEW.to_string()));
}

/// 幂等：同一迁移跑两次，第二次应识别为无事可做（不重复改写、不再造快照差异）。
#[test]
fn apply_is_idempotent() {
    let tmp = make_tempdir("idempotent");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();

    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T00:00:00Z",
        &[(kb, "project-epsilon", "2026-07-23T00:00:00Z")],
    );
    write_registry_project(&state_root, kb, "project-epsilon", &[(LEGACY_OLD, "/tmp/kb")]);
    write_local_config(&local_path, LEGACY_OLD, &[]);
    let backups = tmp.join("backups");

    identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();
    let first = std::fs::read_to_string(machines.join(TARGET).join("snapshot.json")).unwrap();

    let second_plan =
        identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    assert!(
        second_plan.registry_edits.is_empty(),
        "第二次不应再有注册表改写"
    );

    identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();
    let after = std::fs::read_to_string(machines.join(TARGET).join("snapshot.json")).unwrap();
    assert_eq!(first, after, "重复迁移不得改变已合并结果");
}

/// 在临时目录起一个自足的 git 仓：身份与配置都只写进这个仓，不碰机器上的 git 全局配置。
fn git_init_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    run_git_in(repo, &["init", "-q"]);
    run_git_in(repo, &["config", "user.email", "test@example.invalid"]);
    run_git_in(repo, &["config", "user.name", "dashboard-test"]);
}

fn run_git_in(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// 迁移完成后再干跑必须是 no-op。
///
/// 「旧目录只加不删」让被认领的旧目录永远在、合并 sources 永远非空；若不判「这次合并是否
/// 真会改变目标」，设置页会永远挂着一个其实什么都不改的「执行迁移」按钮（真机 T5 就是这么 FAIL 的）。
/// 注意本测试比 `apply_is_idempotent` 严格：那条只验重复执行的结果字节不变，验不出计划非空。
#[test]
fn second_plan_after_apply_is_noop() {
    let tmp = make_tempdir("second-plan-noop");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();
    let board = Uuid::new_v4();

    // 真机形状：Pro-3 有 2 个项目的真历史，改名后的 Pro-4 是空壳但时间戳更新。
    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T00:00:00Z",
        &[
            (kb, "project-epsilon", "2026-07-23T00:00:00Z"),
            (board, "project-dashboard", "2026-07-23T00:00:00Z"),
        ],
    );
    write_events(&machines, LEGACY_OLD, &[r#"{"e":"a"}"#, r#"{"e":"b"}"#]);
    write_snapshot(&machines, LEGACY_NEW, "2026-07-24T00:00:00Z", &[]);
    write_registry_project(&state_root, kb, "project-epsilon", &[(LEGACY_OLD, "/tmp/kb")]);
    write_local_config(&local_path, LEGACY_OLD, &[]);
    let backups = tmp.join("backups");

    let first = identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    assert!(!first.is_noop(), "首次干跑必须有事可做，否则本测试没有意义");

    identity_migration::apply(&state_root, &local_path, &backups, TARGET, LEGACY_NEW).unwrap();

    let second = identity_migration::plan(&state_root, &local_path, TARGET, LEGACY_NEW).unwrap();
    assert!(
        second.registry_edits.is_empty(),
        "第二次不应再有注册表改写"
    );
    assert!(
        second.snapshot_merge.is_none(),
        "合并结果与目标现状相同时，不该再列进计划：{:?}",
        second.snapshot_merge
    );
    assert!(
        second.is_noop(),
        "迁移完成后再干跑必须是 no-op，否则设置页永远挂着一个什么都不改的迁移按钮"
    );
    // 旧目录仍在（只加不删），即 no-op 不是靠删目录换来的。
    assert!(machines.join(LEGACY_OLD).exists());
    assert!(machines.join(LEGACY_NEW).exists());
}

/// 迁移必须让共享仓回到干净工作树。
///
/// 同步走 pull-first，git 在工作树有未提交改动时拒绝 pull（配了 pull.rebase=true 更严格），
/// 于是迁移只要留下脏工作树，此后**所有**同步都失败 —— 真机 T7/R3 正是这么挂的，
/// 且 UI 只报 `[UNKNOWN] 同步失败`，完全看不出跟迁移有关。
#[test]
fn apply_commits_migration_and_leaves_clean_worktree() {
    let tmp = make_tempdir("commit-after-apply");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml"); // 仓外，不该被卷进提交
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();

    git_init_repo(&state_root);
    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T00:00:00Z",
        &[(kb, "project-epsilon", "2026-07-23T00:00:00Z")],
    );
    write_events(&machines, LEGACY_OLD, &[r#"{"e":"a"}"#]);
    write_registry_project(&state_root, kb, "project-epsilon", &[(LEGACY_OLD, "/tmp/kb")]);
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let report = identity_migration::apply(
        &state_root,
        &local_path,
        &tmp.join("backups"),
        TARGET,
        LEGACY_NEW,
    )
    .unwrap();

    assert!(
        report.committed,
        "迁移改动必须已提交，否则后续同步全部被 pull-first 拦死；note={:?}",
        report.commit_note
    );
    assert_eq!(report.commit_note, None);
    assert!(
        run_git_in(&state_root, &["status", "--porcelain"])
            .trim()
            .is_empty(),
        "迁移后工作树必须干净"
    );
    let log = run_git_in(&state_root, &["log", "--oneline"]);
    assert!(log.contains("身份迁移"), "提交信息应说明这是身份迁移：{log}");
    // 白名单生效：只收 registry/ 与 machines/，仓外的 local.yaml 不可能进提交。
    let tracked = run_git_in(&state_root, &["ls-files"]);
    assert!(tracked.contains("registry/projects/"));
    assert!(tracked.contains(&format!("machines/{TARGET}/snapshot.json")));
    assert!(!tracked.contains("local.yaml"));
}

/// 非 git 仓（用户把数据目录配成普通目录）不能让迁移失败 —— 数据已写完，
/// 只如实回报「没生成提交」，由 UI 提示。
#[test]
fn apply_reports_uncommitted_when_state_root_is_not_a_git_repo() {
    let tmp = make_tempdir("commit-non-git");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    let machines = state_root.join("machines");
    let kb = Uuid::new_v4();

    write_snapshot(
        &machines,
        LEGACY_OLD,
        "2026-07-23T00:00:00Z",
        &[(kb, "project-epsilon", "2026-07-23T00:00:00Z")],
    );
    write_registry_project(&state_root, kb, "project-epsilon", &[(LEGACY_OLD, "/tmp/kb")]);
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let report = identity_migration::apply(
        &state_root,
        &local_path,
        &tmp.join("backups"),
        TARGET,
        LEGACY_NEW,
    )
    .unwrap();

    assert!(!report.committed);
    assert!(
        report
            .commit_note
            .as_deref()
            .unwrap_or_default()
            .contains("不是 git 仓库"),
        "应说清没提交的原因：{:?}",
        report.commit_note
    );
    // 数据本身仍然照常写入，不因为没提交就回滚。
    assert!(machines.join(TARGET).join("snapshot.json").exists());
}

/// 目标身份必须是硬件式 —— 拒绝迁到另一个照样会漂的主机名身份上。
#[test]
fn rejects_unstable_target_identity() {
    let tmp = make_tempdir("unstable-target");
    let state_root = tmp.join("state");
    let local_path = tmp.join("local.yaml");
    std::fs::create_dir_all(state_root.join("machines")).unwrap();
    write_local_config(&local_path, LEGACY_OLD, &[]);

    let error = identity_migration::plan(
        &state_root,
        &local_path,
        "exampledeMacBook-Pro-5",
        LEGACY_NEW,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("不是硬件式标识"),
        "错误信息应说清拒绝理由：{error}"
    );
}
