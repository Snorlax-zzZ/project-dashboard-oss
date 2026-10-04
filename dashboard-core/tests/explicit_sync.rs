// 组 5（决策 5 · 显式出机模型）+ 组 6（决策 6 · 顶栏假成功修复）端到端测试（2026-07-21）
//
// 组 5 覆盖 tasks.md 5.5 + 三方评审收窄（2026-07-21）后的语义：
// - 手动同步一条龙：配 remote 的 Ready 仓 → manual_sync(push=true) → pull(no-op) → 采集 → commit → push → bare HEAD 前进
// - CRUD（add sync:true）后 bare HEAD **不变**（只落盘本地 registry，不即时 push）
// - 方案1 收窄：collect_single(push=false) 纯采集**不触发前置 pull**（远端已前进 diverge 也不中止、能落盘）
// - 端到端错仓污染防护：错仓（无 registry/projects）+ CRUD sync:true 被门禁拒 + 手动同步 → origin HEAD 不变
//
// 组 6 覆盖 tasks.md 6.1/6.2 判别式 outcome 契约（前端 app.js 据此渲染顶栏、决定是否写"最后同步时间"）：
// - Unconfigured 输入 → outcome=Unconfigured no-op 退 0；一条龙全成功 → Synced{pushed:true}
// - push 失败（bare 装 pre-receive 拒收）→ Synced{pushed:false} 退 0 不抛 Err（部分成功）
// - ManualSyncResult serde wire 形状（outcome.status / outcome.pushed）锁死前端依赖
//
// 说明：所有 dashboard-state 仓都配真 bare remote origin（三方评审收窄后 resolve_status 要求有 origin 才判 Ready）。

use dashboard_core::commands::{self, AddProjectRequest};
use dashboard_core::config::LocalConfig;
use dashboard_core::sync::{is_dashboard_state_repo, DashboardStateRepo};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const M: &str = "TEST-MACHINE-EXPLICIT";

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-explicit-{}-{}-{}",
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

fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git").args(args).current_dir(cwd).output().unwrap()
}

fn bare_head(bare: &Path) -> Option<String> {
    let out = git(bare, &["rev-parse", "main"]);
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 造 bare remote + 从它 clone 出的合法 dashboard-state 工作区（含 registry/projects），返回
/// (bare, clone_root, remote_url)。clone 出来 origin 指向 bare，pull/push 真前进。
fn seed_ready_clone(tag: &str) -> (PathBuf, PathBuf, String) {
    let tmp = make_tempdir(tag);
    let bare = tmp.join("remote.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);

    let seed = tmp.join("seed");
    std::fs::create_dir_all(seed.join("registry").join("projects")).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@pd"]);
    git(&seed, &["config", "user.name", "pd"]);
    git(&seed, &["config", "commit.gpgsign", "false"]);
    std::fs::write(seed.join("registry").join("projects").join(".keep"), "").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&seed, &["push", "-q", "origin", "main"]);

    let root = tmp.join("dashboard-state-clone");
    git(&tmp, &["clone", "-q", bare.to_str().unwrap(), root.to_str().unwrap()]);
    git(&root, &["config", "user.email", "t@pd"]);
    git(&root, &["config", "user.name", "pd"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let remote_url = bare.to_string_lossy().to_string();
    (bare, root, remote_url)
}

/// 写 local.yaml，可选配 dashboard_state_remote。
fn write_local(local_path: &Path, remote: Option<&str>) {
    std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let mut local = LocalConfig::new(M);
    local.dashboard_state_remote = remote.map(String::from);
    std::fs::write(local_path, serde_yaml::to_string(&local).unwrap()).unwrap();
}

/// 造一个真实 git 项目仓（供采集用），返回路径。
fn make_project_repo(tmp: &Path, name: &str) -> PathBuf {
    let p = tmp.join(name);
    std::fs::create_dir_all(&p).unwrap();
    git(&p, &["init", "-q", "-b", "main"]);
    git(&p, &["config", "user.email", "t@pd"]);
    git(&p, &["config", "user.name", "pd"]);
    git(&p, &["config", "commit.gpgsign", "false"]);
    std::fs::write(p.join("f.txt"), "hi\n").unwrap();
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-q", "-m", "init"]);
    p
}

// ── 手动同步一条龙：pull → 采集 → commit → push，bare HEAD 前进 ──
#[test]
fn manual_sync_one_shot_advances_remote() {
    let (bare, root, remote_url) = seed_ready_clone("one-shot");
    let tmp = root.parent().unwrap().to_path_buf();
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, Some(&remote_url));

    // 加一个 sync:true 项目（真 git 仓），供全库采集
    let proj = make_project_repo(&tmp, "proj-a");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_string(), proj.to_string_lossy().to_string());
    commands::add_project(
        &root,
        &local_path,
        AddProjectRequest {
            name: "proj-a".into(),
            git_url: "https://github.com/example/proj-a".into(),
            path: proj.to_string_lossy().to_string(),
            machines,
            sync: true,
            confirm_dual_form: false,
        },
    )
    .expect("add sync:true 项目应成功（Ready 仓）");

    let before = bare_head(&bare).unwrap();
    let r = commands::manual_sync(&root, &local_path, M, dashboard_core::sync::Trigger::Manual, true)
        .expect("手动同步一条龙应成功");
    assert!(r.pushed, "一条龙 push 应成功");
    assert_eq!(
        r.outcome,
        commands::ManualSyncOutcome::Synced { pushed: true },
        "组 6：一条龙全成功 → outcome=Synced{{pushed:true}}（前端据此写「最后同步时间」）"
    );
    assert_ne!(bare_head(&bare).unwrap(), before, "手动同步一条龙后 bare HEAD 必须前进");
    let history_path = root.with_file_name("dashboard-state-clone.sync-status.json");
    assert!(history_path.is_file(), "真实推送成功必须留下可跨进程读取的本机同步记录");
    let history: serde_json::Value = serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
    assert!(history["status"]["last_success_at"].is_string());
    assert_eq!(history["status"]["last_attempt"]["outcome"], "succeeded");
    assert!(!String::from_utf8_lossy(&git(&root, &["ls-files"]).stdout).contains("sync-status"),
        "本机同步记录不能进入跨机 Git 仓");
    let restored = commands::get_sync_status(&root, &local_path, M).unwrap();
    assert!(restored.last_success_at.is_some());
}

// ── CRUD（add sync:true）只落盘本地 registry，不即时 push：bare HEAD 不变 ──
#[test]
fn crud_does_not_push_immediately() {
    let (bare, root, remote_url) = seed_ready_clone("crud-nopush");
    let tmp = root.parent().unwrap().to_path_buf();
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, Some(&remote_url));

    let before = bare_head(&bare).unwrap();
    let proj = make_project_repo(&tmp, "proj-b");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_string(), proj.to_string_lossy().to_string());
    commands::add_project(
        &root,
        &local_path,
        AddProjectRequest {
            name: "proj-b".into(),
            git_url: "https://github.com/example/proj-b".into(),
            path: proj.to_string_lossy().to_string(),
            machines,
            sync: true,
            confirm_dual_form: false,
        },
    )
    .expect("add sync:true 应成功");

    // 决策 5：CRUD 只写本地 registry/*.yaml，不即时 push → bare HEAD 原地不动
    assert_eq!(bare_head(&bare).unwrap(), before, "CRUD 不得即时 push，bare HEAD 必须不变");
    // 本地 registry 文件已落盘
    let has_yaml = std::fs::read_dir(root.join("registry").join("projects"))
        .unwrap()
        .flatten()
        .any(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false));
    assert!(has_yaml, "CRUD 应已把 registry/*.yaml 落盘本地");
}

// ── 方案1 收窄：collect_single(push=false) 纯采集不触发前置 pull（远端 diverge 也不中止）──
#[test]
fn collect_without_push_does_not_pull() {
    let (bare, root, remote_url) = seed_ready_clone("no-pull");
    let tmp = root.parent().unwrap().to_path_buf();
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, Some(&remote_url));

    // 让远端前进一个 commit（制造本地落后 = 若前置 pull 会拉取；若不 pull 则本地 diverge 但采集仍应继续）
    let other = tmp.join("other-clone");
    git(&tmp, &["clone", "-q", bare.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["config", "user.email", "t@pd"]);
    git(&other, &["config", "user.name", "pd"]);
    git(&other, &["config", "commit.gpgsign", "false"]);
    std::fs::write(other.join("registry").join("projects").join("x.keep"), "x").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "remote advance"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let remote_after_advance = bare_head(&bare).unwrap();

    // 注册一个项目 + collect_single(push=false)：应只采集落盘、不触发前置 pull、不 push
    let proj = make_project_repo(&tmp, "proj-c");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_string(), proj.to_string_lossy().to_string());
    let add = commands::add_project(
        &root,
        &local_path,
        AddProjectRequest {
            name: "proj-c".into(),
            git_url: "https://github.com/example/proj-c".into(),
            path: proj.to_string_lossy().to_string(),
            machines,
            sync: true,
            confirm_dual_form: false,
        },
    )
    .expect("add 应成功");
    let pid = add.project_id.unwrap();

    let r = commands::collect_single(
        &root,
        &local_path,
        M,
        pid,
        dashboard_core::sync::Trigger::Manual,
        false, // verify_remote
        false, // push=false → land-only，收窄后不触发前置 pull
    )
    .expect("push=false 纯采集不应因本地落后远端而中止");
    assert!(!r.pushed, "push=false 不应 push");
    // snapshot 已落盘本地
    assert!(
        root.join("machines").join(M).join("snapshot.json").exists(),
        "push=false 仍应落盘本机 snapshot"
    );
    // bare remote 不动（没 push）
    assert_eq!(bare_head(&bare).unwrap(), remote_after_advance, "push=false 不得推进 bare remote");
}

// ── 5.7 私有洗白根治：错仓（有 .git 无 registry/projects）+ add sync:false → 不建 registry/projects ──
//
// B2 变种（v2 复审 P0）：私有项目走 save_local_only，绝不 create_dir_all(registry/projects)，
// 否则会把错仓（用户填错地址指向的普通仓）洗白成合法 dashboard-state 结构、骗过 InvalidStructure 门禁。
#[test]
fn add_private_in_invalid_repo_does_not_create_registry() {
    let tmp = make_tempdir("private-no-whitewash");
    // 错仓：有 .git 但**无** registry/projects（模拟用户填错地址指向普通 git 仓）
    let root = tmp.join("wrong-repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "t@pd"]);
    git(&root, &["config", "user.name", "pd"]);
    std::fs::write(root.join("README.md"), "普通仓\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "普通仓 commit"]);

    // 前置断言：此刻不是合法 dashboard-state 结构
    assert!(!is_dashboard_state_repo(&root), "前置：错仓不应含 registry/projects");

    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, None); // 私有项目不需要配 remote

    let priv_proj = make_project_repo(&tmp, "private-x");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_string(), priv_proj.to_string_lossy().to_string());
    commands::add_project(
        &root,
        &local_path,
        AddProjectRequest {
            name: "private-x".into(),
            git_url: "https://gitlab.internal/team/private-x".into(),
            path: priv_proj.to_string_lossy().to_string(),
            machines,
            sync: false, // 私有项目
            confirm_dual_form: false,
        },
    )
    .expect("add sync:false 应成功（私有项目不碰共享仓门禁）");

    // 核心断言：add sync:false 后**没有**在错仓里造出 registry/projects（未被洗白）
    assert!(
        !is_dashboard_state_repo(&root),
        "私有项目 add 不得在错仓里造 registry/projects（洗白防护）"
    );
    assert!(
        !root.join("registry").join("projects").exists(),
        "registry/projects 目录不应被 save_local_only 创建"
    );

    // 出机点仍判非法：resolve_status 对该错仓仍应判 InvalidStructure（结构门禁未被洗白）
    let repo = DashboardStateRepo::new(&root, M);
    let observed = repo.observe();
    assert!(!observed.has_registry, "observe 应仍判无 registry 结构");

    // 私有项目本身已落盘 local.yaml（本地记录，不出机）
    let saved = std::fs::read_to_string(&local_path).unwrap();
    assert!(saved.contains("private-x"), "私有项目应已写入 local.yaml");
}

// ══════════════════════════════════════════════════════════════════════════
// 组 6（决策 6 · 顶栏假成功修复）：manual_sync 判别式 outcome 契约
// 前端 app.js 按 outcome.status 决定顶栏渲染 + 是否写"最后同步时间"，这里锁死后端契约。
// ══════════════════════════════════════════════════════════════════════════

// ── Unconfigured：空 root + 未配 remote → Ok(outcome=Unconfigured) no-op，绝不抛 Err ──
#[test]
fn manual_sync_unconfigured_returns_unconfigured_outcome() {
    let tmp = make_tempdir("outcome-unconfigured");
    // 空 root（无 .git、无 registry）+ local.yaml 未配 remote → resolve_status 判 Unconfigured
    let root = tmp.join("empty-state");
    std::fs::create_dir_all(&root).unwrap();
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, None);

    let r =
        commands::manual_sync(&root, &local_path, M, dashboard_core::sync::Trigger::Manual, true)
            .expect("未配存储仓应是 Ok no-op（判别式 Unconfigured），绝不抛 Err");
    assert_eq!(
        r.outcome,
        commands::ManualSyncOutcome::Unconfigured,
        "未配存储仓 → outcome=Unconfigured（前端顶栏不假报同步成功、不写时间戳）"
    );
    assert!(!r.pushed, "Unconfigured 时 pushed 恒 false");
    assert_eq!(r.project_count, 0, "Unconfigured no-op 不采集任何项目");
    let restored = commands::get_sync_status(&root, &local_path, M).unwrap();
    assert!(restored.last_success_at.is_none());
    assert_eq!(restored.last_attempt.unwrap().outcome, dashboard_core::sync_status::SyncOutcome::Unconfigured);
}

// ── push 失败：pull 成功但 remote 拒收 push → Ok(outcome=Synced{pushed:false})、退 0 不抛 Err ──
//
// 关键：manual_sync(push=true) 会前置 `pull --ff-only`，且 `trigger_config_pull` 校验
// origin==配置 remote。故不能像 push_contract.rs 那样"set-url origin 坏路径"（会先撞
// RemoteMismatch Conflict 返 Err、到不了 sync_cycle）。改用 bare 装 pre-receive 钩子拒收
// push——fetch/pull 走 upload-pack 不触发该钩子、origin 仍匹配，才能干净复现"pull 通、push 败"。
#[test]
fn manual_sync_push_failure_returns_synced_not_pushed() {
    let (bare, root, remote_url) = seed_ready_clone("outcome-pushfail");
    let tmp = root.parent().unwrap().to_path_buf();
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    write_local(&local_path, Some(&remote_url));

    // 注册一个 sync:true 项目，保证全库采集有内容可 commit（land+commit 成功才谈 push 失败）
    let proj = make_project_repo(&tmp, "proj-pf");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_string(), proj.to_string_lossy().to_string());
    commands::add_project(
        &root,
        &local_path,
        AddProjectRequest {
            name: "proj-pf".into(),
            git_url: "https://github.com/example/proj-pf".into(),
            path: proj.to_string_lossy().to_string(),
            machines,
            sync: true,
            confirm_dual_form: false,
        },
    )
    .expect("add sync:true 应成功（Ready 仓）");

    // bare 装 pre-receive 钩子：拒收一切 push（exit 1）；fetch/pull 不受影响
    let hook = bare.join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let remote_head_before = bare_head(&bare).unwrap();
    let r =
        commands::manual_sync(&root, &local_path, M, dashboard_core::sync::Trigger::Manual, true)
            .expect("push 失败必须部分成功、不抛 Err（前置 pull + land + commit 已成功）");
    assert_eq!(
        r.outcome,
        commands::ManualSyncOutcome::Synced { pushed: false },
        "remote 拒收 push → outcome=Synced{{pushed:false}}（前端提示已本地保存·推送失败，不写时间戳）"
    );
    assert!(!r.pushed, "push 被拒 pushed=false");
    assert_eq!(
        bare_head(&bare).unwrap(),
        remote_head_before,
        "push 失败 bare remote HEAD 不得前进"
    );
    // 本地 commit 已产生（land+commit 成功，只是 push 被远端拒）
    let local_head = String::from_utf8_lossy(&git(&root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    assert_ne!(local_head, remote_head_before, "push 失败但本地 commit 必须已落");
}

// ── serde wire 形状：前端 app.js 读 r.outcome.status / r.outcome.pushed，锁死判别式序列化 ──
#[test]
fn manual_sync_result_serializes_discriminated_status_for_frontend() {
    use commands::{ManualSyncOutcome, ManualSyncResult};

    // synced + pushed=true → {"outcome":{"status":"synced","pushed":true}, "collected_at":"..."}
    let synced = ManualSyncResult {
        outcome: ManualSyncOutcome::Synced { pushed: true },
        pushed: true,
        project_count: 3,
        collected_at: chrono::Utc::now(),
        sync_record_warning: None,
    };
    let v = serde_json::to_value(&synced).unwrap();
    assert_eq!(v["outcome"]["status"], "synced", "内部 tag 键为 status");
    assert_eq!(v["outcome"]["pushed"], true);
    assert!(
        v["collected_at"].is_string(),
        "前端写时间戳依赖 collected_at 序列化为字符串"
    );

    // synced + pushed=false（部分成功）
    let partial = ManualSyncResult {
        outcome: ManualSyncOutcome::Synced { pushed: false },
        pushed: false,
        project_count: 3,
        collected_at: chrono::Utc::now(),
        sync_record_warning: None,
    };
    let v = serde_json::to_value(&partial).unwrap();
    assert_eq!(v["outcome"]["status"], "synced");
    assert_eq!(v["outcome"]["pushed"], false);

    // unconfigured（单元变体）→ {"status":"unconfigured"}，不带 pushed 字段
    let unconf = ManualSyncResult {
        outcome: ManualSyncOutcome::Unconfigured,
        pushed: false,
        project_count: 0,
        collected_at: chrono::Utc::now(),
        sync_record_warning: None,
    };
    let v = serde_json::to_value(&unconf).unwrap();
    assert_eq!(v["outcome"]["status"], "unconfigured");
    assert!(
        v["outcome"].get("pushed").is_none(),
        "Unconfigured 是单元变体，不应带 pushed 字段（前端靠 status 分支即可）"
    );
}

#[cfg(unix)]
fn reject_push(bare: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let hook = bare.join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(unix)]
fn history_preserves_real_success_then_failure_and_ignores_local_collection() {
    use dashboard_core::sync_status::SyncOutcome;
    use dashboard_core::sync::Trigger;
    let (bare, root, remote) = seed_ready_clone("history-cycle");
    let local = root.parent().unwrap().join("local.yaml");
    write_local(&local, Some(&remote));
    assert!(commands::manual_sync(&root, &local, M, Trigger::Manual, true).unwrap().pushed);
    let saved = commands::get_sync_status(&root, &local, M).unwrap();
    reject_push(&bare);
    assert!(!commands::manual_sync(&root, &local, M, Trigger::Manual, true).unwrap().pushed);
    let failed = commands::get_sync_status(&root, &local, M).unwrap();
    assert_eq!(failed.last_success_at, saved.last_success_at);
    assert_eq!(failed.last_attempt.as_ref().unwrap().outcome, SyncOutcome::PushFailed);
    commands::manual_sync(&root, &local, M, Trigger::Manual, false).unwrap();
    assert_eq!(commands::get_sync_status(&root, &local, M).unwrap(), failed,
        "纯本地采集不覆盖最近失败");
    std::fs::write(root.join("machines").join(M).join("snapshot.json"), b"broken").unwrap();
    assert!(commands::manual_sync(&root, &local, M, Trigger::Manual, true).is_err());
    let read_failed = commands::get_sync_status(&root, &local, M).unwrap();
    assert_eq!(read_failed.last_success_at, saved.last_success_at);
    assert_eq!(read_failed.last_attempt.unwrap().outcome, SyncOutcome::Failed);
}

#[test]
#[cfg(unix)]
fn unchanged_hook_does_not_clear_failed_push_history() {
    use dashboard_core::sync::{Trigger};
    use dashboard_core::sync_status::SyncOutcome;
    let (bare, root, remote) = seed_ready_clone("history-hook");
    let tmp = root.parent().unwrap();
    let local = tmp.join("local.yaml");
    write_local(&local, Some(&remote));
    let project = make_project_repo(tmp, "hook-project");
    let mut machines = BTreeMap::new();
    machines.insert(M.to_owned(), project.to_string_lossy().into_owned());
    let added = commands::add_project(&root, &local, AddProjectRequest {
        name: "hook-project".into(), git_url: "https://example.com/tests/hook-project".into(),
        path: project.to_string_lossy().into_owned(), machines, sync: true, confirm_dual_form: false,
    }).unwrap();
    let id = added.project_id.unwrap();
    let first = commands::collect_single(&root, &local, M, id, Trigger::Hook, false, true).unwrap();
    assert!(first.pushed);
    let success = commands::get_sync_status(&root, &local, M).unwrap();
    reject_push(&bare);
    std::fs::write(project.join("f.txt"), "changed").unwrap();
    let failed = commands::collect_single(&root, &local, M, id, Trigger::Hook, false, true).unwrap();
    assert!(!failed.pushed);
    let saved = commands::get_sync_status(&root, &local, M).unwrap();
    assert_eq!(saved.last_success_at, success.last_success_at);
    assert_eq!(saved.last_attempt.as_ref().unwrap().outcome, SyncOutcome::PushFailed);
    let skipped = commands::collect_single(&root, &local, M, id, Trigger::Hook, false, true).unwrap();
    assert!(!skipped.pushed);
    assert_eq!(commands::get_sync_status(&root, &local, M).unwrap(), saved);
}

#[test]
fn recording_failure_is_explicit_without_relabeling_a_successful_push() {
    use dashboard_core::sync::Trigger;
    let (_bare, root, remote) = seed_ready_clone("history-write-fail");
    let local = root.parent().unwrap().join("local.yaml");
    write_local(&local, Some(&remote));
    let history = root.with_file_name("dashboard-state-clone.sync-status.json");
    std::fs::write(&history, b"broken").unwrap();
    let result = commands::manual_sync(&root, &local, M, Trigger::Manual, true).unwrap();
    assert!(result.pushed);
    assert!(result.sync_record_warning.unwrap().contains("同步记录保存失败"));
    assert_eq!(std::fs::read(history).unwrap(), b"broken");
    assert!(commands::get_sync_status(&root, &local, M).is_err());
}
