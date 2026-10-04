//! 跨机同步结果的本机 sidecar。与 snapshot 分离：采集成功不代表推送成功。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncOutcome {
    Succeeded,
    PushFailed,
    Failed,
    Unconfigured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncAttempt {
    pub completed_at: DateTime<Utc>,
    pub outcome: SyncOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncStatus {
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_attempt: Option<SyncAttempt>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredStatus {
    schema_version: u32,
    scope_key: String,
    status: SyncStatus,
}

pub struct SyncStatusStore {
    path: PathBuf,
    scope_key: String,
}

impl SyncStatusStore {
    pub fn new(root: &Path, machine_id: &str, remote: Option<&str>) -> Result<Self, String> {
        let root = std::path::absolute(root).map_err(|e| e.to_string())?;
        let mut name = root
            .file_name()
            .ok_or("数据仓路径不能是文件系统根目录")?
            .to_os_string();
        name.push(".sync-status.json");
        // URL 只参与身份散列，不在本机状态文件中复制认证信息。
        let scope = serde_json::to_vec(&(machine_id, remote)).map_err(|e| e.to_string())?;
        Ok(Self {
            path: root.with_file_name(name),
            scope_key: format!("{:x}", Sha256::digest(scope)),
        })
    }

    fn read_stored(&self) -> Result<Option<StoredStatus>, String> {
        let saved: Option<StoredStatus> =
            crate::session_storage::read_optional(&self.path, "SYNC_STATUS_READ")
                .map_err(|e| e.to_string())?;
        if saved.as_ref().is_some_and(|s| s.schema_version != 1) {
            return Err("不支持的同步记录版本".into());
        }
        Ok(saved)
    }

    /// 严格只读；缺失不创建目录，不触发网络或配置迁移。
    pub fn read(&self) -> Result<SyncStatus, String> {
        Ok(self
            .read_stored()?
            .filter(|s| s.scope_key == self.scope_key)
            .map(|s| s.status)
            .unwrap_or_default())
    }

    pub fn record(&self, outcome: SyncOutcome, completed_at: DateTime<Utc>) -> Result<(), String> {
        let parent = self.path.parent().ok_or("同步记录路径缺少父目录")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock)
                    if started.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(format!("同步记录写锁不可用：{e}")),
            }
        }
        let previous = self.read_stored()?;
        // 配置已切换时，旧调用的迟到写入不能抹掉新配置下的记录。
        if previous.as_ref().is_some_and(|s| {
            s.scope_key != self.scope_key
                && s.status
                    .last_attempt
                    .as_ref()
                    .is_some_and(|a| a.completed_at > completed_at)
        }) {
            return Ok(());
        }
        let mut status = previous
            .filter(|s| s.scope_key == self.scope_key)
            .map(|s| s.status)
            .unwrap_or_default();
        if outcome == SyncOutcome::Succeeded {
            status.last_success_at = Some(
                status
                    .last_success_at
                    .map_or(completed_at, |old| old.max(completed_at)),
            );
        }
        if status
            .last_attempt
            .as_ref()
            .is_none_or(|a| a.completed_at <= completed_at)
        {
            status.last_attempt = Some(SyncAttempt {
                completed_at,
                outcome,
            });
        }
        let bytes = serde_json::to_vec_pretty(&StoredStatus {
            schema_version: 1,
            scope_key: self.scope_key.clone(),
            status,
        })
        .map_err(|e| e.to_string())?;
        crate::config::write_local_config_atomic(&self.path, &bytes).map_err(|e| e.to_string())
    }
}
