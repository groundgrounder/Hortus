//! 会话改动审阅：以 P1-33 全库快照为唯一基线，独立扫描当前库。
//!
//! 不接 Git；不跟随符号链接；任意层级的 `.git` 都跳过；覆盖隐藏文件、
//! 空目录、0 字节文件、图片等二进制附件与符号链接本身。文件内容判定
//! 不依赖时间戳：大小不同直接视为内容变化，大小相同也读取字节比较。
//! 列表只统计改动，逐行 diff 与内容详情在选中条目时按需计算。
use crate::HistoryStore;
use crate::snapshot::ManifestEntry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// 单文件逐行 diff 的文本上限；超过后只显示大文件摘要。
pub const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
/// 重命名并伴随编辑的配对只读取这个上限内的文本。
const MAX_RENAME_TEXT_BYTES: u64 = 1024 * 1024;
/// LCS 动态规划的单元上限；超过时退化为公共前后缀加整段替换。
const MAX_DIFF_CELLS: u64 = 4_000_000;
/// 单侧行数上限；超过时同样退化，避免巨型行集合拖慢界面。
const MAX_DIFF_LINES: usize = 20_000;
/// 逐行 diff 默认上下文行数。
pub const HUNK_CONTEXT: usize = 3;
const READ_CHUNK: usize = 64 * 1024;
const RENAME_SIMILARITY_MIN: f64 = 0.5;
const RENAME_SHARED_LINES_MIN: usize = 2;

#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(not(target_os = "macos"))]
const O_NOFOLLOW: i32 = 0x2_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::TypeChanged => "type_changed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeCounts {
    pub added: u64,
    pub modified: u64,
    pub deleted: u64,
    pub renamed: u64,
    pub type_changed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryMetadata {
    pub kind: String,
    pub size: u64,
    pub mode: u32,
    pub modified_ms: Option<i64>,
    pub link: Option<String>,
}

impl EntryMetadata {
    fn from_manifest(entry: &ManifestEntry) -> Self {
        Self {
            kind: entry.kind.clone(),
            size: entry.size,
            mode: entry.mode,
            modified_ms: entry.modified_ms,
            link: entry.link.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEntry {
    /// 展示与选中用的稳定标识：新路径优先，删除条目用旧路径。
    pub id: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change: ChangeKind,
    pub before: Option<EntryMetadata>,
    pub after: Option<EntryMetadata>,
    /// 存在的一侧都是文本时可用于逐行 diff。
    pub text: bool,
    /// 前后两侧内容逐字节相同（纯重命名或仅权限变化）。
    pub content_identical: bool,
}

impl ReviewEntry {
    pub fn display_path(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub session_id: String,
    pub vault: String,
    pub state: String,
    pub snapshot_at_ms: i64,
    pub counts: ChangeCounts,
    pub elapsed_ms: u64,
    pub entries: Vec<ReviewEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    /// `context`、`removed` 或 `added`。
    pub kind: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDetail {
    pub entry: ReviewEntry,
    pub text: bool,
    pub binary: bool,
    pub large: bool,
    pub content_identical: bool,
    pub old_no_final_newline: bool,
    pub new_no_final_newline: bool,
    pub line_endings_changed: bool,
    pub hunks: Vec<DiffHunk>,
}

/// 审阅入口。所有函数只读源库与快照目录，不向用户库写任何文件。
impl HistoryStore {
    /// 计算会话的全库改动摘要；列表不含逐行 diff。
    pub fn review(&self, id: &str, cancel: Option<&AtomicBool>) -> io::Result<ReviewSummary> {
        let started = Instant::now();
        let record = self.reviewable_record(id)?;
        let (snapshot, before) = self.load_snapshot(&record, cancel, true)?;
        let after = crate::snapshot::scan_manifest(Path::new(&record.vault), cancel)?;
        let after = index_manifest(after)?;
        let mut changes =
            compare_entries(Path::new(&record.vault), &snapshot, &before, &after, cancel)?;
        pair_renames(
            Path::new(&record.vault),
            &snapshot,
            &mut changes.entries,
            cancel,
        )?;
        changes
            .entries
            .sort_by(|a, b| a.id.as_bytes().cmp(b.id.as_bytes()));
        let counts = count_changes(&changes.entries);
        Ok(ReviewSummary {
            session_id: record.id.clone(),
            vault: record.vault.clone(),
            state: record.state.as_str().into(),
            snapshot_at_ms: record.created_at_ms.saturating_add(
                record
                    .stats
                    .as_ref()
                    .map(|stats| stats.elapsed_ms as i64)
                    .unwrap_or(0),
            ),
            counts,
            elapsed_ms: started.elapsed().as_millis() as u64,
            entries: changes.entries,
        })
    }

    /// 单条目详情：严格校验会话状态、相对路径与快照/当前边界后按需读取内容。
    pub fn review_entry(
        &self,
        id: &str,
        path: &str,
        old_path: Option<&str>,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<ReviewDetail> {
        let record = self.reviewable_record(id)?;
        let (snapshot, before) = self.load_snapshot(&record, cancel, false)?;
        let vault = Path::new(&record.vault);
        let (old_relative, new_relative) = match old_path {
            Some(old) if old != path => (Some(old), Some(path)),
            _ => (None, Some(path)),
        };
        if let Some(old) = old_relative {
            validate_relative(old)?;
        }
        validate_relative(path)?;
        check_cancel(cancel)?;

        let before_entry = match old_relative {
            Some(old) => Some(
                before
                    .get(old)
                    .ok_or_else(|| key_error("history.invalid_entry"))?,
            ),
            None => before.get(path),
        };
        let after_path = new_relative.unwrap();
        let after_entry = match disk_entry(vault, after_path) {
            Ok(entry) => Some(entry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if before_entry.is_none() && after_entry.is_none() {
            return Err(key_error("history.invalid_entry"));
        }
        check_cancel(cancel)?;

        let change = match (before_entry, after_entry.as_ref()) {
            (Some(before), Some(after)) if before.kind != after.kind => ChangeKind::TypeChanged,
            (Some(_), Some(_)) if old_relative.is_some() => ChangeKind::Renamed,
            (Some(_), Some(_)) => ChangeKind::Modified,
            (Some(_), None) => ChangeKind::Deleted,
            (None, Some(_)) => ChangeKind::Added,
            (None, None) => unreachable!(),
        };
        let before_path = old_relative.unwrap_or(path);
        let identical = match (before_entry, after_entry.as_ref()) {
            (Some(before), Some(after)) if before.kind == after.kind => same_content(
                vault,
                &snapshot,
                before,
                before_path,
                after,
                after_path,
                cancel,
            )?,
            _ => false,
        };

        let before_file = before_entry.filter(|entry| entry.kind == "file");
        let after_file = after_entry.as_ref().filter(|entry| entry.kind == "file");
        let large = before_file.is_some_and(|entry| entry.size > MAX_TEXT_BYTES)
            || after_file.is_some_and(|entry| entry.size > MAX_TEXT_BYTES);
        let mut text = false;
        let mut binary = false;
        let mut old_no_final_newline = false;
        let mut new_no_final_newline = false;
        let mut line_endings_changed = false;
        let mut hunks = Vec::new();

        if !large
            && change != ChangeKind::TypeChanged
            && (before_file.is_some() || after_file.is_some())
        {
            let old_text = before_file
                .map(|_| read_text(&snapshot, before_path, MAX_TEXT_BYTES, cancel))
                .transpose()?
                .flatten();
            let new_text = after_file
                .map(|_| read_text(vault, after_path, MAX_TEXT_BYTES, cancel))
                .transpose()?
                .flatten();
            let classified = before_file.is_none_or(|_| old_text.is_some())
                && after_file.is_none_or(|_| new_text.is_some());
            if classified {
                text = true;
                let (old_lines, old_missing) =
                    old_text.as_deref().map(text_lines).unwrap_or_default();
                let (new_lines, new_missing) =
                    new_text.as_deref().map(text_lines).unwrap_or_default();
                old_no_final_newline = old_missing;
                new_no_final_newline = new_missing;
                match change {
                    ChangeKind::Modified | ChangeKind::Renamed => {
                        if !identical {
                            line_endings_changed = old_text.as_deref() != new_text.as_deref()
                                && normalize_eol(old_text.as_deref().unwrap_or_default())
                                    == normalize_eol(new_text.as_deref().unwrap_or_default());
                            hunks = self::hunks(&old_lines, &new_lines, HUNK_CONTEXT);
                        }
                    }
                    ChangeKind::Added | ChangeKind::Deleted => {
                        hunks = self::hunks(&old_lines, &new_lines, HUNK_CONTEXT);
                    }
                    ChangeKind::TypeChanged => {}
                }
            } else {
                binary = true;
            }
        }

        let entry = ReviewEntry {
            id: path.to_string(),
            old_path: old_relative.map(str::to_string),
            new_path: after_entry.as_ref().map(|_| path.to_string()),
            change,
            before: before_entry.map(EntryMetadata::from_manifest),
            after: after_entry.as_ref().map(EntryMetadata::from_manifest),
            text,
            content_identical: identical,
        };
        Ok(ReviewDetail {
            entry,
            text,
            binary,
            large,
            content_identical: identical,
            old_no_final_newline,
            new_no_final_newline,
            line_endings_changed,
            hunks,
        })
    }

    fn reviewable_record(&self, id: &str) -> io::Result<crate::SessionRecord> {
        let record = self.record(id)?;
        if !record.state.usable() {
            return Err(key_error("history.session_unusable"));
        }
        Ok(record)
    }

    /// 读取并校验清单。`verify` 为真时逐条核对快照目录中的类型、大小与链接目标，
    /// 防止损坏或被清空的快照被误报成“全新增/全删除”。
    fn load_snapshot(
        &self,
        record: &crate::SessionRecord,
        cancel: Option<&AtomicBool>,
        verify: bool,
    ) -> io::Result<(PathBuf, BTreeMap<String, ManifestEntry>)> {
        let snapshot = self.snapshot_directory(&record.id)?;
        match fs::metadata(&snapshot) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(key_error("history.missing_snapshot")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(key_error("history.missing_snapshot"));
            }
            Err(error) => return Err(error),
        }
        let manifest_path = self.manifest_path(&record.id)?;
        let data = match fs::read(&manifest_path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(key_error("history.missing_manifest"));
            }
            Err(error) => return Err(error),
        };
        let entries: Vec<ManifestEntry> =
            serde_json::from_slice(&data).map_err(|_| key_error("history.invalid_manifest"))?;
        if let Some(stats) = record.stats.as_ref()
            && stats.entries != entries.len() as u64
        {
            return Err(key_error("history.snapshot_corrupt"));
        }
        let mut index = BTreeMap::new();
        for entry in entries {
            validate_relative(&entry.path)?;
            if !matches!(entry.kind.as_str(), "file" | "directory" | "symlink") {
                return Err(key_error("history.invalid_manifest"));
            }
            if index.insert(entry.path.clone(), entry).is_some() {
                return Err(key_error("history.invalid_manifest"));
            }
        }
        if verify {
            for entry in index.values() {
                check_cancel(cancel)?;
                let path = safe_join(&snapshot, &entry.path, "history.snapshot_corrupt")?;
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|_| key_error("history.snapshot_corrupt"))?;
                if kind_name(&metadata) != Some(entry.kind.as_str()) {
                    return Err(key_error("history.snapshot_corrupt"));
                }
                if entry.kind != "directory" && metadata.size() != entry.size {
                    return Err(key_error("history.snapshot_corrupt"));
                }
                if entry.kind == "symlink"
                    && fs::read_link(&path)
                        .ok()
                        .map(|target| target.to_string_lossy().into_owned())
                        != entry.link
                {
                    return Err(key_error("history.snapshot_corrupt"));
                }
            }
        }
        Ok((snapshot, index))
    }
}

struct Changes {
    entries: Vec<ReviewEntry>,
}

fn count_changes(entries: &[ReviewEntry]) -> ChangeCounts {
    let mut counts = ChangeCounts::default();
    for entry in entries {
        match entry.change {
            ChangeKind::Added => counts.added += 1,
            ChangeKind::Modified => counts.modified += 1,
            ChangeKind::Deleted => counts.deleted += 1,
            ChangeKind::Renamed => counts.renamed += 1,
            ChangeKind::TypeChanged => counts.type_changed += 1,
        }
    }
    counts
}

fn index_manifest(entries: Vec<ManifestEntry>) -> io::Result<BTreeMap<String, ManifestEntry>> {
    let mut index = BTreeMap::new();
    for entry in entries {
        validate_relative(&entry.path)?;
        if index.insert(entry.path.clone(), entry).is_some() {
            return Err(key_error("history.invalid_manifest"));
        }
    }
    Ok(index)
}

fn compare_entries(
    vault: &Path,
    snapshot: &Path,
    before: &BTreeMap<String, ManifestEntry>,
    after: &BTreeMap<String, ManifestEntry>,
    cancel: Option<&AtomicBool>,
) -> io::Result<Changes> {
    let mut paths = BTreeSet::new();
    paths.extend(before.keys().cloned());
    paths.extend(after.keys().cloned());
    let mut entries = Vec::new();
    let mut pending_deleted = Vec::new();
    let mut pending_added = Vec::new();
    for path in paths {
        check_cancel(cancel)?;
        match (before.get(&path), after.get(&path)) {
            (Some(old), Some(new)) => {
                if old.kind != new.kind {
                    entries.push(ReviewEntry {
                        id: path.clone(),
                        old_path: Some(path.clone()),
                        new_path: Some(path.clone()),
                        change: ChangeKind::TypeChanged,
                        before: Some(EntryMetadata::from_manifest(old)),
                        after: Some(EntryMetadata::from_manifest(new)),
                        text: false,
                        content_identical: false,
                    });
                    continue;
                }
                let identical = same_content(vault, snapshot, old, &path, new, &path, cancel)?;
                if identical && old.mode == new.mode {
                    continue;
                }
                entries.push(ReviewEntry {
                    id: path.clone(),
                    old_path: Some(path.clone()),
                    new_path: Some(path.clone()),
                    change: ChangeKind::Modified,
                    before: Some(EntryMetadata::from_manifest(old)),
                    after: Some(EntryMetadata::from_manifest(new)),
                    text: false,
                    content_identical: identical,
                });
            }
            (Some(old), None) => pending_deleted.push(old.clone()),
            (None, Some(new)) => pending_added.push(new.clone()),
            (None, None) => unreachable!(),
        }
    }
    for entry in &pending_deleted {
        entries.push(ReviewEntry {
            id: entry.path.clone(),
            old_path: Some(entry.path.clone()),
            new_path: None,
            change: ChangeKind::Deleted,
            before: Some(EntryMetadata::from_manifest(entry)),
            after: None,
            text: false,
            content_identical: false,
        });
    }
    for entry in &pending_added {
        entries.push(ReviewEntry {
            id: entry.path.clone(),
            old_path: None,
            new_path: Some(entry.path.clone()),
            change: ChangeKind::Added,
            before: None,
            after: Some(EntryMetadata::from_manifest(entry)),
            text: false,
            content_identical: false,
        });
    }
    Ok(Changes { entries })
}

/// 重命名配对：先内容完全相同的删+增（含同目标符号链接），再对文本做
/// 行集合相似度的一对一配对。删除/新增候选按路径排序，结果确定。
fn pair_renames(
    vault: &Path,
    snapshot: &Path,
    entries: &mut Vec<ReviewEntry>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    let mut deleted: Vec<ManifestEntry> = Vec::new();
    let mut added: Vec<ManifestEntry> = Vec::new();
    for entry in entries.iter() {
        match entry.change {
            ChangeKind::Deleted => deleted.push(manifest_from_review(entry)),
            ChangeKind::Added => added.push(manifest_from_review(entry)),
            _ => {}
        }
    }
    if deleted.is_empty() || added.is_empty() {
        return Ok(());
    }
    deleted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    added.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut used_deleted = BTreeSet::new();
    let mut used_added = BTreeSet::new();
    let mut pairs: Vec<(String, String, bool)> = Vec::new();
    pair_exact(
        vault,
        snapshot,
        &deleted,
        &added,
        &mut used_deleted,
        &mut used_added,
        &mut pairs,
        cancel,
    )?;
    pair_text(
        vault,
        snapshot,
        &deleted,
        &added,
        &used_deleted,
        &used_added,
        &mut pairs,
        cancel,
    )?;
    for (old, new, exact) in pairs {
        let Some(old_entry) = deleted.iter().find(|entry| entry.path == old) else {
            continue;
        };
        let Some(new_entry) = added.iter().find(|entry| entry.path == new) else {
            continue;
        };
        entries.retain(|entry| {
            !((entry.change == ChangeKind::Deleted && entry.id == old)
                || (entry.change == ChangeKind::Added && entry.id == new))
        });
        entries.push(ReviewEntry {
            id: new.clone(),
            old_path: Some(old.clone()),
            new_path: Some(new.clone()),
            change: ChangeKind::Renamed,
            before: Some(EntryMetadata::from_manifest(old_entry)),
            after: Some(EntryMetadata::from_manifest(new_entry)),
            text: false,
            content_identical: exact,
        });
    }
    Ok(())
}

fn manifest_from_review(entry: &ReviewEntry) -> ManifestEntry {
    let metadata = entry
        .before
        .as_ref()
        .or(entry.after.as_ref())
        .expect("change entry has metadata");
    ManifestEntry {
        path: entry
            .old_path
            .clone()
            .or_else(|| entry.new_path.clone())
            .unwrap_or_else(|| entry.id.clone()),
        kind: metadata.kind.clone(),
        size: metadata.size,
        mode: metadata.mode,
        modified_ms: metadata.modified_ms,
        link: metadata.link.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn pair_exact(
    vault: &Path,
    snapshot: &Path,
    deleted: &[ManifestEntry],
    added: &[ManifestEntry],
    used_deleted: &mut BTreeSet<String>,
    used_added: &mut BTreeSet<String>,
    pairs: &mut Vec<(String, String, bool)>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    let mut groups: BTreeMap<(String, u64, u64, u64), Vec<&ManifestEntry>> = BTreeMap::new();
    for entry in added {
        if entry.kind == "directory" {
            continue;
        }
        let key = content_key(entry, vault, cancel)?;
        groups.entry(key).or_default().push(entry);
    }
    for old in deleted {
        if old.kind == "directory" {
            continue;
        }
        check_cancel(cancel)?;
        let key = content_key(old, snapshot, cancel)?;
        let Some(candidates) = groups.get(&key) else {
            continue;
        };
        let mut best: Option<(&ManifestEntry, f64)> = None;
        for candidate in candidates {
            if used_added.contains(&candidate.path) || candidate.kind != old.kind {
                continue;
            }
            let score = path_similarity(&old.path, &candidate.path);
            let replace = match best {
                None => true,
                Some((current, current_score)) => {
                    score > current_score
                        || (score == current_score && candidate.path < current.path)
                }
            };
            if replace {
                best = Some((candidate, score));
            }
        }
        let Some((new, _)) = best else { continue };
        let identical = match old.kind.as_str() {
            "file" => same_file(&snapshot.join(&old.path), &vault.join(&new.path), cancel)?,
            "symlink" => old.link == new.link,
            _ => false,
        };
        if !identical {
            continue;
        }
        used_deleted.insert(old.path.clone());
        used_added.insert(new.path.clone());
        pairs.push((old.path.clone(), new.path.clone(), true));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn pair_text(
    vault: &Path,
    snapshot: &Path,
    deleted: &[ManifestEntry],
    added: &[ManifestEntry],
    used_deleted: &BTreeSet<String>,
    used_added: &BTreeSet<String>,
    pairs: &mut Vec<(String, String, bool)>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    let deleted: Vec<&ManifestEntry> = deleted
        .iter()
        .filter(|entry| {
            entry.kind == "file"
                && entry.size <= MAX_RENAME_TEXT_BYTES
                && !used_deleted.contains(&entry.path)
        })
        .collect();
    let added: Vec<&ManifestEntry> = added
        .iter()
        .filter(|entry| {
            entry.kind == "file"
                && entry.size <= MAX_RENAME_TEXT_BYTES
                && !used_added.contains(&entry.path)
        })
        .collect();
    if deleted.is_empty() || added.is_empty() {
        return Ok(());
    }
    let mut signatures: Vec<Option<BTreeSet<u64>>> = Vec::with_capacity(added.len());
    let mut index: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (position, entry) in added.iter().enumerate() {
        check_cancel(cancel)?;
        let signature = read_text(vault, &entry.path, MAX_RENAME_TEXT_BYTES, cancel)?
            .map(|text| line_hashes(&text))
            .filter(|lines| !lines.is_empty());
        if let Some(lines) = signature.as_ref() {
            for line in lines {
                index.entry(*line).or_default().push(position);
            }
        }
        signatures.push(signature);
    }
    let mut candidates: Vec<(u64, usize, usize, String, String)> = Vec::new();
    for (old_position, old) in deleted.iter().enumerate() {
        check_cancel(cancel)?;
        let Some(old_text) = read_text(snapshot, &old.path, MAX_RENAME_TEXT_BYTES, cancel)? else {
            continue;
        };
        let old_lines = line_hashes(&old_text);
        if old_lines.len() < RENAME_SHARED_LINES_MIN {
            continue;
        }
        let mut shared: BTreeMap<usize, usize> = BTreeMap::new();
        for line in &old_lines {
            if let Some(positions) = index.get(line) {
                for position in positions {
                    *shared.entry(*position).or_default() += 1;
                }
            }
        }
        for (position, count) in shared {
            let Some(new_lines) = signatures[position].as_ref() else {
                continue;
            };
            let union = old_lines.len() + new_lines.len() - count;
            if union == 0 || count < RENAME_SHARED_LINES_MIN {
                continue;
            }
            let score = (count as f64) / (union as f64);
            if score < RENAME_SIMILARITY_MIN {
                continue;
            }
            candidates.push((
                (score * 1_000_000.0) as u64,
                old_position,
                position,
                old.path.clone(),
                added[position].path.clone(),
            ));
        }
    }
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.3.as_bytes().cmp(b.3.as_bytes()))
            .then_with(|| a.4.as_bytes().cmp(b.4.as_bytes()))
    });
    let mut paired_deleted = used_deleted.clone();
    let mut paired_added = used_added.clone();
    for (_, _, _, old, new) in candidates {
        if paired_deleted.contains(&old) || paired_added.contains(&new) {
            continue;
        }
        validate_relative(&new)?;
        paired_deleted.insert(old.clone());
        paired_added.insert(new.clone());
        pairs.push((old, new, false));
    }
    Ok(())
}

fn content_key(
    entry: &ManifestEntry,
    snapshot: &Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<(String, u64, u64, u64)> {
    if entry.kind == "symlink" {
        let target = entry.link.clone().unwrap_or_default();
        let (a, b) = hash_bytes(target.as_bytes());
        return Ok(("symlink".into(), target.len() as u64, a, b));
    }
    let (first, second) = file_hash(&snapshot.join(&entry.path), cancel)?;
    Ok((entry.kind.clone(), entry.size, first, second))
}

fn file_hash(path: &Path, cancel: Option<&AtomicBool>) -> io::Result<(u64, u64)> {
    let mut file = open_read(path)?;
    let mut first: u64 = 0xcbf2_9ce4_8422_2325;
    let mut second: u64 = 0x8422_2325_cbf2_9ce4;
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            first ^= *byte as u64;
            first = first.wrapping_mul(0x100_0000_01b3);
            second ^= (*byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
            second = second.rotate_left(5).wrapping_mul(0x100_0000_01b3);
        }
    }
    Ok((first, second))
}

fn hash_bytes(bytes: &[u8]) -> (u64, u64) {
    let mut first: u64 = 0xcbf2_9ce4_8422_2325;
    let mut second: u64 = 0x8422_2325_cbf2_9ce4;
    for byte in bytes {
        first ^= *byte as u64;
        first = first.wrapping_mul(0x100_0000_01b3);
        second ^= (*byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        second = second.rotate_left(5).wrapping_mul(0x100_0000_01b3);
    }
    (first, second)
}

fn same_file(a: &Path, b: &Path, cancel: Option<&AtomicBool>) -> io::Result<bool> {
    let mut left = open_read(a)?;
    let mut right = open_read(b)?;
    let mut left_buffer = vec![0u8; READ_CHUNK];
    let mut right_buffer = vec![0u8; READ_CHUNK];
    loop {
        check_cancel(cancel)?;
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn same_content(
    vault: &Path,
    snapshot: &Path,
    before: &ManifestEntry,
    before_path: &str,
    after: &ManifestEntry,
    after_path: &str,
    cancel: Option<&AtomicBool>,
) -> io::Result<bool> {
    match before.kind.as_str() {
        "file" => {
            if before.size != after.size {
                return Ok(false);
            }
            let old = safe_join(snapshot, before_path, "history.snapshot_corrupt")?;
            let new = safe_join(vault, after_path, "history.invalid_path")?;
            same_file(&old, &new, cancel)
        }
        "symlink" => Ok(before.link == after.link),
        "directory" => Ok(true),
        _ => Ok(false),
    }
}

fn safe_join(root: &Path, relative: &str, missing: &str) -> io::Result<PathBuf> {
    validate_relative(relative)?;
    let mut path = root.to_path_buf();
    let mut components = Path::new(relative).components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(key_error("history.invalid_path"));
        };
        path.push(name);
        if components.peek().is_some() {
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(io::ErrorKind::NotFound, missing)
                } else {
                    error
                }
            })?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(key_error("history.invalid_path"));
            }
        }
    }
    Ok(path)
}

fn validate_relative(path: &str) -> io::Result<()> {
    if path.is_empty()
        || path.contains('\0')
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
    {
        return Err(key_error("history.invalid_path"));
    }
    if !Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(key_error("history.invalid_path"));
    }
    Ok(())
}

fn disk_entry(root: &Path, relative: &str) -> io::Result<ManifestEntry> {
    let path = safe_join(root, relative, "history.entry_missing")?;
    let metadata = fs::symlink_metadata(&path)?;
    let Some(kind) = kind_name(&metadata) else {
        return Err(key_error("history.unsupported_entry"));
    };
    let link = if kind == "symlink" {
        Some(fs::read_link(&path)?.to_string_lossy().into_owned())
    } else {
        None
    };
    Ok(ManifestEntry {
        path: relative.to_string(),
        kind: kind.into(),
        size: metadata.size(),
        mode: metadata.mode() & 0o7777,
        modified_ms: Some(
            metadata
                .mtime()
                .saturating_mul(1000)
                .saturating_add(metadata.mtime_nsec() / 1_000_000),
        ),
        link,
    })
}

fn kind_name(metadata: &fs::Metadata) -> Option<&'static str> {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        Some("symlink")
    } else if file_type.is_dir() {
        Some("directory")
    } else if file_type.is_file() {
        Some("file")
    } else {
        None
    }
}

fn open_read(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
}

fn read_text(
    root: &Path,
    relative: &str,
    limit: u64,
    cancel: Option<&AtomicBool>,
) -> io::Result<Option<String>> {
    let path = safe_join(root, relative, "history.entry_missing")?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(key_error("history.entry_missing"));
        }
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.size() > limit {
        return Ok(None);
    }
    let file = open_read(&path)?;
    let mut bytes = Vec::with_capacity(metadata.size() as usize);
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    check_cancel(cancel)?;
    if bytes.len() as u64 > limit || bytes.contains(&0) {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

fn line_hashes(text: &str) -> BTreeSet<u64> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| hash_bytes(line.as_bytes()).0)
        .collect()
}

fn path_similarity(a: &str, b: &str) -> f64 {
    let a = Path::new(a)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let b = Path::new(b)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let distance = levenshtein(&a, &b);
    let longest = a.chars().count().max(b.chars().count());
    1.0 - distance as f64 / longest as f64
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let cost = usize::from(left != right);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn normalize_eol(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// 把正文拆成行：去掉 CRLF 的 `\r`，记录是否缺末尾换行。
fn text_lines(text: &str) -> (Vec<String>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let missing = !text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if !missing {
        lines.pop();
    }
    (lines, missing)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Keep,
    Delete,
    Insert,
}

fn edit_script(old: &[String], new: &[String]) -> Vec<Edit> {
    let (prefix, old_middle, new_middle, suffix) = trim_common(old, new);
    let mut script = Vec::with_capacity(old.len() + new.len());
    script.extend(std::iter::repeat_n(Edit::Keep, prefix));
    let cells = (old_middle.len() as u64).saturating_mul(new_middle.len() as u64);
    if old_middle.len() > MAX_DIFF_LINES
        || new_middle.len() > MAX_DIFF_LINES
        || cells > MAX_DIFF_CELLS
    {
        script.extend(std::iter::repeat_n(Edit::Delete, old_middle.len()));
        script.extend(std::iter::repeat_n(Edit::Insert, new_middle.len()));
    } else {
        script.extend(lcs_script(old_middle, new_middle));
    }
    script.extend(std::iter::repeat_n(Edit::Keep, suffix));
    script
}

fn trim_common<'a>(
    old: &'a [String],
    new: &'a [String],
) -> (usize, &'a [String], &'a [String], usize) {
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    (
        prefix,
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
        suffix,
    )
}

fn lcs_script(old: &[String], new: &[String]) -> Vec<Edit> {
    let rows = old.len() + 1;
    let columns = new.len() + 1;
    let mut table = vec![0u32; rows * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * columns + j] = if old[i] == new[j] {
                table[(i + 1) * columns + j + 1] + 1
            } else {
                table[(i + 1) * columns + j].max(table[i * columns + j + 1])
            };
        }
    }
    let mut script = Vec::with_capacity(old.len() + new.len());
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            script.push(Edit::Keep);
            i += 1;
            j += 1;
        } else if table[(i + 1) * columns + j] >= table[i * columns + j + 1] {
            script.push(Edit::Delete);
            i += 1;
        } else {
            script.push(Edit::Insert);
            j += 1;
        }
    }
    while i < old.len() {
        script.push(Edit::Delete);
        i += 1;
    }
    while j < new.len() {
        script.push(Edit::Insert);
        j += 1;
    }
    script
}

/// 按 hunk 组织 diff，携带上下文行、旧/新行号与行类型。
pub fn hunks(old: &[String], new: &[String], context: usize) -> Vec<DiffHunk> {
    let script = edit_script(old, new);
    let mut indexed = Vec::with_capacity(script.len());
    let (mut old_line, mut new_line) = (1u32, 1u32);
    for edit in &script {
        indexed.push(match edit {
            Edit::Keep => {
                let line = DiffLine {
                    kind: "context".into(),
                    old_line: Some(old_line),
                    new_line: Some(new_line),
                    text: old[(old_line - 1) as usize].clone(),
                };
                old_line += 1;
                new_line += 1;
                line
            }
            Edit::Delete => {
                let line = DiffLine {
                    kind: "removed".into(),
                    old_line: Some(old_line),
                    new_line: None,
                    text: old[(old_line - 1) as usize].clone(),
                };
                old_line += 1;
                line
            }
            Edit::Insert => {
                let line = DiffLine {
                    kind: "added".into(),
                    old_line: None,
                    new_line: Some(new_line),
                    text: new[(new_line - 1) as usize].clone(),
                };
                new_line += 1;
                line
            }
        });
    }
    let changes: Vec<usize> = indexed
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != "context")
        .map(|(index, _)| index)
        .collect();
    if changes.is_empty() {
        return Vec::new();
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in changes {
        let start = index.saturating_sub(context);
        let end = (index + context + 1).min(indexed.len());
        if let Some(last) = ranges.last_mut()
            && start <= last.1
        {
            last.1 = end;
        } else {
            ranges.push((start, end));
        }
    }
    ranges
        .into_iter()
        .map(|(start, end)| {
            let lines = indexed[start..end].to_vec();
            let old_start = lines
                .iter()
                .find_map(|line| line.old_line)
                .unwrap_or_else(|| old_line_at(&indexed, start));
            let new_start = lines
                .iter()
                .find_map(|line| line.new_line)
                .unwrap_or_else(|| new_line_at(&indexed, start));
            let old_count = lines.iter().filter(|line| line.old_line.is_some()).count() as u32;
            let new_count = lines.iter().filter(|line| line.new_line.is_some()).count() as u32;
            let old_start = if old_count == 0 {
                old_start.saturating_sub(1)
            } else {
                old_start
            };
            let new_start = if new_count == 0 {
                new_start.saturating_sub(1)
            } else {
                new_start
            };
            DiffHunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines,
            }
        })
        .collect()
}

fn old_line_at(lines: &[DiffLine], index: usize) -> u32 {
    lines[..index]
        .iter()
        .rev()
        .find_map(|line| line.old_line)
        .map(|line| line + 1)
        .unwrap_or(1)
}

fn new_line_at(lines: &[DiffLine], index: usize) -> u32 {
    lines[..index]
        .iter()
        .rev()
        .find_map(|line| line.new_line)
        .map(|line| line + 1)
        .unwrap_or(1)
}

fn key_error(key: &str) -> io::Error {
    io::Error::other(key)
}

fn check_cancel(cancel: Option<&AtomicBool>) -> io::Result<()> {
    match cancel {
        Some(flag) if flag.load(Ordering::Acquire) => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "history.cancelled",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_hunks_keep_context_and_line_numbers() {
        let old = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let new = vec!["a".to_string(), "B".to_string(), "c".to_string()];
        let hunks = hunks(&old, &new, 1);
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!((hunk.old_start, hunk.old_count), (1, 3));
        assert_eq!((hunk.new_start, hunk.new_count), (1, 3));
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[1].kind, "removed");
        assert_eq!(hunk.lines[1].old_line, Some(2));
        assert_eq!(hunk.lines[2].kind, "added");
        assert_eq!(hunk.lines[2].new_line, Some(2));
    }

    #[test]
    fn insertion_at_start_uses_zero_old_start() {
        let added = vec!["new".to_string()];
        let hunks = hunks(&[], &added, 3);
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_start, hunks[0].old_count), (0, 0));
        assert_eq!((hunks[0].new_start, hunks[0].new_count), (1, 1));
        assert_eq!(hunks[0].lines[0].new_line, Some(1));
    }

    #[test]
    fn text_lines_handle_crlf_and_missing_final_newline() {
        assert_eq!(text_lines(""), (Vec::<String>::new(), false));
        assert_eq!(
            text_lines("a\r\nb"),
            (vec!["a".to_string(), "b".to_string()], true)
        );
        assert_eq!(
            text_lines("a\r\nb\r\n"),
            (vec!["a".to_string(), "b".to_string()], false)
        );
        assert_eq!(text_lines("\n"), (vec![String::new()], false));
    }

    #[test]
    fn unrelated_replacements_are_not_renames() {
        assert!(
            path_similarity("notes/alpha.md", "notes/alpha-copy.md")
                > path_similarity("notes/alpha.md", "notes/beta.md")
        );
        assert!(path_similarity("notes/alpha.md", "archive/alpha.md") == 1.0);
    }

    #[test]
    fn large_diff_falls_back_to_replacement_but_keeps_lines() {
        let old: Vec<String> = (0..MAX_DIFF_LINES + 10)
            .map(|index| format!("old {index}"))
            .collect();
        let new: Vec<String> = (0..MAX_DIFF_LINES + 10)
            .map(|index| format!("new {index}"))
            .collect();
        let hunks = hunks(&old, &new, 3);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_count as usize, old.len());
        assert_eq!(hunks[0].new_count as usize, new.len());
    }
}
