#![cfg(windows)]
//! 最终原生包与看板联合验收，使用 Windows CLI 矩阵留下的合成仓。
use dashboard_core::{
    config::{LocalConfig, PrivateProject, Registry},
    repowiki::{self, RepoWikiProjectSelector},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(root).unwrap().to_owned(),
                format!("{:x}", Sha256::digest(fs::read(e.path()).unwrap())),
            )
        })
        .collect()
}

#[test]
#[ignore = "需要显式 REPOWIKI_INTEROP_JSON 指向真实包验收现场"]
fn packaged_windows_cli_and_direct_v2_reads_are_read_only() {
    let context: Value = serde_json::from_slice(
        &fs::read(std::env::var_os("REPOWIKI_INTEROP_JSON").expect("缺少合成仓上下文")).unwrap(),
    )
    .unwrap();
    let repo = Path::new(context["repo"].as_str().unwrap());
    let data = Path::new(context["data"].as_str().unwrap());
    let exe = Path::new(context["exe"].as_str().unwrap());
    assert!(exe
        .file_name()
        .unwrap()
        .eq_ignore_ascii_case("repowiki.exe"));
    let root = repo.parent().unwrap().join("dashboard-interop");
    fs::create_dir_all(&root).unwrap();
    let state = root.join("state");
    let local = root.join("local.yaml");
    let selector = RepoWikiProjectSelector::Private {
        name: "package".into(),
    };
    let mut cfg = LocalConfig::new("windows-interop");
    cfg.repowiki_cli_path = Some(exe.display().to_string());
    cfg.repowiki_data_root = Some(data.display().to_string());
    cfg.private_projects.push(PrivateProject {
        schema_version: dashboard_core::config::CURRENT_SCHEMA_VERSION,
        project_id: None,
        sync: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        name: "package".into(),
        git_url: "https://example.test/team/windows-demo.git".into(),
        path: repo.display().to_string(),
    });
    Registry {
        projects: BTreeMap::new(),
        local: Some(cfg),
    }
    .save_at(&state, &local)
    .unwrap();
    let before_repo = snapshot(repo);
    let before_data = snapshot(data);
    assert!(repowiki::status(&state, &local, &selector).unwrap().ok);
    let navigation = repowiki::read_project(&state, &local, &selector).unwrap();
    assert!(navigation.ok);
    for page in navigation.result["navigation"]["pages"].as_array().unwrap() {
        let result =
            repowiki::read_page(&state, &local, &selector, page["page_id"].as_str().unwrap())
                .unwrap();
        let expected =
            fs::read_to_string(repo.join(".repowiki").join(page["path"].as_str().unwrap()))
                .unwrap();
        assert_eq!(
            result.result["content"].as_str().unwrap(),
            expected,
            "页面必须与原生包正式产物逐字一致"
        );
    }
    assert!(
        repowiki::read_generations(&state, &local, &selector)
            .unwrap()
            .ok
    );
    let nodes = repowiki::graph_search(&state, &local, &selector, "entry", 20).unwrap();
    assert!(nodes.ok);
    let node = nodes.result["nodes"][0]["node_id"].as_str().unwrap();
    assert!(
        repowiki::graph_show(&state, &local, &selector, node)
            .unwrap()
            .ok
    );
    for query in [
        repowiki::graph_callers,
        repowiki::graph_callees,
        repowiki::graph_impact,
    ] {
        assert!(query(&state, &local, &selector, node, 2, 100).unwrap().ok);
    }
    assert_eq!(snapshot(repo), before_repo, "看板读取改写了项目文件");
    assert_eq!(snapshot(data), before_data, "看板读取改写了私有派生数据");
}
