// M3 §10 同步载体 —— `dashboard-state` 私有仓 clone / pull / push 封装（shell out git）。
// 双文件数据模型：
//   - `machines/<id>/snapshot.json`：当前态覆盖写
//   - `machines/<id>/events.ndjson`：事件流 append-only（snapshot 未变时跳过 events + commit + push）

use crate::schema::{validate_event, validate_snapshot, SchemaError};
use crate::tracker::{CommitEntry, DangerousState, ProjectStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const CURRENT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub machine_id: String,
    /// 当次真实主机名，**纯展示字段，不参与任何匹配**。存它是为了让主机名漂移可见：
    /// machine_id 不变而此值变了，就能断定是同一台机改了名，而不是又多出一台机器。
    /// `#[serde(default)]` 保证老 snapshot（无此字段）照样能反序列化。
    #[serde(default)]
    pub hostname: String,
    pub updated_at: DateTime<Utc>,
    pub projects: Vec<SnapshotProject>,
}

/// snapshot.json 里单项目条目 —— 字段严格对齐 `schemas/snapshot.schema.json`。
///
/// v4.3 新增两个可选字段：`head_full`（HEAD 完整 40 位 sha）+ `recent_commits`（三态语义）。
/// 都带 `#[serde(default)]` 向后兼容 v4.2 老 snapshot（反序列化后新字段全 None）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotProject {
    pub project_id: Option<Uuid>,
    pub name: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// v4.3：HEAD 完整 40 位 sha。aggregate 严格相等比对用。老 snapshot 读到时为 None。
    #[serde(default)]
    pub head_full: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub dirty: u32,
    pub untracked: u32,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub last_fs_change_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub progress: Option<crate::tracker::Progress>,
    pub dangerous_state: Option<DangerousState>,
    /// v4.3：`git log -n 20` 最近提交历史，三态语义（None / Some(vec![]) / Some(非空)）。
    /// 参见 `CommitEntry` 与 `tracker.rs` 里的三态语义详解。
    #[serde(default)]
    pub recent_commits: Option<Vec<CommitEntry>>,
}

impl From<ProjectStatus> for SnapshotProject {
    fn from(p: ProjectStatus) -> Self {
        Self {
            project_id: p.project_id,
            name: p.name,
            branch: p.branch,
            head: p.head,
            head_full: p.head_full,
            upstream: p.upstream,
            ahead: p.ahead,
            behind: p.behind,
            dirty: p.dirty,
            untracked: p.untracked,
            last_commit_at: p.last_commit_at,
            last_fs_change_at: p.last_fs_change_at,
            collected_at: p.collected_at,
            progress: p.progress,
            dangerous_state: p.dangerous_state,
            recent_commits: p.recent_commits,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Hook,
    Cron,
    AppOpen,
    Tick,
    Manual,
}

impl Trigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Cron => "cron",
            Self::AppOpen => "app-open",
            Self::Tick => "tick",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Commit,
    #[serde(rename = "dirty-change")]
    DirtyChange,
    #[serde(rename = "head-change")]
    HeadChange,
    Collect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: DateTime<Utc>,
    pub trigger: Trigger,
    pub project_id: Option<Uuid>,
    pub action: Action,
    pub from_head: Option<String>,
    pub to_head: Option<String>,
    pub dirty: u32,
    pub untracked: u32,
    pub dangerous_state: Option<DangerousState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<bool>,
    /// v4.2 Decision 15 采集器可选防伪校验：`git remote get-url origin` 与 registry `git_url` /
    /// `git_url_aliases` 全不匹配时追加。snapshot 依然正常写入（不阻塞采集）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_remote_mismatch: Option<GitRemoteMismatch>,
    /// v4.2 Decision 15：`git remote get-url origin` 非 0 退出（无 origin / 非 git 仓 / 命令失败）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_remote_failed: Option<String>,
}

/// v4.2 防伪校验诊断行。字段 URL 已归一化（若归一化失败则填原始值）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitRemoteMismatch {
    pub expected: String,
    pub actual: String,
}

/// P0-5：repo-global 排他同步锁（RAII，Drop 时 File 关闭、OS 锁自动释放）。持有期间
/// 独占 dashboard-state 工作区的 snapshot / events / git 写，串行化多个采集进程
/// （每次 commit 触发的 hook CLI + 手动 sync + 计划任务全量），避免并发
/// read→merge→write→commit→push 互相覆盖、写出半截 JSON、或 git 工作区打架。
#[derive(Debug)]
pub struct RepoLock {
    _file: File,
}

/// 本机 dashboard-state 工作区结构：`<root>/registry/projects/*.yaml` + `<root>/machines/<id>/`
pub struct DashboardStateRepo {
    pub root: PathBuf,
    pub machine_id: String,
}

impl DashboardStateRepo {
    pub fn new(root: impl Into<PathBuf>, machine_id: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            machine_id: machine_id.into(),
        }
    }

    pub fn machine_dir(&self) -> PathBuf {
        self.root.join("machines").join(&self.machine_id)
    }

    pub fn snapshot_path(&self) -> PathBuf {
        self.machine_dir().join("snapshot.json")
    }

    pub fn events_path(&self) -> PathBuf {
        self.machine_dir().join("events.ndjson")
    }

    /// repo-global 排他锁文件路径 —— 放在 `dashboard_state_root` 的**同级兄弟**位置
    /// （`<root>.sync.lock`），而非 root 内部的 `.git/`。关键理由：锁的存在不再依赖 `.git`
    /// 是否已 clone，first-run（未 clone）与 clone 完成后所有入口都锁**同一个文件**，从根上
    /// 消除"按有无 .git 切换锁路径"带来的 clone 过渡期竞争窗口（TOCTOU）。
    ///
    /// 路径只依赖 `root` 不依赖 machine_id，保证同一 clone 的所有进程（不同 machine_id 的
    /// repo 实例）锁到同一把锁。文件落在 root 之外、不属任何 git 工作区，绝不会被 `git add` 收录。
    fn lock_path(&self) -> PathBuf {
        // 契约：root 应为绝对且具名（生产恒为 `~/.project-dashboard/dashboard-state-clone`）。
        // 对相对 / 无 basename 的异常 root（"."/".."/空/文件系统根 `/`），先 std::path::absolute
        // 绝对化（纯路径运算、不碰文件系统、不要求 root 存在，对已绝对的 root 是 no-op），避免
        // 同一 repo 在不同 CWD 下锁到不同相对文件、互斥失效。symlink 与真实路径混用的分裂不在此
        // 消解（需 canonicalize + root 已存在），依赖调用方统一用同一路径表示
        // （default_dashboard_state_root 已保证）。
        let base = std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone());
        let mut name = base
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| std::ffi::OsString::from("dashboard-state"));
        name.push(".sync.lock");
        match base.parent() {
            Some(parent) => parent.join(name),
            // 文件系统根（如 `/`）：parent 与 file_name 均为 None。用 base.join 落到根下的绝对
            // 锁文件（`/dashboard-state.sync.lock`），而非退化成相对 `PathBuf::from(name)`——后者
            // 会让同一根在不同 CWD 下锁到各自的 `./dashboard-state.sync.lock`、互斥失效。
            None => base.join(name),
        }
    }

    /// 打开并排他锁定 sidecar 文件（open + 50ms 轮询 try_lock），strict / allow 两个入口共用。
    /// 父目录（root 的上级，如 `~/.project-dashboard/`）缺失时先 `create_dir_all`，保证
    /// first-run（父目录尚未建）也能落锁文件；到 `timeout` 仍抢不到 → `Err(LockBusy)`。
    ///
    /// 用 OS advisory 锁（`std::fs::File::try_lock`，Rust 1.89+ 稳定）：进程崩溃时内核自动
    /// 释放，无 stale 文件协议、无第三方依赖。锁只协调本机同一 clone 的多进程；跨机 clone
    /// 同时 push 的冲突由 git non-fast-forward 兜（不在本锁职责内）。
    fn lock_sidecar(&self, timeout: Duration) -> Result<RepoLock, SyncError> {
        let path = self.lock_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(RepoLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if start.elapsed() >= timeout {
                        return Err(SyncError::LockBusy);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(SyncError::Io(e)),
            }
        }
    }

    /// 严格锁（采集 collect_single / manual_sync、pull_registry 用）：**先取 sidecar 锁独占，
    /// 再校验 `.git` 存在**——顺序不能反，先独占再检查才能保证检查到的 `.git` 状态在整个临界区
    /// 内稳定。未 clone（无 `.git`）→ `NotFound`，让上层暴露"未初始化"而非在假仓上继续写。
    ///
    /// 场景化超时：hook / 计划任务采集传长超时（可等，避免丢采集）；UI 触发
    /// （manual_sync / pull_registry）传短超时（不冻结界面，超时向用户提示"同步进行中"）。
    pub fn acquire_lock(&self, timeout: Duration) -> Result<RepoLock, SyncError> {
        let lock = self.lock_sidecar(timeout)?;
        let git_dir = self.root.join(".git");
        if !git_dir.exists() {
            return Err(SyncError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "dashboard-state 未初始化（{} 不存在），请先 clone",
                    git_dir.display()
                ),
            )));
        }
        Ok(lock)
    }

    /// 宽松锁（CRUD：add/edit/delete_project + update_settings 用）：**无条件**取同一把
    /// sidecar 锁，不校验 `.git`。first-run（尚未 clone、无 `.git`）时 CRUD 仍要互斥写
    /// local.yaml / registry，这里必须真正拿到锁——旧的"无 .git 就返 None 放行"会漏掉
    /// first-run 下 CRUD↔CRUD 对 local.yaml 的并发覆盖（本次 P1 根因）。
    ///
    /// clone 完成后它与严格入口锁**同一文件**，CRUD 写 local.yaml 与采集在锁内读解析
    /// local.yaml 也天然互斥（采集虽不写 local，但会读，需防非原子 fs::write 的半截 YAML）。
    pub fn acquire_lock_allow_uninit(&self, timeout: Duration) -> Result<RepoLock, SyncError> {
        self.lock_sidecar(timeout)
    }

    /// 读 snapshot.json，用于 v4.3 §1.4 collect_single 的 **read → merge → write** 语义：
    /// 先读旧 snapshot 里的全部 projects，按 project_id 更新单项后写回，**绝不用
    /// `vec![单项]` 覆盖整机 snapshot**（否则其他项目会暂时从 Fleet Board 消失）。
    ///
    /// 返回语义（P0-3）：
    /// - `Ok(None)` —— **仅**文件不存在（首次采集），调用方退到 `Vec::new()` 起手安全；
    /// - `Ok(Some(s))` —— 读取 + 反序列化 + schema 校验 + machine_id 一致性全通过；
    /// - `Err(..)` —— 文件存在但 IO 出错 / JSON 非法 / 结构不符 schema / machine_id 不匹配。
    ///   调用方**必须**透传中止同步、保留磁盘原文件，**绝不能**退空 Vec 覆盖整机 snapshot。
    ///
    /// 旧实现 `serde_json::from_slice(..).ok()` 把**一切**读取失败（含反序列化错）吞成
    /// `None`，与"文件不存在"混为一谈 → collect_single 误判首次采集、空 Vec 起手 →
    /// 一次解析失败就把整机其他项目从 Fleet Board 抹掉。
    ///
    /// 未知字段 fail-closed（P0-3-b）：`Snapshot` / `SnapshotProject` 带
    /// `deny_unknown_fields`，未来版本写入的新字段会让反序列化直接 `Err`、中止本机同步。
    /// 这是刻意的——旧客户端若"忽略未知字段"读进来，随后 collect_single 重建整份 snapshot
    /// 写回时会把这些新字段**静默丢弃**（reader-compatible 但非 read-modify-write compatible），
    /// 反而悄悄抹掉别机的新数据。显式失败比静默丢字段安全，也与 `CommitEntry` 的
    /// `deny_unknown_fields` 一致（不再局部严格局部宽松）。
    pub fn read_snapshot(&self) -> Result<Option<Snapshot>, SyncError> {
        let bytes = match std::fs::read(self.snapshot_path()) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SyncError::Io(e)),
        };
        // 非法 JSON / 结构不兼容（缺必需字段、类型错）→ Json err，不再被静默吞成 None
        let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
        // 读取端补 schema 校验（与 write_snapshot 对称）：serde 类型解析不管 head_full 的
        // 40-hex pattern / recent_commits maxItems 等值约束，这里补上。
        let json = serde_json::to_value(&snapshot)?;
        validate_snapshot(&json)?;
        // 读的是 machines/<self.machine_id>/snapshot.json，其内 machine_id 必须匹配自身，
        // 否则是文件错位 / 污染（比如手工挪错目录），中止而非拿别机数据 merge。
        if snapshot.machine_id != self.machine_id {
            return Err(SyncError::MachineIdMismatch {
                expected: self.machine_id.clone(),
                found: snapshot.machine_id,
            });
        }
        Ok(Some(snapshot))
    }

    /// 写 snapshot.json 覆盖写。校验通过才落盘。
    pub fn write_snapshot(&self, snapshot: &Snapshot) -> Result<(), SyncError> {
        std::fs::create_dir_all(self.machine_dir())?;
        let json = serde_json::to_value(snapshot)?;
        validate_snapshot(&json)?;
        let text = serde_json::to_string_pretty(snapshot)?;
        std::fs::write(self.snapshot_path(), text)?;
        Ok(())
    }

    /// 追加一行 event 到 events.ndjson。校验通过才写。
    pub fn append_event(&self, event: &Event) -> Result<(), SyncError> {
        std::fs::create_dir_all(self.machine_dir())?;
        let json = serde_json::to_value(event)?;
        validate_event(&json)?;
        let line = serde_json::to_string(event)?;
        let path = self.events_path();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{line}")?;
        Ok(())
    }

    /// 计算 snapshot 的**语义**指纹 —— 只对 `projects` 数组序列化后哈希，**排除
    /// `updated_at` / `collected_at` 时间戳**，保证 git 状态未变时 hash 稳定。
    ///
    /// 审计对账 P0-B（2026-07-06）：原实现对整个文件字节哈希，导致每次 tick 生成新
    /// `updated_at` → hash 每次都变 → spec 「snapshot 未变时跳过 events + commit + push」永远不生效。
    pub fn snapshot_hash(&self) -> Option<String> {
        let bytes = std::fs::read(self.snapshot_path()).ok()?;
        let snap: Snapshot = serde_json::from_slice(&bytes).ok()?;
        Self::hash_projects(&snap.projects)
    }

    /// 供 sync_cycle 直接对 in-memory 的新 snapshot 算指纹（不用落盘再读一次）。
    pub fn hash_of(snapshot: &Snapshot) -> Option<String> {
        Self::hash_projects(&snapshot.projects)
    }

    fn hash_projects(projects: &[SnapshotProject]) -> Option<String> {
        // 逐项目算稳定投影 —— 排除 collected_at / last_fs_change_at 时间戳，
        // 只保留跟 git 状态相关的字段。
        //
        // v4.3 §1.3 R2-P0-A 修：投影加 `head_full` + `recent_commits_shas`（仅 sha_full 序列，
        // 不含 subject/author/time —— 避免 amend commit 只改 subject 时触发 push 噪音）。
        // 首次升级到 v4.3 时因新字段引入 → 稳定 hash 不同 → 触发首次 push，让
        // recent_commits 得以写入 dashboard-state（否则 spec §"snapshot 未变时跳过"会永远压制）。
        //
        // 顶层 `hostname` **刻意不进哈希**（此函数只吃 projects）：它是诊断用展示字段，
        // 单纯改主机名不该触发一次跨机 push。下次任意项目状态变化时它会顺带写上去；
        // 本机的漂移轨迹另有 local.yaml 的 hostname_history 完整记录，不依赖这条链路。
        #[derive(Serialize)]
        struct Stable<'a> {
            project_id: Option<Uuid>,
            name: &'a str,
            branch: Option<&'a str>,
            head: Option<&'a str>,
            head_full: Option<&'a str>,
            upstream: Option<&'a str>,
            ahead: Option<u32>,
            behind: Option<u32>,
            dirty: u32,
            untracked: u32,
            last_commit_at: Option<DateTime<Utc>>,
            progress: Option<&'a crate::tracker::Progress>,
            dangerous_state: Option<DangerousState>,
            /// 三态语义压平成 `Option<Vec<&str>>`：None 保 None、Some(vec![])
            /// 保 Some(vec![])、Some(非空)取每条的 sha_full。**不 hash subject/author/time**。
            recent_commits_shas: Option<Vec<&'a str>>,
        }
        let stable: Vec<Stable<'_>> = projects
            .iter()
            .map(|p| Stable {
                project_id: p.project_id,
                name: p.name.as_str(),
                branch: p.branch.as_deref(),
                head: p.head.as_deref(),
                head_full: p.head_full.as_deref(),
                upstream: p.upstream.as_deref(),
                ahead: p.ahead,
                behind: p.behind,
                dirty: p.dirty,
                untracked: p.untracked,
                last_commit_at: p.last_commit_at,
                progress: p.progress.as_ref(),
                dangerous_state: p.dangerous_state,
                recent_commits_shas: p
                    .recent_commits
                    .as_ref()
                    .map(|v| v.iter().map(|c| c.sha_full.as_str()).collect()),
            })
            .collect();
        let bytes = serde_json::to_vec(&stable).ok()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        Some(format!("{:x}", h.finalize()))
    }

    /// 用 shell out `git` 做仓操作 —— v4.1 定死走命令，不引 `git2`。
    ///
    /// clone 中途失败（网络断 / 凭证错 / 仓不存在）可能已建半初始化 root。此处**清理残留目标
    /// 目录**，保证"配置即拉"可重复执行、不留半成品让下次误判 `Conflict{NotGitDir}`。
    /// 契约：只在 `NeedsInit`（root 空 / 不存在）后被调用，故失败清理不会误删用户既有数据。
    pub fn git_clone(&self, remote: &str) -> Result<(), SyncError> {
        if let Some(parent) = self.root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 审计 P2：失败清理只删**本次 clone 新建**的 root（clone 前不存在）。clone 前已存在的
        // 目录一律不删，避免 git_clone 被误用于非空 root 时误删用户既有数据（虽然生产路径只在
        // NeedsInit=root 空/不存在 时才调它，这里做函数自证的防御）。
        let root_existed_before = self.root.exists();
        match run_git(None, &["clone", remote, &self.root.to_string_lossy()]) {
            Ok(_) => Ok(()),
            Err(e) => {
                if !root_existed_before && self.root.exists() {
                    let _ = std::fs::remove_dir_all(&self.root);
                }
                Err(e)
            }
        }
    }

    /// 采集 clone 状态观测值（IO 层，供 `resolve_status` 纯函数判态）：root 是否存在 / 是否空、
    /// 有无 `.git`、以及 root 的 `git remote get-url origin`（有 `.git` 才读）。把所有 fs / git
    /// 副作用收敛在此，`resolve_status` 保持纯函数便于隔离测试。
    pub fn observe(&self) -> Observed {
        let root_exists = self.root.exists();
        let has_git = self.root.join(".git").exists();
        let root_empty = if root_exists {
            std::fs::read_dir(&self.root)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
        } else {
            true
        };
        let origin = if has_git {
            crate::tracker::get_git_remote_url(&self.root)
        } else {
            None
        };
        Observed {
            root_exists,
            root_empty,
            has_git,
            has_registry: is_dashboard_state_repo(&self.root),
            origin,
        }
    }

    pub fn git_pull(&self) -> Result<(), SyncError> {
        run_git(Some(&self.root), &["pull", "--ff-only"])?;
        Ok(())
    }

    pub fn git_push(&self) -> Result<(), SyncError> {
        run_git(Some(&self.root), &["push"])?;
        Ok(())
    }

    /// 只让共享仓受管的两个顶层目录进入 commit：`registry/`（项目注册表）+ `machines/`
    /// （本机 `snapshot.json` / `events.ndjson` 都在 `machines/<id>/` 下）。
    ///
    /// **不用 `git add -A`**（LEAK-05）：数据目录根可由用户在设置页配置，若配成含
    /// `local.yaml` / `.env` / 密钥的目录，`-A` 会把这些无关文件一并 commit 并 push 到
    /// 跨机共享仓造成泄露。显式白名单比依赖 `.gitignore` 黑名单更稳。
    ///
    /// 先 `git reset` 清空 index 再白名单 add：光靠 `git add <白名单>` 只挡"本次新增"，
    /// **挡不住 index 里此前已被 stage 的文件**（历史 `add -A` 遗留 / 用户手动 `git add`
    /// 敏感根文件）——数据目录可配，不能假定 index 干净。reset 后 index 只剩白名单，后续
    /// `git_commit` 自然只提交受管数据，无需在 commit 侧再加 pathspec，从根上堵死
    /// commit 侧的泄露通道。
    ///
    /// `machines/` 首次同步前可能不存在（`registry/` 由 clone 保证存在），故存在才 add，
    /// 避免 pathspec 不匹配 `fatal`（正常 sync_cycle 中 `write_snapshot` 已先建 machines）。
    /// `-A` 让删除也被 stage（删项目 → registry 下文件删除需同步）。
    /// emergency marker（仓根 `.emergency-purge-*`）走人工 push，不经此路径。
    pub fn git_add_all(&self) -> Result<(), SyncError> {
        run_git(Some(&self.root), &["reset", "-q"])?;
        run_git(Some(&self.root), &["add", "-A", "--", "registry"])?;
        if self.root.join("machines").exists() {
            run_git(Some(&self.root), &["add", "-A", "--", "machines"])?;
        }
        Ok(())
    }

    pub fn git_commit(&self, message: &str) -> Result<(), SyncError> {
        // 允许空提交？不，spec 明说 snapshot 未变时跳过 commit
        run_git(Some(&self.root), &["commit", "-m", message])?;
        Ok(())
    }

    /// index 里是否有待提交内容。
    ///
    /// `git commit` 在无改动时退出码非 0，会被 `run_git` 判成失败；调用方需要先问一句
    /// 「到底有没有东西可提交」才能区分「真出错」和「本来就没得提交」。
    /// 用 `diff --cached --name-only`（无论有无改动都退出 0），输出非空即有。
    pub fn git_has_staged_changes(&self) -> Result<bool, SyncError> {
        let out = run_git(Some(&self.root), &["diff", "--cached", "--name-only"])?;
        Ok(!out.trim().is_empty())
    }

    /// 一次完整同步的锁内编排（决策 4）：land 段（write snapshot + append events）与 git 段
    /// （add + commit + push）在调用方持有的 repo 锁内完成，避免中间插入其他写入；由 push: bool
    /// 决定是否执行 git 段；git_push 单独 catch（失败返回 pushed: false 不抛 Err，部分成功）。
    ///
    /// P0-I（2026-07-06）：先用 `hash_of(&snapshot)` 算内存 hash，与磁盘 prev_hash 比较；相同且非 force 直接返 Ok(false)。
    pub fn sync_cycle(
        &self,
        snapshot: Snapshot,
        events: Vec<Event>,
        trigger: Trigger,
        force: bool,
        push: bool,
    ) -> Result<SyncCycleResult, SyncError> {
        let prev_hash = self.snapshot_hash();
        let new_hash = Self::hash_of(&snapshot);
        if !force && prev_hash == new_hash {
            // spec: snapshot 未变时跳过一切 —— 不落盘、不写 events、不 commit、不 push
            return Ok(SyncCycleResult { changed: false, pushed: false });
        }
        // land 段：落盘 snapshot + 追加 events
        self.write_snapshot(&snapshot)?;
        for e in events {
            self.append_event(&e)?;
        }
        if !push {
            // land-only：不碰 git，本次变更留待后续 push=true 的 sync_cycle 一起带走
            return Ok(SyncCycleResult { changed: true, pushed: false });
        }
        // git 段：add + commit（失败向上抛 Err）；push 单独 catch（失败不抛，返回 pushed: false）
        self.git_add_all()?;
        let msg = format!(
            "sync[{}]: machine={} at {}",
            trigger.as_str(),
            self.machine_id,
            snapshot.updated_at.to_rfc3339()
        );
        self.git_commit(&msg)?;
        let pushed = self.git_push().is_ok();
        Ok(SyncCycleResult { changed: true, pushed })
    }

    /// 隐私事故标记：`.emergency-purge-<ts>` 空文件写入仓根 + push。
    pub fn write_emergency_marker(&self) -> Result<PathBuf, SyncError> {
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let path = self.root.join(format!(".emergency-purge-{ts}"));
        std::fs::write(&path, "")?;
        Ok(path)
    }

    /// 检测另一台机 push 的隐急标记 —— App 打开时用来引导本机清理老 clone。
    pub fn list_emergency_markers(&self) -> Vec<PathBuf> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".emergency-purge-") {
                    Some(e.path())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// sync_cycle 的返回值（决策 4：编排结果，区分 changed 与 pushed）。
#[derive(Debug, Clone, PartialEq)]
pub struct SyncCycleResult {
    /// snapshot 语义是否变化（= 本次是否落盘 write_snapshot + append_events）。
    /// hash-gate：仅 changed=true 时才走 commit+push（除非 force=true 覆盖）。
    pub changed: bool,
    /// push 是否成功。push 失败（网络 / 认证 / 离线）时也是 false（部分成功不抛 Err）。
    pub pushed: bool,
}

/// dashboard-state 本地 clone 的稳定状态（砍存储仓 A→B 切换后**封闭四态**，spec 明文"稳定态仅
/// 四种"，不加"待切换 / 原子切换 / crash 恢复"过渡态）。
#[derive(Debug, Clone, PartialEq)]
pub enum StateRepoStatus {
    /// 未配置 remote 且本地无 `.git` —— 无法 clone 也无法同步，入口对 sync:true 共享操作 no-op 提示。
    Unconfigured,
    /// 已配置 remote 且本地 root 空 / 不存在 —— 触发配置即拉 `git_clone`。
    NeedsInit,
    /// 本地就绪：root 有 `.git`（配了 remote 时额外要求 origin==配置值；未配 remote 的存量老机器
    /// 有 `.git` 即视为就绪，用现有 origin 续同步）。
    Ready,
    /// 冲突态：不自动 clone / 覆盖，提示用户手动处理（删目录重配）。
    Conflict(ConflictReason),
}

/// Conflict 细分原因，决定 UI 提示与引导动作。
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictReason {
    /// root 非空但无 `.git`（如 `add_project` 曾向未初始化 root 写过 registry）——不静默覆盖。
    NotGitDir,
    /// root 有 `.git` 但 origin ≠ 配置 remote —— 不静默 `set-url`，引导用户删目录重配。
    RemoteMismatch {
        configured: String,
        actual: Option<String>,
    },
    /// root 有 `.git` 但不是合法 dashboard-state 结构（无 `registry/projects`）——用户填错地址
    /// 指向普通仓 / 空仓，不静默写入污染，引导删目录重配。映射复用稳定 tag `INVALID_STRUCTURE`。
    InvalidStructure,
}

/// clone 状态判定的观测输入（`DashboardStateRepo::observe` 采集，纯函数据此判态，隔离 IO 便于测试）。
#[derive(Debug, Clone)]
pub struct Observed {
    pub root_exists: bool,
    pub root_empty: bool,
    pub has_git: bool,
    /// root 是否为合法 dashboard-state 结构（含 `registry/projects`）。决策 3：并入判态，让有
    /// `.git` 但结构非法的普通仓被判 `Conflict(InvalidStructure)`，跨出机 / CRUD 入口一致拦截。
    pub has_registry: bool,
    /// root 的 `git remote get-url origin`（已 trim）；无 origin / 非 git → None。
    pub origin: Option<String>,
}

/// 纯函数：据观测值 + 配置 remote 判 clone 状态。**不跑 git、不碰 fs**，IO 全部由 `observe` 先采好。
///
/// 存量兼容决策（2026-07-16，cc 判断，**偏离 spec 字面"未配 remote 即 Unconfigured no-op"**）：
/// root 已有 `.git` 时一律视为就绪（Ready），无论是否配 remote——
///   - 配了 remote：额外校验 origin==配置值，不符 → `Conflict{RemoteMismatch}`（不静默 set-url）；
///   - 未配 remote（存量老机器升级后新字段默认 None / 现有集成测试）：用现有 origin 续同步。
/// no-op 的 `Unconfigured` 在两种情形触发：①"无 `.git` 且未配 remote"（真的无法 clone 也无法
/// 同步）；②有 `.git` 但用户**显式清空**过 remote（`remote_cleared=true`，决策 2，撤销后停用旧
/// origin）。存量老机器"有 `.git`、未配 remote、从没清空"仍判 Ready 续同步（兼容不破）。
/// 理由：spec 门禁本意是防"没 clone 又没地址却假装同步"，而 root 有 `.git` 且非显式清空时同步
/// 能正常工作；字面 no-op 会让存量老机器同步静默死 + 破坏全部现有采集测试。此决策已过审计复核。
pub fn resolve_status(
    observed: &Observed,
    configured_remote: Option<&str>,
    remote_cleared: bool,
) -> StateRepoStatus {
    let configured = configured_remote
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if observed.has_git {
        // 决策 3：有 .git 但非合法 dashboard-state 结构（无 registry/projects）→ InvalidStructure，
        // 在 origin 匹配前拦（结构比 origin 更根本），挡「填错地址指向普通仓」被出机 / CRUD 污染。
        if !observed.has_registry {
            return StateRepoStatus::Conflict(ConflictReason::InvalidStructure);
        }
        match configured {
            Some(remote) => {
                let origin_matches = observed
                    .origin
                    .as_deref()
                    .map(str::trim)
                    .map(|o| o == remote)
                    .unwrap_or(false);
                if origin_matches {
                    StateRepoStatus::Ready
                } else {
                    StateRepoStatus::Conflict(ConflictReason::RemoteMismatch {
                        configured: remote.to_string(),
                        actual: observed.origin.clone(),
                    })
                }
            }
            // 有 .git 但未配 remote：区分「用户显式清空」vs「存量老机器从没配过」
            // （磁盘信号相同，靠持久化的 remote_cleared 意图位判别，见决策 2）。
            None => {
                if remote_cleared {
                    // 显式清空 → 停用旧 origin，不再续同步（撤销真正生效）
                    StateRepoStatus::Unconfigured
                } else if observed.origin.is_some() {
                    // 存量老机器从没配过 remote、但实际有 origin → 用现有 origin 续同步（兼容不破）
                    StateRepoStatus::Ready
                } else {
                    // 三方评审收窄（2026-07-21，P0）：既没配 remote、实际也没有 origin →
                    // 判 Unconfigured，而非无条件 Ready。否则 manual_sync 前置 pull（决策 5）会对这种
                    // 「无处可 pull/push」的仓必然失败、把整个同步中止。此前无条件 Ready 是语义漏洞：
                    // 有 `.git` 但既无配置 remote 又无 origin 的仓，本就无法出机，应显式 Unconfigured no-op。
                    StateRepoStatus::Unconfigured
                }
            }
        }
    } else if observed.root_exists && !observed.root_empty {
        // root 非空但无 .git —— 不静默覆盖
        StateRepoStatus::Conflict(ConflictReason::NotGitDir)
    } else {
        // root 不存在或为空
        match configured {
            Some(_) => StateRepoStatus::NeedsInit,
            None => StateRepoStatus::Unconfigured,
        }
    }
}

/// git 子进程执行超时上限。dashboard-state 是小仓，正常 clone/pull 秒级；超过此值基本是卡住
/// （凭证弹窗遮挡 / 网络挂起），主动 kill 避免 App 持锁 hang 导致后续同步全被锁挡。
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// 带超时地跑一个已配置好的命令，拿 `Output`。超时则杀子进程（组）返回 `SyncError::Timeout`。
///
/// 用两个线程分别 drain stdout/stderr 管道（防管道写满 → 子进程阻塞、而主线程在等它退出的死锁），
/// 主线程 `try_wait` 轮询 + 到点 kill。**不动 env**——子进程继承父进程全部环境变量（含 http_proxy
/// 等代理配置），与原 `output()` 行为一致，不改出网链路。
///
/// stdin 置空：无 TTY 环境下 git 遇凭证 / passphrase 提示不再读父 stdin 而 hang。
/// Unix 上把子进程放进独立进程组，超时用 killpg 杀整组——见 kill_process_tree。
fn run_command_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Result<std::process::Output, SyncError> {
    use std::io::Read;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Unix：子进程自成进程组组长（pgid == 子进程 pid），超时可 killpg 杀整组，连带 git fork 的
    // ssh / credential helper 孙进程一起收掉；否则孙进程持 stdout/stderr 写端，drain 线程
    // read_to_end 收不到 EOF，超时分支的 join 会 hang，GIT_TIMEOUT 上限失效（审计 P1-3）。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            kill_process_tree(&mut child);
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            return Err(SyncError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// 超时终止子进程。Unix：子进程经 `process_group(0)` 自成组长，`killpg(pgid, SIGKILL)` 杀整组
/// （含 git fork 的 ssh / credential helper 孙进程），保证 drain 线程收到管道 EOF、join 不 hang。
/// 其他平台（Windows）暂只杀直接子进程——进程树终止（Job Object）留接力待办。
#[cfg(unix)]
fn kill_process_tree(child: &mut std::process::Child) {
    // process_group(0) 后 pgid == 子进程 pid；向该进程组发 SIGKILL。
    // killpg 失败（返回非零，如组已消失 / 权限异常）时退回杀直接子进程，
    // 保证至少收掉父 git，不因组信号失败而彻底放任子进程。
    let pgid = child.id() as libc::pid_t;
    let killed = unsafe { libc::killpg(pgid, libc::SIGKILL) };
    if killed != 0 {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn kill_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

/// 用 shell out `git` 做仓操作 —— v4.1 定死走命令，不引 `git2`；带 `GIT_TIMEOUT` 超时兜底。
fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String, SyncError> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let out = run_command_with_timeout(cmd, GIT_TIMEOUT)?;
    if !out.status.success() {
        return Err(SyncError::GitFailed {
            args: args.iter().map(|s| s.to_string()).collect(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[derive(Debug)]
pub enum SyncError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Schema(SchemaError),
    GitFailed { args: Vec<String>, stderr: String },
    /// P0-3：读到的 snapshot.json 内 machine_id 与本机不符（文件错位 / 污染）。
    MachineIdMismatch { expected: String, found: String },
    /// P0-5：取 repo-global 同步锁超时（另一个采集 / 同步进程正持锁）。
    LockBusy,
    /// Q2-D1：git 子进程执行超时被主动终止（凭证弹窗遮挡 / 网络挂起）。
    /// 产生此变体的执行层 timeout 实现单列（涉及改子进程执行方式），本变体先入契约供分类映射。
    Timeout,
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::Schema(e) => write!(f, "schema: {e}"),
            Self::GitFailed { args, stderr } => {
                write!(f, "git {} failed: {}", args.join(" "), stderr)
            }
            Self::MachineIdMismatch { expected, found } => {
                write!(
                    f,
                    "snapshot machine_id 不一致：期望 {expected}，实际 {found}（文件错位/污染）"
                )
            }
            Self::LockBusy => {
                write!(f, "另一个同步进程正在写 dashboard-state，请稍后重试")
            }
            Self::Timeout => {
                write!(f, "git 操作超时被终止（可能凭证弹窗遮挡或网络挂起）")
            }
        }
    }
}

impl std::error::Error for SyncError {}

impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for SyncError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<SchemaError> for SyncError {
    fn from(e: SchemaError) -> Self {
        Self::Schema(e)
    }
}

/// 配置即拉（clone/pull）失败的面向用户分类（Q2-D1 认证/结构错误契约）。
///
/// 每个变体对应一个稳定 `tag()`（前端靠 tag 判分支，**不解析中文文案**，与 CommandError 的
/// `[NEED_DUAL_FORM_CONFIRM]` 同口径）+ 一句中文 `user_hint()`。git 层错误经 `classify_sync_error`
/// 从 stderr / 退出码映射；结构非法（clone 到的不是 dashboard-state 仓）由调用方在
/// `is_dashboard_state_repo` 校验失败时直接归类，不走 stderr 映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPullErrorKind {
    /// git 可执行文件不存在（未安装 / 不在 PATH）。
    GitNotInstalled,
    /// 网络 / DNS / 代理不通（解析主机失败、连接失败、网络不可达）。
    Network,
    /// SSH 认证失败（publickey 被拒 / host key 校验失败）。
    SshAuth,
    /// HTTPS 凭证失败（用户名密码错 / 无凭证且禁交互提示）。
    HttpsAuth,
    /// 仓不存在或无访问权限。
    RepoNotFoundOrDenied,
    /// clone/pull 成功但拉到的不是 dashboard-state 仓（缺 registry/ 结构）。
    InvalidStructure,
    /// 目标目录已存在且非空（不覆盖，引导手动清理）。
    TargetNotEmpty,
    /// 同步锁被占用（另一进程正持锁）。
    LockBusy,
    /// git 子进程执行超时被终止。
    Timeout,
    /// 未归入以上任一类的兜底。
    Unknown,
}

impl ConfigPullErrorKind {
    /// 稳定标识：前端按此判分支，改动即前端 breaking（受 tests/pull_error.rs::tags_are_stable 锁）。
    pub fn tag(&self) -> &'static str {
        match self {
            Self::GitNotInstalled => "GIT_MISSING",
            Self::Network => "NETWORK",
            Self::SshAuth => "SSH_AUTH",
            Self::HttpsAuth => "HTTPS_AUTH",
            Self::RepoNotFoundOrDenied => "REPO_ACCESS",
            Self::InvalidStructure => "INVALID_STRUCTURE",
            Self::TargetNotEmpty => "TARGET_NOT_EMPTY",
            Self::LockBusy => "LOCK_BUSY",
            Self::Timeout => "TIMEOUT",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// 面向用户的中文提示（一句话，不含技术栈细节）。
    pub fn user_hint(&self) -> &'static str {
        match self {
            Self::GitNotInstalled => "未检测到 git，请先安装 git 并确认在 PATH 中",
            Self::Network => "网络不通（无法解析主机 / 连接失败 / 代理异常），请检查网络或代理",
            Self::SshAuth => "SSH 认证失败，请检查 SSH key 是否已配置并加入远端",
            Self::HttpsAuth => "HTTPS 凭证失败，请检查用户名密码或访问令牌",
            Self::RepoNotFoundOrDenied => "仓不存在或无访问权限，请确认存储仓地址与账号权限",
            Self::InvalidStructure => {
                "该地址不是 dashboard-state 存储仓（缺 registry/ 结构），请确认是否填错地址"
            }
            Self::TargetNotEmpty => "目标数据目录已存在且非空，请手动清理后重试",
            Self::LockBusy => "另一个同步进程正在占用，请稍后重试",
            Self::Timeout => "git 操作超时（可能凭证弹窗遮挡或网络挂起），请重试",
            Self::Unknown => "同步失败，请查看详细日志",
        }
    }
}

/// 把底层 `SyncError` 映射为面向用户的 `ConfigPullErrorKind`。
///
/// `Io(NotFound)` = git 可执行文件缺失；`LockBusy` / `Timeout` 直接对应；`GitFailed` 走 stderr 关键词
/// 匹配。**不含** `InvalidStructure`——那是 clone 成功后 `is_dashboard_state_repo` 校验失败时由调用方归类。
pub fn classify_sync_error(err: &SyncError) -> ConfigPullErrorKind {
    match err {
        SyncError::LockBusy => ConfigPullErrorKind::LockBusy,
        SyncError::Timeout => ConfigPullErrorKind::Timeout,
        SyncError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ConfigPullErrorKind::GitNotInstalled
        }
        SyncError::GitFailed { stderr, .. } => classify_git_stderr(stderr),
        _ => ConfigPullErrorKind::Unknown,
    }
}

/// git stderr 关键词分类（内部）。**顺序敏感**：先判最具体的模式，再把明确网络/代理模式提到
/// 宽泛仓访问之前——避免 "proxy command not found" / "object not found" 被误判为仓不存在。
/// 顺序：目标非空 → SSH → HTTPS → 网络/代理 → 仓不存在/无权限 → 兜底。
fn classify_git_stderr(stderr: &str) -> ConfigPullErrorKind {
    let s = stderr.to_lowercase();
    let has = |needle: &str| s.contains(needle);
    if has("already exists and is not an empty directory") {
        ConfigPullErrorKind::TargetNotEmpty
    } else if has("permission denied (publickey)") || has("host key verification failed") {
        ConfigPullErrorKind::SshAuth
    } else if has("authentication failed")
        || has("could not read username")
        || has("could not read password")
        || has("invalid username or password")
    {
        ConfigPullErrorKind::HttpsAuth
    } else if has("could not resolve host")
        || has("could not resolve hostname")
        || has("network is unreachable")
        || has("failed to connect")
        || has("connection refused")
        || has("temporary failure in name resolution")
        || has("failed to connect to proxy")
        || has("proxy authentication")
        || has("early eof")
        || has("remote end hung up")
        || has("rpc failed")
    {
        ConfigPullErrorKind::Network
    } else if has("repository not found")
        || has("does not exist")
        || (has("permission to") && has("denied"))
        || has("access denied")
        || has("403 forbidden")
    {
        ConfigPullErrorKind::RepoNotFoundOrDenied
    } else {
        ConfigPullErrorKind::Unknown
    }
}

/// clone/pull 后校验：root 是否是合法 dashboard-state 仓。判据 = 仓根含 `registry/projects/`
/// （spec 核心结构标识；决策 3 增强：不只判 `registry/` 目录存在，还要 `registry/projects`，
/// 挡「普通仓恰好有 registry/ 目录但无 projects」被误判为合法）。`machines/<id>/` 按 spec 本机
/// 首次同步才创建，不硬校验（避免误判全新空仓）。
pub fn is_dashboard_state_repo(root: &Path) -> bool {
    root.join("registry").join("projects").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Q2-D1 timeout 执行层 =====
    // #[cfg(unix)]：用 sleep/sh/yes 驱动；Windows 侧走同一份 std API 代码路径（try_wait/kill 跨平台）。

    #[cfg(unix)]
    #[test]
    fn command_timeout_kills_and_returns_timeout() {
        // 会 hang 10s 的命令，200ms 超时 → 主动 kill 返回 Timeout，不等它自然结束
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let start = Instant::now();
        let r = run_command_with_timeout(cmd, Duration::from_millis(200));
        assert!(matches!(r, Err(SyncError::Timeout)), "hang 命令应超时返回 Timeout");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "应被 kill 而非等命令自然结束（10s）"
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_completes_returns_output_and_code() {
        // 正常完成 → 拿到退出码 + stdout + stderr
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf out; printf err 1>&2; exit 3"]);
        let out = run_command_with_timeout(cmd, Duration::from_secs(5)).unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "err");
    }

    #[cfg(unix)]
    #[test]
    fn command_large_output_no_deadlock() {
        // 200KB stdout 远超管道缓冲(~64KB)；必须线程 drain 管道才不死锁
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes | head -c 200000"]);
        let out = run_command_with_timeout(cmd, Duration::from_secs(10)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 200000);
    }

    #[cfg(unix)]
    #[test]
    fn command_inherits_parent_env() {
        // 子进程继承父进程环境变量（不 env_clear）→ 代理配置照常传给 git
        std::env::set_var("PD_TEST_INHERIT_VAR", "inherited_value");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf %s \"$PD_TEST_INHERIT_VAR\""]);
        let out = run_command_with_timeout(cmd, Duration::from_secs(5)).unwrap();
        std::env::remove_var("PD_TEST_INHERIT_VAR");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "inherited_value");
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_kills_grandchild_holding_pipe() {
        // P1-5：`sh -c 'sleep 30 | cat'` 里 cat 是孙进程，继承子进程 stdout 写端并阻塞在管道上。
        // 只杀直接子进程 sh 时孙进程仍持写端 → drain 线程 read_to_end 收不到 EOF → join hang，
        // GIT_TIMEOUT 上限失效。process_group + killpg 杀整组后孙进程一起死 → EOF → 上限内返回。
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 | cat"]);
        let start = Instant::now();
        let r = run_command_with_timeout(cmd, Duration::from_millis(300));
        assert!(matches!(r, Err(SyncError::Timeout)), "hang 命令应超时返回 Timeout");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "killpg 杀整组后应在上限内返回，而非被孙进程持管道拖住（实测 {:?}）",
            start.elapsed()
        );
    }

    // 具名绝对 root（生产形态）：锁文件是 root 的兄弟位 `<root>.sync.lock`。
    #[test]
    fn lock_path_named_root_is_sibling() {
        let root = std::env::temp_dir().join("dashboard-state-clone");
        let repo = DashboardStateRepo::new(&root, "m");
        let expected = std::env::temp_dir().join("dashboard-state-clone.sync.lock");
        assert_eq!(repo.lock_path(), expected);
    }

    // 相对 root（异常输入）：先被 std::path::absolute 绝对化，锁路径不再相对、跨 CWD 稳定。
    #[test]
    fn lock_path_relative_root_is_absolutized() {
        let repo = DashboardStateRepo::new("dashboard-state", "m");
        assert!(
            repo.lock_path().is_absolute(),
            "相对 root 的锁路径必须被绝对化，实得 {:?}",
            repo.lock_path()
        );
    }

    // P2-A 回归：文件系统根 `/` 这种 file_name/parent 均为 None 的异常 root，锁路径必须落绝对
    // 文件（`/dashboard-state.sync.lock`），不得退化成相对路径（否则不同 CWD 锁分裂、互斥失效）。
    #[cfg(unix)]
    #[test]
    fn lock_path_fs_root_stays_absolute_not_relative() {
        let repo = DashboardStateRepo::new("/", "m");
        let p = repo.lock_path();
        assert!(p.is_absolute(), "fs 根的锁路径必须绝对，实得 {p:?}");
        assert_eq!(p, PathBuf::from("/dashboard-state.sync.lock"));
    }

    // Q2-B：resolve_status 纯函数封闭四态 + Conflict 两 reason + 存量兼容判定。
    #[test]
    fn resolve_status_states() {
        let obs = |exists: bool, empty: bool, git: bool, registry: bool, origin: Option<&str>| {
            Observed {
                root_exists: exists,
                root_empty: empty,
                has_git: git,
                has_registry: registry,
                origin: origin.map(|s| s.to_string()),
            }
        };
        let remote = "git@host:team/ds.git";

        // 未配 remote 且无 .git（root 不存在）→ Unconfigured
        assert_eq!(
            resolve_status(&obs(false, true, false, false, None), None, false),
            StateRepoStatus::Unconfigured
        );

        // 配 remote 且 root 空 / 不存在 → NeedsInit
        assert_eq!(
            resolve_status(&obs(false, true, false, false, None), Some(remote), false),
            StateRepoStatus::NeedsInit
        );
        assert_eq!(
            resolve_status(&obs(true, true, false, false, None), Some(remote), false),
            StateRepoStatus::NeedsInit
        );

        // 有 .git + 合法结构 + origin==配置值 → Ready
        assert_eq!(
            resolve_status(&obs(true, false, true, true, Some(remote)), Some(remote), false),
            StateRepoStatus::Ready
        );

        // 有 .git + 合法结构、未配 remote、从没清空、**有实际 origin**（存量老机器）→ Ready（续同步）
        assert_eq!(
            resolve_status(
                &obs(true, false, true, true, Some("git@host:team/legacy.git")),
                None,
                false
            ),
            StateRepoStatus::Ready
        );

        // 三方评审收窄（2026-07-21）：有 .git + 合法结构、未配 remote、从没清空、但**无实际 origin**
        // → Unconfigured（而非旧逻辑的无条件 Ready）。无处可 pull/push 的仓不应假装可同步，否则
        // manual_sync 前置 pull 必失败中止。
        assert_eq!(
            resolve_status(&obs(true, false, true, true, None), None, false),
            StateRepoStatus::Unconfigured
        );

        // 有 .git + 合法结构、未配 remote、**显式清空**（cleared=true）→ Unconfigured
        assert_eq!(
            resolve_status(
                &obs(true, false, true, true, Some("git@host:team/legacy.git")),
                None,
                true
            ),
            StateRepoStatus::Unconfigured
        );

        // 有 .git + 合法结构、配了 remote + 已清空标记：Some(remote) 分支短路不看 cleared → Ready
        assert_eq!(
            resolve_status(&obs(true, false, true, true, Some(remote)), Some(remote), true),
            StateRepoStatus::Ready
        );

        // 决策 3：有 .git 但**无 registry/projects**（普通仓 / 填错地址）→ Conflict(InvalidStructure)，
        // 在 origin 匹配前拦（配 remote 与否、origin 是否匹配都不影响，结构非法优先）
        assert_eq!(
            resolve_status(&obs(true, false, true, false, Some(remote)), Some(remote), false),
            StateRepoStatus::Conflict(ConflictReason::InvalidStructure)
        );
        assert_eq!(
            resolve_status(&obs(true, false, true, false, None), None, false),
            StateRepoStatus::Conflict(ConflictReason::InvalidStructure)
        );

        // 有 .git + 合法结构 但 origin ≠ 配置值 → Conflict{RemoteMismatch}
        assert_eq!(
            resolve_status(
                &obs(true, false, true, true, Some("git@host:team/other.git")),
                Some(remote),
                false
            ),
            StateRepoStatus::Conflict(ConflictReason::RemoteMismatch {
                configured: remote.to_string(),
                actual: Some("git@host:team/other.git".to_string()),
            })
        );

        // root 非空但无 .git → Conflict{NotGitDir}（配 remote 与否都是待处理数据，非 Unconfigured）
        assert_eq!(
            resolve_status(&obs(true, false, false, false, None), Some(remote), false),
            StateRepoStatus::Conflict(ConflictReason::NotGitDir)
        );
        assert_eq!(
            resolve_status(&obs(true, false, false, false, None), None, false),
            StateRepoStatus::Conflict(ConflictReason::NotGitDir)
        );

        // origin / 配置值两边带空白 → trim 后仍匹配（有 .git + 合法结构）
        assert_eq!(
            resolve_status(
                &obs(true, false, true, true, Some("  git@host:team/ds.git  ")),
                Some(remote),
                false
            ),
            StateRepoStatus::Ready
        );
    }
}
