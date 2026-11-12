//! 配置、布局和可删除的会话缓存；所有写入都在目标目录原子替换。
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = "# Hortus 通用设置；布局与会话分别保存在 layout.toml 和系统缓存目录。\n# 正文字号（单位：pt）。\nfont_size = 15.0\ndefault_view = \"reading\"\n\n# Agent 启动模板（当前不启用）。command 是可执行文件；裸命令名按应用启动时的 PATH 查找。\n# args 的每个元素是一个 argv 元素，不做 shell 拆分；\"{guide}\" 会替换成完整的内置指南。\n# 含路径分隔符的 command 必须是绝对路径。\n# [agent]\n# command = \"codex\"\n# args = [\"{guide}\"]\n";

/// 内置 Agent 指南；CLI 与 FFI 共用同一份文本。
pub const AGENT_GUIDE: &str = include_str!("../agent-guide.md");

#[derive(Clone, Debug, PartialEq)]
pub struct Directories {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// 不可删除的持久数据（Agent 会话与快照），不是缓存。
    pub data: PathBuf,
}

impl Directories {
    pub fn from_env() -> io::Result<Self> {
        Self::from_lookup(environment_lookup)
    }

    /// 只解析路径，不创建目录、不迁移旧版本目录。
    pub fn resolve_from_env() -> io::Result<Self> {
        Self::resolve_from_lookup(environment_lookup)
    }

    /// 注入环境查询，测试无需修改进程级环境变量。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<PathBuf>) -> io::Result<Self> {
        let directories = Self::resolve_from_lookup(&lookup)?;
        // 空值视同未设置：launchctl setenv 留下的空字符串不能变成相对路径而阻止启动。
        let get = |key: &str| lookup(key).filter(|path| !path.as_os_str().is_empty());
        let home = get("HOME").ok_or_else(|| io::Error::other("config.home_missing"))?;
        let old_config = get("MARKNOTE_CONFIG_DIR").or_else(|| {
            get("HORTUS_CONFIG_DIR")
                .is_none()
                .then(|| home.join(".config/marknote"))
        });
        let old_cache = get("MARKNOTE_CACHE_DIR").or_else(|| {
            get("HORTUS_CACHE_DIR").is_none().then(|| {
                if cfg!(target_os = "macos") {
                    home.join("Library/Caches/MarkNote")
                } else {
                    get("XDG_CACHE_HOME")
                        .unwrap_or_else(|| home.join(".cache"))
                        .join("marknote")
                }
            })
        });
        // 新覆盖目录隔离测试与个人状态；只有显式提供旧覆盖目录时才迁移它。
        if let Some(old) = old_config {
            migrate_directory(&old, &directories.config)?;
        }
        if let Some(old) = old_cache {
            migrate_directory(&old, &directories.cache)?;
        }
        Ok(directories)
    }

    pub fn resolve_from_lookup(lookup: impl Fn(&str) -> Option<PathBuf>) -> io::Result<Self> {
        let get = |key: &str| lookup(key).filter(|path| !path.as_os_str().is_empty());
        let home = get("HOME").ok_or_else(|| io::Error::other("config.home_missing"))?;
        let config = get("HORTUS_CONFIG_DIR").unwrap_or_else(|| home.join(".config/hortus"));
        let cache = get("HORTUS_CACHE_DIR").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                home.join("Library/Caches/Hortus")
            } else {
                get("XDG_CACHE_HOME")
                    .unwrap_or_else(|| home.join(".cache"))
                    .join("hortus")
            }
        });
        // 快照与保留会话不属于可删除缓存，单独放在持久数据根。
        let data = get("HORTUS_DATA_DIR").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/Hortus")
            } else {
                get("XDG_DATA_HOME")
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("hortus")
            }
        });
        if [&config, &cache, &data]
            .into_iter()
            .any(|path| !path.is_absolute())
        {
            return Err(io::Error::other("config.path_not_absolute"));
        }
        Ok(Self {
            config,
            cache,
            data,
        })
    }
}

fn environment_lookup(key: &str) -> Option<PathBuf> {
    if key == "HOME" {
        // 图形界面启动可能没有 HOME；由标准库回退到系统账户的主目录。
        std::env::home_dir()
    } else {
        std::env::var_os(key).map(PathBuf::from)
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
    /// 仅供旧布局迁移；新布局不再存储窗口可见性。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<BTreeMap<String, bool>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WindowFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub screen: Option<String>,
}

/// 窗口身份独立于库路径，保留顺序并为后续同库窗口留出边界。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub id: String,
    pub path: String,
    pub tabs: Vec<String>,
    pub selected: Option<String>,
    #[serde(default)]
    pub views: BTreeMap<String, ViewMode>,
    pub frame: Option<WindowFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<BTreeMap<String, bool>>,
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
    /// None 是旧格式，Some([]) 是明确没有打开的窗口。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<WindowState>>,
    #[serde(default)]
    pub default_visible: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentLaunch {
    pub executable: String,
    pub args: Vec<String>,
}

#[derive(serde::Deserialize)]
struct RawConfig {
    #[serde(default)]
    agent: Option<RawAgent>,
}

#[derive(serde::Deserialize, Default)]
struct RawAgent {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
}

/// 有效指南：`<config>/agent-guide.md` 整篇覆盖内置文本，读取失败不静默回退。
pub fn effective_guide(config_directory: &Path) -> io::Result<String> {
    let path = config_directory.join("agent-guide.md");
    match fs::read(&path) {
        Ok(bytes) => {
            let text =
                String::from_utf8(bytes).map_err(|_| io::Error::other("guide.invalid_utf8"))?;
            if text.trim().is_empty() {
                return Err(io::Error::other("guide.empty"));
            }
            Ok(text)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AGENT_GUIDE.to_string()),
        Err(_) => Err(io::Error::other("guide.read_failed")),
    }
}

/// 裸命令名按应用启动时的 PATH 逐项查找；返回可直接 execve 的绝对路径。
fn resolve_executable(command: &str) -> io::Result<String> {
    resolve_executable_in(command, std::env::var_os("PATH").as_deref())
}

fn resolve_executable_in(command: &str, search: Option<&std::ffi::OsStr>) -> io::Result<String> {
    if command.contains('/') {
        let path = Path::new(command);
        if !path.is_absolute() {
            return Err(io::Error::other("agent.command_not_absolute"));
        }
        return verified_executable(path);
    }
    for directory in std::env::split_paths(search.unwrap_or_default()) {
        // 空项与相对 POSIX 项按进程 cwd 解析；子进程可能先 chdir 到库，
        // 校验对象和 exec 对象会不同，因此不采用。
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(command);
        if let Ok(executable) = verified_executable(&candidate) {
            return Ok(executable);
        }
    }
    Err(io::Error::other("agent.not_found"))
}

/// 只接受常规可执行文件；返回规范化后的绝对路径，保证 exec 的就是校验过的对象。
fn verified_executable(path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::other("agent.not_found")
        } else {
            error
        }
    })?;
    if !metadata.is_file() || !is_executable(path) {
        return Err(io::Error::other("agent.not_executable"));
    }
    let resolved = fs::canonicalize(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::other("agent.not_found")
        } else {
            error
        }
    })?;
    Ok(resolved.to_string_lossy().into_owned())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn access(path: *const std::ffi::c_char, mode: std::ffi::c_int) -> std::ffi::c_int;
        }
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // X_OK = 1；access 还覆盖 ACL 与文件系统标志。
        unsafe { access(path.as_ptr(), 1) == 0 }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
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
            let mut state: SessionState = read_toml(&path)?;
            if state.windows.is_none() {
                state.windows = Some(
                    state
                        .last_vault
                        .as_ref()
                        .map(|path| {
                            let saved = state.recent.iter().find(|v| &v.path == path);
                            WindowState {
                                id: "legacy".into(),
                                path: path.clone(),
                                tabs: saved.map(|v| v.tabs.clone()).unwrap_or_default(),
                                selected: saved.and_then(|v| v.selected.clone()),
                                views: saved.map(|v| v.views.clone()).unwrap_or_default(),
                                frame: None,
                                visible: saved.and_then(|v| v.visible.clone()),
                            }
                        })
                        .into_iter()
                        .collect(),
                );
            }
            Ok(state)
        } else {
            Ok(SessionState::default())
        }
    }

    /// 应用级单一写入者提交完整窗口快照；图状态由同一 Store 锁保护并保留。
    pub fn save_windows(&self, snapshot: SessionState) -> io::Result<()> {
        let mut state = self.session()?;
        state.windows = Some(snapshot.windows.unwrap_or_default());
        state.default_visible = snapshot.default_visible;
        state.last_vault = snapshot.last_vault.or(state.last_vault);
        for vault in snapshot.recent.into_iter().rev() {
            state.recent.retain(|old| old.path != vault.path);
            state.recent.insert(0, vault);
        }
        state.recent.truncate(20);
        write_toml(&self.directories.cache.join("session.toml"), &state)
    }

    pub fn remember(&self, state: VaultState) -> io::Result<SessionState> {
        let mut session = self.session()?;
        session.last_vault = Some(state.path.clone());
        session.recent.retain(|v| v.path != state.path);
        session.recent.insert(0, state);
        session.recent.truncate(20);
        write_toml(&self.directories.cache.join("session.toml"), &session)?;
        self.session()
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

    /// 只读取并校验模板，不创建目录、不启动进程。
    pub fn agent_launch(&self) -> io::Result<AgentLaunch> {
        let text = fs::read_to_string(self.config_path())?;
        let raw: RawConfig =
            toml_edit::de::from_str(&text).map_err(|_| io::Error::other("agent.config_invalid"))?;
        let agent = raw
            .agent
            .ok_or_else(|| io::Error::other("agent.not_configured"))?;
        let command = agent.command.unwrap_or_default();
        if command.trim().is_empty() {
            return Err(io::Error::other("agent.command_missing"));
        }
        let args = agent.args.unwrap_or_default();
        if args.is_empty() || !args.iter().any(|argument| argument.contains("{guide}")) {
            return Err(io::Error::other("agent.guide_missing"));
        }
        if command.contains('\0') || args.iter().any(|argument| argument.contains('\0')) {
            return Err(io::Error::other("agent.invalid_argument"));
        }
        let guide = effective_guide(&self.directories.config)?;
        let executable = resolve_executable(&command)?;
        // 每个元素仍是一个 argv 元素；替换指南不会拆分或再解释内容。
        let args = args
            .iter()
            .map(|argument| argument.replace("{guide}", &guide))
            .collect();
        Ok(AgentLaunch { executable, args })
    }

    pub fn data_root(&self) -> &Path {
        &self.directories.data
    }

    /// 即使用户把覆盖目录或其祖先当成库打开，也不能让配置写进笔记库。
    pub fn validate_vault(&self, root: &Path) -> io::Result<()> {
        let root = fs::canonicalize(root)?;
        for directory in [
            &self.directories.config,
            &self.directories.cache,
            &self.directories.data,
        ] {
            let path = canonicalize_allow_missing(directory)?;
            if path.starts_with(&root) {
                return Err(io::Error::other("config.vault_contains_state"));
            }
        }
        Ok(())
    }
}

enum MissingComponent {
    Name(std::ffi::OsString),
    Current,
    Parent,
}

/// 规范化可能尚不存在的路径：canonicalize 最近已存在的祖先，再拼接剩余部分。
/// 符号链接祖先按真实目标判断，库内不存在的持久数据根不能借“不存在”绕过。
fn canonicalize_allow_missing(path: &Path) -> io::Result<PathBuf> {
    if let Ok(resolved) = fs::canonicalize(path) {
        return Ok(resolved);
    }
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    match component {
                        MissingComponent::Name(name) => resolved.push(name),
                        MissingComponent::Current => {}
                        MissingComponent::Parent => {
                            resolved.pop();
                        }
                    }
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match current.components().next_back() {
                    Some(std::path::Component::Normal(name)) => {
                        missing.push(MissingComponent::Name(name.to_os_string()));
                    }
                    Some(std::path::Component::CurDir) => {
                        missing.push(MissingComponent::Current);
                    }
                    Some(std::path::Component::ParentDir) => {
                        missing.push(MissingComponent::Parent);
                    }
                    _ => return Err(error),
                }
                current = current.parent().ok_or(error)?.to_path_buf();
            }
            Err(error) => return Err(error),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn make_executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn resolved(path: &Path) -> String {
        fs::canonicalize(path)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn bare_commands_resolve_to_the_verified_absolute_file() {
        let root = tempfile::TempDir::new().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let tool = bin.join("hortus-test-agent");
        make_executable(&tool);
        // 空项与相对 PATH 项按进程 cwd 解析，和实际 exec 对象可能不同，必须跳过。
        let search =
            std::ffi::OsString::from(format!(":relative-dir:.:{}:also-relative", bin.display()));
        assert_eq!(
            resolve_executable_in("hortus-test-agent", Some(&search)).unwrap(),
            resolved(&tool)
        );

        // 唯一命中在相对项里时不返回相对 executable。
        let relative_only = std::ffi::OsString::from(format!("relative-dir:{}", bin.display()));
        fs::remove_file(&tool).unwrap();
        assert_eq!(
            resolve_executable_in("hortus-test-agent", Some(&relative_only))
                .unwrap_err()
                .to_string(),
            "agent.not_found"
        );

        // 绝对路径命中同样返回规范化后的同一文件。
        make_executable(&tool);
        let link = root.path().join("link-agent");
        std::os::unix::fs::symlink(&tool, &link).unwrap();
        assert_eq!(
            resolve_executable_in(link.to_str().unwrap(), None).unwrap(),
            resolved(&tool)
        );
    }

    #[test]
    fn directories_and_unverifiable_targets_are_rejected() {
        let root = tempfile::TempDir::new().unwrap();
        let directory = root.path().join("directory");
        fs::create_dir(&directory).unwrap();
        assert_eq!(
            resolve_executable_in(directory.to_str().unwrap(), None)
                .unwrap_err()
                .to_string(),
            "agent.not_executable"
        );

        let plain = root.path().join("plain");
        fs::write(&plain, b"text\n").unwrap();
        assert_eq!(
            resolve_executable_in(plain.to_str().unwrap(), None)
                .unwrap_err()
                .to_string(),
            "agent.not_executable"
        );

        let dangling = root.path().join("dangling");
        std::os::unix::fs::symlink(root.path().join("missing"), &dangling).unwrap();
        assert_eq!(
            resolve_executable_in(dangling.to_str().unwrap(), None)
                .unwrap_err()
                .to_string(),
            "agent.not_found"
        );
    }
}
