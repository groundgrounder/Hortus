//! Agent 会话历史：启动前全库快照与持久会话元数据。
//!
//! 会话目录在 `<data>/agent-sessions/<UUID>/`，不是可删除缓存。元数据每次
//! 原子替换；只有快照完整结束后才标记为 `ready`。
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub mod snapshot;

pub const FORMAT_VERSION: u32 = 1;
pub const SESSIONS_DIRECTORY: &str = "agent-sessions";
const METADATA_FILE: &str = "session.json";
const MANIFEST_FILE: &str = "manifest.json";
const SNAPSHOT_DIRECTORY: &str = "snapshot";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// 快照仍在建立；不可审阅、不可启动。
    Preparing,
    /// 快照完整；等待 exec。
    Ready,
    /// Agent 进程已启动。
    Running,
    /// Agent 已退出，`exit_code` 记录结果。
    Exited,
    /// 应用重启后发现记录仍是运行中。
    Interrupted,
    /// 启动在 exec 前被标签、库或退出取消。
    Cancelled,
    /// 配置、快照或 exec 失败。
    Failed,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "preparing" => Self::Preparing,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "exited" => Self::Exited,
            "interrupted" => Self::Interrupted,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    /// 只有 `ready`、`running`、`interrupted`、`exited` 代表快照完整可用。
    pub fn usable(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Running | Self::Interrupted | Self::Exited
        )
    }

    /// 终态不再接受 running 或其它终态写入。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Interrupted | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub bytes: u64,
    pub cloned: u64,
    pub copied: u64,
    pub elapsed_ms: u64,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub format: u32,
    pub id: String,
    pub vault: String,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    pub state: SessionState,
    pub snapshot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<SnapshotStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct HistoryStore {
    data: PathBuf,
}

impl HistoryStore {
    pub fn new(data: impl Into<PathBuf>) -> Self {
        Self { data: data.into() }
    }

    pub fn data_root(&self) -> &Path {
        &self.data
    }

    pub fn sessions_directory(&self) -> PathBuf {
        self.data.join(SESSIONS_DIRECTORY)
    }

    pub fn session_directory(&self, id: &str) -> io::Result<PathBuf> {
        if !valid_id(id) {
            return Err(io::Error::other("history.invalid_session"));
        }
        Ok(self.sessions_directory().join(id))
    }

    pub fn snapshot_directory(&self, id: &str) -> io::Result<PathBuf> {
        Ok(self.session_directory(id)?.join(SNAPSHOT_DIRECTORY))
    }

    pub fn manifest_path(&self, id: &str) -> io::Result<PathBuf> {
        Ok(self.session_directory(id)?.join(MANIFEST_FILE))
    }

    /// 建立独立会话目录，完成完整快照后才发布 `ready`。
    ///
    /// `cancel` 在每个条目处理前检查；取消时快照作废并标记 `cancelled`，
    /// 迟到的完成结果不会变成可启动会话。
    pub fn begin(&self, vault: &Path, cancel: Option<&AtomicBool>) -> io::Result<SessionRecord> {
        fs::create_dir_all(self.sessions_directory())?;
        fs::set_permissions(self.sessions_directory(), fs::Permissions::from_mode(0o700))?;
        let vault = fs::canonicalize(vault)?;
        let id = new_session_id()?;
        let directory = self.session_directory(&id)?;
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let mut record = SessionRecord {
            format: FORMAT_VERSION,
            id: id.clone(),
            vault: vault.to_string_lossy().into_owned(),
            created_at_ms: now_ms(),
            started_at_ms: None,
            ended_at_ms: None,
            state: SessionState::Preparing,
            snapshot: SNAPSHOT_DIRECTORY.into(),
            stats: None,
            exit_code: None,
            error: None,
        };
        self.write_record(&record)?;
        let started = Instant::now();
        let result = snapshot::create(
            &vault,
            &directory.join(SNAPSHOT_DIRECTORY),
            &directory.join(MANIFEST_FILE),
            cancel,
        );
        match result {
            Ok((stats, _manifest)) => {
                record.state = SessionState::Ready;
                record.stats = Some(SnapshotStats {
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    ..stats
                });
                if let Err(error) = self.write_record(&record) {
                    // ready 发布失败也必须留下不可用的 failed，不能停在 preparing。
                    self.record_failure(&record, &error, started);
                    return Err(error);
                }
                Ok(record)
            }
            Err(error) => {
                self.record_failure(&record, &error, started);
                Err(error)
            }
        }
    }

    /// 尽力把未发布的半成品写成不可用终态；写盘本身失败时保留原始错误。
    fn record_failure(&self, record: &SessionRecord, error: &io::Error, started: Instant) {
        let mut failed = record.clone();
        failed.state = if error.kind() == io::ErrorKind::Interrupted {
            SessionState::Cancelled
        } else {
            SessionState::Failed
        };
        failed.ended_at_ms = Some(now_ms());
        failed.error = Some(error.to_string());
        failed.stats = Some(SnapshotStats {
            elapsed_ms: started.elapsed().as_millis() as u64,
            ..SnapshotStats::default()
        });
        if let Err(write_error) = self.write_record(&failed) {
            eprintln!("Hortus：写入失败会话元数据失败：{write_error}");
        }
    }

    pub fn record(&self, id: &str) -> io::Result<SessionRecord> {
        let path = self.session_directory(id)?.join(METADATA_FILE);
        read_record(&path)
    }

    /// 只有完整快照发布出的 `ready` 才能进入 `running`；迟到的 running 不改终态。
    pub fn mark_running(&self, id: &str) -> io::Result<SessionRecord> {
        let record = self.record(id)?;
        if record.state != SessionState::Ready {
            return Ok(record);
        }
        self.update(id, |record| {
            record.state = SessionState::Running;
            record.started_at_ms = Some(now_ms());
        })
    }

    /// 终态单调：已经退出、中断、取消或失败的会话不再接受新的终态或 running。
    pub fn finish(
        &self,
        id: &str,
        state: SessionState,
        exit_code: Option<i32>,
    ) -> io::Result<SessionRecord> {
        let record = self.record(id)?;
        if record.state.is_terminal() {
            return Ok(record);
        }
        self.update(id, |record| {
            record.state = state;
            record.exit_code = exit_code;
            record.ended_at_ms = Some(now_ms());
        })
    }

    /// 应用重启后：只有完整且正在运行的会话转为中断。
    /// 准备中或缺少完整清单/统计的记录转为不可用的 failed。
    pub fn recover(&self) -> io::Result<u64> {
        let mut recovered = 0;
        for record in self.list(None)? {
            match record.state {
                SessionState::Running if self.snapshot_complete(&record) => {
                    self.update(&record.id, |record| {
                        record.state = SessionState::Interrupted;
                        record.ended_at_ms = Some(now_ms());
                    })?;
                    recovered += 1;
                }
                SessionState::Preparing | SessionState::Running => {
                    self.mark_failed(&record)?;
                }
                // 已发布的 ready 也必须仍有完整清单，否则不能冒充可用会话。
                SessionState::Ready if !self.snapshot_complete(&record) => {
                    self.mark_failed(&record)?;
                }
                _ => {}
            }
        }
        Ok(recovered)
    }

    fn mark_failed(&self, record: &SessionRecord) -> io::Result<()> {
        self.update(&record.id, |record| {
            record.state = SessionState::Failed;
            record.ended_at_ms = Some(now_ms());
            record.error = Some("history.incomplete_session".into());
        })?;
        Ok(())
    }

    /// 完整快照必须同时具备统计、快照目录与条目数一致的清单。
    fn snapshot_complete(&self, record: &SessionRecord) -> bool {
        let Some(stats) = record.stats.as_ref() else {
            return false;
        };
        let (Ok(directory), Ok(manifest)) = (
            self.snapshot_directory(&record.id),
            self.manifest_path(&record.id),
        ) else {
            return false;
        };
        let Ok(data) = fs::read(manifest) else {
            return false;
        };
        let Ok(entries) = serde_json::from_slice::<Vec<snapshot::ManifestEntry>>(&data) else {
            return false;
        };
        directory.is_dir() && entries.len() as u64 == stats.entries
    }

    /// 按创建时间倒序列出会话；可按规范化库根筛选。
    pub fn list(&self, vault: Option<&str>) -> io::Result<Vec<SessionRecord>> {
        let directory = self.sessions_directory();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let record = match read_record(&path.join(METADATA_FILE)) {
                Ok(record) => record,
                // 别的进程可能正在创建或替换元数据；跳过无法解析的目录。
                Err(_) => continue,
            };
            if vault.is_some_and(|vault| record.vault != vault) {
                continue;
            }
            records.push(record);
        }
        records.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(records)
    }

    fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut SessionRecord),
    ) -> io::Result<SessionRecord> {
        let mut record = self.record(id)?;
        change(&mut record);
        self.write_record(&record)?;
        Ok(record)
    }

    fn write_record(&self, record: &SessionRecord) -> io::Result<()> {
        let directory = self.session_directory(&record.id)?;
        fs::create_dir_all(&directory)?;
        let data = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        atomic_write(&directory.join(METADATA_FILE), &data)
    }
}

fn read_record(path: &Path) -> io::Result<SessionRecord> {
    let data = fs::read_to_string(path)?;
    serde_json::from_str(&data).map_err(|_| io::Error::other("history.invalid_metadata"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 读取系统随机源生成 UUID v4；不引入额外依赖。
fn new_session_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    let mut source = fs::File::open("/dev/urandom")?;
    io::Read::read_exact(&mut source, &mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// 单元测试用故障注入；只在测试构建存在。
#[cfg(test)]
pub(crate) static TEST_FAIL_WRITE: std::sync::Mutex<Option<fn(&Path) -> bool>> =
    std::sync::Mutex::new(None);

/// 元数据与清单共用原子写入：同目录临时文件、fsync、替换、目录 fsync。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(test)]
    if let Some(predicate) = *TEST_FAIL_WRITE.lock().unwrap() {
        if predicate(path) {
            return Err(io::Error::other("history.test_write_failed"));
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("history.path_invalid"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(io::Error::other)?;
    fs::File::open(parent)?.sync_all()
}

/// 供自检使用的取消标志；不参与生产状态。
pub struct Cancellation(AtomicBool);

impl Cancellation {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::sync::atomic::AtomicUsize;

    static METADATA_WRITES: AtomicUsize = AtomicUsize::new(0);

    fn fail_second_metadata(path: &Path) -> bool {
        if path.file_name() == Some(OsStr::new(METADATA_FILE)) {
            METADATA_WRITES.fetch_add(1, Ordering::SeqCst) == 1
        } else {
            false
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::TempDir::new().unwrap();
        let vault = root.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("note.md"), "# Note\n").unwrap();
        (root, vault)
    }

    #[test]
    fn publish_failures_leave_only_unusable_records() {
        let (root, vault) = fixture();
        let store = HistoryStore::new(root.path().join("data-manifest"));
        *TEST_FAIL_WRITE.lock().unwrap() =
            Some(|path| path.file_name() == Some(OsStr::new(MANIFEST_FILE)));
        let error = store.begin(&vault, None).unwrap_err();
        *TEST_FAIL_WRITE.lock().unwrap() = None;
        assert_eq!(error.to_string(), "history.test_write_failed");
        let sessions = store.list(None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, SessionState::Failed);
        assert!(
            sessions[0]
                .error
                .as_deref()
                .unwrap()
                .contains("test_write_failed")
        );
        assert!(!sessions[0].state.usable());

        // ready 元数据发布失败：第二次 session.json 写入失败，第三次尽力写 failed 成功。
        let (root, vault) = fixture();
        let store = HistoryStore::new(root.path().join("data-ready"));
        METADATA_WRITES.store(0, Ordering::SeqCst);
        *TEST_FAIL_WRITE.lock().unwrap() = Some(fail_second_metadata);
        let error = store.begin(&vault, None).unwrap_err();
        *TEST_FAIL_WRITE.lock().unwrap() = None;
        assert_eq!(error.to_string(), "history.test_write_failed");
        let sessions = store.list(None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, SessionState::Failed);
        assert!(!sessions[0].state.usable());
    }
}
