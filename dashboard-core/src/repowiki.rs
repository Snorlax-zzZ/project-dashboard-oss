use crate::config::Registry;
use crate::tracker::normalize_git_url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

const PROTOCOL_VERSION: u32 = 1;
const MAX_SEARCH_BYTES: usize = 512;
const MAX_SEARCH_RESULTS: u32 = 100;
const MAX_GRAPH_DEPTH: u32 = 5;
const MAX_GRAPH_RESULTS: u32 = 200;
const PROCESS_LIMITS: ProcessLimits = ProcessLimits {
    timeout: Duration::from_secs(30),
    stdout_bytes: 20 * 1024 * 1024,
    stderr_bytes: 64 * 1024,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RepoWikiProjectSelector {
    Shared { project_id: Uuid },
    Private { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoWikiRemoteError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoWikiEnvelope {
    pub protocol_version: u32,
    pub command: String,
    pub ok: bool,
    pub result: Value,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub error: Option<RepoWikiRemoteError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoWikiError {
    RegistryUnavailable,
    LocalConfigMissing,
    CliPathMissing,
    DataRootMissing,
    InvalidCliPath,
    InvalidDataRoot,
    ProjectNotFound,
    ProjectNotBound,
    ProjectBindingMismatch,
    InvalidProjectPath,
    InvalidArgument,
    PlatformUnsupported,
    SpawnFailed,
    Timeout,
    OutputTooLarge,
    ProtocolViolation,
}

impl RepoWikiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RegistryUnavailable => "registry_unavailable",
            Self::LocalConfigMissing => "local_config_missing",
            Self::CliPathMissing => "repowiki_cli_path_missing",
            Self::DataRootMissing => "repowiki_data_root_missing",
            Self::InvalidCliPath => "invalid_repowiki_cli_path",
            Self::InvalidDataRoot => "invalid_repowiki_data_root",
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectNotBound => "project_not_bound",
            Self::ProjectBindingMismatch => "project_binding_mismatch",
            Self::InvalidProjectPath => "invalid_project_path",
            Self::InvalidArgument => "invalid_repowiki_argument",
            Self::PlatformUnsupported => "repowiki_platform_unsupported",
            Self::SpawnFailed => "repowiki_spawn_failed",
            Self::Timeout => "repowiki_timeout",
            Self::OutputTooLarge => "repowiki_output_too_large",
            Self::ProtocolViolation => "repowiki_protocol_violation",
        }
    }
}

impl std::fmt::Display for RepoWikiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RegistryUnavailable => "无法读取 Dashboard 项目注册表",
            Self::LocalConfigMissing => "本机配置不存在",
            Self::CliPathMissing => "尚未配置 RepoWiki CLI 绝对路径",
            Self::DataRootMissing => "尚未配置 RepoWiki data root 绝对路径",
            Self::InvalidCliPath => "RepoWiki CLI 路径不安全或不可执行",
            Self::InvalidDataRoot => "RepoWiki data root 路径无效",
            Self::ProjectNotFound => "Dashboard 注册表中不存在该项目",
            Self::ProjectNotBound => "该共享项目没有当前机器的本地路径绑定",
            Self::ProjectBindingMismatch => "Dashboard 项目路径与登记的 Git 仓库不一致",
            Self::InvalidProjectPath => "Dashboard 注册的本地项目路径无效",
            Self::InvalidArgument => "RepoWiki 只读请求参数非法",
            Self::PlatformUnsupported => "当前 RepoWiki 只读适配器仅支持 POSIX 平台",
            Self::SpawnFailed => "无法启动 RepoWiki CLI",
            Self::Timeout => "RepoWiki CLI 执行超时，进程已终止",
            Self::OutputTooLarge => "RepoWiki CLI 输出超过安全上限，进程已终止",
            Self::ProtocolViolation => "RepoWiki CLI 返回了无效协议响应",
        };
        write!(formatter, "[{}] {message}", self.code())
    }
}

impl std::error::Error for RepoWikiError {}

struct InvocationContext {
    cli: PathBuf,
    data_root: PathBuf,
    repo: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct ProcessLimits {
    timeout: Duration,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

#[derive(Debug)]
struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

pub fn status(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    invoke(
        dashboard_state_root,
        local_path,
        selector,
        "status",
        &["status".to_string()],
    )
}

// v2 读取：Wiki 已从 v1「固定 10 页」升级为 v2「动态目录」，CLI 的 read 命令
// 明文不支持 v2（help：不支持动态 v2 navigation）。Dashboard 只读展示改为直接读取
// 项目内 `.repowiki/` 的 v2 产物：读 navigation.json 得到目录树，点页面时读对应 md。
pub fn read_project(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    let context = resolve_context(dashboard_state_root, local_path, selector)?;
    let navigation = read_v2_navigation(&context.repo)?;
    Ok(v2_envelope("read project", json!({ "navigation": navigation })))
}

// page_id 来自前端点击导航项，取值是 navigation.json 列出的 v2 动态 page_id。
// 页面正文路径只信任 navigation 里登记的 path（白名单），再经 read_repowiki_file
// 的 canonical 前缀校验兜底，双重防止越出 `.repowiki/` 目录。
pub fn read_page(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    page_id: &str,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    if page_id.is_empty() || page_id.len() > MAX_PAGE_ID_BYTES {
        return Err(RepoWikiError::InvalidArgument);
    }
    let context = resolve_context(dashboard_state_root, local_path, selector)?;
    let navigation = read_v2_navigation(&context.repo)?;
    let pages = navigation
        .get("pages")
        .and_then(|value| value.as_array())
        .ok_or(RepoWikiError::ProtocolViolation)?;
    let page = pages
        .iter()
        .find(|entry| entry.get("page_id").and_then(|value| value.as_str()) == Some(page_id))
        .ok_or(RepoWikiError::InvalidArgument)?;
    let path = page
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or(RepoWikiError::ProtocolViolation)?;
    let title = page
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or(page_id);
    let content = read_repowiki_file(&context.repo, path, MAX_PAGE_BYTES)?;
    Ok(v2_envelope(
        "read page",
        json!({
            "page_id": page_id,
            "title": title,
            "path": path,
            "content": content,
        }),
    ))
}

// read_generations 回传 v2 正式代次。不能沿用 CLI `status` 的 wiki 段：那一段走的是 v1
// 校验器（CLI `wiki.py` 的 `_formal_baseline`，硬要求 `schema_version==1` 与 v1 固定 10 页
// 清单），喂 v2 wiki 必然判 `damaged` 且 `generation_id` 为 null —— 那是 v1/v2 契约分裂的
// 产物，不是 wiki 真坏。故代次与审查一律以 `.repowiki/state/generation.json` 直读为准，
// status 只继续供 graph 与分支（那两段本来就按当前源码算，是准的）。
pub fn read_generations(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    let context = resolve_context(dashboard_state_root, local_path, selector)?;
    let generation = read_v2_generation(&context.repo)?;
    Ok(v2_envelope("read generations", generation))
}

// v2 正式代次直读。只回传摘要字段：`pages` 有数百条（此处只需摘要），
// 全量回传对「代次 / 审查」摘要卡没有用处，与 navigation 剔除 source_anchors 同理。
fn read_v2_generation(repo: &Path) -> Result<Value, RepoWikiError> {
    let raw = read_repowiki_file(repo, "state/generation.json", MAX_GENERATION_BYTES)?;
    let document: Value =
        serde_json::from_str(&raw).map_err(|_| RepoWikiError::ProtocolViolation)?;
    if document.get("schema_version").and_then(|value| value.as_u64()) != Some(2) {
        return Err(RepoWikiError::ProtocolViolation);
    }
    let generation_id = document
        .get("generation_id")
        .and_then(|value| value.as_str())
        .ok_or(RepoWikiError::ProtocolViolation)?;
    let source = document.get("source");
    let source_field = |key: &str| {
        source
            .and_then(|value| value.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let pages = document.get("pages").and_then(|value| value.as_object());
    let page_total = pages.map(|map| map.len()).unwrap_or(0);
    let reviewed = read_v2_reviewed_total(repo, pages);
    // 新鲜度需要跟「当前 HEAD / 当前图谱代次」比才能判，那两个值只有 status 有，
    // 故这里只回传生成时的事实，freshness 交前端合并 status 后算，不在此处臆断。
    let integrity = if document.get("status").and_then(|value| value.as_str()) == Some("complete") {
        "healthy"
    } else {
        "damaged"
    };
    Ok(json!({
        "wiki": {
            "generation_id": generation_id,
            "status": document.get("status").cloned().unwrap_or(Value::Null),
            "integrity": integrity,
            "layout_version": document.get("layout_version").cloned().unwrap_or(Value::Null),
            "mode": document.get("mode").cloned().unwrap_or(Value::Null),
            "generated_at": source
                .and_then(|value| value.get("generated_at"))
                .cloned()
                .unwrap_or_else(|| document.get("published_at").cloned().unwrap_or(Value::Null)),
            "head_commit": source_field("head_commit"),
            "graph_generation_id": document.get("graph_generation_id").cloned().unwrap_or(Value::Null),
            "page_total": page_total,
        },
        "branch": { "branch_ref": source_field("branch_ref") },
        "review": {
            "totals": {
                "total": page_total,
                "reviewed": reviewed,
                "unreviewed": page_total.saturating_sub(reviewed),
            }
        },
    }))
}

// 审查态：`state/review.json` 是可选文件，读不出或契约不符都按「无审查记录」算，
// 不让它拖垮代次展示。注意上游 `_load_review_state` 只接受 v1 固定 page_id，
// 因此 v2 wiki 下 entries 实测恒为空；这里按 v2 代次页 id 取交集，不认 v1 残留 id。
fn read_v2_reviewed_total(
    repo: &Path,
    pages: Option<&serde_json::Map<String, Value>>,
) -> usize {
    let Some(known) = pages else {
        return 0;
    };
    let Ok(raw) = read_repowiki_file(repo, "state/review.json", MAX_REVIEW_BYTES) else {
        return 0;
    };
    let Ok(document) = serde_json::from_str::<Value>(&raw) else {
        return 0;
    };
    document
        .get("entries")
        .and_then(|value| value.as_object())
        .map(|entries| {
            entries
                .keys()
                .filter(|page_id| known.contains_key(page_id.as_str()))
                .count()
        })
        .unwrap_or(0)
}

const MAX_NAVIGATION_BYTES: u64 = 32 * 1024 * 1024;
const MAX_GENERATION_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REVIEW_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PAGE_ID_BYTES: usize = 512;

fn v2_envelope(command: &str, result: Value) -> RepoWikiEnvelope {
    RepoWikiEnvelope {
        protocol_version: PROTOCOL_VERSION,
        command: command.to_string(),
        ok: true,
        result,
        warnings: Vec::new(),
        error: None,
    }
}

// 读取并精简 v2 navigation.json：只保留导航渲染需要的字段，剔除体积巨大的
// source_anchors（每页可含数百条源码符号引用，全量回传会让响应膨胀到数 MB）。
fn read_v2_navigation(repo: &Path) -> Result<Value, RepoWikiError> {
    let raw = read_repowiki_file(repo, "navigation.json", MAX_NAVIGATION_BYTES)?;
    let document: Value =
        serde_json::from_str(&raw).map_err(|_| RepoWikiError::ProtocolViolation)?;
    if document.get("schema_version").and_then(|value| value.as_u64()) != Some(2) {
        return Err(RepoWikiError::ProtocolViolation);
    }
    let pages = document
        .get("pages")
        .and_then(|value| value.as_array())
        .ok_or(RepoWikiError::ProtocolViolation)?;
    let slim_pages: Vec<Value> = pages
        .iter()
        .map(|page| {
            json!({
                "page_id": page.get("page_id").cloned().unwrap_or(Value::Null),
                "title": page.get("title").cloned().unwrap_or(Value::Null),
                "path": page.get("path").cloned().unwrap_or(Value::Null),
                "page_type": page.get("page_type").cloned().unwrap_or(Value::Null),
                "parent_page_id": page.get("parent_page_id").cloned().unwrap_or(Value::Null),
                "order": page.get("order").cloned().unwrap_or(Value::Null),
                "children": page.get("children").cloned().unwrap_or(Value::Array(Vec::new())),
            })
        })
        .collect();
    Ok(json!({
        "schema_version": 2,
        "home_page_id": document.get("home_page_id").cloned().unwrap_or(Value::Null),
        "layout_version": document.get("layout_version").cloned().unwrap_or(Value::Null),
        "pages": slim_pages,
    }))
}

// 安全读取 `<repo>/.repowiki/` 下的文件：拒绝绝对路径 / `..` / 盘符，再用
// canonical 前缀校验挡住符号链接逃逸，最后限制普通文件与大小。
fn read_repowiki_file(repo: &Path, relative: &str, max_bytes: u64) -> Result<String, RepoWikiError> {
    if relative.is_empty() {
        return Err(RepoWikiError::InvalidArgument);
    }
    let relative_path = Path::new(relative);
    let has_unsafe_component = relative_path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    });
    if relative_path.is_absolute() || has_unsafe_component {
        return Err(RepoWikiError::InvalidArgument);
    }
    let base = std::fs::canonicalize(repo.join(".repowiki"))
        .map_err(|_| RepoWikiError::InvalidProjectPath)?;
    let resolved = std::fs::canonicalize(base.join(relative_path))
        .map_err(|_| RepoWikiError::InvalidArgument)?;
    if !resolved.starts_with(&base) {
        return Err(RepoWikiError::InvalidArgument);
    }
    let metadata =
        std::fs::symlink_metadata(&resolved).map_err(|_| RepoWikiError::InvalidArgument)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(RepoWikiError::InvalidArgument);
    }
    std::fs::read_to_string(&resolved).map_err(|_| RepoWikiError::InvalidArgument)
}

pub fn graph_search(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    text: &str,
    limit: u32,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    if text.trim().is_empty()
        || text.len() > MAX_SEARCH_BYTES
        || !(1..=MAX_SEARCH_RESULTS).contains(&limit)
    {
        return Err(RepoWikiError::InvalidArgument);
    }
    invoke(
        dashboard_state_root,
        local_path,
        selector,
        "graph search",
        &[
            "graph".to_string(),
            "search".to_string(),
            "--text".to_string(),
            text.to_string(),
            "--limit".to_string(),
            limit.to_string(),
        ],
    )
}

pub fn graph_show(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    node_id: &str,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    validate_node_id(node_id)?;
    invoke(
        dashboard_state_root,
        local_path,
        selector,
        "graph show",
        &[
            "graph".to_string(),
            "show".to_string(),
            "--node".to_string(),
            node_id.to_string(),
        ],
    )
}

pub fn graph_callers(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    node_id: &str,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    graph_neighborhood(
        dashboard_state_root,
        local_path,
        selector,
        "callers",
        node_id,
        depth,
        limit,
    )
}

pub fn graph_callees(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    node_id: &str,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    graph_neighborhood(
        dashboard_state_root,
        local_path,
        selector,
        "callees",
        node_id,
        depth,
        limit,
    )
}

pub fn graph_impact(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    node_id: &str,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    graph_neighborhood(
        dashboard_state_root,
        local_path,
        selector,
        "impact",
        node_id,
        depth,
        limit,
    )
}

fn graph_neighborhood(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    operation: &'static str,
    node_id: &str,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    validate_node_id(node_id)?;
    if !(1..=MAX_GRAPH_DEPTH).contains(&depth) || !(1..=MAX_GRAPH_RESULTS).contains(&limit) {
        return Err(RepoWikiError::InvalidArgument);
    }
    invoke(
        dashboard_state_root,
        local_path,
        selector,
        &format!("graph {operation}"),
        &[
            "graph".to_string(),
            operation.to_string(),
            "--node".to_string(),
            node_id.to_string(),
            "--depth".to_string(),
            depth.to_string(),
            "--limit".to_string(),
            limit.to_string(),
        ],
    )
}

fn validate_node_id(node_id: &str) -> Result<(), RepoWikiError> {
    if node_id.len() == 64
        && node_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(RepoWikiError::InvalidArgument)
    }
}

fn invoke(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
    expected_command: &str,
    command_args: &[String],
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    #[cfg(not(unix))]
    {
        let _ = (
            dashboard_state_root,
            local_path,
            selector,
            expected_command,
            command_args,
        );
        return Err(RepoWikiError::PlatformUnsupported);
    }
    let context = resolve_context(dashboard_state_root, local_path, selector)?;
    let mut command = Command::new(&context.cli);
    command
        .arg("--repo")
        .arg(&context.repo)
        .arg("--data-root")
        .arg(&context.data_root)
        .arg("--json")
        .args(command_args);
    let output = run_bounded(command, PROCESS_LIMITS)?;
    parse_envelope(&output.stdout, output.status.success(), expected_command)
}

fn run_bounded(
    mut command: Command,
    limits: ProcessLimits,
) -> Result<BoundedOutput, RepoWikiError> {
    #[cfg(not(unix))]
    {
        let _ = (command, limits);
        return Err(RepoWikiError::PlatformUnsupported);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| RepoWikiError::SpawnFailed)?;
    let mut stdout_reader = child.stdout.take().ok_or(RepoWikiError::SpawnFailed)?;
    let mut stderr_reader = child.stderr.take().ok_or(RepoWikiError::SpawnFailed)?;
    set_nonblocking(&stdout_reader)?;
    set_nonblocking(&stderr_reader)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let started = Instant::now();

    let status = loop {
        if let Err(error) = drain_nonblocking(&mut stdout_reader, &mut stdout, limits.stdout_bytes)
            .and_then(|_| {
                drain_nonblocking(&mut stderr_reader, &mut stderr, limits.stderr_bytes)
            })
        {
            terminate_child(&mut child);
            return Err(error);
        }
        if started.elapsed() >= limits.timeout {
            terminate_child(&mut child);
            return Err(RepoWikiError::Timeout);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_process_group(child.id());
                drain_nonblocking(&mut stdout_reader, &mut stdout, limits.stdout_bytes)?;
                drain_nonblocking(&mut stderr_reader, &mut stderr, limits.stderr_bytes)?;
                break status;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                terminate_child(&mut child);
                return Err(RepoWikiError::SpawnFailed);
            }
        }
    };

    // 不等待 EOF：逃逸出进程组的后代可能仍继承写端。此处丢弃读端后，后代继续
    // 写会得到 EPIPE/SIGPIPE，既不会拖住 Dashboard，也不会在磁盘留下无界输出。
    drop(stdout_reader);
    drop(stderr_reader);
    Ok(BoundedOutput { status, stdout })
}

#[cfg(unix)]
fn set_nonblocking<T: std::os::fd::AsRawFd>(stream: &T) -> Result<(), RepoWikiError> {
    let descriptor = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(RepoWikiError::SpawnFailed);
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_nonblocking<T>(_stream: &T) -> Result<(), RepoWikiError> {
    Err(RepoWikiError::PlatformUnsupported)
}

fn drain_nonblocking(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<(), RepoWikiError> {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                if output.len() > limit {
                    return Err(RepoWikiError::OutputTooLarge);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(_) => return Err(RepoWikiError::SpawnFailed),
        }
    }
}

fn terminate_child(child: &mut std::process::Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_process_group(process_id: u32) {
    const SIGKILL: i32 = 9;
    unsafe extern "C" {
        fn kill(process: i32, signal: i32) -> i32;
    }
    if let Ok(group) = i32::try_from(process_id) {
        // run_bounded 为每次 CLI 调用创建独立 process group；负 PID 只
        // 终止该调用残留的后代，不触及 Dashboard 自身。
        let _ = unsafe { kill(-group, SIGKILL) };
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_process_id: u32) {}

fn resolve_context(
    dashboard_state_root: &Path,
    local_path: &Path,
    selector: &RepoWikiProjectSelector,
) -> Result<InvocationContext, RepoWikiError> {
    let registry = Registry::load_at_read_only(dashboard_state_root, local_path)
        .map_err(|_| RepoWikiError::RegistryUnavailable)?;
    let local = registry
        .local
        .as_ref()
        .ok_or(RepoWikiError::LocalConfigMissing)?;
    let cli = configured_absolute_path(
        local.repowiki_cli_path.as_deref(),
        RepoWikiError::CliPathMissing,
        RepoWikiError::InvalidCliPath,
    )?;
    validate_cli(&cli)?;
    let data_root = configured_absolute_path(
        local.repowiki_data_root.as_deref(),
        RepoWikiError::DataRootMissing,
        RepoWikiError::InvalidDataRoot,
    )?;
    validate_data_root(&data_root)?;
    let (configured_repo, expected_git_urls) = match selector {
        RepoWikiProjectSelector::Shared { project_id } => {
            let project = registry
                .projects
                .get(project_id)
                .ok_or(RepoWikiError::ProjectNotFound)?;
            (
                project
                    .machines
                    .get(&local.machine_id)
                    .ok_or(RepoWikiError::ProjectNotBound)?,
                std::iter::once(project.git_url.clone())
                    .chain(project.git_url_aliases.iter().cloned())
                    .collect::<Vec<_>>(),
            )
        }
        RepoWikiProjectSelector::Private { name } => {
            let project = registry
                .list_private()
                .into_iter()
                .find(|project| project.name == *name)
                .ok_or(RepoWikiError::ProjectNotFound)?;
            (&project.path, vec![project.git_url.clone()])
        }
    };
    let lexical_repo = expand_home(configured_repo);
    if !lexical_repo.is_absolute() {
        return Err(RepoWikiError::InvalidProjectPath);
    }
    let metadata =
        std::fs::symlink_metadata(&lexical_repo).map_err(|_| RepoWikiError::InvalidProjectPath)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RepoWikiError::InvalidProjectPath);
    }
    let repo =
        std::fs::canonicalize(lexical_repo).map_err(|_| RepoWikiError::InvalidProjectPath)?;
    verify_project_binding(&repo, &expected_git_urls)?;
    Ok(InvocationContext {
        cli,
        data_root,
        repo,
    })
}

fn verify_project_binding(repo: &Path, expected_urls: &[String]) -> Result<(), RepoWikiError> {
    let actual = read_origin_url(repo).ok_or(RepoWikiError::ProjectBindingMismatch)?;
    let actual = normalize_git_url(&actual).map_err(|_| RepoWikiError::ProjectBindingMismatch)?;
    let matches = expected_urls.iter().any(|expected| {
        normalize_git_url(expected)
            .ok()
            .is_some_and(|candidate| candidate == actual)
    });
    if matches {
        Ok(())
    } else {
        Err(RepoWikiError::ProjectBindingMismatch)
    }
}

fn read_origin_url(repo: &Path) -> Option<String> {
    const MAX_GIT_CONFIG_BYTES: u64 = 1024 * 1024;
    let path = resolve_git_config_path(repo)?;
    let content = read_regular_text(&path, MAX_GIT_CONFIG_BYTES)?;
    parse_origin_url(&content)
}

fn resolve_git_config_path(repo: &Path) -> Option<PathBuf> {
    const MAX_GIT_POINTER_BYTES: u64 = 4096;
    let dot_git = repo.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.is_dir() {
        return Some(dot_git.join("config"));
    }
    if !metadata.file_type().is_file() {
        return None;
    }
    let pointer = read_regular_text(&dot_git, MAX_GIT_POINTER_BYTES)?;
    let mut lines = pointer.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next()?.trim();
    if lines.next().is_some() {
        return None;
    }
    let (key, raw_git_dir) = line.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case("gitdir") || raw_git_dir.trim().is_empty() {
        return None;
    }
    let configured_git_dir = PathBuf::from(raw_git_dir.trim());
    let lexical_git_dir = if configured_git_dir.is_absolute() {
        configured_git_dir
    } else {
        repo.join(configured_git_dir)
    };
    let git_dir = std::fs::canonicalize(lexical_git_dir).ok()?;
    let git_dir_metadata = std::fs::symlink_metadata(&git_dir).ok()?;
    if git_dir_metadata.file_type().is_symlink() || !git_dir_metadata.is_dir() {
        return None;
    }

    let common_pointer = git_dir.join("commondir");
    let common_dir = match std::fs::symlink_metadata(&common_pointer) {
        Ok(_) => {
            let configured_common = read_regular_text(&common_pointer, MAX_GIT_POINTER_BYTES)?;
            let mut lines = configured_common
                .lines()
                .filter(|line| !line.trim().is_empty());
            let raw_common = lines.next()?.trim();
            if raw_common.is_empty() || lines.next().is_some() {
                return None;
            }
            let configured_common = PathBuf::from(raw_common);
            let lexical_common = if configured_common.is_absolute() {
                configured_common
            } else {
                git_dir.join(configured_common)
            };
            let common = std::fs::canonicalize(lexical_common).ok()?;
            let expected = std::fs::canonicalize(git_dir.parent()?.parent()?).ok()?;
            if common != expected {
                return None;
            }
            common
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir,
        Err(_) => return None,
    };
    let common_metadata = std::fs::symlink_metadata(&common_dir).ok()?;
    if common_metadata.file_type().is_symlink() || !common_metadata.is_dir() {
        return None;
    }
    Some(common_dir.join("config"))
}

fn read_regular_text(path: &Path, max_bytes: u64) -> Option<String> {
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > max_bytes
    {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return None;
        }
    }
    std::fs::read_to_string(path).ok()
}

fn parse_origin_url(content: &str) -> Option<String> {
    let mut origin_section = false;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            let compact = line
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            origin_section = compact == "[remote\"origin\"]";
            continue;
        }
        if !origin_section || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn configured_absolute_path(
    configured: Option<&str>,
    missing: RepoWikiError,
    invalid: RepoWikiError,
) -> Result<PathBuf, RepoWikiError> {
    let value = configured.map(str::trim).filter(|value| !value.is_empty());
    let path = PathBuf::from(value.ok_or(missing)?);
    if !path.is_absolute() {
        return Err(invalid);
    }
    Ok(path)
}

fn validate_cli(path: &Path) -> Result<(), RepoWikiError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| RepoWikiError::InvalidCliPath)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(RepoWikiError::InvalidCliPath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.permissions().mode() & 0o111 == 0 {
            return Err(RepoWikiError::InvalidCliPath);
        }
    }
    Ok(())
}

fn validate_data_root(path: &Path) -> Result<(), RepoWikiError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            Err(RepoWikiError::InvalidDataRoot)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RepoWikiError::InvalidDataRoot),
    }
}

fn expand_home(configured: &str) -> PathBuf {
    for prefix in ["~/", "~\\", "$HOME/"] {
        if let Some(relative) = configured.strip_prefix(prefix) {
            if let Some(home) = dirs::home_dir() {
                return home.join(relative);
            }
        }
    }
    PathBuf::from(configured)
}

fn parse_envelope(
    stdout: &[u8],
    process_success: bool,
    expected_command: &str,
) -> Result<RepoWikiEnvelope, RepoWikiError> {
    let envelope: RepoWikiEnvelope =
        serde_json::from_slice(stdout).map_err(|_| RepoWikiError::ProtocolViolation)?;
    let error_shape_valid = match (&envelope.error, envelope.ok) {
        (None, true) => true,
        (Some(error), false) => {
            !error.code.trim().is_empty()
                && !error.message.trim().is_empty()
                && error.details.is_object()
        }
        _ => false,
    };
    if envelope.protocol_version != PROTOCOL_VERSION
        || envelope.command != expected_command
        || envelope.result.as_object().is_none()
        || envelope.ok != process_success
        || !error_shape_valid
    {
        return Err(RepoWikiError::ProtocolViolation);
    }
    Ok(envelope)
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    fn executable(tag: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dashboard-repowiki-process-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn process_timeout_kills_the_cli() {
        let cli = executable("timeout", "while :; do :; done");
        let started = Instant::now();

        let error = run_bounded(
            Command::new(&cli),
            ProcessLimits {
                timeout: Duration::from_millis(100),
                stdout_bytes: 1024,
                stderr_bytes: 1024,
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "repowiki_timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn process_output_limit_stops_stdout_and_stderr_floods() {
        for (tag, body) in [
            (
                "stdout",
                "i=0; while [ $i -lt 4096 ]; do printf x; i=$((i + 1)); done",
            ),
            (
                "stderr",
                "i=0; while [ $i -lt 4096 ]; do printf x >&2; i=$((i + 1)); done",
            ),
        ] {
            let cli = executable(tag, body);
            let error = run_bounded(
                Command::new(&cli),
                ProcessLimits {
                    timeout: Duration::from_secs(2),
                    stdout_bytes: 64,
                    stderr_bytes: 64,
                },
            )
            .unwrap_err();

            assert_eq!(error.code(), "repowiki_output_too_large");
        }
    }

    #[test]
    fn successful_parent_does_not_wait_for_descendants_inheriting_output() {
        let cli = executable(
            "descendant-pipe",
            "exec python -c 'import os,time\npid=os.fork()\nif pid == 0: time.sleep(4); os._exit(0)\nos.write(1,b\"ok\"); os._exit(0)'",
        );
        let started = Instant::now();

        let output = run_bounded(
            Command::new(&cli),
            ProcessLimits {
                timeout: Duration::from_secs(2),
                stdout_bytes: 1024,
                stderr_bytes: 1024,
            },
        )
        .unwrap();

        assert_eq!(output.stdout, b"ok");
        assert!(started.elapsed() < Duration::from_millis(2500));
    }

    #[test]
    fn successful_parent_is_bounded_even_if_descendant_escapes_process_group() {
        let cli = executable(
            "escaped-descendant",
            "exec python -c 'import os,time\nready_r,ready_w=os.pipe()\npid=os.fork()\nif pid == 0:\n grandchild=os.fork()\n if grandchild != 0: os._exit(0)\n os.close(ready_r); os.setsid(); os.write(ready_w,b\"1\"); os.close(ready_w); time.sleep(4); os._exit(0)\nos.close(ready_w); os.read(ready_r,1); os.close(ready_r); os.write(1,b\"ok\"); os._exit(0)'",
        );
        let started = Instant::now();

        let output = run_bounded(
            Command::new(&cli),
            ProcessLimits {
                timeout: Duration::from_secs(2),
                stdout_bytes: 1024,
                stderr_bytes: 1024,
            },
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
        assert!(started.elapsed() < Duration::from_millis(2500));
    }

    #[test]
    fn escaped_descendant_cannot_keep_writing_after_parent_returns() {
        let outcome = std::env::temp_dir().join(format!(
            "dashboard-repowiki-escaped-writer-outcome-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let cli = executable(
            "escaped-writer",
            &format!(
                "exec python -c 'import os,time\noutcome=r\"{}\"\nroot=os.getpid(); ready_r,ready_w=os.pipe(); pid=os.fork()\nif pid == 0:\n grandchild=os.fork()\n if grandchild != 0: os._exit(0)\n os.close(ready_r); os.setsid(); os.write(ready_w,b\"1\"); os.close(ready_w)\n while True:\n  try: os.kill(root,0)\n  except ProcessLookupError: break\n  time.sleep(0.005)\n time.sleep(0.05)\n try:\n  for _ in range(10000): os.write(1,b\"x\"*8192)\n except OSError:\n  open(outcome,\"w\").write(\"blocked\")\n else:\n  open(outcome,\"w\").write(\"wrote\")\n os._exit(0)\nos.close(ready_w); os.read(ready_r,1); os.close(ready_r); os.write(1,b\"ok\"); os._exit(0)'",
                outcome.display()
            ),
        );

        let output = run_bounded(
            Command::new(&cli),
            ProcessLimits {
                timeout: Duration::from_secs(2),
                stdout_bytes: 16 * 1024,
                stderr_bytes: 1024,
            },
        )
        .unwrap();

        assert_eq!(output.stdout, b"ok");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !outcome.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&outcome).unwrap(), "blocked");
        let _ = fs::remove_file(outcome);
    }
}
