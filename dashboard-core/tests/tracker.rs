// §4 采集器集成测试 —— 真 git init + commit + 采集字段验证 + 危险态 + 归一化
// v4.2: UUID 读写函数已删除（迁 dashboard-state registry），此处只留 progress / 危险态 / normalize。
// `get_git_remote_url` 的新单测在 §16.9 补齐。

use dashboard_core::tracker::{
    collect, detect_dangerous_state, get_git_remote_url, is_git_repo, normalize_git_url,
    read_progress, DangerousState,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-tracker-{}-{}-{}",
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

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git must be on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(cwd: &Path) {
    git(cwd, &["init", "-q", "-b", "main"]);
    git(cwd, &["config", "user.email", "test@project-dashboard"]);
    git(cwd, &["config", "user.name", "project-dashboard-test"]);
    git(cwd, &["config", "commit.gpgsign", "false"]);
}

fn commit(cwd: &Path, file: &str, content: &str, msg: &str) {
    fs::write(cwd.join(file), content).unwrap();
    git(cwd, &["add", file]);
    git(cwd, &["commit", "-q", "-m", msg]);
}

#[test]
fn collect_clean_repo() {
    let tmp = make_tempdir("clean");
    init_repo(&tmp);
    commit(&tmp, "hello.txt", "hi\n", "init");
    let status = collect(&tmp, "clean-proj");
    assert_eq!(status.name, "clean-proj");
    assert_eq!(status.dirty, 0);
    assert_eq!(status.untracked, 0);
    assert!(status.head.is_some());
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.upstream.is_none()); // 无 remote
    assert!(status.ahead.is_none());
    assert!(status.behind.is_none());
    assert!(status.last_commit_at.is_some());
    assert!(status.dangerous_state.is_none());
    assert!(status.project_id.is_none());
    assert!(status.progress.is_none());
}

#[test]
fn collect_dirty_and_untracked_counts() {
    let tmp = make_tempdir("dirty");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    fs::write(tmp.join("a.txt"), "1 modified\n").unwrap(); // 1 dirty
    fs::write(tmp.join("b.txt"), "new\n").unwrap();         // 1 untracked
    fs::write(tmp.join("c.txt"), "new\n").unwrap();         // 1 untracked
    let status = collect(&tmp, "dirty-proj");
    assert_eq!(status.dirty, 1);
    assert_eq!(status.untracked, 2);
}

#[test]
fn collect_detached_head() {
    let tmp = make_tempdir("detached");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    let head_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&tmp)
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&head_sha.stdout).trim().to_string();
    git(&tmp, &["checkout", "-q", "--detach", &sha]);
    let status = collect(&tmp, "detached-proj");
    assert_eq!(status.dangerous_state, Some(DangerousState::Detached));
    // 分离头指针时 branch = None
    assert!(status.branch.is_none());
}

#[test]
fn collect_reads_progress_and_leaves_project_id_none() {
    // v4.2: collect() 不再从项目根读 UUID；project_id 始终为 None，由 caller 从 registry
    // iterator 传入并写回。此测试只验证 progress 依然从 .dashboard-progress.yaml 读到。
    let tmp = make_tempdir("id-progress");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    fs::write(
        tmp.join(".dashboard-progress.yaml"),
        "done: 3\ntotal: 5\nnext: '写文档'\n",
    )
    .unwrap();
    let status = collect(&tmp, "id-progress-proj");
    assert!(status.project_id.is_none(), "v4.2 采集器不再读项目根 UUID");
    let p = status.progress.unwrap();
    assert_eq!(p.done, 3);
    assert_eq!(p.total, 5);
    assert_eq!(p.next, "写文档");
}

#[test]
fn read_progress_none_when_missing() {
    let tmp = make_tempdir("no-progress");
    assert!(read_progress(&tmp).is_none());
}

#[test]
fn is_git_repo_true_when_dot_git_exists() {
    let tmp = make_tempdir("is-git");
    init_repo(&tmp);
    assert!(is_git_repo(&tmp));
    let no_git = make_tempdir("no-git");
    assert!(!is_git_repo(&no_git));
}

// ---- get_git_remote_url（v4.2 §16.1 §16.9）----

#[test]
fn get_git_remote_url_returns_origin_when_set() {
    let tmp = make_tempdir("remote-set");
    init_repo(&tmp);
    // 手工设 origin remote（不 fetch，避免网络依赖）
    git(&tmp, &["remote", "add", "origin", "git@github.com:example/project-dashboard.git"]);
    let url = get_git_remote_url(&tmp).expect("origin remote 已设，应返回 URL");
    assert_eq!(url, "git@github.com:example/project-dashboard.git");
}

#[test]
fn get_git_remote_url_returns_none_when_no_origin() {
    let tmp = make_tempdir("no-origin");
    init_repo(&tmp);
    // git init 出的 repo 默认无 origin remote
    let url = get_git_remote_url(&tmp);
    assert!(url.is_none(), "无 origin remote 时应返 None，实际返回: {url:?}");
}

#[test]
fn get_git_remote_url_returns_none_when_not_git_repo() {
    let tmp = make_tempdir("not-git");
    // 目录存在但不是 git 仓 → git remote 命令会非 0 退出
    let url = get_git_remote_url(&tmp);
    assert!(url.is_none(), "非 git 仓时应返 None，实际返回: {url:?}");
}

// ---- normalize_git_url ----

#[test]
fn normalize_covers_all_allowed_hosts() {
    for host in [
        "github.com",
        "gitlab.com",
        "bitbucket.org",
        "codeberg.org",
    ] {
        let src = format!("https://{host}/owner/repo.git");
        let n = normalize_git_url(&src).unwrap();
        assert_eq!(n, format!("https://{host}/owner/repo"));
    }
}

#[test]
fn normalize_self_hosted_gitlab_prefix() {
    assert_eq!(
        normalize_git_url("https://gitlab.example.com/team/proj").unwrap(),
        "https://gitlab.example.com/team/proj"
    );
}

#[test]
fn detect_dangerous_state_rebase() {
    let tmp = make_tempdir("rebase");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    // 手工造 rebase-merge 目录（真 rebase 步骤太脆弱）
    fs::create_dir_all(tmp.join(".git").join("rebase-merge")).unwrap();
    assert_eq!(detect_dangerous_state(&tmp), Some(DangerousState::Rebase));
}

#[test]
fn detect_dangerous_state_merge() {
    let tmp = make_tempdir("merge-danger");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    fs::write(tmp.join(".git").join("MERGE_HEAD"), "abc123\n").unwrap();
    assert_eq!(detect_dangerous_state(&tmp), Some(DangerousState::Merge));
}

#[test]
fn detect_dangerous_state_cherry_pick() {
    let tmp = make_tempdir("cherry");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    fs::write(tmp.join(".git").join("CHERRY_PICK_HEAD"), "abc123\n").unwrap();
    assert_eq!(detect_dangerous_state(&tmp), Some(DangerousState::CherryPick));
}

#[test]
fn detect_dangerous_state_worktree_dot_git_file_rebase() {
    // 审计对账 P0-F（2026-07-06）：worktree/submodule 场景下 `.git` 是文件不是目录。
    // 修复后 detect_dangerous_state 应能解析 gitdir: 指向的真实 git 目录并检测 rebase。
    let tmp = make_tempdir("worktree");
    fs::create_dir_all(tmp.join("real-git-dir")).unwrap();
    fs::create_dir_all(tmp.join("real-git-dir").join("rebase-merge")).unwrap();
    // 项目根的 .git 是个文件，内容形如 `gitdir: ../real-git-dir\n`
    let dot_git_content = format!("gitdir: {}\n", tmp.join("real-git-dir").display());
    fs::write(tmp.join(".git"), dot_git_content).unwrap();
    assert_eq!(
        detect_dangerous_state(&tmp),
        Some(DangerousState::Rebase),
        "worktree 场景下 .git 是文件时也要正确检测 rebase"
    );
}

#[test]
fn detect_dangerous_state_worktree_dot_git_file_merge() {
    let tmp = make_tempdir("worktree-merge");
    fs::create_dir_all(tmp.join("real-git")).unwrap();
    fs::write(tmp.join("real-git").join("MERGE_HEAD"), "abc123\n").unwrap();
    let dot_git_content = format!("gitdir: {}\n", tmp.join("real-git").display());
    fs::write(tmp.join(".git"), dot_git_content).unwrap();
    assert_eq!(detect_dangerous_state(&tmp), Some(DangerousState::Merge));
}

#[test]
fn is_git_repo_accepts_dot_git_file() {
    // .git 是文件时（worktree）也算 git 仓
    use dashboard_core::tracker::is_git_repo;
    let tmp = make_tempdir("worktree-is-repo");
    fs::create_dir_all(tmp.join("real-git")).unwrap();
    fs::write(tmp.join(".git"), format!("gitdir: {}\n", tmp.join("real-git").display())).unwrap();
    assert!(is_git_repo(&tmp), ".git 是文件时也应算作 git 仓");
}

// ============================================================================
// v4.3 §1.2 采集器 —— head_full + recent_commits 三态语义 + head/log 原子性校验
// ============================================================================

/// §1.2.1：正常仓 → head_full 是完整 40 位 sha 且短 sha 是它的前缀
#[test]
fn v43_collect_writes_head_full_40chars() {
    let tmp = make_tempdir("v43-head-full");
    init_repo(&tmp);
    commit(&tmp, "hello.txt", "hi\n", "first");
    let status = collect(&tmp, "v43-proj");
    let head_full = status.head_full.expect("正常仓必须有 head_full");
    assert_eq!(head_full.len(), 40, "head_full 必须是 40 位完整 sha");
    assert!(
        head_full.chars().all(|c| c.is_ascii_hexdigit()),
        "head_full 必须全是十六进制字符"
    );
    let head_short = status.head.expect("正常仓也必须有短 sha");
    assert!(
        head_full.starts_with(&head_short),
        "短 sha 必须是 head_full 的前缀"
    );
}

/// §1.2.3：正常仓多次 commit → recent_commits 有 N 条，first().sha_full == head_full
#[test]
fn v43_collect_recent_commits_populates_when_available() {
    let tmp = make_tempdir("v43-populated");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "a\n", "first");
    commit(&tmp, "b.txt", "b\n", "second");
    commit(&tmp, "c.txt", "c\n", "third");
    let status = collect(&tmp, "populated-proj");
    let commits = status.recent_commits.expect("正常仓必须有 Some(vec)");
    assert_eq!(commits.len(), 3, "3 次 commit 必须采到 3 条");
    // git log 倒序 —— 最新在前
    assert_eq!(commits[0].subject, "third");
    assert_eq!(commits[1].subject, "second");
    assert_eq!(commits[2].subject, "first");
    // head/log 原子性契约：first().sha_full == head_full
    let head_full = status.head_full.expect("正常仓必须有 head_full");
    assert_eq!(commits[0].sha_full, head_full, "commits[0] 必须是 HEAD");
    // sha_full 40 位完整
    for c in &commits {
        assert_eq!(c.sha_full.len(), 40);
    }
}

/// v4.3+：collect 给最近提交补文件级改动（git show --numstat）——端到端验证 enrich 生效。
#[test]
fn v43_collect_enriches_recent_commits_with_files() {
    let tmp = make_tempdir("v43-files");
    init_repo(&tmp);
    commit(&tmp, "foo.txt", "l1\nl2\nl3\n", "add foo"); // 新增 3 行
    let status = collect(&tmp, "files-proj");
    let commits = status.recent_commits.expect("正常仓必须有 Some(vec)");
    let files = commits[0]
        .files
        .as_ref()
        .expect("最近 1 条应带文件级改动（enrich 前 5 条）");
    let foo = files
        .iter()
        .find(|f| f.path == "foo.txt")
        .expect("应含 foo.txt");
    assert_eq!(foo.additions, Some(3), "foo.txt 新增 3 行");
    assert_eq!(foo.deletions, Some(0));
}

/// §1.2.7：有效空仓 unborn repo → head_full = None，recent_commits = Some(vec![])（不是 None！）
#[test]
fn v43_collect_recent_commits_empty_vec_when_unborn_repo() {
    let tmp = make_tempdir("v43-unborn");
    init_repo(&tmp); // git init 但不 commit
    let status = collect(&tmp, "unborn-proj");
    assert!(status.head_full.is_none(), "unborn 仓 head_full 必须为 None");
    assert_eq!(
        status.recent_commits,
        Some(Vec::new()),
        "unborn 仓 recent_commits 必须为 Some(vec![])，绝不能是 None"
    );
}

/// §1.2.8 变体：非 git 目录 → head_full = None，recent_commits = None（不可用信号）
#[test]
fn v43_collect_recent_commits_none_when_not_git_repo() {
    let tmp = make_tempdir("v43-not-repo");
    let status = collect(&tmp, "not-repo");
    assert!(status.head_full.is_none());
    assert!(
        status.recent_commits.is_none(),
        "非 git 仓 recent_commits 必须为 None（不可用信号，绝不降级成空 vec）"
    );
}

/// subject 40 字截断（英文 ASCII，隐私边界）
#[test]
fn v43_collect_subject_truncated_to_40_chars() {
    let tmp = make_tempdir("v43-subject-trunc");
    init_repo(&tmp);
    let long_msg = "a".repeat(60);
    commit(&tmp, "a.txt", "a\n", &long_msg);
    let status = collect(&tmp, "trunc-proj");
    let commits = status.recent_commits.unwrap();
    assert_eq!(commits[0].subject.chars().count(), 40, "subject 必须截断到 40 chars");
    assert_eq!(commits[0].subject, "a".repeat(40));
}

/// subject 40 字截断（中文 —— 用 chars 而非 bytes 确保多字节安全）
#[test]
fn v43_collect_subject_truncated_by_chars_not_bytes() {
    let tmp = make_tempdir("v43-subject-cjk");
    init_repo(&tmp);
    let cjk_msg: String = "字".repeat(50);
    commit(&tmp, "a.txt", "a\n", &cjk_msg);
    let status = collect(&tmp, "cjk-proj");
    let commits = status.recent_commits.unwrap();
    assert_eq!(
        commits[0].subject.chars().count(),
        40,
        "中文 subject 必须按 chars 截到 40，不能按 bytes 截"
    );
    assert_eq!(commits[0].subject, "字".repeat(40));
}

/// 采集窗口封顶 20 条
#[test]
fn v43_collect_recent_commits_capped_at_20() {
    let tmp = make_tempdir("v43-cap-20");
    init_repo(&tmp);
    for i in 0..25u32 {
        commit(&tmp, "a.txt", &format!("{i}\n"), &format!("c{i}"));
    }
    let status = collect(&tmp, "cap-proj");
    let commits = status.recent_commits.unwrap();
    assert_eq!(commits.len(), 20, "采集窗口最多 20 条");
    // 最新 c24 在前
    assert_eq!(commits[0].subject, "c24");
    // 0..25 里最新 20 条是 c5..c24
    assert_eq!(commits[19].subject, "c5");
}

/// clean_repo 场景回归：新字段不破坏 v4.2 已有断言
#[test]
fn v43_collect_clean_repo_still_matches_v42_fields() {
    let tmp = make_tempdir("v43-clean");
    init_repo(&tmp);
    commit(&tmp, "hello.txt", "hi\n", "init");
    let status = collect(&tmp, "clean-proj");
    assert_eq!(status.name, "clean-proj");
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.head.as_deref().is_some_and(|h| h.chars().count() >= 7));
    assert_eq!(status.dirty, 0);
    assert_eq!(status.untracked, 0);
    assert!(status.head_full.is_some());
    assert!(status.recent_commits.is_some());
}

/// P0-2：短 head 从 head_full 前 7 位派生，不再独立 rev-parse --short，
/// 消除两次 rev-parse 之间的时序间隙。契约：head 恰好是 head_full 的前 7 位前缀。
#[test]
fn v43_short_head_is_prefix_of_head_full() {
    let tmp = make_tempdir("v43-head-derive");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    let status = collect(&tmp, "derive-proj");
    let head = status.head.expect("有 commit 应有短 head");
    let head_full = status.head_full.expect("有 commit 应有 head_full");
    assert_eq!(head.chars().count(), 7, "短 head 应为 7 位");
    assert_eq!(head.as_str(), &head_full[..7], "短 head 必须是 head_full 的前 7 位前缀");
}

/// P0-1（实测确认）：loose ref 损坏（HEAD 仍合法指向 main，但
/// `.git/refs/heads/main` 内容损坏）—— is-inside-work-tree 成功，但 rev-parse HEAD 与
/// symbolic-ref --quiet HEAD 都失败（实测 exit 128）→ 判采集失败 (None, None)，绝不误判成
/// unborn 的 Some(vec![])。旧实现 `(None,_) => (None, Some(vec![]))` 会把这种损坏当空仓，
/// 违反 tracker "git 命令失败绝不降级成空数组" 契约。
#[test]
fn v43_collect_loose_ref_corrupt_is_failed_not_unborn() {
    let tmp = make_tempdir("v43-loose-ref-corrupt");
    init_repo(&tmp);
    commit(&tmp, "a.txt", "1\n", "init");
    // 损坏当前分支的 loose ref（HEAD 仍是合法的 `ref: refs/heads/main`）
    fs::write(
        tmp.join(".git").join("refs").join("heads").join("main"),
        "garbage-not-a-sha\n",
    )
    .unwrap();
    // 删 packed-refs 确保不走打包备份兜底（新 init 一般没有，防御性删）
    let _ = fs::remove_file(tmp.join(".git").join("packed-refs"));
    let status = collect(&tmp, "loose-corrupt-proj");
    assert!(status.head_full.is_none(), "loose ref 损坏 head_full 必须为 None");
    assert!(
        status.recent_commits.is_none(),
        "P0-1：loose ref 损坏（symbolic-ref 也失败）必须判采集失败 → recent_commits=None，绝不能误判成 unborn 的 Some(vec![])"
    );
}
