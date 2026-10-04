// M2-3.7 集成测试：注册表加载 / 合并 / CRUD / schema validator 拒收未声明字段

use chrono::Utc;
use dashboard_core::config::{
    EditRegistryProject, LocalConfig, PrivateProject, Registry, RegistryProject,
    CURRENT_SCHEMA_VERSION,
};
use dashboard_core::schema::{
    check_sensitive_fields, validate_event, validate_registry_project, validate_snapshot,
    SchemaError,
};
use dashboard_core::tracker::normalize_git_url;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-config-test-{}-{}-{}",
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

fn sample_registry_project() -> RegistryProject {
    let mut machines = BTreeMap::new();
    machines.insert("DESKTOP-DEMO01".to_string(), "F:\\project-dashboard".to_string());
    machines.insert("example-mac".to_string(), "~/code/project-dashboard".to_string());
    let now = Utc::now();
    RegistryProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        project_id: Uuid::new_v4(),
        name: "project-dashboard".to_string(),
        git_url: "https://github.com/example/project-dashboard".to_string(),
        git_url_aliases: Vec::new(),
        sync: true,
        machines,
        created_at: now,
        updated_at: now,
    }
}

fn sample_private_project(name: &str) -> PrivateProject {
    let now = Utc::now();
    PrivateProject {
        schema_version: CURRENT_SCHEMA_VERSION,
        name: name.to_string(),
        project_id: None,
        path: format!("F:\\{name}"),
        git_url: format!("https://gitlab.company.internal/{name}"),
        sync: false,
        created_at: now,
        updated_at: now,
    }
}

fn seed_local(tmp: &Path) -> PathBuf {
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let cfg = LocalConfig::new("DESKTOP-DEMO01");
    let yaml = serde_yaml::to_string(&cfg).unwrap();
    fs::write(&local_path, yaml).unwrap();
    local_path
}

#[test]
fn load_empty_dashboard_state_returns_empty_registry() {
    let tmp = make_tempdir("empty");
    let local_path = seed_local(&tmp);
    let registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    assert!(registry.projects.is_empty());
    assert!(registry.local.is_some());
    assert_eq!(registry.local.as_ref().unwrap().machine_id, "DESKTOP-DEMO01");
    assert_eq!(registry.local.as_ref().unwrap().sync_tick_minutes, 3); // 默认值
}

#[test]
fn roundtrip_registry_project_save_load() {
    let tmp = make_tempdir("rt");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let sample = sample_registry_project();
    let id = sample.project_id;
    registry.projects.insert(id, sample.clone());
    registry
        .save_at(&tmp.join("dashboard-state"), &local_path)
        .unwrap();
    // 落盘后 registry/projects/<uuid>.yaml 存在
    let file = tmp
        .join("dashboard-state")
        .join("registry")
        .join("projects")
        .join(format!("{}.yaml", id));
    assert!(file.is_file());
    // 重新加载 → 一致
    let reloaded = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let got = reloaded.get_shared(&id).unwrap();
    assert_eq!(got.name, "project-dashboard");
    assert_eq!(got.git_url, sample.git_url);
    assert!(got.sync);
    assert_eq!(got.machines.len(), 2);
}

#[cfg(unix)]
#[test]
fn registry_save_atomically_replaces_local_yaml() {
    use std::os::unix::fs::MetadataExt;

    let tmp = make_tempdir("atomic-local-registry");
    let local_path = seed_local(&tmp);
    let before_inode = fs::metadata(&local_path).unwrap().ino();
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    registry.local.as_mut().unwrap().diagnostic_mode = true;

    registry
        .save_at(&tmp.join("dashboard-state"), &local_path)
        .unwrap();

    let after_inode = fs::metadata(&local_path).unwrap().ino();
    assert_ne!(before_inode, after_inode, "local.yaml 必须通过同目录原子替换落盘");
    let restored: LocalConfig =
        serde_yaml::from_str(&fs::read_to_string(&local_path).unwrap()).unwrap();
    assert!(restored.diagnostic_mode);
}

// v4.2 §16.6：`add_pending_and_promote_to_registry` 删除 —— pending 中转已废弃。
// 老 pending_sync_projects 字段在 load_local_config preflight 里做 backup（覆盖测试见新增 preflight
// 测试 `preflight_backs_up_legacy_pending`）。

#[test]
fn preflight_backs_up_legacy_pending() {
    // 老 yaml 含 pending_sync_projects 段 → load 时 backup 到 backups/pending_migration_*.yaml
    // + serde silently ignore 该字段，App 继续正常启动
    let tmp = make_tempdir("preflight-pending");
    let local_dir = tmp.join(".project-dashboard");
    fs::create_dir_all(&local_dir).unwrap();
    let local_path = local_dir.join("local.yaml");
    let legacy_yaml = r#"schema_version: 1
machine_id: TEST-MACHINE
work_dirs: []
private_projects: []
pending_sync_projects:
  - schema_version: 1
    project_id: 11111111-1111-4111-8111-111111111111
    name: legacy-proj
    git_url: https://github.com/demo-user/legacy
    path: F:\legacy
    machines:
      TEST-MACHINE: F:\legacy
    created_at: 2026-06-01T00:00:00Z
    updated_at: 2026-06-01T00:00:00Z
diagnostic_mode: false
sync_tick_minutes: 3
"#;
    fs::write(&local_path, legacy_yaml).unwrap();

    let registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let local = registry.local.expect("local loaded");
    // preflight 不阻塞加载
    assert_eq!(local.machine_id, "TEST-MACHINE");
    // backup 文件存在
    let backups_dir = local_dir.join("backups");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .expect("backups dir created")
        .filter_map(Result::ok)
        .collect();
    assert!(
        entries.iter().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with("pending_migration_")),
        "应至少有一份 pending_migration_*.yaml 备份"
    );
}

#[test]
fn edit_registry_project_cannot_change_sync() {
    // 编译期保证：EditRegistryProject 结构里没有 sync 字段，天然不允许改 sync 状态。
    // 这个测试通过反射（此处：Debug 输出 + serde serialize）验证 sync 字段永远为 true。
    let tmp = make_tempdir("edit");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let sample = sample_registry_project();
    let id = sample.project_id;
    registry.projects.insert(id, sample);
    let edit = EditRegistryProject {
        name: Some("renamed-tools".to_string()),
        git_url: Some("https://github.com/example/renamed-tools".to_string()),
        machines: None,
    };
    let updated = registry.edit_shared(&id, edit).unwrap();
    assert_eq!(updated.name, "renamed-tools");
    assert!(updated.sync, "sync must stay true");
}

#[test]
fn delete_shared_project_removes_from_registry() {
    let tmp = make_tempdir("delete");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let sample = sample_registry_project();
    let id = sample.project_id;
    registry.projects.insert(id, sample);
    assert!(registry.get_shared(&id).is_some());
    let removed = registry.delete_shared(&id).unwrap();
    assert_eq!(removed.name, "project-dashboard");
    assert!(registry.get_shared(&id).is_none());
}

#[test]
fn add_private_rejects_sync_true() {
    let tmp = make_tempdir("priv-sync");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let mut bad = sample_private_project("company-proj");
    bad.sync = true;
    let result = registry.add_private(bad);
    assert!(result.is_err(), "sync:true 项目不许进 private_projects");
}

#[test]
fn add_private_deduplicates_by_name() {
    let tmp = make_tempdir("priv-dup");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    registry.add_private(sample_private_project("company-proj")).unwrap();
    let dup = registry.add_private(sample_private_project("company-proj"));
    assert!(dup.is_err());
}

#[test]
fn merged_view_combines_shared_and_private() {
    // v4.2 §16.6：`list_pending` 删除；只测 shared / private 两通道。
    let tmp = make_tempdir("merged");
    let local_path = seed_local(&tmp);
    let mut registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    let shared = sample_registry_project();
    registry.projects.insert(shared.project_id, shared);
    registry.add_private(sample_private_project("company-x")).unwrap();
    registry.add_private(sample_private_project("company-y")).unwrap();
    assert_eq!(registry.list_shared().len(), 1);
    assert_eq!(registry.list_private().len(), 2);
}

// ---------- Schema validator 拒收测试 ----------

#[test]
fn schema_accepts_valid_registry_project() {
    let proj = sample_registry_project();
    let json = serde_json::to_value(&proj).unwrap();
    validate_registry_project(&json).expect("valid project should pass");
}

#[test]
fn schema_rejects_extra_field_in_registry_project() {
    let proj = sample_registry_project();
    let mut json = serde_json::to_value(&proj).unwrap();
    json.as_object_mut().unwrap().insert(
        "arbitrary_extra_field".into(),
        serde_json::Value::String("this should be rejected".into()),
    );
    let err = validate_registry_project(&json).unwrap_err();
    assert!(
        matches!(err, SchemaError::Violation(_)),
        "unexpected error: {err}"
    );
}

#[test]
fn schema_rejects_sync_false_in_registry() {
    let proj = sample_registry_project();
    let mut json = serde_json::to_value(&proj).unwrap();
    json["sync"] = serde_json::Value::Bool(false);
    let err = validate_registry_project(&json).unwrap_err();
    assert!(matches!(err, SchemaError::Violation(_)));
}

#[test]
fn schema_rejects_invalid_uuid() {
    let proj = sample_registry_project();
    let mut json = serde_json::to_value(&proj).unwrap();
    json["project_id"] = serde_json::Value::String("not-a-uuid".into());
    let err = validate_registry_project(&json).unwrap_err();
    assert!(matches!(err, SchemaError::Violation(_)));
}

#[test]
fn schema_rejects_non_normalized_git_url() {
    let proj = sample_registry_project();
    let mut json = serde_json::to_value(&proj).unwrap();
    // SSH 形式（未归一化）
    json["git_url"] = serde_json::Value::String("git@github.com:example/project-dashboard.git".into());
    let err = validate_registry_project(&json).unwrap_err();
    assert!(matches!(err, SchemaError::Violation(_)));
}

// Q1-B：schema 与 normalize 严格双向对齐（覆盖 AC-4）
#[test]
fn schema_accepts_normalize_output() {
    // 正向：normalize 产出（多段 Bitbucket / 带端口 / 自建 host）都应过 schema pattern
    let cases = [
        "https://git.example.org/projects/team/repos/exampleapp", // 四段
        "https://host.example:7990/o/r",                                            // 带端口
        "https://random-host.example/o/r",                                          // 自建 host 两段
        "https://gitea.company.internal/team/proj",                                 // 自建 host
    ];
    for url in cases {
        // 自证：用例确实是 normalize 的稳定产出（幂等）
        assert_eq!(normalize_git_url(url).unwrap(), url, "用例应是 normalize 稳定产出");
        let proj = sample_registry_project();
        let mut json = serde_json::to_value(&proj).unwrap();
        json["git_url"] = serde_json::Value::String(url.to_string());
        json["git_url_aliases"] =
            serde_json::Value::Array(vec![serde_json::Value::String(url.to_string())]);
        validate_registry_project(&json)
            .unwrap_or_else(|e| panic!("normalize 产出 {url} 应过 schema：{e}"));
    }
}

#[test]
fn schema_rejects_boundary_and_normalize_agrees() {
    // 反向：schema pattern 接受集 ⊆ normalize 可产出集；边界脏形态 schema 拒，且 normalize 也拒
    let bad = [
        "https://host.example/only",           // 单段（<2）
        "https://user:token@host.example/o/r", // userinfo
        "https://host.example:abc/o/r",        // 端口非法
        "https://host.example/o//r",           // 空段
        "https://host.example/o/r epo",        // 段内非法字符（空格）
    ];
    for url in bad {
        let proj = sample_registry_project();
        let mut json = serde_json::to_value(&proj).unwrap();
        json["git_url"] = serde_json::Value::String(url.to_string());
        assert!(
            matches!(
                validate_registry_project(&json),
                Err(SchemaError::Violation(_))
            ),
            "边界脏 URL {url} 应被 schema pattern 拒"
        );
        assert!(
            normalize_git_url(url).is_err(),
            "边界脏 URL {url} normalize 也应拒（双向对齐闭环）"
        );
    }
}

// Q1-B：存量免迁移幂等（覆盖 AC-3）—— 旧 canonical 过新 normalize 逐字节不变，且过新 schema
#[test]
fn legacy_canonical_idempotent() {
    let legacy = [
        "https://github.com/example/project-dashboard",
        "https://gitlab.com/some-org/some-repo",
        "https://bitbucket.org/team/repo",
    ];
    for url in legacy {
        assert_eq!(
            normalize_git_url(url).unwrap(),
            url,
            "存量 canonical {url} 应幂等（免迁移前提）"
        );
        let proj = sample_registry_project();
        let mut json = serde_json::to_value(&proj).unwrap();
        json["git_url"] = serde_json::Value::String(url.to_string());
        validate_registry_project(&json)
            .unwrap_or_else(|e| panic!("legacy canonical {url} 应过新 schema：{e}"));
    }
}

#[test]
fn schema_accepts_valid_snapshot() {
    let snapshot = serde_json::json!({
        "schema_version": 1,
        "machine_id": "DESKTOP-DEMO01",
        "updated_at": "2026-07-06T12:00:00Z",
        "projects": [{
            "project_id": "550e8400-e29b-41d4-a716-446655440000",
            "name": "project-dashboard",
            "branch": "main",
            "head": "abc1234",
            "upstream": "origin/main",
            "ahead": 0,
            "behind": 0,
            "dirty": 3,
            "untracked": 0,
            "last_commit_at": "2026-07-05T22:00:00Z",
            "last_fs_change_at": "2026-07-06T11:59:00Z",
            "collected_at": "2026-07-06T12:00:00Z",
            "progress": null,
            "dangerous_state": null
        }]
    });
    validate_snapshot(&snapshot).expect("valid snapshot");
}

#[test]
fn schema_rejects_sensitive_field_diff() {
    // 即便某人企图在 snapshot 里塞 diff 字段（未来 schema 演进意外放宽 allowlist 也要拦）
    let bad = serde_json::json!({
        "schema_version": 1,
        "machine_id": "M1",
        "updated_at": "2026-07-06T12:00:00Z",
        "projects": [],
        "diff": "some raw diff content leaking here"
    });
    // 一定会 fail —— 要么 additionalProperties: false 拦，要么 sensitive keywords 拦
    let err = validate_snapshot(&bad).unwrap_err();
    // 优先 additionalProperties 命中
    match err {
        SchemaError::Violation(_) | SchemaError::SensitiveFieldHit(_) => {}
    }
}

#[test]
fn sensitive_check_catches_message_field() {
    // 直接调 check_sensitive_fields —— 不经过 jsonschema
    let bad = serde_json::json!({
        "top": {
            "message": "prompt injection attempt",
            "harmless": "value"
        }
    });
    let err = check_sensitive_fields(&bad).unwrap_err();
    match err {
        SchemaError::SensitiveFieldHit(hits) => {
            assert!(hits.iter().any(|h| h.contains("message")));
        }
        _ => panic!("expected SensitiveFieldHit"),
    }
}

#[test]
fn sensitive_check_catches_diff_hunk_patch_keywords() {
    for kw in ["diff_details", "hunk_ranges", "patch_body", "prompt_history", "chat_log"] {
        let bad = serde_json::json!({ kw: "any value" });
        let err = check_sensitive_fields(&bad).unwrap_err();
        assert!(matches!(err, SchemaError::SensitiveFieldHit(_)),
            "keyword {kw} should be rejected");
    }
}

#[test]
fn sensitive_check_catches_camelcase_field_names() {
    // 审计对账 P0-E：`apiKey` 转小写 `apikey` 应命中黑名单
    for kw in ["apiKey", "API_KEY", "api-key", "sessionId", "session-id", "filesContent"] {
        let bad = serde_json::json!({ kw: "any value" });
        let err = check_sensitive_fields(&bad).unwrap_err();
        assert!(
            matches!(err, SchemaError::SensitiveFieldHit(_)),
            "camelCase/kebab-case 变体 {kw} 应被归一化后命中"
        );
    }
}

#[test]
fn sensitive_check_catches_string_value_secrets() {
    // 审计对账 P0-E：`Value::String` 分支扫描 —— 高熵密钥前缀 & 关键词
    for secret in [
        "sk-ant-api03-abcdef123456",     // Anthropic
        "ghp_1234567890abcdefghij",       // GitHub personal
        "xoxb-1234-5678-abcdefghi",       // Slack bot
        "AKIA1234567890ABCDEF",           // AWS
    ] {
        let bad = serde_json::json!({ "note": secret });
        let err = check_sensitive_fields(&bad).unwrap_err();
        assert!(
            matches!(err, SchemaError::SensitiveFieldHit(_)),
            "字符串值 {secret} 应被密钥前缀识别"
        );
    }
}

#[test]
fn sensitive_check_catches_string_value_keywords() {
    // 字符串值里包含敏感关键词也拒
    let bad = serde_json::json!({
        "notes": "here is my chat log leaking prompt"
    });
    let err = check_sensitive_fields(&bad).unwrap_err();
    assert!(matches!(err, SchemaError::SensitiveFieldHit(_)));
}

#[test]
fn sensitive_check_does_not_false_positive_on_task_or_risk() {
    // 保证 sk- 短前缀不会误伤 task-*, risk-*, asterisk-* 之类正常字符串
    // （我们的实现已明确排除 `sk-`，只保留 `sk-ant-`）
    let ok = serde_json::json!({
        "labels": ["task-done", "risk-assessment", "asterisk-based"]
    });
    check_sensitive_fields(&ok).expect("task-/risk-/asterisk- 类值不应命中");
}

#[test]
fn sensitive_check_passes_clean_object() {
    let ok = serde_json::json!({
        "name": "project-dashboard",
        "machine_id": "DESKTOP-DEMO01",
        "machines": { "DESKTOP-DEMO01": "F:\\project-dashboard" }
    });
    check_sensitive_fields(&ok).expect("clean object should pass");
}

// P1-C（2026-07-06）：
//   - 字段值层用**收窄的** SENSITIVE_VALUE_KEYWORDS（不含 token/auth/apikey 等业务常用词）
//   - SECRET_PREFIXES 走 starts_with + 边界匹配（非 substring）
// 目标：合法业务名不误伤，同时保留高熵密钥前缀兜底

#[test]
fn sensitive_value_no_false_positive_on_business_names() {
    // P1-C：project name / 描述里含 token/auth/secret 等业务常用词**不应**被拒
    let ok_cases = [
        serde_json::json!({ "name": "auth-token-service" }),
        serde_json::json!({ "name": "jwt-rotation-job" }),
        serde_json::json!({ "name": "userkey-store" }),
        serde_json::json!({ "name": "password-reset-flow" }),
        serde_json::json!({ "name": "bearer-auth-middleware" }),
        serde_json::json!({ "name": "credential-vault-cli" }),
        serde_json::json!({ "description": "handles OAuth token refresh" }),
    ];
    for v in ok_cases {
        check_sensitive_fields(&v)
            .unwrap_or_else(|e| panic!("合法业务名 {v} 不应被拒: {e:?}"));
    }
}

#[test]
fn sensitive_value_business_keyword_still_caught_in_field_name() {
    // 反向验证：**字段名**层 token/apikey/password 仍然拦截（未收窄）
    for kw in ["token", "apiKey", "PASSWORD", "credential"] {
        let bad = serde_json::json!({ kw: "not-a-secret-value" });
        let err = check_sensitive_fields(&bad).unwrap_err();
        assert!(
            matches!(err, SchemaError::SensitiveFieldHit(_)),
            "字段名 {kw} 仍应被拦截（收窄只针对字符串值层）"
        );
    }
}

#[test]
fn sensitive_value_secret_prefix_boundary_match() {
    // P1-C：SECRET_PREFIXES 走边界匹配 —— 段首 or token 首出现命中，中段不命中
    // ✓ 命中：整串起始 / 空格切分 / 冒号切分 / 点号切分后 token 首
    let bad_cases = [
        serde_json::json!({ "note": "sk-ant-api03-abcdef" }),
        serde_json::json!({ "note": "leaked: ghp_abcdef1234567890" }),
        serde_json::json!({ "note": "leak=xoxb-1234567890-abcdefghij" }),
        serde_json::json!({ "note": "aws creds AKIAIOSFODNN7EXAMPLE" }),
    ];
    for v in bad_cases {
        let err = check_sensitive_fields(&v)
            .err()
            .unwrap_or_else(|| panic!("段首/token 首密钥前缀 {v} 应被命中"));
        assert!(matches!(err, SchemaError::SensitiveFieldHit(_)));
    }

    // ✗ 不命中：字符串**中段**含前缀 substring（合法域名 / user id / 项目名 等）
    let ok_cases = [
        serde_json::json!({ "domain": "api.xoxa-endpoint.com" }),
        serde_json::json!({ "name": "user-xoxa-service" }),
        serde_json::json!({ "note": "prefix-ghp_notarealtoken-suffix" }),
    ];
    for v in ok_cases {
        check_sensitive_fields(&v)
            .unwrap_or_else(|e| panic!("中段含前缀 {v} 不应被误命中: {e:?}"));
    }
}

#[test]
fn sensitive_value_still_catches_content_leaks() {
    // 二次收窄 P1-F（2026-07-07）后仍必须拦截：filescontent / filecontent / prompt / sessionid / diff / hunk
    let bad_cases = [
        serde_json::json!({ "extra": "the prompt was: hello world" }),
        serde_json::json!({ "detail": "diff --git a/foo b/foo" }),
        serde_json::json!({ "detail": "@@ -1,3 +1,5 @@ hunk header" }),
        serde_json::json!({ "extra": "filesContent leaked" }),
    ];
    for v in bad_cases {
        let err = check_sensitive_fields(&v).unwrap_err();
        assert!(
            matches!(err, SchemaError::SensitiveFieldHit(_)),
            "隐私泄漏关键词 {v} 应被拦截"
        );
    }
}

#[test]
fn sensitive_value_no_false_positive_on_substring_business_names_p1f() {
    // 二次收窄 P1-F（2026-07-07）：`patch/chat/response` 通过 substring `contains`
    // 会命中业务名，从 SENSITIVE_VALUE_KEYWORDS 移除后 value 层不再拦。
    // 反向断言：以下业务名**不应**被拒。
    let ok_cases = [
        serde_json::json!({ "name": "dispatch-service" }),  // 含 patch
        serde_json::json!({ "name": "chatbot-ui" }),         // 含 chat
        serde_json::json!({ "name": "incident-response" }),  // 含 response
        serde_json::json!({ "name": "hot-patch-service" }),  // 含 patch
        serde_json::json!({ "description": "gRPC response streaming" }), // 含 response
        serde_json::json!({ "name": "chatgpt-clone" }),      // 含 chat
    ];
    for v in ok_cases {
        check_sensitive_fields(&v)
            .unwrap_or_else(|e| panic!("业务名 substring {v} 不应被拒: {e:?}"));
    }
}

#[test]
fn sensitive_value_business_substring_still_caught_in_field_name_p1f() {
    // 反向断言：patch / chat / response **字段名**层仍拦截（未收窄，SENSITIVE_KEYWORDS 保留）
    for kw in ["patch", "chat", "response"] {
        let bad = serde_json::json!({ kw: "harmless value" });
        let err = check_sensitive_fields(&bad).unwrap_err();
        assert!(
            matches!(err, SchemaError::SensitiveFieldHit(_)),
            "字段名 {kw} 仍应被拦截（P1-F 只收窄值层）"
        );
    }
}

#[test]
fn schema_accepts_valid_event() {
    let evt = serde_json::json!({
        "ts": "2026-07-06T12:00:00Z",
        "trigger": "hook",
        "project_id": "550e8400-e29b-41d4-a716-446655440000",
        "action": "commit",
        "from_head": "abc1234",
        "to_head": "def5678",
        "dirty": 0,
        "untracked": 0,
        "dangerous_state": null
    });
    validate_event(&evt).expect("valid event");
}

#[test]
fn schema_rejects_unknown_trigger() {
    let evt = serde_json::json!({
        "ts": "2026-07-06T12:00:00Z",
        "trigger": "malicious-trigger",
        "project_id": null,
        "action": "commit",
        "from_head": null,
        "to_head": null,
        "dirty": 0,
        "untracked": 0,
        "dangerous_state": null
    });
    let err = validate_event(&evt).unwrap_err();
    assert!(matches!(err, SchemaError::Violation(_)));
}

// ---------- glob 加载 ----------

#[test]
fn load_ignores_non_yaml_files_in_registry_dir() {
    let tmp = make_tempdir("glob");
    let local_path = seed_local(&tmp);
    let projects_dir = tmp.join("dashboard-state").join("registry").join("projects");
    fs::create_dir_all(&projects_dir).unwrap();
    // 写一份合法 YAML
    let proj = sample_registry_project();
    let yaml = serde_yaml::to_string(&proj).unwrap();
    fs::write(projects_dir.join(format!("{}.yaml", proj.project_id)), yaml).unwrap();
    // 写一份误入的 README.md（应被忽略）
    fs::write(projects_dir.join("README.md"), "# do not parse me").unwrap();
    // 写一份 .DS_Store 类噪音（应被忽略）
    fs::write(projects_dir.join(".DS_Store"), &[0u8, 1, 2, 3]).unwrap();
    let registry = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap();
    assert_eq!(registry.projects.len(), 1);
}

#[test]
fn load_rejects_malformed_registry_yaml() {
    let tmp = make_tempdir("bad-yaml");
    let local_path = seed_local(&tmp);
    let projects_dir = tmp.join("dashboard-state").join("registry").join("projects");
    fs::create_dir_all(&projects_dir).unwrap();
    fs::write(
        projects_dir.join("11111111-1111-4111-8111-111111111111.yaml"),
        "this is: not: valid: yaml: no",
    )
    .unwrap();
    let err = Registry::load_at(&tmp.join("dashboard-state"), &local_path).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("yaml"));
}

/// 首次使用场景：`~/.project-dashboard/local.yaml` 不存在。
/// add_project 必须自动 init 一个空 LocalConfig 而不是报 `NoLocalConfig`。
/// 回归修复：从零装完 App 双击开 → 新增第一个私有项目必挂。
#[test]
fn add_project_auto_inits_local_when_missing_private() {
    let tmp = make_tempdir("first-run-private");
    let dashboard_state = tmp.join("dashboard-state");
    let local_path = tmp.join(".project-dashboard").join("local.yaml");

    // 前置：local.yaml 不存在
    assert!(!local_path.exists());

    let mut machines = BTreeMap::new();
    machines.insert("test-machine".to_string(), "F:\\zero-first".to_string());
    let req = dashboard_core::commands::AddProjectRequest {
        name: "zero-first-project".to_string(),
        git_url: "https://github.com/test/zero-first".to_string(),
        path: "F:\\zero-first".to_string(),
        machines,
        sync: false,
        confirm_dual_form: false,
    };
    let result =
        dashboard_core::commands::add_project(&dashboard_state, &local_path, req).unwrap();
    assert!(result.project_id.is_none()); // sync:false 不生成 UUID

    // 后置：local.yaml 被创建且项目落盘
    assert!(local_path.is_file());
    let reloaded = Registry::load_at(&dashboard_state, &local_path).unwrap();
    let private = reloaded.list_private();
    assert_eq!(private.len(), 1);
    assert_eq!(private[0].name, "zero-first-project");
    // machine_id 具体值取决于 DASHBOARD_MACHINE_ID / COMPUTERNAME env，只 assert 非空
    assert!(!reloaded.local.as_ref().unwrap().machine_id.is_empty());
}

/// Q2-B 门禁：first-run（未配存储仓地址 + 无本地仓 = Unconfigured）新增 sync:true 共享项目 →
/// 被拦，提示"尚未配置存储仓地址"，不向未初始化 root 写 registry。
///
/// （原 `add_project_auto_inits_local_when_missing_shared` 的"未配地址也能加共享项目成功"语义
/// 已被 Q2 门禁作废；add sync:true 成功路径改由 url_binding 的就绪存储仓版覆盖。）
#[test]
fn add_shared_blocked_when_state_repo_unconfigured() {
    let tmp = make_tempdir("first-run-shared-blocked");
    let dashboard_state = tmp.join("dashboard-state"); // 无 .git
    let local_path = tmp.join(".project-dashboard").join("local.yaml");
    assert!(!local_path.exists());

    let mut machines = BTreeMap::new();
    machines.insert("test-machine".to_string(), "F:\\shared-first".to_string());
    let req = dashboard_core::commands::AddProjectRequest {
        name: "shared-first-project".to_string(),
        git_url: "https://github.com/test/shared-first".to_string(),
        path: tmp.join("nonexistent-project-path").to_string_lossy().into_owned(),
        machines,
        sync: true,
        confirm_dual_form: false,
    };
    let err = dashboard_core::commands::add_project(&dashboard_state, &local_path, req).unwrap_err();
    assert!(
        matches!(err, dashboard_core::commands::CommandError::InvalidSettings(_)),
        "未配存储仓地址时新增共享项目应被拦，got {err:?}"
    );
    assert!(
        !dashboard_state.join("registry").exists(),
        "拦截后不应向未初始化 root 写 registry"
    );
}

#[test]
fn delete_shared_then_save_removes_yaml_no_resurrect() {
    // delete_shared 只从内存移除；save_at 必须同步删磁盘 <uuid>.yaml，
    // 否则 load_at glob 目录会把已删项目读回来"复活"（本次修复的 bug）。
    let tmp = make_tempdir("delete-no-resurrect");
    let dashboard_state = tmp.join("dashboard-state");
    let local_path = tmp.join(".project-dashboard").join("local.yaml");

    // 建 registry 含一个 shared 项目并存盘
    let mut reg = Registry::load_at(&dashboard_state, &local_path).unwrap();
    reg.local = Some(LocalConfig::new("test-machine"));
    let proj = sample_registry_project();
    let uuid = proj.project_id;
    reg.add_shared(proj).unwrap();
    reg.save_at(&dashboard_state, &local_path).unwrap();

    let yaml_path = dashboard_state
        .join("registry")
        .join("projects")
        .join(format!("{}.yaml", uuid));
    assert!(yaml_path.is_file(), "存盘后磁盘应有 <uuid>.yaml");
    assert_eq!(
        Registry::load_at(&dashboard_state, &local_path)
            .unwrap()
            .projects
            .len(),
        1,
        "reload 应能读到项目"
    );

    // delete + save：磁盘 yaml 应被同步删除
    let mut reg2 = Registry::load_at(&dashboard_state, &local_path).unwrap();
    reg2.delete_shared(&uuid).unwrap();
    reg2.save_at(&dashboard_state, &local_path).unwrap();

    assert!(
        !yaml_path.exists(),
        "delete + save 后磁盘 <uuid>.yaml 应被删除（否则 reload 会复活）"
    );
    assert!(
        Registry::load_at(&dashboard_state, &local_path)
            .unwrap()
            .projects
            .is_empty(),
        "delete + save 后 reload 不应复活已删项目"
    );
}

/// Q2-A：LocalConfig 新增 dashboard_state_remote / dashboard_state_root 两字段的 serde 往返 +
/// 老 local.yaml（无新字段）向后兼容（缺席 → None，非 BREAKING）。
#[test]
fn localconfig_serde_new_fields() {
    // 设值往返：序列化后再反序列化应逐字节保留。
    let mut cfg = LocalConfig::new("TEST-MACHINE");
    cfg.dashboard_state_remote = Some("git@host.internal:team/dashboard-state.git".to_string());
    cfg.dashboard_state_root = Some("/data/dashboard-state".to_string());
    let yaml = serde_yaml::to_string(&cfg).unwrap();
    let back: LocalConfig = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        back.dashboard_state_remote.as_deref(),
        Some("git@host.internal:team/dashboard-state.git")
    );
    assert_eq!(back.dashboard_state_root.as_deref(), Some("/data/dashboard-state"));

    // 老 local.yaml（无新字段）→ 缺席字段反序列化为 None，不报错。
    let legacy = "schema_version: 1\nmachine_id: OLD\nwork_dirs: []\nprivate_projects: []\ndiagnostic_mode: false\nsync_tick_minutes: 3\n";
    let old: LocalConfig = serde_yaml::from_str(legacy).unwrap();
    assert_eq!(old.dashboard_state_remote, None, "缺席字段应为 None（老 yaml 兼容）");
    assert_eq!(old.dashboard_state_root, None);
}

// ===== Q2-C 数据目录可配（覆盖 AC-8）=====
// resolve_* 是纯函数（不碰文件系统），load_*_from 从注入的 local.yaml 读，
// 生产壳 load_dashboard_state_root 走固定 ~/.project-dashboard/local.yaml 每次重读。

#[test]
fn resolve_state_root_uses_configured_value() {
    // 配了非空 dashboard_state_root → 原样用（跨盘 / 自定义路径，防 C 盘膨胀场景）
    let got = dashboard_core::config::resolve_dashboard_state_root(Some(
        "/data/pd/dashboard-state-clone",
    ));
    assert_eq!(got, PathBuf::from("/data/pd/dashboard-state-clone"));
}

#[test]
fn resolve_state_root_falls_back_when_none() {
    // 未配（老 local.yaml / 首次启动）→ fallback ~/.project-dashboard/dashboard-state-clone
    let got = dashboard_core::config::resolve_dashboard_state_root(None);
    assert_eq!(
        got,
        dashboard_core::config::default_dashboard_state_root_fallback()
    );
    assert!(
        got.ends_with(Path::new(".project-dashboard").join("dashboard-state-clone")),
        "fallback 结构应为 .project-dashboard/dashboard-state-clone，实际 {got:?}"
    );
}

#[test]
fn resolve_state_root_falls_back_when_empty() {
    // 空串 / 纯空白当作未配 → fallback（防设置页清空后把根落到 "" 空路径）
    for empty in ["", "   ", "\t"] {
        let got = dashboard_core::config::resolve_dashboard_state_root(Some(empty));
        assert_eq!(
            got,
            dashboard_core::config::default_dashboard_state_root_fallback(),
            "空串 {empty:?} 应 fallback 而非落到空路径"
        );
    }
}

#[test]
fn load_state_root_reads_local_yaml() {
    // 固定 local.yaml 里配了 dashboard_state_root → 读到它（运行时每次重读，无缓存）
    let tmp = make_tempdir("q2c-load");
    let local_path = tmp.join("local.yaml");
    let yaml = "schema_version: 1\nmachine_id: MAC\ndashboard_state_root: /custom/data/root\n";
    fs::write(&local_path, yaml).unwrap();
    let got = dashboard_core::config::load_dashboard_state_root_from(&local_path);
    assert_eq!(got, PathBuf::from("/custom/data/root"));
}

#[test]
fn load_state_root_missing_file_falls_back() {
    // local.yaml 不存在（尚未引导）→ fallback，不 panic
    let tmp = make_tempdir("q2c-missing");
    let local_path = tmp.join("nonexistent-local.yaml");
    let got = dashboard_core::config::load_dashboard_state_root_from(&local_path);
    assert_eq!(
        got,
        dashboard_core::config::default_dashboard_state_root_fallback()
    );
}

#[test]
fn load_state_root_no_field_falls_back() {
    // local.yaml 存在但没配 dashboard_state_root（老配置）→ fallback
    let tmp = make_tempdir("q2c-nofield");
    let local_path = tmp.join("local.yaml");
    fs::write(&local_path, "schema_version: 1\nmachine_id: MAC\n").unwrap();
    let got = dashboard_core::config::load_dashboard_state_root_from(&local_path);
    assert_eq!(
        got,
        dashboard_core::config::default_dashboard_state_root_fallback()
    );
}

#[test]
fn resolve_state_root_trims_surrounding_whitespace() {
    // P2-8：配置值含首尾空白 → trim 后再建 PathBuf（与判空口径一致），防跨平台 fs 路径不一致
    let got = dashboard_core::config::resolve_dashboard_state_root(Some("  /data/pd/root  "));
    assert_eq!(got, PathBuf::from("/data/pd/root"));
}

/// P2-9：解析失败（配置损坏）→ fail-open fallback（保 App 能起）、不 panic。
/// 已知测试边界（三方复审 2026-07-17）：本测试仅锁"解析失败不崩、回退默认路径"；返回值与
/// load_state_root_missing_file_falls_back 相同，WARN（eprintln stderr 副作用）**未断言**——
/// 锁死 WARN 需注入 logger 改调用链，对 P2 日志项过度；经评估记为技术债，不上重测试。
#[test]
fn load_state_root_malformed_yaml_falls_back() {
    let tmp = make_tempdir("q2c-malformed");
    let local_path = tmp.join("local.yaml");
    fs::write(&local_path, "dashboard_state_root: [unclosed\n").unwrap();
    let got = dashboard_core::config::load_dashboard_state_root_from(&local_path);
    assert_eq!(
        got,
        dashboard_core::config::default_dashboard_state_root_fallback()
    );
}

#[test]
fn local_yaml_path_stays_fixed() {
    // 数据目录可配后，local.yaml 自身路径仍恒定 ~/.project-dashboard/local.yaml
    // （引导入口自举 + 含不出机 private_projects，不随数据目录挪）
    let got = dashboard_core::config::default_local_config_path();
    assert!(
        got.ends_with(Path::new(".project-dashboard").join("local.yaml")),
        "local.yaml 路径应固定，实际 {got:?}"
    );
}
