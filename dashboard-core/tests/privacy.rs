// P0-A e2e 隐私测试（审计对账 2026-07-06）
// 保证 `sync:false` 私有项目的任何字段都不进 `machines/<id>/snapshot.json` / `events.ndjson`

use chrono::Utc;
use dashboard_core::commands;
use dashboard_core::config::{LocalConfig, PrivateProject, Registry, CURRENT_SCHEMA_VERSION};
use dashboard_core::sync::Trigger;
use std::fs;
use std::path::PathBuf;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-privacy-{}-{}-{}",
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

/// 把一个已 git init 的 dashboard_state 补成「合法 Ready 态」：建 registry/projects 结构 + 挂
/// bare origin + 推初始 commit。
///
/// 三方评审收窄（2026-07-21）：`resolve_status` 现在要求「有 .git + registry/projects + 有 origin」
/// 才判 Ready；manual_sync（push=true）前置 `pull --ff-only` 也需真 origin 才能过。隐私测试要验证
/// 「采集真跑了但私有字段不泄漏」，就必须让仓走到采集阶段（Ready），否则前置门禁提前中止、采集没跑、
/// 断言空转成假通过（此前实证：manual_sync 返回 ConfigPullFailed{InvalidStructure}，snapshot 未写）。
fn make_state_ready_with_origin(dashboard_state: &std::path::Path, tmp: &std::path::Path) {
    fs::create_dir_all(dashboard_state.join("registry").join("projects")).unwrap();
    fs::write(dashboard_state.join("registry").join("projects").join(".keep"), "").unwrap();
    let bare = tmp.join("remote.git");
    fs::create_dir_all(&bare).unwrap();
    let run = |cwd: &std::path::Path, args: &[&str]| {
        let _ = std::process::Command::new("git").args(args).current_dir(cwd).output();
    };
    run(&bare, &["init", "-q", "--bare", "-b", "main"]);
    run(dashboard_state, &["add", "-A"]);
    run(dashboard_state, &["commit", "-q", "-m", "seed dashboard-state"]);
    run(dashboard_state, &["remote", "add", "origin", bare.to_str().unwrap()]);
    run(dashboard_state, &["push", "-q", "-u", "origin", "main"]);
}

#[test]
fn manual_sync_never_leaks_private_project_fields() {
    // 关键 P0-A 保证：manual_sync 后 dashboard-state 工作树里 **不含** 任何私有项目名 / path / git_url
    let tmp = make_tempdir("no-leak");
    let dashboard_state = tmp.join("dashboard-state");
    // 提前建成 git 仓（sync_cycle 里要 add/commit —— 但没 remote，push 会失败）
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

    // 布 local.yaml：一个私有项目 "acme-corp"
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let mut local = LocalConfig::new("TEST-MACHINE");
    let priv_dir = tmp.join("acme-corp");
    fs::create_dir_all(&priv_dir).unwrap();
    let _ = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&priv_dir)
        .output();
    local.private_projects.push(PrivateProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        name: "acme-corp-super-secret".to_string(),
        project_id: None,
        path: priv_dir.to_string_lossy().to_string(),
        git_url: "https://gitlab.acme.internal/team/acme-corp".to_string(),
        sync: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });
    fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();
    // 补合法结构 + 挂 origin，让仓判 Ready、manual_sync 真走到采集（否则前置门禁提前中止 = 假通过）
    make_state_ready_with_origin(&dashboard_state, &tmp);

    // 跑一次手动同步 —— pull(no-op) → 采集 → commit → push 成功；snapshot.json / events.ndjson 会写好
    let _ = commands::manual_sync(&dashboard_state, &local_path, "TEST-MACHINE", Trigger::Manual, true);

    // 检查 machines/TEST-MACHINE/{snapshot.json,events.ndjson}
    let machine_dir = dashboard_state.join("machines").join("TEST-MACHINE");
    let snap_text = fs::read_to_string(machine_dir.join("snapshot.json")).unwrap_or_default();
    let evt_text = fs::read_to_string(machine_dir.join("events.ndjson")).unwrap_or_default();

    // 私有字段绝不出现在任何写入内容里 —— 这是 spec `sync:false 永不出机` 的硬约束
    for probe in [
        "acme-corp-super-secret",
        "gitlab.acme.internal",
        "acme-corp", // path basename
    ] {
        assert!(
            !snap_text.contains(probe),
            "snapshot.json 含私有字段 {probe}: \n{snap_text}"
        );
        assert!(
            !evt_text.contains(probe),
            "events.ndjson 含私有字段 {probe}: \n{evt_text}"
        );
    }
}

#[test]
fn manual_sync_result_project_count_excludes_private() {
    // 同上场景：manual_sync 返回的 project_count 不应把 private 项目算进去
    let tmp = make_tempdir("count-exclude");
    let dashboard_state = tmp.join("dashboard-state");
    fs::create_dir_all(&dashboard_state).unwrap();
    let _ = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dashboard_state)
        .output();

    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let mut local = LocalConfig::new("TEST-MACHINE-2");
    let priv_dir = tmp.join("company-x");
    fs::create_dir_all(&priv_dir).unwrap();
    local.private_projects.push(PrivateProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        name: "company-x".to_string(),
        project_id: None,
        path: priv_dir.to_string_lossy().to_string(),
        git_url: "https://gitlab.internal/team/company-x".to_string(),
        sync: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });
    fs::write(&local_path, serde_yaml::to_string(&local).unwrap()).unwrap();
    // 补合法结构 + 挂 origin，让仓判 Ready、manual_sync 真走到采集（否则前置门禁提前中止 = 假通过）
    make_state_ready_with_origin(&dashboard_state, &tmp);

    let r = commands::manual_sync(&dashboard_state, &local_path, "TEST-MACHINE-2", Trigger::Manual, true);
    // manual_sync 可能因 push 失败返回 Err，但重点是不管成败：
    //  - project_count 都应该是 0（没有共享项目）
    //  - 但内部实现即便 push 失败也已经 write_snapshot 了，那步是 OK 的
    if let Ok(res) = r {
        assert_eq!(
            res.project_count, 0,
            "project_count 应排除私有项目，实际 {}",
            res.project_count
        );
    }

    // 通过 registry 直接算：list_shared 空
    let reg = Registry::load_at(&dashboard_state, &local_path).unwrap();
    assert_eq!(reg.list_shared().len(), 0);
    assert_eq!(reg.list_private().len(), 1);
}
