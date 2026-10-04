// P1-B collect_single 集成测试（2026-07-06）
//
// 覆盖 spec 定义的 5 类失败路径 + 1 类正常路径：
// - 正常单项目采集（project_count == 1）
// - 未知 uuid → ProjectNotFound
// - 私有项目 uuid（get_shared 找不到）→ ProjectNotFound（P0-A 隐私回归防护）
// - 项目在 registry 但 machines 未配置本机 → InvalidSettings
// - 项目在 registry 且 machines 有本机 key 但路径不存在 → InvalidSettings
// - CLI 层非法 uuid 走 dashboard-cli 自己 tests，本文件不覆盖

use chrono::Utc;
use dashboard_core::commands::{self, CommandError};
use dashboard_core::config::{
    LocalConfig, PrivateProject, Registry, RegistryProject, CURRENT_SCHEMA_VERSION,
};
use dashboard_core::sync::Trigger;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const TEST_MACHINE: &str = "TEST-MACHINE-COLLECT";

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-collect-single-{}-{}-{}",
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

fn git_init(cwd: &Path) {
    let _ = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(cwd)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@project-dashboard"])
        .current_dir(cwd)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "project-dashboard-test"])
        .current_dir(cwd)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "commit.gpgsign", "false"])
        .current_dir(cwd)
        .output();
}

/// 给一个已 git_init 的 dashboard_state 配一个 bare origin 并推初始 commit，返回 bare 路径。
///
/// 决策 5：`manual_sync` 前置 `pull --ff-only`。要走到「采集 / snapshot 检查」阶段，前置 pull 必须
/// 能成功——纯 `git_init`（无 origin）的仓 pull 会失败提前中止。此 helper 让仓有可 ff-only pull 的
/// 上游（clone 同源、pull 即 no-op），从而测到 pull 之后的语义（如 fail-closed snapshot 检查）。
fn attach_bare_origin(dashboard_state: &Path, tmp: &Path) -> PathBuf {
    let bare = tmp.join("remote.git");
    fs::create_dir_all(&bare).unwrap();
    let run = |cwd: &Path, args: &[&str]| {
        let _ = std::process::Command::new("git").args(args).current_dir(cwd).output();
    };
    run(&bare, &["init", "-q", "--bare", "-b", "main"]);
    // dashboard_state 里先有一个 commit（registry/projects 已由 seed_registry 建），推到 bare
    run(dashboard_state, &["add", "-A"]);
    run(dashboard_state, &["commit", "-q", "-m", "seed"]);
    run(dashboard_state, &["remote", "add", "origin", bare.to_str().unwrap()]);
    run(dashboard_state, &["push", "-q", "-u", "origin", "main"]);
    bare
}

/// 布 dashboard_state git 仓 + 一个 sync:true 项目 + local.yaml
fn seed_registry(
    tmp: &Path,
    project_id: Uuid,
    project_name: &str,
    machine_key: Option<&str>,
    machine_path: Option<&str>,
) -> (PathBuf, PathBuf) {
    let dashboard_state = tmp.join("dashboard-state");
    fs::create_dir_all(&dashboard_state).unwrap();
    git_init(&dashboard_state);

    let mut machines = BTreeMap::new();
    if let (Some(k), Some(p)) = (machine_key, machine_path) {
        machines.insert(k.to_string(), p.to_string());
    }
    let now = Utc::now();
    let reg = RegistryProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        project_id,
        name: project_name.to_string(),
        git_url: "https://github.com/example/test-repo".to_string(),
        git_url_aliases: Vec::new(),
        sync: true,
        machines,
        created_at: now,
        updated_at: now,
    };
    let projects_dir = dashboard_state.join("registry").join("projects");
    fs::create_dir_all(&projects_dir).unwrap();
    let yaml = serde_yaml::to_string(&reg).unwrap();
    fs::write(projects_dir.join(format!("{project_id}.yaml")), yaml).unwrap();

    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let local = LocalConfig::new(TEST_MACHINE);
    fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();

    // 三方评审收窄（2026-07-21）：resolve_status 对「有 .git + 无配置 remote + **无 origin**」的仓
    // 改判 Unconfigured（不再无条件 Ready）。测试仓必须挂一个真 origin 才被判 Ready、走到采集逻辑，
    // 否则 collect_single 遇 Unconfigured 直接 no-op 返回、测不到目标路径。挂 bare origin 也更贴近
    // 生产（真实 dashboard-state 仓恒有 origin）。collect_single 前置 pull_if_ready=false，不触发 pull。
    attach_bare_origin(&dashboard_state, tmp);

    (dashboard_state, local_path)
}

#[test]
fn collect_single_ok_returns_project_count_1() {
    // 正常路径：sync:true 项目 + 本机路径存在 → collect_single 返回 project_count = 1
    let tmp = make_tempdir("ok");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);

    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );

    let r = commands::collect_single(&dashboard_state, &local_path, TEST_MACHINE, id, Trigger::Manual, false, true);
    // push 会因为无 remote 失败，但 write_snapshot + append_event 成功即语义 OK；
    // 若 push 失败会拿到 Err(Sync) —— 仍然值得确认走完前置采集逻辑
    match r {
        Ok(res) => assert_eq!(res.project_count, 1),
        Err(CommandError::Sync(_)) => { /* push 失败但已采集，视为通过 */ }
        Err(e) => panic!("unexpected error: {e}"),
    }

    // 无论 push 结果，snapshot.json 应该被写好（P0-I：仅 hash 不变时不写盘，首次必写）
    let snap = dashboard_state.join("machines").join(TEST_MACHINE).join("snapshot.json");
    assert!(snap.exists(), "首次 collect_single 必须落 snapshot.json");
    let snap_text = fs::read_to_string(&snap).unwrap();
    assert!(
        snap_text.contains("test-proj"),
        "snapshot 应含 project 名"
    );
}

#[test]
fn collect_single_unknown_uuid_returns_project_not_found() {
    // 未知 uuid → ProjectNotFound
    let tmp = make_tempdir("unknown-uuid");
    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some("/nonexistent"),
    );

    let bogus = Uuid::new_v4();
    let err = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        bogus,
        Trigger::Manual,
        false,
        true,
    )
    .unwrap_err();
    match err {
        CommandError::Config(dashboard_core::config::ConfigError::ProjectNotFound(u)) => {
            assert_eq!(u, bogus);
        }
        e => panic!("expected ProjectNotFound, got {e}"),
    }
}

#[test]
fn collect_single_private_uuid_never_leaks_across_registry() {
    // P0-A 回归防护：假设有人把私有项目某个 UUID 传进来，
    // collect_single 应该走 get_shared() 查不到 → ProjectNotFound。
    // 私有项目不进 shared registry，UUID 只在 local.yaml 里存（且 PrivateProject.project_id 通常是 None）。
    let tmp = make_tempdir("private-uuid");
    let (dashboard_state, local_path) = {
        let dashboard_state = tmp.join("dashboard-state");
        fs::create_dir_all(&dashboard_state).unwrap();
        git_init(&dashboard_state);
        // 决策 3：补合法结构，否则 collect_single 先判 InvalidStructure 而走不到 get_shared 查询
        fs::create_dir_all(dashboard_state.join("registry").join("projects")).unwrap();

        let local_path = tmp.join(".project-dashboard").join("local.yaml");
        fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        let mut local = LocalConfig::new(TEST_MACHINE);
        // 给一个私有项目伪造 UUID（正常场景 project_id = None，这里模拟攻击者手工塞入）
        let fake_id = Uuid::new_v4();
        local.private_projects.push(PrivateProject {
            schema_version: CURRENT_SCHEMA_VERSION,
            name: "private-secret".to_string(),
            project_id: Some(fake_id),
            path: tmp.join("private-src").to_string_lossy().to_string(),
            git_url: "https://gitlab.acme.internal/team/private-secret".to_string(),
            sync: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();
        // 三方评审收窄（2026-07-21）：挂 bare origin 让仓被判 Ready，才走到 get_shared 查询报
        // ProjectNotFound；否则无 origin → Unconfigured → collect_single no-op 返回 Ok（测不到目标）。
        attach_bare_origin(&dashboard_state, &tmp);
        (dashboard_state, local_path)
    };

    // 从 local 读回来拿伪造 UUID
    let reg = Registry::load_at(&dashboard_state, &local_path).unwrap();
    let priv_uuid = reg.list_private()[0].project_id.unwrap();

    let err = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        priv_uuid,
        Trigger::Manual,
        false,
        true,
    )
    .unwrap_err();
    match err {
        CommandError::Config(dashboard_core::config::ConfigError::ProjectNotFound(_)) => {}
        e => panic!(
            "expected ProjectNotFound (私有项目不进 shared registry)，got {e}"
        ),
    }

    // 二次断言：即便部分逻辑 bug 让 snapshot 部分写入，也必须**不含**私有名字
    let snap_path = dashboard_state.join("machines").join(TEST_MACHINE).join("snapshot.json");
    if snap_path.exists() {
        let s = fs::read_to_string(&snap_path).unwrap();
        assert!(!s.contains("private-secret"), "snapshot 不得含私有项目名");
        assert!(!s.contains("gitlab.acme.internal"), "snapshot 不得含私有 host");
    }
}

#[test]
fn collect_single_machine_missing_returns_invalid_settings() {
    // 项目在 registry 但 machines 未配置本机 → InvalidSettings
    let tmp = make_tempdir("machine-missing");
    let id = Uuid::new_v4();
    // 只给 "OTHER-MACHINE" 配路径，不给 TEST_MACHINE
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some("OTHER-MACHINE"),
        Some("/some/path"),
    );

    let err = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    )
    .unwrap_err();
    match err {
        CommandError::InvalidSettings(msg) => {
            assert!(msg.contains(TEST_MACHINE), "错误消息应含 machine_id, got: {msg}");
        }
        e => panic!("expected InvalidSettings, got {e}"),
    }
}

// v4.2 §16.5 §16.9 防伪校验语义（原 P1-H 已废：旧实现读 `.dashboard-project-id` 对比，
// v4.2 已删该文件读写）：
//   verify_remote: true → 跑 `git remote get-url origin` 对比 registry `git_url` / `git_url_aliases`
//     - 一致 → snapshot 正常写，events 无 mismatch/failed 字段
//     - 不一致 → snapshot **依然写**（不阻塞采集），events 追加 `git_remote_mismatch`
//     - 命令失败（无 origin / 非 git 仓 / 命令 stderr） → snapshot 依然写，events 追加 `git_remote_failed`

/// 布一个 sync:true 项目 + 真 git 仓（可选择性设 origin URL），返回 (dashboard_state, local_path, proj_dir)。
///
/// registry `git_url` 硬编码为 `https://github.com/example/test-repo`，与 seed_registry 保持一致。
fn seed_registry_with_project_repo(
    tag: &str,
    origin_url: Option<&str>,
) -> (PathBuf, PathBuf, PathBuf, Uuid) {
    let tmp = make_tempdir(tag);
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);
    if let Some(url) = origin_url {
        let _ = std::process::Command::new("git")
            .args(["remote", "add", "origin", url])
            .current_dir(&proj_dir)
            .output();
    }
    // registry 需要至少一次 commit 让 HEAD 有效（collect() 里 rev-parse HEAD 才有输出）
    fs::write(proj_dir.join("seed.txt"), "hi\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "seed.txt"])
        .current_dir(&proj_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(&proj_dir)
        .output();

    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "verify-remote-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );
    (dashboard_state, local_path, proj_dir, id)
}

/// 读取本次 collect_single 追加到 events.ndjson 的最后一行 —— sync push 会失败，但本地文件已落。
fn last_event_json(dashboard_state: &Path) -> Option<serde_json::Value> {
    let events_path = dashboard_state
        .join("machines")
        .join(TEST_MACHINE)
        .join("events.ndjson");
    let text = fs::read_to_string(&events_path).ok()?;
    let last = text.trim().lines().last()?.to_string();
    serde_json::from_str(&last).ok()
}

#[test]
fn collect_single_verify_remote_ok_when_origin_matches_registry() {
    // origin URL 归一化后 = registry.git_url —— events 无 mismatch/failed 字段
    let (dashboard_state, local_path, _proj_dir, id) = seed_registry_with_project_repo(
        "verify-ok",
        Some("https://github.com/example/test-repo"),
    );
    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        true, // verify_remote 开
        true,
    );
    // push 无 remote 会失败但本地文件已落
    assert!(matches!(r, Ok(_) | Err(CommandError::Sync(_))), "got {r:?}");

    let evt = last_event_json(&dashboard_state).expect("events.ndjson 应有条目");
    assert!(
        evt.get("git_remote_mismatch").is_none(),
        "URL 一致时不应出现 git_remote_mismatch 字段, got: {evt}"
    );
    assert!(
        evt.get("git_remote_failed").is_none(),
        "URL 一致时不应出现 git_remote_failed 字段, got: {evt}"
    );
}

#[test]
fn collect_single_verify_remote_mismatch_writes_event_but_still_persists_snapshot() {
    // origin 指向别的 repo —— events 追加 git_remote_mismatch；snapshot 依然写入
    let (dashboard_state, local_path, _proj_dir, id) = seed_registry_with_project_repo(
        "verify-mismatch",
        Some("https://github.com/other-org/wrong-repo"),
    );
    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        true,
        true,
    );
    assert!(matches!(r, Ok(_) | Err(CommandError::Sync(_))), "got {r:?}");

    let evt = last_event_json(&dashboard_state).expect("events.ndjson 应有条目");
    let mismatch = evt
        .get("git_remote_mismatch")
        .expect(&format!("URL 不一致应追加 git_remote_mismatch 字段, got: {evt}"));
    let expected = mismatch.get("expected").and_then(|v| v.as_str()).unwrap_or("");
    let actual = mismatch.get("actual").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(expected, "https://github.com/example/test-repo");
    assert_eq!(actual, "https://github.com/other-org/wrong-repo");

    // snapshot 依然写入（不阻塞采集），且 project_id 是 iterator 传入的 UUID
    let snap = fs::read_to_string(
        dashboard_state.join("machines").join(TEST_MACHINE).join("snapshot.json"),
    )
    .expect("snapshot.json 应存在（防伪校验不阻塞）");
    assert!(
        snap.contains(&id.to_string()),
        "snapshot 应含 iterator 传入的 UUID, got: {snap}"
    );
}

#[test]
fn collect_single_verify_remote_failed_when_no_origin_configured() {
    // 项目路径是 git 仓但没配 origin —— events 追加 git_remote_failed；snapshot 依然写入
    let (dashboard_state, local_path, _proj_dir, id) =
        seed_registry_with_project_repo("verify-noorigin", None);
    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        true,
        true,
    );
    assert!(matches!(r, Ok(_) | Err(CommandError::Sync(_))), "got {r:?}");

    let evt = last_event_json(&dashboard_state).expect("events.ndjson 应有条目");
    assert!(
        evt.get("git_remote_mismatch").is_none(),
        "命令失败场景不应报 mismatch, got: {evt}"
    );
    let failed = evt.get("git_remote_failed").expect(&format!(
        "无 origin 应追加 git_remote_failed 字段, got: {evt}"
    ));
    assert!(
        failed.as_str().is_some(),
        "git_remote_failed 应为字符串（诊断原因）, got: {failed:?}"
    );

    let snap = fs::read_to_string(
        dashboard_state.join("machines").join(TEST_MACHINE).join("snapshot.json"),
    )
    .expect("snapshot.json 应存在（防伪校验不阻塞）");
    assert!(snap.contains(&id.to_string()));
}

#[test]
fn collect_single_verify_remote_off_skips_check_regardless_of_origin() {
    // verify_remote=false → 不跑 shell out，events 里无诊断字段（不管 origin 对不对）
    let (dashboard_state, local_path, _proj_dir, id) = seed_registry_with_project_repo(
        "verify-off",
        Some("https://github.com/other-org/wrong-repo"),
    );
    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    );
    assert!(matches!(r, Ok(_) | Err(CommandError::Sync(_))), "got {r:?}");

    let evt = last_event_json(&dashboard_state).expect("events.ndjson 应有条目");
    assert!(evt.get("git_remote_mismatch").is_none(), "verify_remote=false 时不应做校验");
    assert!(evt.get("git_remote_failed").is_none(), "verify_remote=false 时不应做校验");
}

#[test]
fn collect_single_pending_repo_without_project_id_passes_p1h() {
    // P1-H 兼容性：pending 项目 `.dashboard-project-id` 未写入 → status.project_id = None → 跳过校验
    let tmp = make_tempdir("pending-repo");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);
    // 故意不写 .dashboard-project-id

    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "pending-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );

    let r = commands::collect_single(&dashboard_state, &local_path, TEST_MACHINE, id, Trigger::Manual, false, true);
    // pending 场景应该走成功路径（可能因 push 失败拿到 Sync error 但采集本身不该被 P1-H 校验拦截）
    match r {
        Ok(_) | Err(CommandError::Sync(_)) => { /* OK */ }
        Err(e) => panic!(
            "pending 项目（.dashboard-project-id 未写）不该被 P1-H 校验拦截, got {e}"
        ),
    }
}

#[test]
fn collect_single_path_not_dir_returns_invalid_settings() {
    // 项目在 registry 且 machines 有本机 key，但路径不存在 → InvalidSettings
    let tmp = make_tempdir("path-missing");
    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(tmp.join("does-not-exist").to_string_lossy().as_ref()),
    );

    let err = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    )
    .unwrap_err();
    match err {
        CommandError::InvalidSettings(msg) => {
            assert!(msg.contains("路径不存在"), "错误消息应含'路径不存在', got: {msg}");
        }
        e => panic!("expected InvalidSettings, got {e}"),
    }
}

// ============================================================================
// v4.3 §1.4 R2-P0-B：collect_single 采用 read → merge → write 语义
// ============================================================================

/// 单项目采集不能让 snapshot 里的其他项目暂时消失
///
/// 场景：本机 snapshot 已含 A + B → 只对 B 触发 collect_single → snapshot 应仍含 A + 新 B。
/// 原实现（v4.2）用 `vec![单项]` 覆盖，A 从 snapshot 里被抹掉。
#[test]
fn v43_collect_single_preserves_other_projects_in_snapshot() {
    let tmp = make_tempdir("v43-merge-preserve");

    // 布 A 和 B 两个源目录
    let a_dir = tmp.join("proj-A");
    fs::create_dir_all(&a_dir).unwrap();
    git_init(&a_dir);
    let b_dir = tmp.join("proj-B");
    fs::create_dir_all(&b_dir).unwrap();
    git_init(&b_dir);

    let a_id = Uuid::new_v4();
    let b_id = Uuid::new_v4();
    // seed_registry(A) 建立 dashboard_state + registry/A
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        a_id,
        "proj-A",
        Some(TEST_MACHINE),
        Some(a_dir.to_string_lossy().as_ref()),
    );
    // 追加 registry/B（复用相同 dashboard_state 目录 / local.yaml）
    let mut b_machines = BTreeMap::new();
    b_machines.insert(TEST_MACHINE.to_string(), b_dir.to_string_lossy().to_string());
    let b_reg = RegistryProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        project_id: b_id,
        name: "proj-B".to_string(),
        git_url: "https://github.com/example/test-repo-b".to_string(),
        git_url_aliases: Vec::new(),
        sync: true,
        machines: b_machines,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let projects_dir = dashboard_state.join("registry").join("projects");
    fs::write(
        projects_dir.join(format!("{b_id}.yaml")),
        serde_yaml::to_string(&b_reg).unwrap(),
    )
    .unwrap();

    // 第一次采集 A → snapshot 含 A
    let _ = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        a_id,
        Trigger::Manual,
        false,
        true,
    );
    let snap_path = dashboard_state
        .join("machines")
        .join(TEST_MACHINE)
        .join("snapshot.json");
    let after_a = fs::read_to_string(&snap_path).unwrap();
    assert!(after_a.contains("proj-A"), "第一次 collect_single(A) 后 snapshot 必须含 A");
    assert!(!after_a.contains("proj-B"), "此时 B 尚未采集");

    // 第二次只采集 B → snapshot 应含 A + B（**merge 语义**），不能只剩 B
    let _ = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        b_id,
        Trigger::Manual,
        false,
        true,
    );
    let after_b = fs::read_to_string(&snap_path).unwrap();
    assert!(after_b.contains("proj-A"), "R2-P0-B 契约：collect_single(B) 后 A 必须仍在 snapshot 里");
    assert!(after_b.contains("proj-B"), "collect_single(B) 后新 B 也必须在");
}

/// 单项目采集第二次调用只更新 slot（同 project_id）而不重复 push
#[test]
fn v43_collect_single_updates_existing_slot_by_project_id() {
    let tmp = make_tempdir("v43-merge-update");
    let a_dir = tmp.join("proj-a");
    fs::create_dir_all(&a_dir).unwrap();
    git_init(&a_dir);
    // commit 一次让 A 有真 HEAD
    fs::write(a_dir.join("f.txt"), "one\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["-C", a_dir.to_str().unwrap(), "add", "f.txt"])
        .output();
    let _ = std::process::Command::new("git")
        .args(["-C", a_dir.to_str().unwrap(), "commit", "-q", "-m", "one"])
        .output();

    let a_id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        a_id,
        "proj-a",
        Some(TEST_MACHINE),
        Some(a_dir.to_string_lossy().as_ref()),
    );

    let _ = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        a_id,
        Trigger::Manual,
        false,
        true,
    );

    let snap_path = dashboard_state.join("machines").join(TEST_MACHINE).join("snapshot.json");
    let text_before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&snap_path).unwrap()).unwrap();
    let before_projects = text_before["projects"].as_array().unwrap();
    assert_eq!(before_projects.len(), 1, "第一次采集后应只有 1 个项目");

    // 追加一个 commit 让 head 变
    fs::write(a_dir.join("f.txt"), "two\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["-C", a_dir.to_str().unwrap(), "add", "f.txt"])
        .output();
    let _ = std::process::Command::new("git")
        .args(["-C", a_dir.to_str().unwrap(), "commit", "-q", "-m", "two"])
        .output();

    // 第二次采集 A → 应更新 slot，不是 push 新条目
    let _ = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        a_id,
        Trigger::Manual,
        false,
        true,
    );
    let text_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&snap_path).unwrap()).unwrap();
    let after_projects = text_after["projects"].as_array().unwrap();
    assert_eq!(after_projects.len(), 1, "第二次采集同 project_id 应更新 slot，不复制成 2 条");
}

// ============================================================================
// v4.3 P0-3：read_snapshot 区分「不存在」与「读取失败」，
// 损坏 / 错位的 snapshot 必须中止同步、保留原文件，绝不退空 Vec 覆盖整机其他项目。
// ============================================================================

/// snapshot.json 存在但结构损坏（缺必需字段 / 类型错）时，collect_single 必须返回 Err
/// 中止，且磁盘上的原文件原样保留（不被空 Vec 覆盖成只含当前单项）。
#[test]
fn v43_collect_single_corrupt_snapshot_aborts_without_overwrite() {
    let tmp = make_tempdir("v43-corrupt-snapshot");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);

    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );

    // 预置一份"损坏"snapshot：合法 JSON 但结构不符（projects 是字符串而非数组、缺必需字段）
    let machine_dir = dashboard_state.join("machines").join(TEST_MACHINE);
    fs::create_dir_all(&machine_dir).unwrap();
    let corrupt = r#"{"this":"is not a valid snapshot","projects":"should be an array"}"#;
    let snap_path = machine_dir.join("snapshot.json");
    fs::write(&snap_path, corrupt).unwrap();

    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    );
    assert!(
        matches!(r, Err(CommandError::Sync(_))),
        "损坏 snapshot 必须让 collect_single 返回 Sync 错误中止，got {r:?}"
    );
    // 关键契约：原损坏文件原样保留 —— 没有被空 Vec 起手的新 snapshot 覆盖
    let after = fs::read_to_string(&snap_path).unwrap();
    assert_eq!(after, corrupt, "损坏 snapshot 必须原样保留，不能被覆盖");
}

/// snapshot.json 合法但 machine_id 指向别的机器（文件错位 / 污染）时，collect_single
/// 必须中止而非拿别机数据 merge，且原文件保留。
#[test]
fn v43_collect_single_foreign_machine_id_aborts_without_overwrite() {
    let tmp = make_tempdir("v43-foreign-machine");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);

    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );

    // 预置一份结构合法、但 machine_id 是别的机器的 snapshot
    let machine_dir = dashboard_state.join("machines").join(TEST_MACHINE);
    fs::create_dir_all(&machine_dir).unwrap();
    let foreign = r#"{"schema_version":1,"machine_id":"SOME-OTHER-MACHINE","updated_at":"2026-07-11T15:00:00Z","projects":[]}"#;
    let snap_path = machine_dir.join("snapshot.json");
    fs::write(&snap_path, foreign).unwrap();

    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    );
    assert!(
        matches!(r, Err(CommandError::Sync(_))),
        "machine_id 不匹配必须让 collect_single 返回 Sync 错误中止，got {r:?}"
    );
    let after = fs::read_to_string(&snap_path).unwrap();
    assert_eq!(after, foreign, "machine_id 错位的 snapshot 必须原样保留，不能被覆盖");
}

// ============================================================================
// P0-3-b fail-closed 闭环 + P0-5 update_settings 纳入互斥
// ============================================================================

/// manual_sync（全量重建 + force=true）遇旧 snapshot 含未来未知字段时，必须 read_snapshot
/// fail-closed 中止、不覆盖，避免静默抹掉别机新数据。
#[test]
fn v43_manual_sync_unknown_field_snapshot_aborts_without_overwrite() {
    let tmp = make_tempdir("v43-manual-unknown");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);
    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );
    let machine_dir = dashboard_state.join("machines").join(TEST_MACHINE);
    fs::create_dir_all(&machine_dir).unwrap();
    let with_unknown = r#"{"schema_version":1,"machine_id":"TEST-MACHINE-COLLECT","updated_at":"2026-07-11T15:00:00Z","projects":[],"future_field":"x"}"#;
    let snap_path = machine_dir.join("snapshot.json");
    fs::write(&snap_path, with_unknown).unwrap();

    let r = commands::manual_sync(&dashboard_state, &local_path, TEST_MACHINE, Trigger::Manual, true);
    assert!(
        matches!(r, Err(CommandError::Sync(_))),
        "含未知字段的旧 snapshot 必须让 manual_sync fail-closed 中止，got {r:?}"
    );
    let after = fs::read_to_string(&snap_path).unwrap();
    assert_eq!(after, with_unknown, "含未知字段 snapshot 必须原样保留、不被 force 覆盖");
}

/// collect_single 遇旧 snapshot 里 `progress` 层未知字段时也 fail-closed（Progress
/// deny_unknown_fields），中止不覆盖。
#[test]
fn v43_collect_single_progress_unknown_field_aborts_without_overwrite() {
    let tmp = make_tempdir("v43-progress-unknown");
    let proj_dir = tmp.join("proj-src");
    fs::create_dir_all(&proj_dir).unwrap();
    git_init(&proj_dir);
    let id = Uuid::new_v4();
    let (dashboard_state, local_path) = seed_registry(
        &tmp,
        id,
        "test-proj",
        Some(TEST_MACHINE),
        Some(proj_dir.to_string_lossy().as_ref()),
    );
    let machine_dir = dashboard_state.join("machines").join(TEST_MACHINE);
    fs::create_dir_all(&machine_dir).unwrap();
    let with_progress_unknown = r#"{"schema_version":1,"machine_id":"TEST-MACHINE-COLLECT","updated_at":"2026-07-11T15:00:00Z","projects":[{"project_id":null,"name":"p","branch":null,"head":null,"upstream":null,"ahead":null,"behind":null,"dirty":0,"untracked":0,"last_commit_at":null,"last_fs_change_at":null,"collected_at":"2026-07-11T15:00:00Z","progress":{"done":1,"total":2,"next":"x","future_field":"y"},"dangerous_state":null}]}"#;
    let snap_path = machine_dir.join("snapshot.json");
    fs::write(&snap_path, with_progress_unknown).unwrap();

    let r = commands::collect_single(
        &dashboard_state,
        &local_path,
        TEST_MACHINE,
        id,
        Trigger::Manual,
        false,
        true,
    );
    assert!(
        matches!(r, Err(CommandError::Sync(_))),
        "progress 层未知字段必须让 collect_single fail-closed 中止，got {r:?}"
    );
    let after = fs::read_to_string(&snap_path).unwrap();
    assert_eq!(after, with_progress_unknown, "含 progress 未知字段 snapshot 必须原样保留");
}

/// update_settings 纳入 repo 锁后仍能正常改设置（单次无竞争路径），且 acquire_lock 的
/// .git 守卫在正常 clone（.git 存在）下放行。
#[test]
fn v43_update_settings_works_under_repo_lock() {
    let tmp = make_tempdir("v43-update-settings");
    let dashboard_state = tmp.join("dashboard-state");
    fs::create_dir_all(&dashboard_state).unwrap();
    git_init(&dashboard_state); // acquire_lock 守卫要求 .git 存在
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let local = LocalConfig::new(TEST_MACHINE);
    fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();

    let r = commands::update_settings(
        &dashboard_state,
        &local_path,
        commands::SettingsUpdate {
            sync_tick_minutes: Some(10),
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    );
    assert!(r.is_ok(), "update_settings 纳入锁后应正常工作，got {r:?}");
    assert_eq!(r.unwrap().sync_tick_minutes, 10);
}
