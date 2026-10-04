//! 只维护看板显示偏好，不修改源会话或将 ID 同步到共享仓。
use crate::session_storage::{
    self, SessionStorage, SessionStorageError, PREFERENCE_READ, PREFERENCE_WRITE,
};
use crate::sessions::{SessionPage, SessionSource};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    str::FromStr,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}
impl FromStr for Visibility {
    type Err = SessionStorageError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "visible" => Ok(Self::Visible),
            "hidden" => Ok(Self::Hidden),
            _ => Err(SessionStorageError::new(
                "INVALID_SESSION_VISIBILITY",
                "显示范围只支持 visible 或 hidden",
            )),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiddenSessions {
    version: u32,
    hidden: BTreeMap<String, HashSet<String>>,
}
impl Default for HiddenSessions {
    fn default() -> Self {
        Self {
            version: 1,
            hidden: session_storage::SOURCES
                .into_iter()
                .map(|s| (s.to_owned(), HashSet::new()))
                .collect(),
        }
    }
}
impl HiddenSessions {
    pub fn contains(&self, source: &str, id: &str) -> bool {
        self.hidden.get(source).is_some_and(|ids| ids.contains(id))
    }
    fn validate(&self) -> Result<(), SessionStorageError> {
        if self.version != 1 {
            return Err(SessionStorageError::new(
                PREFERENCE_READ,
                "hidden.json 版本不受支持",
            ));
        }
        for (source, ids) in &self.hidden {
            session_storage::validate_source(source).map_err(|e| {
                SessionStorageError::new(PREFERENCE_READ, format!("hidden.json：{e}"))
            })?;
            for id in ids {
                session_storage::validate_id(id).map_err(|e| {
                    SessionStorageError::new(PREFERENCE_READ, format!("hidden.json：{e}"))
                })?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HiddenSessionUpdate {
    pub source: String,
    pub session_id: String,
    pub hidden: bool,
}

impl SessionStorage {
    fn read_hidden_unchecked(&self) -> Result<HiddenSessions, SessionStorageError> {
        let value: HiddenSessions =
            session_storage::read_optional(&self.root().join("hidden.json"), PREFERENCE_READ)?
                .unwrap_or_default();
        value.validate()?;
        Ok(value)
    }
    pub fn load_hidden(&self) -> Result<HiddenSessions, SessionStorageError> {
        self.ensure_initialized()?;
        self.read_hidden_unchecked()
    }
    pub fn set_hidden(
        &self,
        source: &str,
        id: &str,
        hidden: bool,
    ) -> Result<HiddenSessionUpdate, SessionStorageError> {
        session_storage::validate_source(source)?;
        session_storage::validate_id(id)?;
        self.ensure_initialized()?;
        let _guard = self.lock("SESSION_PREFERENCE_BUSY")?;
        let mut data = self.read_hidden_unchecked()?;
        let ids = data.hidden.entry(source.to_owned()).or_default();
        if hidden {
            ids.insert(id.to_owned());
        } else {
            ids.remove(id);
        }
        // 磁盘使用稳定、有序且去重的数组，便于人工检查与修复。
        let sorted: BTreeMap<_, Vec<_>> = data
            .hidden
            .iter()
            .map(|(source, ids)| {
                let mut ids: Vec<_> = ids.iter().collect();
                ids.sort();
                (source, ids)
            })
            .collect();
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({"version":1,"hidden":sorted}))
            .map_err(|e| SessionStorageError::new(PREFERENCE_WRITE, e))?;
        session_storage::atomic_write(&self.root().join("hidden.json"), &bytes, PREFERENCE_WRITE)?;
        Ok(HiddenSessionUpdate {
            source: source.to_owned(),
            session_id: id.to_owned(),
            hidden,
        })
    }
}

/// 产品查询：读取偏好一次，扫描当前来源一次，过滤后才计算 total 与页码。
pub fn list_visible_paged(
    source: &dyn SessionSource,
    storage: &SessionStorage,
    query: Option<&str>,
    page: usize,
    page_size: usize,
    visibility: Visibility,
) -> Result<SessionPage, SessionStorageError> {
    session_storage::validate_source(source.source_id())?;
    storage.ensure_initialized()?;
    let hidden = storage.load_hidden()?;
    let favorites = storage.load_favorites()?;
    let mut metas = source.scan();
    metas.retain(|meta| {
        hidden.contains(source.source_id(), &meta.session_id) == (visibility == Visibility::Hidden)
    });
    Ok(crate::sessions::paginate_metas(
        metas, &favorites, query, page, page_size, true,
    ))
}
