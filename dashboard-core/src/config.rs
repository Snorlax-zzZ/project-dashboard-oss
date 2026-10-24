// 注册表 + 本机配置加载 / 合并 / CRUD（M2 §3.4-3.5）。
// 跨机共享部分：`dashboard-state/registry/projects/<uuid>.yaml` 每项目一文件。
// 本机私有部分：`~/.project-dashboard/local.yaml`。
// sync 状态单向不可逆 —— 编辑不允许改 sync；改状态唯一路径 = 删除重建。

use crate::schema::{validate_registry_project, SchemaError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 跨机共享的 sync:true 项目条目（registry/projects/<uuid>.yaml）。
/// sync 字段永远为 true —— 由 Serialize 的 serialize_true 序列化保证。
///
/// v4.2（Decision 15）新增 `git_url_aliases`：项目 remote URL 历史别名列表。
/// UI 层"更新 git URL"时，旧 `git_url` 追加进 aliases；采集时防伪校验对比 git_url + aliases 全部
/// 命中一次即算一致。字段名 `git_url` 保留不改名（撤销 v4.2 首版 rename `git_url_primary` 方案），
/// 避免 Win/Mac 未同时升级时旧 App 读到新 YAML 缺 required 字段崩。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistryProject {
    pub schema_version: u32,
    pub project_id: Uuid,
    pub name: String,
    pub git_url: String,
    #[serde(default)]
    pub git_url_aliases: Vec<String>,
    #[serde(
        serialize_with = "serialize_true",
        deserialize_with = "deserialize_true"
    )]
    pub sync: bool,
    pub machines: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 本机私有的 sync:false 项目条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrivateProject {
    pub schema_version: u32,
    pub name: String,
    pub project_id: Option<Uuid>,
    pub path: String,
    pub git_url: String,
    #[serde(
        serialize_with = "serialize_false",
        deserialize_with = "deserialize_false"
    )]
    pub sync: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 本机 `~/.project-dashboard/local.yaml`。
///
/// v4.2 Decision 15：`pending_sync_projects` 字段已删除（UUID 权威迁 dashboard-state registry，
/// 无 pending 中转）。老 yaml 里若含 `pending_sync_projects` 段，`load_local_config` 的 preflight
/// 会 backup 该内容 + stderr WARN，再由 serde 因无 `deny_unknown_fields` 而 silently 忽略字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalConfig {
    pub schema_version: u32,
    /// 本机身份，形如 `mbp-00000001`（短前缀 + 硬件标识哈希）。
    ///
    /// **语义是「首次算出后冻结」**，不再跟主机名走。原因：macOS 局域网撞名会自动给主机名
    /// 加后缀（`...Pro-3` → `...Pro-4`），跟着漂就会让同一台机在云端裂成两个身份 ——
    /// 实测 `machines/` 下已同时躺着 Pro-3 与 Pro-4 两份快照。
    pub machine_id: String,
    /// 当次真实主机名，**纯展示字段**，不参与任何匹配。存它是为了让主机名漂移可见：
    /// machine_id 不变而 hostname 变了，就能一眼断定是同一台机改了名，
    /// 而不是像以前那样凭空多出一台机器、且事后无从追查。
    #[serde(default)]
    pub hostname: String,
    /// 主机名漂移轨迹（去重、按首次出现顺序追加）。留痕用，不参与匹配。
    #[serde(default)]
    pub hostname_history: Vec<String>,
    #[serde(default)]
    pub work_dirs: Vec<String>,
    #[serde(default)]
    pub private_projects: Vec<PrivateProject>,
    #[serde(default)]
    pub diagnostic_mode: bool,
    #[serde(default = "default_sync_tick_minutes")]
    pub sync_tick_minutes: u32,
    /// 存储仓（dashboard-state）远端连接串。存**原始不归一化**（保留 git@/.git/端口/大小写），
    /// 直接拿去 clone；缺席（老 local.yaml）反序列化为 None。禁内嵌 token，由 update_settings 校验。
    #[serde(default)]
    pub dashboard_state_remote: Option<String>,
    /// 用户是否**显式清空**过 `dashboard_state_remote`（vs 存量老机器从没配过 remote）。二者在
    /// `resolve_status` 眼里都是 `configured=None + has_git=true`，磁盘无判别信号 → 必须持久化此
    /// 意图位：清空置 `true`（停用旧 origin、判 Unconfigured）、设值置 `false`。老 yaml 缺字段 →
    /// serde 默认 `false` → **存量逐字节不变**、续同步兼容不破（决策 2）。
    #[serde(default)]
    pub dashboard_state_remote_cleared: bool,
    /// 数据目录根（dashboard-state clone + backups 落地位置）。缺席 → None → 上层 fallback
    /// `~/.project-dashboard/dashboard-state-clone`。local.yaml 自身路径固定，不受此字段影响。
    #[serde(default)]
    pub dashboard_state_root: Option<String>,
    /// RepoWiki CLI 的本机绝对路径。仅用于用户显式触发的只读浏览/查询，
    /// 不进入 dashboard-state registry 或 snapshot。
    #[serde(default)]
    pub repowiki_cli_path: Option<String>,
    /// RepoWiki 本机 data root。它只定位派生图谱，不随项目或 dashboard-state 同步。
    #[serde(default)]
    pub repowiki_data_root: Option<String>,
}

fn default_sync_tick_minutes() -> u32 {
    3
}

impl LocalConfig {
    pub fn new(machine_id: impl Into<String>) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            machine_id: machine_id.into(),
            hostname: String::new(),
            hostname_history: Vec::new(),
            work_dirs: Vec::new(),
            private_projects: Vec::new(),
            diagnostic_mode: false,
            sync_tick_minutes: default_sync_tick_minutes(),
            dashboard_state_remote: None,
            dashboard_state_remote_cleared: false,
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        }
    }

    /// 记录当次主机名：写入 `hostname`，并在 `hostname_history` 里去重追加。
    /// 返回是否发生了漂移（历史里已有别的名字），供调用方决定要不要提示用户。
    pub fn record_hostname(&mut self, hostname: &str) -> bool {
        if hostname.is_empty() {
            return false;
        }
        let drifted = !self.hostname_history.is_empty()
            && !self.hostname_history.iter().any(|name| name == hostname);
        if !self.hostname_history.iter().any(|name| name == hostname) {
            self.hostname_history.push(hostname.to_string());
        }
        self.hostname = hostname.to_string();
        drifted
    }
}

/// 合并后的内存索引：跨机注册表 + 本机私有 + 待 commit 暂留。
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub projects: BTreeMap<Uuid, RegistryProject>,
    pub local: Option<LocalConfig>,
}

impl Registry {
    /// 默认加载路径：`dashboard_state_root/registry/projects/*.yaml` + `~/.project-dashboard/local.yaml`。
    pub fn load_default(dashboard_state_root: &Path) -> Result<Self, ConfigError> {
        let local_path = default_local_config_path();
        Self::load_at(dashboard_state_root, &local_path)
    }

    /// 可注入版：用于测试 / 自定义部署路径。
    pub fn load_at(dashboard_state_root: &Path, local_path: &Path) -> Result<Self, ConfigError> {
        let registry_dir = dashboard_state_root.join("registry").join("projects");
        let projects = load_registry_projects(&registry_dir)?;
        let local = load_local_config(local_path)?;
        Ok(Self { projects, local })
    }

    /// 严格无副作用加载：只读取注册表和本机配置，不执行 legacy pending 迁移备份。
    ///
    /// 仅供 RepoWiki 等只读适配器使用；正常 Dashboard 启动仍应调用 `load_at`，
    /// 保留既有迁移提醒与备份行为。
    pub fn load_at_read_only(
        dashboard_state_root: &Path,
        local_path: &Path,
    ) -> Result<Self, ConfigError> {
        let registry_dir = dashboard_state_root.join("registry").join("projects");
        let projects = load_registry_projects(&registry_dir)?;
        let local = load_local_config_read_only(local_path)?;
        Ok(Self { projects, local })
    }

    pub fn list_shared(&self) -> Vec<&RegistryProject> {
        self.projects.values().collect()
    }

    pub fn list_private(&self) -> Vec<&PrivateProject> {
        self.local
            .as_ref()
            .map(|l| l.private_projects.iter().collect())
            .unwrap_or_default()
    }

    pub fn get_shared(&self, id: &Uuid) -> Option<&RegistryProject> {
        self.projects.get(id)
    }

    /// v4.2 直接落 registry（未命中反查场景）。UUID 权威在 dashboard-state，
    /// 不再走 `pending_sync_projects` 暂留。
    /// 前置：调用方保证已 pull dashboard-state（pull-first 硬约束由上层 `add_project` 保证）。
    pub fn add_shared(&mut self, project: RegistryProject) -> Result<(), ConfigError> {
        if self.projects.contains_key(&project.project_id) {
            return Err(ConfigError::DuplicateProjectId(project.project_id));
        }
        self.projects.insert(project.project_id, project);
        Ok(())
    }

    /// v4.2 关联本机路径到已有 shared 项目（`find_project_id_by_git_url` 命中场景）。
    /// 校验 `machines[<machine_id>]` 不重复；重复 → `MachineAlreadyRegistered`。
    /// 成功则追加条目并刷新 `updated_at`。
    pub fn associate_machine(
        &mut self,
        project_id: &Uuid,
        machine_id: &str,
        path: String,
    ) -> Result<(), ConfigError> {
        let project = self
            .projects
            .get_mut(project_id)
            .ok_or(ConfigError::ProjectNotFound(*project_id))?;
        if project.machines.contains_key(machine_id) {
            return Err(ConfigError::MachineAlreadyRegistered {
                project_id: *project_id,
                machine_id: machine_id.to_string(),
            });
        }
        project.machines.insert(machine_id.to_string(), path);
        project.updated_at = Utc::now();
        Ok(())
    }

    /// 新增 sync:false 项目 —— 直接落到 local.yaml.private_projects。
    pub fn add_private(&mut self, project: PrivateProject) -> Result<(), ConfigError> {
        let local = self.local.as_mut().ok_or(ConfigError::NoLocalConfig)?;
        if project.sync {
            return Err(ConfigError::SyncMustBeFalse);
        }
        if local
            .private_projects
            .iter()
            .any(|p| p.name == project.name)
        {
            return Err(ConfigError::DuplicateName(project.name.clone()));
        }
        local.private_projects.push(project);
        Ok(())
    }

    /// 编辑共享项目 —— sync 字段**永远不允许改**（spec: sync 状态单向不可逆）。
    /// name / git_url / machines 可改。
    pub fn edit_shared(
        &mut self,
        id: &Uuid,
        edit: EditRegistryProject,
    ) -> Result<&RegistryProject, ConfigError> {
        let project = self
            .projects
            .get_mut(id)
            .ok_or(ConfigError::ProjectNotFound(*id))?;
        if let Some(name) = edit.name {
            project.name = name;
        }
        if let Some(git_url) = edit.git_url {
            project.git_url = git_url;
        }
        if let Some(machines) = edit.machines {
            project.machines = machines;
        }
        project.updated_at = Utc::now();
        Ok(project)
    }

    /// 删除共享项目（"改 sync 状态唯一路径 = 删除重建"的删除侧）。
    pub fn delete_shared(&mut self, id: &Uuid) -> Result<RegistryProject, ConfigError> {
        self.projects
            .remove(id)
            .ok_or(ConfigError::ProjectNotFound(*id))
    }

    pub fn delete_private(&mut self, name: &str) -> Result<PrivateProject, ConfigError> {
        let local = self.local.as_mut().ok_or(ConfigError::NoLocalConfig)?;
        let pos = local
            .private_projects
            .iter()
            .position(|p| p.name == name)
            .ok_or(ConfigError::PrivateNotFound(name.to_string()))?;
        Ok(local.private_projects.remove(pos))
    }

    /// 只落盘 local.yaml（私有项目 sync:false 专用，B2 变种洗白根治）。
    ///
    /// 决策 5：sync:false 私有项目**永不出机、永不碰共享 registry**。若走 `save_at`，其
    /// `create_dir_all(registry/projects)` 会在错仓（用户填错地址指向的普通仓）里凭空造出
    /// `registry/projects/` 空目录——把非法结构“洗白”成合法 dashboard-state 结构，骗过
    /// `resolve_status` 的 `InvalidStructure` 门禁，让后续出机点误判 Ready 而污染 origin。
    ///
    /// `save_local_only` 只序列化 `local` 并原子写 local.yaml，**绝不** `create_dir_all`
    /// registry 目录、**绝不**写 `registry/projects/*.yaml`，从根上断掉私有 CRUD 复用
    /// `save_at` 的洗白路径。local.yaml 本身在 `dashboard_state_root` 之外（固定路径），
    /// 写它不会触碰存储仓工作区。
    pub fn save_local_only(&self, local_path: &Path) -> Result<(), ConfigError> {
        if let Some(local) = &self.local {
            let yaml = serde_yaml::to_string(local)?;
            write_local_config_atomic(local_path, yaml.as_bytes())?;
        }
        Ok(())
    }

    /// 落盘：写 registry/projects/*.yaml + local.yaml。
    /// 每次写入前跑 jsonschema validator 拒收非法字段（Schema Allowlist 硬约束）。
    pub fn save_at(
        &self,
        dashboard_state_root: &Path,
        local_path: &Path,
    ) -> Result<(), ConfigError> {
        let registry_dir = dashboard_state_root.join("registry").join("projects");
        std::fs::create_dir_all(&registry_dir)?;
        // 写 registry: 每项目一文件
        for (id, proj) in &self.projects {
            // 校验前先转成 JSON Value（jsonschema 只吃 JSON）
            let json = serde_json::to_value(proj)?;
            validate_registry_project(&json)?;
            let yaml = serde_yaml::to_string(proj)?;
            let file = registry_dir.join(format!("{}.yaml", id));
            std::fs::write(file, yaml)?;
        }
        // 删除磁盘上已不在内存 registry 的 `<uuid>.yaml` —— delete_shared 只从内存 map 移除，
        // 若不在此同步删磁盘文件，下次 load_registry_projects glob 目录会把已删项目读回来"复活"。
        // 只删文件名能解析为 UUID 且不在 self.projects 的（非 UUID 文件名不碰，保守避免误删）。
        // 并发安全由上层 repo-global sidecar 锁保证：CRUD 串行，self.projects 是全量快照。
        if let Ok(entries) = std::fs::read_dir(&registry_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                if let Some(uuid) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    if !self.projects.contains_key(&uuid) {
                        std::fs::remove_file(&path)?;
                    }
                }
            }
        }
        // 写 local.yaml
        if let Some(local) = &self.local {
            let yaml = serde_yaml::to_string(local)?;
            write_local_config_atomic(local_path, yaml.as_bytes())?;
        }
        Ok(())
    }
}

/// 在目标目录先写完整临时文件，再一次性替换 `local.yaml`。这样未参与 writer
/// 锁的只读入口只会看到旧版或新版，不会读到 truncate→write 的半截 YAML。
pub(crate) fn write_local_config_atomic(
    local_path: &Path,
    contents: &[u8],
) -> Result<(), ConfigError> {
    let parent = local_path.parent().ok_or_else(|| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local.yaml path has no parent",
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let name = local_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("local.yaml");
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let mut cleanup = TemporaryLocalConfig::new(temporary.clone());
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    #[cfg(windows)]
    file.sync_all()?;
    #[cfg(not(windows))]
    file.flush()?;
    drop(file);
    replace_local_config(&temporary, local_path)?;
    cleanup.disarm();
    Ok(())
}

struct TemporaryLocalConfig {
    path: Option<PathBuf>,
}

#[cfg(all(test, windows))]
mod atomic_replace_tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn failed_replace_preserves_original_and_cleans_temporary() {
        let dir = std::env::temp_dir().join(format!("pd-atomic-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registry.yaml");
        std::fs::write(&path, b"old-complete").unwrap();
        // 真实 Windows 拒删除共享句柄让最终原子替换失败；旧文件不能先被截断。
        let held = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(write_local_config_atomic(&path, b"new-complete").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old-complete");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        drop(held);
        write_local_config_atomic(&path, b"new-complete").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new-complete");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}

impl TemporaryLocalConfig {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TemporaryLocalConfig {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(not(windows))]
fn replace_local_config(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_local_config(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 编辑 registry 项目的可选字段集合。sync 字段刻意不在这里 —— spec 硬约束不许改。
#[derive(Debug, Default)]
pub struct EditRegistryProject {
    pub name: Option<String>,
    pub git_url: Option<String>,
    pub machines: Option<BTreeMap<String, String>>,
}

pub fn default_local_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".project-dashboard")
        .join("local.yaml")
}

/// 数据目录根 fallback：`~/.project-dashboard/dashboard-state-clone`。
/// `local.yaml` 未配 `dashboard_state_root` 时用它（老配置 / 首次启动）。
pub fn default_dashboard_state_root_fallback() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".project-dashboard")
        .join("dashboard-state-clone")
}

/// 纯函数：据 `local.yaml` 的 `dashboard_state_root` 字段解析数据目录根。
/// `Some(非空)` → 取 `trim` 后的值（去首尾空白，防跨平台 fs 路径不一致）；`None` / 空串 /
/// 纯空白 → fallback。空串 / 纯空白按未配处理，防设置页清空后把根落到空路径。
pub fn resolve_dashboard_state_root(configured: Option<&str>) -> PathBuf {
    match configured {
        Some(s) if !s.trim().is_empty() => PathBuf::from(s.trim()),
        _ => default_dashboard_state_root_fallback(),
    }
}

/// 从指定 `local.yaml` 读数据目录根（可注入版，供测试 / 自定义部署）。
/// 文件不存在 → 静默 fallback（正常首次启动）；解析失败 → WARN + fallback（配置损坏，
/// 仍保证 App 能起、用户可进设置页修，但不静默吞掉损坏信号）。
pub fn load_dashboard_state_root_from(local_path: &Path) -> PathBuf {
    let configured = match load_local_config(local_path) {
        Ok(cfg) => cfg.and_then(|c| c.dashboard_state_root),
        Err(e) => {
            eprintln!(
                "[config] 读 local.yaml 解析数据目录根失败，回退默认目录（可进设置页修复）：{e}"
            );
            None
        }
    };
    resolve_dashboard_state_root(configured.as_deref())
}

/// RepoWiki 只读 IPC 专用：解析 dashboard-state 根，但绝不运行 legacy 迁移备份。
pub fn load_dashboard_state_root_from_read_only(local_path: &Path) -> PathBuf {
    let configured = load_local_config_read_only(local_path)
        .ok()
        .flatten()
        .and_then(|config| config.dashboard_state_root);
    resolve_dashboard_state_root(configured.as_deref())
}

/// 生产入口：读固定 `~/.project-dashboard/local.yaml` 解析数据目录根。
/// **每次调用都重读**（不做启动期缓存，审计 Medium-5）——设置页改路径保存后即生效，无缓存失效窗口。
pub fn load_dashboard_state_root() -> PathBuf {
    load_dashboard_state_root_from(&default_local_config_path())
}

pub fn load_dashboard_state_root_read_only() -> PathBuf {
    load_dashboard_state_root_from_read_only(&default_local_config_path())
}

fn load_registry_projects(dir: &Path) -> Result<BTreeMap<Uuid, RegistryProject>, ConfigError> {
    let mut out = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let proj: RegistryProject = serde_yaml::from_str(&text)
            .map_err(|e| ConfigError::YamlParse(path.display().to_string(), e))?;
        // 加载时也过一遍 schema 校验（拒收未声明字段污染）
        let json = serde_json::to_value(&proj)?;
        validate_registry_project(&json)?;
        out.insert(proj.project_id, proj);
    }
    Ok(out)
}

fn load_local_config(path: &Path) -> Result<Option<LocalConfig>, ConfigError> {
    load_local_config_inner(path, true)
}

/// 供桌面只读 IPC 使用，不执行旧配置迁移或备份写入。
pub fn load_local_config_read_only(path: &Path) -> Result<Option<LocalConfig>, ConfigError> {
    load_local_config_inner(path, false)
}

fn load_local_config_inner(
    path: &Path,
    run_pending_preflight: bool,
) -> Result<Option<LocalConfig>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if run_pending_preflight {
                preflight_migrate_pending(&text, path);
            }
            let cfg: LocalConfig = serde_yaml::from_str(&text)
                .map_err(|e| ConfigError::YamlParse(path.display().to_string(), e))?;
            Ok(Some(cfg))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// v4.2 Decision 15 迁移期 preflight（§16.6）：老 yaml 里若含非空 `pending_sync_projects`，
/// dump 内容到 `<home>/.project-dashboard/backups/pending_migration_<ts>.yaml` + stderr WARN。
///
/// 失败静默（保底 —— 让 App 正常启动比 backup 更重要）。
fn preflight_migrate_pending(raw_yaml: &str, local_path: &Path) {
    let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(raw_yaml) else {
        return;
    };
    let Some(seq) = val
        .get("pending_sync_projects")
        .and_then(|v| v.as_sequence())
    else {
        return;
    };
    if seq.is_empty() {
        return;
    }
    let backup_dir = local_path
        .parent()
        .map(|p| p.join("backups"))
        .unwrap_or_else(|| PathBuf::from("."));
    if std::fs::create_dir_all(&backup_dir).is_err() {
        return;
    }
    let ts = Utc::now().format("%Y%m%dT%H%M%S");
    let backup_path = backup_dir.join(format!("pending_migration_{ts}.yaml"));
    let dump = match serde_yaml::to_string(&serde_yaml::Value::Mapping({
        let mut m = serde_yaml::Mapping::new();
        m.insert(
            serde_yaml::Value::String("pending_sync_projects".to_string()),
            serde_yaml::Value::Sequence(seq.clone()),
        );
        m
    })) {
        Ok(s) => s,
        Err(_) => return,
    };
    if std::fs::write(&backup_path, dump).is_ok() {
        eprintln!(
            "[project-dashboard] v4.2 迁移警告：本机 {} 含 {} 个遗留 `pending_sync_projects` 条目，\
             已 dump 到 {} 供手工按 tasks §16.10 runbook 迁移。App 启动会 silently ignore 该字段。",
            local_path.display(),
            seq.len(),
            backup_path.display()
        );
    }
}

fn serialize_true<S: serde::Serializer>(v: &bool, s: S) -> Result<S::Ok, S::Error> {
    if !*v {
        return Err(serde::ser::Error::custom(
            "RegistryProject.sync must be true; sync:false 项目应放 local.yaml.private_projects",
        ));
    }
    s.serialize_bool(true)
}

fn deserialize_true<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = bool::deserialize(d)?;
    if !v {
        return Err(serde::de::Error::custom(
            "RegistryProject.sync must be true",
        ));
    }
    Ok(v)
}

fn serialize_false<S: serde::Serializer>(v: &bool, s: S) -> Result<S::Ok, S::Error> {
    if *v {
        return Err(serde::ser::Error::custom(
            "PrivateProject.sync must be false",
        ));
    }
    s.serialize_bool(false)
}

fn deserialize_false<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = bool::deserialize(d)?;
    if v {
        return Err(serde::de::Error::custom(
            "PrivateProject.sync must be false",
        ));
    }
    Ok(v)
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    YamlParse(String, serde_yaml::Error),
    YamlWrite(serde_yaml::Error),
    JsonConvert(serde_json::Error),
    Schema(SchemaError),
    ProjectNotFound(Uuid),
    PrivateNotFound(String),
    DuplicateProjectId(Uuid),
    DuplicateName(String),
    NoLocalConfig,
    SyncMustBeFalse,
    MachineAlreadyRegistered {
        project_id: Uuid,
        machine_id: String,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::YamlParse(p, e) => write!(f, "yaml parse error in {p}: {e}"),
            Self::YamlWrite(e) => write!(f, "yaml write error: {e}"),
            Self::JsonConvert(e) => write!(f, "json convert error: {e}"),
            Self::Schema(e) => write!(f, "schema violation: {e}"),
            Self::ProjectNotFound(id) => write!(f, "project not found: {id}"),
            Self::PrivateNotFound(name) => write!(f, "private project not found: {name}"),
            Self::DuplicateProjectId(id) => write!(f, "duplicate project_id: {id}"),
            Self::DuplicateName(name) => write!(f, "duplicate project name: {name}"),
            Self::NoLocalConfig => write!(f, "local.yaml not loaded"),
            Self::SyncMustBeFalse => write!(f, "PrivateProject.sync must be false"),
            Self::MachineAlreadyRegistered { project_id, machine_id } => write!(
                f,
                "machine_id={machine_id} 已在 project {project_id} 注册过（关联操作不能重复注册同一台机）"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_yaml::Error> for ConfigError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::YamlWrite(e)
    }
}
impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::JsonConvert(e)
    }
}
impl From<SchemaError> for ConfigError {
    fn from(e: SchemaError) -> Self {
        Self::Schema(e)
    }
}
