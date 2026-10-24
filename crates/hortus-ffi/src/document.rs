//! 文档会话负责增量镜像与保存状态，平台只提交原生编辑范围和用户的冲突选择。
use crate::{CoreError, WorkspaceCore};
use hortus_document::{
    Document,
    reading::{Model, Run},
};
use hortus_vault::note::{self, DiskVersion, Format, Recovery, WriteFailure};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DocumentIssue {
    None,
    Conflict,
    Missing,
    SaveFailed,
    Recovery,
}

const FULL_READING_MODEL: u64 = u64::MAX;
const EMPTY_READING_DELTA: u64 = u64::MAX - 1;

fn reading_model_is_compact(model: &Model) -> bool {
    model.blocks.iter().all(|block| {
        block.source.start <= u32::MAX as usize
            && block.source.len() <= u32::MAX as usize
            && block.content.display_len() <= u32::MAX as usize
            && block.content.runs.len() <= u32::MAX as usize
            && block.content.runs.iter().all(|run| {
                run.display.start <= u32::MAX as usize
                    && run.display.len() <= u32::MAX as usize
                    && run.source.start <= u32::MAX as usize
                    && run.source.len() <= u32::MAX as usize
            })
    })
}

fn pack_reading_runs(runs: &[Run], compact: bool) -> Vec<u8> {
    let record_size = if compact { 20 } else { 40 };
    let version = if compact { 2 } else { 1 };
    let mut packed = Vec::with_capacity(8 + runs.len() * record_size);
    push_header(&mut packed, version, runs.len());
    for run in runs {
        if compact {
            push_u32(&mut packed, run.display.start as u32);
            push_u32(&mut packed, run.display.len() as u32);
            push_u32(&mut packed, run.source.start as u32);
            push_u32(&mut packed, run.source.len() as u32);
            push_u32(&mut packed, run.mask);
        } else {
            push_usize(&mut packed, run.display.start);
            push_usize(&mut packed, run.display.len());
            push_usize(&mut packed, run.source.start);
            push_usize(&mut packed, run.source.len());
            push_u64(&mut packed, u64::from(run.mask));
        }
    }
    packed
}

fn pack_full_reading_model(model: &Model, revision: u64) -> ReadingModel {
    let block_count = model.blocks.len();
    let run_count = model
        .blocks
        .iter()
        .map(|block| block.content.runs.len())
        .sum::<usize>();
    let text_bytes = model
        .blocks
        .iter()
        .map(|block| block.content.text.len())
        .sum::<usize>();
    let compact = reading_model_is_compact(model);
    let (block_record_size, run_record_size, version) =
        if compact { (36, 20, 2) } else { (64, 40, 1) };
    let mut text = String::with_capacity(text_bytes);
    let mut packed_blocks = Vec::with_capacity(8 + block_count * block_record_size);
    let mut packed_runs = Vec::with_capacity(8 + run_count * run_record_size);
    push_header(&mut packed_blocks, version, block_count);
    push_header(&mut packed_runs, version, run_count);
    let mut display_offset = 0usize;
    let mut run_offset = 0usize;
    for block in &model.blocks {
        let display_length = block.content.display_len();
        if compact {
            push_u64(&mut packed_blocks, block.id);
            push_u32(&mut packed_blocks, block.source.start as u32);
            push_u32(&mut packed_blocks, block.source.len() as u32);
            push_u32(&mut packed_blocks, display_offset as u32);
            push_u32(&mut packed_blocks, display_length as u32);
            push_u32(&mut packed_blocks, run_offset as u32);
            push_u32(&mut packed_blocks, block.content.runs.len() as u32);
            push_u32(
                &mut packed_blocks,
                u32::from(u8::from(block.content.diagram)),
            );
        } else {
            push_u64(&mut packed_blocks, block.id);
            push_usize(&mut packed_blocks, block.source.start);
            push_usize(&mut packed_blocks, block.source.len());
            push_usize(&mut packed_blocks, display_offset);
            push_usize(&mut packed_blocks, display_length);
            push_usize(&mut packed_blocks, run_offset);
            push_usize(&mut packed_blocks, block.content.runs.len());
            push_usize(&mut packed_blocks, u8::from(block.content.diagram) as usize);
        }
        for run in &block.content.runs {
            if compact {
                push_u32(&mut packed_runs, run.display.start as u32);
                push_u32(&mut packed_runs, run.display.len() as u32);
                push_u32(&mut packed_runs, run.source.start as u32);
                push_u32(&mut packed_runs, run.source.len() as u32);
                push_u32(&mut packed_runs, run.mask);
            } else {
                push_usize(&mut packed_runs, run.display.start);
                push_usize(&mut packed_runs, run.display.len());
                push_usize(&mut packed_runs, run.source.start);
                push_usize(&mut packed_runs, run.source.len());
                push_u64(&mut packed_runs, u64::from(run.mask));
            }
        }
        text.push_str(&block.content.text);
        display_offset += display_length;
        run_offset += block.content.runs.len();
    }
    ReadingModel {
        revision,
        rebuilt: model.rebuilt as u64,
        text,
        packed_blocks,
        packed_runs,
        delta_index: FULL_READING_MODEL,
        block_id: 0,
        source_start: 0,
        source_length: 0,
        display_start: 0,
        display_length: 0,
        diagram: false,
    }
}

fn changed_reading_block(old: &Model, new: &Model) -> Option<Option<usize>> {
    if old.blocks.len() != new.blocks.len() {
        return None;
    }
    let changed = old
        .blocks
        .iter()
        .zip(&new.blocks)
        .enumerate()
        .filter_map(|(index, (before, after))| (before.id != after.id).then_some(index))
        .collect::<Vec<_>>();
    if changed.len() <= 1 {
        Some(changed.into_iter().next())
    } else {
        None
    }
}

fn pack_reading_delta(old: &Model, new: &Model, revision: u64) -> Option<ReadingModel> {
    let index = changed_reading_block(old, new)?;
    let Some(index) = index else {
        return Some(ReadingModel {
            revision,
            rebuilt: new.rebuilt as u64,
            text: String::new(),
            packed_blocks: Vec::new(),
            packed_runs: Vec::new(),
            delta_index: EMPTY_READING_DELTA,
            block_id: 0,
            source_start: 0,
            source_length: 0,
            display_start: 0,
            display_length: 0,
            diagram: false,
        });
    };
    let compact = reading_model_is_compact(new);
    if compact != reading_model_is_compact(old) {
        return None;
    }
    let block = &new.blocks[index];
    let display_start = new.blocks[..index]
        .iter()
        .map(|block| block.content.display_len())
        .sum::<usize>();
    Some(ReadingModel {
        revision,
        rebuilt: new.rebuilt as u64,
        text: block.content.text.clone(),
        packed_blocks: Vec::new(),
        packed_runs: pack_reading_runs(&block.content.runs, compact),
        delta_index: index as u64,
        block_id: block.id,
        source_start: block.source.start as u64,
        source_length: block.source.len() as u64,
        display_start: display_start as u64,
        display_length: block.content.display_len() as u64,
        diagram: block.content.diagram,
    })
}
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum SaveChoice {
    Normal,
    KeepMine,
    Recreate,
    ConflictCopy,
}
#[derive(uniffi::Record)]
pub struct DocumentUpdate {
    pub revision: u64,
    pub text: Option<String>,
    pub dirty: bool,
    pub read_only_reason: Option<String>,
    pub issue: DocumentIssue,
    pub message: String,
    pub copy_path: Option<String>,
    pub save_micros: u64,
}
#[derive(uniffi::Record)]
pub struct DocumentBlockRange {
    pub ordinal: u32,
    pub start: u64,
    pub length: u64,
}
#[derive(uniffi::Record)]
pub struct DocumentSnapshot {
    pub revision: u64,
    pub text: String,
    pub blocks: Vec<DocumentBlockRange>,
}
#[derive(uniffi::Record)]
pub struct DocumentHeading {
    pub level: u8,
    pub title: String,
    pub start: u64,
}
#[derive(uniffi::Record)]
pub struct DocumentOutline {
    pub revision: u64,
    pub headings: Vec<DocumentHeading>,
}
/// 按小端序连续编码 start:u64、length:u64、语义 mask:u32（每段 20 字节）。
/// 使用 UniFFI 的 bytes 批量传递，避免大型笔记逐字段桥接数十万条记录。
#[derive(uniffi::Record)]
pub struct SourceStyles {
    pub revision: u64,
    pub packed_runs: Vec<u8>,
    pub analysis_micros: u64,
}
#[derive(uniffi::Record)]
pub struct ReadingModel {
    pub revision: u64,
    pub rebuilt: u64,
    pub text: String,
    pub packed_blocks: Vec<u8>,
    pub packed_runs: Vec<u8>,
    /// u64::MAX 表示完整模型；u64::MAX - 1 表示没有变化；其它值表示只替换这一块。
    pub delta_index: u64,
    pub block_id: u64,
    pub source_start: u64,
    pub source_length: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub diagram: bool,
}

fn push_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_usize(buffer: &mut Vec<u8>, value: usize) {
    push_u64(buffer, value as u64);
}

fn push_header(buffer: &mut Vec<u8>, version: u32, count: usize) {
    buffer.extend_from_slice(&version.to_le_bytes());
    buffer.extend_from_slice(&(count as u32).to_le_bytes());
}

pub(crate) struct State {
    pub(crate) document: Document,
    pub(crate) root: PathBuf,
    pub(crate) relative: String,
    path: PathBuf,
    baseline: Option<DiskVersion>,
    format: Format,
    permissions: Option<fs::Permissions>,
    recovery: Recovery,
    pending_recovery: Option<(String, Format)>,
    pending_external: Option<PendingExternal>,
    pub(crate) reading_snapshot: Option<(u64, Model)>,
    pub(crate) image_resolver: hortus_document::media::ImageResolver,
    read_only: Option<String>,
    dirty: bool,
    issue: DocumentIssue,
    message: String,
}
struct PendingExternal {
    disk: DiskVersion,
    format: Format,
    text: String,
    valid: bool,
}
impl State {
    fn propose(&mut self, disk: DiskVersion) -> DocumentUpdate {
        let (format, text, valid) = Format::decode(&disk.bytes);
        let mut update = self.update(Some(text.clone()));
        update.read_only_reason = if !valid {
            Some("readonly.invalid_utf8".into())
        } else if self.path.exists() && !note::writable(&self.path) {
            Some("readonly.no_write_permission".into())
        } else {
            None
        };
        self.pending_external = Some(PendingExternal {
            disk,
            format,
            text,
            valid,
        });
        update
    }
    fn update(&self, text: Option<String>) -> DocumentUpdate {
        DocumentUpdate {
            revision: self.document.revision(),
            text,
            dirty: self.dirty,
            read_only_reason: self.read_only.clone(),
            issue: self.issue,
            message: self.message.clone(),
            copy_path: None,
            save_micros: 0,
        }
    }
    fn checkpoint(&mut self) -> Result<(), CoreError> {
        if self.dirty || matches!(self.issue, DocumentIssue::Conflict | DocumentIssue::Missing) {
            self.recovery
                .write_note(&self.document.text()?, &self.format)?;
        }
        Ok(())
    }
    fn reload(
        &mut self,
        pending: PendingExternal,
        return_text: bool,
    ) -> Result<DocumentUpdate, CoreError> {
        let PendingExternal {
            disk,
            format,
            text,
            valid,
        } = pending;
        self.format = format;
        self.baseline = Some(disk);
        self.permissions = fs::metadata(&self.path).ok().map(|m| m.permissions());
        self.read_only = if !valid {
            Some("readonly.invalid_utf8".into())
        } else if !note::writable(&self.path) {
            Some("readonly.no_write_permission".into())
        } else {
            None
        };
        self.document.reset(&text);
        self.dirty = false;
        self.issue = DocumentIssue::None;
        self.message.clear();
        self.pending_recovery = None;
        self.recovery.clear()?;
        Ok(self.update(return_text.then_some(text)))
    }
    fn conflict(&mut self, missing: bool) -> Result<DocumentUpdate, CoreError> {
        self.issue = if missing {
            DocumentIssue::Missing
        } else {
            DocumentIssue::Conflict
        };
        self.message = if missing {
            "document.missing"
        } else {
            "document.conflict"
        }
        .into();
        self.checkpoint()?;
        Ok(self.update(None))
    }
}

#[derive(uniffi::Object)]
pub struct DocumentSession {
    pub(crate) state: Mutex<State>,
}

#[uniffi::export]
impl WorkspaceCore {
    pub fn open_document(
        &self,
        vault: String,
        path: String,
    ) -> Result<Arc<DocumentSession>, CoreError> {
        let store = self.store.lock().unwrap();
        let root = fs::canonicalize(vault)?;
        store.validate_vault(&root)?;
        let absolute = note::note_path(&root, &path)?;
        let recovery = Recovery::new(&store.directories.cache, &root, &path);
        let recovered = recovery.load_note()?;
        let baseline = note::read(&absolute)?;
        if baseline.is_none() && recovered.is_none() {
            return Err(CoreError::failure("document.missing"));
        }
        let (format, text, valid) = Format::decode(
            baseline
                .as_ref()
                .map(|b| b.bytes.as_slice())
                .unwrap_or_default(),
        );
        let read_only = if !valid {
            Some("readonly.invalid_utf8".into())
        } else if baseline.is_some() && !note::writable(&absolute) {
            Some("readonly.no_write_permission".into())
        } else {
            None
        };
        let pending_recovery = recovered.filter(|(saved, _)| saved != &text || baseline.is_none());
        if pending_recovery.is_none() {
            recovery.clear()?;
        }
        let issue = if pending_recovery.is_some() {
            DocumentIssue::Recovery
        } else {
            DocumentIssue::None
        };
        Ok(Arc::new(DocumentSession {
            state: Mutex::new(State {
                document: Document::new(&text),
                root,
                relative: path,
                path: absolute.clone(),
                baseline,
                format,
                permissions: fs::metadata(absolute).ok().map(|m| m.permissions()),
                recovery,
                pending_recovery,
                pending_external: None,
                reading_snapshot: None,
                image_resolver: Default::default(),
                read_only,
                dirty: false,
                issue,
                message: if issue == DocumentIssue::Recovery {
                    "document.recovery".into()
                } else {
                    String::new()
                },
            }),
        }))
    }
}

#[uniffi::export]
impl DocumentSession {
    pub fn outline(&self) -> Result<DocumentOutline, CoreError> {
        let mut state = self.state.lock().unwrap();
        let mmd = state.relative.to_lowercase().ends_with(".mmd");
        let headings = state
            .document
            .outline(mmd)?
            .iter()
            .map(|heading| DocumentHeading {
                level: heading.level,
                title: heading.title.clone(),
                start: heading.start as u64,
            })
            .collect();
        Ok(DocumentOutline {
            revision: state.document.revision(),
            headings,
        })
    }
    pub fn reading_model(&self) -> Result<ReadingModel, CoreError> {
        self.reading_model_since(u64::MAX)
    }

    pub fn reading_model_since(&self, since: u64) -> Result<ReadingModel, CoreError> {
        let mut state = self.state.lock().unwrap();
        let mmd = state.relative.to_lowercase().ends_with(".mmd");
        let model = state.document.reading(mmd)?;
        let revision = state.document.revision();
        let previous = state.reading_snapshot.take();
        let result = if since != FULL_READING_MODEL {
            previous
                .as_ref()
                .filter(|(previous_revision, _)| *previous_revision == since)
                .and_then(|(_, previous_model)| {
                    pack_reading_delta(previous_model, &model, revision)
                })
                .unwrap_or_else(|| pack_full_reading_model(&model, revision))
        } else {
            pack_full_reading_model(&model, revision)
        };
        state.reading_snapshot = Some((revision, model));
        Ok(result)
    }
    pub fn source_styles(&self, since: u64, force: bool) -> Result<SourceStyles, CoreError> {
        let mut state = self.state.lock().unwrap();
        let started = Instant::now();
        let runs = state.document.style_changes(since, force)?;
        let analysis_micros = started.elapsed().as_micros() as u64;
        let mut packed_runs = Vec::with_capacity(runs.len() * 20);
        for run in runs {
            packed_runs.extend_from_slice(&(run.range.start as u64).to_le_bytes());
            packed_runs.extend_from_slice(&(run.range.len() as u64).to_le_bytes());
            packed_runs.extend_from_slice(&run.mask.to_le_bytes());
        }
        Ok(SourceStyles {
            revision: state.document.revision(),
            packed_runs,
            analysis_micros,
        })
    }

    pub fn initial(&self) -> Result<DocumentUpdate, CoreError> {
        let state = self.state.lock().unwrap();
        Ok(state.update(Some(state.document.text()?)))
    }
    pub fn replace(&self, start: u64, length: u64, replacement: String) -> Result<u64, CoreError> {
        let mut state = self.state.lock().unwrap();
        if state.read_only.is_some() {
            return Err(CoreError::failure("document.read_only"));
        }
        let revision = state
            .document
            .replace(start as usize, length as usize, &replacement)?;
        state.dirty = true;
        Ok(revision)
    }
    pub fn snapshot(&self) -> Result<DocumentSnapshot, CoreError> {
        let state = self.state.lock().unwrap();
        let text = state.document.text()?;
        let blocks = if state.relative.to_lowercase().ends_with(".mmd") {
            vec![DocumentBlockRange {
                ordinal: 1,
                start: 0,
                length: text.encode_utf16().count() as u64,
            }]
        } else {
            Document::mermaid_ranges(&text)
                .into_iter()
                .map(|(ordinal, start, length)| DocumentBlockRange {
                    ordinal,
                    start,
                    length,
                })
                .collect()
        };
        Ok(DocumentSnapshot {
            revision: state.document.revision(),
            text,
            blocks,
        })
    }
    pub fn failed_save(&self, message: String) -> DocumentUpdate {
        let mut state = self.state.lock().unwrap();
        state.issue = DocumentIssue::SaveFailed;
        state.message = if message.starts_with("io.") {
            message
        } else {
            "document.save_failed".into()
        };
        let _ = state.checkpoint();
        state.update(None)
    }
    pub fn checkpoint(&self) -> Result<(), CoreError> {
        self.state.lock().unwrap().checkpoint()
    }
    pub fn refresh(&self) -> Result<DocumentUpdate, CoreError> {
        let mut state = self.state.lock().unwrap();
        let _ = note::note_path(&state.root, &state.relative)?;
        let disk = note::read(&state.path)?;
        if note::same_version(state.baseline.as_ref(), disk.as_ref()) {
            return Ok(state.update(None));
        }
        if state.dirty || state.pending_recovery.is_some() {
            return state.conflict(disk.is_none());
        }
        match disk {
            Some(disk) => Ok(state.propose(disk)),
            None => state.conflict(true),
        }
    }
    /// 监听到外部改动后，在同一条文档队列中完成读取、冲突判断和载入，避免阅读视图再排一次队。
    pub fn refresh_and_accept(&self) -> Result<DocumentUpdate, CoreError> {
        let mut state = self.state.lock().unwrap();
        let _ = note::note_path(&state.root, &state.relative)?;
        let disk = note::read(&state.path)?;
        if note::same_version(state.baseline.as_ref(), disk.as_ref()) {
            return Ok(state.update(None));
        }
        if state.dirty || state.pending_recovery.is_some() {
            return state.conflict(disk.is_none());
        }
        let Some(disk) = disk else {
            return state.conflict(true);
        };
        let (format, text, valid) = Format::decode(&disk.bytes);
        state.reload(
            PendingExternal {
                disk,
                format,
                text,
                valid,
            },
            true,
        )
    }
    pub fn use_disk(&self) -> Result<DocumentUpdate, CoreError> {
        let mut state = self.state.lock().unwrap();
        let _ = note::note_path(&state.root, &state.relative)?;
        match note::read(&state.path)? {
            Some(disk) => Ok(state.propose(disk)),
            None => state.conflict(true),
        }
    }
    pub fn accept_external(&self, revision: u64) -> Result<DocumentUpdate, CoreError> {
        let mut state = self.state.lock().unwrap();
        if state.document.revision() != revision {
            return state.conflict(false);
        }
        match state.pending_external.take() {
            Some(pending) => state.reload(pending, false),
            None => Ok(state.update(None)),
        }
    }
    pub fn recover(&self, restore: bool) -> Result<DocumentUpdate, CoreError> {
        let mut state = self.state.lock().unwrap();
        if restore {
            if state.read_only.is_some() {
                return Err(CoreError::failure("document.recovery_read_only"));
            }
            if let Some((text, format)) = state.pending_recovery.take() {
                state.format = format;
                state.document.reset(&text);
                state.dirty = true;
                state.issue = DocumentIssue::None;
                state.message.clear();
                return Ok(state.update(Some(text)));
            }
        } else {
            state.pending_recovery = None;
            state.recovery.clear()?;
        }
        state.issue = DocumentIssue::None;
        state.message.clear();
        Ok(state.update(None))
    }
    pub fn save(&self, temporary: String, choice: SaveChoice) -> Result<DocumentUpdate, CoreError> {
        let started = Instant::now();
        let mut state = self.state.lock().unwrap();
        let result = (|| {
            if state.read_only.is_some() || state.pending_recovery.is_some() {
                return Ok(state.update(None));
            }
            if matches!(choice, SaveChoice::Normal)
                && matches!(
                    state.issue,
                    DocumentIssue::Conflict | DocumentIssue::Missing
                )
            {
                state.checkpoint()?;
                return Ok(state.update(None));
            }
            if !state.dirty && state.issue != DocumentIssue::Missing {
                return Ok(state.update(None));
            }
            state.checkpoint()?;
            let _ = note::note_path(&state.root, &state.relative)?;
            let text = state.document.text()?;
            let bytes = state.format.encode(&text);
            let temp = PathBuf::from(temporary);
            if matches!(choice, SaveChoice::ConflictCopy) {
                let stem = state.path.file_stem().unwrap().to_string_lossy();
                let ext = state.path.extension().unwrap_or_default().to_string_lossy();
                for index in 1..=10000 {
                    let suffix = if index == 1 {
                        String::new()
                    } else {
                        format!(" {index}")
                    };
                    let filename = format!("{stem} (conflict copy){suffix}.{ext}");
                    let path = state.path.with_file_name(filename);
                    match note::atomic_replace(
                        &path,
                        &bytes,
                        None,
                        &temp,
                        &state.root,
                        state.permissions.clone(),
                    ) {
                        Ok(_) => {
                            let relative = path
                                .strip_prefix(&state.root)
                                .unwrap()
                                .to_string_lossy()
                                .into_owned();
                            let disk = note::read(&state.path)?;
                            let mut update = if let Some(disk) = disk {
                                state.propose(disk)
                            } else {
                                state.dirty = false;
                                state.issue = DocumentIssue::Missing;
                                state.recovery.clear()?;
                                state.update(None)
                            };
                            update.copy_path = Some(relative);
                            return Ok(update);
                        }
                        Err(WriteFailure::Changed) => continue,
                        Err(WriteFailure::Missing) => return state.conflict(true),
                        Err(WriteFailure::Io(error)) => return Err(error.into()),
                    }
                }
                return Err(CoreError::failure("document.conflict_copy_exhausted"));
            }
            let baseline = match choice {
                SaveChoice::Normal => state.baseline.clone(),
                SaveChoice::KeepMine => note::read(&state.path)?,
                SaveChoice::Recreate => None,
                SaveChoice::ConflictCopy => unreachable!(),
            };
            match note::atomic_replace(
                &state.path,
                &bytes,
                baseline.as_ref(),
                &temp,
                &state.root,
                state.permissions.clone(),
            ) {
                Ok(version) => {
                    state.baseline = Some(version);
                    state.dirty = false;
                    state.issue = DocumentIssue::None;
                    state.message.clear();
                    state.recovery.clear()?;
                    Ok(state.update(None))
                }
                Err(WriteFailure::Changed) => state.conflict(false),
                Err(WriteFailure::Missing) => state.conflict(true),
                Err(WriteFailure::Io(error)) => Err(error.into()),
            }
        })();
        let mut update = match result {
            Ok(update) => update,
            Err(error) => {
                state.issue = DocumentIssue::SaveFailed;
                let _ = error;
                state.message = "document.save_failed".into();
                let _ = state.checkpoint();
                state.update(None)
            }
        };
        update.save_micros = started.elapsed().as_micros() as u64;
        Ok(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    struct Fixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
        temporary: PathBuf,
        core: Arc<WorkspaceCore>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("vault");
            let temporary = directory.path().join("replacement");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&temporary).unwrap();
            let core = WorkspaceCore::in_directories(
                directory.path().join("config").to_string_lossy().into(),
                directory.path().join("cache").to_string_lossy().into(),
            )
            .unwrap();
            Self {
                _directory: directory,
                root,
                temporary,
                core,
            }
        }
        fn open(&self, name: &str) -> Arc<DocumentSession> {
            self.core
                .open_document(self.root.to_string_lossy().into(), name.into())
                .unwrap()
        }
        fn save(&self, session: &DocumentSession, choice: SaveChoice) -> DocumentUpdate {
            session
                .save(self.temporary.to_string_lossy().into(), choice)
                .unwrap()
        }
    }

    #[test]
    fn self_save_external_proposal_race_final_check_and_missing_recreate() {
        let f = Fixture::new();
        let path = f.root.join("note.md");
        fs::write(&path, "原文\n").unwrap();
        let s = f.open("note.md");
        s.replace(0, 0, "本地".into()).unwrap();
        assert_eq!(f.save(&s, SaveChoice::Normal).issue, DocumentIssue::None);
        assert!(s.refresh().unwrap().text.is_none());
        fs::write(&path, "外部\n").unwrap();
        let proposal = s.refresh().unwrap();
        assert!(proposal.text.is_some());
        // 读取外部版本后仍有新键入时，原生层拒绝接受，核心也再次核对修订号。
        s.replace(0, 0, "后来".into()).unwrap();
        assert_eq!(
            s.accept_external(proposal.revision).unwrap().issue,
            DocumentIssue::Conflict
        );
        assert!(s.snapshot().unwrap().text.starts_with("后来"));
        assert_eq!(
            f.save(&s, SaveChoice::Normal).issue,
            DocumentIssue::Conflict
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "外部\n");
        assert_eq!(f.save(&s, SaveChoice::KeepMine).issue, DocumentIssue::None);
        s.replace(0, 0, "改动".into()).unwrap();
        fs::write(&path, "刚刚改写").unwrap();
        assert_eq!(
            f.save(&s, SaveChoice::Normal).issue,
            DocumentIssue::Conflict
        );
        let disk = s.use_disk().unwrap();
        s.accept_external(disk.revision).unwrap();
        assert_eq!(s.snapshot().unwrap().text, "刚刚改写");
        fs::remove_file(&path).unwrap();
        assert_eq!(s.refresh().unwrap().issue, DocumentIssue::Missing);
        assert_eq!(f.save(&s, SaveChoice::Recreate).issue, DocumentIssue::None);
        assert_eq!(fs::read_to_string(&path).unwrap(), "刚刚改写");
    }

    #[test]
    fn failure_recovery_survives_restart_and_retry_removes_recovery() {
        let f = Fixture::new();
        let path = f.root.join("note.md");
        fs::write(&path, "原文").unwrap();
        let s = f.open("note.md");
        s.replace(0, 0, "未存😀".into()).unwrap();
        for target in [&path, &f.root] {
            fs::set_permissions(
                target,
                fs::Permissions::from_mode(if target == &path { 0o444 } else { 0o555 }),
            )
            .unwrap();
            let update = f.save(&s, SaveChoice::Normal);
            assert_eq!(update.issue, DocumentIssue::SaveFailed);
            assert!(update.dirty);
            assert_eq!(fs::read_to_string(&path).unwrap(), "原文");
            fs::set_permissions(
                target,
                fs::Permissions::from_mode(if target == &path { 0o644 } else { 0o755 }),
            )
            .unwrap();
        }
        let reopened = f.open("note.md");
        assert_eq!(reopened.initial().unwrap().issue, DocumentIssue::Recovery);
        reopened.recover(true).unwrap();
        assert_eq!(reopened.snapshot().unwrap().text, "未存😀原文");
        assert_eq!(
            f.save(&reopened, SaveChoice::Normal).issue,
            DocumentIssue::None
        );
        let again = f.open("note.md");
        assert_eq!(again.initial().unwrap().issue, DocumentIssue::None);
        assert_eq!(again.snapshot().unwrap().text, "未存😀原文");
        assert_eq!(fs::read_dir(&f.root).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&f.temporary).unwrap().count(), 0);
    }

    #[test]
    fn read_only_encoding_permissions_and_recovery_discard() {
        let f = Fixture::new();
        let path = f.root.join("note.md");
        fs::write(&path, [0xff, 0xfe, 65]).unwrap();
        let s = f.open("note.md");
        assert!(s.initial().unwrap().read_only_reason.is_some());
        assert!(s.replace(0, 0, "bad".into()).is_err());
        fs::write(&path, "原文").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(
            f.open("note.md")
                .initial()
                .unwrap()
                .read_only_reason
                .is_some()
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let s = f.open("note.md");
        s.replace(0, 0, "恢复".into()).unwrap();
        s.checkpoint().unwrap();
        let reopened = f.open("note.md");
        assert_eq!(reopened.initial().unwrap().issue, DocumentIssue::Recovery);
        reopened.recover(false).unwrap();
        assert_eq!(
            f.open("note.md").initial().unwrap().issue,
            DocumentIssue::None
        );
    }

    #[test]
    fn deleted_clean_note_can_recover_after_restart_with_original_encoding() {
        let f = Fixture::new();
        let path = f.root.join("note.md");
        let original = "\u{feff}正文\r\n末尾".as_bytes();
        fs::write(&path, original).unwrap();
        let session = f.open("note.md");
        fs::remove_file(&path).unwrap();
        assert_eq!(session.refresh().unwrap().issue, DocumentIssue::Missing);
        let reopened = f.open("note.md");
        assert_eq!(reopened.initial().unwrap().issue, DocumentIssue::Recovery);
        reopened.recover(true).unwrap();
        assert_eq!(
            f.save(&reopened, SaveChoice::Normal).issue,
            DocumentIssue::None
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }
}
