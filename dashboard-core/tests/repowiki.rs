#![cfg(unix)]

use chrono::Utc;
use dashboard_core::config::{LocalConfig, Registry, RegistryProject, CURRENT_SCHEMA_VERSION};
use dashboard_core::repowiki::{self, RepoWikiProjectSelector};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "repowiki-adapter-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn shell_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_fake_cli(path: &Path, script_body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\nset -eu\n{script_body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn write_git_origin(repo: &Path, origin: &str) {
    let git = repo.join(".git");
    fs::create_dir_all(&git).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n[remote \"origin\"]\n\turl = {origin}\n"
        ),
    )
    .unwrap();
}

// 造出 v2 只读所需的 `.repowiki/` 产物：navigation.json + 若干 md 文件（路径相对
// `.repowiki/`）。read_project/read_page 改造后直接读这些产物，不再 spawn CLI。
fn write_repowiki(repo: &Path, navigation: &str, files: &[(&str, &str)]) {
    let base = repo.join(".repowiki");
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("navigation.json"), navigation).unwrap();
    for (relative, content) in files {
        let path = base.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    }
}

// 一份典型的 v2 navigation：两页、含父子层级，且每页都塞了 source_anchors，
// 用来验证后端读取时会把体积巨大的 source_anchors 字段剔除。
fn sample_navigation() -> String {
    serde_json::json!({
        "schema_version": 2,
        "home_page_id": "home",
        "layout_version": 7,
        "pages": [
            {
                "page_id": "home",
                "title": "项目概览",
                "path": "docs/home.md",
                "page_type": "overview",
                "parent_page_id": null,
                "order": 0,
                "children": ["arch"],
                "source_anchors": [{"symbol": "main", "file": "src/main.rs"}]
            },
            {
                "page_id": "arch",
                "title": "架构说明",
                "path": "docs/arch.md",
                "page_type": "section",
                "parent_page_id": "home",
                "order": 1,
                "children": [],
                "source_anchors": [{"symbol": "run", "file": "src/run.rs"}]
            }
        ]
    })
    .to_string()
}

// 写一个「不应被调用」的 CLI 占位：resolve_context 仍会校验 CLI 路径存在且可执行，
// 但 v2 只读不再 spawn 它。返回「被调用」标记路径，测试断言其不存在即证明未 spawn。
fn stub_uninvoked_cli(fixture: &Fixture) -> PathBuf {
    let called = fixture.root.join("cli-called.log");
    write_fake_cli(
        &fixture.cli,
        &format!("printf CALLED > {}", shell_quote(&called)),
    );
    called
}

fn byte_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, current: &Path, output: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries = match fs::read_dir(current) {
            Ok(entries) => entries.map(|entry| entry.unwrap()).collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("无法读取 {}: {error}", current.display()),
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                output.push((relative.clone(), b"<directory>".to_vec()));
                visit(root, &path, output);
            } else {
                output.push((relative, fs::read(&path).unwrap()));
            }
        }
    }

    let mut output = Vec::new();
    visit(root, root, &mut output);
    output
}

struct Fixture {
    root: PathBuf,
    dashboard_state: PathBuf,
    local_path: PathBuf,
    repo: PathBuf,
    cli: PathBuf,
    data_root: PathBuf,
    project_id: Uuid,
}

impl Fixture {
    fn shared(tag: &str, bind_current_machine: bool) -> Self {
        let root = make_tempdir(tag);
        let dashboard_state = root.join("dashboard-state");
        let local_path = root.join("local.yaml");
        let repo = root.join("project;not-a-shell-command");
        let cli = root.join("bin").join("repowiki");
        let data_root = root.join("repowiki-data");
        fs::create_dir_all(&repo).unwrap();
        write_git_origin(&repo, "https://example.test/team/example.git");
        fs::create_dir_all(&data_root).unwrap();
        let project_id = Uuid::new_v4();
        let mut machines = BTreeMap::new();
        machines.insert(
            if bind_current_machine {
                "machine-a".to_string()
            } else {
                "machine-b".to_string()
            },
            repo.display().to_string(),
        );
        let now = Utc::now();
        let project = RegistryProject {
            schema_version: CURRENT_SCHEMA_VERSION,
            project_id,
            name: "example".to_string(),
            git_url: "https://example.test/team/example.git".to_string(),
            git_url_aliases: Vec::new(),
            sync: true,
            machines,
            created_at: now,
            updated_at: now,
        };
        let mut local = LocalConfig::new("machine-a");
        local.repowiki_cli_path = Some(cli.display().to_string());
        local.repowiki_data_root = Some(data_root.display().to_string());
        let mut projects = BTreeMap::new();
        projects.insert(project_id, project);
        Registry {
            projects,
            local: Some(local),
        }
        .save_at(&dashboard_state, &local_path)
        .unwrap();
        Self {
            root,
            dashboard_state,
            local_path,
            repo,
            cli,
            data_root,
            project_id,
        }
    }

    fn selector(&self) -> RepoWikiProjectSelector {
        RepoWikiProjectSelector::Shared {
            project_id: self.project_id,
        }
    }
}

#[test]
fn status_uses_registry_bound_repo_and_exact_argv_without_shell() {
    let fixture = Fixture::shared("status-argv", true);
    let argv_log = fixture.root.join("argv.log");
    let shell_marker = fixture.root.join("not-a-shell-command");
    write_fake_cli(
        &fixture.cli,
        &format!(
            "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' '{{\"protocol_version\":1,\"command\":\"status\",\"ok\":true,\"result\":{{\"initialization\":\"initialized\"}},\"warnings\":[]}}'",
            shell_quote(&argv_log)
        ),
    );

    let envelope = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap();

    assert!(envelope.ok);
    assert_eq!(envelope.command, "status");
    assert_eq!(
        fs::read_to_string(&argv_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec![
            "--repo",
            fs::canonicalize(&fixture.repo).unwrap().to_str().unwrap(),
            "--data-root",
            fixture.data_root.to_str().unwrap(),
            "--json",
            "status",
        ]
    );
    assert!(!shell_marker.exists(), "repo 参数不得被 shell 解释");
}

#[test]
fn status_is_strictly_read_only_even_with_legacy_pending_config() {
    let fixture = Fixture::shared("legacy-read-only", true);
    let argv_log = fixture.root.join("argv.log");
    write_fake_cli(
        &fixture.cli,
        &format!(
            "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' '{{\"protocol_version\":1,\"command\":\"status\",\"ok\":true,\"result\":{{}},\"warnings\":[]}}'",
            shell_quote(&argv_log)
        ),
    );
    let mut local = fs::read_to_string(&fixture.local_path).unwrap();
    local.push_str("pending_sync_projects:\n  - schema_version: 1\n    name: legacy-project\n");
    fs::write(&fixture.local_path, local).unwrap();

    let backup_root = fixture.local_path.parent().unwrap().join("backups");
    let dashboard_before = byte_tree(&fixture.dashboard_state);
    let local_before = fs::read(&fixture.local_path).unwrap();
    let backup_before = byte_tree(&backup_root);

    let envelope = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap();

    assert!(envelope.ok);
    assert!(
        argv_log.exists(),
        "假 CLI 应实际执行，避免测试只覆盖前置失败"
    );
    assert_eq!(byte_tree(&fixture.dashboard_state), dashboard_before);
    assert_eq!(fs::read(&fixture.local_path).unwrap(), local_before);
    assert_eq!(byte_tree(&backup_root), backup_before);
}

#[test]
fn shared_project_without_current_machine_binding_is_rejected_before_spawn() {
    let fixture = Fixture::shared("foreign-machine", false);
    let argv_log = fixture.root.join("argv.log");
    write_fake_cli(
        &fixture.cli,
        &format!(
            "printf '%s\\n' \"$@\" > {}\nexit 99",
            shell_quote(&argv_log)
        ),
    );

    let error = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "project_not_bound");
    assert!(!argv_log.exists());
}

#[test]
fn shared_project_origin_mismatch_is_rejected_before_repowiki_spawn() {
    let fixture = Fixture::shared("origin-mismatch", true);
    let argv_log = fixture.root.join("argv.log");
    write_git_origin(&fixture.repo, "https://example.test/other/repository.git");
    write_fake_cli(
        &fixture.cli,
        &format!("printf '%s\\n' CALLED > {}", shell_quote(&argv_log)),
    );

    let error = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "project_binding_mismatch");
    assert!(!argv_log.exists());
}

#[test]
fn shared_project_linked_worktree_uses_common_git_config_for_binding() {
    let fixture = Fixture::shared("linked-worktree", true);
    let common_git = fixture.root.join("main-repository.git");
    fs::rename(fixture.repo.join(".git"), &common_git).unwrap();
    let worktree_git = common_git.join("worktrees").join("feature-a");
    fs::create_dir_all(&worktree_git).unwrap();
    fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
    fs::write(
        fixture.repo.join(".git"),
        format!("gitdir: {}\n", worktree_git.display()),
    )
    .unwrap();
    write_fake_cli(
        &fixture.cli,
        "printf '%s\n' '{\"protocol_version\":1,\"command\":\"status\",\"ok\":true,\"result\":{},\"warnings\":[]}'",
    );

    let envelope = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap();

    assert!(envelope.ok);
}

#[test]
fn linked_worktree_rejects_commondir_outside_standard_git_layout() {
    let fixture = Fixture::shared("linked-worktree-escape", true);
    let common_git = fixture.root.join("main-repository.git");
    fs::rename(fixture.repo.join(".git"), &common_git).unwrap();
    let worktree_git = common_git.join("worktrees").join("feature-a");
    fs::create_dir_all(&worktree_git).unwrap();
    let unrelated = fixture.root.join("unrelated-git-dir");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(
        unrelated.join("config"),
        "[remote \"origin\"]\nurl = https://example.test/team/example.git\n",
    )
    .unwrap();
    fs::write(
        worktree_git.join("commondir"),
        format!("{}\n", unrelated.display()),
    )
    .unwrap();
    fs::write(
        fixture.repo.join(".git"),
        format!("gitdir: {}\n", worktree_git.display()),
    )
    .unwrap();
    let called = fixture.root.join("called.log");
    write_fake_cli(
        &fixture.cli,
        &format!("printf CALLED > {}", shell_quote(&called)),
    );

    let error = repowiki::status(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "project_binding_mismatch");
    assert!(!called.exists());
}

// graph 系列仍 spawn CLI：验证只发白名单固定 argv、查询文本不被 shell 解释。
// read 系列已改为直读 v2 产物（见下方 v2 用例），不再走 CLI，故此处只覆盖 graph。
#[test]
fn graph_functions_emit_only_whitelisted_argv() {
    let fixture = Fixture::shared("graph-whitelist", true);
    let argv_log = fixture.root.join("argv.log");
    let shell_marker = fixture.root.join("graph-shell-marker");
    write_fake_cli(
        &fixture.cli,
        &format!(
            "printf '%s\\n' CALL >> {log}\nprintf '%s\\n' \"$@\" >> {log}\nprintf '{{\"protocol_version\":1,\"command\":\"%s\",\"ok\":true,\"result\":{{}},\"warnings\":[]}}\\n' \"$6 $7\"",
            log = shell_quote(&argv_log),
        ),
    );
    let selector = fixture.selector();
    let node = "a".repeat(64);
    let search = format!("order;touch {}", shell_marker.display());

    repowiki::graph_search(
        &fixture.dashboard_state,
        &fixture.local_path,
        &selector,
        &search,
        20,
    )
    .unwrap();
    repowiki::graph_show(
        &fixture.dashboard_state,
        &fixture.local_path,
        &selector,
        &node,
    )
    .unwrap();
    repowiki::graph_callers(
        &fixture.dashboard_state,
        &fixture.local_path,
        &selector,
        &node,
        2,
        50,
    )
    .unwrap();
    repowiki::graph_callees(
        &fixture.dashboard_state,
        &fixture.local_path,
        &selector,
        &node,
        2,
        50,
    )
    .unwrap();
    repowiki::graph_impact(
        &fixture.dashboard_state,
        &fixture.local_path,
        &selector,
        &node,
        2,
        50,
    )
    .unwrap();

    let logged = fs::read_to_string(&argv_log).unwrap();
    for command in [
        "graph\nsearch\n--text",
        "graph\nshow\n--node",
        "graph\ncallers\n--node",
        "graph\ncallees\n--node",
        "graph\nimpact\n--node",
    ] {
        assert!(
            logged.contains(command),
            "缺少固定 argv：{command}\n{logged}"
        );
    }
    for forbidden in [
        "build", "update", "prepare", "publish", "review", "--sql", "cypher",
    ] {
        assert!(!logged.lines().any(|line| line == forbidden));
    }
    assert!(logged.contains(&search));
    assert!(!shell_marker.exists(), "查询文本不得被 shell 解释");
}

// read_project：直读 v2 navigation，剔除 source_anchors，
// 保留 title/path/page_type/parent_page_id/children，且全程不 spawn CLI。
#[test]
fn v2_read_functions_return_slim_navigation_without_spawning_cli() {
    let fixture = Fixture::shared("v2-read-navigation", true);
    let called = stub_uninvoked_cli(&fixture);
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[
            ("docs/home.md", "# 项目概览\n"),
            ("docs/arch.md", "# 架构说明\n\n正文。\n"),
        ],
    );
    let selector = fixture.selector();

    for envelope in [
        repowiki::read_project(&fixture.dashboard_state, &fixture.local_path, &selector).unwrap(),
    ] {
        assert!(envelope.ok);
        let navigation = &envelope.result["navigation"];
        assert_eq!(navigation["schema_version"].as_u64(), Some(2));
        assert_eq!(navigation["home_page_id"].as_str(), Some("home"));
        assert_eq!(navigation["layout_version"].as_u64(), Some(7));
        let pages = navigation["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2);
        for page in pages {
            assert!(
                page.get("source_anchors").is_none(),
                "source_anchors 必须被剔除：{page}"
            );
        }
        let home = &pages[0];
        assert_eq!(home["page_id"].as_str(), Some("home"));
        assert_eq!(home["title"].as_str(), Some("项目概览"));
        assert_eq!(home["path"].as_str(), Some("docs/home.md"));
        assert_eq!(home["page_type"].as_str(), Some("overview"));
        assert!(home["parent_page_id"].is_null());
        assert_eq!(home["children"], serde_json::json!(["arch"]));
        let arch = &pages[1];
        assert_eq!(arch["parent_page_id"].as_str(), Some("home"));
        assert_eq!(arch["page_type"].as_str(), Some("section"));
    }

    assert!(!called.exists(), "v2 只读不得 spawn RepoWiki CLI");
}

// 合成的 v2 state/generation.json：哈希、代次与时间均为虚构数据，用来验证摘要字段抽取。
fn sample_generation() -> String {
    serde_json::json!({
        "schema_version": 2,
        "generation_id": "rwwiki_111111111111_222222222222",
        "graph_generation_id": "rwgen_333333333333_444444444444",
        "layout_version": 3,
        "mode": "update",
        "status": "complete",
        "published_at": "2020-01-01T00:00:00+00:00",
        "source": {
            "branch_ref": "refs/heads/demo-feature",
            "generated_at": "2020-01-02T00:00:00+00:00",
            "head_commit": "1111111111111111111111111111111111111111"
        },
        "pages": {
            "home": {"content_sha256": "a".repeat(64)},
            "arch": {"content_sha256": "b".repeat(64)},
            "extra": {"content_sha256": "c".repeat(64)}
        }
    })
    .to_string()
}

// read_generations：改为直读 v2 state/generation.json 出真实代次，不再回传导航、不 spawn CLI。
// 这是 T1 的修复点——CLI status 的 wiki 段走 v1 校验器，对 v2 wiki 恒报 damaged / 代次 null。
#[test]
fn read_generations_returns_v2_generation_summary_without_spawning_cli() {
    let fixture = Fixture::shared("v2-read-generations", true);
    let called = stub_uninvoked_cli(&fixture);
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[("state/generation.json", &sample_generation())],
    );

    let envelope =
        repowiki::read_generations(&fixture.dashboard_state, &fixture.local_path, &fixture.selector())
            .unwrap();

    assert!(envelope.ok);
    assert_eq!(envelope.command, "read generations");
    let wiki = &envelope.result["wiki"];
    assert_eq!(
        wiki["generation_id"].as_str(),
        Some("rwwiki_111111111111_222222222222")
    );
    assert_eq!(wiki["status"].as_str(), Some("complete"));
    assert_eq!(wiki["integrity"].as_str(), Some("healthy"));
    assert_eq!(wiki["layout_version"].as_u64(), Some(3));
    assert_eq!(wiki["page_total"].as_u64(), Some(3));
    // generated_at 优先取 source.generated_at（本次更新时间），不是首次 published_at。
    assert_eq!(
        wiki["generated_at"].as_str(),
        Some("2020-01-02T00:00:00+00:00")
    );
    assert_eq!(
        wiki["head_commit"].as_str(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(
        envelope.result["branch"]["branch_ref"].as_str(),
        Some("refs/heads/demo-feature")
    );
    // pages 是数百条的明细，摘要卡只需要总数，不得整包回传。
    assert!(wiki.get("pages").is_none(), "pages 明细必须被剔除：{wiki}");
    // 无 review.json 时按「零审查」算，且不报错。
    let totals = &envelope.result["review"]["totals"];
    assert_eq!(totals["total"].as_u64(), Some(3));
    assert_eq!(totals["reviewed"].as_u64(), Some(0));
    assert_eq!(totals["unreviewed"].as_u64(), Some(3));
    assert!(!called.exists(), "v2 代次直读不得 spawn RepoWiki CLI");
}

// 审查计数只认 v2 代次里登记的 page_id：上游 review.json 校验器只接受 v1 固定 page_id，
// 残留的 v1 id（如 00-overview）不得被算进 v2 的已审数。
#[test]
fn read_generations_counts_only_reviewed_pages_known_to_v2_generation() {
    let fixture = Fixture::shared("v2-read-generations-review", true);
    let called = stub_uninvoked_cli(&fixture);
    let review = serde_json::json!({
        "schema_version": 1,
        "entries": {
            "home": {"reviewed_at": "2020-01-02T01:00:00Z"},
            "00-overview": {"reviewed_at": "2026-07-01T12:00:00Z"}
        }
    })
    .to_string();
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[
            ("state/generation.json", &sample_generation()),
            ("state/review.json", &review),
        ],
    );

    let envelope =
        repowiki::read_generations(&fixture.dashboard_state, &fixture.local_path, &fixture.selector())
            .unwrap();

    let totals = &envelope.result["review"]["totals"];
    assert_eq!(totals["reviewed"].as_u64(), Some(1), "v1 残留 id 不得计入");
    assert_eq!(totals["unreviewed"].as_u64(), Some(2));
    assert!(!called.exists(), "v2 代次直读不得 spawn RepoWiki CLI");
}

// 非 v2 代次（schema_version != 2）必须拒绝，不能把 v1 代次当 v2 展示。
#[test]
fn read_generations_rejects_non_v2_generation_schema() {
    let fixture = Fixture::shared("v2-read-generations-schema", true);
    let _called = stub_uninvoked_cli(&fixture);
    let v1_generation = serde_json::json!({
        "schema_version": 1,
        "generation_id": "rwwiki_111111111111_222222222222",
        "status": "complete"
    })
    .to_string();
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[("state/generation.json", &v1_generation)],
    );

    let error = repowiki::read_generations(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "repowiki_protocol_violation");
}

// read_page：用 v2 page_id 命中 navigation 登记的 path，读回对应 md 正文。
#[test]
fn read_page_returns_markdown_for_known_v2_page_id() {
    let fixture = Fixture::shared("v2-read-page", true);
    let called = stub_uninvoked_cli(&fixture);
    let body = "# 架构说明\n\n这是架构页面正文。\n";
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[("docs/home.md", "# 项目概览\n"), ("docs/arch.md", body)],
    );

    let envelope = repowiki::read_page(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "arch",
    )
    .unwrap();

    assert!(envelope.ok);
    assert_eq!(envelope.command, "read page");
    assert_eq!(envelope.result["page_id"].as_str(), Some("arch"));
    assert_eq!(envelope.result["title"].as_str(), Some("架构说明"));
    assert_eq!(envelope.result["path"].as_str(), Some("docs/arch.md"));
    assert_eq!(envelope.result["content"].as_str(), Some(body));
    assert!(!called.exists(), "v2 只读不得 spawn RepoWiki CLI");
}

// read_page：navigation 里没有的 page_id → InvalidArgument。
#[test]
fn read_page_rejects_unknown_v2_page_id() {
    let fixture = Fixture::shared("v2-read-page-unknown", true);
    stub_uninvoked_cli(&fixture);
    write_repowiki(
        &fixture.repo,
        &sample_navigation(),
        &[
            ("docs/home.md", "# 项目概览\n"),
            ("docs/arch.md", "# 架构\n"),
        ],
    );

    let error = repowiki::read_page(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "does-not-exist",
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_repowiki_argument");
}

// schema_version 非 2 的 navigation → read_project 返回 ProtocolViolation。
#[test]
fn read_project_rejects_non_v2_navigation_schema() {
    let fixture = Fixture::shared("v2-read-wrong-schema", true);
    stub_uninvoked_cli(&fixture);
    let navigation = serde_json::json!({
        "schema_version": 1,
        "home_page_id": "home",
        "pages": [{"page_id": "home", "title": "旧版", "path": "docs/home.md"}]
    })
    .to_string();
    write_repowiki(&fixture.repo, &navigation, &[("docs/home.md", "# 旧版\n")]);

    let error = repowiki::read_project(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "repowiki_protocol_violation");
}

// 安全：navigation 登记的 path 含 `../` 试图逃出 `.repowiki/`，read_page 必须拒绝，
// 且绝不读到 `.repowiki/` 之外的机密文件。
#[test]
fn read_page_rejects_navigation_path_with_parent_escape() {
    let fixture = Fixture::shared("v2-path-parent-escape", true);
    stub_uninvoked_cli(&fixture);
    // 在 repo 根（.repowiki 之外）放一个机密文件，验证它读不到。
    fs::write(fixture.repo.join("outside-secret.md"), "TOP SECRET").unwrap();
    let navigation = serde_json::json!({
        "schema_version": 2,
        "home_page_id": "leak",
        "layout_version": 1,
        "pages": [{
            "page_id": "leak",
            "title": "父目录逃逸",
            "path": "../outside-secret.md",
            "page_type": "overview",
            "parent_page_id": null,
            "order": 0,
            "children": []
        }]
    })
    .to_string();
    write_repowiki(&fixture.repo, &navigation, &[]);

    let error = repowiki::read_page(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "leak",
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_repowiki_argument");
}

// 安全：navigation 登记绝对路径必须被拒（不得读取 `.repowiki/` 之外的任意文件）。
#[test]
fn read_page_rejects_absolute_navigation_path() {
    let fixture = Fixture::shared("v2-path-absolute", true);
    stub_uninvoked_cli(&fixture);
    let secret = fixture.root.join("absolute-secret.md");
    fs::write(&secret, "TOP SECRET").unwrap();
    let navigation = serde_json::json!({
        "schema_version": 2,
        "home_page_id": "leak",
        "layout_version": 1,
        "pages": [{
            "page_id": "leak",
            "title": "绝对路径",
            "path": secret.display().to_string(),
            "page_type": "overview",
            "parent_page_id": null,
            "order": 0,
            "children": []
        }]
    })
    .to_string();
    write_repowiki(&fixture.repo, &navigation, &[]);

    let error = repowiki::read_page(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "leak",
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_repowiki_argument");
}

// 安全：`.repowiki/` 内的符号链接若指向目录外，canonical 前缀校验必须拦下。
#[test]
fn read_page_rejects_symlink_escaping_repowiki() {
    let fixture = Fixture::shared("v2-path-symlink", true);
    stub_uninvoked_cli(&fixture);
    let secret = fixture.root.join("symlink-secret.md");
    fs::write(&secret, "TOP SECRET").unwrap();
    let navigation = serde_json::json!({
        "schema_version": 2,
        "home_page_id": "leak",
        "layout_version": 1,
        "pages": [{
            "page_id": "leak",
            "title": "软链逃逸",
            "path": "docs/leak.md",
            "page_type": "overview",
            "parent_page_id": null,
            "order": 0,
            "children": []
        }]
    })
    .to_string();
    write_repowiki(&fixture.repo, &navigation, &[]);
    let link = fixture.repo.join(".repowiki").join("docs").join("leak.md");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&secret, &link).unwrap();

    let error = repowiki::read_page(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "leak",
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_repowiki_argument");
}

// graph 参数预算：非法 node / 空查询 / 越界 limit 在 spawn 前被拒。
#[test]
fn invalid_node_and_query_budgets_are_rejected_before_spawn() {
    let fixture = Fixture::shared("invalid-arguments", true);
    let argv_log = fixture.root.join("argv.log");
    write_fake_cli(
        &fixture.cli,
        &format!(
            "printf '%s\\n' \"$@\" > {}\nexit 99",
            shell_quote(&argv_log)
        ),
    );
    let selector = fixture.selector();
    let invalid_results = [
        repowiki::graph_show(
            &fixture.dashboard_state,
            &fixture.local_path,
            &selector,
            "not-a-node",
        ),
        repowiki::graph_search(
            &fixture.dashboard_state,
            &fixture.local_path,
            &selector,
            "   ",
            20,
        ),
        repowiki::graph_search(
            &fixture.dashboard_state,
            &fixture.local_path,
            &selector,
            "order",
            0,
        ),
    ];

    for result in invalid_results {
        assert_eq!(result.unwrap_err().code(), "invalid_repowiki_argument");
    }
    assert!(!argv_log.exists());
}

#[test]
fn malformed_or_inconsistent_envelopes_are_rejected_without_stderr_leakage() {
    let cases = [
        ("not-json", 0),
        (
            r#"{"protocol_version":2,"command":"status","ok":true,"result":{},"warnings":[]}"#,
            0,
        ),
        (
            r#"{"protocol_version":1,"command":"read project","ok":true,"result":{},"warnings":[]}"#,
            0,
        ),
        (
            r#"{"protocol_version":1,"command":"status","ok":true,"result":[],"warnings":[]}"#,
            0,
        ),
        (
            r#"{"protocol_version":1,"command":"status","ok":false,"result":{},"warnings":[],"error":{"code":"","message":"","details":{}}}"#,
            6,
        ),
        (
            r#"{"protocol_version":1,"command":"status","ok":true,"result":{},"warnings":[]}"#,
            7,
        ),
    ];
    for (index, (stdout, exit_code)) in cases.into_iter().enumerate() {
        let fixture = Fixture::shared(&format!("bad-protocol-{index}"), true);
        write_fake_cli(
            &fixture.cli,
            &format!(
                "printf '%s\\n' 'MODEL-SECRET-MUST-NOT-LEAK' >&2\nprintf '%s\\n' {}\nexit {exit_code}",
                shell_quote_text(stdout),
            ),
        );

        let error = repowiki::status(
            &fixture.dashboard_state,
            &fixture.local_path,
            &fixture.selector(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "repowiki_protocol_violation");
        assert!(!error.to_string().contains("MODEL-SECRET-MUST-NOT-LEAK"));
    }
}

// graph 仍走 CLI：合法的远端错误信封应作为数据原样返回（不当成协议违规吞掉）。
#[test]
fn valid_remote_error_envelope_is_returned_as_data() {
    let fixture = Fixture::shared("remote-error", true);
    let envelope = r#"{"protocol_version":1,"command":"graph search","ok":false,"result":{},"warnings":[],"error":{"code":"graph_baseline_missing","message":"尚无正式 Wiki","details":{}}}"#;
    write_fake_cli(
        &fixture.cli,
        &format!("printf '%s\\n' {}\nexit 6", shell_quote_text(envelope)),
    );

    let result = repowiki::graph_search(
        &fixture.dashboard_state,
        &fixture.local_path,
        &fixture.selector(),
        "order",
        20,
    )
    .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "graph_baseline_missing");
}

#[test]
fn project_selector_uses_stable_frontend_json_shape() {
    let project_id = Uuid::new_v4();
    let shared = RepoWikiProjectSelector::Shared { project_id };
    assert_eq!(
        serde_json::to_value(&shared).unwrap(),
        serde_json::json!({"kind": "shared", "projectId": project_id})
    );
    let restored: RepoWikiProjectSelector =
        serde_json::from_value(serde_json::json!({"kind": "shared", "projectId": project_id}))
            .unwrap();
    assert_eq!(restored, shared);
}

#[test]
fn missing_or_relative_local_paths_are_rejected_before_spawn() {
    enum ConfigCase {
        MissingCli,
        RelativeCli,
        MissingDataRoot,
        RelativeDataRoot,
    }
    let cases = [
        (ConfigCase::MissingCli, "repowiki_cli_path_missing"),
        (ConfigCase::RelativeCli, "invalid_repowiki_cli_path"),
        (ConfigCase::MissingDataRoot, "repowiki_data_root_missing"),
        (ConfigCase::RelativeDataRoot, "invalid_repowiki_data_root"),
    ];
    for (index, (case, expected_code)) in cases.into_iter().enumerate() {
        let fixture = Fixture::shared(&format!("config-case-{index}"), true);
        write_fake_cli(
            &fixture.cli,
            "printf '%s\\n' '{\"protocol_version\":1,\"command\":\"status\",\"ok\":true,\"result\":{},\"warnings\":[]}'",
        );
        let mut local: LocalConfig =
            serde_yaml::from_str(&fs::read_to_string(&fixture.local_path).unwrap()).unwrap();
        match case {
            ConfigCase::MissingCli => local.repowiki_cli_path = None,
            ConfigCase::RelativeCli => local.repowiki_cli_path = Some("bin/repowiki".to_string()),
            ConfigCase::MissingDataRoot => local.repowiki_data_root = None,
            ConfigCase::RelativeDataRoot => {
                local.repowiki_data_root = Some("repowiki-data".to_string())
            }
        }
        fs::write(&fixture.local_path, serde_yaml::to_string(&local).unwrap()).unwrap();

        let error = repowiki::status(
            &fixture.dashboard_state,
            &fixture.local_path,
            &fixture.selector(),
        )
        .unwrap_err();

        assert_eq!(error.code(), expected_code);
    }
}
