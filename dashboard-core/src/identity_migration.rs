//! 旧身份认领 —— 把「按主机名当身份」的历史数据迁到硬件身份下。
//!
//! 背景：machine_id 原本跟主机名走，macOS 局域网撞名会自动加后缀（`...Pro-3` → `...Pro-4`），
//! 于是同一台机在 `machines/` 下裂成两个身份。旧快照可能保留不同的历史数据：
//! 旧主机名对应的 snapshot 可能仍有历史项目，改名后的新身份可能只有空快照。
//!
//! **本模块的两条硬规矩**：
//! 1. **干跑与真改分离** —— [`plan`] 只算不写，[`apply`] 才落盘。用户先看清「哪几行改成什么」。
//! 2. **只加不删** —— 旧目录原样留着当存档，绝不 remove。合并只是把内容归到新身份名下。

use crate::backup::snapshot_backup;
use crate::config::{LocalConfig, Registry};
use crate::sync::{DashboardStateRepo, Snapshot, SnapshotProject};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 认领理由 —— 每条认领都要能向用户解释「凭什么说这个旧身份是本机的」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimReason {
    /// 本机 local.yaml 里冻结的旧身份 —— 最硬的证据，这份文件只有本机自己写。
    FrozenInLocalConfig,
    /// 当次真实主机名。
    CurrentHostname,
    /// local.yaml 的主机名漂移轨迹里记过。
    HostnameHistory,
}

impl ClaimReason {
    pub fn explain(&self) -> &'static str {
        match self {
            Self::FrozenInLocalConfig => "本机 local.yaml 记录的旧身份（该文件只有本机写）",
            Self::CurrentHostname => "与当次真实主机名一致",
            Self::HostnameHistory => "在本机主机名漂移轨迹中出现过",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LegacyClaim {
    pub machine_id: String,
    pub reason: ClaimReason,
    /// 该旧身份在 `machines/` 下是否真有目录（没有也照样认领，只是无数据可并）。
    pub has_snapshot_dir: bool,
}

/// 未认领的身份 + 原因。列出来是为了让用户看见「我没动这些」，而不是悄悄跳过。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedIdentity {
    pub machine_id: String,
    pub reason: String,
}

/// 一条注册表键改写：`registry/projects/<uuid>.yaml` 里 `machines` 下的键名变更。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistryEdit {
    pub project_id: Uuid,
    pub project_name: String,
    pub from_key: String,
    pub to_key: String,
    /// 路径值原样保留，不参与改写 —— 列出来让用户确认「路径没动」。
    pub path: String,
}

/// 快照合并计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotMerge {
    /// 参与合并的旧目录名（按认领顺序）。
    pub sources: Vec<String>,
    pub target: String,
    /// 合并后项目条数。
    pub project_count: usize,
    /// 合并后 events.ndjson 行数。
    pub event_lines: usize,
    /// 各来源贡献的项目条数，用来暴露「哪份是真数据、哪份是空壳」。
    pub source_project_counts: Vec<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityMigrationPlan {
    /// 迁移目标身份（硬件式）。
    pub target_machine_id: String,
    pub current_hostname: String,
    /// local.yaml 里现存的身份（迁移前）。
    pub current_frozen_id: String,
    pub claims: Vec<LegacyClaim>,
    pub skipped: Vec<SkippedIdentity>,
    pub registry_edits: Vec<RegistryEdit>,
    pub snapshot_merge: Option<SnapshotMerge>,
    /// 目标身份已存在数据 → 冲突提示（合并会保留已有值，不覆盖）。
    pub conflicts: Vec<String>,
}

impl IdentityMigrationPlan {
    /// 是否真有事可做。全空时前端应显示「无需迁移」而不是给个空按钮。
    pub fn is_noop(&self) -> bool {
        self.registry_edits.is_empty()
            && self.snapshot_merge.is_none()
            && self.current_frozen_id == self.target_machine_id
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentityMigrationReport {
    pub plan: IdentityMigrationPlan,
    /// 迁移前的整仓备份位置 —— 出问题照这个路径回滚。
    pub backup_path: PathBuf,
    pub registry_files_written: usize,
    pub snapshot_written: bool,
    /// 保留下来的旧目录（**没删**），供用户日后自行清理。
    pub legacy_dirs_kept: Vec<String>,
    /// 迁移改动是否已收进共享仓的一个本地 commit（**不 push**，推送仍由显式同步负责）。
    pub committed: bool,
    /// 未提交时的原因（非 git 仓 / 无受管改动 / git 报错），供 UI 如实提示 —— 未提交
    /// 意味着工作树是脏的，后续同步会被 pull-first 拦住，用户必须知道。
    pub commit_note: Option<String>,
}

#[derive(Debug)]
pub enum MigrationError {
    Io(std::io::Error),
    Config(crate::config::ConfigError),
    Json(serde_json::Error),
    /// 目标身份不是硬件式（形如 `mbp-00000001`）—— 拒绝迁到另一个会漂的身份上。
    TargetNotStable(String),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Config(error) => write!(formatter, "config: {error}"),
            Self::Json(error) => write!(formatter, "json: {error}"),
            Self::TargetNotStable(id) => write!(
                formatter,
                "目标身份 {id} 不是硬件式标识，拒绝迁移（否则等于换一个照样会漂的身份）"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<std::io::Error> for MigrationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<crate::config::ConfigError> for MigrationError {
    fn from(error: crate::config::ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<serde_json::Error> for MigrationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// 干跑：算出迁移计划，**一个字节都不写**。
pub fn plan(
    dashboard_state_root: &Path,
    local_path: &Path,
    target_machine_id: &str,
    current_hostname: &str,
) -> Result<IdentityMigrationPlan, MigrationError> {
    if crate::commands::is_legacy_hostname_id(target_machine_id) {
        return Err(MigrationError::TargetNotStable(
            target_machine_id.to_string(),
        ));
    }
    let registry = Registry::load_at_read_only(dashboard_state_root, local_path)?;
    let local = registry.local.clone().unwrap_or_else(|| LocalConfig::new(""));

    let claimed = claimed_identities(&local, current_hostname, target_machine_id);
    let machines_dir = dashboard_state_root.join("machines");
    let existing_dirs = list_machine_dirs(&machines_dir);

    let claims: Vec<LegacyClaim> = claimed
        .iter()
        .map(|(machine_id, reason)| LegacyClaim {
            machine_id: machine_id.clone(),
            reason: reason.clone(),
            has_snapshot_dir: existing_dirs.contains(machine_id),
        })
        .collect();

    // 没被认领的目录一律不动，并把「为什么不动」写清楚。
    let claimed_names: BTreeSet<&String> = claimed.iter().map(|(id, _)| id).collect();
    let skipped: Vec<SkippedIdentity> = existing_dirs
        .iter()
        .filter(|name| !claimed_names.contains(name) && *name != target_machine_id)
        .map(|name| SkippedIdentity {
            machine_id: name.clone(),
            reason: "无证据表明属于本机（不在 local.yaml 身份、当次主机名或漂移轨迹中）——原样保留"
                .to_string(),
        })
        .collect();

    // 注册表键改写：只改被认领的键，路径值原样搬过去。
    let mut registry_edits = Vec::new();
    let mut conflicts = Vec::new();
    for (project_id, project) in &registry.projects {
        for legacy in claimed_names.iter() {
            if legacy.as_str() == target_machine_id {
                continue;
            }
            if let Some(path) = project.machines.get(legacy.as_str()) {
                if project.machines.contains_key(target_machine_id) {
                    conflicts.push(format!(
                        "项目「{}」下 {} 与 {} 同时存在，保留已有的 {} 不覆盖",
                        project.name, legacy, target_machine_id, target_machine_id
                    ));
                    continue;
                }
                registry_edits.push(RegistryEdit {
                    project_id: *project_id,
                    project_name: project.name.clone(),
                    from_key: (*legacy).clone(),
                    to_key: target_machine_id.to_string(),
                    path: path.clone(),
                });
            }
        }
    }

    let snapshot_merge = plan_snapshot_merge(
        &machines_dir,
        &claims,
        target_machine_id,
        &existing_dirs,
    )?;

    Ok(IdentityMigrationPlan {
        target_machine_id: target_machine_id.to_string(),
        current_hostname: current_hostname.to_string(),
        current_frozen_id: local.machine_id.clone(),
        claims,
        skipped,
        registry_edits,
        snapshot_merge,
        conflicts,
    })
}

/// 认领判定。保守策略：只认「本机自己写下的证据」，其余一律不认。
fn claimed_identities(
    local: &LocalConfig,
    current_hostname: &str,
    target_machine_id: &str,
) -> Vec<(String, ClaimReason)> {
    let mut claimed: Vec<(String, ClaimReason)> = Vec::new();
    let mut push = |id: &str, reason: ClaimReason| {
        let trimmed = id.trim();
        if trimmed.is_empty() || trimmed == target_machine_id {
            return;
        }
        // 已是硬件式身份的不认领 —— 那不是「旧身份」，可能是别的机器。
        if !crate::commands::is_legacy_hostname_id(trimmed) {
            return;
        }
        if claimed.iter().any(|(existing, _)| existing == trimmed) {
            return;
        }
        claimed.push((trimmed.to_string(), reason));
    };
    push(&local.machine_id, ClaimReason::FrozenInLocalConfig);
    push(current_hostname, ClaimReason::CurrentHostname);
    let history = local.hostname_history.clone();
    for name in &history {
        push(name, ClaimReason::HostnameHistory);
    }
    claimed
}

fn list_machine_dirs(machines_dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(machines_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

fn plan_snapshot_merge(
    machines_dir: &Path,
    claims: &[LegacyClaim],
    target_machine_id: &str,
    existing_dirs: &BTreeSet<String>,
) -> Result<Option<SnapshotMerge>, MigrationError> {
    let mut sources: Vec<String> = claims
        .iter()
        .filter(|claim| claim.has_snapshot_dir)
        .map(|claim| claim.machine_id.clone())
        .collect();
    if sources.is_empty() {
        return Ok(None);
    }
    // 目标目录若已有数据，也纳入合并（它可能就是改名后新开的那个空壳）。
    if existing_dirs.contains(target_machine_id) {
        sources.push(target_machine_id.to_string());
    }
    let merged = merge_sources(machines_dir, &sources, target_machine_id)?;
    // 「旧目录只加不删」是本模块的硬规矩，所以认领到的旧目录永远在、sources 永远非空。
    // 若不判「这次合并是否真会改变目标」，迁移完成后再干跑照样算出一份计划，
    // is_noop() 永远为 false —— 用户每次进设置页都看到一个其实什么都不改的「执行迁移」按钮。
    if !merge_changes_target(machines_dir, target_machine_id, &merged)? {
        return Ok(None);
    }
    Ok(Some(SnapshotMerge {
        sources: sources.clone(),
        target: target_machine_id.to_string(),
        project_count: merged.snapshot.projects.len(),
        event_lines: merged.event_lines.len(),
        source_project_counts: merged.source_project_counts,
    }))
}

/// 合并结果是否真会改变目标身份的现有数据。
///
/// 只比 `projects` 与 events —— `updated_at` / `hostname` 由正常同步刷新，它们不同不构成迁移理由。
fn merge_changes_target(
    machines_dir: &Path,
    target_machine_id: &str,
    merged: &MergeResult,
) -> Result<bool, MigrationError> {
    let target_dir = machines_dir.join(target_machine_id);
    let Ok(existing_text) = std::fs::read_to_string(target_dir.join("snapshot.json")) else {
        return Ok(true); // 目标还没有数据，合并必然是新增
    };
    let existing: Snapshot = serde_json::from_str(&existing_text)?;
    if project_index(&existing.projects)? != project_index(&merged.snapshot.projects)? {
        return Ok(true);
    }
    let existing_events: Vec<String> = std::fs::read_to_string(target_dir.join("events.ndjson"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    Ok(existing_events != merged.event_lines)
}

/// 按 `merge_sources` 用的同一套键索引项目并取其序列化值 —— 与文件里的排列顺序无关，只比内容。
fn project_index(
    projects: &[SnapshotProject],
) -> Result<BTreeMap<String, String>, MigrationError> {
    let mut index = BTreeMap::new();
    for project in projects {
        let key = project
            .project_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| format!("name:{}", project.name));
        index.insert(key, serde_json::to_string(project)?);
    }
    Ok(index)
}

struct MergeResult {
    snapshot: Snapshot,
    event_lines: Vec<String>,
    source_project_counts: Vec<(String, usize)>,
}

/// 合并多份快照。
///
/// **绝不按整份 snapshot 的 `updated_at` 取胜者** —— 实测改名后新开的空壳快照
/// 时间戳更新但 `projects` 为空，那样合并会把真实历史整份抹掉。
/// 正确做法：按项目条目求并集，同一项目取 `collected_at` 更新的那条。
fn merge_sources(
    machines_dir: &Path,
    sources: &[String],
    target_machine_id: &str,
) -> Result<MergeResult, MigrationError> {
    let mut by_key: BTreeMap<String, SnapshotProject> = BTreeMap::new();
    let mut latest_updated: Option<DateTime<Utc>> = None;
    let mut latest_hostname = String::new();
    let mut schema_version = 1;
    let mut event_lines: Vec<String> = Vec::new();
    let mut seen_events: BTreeSet<String> = BTreeSet::new();
    let mut source_project_counts = Vec::new();

    for source in sources {
        let dir = machines_dir.join(source);
        let snapshot_path = dir.join("snapshot.json");
        let mut contributed = 0usize;
        if let Ok(text) = std::fs::read_to_string(&snapshot_path) {
            let snapshot: Snapshot = serde_json::from_str(&text)?;
            schema_version = schema_version.max(snapshot.schema_version);
            // 整份 updated_at 只用来定合并结果的时间戳，**不用来挑项目胜者**（见 merge_sources 文档）。
            let is_latest = latest_updated.is_none_or(|current| snapshot.updated_at > current);
            if is_latest {
                latest_updated = Some(snapshot.updated_at);
                // 主机名取最新那份 —— 它才是这台机当前叫什么。
                if !snapshot.hostname.is_empty() {
                    latest_hostname = snapshot.hostname.clone();
                }
            }
            contributed = snapshot.projects.len();
            for project in snapshot.projects {
                // project_id 缺失的老条目退化用 name 做键，避免同名项目互相顶掉。
                let key = project
                    .project_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| format!("name:{}", project.name));
                match by_key.get(&key) {
                    Some(existing) if is_newer(existing, &project) => {}
                    _ => {
                        by_key.insert(key, project);
                    }
                }
            }
        }
        source_project_counts.push((source.clone(), contributed));

        // events.ndjson 是 append-only 流：按来源顺序拼接、逐行去重，保持原有先后。
        if let Ok(text) = std::fs::read_to_string(dir.join("events.ndjson")) {
            for line in text.lines() {
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    continue;
                }
                if seen_events.insert(trimmed.to_string()) {
                    event_lines.push(trimmed.to_string());
                }
            }
        }
    }

    Ok(MergeResult {
        snapshot: Snapshot {
            schema_version,
            machine_id: target_machine_id.to_string(),
            hostname: latest_hostname,
            updated_at: latest_updated.unwrap_or_else(Utc::now),
            projects: by_key.into_values().collect(),
        },
        event_lines,
        source_project_counts,
    })
}

/// 同一项目的两条记录，现存那条是否已经更新（更新则保留现存、丢弃候选）。
/// 用项目级 `collected_at` 而非整份快照的 `updated_at` —— 后者会被空壳快照的新时间戳骗到。
fn is_newer(existing: &SnapshotProject, candidate: &SnapshotProject) -> bool {
    existing.collected_at >= candidate.collected_at
}

/// 真改：先整仓备份，再写注册表 + 合并快照。**旧目录一个都不删。**
pub fn apply(
    dashboard_state_root: &Path,
    local_path: &Path,
    backups_root: &Path,
    target_machine_id: &str,
    current_hostname: &str,
) -> Result<IdentityMigrationReport, MigrationError> {
    let plan = plan(
        dashboard_state_root,
        local_path,
        target_machine_id,
        current_hostname,
    )?;
    // 备份先于任何写入 —— 出问题照 backup_path 整目录拷回即可。
    let backup_path = snapshot_backup(dashboard_state_root, backups_root, Utc::now())?;

    let mut registry = Registry::load_at(dashboard_state_root, local_path)?;
    for edit in &plan.registry_edits {
        if let Some(project) = registry.projects.get_mut(&edit.project_id) {
            if let Some(path) = project.machines.remove(&edit.from_key) {
                project.machines.insert(edit.to_key.clone(), path);
            }
        }
    }
    // 身份冻结进 local.yaml，并记下当次主机名（漂移留痕）。
    let local = registry
        .local
        .get_or_insert_with(|| LocalConfig::new(target_machine_id));
    local.machine_id = target_machine_id.to_string();
    local.record_hostname(current_hostname);
    registry.save_at(dashboard_state_root, local_path)?;

    let machines_dir = dashboard_state_root.join("machines");
    let mut snapshot_written = false;
    if let Some(merge) = &plan.snapshot_merge {
        let merged = merge_sources(&machines_dir, &merge.sources, target_machine_id)?;
        let target_dir = machines_dir.join(target_machine_id);
        std::fs::create_dir_all(&target_dir)?;
        let json = serde_json::to_string_pretty(&merged.snapshot)?;
        std::fs::write(target_dir.join("snapshot.json"), json)?;
        if !merged.event_lines.is_empty() {
            let mut body = merged.event_lines.join("\n");
            body.push('\n');
            std::fs::write(target_dir.join("events.ndjson"), body)?;
        }
        snapshot_written = true;
    }

    let legacy_dirs_kept = plan
        .claims
        .iter()
        .filter(|claim| claim.has_snapshot_dir)
        .map(|claim| claim.machine_id.clone())
        .collect();

    let (committed, commit_note) = commit_migration(dashboard_state_root, target_machine_id);

    Ok(IdentityMigrationReport {
        registry_files_written: plan.registry_edits.len(),
        snapshot_written,
        legacy_dirs_kept,
        committed,
        commit_note,
        plan,
        backup_path,
    })
}

/// 把迁移产生的改动收进共享仓的一个本地 commit（**不 push**）。
///
/// 为什么必须做：同步走 pull-first（`sync.rs` 的 `git_pull`），而 git 在工作树有未提交改动时
/// 会拒绝 pull —— 用户若配了 `pull.rebase=true`，走的 rebase 路径更是要求工作树完全干净。
/// 迁移直接往 clone 里写 `registry/` 与 `machines/`，留下脏工作树后，此后**所有**同步都会失败
/// （实测报 `cannot pull with rebase: You have unstaged changes`，UI 只显示 `[UNKNOWN] 同步失败`）。
///
/// 复用 `git_add_all` 的白名单（只收 `registry/` + `machines/`），数据目录若被配成含
/// `local.yaml` / 密钥的目录也不会被裹进提交。
///
/// 失败不向上抛：走到这里数据已经写完了，返回 Err 会让调用方以为迁移没成功、诱导重复执行。
/// 改为如实回报给 report，由 UI 提示用户手工处理。
fn commit_migration(root: &Path, target_machine_id: &str) -> (bool, Option<String>) {
    if !root.join(".git").exists() {
        return (false, Some("数据目录不是 git 仓库，未生成提交".to_string()));
    }
    let repo = DashboardStateRepo::new(root, target_machine_id);
    if let Err(error) = repo.git_add_all() {
        return (false, Some(format!("git add 失败：{error}")));
    }
    match repo.git_has_staged_changes() {
        Ok(true) => {}
        Ok(false) => return (false, Some("没有受管改动需要提交".to_string())),
        Err(error) => return (false, Some(format!("git 状态检查失败：{error}"))),
    }
    match repo.git_commit(&format!("身份迁移：旧主机名身份 → {target_machine_id}")) {
        Ok(()) => (true, None),
        Err(error) => (false, Some(format!("git commit 失败：{error}"))),
    }
}
