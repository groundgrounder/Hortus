//! 按处、按文件与按会话的安全回滚，以及显式快照清理。
//!
//! 所有写入都基于 P1-33 快照；不调用 Git、不跟随符号链接。回滚前按审阅
//! 指纹核对磁盘状态，审阅后发生的任何等长改写、权限或链接目标变化都会
//! 拒绝覆盖。写入复用 `hortus-vault` 的同卷原子替换与 macOS 元数据保留。
use crate::HistoryStore;
use crate::digest::state_digest;
use crate::review::{
    ChangeKind, DiffHunk, EntryMetadata, ReviewEntry, check_cancel, disk_entry, key_error,
    safe_join, text_lines, validate_relative,
};
use crate::snapshot::ManifestEntry;
use hortus_vault::note::{DiskVersion, Format, WriteFailure, atomic_replace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// UI 在发起回滚时提交的审阅记录；`expected_after` 必须来自同一次审阅。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackExpectation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    pub change: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackFailure {
    pub path: String,
    pub change: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRollbackReport {
    pub reverted_paths: Vec<String>,
    pub skipped_stale: Vec<RollbackFailure>,
    pub failed: Vec<RollbackFailure>,
}

impl SessionRollbackReport {
    pub fn is_complete(&self) -> bool {
        self.skipped_stale.is_empty() && self.failed.is_empty()
    }
}

struct Plan {
    expectation: RollbackExpectation,
    before: Option<ManifestEntry>,
    after: Option<ManifestEntry>,
}

impl HistoryStore {
    /// 反向应用一个 hunk；只接受仍与审阅时完全一致的文本文件。
    #[allow(clippy::too_many_arguments)]
    pub fn rollback_hunk(
        &self,
        id: &str,
        path: &str,
        old_path: Option<&str>,
        expectation: &RollbackExpectation,
        hunk_index: usize,
        expected_hunk: &DiffHunk,
        temporary: &Path,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<()> {
        if expectation.change == ChangeKind::TypeChanged {
            return Err(key_error("history.hunk_unsupported"));
        }
        let record = self.reviewable_record(id)?;
        let snapshot = self.snapshot_directory(id)?;
        let detail = self.review_entry(id, path, old_path, cancel)?;
        let entry = &detail.entry;
        if entry.change != expectation.change {
            return Err(key_error("history.review_stale"));
        }
        verify_before(
            &snapshot,
            before_path(entry),
            expectation.expected_before.as_deref(),
        )?;
        verify_after(
            Path::new(&record.vault),
            after_path(entry),
            expectation.expected_after.as_deref(),
        )?;
        if !detail.text || detail.binary || detail.large {
            return Err(key_error("history.hunk_unsupported"));
        }
        let Some(after) = entry.after.as_ref() else {
            return Err(key_error("history.hunk_unsupported"));
        };
        if after.kind != "file" {
            return Err(key_error("history.hunk_unsupported"));
        }
        let actual = detail
            .hunks
            .get(hunk_index)
            .ok_or_else(|| key_error("history.hunk_invalid"))?;
        if actual != expected_hunk {
            return Err(key_error("history.hunk_invalid"));
        }
        apply_hunk(
            Path::new(&record.vault),
            path,
            &detail,
            expected_hunk,
            temporary,
            cancel,
        )
    }

    /// 按文件回滚单条目；任何指纹不匹配都拒绝写入。
    pub fn rollback_entry(
        &self,
        id: &str,
        expectation: &RollbackExpectation,
        temporary: &Path,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<()> {
        validate_expectation(expectation)?;
        let record = self.reviewable_record(id)?;
        let (snapshot, before_index) = self.load_snapshot(&record, cancel, false)?;
        let after_relative = expectation.new_path.as_deref().unwrap_or(&expectation.path);
        let detail =
            self.review_entry(id, after_relative, expectation.old_path.as_deref(), cancel)?;
        let entry = &detail.entry;
        if entry.change != expectation.change {
            return Err(key_error("history.review_stale"));
        }
        verify_before(
            &snapshot,
            before_path(entry),
            expectation.expected_before.as_deref(),
        )?;
        verify_after(
            Path::new(&record.vault),
            after_path(entry),
            expectation.expected_after.as_deref(),
        )?;
        let plan = Plan {
            expectation: expectation.clone(),
            before: before_manifest(entry, &before_index)?.cloned(),
            after: entry.after.clone().map(manifest_from_metadata),
        };
        let mut pending_modes = BTreeMap::new();
        revert_plan(
            Path::new(&record.vault),
            &snapshot,
            &before_index,
            &plan,
            temporary,
            &mut pending_modes,
            cancel,
        )?;
        restore_directory_modes(&pending_modes)
    }

    /// 按会话回滚整批条目：按依赖顺序执行，逐项汇报成功、stale 与失败。
    pub fn rollback_session(
        &self,
        id: &str,
        expectations: &[RollbackExpectation],
        temporary: &Path,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<SessionRollbackReport> {
        let record = self.reviewable_record(id)?;
        let (snapshot, before_index) = self.load_snapshot(&record, cancel, true)?;
        let vault = Path::new(&record.vault);
        let mut plans = Vec::with_capacity(expectations.len());
        for expectation in expectations {
            check_cancel(cancel)?;
            validate_expectation(expectation)?;
            let after_relative = display_path(expectation);
            let before = match expectation.change {
                ChangeKind::Added => None,
                _ => match before_index.get(before_path_expectation(expectation)) {
                    Some(entry) => Some(entry.clone()),
                    None => return Err(key_error("history.invalid_entry")),
                },
            };
            let after = match expectation.change {
                ChangeKind::Deleted => None,
                _ => match disk_entry(vault, after_relative) {
                    Ok(entry) => Some(entry),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                },
            };
            plans.push(Plan {
                expectation: expectation.clone(),
                before,
                after,
            });
        }

        let mut report = SessionRollbackReport::default();
        let mut pending_modes = BTreeMap::new();
        let phases: [fn(&Plan) -> bool; 5] = [
            |plan| {
                plan.expectation.change == ChangeKind::TypeChanged
                    && plan
                        .before
                        .as_ref()
                        .is_some_and(|entry| entry.kind == "directory")
                    && plan
                        .after
                        .as_ref()
                        .is_some_and(|entry| entry.kind != "directory")
            },
            |plan| plan.expectation.change == ChangeKind::Deleted,
            |plan| plan.expectation.change == ChangeKind::Renamed,
            |plan| plan.expectation.change == ChangeKind::Added,
            |plan| {
                plan.expectation.change == ChangeKind::Modified
                    || (plan.expectation.change == ChangeKind::TypeChanged
                        && plan
                            .before
                            .as_ref()
                            .is_none_or(|entry| entry.kind != "directory"))
            },
        ];

        for (phase, matches) in phases.iter().enumerate() {
            let mut indices: Vec<usize> = plans
                .iter()
                .enumerate()
                .filter(|(_, plan)| (*matches)(plan))
                .map(|(index, _)| index)
                .collect();
            // 新增条目的删除必须由深到浅；其余恢复由浅到深，先建父目录。
            indices.sort_by(|a, b| {
                let left = &plans[*a];
                let right = &plans[*b];
                let left_depth = path_depth(display_path(&left.expectation));
                let right_depth = path_depth(display_path(&right.expectation));
                if phase == 3 {
                    right_depth.cmp(&left_depth).then_with(|| {
                        display_path(&left.expectation).cmp(display_path(&right.expectation))
                    })
                } else {
                    left_depth.cmp(&right_depth).then_with(|| {
                        display_path(&left.expectation).cmp(display_path(&right.expectation))
                    })
                }
            });
            for index in indices {
                check_cancel(cancel)?;
                let plan = &plans[index];
                let display = display_path(&plan.expectation).to_string();
                match revert_plan(
                    vault,
                    &snapshot,
                    &before_index,
                    plan,
                    temporary,
                    &mut pending_modes,
                    cancel,
                ) {
                    Ok(()) => report.reverted_paths.push(display),
                    Err(error) if error.to_string() == "history.review_stale" => {
                        report.skipped_stale.push(RollbackFailure {
                            path: display,
                            change: plan.expectation.change.as_str().into(),
                            reason: error.to_string(),
                        });
                    }
                    Err(error) => {
                        report.failed.push(RollbackFailure {
                            path: display,
                            change: plan.expectation.change.as_str().into(),
                            reason: error.to_string(),
                        });
                    }
                }
            }
        }
        // 回滚期间临时放开只读父目录的写权限，全部条目完成后再恢复快照 mode。
        restore_directory_modes(&pending_modes)?;
        Ok(report)
    }

    /// 显式删除一个历史会话目录。只清理 `<data>/agent-sessions/<UUID>/`，
    /// 先解除只读权限与 macOS `uchg`/`schg` 标志，绝不触碰用户库。
    pub fn delete_session(&self, id: &str) -> io::Result<()> {
        let record = self.record(id)?;
        match record.state {
            crate::SessionState::Preparing | crate::SessionState::Running => {
                return Err(key_error("history.session_active"));
            }
            _ => {}
        }
        let directory = self.session_directory(id)?;
        let sessions = self.sessions_directory();
        if directory.parent() != Some(sessions.as_path()) {
            return Err(key_error("history.invalid_session"));
        }
        match fs::symlink_metadata(&sessions) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(key_error("history.invalid_session")),
            Err(error) => return Err(error),
        }
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            }
            Ok(_) => return Err(key_error("history.invalid_session")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(key_error("history.session_missing"));
            }
            Err(error) => return Err(error),
        }
        remove_tree(&directory)
    }
}

fn validate_expectation(expectation: &RollbackExpectation) -> io::Result<()> {
    validate_relative(&expectation.path)?;
    if let Some(old_path) = &expectation.old_path {
        validate_relative(old_path)?;
    }
    if let Some(new_path) = &expectation.new_path {
        validate_relative(new_path)?;
    }
    Ok(())
}

fn before_path_expectation(expectation: &RollbackExpectation) -> &str {
    expectation.old_path.as_deref().unwrap_or(&expectation.path)
}

fn before_path(entry: &ReviewEntry) -> &str {
    entry.old_path.as_deref().unwrap_or(&entry.id)
}

fn after_path(entry: &ReviewEntry) -> &str {
    entry.new_path.as_deref().unwrap_or(&entry.id)
}

fn display_path(expectation: &RollbackExpectation) -> &str {
    expectation.new_path.as_deref().unwrap_or(&expectation.path)
}

fn path_depth(path: &str) -> usize {
    Path::new(path).components().count()
}

fn before_manifest<'a>(
    entry: &ReviewEntry,
    index: &'a BTreeMap<String, ManifestEntry>,
) -> io::Result<Option<&'a ManifestEntry>> {
    match entry.change {
        ChangeKind::Added => Ok(None),
        _ => index
            .get(before_path(entry))
            .map(Some)
            .ok_or_else(|| key_error("history.invalid_entry")),
    }
}

fn manifest_from_metadata(metadata: EntryMetadata) -> ManifestEntry {
    ManifestEntry {
        path: String::new(),
        kind: metadata.kind,
        size: metadata.size,
        mode: metadata.mode,
        modified_ms: metadata.modified_ms,
        link: metadata.link,
    }
}

fn verify_after(vault: &Path, relative: &str, expected: Option<&str>) -> io::Result<()> {
    let actual = state_digest(vault, relative)?;
    if actual.as_deref() != expected {
        return Err(key_error("history.review_stale"));
    }
    Ok(())
}

fn verify_before(snapshot: &Path, relative: &str, expected: Option<&str>) -> io::Result<()> {
    if expected.is_none() {
        return Ok(());
    }
    let actual = state_digest(snapshot, relative)?;
    if actual.as_deref() != expected {
        return Err(key_error("history.snapshot_corrupt"));
    }
    Ok(())
}

fn revert_plan(
    vault: &Path,
    snapshot: &Path,
    before_index: &BTreeMap<String, ManifestEntry>,
    plan: &Plan,
    temporary: &Path,
    pending_modes: &mut BTreeMap<PathBuf, u32>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    check_cancel(cancel)?;
    let expectation = &plan.expectation;
    let after_relative = display_path(expectation);
    verify_after(vault, after_relative, expectation.expected_after.as_deref())?;
    let before_relative = before_path_expectation(expectation);
    verify_before(
        snapshot,
        before_relative,
        expectation.expected_before.as_deref(),
    )?;
    match expectation.change {
        ChangeKind::Added => {
            let after = plan
                .after
                .as_ref()
                .ok_or_else(|| key_error("history.review_stale"))?;
            remove_after(vault, after_relative, after)?;
        }
        ChangeKind::Deleted => {
            let before = plan
                .before
                .as_ref()
                .ok_or_else(|| key_error("history.invalid_entry"))?;
            ensure_parents(
                vault,
                snapshot,
                before_index,
                before_relative,
                pending_modes,
            )?;
            restore_before(
                snapshot,
                vault,
                before_relative,
                before,
                temporary,
                pending_modes,
            )?;
        }
        ChangeKind::Renamed => {
            let before = plan
                .before
                .as_ref()
                .ok_or_else(|| key_error("history.invalid_entry"))?;
            let after = plan
                .after
                .as_ref()
                .ok_or_else(|| key_error("history.review_stale"))?;
            if fs::symlink_metadata(vault_join(vault, before_relative)?).is_ok() {
                return Err(key_error("history.target_exists"));
            }
            ensure_parents(
                vault,
                snapshot,
                before_index,
                before_relative,
                pending_modes,
            )?;
            restore_before(
                snapshot,
                vault,
                before_relative,
                before,
                temporary,
                pending_modes,
            )?;
            // 只有旧路径成功恢复后才删除新路径；任何失败都保留可读副本。
            remove_after(vault, after_relative, after)?;
        }
        ChangeKind::Modified => {
            let before = plan
                .before
                .as_ref()
                .ok_or_else(|| key_error("history.invalid_entry"))?;
            let after = plan
                .after
                .as_ref()
                .ok_or_else(|| key_error("history.review_stale"))?;
            revert_modified(snapshot, vault, before_relative, before, after, temporary)?;
        }
        ChangeKind::TypeChanged => {
            let before = plan
                .before
                .as_ref()
                .ok_or_else(|| key_error("history.invalid_entry"))?;
            let after = plan
                .after
                .as_ref()
                .ok_or_else(|| key_error("history.review_stale"))?;
            remove_after(vault, after_relative, after)?;
            ensure_parents(
                vault,
                snapshot,
                before_index,
                before_relative,
                pending_modes,
            )?;
            restore_before(
                snapshot,
                vault,
                before_relative,
                before,
                temporary,
                pending_modes,
            )?;
        }
    }
    Ok(())
}

fn revert_modified(
    snapshot: &Path,
    vault: &Path,
    relative: &str,
    before: &ManifestEntry,
    after: &ManifestEntry,
    temporary: &Path,
) -> io::Result<()> {
    let absolute = vault_join(vault, relative)?;
    match (before.kind.as_str(), after.kind.as_str()) {
        ("file", "file") => {
            let snapshot_bytes =
                fs::read(safe_join(snapshot, relative, "history.snapshot_corrupt")?)?;
            let current = fs::read(&absolute)?;
            if current != snapshot_bytes {
                let baseline = DiskVersion::new(current);
                write_existing(
                    &absolute,
                    &snapshot_bytes,
                    &baseline,
                    temporary,
                    vault,
                    before.mode,
                )?;
            } else if after.mode != before.mode {
                fs::set_permissions(&absolute, fs::Permissions::from_mode(before.mode))?;
            }
        }
        ("symlink", "symlink") => {
            let target = before
                .link
                .as_deref()
                .ok_or_else(|| key_error("history.snapshot_corrupt"))?;
            let actual = fs::read_link(&absolute)?.to_string_lossy().into_owned();
            if actual != target {
                replace_symlink(vault, relative, target, after.link.as_deref(), temporary)?;
            }
        }
        ("directory", "directory") => {
            fs::set_permissions(&absolute, fs::Permissions::from_mode(before.mode))?;
        }
        _ => return Err(key_error("history.invalid_entry")),
    }
    Ok(())
}

fn remove_after(vault: &Path, relative: &str, after: &ManifestEntry) -> io::Result<()> {
    let absolute = vault_join(vault, relative)?;
    let metadata = fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(&absolute)?;
        return Ok(());
    }
    if metadata.is_dir() {
        if fs::read_dir(&absolute)?.next().is_some() {
            return Err(key_error("history.directory_not_empty"));
        }
        fs::remove_dir(&absolute)?;
        return Ok(());
    }
    let _ = after;
    Err(key_error("history.unsupported_entry"))
}

fn restore_before(
    snapshot: &Path,
    vault: &Path,
    relative: &str,
    before: &ManifestEntry,
    temporary: &Path,
    pending_modes: &mut BTreeMap<PathBuf, u32>,
) -> io::Result<()> {
    let absolute = vault_join(vault, relative)?;
    match before.kind.as_str() {
        "file" => {
            let bytes = fs::read(safe_join(snapshot, relative, "history.snapshot_corrupt")?)?;
            match atomic_replace(
                &absolute,
                &bytes,
                None,
                temporary,
                vault,
                Some(fs::Permissions::from_mode(before.mode)),
            ) {
                Ok(_) => {
                    fs::set_permissions(&absolute, fs::Permissions::from_mode(before.mode))?;
                    Ok(())
                }
                Err(WriteFailure::Changed) | Err(WriteFailure::Missing) => {
                    Err(key_error("history.target_exists"))
                }
                Err(WriteFailure::Io(error)) => Err(error),
            }
        }
        "symlink" => {
            let target = before
                .link
                .as_deref()
                .ok_or_else(|| key_error("history.snapshot_corrupt"))?;
            if fs::symlink_metadata(&absolute).is_ok() {
                return Err(key_error("history.target_exists"));
            }
            std::os::unix::fs::symlink(target, &absolute)?;
            Ok(())
        }
        "directory" => {
            if fs::symlink_metadata(&absolute).is_ok() {
                return Err(key_error("history.target_exists"));
            }
            fs::DirBuilder::new()
                .mode((before.mode | 0o700) & 0o7777)
                .create(&absolute)?;
            pending_modes.insert(absolute, before.mode);
            Ok(())
        }
        _ => Err(key_error("history.unsupported_entry")),
    }
}

fn replace_symlink(
    vault: &Path,
    relative: &str,
    target: &str,
    expected_current: Option<&str>,
    temporary: &Path,
) -> io::Result<()> {
    let absolute = vault_join(vault, relative)?;
    let staged = temporary.join(format!(
        "rollback-link-{}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    let _ = fs::remove_file(&staged);
    std::os::unix::fs::symlink(target, &staged)?;
    // 替换前再次核对当前链接目标，防止覆盖审阅后的并发改动。
    let current = fs::read_link(&absolute)
        .ok()
        .map(|link| link.to_string_lossy().into_owned());
    if current.as_deref() != expected_current {
        let _ = fs::remove_file(&staged);
        return Err(key_error("history.review_stale"));
    }
    fs::rename(&staged, &absolute)?;
    let _ = fs::File::open(
        absolute
            .parent()
            .ok_or_else(|| key_error("history.path_invalid"))?,
    )
    .and_then(|directory| directory.sync_all());
    Ok(())
}

fn write_existing(
    path: &Path,
    bytes: &[u8],
    baseline: &DiskVersion,
    temporary: &Path,
    vault: &Path,
    final_mode: u32,
) -> io::Result<()> {
    // 只读文件也允许显式回滚：临时补上 owner write，写完后恢复期望权限。
    let original_mode = fs::metadata(path)?.permissions().mode() & 0o7777;
    let widened = original_mode & 0o200 == 0;
    if widened {
        fs::set_permissions(path, fs::Permissions::from_mode(original_mode | 0o200))?;
    }
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let result = atomic_replace(path, bytes, Some(baseline), temporary, vault, permissions);
    match result {
        Ok(_) => {
            fs::set_permissions(path, fs::Permissions::from_mode(final_mode))?;
            Ok(())
        }
        Err(error) => {
            if widened {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(original_mode));
            }
            match error {
                WriteFailure::Changed | WriteFailure::Missing => {
                    Err(key_error("history.review_stale"))
                }
                WriteFailure::Io(error) => Err(error),
            }
        }
    }
}

fn apply_hunk(
    vault: &Path,
    relative: &str,
    detail: &crate::review::ReviewDetail,
    hunk: &DiffHunk,
    temporary: &Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    let absolute = vault_join(vault, relative)?;
    let bytes = fs::read(&absolute)?;
    let (format, text, valid) = Format::decode(&bytes);
    if !valid {
        return Err(key_error("history.hunk_unsupported"));
    }
    let (mut lines, missing) = text_lines(&text);
    let start = if hunk.new_count == 0 {
        hunk.new_start as usize
    } else {
        (hunk.new_start as usize).saturating_sub(1)
    };
    let end = start.saturating_add(hunk.new_count as usize);
    if start > lines.len() || end > lines.len() {
        return Err(key_error("history.hunk_stale"));
    }
    let expected_new: Vec<&str> = hunk
        .lines
        .iter()
        .filter(|line| line.new_line.is_some())
        .map(|line| line.text.as_str())
        .collect();
    if lines[start..end] != expected_new {
        return Err(key_error("history.hunk_stale"));
    }
    let touches_end = end >= lines.len();
    let expected_old: Vec<String> = hunk
        .lines
        .iter()
        .filter(|line| line.old_line.is_some())
        .map(|line| line.text.clone())
        .collect();
    lines.splice(start..end, expected_old);
    let missing = if touches_end {
        detail.old_no_final_newline
    } else {
        missing
    };
    let mut result = lines.join("\n");
    if !missing && !lines.is_empty() {
        result.push('\n');
    }
    let encoded = format.encode(&result);
    let baseline = DiskVersion::new(bytes);
    check_cancel(cancel)?;
    let mode = fs::metadata(&absolute)?.permissions().mode() & 0o7777;
    write_existing(&absolute, &encoded, &baseline, temporary, vault, mode)
}

fn ensure_parents(
    vault: &Path,
    snapshot: &Path,
    before_index: &BTreeMap<String, ManifestEntry>,
    relative: &str,
    pending_modes: &mut BTreeMap<PathBuf, u32>,
) -> io::Result<()> {
    let components: Vec<Component> = Path::new(relative).components().collect();
    if components.len() <= 1 {
        return Ok(());
    }
    let mut current = vault.to_path_buf();
    let mut relative_so_far = String::new();
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            return Err(key_error("history.invalid_path"));
        };
        current.push(name);
        if !relative_so_far.is_empty() {
            relative_so_far.push('/');
        }
        relative_so_far.push_str(&name.to_string_lossy());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let mode = metadata.mode() & 0o7777;
                if mode & 0o200 == 0 {
                    fs::set_permissions(&current, fs::Permissions::from_mode(mode | 0o200))?;
                    pending_modes.insert(current.clone(), mode);
                }
            }
            Ok(_) => return Err(key_error("history.path_blocked")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(entry) = before_index.get(&relative_so_far) else {
                    return Err(key_error("history.path_blocked"));
                };
                if entry.kind != "directory" {
                    return Err(key_error("history.path_blocked"));
                }
                let snapshot_directory =
                    safe_join(snapshot, &relative_so_far, "history.snapshot_corrupt")?;
                if !snapshot_directory.is_dir() {
                    return Err(key_error("history.snapshot_corrupt"));
                }
                fs::DirBuilder::new()
                    .mode((entry.mode | 0o700) & 0o7777)
                    .create(&current)?;
                pending_modes.insert(current.clone(), entry.mode);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn vault_join(vault: &Path, relative: &str) -> io::Result<PathBuf> {
    validate_relative(relative)?;
    let mut path = vault.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            return Err(key_error("history.invalid_path"));
        };
        path.push(name);
    }
    Ok(path)
}

fn restore_directory_modes(pending_modes: &BTreeMap<PathBuf, u32>) -> io::Result<()> {
    let mut modes: Vec<_> = pending_modes.iter().collect();
    // 先恢复深层目录，再恢复父目录，避免父目录先变只读。
    modes.sort_by(|left, right| {
        right
            .0
            .components()
            .count()
            .cmp(&left.0.components().count())
    });
    for (path, mode) in modes {
        fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
    }
    Ok(())
}

fn remove_tree(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        clear_immutable(path, &metadata)?;
        fs::remove_file(path)?;
        return Ok(());
    }
    if metadata.is_file() {
        clear_immutable(path, &metadata)?;
        clear_read_only(path, false)?;
        fs::remove_file(path)?;
        return Ok(());
    }
    if metadata.is_dir() {
        clear_immutable(path, &metadata)?;
        clear_read_only(path, true)?;
        let mut entries: Vec<PathBuf> = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()?;
        entries.sort();
        for entry in entries {
            remove_tree(&entry)?;
        }
        fs::remove_dir(path)?;
        return Ok(());
    }
    Err(key_error("history.unsupported_entry"))
}

#[cfg(target_os = "macos")]
fn clear_immutable(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn lchflags(path: *const std::ffi::c_char, flags: std::ffi::c_uint) -> std::ffi::c_int;
    }
    const UF_IMMUTABLE: u32 = 0x0000_0002;
    const UF_APPEND: u32 = 0x0000_0004;
    const SF_IMMUTABLE: u32 = 0x0002_0000;
    const SF_APPEND: u32 = 0x0004_0000;
    let blocking = metadata.st_flags() & (UF_IMMUTABLE | UF_APPEND | SF_IMMUTABLE | SF_APPEND);
    if blocking == 0 {
        return Ok(());
    }
    let path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| key_error("history.invalid_path"))?;
    let flags = metadata.st_flags() & !blocking;
    if unsafe { lchflags(path.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn clear_immutable(_path: &Path, _metadata: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

fn clear_read_only(path: &Path, directory: bool) -> io::Result<()> {
    let mode = if directory { 0o700 } else { 0o600 };
    match fs::set_permissions(path, fs::Permissions::from_mode(mode)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
