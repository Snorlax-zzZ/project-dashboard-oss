// Q2-D1 集成测试：配置即拉错误分类契约（git stderr / 退出码 → 面向用户分类 + 稳定 tag）
// + dashboard-state 结构校验。全部纯映射 / 纯判定，不起真 git 进程。

use dashboard_core::sync::{
    classify_sync_error, is_dashboard_state_repo, ConfigPullErrorKind, SyncError,
};
use std::path::PathBuf;

fn git_failed(stderr: &str) -> SyncError {
    SyncError::GitFailed {
        args: vec!["clone".into()],
        stderr: stderr.to_string(),
    }
}

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-pullerr-{}-{}-{}",
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

#[test]
fn lock_busy_maps() {
    assert_eq!(
        classify_sync_error(&SyncError::LockBusy),
        ConfigPullErrorKind::LockBusy
    );
}

#[test]
fn git_missing_maps() {
    // git binary 不存在 → Command::new("git").output() 返回 io NotFound
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "program not found");
    assert_eq!(
        classify_sync_error(&SyncError::Io(io)),
        ConfigPullErrorKind::GitNotInstalled
    );
}

#[test]
fn network_dns_maps() {
    for s in [
        "fatal: unable to access 'https://github.com/x/y.git/': Could not resolve host: github.com",
        "ssh: Could not resolve hostname gitlab.internal: nodename nor servname provided",
        "fatal: unable to access '...': Failed to connect to host port 443: Network is unreachable",
        "fatal: unable to access '...': Failed to connect to proxy",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::Network,
            "stderr={s}"
        );
    }
}

#[test]
fn ssh_auth_maps() {
    for s in [
        "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
        "Host key verification failed.\nfatal: Could not read from remote repository.",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::SshAuth,
            "stderr={s}"
        );
    }
}

#[test]
fn https_auth_maps() {
    for s in [
        "fatal: Authentication failed for 'https://bitbucket.org/x/y.git/'",
        "remote: Invalid username or password.\nfatal: Authentication failed",
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::HttpsAuth,
            "stderr={s}"
        );
    }
}

#[test]
fn repo_not_found_or_denied_maps() {
    for s in [
        "remote: Repository not found.\nfatal: repository 'https://github.com/x/y.git/' not found",
        "ERROR: Repository not found.",
        "remote: Permission to x/y.git denied to user.\nfatal: unable to access",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::RepoNotFoundOrDenied,
            "stderr={s}"
        );
    }
}

#[test]
fn target_not_empty_maps() {
    let s = "fatal: destination path 'dashboard-state-clone' already exists and is not an empty directory.";
    assert_eq!(
        classify_sync_error(&git_failed(s)),
        ConfigPullErrorKind::TargetNotEmpty
    );
}

#[test]
fn timeout_maps() {
    // 进程级超时（我们主动 kill）→ Timeout；区别于 git 自报的网络 "connection timed out"（归 Network）
    assert_eq!(
        classify_sync_error(&SyncError::Timeout),
        ConfigPullErrorKind::Timeout
    );
}

#[test]
fn unknown_maps() {
    assert_eq!(
        classify_sync_error(&git_failed("fatal: some unclassified git error xyz")),
        ConfigPullErrorKind::Unknown
    );
}

/// P1-3 关键词收紧回归防线：删裸 "not found" + proxy 收紧后，本地 helper 缺失 / 对象缺失
/// 不再被误判为 RepoNotFoundOrDenied，也不落入 Network，而是归 Unknown。
#[test]
fn narrowed_keywords_no_false_repo_access() {
    for s in [
        "fatal: unable to access '...': proxy command not found",
        "fatal: object not found: abc123",
        "git: 'manager' credential helper not found",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::Unknown,
            "应归 Unknown（非 REPO_ACCESS / 非 Network），stderr={s}"
        );
    }
}

/// P1-3 补大仓 / 网络中断模式 → Network（跨机 VPN 抖动、代理认证）。
#[test]
fn network_interruption_and_proxy_auth_map() {
    for s in [
        "fatal: early EOF",
        "fatal: the remote end hung up unexpectedly",
        "error: RPC failed; curl 56 recv failure",
        "fatal: unable to access '...': Proxy Authentication Required",
    ] {
        assert_eq!(
            classify_sync_error(&git_failed(s)),
            ConfigPullErrorKind::Network,
            "stderr={s}"
        );
    }
}

#[test]
fn tags_are_stable() {
    // 前端靠这些 tag 判分支（不解析中文文案）。tag 契约锁死，改动 = 前端 breaking。
    use ConfigPullErrorKind::*;
    let cases = [
        (GitNotInstalled, "GIT_MISSING"),
        (Network, "NETWORK"),
        (SshAuth, "SSH_AUTH"),
        (HttpsAuth, "HTTPS_AUTH"),
        (RepoNotFoundOrDenied, "REPO_ACCESS"),
        (InvalidStructure, "INVALID_STRUCTURE"),
        (TargetNotEmpty, "TARGET_NOT_EMPTY"),
        (LockBusy, "LOCK_BUSY"),
        (Timeout, "TIMEOUT"),
        (Unknown, "UNKNOWN"),
    ];
    for (k, tag) in cases {
        assert_eq!(k.tag(), tag, "{k:?} 的 tag 变了 → 前端契约 breaking");
        assert!(!k.user_hint().is_empty(), "{k:?} 缺中文提示");
    }
}

#[test]
fn structure_valid_needs_registry_only() {
    // spec dashboard-sync: 仓根 registry/ 是核心结构标识
    let root = make_tempdir("struct-ok");
    std::fs::create_dir_all(root.join("registry").join("projects")).unwrap();
    assert!(
        is_dashboard_state_repo(&root),
        "有 registry/ 即合法 dashboard-state 仓"
    );
}

#[test]
fn structure_missing_registry_invalid() {
    // 用户配了别的 git 仓地址（无 registry/）→ clone 成功但结构非法
    let root = make_tempdir("struct-bad");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "some other repo").unwrap();
    assert!(!is_dashboard_state_repo(&root));
}

#[test]
fn structure_machines_optional() {
    // 全新 clone 的仓有 registry/projects、还没 machines/（本机未采集）→ 仍合法
    // （spec: machines/<id>/ 本机首次同步时才创建，故不作硬校验；决策 3：结构标识是 registry/projects）
    let root = make_tempdir("struct-nomachines");
    std::fs::create_dir_all(root.join("registry").join("projects")).unwrap();
    assert!(is_dashboard_state_repo(&root));
}

#[test]
fn structure_registry_without_projects_invalid() {
    // 决策 3（T7）：普通仓恰好有 registry/ 目录但无 registry/projects → 判非法，
    // 挡「碰巧有 registry 目录的普通仓」被误判为合法 dashboard-state 而污染。
    let root = make_tempdir("struct-registry-no-projects");
    std::fs::create_dir_all(root.join("registry")).unwrap(); // 只 registry/，无 projects
    assert!(
        !is_dashboard_state_repo(&root),
        "有 registry/ 但无 registry/projects 应判非法（决策 3 增强）"
    );
}
