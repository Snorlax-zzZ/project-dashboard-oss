//! 偏好持久化与产品分页必须在真实存储和扫描边界验证。
use dashboard_core::{
    session_storage::SessionStorage,
    session_visibility::{list_visible_paged, Visibility},
    sessions::{SessionMeta, SessionSource},
};
use std::{cell::Cell, fs, path::PathBuf, str::FromStr};

fn store() -> SessionStorage {
    let root = std::env::temp_dir().join(format!("dashboard-visibility-{}", uuid::Uuid::new_v4()));
    SessionStorage::new(root.join("sessions"), root.join("legacy"))
}
struct Source {
    id: &'static str,
    calls: Cell<usize>,
    items: Vec<SessionMeta>,
}
impl SessionSource for Source {
    fn source_id(&self) -> &'static str {
        self.id
    }
    fn scan(&self) -> Vec<SessionMeta> {
        self.calls.set(self.calls.get() + 1);
        self.items.clone()
    }
}
fn source(id: &'static str, count: usize) -> Source {
    Source {
        id,
        calls: Cell::new(0),
        items: (0..count)
            .map(|i| SessionMeta {
                session_id: format!("id-{i}"),
                source: "untrusted-cache-source".into(),
                title: format!("topic {}", i % 2),
                mtime: i as f64,
                ..Default::default()
            })
            .collect(),
    }
}
#[test]
fn missing_hidden_is_empty_without_creating_file_after_initialization() {
    let s = store();
    s.ensure_initialized().unwrap();
    assert!(!s.load_hidden().unwrap().contains("pi", "same"));
    assert!(!s.root().join("hidden.json").exists());
}
#[test]
fn hidden_survives_restart_and_cache_deletion_and_is_source_specific() {
    let s = store();
    s.set_hidden("claude-cli", "same", true).unwrap();
    s.set_hidden("claude-cli", "same", true).unwrap();
    s.toggle_favorite("same").unwrap();
    fs::remove_dir_all(s.root().join("cache")).unwrap();
    let restarted = SessionStorage::new(s.root().to_owned(), PathBuf::from("/nonexistent-fixture"));
    let hidden = restarted.load_hidden().unwrap();
    assert!(hidden.contains("claude-cli", "same"));
    assert!(!hidden.contains("codex", "same"));
    restarted.set_hidden("claude-cli", "same", false).unwrap();
    restarted.set_hidden("claude-cli", "same", false).unwrap();
    assert!(!restarted
        .load_hidden()
        .unwrap()
        .contains("claude-cli", "same"));
    assert_eq!(restarted.load_favorites().unwrap().get("same"), Some(&true));
}
#[test]
fn invalid_requests_do_not_initialize_or_write_anything() {
    let s = store();
    for (source, id) in [
        ("unexpected", "ok"),
        ("pi", ""),
        ("pi", "../bad"),
        ("pi", "bad id"),
    ] {
        assert!(s.set_hidden(source, id, true).is_err());
    }
    assert!(Visibility::from_str("unexpected").is_err());
    assert!(!s.root().exists());
}
#[test]
fn invalid_hidden_never_becomes_an_empty_successful_write() {
    for data in [
        "broken",
        r#"{"version":2,"hidden":{}}"#,
        r#"{"version":1,"hidden":{"pi":[8]}}"#,
    ] {
        let s = store();
        s.ensure_initialized().unwrap();
        fs::write(s.root().join("hidden.json"), data).unwrap();
        assert!(s.set_hidden("pi", "ok", true).is_err());
        assert!(s.load_hidden().is_err());
        assert_eq!(
            fs::read_to_string(s.root().join("hidden.json")).unwrap(),
            data
        );
    }
}
#[test]
fn concurrent_writes_keep_every_id_and_deduplicate_arrays() {
    let s = store();
    s.ensure_initialized().unwrap();
    fs::write(
        s.root().join("hidden.json"),
        r#"{"version":1,"hidden":{"pi":["same","same"]}}"#,
    )
    .unwrap();
    let threads: Vec<_> = (0..20)
        .map(|i| {
            let s = s.clone();
            std::thread::spawn(move || s.set_hidden("pi", &format!("id-{i}"), true).unwrap())
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let data: serde_json::Value =
        serde_json::from_slice(&fs::read(s.root().join("hidden.json")).unwrap()).unwrap();
    assert_eq!(data["hidden"]["pi"].as_array().unwrap().len(), 21);
}
#[test]
fn filtering_precedes_count_and_pagination_with_one_source_scan() {
    for id in ["claude-cli", "codex", "pi", "grok"] {
        let s = store();
        for i in 0..55 {
            s.set_hidden(id, &format!("id-{i}"), true).unwrap();
        }
        let input = source(id, 105);
        for size in [10, 20, 50] {
            let before = input.calls.get();
            let result =
                list_visible_paged(&input, &s, None, 999, size, Visibility::Visible).unwrap();
            assert_eq!(input.calls.get(), before + 1);
            assert_eq!(result.total, 50);
            assert_eq!(result.page, 50_usize.div_ceil(size));
            assert!(result
                .items
                .iter()
                .all(|m| m.session_id[3..].parse::<usize>().unwrap() >= 55));
        }
        let hidden = list_visible_paged(&input, &s, None, 1, 50, Visibility::Hidden).unwrap();
        assert_eq!(hidden.total, 55);
        assert_eq!(hidden.items.len(), 50);
    }
}
#[test]
fn search_favorites_and_empty_last_page_use_filtered_results() {
    let s = store();
    let mut input = source("codex", 3);
    s.toggle_favorite("id-0").unwrap();
    let page = list_visible_paged(&input, &s, Some("topic 0"), 1, 10, Visibility::Visible).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].session_id, "id-0");
    assert!(page.items.iter().all(|m| m.source == "codex"));
    for i in 0..3 {
        s.set_hidden("codex", &format!("id-{i}"), true).unwrap();
    }
    let empty = list_visible_paged(&input, &s, None, 99, 50, Visibility::Visible).unwrap();
    assert_eq!((empty.total, empty.page), (0, 1));
    // 外部删除：名单只能过滤当前扫描结果，不能凭 ID 生成幽灵会话。
    input.items.clear();
    assert_eq!(
        list_visible_paged(&input, &s, None, 1, 20, Visibility::Hidden)
            .unwrap()
            .total,
        0
    );
    assert!(s.load_hidden().unwrap().contains("codex", "id-0"));
}
#[test]
fn corrupt_preferences_fail_before_scanning() {
    let s = store();
    s.ensure_initialized().unwrap();
    fs::write(s.favorites_file(), "broken").unwrap();
    let input = source("pi", 5);
    assert!(list_visible_paged(&input, &s, None, 1, 20, Visibility::Visible).is_err());
    assert_eq!(input.calls.get(), 0);
}

#[test]
fn actual_source_bytes_and_metadata_stay_unchanged_and_removed_files_do_not_reappear() {
    use dashboard_core::sessions::ClaudeCliSource;
    let s = store();
    s.ensure_initialized().unwrap();
    let root = s.root().parent().unwrap().join("harness");
    fs::create_dir_all(root.join("project")).unwrap();
    let path = root.join("project/real-id.jsonl");
    let bytes = b"{\"type\":\"ai-title\",\"aiTitle\":\"synthetic source\"}\n";
    fs::write(&path, bytes).unwrap();
    let before = fs::metadata(&path).unwrap();
    let input = ClaudeCliSource {
        root,
        cache: s.cache_dir("claude-cli"),
    };
    assert_eq!(
        list_visible_paged(&input, &s, None, 1, 20, Visibility::Visible)
            .unwrap()
            .total,
        1
    );
    s.set_hidden("claude-cli", "real-id", true).unwrap();
    assert_eq!(
        list_visible_paged(&input, &s, None, 1, 20, Visibility::Visible)
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        list_visible_paged(&input, &s, None, 1, 20, Visibility::Hidden)
            .unwrap()
            .total,
        1
    );
    let after = fs::metadata(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(before.permissions(), after.permissions());
    fs::remove_file(&path).unwrap();
    assert_eq!(
        list_visible_paged(&input, &s, None, 1, 20, Visibility::Hidden)
            .unwrap()
            .total,
        0
    );
    assert!(s.load_hidden().unwrap().contains("claude-cli", "real-id"));
}
