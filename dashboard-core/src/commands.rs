// §5-§9 Tauri IPC 业务逻辑层（跟 Tauri runtime 解耦，方便单测 + 未来 UI 换栈）。
//
// 每个函数 = 一个 UI 场景。dashboard-tauri 里 `#[tauri::command]` wrapper 只做参数透传 + Result 转 String。
//
// 返回类型全部实现 Serialize，方便 Tauri IPC 直接 emit。

use crate::aggregate::{
    aggregate_from_dashboard_state, find_project_id_by_git_url, ProjectAggregate, UuidLookupError,
    DEFAULT_STALE_THRESHOLD,
};
use crate::backup::{prune_old_backups, snapshot_backup};
use crate::config::{
    EditRegistryProject, LocalConfig, PrivateProject, Registry, RegistryProject,
    CURRENT_SCHEMA_VERSION,
};
use crate::hardware_id;
use crate::hooks::{detect_hook_framework, install_hook, uninstall_hook, HookFramework};
use crate::sessions::{
    codex_detail, detail, grok_detail, list_paged, pi_detail, scan, ClaudeCliSource,
    CodexSessionSource, GrokSessionSource, PiSessionSource, SessionDetail, SessionPage,
    SessionSource, CLAUDE_SOURCE_ID, CODEX_SOURCE_ID, GROK_SOURCE_ID, PI_SOURCE_ID,
};
use crate::sync::{
    classify_sync_error, is_dashboard_state_repo, resolve_status, ConfigPullErrorKind,
    ConflictReason, DashboardStateRepo, Event, GitRemoteMismatch, Snapshot, SnapshotProject,
    StateRepoStatus, Trigger, CURRENT_SNAPSHOT_SCHEMA_VERSION,
};
use crate::tracker::{
    classify_url_binding, collect, detect_git_filter_repo, get_git_remote_url, normalize_git_url,
    NormalizeError, ProjectStatus, UrlBinding,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug)]
struct LocalConfigLock {
    _file: std::fs::File,
}

/// `local.yaml` 的锁只由其固定路径决定，不能跟随可变的 dashboard-state root。
/// 所有 read→modify→write 入口先拿这把锁，再拿各自的数据仓锁，避免切根期间
/// 两个调用持不同 repo sidecar 锁却并发覆盖同一份本机配置。
fn acquire_local_config_lock(
    local_path: &std::path::Path,
    timeout: std::time::Duration,
) -> Result<LocalConfigLock, CommandError> {
    let absolute = std::path::absolute(local_path).map_err(CommandError::Io)?;
    let mut lock_name = absolute
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("local.yaml"));
    lock_name.push(".write.lock");
    let lock_path = absolute
        .parent()
        .map(|parent| parent.join(&lock_name))
        .unwrap_or_else(|| absolute.join(lock_name));
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(CommandError::Io)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(CommandError::Io)?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(LocalConfigLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if started.elapsed() >= timeout {
                    return Err(CommandError::Sync(crate::sync::SyncError::LockBusy));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(CommandError::Io(error));
            }
        }
    }
}

// -------- Session Board --------

/// v1 分页版：SessionPage 直接透传给前端（total / page / page_size / items）。
/// SessionSource 抽象层在 sessions.rs，v1 只装配 ClaudeCliSource；v2+ 加多源无痛。
pub fn list_sessions_paged(query: Option<&str>, page: usize, page_size: usize) -> SessionPage {
    let source = ClaudeCliSource::default_paths();
    let sources: Vec<&dyn SessionSource> = vec![&source];
    list_paged(&sources, query, page, page_size)
}

/// 产品入口：来源必须显式选择且 fail-closed，避免同 SID 串源或生成错误的恢复命令。
pub fn list_sessions_paged_for_source(
    source_id: &str,
    query: Option<&str>,
    page: usize,
    page_size: usize,
) -> Result<SessionPage, CommandError> {
    if crate::partner_sessions::PARTNER_SOURCES.contains(&source_id) {
        crate::session_storage::default_storage().ensure_initialized()?;
        let source = crate::partner_sessions::PartnerSessionSource::default_paths(source_id)?;
        return Ok(crate::sessions::paginate_metas(
            source.try_scan()?,
            &crate::session_storage::default_storage().load_favorites()?,
            query,
            page,
            page_size,
            false,
        ));
    }
    match source_id {
        CLAUDE_SOURCE_ID => {
            let source = ClaudeCliSource::default_paths();
            let sources: Vec<&dyn SessionSource> = vec![&source];
            Ok(list_paged(&sources, query, page, page_size))
        }
        CODEX_SOURCE_ID => {
            let source = CodexSessionSource::default_paths();
            let sources: Vec<&dyn SessionSource> = vec![&source];
            Ok(list_paged(&sources, query, page, page_size))
        }
        PI_SOURCE_ID => {
            let source = PiSessionSource::default_paths();
            let sources: Vec<&dyn SessionSource> = vec![&source];
            Ok(list_paged(&sources, query, page, page_size))
        }
        GROK_SOURCE_ID => {
            let source = GrokSessionSource::default_paths();
            let sources: Vec<&dyn SessionSource> = vec![&source];
            Ok(list_paged(&sources, query, page, page_size))
        }
        other => Err(CommandError::UnsupportedSessionSource(other.to_string())),
    }
}

/// 带显示范围的产品入口；旧 list_paged 的签名和越界语义保持不变。
pub fn list_sessions_for_view(
    source_id: &str,
    query: Option<&str>,
    page: usize,
    page_size: usize,
    visibility: Option<&str>,
) -> Result<SessionPage, CommandError> {
    use crate::session_visibility::{list_visible_paged, Visibility};
    crate::session_storage::validate_source(source_id)?;
    let visibility: Visibility = visibility.unwrap_or("visible").parse()?;
    let storage = crate::session_storage::default_storage();
    let source: Box<dyn SessionSource> = match source_id {
        CLAUDE_SOURCE_ID => Box::new(ClaudeCliSource::default_paths()),
        CODEX_SOURCE_ID => Box::new(CodexSessionSource::default_paths()),
        PI_SOURCE_ID => Box::new(PiSessionSource::default_paths()),
        GROK_SOURCE_ID => Box::new(GrokSessionSource::default_paths()),
        id => Box::new(crate::partner_sessions::PartnerSessionSource::default_paths(id)?),
    };
    Ok(list_visible_paged(
        source.as_ref(),
        storage,
        query,
        page,
        page_size,
        visibility,
    )?)
}

pub fn set_session_hidden(
    source: &str,
    session_id: &str,
    hidden: bool,
) -> Result<crate::session_visibility::HiddenSessionUpdate, CommandError> {
    Ok(crate::session_storage::default_storage().set_hidden(source, session_id, hidden)?)
}

pub fn session_detail_by_id(session_id: &str) -> Option<SessionDetail> {
    let metas = scan();
    let meta = metas.into_iter().find(|m| m.session_id == session_id)?;
    // rounds=5：最后 5 轮 user→assistant，跟前端 UI 文案对齐
    Some(detail(&meta, 5))
}

pub fn session_detail_by_source(
    source_id: &str,
    session_id: &str,
) -> Result<Option<SessionDetail>, CommandError> {
    if !crate::session_storage::SOURCES.contains(&source_id) {
        return Err(CommandError::UnsupportedSessionSource(source_id.to_owned()));
    }
    crate::session_storage::validate_id(session_id)?;
    crate::session_storage::default_storage().ensure_initialized()?;
    if crate::partner_sessions::PARTNER_SOURCES.contains(&source_id) {
        return Ok(
            crate::partner_sessions::PartnerSessionSource::default_paths(source_id)?
                .detail(session_id)?,
        );
    }
    match source_id {
        CLAUDE_SOURCE_ID => Ok(session_detail_by_id(session_id)),
        CODEX_SOURCE_ID => {
            let source = CodexSessionSource::default_paths();
            let meta = source.find_by_id(session_id);
            Ok(meta.map(|value| codex_detail(&value, 5)))
        }
        PI_SOURCE_ID => {
            let source = PiSessionSource::default_paths();
            let meta = source.find_by_id(session_id);
            Ok(meta.map(|value| pi_detail(&value, 5)))
        }
        GROK_SOURCE_ID => {
            let source = GrokSessionSource::default_paths();
            let meta = source.find_by_id(session_id);
            Ok(meta.map(|value| grok_detail(&value, 5)))
        }
        other => Err(CommandError::UnsupportedSessionSource(other.to_string())),
    }
}

pub fn copy_resume_command(session_id: &str) -> String {
    format!("claude --resume {session_id}")
}

pub fn copy_resume_command_for_source(
    source_id: &str,
    session_id: &str,
) -> Result<String, CommandError> {
    if !crate::session_storage::SOURCES.contains(&source_id) {
        return Err(CommandError::UnsupportedSessionSource(
            source_id.to_string(),
        ));
    }
    if session_id.is_empty()
        || !session_id
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || session_id.len() > 128
        || !session_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
    {
        return Err(CommandError::InvalidSessionId);
    }
    match source_id {
        CLAUDE_SOURCE_ID => Ok(copy_resume_command(session_id)),
        CODEX_SOURCE_ID => Ok(format!("codex resume {session_id}")),
        // pi 的 `--resume/-r` 是交互式选择器不接 id，恢复指定会话只能用 `--session`
        PI_SOURCE_ID => Ok(format!("pi --session {session_id}")),
        // grok 的 `--resume` 传非 UUID 会按标题模糊匹配，这里传的是 UUID 故走精确 ID 分支
        GROK_SOURCE_ID => Ok(format!("grok --resume {session_id}")),
        "opencode" => Ok(format!("opencode --session {session_id}")),
        "antigravity" => Ok(format!("agy --conversation {session_id}")),
        "dsh" | "zcode" | "gemini-cli" => Err(CommandError::ResumeUnsupported),
        _ => unreachable!("source allowlist 已在上方校验"),
    }
}

// -------- 项目管理 --------

/// v4.2 Decision 15：`pending` 字段已删除（UUID 权威在 dashboard-state registry，无 pending 中转）。
#[derive(Debug, Serialize)]
pub struct ProjectListResponse {
    pub shared: Vec<RegistryProject>,
    pub private: Vec<PrivateProject>,
}

pub fn list_projects(dashboard_state_root: &std::path::Path) -> Result<ProjectListResponse, CommandError> {
    let registry = Registry::load_default(dashboard_state_root)?;
    Ok(ProjectListResponse {
        shared: registry.list_shared().into_iter().cloned().collect(),
        private: registry.list_private().into_iter().cloned().collect(),
    })
}

#[derive(Debug, Deserialize)]
pub struct AddProjectRequest {
    pub name: String,
    pub git_url: String,
    pub path: String,
    pub machines: BTreeMap<String, String>,
    pub sync: bool,
    /// Q1-C：Bitbucket 双形态显式确认信号。默认 false（旧前端/未传字段反序列化不失败）。
    #[serde(default)]
    pub confirm_dual_form: bool,
}

/// 新增项目（v4.2 Decision 15）。
///
/// sync:true 流程：
/// 1. **前置** —— 调用方（Tauri command / CLI）保证已成功 `dashboard-state pull --rebase`；
///    pull 失败则拒绝执行本函数（pull-first 硬约束由上层保证）
/// 2. normalize `git_url`
/// 3. 用 `find_project_id_by_git_url` 反查 registry：
///    - 唯一命中 → 追加 `machines[<machine_id>]` 到已有条目（"关联"路径），
///      machine_id 重复 → `MachineAlreadyRegistered` 错误
///    - 多命中 → 拒绝保存（URL→UUID 全局唯一 invariant 违反），
///      前端 Fleet Board 顶部弹红色告警提示删除冗余 UUID
///    - 未命中 → 生成新 UUID 直接写 registry（含 `git_url` + `git_url_aliases: []`）
///
/// sync:false 流程：直接写 `local.yaml.private_projects`（未参与跨机反查）。
///
/// v4.2 撤销的 v4.1 行为：不再写项目根 `.dashboard-project-id`；不再走
/// `pending_sync_projects` 暂留（UUID 权威已迁 dashboard-state，无需等 commit 确认）。
///
/// 返回的 `AddProjectResult::requires_commit_hint` 现固定为 false —— 用户项目仓不需要 commit 任何东西。
pub fn add_project(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    req: AddProjectRequest,
) -> Result<AddProjectResult, CommandError> {
    let _local_lock = acquire_local_config_lock(local_path, std::time::Duration::from_secs(5))?;
    // local.yaml 先用固定路径锁串行；再用 repo-global 锁保护 registry/git。CRUD 写 registry
    // yaml（save_at 逐文件 fs::write 非原子）+ 整份重写 local.yaml，与采集的
    // git add -A / 锁内读解析 local.yaml 交叉会提交半写 yaml 或读到半截 —— 纳入同一把 repo-global
    // sidecar 锁。宽松版：first-run 尚未 clone（无 .git）时也真正上锁，堵住 first-run 下
    // CRUD↔CRUD 对 local.yaml 的并发覆盖（sidecar 锁不依赖 .git，无 clone 过渡期 TOCTOU）。
    let repo = DashboardStateRepo::new(dashboard_state_root, "_crud_only_");
    let _lock = repo.acquire_lock_allow_uninit(std::time::Duration::from_secs(5))?;
    // Q2-B 门禁：sync:true 共享项目要求存储仓就绪（NeedsInit → 锁内 clone）；sync:false 私有项目
    // 不出机、不碰存储仓，放行（防"配 remote 需先有 remote"死锁）。门禁在锁内做、不重入锁。
    if req.sync {
        let gate = read_remote_gate(local_path);
        gate_shared_crud(&repo, gate.remote.as_deref(), gate.cleared)?;
    }
    let normalized = normalize_git_url(&req.git_url)?;
    let mut registry = Registry::load_at(dashboard_state_root, local_path)?;
    if registry.local.is_none() {
        registry.local = Some(LocalConfig::new(compute_machine_id()?));
    }
    let now = Utc::now();

    if req.sync {
        let machine_id = resolve_machine_id(registry.local.as_ref())?;

        // §16.3 反查 registry —— pull-first 由调用方保证
        match find_project_id_by_git_url(&registry, &normalized) {
            Ok(uuid) => {
                // 关联路径：追加本机 machines 条目到已有 shared 项目
                registry.associate_machine(&uuid, &machine_id, req.path.clone())?;
                registry.save_at(dashboard_state_root, local_path)?;
                Ok(AddProjectResult {
                    project_id: Some(uuid),
                    requires_commit_hint: false,
                    git_url_normalized: normalized,
                })
            }
            Err(UuidLookupError::NotFound) => {
                // Q1-C：未命中 = 新增 sync 项目。读本地真实 origin 做 URL 绑定校验四路（防选错项目目录）。
                // Q1-C 审计收紧 fail-open：区分"有 origin 但归一化失败"与"无 origin"——
                //   - 有 origin 且归一化成功 → 进四路 classify
                //   - 有 origin 但归一化失败（格式坏）→ 视为与手填无关，直接拒绝（不回退，防把 A 目录挂 B 地址）
                //   - None（git 仓无 remote / 非 git 目录 / git 读取失败）→ 回退手填作 primary（新 init 合法场景，
                //     且后续采集防伪 classify_remote 会兜底 mismatch）
                let raw_origin = get_git_remote_url(std::path::Path::new(&req.path));
                let origin_norm = match &raw_origin {
                    Some(raw) => match normalize_git_url(raw) {
                        Ok(o) => Some(o),
                        Err(_) => {
                            return Err(CommandError::OriginUnrelated {
                                actual_origin: raw.clone(),
                                input: normalized.clone(),
                            });
                        }
                    },
                    None => None,
                };
                let (primary, aliases) =
                    match classify_url_binding(origin_norm.as_deref(), &normalized) {
                        // 一致 / 无 origin → primary = 手填归一化值（一致时它逐字节等于 origin），无 alias
                        UrlBinding::Consistent | UrlBinding::NoOrigin => {
                            (normalized.clone(), Vec::new())
                        }
                        // Bitbucket 双形态：须用户显式确认；未确认 → 返回机器可识别的"需确认"错误，不写 registry
                        UrlBinding::BitbucketDualForm => {
                            if !req.confirm_dual_form {
                                return Err(CommandError::NeedDualFormConfirm {
                                    origin: origin_norm.unwrap_or_default(),
                                    input: normalized.clone(),
                                });
                            }
                            // 确认后：primary = 真实 origin（clone 形式），手填 canonical 进 aliases（跨机配对锚点）
                            let origin =
                                origin_norm.expect("BitbucketDualForm 分类必有 origin");
                            (origin, vec![normalized.clone()])
                        }
                        // 完全无关 → 拒绝保存（防选错项目目录），机器可识别错误，不写 registry
                        UrlBinding::Unrelated => {
                            return Err(CommandError::OriginUnrelated {
                                actual_origin: origin_norm.unwrap_or_default(),
                                input: normalized.clone(),
                            });
                        }
                    };
                let uuid = Uuid::new_v4();
                let mut machines = req.machines.clone();
                // 保证请求方给的 machines 里包含本机（若前端漏传）
                machines
                    .entry(machine_id.clone())
                    .or_insert_with(|| req.path.clone());
                registry.add_shared(RegistryProject {
                    schema_version: CURRENT_SCHEMA_VERSION,
                    project_id: uuid,
                    name: req.name.clone(),
                    git_url: primary.clone(),
                    git_url_aliases: aliases,
                    sync: true,
                    machines,
                    created_at: now,
                    updated_at: now,
                })?;
                registry.save_at(dashboard_state_root, local_path)?;
                Ok(AddProjectResult {
                    project_id: Some(uuid),
                    requires_commit_hint: false,
                    git_url_normalized: primary,
                })
            }
            Err(UuidLookupError::MultipleMatch { url, uuids }) => {
                Err(CommandError::InvalidSettings(format!(
                    "URL→UUID 冲突：{url} 已在 registry 命中多个项目 [{}]，请去 Fleet Board 删除冗余 UUID 后重试",
                    uuids
                        .iter()
                        .map(|u| u.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
            Err(UuidLookupError::Normalize(e)) => Err(CommandError::Normalize(e)),
        }
    } else {
        registry.add_private(PrivateProject {
            schema_version: CURRENT_SCHEMA_VERSION,
            name: req.name.clone(),
            project_id: None,
            path: req.path.clone(),
            git_url: normalized.clone(),
            sync: false,
            created_at: now,
            updated_at: now,
        })?;
        // 决策 5（B2 变种洗白根治）：私有项目只写 local.yaml，**不**走 save_at——后者的
        // create_dir_all(registry/projects) 会在错仓凭空造合法结构、洗白 InvalidStructure 门禁。
        registry.save_local_only(local_path)?;
        Ok(AddProjectResult {
            project_id: None,
            requires_commit_hint: false,
            git_url_normalized: normalized,
        })
    }
}

/// v4.2 新增：前端"新增项目"对话框保存前调用，反查 registry 判断是"关联已有 UUID"还是"新建 UUID"。
/// 前置：调用方保证已成功 pull dashboard-state（pull-first 硬约束）。
pub fn find_project_by_git_url(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    git_url: &str,
) -> Result<FindProjectResult, CommandError> {
    let registry = Registry::load_at(dashboard_state_root, local_path)?;
    let normalized = normalize_git_url(git_url)?;
    match find_project_id_by_git_url(&registry, &normalized) {
        Ok(uuid) => {
            let reg = registry
                .get_shared(&uuid)
                .expect("find_project_id_by_git_url returned existing uuid");
            Ok(FindProjectResult::Existing {
                project_id: uuid,
                name: reg.name.clone(),
                git_url_normalized: normalized,
                machines: reg.machines.keys().cloned().collect(),
            })
        }
        Err(UuidLookupError::NotFound) => Ok(FindProjectResult::NotFound {
            git_url_normalized: normalized,
        }),
        Err(UuidLookupError::MultipleMatch { url, uuids }) => {
            Ok(FindProjectResult::Conflict { url, uuids })
        }
        Err(UuidLookupError::Normalize(e)) => Err(CommandError::Normalize(e)),
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FindProjectResult {
    /// URL 未在 registry 命中 —— 前端提示"将新建 UUID"
    NotFound { git_url_normalized: String },
    /// 唯一命中 —— 前端弹"已在 <machines> 注册，是否关联本机路径？" modal
    Existing {
        project_id: Uuid,
        name: String,
        git_url_normalized: String,
        machines: Vec<String>,
    },
    /// URL→UUID 冲突 —— 前端阻塞保存 + 提示删除冗余
    Conflict { url: String, uuids: Vec<Uuid> },
}

#[derive(Debug, Serialize)]
pub struct AddProjectResult {
    pub project_id: Option<Uuid>,
    pub requires_commit_hint: bool,
    pub git_url_normalized: String,
}

/// v4.2 §16.4 pull-first 硬约束的执行入口：前端在新增 / 关联 / 更新 URL 前必须先调此 IPC。
/// 失败 → 前端禁用保存按钮 + toast 显示错误；成功 → 保证后续 `find_project_by_git_url` /
/// `add_project` 读到的 registry 是最新的（覆盖双机同时新增同 URL race 场景）。
pub fn pull_registry(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
) -> Result<(), CommandError> {
    // Q2-B：pull-first 入口 = 配置即拉的特例。Ready → git_pull 拉最新（原行为）；NeedsInit → clone
    // 初始化；Unconfigured（未配 remote 且无本地仓）→ no-op（无远端可 pull，不再 strict-lock NotFound
    // 报错）；Conflict → Err 引导手动处理。门禁 + clone/pull 共用 trigger_config_pull 内的宽松锁。
    let gate = read_remote_gate(local_path);
    trigger_config_pull(
        dashboard_state_root,
        gate.remote.as_deref(),
        gate.cleared,
        true,
    )?;
    Ok(())
}

// -------- 存储仓配置即拉 + 状态门禁（Q2-B）--------

/// 出机门禁的一次性输入：remote + 是否显式清空。**一次原子读 local.yaml 同时产出二者**
/// （避免分两次读的 TOCTOU）。
struct RemoteGate {
    remote: Option<String>,
    cleared: bool,
}

/// 一次原子读 local.yaml，产出出机门禁输入 `{remote, cleared}`（决策 2 fail-closed + 收敛错误分类）。
///
/// - 成功解析：`{cfg.dashboard_state_remote, cfg.dashboard_state_remote_cleared}`
///   （老 yaml 无 cleared 字段 → serde 默认 false = "存量从没配过 remote"，续同步兼容）。
/// - 任何读不出（NotFound / 权限拒绝 / YAML 损坏 / IO 错误）→ `{None, cleared: true}`：
///   统一当"未就绪"，让 `resolve_status` 判 `Unconfigured`、**绝不沿旧 origin push**（安全底线，
///   不做精细错误分类）。首启（无 local.yaml 且无本地仓）虽也标 cleared=true，但 `resolve_status`
///   的无 `.git` 分支不看 cleared，仍走 `Unconfigured` no-op，无副作用。
fn read_remote_gate(local_path: &std::path::Path) -> RemoteGate {
    match std::fs::read_to_string(local_path) {
        Ok(text) => match serde_yaml::from_str::<LocalConfig>(&text) {
            Ok(cfg) => RemoteGate {
                remote: cfg.dashboard_state_remote,
                cleared: cfg.dashboard_state_remote_cleared,
            },
            // YAML 损坏 → fail-closed（不沿旧 origin push）
            Err(_) => RemoteGate {
                remote: None,
                cleared: true,
            },
        },
        // NotFound / 权限拒绝 / IO 错误 → fail-closed
        Err(_) => RemoteGate {
            remote: None,
            cleared: true,
        },
    }
}

/// Conflict 态 → 面向用户的错误。RemoteMismatch / NotGitDir 都引导手动删目录重配（不静默覆盖）。
fn conflict_to_error(reason: ConflictReason) -> CommandError {
    match reason {
        ConflictReason::NotGitDir => CommandError::InvalidSettings(
            "存储仓数据目录非空但不是 git 仓（可能是残留数据）；请手动清理该目录后重试".to_string(),
        ),
        // LEAK-06（孪生点）：`actual` 来自 `git remote get-url origin`，**未过 reject_inline_token**。
        // 用户若曾用带凭证 URL 手工 clone 过存储仓，actual 可能含 user:token，原样回显会把凭证
        // 暴露到 UI/截图/日志。只回显 configured（来自 local.yaml，已过 reject 校验，安全），
        // 不带 actual 原文；提示用户本地 origin 可能含凭证、删除前自行确认。
        ConflictReason::RemoteMismatch { configured, .. } => CommandError::InvalidSettings(format!(
            "存储仓本地 origin 与配置地址（{configured}）不一致（可能本机曾用其他地址 clone 过存储仓）；\
             请手动删除数据目录后重新配置。注意：本地 origin 可能含敏感凭证，删除前请自行确认，此处不展示。"
        )),
        // 决策 3：有 .git 但非合法 dashboard-state 结构（无 registry/projects）→ 复用稳定 tag
        // INVALID_STRUCTURE（前端已识别，零改），引导用户确认地址 / 删目录重配。
        ConflictReason::InvalidStructure => CommandError::ConfigPullFailed {
            kind: ConfigPullErrorKind::InvalidStructure,
        },
    }
}

/// 非 Ready 的同步状态 → 错误（同步入口锁内二次校验用；`Unconfigured` 由调用方单独 no-op，
/// 此处只映射 `Conflict` / `NeedsInit`）。
fn sync_state_to_error(status: StateRepoStatus) -> CommandError {
    match status {
        StateRepoStatus::Conflict(reason) => conflict_to_error(reason),
        StateRepoStatus::NeedsInit => CommandError::InvalidSettings(
            "存储仓在同步期间状态变化（尚未初始化），请重试".to_string(),
        ),
        StateRepoStatus::Unconfigured => {
            CommandError::InvalidSettings("尚未配置存储仓地址".to_string())
        }
        StateRepoStatus::Ready => unreachable!("Ready 不应转错误"),
    }
}

/// 配置即拉（同步入口锁前 / tauri 层保存 remote 后调用）：**自拿宽松锁**（spec"检查状态前先获取
/// 锁"），observe → resolve_status → 据态动作，返回最终状态供调用方决定 no-op / 继续 / 报错。
/// - `NeedsInit`（配了 remote、本地空 / 不存在）→ `git_clone` 初始化；
/// - `Ready` 且 `pull_if_ready` → `git_pull`（fetch + ff-only，拉最新；保存 remote 后主动同步）；
/// - `Conflict` → 返回 Err（数据冲突，引导手动处理）；
/// - `Unconfigured` → 原样返回（调用方对 sync:true 同步入口安全 no-op）。
///
/// **必须在未持锁时调用**（内部自拿宽松锁，重入同一 sidecar 锁会自锁超时）。异步执行（后台
/// 拉取、UI 三态）由 tauri 层包一层线程/任务，本函数只负责同步的判态 + clone/pull。
pub fn trigger_config_pull(
    dashboard_state_root: &std::path::Path,
    configured_remote: Option<&str>,
    remote_cleared: bool,
    pull_if_ready: bool,
) -> Result<StateRepoStatus, CommandError> {
    let repo = DashboardStateRepo::new(dashboard_state_root, "_config_pull_");
    let _lock = repo.acquire_lock_allow_uninit(std::time::Duration::from_secs(5))?;
    let status = resolve_status(&repo.observe(), configured_remote, remote_cleared);
    match &status {
        StateRepoStatus::NeedsInit => {
            let remote = configured_remote
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .expect("NeedsInit 必有配置 remote");
            // 决策 3：clone + 结构校验 + 失败清理一体（clone 出普通仓 / 空仓时按 root 原状态两分支
            // 清理产物、返 InvalidStructure，不留残仓污染下次判态）。
            clone_then_verify(&repo, remote)?;
        }
        StateRepoStatus::Ready if pull_if_ready => {
            // pull-first 要求工作树干净。land-only 采集（`collect` 不带 `--push`：写盘但不提交）
            // 与身份迁移都会往受管目录写文件，留下的脏工作树会让 git 拒绝 pull —— 用户若配了
            // `pull.rebase=true`，走的 rebase 路径更是要求工作树完全干净，此后所有同步全线失败，
            // 且 UI 只显示 `[UNKNOWN] 同步失败`，完全看不出跟上一次采集有关。
            commit_pending_managed_changes(&repo)?;
            repo.git_pull().map_err(|e| CommandError::ConfigPullFailed {
                kind: classify_sync_error(&e),
            })?;
            verify_pulled_structure(dashboard_state_root)?;
        }
        StateRepoStatus::Conflict(reason) => return Err(conflict_to_error(reason.clone())),
        _ => {}
    }
    Ok(status)
}

/// pull 前收拢受管目录（`registry/` + `machines/`）里的未提交改动，收进一个本地提交，**不 push**
/// （推送仍由后续 `sync_cycle` 负责）。
///
/// 白名单沿用 `git_add_all`：数据目录根可由用户配置，若配成含 `local.yaml` / `.env` / 密钥的目录，
/// 这里也不会把它们裹进提交。
///
/// 无受管改动时静默跳过。提交失败按 config pull 失败上报 —— 不报的话紧接着的 pull 必然失败，
/// 而那个错误信息（`cannot pull with rebase: You have unstaged changes`）指向不了真正的起因。
pub fn commit_pending_managed_changes(repo: &DashboardStateRepo) -> Result<(), CommandError> {
    let to_config_error = |e: &crate::sync::SyncError| CommandError::ConfigPullFailed {
        kind: classify_sync_error(e),
    };
    repo.git_add_all().map_err(|e| to_config_error(&e))?;
    if !repo.git_has_staged_changes().map_err(|e| to_config_error(&e))? {
        return Ok(());
    }
    repo.git_commit("sync: 收拢同步前的未提交受管改动")
        .map_err(|e| to_config_error(&e))
}

/// clone/pull 成功后校验：拉到的是真 dashboard-state 仓（含 registry/projects），否则归
/// `InvalidStructure`（用户填了别的 git 仓地址），前端提示确认地址。`machines/` 按 spec 首次同步才建，不硬校验。
fn verify_pulled_structure(root: &std::path::Path) -> Result<(), CommandError> {
    if is_dashboard_state_repo(root) {
        Ok(())
    } else {
        Err(CommandError::ConfigPullFailed {
            kind: ConfigPullErrorKind::InvalidStructure,
        })
    }
}

/// clone + 结构校验 + 失败清理一体（决策 3.4）。仅在 NeedsInit（root 空 / 不存在）后调用：clone
/// 成功但拉到的不是合法 dashboard-state 结构（无 registry/projects，即用户填错地址指向普通仓 /
/// 空仓）时，按 clone 前 root 是否存在两分支清理本次 clone 产物、返回 InvalidStructure，使下次可重新
/// 判为待初始化、不留残仓（`.git`）污染判态。
/// - root 原不存在 → 删整个新建 root；
/// - root 原为空目录 → 删 clone 产物后重建空目录（维持"存在但空"，符合 NeedsInit 语义）。
///
/// 「root 原非空」由 NeedsInit 前置门禁（root_empty 才判 NeedsInit→clone）挡在 clone 之前，不进此分支，
/// 故只需两分支，与 `git_clone` 失败清理同款不变量，不误删用户预创建目录。
fn clone_then_verify(repo: &DashboardStateRepo, remote: &str) -> Result<(), CommandError> {
    let root_existed_before = repo.root.exists();
    // clone 失败已在 git_clone 内部清理（只删本次新建 root）
    repo.git_clone(remote)
        .map_err(|e| CommandError::ConfigPullFailed {
            kind: classify_sync_error(&e),
        })?;
    if is_dashboard_state_repo(&repo.root) {
        Ok(())
    } else {
        // clone 成功但结构非法 → 清理本次产物，避免 .git 残留让下次 observe 误判为 Ready / Conflict
        let _ = std::fs::remove_dir_all(&repo.root);
        if root_existed_before {
            let _ = std::fs::create_dir_all(&repo.root);
        }
        Err(CommandError::ConfigPullFailed {
            kind: ConfigPullErrorKind::InvalidStructure,
        })
    }
}

/// 共享 CRUD 门禁（add/edit/delete_project 用，**在已持有宽松锁的临界区内**调用，不重入锁）：
/// observe → resolve_status → sync:true 共享操作要求存储仓就绪。
/// - `Ready` → 放行；`NeedsInit` → 锁内 `git_clone` 后放行；
/// - `Unconfigured` → Err"尚未配置存储仓地址"（用户主动操作，明确提示去配，非静默 no-op）；
/// - `Conflict` → Err（引导手动处理）。
///
/// sync:false 私有项目不碰存储仓（写 local.yaml.private_projects），调用方不应对其调本门禁。
fn gate_shared_crud(
    repo: &DashboardStateRepo,
    configured_remote: Option<&str>,
    remote_cleared: bool,
) -> Result<(), CommandError> {
    match resolve_status(&repo.observe(), configured_remote, remote_cleared) {
        StateRepoStatus::Ready => Ok(()),
        StateRepoStatus::NeedsInit => {
            let remote = configured_remote
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .expect("NeedsInit 必有配置 remote");
            // 决策 3：clone + 结构校验 + 失败清理一体（与 trigger_config_pull 对称）；填错地址指向
            // 普通仓时清理产物 + 返 InvalidStructure，不被后续 CRUD 写成半个 dashboard-state。
            clone_then_verify(repo, remote)
        }
        StateRepoStatus::Unconfigured => Err(CommandError::InvalidSettings(
            "尚未配置存储仓地址，请先在设置页配置存储仓地址后再新增 / 编辑 / 删除共享项目".to_string(),
        )),
        StateRepoStatus::Conflict(reason) => Err(conflict_to_error(reason)),
    }
}

#[derive(Debug, Deserialize)]
pub struct EditProjectRequest {
    pub project_id: Uuid,
    pub name: Option<String>,
    pub git_url: Option<String>,
    pub machines: Option<BTreeMap<String, String>>,
    // sync 字段刻意不允许 —— spec 硬约束
}

pub fn edit_project(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    req: EditProjectRequest,
) -> Result<(), CommandError> {
    let _local_lock = acquire_local_config_lock(local_path, std::time::Duration::from_secs(5))?;
    // local.yaml 固定路径锁防切换 root 后锁分裂；repo-global sidecar 锁继续保护 registry/git（防与采集 git add -A /
    // 锁内读 local 交叉）；宽松版：first-run 未 clone 时也真正上锁（堵 first-run CRUD 竞争）。
    let repo = DashboardStateRepo::new(dashboard_state_root, "_crud_only_");
    let _lock = repo.acquire_lock_allow_uninit(std::time::Duration::from_secs(5))?;
    // Q2-B 门禁：edit_shared 只改共享 registry，属 sync:true 操作 —— 无条件要求存储仓就绪。
    let gate = read_remote_gate(local_path);
    gate_shared_crud(&repo, gate.remote.as_deref(), gate.cleared)?;
    let mut registry = Registry::load_at(dashboard_state_root, local_path)?;
    let git_url = match req.git_url {
        Some(u) => Some(normalize_git_url(&u)?),
        None => None,
    };
    registry.edit_shared(
        &req.project_id,
        EditRegistryProject {
            name: req.name,
            git_url,
            machines: req.machines,
        },
    )?;
    registry.save_at(dashboard_state_root, local_path)?;
    Ok(())
}

pub fn delete_project(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    project_id: Uuid,
) -> Result<(), CommandError> {
    let _local_lock = acquire_local_config_lock(local_path, std::time::Duration::from_secs(5))?;
    // local.yaml 固定路径锁防切换 root 后锁分裂；repo-global sidecar 锁继续保护 registry/git（防与采集 git add -A /
    // 锁内读 local 交叉）；宽松版：first-run 未 clone 时也真正上锁（堵 first-run CRUD 竞争）。
    let repo = DashboardStateRepo::new(dashboard_state_root, "_crud_only_");
    let _lock = repo.acquire_lock_allow_uninit(std::time::Duration::from_secs(5))?;
    // Q2-B 门禁：delete_shared 只删共享 registry，属 sync:true 操作 —— 无条件要求存储仓就绪。
    let gate = read_remote_gate(local_path);
    gate_shared_crud(&repo, gate.remote.as_deref(), gate.cleared)?;
    let mut registry = Registry::load_at(dashboard_state_root, local_path)?;
    registry.delete_shared(&project_id)?;
    registry.save_at(dashboard_state_root, local_path)?;
    Ok(())
}

// -------- Hook 状态 --------

#[derive(Debug, Serialize)]
pub struct HookStatus {
    pub project_id: Uuid,
    pub state: HookState,
    pub framework_warning: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HookState {
    On,
    Off,
    Error,
}

pub fn install_project_hook(
    project_path: &std::path::Path,
    fleet_binary: &std::path::Path,
    project_id: &str,
    log_path: &std::path::Path,
) -> Result<Option<HookFramework>, CommandError> {
    let framework = detect_hook_framework(project_path);
    install_hook(project_path, fleet_binary, project_id, log_path).map_err(CommandError::Io)?;
    Ok(framework)
}

pub fn uninstall_project_hook(project_path: &std::path::Path) -> Result<bool, CommandError> {
    uninstall_hook(project_path).map_err(CommandError::Io)
}

// -------- Fleet Board --------

pub fn aggregate_fleet_board(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
) -> Result<Vec<ProjectAggregate>, CommandError> {
    let registry = Registry::load_at(dashboard_state_root, local_path)?;
    let aggs = aggregate_from_dashboard_state(
        dashboard_state_root,
        &registry,
        Utc::now(),
        DEFAULT_STALE_THRESHOLD,
    )
    .map_err(CommandError::Aggregate)?;
    Ok(aggs)
}

// -------- 本机 machine_id 契约 --------

/// 计算本机身份（**不读 local.yaml**），形如 `mbp-00000001`。
///
/// 取值顺序：
/// 1. 环境变量 `DASHBOARD_MACHINE_ID` —— 逃生舱，硬件标识取不到时的人工兜底；
/// 2. 硬件标识哈希 + 主机名推出的短前缀。
///
/// **取不到硬件标识返回 Err，绝不编造假身份。** 旧实现在这里兜底成 `unknown-machine`
/// 并照样往云端写快照，结果 `machines/` 下永久多出一个谁都不认的孤儿目录（实测已存在）。
/// 宁可当次操作失败，也不能污染跨机数据。
///
/// 审计对账 P0-D（2026-07-06）：抽出到 core，供前端 Tauri command 和 CLI 共用；
/// 前端不再硬编码 `DESKTOP-DEMO01`。
pub fn compute_machine_id() -> Result<String, hardware_id::HardwareIdError> {
    if let Some(value) = env_machine_id() {
        return Ok(value);
    }
    let hash = hardware_id::hardware_hash()?;
    let prefix = hardware_id::derive_prefix(&hardware_id::current_hostname());
    Ok(hardware_id::compose_machine_id(&prefix, &hash))
}

/// 人工指定的身份（逃生舱）。空值视作未设置。
fn env_machine_id() -> Option<String> {
    let value = std::env::var("DASHBOARD_MACHINE_ID").ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 解析本机身份：**local.yaml 里冻结的值优先**，缺失才计算。
///
/// 冻结是这套方案的关键 —— 身份一旦落盘就不再随主机名或前缀推导规则变化，
/// 即便日后改了前缀推导逻辑，存量机器的身份也不会跟着变、云端目录不会裂。
pub fn resolve_machine_id(
    local: Option<&LocalConfig>,
) -> Result<String, hardware_id::HardwareIdError> {
    // 环境变量是最高优先级的逃生舱：它要能压过已冻结的值，否则一旦冻错就再也扳不回来。
    if let Some(value) = env_machine_id() {
        return Ok(value);
    }
    if let Some(config) = local {
        let frozen = config.machine_id.trim();
        if !frozen.is_empty() && !is_legacy_hostname_id(frozen) {
            return Ok(frozen.to_string());
        }
    }
    compute_machine_id()
}

/// 旧式身份（纯主机名，如 `demo-MacBook-Pro-3` / `unknown-machine`）识别。
///
/// 判据：新式身份恒为 `<前缀>-<8位十六进制>`。命中旧式则不认冻结值、重新计算，
/// 从而让存量机器在首次启动时自动切到硬件身份（迁移入口）。
pub fn is_legacy_hostname_id(machine_id: &str) -> bool {
    let Some((prefix, suffix)) = machine_id.rsplit_once('-') else {
        return true;
    };
    !(prefix.chars().all(|c| c.is_ascii_alphanumeric())
        && !prefix.is_empty()
        && suffix.len() == 8
        && suffix.chars().all(|c| c.is_ascii_hexdigit()))
}

#[derive(Debug, Serialize)]
pub struct MachineContext {
    /// 本机身份。**取不到时为 None** —— 前端必须据此报错，不得自行兜底成假 id 继续跑。
    pub machine_id: Option<String>,
    /// 取不到身份时的原因（面向用户，含补救指引）。取到则为 None。
    pub machine_id_error: Option<String>,
    /// 当次真实主机名，展示用。与 machine_id 不同，它允许为空、允许变。
    pub hostname: String,
    pub os: String,
    /// App 版本（workspace 统一版本，编译期由 CARGO_PKG_VERSION 注入），供前端侧栏底部显示
    pub version: String,
}

/// 读本机上下文。身份解析走「环境变量 → local.yaml 冻结值 → 算硬件标识」，
/// 需要 local.yaml 路径才能拿到冻结值，故与旧签名不同（旧实现只算主机名、不读配置）。
pub fn get_local_context(local_path: &std::path::Path) -> MachineContext {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unix"
    };
    // 配置读不出来不该让整个上下文失败：退化成「无冻结值」，照样能算硬件身份。
    let local = get_settings(local_path).ok();
    let (machine_id, machine_id_error) = match resolve_machine_id(local.as_ref()) {
        Ok(id) => (Some(id), None),
        Err(error) => (
            None,
            Some(format!(
                "{error}。请设置环境变量 DASHBOARD_MACHINE_ID 后重启 App（诊断：cargo run -p dashboard-core --example hwid_probe）"
            )),
        ),
    };
    MachineContext {
        machine_id,
        machine_id_error,
        hostname: hardware_id::current_hostname(),
        os: os.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// 已知机器条目 —— 供项目登记页的机器下拉用。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KnownMachine {
    /// 真正参与匹配的身份（`machines/<id>/` 目录名、注册表 machines 键）。
    pub machine_id: String,
    /// 该机最近一次上报的主机名，可能为空（老 snapshot 没这个字段）。展示给人认。
    pub hostname: String,
    pub is_local: bool,
    /// 旧式主机名身份（未迁移）。UI 需要标出来，提示用户那台机还没换硬件身份。
    pub is_legacy: bool,
}

/// 列出 dashboard-state 里见过的所有机器。
///
/// 为什么需要它：身份改成硬件哈希（`mbp-00000001`）后，用户没法凭记忆手填对方机的 id 了
/// （旧设计里 machine_id 就是主机名，所以登记页让用户手敲）。改为从这里取列表做下拉，
/// 显示主机名、值存 machine_id；从未同步过的新机器仍可手填兜底。
pub fn list_known_machines(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
) -> Vec<KnownMachine> {
    let local_id = get_settings(local_path)
        .ok()
        .and_then(|cfg| resolve_machine_id(Some(&cfg)).ok())
        .unwrap_or_default();
    let machines_dir = dashboard_state_root.join("machines");
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&machines_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(machine_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            // 主机名从该机 snapshot 里读；读不到就留空，不猜。
            let hostname = std::fs::read_to_string(entry.path().join("snapshot.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<crate::sync::Snapshot>(&text).ok())
                .map(|snapshot| snapshot.hostname)
                .unwrap_or_default();
            found.insert(machine_id, hostname);
        }
    }
    // 本机可能还没落过快照（首次启动），也要出现在列表里。
    if !local_id.is_empty() {
        found
            .entry(local_id.clone())
            .or_insert_with(hardware_id::current_hostname);
    }
    found
        .into_iter()
        .map(|(machine_id, hostname)| KnownMachine {
            is_local: machine_id == local_id,
            is_legacy: is_legacy_hostname_id(&machine_id),
            machine_id,
            hostname,
        })
        .collect()
}

// -------- 单项目采集（hook / CLI --project 用） --------

/// 只采集单个 `sync:true` 项目的状态并同步到 dashboard-state。
/// 用于 post-commit hook 只推该项目 —— **绝不触发全库采集**。
///
/// 审计对账 P0-G（2026-07-06）：原 CLI `--project <uuid>` 参数被解析但
/// 实际调用 `manual_sync()` 全量采集，跟 hook 意图不符，也放大了 P0-A 的私有泄漏面。
///
/// v4.2 Decision 15 重构（2026-07-10，四方审计后）：
/// - **采集主键从 iterator 拿**：`project_id` 参数即为 registry iterator 里的 UUID，
///   写回 `status.project_id`；`collect()` 不再从项目根读文件（§16.1）
/// - **可选防伪校验** `verify_remote`：跑 `git remote get-url origin` normalize 后与
///   registry `git_url` / `git_url_aliases` 全部候选对比；不匹配 → events 追加
///   `git_remote_mismatch` 诊断行；命令失败 → events 追加 `git_remote_failed`；
///   **snapshot 依然写入**（防伪校验不阻塞采集）
/// - hook 场景建议 `verify_remote=true`，tick 场景可 `false` 省 shell out
/// - 旧 P1-H "读 `.dashboard-project-id` 对比" 语义废弃 —— v4.2 项目根不再存 UUID 文件
pub fn collect_single(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    machine_id: &str,
    project_id: Uuid,
    trigger: Trigger,
    verify_remote: bool,
    push: bool,
) -> Result<ManualSyncResult, CommandError> {
    let store = push.then(|| sync_status_store(dashboard_state_root, local_path, machine_id));
    let mut record_attempt = true;
    let result = (|| {
        // P0-5：锁在 registry read 之前取 —— CRUD（add/edit/delete）也持同一把
        // repo 锁写 registry，采集若不前移锁会读到 CRUD 写一半 / 旧的 registry。hook 采集用长超时
        // （后台 CLI 可等，避免丢采集），锁覆盖 read registry → collect → read_snapshot → merge →
        // sync_cycle 全临界区，函数返回时 Drop 释放。
        // Q2-B 门禁 + 配置即拉（拿严格锁前，独立宽松锁单元）：NeedsInit → clone；Unconfigured
        // （未配 remote 且无本地仓）→ 安全 no-op（hook / 后台采集触发不报错）。
        let gate = read_remote_gate(local_path);
        if let StateRepoStatus::Unconfigured = trigger_config_pull(
            dashboard_state_root,
            gate.remote.as_deref(),
            gate.cleared,
            false,
        )? {
            return Ok(ManualSyncResult {
                outcome: ManualSyncOutcome::Unconfigured,
                pushed: false,
                project_count: 0,
                collected_at: Utc::now(),
                sync_record_warning: None,
            });
        }
        let repo = DashboardStateRepo::new(dashboard_state_root, machine_id);
        let _lock = repo.acquire_lock(std::time::Duration::from_secs(60))?;
        // 审计 P1：两段锁 TOCTOU —— 锁前 trigger_config_pull 判态后已释放锁，期间配置可能被
        // update_settings 改动。锁内再复检一次仍 Ready（只观察不 clone、不重入锁）：Unconfigured →
        // no-op，Conflict/NeedsInit → 中止，避免向旧仓 / 错仓写入 push。锁内重读 gate 取最新 cleared。
        let gate = read_remote_gate(local_path);
        match resolve_status(&repo.observe(), gate.remote.as_deref(), gate.cleared) {
            StateRepoStatus::Ready => {}
            StateRepoStatus::Unconfigured => {
                return Ok(ManualSyncResult {
                    outcome: ManualSyncOutcome::Unconfigured,
                    pushed: false,
                    project_count: 0,
                    collected_at: Utc::now(),
                    sync_record_warning: None,
                });
            }
            other => return Err(sync_state_to_error(other)),
        }
        let registry = Registry::load_at(dashboard_state_root, local_path)?;
        // 仅在共享 registry 里查 —— 私有项目永不出机
        let reg = registry
            .get_shared(&project_id)
            .ok_or(CommandError::Config(
                crate::config::ConfigError::ProjectNotFound(project_id),
            ))?;
        let path = reg.machines.get(machine_id).cloned().ok_or_else(|| {
            CommandError::InvalidSettings(format!(
                "项目 {} 未在 machine_id={} 上配置路径",
                reg.name, machine_id
            ))
        })?;
        let p = PathBuf::from(shellexpand(&path));
        if !p.is_dir() {
            return Err(CommandError::InvalidSettings(format!(
                "路径不存在: {}",
                p.display()
            )));
        }
        let now = Utc::now();
        let mut status = collect(&p, &reg.name);
        // v4.2: iterator UUID 是权威主键，写回 status（tracker.collect() 出的 project_id = None）
        status.project_id = Some(project_id);

        // v4.2 可选防伪校验
        let (git_remote_mismatch, git_remote_failed) = if verify_remote {
            classify_remote(&p, reg)
        } else {
            (None, None)
        };

        // v4.3 §1.4 R2-P0-B 修：read → merge → write。**绝不**用 `vec![单项]` 覆盖整机
        // snapshot（会让其他项目暂时从 Fleet Board 消失，spec `Requirement: 双文件数据模型`
        // 违背）。
        // P0-3：read_snapshot 现在区分「文件不存在」(Ok(None) → 首次采集，
        // 空 Vec 起手合法) 与「文件存在但读取/解析/校验失败」(Err)。Err 必须透传中止同步、
        // 保留磁盘原 snapshot，**绝不**退空 Vec 覆盖整机其他项目（那会一次性抹掉 Fleet Board）。
        let mut projects: Vec<SnapshotProject> = match repo.read_snapshot()? {
            Some(s) => s.projects,
            None => Vec::new(),
        };
        let new_proj: SnapshotProject = status.clone().into();
        if let Some(slot) = projects
            .iter_mut()
            .find(|p| p.project_id == Some(project_id))
        {
            *slot = new_proj;
        } else {
            projects.push(new_proj);
        }
        let snapshot = Snapshot {
            schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
            machine_id: machine_id.to_string(),
            hostname: hardware_id::current_hostname(),
            updated_at: now,
            projects,
        };
        let events = vec![Event {
            ts: now,
            trigger: trigger.clone(),
            project_id: status.project_id,
            action: crate::sync::Action::Collect,
            from_head: None,
            to_head: status.head.clone(),
            dirty: status.dirty,
            untracked: status.untracked,
            dangerous_state: status.dangerous_state,
            manual: if matches!(trigger, Trigger::Manual) {
                Some(true)
            } else {
                None
            },
            git_remote_mismatch,
            git_remote_failed,
        }];
        let result = repo
            .sync_cycle(
                snapshot,
                events,
                trigger.clone(),
                matches!(trigger, Trigger::Manual),
                push,
            )
            .map_err(CommandError::Sync)?;
        record_attempt = result.changed;
        Ok(ManualSyncResult {
            outcome: ManualSyncOutcome::Synced {
                pushed: result.pushed,
            },
            pushed: result.pushed,
            project_count: 1,
            collected_at: now,
            sync_record_warning: None,
        })
    })();
    finish_recorded_sync(store, record_attempt, result)
}

/// v4.2 防伪校验核心：跑 `git remote get-url origin` 与 registry 期望 URL 集对比。
///
/// 返回 `(git_remote_mismatch, git_remote_failed)`：
/// - 一致 → `(None, None)`
/// - 不一致 → `(Some(GitRemoteMismatch), None)`；expected = registry primary URL（归一化），
///   actual = 实测 URL（归一化失败时填原始值）
/// - 命令失败 / 无 origin / 非 git 仓 → `(None, Some(reason))`
fn classify_remote(
    project_path: &std::path::Path,
    reg: &RegistryProject,
) -> (Option<GitRemoteMismatch>, Option<String>) {
    let raw = match get_git_remote_url(project_path) {
        Some(u) => u,
        None => {
            return (
                None,
                Some(format!(
                    "git remote get-url origin 无输出（可能非 git 仓 / origin 未配置 / 命令失败）: {}",
                    project_path.display()
                )),
            )
        }
    };
    let actual_norm = normalize_git_url(&raw).ok();
    let expected_primary = normalize_git_url(&reg.git_url).unwrap_or_else(|_| reg.git_url.clone());
    let actual_cmp = actual_norm.clone().unwrap_or_else(|| raw.clone());

    // 匹配 primary
    if actual_cmp == expected_primary {
        return (None, None);
    }
    // 匹配任一 alias
    for alias in &reg.git_url_aliases {
        let alias_norm = normalize_git_url(alias).unwrap_or_else(|_| alias.clone());
        if actual_cmp == alias_norm {
            return (None, None);
        }
    }

    (
        Some(GitRemoteMismatch {
            expected: expected_primary,
            actual: actual_cmp,
        }),
        None,
    )
}

// -------- 手动同步（全库） --------

fn sync_status_store(
    root: &std::path::Path,
    local_path: &std::path::Path,
    machine_id: &str,
) -> Result<crate::sync_status::SyncStatusStore, String> {
    let gate = read_remote_gate(local_path);
    let remote = if gate.cleared {
        None
    } else {
        gate.remote.or_else(|| get_git_remote_url(root))
    };
    crate::sync_status::SyncStatusStore::new(root, machine_id, remote.as_deref())
}

pub fn get_sync_status(
    root: &std::path::Path,
    local_path: &std::path::Path,
    machine_id: &str,
) -> Result<crate::sync_status::SyncStatus, String> {
    sync_status_store(root, local_path, machine_id)?.read()
}

fn finish_recorded_sync(
    store: Option<Result<crate::sync_status::SyncStatusStore, String>>,
    record_attempt: bool,
    mut result: Result<ManualSyncResult, CommandError>,
) -> Result<ManualSyncResult, CommandError> {
    use crate::sync_status::SyncOutcome;
    if !record_attempt {
        return result;
    }
    let Some(store) = store else {
        return result;
    }; // 不带 --push 的本地采集不记跨机历史。
    let outcome = match &result {
        Ok(r) => match r.outcome {
            ManualSyncOutcome::Unconfigured => SyncOutcome::Unconfigured,
            ManualSyncOutcome::Synced { pushed: true } => SyncOutcome::Succeeded,
            ManualSyncOutcome::Synced { pushed: false } => SyncOutcome::PushFailed,
        },
        Err(_) => SyncOutcome::Failed,
    };
    // 取真正的完成时间，不拿采集开始时间或 snapshot 更新时间冒充成功时间。
    let completed_at = Utc::now();
    if let Err(error) = store.and_then(|store| store.record(outcome, completed_at)) {
        let warning = format!("同步记录保存失败，重启后可能无法恢复本次状态：{error}");
        eprintln!("{warning}"); // CLI 也必须可见；保留原错误分类和退出码。
        if let Ok(r) = &mut result {
            r.sync_record_warning = Some(warning);
        }
    }
    result
}

pub fn manual_sync(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    machine_id: &str,
    trigger: Trigger,
    push: bool,
) -> Result<ManualSyncResult, CommandError> {
    let store = push.then(|| sync_status_store(dashboard_state_root, local_path, machine_id));
    let mut record_attempt = true;
    let result = (|| {
        // P0-5：锁在 registry read 之前取（CRUD 也持同一把锁写 registry，
        // 不前移会读到写一半 / 旧的 registry）。UI 触发用短超时（不冻结界面，超时提示"同步进行中"），
        // 覆盖 read registry → 全量 collect → build snapshot → sync_cycle 临界区。
        // 决策 5 + 三方评审收窄（2026-07-21）：手动同步一条龙 = 前置 `pull --ff-only` → 采集 →
        // commit → push（force）。**pull 收窄到仅 `push=true` 时前置**（`pull_if_ready = push`）：
        //   - `push=true`（真出机）：Ready 时先 `pull --ff-only` 拉最新，避免本机 commit 落后于远端导致
        //     push non-fast-forward 被拒（git non-ff 看提交历史祖先关系，与 machines/ 目录是否隔离无关——
        //     两机同基线各自 commit，即便改不同 machines 子目录，后 push 者仍会 non-ff，故 pull-first 有实价值）。
        //   - `push=false`（纯采集落盘，CLI collect 不带 --push）：不承担网络往返，跳过 pull。
        // 前置门禁：NeedsInit → clone；Unconfigured（未配 remote 且无 origin，或显式清空）→ 判别式
        // Ok(unconfigured) no-op（对齐决策 6，顶栏不假报同步成功）；Conflict（含 InvalidStructure）→ Err。
        // pull 失败（diverge / 网络）走 IPC Err（真失败，未成功出机）。
        let gate = read_remote_gate(local_path);
        if let StateRepoStatus::Unconfigured = trigger_config_pull(
            dashboard_state_root,
            gate.remote.as_deref(),
            gate.cleared,
            push,
        )? {
            return Ok(ManualSyncResult {
                outcome: ManualSyncOutcome::Unconfigured,
                pushed: false,
                project_count: 0,
                collected_at: Utc::now(),
                sync_record_warning: None,
            });
        }
        let repo = DashboardStateRepo::new(dashboard_state_root, machine_id);
        let _lock = repo.acquire_lock(std::time::Duration::from_secs(5))?;
        // 审计 P1：两段锁 TOCTOU 复检（同 collect_single）——锁内再确认仍 Ready，非 Ready 中止。锁内重读 gate。
        let gate = read_remote_gate(local_path);
        match resolve_status(&repo.observe(), gate.remote.as_deref(), gate.cleared) {
            StateRepoStatus::Ready => {}
            StateRepoStatus::Unconfigured => {
                return Ok(ManualSyncResult {
                    outcome: ManualSyncOutcome::Unconfigured,
                    pushed: false,
                    project_count: 0,
                    collected_at: Utc::now(),
                    sync_record_warning: None,
                });
            }
            other => return Err(sync_state_to_error(other)),
        }
        let registry = Registry::load_at(dashboard_state_root, local_path)?;
        // P0-3-b：manual_sync 全量重建 + force=true 会无条件覆盖旧 snapshot；
        // 若旧 snapshot 是未来版本写入（含未知字段）或损坏，先 read_snapshot() fail-closed 检查，
        // Err 则中止不覆盖，避免静默抹掉别机新数据（force 路径不看 hash，snapshot_hash 又吞错，
        // 必须在这里挡）。
        let _ = repo.read_snapshot()?;
        let mut projects: Vec<ProjectStatus> = Vec::new();
        // 🔒 spec fleet-sync§sync:false 项目字段绝不出机：
        //    只遍历 list_shared() —— 私有项目 (sync:false) 永远不进 snapshot / events / push 载荷。
        //    审计对账 P0-A（2026-07-06）：删掉了原来的 list_private() 遍历。
        // v4.2 §16.5：主键从 iterator 拿 —— 遍历 shared 项目时把 registry UUID 写回
        // status.project_id（`collect()` 出的默认值为 None）
        for reg in registry.list_shared() {
            if let Some(path) = reg.machines.get(machine_id) {
                let p = PathBuf::from(shellexpand(path));
                if p.is_dir() {
                    let mut status = collect(&p, &reg.name);
                    status.project_id = Some(reg.project_id);
                    projects.push(status);
                }
            }
        }
        let now = Utc::now();
        let snapshot = Snapshot {
            schema_version: CURRENT_SNAPSHOT_SCHEMA_VERSION,
            machine_id: machine_id.to_string(),
            hostname: hardware_id::current_hostname(),
            updated_at: now,
            projects: projects.iter().cloned().map(Into::into).collect(),
        };

        // v4.2：manual_sync 属批量 tick 路径，防伪校验默认关（省 shell out）；
        // hook 触发的单项目采集走 collect_single(verify_remote=true) 路径。
        let events: Vec<Event> = projects
            .iter()
            .map(|p| Event {
                ts: now,
                trigger: trigger.clone(),
                project_id: p.project_id,
                action: crate::sync::Action::Collect,
                from_head: None,
                to_head: p.head.clone(),
                dirty: p.dirty,
                untracked: p.untracked,
                dangerous_state: p.dangerous_state,
                manual: if matches!(trigger, Trigger::Manual) {
                    Some(true)
                } else {
                    None
                },
                git_remote_mismatch: None,
                git_remote_failed: None,
            })
            .collect();

        let result = repo
            .sync_cycle(
                snapshot,
                events,
                trigger.clone(),
                matches!(trigger, Trigger::Manual),
                push,
            )
            .map_err(CommandError::Sync)?;
        record_attempt = result.changed;
        Ok(ManualSyncResult {
            outcome: ManualSyncOutcome::Synced {
                pushed: result.pushed,
            },
            pushed: result.pushed,
            project_count: projects.len(),
            collected_at: now,
            sync_record_warning: None,
        })
    })();
    finish_recorded_sync(store, record_attempt, result)
}

/// 手动同步的判别式结果状态（决策 6：顶栏假成功修复）。前端按 `status` 判分支渲染顶栏——
/// 无法只靠 `pushed`/`project_count` 区分「未配存储仓 no-op」与「配了但无 sync:true 项目」
/// （两种 project_count 都是 0），必须显式状态字段。
///
/// 语义边界（与决策 4/5 对齐）：
/// - `Synced { pushed }`：走到了出机编排。`pushed=true` = 采集+commit+push 全成功（前端写时间戳）；
///   `pushed=false` = land+commit 成功但 push 失败（部分成功，退 0，前端提示「已本地保存·推送失败·下次重试」、**不**写时间戳）。
/// - `Unconfigured`：未配存储仓（或显式清空），no-op 未触发同步（前端「未配置存储仓，未触发同步」、不写时间戳）。
///
/// `pull --ff-only` / land / 门禁（Conflict/InvalidStructure）失败**不**在此枚举——走 IPC `Err`
/// （真失败）；本枚举只承载「已成功走完出机决策」的两种正常结局。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ManualSyncOutcome {
    Synced { pushed: bool },
    Unconfigured,
}

#[derive(Debug, Serialize)]
pub struct ManualSyncResult {
    /// 判别式状态（决策 6，前端据此渲染顶栏 + 决定是否写 syncStatus.last）。
    pub outcome: ManualSyncOutcome,
    /// 是否 push 成功。`Unconfigured` 时恒 false。保留供 CLI `cmd_collect` 打印 + 组 4 契约测试读取。
    pub pushed: bool,
    pub project_count: usize,
    pub collected_at: chrono::DateTime<Utc>,
    /// 同步已执行但本机历史未保存；不把记录错误误报成网络同步失败。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_record_warning: Option<String>,
}

fn shellexpand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if let Some(rest) = path.strip_prefix("$HOME/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

// -------- 设置页 --------

pub fn get_settings(local_path: &std::path::Path) -> Result<LocalConfig, CommandError> {
    // 首启自举（决策 1）：local.yaml 不存在时返内存默认（**绝不落盘**）。get_settings 会被
    // read_configured_remote 等路径在锁内外反复调用，首读即落盘会引入锁内不可控写；首份文件
    // 只由 update_settings 写侧（write_local_config_atomic）建档。复用 load_local_config_inner
    // 的 `NotFound => Ok(None)` 同款 error-kind 判断，只是这里回内存默认而非 None。
    // 其余错误（权限拒绝 / IO / YAML 损坏）仍原样返 Err（不吞）。
    let text = match std::fs::read_to_string(local_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // machine_id 留空而非在此计算：这份默认值绝不落盘，空值会被 resolve_machine_id
            // 当作「无冻结值」重新计算。若在此调 compute_machine_id() 并 `?`，硬件标识取不到
            // 时连设置页都打不开，用户就没法进去设 DASHBOARD_MACHINE_ID 自救 —— 死锁。
            return Ok(LocalConfig::new(String::new()));
        }
        Err(e) => return Err(CommandError::Io(e)),
    };
    let cfg: LocalConfig = serde_yaml::from_str(&text)
        .map_err(|e| CommandError::Config(crate::config::ConfigError::YamlParse(local_path.display().to_string(), e)))?;
    Ok(cfg)
}

pub fn update_settings(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    update: SettingsUpdate,
) -> Result<LocalConfig, CommandError> {
    let _local_lock = acquire_local_config_lock(local_path, std::time::Duration::from_secs(5))?;
    // update_settings 与 CRUD 的 Registry::save_at 都 read→rewrite 同一份 local.yaml；固定路径锁
    // 不随 dashboard_state_root 改变，先阻断跨根覆盖。repo-global sidecar 锁再与当前数据仓操作互斥；
    // first-run 未 clone 时也真正上锁。
    let repo = DashboardStateRepo::new(dashboard_state_root, "_crud_only_");
    let _lock = repo.acquire_lock_allow_uninit(std::time::Duration::from_secs(5))?;
    let mut cfg = get_settings(local_path)?;
    // 落盘前把身份定下来。get_settings 对缺失文件返的默认值 machine_id 为空（避免取不到硬件
    // 标识时连设置页都打不开），但**空身份绝不能写进 local.yaml** —— 那等于把「不知道自己是谁」
    // 持久化，后续所有按 machine_id 匹配的路径都会错。存量旧式主机名身份也在此换成硬件身份。
    if cfg.machine_id.trim().is_empty() || is_legacy_hostname_id(cfg.machine_id.trim()) {
        cfg.machine_id = resolve_machine_id(Some(&cfg))?;
    }
    cfg.record_hostname(&hardware_id::current_hostname());
    if let Some(v) = update.sync_tick_minutes {
        if !(1..=60).contains(&v) {
            return Err(CommandError::InvalidSettings(format!(
                "sync_tick_minutes 需在 1-60 之间，收到 {v}"
            )));
        }
        cfg.sync_tick_minutes = v;
    }
    if let Some(v) = update.diagnostic_mode {
        cfg.diagnostic_mode = v;
    }
    // 存储仓远端：三态（未传保持 / Some(None)|空白清空 / Some(Some) 设值）。
    // 存**原始不归一化**（保留 git@/.git/端口/大小写），仅 trim 首尾空白 + 拒内嵌 token。
    if let Some(remote_opt) = update.dashboard_state_remote {
        let normalized = remote_opt.and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });
        if let Some(ref r) = normalized {
            reject_inline_token(r)?;
        }
        // 决策 2：显式传了 remote 字段才更新 cleared 意图位——清空（normalized=None）置 true、
        // 设值置 false。未传 dashboard_state_remote 时保持原 cleared（不误翻存量标记）。
        cfg.dashboard_state_remote_cleared = normalized.is_none();
        cfg.dashboard_state_remote = normalized;
    }
    // 数据目录根：三态语义同上；本地路径无 token 校验，仅 trim 空白。
    if let Some(root_opt) = update.dashboard_state_root {
        cfg.dashboard_state_root = root_opt.and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });
    }
    if let Some(cli_opt) = update.repowiki_cli_path {
        let normalized = normalize_optional_local_path(cli_opt);
        if let Some(ref value) = normalized {
            validate_repowiki_cli_path(std::path::Path::new(value))?;
        }
        cfg.repowiki_cli_path = normalized;
    }
    if let Some(data_root_opt) = update.repowiki_data_root {
        let normalized = normalize_optional_local_path(data_root_opt);
        if let Some(ref value) = normalized {
            validate_repowiki_data_root(std::path::Path::new(value))?;
        }
        cfg.repowiki_data_root = normalized;
    }
    let yaml = serde_yaml::to_string(&cfg).map_err(|e| CommandError::Config(e.into()))?;
    crate::config::write_local_config_atomic(local_path, yaml.as_bytes())?;
    Ok(cfg)
}

/// D3：设置保存后「配置即拉」的同步子状态，回传给前端消费。
///
/// 语义与 [`trigger_config_pull`] 的返回对齐——保存本身已成功（local.yaml 落盘），此结构只表达
/// 「保存之后那次自动同步」的结果，故 `Failed` 不代表保存失败，而是「设置已保存但自动同步失败」。
/// 前端据此决定 toast：`Synced` 成功、`Unconfigured` 未配存储仓不提示错误、`Failed` 弹带 tag 的失败提示。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SyncOutcome {
    /// 存储仓已同步（首次 clone 或已就绪时 pull 成功）。
    Synced,
    /// 未配置存储仓地址，未触发同步（正常状态，非错误）。
    Unconfigured,
    /// 保存成功但自动同步失败，携 D1 稳定分类 `tag` + 面向用户中文 `hint`。
    Failed { tag: String, hint: String },
}

impl SyncOutcome {
    /// 把 [`trigger_config_pull`] 的返回折算成前端可消费的同步子状态。纯函数，便于单测。
    ///
    /// - `Ok(Ready)` / `Ok(NeedsInit)`：真正执行了 pull / clone → `Synced`
    /// - `Ok(Unconfigured)`：未配 remote，no-op → `Unconfigured`（不算错误）
    /// - `Err(ConfigPullFailed{kind})`：带 D1 分类 → `Failed{tag, hint}`（tag/hint 与 CommandError 口径一致）
    /// - 其余 `Err`（锁占用 / 冲突等）：归 `SYNC_ERROR`，hint 用 Display 文案
    /// - `Ok(Conflict)` 不会由 trigger_config_pull 产生（冲突走 Err 分支），防御式归 `CONFLICT`
    pub fn from_trigger_result(result: &Result<StateRepoStatus, CommandError>) -> Self {
        match result {
            Ok(StateRepoStatus::Ready) | Ok(StateRepoStatus::NeedsInit) => Self::Synced,
            Ok(StateRepoStatus::Unconfigured) => Self::Unconfigured,
            Ok(StateRepoStatus::Conflict(_)) => Self::Failed {
                tag: "CONFLICT".to_string(),
                hint: "存储仓状态冲突，请手动清理数据目录后重试".to_string(),
            },
            Err(CommandError::ConfigPullFailed { kind }) => Self::Failed {
                tag: kind.tag().to_string(),
                hint: kind.user_hint().to_string(),
            },
            Err(other) => Self::Failed {
                tag: "SYNC_ERROR".to_string(),
                hint: other.to_string(),
            },
        }
    }
}

/// D3：`update_settings` Tauri 命令的回传体——保存后的配置 + 那次自动同步的子状态。
/// `Ok(UpdateSettingsResponse)` 恒表示「保存成功」；`sync` 字段单独表达同步是否成功（P1-4 部分成功语义）。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateSettingsResponse {
    pub config: LocalConfig,
    pub sync: SyncOutcome,
}

/// 只保存 RepoWiki 本机路径。该函数不 pull/clone/push dashboard-state；
/// Tauri 使用独立 IPC，避免复用通用设置保存后的配置仓同步链路。
pub fn update_repowiki_settings(
    dashboard_state_root: &std::path::Path,
    local_path: &std::path::Path,
    repowiki_cli_path: Option<String>,
    repowiki_data_root: Option<String>,
) -> Result<LocalConfig, CommandError> {
    update_settings(
        dashboard_state_root,
        local_path,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: Some(repowiki_cli_path),
            repowiki_data_root: Some(repowiki_data_root),
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub sync_tick_minutes: Option<u32>,
    pub diagnostic_mode: Option<bool>,
    /// 存储仓远端连接串，三态语义：`None`=未传（保持原值）；`Some(None)` 或空白串=显式清空；
    /// `Some(Some(x))`=设为 x。`double_option` 让"字段缺席"与"字段传 null"可区分。
    #[serde(default, deserialize_with = "double_option")]
    pub dashboard_state_remote: Option<Option<String>>,
    /// 数据目录根路径，三态语义同 `dashboard_state_remote`。
    #[serde(default, deserialize_with = "double_option")]
    pub dashboard_state_root: Option<Option<String>>,
    /// RepoWiki CLI 绝对路径，三态语义同 `dashboard_state_remote`。
    #[serde(default, deserialize_with = "double_option")]
    pub repowiki_cli_path: Option<Option<String>>,
    /// RepoWiki 本机 data root 绝对路径，三态语义同 `dashboard_state_remote`。
    #[serde(default, deserialize_with = "double_option")]
    pub repowiki_data_root: Option<Option<String>>,
}

/// serde 三态反序列化：字段缺席 → `None`（走 `#[serde(default)]`，语义"不改"）；
/// 字段存在（含 null / 值）→ `Some(inner)`（语义"清空 / 设值"）。
/// 用于 SettingsUpdate 区分「未传（保持原值）」与「显式清空（设为 None）」。
fn double_option<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

fn normalize_optional_local_path(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn validate_repowiki_cli_path(path: &std::path::Path) -> Result<(), CommandError> {
    if !path.is_absolute() {
        return Err(CommandError::InvalidSettings(
            "repowiki_cli_path 必须是绝对路径".to_string(),
        ));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        CommandError::InvalidSettings(
            "repowiki_cli_path 必须指向已存在的可执行常规文件".to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(CommandError::InvalidSettings(
            "repowiki_cli_path 必须指向非符号链接的常规文件".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.permissions().mode() & 0o111 == 0 {
            return Err(CommandError::InvalidSettings(
                "repowiki_cli_path 必须是单链接可执行文件".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_repowiki_data_root(path: &std::path::Path) -> Result<(), CommandError> {
    if !path.is_absolute() {
        return Err(CommandError::InvalidSettings(
            "repowiki_data_root 必须是绝对路径".to_string(),
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() =>
        {
            Err(CommandError::InvalidSettings(
                "repowiki_data_root 若已存在必须是非符号链接目录".to_string(),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CommandError::InvalidSettings(
            "无法安全检查 repowiki_data_root".to_string(),
        )),
    }
}

/// 存储仓远端连接串禁内嵌凭证/token（防 `https://user:token@host` 写进 local.yaml 泄露）。
/// 放行标准 SSH 用户名 `git@host`；authority 段 `@` 前若非纯 `git` 一律拒。
/// remote 存原始不归一化，此处只做安全校验、不改写内容。
fn reject_inline_token(remote: &str) -> Result<(), CommandError> {
    let after_scheme = remote
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(remote);
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    if let Some((userinfo, _host)) = authority.split_once('@') {
        if userinfo != "git" {
            // LEAK-06：不回显 userinfo（可能是 user:token，含密码/令牌），否则会随错误信息
            // 暴露到 UI / 截图 / 日志。只给固定文案，不带任何用户输入。
            return Err(CommandError::InvalidSettings(
                "存储仓地址不应内嵌用户名/密码（形如 user:pass@host）；请去掉地址里的认证部分，改用不带凭证的 https 或 git@ SSH 形式".to_string(),
            ));
        }
    }
    Ok(())
}

// -------- 灾恢 --------

pub fn snapshot_backup_now(
    dashboard_state_root: &std::path::Path,
    backups_root: &std::path::Path,
) -> Result<PathBuf, CommandError> {
    let now = Utc::now();
    let dst = snapshot_backup(dashboard_state_root, backups_root, now).map_err(CommandError::Io)?;
    let _removed = prune_old_backups(backups_root, now).map_err(CommandError::Io)?;
    Ok(dst)
}

// -------- Emergency purge 依赖检测 --------

pub fn emergency_purge_precheck() -> EmergencyPrecheck {
    EmergencyPrecheck {
        git_filter_repo_installed: detect_git_filter_repo(),
    }
}

#[derive(Debug, Serialize)]
pub struct EmergencyPrecheck {
    pub git_filter_repo_installed: bool,
}

// -------- 错误类型 --------

#[derive(Debug)]
pub enum CommandError {
    SessionStorage(crate::session_storage::SessionStorageError),
    Io(std::io::Error),
    Config(crate::config::ConfigError),
    Sync(crate::sync::SyncError),
    Aggregate(crate::aggregate::AggregateError),
    Normalize(NormalizeError),
    InvalidSettings(String),
    /// Q1-C：Bitbucket 双形态需用户显式确认（机器可识别，前端匹配 tag 前缀而非中文文案）。
    NeedDualFormConfirm { origin: String, input: String },
    /// Q1-C：origin 与手填无关，拒绝保存（防选错项目目录）。
    OriginUnrelated { actual_origin: String, input: String },
    /// Q2-D2：配置即拉（clone/pull）失败，携 D1 面向用户分类（前端靠 tag 前缀识别，不解析中文）。
    ConfigPullFailed { kind: ConfigPullErrorKind },
    /// 取不到本机硬件标识 → 无法确定身份。**必须让操作失败**，不得兜底成假身份后继续写云端
    /// （旧实现兜底成 `unknown-machine`，在 `machines/` 下留了个谁都不认的孤儿目录）。
    MachineIdentityUnavailable(hardware_id::HardwareIdError),
    /// Session Board 来源必须显式命中 allowlist；未知来源不能静默回退到 Claude Code。
    UnsupportedSessionSource(String),
    /// 恢复命令只接受文件名安全字符，拒绝空白、路径与 shell 元字符。
    InvalidSessionId,
    ResumeUnsupported,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStorage(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Config(e) => write!(f, "config: {e}"),
            Self::Sync(e) => write!(f, "sync: {e}"),
            Self::Aggregate(e) => write!(f, "aggregate: {e}"),
            Self::Normalize(e) => write!(f, "git_url: {e}"),
            Self::InvalidSettings(m) => write!(f, "invalid settings: {m}"),
            Self::NeedDualFormConfirm { origin, input } => write!(
                f,
                "[NEED_DUAL_FORM_CONFIRM] Bitbucket 双形态需确认：origin={origin} 与手填={input} 结构等价，确认后重试"
            ),
            Self::OriginUnrelated {
                actual_origin,
                input,
            } => write!(
                f,
                "[ORIGIN_UNRELATED] 所选目录的 origin（{actual_origin}）与填写的 git 地址（{input}）不一致，请确认是否选错项目目录"
            ),
            Self::ConfigPullFailed { kind } => write!(f, "[{}] {}", kind.tag(), kind.user_hint()),
            Self::MachineIdentityUnavailable(e) => write!(
                f,
                "[MACHINE_IDENTITY_UNAVAILABLE] 无法确定本机身份：{e}。请设置环境变量 DASHBOARD_MACHINE_ID 后重试（诊断：cargo run -p dashboard-core --example hwid_probe）"
            ),
            Self::UnsupportedSessionSource(source) => write!(
                f,
                "[UNSUPPORTED_SESSION_SOURCE] 不支持的会话来源：{source}"
            ),
            Self::ResumeUnsupported => write!(f, "[RESUME_UNSUPPORTED] 此工具尚无已验证的恢复入口，请复制会话 ID 在原工具中查找"),
            Self::InvalidSessionId => write!(
                f,
                "[INVALID_SESSION_ID] session id 含不允许的字符或长度超限"
            ),
        }
    }
}

impl From<hardware_id::HardwareIdError> for CommandError {
    fn from(error: hardware_id::HardwareIdError) -> Self {
        Self::MachineIdentityUnavailable(error)
    }
}

impl std::error::Error for CommandError {}

impl From<crate::config::ConfigError> for CommandError {
    fn from(e: crate::config::ConfigError) -> Self {
        Self::Config(e)
    }
}
impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<NormalizeError> for CommandError {
    fn from(e: NormalizeError) -> Self {
        Self::Normalize(e)
    }
}
impl From<crate::sync::SyncError> for CommandError {
    fn from(e: crate::sync::SyncError) -> Self {
        Self::Sync(e)
    }
}

#[cfg(test)]
mod local_config_lock_tests {
    use super::*;

    #[test]
    fn local_config_lock_is_stable_and_exclusive() {
        let local_path = std::env::temp_dir().join(format!(
            "dashboard-local-lock-{}-{}.yaml",
            std::process::id(),
            Uuid::new_v4()
        ));
        let first = acquire_local_config_lock(&local_path, std::time::Duration::from_millis(100))
            .expect("first local config lock");
        let second = acquire_local_config_lock(&local_path, std::time::Duration::from_millis(50));
        assert!(
            matches!(
                second,
                Err(CommandError::Sync(crate::sync::SyncError::LockBusy))
            ),
            "同一 local.yaml 必须命中同一把锁，与 dashboard_state_root 无关"
        );
        drop(first);
        acquire_local_config_lock(&local_path, std::time::Duration::from_millis(100))
            .expect("released local config lock");
    }
}

#[cfg(test)]
mod sync_outcome_tests {
    use super::*;

    // Ready / NeedsInit 都表示真正执行了 pull / clone → Synced。
    #[test]
    fn ready_and_needs_init_map_to_synced() {
        assert_eq!(
            SyncOutcome::from_trigger_result(&Ok(StateRepoStatus::Ready)),
            SyncOutcome::Synced
        );
        assert_eq!(
            SyncOutcome::from_trigger_result(&Ok(StateRepoStatus::NeedsInit)),
            SyncOutcome::Synced
        );
    }

    // 未配 remote → no-op，不是错误。
    #[test]
    fn unconfigured_maps_to_unconfigured() {
        assert_eq!(
            SyncOutcome::from_trigger_result(&Ok(StateRepoStatus::Unconfigured)),
            SyncOutcome::Unconfigured
        );
    }

    // ConfigPullFailed 必须原样透出 D1 稳定 tag + 中文 hint，前端据 tag 判分支。
    #[test]
    fn config_pull_failed_carries_tag_and_hint() {
        let outcome = SyncOutcome::from_trigger_result(&Err(CommandError::ConfigPullFailed {
            kind: ConfigPullErrorKind::Network,
        }));
        assert_eq!(
            outcome,
            SyncOutcome::Failed {
                tag: "NETWORK".to_string(),
                hint: ConfigPullErrorKind::Network.user_hint().to_string(),
            }
        );

        // 填错地址（非 dashboard-state 仓）→ INVALID_STRUCTURE，前端提示确认地址。
        let structure = SyncOutcome::from_trigger_result(&Err(CommandError::ConfigPullFailed {
            kind: ConfigPullErrorKind::InvalidStructure,
        }));
        assert!(matches!(
            structure,
            SyncOutcome::Failed { tag, .. } if tag == "INVALID_STRUCTURE"
        ));
    }

    // 非 ConfigPullFailed 的其它错误（锁占用 / 校验失败等）归 SYNC_ERROR 兜底，仍算保存成功+同步失败。
    #[test]
    fn other_errors_fall_back_to_sync_error() {
        let outcome = SyncOutcome::from_trigger_result(&Err(CommandError::InvalidSettings(
            "boom".to_string(),
        )));
        assert!(matches!(
            outcome,
            SyncOutcome::Failed { tag, .. } if tag == "SYNC_ERROR"
        ));
    }

    // Conflict 不会由 trigger_config_pull 以 Ok 形态产生，但分类器要防御式归 CONFLICT 不 panic。
    #[test]
    fn ok_conflict_is_defensively_classified() {
        let outcome = SyncOutcome::from_trigger_result(&Ok(StateRepoStatus::Conflict(
            ConflictReason::NotGitDir,
        )));
        assert!(matches!(
            outcome,
            SyncOutcome::Failed { tag, .. } if tag == "CONFLICT"
        ));
    }
}

#[cfg(test)]
mod conflict_error_tests {
    use super::*;

    // LEAK-06 孪生点：RemoteMismatch 错误不得回显 actual origin（来自 git remote get-url，
    // 未过 reject_inline_token，可能含 user:token）；只回显已脱敏的 configured。
    #[test]
    fn remote_mismatch_error_does_not_leak_actual_origin() {
        let err = conflict_to_error(ConflictReason::RemoteMismatch {
            configured: "git@host:team/ds.git".to_string(),
            actual: Some("https://alice:s3cret@host/team/ds.git".to_string()),
        });
        let msg = err.to_string();
        assert!(!msg.contains("s3cret"), "不应回显 actual origin 的密码：{msg}");
        assert!(!msg.contains("alice"), "不应回显 actual origin 的 userinfo：{msg}");
        assert!(!msg.contains("https://alice"), "不应回显 actual origin 原文：{msg}");
        // configured 来自 local.yaml（已过 reject_inline_token 校验），允许展示。
        assert!(
            msg.contains("git@host:team/ds.git"),
            "configured 应正常展示（它是安全的）：{msg}"
        );
    }
}

#[cfg(test)]
mod remote_gate_tests {
    use super::*;

    fn tmp_local(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "dashboard-remote-gate-{tag}-{}-{}.yaml",
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    // 成功解析：配了 remote、未清空 → gate 如实返回 {Some(remote), false}。
    #[test]
    fn read_remote_gate_parses_configured() {
        let path = tmp_local("configured");
        let mut cfg = LocalConfig::new("m");
        cfg.dashboard_state_remote = Some("git@host:team/ds.git".to_string());
        std::fs::write(&path, serde_yaml::to_string(&cfg).unwrap()).unwrap();

        let gate = read_remote_gate(&path);
        assert_eq!(gate.remote.as_deref(), Some("git@host:team/ds.git"));
        assert!(!gate.cleared);
        let _ = std::fs::remove_file(&path);
    }

    // 2.2 护栏：老 yaml 无 `dashboard_state_remote_cleared` 字段 → serde 默认 false（存量续同步兼容）。
    #[test]
    fn read_remote_gate_legacy_yaml_without_cleared_field_defaults_false() {
        let path = tmp_local("legacy");
        std::fs::write(
            &path,
            "schema_version: 1\nmachine_id: legacy-box\ndashboard_state_remote: git@host:team/ds.git\n",
        )
        .unwrap();

        let gate = read_remote_gate(&path);
        assert_eq!(gate.remote.as_deref(), Some("git@host:team/ds.git"));
        assert!(!gate.cleared, "老 yaml 缺 cleared 字段应默认 false（续同步兼容）");
        let _ = std::fs::remove_file(&path);
    }

    // 2.4b 安全：YAML 损坏 → fail-closed {None, cleared=true}，绝不沿旧 origin push。
    #[test]
    fn read_remote_gate_corrupt_yaml_fails_closed() {
        let path = tmp_local("corrupt");
        std::fs::write(&path, "{{{ not valid yaml :::").unwrap();

        let gate = read_remote_gate(&path);
        assert!(gate.remote.is_none(), "读不出配置时 remote 必须 None");
        assert!(
            gate.cleared,
            "YAML 损坏必须 fail-closed（cleared=true → 判 Unconfigured 不 push）"
        );
        let _ = std::fs::remove_file(&path);
    }

    // 2.4b 安全：local.yaml 读不出（NotFound）→ 统一 fail-closed cleared=true（安全侧）。
    #[test]
    fn read_remote_gate_missing_file_fails_closed() {
        let path = tmp_local("missing"); // 不创建
        assert!(!path.exists());

        let gate = read_remote_gate(&path);
        assert!(gate.remote.is_none());
        assert!(gate.cleared, "读不出（NotFound）统一 fail-closed cleared=true");
    }

    // 2.2 行为：update_settings 显式清空 remote → 持久化 cleared=true → 下次 read_remote_gate 判已清空；
    // 设值 remote → cleared=false（撤销 vs 存量的判别位真的落盘了）。
    #[test]
    fn update_settings_toggles_cleared_flag() {
        let tmp = std::env::temp_dir().join(format!(
            "dashboard-gate-clear-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let local_path = tmp.join("local.yaml");
        let state_root = tmp.join("state");
        std::fs::create_dir_all(&state_root).unwrap();

        let set_remote = |remote: Option<String>| SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: Some(remote),
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        };

        // 设值 remote → cleared=false
        update_settings(&state_root, &local_path, set_remote(Some("git@host:team/ds.git".to_string())))
            .expect("设值 remote 应成功");
        let gate = read_remote_gate(&local_path);
        assert_eq!(gate.remote.as_deref(), Some("git@host:team/ds.git"));
        assert!(!gate.cleared, "设值后 cleared=false");

        // 显式清空 → cleared=true（撤销生效、停用旧 origin）
        update_settings(&state_root, &local_path, set_remote(None)).expect("清空 remote 应成功");
        let gate = read_remote_gate(&local_path);
        assert!(gate.remote.is_none(), "清空后 remote=None");
        assert!(gate.cleared, "显式清空后 cleared=true");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

impl From<crate::session_storage::SessionStorageError> for CommandError {
    fn from(error: crate::session_storage::SessionStorageError) -> Self {
        Self::SessionStorage(error)
    }
}
