// 会话扫描与解析引擎（M1.5 Rust 版，对拍 M1 fleet/sessions.py）
// 铁律：对 Claude Code / Codex 会话目录只读；防御式解析（坏行/截断行跳过）；两阶段懒加载。
// 输出结构与字段名严格对拍 Python 版（`session_id / mtime / cwd / git_branch / haystack / projects`）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const CLAUDE_SOURCE_ID: &str = "claude-cli";
pub const CODEX_SOURCE_ID: &str = "codex";
pub const PI_SOURCE_ID: &str = "pi";
pub const GROK_SOURCE_ID: &str = "grok";
const CODEX_CACHE_VERSION: u32 = 2;
const PI_CACHE_VERSION: u32 = 1;
const GROK_CACHE_VERSION: u32 = 1;
pub const HEAD_CHUNK: usize = 64 * 1024;
pub const TAIL_CHUNK: usize = 256 * 1024;
const CODEX_TITLE_FALLBACK_CHUNK: usize = 1024 * 1024;
/// 详情侧滑抽屉专用 tail 大小：比 scan 用的 TAIL_CHUNK 多一倍，让"最近 5 轮"更容易凑齐
/// （assistant 单次回复常见 20-100KB，5 轮 = 至少 100KB 用户消息 + 200-500KB assistant，256KB 常不够）
pub const DETAIL_TAIL_CHUNK: usize = 512 * 1024;

// 从文件路径反推项目名时只认 F: 盘符下的一级目录（对拍 Python `_pick_project` 实际使用列表）
const PROJECT_ROOTS: &[&str] = &["F:/", "/f/", "/mnt/f/"];

// v4.2 补丁：Mac / Linux 上项目一般放 $HOME/Documents/ 下，运行时算前缀（避免硬编码用户名）
fn extra_project_roots() -> &'static [String] {
    static ROOTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut v = Vec::new();
        if let Some(h) = dirs::home_dir() {
            let docs = h.join("Documents");
            if let Some(s) = docs.to_str() {
                let mut s = s.replace('\\', "/");
                if !s.ends_with('/') {
                    s.push('/');
                }
                v.push(s);
            }
        }
        v
    })
}

const NOISE_PREFIXES: &[&str] = &[
    "<local-command",
    "<command-name",
    "<system-reminder",
    "[Request interrupted",
    "Caveat:",
    "<task-notification",
];

const CODEX_NOISE_PREFIXES: &[&str] = &[
    "<environment_context",
    "<codex_delegation",
    "<recommended_plugins",
    "<app-context",
    "<permissions instructions",
    "<collaboration_mode",
    "<apps_instructions",
    "<plugins_instructions",
    "<skills_instructions",
    "# AGENTS.md instructions",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub path: String,
    pub mtime: f64,
    pub size: u64,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub git_branch: String,
    #[serde(default)]
    pub haystack: String,
    #[serde(default)]
    pub projects: String,
    /// 来源标识："claude-cli"（Claude Code CLI）或 "codex"。
    /// 给现有缓存 JSON 用 `#[serde(default)]` 兼容老数据（回落到空串）。
    #[serde(default)]
    pub source: String,
    /// 收藏标记：true 表示已收藏，排序时优先展示。
    #[serde(default)]
    pub favorited: bool,
}

impl SessionMeta {
    pub fn age(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let sec = now - self.mtime;
        if sec < 3600.0 {
            format!("{}m前", (sec / 60.0) as u64)
        } else if sec < 86400.0 {
            format!("{}h前", (sec / 3600.0) as u64)
        } else {
            format!("{}d前", (sec / 86400.0) as u64)
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionDetail {
    pub preview: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexCacheEntry {
    version: u32,
    meta: SessionMeta,
}

pub fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

pub fn cache_dir() -> PathBuf {
    crate::session_storage::default_storage().cache_dir(CLAUDE_SOURCE_ID)
}

pub fn codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
}

pub fn codex_cache_dir() -> PathBuf {
    crate::session_storage::default_storage().cache_dir(CODEX_SOURCE_ID)
}

/// pi 会话根：`~/.pi/agent/sessions/<cwd slug>/<时间戳>_<uuid>.jsonl`（两层，与 Claude 同构）。
pub fn pi_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi")
        .join("agent")
        .join("sessions")
}

pub fn pi_cache_dir() -> PathBuf {
    crate::session_storage::default_storage().cache_dir(PI_SOURCE_ID)
}

/// grok 会话根：`~/.grok/sessions/<URL 编码 cwd>/<uuid>/`——**一个会话一个目录**，
/// 元数据在 `summary.json`、正文在 `chat_history.jsonl`，与其余三源的单文件形态都不同。
pub fn grok_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("sessions")
}

pub fn grok_cache_dir() -> PathBuf {
    crate::session_storage::default_storage().cache_dir(GROK_SOURCE_ID)
}

// 从绝对路径反推项目名：F:\project-beta\src\... → 'project-beta'
fn pick_project(fp: &str, out: &mut Vec<String>) {
    if fp.is_empty() {
        return;
    }
    let norm = fp.replace('\\', "/");
    let norm_lower = norm.to_lowercase();
    for root in PROJECT_ROOTS {
        let root_lower = root.to_lowercase();
        if norm_lower.starts_with(&root_lower) {
            let rest = &norm[root.len()..];
            let name = rest.split('/').next().unwrap_or("");
            if !name.is_empty() && name != "tmp" && !out.iter().any(|s| s == name) {
                out.push(name.to_string());
            }
            return;
        }
    }
    // v4.2 补丁：Mac/Linux 上项目一般在 $HOME/Documents/ 下（extra_project_roots 运行时算）
    for root in extra_project_roots() {
        let root_lower = root.to_lowercase();
        if norm_lower.starts_with(&root_lower) {
            let rest = &norm[root.len()..];
            let name = rest.split('/').next().unwrap_or("");
            if !name.is_empty() && name != "tmp" && !out.iter().any(|s| s == name) {
                out.push(name.to_string());
            }
            return;
        }
    }
}

// 粘性合并项目归属：本次尾块新扫到的（fresh）在前，再补上次缓存已识别、但本次因
// TAIL_CHUNK 窗口向后滚动没扫到的（prev），去重、只增不丢，最多 max 个。项目归属是
// 会话的稳定属性，长会话每次同步不该因尾块滚动把已识别的项目清空（否则表现为“同步后
// 项目没了”）。
fn merge_sticky_projects(fresh: &[String], prev: &[String], max: usize) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for p in fresh.iter().chain(prev.iter()) {
        if !p.is_empty() && !merged.iter().any(|s| s == p) {
            merged.push(p.clone());
        }
    }
    merged.truncate(max);
    merged
}

// 逐行防御式解析：坏行（含正在写入的截断尾行）直接跳过。
fn iter_json_lines(raw: &[u8]) -> impl Iterator<Item = serde_json::Value> + '_ {
    raw.split(|&b| b == b'\n').filter_map(|line| {
        let line = line.trim_ascii();
        if line.is_empty() {
            return None;
        }
        serde_json::from_slice::<serde_json::Value>(line).ok()
    })
}

fn read_head(path: &Path, size: usize) -> Vec<u8> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut buf = Vec::with_capacity(size);
    let _ = f.take(size as u64).read_to_end(&mut buf);
    buf
}

fn read_tail(path: &Path, size: usize) -> Vec<u8> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let total = match f.seek(SeekFrom::End(0)) {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let start = total.saturating_sub(size as u64);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity(size);
    let _ = f.take(size as u64).read_to_end(&mut buf);
    buf
}

// 从 user/assistant 行抽纯文本；content 为数组时只拼 text block；滤系统噪音
fn msg_text(obj: &serde_json::Value) -> String {
    let content = obj.get("message").and_then(|m| m.get("content"));
    let text = match content {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|b| {
                    if !b.is_object() {
                        return None;
                    }
                    if b.get("type").and_then(|v| v.as_str()) != Some("text") {
                        return None;
                    }
                    b.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .collect();
            parts.join(" ")
        }
        _ => return String::new(),
    };
    for pre in NOISE_PREFIXES {
        if text.starts_with(pre) {
            return String::new();
        }
    }
    text
}

// Python 用字符切片 text[:N]；Rust 按 char 切避免 UTF-8 边界 panic 且对拍字符数
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    let skip = total.saturating_sub(max_chars);
    s.chars().skip(skip).collect()
}

fn snapshot_pick_paths(snap: &serde_json::Value, out: &mut Vec<String>) {
    for k in ["trackedFileBackups", "files", "trackedFiles"] {
        let v = snap.get(k);
        match v {
            Some(serde_json::Value::Object(map)) => {
                for p in map.keys() {
                    pick_project(p, out);
                }
            }
            Some(serde_json::Value::Array(arr)) => {
                for p in arr {
                    if let Some(s) = p.as_str() {
                        pick_project(s, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn snapshot_all_paths(snap: &serde_json::Value, out: &mut Vec<String>) {
    // 有序去重（Python 用 dict[str, None] 保插入序）
    for k in ["trackedFileBackups", "files", "trackedFiles"] {
        let v = snap.get(k);
        match v {
            Some(serde_json::Value::Object(map)) => {
                for p in map.keys() {
                    if !out.iter().any(|s| s == p) {
                        out.push(p.clone());
                    }
                }
            }
            Some(serde_json::Value::Array(arr)) => {
                for p in arr {
                    if let Some(s) = p.as_str() {
                        let s = s.to_string();
                        if !out.iter().any(|x| x == &s) {
                            out.push(s);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// 阶段一（轻）：stat + 头/尾块抽 cwd 与末条 ai-title。带缓存。
/// 默认扫 `~/.claude/projects` + 缓存到 `~/.project-dashboard/sessions/cache/claude-cli`。
pub fn scan() -> Vec<SessionMeta> {
    scan_at(&claude_projects_dir(), &cache_dir())
}

/// scan() 的可注入版本：用于 fixture 测试 / 自定义部署。
pub fn scan_at(projects_dir: &Path, cache: &Path) -> Vec<SessionMeta> {
    let _ = std::fs::create_dir_all(cache);
    let mut metas: Vec<SessionMeta> = Vec::new();
    let top_entries = match std::fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return metas,
    };
    for proj_entry in top_entries.flatten() {
        let proj_path = proj_entry.path();
        // 过滤 claude-mem observer sessions：目录名是 cwd slug（/→-），命中
        // 同时包含 `claude-mem` + `observer-sessions` 的项目直接跳过。
        // 场景：claude-mem 插件跑 observer 子进程时会在 ~/.claude-mem/observer-sessions
        // 下产生大量 claude session，噪音淹没 Session Board。
        if let Some(dir_name) = proj_path.file_name().and_then(|n| n.to_str()) {
            if dir_name.contains("claude-mem") && dir_name.contains("observer-sessions") {
                continue;
            }
        }
        let sub_entries = match std::fs::read_dir(&proj_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for jsonl_entry in sub_entries.flatten() {
            let jsonl = jsonl_entry.path();
            if jsonl.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let st = match jsonl.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !st.is_file() {
                continue;
            }
            let mtime = st
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let size = st.len();
            let sid = jsonl
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let cache_file = cache.join(format!("{}.json", sid));

            // 缓存命中：mtime + size 一致直接复用；未命中（文件已增长）时暂存上次已识别
            // 的项目，供重扫后粘性合并——见 merge_sticky_projects。
            let mut prev_projects: Vec<String> = Vec::new();
            if cache_file.is_file() {
                if let Ok(txt) = std::fs::read_to_string(&cache_file) {
                    if let Ok(mut c) = serde_json::from_str::<SessionMeta>(&txt) {
                        // mtime 以 f64 秒存储：在 ~1.7e9 秒量级 f64 单个 ULP 已≈2.4e-7 秒，
                        // 且 APFS 对刚落盘文件的 mtime 有亚微秒级 settle 抖动（同一未改文件先后
                        // 两次 stat 的纳秒可差数百 ns）。原 1e-9 容差比单个 ULP 还小，会把未改文件
                        // 偶发误判为已改、缓存 miss。放宽到 1μs：远大于 ULP 与 settle 抖动、又远小
                        // 于任何真实文件修改间隔；size 精确比对作第二道防线兜底真实变更。
                        if (c.mtime - mtime).abs() < 1e-6 && c.size == size {
                            // v1 分页 change 加了 source 字段：老 cache 反序列化后为空，backfill
                            if c.source.is_empty() {
                                c.source = "claude-cli".to_string();
                            }
                            metas.push(c);
                            continue;
                        }
                        // 缓存 miss：留住上次已识别的项目名，供本次重扫后粘性合并
                        if !c.projects.is_empty() {
                            prev_projects =
                                c.projects.split(',').map(|s| s.to_string()).collect();
                        }
                    }
                }
            }

            let mut meta = SessionMeta {
                session_id: sid.clone(),
                path: jsonl.to_string_lossy().replace('/', std::path::MAIN_SEPARATOR_STR),
                mtime,
                size,
                source: "claude-cli".to_string(),
                ..Default::default()
            };

            // 头块：找第一个 cwd（含 gitBranch）即 break
            let head = read_head(&jsonl, HEAD_CHUNK);
            for obj in iter_json_lines(&head) {
                if meta.cwd.is_empty() {
                    if let Some(c) = obj.get("cwd").and_then(|v| v.as_str()) {
                        if !c.is_empty() {
                            meta.cwd = c.to_string();
                            meta.git_branch = obj
                                .get("gitBranch")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            break;
                        }
                    }
                }
            }

            // 尾块：ai-title 末条 / cwd 兜底 / haystack / project 名反推
            let tail = read_tail(&jsonl, TAIL_CHUNK);
            let mut hay_parts: Vec<String> = Vec::new();
            let mut proj_names: Vec<String> = Vec::new();
            for obj in iter_json_lines(&tail) {
                let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if t == "ai-title" {
                    if let Some(at) = obj.get("aiTitle").and_then(|v| v.as_str()) {
                        if !at.is_empty() {
                            meta.title = at.to_string(); // 不 break：取末条
                        }
                    }
                }
                if meta.cwd.is_empty() {
                    if let Some(c) = obj.get("cwd").and_then(|v| v.as_str()) {
                        if !c.is_empty() {
                            meta.cwd = c.to_string();
                        }
                    }
                }
                if t == "user" || t == "assistant" {
                    let text = msg_text(&obj);
                    if !text.is_empty() {
                        hay_parts.push(truncate_chars(&text, 400));
                    }
                }
                // 从工具调用里的 file_path 反推项目名
                if let Some(serde_json::Value::Array(arr)) =
                    obj.get("message").and_then(|m| m.get("content"))
                {
                    for b in arr {
                        if b.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            if let Some(fp) = b
                                .get("input")
                                .and_then(|i| i.get("file_path"))
                                .and_then(|v| v.as_str())
                            {
                                pick_project(fp, &mut proj_names);
                            }
                        }
                    }
                }
                // file-history-snapshot 里的路径也反推
                if t == "file-history-snapshot" {
                    if let Some(snap) = obj.get("snapshot") {
                        snapshot_pick_paths(snap, &mut proj_names);
                    }
                }
            }

            meta.haystack = tail_chars(&hay_parts.join(" "), 8000);
            // 粘性合并：本次尾块新扫到的 + 上次缓存已识别的（去重、只增不丢），最多 6 个
            meta.projects = merge_sticky_projects(&proj_names, &prev_projects, 6).join(",");

            // ai-title 缺失时用首条 user 消息前 30 字兜底（cc 短会话 / 中断会话没自动 title）
            if meta.title.is_empty() {
                meta.title = fallback_title_from_tail(&tail, 30);
            }

            // 写缓存（失败不阻塞）
            if let Ok(txt) = serde_json::to_string(&meta) {
                let _ = std::fs::write(&cache_file, txt);
            }
            metas.push(meta);
        }
    }
    metas.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    metas
}

/// 阶段二（重，选中才跑）：尾块抽对话预览 + 改过的文件。
pub fn detail(meta: &SessionMeta, rounds: usize) -> SessionDetail {
    let mut d = SessionDetail::default();
    let path = PathBuf::from(&meta.path);
    let tail = read_tail(&path, DETAIL_TAIL_CHUNK);
    let mut dialog: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();

    for obj in iter_json_lines(&tail) {
        let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "user" || t == "assistant" {
            let text = msg_text(&obj);
            if !text.is_empty() {
                let who = if t == "user" { "🧑" } else { "🤖" };
                dialog.push(format!("{} {}", who, truncate_chars(&text, 200)));
            }
        } else if t == "file-history-snapshot" {
            // Python: snap = obj.get("snapshot") or obj —— 无 snapshot 键则用 obj 本身
            let snap = obj.get("snapshot").unwrap_or(&obj).clone();
            snapshot_all_paths(&snap, &mut files);
        }
    }

    // 兜底：tool_use Edit/Write/NotebookEdit 的 file_path
    if files.is_empty() {
        for obj in iter_json_lines(&tail) {
            let content = obj.get("message").and_then(|m| m.get("content"));
            let arr = match content {
                Some(serde_json::Value::Array(a)) => a,
                _ => continue,
            };
            for b in arr {
                if b.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                    continue;
                }
                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !matches!(name, "Edit" | "Write" | "NotebookEdit") {
                    continue;
                }
                if let Some(fp) = b
                    .get("input")
                    .and_then(|i| i.get("file_path"))
                    .and_then(|v| v.as_str())
                {
                    if !fp.is_empty() && !files.iter().any(|s| s == fp) {
                        files.push(fp.to_string());
                    }
                }
            }
        }
    }

    // rounds 语义：1 轮 = 1 user + 1 assistant。倒序数 user 消息，凑齐 N 个 user 停。
    // 不足 N 轮时返回全部；user/assistant 前面孤立的 assistant 也保留（对话上下文）。
    let mut kept_rev: Vec<String> = Vec::new();
    let mut user_seen = 0usize;
    for line in dialog.iter().rev() {
        kept_rev.push(line.clone());
        if line.starts_with("🧑") {
            user_seen += 1;
            if user_seen >= rounds {
                break;
            }
        }
    }
    kept_rev.reverse();
    d.preview = kept_rev;
    let files_start = files.len().saturating_sub(20);
    d.files = files[files_start..].to_vec();
    d
}

/// 从 tail 里找首条 user 消息前 N 字符做 title 兜底（ai-title 缺失场景）。
/// 返回空字符串表示无兜底（tail 里也没 user 消息）。
pub fn fallback_title_from_tail(tail: &[u8], max_chars: usize) -> String {
    for obj in iter_json_lines(tail) {
        let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "user" {
            let text = msg_text(&obj);
            if !text.is_empty() {
                return truncate_chars(&text, max_chars);
            }
        }
    }
    String::new()
}

// -------- SessionSource 抽象层（Q3 留口子：v2+ 加 opencode / codex 等多源汇总）--------

/// 会话数据源抽象。v1 只有 ClaudeCliSource；v2+ 可加 OpencodeSource / CodexSource 等，
/// `list_paged` 内部收集所有 source 的 scan 结果统一排序分页。
pub trait SessionSource {
    /// 稳定 source 标识（前端渲染 / 未来筛选按 source 分组）。
    fn source_id(&self) -> &'static str;
    /// 全量扫描本 source 的会话 meta。返回值里每条 meta.source 应等于 self.source_id()。
    fn scan(&self) -> Vec<SessionMeta>;
}

/// v1 默认 source：读 `~/.claude/projects/**/*.jsonl`（Claude Code CLI 落盘位置）+ 用本地 cache。
pub struct ClaudeCliSource {
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl ClaudeCliSource {
    /// 走默认路径：`~/.claude/projects` + `~/.project-dashboard/sessions/cache/claude-cli`。
    pub fn default_paths() -> Self {
        Self {
            root: claude_projects_dir(),
            cache: cache_dir(),
        }
    }
}

impl SessionSource for ClaudeCliSource {
    fn source_id(&self) -> &'static str {
        CLAUDE_SOURCE_ID
    }
    fn scan(&self) -> Vec<SessionMeta> {
        scan_at(&self.root, &self.cache)
    }
}

/// Codex 活动会话 source：递归只读 `~/.codex/sessions/**/*.jsonl`。
///
/// 归档会话不在本 source 内：Codex CLI 对归档会话要求先显式 unarchive，不能把它们
/// 混进可直接 `codex resume <id>` 的列表。
pub struct CodexSessionSource {
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl CodexSessionSource {
    pub fn default_paths() -> Self {
        Self {
            root: codex_sessions_dir(),
            cache: codex_cache_dir(),
        }
    }

    pub fn find_by_id(&self, session_id: &str) -> Option<SessionMeta> {
        codex_session_by_id_at(&self.root, &self.cache, session_id)
    }
}

impl SessionSource for CodexSessionSource {
    fn source_id(&self) -> &'static str {
        CODEX_SOURCE_ID
    }

    fn scan(&self) -> Vec<SessionMeta> {
        scan_codex_at(&self.root, &self.cache)
    }
}

fn codex_payload_type(obj: &serde_json::Value) -> &str {
    obj.get("payload")
        .and_then(|payload| payload.get("type"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn codex_clean_text(text: &str) -> String {
    let text = text.trim();
    if text.is_empty()
        || CODEX_NOISE_PREFIXES
            .iter()
            .any(|prefix| text.starts_with(prefix))
    {
        String::new()
    } else {
        text.to_string()
    }
}

fn codex_message_text(obj: &serde_json::Value) -> String {
    let text = obj
        .get("payload")
        .and_then(|payload| payload.get("message"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    codex_clean_text(text)
}

fn codex_response_message(obj: &serde_json::Value) -> Option<(&str, String)> {
    if obj.get("type").and_then(|value| value.as_str()) != Some("response_item") {
        return None;
    }
    let payload = obj.get("payload")?;
    if payload.get("type").and_then(|value| value.as_str()) != Some("message") {
        return None;
    }
    let role = payload.get("role").and_then(|value| value.as_str())?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let text = match payload.get("content") {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                let block_type = block.get("type").and_then(|value| value.as_str())?;
                if !matches!(block_type, "input_text" | "output_text" | "text") {
                    return None;
                }
                block
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let text = codex_clean_text(&text);
    if text.is_empty() {
        None
    } else {
        Some((role, text))
    }
}

fn codex_session_meta_is_subagent(obj: &serde_json::Value) -> bool {
    if obj.get("type").and_then(|value| value.as_str()) != Some("session_meta") {
        return false;
    }
    let source = obj.get("payload").and_then(|payload| payload.get("source"));
    source.and_then(|value| value.as_str()) == Some("subagent")
        || source
            .and_then(|value| value.as_object())
            .is_some_and(|value| value.contains_key("subagent"))
}

fn codex_session_id_from_filename(path: &Path) -> String {
    let stem = match path.file_stem().and_then(|value| value.to_str()) {
        Some(value) => value,
        None => return String::new(),
    };
    let suffix: String = stem
        .chars()
        .rev()
        .take(36)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if uuid::Uuid::parse_str(&suffix).is_ok() {
        suffix
    } else {
        String::new()
    }
}

fn codex_valid_session_id(value: &str) -> Option<String> {
    uuid::Uuid::parse_str(value)
        .ok()
        .map(|parsed| parsed.to_string())
}

fn codex_pick_change_paths(obj: &serde_json::Value, out: &mut Vec<String>) {
    if obj.get("type").and_then(|value| value.as_str()) != Some("event_msg")
        || codex_payload_type(obj) != "patch_apply_end"
    {
        return;
    }
    let Some(changes) = obj
        .get("payload")
        .and_then(|payload| payload.get("changes"))
        .and_then(|value| value.as_object())
    else {
        return;
    };
    for (path, change) in changes {
        if !path.is_empty() && !out.iter().any(|value| value == path) {
            out.push(path.clone());
        }
        if let Some(move_path) = change
            .get("move_path")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        {
            if !out.iter().any(|value| value == move_path) {
                out.push(move_path.to_string());
            }
        }
    }
}

fn codex_parse_session_meta(obj: &serde_json::Value, meta: &mut SessionMeta) {
    if obj.get("type").and_then(|value| value.as_str()) != Some("session_meta") {
        return;
    }
    let Some(payload) = obj.get("payload") else {
        return;
    };
    if meta.session_id.is_empty() {
        meta.session_id = payload
            .get("id")
            .or_else(|| payload.get("session_id"))
            .and_then(|value| value.as_str())
            .and_then(codex_valid_session_id)
            .unwrap_or_default();
    }
    if meta.cwd.is_empty() {
        meta.cwd = payload
            .get("cwd")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
    }
    if meta.git_branch.is_empty() {
        meta.git_branch = payload
            .get("git")
            .and_then(|git| git.get("branch"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
    }
}

fn codex_parse_turn_context_cwd(obj: &serde_json::Value, meta: &mut SessionMeta) {
    if meta.cwd.is_empty()
        && obj.get("type").and_then(|value| value.as_str()) == Some("turn_context")
    {
        meta.cwd = obj
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
    }
}

/// Codex 列表阶段：递归枚举活动会话，单文件只读 stat + 头 64KB + 尾 256KB。
pub fn scan_codex_at(root: &Path, cache: &Path) -> Vec<SessionMeta> {
    let _ = std::fs::create_dir_all(cache);
    if !root.is_dir() {
        return Vec::new();
    }

    let mut by_session_id: HashMap<String, SessionMeta> = HashMap::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let jsonl = entry.path();
        if !entry.file_type().is_file()
            || jsonl.extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            continue;
        }
        let stat = match jsonl.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };
        let mtime = stat
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs_f64())
            .unwrap_or(0.0);
        let size = stat.len();

        let head = read_head(jsonl, HEAD_CHUNK);
        let mut is_subagent = false;
        let mut response_title = String::new();
        let mut meta = SessionMeta {
            path: jsonl.to_string_lossy().to_string(),
            mtime,
            size,
            source: CODEX_SOURCE_ID.to_string(),
            ..Default::default()
        };
        for obj in iter_json_lines(&head) {
            is_subagent |= codex_session_meta_is_subagent(&obj);
            codex_parse_session_meta(&obj, &mut meta);
            codex_parse_turn_context_cwd(&obj, &mut meta);
            if meta.title.is_empty()
                && obj.get("type").and_then(|value| value.as_str()) == Some("event_msg")
                && codex_payload_type(&obj) == "user_message"
            {
                let text = codex_message_text(&obj);
                if !text.is_empty() {
                    meta.title = truncate_chars(&text, 30);
                }
            }
            if response_title.is_empty() {
                if let Some(("user", text)) = codex_response_message(&obj) {
                    response_title = truncate_chars(&text, 30);
                }
            }
        }
        if is_subagent {
            continue;
        }
        if meta.session_id.is_empty() {
            meta.session_id = codex_session_id_from_filename(jsonl);
        }
        if meta.session_id.is_empty() {
            continue;
        }

        let cache_file = cache.join(format!("{}.json", meta.session_id));
        let mut previous_title = String::new();
        let mut previous_projects = Vec::new();
        if let Ok(raw) = std::fs::read_to_string(&cache_file) {
            if let Ok(mut cached) = serde_json::from_str::<CodexCacheEntry>(&raw) {
                if cached.version == CODEX_CACHE_VERSION
                    && (cached.meta.mtime - mtime).abs() < 1e-6
                    && cached.meta.size == size
                {
                    cached.meta.source = CODEX_SOURCE_ID.to_string();
                    match by_session_id.get(&cached.meta.session_id) {
                        Some(existing) if existing.mtime >= cached.meta.mtime => {}
                        _ => {
                            by_session_id.insert(cached.meta.session_id.clone(), cached.meta);
                        }
                    }
                    continue;
                }
                previous_title = cached.meta.title;
                previous_projects = cached
                    .meta
                    .projects
                    .split(',')
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }

        let tail = read_tail(jsonl, TAIL_CHUNK);
        let mut event_haystack_parts = Vec::new();
        let mut response_haystack_parts = Vec::new();
        let mut change_haystack_parts = Vec::new();
        let mut projects = Vec::new();
        if !meta.cwd.is_empty() {
            pick_project(&meta.cwd, &mut projects);
        }
        for obj in iter_json_lines(&tail) {
            is_subagent |= codex_session_meta_is_subagent(&obj);
            codex_parse_session_meta(&obj, &mut meta);
            codex_parse_turn_context_cwd(&obj, &mut meta);
            if obj.get("type").and_then(|value| value.as_str()) == Some("event_msg") {
                let event_type = codex_payload_type(&obj);
                if matches!(event_type, "user_message" | "agent_message") {
                    let text = codex_message_text(&obj);
                    if !text.is_empty() {
                        if meta.title.is_empty() && event_type == "user_message" {
                            meta.title = truncate_chars(&text, 30);
                        }
                        event_haystack_parts.push(truncate_chars(&text, 400));
                    }
                }
            }
            if let Some((role, text)) = codex_response_message(&obj) {
                if response_title.is_empty() && role == "user" {
                    response_title = truncate_chars(&text, 30);
                }
                response_haystack_parts.push(truncate_chars(&text, 400));
            }
            let mut change_paths = Vec::new();
            codex_pick_change_paths(&obj, &mut change_paths);
            for path in change_paths {
                change_haystack_parts.push(truncate_chars(&path, 400));
                pick_project(&path, &mut projects);
            }
        }
        if is_subagent {
            continue;
        }
        if meta.title.is_empty() && response_title.is_empty() && previous_title.is_empty() {
            let prefix = read_head(jsonl, CODEX_TITLE_FALLBACK_CHUNK);
            for obj in iter_json_lines(&prefix) {
                if obj.get("type").and_then(|value| value.as_str()) == Some("event_msg")
                    && codex_payload_type(&obj) == "user_message"
                {
                    let text = codex_message_text(&obj);
                    if !text.is_empty() {
                        meta.title = truncate_chars(&text, 30);
                        break;
                    }
                }
                if response_title.is_empty() {
                    if let Some(("user", text)) = codex_response_message(&obj) {
                        response_title = truncate_chars(&text, 30);
                    }
                }
            }
        }
        if meta.title.is_empty() {
            meta.title = if response_title.is_empty() {
                previous_title
            } else {
                response_title
            };
        }
        if projects.is_empty() && !meta.cwd.is_empty() {
            if let Some(name) = Path::new(&meta.cwd)
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty() && *value != "tmp")
            {
                projects.push(name.to_string());
            }
        }
        let mut haystack_parts = if event_haystack_parts.is_empty() {
            response_haystack_parts
        } else {
            event_haystack_parts
        };
        haystack_parts.extend(change_haystack_parts);
        meta.haystack = tail_chars(&haystack_parts.join(" "), 8000);
        meta.projects = merge_sticky_projects(&projects, &previous_projects, 6).join(",");

        let cache_entry = CodexCacheEntry {
            version: CODEX_CACHE_VERSION,
            meta: meta.clone(),
        };
        if let Ok(raw) = serde_json::to_string(&cache_entry) {
            let _ = std::fs::write(&cache_file, raw);
        }
        match by_session_id.get(&meta.session_id) {
            Some(existing) if existing.mtime >= meta.mtime => {}
            _ => {
                by_session_id.insert(meta.session_id.clone(), meta);
            }
        }
    }

    let mut metas: Vec<SessionMeta> = by_session_id.into_values().collect();
    metas.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    metas
}

/// 按 SID 读取 Codex meta：优先 O(1) 校验本地缓存，缓存失效才回退扫描。
///
/// 缓存里的源路径必须 canonicalize 后仍位于注入的 sessions root 内，避免被篡改缓存
/// 引导详情读取任意本机文件；SID 必须是 UUID，不能参与任意路径拼接。
pub fn codex_session_by_id_at(
    root: &Path,
    cache: &Path,
    session_id: &str,
) -> Option<SessionMeta> {
    let session_id = codex_valid_session_id(session_id)?;
    let cache_file = cache.join(format!("{session_id}.json"));
    if let Ok(raw) = std::fs::read_to_string(cache_file) {
        if let Ok(mut cached) = serde_json::from_str::<CodexCacheEntry>(&raw) {
            let source_path = PathBuf::from(&cached.meta.path);
            let root_canonical = root.canonicalize().ok();
            let source_canonical = source_path.canonicalize().ok();
            let path_is_allowed = matches!(
                (&root_canonical, &source_canonical),
                (Some(root), Some(source)) if source.starts_with(root)
            );
            let stat = source_path.metadata().ok();
            let stat_mtime = stat
                .as_ref()
                .and_then(|value| value.modified().ok())
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64());
            let stat_matches = stat.as_ref().is_some_and(|value| {
                value.is_file()
                    && value.len() == cached.meta.size
                    && stat_mtime.is_some_and(|mtime| (cached.meta.mtime - mtime).abs() < 1e-6)
            });
            if cached.version == CODEX_CACHE_VERSION
                && cached.meta.session_id == session_id
                && path_is_allowed
                && source_path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && stat_matches
            {
                cached.meta.source = CODEX_SOURCE_ID.to_string();
                return Some(cached.meta);
            }
        }
    }
    scan_codex_at(root, cache)
        .into_iter()
        .find(|meta| meta.session_id == session_id)
}

/// Codex 详情阶段：只读尾 512KB，抽最近若干轮消息与 patch_apply_end 改动路径。
pub fn codex_detail(meta: &SessionMeta, rounds: usize) -> SessionDetail {
    let tail = read_tail(Path::new(&meta.path), DETAIL_TAIL_CHUNK);
    let mut event_dialog = Vec::new();
    let mut response_dialog = Vec::new();
    let mut files = Vec::new();
    for obj in iter_json_lines(&tail) {
        if obj.get("type").and_then(|value| value.as_str()) == Some("event_msg") {
            let event_type = codex_payload_type(&obj);
            if matches!(event_type, "user_message" | "agent_message") {
                let text = codex_message_text(&obj);
                if !text.is_empty() {
                    let who = if event_type == "user_message" {
                        "🧑"
                    } else {
                        "🤖"
                    };
                    event_dialog.push(format!("{who} {}", truncate_chars(&text, 200)));
                }
            }
        }
        if let Some((role, text)) = codex_response_message(&obj) {
            let who = if role == "user" { "🧑" } else { "🤖" };
            response_dialog.push(format!("{who} {}", truncate_chars(&text, 200)));
        }
        codex_pick_change_paths(&obj, &mut files);
    }

    let dialog = if event_dialog.is_empty() {
        response_dialog
    } else {
        event_dialog
    };
    let mut kept_rev = Vec::new();
    let mut user_seen = 0usize;
    for line in dialog.iter().rev() {
        kept_rev.push(line.clone());
        if line.starts_with("🧑") {
            user_seen += 1;
            if user_seen >= rounds {
                break;
            }
        }
    }
    kept_rev.reverse();
    let files_start = files.len().saturating_sub(20);
    SessionDetail {
        preview: kept_rev,
        files: files[files_start..].to_vec(),
    }
}

// -------- pi / grok 共用小工具 --------

/// pi / grok 的缓存条目。与 CodexCacheEntry 同形，但各来源独立版本号，
/// 格式演进时只失效自己那一份缓存。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceCacheEntry {
    version: u32,
    /// 来源自带的内容指纹，命中缓存时必须精确相等。
    ///
    /// grok 存 `summary.json` 的 `updated_at` 原文：它的 mtime 是从这个字符串换算的 f64 秒，
    /// 在 1.7e9 量级单个 ULP 已≈2.4e-7 秒，微秒级的 `updated_at` 变化会被 1e-6 容差整个吃掉
    /// （实测 1μs 差在 f64 里只剩 9.5e-7）。存原文做字符串比对才不漏。
    /// pi 的 mtime 直接取自被解析的那个文件，不需要额外指纹，留空。
    #[serde(default)]
    fingerprint: String,
    meta: SessionMeta,
}

/// UUID 形式校验：函数本身与来源无关，pi / grok 复用 codex 的实现，避免同一段逻辑写三遍。
fn valid_uuid_id(value: &str) -> Option<String> {
    codex_valid_session_id(value)
}

/// 同一个 session_id 只保留 mtime 最新的那条。
///
/// 一个 SID 对应多个文件是有可能的（例如手工 cp 备份一份会话文件），此时两条记录会
/// 共用同一个缓存文件互相覆盖、列表里也会出现重复行。codex 早就用同款 HashMap 去重。
fn keep_newest(by_session_id: &mut HashMap<String, SessionMeta>, meta: SessionMeta) {
    match by_session_id.get(&meta.session_id) {
        Some(existing) if existing.mtime >= meta.mtime => {}
        _ => {
            by_session_id.insert(meta.session_id.clone(), meta);
        }
    }
}

/// 缓存里记的源文件路径必须 canonicalize 后仍落在来源根目录内，且确实是 jsonl。
///
/// 缓存是本机可写文件：被篡改过的 `meta.path` 会让详情阶段照着读任意本机文件
/// （`pi_detail` / `grok_detail` 只认 `meta.path`，不重新校验）。codex 的
/// `codex_session_by_id_at` 早有同款 containment 校验，pi / grok 必须对齐。
fn cached_path_within_root(root: &Path, path: &str) -> bool {
    let source = PathBuf::from(path);
    if source.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return false;
    }
    match (root.canonicalize(), source.canonicalize()) {
        (Ok(root), Ok(source)) => source.starts_with(root),
        _ => false,
    }
}

/// 目录项必须是**真实**的文件 / 目录，symlink 一律不认。
///
/// `metadata()` / `is_dir()` 都会跟随 symlink：会话目录里放一个文件名带合法 UUID 的
/// symlink（或 grok 那边整个会话目录是 symlink），扫描阶段就能把来源根目录之外的内容
/// 当成正常会话收进来——这条路径根本不经过缓存，`cached_path_within_root` 拦不住
/// （实测能读出外部文件正文）。codex 源用 `WalkDir::follow_links(false)` 防的就是这个。
fn real_entry_metadata(path: &Path) -> Option<std::fs::Metadata> {
    path.symlink_metadata().ok()
}

// -------- pi 来源 --------

/// pi 的 role 在 `message.role`（顶层 `type` 恒为 `"message"`），跟 Claude 的顶层 type 不同。
/// 只认 user / assistant：pi 还有 `toolResult` 角色，那是工具回显不是对话。
fn pi_message_role(obj: &serde_json::Value) -> Option<&str> {
    if obj.get("type").and_then(|value| value.as_str()) != Some("message") {
        return None;
    }
    let role = obj.get("message")?.get("role")?.as_str()?;
    if matches!(role, "user" | "assistant") {
        Some(role)
    } else {
        None
    }
}

/// 从 pi 的 toolCall 块里取被改动的文件路径。
/// 实测（2026-09-09 真实会话）pi 只有 bash / read / edit / write 四种工具，
/// 其中改文件的是 edit / write，参数键统一叫 `path`。
fn pi_pick_changed_paths(obj: &serde_json::Value, out: &mut Vec<String>) {
    let Some(serde_json::Value::Array(blocks)) =
        obj.get("message").and_then(|message| message.get("content"))
    else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(|value| value.as_str()) != Some("toolCall") {
            continue;
        }
        let name = block.get("name").and_then(|value| value.as_str()).unwrap_or("");
        if !matches!(name, "edit" | "write") {
            continue;
        }
        if let Some(path) = block
            .get("arguments")
            .and_then(|args| args.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        {
            if !out.iter().any(|value| value == path) {
                out.push(path.to_string());
            }
        }
    }
}

/// pi 列表阶段：扫 `<root>/<cwd slug>/*.jsonl`，单文件只读 stat + 头 64KB + 尾 256KB。
///
/// session_id 优先取文件名尾部的 UUID，取不到才回落到首行 `session` 记录里的 `id`；
/// 两条路径都过 `valid_uuid_id` 归一化，保证列表给出的 id 能原样查回详情。
pub fn scan_pi_at(root: &Path, cache: &Path) -> Vec<SessionMeta> {
    let _ = std::fs::create_dir_all(cache);
    if !root.is_dir() {
        return Vec::new();
    }
    let mut by_session_id: HashMap<String, SessionMeta> = HashMap::new();
    let top_entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    for cwd_entry in top_entries.flatten() {
        let cwd_dir = cwd_entry.path();
        // symlink 一律不认，见 real_entry_metadata
        if !real_entry_metadata(&cwd_dir).is_some_and(|stat| stat.is_dir()) {
            continue;
        }
        let files = match std::fs::read_dir(&cwd_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for file_entry in files.flatten() {
            let jsonl = file_entry.path();
            if jsonl.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let stat = match real_entry_metadata(&jsonl) {
                Some(value) => value,
                None => continue,
            };
            // symlink 的 symlink_metadata().is_file() 为 false，这里同时挡住 symlink 与目录
            if !stat.is_file() {
                continue;
            }
            let mtime = stat
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let size = stat.len();

            // 必须过 valid_uuid_id 归一化：文件名尾缀取的是原始大小写，而 find_by_id
            // 查的是归一化后的小写。不对称会让大写 UUID 文件的详情静默查不到（Ok(None)）。
            let mut session_id =
                valid_uuid_id(&codex_session_id_from_filename(&jsonl)).unwrap_or_default();
            let head = read_head(&jsonl, HEAD_CHUNK);
            let mut meta = SessionMeta {
                path: jsonl.to_string_lossy().to_string(),
                mtime,
                size,
                source: PI_SOURCE_ID.to_string(),
                ..Default::default()
            };
            let mut head_title = String::new();
            for obj in iter_json_lines(&head) {
                if obj.get("type").and_then(|value| value.as_str()) == Some("session") {
                    if session_id.is_empty() {
                        session_id = obj
                            .get("id")
                            .and_then(|value| value.as_str())
                            .and_then(valid_uuid_id)
                            .unwrap_or_default();
                    }
                    if meta.cwd.is_empty() {
                        meta.cwd = obj
                            .get("cwd")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                }
                if head_title.is_empty() && pi_message_role(&obj) == Some("user") {
                    let text = msg_text(&obj);
                    if !text.is_empty() {
                        head_title = truncate_chars(&text, 30);
                    }
                }
            }
            if session_id.is_empty() {
                continue;
            }
            meta.session_id = session_id;

            let cache_file = cache.join(format!("{}.json", meta.session_id));
            let mut previous_projects: Vec<String> = Vec::new();
            if let Ok(raw) = std::fs::read_to_string(&cache_file) {
                if let Ok(mut cached) = serde_json::from_str::<SourceCacheEntry>(&raw) {
                    if cached.version == PI_CACHE_VERSION
                        && cached.meta.session_id == meta.session_id
                        && cached_path_within_root(root, &cached.meta.path)
                        && (cached.meta.mtime - mtime).abs() < 1e-6
                        && cached.meta.size == size
                    {
                        cached.meta.source = PI_SOURCE_ID.to_string();
                        keep_newest(&mut by_session_id, cached.meta);
                        continue;
                    }
                    previous_projects = cached
                        .meta
                        .projects
                        .split(',')
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .collect();
                }
            }

            let tail = read_tail(&jsonl, TAIL_CHUNK);
            let mut haystack_parts: Vec<String> = Vec::new();
            let mut projects: Vec<String> = Vec::new();
            let mut tail_title = String::new();
            if !meta.cwd.is_empty() {
                pick_project(&meta.cwd, &mut projects);
            }
            for obj in iter_json_lines(&tail) {
                if meta.cwd.is_empty()
                    && obj.get("type").and_then(|value| value.as_str()) == Some("session")
                {
                    meta.cwd = obj
                        .get("cwd")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                }
                if let Some(role) = pi_message_role(&obj) {
                    let text = msg_text(&obj);
                    if !text.is_empty() {
                        if tail_title.is_empty() && role == "user" {
                            tail_title = truncate_chars(&text, 30);
                        }
                        haystack_parts.push(truncate_chars(&text, 400));
                    }
                }
                let mut changed = Vec::new();
                pi_pick_changed_paths(&obj, &mut changed);
                for path in changed {
                    pick_project(&path, &mut projects);
                }
            }

            meta.title = if head_title.is_empty() {
                tail_title
            } else {
                head_title
            };
            // cwd 不在 PROJECT_ROOTS 覆盖范围内时用目录名兜底，跟 Codex 同策略
            if projects.is_empty() && !meta.cwd.is_empty() {
                if let Some(name) = Path::new(&meta.cwd)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty() && *value != "tmp")
                {
                    projects.push(name.to_string());
                }
            }
            meta.haystack = tail_chars(&haystack_parts.join(" "), 8000);
            meta.projects = merge_sticky_projects(&projects, &previous_projects, 6).join(",");

            let entry = SourceCacheEntry {
                version: PI_CACHE_VERSION,
                fingerprint: String::new(),
                meta: meta.clone(),
            };
            if let Ok(raw) = serde_json::to_string(&entry) {
                let _ = std::fs::write(&cache_file, raw);
            }
            keep_newest(&mut by_session_id, meta);
        }
    }
    let mut metas: Vec<SessionMeta> = by_session_id.into_values().collect();
    // 二级排序必须有：去重用的 HashMap 遍历顺序不确定，只按 mtime 倒排的话，mtime 相同的
    // 几条每次扫描都会换位置（实测 8 轮扫出 8 种顺序），表现为列表刷新时行乱跳。
    metas.sort_by(|a, b| {
        b.mtime
            .partial_cmp(&a.mtime)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    metas
}

/// pi 详情阶段：只读尾 512KB，抽最近若干轮对话与 edit / write 改过的文件。
pub fn pi_detail(meta: &SessionMeta, rounds: usize) -> SessionDetail {
    let tail = read_tail(Path::new(&meta.path), DETAIL_TAIL_CHUNK);
    let mut dialog: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for obj in iter_json_lines(&tail) {
        if let Some(role) = pi_message_role(&obj) {
            let text = msg_text(&obj);
            if !text.is_empty() {
                let who = if role == "user" { "🧑" } else { "🤖" };
                dialog.push(format!("{who} {}", truncate_chars(&text, 200)));
            }
        }
        pi_pick_changed_paths(&obj, &mut files);
    }
    let mut kept_rev: Vec<String> = Vec::new();
    let mut user_seen = 0usize;
    for line in dialog.iter().rev() {
        kept_rev.push(line.clone());
        if line.starts_with("🧑") {
            user_seen += 1;
            if user_seen >= rounds {
                break;
            }
        }
    }
    kept_rev.reverse();
    let files_start = files.len().saturating_sub(20);
    SessionDetail {
        preview: kept_rev,
        files: files[files_start..].to_vec(),
    }
}

/// pi 会话 source：只读 `~/.pi/agent/sessions/<cwd slug>/*.jsonl`。
pub struct PiSessionSource {
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl PiSessionSource {
    pub fn default_paths() -> Self {
        Self {
            root: pi_sessions_dir(),
            cache: pi_cache_dir(),
        }
    }

    /// 按 SID 取 meta：直接走 scan——缓存命中时 scan 每条只做 stat + 读缓存 JSON，
    /// 成本足够低，不再像 codex 那样额外维护一条要做路径越权校验的缓存快路。
    pub fn find_by_id(&self, session_id: &str) -> Option<SessionMeta> {
        let session_id = valid_uuid_id(session_id)?;
        scan_pi_at(&self.root, &self.cache)
            .into_iter()
            .find(|meta| meta.session_id == session_id)
    }
}

impl SessionSource for PiSessionSource {
    fn source_id(&self) -> &'static str {
        PI_SOURCE_ID
    }

    fn scan(&self) -> Vec<SessionMeta> {
        scan_pi_at(&self.root, &self.cache)
    }
}

// -------- grok 来源 --------

const GROK_QUERY_OPEN: &str = "<user_query>";
const GROK_QUERY_CLOSE: &str = "</user_query>";

/// 解析 grok 的 ISO-8601 时间（`summary.json` 里的 `created_at` / `updated_at`）为 epoch 秒。
fn grok_parse_time(value: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.timestamp() as f64 + f64::from(parsed.timestamp_subsec_nanos()) / 1e9)
}

/// 抽 grok 用户消息里的真实提问。
///
/// grok 把每轮用户输入包在 `<user_query>` 标签里，同一个 text 块的其余部分是运行时
/// 注入的环境信息 / 用户规则 / skill 清单（动辄 20KB）。取不到标签就当整块是注入内容
/// 跳过——否则标题和预览会变成一大段系统提示。
fn grok_user_text(obj: &serde_json::Value) -> String {
    let Some(serde_json::Value::Array(blocks)) = obj.get("content") else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        if block.get("type").and_then(|value| value.as_str()) != Some("text") {
            continue;
        }
        let text = block.get("text").and_then(|value| value.as_str()).unwrap_or("");
        let Some(start) = text.find(GROK_QUERY_OPEN) else {
            continue;
        };
        let rest = &text[start + GROK_QUERY_OPEN.len()..];
        // 收尾用 rfind：grok 的包裹标签在最外层，取第一个闭合标签会让「提问正文里出现
        // 字面 </user_query>」（比如问「这个标签干嘛用的」）的后半句整段丢失。
        let inner = match rest.rfind(GROK_QUERY_CLOSE) {
            Some(end) => &rest[..end],
            None => rest,
        };
        let inner = inner.trim();
        if !inner.is_empty() {
            parts.push(inner.to_string());
        }
    }
    parts.join(" ")
}

/// grok 的 assistant 行 content 是纯字符串（不是 block 数组）。
fn grok_assistant_text(obj: &serde_json::Value) -> String {
    obj.get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 按 chat_history 的一行返回 (角色, 文本)。system / tool_result / reasoning 一律不算对话。
fn grok_dialog_line(obj: &serde_json::Value) -> Option<(&'static str, String)> {
    match obj.get("type").and_then(|value| value.as_str()) {
        Some("user") => {
            let text = grok_user_text(obj);
            (!text.is_empty()).then_some(("user", text))
        }
        Some("assistant") => {
            let text = grok_assistant_text(obj);
            (!text.is_empty()).then_some(("assistant", text))
        }
        _ => None,
    }
}

/// grok 列表阶段：枚举 `<root>/<cwd 目录>/<uuid>/`，元数据全部来自 `summary.json`。
///
/// 与其余三源的差别：mtime 用 `summary.updated_at`（不是文件 mtime——目录里有 `.lock`
/// 等旁路文件会扰动），size 用 `chat_history.jsonl` 的大小，标题直接用 `session_summary`
/// （grok 自己生成好了，不用我们从正文提）。
pub fn scan_grok_at(root: &Path, cache: &Path) -> Vec<SessionMeta> {
    let _ = std::fs::create_dir_all(cache);
    if !root.is_dir() {
        return Vec::new();
    }
    let mut by_session_id: HashMap<String, SessionMeta> = HashMap::new();
    let top_entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // 顶层混着 session_search.sqlite 等文件，非目录项在下面统一跳过
        Err(_) => return Vec::new(),
    };
    for cwd_entry in top_entries.flatten() {
        let cwd_dir = cwd_entry.path();
        // symlink 一律不认，见 real_entry_metadata
        if !real_entry_metadata(&cwd_dir).is_some_and(|stat| stat.is_dir()) {
            continue;
        }
        let session_dirs = match std::fs::read_dir(&cwd_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for session_entry in session_dirs.flatten() {
            let session_dir = session_entry.path();
            // 同级还有 prompt_history.jsonl 之类的散文件，只认带 summary.json 的真实目录；
            // 会话目录本身是 symlink 时同样跳过（否则整个会话内容可以来自 root 外）
            if !real_entry_metadata(&session_dir).is_some_and(|stat| stat.is_dir()) {
                continue;
            }
            let summary_path = session_dir.join("summary.json");
            if !real_entry_metadata(&summary_path).is_some_and(|stat| stat.is_file()) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&summary_path) else {
                continue;
            };
            let Ok(summary) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let info = summary.get("info");
            let Some(session_id) = info
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str())
                .and_then(valid_uuid_id)
            else {
                continue;
            };

            let history = session_dir.join("chat_history.jsonl");
            let size = history.metadata().ok().map(|stat| stat.len()).unwrap_or(0);
            // updated_at 原文同时用作缓存指纹（见 SourceCacheEntry::fingerprint）
            let updated_at_raw = summary
                .get("updated_at")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .or_else(|| summary.get("created_at").and_then(|value| value.as_str()))
                .unwrap_or("")
                .to_string();
            let mtime = grok_parse_time(&updated_at_raw).unwrap_or(0.0);

            let cache_file = cache.join(format!("{session_id}.json"));
            let mut previous_projects: Vec<String> = Vec::new();
            if let Ok(cached_raw) = std::fs::read_to_string(&cache_file) {
                if let Ok(mut cached) = serde_json::from_str::<SourceCacheEntry>(&cached_raw) {
                    // 指纹为空（summary.json 里 updated_at / created_at 都缺）时一律不命中：
                    // 空指纹跟 serde 的默认值撞在一起，会让「坏 summary + 等长正文」的会话
                    // 永远返回旧标题。宁可每次重扫，也不能拿不出证据还复用缓存。
                    if cached.version == GROK_CACHE_VERSION
                        && !updated_at_raw.is_empty()
                        && cached.meta.session_id == session_id
                        && cached_path_within_root(root, &cached.meta.path)
                        && cached.fingerprint == updated_at_raw
                        && cached.meta.size == size
                    {
                        cached.meta.source = GROK_SOURCE_ID.to_string();
                        keep_newest(&mut by_session_id, cached.meta);
                        continue;
                    }
                    previous_projects = cached
                        .meta
                        .projects
                        .split(',')
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .collect();
                }
            }

            let cwd = info
                .and_then(|value| value.get("cwd"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            // 先取字符串再回退：`session_summary` 为 null / 空串时 `get()` 仍返回 Some，
            // 在 Value 层做 or_else 会让 generated_title 永远轮不到。
            let title = summary
                .get("session_summary")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    summary
                        .get("generated_title")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("")
                .to_string();
            let mut meta = SessionMeta {
                session_id,
                path: history.to_string_lossy().to_string(),
                mtime,
                size,
                cwd: cwd.clone(),
                title,
                source: GROK_SOURCE_ID.to_string(),
                ..Default::default()
            };

            let tail = read_tail(&history, TAIL_CHUNK);
            let mut haystack_parts: Vec<String> = Vec::new();
            let mut fallback_title = String::new();
            for obj in iter_json_lines(&tail) {
                if let Some((role, text)) = grok_dialog_line(&obj) {
                    if fallback_title.is_empty() && role == "user" {
                        fallback_title = truncate_chars(&text, 30);
                    }
                    haystack_parts.push(truncate_chars(&text, 400));
                }
            }
            if meta.title.is_empty() {
                meta.title = fallback_title;
            }
            meta.haystack = tail_chars(&haystack_parts.join(" "), 8000);

            let mut projects: Vec<String> = Vec::new();
            pick_project(&cwd, &mut projects);
            if projects.is_empty() && !cwd.is_empty() {
                if let Some(name) = Path::new(&cwd)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty() && *value != "tmp")
                {
                    projects.push(name.to_string());
                }
            }
            meta.projects = merge_sticky_projects(&projects, &previous_projects, 6).join(",");

            let entry = SourceCacheEntry {
                version: GROK_CACHE_VERSION,
                fingerprint: updated_at_raw.clone(),
                meta: meta.clone(),
            };
            if let Ok(serialized) = serde_json::to_string(&entry) {
                let _ = std::fs::write(&cache_file, serialized);
            }
            keep_newest(&mut by_session_id, meta);
        }
    }
    // 同 pi 的二级排序：grok 的 mtime 来自 summary.updated_at，缺这个字段时统一兜底成 0.0，
    // 多个损坏会话会并列在同一个 mtime 上，只按 mtime 排会让它们的相对顺序不稳定。
    let mut metas: Vec<SessionMeta> = by_session_id.into_values().collect();
    metas.sort_by(|a, b| {
        b.mtime
            .partial_cmp(&a.mtime)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    metas
}

/// grok 详情阶段：只读 `chat_history.jsonl` 尾 512KB 抽最近若干轮对话。
///
/// `files` 恒为空：grok 的 chat_history 里 assistant 只落纯文本、tool_result 只落回显，
/// 工具入参（改了哪个文件）不在这个文件里——只有同目录 events.jsonl 有，而那份 90% 是
/// phase_changed 噪音（实测单会话 14k 行）。为一个次要字段解析它不划算。
pub fn grok_detail(meta: &SessionMeta, rounds: usize) -> SessionDetail {
    let tail = read_tail(Path::new(&meta.path), DETAIL_TAIL_CHUNK);
    let mut dialog: Vec<String> = Vec::new();
    for obj in iter_json_lines(&tail) {
        if let Some((role, text)) = grok_dialog_line(&obj) {
            let who = if role == "user" { "🧑" } else { "🤖" };
            dialog.push(format!("{who} {}", truncate_chars(&text, 200)));
        }
    }
    let mut kept_rev: Vec<String> = Vec::new();
    let mut user_seen = 0usize;
    for line in dialog.iter().rev() {
        kept_rev.push(line.clone());
        if line.starts_with("🧑") {
            user_seen += 1;
            if user_seen >= rounds {
                break;
            }
        }
    }
    kept_rev.reverse();
    SessionDetail {
        preview: kept_rev,
        files: Vec::new(),
    }
}

/// grok 会话 source：只读 `~/.grok/sessions/<cwd 目录>/<uuid>/`。
/// `session_search.sqlite` 是 grok 自己的检索库，本 source 从不碰它。
pub struct GrokSessionSource {
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl GrokSessionSource {
    pub fn default_paths() -> Self {
        Self {
            root: grok_sessions_dir(),
            cache: grok_cache_dir(),
        }
    }

    /// 同 PiSessionSource::find_by_id：走 scan，缓存命中时每条只读 summary.json + 缓存 JSON。
    pub fn find_by_id(&self, session_id: &str) -> Option<SessionMeta> {
        let session_id = valid_uuid_id(session_id)?;
        scan_grok_at(&self.root, &self.cache)
            .into_iter()
            .find(|meta| meta.session_id == session_id)
    }
}

impl SessionSource for GrokSessionSource {
    fn source_id(&self) -> &'static str {
        GROK_SOURCE_ID
    }

    fn scan(&self) -> Vec<SessionMeta> {
        scan_grok_at(&self.root, &self.cache)
    }
}

// -------- 分页 --------

/// 分页响应。跟前端 SPA 直接 IPC 传输。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPage {
    /// 过滤后（含 query 过滤）的总条数，用于前端算总页数
    pub total: usize,
    /// 当前页码（1-based，跟前端 UI 显示一致）
    pub page: usize,
    /// 每页条数（前端 selector 10/20/50）
    pub page_size: usize,
    /// 本页 slice 数据
    pub items: Vec<SessionMeta>,
}

/// 分页 + 可选 query 过滤。多个 source 的结果统一按收藏状态 + mtime 倒排后再切片。
/// page 从 1 开始；page 越界（超过总页数）返回空 items 但 total 保持真值。
pub fn list_paged(
    sources: &[&dyn SessionSource],
    query: Option<&str>,
    page: usize,
    page_size: usize,
) -> SessionPage {
    let all: Vec<SessionMeta> = sources.iter().flat_map(|s| s.scan()).collect();
    paginate_metas(all, &load_favorites(), query, page, page_size, false)
}

pub(crate) fn paginate_metas(
    mut all: Vec<SessionMeta>,
    favorites: &HashMap<String, bool>,
    query: Option<&str>,
    page: usize,
    page_size: usize,
    clamp_page: bool,
) -> SessionPage {
    let page_size = page_size.clamp(1, 500);
    for meta in &mut all {
        meta.favorited = favorites.get(&meta.session_id).copied().unwrap_or(false);
    }
    // 排序：收藏的优先（favorited 倒排），同收藏状态内按 mtime 倒排
    all.sort_by(|a, b| {
        b.favorited.cmp(&a.favorited).then_with(|| {
            b.mtime
                .partial_cmp(&a.mtime)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    let filtered: Vec<SessionMeta> = match query {
        Some(q) if !q.trim().is_empty() => search(&all, q).into_iter().cloned().collect(),
        _ => all,
    };
    let total = filtered.len();
    let page = if clamp_page {
        page.max(1).min(total.div_ceil(page_size).max(1))
    } else {
        page.max(1)
    };
    let start = (page - 1).saturating_mul(page_size);
    let items: Vec<SessionMeta> = if start >= total {
        Vec::new()
    } else {
        let end = (start + page_size).min(total);
        filtered[start..end].to_vec()
    };
    SessionPage {
        total,
        page,
        page_size,
        items,
    }
}

/// 全文搜索：query 在 haystack / title / cwd / projects / session_id 里命中即返回。
/// 空 query 返回全部（对拍 Python 版空搜等价于返回全集）。
pub fn search<'a>(metas: &'a [SessionMeta], query: &str) -> Vec<&'a SessionMeta> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return metas.iter().collect();
    }
    metas
        .iter()
        .filter(|m| {
            m.haystack.to_lowercase().contains(&q)
                || m.title.to_lowercase().contains(&q)
                || m.cwd.to_lowercase().contains(&q)
                || m.projects.to_lowercase().contains(&q)
                || m.session_id.to_lowercase().contains(&q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 粘性合并：本次没扫到（尾块窗口滚动）时保留上次已识别的项目——“同步后项目没了”的回归
    #[test]
    fn sticky_projects_keeps_prev_when_fresh_empty() {
        let got = merge_sticky_projects(&[], &["project-dashboard".to_string()], 6);
        assert_eq!(got, vec!["project-dashboard".to_string()]);
    }

    // 本次扫到的在前，历史补后，去重
    #[test]
    fn sticky_projects_fresh_first_then_prev_dedup() {
        let got = merge_sticky_projects(
            &["project-epsilon".to_string(), "project-dashboard".to_string()],
            &["project-dashboard".to_string(), "project-alpha".to_string()],
            6,
        );
        assert_eq!(
            got,
            vec![
                "project-epsilon".to_string(),
                "project-dashboard".to_string(),
                "project-alpha".to_string(),
            ]
        );
    }

    // 上限截断
    #[test]
    fn sticky_projects_caps_at_max() {
        let fresh: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        assert_eq!(merge_sticky_projects(&fresh, &[], 6).len(), 6);
    }

    #[test]
    fn pick_project_win_backslash() {
        let mut out = Vec::new();
        pick_project(r"F:\project-beta\src\main.rs", &mut out);
        assert_eq!(out, vec!["project-beta".to_string()]);
    }

    #[test]
    fn pick_project_msys_forwardslash() {
        let mut out = Vec::new();
        pick_project("/f/project-delta/app.py", &mut out);
        assert_eq!(out, vec!["project-delta".to_string()]);
    }

    #[test]
    fn pick_project_dedup() {
        let mut out = Vec::new();
        pick_project(r"F:\proj-a\a.txt", &mut out);
        pick_project(r"F:\proj-a\b.txt", &mut out);
        pick_project(r"F:\proj-b\c.txt", &mut out);
        assert_eq!(out, vec!["proj-a".to_string(), "proj-b".to_string()]);
    }

    #[test]
    fn pick_project_skips_tmp() {
        let mut out = Vec::new();
        pick_project(r"F:\tmp\scratch.txt", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn iter_json_lines_skips_bad() {
        let raw = br#"{"a":1}
not json
{"b":2}

{"c":3}"#;
        let values: Vec<_> = iter_json_lines(raw).collect();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0]["a"], 1);
        assert_eq!(values[1]["b"], 2);
        assert_eq!(values[2]["c"], 3);
    }

    #[cfg(unix)]
    #[test]
    fn read_head_retries_short_reads_until_limit() {
        use std::ffi::CString;
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        use std::thread;
        use std::time::{Duration, SystemTime};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "project-dashboard-read-head-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("short-reads.fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);

        let writer_path = fifo.clone();
        let writer = thread::spawn(move || {
            let mut file = OpenOptions::new().write(true).open(writer_path).unwrap();
            file.write_all(b"abc").unwrap();
            file.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
            let _ = file.write_all(b"defgh");
        });

        let got = read_head(&fifo, 8);
        let _ = writer.join();
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(
            got, b"abcdefgh",
            "单次 Read::read 允许短读，头部读取必须循环直到上限或 EOF"
        );
    }

    #[test]
    fn msg_text_string_content() {
        let v: serde_json::Value = serde_json::from_str(r#"{"message":{"content":"  hello  "}}"#).unwrap();
        assert_eq!(msg_text(&v), "hello");
    }

    #[test]
    fn msg_text_array_text_blocks() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"message":{"content":[
                {"type":"text","text":" foo "},
                {"type":"tool_use","name":"Edit","input":{}},
                {"type":"text","text":"bar"}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(msg_text(&v), "foo bar");
    }

    #[test]
    fn msg_text_filters_noise_prefix() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"message":{"content":"<system-reminder> noop"}}"#).unwrap();
        assert_eq!(msg_text(&v), "");
    }

    #[test]
    fn truncate_chars_utf8_safe() {
        // 每个汉字 3 字节 UTF-8；按字符切不能 panic
        let s = "你好世界";
        assert_eq!(truncate_chars(s, 2), "你好");
        assert_eq!(truncate_chars(s, 100), "你好世界");
    }

    #[test]
    fn tail_chars_takes_last_n() {
        assert_eq!(tail_chars("abcdefghij", 3), "hij");
        assert_eq!(tail_chars("abc", 10), "abc");
    }

    #[test]
    fn search_empty_query_returns_all() {
        let metas = vec![
            SessionMeta { session_id: "a".into(), ..Default::default() },
            SessionMeta { session_id: "b".into(), ..Default::default() },
        ];
        assert_eq!(search(&metas, "").len(), 2);
    }

    #[test]
    fn search_matches_haystack_case_insensitive() {
        let metas = vec![SessionMeta {
            session_id: "x".into(),
            haystack: "Fleet Tools Rust Rewrite".into(),
            ..Default::default()
        }];
        assert_eq!(search(&metas, "rust").len(), 1);
        assert_eq!(search(&metas, "FLEET").len(), 1);
        assert_eq!(search(&metas, "python").len(), 0);
    }
}

// -------- 收藏状态管理 --------

/// 收藏状态持久化文件路径
pub fn favorites_file() -> PathBuf {
    crate::session_storage::default_storage().favorites_file()
}

/// 旧接口的尽力读取契约保留；产品入口使用严格读取并返回错误。
pub fn load_favorites() -> HashMap<String, bool> {
    crate::session_storage::default_storage()
        .load_favorites()
        .unwrap_or_default()
}

pub fn save_favorites(favorites: &HashMap<String, bool>) -> std::io::Result<()> {
    crate::session_storage::default_storage()
        .save_favorites(favorites)
        .map_err(std::io::Error::other)
}

pub fn toggle_favorite(session_id: &str) -> std::io::Result<bool> {
    crate::session_storage::default_storage()
        .toggle_favorite(session_id)
        .map_err(std::io::Error::other)
}

/// 应用收藏状态到 SessionMeta 列表：从持久化文件读取并设置每条的 favorited 字段
pub fn apply_favorites(metas: &mut [SessionMeta]) {
    let favorites = load_favorites();
    for meta in metas.iter_mut() {
        meta.favorited = favorites.get(&meta.session_id).copied().unwrap_or(false);
    }
}

/// 导入仅校验既有格式和来源，不改动版本或重新推导累积项目标签。
pub(crate) fn valid_cache_bytes(source: &str, bytes: &[u8]) -> bool {
    let meta = match source {
        CLAUDE_SOURCE_ID => serde_json::from_slice::<SessionMeta>(bytes).ok(),
        CODEX_SOURCE_ID => serde_json::from_slice::<CodexCacheEntry>(bytes)
            .ok()
            .map(|v| v.meta),
        PI_SOURCE_ID | GROK_SOURCE_ID => serde_json::from_slice::<SourceCacheEntry>(bytes)
            .ok()
            .map(|v| v.meta),
        _ => None,
    };
    meta.is_some_and(|meta| {
        !meta.session_id.is_empty() && (meta.source.is_empty() || meta.source == source)
    })
}
