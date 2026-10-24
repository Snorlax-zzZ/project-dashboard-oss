#![cfg(windows)]
use dashboard_core::session_storage::SessionStorage;
use dashboard_core::session_visibility::{list_visible_paged, Visibility};
use dashboard_core::sessions::{
    scan_at, scan_codex_at, scan_grok_at, scan_pi_at, ClaudeCliSource, CodexSessionSource,
    GrokSessionSource, PiSessionSource,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};
fn write(path: &Path, lines: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n";
    fs::write(path, text).unwrap();
}
fn bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            result.push((
                entry.path().strip_prefix(root).unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            ));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
#[test]
fn four_windows_layouts_have_real_fixture_sessions_and_preserve_input_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("four-sources-{}", uuid::Uuid::new_v4()));
    let sources = root.join("sources");
    let cache = root.join("cache");
    let cwd = r"F:\中文 项目";
    let id = "00000000-0000-4000-8000-000000000201";
    write(
        &sources
            .join("claude/F--中文-项目")
            .join(format!("{id}.jsonl")),
        &[
            json!({"cwd":cwd,"gitBranch":"main"}),
            json!({"type":"user","message":{"content":"Windows 会话"}}),
        ],
    );
    write(
        &sources.join("claude/F--Users-admin--claude-mem-observer-sessions/noise.jsonl"),
        &[json!({"type":"user","message":{"content":"observer noise"}})],
    );
    write(
        &sources
            .join("codex/2026/10/20")
            .join(format!("rollout-2026-10-20T01-00-00-{id}.jsonl")),
        &[
            json!({"type":"session_meta","timestamp":"2026-10-20T01:00:00Z","payload":{"id":id,"cwd":cwd}}),
            json!({"type":"event_msg","timestamp":"2026-10-20T01:00:01Z","payload":{"type":"user_message","message":"Windows 会话"}}),
        ],
    );
    write(
        &sources
            .join("pi/--F----中文-项目--")
            .join(format!("2026-10-20T01-00-00-000Z_{id}.jsonl")),
        &[
            json!({"type":"session","version":3,"id":id,"cwd":cwd,"timestamp":"2026-10-20T01:00:00Z"}),
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"Windows 会话"}]}}),
        ],
    );
    let grok = sources
        .join("grok/F%3A%5C%E4%B8%AD%E6%96%87%20%E9%A1%B9%E7%9B%AE")
        .join(id);
    write(
        &grok.join("summary.json"),
        &[
            json!({"info":{"id":id,"cwd":cwd},"session_summary":"Windows 会话","generated_at":"2026-10-20T01:00:01Z"}),
        ],
    );
    write(
        &grok.join("chat_history.jsonl"),
        &[
            json!({"type":"user","content":[{"type":"text","text":"<user_query>Windows 会话</user_query>"}]}),
        ],
    );
    let before = bytes(&sources);
    for (name, scan) in [
        (
            "claude",
            scan_at as fn(&Path, &Path) -> Vec<dashboard_core::sessions::SessionMeta>,
        ),
        ("codex", scan_codex_at),
        ("pi", scan_pi_at),
        ("grok", scan_grok_at),
    ] {
        let found = scan(&sources.join(name), &cache.join(name));
        assert_eq!(found.len(), 1, "{name}");
        assert_eq!(found[0].cwd, cwd, "{name}");
        assert_eq!(found[0].session_id, id, "{name}");
        assert!(!found[0].title.contains("observer"));
    }
    assert_eq!(bytes(&sources), before, "扫描不得改写会话源");
    fs::remove_dir_all(root).unwrap();
}

// 把文件 mtime 固定到给定 epoch 秒，保证分页 / 排序断言确定。
fn set_mtime(path: &Path, secs: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

// 4.2：四源真实 Windows 布局上的分页、收藏置顶、隐藏视图与来源隔离，外加四源详情抽取。
// 偏好持久化 / 过滤先于计数等平台无关语义已由 session_visibility.rs 覆盖，这里不镜像，
// 只验证真实源对象 + 真实布局 + SessionStorage 组合下的 Windows 端到端行为。
#[test]
fn four_sources_paging_favorites_hiding_isolation_and_details_on_windows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("win4src-{}", uuid::Uuid::new_v4()));
    let sources = root.join("sources");
    let store = SessionStorage::new(root.join("storage/sessions"), root.join("storage/legacy"));
    store.ensure_initialized().unwrap();
    let cwd = r"F:\中文 项目";

    // claude 三条：mtime 3000/2000/1000；最旧那条带 file-history-snapshot 供详情 files 断言。
    let claude_ids = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
    ];
    for (index, id) in claude_ids.iter().enumerate() {
        let mut lines = vec![
            json!({"cwd":cwd,"gitBranch":"main"}),
            json!({"type":"user","message":{"content":"Windows 会话"}}),
        ];
        if index == 2 {
            lines.push(json!({"type":"file-history-snapshot","snapshot":{"files":{r"F:\中文 项目\a.rs":{}}}}));
        }
        let path = sources
            .join("claude/F--中文-项目")
            .join(format!("{id}.jsonl"));
        write(&path, &lines);
        set_mtime(&path, 3000 - 1000 * index as u64);
    }

    // codex 三条：文件 mtime 决定排序（3100/2100/1100）；最旧那条带 patch_apply_end 变更路径。
    let codex_ids = [
        "aaaaaaaa-1111-4aaa-8aaa-aaaaaaaaaaaa",
        "aaaaaaaa-2222-4aaa-8aaa-aaaaaaaaaaaa",
        "aaaaaaaa-3333-4aaa-8aaa-aaaaaaaaaaaa",
    ];
    for (index, id) in codex_ids.iter().enumerate() {
        let mut lines = vec![
            json!({"type":"session_meta","timestamp":"2026-10-20T01:00:00Z","payload":{"id":id,"cwd":cwd}}),
            json!({"type":"event_msg","timestamp":"2026-10-20T01:00:01Z","payload":{"type":"user_message","message":"Windows 会话"}}),
        ];
        if index == 2 {
            lines.push(json!({"type":"event_msg","payload":{"type":"patch_apply_end","changes":{"F:/codex/mod.rs":{}}}}));
        }
        let path = sources.join("codex/2026/10/20").join(format!(
            "rollout-2026-10-20T0{}-00-00-{id}.jsonl",
            index + 1
        ));
        write(&path, &lines);
        set_mtime(&path, 3100 - 1000 * index as u64);
    }

    // pi 两条：mtime 3200/2200；最旧那条带 write toolCall 供详情 files 断言。
    let pi_ids = [
        "bbbbbbbb-1111-4bbb-8bbb-bbbbbbbbbbbb",
        "bbbbbbbb-2222-4bbb-8bbb-bbbbbbbbbbbb",
    ];
    for (index, id) in pi_ids.iter().enumerate() {
        let mut lines = vec![
            json!({"type":"session","version":3,"id":id,"cwd":cwd,"timestamp":"2026-10-20T01:00:00Z"}),
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"Windows 会话"}]}}),
        ];
        if index == 1 {
            lines.push(
                json!({"type":"message","message":{"role":"assistant","content":[
                    {"type":"toolCall","name":"write","arguments":{"path":r"F:\pi 目标\新.rs"}}
                ]}}),
            );
        }
        let path = sources
            .join("pi/--F----中文-项目--")
            .join(format!("2026-10-20T01-00-0{}-000Z_{id}.jsonl", index));
        write(&path, &lines);
        set_mtime(&path, 3200 - 1000 * index as u64);
    }

    // grok 两条：mtime 由 summary.json 的 updated_at 决定（3300/2300）。
    let grok_ids = [
        "cccccccc-1111-4ccc-8ccc-cccccccccccc",
        "cccccccc-2222-4ccc-8ccc-cccccccccccc",
    ];
    for (index, id) in grok_ids.iter().enumerate() {
        let session = sources
            .join("grok/F%3A%5C%E4%B8%AD%E6%96%87%20%E9%A1%B9%E7%9B%AE")
            .join(id);
        write(
            &session.join("summary.json"),
            &[
                // updated_at 直接决定 grok 的排序 mtime：index 0 → epoch 3300，index 1 → 2300。
                json!({"info":{"id":id,"cwd":cwd},"session_summary":"Windows 会话",
                       "updated_at": if index == 0 { "1970-01-01T00:55:00Z" } else { "1970-01-01T00:38:20Z" }}),
            ],
        );
        write(
            &session.join("chat_history.jsonl"),
            &[
                json!({"type":"user","content":[{"type":"text","text":"<user_query>Windows 会话</user_query>"}]}),
            ],
        );
    }

    // 分页：真实 codex 源对象 + SessionStorage，按 mtime 倒排切片、越界页码钳回最后一页。
    let codex = CodexSessionSource {
        root: sources.join("codex"),
        cache: store.cache_dir("codex"),
    };
    let page1 = list_visible_paged(&codex, &store, None, 1, 2, Visibility::Visible).unwrap();
    assert_eq!((page1.total, page1.page, page1.page_size), (3, 1, 2));
    assert_eq!(
        page1
            .items
            .iter()
            .map(|m| m.session_id.as_str())
            .collect::<Vec<_>>(),
        [codex_ids[0], codex_ids[1]]
    );
    let page2 = list_visible_paged(&codex, &store, None, 2, 2, Visibility::Visible).unwrap();
    assert_eq!(
        page2
            .items
            .iter()
            .map(|m| m.session_id.as_str())
            .collect::<Vec<_>>(),
        [codex_ids[2]]
    );
    let clamped = list_visible_paged(&codex, &store, None, 99, 2, Visibility::Visible).unwrap();
    assert_eq!((clamped.page, clamped.items.len()), (2, 1));

    // 收藏：对最旧会话收藏后，它越过更新的会话置顶第一页。
    store.toggle_favorite(codex_ids[2]).unwrap();
    let page1 = list_visible_paged(&codex, &store, None, 1, 2, Visibility::Visible).unwrap();
    assert_eq!(page1.items[0].session_id, codex_ids[2]);
    assert!(page1.items[0].favorited);
    assert_eq!(page1.items[1].session_id, codex_ids[0]);

    // 隐藏 + 来源隔离：codex 的隐藏只影响 codex 可见列表与对应 Hidden 视图，其余三源不受波及。
    store.set_hidden("codex", codex_ids[2], true).unwrap();
    let visible = list_visible_paged(&codex, &store, None, 1, 10, Visibility::Visible).unwrap();
    assert_eq!(visible.total, 2);
    assert!(!visible.items.iter().any(|m| m.session_id == codex_ids[2]));
    let hidden = list_visible_paged(&codex, &store, None, 1, 10, Visibility::Hidden).unwrap();
    assert_eq!(hidden.total, 1);
    assert_eq!(hidden.items[0].session_id, codex_ids[2]);
    let claude = ClaudeCliSource {
        root: sources.join("claude"),
        cache: store.cache_dir("claude-cli"),
    };
    assert_eq!(
        list_visible_paged(&claude, &store, None, 1, 10, Visibility::Visible)
            .unwrap()
            .total,
        3
    );
    let pi = PiSessionSource {
        root: sources.join("pi"),
        cache: store.cache_dir("pi"),
    };
    assert_eq!(
        list_visible_paged(&pi, &store, None, 1, 10, Visibility::Visible)
            .unwrap()
            .total,
        2
    );
    let grok = GrokSessionSource {
        root: sources.join("grok"),
        cache: store.cache_dir("grok"),
    };
    assert_eq!(
        list_visible_paged(&grok, &store, None, 1, 10, Visibility::Visible)
            .unwrap()
            .total,
        2
    );
    // 隐藏按源隔离：claude 下登记的隐藏只在 claude 生效，codex 同名 id 依旧可见（id 不同源各自独立）。
    store.set_hidden("claude-cli", claude_ids[0], true).unwrap();
    assert_eq!(
        list_visible_paged(&claude, &store, None, 1, 10, Visibility::Visible)
            .unwrap()
            .total,
        2
    );
    assert_eq!(
        list_visible_paged(&codex, &store, None, 1, 10, Visibility::Visible)
            .unwrap()
            .total,
        2
    );

    // 详情：四源各自的详情解析在真实 Windows 路径上抽对话预览与改动文件。
    let claude_meta = scan_at(&sources.join("claude"), &store.cache_dir("claude-cli"))
        .into_iter()
        .find(|m| m.session_id == claude_ids[2])
        .unwrap();
    let claude_detail = dashboard_core::sessions::detail(&claude_meta, 5);
    assert!(claude_detail
        .preview
        .iter()
        .any(|line| line.contains("Windows 会话")));
    assert!(claude_detail.files.iter().any(|f| f.ends_with("a.rs")));

    let codex_meta = scan_codex_at(&sources.join("codex"), &store.cache_dir("codex"))
        .into_iter()
        .find(|m| m.session_id == codex_ids[2])
        .unwrap();
    let codex_detail = dashboard_core::sessions::codex_detail(&codex_meta, 5);
    assert!(codex_detail
        .preview
        .iter()
        .any(|line| line.contains("Windows 会话")));
    assert!(codex_detail.files.iter().any(|f| f.ends_with("mod.rs")));

    let pi_meta = scan_pi_at(&sources.join("pi"), &store.cache_dir("pi"))
        .into_iter()
        .find(|m| m.session_id == pi_ids[1])
        .unwrap();
    let pi_detail = dashboard_core::sessions::pi_detail(&pi_meta, 5);
    assert!(pi_detail
        .preview
        .iter()
        .any(|line| line.contains("Windows 会话")));
    assert!(pi_detail.files.iter().any(|f| f.ends_with("新.rs")));

    let grok_meta = scan_grok_at(&sources.join("grok"), &store.cache_dir("grok"))
        .into_iter()
        .find(|m| m.session_id == grok_ids[1])
        .unwrap();
    let grok_detail = dashboard_core::sessions::grok_detail(&grok_meta, 5);
    assert!(grok_detail
        .preview
        .iter()
        .any(|line| line.contains("Windows 会话")));

    fs::remove_dir_all(root).unwrap();
}
