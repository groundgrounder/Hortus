//! 配置、布局和可删除的会话缓存；所有写入都在目标目录原子替换。
mod sha256;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_CONFIG: &str = "# Hortus 通用设置；布局与会话分别保存在 layout.toml 和系统缓存目录。\n# 正文字号（单位：pt）。\nfont_size = 15.0\ndefault_view = \"reading\"\n\n# 终端等宽字体族；留空或删除跟随系统 SF Mono。\n# terminal_font = \"Menlo\"\n\n# Agent 启动模板（当前不启用）。command 是可执行文件；裸命令名按应用启动时的 PATH 查找。\n# args 的每个元素是一个 argv 元素，不做 shell 拆分；\"{guide}\" 会替换成完整的内置指南。\n# 含路径分隔符的 command 必须是绝对路径。\n# [agent]\n# command = \"codex\"\n# args = [\"{guide}\"]\n";

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
    /// 终端等宽字体族；缺省或空白表示跟随系统 SF Mono。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font: Option<String>,
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
        if self.regions.len() != 3 {
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
    /// 每窗口工具布局；终端只用 "terminal" 占位，不写实例 UUID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutState>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DiagramState {
    pub vault: String,
    pub note: String,
    pub block: u32,
    #[serde(default)]
    pub collapsed: Vec<String>,
    /// 专注模式的缩放与平移；旧版 session.toml 没有这些字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_offset_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_offset_y: Option<f64>,
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

/// 单个命名 Agent 配置档；对应 `config.toml` 中的 `[[agent.profiles]]`。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// 自定义命令启动 Agent 的内核规格；多行指南写入缓存只读文件并通过环境变量注入。
#[derive(Clone, Debug, PartialEq)]
pub struct CustomAgentLaunch {
    pub executable: String,
    pub args: Vec<String>,
    pub guide_file: String,
    pub path: String,
}

const CUSTOM_AGENT_HISTORY_LIMIT: usize = 12;

/// 设置界面的完整配置快照；`digest` 是 `config.toml` 原始字节的指纹。
#[derive(Clone, Debug, PartialEq)]
pub struct EditableConfig {
    pub config_path: String,
    pub digest: String,
    pub settings: Settings,
    /// `[agent]` 或 `[[agent.profiles]]` 是否已配置；未配置时 command/args 为空。
    pub agent_configured: bool,
    pub agent_command: Option<String>,
    pub agent_args: Vec<String>,
    pub agent_profiles: Vec<AgentProfile>,
    pub agent_default_profile: Option<String>,
    /// 稳定错误键；界面据此提示，不阻止读取其余可用字段。
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditableConfigInput {
    pub settings: Settings,
    pub agent_command: Option<String>,
    pub agent_args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommandStatus {
    pub available: bool,
    pub executable: Option<String>,
    pub error: Option<String>,
}

/// 内置 Agent 预设：只描述命令名与交互启动参数，不执行任何文件。
#[derive(Clone, Debug, PartialEq)]
pub struct AgentPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub binary_name: &'static str,
    pub suggested_args: &'static [&'static str],
}

/// 设置界面按此顺序展示；参数都可被用户修改后保存。
pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "omp",
        display_name: "OMP",
        binary_name: "omp",
        suggested_args: &["--append-system-prompt={guide}"],
    },
    AgentPreset {
        id: "codex",
        display_name: "Codex",
        binary_name: "codex",
        suggested_args: &["{guide}"],
    },
    AgentPreset {
        id: "claude",
        display_name: "Claude Code",
        binary_name: "claude",
        suggested_args: &["--append-system-prompt", "{guide}"],
    },
    AgentPreset {
        id: "gemini",
        display_name: "Gemini CLI",
        binary_name: "gemini",
        suggested_args: &["-i", "{guide}"],
    },
    AgentPreset {
        id: "opencode",
        display_name: "opencode",
        binary_name: "opencode",
        suggested_args: &["--prompt", "{guide}"],
    },
    AgentPreset {
        id: "aider",
        display_name: "Aider",
        binary_name: "aider",
        suggested_args: &["--message", "{guide}"],
    },
    AgentPreset {
        id: "goose",
        display_name: "Goose",
        binary_name: "goose",
        suggested_args: &["run", "--text", "{guide}"],
    },
];

/// 本机实际存在的预设 Agent；`executable` 已规范化为绝对路径。
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredAgent {
    pub id: String,
    pub display_name: String,
    pub binary_name: String,
    pub executable: String,
    pub suggested_args: Vec<String>,
}

/// 有效指南及其来源，供设置界面审阅、覆盖和恢复。
#[derive(Clone, Debug, PartialEq)]
pub struct GuideState {
    pub text: String,
    pub overridden: bool,
    pub path: String,
    pub default_text: String,
}

/// 与 `agent_launch` 一致的结构校验；不解析可执行文件，也不读取指南。
fn validate_agent_template(command: &str, args: &[String]) -> Result<(), &'static str> {
    if command.trim().is_empty() {
        return Err("agent.command_missing");
    }
    if args.is_empty() || !args.iter().any(|argument| argument.contains("{guide}")) {
        return Err("agent.guide_missing");
    }
    if command.contains('\0') || args.iter().any(|argument| argument.contains('\0')) {
        return Err("agent.invalid_argument");
    }
    if command.contains('/') && !Path::new(command).is_absolute() {
        return Err("agent.command_not_absolute");
    }
    Ok(())
}

fn invalid_font_size(size: f64) -> bool {
    !size.is_finite() || !(9.0..=40.0).contains(&size)
}

fn normalized_terminal_font(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_color_scheme(value: &str) -> io::Result<&'static str> {
    match value.trim() {
        "" | "system" => Ok("system"),
        "light" => Ok("light"),
        "dark" => Ok("dark"),
        _ => Err(io::Error::other("config.color_scheme_invalid")),
    }
}

fn resolve_custom_agent_shell(env_shell: Option<&str>) -> String {
    #[cfg(target_os = "macos")]
    const FALLBACK_SHELL: &str = "/bin/zsh";
    #[cfg(not(target_os = "macos"))]
    const FALLBACK_SHELL: &str = "/bin/sh";

    env_shell
        .map(str::trim)
        .filter(|value| {
            let path = Path::new(value);
            path.is_absolute() && verified_executable(path).is_ok()
        })
        .map(str::to_string)
        .unwrap_or_else(|| FALLBACK_SHELL.to_string())
}

fn set_value(table: &mut toml_edit::Table, key: &str, value: toml_edit::Value) {
    let mut value = value;
    // 沿用旧值的空白与行尾注释；只替换值本身。索引赋值保留键前缀里的独立注释。
    if let Some(old) = table.get(key).and_then(toml_edit::Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    table[key] = toml_edit::Item::Value(value);
}

fn set_array(table: &mut toml_edit::Table, key: &str, values: &[String]) {
    if table
        .get(key)
        .and_then(toml_edit::Item::as_array)
        .is_some_and(|array| {
            array
                .iter()
                .map(toml_edit::Value::as_str)
                .eq(values.iter().map(|value| Some(value.as_str())))
        })
    {
        return;
    }
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value.as_str());
    }
    set_value(table, key, toml_edit::Value::Array(array));
}

fn set_profile_position(table: &mut toml_edit::Table, position: usize) {
    table.set_position(position);
    // 档内子表要跟随所属配置档输出，否则 TOML 会将其关联到前一个同名数组表。
    for (_, item) in table.iter_mut() {
        match item {
            toml_edit::Item::Table(child) => set_profile_position(child, position),
            toml_edit::Item::ArrayOfTables(children) => {
                for child in children.iter_mut() {
                    set_profile_position(child, position);
                }
            }
            _ => {}
        }
    }
}

fn update_agent_profiles(table: &mut toml_edit::Table, profiles: &[AgentProfile]) {
    fn profile_id(entry: &toml_edit::Table) -> Option<&str> {
        entry
            .get("id")
            .and_then(toml_edit::Item::as_str)
            .map(str::trim)
    }

    let original = table
        .get_mut("profiles")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .map(std::mem::take)
        .unwrap_or_default();
    let reordered = original
        .iter()
        .map(profile_id)
        .ne(profiles.iter().map(|profile| Some(profile.id.as_str())));
    let positions: Vec<_> = original
        .iter()
        .filter_map(toml_edit::Table::position)
        .collect();
    let mut by_id = BTreeMap::new();
    for entry in original {
        if let Some(id) = profile_id(&entry) {
            by_id.entry(id.to_owned()).or_insert(entry);
        }
    }

    let mut entries = toml_edit::ArrayOfTables::new();
    for (index, profile) in profiles.iter().enumerate() {
        let mut entry = by_id.remove(&profile.id).unwrap_or_default();
        for (key, value) in [
            ("id", profile.id.as_str()),
            ("name", profile.name.as_str()),
            ("command", profile.command.as_str()),
        ] {
            if entry.get(key).and_then(toml_edit::Item::as_str) != Some(value) {
                set_value(&mut entry, key, toml_edit::Value::from(value));
            }
        }
        set_array(&mut entry, "args", &profile.args);
        if reordered && let Some(&position) = positions.get(index).or_else(|| positions.last()) {
            set_profile_position(&mut entry, position);
        }
        entries.push(entry);
    }
    table["profiles"] = toml_edit::Item::ArrayOfTables(entries);
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
    #[serde(default)]
    default_profile: Option<String>,
    #[serde(default)]
    profiles: Option<Vec<RawAgentProfile>>,
}

#[derive(serde::Deserialize, Default)]
struct RawAgentProfile {
    #[serde(default)]
    id: Option<String>,
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

/// 组合搜索路径：进程 PATH → 登录 shell PATH → 常见安装目录。
///
/// 只保留非空绝对目录，按首次出现去重；相对项与 POSIX 路径列表无法表示的
/// 含冒号目录被跳过。`login_path` 由调用方显式传入，本函数不启动任何进程。
pub fn build_search_paths(
    process_path: Option<&std::ffi::OsStr>,
    login_path: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
    include_common_dirs: bool,
) -> std::ffi::OsString {
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |directory: PathBuf| {
        if directory.as_os_str().is_empty() || !directory.is_absolute() {
            return;
        }
        if !path_entry_is_listable(&directory) {
            return;
        }
        if seen.insert(directory.clone()) {
            directories.push(directory);
        }
    };
    for source in [process_path, login_path] {
        for directory in std::env::split_paths(source.unwrap_or_default()) {
            push(directory);
        }
    }
    if include_common_dirs {
        for directory in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
            push(PathBuf::from(directory));
        }
        if let Some(home) = home {
            for relative in [
                ".local/bin",
                ".cargo/bin",
                ".npm-global/bin",
                ".bun/bin",
                ".opencode/bin",
                ".volta/bin",
            ] {
                push(home.join(relative));
            }
        }
    }
    std::env::join_paths(directories).unwrap_or_default()
}

/// POSIX 路径列表用冒号分隔；含冒号的目录无法原样表达，宁可跳过。
fn path_entry_is_listable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        !path.as_os_str().as_bytes().contains(&b':')
    }
    #[cfg(not(unix))]
    {
        !path.to_string_lossy().contains(':')
    }
}

/// 进程 PATH + 登录 shell PATH + 常见安装目录；`None` 时不启动外部 shell。
fn default_search_paths(login_path: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
    let home = std::env::home_dir();
    build_search_paths(
        std::env::var_os("PATH").as_deref(),
        login_path,
        home.as_deref(),
        true,
    )
}

/// 裸命令名按组合搜索路径逐项查找；返回可直接 execve 的绝对路径。
/// `search` 为空时退回进程 PATH + 常见安装目录，不启动外部 shell。
fn resolve_executable_with_paths(
    command: &str,
    search: Option<&std::ffi::OsStr>,
) -> io::Result<String> {
    match search.filter(|search| !search.is_empty()) {
        Some(search) => resolve_executable_in(command, Some(search)),
        None => {
            let search = default_search_paths(None);
            resolve_executable_in(command, Some(&search))
        }
    }
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
        let mut settings: Settings = read_toml(&self.config_path())?;
        settings.terminal_font = normalized_terminal_font(settings.terminal_font.as_deref());
        if invalid_font_size(settings.font_size) {
            return Err(io::Error::other("config.font_size_invalid"));
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        if invalid_font_size(settings.font_size) {
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

    /// 读取顶层可选 `color_scheme`（`"system"` / `"light"` / `"dark"`，缺省或空白视为 `"system"`）。
    pub fn color_scheme(&self) -> io::Result<String> {
        let raw = self.read_color_scheme()?.unwrap_or_default();
        normalize_color_scheme(&raw).map(str::to_owned)
    }

    /// 缺少文件或顶层键时返回 `None`，显式有效时返回规范值。
    /// 显式空白、非字符串和未知值返回 `config.color_scheme_invalid`，迁移时只应对 `None` 写入旧值。
    pub fn explicit_color_scheme(&self) -> io::Result<Option<String>> {
        match self.read_color_scheme() {
            Ok(None) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Ok(Some(raw)) if !raw.trim().is_empty() => {
                normalize_color_scheme(&raw).map(|value| Some(value.to_owned()))
            }
            Ok(Some(_)) => Err(io::Error::other("config.color_scheme_invalid")),
            Err(error) => Err(error),
        }
    }

    fn read_color_scheme(&self) -> io::Result<Option<String>> {
        let bytes = fs::read(self.config_path())?;
        let text = String::from_utf8(bytes).map_err(|_| io::Error::other("config.invalid_utf8"))?;
        let document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| io::Error::other("config.invalid_toml"))?;
        document
            .get("color_scheme")
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| io::Error::other("config.color_scheme_invalid"))
            })
            .transpose()
    }

    /// 原地更新顶层 `color_scheme`，完整保留用户注释、未知键与既有配置段。
    pub fn save_color_scheme(&self, scheme: &str) -> io::Result<()> {
        let normalized = normalize_color_scheme(scheme)?;
        let bytes = fs::read(self.config_path())?;
        let text = String::from_utf8(bytes).map_err(|_| io::Error::other("config.invalid_utf8"))?;
        let mut document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| io::Error::other("config.invalid_toml"))?;
        set_value(
            &mut document,
            "color_scheme",
            toml_edit::Value::from(normalized),
        );
        atomic_write(&self.config_path(), document.to_string().as_bytes())
    }

    /// 读取设置界面的完整快照；语法损坏以外的字段问题返回诊断而不是整体失败。
    pub fn editable_config(&self) -> io::Result<EditableConfig> {
        let path = self.config_path();
        let bytes = fs::read(&path)?;
        let digest = sha256::digest(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| io::Error::other("config.invalid_utf8"))?;
        let document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| io::Error::other("config.invalid_toml"))?;
        let mut diagnostics: Vec<String> = Vec::new();
        let font_size = match document
            .get("font_size")
            .and_then(toml_edit::Item::as_value)
            .and_then(|value| {
                value
                    .as_float()
                    .or_else(|| value.as_integer().map(|integer| integer as f64))
            }) {
            Some(size) if size.is_finite() => {
                if invalid_font_size(size) {
                    diagnostics.push("config.font_size_invalid".into());
                }
                size
            }
            Some(_) => {
                diagnostics.push("config.font_size_invalid".into());
                15.0
            }
            None => {
                diagnostics.push("config.font_size_invalid".into());
                15.0
            }
        };
        let default_view = match document
            .get("default_view")
            .and_then(toml_edit::Item::as_str)
        {
            Some("reading") | None => ViewMode::Reading,
            Some("editing") => ViewMode::Editing,
            Some(_) => {
                diagnostics.push("config.default_view_invalid".into());
                ViewMode::Reading
            }
        };
        let terminal_font = match document.get("terminal_font") {
            None => None,
            Some(item) => match item.as_str() {
                Some(value) => normalized_terminal_font(Some(value)),
                None => {
                    diagnostics.push("config.terminal_font_invalid".into());
                    None
                }
            },
        };
        let mut agent_configured = false;
        let mut agent_command = None;
        let mut agent_args = Vec::new();
        let mut agent_profiles = Vec::new();
        let mut agent_default_profile = None;
        if let Some(item) = document.get("agent") {
            match item.as_table() {
                Some(table) => {
                    if let Some(command) = table.get("command") {
                        match command.as_str() {
                            Some(value) => agent_command = Some(value.to_string()),
                            None => diagnostics.push("agent.config_invalid".into()),
                        }
                    }
                    if let Some(args) = table.get("args") {
                        match args.as_array() {
                            Some(values) => {
                                for value in values.iter() {
                                    match value.as_str() {
                                        Some(argument) => agent_args.push(argument.to_string()),
                                        None => diagnostics.push("agent.config_invalid".into()),
                                    }
                                }
                            }
                            None => diagnostics.push("agent.config_invalid".into()),
                        }
                    }
                    if let Some(default_item) = table.get("default_profile") {
                        match default_item.as_str() {
                            Some(value) if !value.trim().is_empty() => {
                                agent_default_profile = Some(value.trim().to_string());
                            }
                            _ => diagnostics.push("agent.config_invalid".into()),
                        }
                    }
                    if let Some(profiles_item) = table.get("profiles") {
                        match profiles_item.as_array_of_tables() {
                            Some(tables) => {
                                let mut seen_ids = BTreeSet::new();
                                for profile_table in tables.iter() {
                                    let id = profile_table
                                        .get("id")
                                        .and_then(toml_edit::Item::as_str)
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                    let name = profile_table
                                        .get("name")
                                        .and_then(toml_edit::Item::as_str)
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                    let command = profile_table
                                        .get("command")
                                        .and_then(toml_edit::Item::as_str)
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                    let mut args = Vec::new();
                                    if let Some(args_item) = profile_table.get("args") {
                                        match args_item.as_array() {
                                            Some(values) => {
                                                for value in values.iter() {
                                                    match value.as_str() {
                                                        Some(arg) => args.push(arg.to_string()),
                                                        None => diagnostics
                                                            .push("agent.config_invalid".into()),
                                                    }
                                                }
                                            }
                                            None => diagnostics.push("agent.config_invalid".into()),
                                        }
                                    }
                                    if id.is_empty()
                                        || name.is_empty()
                                        || !seen_ids.insert(id.clone())
                                    {
                                        diagnostics.push("agent.profile_invalid".into());
                                    }
                                    if let Err(key) = validate_agent_template(&command, &args) {
                                        diagnostics.push(key.into());
                                    }
                                    agent_profiles.push(AgentProfile {
                                        id,
                                        name,
                                        command,
                                        args,
                                    });
                                }
                            }
                            None if profiles_item
                                .as_array()
                                .is_some_and(toml_edit::Array::is_empty) => {}
                            None => diagnostics.push("agent.config_invalid".into()),
                        }
                    }
                    if !agent_profiles.is_empty() {
                        agent_configured = true;
                        let active = agent_default_profile
                            .as_deref()
                            .and_then(|id| agent_profiles.iter().find(|p| p.id == id))
                            .or_else(|| agent_profiles.first());
                        if let Some(active) = active {
                            if agent_command.is_none() && agent_args.is_empty() {
                                agent_command = Some(active.command.clone());
                                agent_args = active.args.clone();
                            }
                            if agent_default_profile.is_none() {
                                agent_default_profile = Some(active.id.clone());
                            }
                        }
                    } else {
                        // 只有 command/args 组成启动模板；其它 [agent] 子键不算已配置。
                        agent_configured = agent_command.is_some() || !agent_args.is_empty();
                        if agent_configured
                            && let Err(key) = validate_agent_template(
                                agent_command.as_deref().unwrap_or_default(),
                                &agent_args,
                            )
                        {
                            diagnostics.push(key.into());
                        }
                    }
                }
                None => diagnostics.push("agent.config_invalid".into()),
            }
        }
        let mut seen = BTreeSet::new();
        diagnostics.retain(|key| seen.insert(key.clone()));
        Ok(EditableConfig {
            config_path: path.to_string_lossy().into_owned(),
            digest,
            settings: Settings {
                font_size,
                default_view,
                terminal_font,
            },
            agent_configured,
            agent_command,
            agent_args,
            agent_profiles,
            agent_default_profile,
            diagnostics,
        })
    }

    /// 原地保存已知键；外部手改与打开时的指纹不一致时拒绝覆盖。
    pub fn save_config(
        &self,
        input: &EditableConfigInput,
        expected_digest: Option<&str>,
    ) -> io::Result<EditableConfig> {
        self.save_config_with_profiles(input, &[], None, expected_digest)
    }

    /// 原地保存基础设置与 `[[agent.profiles]]` 多配置档，完整保留注释、未知键与 `[[vault]]`。
    pub fn save_config_with_profiles(
        &self,
        input: &EditableConfigInput,
        profiles: &[AgentProfile],
        default_profile: Option<&str>,
        expected_digest: Option<&str>,
    ) -> io::Result<EditableConfig> {
        let path = self.config_path();
        let bytes = fs::read(&path)?;
        if let Some(expected) = expected_digest
            && expected != sha256::digest(&bytes)
        {
            return Err(io::Error::other("config.externally_modified"));
        }
        if invalid_font_size(input.settings.font_size) {
            return Err(io::Error::other("config.font_size_invalid"));
        }
        let terminal_font = normalized_terminal_font(input.settings.terminal_font.as_deref());
        if terminal_font
            .as_deref()
            .is_some_and(|font| font.contains('\0'))
        {
            return Err(io::Error::other("config.terminal_font_invalid"));
        }

        let mut normalized_profiles = Vec::with_capacity(profiles.len());
        let mut seen_ids = BTreeSet::new();
        for profile in profiles {
            let id = profile.id.trim().to_string();
            let name = profile.name.trim().to_string();
            let command = profile.command.trim().to_string();
            if id.is_empty()
                || name.is_empty()
                || id.contains('\0')
                || name.contains('\0')
                || !seen_ids.insert(id.clone())
            {
                return Err(io::Error::other("agent.profile_invalid"));
            }
            validate_agent_template(&command, &profile.args).map_err(io::Error::other)?;
            normalized_profiles.push(AgentProfile {
                id,
                name,
                command,
                args: profile.args.clone(),
            });
        }

        let command = input
            .agent_command
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        let args = &input.agent_args;
        let legacy_cleared =
            command.is_empty() && args.iter().all(|argument| argument.trim().is_empty());
        if normalized_profiles.is_empty() && !legacy_cleared {
            validate_agent_template(&command, args).map_err(io::Error::other)?;
        }

        let text = String::from_utf8(bytes).map_err(|_| io::Error::other("config.invalid_utf8"))?;
        let mut document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| io::Error::other("config.invalid_toml"))?;
        set_value(
            &mut document,
            "font_size",
            toml_edit::Value::from(input.settings.font_size),
        );
        set_value(
            &mut document,
            "default_view",
            toml_edit::Value::from(match input.settings.default_view {
                ViewMode::Reading => "reading",
                ViewMode::Editing => "editing",
            }),
        );
        match terminal_font {
            Some(font) => set_value(&mut document, "terminal_font", toml_edit::Value::from(font)),
            None => {
                document.remove("terminal_font");
            }
        };

        if !normalized_profiles.is_empty() {
            if document.get("agent").is_none() {
                document.insert("agent", toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let table = document
                .get_mut("agent")
                .and_then(toml_edit::Item::as_table_mut)
                .ok_or_else(|| io::Error::other("agent.config_invalid"))?;
            let active = default_profile
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .and_then(|id| normalized_profiles.iter().find(|p| p.id == id))
                .unwrap_or(&normalized_profiles[0]);
            set_value(
                table,
                "default_profile",
                toml_edit::Value::from(active.id.as_str()),
            );
            // 同步更新 `[agent]` 的 command/args 到默认配置档，保持旧客户端向后兼容。
            set_value(
                table,
                "command",
                toml_edit::Value::from(active.command.as_str()),
            );
            set_array(table, "args", &active.args);
            update_agent_profiles(table, &normalized_profiles);
        } else if legacy_cleared {
            if let Some(item) = document.get_mut("agent") {
                let table = item
                    .as_table_mut()
                    .ok_or_else(|| io::Error::other("agent.config_invalid"))?;
                table.remove("command");
                table.remove("args");
                table.remove("default_profile");
                table.remove("profiles");
            }
            let empty = document
                .get("agent")
                .and_then(toml_edit::Item::as_table)
                .is_some_and(toml_edit::Table::is_empty);
            if empty {
                document.remove("agent");
            }
        } else {
            if document.get("agent").is_none() {
                document.insert("agent", toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let table = document
                .get_mut("agent")
                .and_then(toml_edit::Item::as_table_mut)
                .ok_or_else(|| io::Error::other("agent.config_invalid"))?;
            table.remove("default_profile");
            table.remove("profiles");
            set_value(table, "command", toml_edit::Value::from(command));
            set_array(table, "args", args);
        }
        atomic_write(&path, document.to_string().as_bytes())?;
        self.editable_config()
    }

    /// 只检查模板结构与本机可执行文件解析；不启动进程、不读指南、不创建目录。
    pub fn probe_agent_command(&self, command: &str, args: &[String]) -> AgentCommandStatus {
        self.probe_agent_command_with_paths(command, args, None)
    }

    /// 探测时追加后台登录 shell 得到的 PATH；仍只做静态解析。
    pub fn probe_agent_command_with_paths(
        &self,
        command: &str,
        args: &[String],
        login_path: Option<&std::ffi::OsStr>,
    ) -> AgentCommandStatus {
        let command = command.trim();
        if let Err(key) = validate_agent_template(command, args) {
            return AgentCommandStatus {
                available: false,
                executable: None,
                error: Some(key.into()),
            };
        }
        let search = default_search_paths(login_path);
        match resolve_executable_with_paths(command, Some(&search)) {
            Ok(executable) => AgentCommandStatus {
                available: true,
                executable: Some(executable),
                error: None,
            },
            Err(error) => AgentCommandStatus {
                available: false,
                executable: None,
                error: Some(error.to_string()),
            },
        }
    }

    /// 按预设顺序只读扫描默认组合路径；不启动任何目标二进制。
    pub fn discover_agents(&self) -> Vec<DiscoveredAgent> {
        let search = default_search_paths(None);
        self.discover_agents_in(Some(&search))
    }

    /// 显式搜索路径版本；单测与检查可完全隔离宿主机目录。
    pub fn discover_agents_in(&self, search: Option<&std::ffi::OsStr>) -> Vec<DiscoveredAgent> {
        let mut discovered: Vec<DiscoveredAgent> = Vec::new();
        for preset in AGENT_PRESETS {
            let Ok(executable) = resolve_executable_with_paths(preset.binary_name, search) else {
                continue;
            };
            // 同一真实文件只给第一个预设，避免符号链接造成重复行。
            if discovered
                .iter()
                .any(|agent| agent.executable == executable)
            {
                continue;
            }
            discovered.push(DiscoveredAgent {
                id: preset.id.into(),
                display_name: preset.display_name.into(),
                binary_name: preset.binary_name.into(),
                executable,
                suggested_args: preset
                    .suggested_args
                    .iter()
                    .map(|argument| (*argument).to_string())
                    .collect(),
            });
        }
        discovered
    }

    /// 子进程 PATH：发现、探测与启动共用同一份组合路径。
    pub fn effective_agent_path(&self, login_path: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
        default_search_paths(login_path)
    }

    pub fn guide_state(&self) -> io::Result<GuideState> {
        let path = self.directories.config.join("agent-guide.md");
        match fs::read(&path) {
            Ok(bytes) => {
                let text =
                    String::from_utf8(bytes).map_err(|_| io::Error::other("guide.invalid_utf8"))?;
                if text.trim().is_empty() {
                    return Err(io::Error::other("guide.empty"));
                }
                Ok(GuideState {
                    text,
                    overridden: true,
                    path: path.to_string_lossy().into_owned(),
                    default_text: AGENT_GUIDE.to_string(),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(GuideState {
                text: AGENT_GUIDE.to_string(),
                overridden: false,
                path: path.to_string_lossy().into_owned(),
                default_text: AGENT_GUIDE.to_string(),
            }),
            Err(_) => Err(io::Error::other("guide.read_failed")),
        }
    }

    pub fn save_agent_guide(&self, content: &str) -> io::Result<GuideState> {
        if content.trim().is_empty() {
            return Err(io::Error::other("guide.empty"));
        }
        fs::create_dir_all(&self.directories.config)?;
        atomic_write(
            &self.directories.config.join("agent-guide.md"),
            content.as_bytes(),
        )?;
        self.guide_state()
    }

    /// 只在覆盖文件存在时删除；已经不存在视同成功。
    pub fn reset_agent_guide(&self) -> io::Result<GuideState> {
        let path = self.directories.config.join("agent-guide.md");
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.is_dir() {
                    return Err(io::Error::other("guide.write_failed"));
                }
                fs::remove_file(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.guide_state()
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
                                layout: None,
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

    /// 折叠状态与专注视图状态写同一条目；调用方只提交自己负责的字段。
    pub fn remember_diagram(&self, state: DiagramState) -> io::Result<()> {
        let mut session = self.session()?;
        let existing = session
            .diagrams
            .iter()
            .find(|s| s.vault == state.vault && s.note == state.note && s.block == state.block);
        let mut next = state;
        if next.focus_scale.is_none()
            && next.focus_offset_x.is_none()
            && next.focus_offset_y.is_none()
            && let Some(old) = existing
        {
            next.focus_scale = old.focus_scale;
            next.focus_offset_x = old.focus_offset_x;
            next.focus_offset_y = old.focus_offset_y;
        }
        session
            .diagrams
            .retain(|s| !(s.vault == next.vault && s.note == next.note && s.block == next.block));
        let has_focus = next.focus_scale.is_some()
            || next.focus_offset_x.is_some()
            || next.focus_offset_y.is_some();
        if !next.collapsed.is_empty() || has_focus {
            session.diagrams.push(next);
        }
        write_toml(&self.directories.cache.join("session.toml"), &session)
    }

    /// 重命名笔记或文件夹后迁移图缓存：精确路径与文件夹前缀一起搬到新路径。
    /// 目标图块已有记录时合并折叠节点，并优先采用迁移条目的专注视口。
    pub fn rename_diagram_state(
        &self,
        vault: &str,
        old_path: &str,
        new_path: &str,
        directory: bool,
    ) -> io::Result<()> {
        if old_path == new_path {
            return Ok(());
        }
        let mut session = self.session()?;
        let original = session.diagrams.clone();
        let prefix = directory.then(|| format!("{old_path}/"));
        let remapped = original
            .iter()
            .map(|state| {
                let mut next = state.clone();
                if state.vault == vault {
                    if state.note == old_path {
                        next.note = new_path.to_owned();
                    } else if let Some(prefix) = &prefix
                        && let Some(suffix) = state.note.strip_prefix(prefix)
                    {
                        next.note = format!("{new_path}/{suffix}");
                    }
                }
                next
            })
            .collect::<Vec<_>>();
        let mut merged: Vec<DiagramState> = Vec::with_capacity(remapped.len());
        for state in remapped {
            if let Some(existing) = merged
                .iter_mut()
                .find(|s| s.vault == state.vault && s.note == state.note && s.block == state.block)
            {
                existing.collapsed.extend(state.collapsed);
                existing.collapsed.sort();
                existing.collapsed.dedup();
                if state.focus_scale.is_some() {
                    existing.focus_scale = state.focus_scale;
                    existing.focus_offset_x = state.focus_offset_x;
                    existing.focus_offset_y = state.focus_offset_y;
                }
            } else {
                merged.push(state);
            }
        }
        if merged == original {
            return Ok(());
        }
        session.diagrams = merged;
        write_toml(&self.directories.cache.join("session.toml"), &session)
    }

    /// 笔记或文件夹进入废纸篓后清理图缓存：同时删除文件夹前缀下的全部条目。
    pub fn remove_diagram_state(&self, vault: &str, path: &str, directory: bool) -> io::Result<()> {
        let mut session = self.session()?;
        let prefix = directory.then(|| format!("{path}/"));
        let before = session.diagrams.len();
        session.diagrams.retain(|state| {
            if state.vault != vault {
                return true;
            }
            state.note != path
                && !prefix
                    .as_ref()
                    .is_some_and(|prefix| state.note.starts_with(prefix))
        });
        if session.diagrams.len() == before {
            return Ok(());
        }
        write_toml(&self.directories.cache.join("session.toml"), &session)
    }

    /// 只更新专注模式的缩放与平移，保留同一图块的折叠状态。
    pub fn remember_diagram_focus(
        &self,
        vault: String,
        note: String,
        block: u32,
        scale: Option<f64>,
        offset_x: Option<f64>,
        offset_y: Option<f64>,
    ) -> io::Result<()> {
        let mut session = self.session()?;
        let existing = session
            .diagrams
            .iter()
            .find(|s| s.vault == vault && s.note == note && s.block == block);
        let collapsed = existing.map(|s| s.collapsed.clone()).unwrap_or_default();
        session
            .diagrams
            .retain(|s| !(s.vault == vault && s.note == note && s.block == block));
        if !collapsed.is_empty() || scale.is_some() {
            session.diagrams.push(DiagramState {
                vault,
                note,
                block,
                collapsed,
                focus_scale: scale,
                focus_offset_x: offset_x,
                focus_offset_y: offset_y,
            });
        }
        write_toml(&self.directories.cache.join("session.toml"), &session)
    }

    /// 只读取并校验模板，不创建目录、不启动进程。
    pub fn agent_launch(&self) -> io::Result<AgentLaunch> {
        self.agent_launch_for_profile_with_paths(None, None)
    }

    /// 启动解析追加后台登录 shell 得到的 PATH；仍不执行模板里的命令。
    pub fn agent_launch_with_paths(
        &self,
        login_path: Option<&std::ffi::OsStr>,
    ) -> io::Result<AgentLaunch> {
        self.agent_launch_for_profile_with_paths(None, login_path)
    }

    /// 按配置档解析 Agent 启动规格：`profile_id` 为 `None` 时优先使用默认配置档，
    /// 无 `[[agent.profiles]]` 时无缝回退至 `[agent]`。
    pub fn agent_launch_for_profile_with_paths(
        &self,
        profile_id: Option<&str>,
        login_path: Option<&std::ffi::OsStr>,
    ) -> io::Result<AgentLaunch> {
        let text = fs::read_to_string(self.config_path())?;
        let raw: RawConfig =
            toml_edit::de::from_str(&text).map_err(|_| io::Error::other("agent.config_invalid"))?;
        let agent = raw
            .agent
            .ok_or_else(|| io::Error::other("agent.not_configured"))?;
        let (command, args) = if let Some(profiles) = agent.profiles.filter(|p| !p.is_empty()) {
            let selected = if let Some(requested_id) = profile_id {
                profiles
                    .into_iter()
                    .find(|p| p.id.as_deref().map(str::trim) == Some(requested_id))
                    .ok_or_else(|| io::Error::other("agent.profile_not_found"))?
            } else if let Some(default_id) = agent
                .default_profile
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                let mut iter = profiles.into_iter();
                let first = iter.next().unwrap();
                if first.id.as_deref().map(str::trim) == Some(default_id) {
                    first
                } else {
                    iter.find(|p| p.id.as_deref().map(str::trim) == Some(default_id))
                        .unwrap_or(first)
                }
            } else {
                profiles.into_iter().next().unwrap()
            };
            (
                selected.command.unwrap_or_default(),
                selected.args.unwrap_or_default(),
            )
        } else {
            if profile_id.is_some() {
                return Err(io::Error::other("agent.profile_not_found"));
            }
            (
                agent.command.unwrap_or_default(),
                agent.args.unwrap_or_default(),
            )
        };
        if command.trim().is_empty() {
            return Err(io::Error::other("agent.command_missing"));
        }
        if args.is_empty() || !args.iter().any(|argument| argument.contains("{guide}")) {
            return Err(io::Error::other("agent.guide_missing"));
        }
        if command.contains('\0') || args.iter().any(|argument| argument.contains('\0')) {
            return Err(io::Error::other("agent.invalid_argument"));
        }
        let guide = effective_guide(&self.directories.config)?;
        let search = default_search_paths(login_path);
        let executable = resolve_executable_with_paths(&command, Some(&search))?;
        // 每个元素仍是一个 argv 元素；替换指南不会拆分或再解释内容。
        let args = args
            .iter()
            .map(|argument| argument.replace("{guide}", &guide))
            .collect();
        Ok(AgentLaunch { executable, args })
    }

    /// 准备自定义命令启动 Agent：将当前有效指南写入缓存目录只读文件，
    /// 返回 `$SHELL -lc <command>` 启动规格与只读指南路径，绝不将指南拼入命令行。
    pub fn prepare_custom_agent_launch(
        &self,
        command: &str,
        login_path: Option<&std::ffi::OsStr>,
    ) -> io::Result<CustomAgentLaunch> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err(io::Error::other("agent.command_missing"));
        }
        if trimmed.contains('\0') {
            return Err(io::Error::other("agent.invalid_argument"));
        }
        let guide = effective_guide(&self.directories.config)?;
        let guide_directory = self.directories.cache.join("agent-guides");
        fs::create_dir_all(&guide_directory)?;
        let guide_path = write_launch_guide(&guide_directory, guide.as_bytes())?;
        prune_launch_guides(&guide_directory);
        let shell = resolve_custom_agent_shell(std::env::var("SHELL").ok().as_deref());
        let path = self
            .effective_agent_path(login_path)
            .to_string_lossy()
            .into_owned();
        Ok(CustomAgentLaunch {
            executable: shell,
            args: vec!["-lc".to_string(), trimmed.to_string()],
            guide_file: guide_path.to_string_lossy().into_owned(),
            path,
        })
    }

    /// 读取系统缓存目录中的最近自定义 Agent 命令历史（最多 12 条）。
    pub fn custom_agent_history(&self) -> io::Result<Vec<String>> {
        let path = self.directories.cache.join("agent-custom-history.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let items: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
                let mut deduped = Vec::with_capacity(items.len().min(CUSTOM_AGENT_HISTORY_LIMIT));
                for item in items {
                    let trimmed = item.trim();
                    if trimmed.is_empty() || trimmed.contains('\0') {
                        continue;
                    }
                    if !deduped.iter().any(|existing| existing == trimmed) {
                        deduped.push(trimmed.to_string());
                    }
                    if deduped.len() >= CUSTOM_AGENT_HISTORY_LIMIT {
                        break;
                    }
                }
                Ok(deduped)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    /// 记录一条自定义 Agent 命令至系统缓存目录（最近优先去重，上限 12 条）。
    pub fn record_custom_agent_command(&self, command: &str) -> io::Result<Vec<String>> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err(io::Error::other("agent.command_missing"));
        }
        if trimmed.contains('\0') {
            return Err(io::Error::other("agent.invalid_argument"));
        }
        fs::create_dir_all(&self.directories.cache)?;
        let mut history = self.custom_agent_history().unwrap_or_default();
        history.retain(|item| item != trimmed);
        history.insert(0, trimmed.to_string());
        history.truncate(CUSTOM_AGENT_HISTORY_LIMIT);
        let bytes = serde_json::to_vec_pretty(&history).map_err(io::Error::other)?;
        atomic_write(
            &self.directories.cache.join("agent-custom-history.json"),
            &bytes,
        )?;
        Ok(history)
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

const RETAINED_LAUNCH_GUIDES: usize = 32;
const RECENT_LAUNCH_GUIDE_AGE: Duration = Duration::from_secs(5 * 60);

fn write_launch_guide(directory: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    static NEXT_GUIDE: AtomicU64 = AtomicU64::new(0);
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o444))?;
    }
    #[cfg(not(unix))]
    {
        let mut perms = temporary.as_file().metadata()?.permissions();
        perms.set_readonly(true);
        temporary.as_file().set_permissions(perms)?;
    }
    temporary.as_file().sync_all()?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let sequence = NEXT_GUIDE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("guide-{nanos}-{pid}-{sequence}.md"));
    temporary
        .persist_noclobber(&path)
        .map_err(io::Error::other)?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(path)
}

fn prune_launch_guides(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut guides: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.starts_with("guide-") || !name.ends_with(".md") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || !metadata.permissions().readonly() {
                return None;
            }
            Some((metadata.modified().ok()?, entry.path()))
        })
        .collect();
    guides.sort_unstable_by(|a, b| b.cmp(a));
    let now = SystemTime::now();
    for (modified, path) in guides.into_iter().skip(RETAINED_LAUNCH_GUIDES) {
        // 并发准备或稍后才启动的进程仍可能需要最近发布的指南。
        if now
            .duration_since(modified)
            .is_ok_and(|age| age >= RECENT_LAUNCH_GUIDE_AGE)
        {
            #[cfg(not(unix))]
            if let Ok(metadata) = fs::symlink_metadata(&path)
                && metadata.is_file()
            {
                let mut permissions = metadata.permissions();
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&path, permissions);
            }
            let _ = fs::remove_file(path);
        }
    }
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

    fn entries(paths: &std::ffi::OsStr) -> Vec<PathBuf> {
        std::env::split_paths(paths).collect()
    }

    #[test]
    fn search_paths_merge_process_login_and_common_directories_in_order() {
        let process = std::ffi::OsString::from("/process/bin:/shared/bin:relative:.:");
        let login = std::ffi::OsString::from("/login/bin:/shared/bin:/opt/homebrew/bin");
        let merged = build_search_paths(
            Some(&process),
            Some(&login),
            Some(Path::new("/Users/tester")),
            true,
        );
        assert_eq!(
            entries(&merged),
            vec![
                PathBuf::from("/process/bin"),
                PathBuf::from("/shared/bin"),
                PathBuf::from("/login/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/opt/homebrew/sbin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/Users/tester/.local/bin"),
                PathBuf::from("/Users/tester/.cargo/bin"),
                PathBuf::from("/Users/tester/.npm-global/bin"),
                PathBuf::from("/Users/tester/.bun/bin"),
                PathBuf::from("/Users/tester/.opencode/bin"),
                PathBuf::from("/Users/tester/.volta/bin"),
            ]
        );

        // 关闭常见目录后只保留显式来源；相对项始终跳过。
        let merged = build_search_paths(Some(&process), None, None, false);
        assert_eq!(
            entries(&merged),
            vec![PathBuf::from("/process/bin"), PathBuf::from("/shared/bin")]
        );
        assert!(build_search_paths(None, None, None, false).is_empty());
    }

    #[test]
    fn combined_paths_resolve_only_inside_the_injected_search() {
        let root = tempfile::TempDir::new().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let tool = bin.join("hortus-test-agent");
        make_executable(&tool);
        let search = build_search_paths(None, Some(bin.as_os_str()), None, false);
        assert_eq!(
            resolve_executable_with_paths("hortus-test-agent", Some(&search)).unwrap(),
            resolved(&tool)
        );
        fs::remove_file(&tool).unwrap();
        assert_eq!(
            resolve_executable_with_paths("hortus-test-agent", Some(&search))
                .unwrap_err()
                .to_string(),
            "agent.not_found"
        );
    }

    #[test]
    fn discovery_lists_only_executable_presets_without_running_them() {
        let root = tempfile::TempDir::new().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let marker = root.path().join("trap-marker");
        for preset in AGENT_PRESETS {
            let path = bin.join(preset.binary_name);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s' ran > '{}'\n", marker.display()),
            )
            .unwrap();
            #[cfg(unix)]
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if preset.id == "aider" { 0o644 } else { 0o755 }),
            )
            .unwrap();
        }
        // `codex` 指向同一真实文件时只保留先出现的预设。
        fs::remove_file(bin.join("codex")).unwrap();
        std::os::unix::fs::symlink(bin.join("omp"), bin.join("codex")).unwrap();
        // 无效的相对目录不参与解析。
        let search = std::ffi::OsString::from(format!("relative-dir:{}", bin.display()));
        let store = Store::open(Directories {
            config: root.path().join("config"),
            cache: root.path().join("cache"),
            data: root.path().join("data"),
        })
        .unwrap();
        let agents = store.discover_agents_in(Some(&search));
        assert_eq!(
            agents
                .iter()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            vec!["omp", "claude", "gemini", "opencode", "goose"]
        );
        assert!(agents.iter().all(|agent| {
            agent
                .suggested_args
                .iter()
                .any(|arg| arg.contains("{guide}"))
        }));
        assert_eq!(
            agents[0].executable,
            resolved(&bin.join("omp")),
            "executable must be the canonical target"
        );
        assert!(!marker.exists(), "discovery must never execute a preset");
    }

    fn diagram_store(root: &tempfile::TempDir) -> Store {
        Store::open(Directories {
            config: root.path().join("config"),
            cache: root.path().join("cache"),
            data: root.path().join("data"),
        })
        .unwrap()
    }

    fn diagram_entry(store: &Store, vault: &str, note: &str, block: u32) -> Option<DiagramState> {
        store
            .session()
            .unwrap()
            .diagrams
            .into_iter()
            .find(|s| s.vault == vault && s.note == note && s.block == block)
    }

    #[test]
    fn legacy_session_without_focus_fields_still_loads() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        fs::write(
            store.directories.cache.join("session.toml"),
            "[[diagrams]]\nvault = \"/v\"\nnote = \"n.md\"\nblock = 1\ncollapsed = [\"a\"]\n",
        )
        .unwrap();
        let entry = diagram_entry(&store, "/v", "n.md", 1).unwrap();
        assert_eq!(entry.collapsed, vec!["a"]);
        assert_eq!(entry.focus_scale, None);
        assert_eq!(entry.focus_offset_x, None);
        assert_eq!(entry.focus_offset_y, None);
    }

    #[test]
    fn collapsed_and_focus_fields_do_not_erase_each_other() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "n.md".into(),
                block: 1,
                collapsed: vec!["root".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            diagram_entry(&store, "/v", "n.md", 1).unwrap().collapsed,
            vec!["root"]
        );

        store
            .remember_diagram_focus(
                "/v".into(),
                "n.md".into(),
                1,
                Some(2.5),
                Some(-40.0),
                Some(12.0),
            )
            .unwrap();
        let entry = diagram_entry(&store, "/v", "n.md", 1).unwrap();
        assert_eq!(entry.collapsed, vec!["root"], "focus write keeps collapse");
        assert_eq!(entry.focus_scale, Some(2.5));
        assert_eq!(entry.focus_offset_x, Some(-40.0));
        assert_eq!(entry.focus_offset_y, Some(12.0));

        // 展开全部节点后只更新折叠字段，专注视图位置仍然保留。
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "n.md".into(),
                block: 1,
                collapsed: vec![],
                ..Default::default()
            })
            .unwrap();
        let entry = diagram_entry(&store, "/v", "n.md", 1).expect("focus keeps the entry");
        assert!(entry.collapsed.is_empty());
        assert_eq!(entry.focus_scale, Some(2.5));
    }

    #[test]
    fn diagram_state_migrates_with_note_and_folder_renames() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        for (note, block, collapsed) in [
            ("note.md", 1, vec!["a".to_owned()]),
            ("docs/inside.md", 2, vec!["b".to_owned()]),
            ("docs/deep/leaf.md", 3, vec!["c".to_owned()]),
            ("other.md", 1, vec!["d".to_owned()]),
        ] {
            store
                .remember_diagram(DiagramState {
                    vault: "/v".into(),
                    note: note.into(),
                    block,
                    collapsed,
                    ..Default::default()
                })
                .unwrap();
        }
        store
            .remember_diagram_focus(
                "/v".into(),
                "docs/inside.md".into(),
                2,
                Some(2.0),
                Some(10.0),
                Some(-4.0),
            )
            .unwrap();

        store
            .rename_diagram_state("/v", "note.md", "renamed.md", false)
            .unwrap();
        assert!(diagram_entry(&store, "/v", "note.md", 1).is_none());
        assert_eq!(
            diagram_entry(&store, "/v", "renamed.md", 1)
                .unwrap()
                .collapsed,
            vec!["a"]
        );

        store
            .rename_diagram_state("/v", "docs", "papers", true)
            .unwrap();
        let inside = diagram_entry(&store, "/v", "papers/inside.md", 2).unwrap();
        assert_eq!(inside.collapsed, vec!["b"]);
        assert_eq!(inside.focus_scale, Some(2.0));
        assert_eq!(
            diagram_entry(&store, "/v", "papers/deep/leaf.md", 3)
                .unwrap()
                .collapsed,
            vec!["c"]
        );
        assert!(diagram_entry(&store, "/v", "docs/inside.md", 2).is_none());
        // 其他笔记不被文件夹迁移影响。
        assert!(diagram_entry(&store, "/v", "other.md", 1).is_some());

        store.remove_diagram_state("/v", "papers", true).unwrap();
        assert!(diagram_entry(&store, "/v", "papers/inside.md", 2).is_none());
        assert!(diagram_entry(&store, "/v", "papers/deep/leaf.md", 3).is_none());
        assert!(diagram_entry(&store, "/v", "renamed.md", 1).is_some());
        assert!(diagram_entry(&store, "/v", "other.md", 1).is_some());
    }

    #[test]
    fn renamed_diagram_state_merges_an_existing_target_entry() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "old.md".into(),
                block: 1,
                collapsed: vec!["a".into(), "b".into()],
                ..Default::default()
            })
            .unwrap();
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "new.md".into(),
                block: 1,
                collapsed: vec!["b".into(), "c".into()],
                ..Default::default()
            })
            .unwrap();
        store
            .remember_diagram_focus(
                "/v".into(),
                "new.md".into(),
                1,
                Some(1.5),
                Some(4.0),
                Some(8.0),
            )
            .unwrap();
        store
            .rename_diagram_state("/v", "old.md", "new.md", false)
            .unwrap();
        let merged = diagram_entry(&store, "/v", "new.md", 1).unwrap();
        assert_eq!(merged.collapsed, vec!["a", "b", "c"]);
        assert_eq!(merged.focus_scale, Some(1.5));
        assert!(diagram_entry(&store, "/v", "old.md", 1).is_none());
    }

    #[test]
    fn entry_is_removed_only_when_no_state_remains() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "n.md".into(),
                block: 1,
                collapsed: vec!["a".into()],
                ..Default::default()
            })
            .unwrap();
        store
            .remember_diagram(DiagramState {
                vault: "/v".into(),
                note: "n.md".into(),
                block: 1,
                collapsed: vec![],
                ..Default::default()
            })
            .unwrap();
        assert!(diagram_entry(&store, "/v", "n.md", 1).is_none());
    }

    #[test]
    fn agent_profiles_round_trip_preserves_comments_and_legacy_fallback() {
        let root = tempfile::TempDir::new().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let codex = bin.join("codex");
        let claude = bin.join("claude");
        make_executable(&codex);
        make_executable(&claude);

        let store = diagram_store(&root);
        let initial = format!(
            "# 顶部自定义注释\nfont_size = 16.0\ndefault_view = \"reading\"\nunknown_flag = true\n\n[agent]\ncommand = \"{}\"\nargs = [\"{{guide}}\"]\n\n[[vault]]\npath = \"/tmp/example\"\nfont_size = 18.0\n",
            codex.display()
        );
        fs::write(store.config_path(), &initial).unwrap();

        // 1. Legacy [agent] without [[agent.profiles]] works seamlessly.
        let cfg = store.editable_config().unwrap();
        assert!(cfg.agent_configured);
        assert!(cfg.agent_profiles.is_empty());
        let launch = store.agent_launch().unwrap();
        assert_eq!(launch.executable, resolved(&codex));
        assert!(launch.args[0].contains("Hortus"));
        assert_eq!(
            store
                .agent_launch_for_profile_with_paths(Some("missing"), None)
                .unwrap_err()
                .to_string(),
            "agent.profile_not_found"
        );

        // 2. Save multiple [[agent.profiles]] and switch default_profile.
        let profiles = vec![
            AgentProfile {
                id: "codex".into(),
                name: "Codex".into(),
                command: codex.to_string_lossy().into_owned(),
                args: vec!["{guide}".into()],
            },
            AgentProfile {
                id: "claude".into(),
                name: "Claude Code".into(),
                command: claude.to_string_lossy().into_owned(),
                args: vec!["--append-system-prompt".into(), "{guide}".into()],
            },
        ];
        let saved = store
            .save_config_with_profiles(
                &EditableConfigInput {
                    settings: cfg.settings.clone(),
                    agent_command: None,
                    agent_args: vec![],
                },
                &profiles,
                Some("claude"),
                Some(&cfg.digest),
            )
            .unwrap();
        assert_eq!(saved.agent_profiles.len(), 2);
        assert_eq!(saved.agent_default_profile.as_deref(), Some("claude"));
        assert_eq!(
            saved.agent_command.as_deref(),
            Some(claude.to_string_lossy().as_ref())
        );

        // Comments, unknown keys, and [[vault]] must remain intact.
        let raw = fs::read_to_string(store.config_path()).unwrap();
        assert!(raw.contains("# 顶部自定义注释"));
        assert!(raw.contains("unknown_flag = true"));
        assert!(raw.contains("[[vault]]"));
        assert!(raw.contains("path = \"/tmp/example\""));

        // Default launch resolves "claude"; explicit profile_id resolves "codex".
        let default_launch = store.agent_launch().unwrap();
        assert_eq!(default_launch.executable, resolved(&claude));
        let codex_launch = store
            .agent_launch_for_profile_with_paths(Some("codex"), None)
            .unwrap();
        assert_eq!(codex_launch.executable, resolved(&codex));
    }

    #[test]
    fn saving_profiles_preserves_comments_and_unknown_keys_by_id() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        fs::write(
            store.config_path(),
            r#"font_size = 15.0

[agent]
default_profile = "alpha"

# Alpha header
[[agent.profiles]] # Alpha inline
id = "alpha"
name = "Alpha"
command = "codex" # Alpha command
args = [
    "--alpha", # Keep argument note
    "{guide}",
] # Alpha arguments
extra = "keep-alpha"

[agent.profiles.extra_table] # Alpha custom section
value = "belongs-to-alpha"

# Beta header
[[agent.profiles]]
id = "beta"
name = "Beta"
command = "claude" # Beta command
args = ["{guide}"]
extra = "keep-beta"

# Obsolete header
[[agent.profiles]]
id = "old"
name = "Old"
command = "old"
args = ["{guide}"]

[[vault]]
path = "/tmp/example"
"#,
        )
        .unwrap();
        let save = |mut config: EditableConfig, profiles: &[AgentProfile]| {
            config.settings.font_size += 1.0;
            store
                .save_config_with_profiles(
                    &EditableConfigInput {
                        settings: config.settings,
                        agent_command: config.agent_command,
                        agent_args: config.agent_args,
                    },
                    profiles,
                    Some("alpha"),
                    Some(&config.digest),
                )
                .unwrap()
        };
        let check_retained = |command: &str| {
            let text = fs::read_to_string(store.config_path()).unwrap();
            for expected in [
                "# Alpha header\n[[agent.profiles]] # Alpha inline\nid = \"alpha\"",
                "# Beta header\n[[agent.profiles]]\nid = \"beta\"",
                "\"--alpha\", # Keep argument note",
                "] # Alpha arguments",
                "extra = \"keep-alpha\"",
                "extra = \"keep-beta\"",
                "command = \"claude\" # Beta command",
                "[[vault]]\npath = \"/tmp/example\"",
            ] {
                assert!(text.contains(expected), "missing {expected:?} in {text}");
            }
            assert!(text.contains(&format!("command = \"{command}\" # Alpha command")));
            let parsed = text.parse::<toml_edit::DocumentMut>().unwrap();
            let entries = parsed["agent"]["profiles"].as_array_of_tables().unwrap();
            for entry in entries {
                if entry["id"].as_str() == Some("alpha") {
                    assert_eq!(
                        entry["extra_table"]["value"].as_str(),
                        Some("belongs-to-alpha")
                    );
                } else {
                    assert!(entry.get("extra_table").is_none());
                }
            }
            assert!(text.contains("[agent.profiles.extra_table] # Alpha custom section"));
            text
        };
        let initial = store.editable_config().unwrap();
        let mut profiles = initial.agent_profiles.clone();
        let saved = save(initial, &profiles);
        check_retained("codex");

        profiles[0].command = "codex-next".into();
        let saved = save(saved, &profiles);
        check_retained("codex-next");

        profiles.pop();
        profiles.swap(0, 1);
        profiles.push(AgentProfile {
            id: "new".into(),
            name: "New".into(),
            command: "new-agent".into(),
            args: vec!["{guide}".into()],
        });
        let saved = save(saved, &profiles);
        assert_eq!(
            saved
                .agent_profiles
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["beta", "alpha", "new"]
        );
        assert!(!check_retained("codex-next").contains("# Obsolete header"));
    }

    #[test]
    fn empty_agent_table_has_no_command_diagnostic() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        for text in [
            "[agent]\n# No command\n",
            "[agent]\nextra = true\nprofiles = []\n",
        ] {
            fs::write(store.config_path(), format!("font_size = 15.0\n{text}")).unwrap();
            let config = store.editable_config().unwrap();
            assert!(!config.agent_configured);
            assert!(config.diagnostics.is_empty(), "{:?}", config.diagnostics);
        }
    }

    #[test]
    fn custom_agent_launch_writes_readonly_guide_in_cache_and_tracks_history() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);

        // Rejects empty and NUL commands.
        assert_eq!(
            store
                .prepare_custom_agent_launch("   ", None)
                .unwrap_err()
                .to_string(),
            "agent.command_missing"
        );
        assert_eq!(
            store
                .prepare_custom_agent_launch("echo \0 bad", None)
                .unwrap_err()
                .to_string(),
            "agent.invalid_argument"
        );

        let launch = store
            .prepare_custom_agent_launch("codex --model o3", None)
            .unwrap();
        assert!(!launch.executable.is_empty());
        assert_eq!(launch.args, vec!["-lc", "codex --model o3"]);
        // Guide text is NEVER concatenated into the shell command line.
        assert!(!launch.args[1].contains('\n'));
        let guide_path = PathBuf::from(&launch.guide_file);
        assert!(guide_path.starts_with(&store.directories.cache));
        assert!(guide_path.exists());
        let guide_content = fs::read_to_string(&guide_path).unwrap();
        assert_eq!(guide_content, AGENT_GUIDE);
        #[cfg(unix)]
        {
            let mode = fs::metadata(&guide_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o444, "guide file must be read-only");
        }
        let override_guide = "# Alternate guide\n";
        store.save_agent_guide(override_guide).unwrap();
        let second = store
            .prepare_custom_agent_launch("claude --print", None)
            .unwrap();
        assert_ne!(second.guide_file, launch.guide_file);
        assert_eq!(fs::read_to_string(&guide_path).unwrap(), AGENT_GUIDE);
        assert_eq!(
            fs::read_to_string(&second.guide_file).unwrap(),
            override_guide
        );
        assert_eq!(
            fs::metadata(&second.guide_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );

        // History deduplicates and caps at 12 entries (most recent first).
        for i in 1..=15 {
            store
                .record_custom_agent_command(&format!("cmd-{i}"))
                .unwrap();
        }
        let history = store.record_custom_agent_command("cmd-10").unwrap();
        assert_eq!(history.len(), 12);
        assert_eq!(history[0], "cmd-10");
        assert_eq!(history[1], "cmd-15");
        assert_eq!(
            history
                .iter()
                .filter(|item| item.as_str() == "cmd-10")
                .count(),
            1
        );
        assert!(
            store
                .directories
                .cache
                .join("agent-custom-history.json")
                .exists()
        );
    }

    #[test]
    fn launch_guide_cleanup_retains_newest_and_protects_recent_files() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        let directory = store.directories.cache.join("agent-guides");
        fs::create_dir_all(&directory).unwrap();
        for i in 0..40 {
            let path = directory.join(format!("guide-old-{i}.md"));
            let mut file = fs::File::create(path).unwrap();
            file.write_all(b"# Old guide\n").unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(i + 10))
                .unwrap();
            file.set_permissions(fs::Permissions::from_mode(0o444))
                .unwrap();
        }
        let extra = directory.join("unrelated.md");
        fs::write(&extra, "Keep").unwrap();
        fs::set_permissions(&extra, fs::Permissions::from_mode(0o444)).unwrap();

        let launch = store.prepare_custom_agent_launch("first", None).unwrap();
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            RETAINED_LAUNCH_GUIDES + 1
        );
        assert!(!directory.join("guide-old-0.md").exists());
        assert!(directory.join("guide-old-39.md").exists());
        assert_eq!(fs::read_to_string(&extra).unwrap(), "Keep");

        let recent: Vec<_> = (0..RETAINED_LAUNCH_GUIDES)
            .map(|_| write_launch_guide(&directory, b"# Recent guide\n").unwrap())
            .collect();
        store.prepare_custom_agent_launch("next", None).unwrap();
        assert!(Path::new(&launch.guide_file).exists());
        assert!(recent.iter().all(|path| path.exists()));
        assert!(!directory.join("guide-old-39.md").exists());
    }

    #[test]
    fn explicit_color_scheme_distinguishes_absent_valid_and_invalid() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        let path = store.config_path();
        fs::remove_file(&path).unwrap();
        assert_eq!(store.explicit_color_scheme().unwrap(), None);
        for text in [
            "font_size = 15.0",
            "# color_scheme = \"dark\"",
            "[extra]\ncolor_scheme = \"dark\"",
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(store.explicit_color_scheme().unwrap(), None, "{text}");
        }
        for (text, value) in [
            ("color_scheme = \"system\"", "system"),
            ("\"color_scheme\" = \"system\"", "system"),
            ("color_scheme = \" dark \"", "dark"),
            ("color_scheme = \"light\"", "light"),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(
                store.explicit_color_scheme().unwrap().as_deref(),
                Some(value)
            );
        }
        for text in [
            "color_scheme = \"neon\"",
            "color_scheme = \" \"",
            "color_scheme = false",
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(
                store.explicit_color_scheme().unwrap_err().to_string(),
                "config.color_scheme_invalid"
            );
        }
        assert!(store.color_scheme().is_err());
    }

    #[test]
    fn color_scheme_roundtrip_preserves_comments_and_rejects_invalid() {
        let root = tempfile::TempDir::new().unwrap();
        let store = diagram_store(&root);
        let original = "\
# 顶部外观注释
font_size = 15.0
default_view = \"reading\"
unknown_flag = \"keep\"

[agent]
default_profile = \"codex\"
command = \"codex\"
args = [\"{guide}\"]

[[agent.profiles]]
id = \"codex\"
name = \"Codex\"
command = \"codex\"
args = [\"{guide}\"]

[[vault]]
path = \"/tmp/example\"
font_size = 17.0
";
        fs::write(store.config_path(), original).unwrap();
        assert_eq!(store.color_scheme().unwrap(), "system");

        store.save_color_scheme("dark").unwrap();
        assert_eq!(store.color_scheme().unwrap(), "dark");

        store.save_color_scheme("light").unwrap();
        assert_eq!(store.color_scheme().unwrap(), "light");

        store.save_color_scheme("   ").unwrap();
        assert_eq!(store.color_scheme().unwrap(), "system");

        let err = store.save_color_scheme("neon").unwrap_err();
        assert_eq!(err.to_string(), "config.color_scheme_invalid");

        let raw = fs::read_to_string(store.config_path()).unwrap();
        assert!(raw.contains("# 顶部外观注释"));
        assert!(raw.contains("unknown_flag = \"keep\""));
        assert!(raw.contains("[[agent.profiles]]"));
        assert!(raw.contains("[[vault]]"));
        assert!(raw.contains("color_scheme = \"system\""));
    }

    #[test]
    fn custom_agent_shell_fallback_handles_missing_and_nonexistent() {
        #[cfg(target_os = "macos")]
        let expected = "/bin/zsh";
        #[cfg(not(target_os = "macos"))]
        let expected = "/bin/sh";

        assert_eq!(resolve_custom_agent_shell(None), expected);
        assert_eq!(resolve_custom_agent_shell(Some("")), expected);
        assert_eq!(resolve_custom_agent_shell(Some("   ")), expected);
        assert_eq!(
            resolve_custom_agent_shell(Some("/nonexistent/hortus-missing-shell")),
            expected
        );
        let root = tempfile::TempDir::new().unwrap();
        let nonexecutable = root.path().join("not-executable");
        fs::write(&nonexecutable, b"data").unwrap();
        for value in [
            ".",
            root.path().to_str().unwrap(),
            nonexecutable.to_str().unwrap(),
        ] {
            assert_eq!(resolve_custom_agent_shell(Some(value)), expected);
        }
        if Path::new("/bin/sh").exists() {
            assert_eq!(resolve_custom_agent_shell(Some("/bin/sh")), "/bin/sh");
        }
    }
}
