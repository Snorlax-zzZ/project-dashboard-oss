//! 新工具历史读取。固定来源、只读原文件；不调用工具进程，不访问认证配置。
use crate::{
    session_storage::{self, SessionStorageError},
    sessions::{SessionDetail, SessionMeta, SessionSource},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
    Deserialize, Serialize,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

pub const PARTNER_SOURCES: [&str; 5] = ["opencode", "antigravity", "dsh", "zcode", "gemini-cli"];
const LIST_BUDGET: u64 = 1024 * 1024;
const DETAIL_BUDGET: u64 = 64 * 1024 * 1024;
const CACHE_VERSION: u32 = 1;
type Result<T> = std::result::Result<T, SessionStorageError>;
fn error(detail: impl std::fmt::Display) -> SessionStorageError {
    SessionStorageError::new("SESSION_SOURCE_READ_FAILED", detail)
}

pub struct PartnerSessionSource {
    id: &'static str,
    root: PathBuf,
    cache: PathBuf,
}
impl PartnerSessionSource {
    pub fn new(id: &str, root: PathBuf, cache: PathBuf) -> Result<Self> {
        let id = PARTNER_SOURCES
            .iter()
            .copied()
            .find(|v| *v == id)
            .ok_or_else(|| error("不支持的会话来源"))?;
        Ok(Self { id, root, cache })
    }
    pub fn default_paths(id: &str) -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| error("无法确定用户目录"))?;
        let path = match id {
            "opencode" => std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".local/share"))
                .join("opencode/opencode.db"),
            "zcode" => home.join(".zcode/cli/db/db.sqlite"),
            "antigravity" => home.join(".gemini/antigravity-cli/conversation_summaries.db"),
            "dsh" => home.join(".dsh/sessions"),
            "gemini-cli" => home.join(".gemini/tmp"),
            _ => return Err(error("不支持的会话来源")),
        };
        Self::new(id, path, session_storage::default_storage().cache_dir(id))
    }
    fn is_database(&self) -> bool {
        matches!(self.id, "opencode" | "zcode" | "antigravity")
    }
    pub fn detail(&self, sid: &str) -> Result<Option<SessionDetail>> {
        session_storage::validate_id(sid)?;
        if self.is_database() {
            let Some(c) = open_database(&self.root)? else {
                return Ok(None);
            };
            if self.id == "antigravity" {
                let summary: Option<String> = c.query_row("SELECT preview FROM conversation_summaries WHERE conversation_id=?1 AND COALESCE(parent_conversation_id,'')=''", [sid], |r| r.get(0)).optional().map_err(error)?;
                return Ok(summary.map(|summary| SessionDetail {
                    summary: clipped(&summary, 16000),
                    notice: "此工具目前仅支持会话摘要，暂不支持完整对话。".into(),
                    ..Default::default()
                }));
            }
            let filter = sql_filter(&c)?;
            let exists: bool = c
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM session WHERE id=?1 AND {filter})"),
                    [sid],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if !exists {
                return Ok(None);
            }
            return sql_detail(&c, sid).map(Some);
        }
        let Some(meta) = self.try_scan()?.into_iter().find(|m| m.session_id == sid) else {
            return Ok(None);
        };
        let p = Path::new(&meta.path);
        ensure_within(&self.root, p)?;
        let parsed = self.parse_file(p, DETAIL_BUDGET)?;
        if parsed.meta.session_id != sid {
            return Ok(None);
        }
        Ok(Some(parsed.detail()))
    }
    fn scan_database(&self) -> Result<Vec<SessionMeta>> {
        let Some(c) = open_database(&self.root)? else {
            return Ok(Vec::new());
        };
        let mut metas = Vec::new();
        if self.id == "antigravity" {
            let mut stmt = c.prepare("SELECT conversation_id,title,preview,last_modified_time,workspace_uris FROM conversation_summaries WHERE COALESCE(parent_conversation_id,'')='' ORDER BY last_modified_time DESC").map_err(error)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(error)?;
            for row in rows {
                let (sid, title, preview, time, uris) = row.map_err(error)?;
                if session_storage::validate_id(&sid).is_err() {
                    continue;
                }
                let cwd = serde_json::from_str::<Vec<String>>(&uris)
                    .unwrap_or_default()
                    .iter()
                    .find_map(|s| file_uri_path(s))
                    .unwrap_or_default();
                metas.push(self.meta(sid, title, cwd, parse_time(&time), clipped(&preview, 8000)));
            }
        } else {
            let filter = sql_filter(&c)?;
            let mut stmt = c.prepare(&format!("SELECT id,title,directory,time_updated FROM session WHERE {filter} ORDER BY time_updated DESC")).map_err(error)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(error)?;
            for row in rows {
                let (sid, title, cwd, time) = row.map_err(error)?;
                if session_storage::validate_id(&sid).is_err() {
                    continue;
                }
                metas.push(self.meta(sid, title, cwd, time as f64 / 1000.0, String::new()));
            }
        }
        Ok(metas)
    }
    fn meta(
        &self,
        sid: String,
        title: String,
        cwd: String,
        mtime: f64,
        haystack: String,
    ) -> SessionMeta {
        SessionMeta {
            session_id: sid,
            title: clipped(&title, 500),
            projects: project(&cwd),
            cwd,
            mtime,
            haystack,
            source: self.id.into(),
            path: self.root.to_string_lossy().into(),
            ..Default::default()
        }
    }
    fn parse_file(&self, path: &Path, budget: u64) -> Result<Parsed> {
        ensure_within(&self.root, path)?;
        let file = File::open(path).map_err(error)?;
        let stat = file.metadata().map_err(error)?;
        let mut result = if self.id == "dsh" {
            let mut decoder = zstd::Decoder::new(file).map_err(error)?;
            decoder.window_log_max(26).map_err(error)?;
            parse_dsh(decoder, budget)?
        } else {
            parse_gemini(file, budget)?
        };
        session_storage::validate_id(&result.meta.session_id)?;
        result.meta.source = self.id.into();
        result.meta.path = path.to_string_lossy().into();
        result.meta.size = stat.len();
        if result.meta.mtime <= 0.0 || self.id == "dsh" {
            result.meta.mtime = stat
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
        }
        if self.id == "gemini-cli" && result.meta.cwd.is_empty() {
            if let Some(parent) = path.parent().and_then(Path::parent) {
                let root_file = parent.join(".project_root");
                if ensure_within(&self.root, &root_file).is_ok() {
                    if let Ok(mut f) = File::open(root_file) {
                        let mut text = String::new();
                        if f.by_ref().take(16384).read_to_string(&mut text).is_ok() {
                            result.meta.cwd = text.trim().into();
                        }
                    }
                }
            }
        }
        if result.meta.title.trim().is_empty() {
            result.meta.title = "未命名会话".into();
        }
        result.meta.title = clipped(&result.meta.title, 500);
        result.meta.projects = project(&result.meta.cwd);
        result.meta.haystack = clipped(
            &result
                .messages
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            8000,
        );
        Ok(result)
    }
    fn scan_files(&self) -> Result<Vec<SessionMeta>> {
        if !root_exists(&self.root)? {
            return Ok(Vec::new());
        }
        let mut newest = HashMap::<String, SessionMeta>::new();
        let mut failure = None;
        for entry in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .max_depth(3)
        {
            let entry = match entry {
                Ok(v) => v,
                Err(e) => {
                    failure = Some(error(e));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            let name = entry.file_name().to_string_lossy();
            if self.id == "dsh"
                && name.starts_with("session.v")
                && name.ends_with(".jsonl.zstd")
                && name != "session.v4.jsonl.zstd"
            {
                failure = Some(error("DSH 会话版本暂不支持，目前支持 v4"));
                continue;
            }
            let wanted = if self.id == "dsh" {
                name == "session.v4.jsonl.zstd"
            } else {
                p.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|v| v == "chats")
                    && name.starts_with("session-")
                    && name.ends_with(".json")
            };
            if !wanted {
                continue;
            }
            let meta = match self.cached_meta(p) {
                Ok(v) => v,
                Err(e) => {
                    failure = Some(e);
                    continue;
                }
            };
            match newest.get(&meta.session_id) {
                Some(old) if old.mtime >= meta.mtime => {}
                _ => {
                    newest.insert(meta.session_id.clone(), meta);
                }
            }
        }
        // 部分损坏不能悄悄减少 total；错误只影响当前来源，可修复后重新读取。
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(newest.into_values().collect())
    }
    fn cached_meta(&self, path: &Path) -> Result<SessionMeta> {
        ensure_within(&self.root, path)?;
        let stat = fs::metadata(path).map_err(error)?;
        let fingerprint = format!("{}:{:?}", stat.len(), stat.modified().map_err(error)?);
        let hash = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        let cache_path = self.cache.join(format!("{hash}.json"));
        if let Ok(Some(entry)) =
            session_storage::read_optional::<FileCache>(&cache_path, "SESSION_CACHE_READ_FAILED")
        {
            if entry.version == CACHE_VERSION
                && entry.fingerprint == fingerprint
                && entry.meta.source == self.id
                && entry.meta.path == path.to_string_lossy()
                && session_storage::validate_id(&entry.meta.session_id).is_ok()
            {
                return Ok(entry.meta);
            }
        }
        let result = self.parse_file(path, LIST_BUDGET)?;
        let entry = FileCache {
            version: CACHE_VERSION,
            fingerprint,
            meta: result.meta,
        };
        // 缓存失败不影响只读历史；偏好写入仍由严格存储入口单独保证。
        if cache_dir_safe(&self.cache).is_ok() {
            if let Ok(bytes) = serde_json::to_vec(&entry) {
                let _ = session_storage::atomic_write(
                    &cache_path,
                    &bytes,
                    "SESSION_CACHE_WRITE_FAILED",
                );
            }
        }
        Ok(entry.meta)
    }
}
impl SessionSource for PartnerSessionSource {
    fn source_id(&self) -> &'static str {
        self.id
    }
    fn scan(&self) -> Vec<SessionMeta> {
        self.try_scan().unwrap_or_default()
    }
    fn try_scan(&self) -> Result<Vec<SessionMeta>> {
        if self.is_database() {
            self.scan_database()
        } else {
            self.scan_files()
        }
    }
}

#[derive(Serialize, Deserialize)]
struct FileCache {
    version: u32,
    fingerprint: String,
    meta: SessionMeta,
}
fn cache_dir_safe(p: &Path) -> Result<()> {
    if let Ok(m) = fs::symlink_metadata(p) {
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(error("缓存目录异常"));
        }
    }
    fs::create_dir_all(p).map_err(error)
}
fn root_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(error("会话来源不接受符号链接")),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}
fn ensure_within(root: &Path, path: &Path) -> Result<()> {
    let root = root.canonicalize().map_err(error)?;
    let m = fs::symlink_metadata(path).map_err(error)?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || !path.canonicalize().map_err(error)?.starts_with(root)
    {
        return Err(error("会话路径越出来源目录或不是普通文件"));
    }
    Ok(())
}
fn open_database(path: &Path) -> Result<Option<Connection>> {
    if !root_exists(path)? {
        return Ok(None);
    }
    if !fs::metadata(path).map_err(error)?.is_file() {
        return Err(error("会话数据库不是普通文件"));
    }
    for suffix in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        if let Ok(m) = fs::symlink_metadata(p) {
            if !m.is_file() || m.file_type().is_symlink() {
                return Err(error("数据库伴随文件异常"));
            }
        }
    }
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(error)?;
    c.busy_timeout(Duration::from_millis(300)).map_err(error)?;
    let started = std::time::Instant::now();
    c.progress_handler(
        10000,
        Some(move || started.elapsed() > Duration::from_secs(2)),
    )
    .map_err(error)?;
    c.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        16 * 1024 * 1024,
    )
    .map_err(error)?;
    // 只限制此连接的行为，不改变库文件；禁止 schema 中的函数扩展和写入。
    c.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
        .map_err(error)?;
    Ok(Some(c))
}
fn sql_filter(c: &Connection) -> Result<String> {
    let mut stmt = c.prepare("PRAGMA table_info(session)").map_err(error)?;
    let columns: HashSet<String> = stmt
        .query_map([], |r| r.get(1))
        .map_err(error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(error)?;
    if ![
        "id",
        "title",
        "directory",
        "time_updated",
        "parent_id",
        "time_archived",
    ]
    .iter()
    .all(|k| columns.contains(*k))
    {
        return Err(error("工具会话库格式不支持，请更新适配"));
    }
    let mut filter = "COALESCE(parent_id,'')='' AND time_archived IS NULL".to_string();
    if columns.contains("time_deleted") {
        filter.push_str(" AND time_deleted IS NULL");
    }
    Ok(filter)
}
fn sql_detail(c: &Connection, sid: &str) -> Result<SessionDetail> {
    let mut stmt=c.prepare("SELECT id,json_extract(data,'$.role') FROM message WHERE session_id=?1 ORDER BY time_created DESC,id DESC LIMIT 100").map_err(error)?;
    let messages = stmt
        .query_map([sid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    let mut out = Vec::new();
    let mut users = 0;
    let mut parts=c.prepare("SELECT substr(json_extract(data,'$.text'),1,4001) FROM part WHERE session_id=?1 AND message_id=?2 AND json_extract(data,'$.type')='text' ORDER BY time_created,id LIMIT 100").map_err(error)?;
    for (mid, data) in messages {
        let role = data.as_str();
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let mut text = String::new();
        for p in parts
            .query_map(rusqlite::params![sid, mid], |r| r.get::<_, String>(0))
            .map_err(error)?
        {
            text.push_str(&clipped(&p.map_err(error)?, 4000));
            text.push('\n');
            if text.len() > 16000 {
                break;
            }
        }
        if text.trim().is_empty() {
            continue;
        }
        out.push(format!(
            "{} {}",
            if role == "user" { "🧑" } else { "🤖" },
            clipped(text.trim(), 4000)
        ));
        if role == "user" {
            users += 1;
            if users == 5 {
                break;
            }
        }
    }
    out.reverse();
    Ok(SessionDetail {
        preview: out,
        notice: "近期文本预览，最多 5 轮；单条长消息会缩略。".into(),
        ..Default::default()
    })
}
fn clipped(s: &str, n: usize) -> String {
    let mut v: String = s.chars().take(n).collect();
    if s.chars().nth(n).is_some() {
        v.push('…');
    }
    v
}
fn project(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .into()
}
fn parse_time(s: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|_| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .map(|t| t.timestamp_millis() as f64 / 1000.0)
        .unwrap_or(0.0)
}
fn file_uri_path(s: &str) -> Option<String> {
    let rest = s.strip_prefix("file://")?;
    let rest = rest
        .strip_prefix("localhost/")
        .map(|s| format!("/{s}"))
        .unwrap_or_else(|| rest.to_owned());
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::new();
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            bytes.push(u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).ok()?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let p = String::from_utf8(bytes).ok()?;
    if p.as_bytes().get(2) == Some(&b':') {
        Some(p[1..].into())
    } else {
        Some(p)
    }
}

#[derive(Default)]
struct Parsed {
    meta: SessionMeta,
    messages: VecDeque<String>,
    limited: bool,
}
impl Parsed {
    fn message(&mut self, role: &str, content: &Value) {
        let text = text_content(content);
        if text.trim().is_empty() {
            return;
        }
        if self.meta.title.is_empty() && role == "user" {
            self.meta.title = clipped(text.trim(), 80);
        }
        self.messages.push_back(format!(
            "{} {}",
            if role == "user" { "🧑" } else { "🤖" },
            clipped(text.trim(), 4000)
        ));
        while self.messages.len() > 100 {
            self.messages.pop_front();
        }
    }
    fn detail(self) -> SessionDetail {
        let mut preview = Vec::new();
        let mut users = 0;
        for line in self.messages.into_iter().rev() {
            let user = line.starts_with("🧑");
            preview.push(line);
            if user {
                users += 1;
                if users == 5 {
                    break;
                }
            }
        }
        preview.reverse();
        SessionDetail {
            preview,
            notice: if self.limited {
                "会话超过本次读取预算或仍在写入；这里只展示已读取片段，不保证是最新对话。"
            } else {
                "近期文本预览，最多 5 轮；单条长消息会缩略。"
            }
            .into(),
            ..Default::default()
        }
    }
}
fn text_content(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return clipped(s, 16000);
    }
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|x| {
                    x.get("type")
                        .and_then(Value::as_str)
                        .is_none_or(|t| t == "text")
                })
                .filter_map(|x| x.get("text").and_then(Value::as_str))
                .take(100)
                .map(|s| clipped(s, 4000))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn parse_dsh(input: impl Read, budget: u64) -> Result<Parsed> {
    let mut reader = BufReader::new(input.take(budget + 1));
    let mut parsed = Parsed::default();
    let mut consumed = 0;
    loop {
        let mut line = String::new();
        let count = match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if parsed.meta.session_id.is_empty() {
                    return Err(error(e));
                }
                parsed.limited = true;
                break;
            }
        };
        consumed += count as u64;
        if consumed > budget {
            parsed.limited = true;
            break;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                parsed.limited = true;
                continue;
            }
        };
        let data = &v["data"];
        match v["type"].as_str().unwrap_or("") {
            "session" => {
                if v["version"].as_u64() != Some(4) {
                    return Err(error("DSH 会话版本暂不支持"));
                }
                parsed.meta.session_id = v["id"].as_str().unwrap_or("").into();
                parsed.meta.cwd = v["cwd"].as_str().unwrap_or("").into();
            }
            "session/title" => {
                if let Some(s) = data["title"].as_str() {
                    parsed.meta.title = s.into();
                }
            }
            "user/message" => parsed.message("user", &data["content"]),
            "assistant/message" => parsed.message("assistant", &data["message"]["content"]),
            _ => {}
        }
    }
    Ok(parsed)
}

// JSON 整体可能达到数百 MB；流式 visitor 在预算用尽时保留已解析的头部元信息。
struct Gemini<'a>(&'a mut Parsed);
impl<'de> Visitor<'de> for Gemini<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Gemini 会话对象")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "sessionId" => self.0.meta.session_id = map.next_value()?,
                "lastUpdated" => self.0.meta.mtime = parse_time(&map.next_value::<String>()?),
                "cwd" => self.0.meta.cwd = map.next_value()?,
                "summary" | "title" => {
                    let v: Value = map.next_value()?;
                    if let Some(s) = v.as_str() {
                        if !s.is_empty() {
                            self.0.meta.title = clipped(s, 500);
                        }
                    }
                }
                "messages" => map.next_value_seed(GeminiMessages(self.0))?,
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}
struct GeminiMessages<'a>(&'a mut Parsed);
impl<'de> DeserializeSeed<'de> for GeminiMessages<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for GeminiMessages<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Gemini 消息数组")
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<(), S::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            match v["type"].as_str() {
                Some("user") => self.0.message("user", &v["content"]),
                Some("gemini") => self.0.message("assistant", &v["content"]),
                _ => {}
            }
        }
        Ok(())
    }
}
fn parse_gemini(input: impl Read, budget: u64) -> Result<Parsed> {
    let mut reader = BufReader::new(input.take(budget));
    let mut p = Parsed::default();
    let mut d = serde_json::Deserializer::from_reader(&mut reader);
    if let Err(e) = serde::Deserializer::deserialize_map(&mut d, Gemini(&mut p)) {
        if e.is_eof() && !p.meta.session_id.is_empty() {
            p.limited = true;
        } else {
            return Err(error("Gemini 会话 JSON 损坏或格式不支持"));
        }
    }
    Ok(p)
}
