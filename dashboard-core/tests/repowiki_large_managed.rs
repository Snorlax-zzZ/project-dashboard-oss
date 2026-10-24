#![cfg(windows)]
//! 发布证据可大于导航展示预算，代次核验应按上游 managed 文件预算处理。
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
};
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[test]
fn git_includes_cannot_spoof_project_binding() {
    let (root, repo, state, config, selector) = fixture(2);
    for section in ["[include]", "[include] # comment", "[include] ; comment"] {
        fs::write(repo.join(".git/config"),format!("{section}\npath=../../untrusted-config\n[remote \"origin\"]\nurl=https://example.test/team/large.git\n")).unwrap();
        let result = repowiki::read_generations(&state, &config, &selector);
        assert!(matches!(result, Err(repowiki::RepoWikiError::ProjectBindingMismatch)),
                "应拒绝带注释的 include: {section}");
    }
    fs::remove_dir_all(root).unwrap();
}
fn fixture(
    inventory_bytes: usize,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, RepoWikiProjectSelector) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("large-wiki-{}", uuid::Uuid::new_v4()));
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(
        repo.join(".git/config"),
        "[remote \"origin\"]\nurl=https://example.test/team/large.git\n",
    )
    .unwrap();
    let base = repo.join(".repowiki");
    let mut managed = BTreeMap::new();
    for (path, bytes) in [
        ("README.md", b"# wiki".to_vec()),
        ("docs/home.md", b"# home".to_vec()),
        (
            "navigation.json",
            br#"{"schema_version":2,"pages":[]}"#.to_vec(),
        ),
        ("catalog/accepted.json", b"{}".to_vec()),
        ("catalog/inventory.json", {
            let mut bytes = vec![b' '; inventory_bytes.max(2)];
            bytes[..2].copy_from_slice(b"{}");
            bytes
        }),
        ("coverage/coverage.json", b"{}".to_vec()),
        ("evidence/map.json", b"{}".to_vec()),
        ("state/quality-report.json", b"{}".to_vec()),
    ] {
        let target = base.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, &bytes).unwrap();
        managed.insert(path, hash(&bytes));
    }
    let generation = "rwwiki_012345678901_012345678901";
    let manifest=json!({"schema_version":2,"generation_id":generation,"managed_files":managed,"managed_fingerprint":hash(&serde_json::to_vec(&managed).unwrap())}).to_string();
    fs::write(base.join("state/content-manifest.json"), &manifest).unwrap();
    fs::write(base.join("state/generation.json"),json!({"schema_version":2,"status":"complete","generation_id":generation,"manifest_sha256":hash(manifest.as_bytes()),"pages":{}}).to_string()).unwrap();
    let mut local = LocalConfig::new("large-wiki-test");
    local.private_projects.push(PrivateProject {
        schema_version: dashboard_core::config::CURRENT_SCHEMA_VERSION,
        project_id: None,
        sync: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        name: "large".into(),
        git_url: "https://example.test/team/large.git".into(),
        path: repo.display().to_string(),
    });
    let cli = root.join("repowiki.exe");
    fs::copy(std::env::current_exe().unwrap(), &cli).unwrap();
    local.repowiki_cli_path = Some(cli.display().to_string());
    local.repowiki_data_root = Some(root.join("data").display().to_string());
    let state = root.join("state");
    let config = root.join("local.yaml");
    Registry {
        projects: BTreeMap::new(),
        local: Some(local),
    }
    .save_at(&state, &config)
    .unwrap();
    (
        root,
        repo,
        state,
        config,
        RepoWikiProjectSelector::Private {
            name: "large".into(),
        },
    )
}
#[test]
fn generation_verifies_large_managed_evidence_without_navigation_limit() {
    let (root, _, state, config, selector) = fixture(33 * 1024 * 1024);
    let result = repowiki::read_generations(&state, &config, &selector);
    fs::remove_dir_all(root).unwrap();
    assert!(result.unwrap().ok);
}

#[test]
fn malformed_or_unbound_review_cannot_masquerade_as_a_verified_page() {
    let (root, repo, state, config, selector) = fixture(2);
    let base = repo.join(".repowiki");
    fs::write(base.join("state/review.json"), b"{invalid").unwrap();
    assert!(repowiki::read_generations(&state, &config, &selector).is_err());
    fs::write(base.join("state/review.json"), b"{}").unwrap();
    let navigation=json!({"schema_version":2,"pages":[{"page_id":"review","path":"state/review.json","title":"review"}]}).to_string();
    fs::write(base.join("navigation.json"), &navigation).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(base.join("state/content-manifest.json")).unwrap())
            .unwrap();
    manifest["managed_files"]["navigation.json"] = json!(hash(navigation.as_bytes()));
    manifest["managed_fingerprint"] = json!(hash(
        &serde_json::to_vec(&manifest["managed_files"]).unwrap()
    ));
    let raw = manifest.to_string();
    fs::write(base.join("state/content-manifest.json"), &raw).unwrap();
    let mut generation: serde_json::Value =
        serde_json::from_slice(&fs::read(base.join("state/generation.json")).unwrap()).unwrap();
    generation["manifest_sha256"] = json!(hash(raw.as_bytes()));
    fs::write(base.join("state/generation.json"), generation.to_string()).unwrap();
    assert!(repowiki::read_page(&state, &config, &selector, "review").is_err());
    fs::remove_dir_all(root).unwrap();
}
