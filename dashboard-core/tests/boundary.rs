// 边界样本 fixture 集成测试（openspec tasks.md §2.10）
// 覆盖 4 类：尾行半 JSON / 缺 attachment / 空 ai-title / 多条 ai-title 末条覆盖
// 手写 jsonl 到 tempdir，跑 scan_at()，断言字段。

use dashboard_core::sessions::{
    detail, list_paged, scan_at, ClaudeCliSource, SessionMeta, SessionSource,
};
use std::fs;
use std::path::{Path, PathBuf};

// 每次测试新建独立 tempdir（避免并发测试互相污染）
fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "dashboard-core-test-{}-{}-{}",
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

// 在 tempdir 下新建 projects/<proj>/<sid>.jsonl 并写入 lines
fn write_session(root: &Path, project: &str, sid: &str, lines: &[&str]) {
    let dir = root.join("projects").join(project);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.jsonl", sid));
    fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn scan_meta(root: &Path, sid: &str) -> SessionMeta {
    let projects_dir = root.join("projects");
    let cache = root.join("cache");
    let metas = scan_at(&projects_dir, &cache);
    metas
        .into_iter()
        .find(|m| m.session_id == sid)
        .expect("session should be scanned")
}

#[test]
fn truncated_tail_line_is_skipped_not_panic() {
    // 场景：文件正在写入，最后一行只写了半条 JSON。scan 必须防御性跳过而不 panic。
    let tmp = make_tempdir("truncated");
    let sid = "trunc-01";
    let lines = &[
        r#"{"type":"user","cwd":"F:\\proj-x","gitBranch":"main"}"#,
        r#"{"type":"ai-title","aiTitle":"good-title"}"#,
        r#"{"type":"user","message":{"content":"hello"}"#, // 半条：缺右括号
    ];
    write_session(&tmp, "-c-Users-admin-proj-x", sid, lines);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.cwd, "F:\\proj-x");
    assert_eq!(meta.git_branch, "main");
    assert_eq!(meta.title, "good-title");
    // 半条不应污染 haystack
    assert!(!meta.haystack.contains("hello"));
}

#[test]
fn missing_attachment_falls_back_gracefully() {
    // 场景：session 里没有 attachment / cwd 字段，scan 不炸，cwd 留空。
    let tmp = make_tempdir("no-attach");
    let sid = "no-attach-01";
    let lines = &[
        r#"{"type":"user","message":{"content":"just some text"}}"#,
        r#"{"type":"ai-title","aiTitle":"still-titled"}"#,
    ];
    write_session(&tmp, "-c-Users-admin-void", sid, lines);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.cwd, "");
    assert_eq!(meta.git_branch, "");
    assert_eq!(meta.title, "still-titled");
    assert!(meta.haystack.contains("just some text"));
}

#[test]
fn empty_ai_title_does_not_overwrite_prior_or_leave_bogus() {
    // 场景：aiTitle 为空字符串时不应覆盖 meta.title（Python `if obj.get("aiTitle"):` 语义）
    let tmp = make_tempdir("empty-title");
    let sid = "empty-title-01";
    let lines = &[
        r#"{"type":"user","cwd":"F:\\proj-y"}"#,
        r#"{"type":"ai-title","aiTitle":"first-title"}"#,
        r#"{"type":"ai-title","aiTitle":""}"#, // 空 title 不应覆盖
    ];
    write_session(&tmp, "-c-Users-admin-proj-y", sid, lines);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.title, "first-title");
}

#[test]
fn multiple_ai_title_last_wins() {
    // 场景：多条 ai-title 全一致 / 或不一致 —— 末条覆盖前面所有
    let tmp = make_tempdir("multi-title");
    let sid = "multi-title-01";
    let lines = &[
        r#"{"type":"user","cwd":"F:\\proj-z"}"#,
        r#"{"type":"ai-title","aiTitle":"draft-1"}"#,
        r#"{"type":"ai-title","aiTitle":"draft-2"}"#,
        r#"{"type":"ai-title","aiTitle":"final-version"}"#,
    ];
    write_session(&tmp, "-c-Users-admin-proj-z", sid, lines);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.title, "final-version");
}

#[test]
fn detail_extracts_dialog_and_files_from_snapshot() {
    let tmp = make_tempdir("detail");
    let sid = "detail-01";
    let lines = &[
        r#"{"type":"user","cwd":"F:\\proj-d","message":{"content":"question one"}}"#,
        r#"{"type":"assistant","message":{"content":"answer one"}}"#,
        r#"{"type":"file-history-snapshot","snapshot":{"trackedFileBackups":{"F:\\proj-d\\a.rs":"...","F:\\proj-d\\b.rs":"..."}}}"#,
    ];
    write_session(&tmp, "-c-Users-admin-proj-d", sid, lines);
    let meta = scan_meta(&tmp, sid);
    let d = detail(&meta, 10);
    assert_eq!(d.preview.len(), 2);
    assert!(d.preview[0].contains("question one"));
    assert!(d.preview[1].contains("answer one"));
    assert_eq!(d.files, vec!["F:\\proj-d\\a.rs".to_string(), "F:\\proj-d\\b.rs".to_string()]);
}

#[test]
fn detail_falls_back_to_tool_use_when_no_snapshot() {
    let tmp = make_tempdir("detail-fb");
    let sid = "detail-fb-01";
    let lines = &[
        r#"{"type":"user","cwd":"F:\\proj-f"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"F:\\proj-f\\edited.rs"}}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"F:\\proj-f\\new.rs"}}]}}"#,
    ];
    write_session(&tmp, "-c-Users-admin-proj-f", sid, lines);
    let meta = scan_meta(&tmp, sid);
    let d = detail(&meta, 10);
    assert_eq!(d.files, vec!["F:\\proj-f\\edited.rs".to_string(), "F:\\proj-f\\new.rs".to_string()]);
}

#[test]
fn scan_produces_stable_sort_by_mtime_desc() {
    // 两个 session 依赖文件系统 mtime 排序 —— 靠创建顺序保证 old→new 关系
    let tmp = make_tempdir("sort");
    write_session(&tmp, "-c-Users-admin-proj-old", "old-sid", &[
        r#"{"type":"user","cwd":"F:\\proj-old"}"#,
    ]);
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_session(&tmp, "-c-Users-admin-proj-new", "new-sid", &[
        r#"{"type":"user","cwd":"F:\\proj-new"}"#,
    ]);
    let metas = scan_at(&tmp.join("projects"), &tmp.join("cache"));
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].session_id, "new-sid"); // 最新在前
    assert_eq!(metas[1].session_id, "old-sid");
}

#[test]
fn cache_hit_short_circuits_reparse() {
    // 冷扫写缓存 → 第二次相同 mtime+size → 命中缓存直接返回
    let tmp = make_tempdir("cache");
    write_session(&tmp, "-c-Users-admin-proj-cache", "cache-01", &[
        r#"{"type":"user","cwd":"F:\\proj-cache"}"#,
        r#"{"type":"ai-title","aiTitle":"cached"}"#,
    ]);
    let projects_dir = tmp.join("projects");
    let cache = tmp.join("cache");
    let first = scan_at(&projects_dir, &cache);
    assert_eq!(first[0].title, "cached");
    // 缓存文件应存在
    assert!(cache.join("cache-01.json").is_file());
    // 二扫：即使缓存文件被替换成"错的"内容，只要 mtime+size 匹配也应直接返回缓存
    let fake = r#"{"session_id":"cache-01","path":"fake","mtime":0.0,"size":0,"title":"from-cache-not-file"}"#;
    // 保持 mtime+size 与真正的 meta 一致 —— 直接从磁盘再读一次拿真值
    let real_meta = &first[0];
    let injected = serde_json::json!({
        "session_id": "cache-01",
        "path": real_meta.path,
        "mtime": real_meta.mtime,
        "size": real_meta.size,
        "cwd": real_meta.cwd,
        "title": "from-cache-not-file",
        "git_branch": "",
        "haystack": "",
        "projects": "",
    });
    fs::write(cache.join("cache-01.json"), injected.to_string()).unwrap();
    let second = scan_at(&projects_dir, &cache);
    assert_eq!(second[0].title, "from-cache-not-file", "should serve from cache");
    let _ = fake; // silence unused
}

#[test]
fn detail_respects_rounds_by_user_count() {
    // rounds 语义：1 轮 = 1 user + 1 assistant。请求 rounds=3 → preview 恰好含最后 3 个 user 消息
    // 及其后的 assistant 回应；早于此的对话不出现。
    let tmp = make_tempdir("detail-rounds");
    let sid = "rounds-01";
    let lines: Vec<String> = (1..=6)
        .flat_map(|i| vec![
            format!(r#"{{"type":"user","message":{{"content":"Q{}"}}}}"#, i),
            format!(r#"{{"type":"assistant","message":{{"content":"A{}"}}}}"#, i),
        ])
        .collect();
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    write_session(&tmp, "-c-Users-admin-rounds", sid, &refs);
    let meta = scan_meta(&tmp, sid);
    let d = detail(&meta, 3);
    // 只应含 Q4/A4/Q5/A5/Q6/A6（最后 3 轮）
    let joined = d.preview.join("|");
    assert!(joined.contains("Q4"), "should include Q4: {}", joined);
    assert!(joined.contains("A6"), "should include A6: {}", joined);
    assert!(!joined.contains("Q3"), "should NOT include Q3: {}", joined);
    assert!(!joined.contains("Q1"), "should NOT include Q1: {}", joined);
    let user_count = d.preview.iter().filter(|l| l.starts_with("🧑")).count();
    assert_eq!(user_count, 3, "exactly 3 user messages kept");
}

#[test]
fn title_fallback_uses_first_user_message_when_ai_title_missing() {
    // cc 短会话不生成 ai-title 时，用首条 user 消息前 30 字兜底
    let tmp = make_tempdir("title-fallback");
    let sid = "no-title-01";
    write_session(&tmp, "-c-Users-admin-notitle", sid, &[
        r#"{"type":"user","cwd":"F:\\notitle","message":{"content":"帮我看看这段 Rust 代码有什么问题这句话超过三十字符了嘛应该差不多了"}}"#,
        r#"{"type":"assistant","message":{"content":"没问题"}}"#,
    ]);
    let meta = scan_meta(&tmp, sid);
    assert!(!meta.title.is_empty(), "fallback title should be set");
    assert!(meta.title.starts_with("帮我看看"), "fallback title should start from user msg: {}", meta.title);
    let char_count = meta.title.chars().count();
    assert!(char_count <= 30, "fallback title truncated to <=30 chars, got {}", char_count);
}

#[test]
fn title_ai_title_wins_over_fallback() {
    // 有 ai-title 时不走 fallback（回归保护）
    let tmp = make_tempdir("title-win");
    let sid = "has-title-01";
    write_session(&tmp, "-c-Users-admin-hastitle", sid, &[
        r#"{"type":"user","cwd":"F:\\has","message":{"content":"random user query"}}"#,
        r#"{"type":"ai-title","aiTitle":"生成的标题"}"#,
    ]);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.title, "生成的标题");
}

#[test]
fn source_field_defaults_to_claude_cli() {
    // scan_at 出的 meta 必然带 source=claude-cli 标识（Q3 抽象层留口子）
    let tmp = make_tempdir("source-field");
    let sid = "src-01";
    write_session(&tmp, "-c-Users-admin-src", sid, &[
        r#"{"type":"user","cwd":"F:\\src","message":{"content":"x"}}"#,
    ]);
    let meta = scan_meta(&tmp, sid);
    assert_eq!(meta.source, "claude-cli");
}

#[test]
fn source_backfill_when_cache_lacks_source_field() {
    // 老 cache 文件没 source 字段（v0 遗留），scan 时应 backfill 为 claude-cli
    let tmp = make_tempdir("source-backfill");
    let sid = "src-02";
    write_session(&tmp, "-c-Users-admin-src2", sid, &[
        r#"{"type":"user","cwd":"F:\\src2","message":{"content":"x"}}"#,
    ]);
    let projects_dir = tmp.join("projects");
    let cache = tmp.join("cache");
    let first = scan_at(&projects_dir, &cache);
    // 第 1 次 scan 写 cache（含 source 字段）
    let meta1 = first.iter().find(|m| m.session_id == sid).unwrap();
    let cache_file = cache.join(format!("{}.json", sid));
    // 模拟老 v0 cache：把 source 字段从 JSON 里删掉
    let raw = fs::read_to_string(&cache_file).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    v.as_object_mut().unwrap().remove("source");
    // 同时保留 mtime + size 一致（走 cache 命中路径）
    v["mtime"] = serde_json::json!(meta1.mtime);
    v["size"] = serde_json::json!(meta1.size);
    fs::write(&cache_file, v.to_string()).unwrap();
    // 第 2 次 scan：cache 命中，backfill 生效
    let second = scan_at(&projects_dir, &cache);
    let meta2 = second.iter().find(|m| m.session_id == sid).unwrap();
    assert_eq!(meta2.source, "claude-cli", "backfilled from empty");
}

#[test]
fn pagination_slices_correctly() {
    // 写 12 个 session，验证 page_size=5 的 3 页切片正确
    let tmp = make_tempdir("pagination-slice");
    let projects_dir = tmp.join("projects");
    let cache = tmp.join("cache");
    // sid-01 是最老的，sid-12 是最新的（mtime 递增，返回时倒序）
    for i in 1..=12 {
        write_session(&tmp, "-c-Users-admin-pag", &format!("sid-{:02}", i), &[
            &format!(r#"{{"type":"user","cwd":"F:\\pag","message":{{"content":"msg-{:02}"}}}}"#, i),
        ]);
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    struct AdhocSource { root: PathBuf, cache: PathBuf }
    impl SessionSource for AdhocSource {
        fn source_id(&self) -> &'static str { "claude-cli" }
        fn scan(&self) -> Vec<SessionMeta> { scan_at(&self.root, &self.cache) }
    }
    let src = AdhocSource { root: projects_dir.clone(), cache: cache.clone() };
    let sources: Vec<&dyn SessionSource> = vec![&src];

    let p1 = list_paged(&sources, None, 1, 5);
    assert_eq!(p1.total, 12);
    assert_eq!(p1.page, 1);
    assert_eq!(p1.page_size, 5);
    assert_eq!(p1.items.len(), 5);
    assert_eq!(p1.items[0].session_id, "sid-12", "newest first");
    assert_eq!(p1.items[4].session_id, "sid-08");

    let p2 = list_paged(&sources, None, 2, 5);
    assert_eq!(p2.items.len(), 5);
    assert_eq!(p2.items[0].session_id, "sid-07");
    assert_eq!(p2.items[4].session_id, "sid-03");

    let p3 = list_paged(&sources, None, 3, 5);
    assert_eq!(p3.items.len(), 2, "last page partial");
    assert_eq!(p3.items[0].session_id, "sid-02");
    assert_eq!(p3.items[1].session_id, "sid-01");

    // 越界页返回空但 total 保真
    let p99 = list_paged(&sources, None, 99, 5);
    assert_eq!(p99.total, 12);
    assert_eq!(p99.items.len(), 0);
}

#[test]
fn pagination_applies_query_before_slice() {
    // query 过滤后再分页：写 6 个 session 其中 3 个含关键词，page_size=2 应有 2 页
    let tmp = make_tempdir("pagination-query");
    let projects_dir = tmp.join("projects");
    let cache = tmp.join("cache");
    for (i, tag) in ["rust", "python", "rust", "go", "rust", "java"].iter().enumerate() {
        write_session(&tmp, "-c-Users-admin-pq", &format!("q-{:02}", i), &[
            &format!(r#"{{"type":"user","cwd":"F:\\pq","message":{{"content":"about {}"}}}}"#, tag),
        ]);
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let src = ClaudeCliSource { root: projects_dir, cache };
    let sources: Vec<&dyn SessionSource> = vec![&src];
    let p1 = list_paged(&sources, Some("rust"), 1, 2);
    assert_eq!(p1.total, 3, "3 rust matches");
    assert_eq!(p1.items.len(), 2);
    let p2 = list_paged(&sources, Some("rust"), 2, 2);
    assert_eq!(p2.total, 3);
    assert_eq!(p2.items.len(), 1);
}

#[test]
fn pagination_clamps_invalid_inputs() {
    // page=0 应回落到 1；page_size=0 应回落到 clamp 下限；不 panic
    let tmp = make_tempdir("pagination-clamp");
    let projects_dir = tmp.join("projects");
    let cache = tmp.join("cache");
    write_session(&tmp, "-c-Users-admin-clamp", "sid-only", &[
        r#"{"type":"user","cwd":"F:\\clamp","message":{"content":"x"}}"#,
    ]);
    let src = ClaudeCliSource { root: projects_dir, cache };
    let sources: Vec<&dyn SessionSource> = vec![&src];
    let p = list_paged(&sources, None, 0, 0);
    assert_eq!(p.page, 1);
    assert!(p.page_size >= 1);
    assert_eq!(p.items.len(), 1);
}
