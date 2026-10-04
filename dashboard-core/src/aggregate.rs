// M3 §12 跨机聚合 —— 读 dashboard-state/machines/*/snapshot.json → 按 project_id + git_url 复合判等
// 合并成跨机视图 + 用提交图（HEAD/upstream/merge-base）判定"哪台最新"。
//
// 铁律：不用时间戳判"新旧"，只看 git DAG 关系。
//
// 判定输出：Synced / Ahead(a→b) / Ahead(b→a) / Diverged / DualDirty / UuidConflict。

use crate::config::Registry;
use crate::sync::{Snapshot, SnapshotProject};
use crate::tracker::{normalize_git_url, NormalizeError};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use uuid::Uuid;

/// 单个项目在某台机上的最新态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineProjectView {
    pub machine_id: String,
    pub machine_updated_at: DateTime<Utc>,
    pub project: SnapshotProject,
}

/// 跨机聚合后的项目视图 —— 一行代表一个 project_id (+ git_url) 组合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAggregate {
    pub project_id: Option<Uuid>,
    pub name: String,
    pub git_url: Option<String>,
    pub machines: BTreeMap<String, MachineProjectView>,
    pub verdict: Verdict,
    /// 是否有某台机陈旧（updated_at > stale_threshold）
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Verdict {
    /// 双机 HEAD 相同 + 都干净 —— 完全同步
    Synced,
    /// 双机 HEAD 相同但一方 dirty/untracked → 那方是"进行中"
    OneInProgress { machine_id: String },
    /// 一方是另一方的祖先，非祖先方最新
    Ahead {
        latest_machine_id: String,
        behind_machine_id: String,
    },
    /// merge-base 不等于任何一方 —— 分叉，不给结论
    Diverged,
    /// 双机同项目都 dirty > 0
    DualDirty,
    /// 同 UUID 但 git_url 归一化后不同（fork 场景）
    UuidConflict {
        differing_urls: Vec<(String, String)>,
    },
    /// 单机项目（暂未跨机聚合）
    SingleMachine { machine_id: String },
    /// 信息不足以判定 —— 附 `reason` 区分具体成因，前端据此给差异化文案与下一步引导。
    /// v4.3.1 根治：原 unit variant 统一挂"信息不足（HEAD 缺失 / 需 fetch）"对多数场景不准，见 InsufficientReason。
    Insufficient { reason: InsufficientReason },
}

/// `Verdict::Insufficient` 的细分成因 —— 前端按此出差异化文案 + 下一步引导。
///
/// 拆分动机（v4.3.1）：判定铁律「只认完整 sha 严格比对、绝不用时间戳猜新旧」下，多种成因都会
/// 落到 Insufficient，但用户需要的下一步动作完全不同（某台机重新同步 / 看两机提交记录 / 都不是 bug）。
/// 单一文案"信息不足（HEAD 缺失 / 需 fetch）"对多数场景不准 —— 尤其"需 fetch"从来不是本 App 的解法。
///
/// **不落盘**：Verdict 是 aggregate() 每次实时算出、仅经 IPC 传前端的产物，不写入 snapshot/registry，
/// 故本次从 unit variant 扩为 struct variant 无任何跨版本持久化兼容问题。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InsufficientReason {
    /// 至少一台机的 head_full 缺失（老 v4.2 快照 / 有效空仓 / rev-parse 失败）。
    /// 引导：去那台机重新同步，补上完整 HEAD。
    HeadMissing,
    /// 两台机 head_full 都在但严格不等（HEAD 不同）。
    /// 引导：打开详情看两机提交记录人肉判断谁领先（本 App 不做自动 merge-base）。
    HeadMismatch,
    /// 3+ 台机场景，当前实现只覆盖 2 机判定。
    /// 引导：暂不支持，等多机判定实装。
    MultiMachine,
    /// project_id 有但 git_url 归一化失败（registry 无此项目 / host 不在白名单），
    /// 跨机分组降级为只按 UUID 合并，可能错并不同 fork → 保守不给结论。
    UrlDegraded,
}

/// 陈旧阈值默认 24h，可由 local.yaml 配置覆盖（下游 UI 层做）。
pub const DEFAULT_STALE_THRESHOLD: Duration = Duration::hours(24);

/// 从 dashboard-state 根目录加载所有 machines/*/snapshot.json 后聚合。
///
/// - `registry` 用于拿 `git_url`（每 project_id 的规范化 URL）
/// - `now` 用于判陈旧
pub fn aggregate_from_dashboard_state(
    dashboard_state_root: &Path,
    registry: &Registry,
    now: DateTime<Utc>,
    stale_threshold: Duration,
) -> Result<Vec<ProjectAggregate>, AggregateError> {
    let mut snapshots: Vec<Snapshot> = Vec::new();
    let machines_dir = dashboard_state_root.join("machines");
    let entries = match std::fs::read_dir(&machines_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    for machine_entry in entries.flatten() {
        let snap_path = machine_entry.path().join("snapshot.json");
        if !snap_path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&snap_path)?;
        let s: Snapshot = serde_json::from_str(&text)
            .map_err(|e| AggregateError::MalformedSnapshot(snap_path.display().to_string(), e))?;
        snapshots.push(s);
    }
    Ok(aggregate(&snapshots, registry, now, stale_threshold))
}

/// 纯函数聚合（测试友好）：给定多台机 snapshot + registry → 每 project 一行视图。
pub fn aggregate(
    snapshots: &[Snapshot],
    registry: &Registry,
    now: DateTime<Utc>,
    stale_threshold: Duration,
) -> Vec<ProjectAggregate> {
    // 按 (project_id, git_url_normalized) 复合分组
    // project_id=null 的用 name 兜底（本机 UUID 未 commit 场景，不参与跨机合并）
    let mut buckets: BTreeMap<GroupKey, Vec<MachineProjectView>> = BTreeMap::new();
    for snap in snapshots {
        for proj in &snap.projects {
            let git_url = proj.project_id.and_then(|id| {
                registry
                    .get_shared(&id)
                    .and_then(|reg| normalize_git_url(&reg.git_url).ok())
            });
            let key = GroupKey {
                project_id: proj.project_id,
                git_url: git_url.clone(),
                name_fallback: if proj.project_id.is_none() {
                    Some(proj.name.clone())
                } else {
                    None
                },
            };
            buckets.entry(key).or_default().push(MachineProjectView {
                machine_id: snap.machine_id.clone(),
                machine_updated_at: snap.updated_at,
                project: proj.clone(),
            });
        }
    }

    // 检查 UUID 冲突：多个 bucket 共享同 project_id 但不同 git_url
    let mut uuid_urls: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
    for key in buckets.keys() {
        if let (Some(id), Some(url)) = (&key.project_id, &key.git_url) {
            uuid_urls.entry(*id).or_default().push(url.clone());
        }
    }

    let mut out: Vec<ProjectAggregate> = Vec::new();
    for (key, views) in buckets {
        let name = views
            .first()
            .map(|v| v.project.name.clone())
            .unwrap_or_default();
        let stale = views.iter().any(|v| now - v.machine_updated_at > stale_threshold);

        // 判定 verdict
        let verdict = if let Some(id) = key.project_id {
            let urls = uuid_urls.get(&id).cloned().unwrap_or_default();
            let mut distinct: Vec<String> = urls.into_iter().collect();
            distinct.sort();
            distinct.dedup();
            if distinct.len() > 1 {
                Verdict::UuidConflict {
                    differing_urls: distinct
                        .into_iter()
                        .map(|u| (id.to_string(), u))
                        .collect(),
                }
            } else if key.git_url.is_none() && views.len() > 1 {
                // P1-D（2026-07-06）：project_id 有但 git_url 归一化失败
                // （registry 不存在该 project，或 host 不在白名单）→ 跨机 bucket 分组降级
                // 到"只按 project_id 合并"，可能把同 UUID 但不同实际 URL 的两个 fork 错误合并。
                // 保守回退 Insufficient，避免给出可能错误的 latest 结论。
                Verdict::Insufficient {
                    reason: InsufficientReason::UrlDegraded,
                }
            } else {
                judge(&views)
            }
        } else {
            judge(&views)
        };

        let machines: BTreeMap<String, MachineProjectView> = views
            .into_iter()
            .map(|v| (v.machine_id.clone(), v))
            .collect();

        out.push(ProjectAggregate {
            project_id: key.project_id,
            name,
            git_url: key.git_url,
            machines,
            verdict,
            stale,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn judge(views: &[MachineProjectView]) -> Verdict {
    if views.len() == 1 {
        return Verdict::SingleMachine {
            machine_id: views[0].machine_id.clone(),
        };
    }
    if views.len() != 2 {
        // 当前只支持两台机器；三台及以上机器的聚合需另行实现
        // 降级为 Insufficient 表示当前实现未覆盖
        return Verdict::Insufficient {
            reason: InsufficientReason::MultiMachine,
        };
    }
    let a = &views[0];
    let b = &views[1];

    // v4.3 §1.5：HEAD 相等判定升级为 head_full（完整 40 位 sha）严格比对。
    //
    // 动机：短 sha（默认 7 位，rev-parse --short 会根据本地对象数量动态选长度）在两机
    // 对象库不同时长度可能差异，且极低概率下同前缀不同 commit 会碰撞。head_full 是零成本
    // 的严谨性提升 —— 前端展示时仍缩到 7 位，只有判定用完整 sha。
    //
    // 老 v4.2 snapshot 的 head_full = None → 走 Insufficient 分支（跟 HEAD 缺失同处理）。
    // 首次升级到 v4.3 后 R2-P0-A 稳定 hash 会触发一次 push，此后两机的 head_full 都有值。
    let a_head_full = a.project.head_full.as_deref();
    let b_head_full = b.project.head_full.as_deref();

    match (a_head_full, b_head_full) {
        (Some(ha), Some(hb)) if ha == hb => {
            let a_dirty = a.project.dirty > 0 || a.project.untracked > 0;
            let b_dirty = b.project.dirty > 0 || b.project.untracked > 0;
            match (a_dirty, b_dirty) {
                (true, true) => Verdict::DualDirty,
                (true, false) => Verdict::OneInProgress {
                    machine_id: a.machine_id.clone(),
                },
                (false, true) => Verdict::OneInProgress {
                    machine_id: b.machine_id.clone(),
                },
                (false, false) => Verdict::Synced,
            }
        }
        (Some(_), Some(_)) => {
            // head_full 都在但不等 → HEAD 不同。spec 铁律「用 git 提交图」判定，
            // 采集器不携带跨机 merge-base 证据（本 change 不做自动 verdict），保守 Insufficient。
            // 前端 head-mismatch 文案："HEAD 不同，请查看两机提交记录"（不引导 fetch），
            // 引导用户用详情面板两机并排 commits 做人肉判断。
            Verdict::Insufficient {
                reason: InsufficientReason::HeadMismatch,
            }
        }
        _ => {
            // 任一机 head_full 缺失（老 v4.2 snapshot / 有效空仓 / rev-parse 失败）→ Insufficient
            Verdict::Insufficient {
                reason: InsufficientReason::HeadMissing,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    project_id: Option<Uuid>,
    git_url: Option<String>,
    name_fallback: Option<String>,
}

#[derive(Debug)]
pub enum AggregateError {
    Io(std::io::Error),
    MalformedSnapshot(String, serde_json::Error),
}

impl std::fmt::Display for AggregateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::MalformedSnapshot(p, e) => write!(f, "malformed snapshot {p}: {e}"),
        }
    }
}

impl std::error::Error for AggregateError {}

impl From<std::io::Error> for AggregateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// -------- v4.2 Decision 15: URL → UUID 反查 --------

/// URL → UUID 反查错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UuidLookupError {
    /// URL 无法归一化（格式非法 / 未识别 host）
    Normalize(NormalizeError),
    /// registry 里没有任何项目匹配该 URL
    NotFound,
    /// 多命中 —— URL→UUID 全局唯一 invariant 违反（双机 race 或历史脏数据）
    ///
    /// 调用方策略：
    /// - 新增/关联场景 → 拒绝保存，前端 Fleet Board 弹红色告警
    /// - 防伪校验场景 → 取 `created_at` 最早的 UUID + warning log
    MultipleMatch { url: String, uuids: Vec<Uuid> },
}

impl std::fmt::Display for UuidLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normalize(e) => write!(f, "{e}"),
            Self::NotFound => write!(f, "URL 未在 registry 命中任何 UUID"),
            Self::MultipleMatch { url, uuids } => write!(
                f,
                "URL→UUID 冲突: {url} 命中 {} 个 UUID: {}",
                uuids.len(),
                uuids
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for UuidLookupError {}

impl From<NormalizeError> for UuidLookupError {
    fn from(e: NormalizeError) -> Self {
        Self::Normalize(e)
    }
}

/// 在 registry 里按 git URL 反查 project UUID。
///
/// 语义：
/// 1. 先归一化输入 `url`（SSH→HTTPS、全小写、去 `.git` 后缀）；归一化失败直接返 `Normalize`
/// 2. 遍历 `registry.projects` 每个 RegistryProject，把它的 `git_url` 和 `git_url_aliases`
///    每一项都归一化，任一命中 → 记入候选
/// 3. 命中 0 个 → `NotFound`；命中 1 个 → `Ok(uuid)`；命中 ≥ 2 个 → `MultipleMatch`
///
/// **注意**：私有项目（`local.private_projects`）不参与反查（sync:false 永不出机，也不认领跨机 UUID）。
///
/// **前置**：调用方（Tauri command / CLI）应保证 registry 已 pull 到最新
/// （`dashboard-state pull --rebase` 成功）—— 见 §16.4 add_project 的 pull-first 硬约束。
/// 本函数不做 IO，只在已加载的 registry 上工作。
pub fn find_project_id_by_git_url(
    registry: &Registry,
    url: &str,
) -> Result<Uuid, UuidLookupError> {
    let target = normalize_git_url(url)?;
    let mut hits: Vec<Uuid> = Vec::new();
    for (uuid, project) in &registry.projects {
        if normalize_git_url(&project.git_url).ok().as_deref() == Some(target.as_str()) {
            hits.push(*uuid);
            continue;
        }
        if project
            .git_url_aliases
            .iter()
            .any(|a| normalize_git_url(a).ok().as_deref() == Some(target.as_str()))
        {
            hits.push(*uuid);
        }
    }
    match hits.len() {
        0 => Err(UuidLookupError::NotFound),
        1 => Ok(hits[0]),
        _ => Err(UuidLookupError::MultipleMatch {
            url: target,
            uuids: hits,
        }),
    }
}
