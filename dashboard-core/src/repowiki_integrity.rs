//! Windows v2 读取事务：提交标记/manifest 固定，返回的每份正文与该 manifest 核对。
use super::{read_repowiki_file, RepoWikiError, MAX_GENERATION_BYTES, MAX_REVIEW_BYTES};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

fn changed() -> RepoWikiError {
    RepoWikiError::IntegrityChanged
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn generation_verification_limits_total_bytes_and_elapsed_time() {
        let root = std::env::temp_dir().join(format!("rw-hash-budget-{}", uuid::Uuid::new_v4()));
        let base = root.join(".repowiki");
        std::fs::create_dir_all(&base).unwrap();
        let mut managed = BTreeMap::new();
        for name in ["one", "two"] {
            std::fs::write(base.join(name), b"123456").unwrap();
            managed.insert(name.into(), hash(b"123456"));
        }
        let snapshot = Snapshot {
            generation: String::new(), manifest: String::new(), managed, review: None,
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        assert!(snapshot.verify_with_budget(&root, 12, deadline).is_ok());
        assert!(matches!(snapshot.verify_with_budget(&root, 11, deadline),
                        Err(RepoWikiError::VerificationLimit)));
        assert!(matches!(snapshot.verify_with_budget(&root, 12, Instant::now()),
                        Err(RepoWikiError::VerificationLimit)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) struct Snapshot {
    generation: String,
    manifest: String,
    managed: BTreeMap<String, String>,
    review: Option<String>,
}
fn read_review(repo: &Path) -> Result<Option<String>, RepoWikiError> {
    match crate::windows_files::read(&repo.join(".repowiki/state/review.json"), MAX_REVIEW_BYTES) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| changed()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(changed()),
    }
}
impl Snapshot {
    pub(super) fn open(repo: &Path) -> Result<Self, RepoWikiError> {
        let generation = read_repowiki_file(repo, "state/generation.json", MAX_GENERATION_BYTES)
            .map_err(|_| changed())?;
        let manifest =
            read_repowiki_file(repo, "state/content-manifest.json", MAX_GENERATION_BYTES)
                .map_err(|_| changed())?;
        let g: Value = serde_json::from_str(&generation).map_err(|_| changed())?;
        let m: Value = serde_json::from_str(&manifest).map_err(|_| changed())?;
        let id = g["generation_id"].as_str().ok_or_else(changed)?;
        if g["schema_version"] != 2
            || m["schema_version"] != 2
            || g["status"] != "complete"
            || id.is_empty()
            || m["generation_id"] != id
            || g["manifest_sha256"] != hash(manifest.as_bytes())
            || g.get("layout_version") != m.get("layout_version")
        {
            return Err(changed());
        }
        let layout = match g.get("layout_version") {
            None => 2, // 已有 layout2 产物允许缺字段；有字段时必须是合法整数。
            Some(value) => value.as_u64().ok_or_else(changed)?,
        };
        if ![2, 3].contains(&layout) {
            return Err(changed());
        }
        let managed: BTreeMap<String, String> =
            serde_json::from_value(m["managed_files"].clone()).map_err(|_| changed())?;
        let mut names = std::collections::HashSet::new();
        for (path, digest) in &managed {
            if path.is_empty()
                || path.starts_with('/')
                || path.contains(['\\', ':'])
                || path
                    .split('/')
                    .any(|p| p.is_empty() || p == "." || p == "..")
                || !valid_hash(digest)
                || !names.insert(path.to_lowercase())
            {
                return Err(changed());
            }
        }
        for required in [
            "README.md",
            "navigation.json",
            "catalog/accepted.json",
            "catalog/inventory.json",
            "coverage/coverage.json",
            "evidence/map.json",
            "state/quality-report.json",
        ] {
            if !managed.contains_key(required) {
                return Err(changed());
            }
        }
        let home = if layout == 3 {
            "docs/00-从这里开始.md"
        } else {
            "docs/home.md"
        };
        if !managed.contains_key(home) {
            return Err(changed());
        }
        if m["managed_fingerprint"] != hash(&serde_json::to_vec(&managed).map_err(|_| changed())?) {
            return Err(changed());
        }
        let result = Self {
            generation,
            manifest,
            managed,
            review: read_review(repo)?,
        };
        result.finish(repo)?;
        Ok(result)
    }
    pub(super) fn check(&self, relative: &str, raw: &str) -> Result<(), RepoWikiError> {
        match relative {
            "state/generation.json" if raw == self.generation => Ok(()),
            _ if self
                .managed
                .get(relative)
                .is_some_and(|h| *h == hash(raw.as_bytes())) =>
            {
                Ok(())
            }
            _ => Err(changed()),
        }
    }
    pub(super) fn finish(&self, repo: &Path) -> Result<(), RepoWikiError> {
        if read_repowiki_file(repo, "state/generation.json", MAX_GENERATION_BYTES)
            .map_err(|_| changed())?
            != self.generation
            || read_repowiki_file(repo, "state/content-manifest.json", MAX_GENERATION_BYTES)
                .map_err(|_| changed())?
                != self.manifest
            || read_review(repo)? != self.review
        {
            return Err(changed());
        }
        Ok(())
    }
    pub(super) fn review(&self) -> Option<&str> {
        self.review.as_deref()
    }
    pub(super) fn verify_all(&self, repo: &Path) -> Result<(), RepoWikiError> {
        self.verify_with_budget(
            repo,
            256 * 1024 * 1024,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
    }
    fn verify_with_budget(
        &self,
        repo: &Path,
        mut remaining: u64,
        deadline: std::time::Instant,
    ) -> Result<(), RepoWikiError> {
        // 上游 v2 managed 文件上限128MiB；导航/页面展示预算不能限制证据摘要。
        const MAX_MANAGED_BYTES: u64 = 128 * 1024 * 1024;
        if self.managed.len() > 4096 {
            return Err(RepoWikiError::VerificationLimit);
        }
        for (path, expected) in &self.managed {
            let observed = crate::windows_files::sha256(
                &repo.join(".repowiki").join(path), MAX_MANAGED_BYTES, &mut remaining, deadline,
            ).map_err(|error| match error.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::FileTooLarge =>
                    RepoWikiError::VerificationLimit,
                _ => changed(),
            })?;
            if observed != *expected {
                return Err(changed());
            }
        }
        Ok(())
    }
}
