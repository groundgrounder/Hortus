//! 配置、布局和可删除的会话缓存；所有写入都在目标目录原子替换。
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = "# Hortus 通用设置；布局与会话分别保存在 layout.toml 和系统缓存目录。\n# 正文字号（单位：pt）。\nfont_size = 15.0\ndefault_view = \"reading\"\n";

#[derive(Clone, Debug, PartialEq)]
pub struct Directories {
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Directories {
    pub fn from_env() -> io::Result<Self> {
        Self::from_lookup(|key| {
            if key == "HOME" {
                // 图形界面启动可能没有 HOME；由标准库回退到系统账户的主目录。
                std::env::home_dir()
            } else {
                std::env::var_os(key).map(PathBuf::from)
            }
        })
    }

    /// 注入环境查询，测试无需修改进程级环境变量。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<PathBuf>) -> io::Result<Self> {
        // 空值视同未设置：launchctl setenv 留下的空字符串不能变成相对路径而阻止启动。
        let get = |key: &str| lookup(key).filter(|path| !path.as_os_str().is_empty());
        let home = get("HOME").ok_or_else(|| io::Error::other("config.home_missing"))?;
        let config_override = get("HORTUS_CONFIG_DIR");
        let cache_override = get("HORTUS_CACHE_DIR");
        let old_config = get("MARKNOTE_CONFIG_DIR");
        let old_cache = get("MARKNOTE_CACHE_DIR");
        let config = config_override
            .clone()
            .unwrap_or_else(|| home.join(".config/hortus"));
        let cache = cache_override.clone().unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                home.join("Library/Caches/Hortus")
            } else {
                get("XDG_CACHE_HOME")
                    .unwrap_or_else(|| home.join(".cache"))
                    .join("hortus")
            }
        });
        let old_config = old_config.or_else(|| {
            config_override
                .is_none()
                .then(|| home.join(".config/marknote"))
        });
        let old_cache = old_cache.or_else(|| {
            cache_override.is_none().then(|| {
                if cfg!(target_os = "macos") {
                    home.join("Library/Caches/MarkNote")
                } else {
                    get("XDG_CACHE_HOME")
                        .unwrap_or_else(|| home.join(".cache"))
                        .join("marknote")
                }
            })
        });
        if [&config, &cache]
            .into_iter()
            .chain(old_config.iter())
            .chain(old_cache.iter())
            .any(|path| !path.is_absolute())
        {
            return Err(io::Error::other("config.path_not_absolute"));
        }
        // 新覆盖目录隔离测试与个人状态；只有显式提供旧覆盖目录时才迁移它。
        if let Some(old) = old_config {
            migrate_directory(&old, &config)?;
        }
        if let Some(old) = old_cache {
            migrate_directory(&old, &cache)?;
        }
        Ok(Self { config, cache })
    }
}

fn migrate_directory(old: &Path, new: &Path) -> io::Result<()> {
    if old == new || !old.try_exists()? {
        return Ok(());
    }
    // 包括悬空符号链接在内，任何已有目标都保留，绝不合并或删除旧目录。
    match fs::symlink_metadata(new) {
        Ok(_) => {
            eprintln!(
                "Hortus：新旧目录同时存在，使用 {}，保留旧目录 {}，不合并",
                new.display(),
                old.display()
            );
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let result = (|| {
        fs::create_dir_all(
            new.parent()
                .ok_or_else(|| io::Error::other("config.path_invalid"))?,
        )?;
        fs::rename(old, new)
    })();
    match result {
        Ok(()) => {
            eprintln!("Hortus：已迁移 {} → {}", old.display(), new.display());
            Ok(())
        }
        Err(error) => {
            let message = format!(
                "Hortus：迁移 {} → {} 失败，旧目录保留：{error}",
                old.display(),
                new.display()
            );
            eprintln!("{message}");
            // 不创建空的新缓存继续启动，以免恢复稿被悄悄忽略。
            Err(io::Error::new(error.kind(), "config.migration_failed"))
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub font_size: f64,
    #[serde(default)]
    pub default_view: ViewMode,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Reading,
    Editing,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PanelGroup {
    pub id: String,
    pub tabs: Vec<String>,
    pub selected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RegionLayout {
    pub groups: Vec<PanelGroup>,
    pub visible: bool,
    pub size: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LayoutState {
    pub regions: BTreeMap<String, RegionLayout>,
}

impl LayoutState {
    pub fn validate(&self) -> io::Result<()> {
        let views = [
            "files",
            "search",
            "tags",
            "outline",
            "backlinks",
            "terminal",
            "changes",
        ];
        let mut seen = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for region in ["left", "right", "bottom"] {
            let r = self
                .regions
                .get(region)
                .ok_or_else(|| io::Error::other("config.layout_missing_region"))?;
            let bounds = if region == "bottom" {
                120.0..=520.0
            } else {
                180.0..=480.0
            };
            if !bounds.contains(&r.size) {
                return Err(io::Error::other("config.layout_invalid_size"));
            }
            for group in &r.groups {
                if group.id.is_empty()
                    || !ids.insert(&group.id)
                    || group.tabs.is_empty()
                    || !group.tabs.contains(&group.selected)
                {
                    return Err(io::Error::other("config.layout_invalid_group"));
                }
                for tab in &group.tabs {
                    if !views.contains(&tab.as_str()) || !seen.insert(tab.as_str()) {
                        return Err(io::Error::other("config.layout_invalid_view"));
                    }
                }
            }
        }
        if self.regions.len() != 3 || seen.len() != views.len() {
            return Err(io::Error::other("config.layout_incomplete"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct VaultState {
    pub path: String,
    pub tabs: Vec<String>,
    pub selected: Option<String>,
    #[serde(default)]
    pub views: BTreeMap<String, ViewMode>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DiagramState {
    pub vault: String,
    pub note: String,
    pub block: u32,
    pub collapsed: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagrams: Vec<DiagramState>,
    pub last_vault: Option<String>,
    #[serde(default)]
    pub recent: Vec<VaultState>,
}

pub struct Store {
    pub directories: Directories,
}

impl Store {
    pub fn open(directories: Directories) -> io::Result<Self> {
        fs::create_dir_all(&directories.config)?;
        fs::create_dir_all(&directories.cache)?;
        let store = Self { directories };
        if !store.config_path().exists() {
            atomic_write(&store.config_path(), DEFAULT_CONFIG.as_bytes())?;
        }
        store.settings()?;
        Ok(store)
    }

    fn config_path(&self) -> PathBuf {
        self.directories.config.join("config.toml")
    }

    pub fn settings(&self) -> io::Result<Settings> {
        let settings: Settings = read_toml(&self.config_path())?;
        if !(9.0..=40.0).contains(&settings.font_size) {
            return Err(io::Error::other("config.font_size_invalid"));
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        if !(9.0..=40.0).contains(&settings.font_size) {
            return Err(io::Error::other("config.font_size_invalid"));
        }
        let mut document = fs::read_to_string(self.config_path())?
            .parse::<toml_edit::DocumentMut>()
            .map_err(io::Error::other)?;
        // 只改已知设置的值，保留用户注释和未知设置。
        let mut value = toml_edit::Value::from(settings.font_size);
        if let Some(old) = document["font_size"].as_value() {
            *value.decor_mut() = old.decor().clone();
        }
        document["font_size"] = toml_edit::Item::Value(value);
        document["default_view"] = toml_edit::value(match settings.default_view {
            ViewMode::Reading => "reading",
            ViewMode::Editing => "editing",
        });
        atomic_write(&self.config_path(), document.to_string().as_bytes())
    }

    pub fn layout(&self) -> io::Result<Option<LayoutState>> {
        let path = self.directories.config.join("layout.toml");
        if !path.exists() {
            return Ok(None);
        }
        let state: LayoutState = read_toml(&path)?;
        state.validate()?;
        Ok(Some(state))
    }

    pub fn save_layout(&self, state: &LayoutState) -> io::Result<()> {
        state.validate()?;
        write_toml(&self.directories.config.join("layout.toml"), state)
    }

    pub fn session(&self) -> io::Result<SessionState> {
        let path = self.directories.cache.join("session.toml");
        if path.exists() {
            read_toml(&path)
        } else {
            Ok(SessionState::default())
        }
    }

    pub fn remember(&self, state: VaultState) -> io::Result<SessionState> {
        let mut session = self.session()?;
        session.last_vault = Some(state.path.clone());
        session.recent.retain(|v| v.path != state.path);
        session.recent.insert(0, state);
        session.recent.truncate(20);
        write_toml(&self.directories.cache.join("session.toml"), &session)?;
        Ok(session)
    }

    pub fn remember_diagram(&self, state: DiagramState) -> io::Result<()> {
        let mut session = self.session()?;
        session.diagrams.retain(|s| {
            !(s.vault == state.vault && s.note == state.note && s.block == state.block)
        });
        if !state.collapsed.is_empty() {
            session.diagrams.push(state);
        }
        write_toml(&self.directories.cache.join("session.toml"), &session)
    }

    /// 即使用户把覆盖目录或其祖先当成库打开，也不能让配置写进笔记库。
    pub fn validate_vault(&self, root: &Path) -> io::Result<()> {
        let root = fs::canonicalize(root)?;
        for directory in [&self.directories.config, &self.directories.cache] {
            if fs::canonicalize(directory)?.starts_with(&root) {
                return Err(io::Error::other("config.vault_contains_state"));
            }
        }
        Ok(())
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    toml_edit::de::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)
}

fn write_toml(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let text = toml_edit::ser::to_string_pretty(value).map_err(io::Error::other)?;
    atomic_write(path, text.as_bytes())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("config.path_invalid"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(io::Error::other)?;
    fs::File::open(parent)?.sync_all()
}
