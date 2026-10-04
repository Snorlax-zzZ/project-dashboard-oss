// M3 §13 灾恢 —— 本机 dashboard-state 副本 + 4 周滚动清理 + 一键重建。
//
// 铁律：force push 前必须二次确认（UI 层做），本模块只提供数据合并逻辑。

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

pub const BACKUP_RETENTION_DAYS: i64 = 28;

/// 备份根：数据目录 `dashboard_state_root` 的**兄弟**目录 `backups`
/// （不放数据目录内部，避免被 dashboard-state 仓 git 跟踪）。数据目录默认
/// `~/.project-dashboard/dashboard-state-clone` 时，backups 即 `~/.project-dashboard/backups`（向后兼容）。
/// 数据目录挪到自定义位置（如 D 盘防 C 盘膨胀）→ backups 一并跟随。
/// `state_root` 无父目录（如盘根）时退化为其下的 `backups`。
pub fn default_backups_root(dashboard_state_root: &Path) -> PathBuf {
    dashboard_state_root
        .parent()
        .map(|p| p.join("backups"))
        .unwrap_or_else(|| dashboard_state_root.join("backups"))
}

/// 拷贝 dashboard-state 关键数据到 `backups/YYYY-MM-DD/`。
///
/// - `dashboard_state_root`：本机 dashboard-state 工作区
/// - `backups_root`：默认 `~/.project-dashboard/backups/`
/// - 拷贝 `registry/` + 全部 `machines/*/`（本机 pull 拿到过其他机副本）
pub fn snapshot_backup(
    dashboard_state_root: &Path,
    backups_root: &Path,
    now: DateTime<Utc>,
) -> std::io::Result<PathBuf> {
    let day = now.format("%Y-%m-%d").to_string();
    let dst = backups_root.join(&day);
    std::fs::create_dir_all(&dst)?;
    copy_dir_all(&dashboard_state_root.join("registry"), &dst.join("registry"))?;
    copy_dir_all(&dashboard_state_root.join("machines"), &dst.join("machines"))?;
    Ok(dst)
}

/// 清理超过 `BACKUP_RETENTION_DAYS` 天的备份子目录。
pub fn prune_old_backups(backups_root: &Path, now: DateTime<Utc>) -> std::io::Result<u32> {
    let entries = match std::fs::read_dir(backups_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        // 目录名格式 YYYY-MM-DD
        if let Ok(day) = chrono::NaiveDate::parse_from_str(&name_s, "%Y-%m-%d") {
            let diff = now.date_naive().signed_duration_since(day).num_days();
            if diff > BACKUP_RETENTION_DAYS {
                std::fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// 一键重建：合并本机 backups 最新 + （可选）另一台机 backups 副本 → 构造新 dashboard-state。
///
/// 现阶段实现：拿本机 backups 里最新一份直接同名回填到 dashboard-state（另一台机合并留 M4）。
/// 返回构造好的目录路径。**force push 由上层 UI 触发，本函数只做本地合并。**
pub fn rebuild_from_local_backup(
    backups_root: &Path,
    target_dir: &Path,
) -> std::io::Result<PathBuf> {
    let latest = latest_backup(backups_root)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no backups found"))?;
    if target_dir.exists() {
        std::fs::remove_dir_all(target_dir)?;
    }
    std::fs::create_dir_all(target_dir)?;
    copy_dir_all(&latest.join("registry"), &target_dir.join("registry"))?;
    copy_dir_all(&latest.join("machines"), &target_dir.join("machines"))?;
    Ok(target_dir.to_path_buf())
}

pub fn latest_backup(backups_root: &Path) -> std::io::Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(backups_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut days: Vec<(chrono::NaiveDate, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if let Ok(day) = chrono::NaiveDate::parse_from_str(&name_s, "%Y-%m-%d") {
            days.push((day, entry.path()));
        }
    }
    days.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(days.into_iter().next().map(|(_, p)| p))
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src).min_depth(1) {
        let entry = entry.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
