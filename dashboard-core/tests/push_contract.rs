// 组 4（决策 4）CLI --push 契约 · sync_cycle 锁内编排语义测试（2026-07-21）
//
// 覆盖 tasks.md 4.1 的语义边界（用真 bare remote 驱动，不再是「无 remote → push 必失败」简化场景）：
// - 不带 push（land-only）→ bare remote HEAD 不变、本地不 commit
// - 带 push → bare remote HEAD 前进、pushed=true
// - hash 未变 + force（手动同步）→ 仍推；hash 未变 + 不带 push → 不推、不落盘
// - push 失败（remote 不可达）→ 编排返 {changed:true, pushed:false}、本地 commit 保留、退 0 不抛 Err
// - land-only 后再 push=true → 之前落盘的 machines/ 变更被一并带推（决策 4 的 land-only 边界）

use chrono::{TimeZone, Utc};
use dashboard_core::sync::{
    DashboardStateRepo, Snapshot, SnapshotProject, Trigger, CURRENT_SNAPSHOT_SCHEMA_VERSION,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-push-contract-{}-{}-{}",
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

/// 造一个 bare remote + 从它 clone 出的 dashboard-state 工作区（含 registry/projects 合法结构 +
/// 首个 commit 已推），返回 (bare_remote, clone_root)。clone 出来后 origin 指向 bare，push 能真前进。
fn seed_bare_and_clone(tag: &str) -> (PathBuf, PathBuf) {
    let tmp = make_tempdir(tag);
    let bare = tmp.join("remote.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);

    // 先在一个 seed 工作区里建合法 dashboard-state 结构并推到 bare，clone 后才有 main 分支
    let seed = tmp.join("seed");
    std::fs::create_dir_all(seed.join("registry").join("projects")).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@pd"]);
    git(&seed, &["config", "user.name", "pd-test"]);
    git(&seed, &["config", "commit.gpgsign", "false"]);
    std::fs::write(seed.join("registry").join("projects").join(".keep"), "").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed dashboard-state"]);
    git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&seed, &["push", "-q", "origin", "main"]);

    let clone_root = tmp.join("dashboard-state-clone");
    git(&tmp, &["clone", "-q", bare.to_str().unwrap(), clone_root.to_str().unwrap()]);
    // clone 出来的工作区补 git identity（commit 用）
    git(&clone_root, &["config", "user.email", "t@pd"]);
    git(&clone_root, &["config", "user.name", "pd-test"]);
    git(&clone_root, &["config", "commit.gpgsign", "false"]);
    (bare, clone_root)
}

fn bare_head(bare: &Path) -> Option<String> {
    let out = git(bare, &["rev-parse", "main"]);
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None // main 还没有任何 commit（rev-parse 失败）
    }
}

// 固定 project_id —— 同一逻辑项目多次采集必须是同一 UUID，否则语义 hash 会因 UUID 变化而变，
// 「hash 未变则跳过」的契约测不出来（snapshot_hash 把 project_id 纳入稳定投影）。
fn fixed_pid() -> Uuid {
    Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap()
}

fn snap(machine: &str, head: &str) -> Snapshot {
    snap_at(machine, head, 0)
}

/// 同 snap 但可指定 `updated_at`/`collected_at` 的秒偏移 —— 模拟生产里「再次采集时间戳前进、
/// 但 git 语义状态未变」。生产 `updated_at: Utc::now()` 每次不同，故 write_snapshot 落盘字节
/// 必变、force 路径总有变更可 commit（不会空提交）；固定时间戳会制造生产不可能的 byte 完全相同。
fn snap_at(machine: &str, head: &str, secs: u32) -> Snapshot {
    let ts = Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, secs).single().unwrap();
    Snapshot {
        schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
        machine_id: machine.to_string(),
        hostname: String::new(),
        updated_at: ts,
        projects: vec![SnapshotProject {
            project_id: Some(fixed_pid()),
            name: "p".into(),
            branch: Some("main".into()),
            head: Some(head.into()),
            head_full: None,
            upstream: None,
            ahead: None,
            behind: None,
            dirty: 0,
            untracked: 0,
            last_commit_at: None,
            last_fs_change_at: None,
            collected_at: ts,
            progress: None,
            dangerous_state: None,
            recent_commits: None,
        }],
    }
}

// ── 契约 1：不带 push（land-only）→ bare HEAD 不变、本地不新增 commit ──
#[test]
fn land_only_does_not_advance_remote() {
    let (bare, root) = seed_bare_and_clone("land-only");
    let repo = DashboardStateRepo::new(&root, "M1");
    let before = bare_head(&bare).unwrap();

    let r = repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, false).unwrap();
    assert!(r.changed, "首次采集 snapshot 语义变化，changed 应为 true");
    assert!(!r.pushed, "land-only 不 push，pushed 应为 false");

    // snapshot 已落盘（land 完成）
    assert!(repo.snapshot_path().exists(), "land-only 仍应落盘 snapshot.json");
    // 但 bare remote HEAD 不变
    assert_eq!(bare_head(&bare).unwrap(), before, "land-only 不得推进 bare remote HEAD");
    // 本地也没有新 commit（工作树 dirty / untracked，未 commit）
    let local_head_before = String::from_utf8_lossy(&git(&root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    assert_eq!(local_head_before, before, "land-only 不得在本地新增 commit");
}

// ── 契约 2：带 push → bare HEAD 前进、pushed=true ──
#[test]
fn push_advances_remote() {
    let (bare, root) = seed_bare_and_clone("push-advance");
    let repo = DashboardStateRepo::new(&root, "M1");
    let before = bare_head(&bare).unwrap();

    let r = repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, true).unwrap();
    assert!(r.changed);
    assert!(r.pushed, "带 push 且 remote 可达，pushed 应为 true");
    assert_ne!(bare_head(&bare).unwrap(), before, "push 后 bare remote HEAD 必须前进");
}

// ── 契约 3a：hash 未变 + 不带 push → 不落盘、不推 ──
#[test]
fn unchanged_without_push_is_noop() {
    let (bare, root) = seed_bare_and_clone("unchanged-nopush");
    let repo = DashboardStateRepo::new(&root, "M1");
    // 先 push 一次建立基线
    repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, true).unwrap();
    let after_first = bare_head(&bare).unwrap();
    let bytes_before = std::fs::read(repo.snapshot_path()).unwrap();

    // 同 hash（同 head）+ 不 force + 不 push → changed=false，什么都不做
    let r = repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Tick, false, false).unwrap();
    assert!(!r.changed, "语义未变应 changed=false");
    assert!(!r.pushed);
    assert_eq!(bare_head(&bare).unwrap(), after_first, "未变 remote 不动");
    assert_eq!(std::fs::read(repo.snapshot_path()).unwrap(), bytes_before, "未变不重写 snapshot");
}

// ── 契约 3b：hash 未变 + force（手动同步）→ 仍推 ──
#[test]
fn unchanged_with_force_still_pushes() {
    let (bare, root) = seed_bare_and_clone("unchanged-force");
    let repo = DashboardStateRepo::new(&root, "M1");
    repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, true).unwrap();
    let after_first = bare_head(&bare).unwrap();

    // 同 git 状态（同 head，语义 hash 不变）但时间戳前进（生产 updated_at=now 必然如此）+
    // force=true + push=true → 强制走 write_snapshot(字节因 updated_at 变而变) + commit + push
    let r = repo.sync_cycle(snap_at("M1", "abc", 30), vec![], Trigger::Manual, true, true).unwrap();
    assert!(r.changed, "force=true 时 changed 恒为 true（强制落盘）");
    assert!(r.pushed, "force + push 应真的推");
    assert_ne!(bare_head(&bare).unwrap(), after_first, "force 手动同步即使语义 hash 未变也应推进 remote");
}

// ── 契约 4（核心）：push 失败 → {changed:true, pushed:false}、本地 commit 保留、退 0 不抛 Err ──
#[test]
fn push_failure_is_partial_success_not_error() {
    let (bare, root) = seed_bare_and_clone("push-fail");
    let repo = DashboardStateRepo::new(&root, "M1");
    let remote_head_before = bare_head(&bare).unwrap();

    // 把 origin 指向不存在的路径，模拟 push 失败（网络 / 认证 / 离线的等价物）
    git(&root, &["remote", "set-url", "origin", "/nonexistent/unreachable-remote.git"]);
    let local_head_before = String::from_utf8_lossy(&git(&root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();

    let r = repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, true);
    // 关键：不抛 Err（land+commit 已成功，只是 push 失败）
    let r = r.expect("push 失败必须是部分成功、不向上抛 Err");
    assert!(r.changed, "land 已完成，changed=true");
    assert!(!r.pushed, "push 失败 pushed 必须为 false");

    // 本地 commit 已产生（land+commit 成功）
    let local_head_after = String::from_utf8_lossy(&git(&root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    assert_ne!(local_head_after, local_head_before, "push 失败但本地 commit 必须已落");
    // remote HEAD 原地不动（origin 已改指向坏路径，原 bare 不受影响）
    assert_eq!(bare_head(&bare).unwrap(), remote_head_before, "push 失败 remote 不得前进");
}

// ── 契约 5：land-only 后再 push=true → 之前落盘的 machines/ 变更被带推（决策 4 land-only 边界）──
#[test]
fn pull_first_survives_dirty_worktree_left_by_land_only() {
    let (bare, root) = seed_bare_and_clone("pull-first-dirty");
    // 复现真机配置：用户全局配了 pull.rebase=true 时，`pull --ff-only` 仍走 rebase 分支，
    // 而 rebase **要求工作树完全干净**（比 merge 严格）。这里写进仓内配置，让测试不依赖机器全局设置。
    git(&root, &["config", "pull.rebase", "true"]);
    let remote_before = bare_head(&bare).unwrap();

    // land-only 采集（`collect` 不带 `--push`）只写盘不提交，留下脏工作树；
    // 身份迁移在修复前也是这个形状。此后同步走 pull-first 就会被 git 拒绝。
    std::fs::create_dir_all(root.join("machines").join("M1")).unwrap();
    std::fs::write(root.join("machines").join("M1").join("snapshot.json"), "{}").unwrap();
    std::fs::write(root.join("registry").join("projects").join("p.yaml"), "name: x\n").unwrap();
    // 仓根的非受管文件：白名单必须把它挡在提交之外（数据目录可由用户配置，可能含密钥）。
    std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();

    dashboard_core::commands::trigger_config_pull(&root, bare.to_str(), false, true)
        .expect("脏工作树不该让 pull-first 失败——否则此后所有同步全线瘫痪");

    let status = git(&root, &["status", "--porcelain"]);
    let dirty = String::from_utf8_lossy(&status.stdout);
    assert!(
        dirty.lines().all(|line| line.contains(".env")),
        "受管改动必须已被提交，只允许剩下非受管文件，实际:\n{dirty}"
    );
    let ls_files = git(&root, &["ls-files"]);
    let tracked = String::from_utf8_lossy(&ls_files.stdout);
    assert!(tracked.contains("machines/M1/snapshot.json"));
    assert!(tracked.contains("registry/projects/p.yaml"));
    assert!(!tracked.contains(".env"), "白名单必须挡住仓根非受管文件：\n{tracked}");
    assert_eq!(
        bare_head(&bare).unwrap(),
        remote_before,
        "收拢只提交不 push，remote 不该被带动——推送仍由 sync_cycle 负责"
    );
}

#[test]
fn land_only_then_push_carries_pending_changes() {
    let (bare, root) = seed_bare_and_clone("land-then-push");
    let repo = DashboardStateRepo::new(&root, "M1");
    let before = bare_head(&bare).unwrap();

    // 第一步：land-only（不推）——snapshot 落盘但 remote 不动
    let r1 = repo.sync_cycle(snap("M1", "abc"), vec![], Trigger::Manual, false, false).unwrap();
    assert!(r1.changed && !r1.pushed);
    assert_eq!(bare_head(&bare).unwrap(), before, "land-only 阶段 remote 不动");

    // 第二步：同 head（语义 hash 未变）+ 时间戳前进 + force + push —— 手动同步把 pending 的
    // machines/ 变更带推（land-only 已写入的 snapshot 也在 machines/ 白名单里，一并 commit+push）
    let r2 = repo.sync_cycle(snap_at("M1", "abc", 30), vec![], Trigger::Manual, true, true).unwrap();
    assert!(r2.pushed, "后续 push 应成功");
    assert_ne!(bare_head(&bare).unwrap(), before, "land-only 的落盘变更应被后续 push 一并带推");

    // 带推后 remote 的 tree 里应含本机 snapshot.json（machines/M1/snapshot.json）
    let ls = git(&bare, &["ls-tree", "-r", "--name-only", "main"]);
    let files = String::from_utf8_lossy(&ls.stdout);
    assert!(
        files.contains("machines/M1/snapshot.json"),
        "带推后 remote 应含 machines/M1/snapshot.json，实际:\n{files}"
    );
}
