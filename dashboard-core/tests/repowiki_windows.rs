#![cfg(all(windows, feature = "process-fixture"))]
use dashboard_core::{
    config::{LocalConfig, PrivateProject, Registry},
    repowiki::{self, RepoWikiProjectSelector},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    local: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tmp")
            .join(format!("rw-{}", uuid::Uuid::new_v4()));
        let repo = root.join("中文 空格 & $(x)");
        let state = root.join("state");
        let local = root.join("local.yaml");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\nurl = https://example.test/team/a.git\n",
        )
        .unwrap();
        let cli = root.join("repowiki.exe");
        fs::copy(env!("CARGO_BIN_EXE_process-fixture"), &cli).unwrap();
        let mut cfg = LocalConfig::new("test-machine");
        cfg.repowiki_cli_path = Some(cli.display().to_string());
        cfg.repowiki_data_root = Some(root.join("data").display().to_string());
        cfg.private_projects.push(PrivateProject {
            schema_version: dashboard_core::config::CURRENT_SCHEMA_VERSION,
            project_id: None,
            sync: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            name: "test".into(),
            git_url: "https://example.test/team/a.git".into(),
            path: repo.display().to_string(),
        });
        Registry {
            projects: BTreeMap::new(),
            local: Some(cfg),
        }
        .save_at(&state, &local)
        .unwrap();
        Self {
            root,
            repo,
            state,
            local,
        }
    }
    fn selector(&self) -> RepoWikiProjectSelector {
        RepoWikiProjectSelector::Private {
            name: "test".into(),
        }
    }
    fn data_root(&self) -> String {
        self.root.join("data").display().to_string()
    }
    fn argv<'a>(&self, envelope: &'a repowiki::RepoWikiEnvelope) -> Vec<&'a str> {
        envelope.result["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect()
    }
    fn wiki(&self) {
        let base = self.repo.join(".repowiki");
        fs::create_dir_all(base.join("state")).unwrap();
        let navigation=json!({"schema_version":2,"home_page_id":"home","pages":[{"page_id":"home","path":"docs/home.md","title":"首页"}]}).to_string();
        let mut hashes = BTreeMap::new();
        for (path, value) in [
            ("navigation.json", navigation.as_str()),
            ("docs/home.md", "# 正文"),
            ("README.md", "wiki"),
            ("catalog/accepted.json", "{}"),
            ("catalog/inventory.json", "{}"),
            ("coverage/coverage.json", "{}"),
            ("evidence/map.json", "{}"),
            ("state/quality-report.json", "{}"),
        ] {
            fs::create_dir_all(base.join(path).parent().unwrap()).unwrap();
            fs::write(base.join(path), value).unwrap();
            hashes.insert(path, format!("{:x}", Sha256::digest(value.as_bytes())));
        }
        let manifest=json!({"schema_version":2,"generation_id":"rwwiki_012345678901_012345678901","managed_fingerprint":format!("{:x}",Sha256::digest(serde_json::to_vec(&hashes).unwrap())),"managed_files":hashes}).to_string();
        fs::write(base.join("state/content-manifest.json"), &manifest).unwrap();
        fs::write(base.join("state/generation.json"),json!({"schema_version":2,"status":"complete","generation_id":"rwwiki_012345678901_012345678901","manifest_sha256":format!("{:x}",Sha256::digest(manifest.as_bytes())),"pages":{"home":{}}}).to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn native_status_preserves_bound_unicode_arguments() {
    let f = Fixture::new();
    let result = repowiki::status(&f.state, &f.local, &f.selector()).unwrap();
    assert!(result.ok);
    assert_eq!(result.result["argv"][1], f.repo.to_str().unwrap());
}
#[test]
fn direct_v2_reads_validate_manifest_and_reject_changed_content() {
    let f = Fixture::new();
    f.wiki();
    assert!(
        repowiki::read_project(&f.state, &f.local, &f.selector())
            .unwrap()
            .ok
    );
    assert_eq!(
        repowiki::read_page(&f.state, &f.local, &f.selector(), "home")
            .unwrap()
            .result["content"],
        "# 正文"
    );
    fs::write(f.repo.join(".repowiki/docs/home.md"), "外部修改").unwrap();
    assert!(repowiki::read_page(&f.state, &f.local, &f.selector(), "home").is_err());
    assert!(repowiki::read_generations(&f.state, &f.local, &f.selector()).is_err());
}

// 2.4：全部图命令走真实 Windows 路由（run_bounded → windows_process），断言完整 argv
// 编码（repo 路径含中文 / 空格 / & 与 $(x) 元字符）与命令名；非法参数在启动进程前拒绝。
#[test]
fn graph_commands_route_through_windows_runner_with_bound_arguments() {
    let f = Fixture::new();
    let node = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let envelope =
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "Windows 搜索 & $(x)", 7)
            .unwrap();
    assert!(envelope.ok);
    assert_eq!(envelope.command, "graph search");
    assert_eq!(
        f.argv(&envelope),
        [
            "--repo",
            f.repo.to_str().unwrap(),
            "--data-root",
            f.data_root().as_str(),
            "--json",
            "graph",
            "search",
            "--text",
            "Windows 搜索 & $(x)",
            "--limit",
            "7",
        ]
    );
    let envelope = repowiki::graph_show(&f.state, &f.local, &f.selector(), node).unwrap();
    assert_eq!(envelope.command, "graph show");
    assert!(f.argv(&envelope).contains(&node));
    let selectors = &f.selector();
    let neighborhoods = [
        (
            "callers",
            repowiki::graph_callers(&f.state, &f.local, selectors, node, 2, 10).unwrap(),
        ),
        (
            "callees",
            repowiki::graph_callees(&f.state, &f.local, selectors, node, 1, 5).unwrap(),
        ),
        (
            "impact",
            repowiki::graph_impact(&f.state, &f.local, selectors, node, 5, 1).unwrap(),
        ),
    ];
    for (operation, envelope) in neighborhoods {
        assert_eq!(
            envelope.command,
            format!("graph {operation}"),
            "{operation}"
        );
        let argv = f.argv(&envelope);
        assert!(argv.contains(&node), "{operation}");
        let (depth, limit) = match operation {
            "callers" => ("2", "10"),
            "callees" => ("1", "5"),
            _ => ("5", "1"),
        };
        assert!(argv.contains(&depth), "{operation}");
        assert!(argv.contains(&limit), "{operation}");
    }
    // 非法参数在 spawn 前被拒：limit 越界 / node 非法 / depth 越界。
    assert_eq!(
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "文本", 0)
            .unwrap_err()
            .code(),
        "invalid_repowiki_argument"
    );
    assert_eq!(
        repowiki::graph_show(&f.state, &f.local, &f.selector(), "not-a-node")
            .unwrap_err()
            .code(),
        "invalid_repowiki_argument"
    );
    assert_eq!(
        repowiki::graph_callers(&f.state, &f.local, &f.selector(), node, 9, 10)
            .unwrap_err()
            .code(),
        "invalid_repowiki_argument"
    );
}

// 2.4：非零退出 + 规范错误 envelope 透传；非零退出 + 非 JSON 输出判协议违规。
#[test]
fn nonzero_exit_envelope_and_garbage_output_are_classified() {
    let f = Fixture::new();
    let envelope =
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "repowiki-fail", 1).unwrap();
    assert!(!envelope.ok);
    let error = envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "forced_failure");
    assert!(!error.message.is_empty());
    assert!(error.details.is_object());
    assert_eq!(
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "repowiki-crash", 1)
            .unwrap_err()
            .code(),
        "repowiki_protocol_violation"
    );
}

// 2.4：stdout 20MiB / stderr 64KiB 两种输出超限都终止进程并归一为 output_too_large。
// 实测泵循环节奏约 1.4MiB/s（5ms 睡眠 × Windows 定时器粒度），20MiB 预算耗尽约 15 秒，
// 断言口径取「超限在 30 秒期限之内成立」，不虚构亚秒级时限。
#[test]
fn oversized_stdout_and_stderr_are_terminated() {
    let f = Fixture::new();
    for text in ["repowiki-spew-out", "repowiki-spew-err"] {
        let started = Instant::now();
        let error = repowiki::graph_search(&f.state, &f.local, &f.selector(), text, 1).unwrap_err();
        assert_eq!(error.code(), "repowiki_output_too_large", "{text}");
        assert!(
            started.elapsed() < Duration::from_secs(29),
            "超限必须在 30 秒期限之前成立：{text}"
        );
    }
    // 超限清理后执行器可立即复用。
    assert!(
        repowiki::status(&f.state, &f.local, &f.selector())
            .unwrap()
            .ok
    );
}

// 2.4：30 秒期限真实收割卡死的 CLI，且超时清理不拖垮后续调用。
#[test]
fn cli_deadline_terminates_after_30_seconds() {
    let f = Fixture::new();
    let started = Instant::now();
    let error =
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "repowiki-stall", 1).unwrap_err();
    assert_eq!(error.code(), "repowiki_timeout");
    assert!(
        started.elapsed() >= Duration::from_secs(29),
        "必须等待满 30 秒预算而不是提前失败"
    );
    assert!(started.elapsed() < Duration::from_secs(45));
    assert!(
        repowiki::status(&f.state, &f.local, &f.selector())
            .unwrap()
            .ok
    );
}

// 2.4：v2 读取不启动 CLI——把 CLI 换成无效 PE 后 v2 全部读取仍成功；
// 对照组证明同一 CLI 下任何需要进程的命令都会 SpawnFailed，v2 的成功只能是没 spawn。
#[test]
fn v2_reads_succeed_even_when_cli_cannot_spawn() {
    let f = Fixture::new();
    f.wiki();
    fs::write(f.root.join("repowiki.exe"), b"not an executable image").unwrap();
    let project = repowiki::read_project(&f.state, &f.local, &f.selector()).unwrap();
    assert!(project.ok);
    assert_eq!(
        repowiki::read_page(&f.state, &f.local, &f.selector(), "home")
            .unwrap()
            .result["content"],
        "# 正文"
    );
    let generations = repowiki::read_generations(&f.state, &f.local, &f.selector()).unwrap();
    assert_eq!(generations.result["wiki"]["page_total"], 1);
    // 对照：进程路径真的坏了。
    assert_eq!(
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "任意", 1)
            .unwrap_err()
            .code(),
        "repowiki_spawn_failed"
    );
    // 非法参数在 spawn 前拒绝，与 CLI 可用与否无关。
    assert_eq!(
        repowiki::graph_search(&f.state, &f.local, &f.selector(), "任意", 0)
            .unwrap_err()
            .code(),
        "invalid_repowiki_argument"
    );
}
