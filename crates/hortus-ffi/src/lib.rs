//! Swift 只通过这层调用 Rust；目录 I/O 和扫描在 Swift 后台任务及 Rust 工作线程执行。
use hortus_config::{Directories, LayoutState, Store, VaultState};
use hortus_search::SearchIndex;
use hortus_vault::Vault;
use std::fmt;
use std::sync::{Arc, Mutex};

mod diagram;
mod document;
mod media;
mod scene;
pub use diagram::*;
pub use document::*;
pub use scene::*;

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    Failure { kind: String },
}
impl CoreError {
    fn failure(kind: impl Into<String>) -> Self {
        Self::Failure { kind: kind.into() }
    }
}
impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failure { kind } => write!(f, "{kind}"),
        }
    }
}
impl std::error::Error for CoreError {}
impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        let message = error.to_string();
        if message.contains('.')
            && message
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Self::failure(message);
        }
        Self::failure(match error.kind() {
            std::io::ErrorKind::NotFound => "io.not_found",
            std::io::ErrorKind::PermissionDenied => "io.permission_denied",
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                "io.invalid_input"
            }
            std::io::ErrorKind::AlreadyExists => "io.already_exists",
            _ => "io.failure",
        })
    }
}
impl From<serde_json::Error> for CoreError {
    fn from(_: serde_json::Error) -> Self {
        Self::failure("data.invalid")
    }
}

#[derive(uniffi::Record)]
pub struct TreeEntry {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub directory: bool,
}
#[derive(uniffi::Record)]
pub struct TreeBatch {
    pub upserted: Vec<TreeEntry>,
    pub removed: Vec<String>,
    pub scan_complete: bool,
    pub scan_micros: u64,
    pub errors: Vec<String>,
}

#[derive(uniffi::Record)]
pub struct SearchMatchRange {
    pub start: u32,
    pub length: u32,
}

#[derive(uniffi::Record)]
pub struct QuickOpenHit {
    pub path: String,
    pub title: String,
    pub title_matches: Vec<SearchMatchRange>,
    pub path_matches: Vec<SearchMatchRange>,
}

#[derive(uniffi::Record)]
pub struct SearchTextMatch {
    pub start: u32,
    pub length: u32,
    pub snippet: String,
    pub snippet_start: u32,
}

#[derive(uniffi::Record)]
pub struct SearchResult {
    pub path: String,
    pub title: String,
    pub count: u32,
    pub matches: Vec<SearchTextMatch>,
}

#[derive(uniffi::Object)]
pub struct SearchCancellation {
    cancelled: std::sync::atomic::AtomicBool,
}

#[uniffi::export]
impl SearchCancellation {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[derive(uniffi::Object)]
pub struct WorkspaceCore {
    store: Mutex<Store>,
}

#[uniffi::export]
impl WorkspaceCore {
    #[uniffi::constructor]
    pub fn in_directories(config: String, cache: String) -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self {
            store: Mutex::new(Store::open(Directories {
                config: config.into(),
                cache: cache.into(),
            })?),
        }))
    }
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self {
            store: Mutex::new(Store::open(Directories::from_env()?)?),
        }))
    }

    pub fn layout_json(&self) -> Result<Option<String>, CoreError> {
        self.store
            .lock()
            .unwrap()
            .layout()?
            .map(|value| serde_json::to_string(&value).map_err(Into::into))
            .transpose()
    }

    pub fn save_layout(&self, json: String) -> Result<(), CoreError> {
        let state: LayoutState = serde_json::from_str(&json)?;
        Ok(self.store.lock().unwrap().save_layout(&state)?)
    }

    pub fn session_json(&self) -> Result<String, CoreError> {
        Ok(serde_json::to_string(
            &self.store.lock().unwrap().session()?,
        )?)
    }

    pub fn default_view(&self) -> Result<String, CoreError> {
        Ok(match self.store.lock().unwrap().settings()?.default_view {
            hortus_config::ViewMode::Reading => "reading",
            hortus_config::ViewMode::Editing => "editing",
        }
        .into())
    }
    pub fn remember_views(
        &self,
        vault: String,
        views: std::collections::HashMap<String, String>,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap();
        if let Some(mut state) = store
            .session()?
            .recent
            .into_iter()
            .find(|s| s.path == vault)
        {
            state.views = views
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        if v == "editing" {
                            hortus_config::ViewMode::Editing
                        } else {
                            hortus_config::ViewMode::Reading
                        },
                    )
                })
                .collect();
            store.remember(state)?;
        }
        Ok(())
    }

    pub fn font_size(&self) -> Result<f64, CoreError> {
        Ok(self.store.lock().unwrap().settings()?.font_size)
    }

    pub fn remember(
        &self,
        path: String,
        tabs: Vec<String>,
        selected: Option<String>,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap();
        store.validate_vault(std::path::Path::new(&path))?;
        store.remember(VaultState {
            path,
            tabs,
            selected,
            views: Default::default(),
        })?;
        Ok(())
    }

    pub fn collapsed_nodes(
        &self,
        vault: String,
        note: String,
        block: u32,
    ) -> Result<Vec<String>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .session()?
            .diagrams
            .into_iter()
            .find(|s| s.vault == vault && s.note == note && s.block == block)
            .map(|s| s.collapsed)
            .unwrap_or_default())
    }

    pub fn remember_diagram(
        &self,
        vault: String,
        note: String,
        block: u32,
        collapsed: Vec<String>,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap();
        store.validate_vault(std::path::Path::new(&vault))?;
        Ok(store.remember_diagram(hortus_config::DiagramState {
            vault,
            note,
            block,
            collapsed,
        })?)
    }

    pub fn open_vault(&self, path: String) -> Result<Arc<VaultHandle>, CoreError> {
        self.store
            .lock()
            .unwrap()
            .validate_vault(std::path::Path::new(&path))?;
        Ok(Arc::new(VaultHandle {
            vault: Vault::open(path)?,
            search: Mutex::new(SearchIndex::new()),
        }))
    }
}

#[derive(uniffi::Object)]
pub struct VaultHandle {
    vault: Vault,
    search: Mutex<SearchIndex>,
}

#[uniffi::export]
impl VaultHandle {
    pub fn root(&self) -> String {
        self.vault.root().to_string_lossy().into_owned()
    }
    pub fn wait_for_batches(&self) -> Vec<TreeBatch> {
        self.vault
            .wait_for_batches()
            .into_iter()
            .map(|batch| TreeBatch {
                upserted: batch
                    .upserted
                    .into_iter()
                    .map(|e| TreeEntry {
                        path: e.path,
                        parent: e.parent,
                        name: e.name,
                        directory: e.directory,
                    })
                    .collect(),
                removed: batch.removed,
                scan_complete: batch.scan_complete,
                scan_micros: batch.scan_micros,
                errors: batch
                    .errors
                    .iter()
                    .map(|_| "vault.scan_failed".into())
                    .collect(),
            })
            .collect()
    }
    pub fn read_note(&self, path: String) -> Result<String, CoreError> {
        Ok(self.vault.read_note(&path)?)
    }

    pub fn update_search_index(&self, upserted: Vec<String>, removed: Vec<String>) {
        // 首次扫描与监听增量都经过这里；read_note 返回与 DocumentSession 相同的规范化正文。
        let updates = upserted
            .into_iter()
            .map(|path| {
                let text = self.vault.read_note(&path).ok();
                (path, text)
            })
            .collect::<Vec<_>>();
        let mut search = self.search.lock().unwrap();
        for path in removed {
            search.remove(&path);
        }
        for (path, text) in updates {
            if let Some(text) = text {
                search.upsert(path, text);
            } else {
                search.remove(&path);
            }
        }
    }

    pub fn quick_open(&self, query: String, limit: u32) -> Vec<QuickOpenHit> {
        self.search
            .lock()
            .unwrap()
            .quick_open(&query, limit as usize)
            .into_iter()
            .map(|hit| QuickOpenHit {
                path: hit.path,
                title: hit.title,
                title_matches: hit
                    .title_matches
                    .into_iter()
                    .map(|range| SearchMatchRange {
                        start: range.start,
                        length: range.length,
                    })
                    .collect(),
                path_matches: hit
                    .path_matches
                    .into_iter()
                    .map(|range| SearchMatchRange {
                        start: range.start,
                        length: range.length,
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn search(
        &self,
        query: String,
        max_matches_per_note: u32,
        cancellation: Arc<SearchCancellation>,
    ) -> Vec<SearchResult> {
        self.search
            .lock()
            .unwrap()
            .search_cancelled(
                &query,
                max_matches_per_note as usize,
                Some(&cancellation.cancelled),
            )
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchResult {
                path: result.path,
                title: result.title,
                count: result.count,
                matches: result
                    .matches
                    .into_iter()
                    .map(|matched| SearchTextMatch {
                        start: matched.start,
                        length: matched.length,
                        snippet: matched.snippet,
                        snippet_start: matched.snippet_start,
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn close(&self) {
        self.vault.close();
    }
}
