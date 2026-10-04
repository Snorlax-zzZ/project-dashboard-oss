//! 本机会话目录与偏好；源工具文件只读，升级导入完成后不再访问旧目录。
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

pub const SOURCES: [&str; 4] = ["claude-cli", "codex", "pi", "grok"];
const STORAGE_READ: &str = "SESSION_STORAGE_READ_FAILED";
const STORAGE_WRITE: &str = "SESSION_STORAGE_WRITE_FAILED";
pub(crate) const PREFERENCE_READ: &str = "SESSION_PREFERENCE_READ_FAILED";
pub(crate) const PREFERENCE_WRITE: &str = "SESSION_PREFERENCE_WRITE_FAILED";

#[derive(Debug)]
pub struct SessionStorageError {
    tag: &'static str,
    detail: String,
}
impl SessionStorageError {
    pub(crate) fn new(tag: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            tag,
            detail: detail.to_string(),
        }
    }
}
impl std::fmt::Display for SessionStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.tag, self.detail)
    }
}
impl std::error::Error for SessionStorageError {}

type Result<T> = std::result::Result<T, SessionStorageError>;

#[derive(Debug, Default)]
pub struct InitializationReport {
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SessionStorage {
    root: PathBuf,
    legacy: PathBuf,
    ready: Arc<AtomicBool>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    version: u32,
}

pub fn default_storage() -> &'static SessionStorage {
    static STORAGE: OnceLock<SessionStorage> = OnceLock::new();
    STORAGE.get_or_init(|| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        SessionStorage::new(
            home.join(".project-dashboard/sessions"),
            home.join(".cache/project-dashboard/sessions"),
        )
    })
}

pub(crate) fn validate_source(source: &str) -> Result<()> {
    if SOURCES.contains(&source) {
        Ok(())
    } else {
        Err(SessionStorageError::new(
            "UNSUPPORTED_SESSION_SOURCE",
            format!("不支持的会话来源：{source}"),
        ))
    }
}
pub(crate) fn validate_id(id: &str) -> Result<()> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        Ok(())
    } else {
        Err(SessionStorageError::new(
            "INVALID_SESSION_ID",
            "session id 含不允许的字符或长度超限",
        ))
    }
}

/// 文件偏好和迁移只接受普通文件，缺失与不可读严格区分。
pub(crate) fn read_optional<T: DeserializeOwned>(
    path: &Path,
    tag: &'static str,
) -> Result<Option<T>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SessionStorageError::new(
                tag,
                format!("{}：{e}", path.display()),
            ))
        }
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(SessionStorageError::new(
                tag,
                format!("{} 不是普通文件", path.display()),
            ));
        }
        Ok(_) => {}
    }
    let bytes = fs::read(path)
        .map_err(|e| SessionStorageError::new(tag, format!("{}：{e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| SessionStorageError::new(tag, format!("{} 格式错误：{e}", path.display())))
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8], tag: &'static str) -> Result<()> {
    crate::config::write_local_config_atomic(path, bytes)
        .map_err(|e| SessionStorageError::new(tag, format!("保存 {} 失败：{e}", path.display())))
}
fn real_directory(path: &Path, create: bool) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(SessionStorageError::new(
            STORAGE_READ,
            format!("{} 不是普通目录", path.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !create => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|e| SessionStorageError::new(STORAGE_WRITE, e))?;
            Ok(true)
        }
        Err(e) => Err(SessionStorageError::new(
            STORAGE_READ,
            format!("{}：{e}", path.display()),
        )),
    }
}

impl SessionStorage {
    pub fn new(root: PathBuf, legacy: PathBuf) -> Self {
        Self {
            root,
            legacy,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn cache_dir(&self, source: &str) -> PathBuf {
        self.root.join("cache").join(source)
    }
    pub fn favorites_file(&self) -> PathBuf {
        self.root.join("favorites.json")
    }

    /// 锁文件不会被原子替换；所有新版初始化与偏好写入共用这一 inode。
    pub(crate) fn lock(&self, tag: &'static str) -> Result<File> {
        real_directory(&self.root, true)?;
        let path = self.root.join(".write.lock");
        match fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                return Err(SessionStorageError::new(tag, "会话写锁不是普通文件"));
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(SessionStorageError::new(tag, e))
            }
            _ => {}
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|e| SessionStorageError::new(tag, e))?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(fs::TryLockError::WouldBlock) if start.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(SessionStorageError::new(
                        tag,
                        "本机会话数据正被其他进程使用，请稍后重试",
                    ))
                }
                Err(fs::TryLockError::Error(e)) => return Err(SessionStorageError::new(tag, e)),
            }
        }
    }

    pub fn ensure_initialized(&self) -> Result<InitializationReport> {
        if self.ready.load(Ordering::Acquire) {
            return Ok(InitializationReport::default());
        }
        let _guard = self.lock("SESSION_STORAGE_BUSY")?;
        let marker = self.root.join(".layout-v1.json");
        if let Some(layout) = read_optional::<Layout>(&marker, STORAGE_READ)? {
            if layout.version != 1 {
                return Err(SessionStorageError::new(
                    STORAGE_READ,
                    "不支持的本机会话目录版本",
                ));
            }
            self.load_favorites()?;
            self.ready.store(true, Ordering::Release);
            return Ok(InitializationReport::default());
        }
        if read_optional::<HashMap<String, bool>>(&self.favorites_file(), PREFERENCE_READ)?
            .is_none()
        {
            // 旧目录本身也不允许通过符号链接指向其他位置。
            real_directory(&self.legacy, false)?;
            let favorites = read_optional::<HashMap<String, bool>>(
                &self.legacy.join("favorites.json"),
                PREFERENCE_READ,
            )?
            .unwrap_or_default();
            self.write_favorites_unlocked(&favorites)?;
        }
        let mut report = InitializationReport::default();
        real_directory(&self.root.join("cache"), true)?;
        for source in SOURCES {
            real_directory(&self.cache_dir(source), true)?;
            let origin = if source == "claude-cli" {
                self.legacy.clone()
            } else {
                self.legacy.join(source)
            };
            self.import_cache(source, &origin, &mut report);
        }
        atomic_write(
            &marker,
            &serde_json::to_vec(&Layout { version: 1 }).unwrap(),
            STORAGE_WRITE,
        )?;
        self.ready.store(true, Ordering::Release);
        for warning in &report.warnings {
            eprintln!("[SESSION_STORAGE_CACHE_WARNING] {warning}");
        }
        Ok(report)
    }

    pub fn load_favorites(&self) -> Result<HashMap<String, bool>> {
        read_optional(&self.favorites_file(), PREFERENCE_READ)?.ok_or_else(|| {
            SessionStorageError::new(
                PREFERENCE_READ,
                format!("收藏文件缺失：{}", self.favorites_file().display()),
            )
        })
    }
    fn write_favorites_unlocked(&self, favorites: &HashMap<String, bool>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(favorites)
            .map_err(|e| SessionStorageError::new(PREFERENCE_WRITE, e))?;
        atomic_write(&self.favorites_file(), &bytes, PREFERENCE_WRITE)
    }
    pub fn toggle_favorite(&self, session_id: &str) -> Result<bool> {
        validate_id(session_id)?;
        self.ensure_initialized()?;
        let _guard = self.lock("SESSION_PREFERENCE_BUSY")?;
        let mut favorites = self.load_favorites()?;
        let next = !favorites.get(session_id).copied().unwrap_or(false);
        if next {
            favorites.insert(session_id.to_owned(), true);
        } else {
            favorites.remove(session_id);
        }
        self.write_favorites_unlocked(&favorites)?;
        Ok(next)
    }
    /// 显式整表替换的兼容入口；产品单条修改只走持锁读改写的 toggle。
    pub fn save_favorites(&self, favorites: &HashMap<String, bool>) -> Result<()> {
        self.ensure_initialized()?;
        let _guard = self.lock("SESSION_PREFERENCE_BUSY")?;
        self.load_favorites()?;
        self.write_favorites_unlocked(favorites)
    }

    fn import_cache(&self, source: &str, origin: &Path, report: &mut InitializationReport) {
        let result = (|| -> Result<()> {
            if !real_directory(&self.legacy, false)? || !real_directory(origin, false)? {
                return Ok(());
            }
            let entries =
                fs::read_dir(origin).map_err(|e| SessionStorageError::new(STORAGE_READ, e))?;
            for entry in entries {
                let copy = (|| -> Result<()> {
                    let entry = entry.map_err(|e| SessionStorageError::new(STORAGE_READ, e))?;
                    let kind = entry
                        .file_type()
                        .map_err(|e| SessionStorageError::new(STORAGE_READ, e))?;
                    let path = entry.path();
                    if !kind.is_file()
                        || path.extension().and_then(|v| v.to_str()) != Some("json")
                        || entry.file_name() == "favorites.json"
                    {
                        return Ok(());
                    }
                    let bytes = fs::read(&path).map_err(|e| {
                        SessionStorageError::new(STORAGE_READ, format!("{}：{e}", path.display()))
                    })?;
                    if !crate::sessions::valid_cache_bytes(source, &bytes) {
                        return Err(SessionStorageError::new(
                            STORAGE_READ,
                            format!("跳过损坏或未知格式缓存 {}，将从源会话重建", path.display()),
                        ));
                    }
                    let target = self.cache_dir(source).join(entry.file_name());
                    match fs::symlink_metadata(&target) {
                        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                            return Err(SessionStorageError::new(
                                STORAGE_WRITE,
                                format!("跳过非普通缓存目标 {}", target.display()),
                            ))
                        }
                        Ok(_) => {
                            let current = fs::read(&target)
                                .map_err(|e| SessionStorageError::new(STORAGE_READ, e))?;
                            if crate::sessions::valid_cache_bytes(source, &current) {
                                return Ok(());
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(SessionStorageError::new(STORAGE_READ, e)),
                    }
                    atomic_write(&target, &bytes, STORAGE_WRITE)?;
                    let saved =
                        fs::read(&target).map_err(|e| SessionStorageError::new(STORAGE_READ, e))?;
                    if saved != bytes {
                        return Err(SessionStorageError::new(
                            STORAGE_WRITE,
                            format!("缓存复制校验失败：{}", target.display()),
                        ));
                    }
                    Ok(())
                })();
                if let Err(error) = copy {
                    report.warnings.push(error.to_string());
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            report.warnings.push(error.to_string());
        }
    }
}
