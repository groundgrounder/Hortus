//! Swift 只通过这层调用 Rust；目录 I/O 和扫描在 Swift 后台任务及 Rust 工作线程执行。
use hortus_config::{Directories, LayoutState, Store, VaultState};
use hortus_history as history;
use hortus_links::KnowledgeIndex;
use hortus_search::SearchIndex;
use hortus_vault::Vault;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

mod diagram;
mod document;
mod links;
mod math;
mod media;
mod scene;
pub use diagram::*;
pub use document::*;
pub use links::*;
pub use math::*;
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
pub struct DiagramFocus {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
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

#[derive(Debug, uniffi::Record)]
pub struct AgentSpec {
    pub executable: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct AgentProfileInfo {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct CustomAgentLaunchInfo {
    pub executable: String,
    pub args: Vec<String>,
    pub guide_file: String,
    pub path: String,
}

#[derive(Debug, uniffi::Record)]
pub struct EditableConfigInfo {
    pub config_path: String,
    pub digest: String,
    pub font_size: f64,
    pub default_view: String,
    pub terminal_font: Option<String>,
    pub agent_configured: bool,
    pub agent_command: Option<String>,
    pub agent_args: Vec<String>,
    pub agent_profiles: Vec<AgentProfileInfo>,
    pub agent_default_profile: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct EditableConfigInputInfo {
    pub font_size: f64,
    pub default_view: String,
    pub terminal_font: Option<String>,
    pub agent_command: Option<String>,
    pub agent_args: Vec<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct AgentProbeInfo {
    pub available: bool,
    pub executable: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct DiscoveredAgentInfo {
    pub id: String,
    pub display_name: String,
    pub binary_name: String,
    pub executable: String,
    pub suggested_args: Vec<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct AgentGuideInfo {
    pub text: String,
    pub overridden: bool,
    pub path: String,
    pub default_text: String,
}

#[derive(Debug, uniffi::Record)]
pub struct AgentSessionInfo {
    pub id: String,
    pub vault: String,
    pub state: String,
    pub snapshot_path: String,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub entries: u64,
    pub bytes: u64,
    pub method: String,
    pub error: Option<String>,
    pub usable: bool,
    pub snapshot_at_ms: i64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewMetadata {
    pub kind: String,
    pub size: u64,
    pub mode: u32,
    pub modified_ms: Option<i64>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewEntryInfo {
    pub id: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change: String,
    pub before: Option<ReviewMetadata>,
    pub after: Option<ReviewMetadata>,
    pub text: bool,
    pub content_identical: bool,
    pub before_digest: Option<String>,
    pub after_digest: Option<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct ReviewCounts {
    pub added: u64,
    pub modified: u64,
    pub deleted: u64,
    pub renamed: u64,
    pub type_changed: u64,
}

#[derive(Debug, uniffi::Record)]
pub struct SessionReviewSummary {
    pub session_id: String,
    pub vault: String,
    pub state: String,
    pub snapshot_at_ms: i64,
    pub counts: ReviewCounts,
    pub elapsed_ms: u64,
    pub entries: Vec<ReviewEntryInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewDiffLine {
    pub kind: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewDiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<ReviewDiffLine>,
}

#[derive(Debug, uniffi::Record)]
pub struct ReviewEntryDetail {
    pub entry: ReviewEntryInfo,
    pub text: bool,
    pub binary: bool,
    pub large: bool,
    pub content_identical: bool,
    pub old_no_final_newline: bool,
    pub new_no_final_newline: bool,
    pub line_endings_changed: bool,
    pub hunks: Vec<ReviewDiffHunk>,
}

/// UI 提交的审阅记录；回滚只按这份指纹复核，不信任之后重新扫描的结果。
#[derive(Debug, Clone, uniffi::Record)]
pub struct RollbackExpectationInfo {
    pub path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change: String,
    pub expected_before: Option<String>,
    pub expected_after: Option<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct RollbackFailureInfo {
    pub path: String,
    pub change: String,
    pub reason: String,
}

#[derive(Debug, uniffi::Record)]
pub struct SessionRollbackReportInfo {
    pub reverted_paths: Vec<String>,
    pub skipped_stale: Vec<RollbackFailureInfo>,
    pub failed: Vec<RollbackFailureInfo>,
}

fn rollback_expectation(
    info: &RollbackExpectationInfo,
) -> Result<history::rollback::RollbackExpectation, CoreError> {
    let change = history::review::ChangeKind::parse(&info.change)
        .ok_or_else(|| CoreError::failure("history.invalid_entry"))?;
    Ok(history::rollback::RollbackExpectation {
        path: info.path.clone(),
        old_path: info.old_path.clone(),
        new_path: info.new_path.clone(),
        change,
        expected_before: info.expected_before.clone(),
        expected_after: info.expected_after.clone(),
    })
}

fn rollback_report(report: history::rollback::SessionRollbackReport) -> SessionRollbackReportInfo {
    SessionRollbackReportInfo {
        reverted_paths: report.reverted_paths,
        skipped_stale: report
            .skipped_stale
            .into_iter()
            .map(|failure| RollbackFailureInfo {
                path: failure.path,
                change: failure.change,
                reason: failure.reason,
            })
            .collect(),
        failed: report
            .failed
            .into_iter()
            .map(|failure| RollbackFailureInfo {
                path: failure.path,
                change: failure.change,
                reason: failure.reason,
            })
            .collect(),
    }
}

fn rollback_hunk(hunk: &ReviewDiffHunk) -> history::review::DiffHunk {
    history::review::DiffHunk {
        old_start: hunk.old_start,
        old_count: hunk.old_count,
        new_start: hunk.new_start,
        new_count: hunk.new_count,
        lines: hunk
            .lines
            .iter()
            .map(|line| history::review::DiffLine {
                kind: line.kind.clone(),
                old_line: line.old_line,
                new_line: line.new_line,
                text: line.text.clone(),
            })
            .collect(),
    }
}

/// 与保存路径共用同卷临时目录规则：调用方传入 `itemReplacementDirectory`，
/// 未传入时退回系统临时目录并在同卷校验失败时拒绝回滚。
struct RollbackTemporary {
    path: std::path::PathBuf,
}

impl Drop for RollbackTemporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn rollback_temporary(
    vault: &std::path::Path,
    provided: Option<String>,
) -> Result<RollbackTemporary, CoreError> {
    let base = provided
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let unique = format!(
        "hortus-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let path = base.join(unique);
    std::fs::create_dir(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let permissions = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(&path, permissions)?;
        let parent = std::fs::metadata(&path)?;
        let root = std::fs::metadata(vault)?;
        if parent.dev() != root.dev() {
            return Err(CoreError::failure("history.temporary_unavailable"));
        }
    }
    Ok(RollbackTemporary { path })
}

#[derive(uniffi::Object)]
pub struct ReviewCancellation {
    cancelled: std::sync::atomic::AtomicBool,
}

#[uniffi::export]
impl ReviewCancellation {
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

impl ReviewCancellation {
    fn flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.cancelled
    }
}

fn review_metadata(metadata: &history::review::EntryMetadata) -> ReviewMetadata {
    ReviewMetadata {
        kind: metadata.kind.clone(),
        size: metadata.size,
        mode: metadata.mode,
        modified_ms: metadata.modified_ms,
        link: metadata.link.clone(),
    }
}

fn review_entry(entry: &history::review::ReviewEntry) -> ReviewEntryInfo {
    ReviewEntryInfo {
        id: entry.id.clone(),
        old_path: entry.old_path.clone(),
        new_path: entry.new_path.clone(),
        change: entry.change.as_str().into(),
        before: entry.before.as_ref().map(review_metadata),
        after: entry.after.as_ref().map(review_metadata),
        text: entry.text,
        content_identical: entry.content_identical,
        before_digest: entry.before_digest.clone(),
        after_digest: entry.after_digest.clone(),
    }
}

fn review_summary(summary: history::review::ReviewSummary) -> SessionReviewSummary {
    SessionReviewSummary {
        session_id: summary.session_id,
        vault: summary.vault,
        state: summary.state,
        snapshot_at_ms: summary.snapshot_at_ms,
        counts: ReviewCounts {
            added: summary.counts.added,
            modified: summary.counts.modified,
            deleted: summary.counts.deleted,
            renamed: summary.counts.renamed,
            type_changed: summary.counts.type_changed,
        },
        elapsed_ms: summary.elapsed_ms,
        entries: summary.entries.iter().map(review_entry).collect(),
    }
}

fn review_detail(detail: history::review::ReviewDetail) -> ReviewEntryDetail {
    ReviewEntryDetail {
        entry: review_entry(&detail.entry),
        text: detail.text,
        binary: detail.binary,
        large: detail.large,
        content_identical: detail.content_identical,
        old_no_final_newline: detail.old_no_final_newline,
        new_no_final_newline: detail.new_no_final_newline,
        line_endings_changed: detail.line_endings_changed,
        hunks: detail
            .hunks
            .into_iter()
            .map(|hunk| ReviewDiffHunk {
                old_start: hunk.old_start,
                old_count: hunk.old_count,
                new_start: hunk.new_start,
                new_count: hunk.new_count,
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| ReviewDiffLine {
                        kind: line.kind,
                        old_line: line.old_line,
                        new_line: line.new_line,
                        text: line.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[derive(uniffi::Object)]
pub struct WorkspaceCore {
    store: Mutex<Store>,
}

fn session_info(
    data: &std::path::Path,
    record: &history::SessionRecord,
) -> Result<AgentSessionInfo, CoreError> {
    let snapshot = history::HistoryStore::new(data)
        .snapshot_directory(&record.id)?
        .to_string_lossy()
        .into_owned();
    let stats = record.stats.clone().unwrap_or_default();
    Ok(AgentSessionInfo {
        id: record.id.clone(),
        vault: record.vault.clone(),
        state: record.state.as_str().into(),
        snapshot_path: snapshot,
        created_at_ms: record.created_at_ms,
        started_at_ms: record.started_at_ms,
        ended_at_ms: record.ended_at_ms,
        entries: stats.entries,
        bytes: stats.bytes,
        method: stats.method,
        error: record.error.clone(),
        usable: record.state.usable(),
        snapshot_at_ms: record.created_at_ms.saturating_add(stats.elapsed_ms as i64),
        exit_code: record.exit_code,
    })
}

fn editable_config_info(config: hortus_config::EditableConfig) -> EditableConfigInfo {
    EditableConfigInfo {
        config_path: config.config_path,
        digest: config.digest,
        font_size: config.settings.font_size,
        default_view: match config.settings.default_view {
            hortus_config::ViewMode::Reading => "reading",
            hortus_config::ViewMode::Editing => "editing",
        }
        .into(),
        terminal_font: config.settings.terminal_font,
        agent_configured: config.agent_configured,
        agent_command: config.agent_command,
        agent_args: config.agent_args,
        agent_profiles: config
            .agent_profiles
            .into_iter()
            .map(|profile| AgentProfileInfo {
                id: profile.id,
                name: profile.name,
                command: profile.command,
                args: profile.args,
            })
            .collect(),
        agent_default_profile: config.agent_default_profile,
        diagnostics: config.diagnostics,
    }
}

fn agent_guide_info(state: hortus_config::GuideState) -> AgentGuideInfo {
    AgentGuideInfo {
        text: state.text,
        overridden: state.overridden,
        path: state.path,
        default_text: state.default_text,
    }
}

#[uniffi::export]
impl WorkspaceCore {
    #[uniffi::constructor]
    pub fn in_directories(config: String, cache: String) -> Result<Arc<Self>, CoreError> {
        // 测试与检查用显式 config/cache；持久数据根仍按 HORTUS_DATA_DIR 或系统规则解析。
        let directories = Directories::resolve_from_env()?;
        Ok(Arc::new(Self {
            store: Mutex::new(Store::open(Directories {
                config: config.into(),
                cache: cache.into(),
                data: directories.data,
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
            .unwrap_or_else(|error| error.into_inner())
            .layout()?
            .map(|value| serde_json::to_string(&value).map_err(Into::into))
            .transpose()
    }

    pub fn save_layout(&self, json: String) -> Result<(), CoreError> {
        let state: LayoutState = serde_json::from_str(&json)?;
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_layout(&state)?)
    }

    pub fn session_json(&self) -> Result<String, CoreError> {
        Ok(serde_json::to_string(
            &self
                .store
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .session()?,
        )?)
    }

    /// 完整窗口列表只由应用协调器写入，不能由单个窗口覆盖。
    pub fn save_window_session(&self, json: String) -> Result<(), CoreError> {
        let snapshot: hortus_config::SessionState = serde_json::from_str(&json)?;
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_windows(snapshot)?)
    }

    pub fn default_view(&self) -> Result<String, CoreError> {
        Ok(match self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settings()?
            .default_view
        {
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
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
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
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settings()?
            .font_size)
    }

    pub fn terminal_font(&self) -> Result<Option<String>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settings()?
            .terminal_font)
    }

    /// 完整设置快照：配置路径、原始字节指纹、已知字段与稳定诊断键。
    pub fn editable_config(&self) -> Result<EditableConfigInfo, CoreError> {
        let config = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .editable_config()?;
        Ok(editable_config_info(config))
    }

    /// 原地保存已知键；指纹不一致时返回 `config.externally_modified`。
    pub fn save_editable_config(
        &self,
        input: EditableConfigInputInfo,
        expected_digest: Option<String>,
    ) -> Result<EditableConfigInfo, CoreError> {
        let default_view = match input.default_view.as_str() {
            "reading" => hortus_config::ViewMode::Reading,
            "editing" => hortus_config::ViewMode::Editing,
            _ => return Err(CoreError::failure("config.default_view_invalid")),
        };
        let saved = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_config(
                &hortus_config::EditableConfigInput {
                    settings: hortus_config::Settings {
                        font_size: input.font_size,
                        default_view,
                        terminal_font: input.terminal_font,
                    },
                    agent_command: input.agent_command,
                    agent_args: input.agent_args,
                },
                expected_digest.as_deref(),
            )?;
        Ok(editable_config_info(saved))
    }

    /// 原地保存已知键与多配置档；指纹不一致时返回 `config.externally_modified`。
    pub fn save_editable_config_with_profiles(
        &self,
        input: EditableConfigInputInfo,
        agent_profiles: Vec<AgentProfileInfo>,
        agent_default_profile: Option<String>,
        expected_digest: Option<String>,
    ) -> Result<EditableConfigInfo, CoreError> {
        let default_view = match input.default_view.as_str() {
            "reading" => hortus_config::ViewMode::Reading,
            "editing" => hortus_config::ViewMode::Editing,
            _ => return Err(CoreError::failure("config.default_view_invalid")),
        };
        let profiles: Vec<hortus_config::AgentProfile> = agent_profiles
            .into_iter()
            .map(|profile| hortus_config::AgentProfile {
                id: profile.id,
                name: profile.name,
                command: profile.command,
                args: profile.args,
            })
            .collect();
        let saved = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_config_with_profiles(
                &hortus_config::EditableConfigInput {
                    settings: hortus_config::Settings {
                        font_size: input.font_size,
                        default_view,
                        terminal_font: input.terminal_font,
                    },
                    agent_command: input.agent_command,
                    agent_args: input.agent_args,
                },
                &profiles,
                agent_default_profile.as_deref(),
                expected_digest.as_deref(),
            )?;
        Ok(editable_config_info(saved))
    }

    /// 只解析模板与可执行文件；不启动进程、不建立快照、不创建会话目录。
    pub fn probe_agent_launch(&self, command: String, args: Vec<String>) -> AgentProbeInfo {
        self.probe_agent_launch_with_path(command, args, None)
    }

    /// 探测追加后台登录 shell 得到的 PATH；仍只做静态解析。
    pub fn probe_agent_launch_with_path(
        &self,
        command: String,
        args: Vec<String>,
        login_path: Option<String>,
    ) -> AgentProbeInfo {
        let status = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .probe_agent_command_with_paths(
                &command,
                &args,
                login_path.as_deref().map(std::ffi::OsStr::new),
            );
        AgentProbeInfo {
            available: status.available,
            executable: status.executable,
            error: status.error,
        }
    }

    /// 只读扫描内置预设；绝不执行目标二进制。
    pub fn discover_agents(&self, login_path: Option<String>) -> Vec<DiscoveredAgentInfo> {
        self.store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .discover_agents_in(login_path.as_deref().map(std::ffi::OsStr::new))
            .into_iter()
            .map(|agent| DiscoveredAgentInfo {
                id: agent.id,
                display_name: agent.display_name,
                binary_name: agent.binary_name,
                executable: agent.executable,
                suggested_args: agent.suggested_args,
            })
            .collect()
    }

    /// 子进程 PATH：发现、探测与启动共用同一份组合路径。
    pub fn effective_agent_path(&self, login_path: Option<String>) -> String {
        self.store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .effective_agent_path(login_path.as_deref().map(std::ffi::OsStr::new))
            .to_string_lossy()
            .into_owned()
    }

    pub fn agent_guide_state(&self) -> Result<AgentGuideInfo, CoreError> {
        Ok(agent_guide_info(
            self.store
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .guide_state()?,
        ))
    }

    pub fn save_agent_guide(&self, content: String) -> Result<AgentGuideInfo, CoreError> {
        Ok(agent_guide_info(
            self.store
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .save_agent_guide(&content)?,
        ))
    }

    pub fn reset_agent_guide(&self) -> Result<AgentGuideInfo, CoreError> {
        Ok(agent_guide_info(
            self.store
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .reset_agent_guide()?,
        ))
    }

    pub fn remember(
        &self,
        path: String,
        tabs: Vec<String>,
        selected: Option<String>,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        store.validate_vault(std::path::Path::new(&path))?;
        // 只改标签与选择时保留已存的视图模式，避免覆盖成空表。
        let views = store
            .session()?
            .recent
            .into_iter()
            .find(|state| state.path == path)
            .map(|state| state.views)
            .unwrap_or_default();
        store.remember(VaultState {
            path,
            tabs,
            selected,
            views,
            visible: None,
        })?;
        Ok(())
    }

    /// 标签、选择与视图模式一次完整写入；常规会话保存和退出共用这个入口。
    pub fn remember_session(
        &self,
        path: String,
        tabs: Vec<String>,
        selected: Option<String>,
        views: std::collections::HashMap<String, String>,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        store.validate_vault(std::path::Path::new(&path))?;
        store.remember(VaultState {
            path,
            tabs,
            selected,
            visible: None,
            views: views
                .into_iter()
                .map(|(note, mode)| {
                    (
                        note,
                        if mode == "editing" {
                            hortus_config::ViewMode::Editing
                        } else {
                            hortus_config::ViewMode::Reading
                        },
                    )
                })
                .collect(),
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
            .unwrap_or_else(|error| error.into_inner())
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
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        store.validate_vault(std::path::Path::new(&vault))?;
        Ok(store.remember_diagram(hortus_config::DiagramState {
            vault,
            note,
            block,
            collapsed,
            ..Default::default()
        })?)
    }

    /// 笔记或文件夹重命名后迁移 `session.toml` 的折叠与专注视口记录。
    pub fn rename_diagram_state(
        &self,
        vault: String,
        old_path: String,
        new_path: String,
        directory: bool,
    ) -> Result<(), CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .rename_diagram_state(&vault, &old_path, &new_path, directory)?)
    }

    /// 移到废纸篓成功后清理该笔记或文件夹前缀下的图缓存。
    pub fn remove_diagram_state(
        &self,
        vault: String,
        path: String,
        directory: bool,
    ) -> Result<(), CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove_diagram_state(&vault, &path, directory)?)
    }

    /// 读取专注模式保存的缩放与平移；从未进入过专注模式时返回空。
    pub fn diagram_focus(
        &self,
        vault: String,
        note: String,
        block: u32,
    ) -> Result<Option<DiagramFocus>, CoreError> {
        let session = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .session()?;
        Ok(session
            .diagrams
            .into_iter()
            .find(|s| s.vault == vault && s.note == note && s.block == block)
            .and_then(|s| {
                s.focus_scale.map(|scale| DiagramFocus {
                    scale,
                    offset_x: s.focus_offset_x.unwrap_or(0.0),
                    offset_y: s.focus_offset_y.unwrap_or(0.0),
                })
            }))
    }

    /// 只写专注视图状态；同一图块的折叠状态保持不变。
    pub fn remember_diagram_focus(
        &self,
        vault: String,
        note: String,
        block: u32,
        scale: f64,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), CoreError> {
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        store.validate_vault(std::path::Path::new(&vault))?;
        Ok(store.remember_diagram_focus(
            vault,
            note,
            block,
            Some(scale),
            Some(offset_x),
            Some(offset_y),
        )?)
    }

    pub fn open_vault(&self, path: String) -> Result<Arc<VaultHandle>, CoreError> {
        self.store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .validate_vault(std::path::Path::new(&path))?;
        Ok(Arc::new(VaultHandle {
            vault: Vault::open(path)?,
            search: RwLock::new(SearchIndex::new()),
            links: RwLock::new(KnowledgeIndex::new()),
            index_update: Mutex::new(()),
            rename_plans: Mutex::new(Default::default()),
        }))
    }

    /// 读取并校验全局 [agent] 模板；返回已解析的命令与 `{guide}` 已替换的 argv。
    /// 不创建目录、不建立快照、不启动进程。
    pub fn agent_spec(&self) -> Result<AgentSpec, CoreError> {
        self.agent_spec_with_path(None)
    }

    /// 启动解析追加后台登录 shell 得到的 PATH；不执行模板里的命令。
    pub fn agent_spec_with_path(&self, login_path: Option<String>) -> Result<AgentSpec, CoreError> {
        self.agent_spec_for_profile(None, login_path)
    }

    /// 按配置档解析 Agent 启动规格；`profile_id` 为空时使用默认配置档或旧 `[agent]`。
    pub fn agent_spec_for_profile(
        &self,
        profile_id: Option<String>,
        login_path: Option<String>,
    ) -> Result<AgentSpec, CoreError> {
        let launch = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .agent_launch_for_profile_with_paths(
                profile_id.as_deref(),
                login_path.as_deref().map(std::ffi::OsStr::new),
            )?;
        Ok(AgentSpec {
            executable: launch.executable,
            args: launch.args,
        })
    }

    /// 准备自定义命令启动 Agent：将有效指南写入缓存目录只读文件，返回 `$SHELL -lc <command>` 规格。
    pub fn prepare_custom_agent_launch(
        &self,
        command: String,
        login_path: Option<String>,
    ) -> Result<CustomAgentLaunchInfo, CoreError> {
        let launch = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .prepare_custom_agent_launch(
                &command,
                login_path.as_deref().map(std::ffi::OsStr::new),
            )?;
        Ok(CustomAgentLaunchInfo {
            executable: launch.executable,
            args: launch.args,
            guide_file: launch.guide_file,
            path: launch.path,
        })
    }

    /// 读取系统缓存中的最近自定义命令历史。
    pub fn custom_agent_history(&self) -> Result<Vec<String>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .custom_agent_history()?)
    }

    /// 记录一条自定义命令到系统缓存历史（去重、上限 12 条）。
    pub fn record_custom_agent_command(&self, command: String) -> Result<Vec<String>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_custom_agent_command(&command)?)
    }

    /// 读取顶层可选外观配色（`"system"` / `"light"` / `"dark"`）。
    pub fn color_scheme(&self) -> Result<String, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .color_scheme()?)
    }

    /// 返回显式的顶层外观配色。键缺失时返回空值，显式非法时返回配置错误。
    pub fn explicit_color_scheme(&self) -> Result<Option<String>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .explicit_color_scheme()?)
    }

    /// 原地更新顶层外观配色至 `config.toml`，保留注释与未知字段。
    pub fn save_color_scheme(&self, scheme: String) -> Result<(), CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_color_scheme(&scheme)?)
    }

    pub fn data_directory(&self) -> String {
        self.store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_string_lossy()
            .into_owned()
    }

    /// 同步建立全库快照；Swift 在后台任务中调用，不占用主线程。
    /// 只在锁内校验库并复制数据根，遍历、复制与会话写入都在锁外进行。
    pub fn begin_agent_session(&self, vault: String) -> Result<AgentSessionInfo, CoreError> {
        let data = {
            let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
            store.validate_vault(std::path::Path::new(&vault))?;
            store.data_root().to_path_buf()
        };
        let history = history::HistoryStore::new(data.clone());
        let record = history.begin(std::path::Path::new(&vault), None)?;
        session_info(&data, &record)
    }

    pub fn mark_agent_session(
        &self,
        id: String,
        state: String,
        exit_code: Option<i32>,
    ) -> Result<(), CoreError> {
        let state = history::SessionState::parse(&state)
            .ok_or_else(|| CoreError::failure("history.invalid_state"))?;
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        let history = history::HistoryStore::new(store.data_root());
        match state {
            history::SessionState::Running => history.mark_running(&id)?,
            other => history.finish(&id, other, exit_code)?,
        };
        Ok(())
    }

    pub fn list_agent_sessions(
        &self,
        vault: Option<String>,
    ) -> Result<Vec<AgentSessionInfo>, CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data.clone());
        let canonical = vault
            .map(|vault| {
                std::fs::canonicalize(vault)
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(CoreError::from)
            })
            .transpose()?;
        let records = history.list(canonical.as_deref())?;
        records
            .iter()
            .map(|record| session_info(&data, record))
            .collect()
    }

    /// 应用启动时调用：上次仍标为准备中/运行中的会话转为中断。
    pub fn recover_agent_sessions(&self) -> Result<u32, CoreError> {
        let store = self.store.lock().unwrap_or_else(|error| error.into_inner());
        Ok(history::HistoryStore::new(store.data_root()).recover()? as u32)
    }

    /// 在后台任务中计算会话的全库改动摘要；不持有 Store 锁。
    pub fn review_agent_session(
        &self,
        id: String,
        cancellation: Arc<ReviewCancellation>,
    ) -> Result<SessionReviewSummary, CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data);
        let summary = history.review(&id, Some(cancellation.flag()))?;
        Ok(review_summary(summary))
    }

    /// 选中条目后再按需计算逐行 diff 或二进制/符号链接元数据。
    /// 路径只接受库内相对路径；`..`、绝对路径与空路径都拒绝。
    pub fn review_agent_session_entry(
        &self,
        id: String,
        path: String,
        old_path: Option<String>,
        cancellation: Arc<ReviewCancellation>,
    ) -> Result<ReviewEntryDetail, CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data);
        let detail =
            history.review_entry(&id, &path, old_path.as_deref(), Some(cancellation.flag()))?;
        Ok(review_detail(detail))
    }

    /// 按处回滚：hunk 内容必须与当前审阅结果一致；只写命中的文本文件。
    #[allow(clippy::too_many_arguments)]
    pub fn rollback_agent_session_hunk(
        &self,
        id: String,
        path: String,
        old_path: Option<String>,
        expectation: RollbackExpectationInfo,
        hunk_index: u32,
        hunk: ReviewDiffHunk,
        temporary: Option<String>,
        cancellation: Arc<ReviewCancellation>,
    ) -> Result<(), CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data);
        let record = history.record(&id)?;
        let temporary = rollback_temporary(std::path::Path::new(&record.vault), temporary)?;
        let expectation = rollback_expectation(&expectation)?;
        let expected_hunk = rollback_hunk(&hunk);
        history.rollback_hunk(
            &id,
            &path,
            old_path.as_deref(),
            &expectation,
            hunk_index as usize,
            &expected_hunk,
            &temporary.path,
            Some(cancellation.flag()),
        )?;
        Ok(())
    }

    /// 按文件回滚；`after` 指纹不匹配时返回 `history.review_stale`。
    pub fn rollback_agent_session_entry(
        &self,
        id: String,
        expectation: RollbackExpectationInfo,
        temporary: Option<String>,
        cancellation: Arc<ReviewCancellation>,
    ) -> Result<(), CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data);
        let record = history.record(&id)?;
        let temporary = rollback_temporary(std::path::Path::new(&record.vault), temporary)?;
        let expectation = rollback_expectation(&expectation)?;
        history.rollback_entry(
            &id,
            &expectation,
            &temporary.path,
            Some(cancellation.flag()),
        )?;
        Ok(())
    }

    /// 按会话回滚整批条目，逐项返回成功、stale 与失败；不隐藏部分失败。
    pub fn rollback_agent_session(
        &self,
        id: String,
        expectations: Vec<RollbackExpectationInfo>,
        temporary: Option<String>,
        cancellation: Arc<ReviewCancellation>,
    ) -> Result<SessionRollbackReportInfo, CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        let history = history::HistoryStore::new(data);
        let record = history.record(&id)?;
        let temporary = rollback_temporary(std::path::Path::new(&record.vault), temporary)?;
        let expectations = expectations
            .iter()
            .map(rollback_expectation)
            .collect::<Result<Vec<_>, _>>()?;
        let report = history.rollback_session(
            &id,
            &expectations,
            &temporary.path,
            Some(cancellation.flag()),
        )?;
        Ok(rollback_report(report))
    }

    /// 显式删除会话快照；活动会话（Preparing/Running）与用户库都不会被触碰。
    pub fn delete_agent_session(&self, id: String) -> Result<(), CoreError> {
        let data = self
            .store
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_root()
            .to_path_buf();
        history::HistoryStore::new(data).delete_session(&id)?;
        Ok(())
    }
}

fn tree_entry(entry: hortus_vault::Entry) -> TreeEntry {
    TreeEntry {
        path: entry.path,
        parent: entry.parent,
        name: entry.name,
        directory: entry.directory,
    }
}

#[derive(uniffi::Object)]
pub struct VaultHandle {
    vault: Vault,
    search: RwLock<SearchIndex>,
    links: RwLock<KnowledgeIndex>,
    index_update: Mutex<()>,
    rename_plans: Mutex<links::RenamePlans>,
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

    pub fn create_note(&self, parent: String, name: String) -> Result<TreeEntry, CoreError> {
        Ok(tree_entry(self.vault.create_note(&parent, &name)?))
    }

    pub fn create_folder(&self, parent: String, name: String) -> Result<TreeEntry, CoreError> {
        Ok(tree_entry(self.vault.create_folder(&parent, &name)?))
    }

    pub fn rename_entry(&self, path: String, new_name: String) -> Result<TreeEntry, CoreError> {
        Ok(tree_entry(self.vault.rename_entry(&path, &new_name)?))
    }

    /// 移到废纸篓前的安全目标解析；返回确认位于库内的绝对路径，目录与否由平台层复核。
    pub fn resolve_trash_target(&self, path: String) -> Result<String, CoreError> {
        Ok(self
            .vault
            .resolve_trash_target(&path)?
            .0
            .to_string_lossy()
            .into_owned())
    }

    pub fn update_search_index(&self, upserted: Vec<String>, removed: Vec<String>) {
        let _update = self
            .index_update
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.index_paths(upserted, removed);
    }

    pub fn upsert_note_content(&self, path: String, text: String) {
        let _update = self
            .index_update
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if hortus_vault::note::note_path(self.vault.root(), &path).is_ok() {
            self.index_contents([(path, Some(text))]);
        }
    }

    pub fn quick_open(&self, query: String, limit: u32) -> Vec<QuickOpenHit> {
        self.search
            .read()
            .unwrap_or_else(|error| error.into_inner())
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
            .read()
            .unwrap_or_else(|error| error.into_inner())
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
