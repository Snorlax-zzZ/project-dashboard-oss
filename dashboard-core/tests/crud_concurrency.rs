// first-run（dashboard-state 未 clone、无 .git）下 CRUD 并发数据完整性 —— P1 行为级验证。
//
// sync_lock.rs 证明 sidecar 锁互斥（锁原语层）；本测试证明 CRUD 真的走了锁、且并发后
// local.yaml 的两处独立改动（private_projects 与 settings）都不丢（行为层）。
// 修复前（best-effort 锁在 first-run 放行）：无锁并发 → 后写者用自己的旧快照整份覆盖
// local.yaml → 必丢其一。修复后（sidecar allow 锁）：串行 read-modify-write，两者都保留。

use dashboard_core::commands::{
    add_project, get_settings, update_settings, AddProjectRequest, SettingsUpdate,
};
use dashboard_core::config::{LocalConfig, Registry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-crud-concurrency-{}-{}-{}",
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

fn priv_req(name: &str) -> AddProjectRequest {
    AddProjectRequest {
        name: name.to_string(),
        git_url: format!("https://github.com/test/{name}"),
        path: format!("/tmp/{name}"),
        machines: BTreeMap::new(),
        sync: false,
        confirm_dual_form: false,
    }
}

#[test]
fn first_run_concurrent_add_and_update_settings_preserve_both() {
    let tmp = make_tempdir("add-vs-settings");
    let dashboard_state = tmp.join("dashboard-state"); // 不 clone → 无 .git（first-run）
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    assert!(!dashboard_state.join(".git").exists(), "前置：first-run 无 .git");

    // 先建立初始 local.yaml（含私有项目 p0 + 默认 settings），制造后续两个操作的
    // read-modify-write 覆盖窗口。
    add_project(&dashboard_state, &local_path, priv_req("p0")).expect("初始 add 应成功");
    assert!(local_path.is_file(), "初始 local.yaml 应已建立");

    // 并发：T1 追加私有项目 p1；T2 改 settings.sync_tick_minutes=42。
    // 两者都 read-modify-write 同一份 local.yaml 的不同字段，sidecar 锁负责串行化。
    let barrier = Arc::new(Barrier::new(2));

    let (ds1, lp1, b1) = (dashboard_state.clone(), local_path.clone(), barrier.clone());
    let t1 = thread::spawn(move || {
        b1.wait();
        add_project(&ds1, &lp1, priv_req("p1"))
    });

    let (ds2, lp2, b2) = (dashboard_state.clone(), local_path.clone(), barrier.clone());
    let t2 = thread::spawn(move || {
        b2.wait();
        update_settings(
            &ds2,
            &lp2,
            SettingsUpdate {
                sync_tick_minutes: Some(42),
                diagnostic_mode: None,
                dashboard_state_remote: None,
                dashboard_state_root: None,
                repowiki_cli_path: None,
                repowiki_data_root: None,
            },
        )
    });

    let r1 = t1.join().expect("T1 线程 join");
    let r2 = t2.join().expect("T2 线程 join");
    assert!(r1.is_ok(), "并发 add 应成功（锁串行不报错），got {r1:?}");
    assert!(r2.is_ok(), "并发 update_settings 应成功，got {r2:?}");

    // 无论线程调度先后，锁串行都保证最终两处改动都在（后者的 load 读到前者已落盘的结果）。
    let reloaded = Registry::load_at(&dashboard_state, &local_path).unwrap();
    let names: Vec<&str> = reloaded
        .list_private()
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"p0"), "初始 p0 应保留，实际 {names:?}");
    assert!(
        names.contains(&"p1"),
        "并发追加的 p1 不能丢（无锁时会被 settings 写整份覆盖），实际 {names:?}"
    );
    let settings = get_settings(&local_path).unwrap();
    assert_eq!(
        settings.sync_tick_minutes, 42,
        "并发改的 settings 不能丢（无锁时会被 add 写整份覆盖）"
    );
}

#[test]
fn concurrent_writes_with_old_and_new_state_roots_preserve_local_config() {
    let tmp = make_tempdir("cross-root-local-lock");
    let old_root = tmp.join("dashboard-state-old");
    let new_root = tmp.join("dashboard-state-new");
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    add_project(&old_root, &local_path, priv_req("p0")).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let (old_for_add, local_for_add, add_barrier) =
        (old_root.clone(), local_path.clone(), barrier.clone());
    let add = thread::spawn(move || {
        add_barrier.wait();
        add_project(&old_for_add, &local_for_add, priv_req("p1"))
    });
    let (new_for_settings, local_for_settings, settings_barrier) =
        (new_root, local_path.clone(), barrier);
    let settings = thread::spawn(move || {
        settings_barrier.wait();
        update_settings(
            &new_for_settings,
            &local_for_settings,
            SettingsUpdate {
                sync_tick_minutes: Some(37),
                diagnostic_mode: None,
                dashboard_state_remote: None,
                dashboard_state_root: None,
                repowiki_cli_path: None,
                repowiki_data_root: None,
            },
        )
    });

    add.join().unwrap().unwrap();
    settings.join().unwrap().unwrap();
    let reloaded = Registry::load_at(&old_root, &local_path).unwrap();
    let names = reloaded
        .list_private()
        .iter()
        .map(|project| project.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"p0"));
    assert!(names.contains(&"p1"));
    assert_eq!(get_settings(&local_path).unwrap().sync_tick_minutes, 37);
}

/// Q2-A 测试用：建初始 local.yaml（默认 settings），返回 (dashboard_state_root, local_path)。
/// dashboard_state 目录不 clone（first-run 无 .git），update_settings 走宽松锁仍可运行。
fn seed_local(tmp: &Path) -> (PathBuf, PathBuf) {
    let dashboard_state = tmp.join("dashboard-state");
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let cfg = LocalConfig::new("TEST-MACHINE");
    std::fs::write(&local_path, serde_yaml::to_string(&cfg).unwrap()).unwrap();
    (dashboard_state, local_path)
}

/// Q2-A：update_settings 对 dashboard_state_remote / dashboard_state_root 的三态语义——
/// 未传（None）保持原值；Some(Some(x)) 设值；Some(None) / 空白串清空。
#[test]
fn settings_update_tri_state() {
    let tmp = make_tempdir("settings-tri-state");
    let (ds, lp) = seed_local(&tmp);

    // 设值
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(Some("git@host:team/ds.git".to_string())),
            dashboard_state_root: Some(Some("/data/ds".to_string())),
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(r.dashboard_state_remote.as_deref(), Some("git@host:team/ds.git"));
    assert_eq!(r.dashboard_state_root.as_deref(), Some("/data/ds"));

    // 未传：只改 tick，remote/root 保持原值
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: Some(7),
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(r.sync_tick_minutes, 7);
    assert_eq!(
        r.dashboard_state_remote.as_deref(),
        Some("git@host:team/ds.git"),
        "未传的 remote 应保持原值"
    );
    assert_eq!(r.dashboard_state_root.as_deref(), Some("/data/ds"), "未传的 root 应保持原值");

    // 清空 remote：Some(None)
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(None),
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(r.dashboard_state_remote, None, "Some(None) 应清空 remote");
    assert_eq!(
        r.dashboard_state_root.as_deref(),
        Some("/data/ds"),
        "未传的 root 不受清空 remote 影响"
    );

    // 空白串清空 root：Some(Some("  "))
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: Some(Some("  ".to_string())),
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(r.dashboard_state_root, None, "空白串应视同清空 root");
}

/// Q2-A：dashboard_state_remote 存前拒内嵌凭证/token（防泄露进 local.yaml）；拒绝后不落盘。
#[test]
fn remote_rejects_inline_token() {
    let tmp = make_tempdir("remote-reject-token");
    let (ds, lp) = seed_local(&tmp);

    // user:token@host 形式
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(Some(
                "https://user:token@host.internal/team/ds.git".to_string(),
            )),
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    );
    assert!(r.is_err(), "内嵌 user:token 的 remote 应被拒");
    assert_eq!(
        get_settings(&lp).unwrap().dashboard_state_remote,
        None,
        "拒绝路径不应把 remote 写进 local.yaml"
    );
    // LEAK-06：拒绝错误不得回显输入里的敏感段（密码 token / host），否则随错误暴露到 UI/日志。
    let msg = format!("{}", r.as_ref().unwrap_err());
    assert!(!msg.contains("token"), "错误不应回显输入的密码/token：{msg}");
    assert!(!msg.contains("host.internal"), "错误不应回显输入的 host：{msg}");

    // 单串 token（x-token-auth@host）形式
    let r2 = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(Some("https://x-token-auth@host/team/ds.git".to_string())),
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    );
    assert!(r2.is_err(), "单串 token 也应被拒");
    let msg2 = format!("{}", r2.as_ref().unwrap_err());
    assert!(!msg2.contains("x-token-auth"), "错误不应回显输入的 userinfo：{msg2}");
}

/// Q2-A：合法 remote 存**原始不归一化**（保留 git@/.git/端口/大小写），落盘读回逐字节一致。
#[test]
fn remote_preserves_raw() {
    let tmp = make_tempdir("remote-preserve-raw");
    let (ds, lp) = seed_local(&tmp);
    let raw = "git@Host.Internal:Team/Dashboard-State.git";
    let r = update_settings(
        &ds,
        &lp,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(Some(raw.to_string())),
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(r.dashboard_state_remote.as_deref(), Some(raw), "remote 应存原始不归一化");
    assert_eq!(
        get_settings(&lp).unwrap().dashboard_state_remote.as_deref(),
        Some(raw),
        "落盘读回仍原样"
    );
}
