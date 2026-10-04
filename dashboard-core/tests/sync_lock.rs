// P0-5 repo-global 同步锁集成测试。
//
// 验证 std::fs::File OS advisory 锁（Rust 1.89+）的排他性与 RAII 释放：
// - 持锁期间另一实例（模拟另一进程）取锁应超时 LockBusy；
// - 锁只依赖 root、不依赖 machine_id（repo-global，collect_single 用真 id、pull_registry 用
//   占位符 id 必须互斥）；
// - RepoLock Drop 后锁立即释放。
//
// 注：同进程内两个不同 File fd 对同一路径 flock 互斥（per-open-file-description 语义），
// 足以在单进程测试里覆盖跨进程排他契约。

use dashboard_core::sync::{DashboardStateRepo, SyncError};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

fn make_repo_root(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-sync-lock-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // acquire_lock 要求 .git 已存在（模拟 clone 完成的 dashboard-state 工作区）
    fs::create_dir_all(base.join(".git")).unwrap();
    base
}

#[test]
fn repo_lock_is_exclusive_and_times_out() {
    let root = make_repo_root("exclusive");
    let repo = DashboardStateRepo::new(&root, "M");
    let lock1 = repo.acquire_lock(Duration::from_secs(1)).expect("首次取锁应成功");

    // 持锁期间第二次取锁（模拟另一进程）应在超时后返回 LockBusy
    let r = repo.acquire_lock(Duration::from_millis(200));
    assert!(
        matches!(r, Err(SyncError::LockBusy)),
        "持锁期间第二次取锁应 LockBusy，got {r:?}"
    );

    drop(lock1);
    // 释放后应能取到
    let r2 = repo.acquire_lock(Duration::from_millis(500));
    assert!(r2.is_ok(), "释放后应能取到锁，got {r2:?}");
}

#[test]
fn repo_lock_is_repo_global_not_machine_scoped() {
    // repo-global 关键契约：锁只依赖 root，不依赖 machine_id。
    // collect_single 用真 machine_id、pull_registry 用 "_pull_only_" 占位符，二者必须互斥，
    // 否则采集与 pull 之间的 race 依旧存在。
    let root = make_repo_root("shared");
    let repo_real = DashboardStateRepo::new(&root, "REAL-MACHINE");
    let repo_pull = DashboardStateRepo::new(&root, "_pull_only_");

    let _lock = repo_real.acquire_lock(Duration::from_secs(1)).expect("real 取锁成功");
    let r = repo_pull.acquire_lock(Duration::from_millis(200));
    assert!(
        matches!(r, Err(SyncError::LockBusy)),
        "同 root 不同 machine_id 必须共享同一把锁（repo-global），got {r:?}"
    );
}

#[test]
fn repo_lock_released_on_drop() {
    let root = make_repo_root("drop");
    let repo = DashboardStateRepo::new(&root, "M");
    {
        let _l = repo.acquire_lock(Duration::from_secs(1)).unwrap();
    } // 作用域结束 Drop 释放
    let r = repo.acquire_lock(Duration::from_millis(100));
    assert!(r.is_ok(), "Drop 后锁应立即释放可再取，got {r:?}");
}

// ---- sidecar 锁：first-run（未 clone）+ clone 后互斥补测 ----
// 锁文件放 `<root>.sync.lock`（root 的兄弟），不依赖 .git 是否存在，first-run 与 clone 后
// 所有入口锁同一文件。以下覆盖：无 .git 宽松锁排他 / 无 .git 严格锁仍 NotFound /
// clone 后两类入口共锁互斥 / 宽松锁 Drop 释放。

/// first-run 专用 root：父目录在 temp（存在），但 root 本身与其 `.git` 都不存在，
/// 模拟 dashboard-state 尚未 clone。sidecar 锁落在 `<root>.sync.lock`（root 的兄弟），
/// 只需 root 的父目录存在即可创建，不依赖 root/.git。
fn make_uninit_root(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-sync-lock-uninit-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    base
}

#[test]
fn allow_uninit_locks_without_git_and_is_exclusive() {
    // P1 核心回归：first-run 无 .git 时 CRUD 宽松锁仍能取到，且对同 root 排他 ——
    // 堵住 first-run 下 CRUD↔CRUD 对 local.yaml 的并发覆盖（旧 best-effort 版此处返 None
    // 放行，正是本次 P1 根因）。
    let root = make_uninit_root("allow-excl");
    assert!(!root.join(".git").exists(), "前置：root 无 .git（未 clone）");
    let repo = DashboardStateRepo::new(&root, "_crud_only_");

    let lock1 = repo
        .acquire_lock_allow_uninit(Duration::from_secs(1))
        .expect("first-run 无 .git 时宽松锁必须能取到");

    // 第二个 CRUD（模拟并发进程）取同一把 sidecar 锁应超时 LockBusy
    let r = repo.acquire_lock_allow_uninit(Duration::from_millis(200));
    assert!(
        matches!(r, Err(SyncError::LockBusy)),
        "无 .git 下第二个 CRUD 取锁应 LockBusy（证明 first-run CRUD 互斥），got {r:?}"
    );

    drop(lock1);
    let r2 = repo.acquire_lock_allow_uninit(Duration::from_millis(500));
    assert!(r2.is_ok(), "释放后应能再取，got {r2:?}");
}

#[test]
fn strict_lock_still_notfound_without_git() {
    // 采集 / pull 的严格锁：无 .git 必须 Err(NotFound)，不能在假仓上继续写。
    // 方向 C 下先取 sidecar 锁再验 .git，缺失仍要报 NotFound（语义不变）。
    let root = make_uninit_root("strict-notfound");
    let repo = DashboardStateRepo::new(&root, "REAL");
    let r = repo.acquire_lock(Duration::from_millis(200));
    match r {
        Err(SyncError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        other => panic!("无 .git 时严格锁应 NotFound，got {other:?}"),
    }
}

#[test]
fn allow_and_strict_share_same_sidecar_after_clone() {
    // clone 完成（.git 存在）后：CRUD 宽松锁与采集严格锁必须锁**同一把 sidecar**、二者互斥
    // —— 防 CRUD 写 local.yaml 与采集在锁内读解析 local 撞非原子 fs::write 的半截 YAML。
    let root = make_repo_root("allow-strict-share"); // 造了 .git，模拟 clone 完成
    let repo = DashboardStateRepo::new(&root, "REAL");

    let _crud = repo
        .acquire_lock_allow_uninit(Duration::from_secs(1))
        .expect("CRUD 宽松锁取到");
    let r = repo.acquire_lock(Duration::from_millis(200));
    assert!(
        matches!(r, Err(SyncError::LockBusy)),
        "CRUD 持锁时采集严格锁应 LockBusy（同一把 sidecar），got {r:?}"
    );
}

#[test]
fn allow_uninit_released_on_drop() {
    let root = make_uninit_root("allow-drop");
    let repo = DashboardStateRepo::new(&root, "_crud_only_");
    {
        let _l = repo
            .acquire_lock_allow_uninit(Duration::from_secs(1))
            .unwrap();
    } // 作用域结束 Drop 释放 OS 锁
    let r = repo.acquire_lock_allow_uninit(Duration::from_millis(100));
    assert!(r.is_ok(), "Drop 后宽松锁应立即释放可再取，got {r:?}");
}

#[test]
fn allow_uninit_creates_missing_parent_dir() {
    // first-run 首次装机时 root 的父目录（如 ~/.project-dashboard）也可能尚未建。
    // lock_sidecar 的 create_dir_all(sibling.parent()) 必须能把缺失的父目录补出来再落锁。
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-lock-noparent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // 中间目录 not-created-yet 刻意不建 —— root 的父目录不存在
    let root = base.join("not-created-yet").join("dashboard-state");
    assert!(!base.join("not-created-yet").exists(), "前置：root 父目录不存在");
    let repo = DashboardStateRepo::new(&root, "_crud_only_");
    let lock = repo.acquire_lock_allow_uninit(Duration::from_secs(1));
    assert!(
        lock.is_ok(),
        "父目录缺失时宽松锁应 create_dir_all 后取到，got {lock:?}"
    );
}
