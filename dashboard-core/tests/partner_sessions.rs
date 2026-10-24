use dashboard_core::{
    commands, partner_sessions::PartnerSessionSource, session_storage::SessionStorage,
    session_visibility::{list_visible_paged, Visibility},
    sessions::SessionSource,
};
use rusqlite::{params, Connection};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("dashboard-partner-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn source(&self, id: &'static str, path: &str) -> PartnerSessionSource {
        PartnerSessionSource::new(id, self.0.join(path), self.0.join("cache").join(id)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn database(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_updated INTEGER, parent_id TEXT, time_archived INTEGER, time_deleted INTEGER);
      CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
      CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
      CREATE INDEX message_session ON message(session_id, time_created);
      CREATE INDEX part_message ON part(message_id);").unwrap();
    c
}
fn session(c: &Connection, id: &str) {
    c.execute("INSERT INTO session VALUES(?1, '合成会话标题', '/tmp/demo', 1800000000000, NULL, NULL, NULL)", [id]).unwrap();
    for (n, role) in ["user", "assistant"].iter().enumerate() {
        let n = n as i64;
        let mid = format!("{id}-message-{n}");
        c.execute(
            "INSERT INTO message VALUES(?1, ?2, ?3, ?4)",
            params![mid, id, n, json!({"role":role}).to_string()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO part VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                format!("{id}-part-{n}"),
                mid,
                id,
                n,
                json!({"type":"text","text":format!("{role} 合成正文")}).to_string()
            ],
        )
        .unwrap();
        c.execute(
            "INSERT INTO part VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                format!("{id}-secret-{n}"),
                mid,
                id,
                n,
                json!({"type":"reasoning","text":"不能进入预览"}).to_string()
            ],
        )
        .unwrap();
    }
}

#[test]
fn sql_reads_committed_wal_and_filters_internal_archived_deleted_sessions() {
    let f = Fixture::new();
    let c = database(&f.0.join("sessions.db"));
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    for id in ["ses_public", "ses_child", "ses_archived", "ses_deleted"] {
        session(&c, id);
    }
    c.execute_batch("UPDATE session SET parent_id='ses_public' WHERE id='ses_child'; UPDATE session SET time_archived=1 WHERE id='ses_archived'; UPDATE session SET time_deleted=1 WHERE id='ses_deleted';").unwrap();
    let source = f.source("zcode", "sessions.db");
    let before_db = fs::read(f.0.join("sessions.db")).unwrap();
    let before_wal = fs::read(f.0.join("sessions.db-wal")).unwrap();
    let rows = source.try_scan().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "ses_public");
    assert_eq!(rows[0].source, "zcode");
    assert_eq!(rows[0].cwd, "/tmp/demo");
    let detail = source.detail("ses_public").unwrap().unwrap();
    assert_eq!(detail.preview.len(), 2);
    assert!(detail.preview[0].contains("user 合成正文"));
    assert!(!detail.preview.join("").contains("不能进入预览"));
    assert!(source.detail("ses_deleted").unwrap().is_none());
    assert_eq!(fs::read(f.0.join("sessions.db")).unwrap(), before_db);
    assert_eq!(fs::read(f.0.join("sessions.db-wal")).unwrap(), before_wal);
    c.execute(
        "UPDATE session SET title='更新标题' WHERE id='ses_public'",
        [],
    )
    .unwrap();
    assert_eq!(source.try_scan().unwrap()[0].title, "更新标题");
}

#[test]
fn sources_distinguish_missing_corrupt_and_unknown_and_reject_injected_ids() {
    let f = Fixture::new();
    assert!(f
        .source("opencode", "missing.db")
        .try_scan()
        .unwrap()
        .is_empty());
    fs::write(f.0.join("broken.db"), "not sqlite").unwrap();
    assert!(f.source("opencode", "broken.db").try_scan().is_err());
    assert!(PartnerSessionSource::new("invented", f.0.clone(), f.0.join("cache")).is_err());
    assert!(f
        .source("zcode", "missing.db")
        .detail("../../secret")
        .is_err());
    for id in ["../oops", "a;whoami", "", "-flag", "."] {
        assert!(
            commands::copy_resume_command_for_source("opencode", id).is_err(),
            "{id}"
        );
    }
    assert_eq!(
        commands::copy_resume_command_for_source("opencode", "ses_demo").unwrap(),
        "opencode --session ses_demo"
    );
    assert_eq!(
        commands::copy_resume_command_for_source("antigravity", "demo-id").unwrap(),
        "agy --conversation demo-id"
    );
    for tool in ["dsh", "zcode", "gemini-cli"] {
        assert!(commands::copy_resume_command_for_source(tool, "safe-id")
            .unwrap_err()
            .to_string()
            .contains("RESUME_UNSUPPORTED"));
    }
}

#[test]
fn antigravity_summary_is_not_fabricated_as_dialogue() {
    let f = Fixture::new();
    let c = Connection::open(f.0.join("summary.db")).unwrap();
    c.execute_batch("CREATE TABLE conversation_summaries(conversation_id TEXT, title TEXT, preview TEXT, last_modified_time TEXT, workspace_uris TEXT, parent_conversation_id TEXT);").unwrap();
    c.execute("INSERT INTO conversation_summaries VALUES('demo-id', '合成摘要标题', '合成摘要正文', '2026-10-20 12:00:00+00:00', '[\"file:///tmp/demo%20project\"]', '')", []).unwrap();
    let source = f.source("antigravity", "summary.db");
    let rows = source.try_scan().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].mtime > 1_700_000_000.0);
    assert_eq!(rows[0].cwd, "/tmp/demo project");
    let d = source.detail("demo-id").unwrap().unwrap();
    assert!(d.preview.is_empty());
    assert_eq!(d.summary, "合成摘要正文");
    assert!(d.notice.contains("完整对话"));
}

fn dsh_lines(title: &str) -> String {
    [json!({"type":"session","version":4,"id":"test-id","cwd":"C:\\demo","createdAt":"2026-10-20T01:00:00Z"}),
     json!({"type":"user/message","data":{"role":"user","content":[{"type":"text","text":"合成提问"}]}}),
     json!({"type":"session/title","data":{"title":title}}),
     json!({"type":"assistant/message","data":{"message":{"role":"assistant","content":[{"type":"text","text":"合成答复"},{"type":"thinking","text":"隐藏思考"}]}}})]
     .into_iter().map(|x| x.to_string()+"\n").collect()
}

#[test]
fn dsh_v4_compressed_history_is_cached_and_refreshes_on_change() {
    let f = Fixture::new();
    let dir = f.0.join("sessions/workspace/session-test-id");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("session.v4.jsonl.zstd");
    fs::write(
        &p,
        zstd::encode_all(dsh_lines("第一标题").as_bytes(), 1).unwrap(),
    )
    .unwrap();
    let source = f.source("dsh", "sessions");
    let rows = source.try_scan().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "第一标题");
    assert_eq!(rows[0].cwd, "C:\\demo");
    let detail = source.detail("test-id").unwrap().unwrap();
    assert_eq!(detail.preview.len(), 2);
    assert!(!detail.preview.join("").contains("隐藏思考"));
    fs::write(
        &p,
        zstd::encode_all(dsh_lines("一个更长的新标题").as_bytes(), 1).unwrap(),
    )
    .unwrap();
    assert_eq!(source.try_scan().unwrap()[0].title, "一个更长的新标题");
    assert!(f.0.join("cache/dsh").exists());
}

#[test]
fn gemini_json_uses_explicit_project_root_and_deduplicates_session_id() {
    let f = Fixture::new();
    let dir = f.0.join("gemini/project-slug/chats");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.parent().unwrap().join(".project_root"),
        "/tmp/gemini-project\n",
    )
    .unwrap();
    let value = json!({"sessionId":"test-id","startTime":"2026-10-20T00:00:00Z","lastUpdated":"2026-10-21T01:00:00Z","messages":[
        {"type":"user","content":"合成 Gemini 问题"}, {"type":"gemini","content":"合成 Gemini 答案"}, {"type":"tool","content":"工具噪音"}]});
    fs::write(dir.join("session-one.json"), value.to_string()).unwrap();
    fs::write(dir.join("session-two.json"), value.to_string()).unwrap();
    let source = f.source("gemini-cli", "gemini");
    let rows = source.try_scan().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cwd, "/tmp/gemini-project");
    assert!(rows[0].title.contains("Gemini"));
    assert_eq!(source.detail("test-id").unwrap().unwrap().preview.len(), 2);
}

#[test]
fn favorites_are_source_scoped_and_old_entries_remain_compatible() {
    let f = Fixture::new();
    let store = SessionStorage::new(f.0.join("preferences"), f.0.join("legacy"));
    store.ensure_initialized().unwrap();
    store.toggle_favorite("same-id").unwrap();
    let old = store.load_favorites().unwrap();
    assert!(dashboard_core::session_storage::is_favorite(
        &old, "codex", "same-id"
    ));
    assert!(!dashboard_core::session_storage::is_favorite(
        &old, "opencode", "same-id"
    ));
    store
        .toggle_favorite_for_source("opencode", "same-id")
        .unwrap();
    store
        .toggle_favorite_for_source("codex", "same-id")
        .unwrap();
    let values = store.load_favorites().unwrap();
    assert!(dashboard_core::session_storage::is_favorite(
        &values, "opencode", "same-id"
    ));
    assert!(!dashboard_core::session_storage::is_favorite(
        &values, "zcode", "same-id"
    ));
    assert!(!dashboard_core::session_storage::is_favorite(
        &values, "codex", "same-id"
    ));
    assert!(dashboard_core::session_storage::is_favorite(
        &values,
        "claude-cli",
        "same-id"
    ));
}

#[test]
fn sql_busy_and_unknown_schema_are_errors_not_empty_lists() {
    let f = Fixture::new();
    let c = database(&f.0.join("locked.db"));
    session(&c, "ses_demo");
    c.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let start = std::time::Instant::now();
    assert!(f.source("opencode", "locked.db").try_scan().is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    c.execute_batch("ROLLBACK;").unwrap();
    let c = Connection::open(f.0.join("unknown.db")).unwrap();
    c.execute_batch("CREATE TABLE session(id TEXT);").unwrap();
    assert!(f.source("opencode", "unknown.db").try_scan().is_err());
}

#[test]
fn dsh_multiple_frames_and_unsupported_version_are_explicit() {
    let f = Fixture::new();
    let dir = f.0.join("sessions/workspace/test");
    fs::create_dir_all(&dir).unwrap();
    let mut bytes = zstd::encode_all(dsh_lines("合成标题").as_bytes(), 1).unwrap();
    let extra=json!({"type":"assistant/message","data":{"message":{"role":"assistant","content":"第二 frame 的答复"}}}).to_string()+"\n";
    bytes.extend(zstd::encode_all(extra.as_bytes(), 1).unwrap());
    fs::write(dir.join("session.v4.jsonl.zstd"), bytes).unwrap();
    let source = f.source("dsh", "sessions");
    assert!(source
        .detail("test-id")
        .unwrap()
        .unwrap()
        .preview
        .last()
        .unwrap()
        .contains("第二 frame"));
    fs::write(dir.join("session.v5.jsonl.zstd"), "unknown version").unwrap();
    assert!(source.try_scan().is_err(), "未知 DSH 格式不得静默忽略");
}

#[test]
fn large_gemini_list_is_bounded_while_detail_reads_selected_history() {
    let f = Fixture::new();
    let dir = f.0.join("gemini/project/chats");
    fs::create_dir_all(&dir).unwrap();
    let content=json!({"sessionId":"large-id","messages":[{"type":"user","content":"头部用户提问"},{"type":"tool","content":"x".repeat(2*1024*1024)},{"type":"gemini","content":"尾部答复"}]}).to_string();
    fs::write(dir.join("session-large.json"), content).unwrap();
    let source = f.source("gemini-cli", "gemini");
    let rows = source.try_scan().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].title.contains("头部用户提问"));
    assert!(
        !rows[0].haystack.contains("尾部答复"),
        "列表不得越过 1MiB 预算全文读取"
    );
    let detail = source.detail("large-id").unwrap().unwrap();
    assert!(detail.preview.last().unwrap().contains("尾部答复"));
}

#[cfg(unix)]
#[test]
fn source_symlinks_and_sql_sidecar_symlinks_cannot_escape() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let c = database(&f.0.join("real.db"));
    session(&c, "ses_demo");
    drop(c);
    symlink(f.0.join("real.db"), f.0.join("link.db")).unwrap();
    assert!(f.source("zcode", "link.db").try_scan().is_err());
    fs::write(f.0.join("outside"), "private fixture").unwrap();
    symlink(f.0.join("outside"), f.0.join("real.db-wal")).unwrap();
    assert!(f.source("zcode", "real.db").try_scan().is_err());
    let dir = f.0.join("gemini/project/chats");
    fs::create_dir_all(&dir).unwrap();
    symlink(f.0.join("outside"), dir.join("session-symlink.json")).unwrap();
    assert!(f
        .source("gemini-cli", "gemini")
        .try_scan()
        .unwrap()
        .is_empty());
}

#[test]
#[ignore = "显式执行的本机只读验收；仅打印来源数量和耗时，缓存使用临时目录"]
fn local_partner_sources_probe() {
    let f = Fixture::new();
    let storage = SessionStorage::new(f.0.join("preferences"), f.0.join("legacy"));
    let home = dirs::home_dir().unwrap();
    for (id, relative) in [
        ("opencode", ".local/share/opencode/opencode.db"),
        ("zcode", ".zcode/cli/db/db.sqlite"),
        (
            "antigravity",
            ".gemini/antigravity-cli/conversation_summaries.db",
        ),
        ("dsh", ".dsh/sessions"),
        ("gemini-cli", ".gemini/tmp"),
    ] {
        let source = PartnerSessionSource::new(id, home.join(relative), f.0.join(id)).unwrap();
        let start = std::time::Instant::now();
        let mut rows = source.try_scan().unwrap();
        let cold = start.elapsed();
        rows.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        let start = std::time::Instant::now();
        assert_eq!(source.try_scan().unwrap().len(), rows.len());
        let warm = start.elapsed();
        let start = std::time::Instant::now();
        let page = list_visible_paged(&source, &storage, None, 2, 50, Visibility::Visible)
            .unwrap();
        let paged = start.elapsed();
        assert_eq!(page.total, rows.len());
        assert!(page.items.len() <= 50);
        let mut messages = 0;
        for row in rows.iter().take(3) {
            let detail = source.detail(&row.session_id).unwrap().unwrap();
            messages += detail.preview.len();
        }
        println!(
            "{id}: sessions={}, cold={cold:?}, warm={warm:?}, page_50={paged:?}, sample_messages={messages}",
            rows.len()
        );
    }
}
