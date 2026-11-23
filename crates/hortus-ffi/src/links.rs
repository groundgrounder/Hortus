//! 知识索引查询与经过确认的双链磁盘改写。
use crate::{CoreError, VaultHandle};
use hortus_document::{Document, embeds};
use hortus_links::{LinkResolution, LinkRewrite, ResolvedLink};
use hortus_vault::note::{self, WriteFailure};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// 单次异常只丢失当前调用的结果，后续查询仍可继续使用索引。
fn recover_lock<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| {
        lock.clear_poison();
        poisoned.into_inner()
    })
}

#[derive(Debug, uniffi::Record)]
pub struct BacklinkInfo {
    pub source: String,
    pub line: u64,
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub in_diagram: bool,
    pub context: String,
    pub start_utf16: u64,
    pub length_utf16: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ResolvedLinkInfo {
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub in_diagram: bool,
    pub attachment: bool,
    pub resolution_kind: String,
    pub resolved_path: Option<String>,
    pub candidates: Vec<String>,
    pub line: u64,
    pub start_utf16: u64,
    pub length_utf16: u64,
}

#[derive(Debug, uniffi::Record)]
pub struct LinkResolutionInfo {
    pub resolution_kind: String,
    pub resolved_path: Option<String>,
    pub candidates: Vec<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct TagSummaryInfo {
    pub tag: String,
    pub raw: String,
    pub count: u64,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct InlineTagInfo {
    pub tag: String,
    pub start_utf16: u64,
    pub length_utf16: u64,
}

#[derive(Debug, uniffi::Record)]
pub struct KnowledgeMarkupInfo {
    pub links: Vec<ResolvedLinkInfo>,
    pub tags: Vec<InlineTagInfo>,
}

#[derive(Debug, uniffi::Record)]
pub struct LinkCompletionInfo {
    pub path: String,
    pub title: String,
    pub insert_target: String,
    pub matched_alias: Option<String>,
}

#[derive(Debug, uniffi::Record)]
pub struct RenameLinkEditInfo {
    pub source_path: String,
    pub line: u64,
    pub before: String,
    pub after: String,
    pub start_utf16: u64,
    pub length_utf16: u64,
    pub replacement: String,
}

#[derive(Debug, uniffi::Record)]
pub struct RenameLinkFileInfo {
    pub source_path: String,
    pub edits: Vec<RenameLinkEditInfo>,
}

#[derive(Debug, uniffi::Record)]
pub struct RenameLinkPlanInfo {
    pub affected_notes: u64,
    pub total_rewrites: u64,
    pub files: Vec<RenameLinkFileInfo>,
}

#[derive(Debug, uniffi::Record)]
pub struct RenameLinkFailureInfo {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, uniffi::Record)]
pub struct RenameLinksResultInfo {
    pub updated: Vec<String>,
    pub total_rewrites: u64,
    pub skipped: Vec<RenameLinkFailureInfo>,
    pub failed: Vec<RenameLinkFailureInfo>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct EmbedRunInfo {
    pub start: u64,
    pub length: u64,
    pub source_start_utf16: u64,
    pub source_length_utf16: u64,
    pub mask: u32,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct EmbedBlockInfo {
    pub id: u64,
    pub source_start_byte: u64,
    pub source_length_byte: u64,
    pub source_start_utf16: u64,
    pub source_length_utf16: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub diagram: bool,
    pub runs: Vec<EmbedRunInfo>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct EmbedDiagramInfo {
    pub ordinal: u32,
    pub start_utf16: u64,
    pub length_utf16: u64,
}

/// 只读嵌入内容：条目类型先于正文给出，平台据此决定卡片形态。
#[derive(Clone, Debug, uniffi::Record)]
pub struct EmbedContentInfo {
    /// `note` | `diagram` | `cycle` | `depth_exceeded` | `heading_missing` |
    /// `ambiguous` | `broken`
    pub kind: String,
    pub resolved_path: Option<String>,
    pub display_title: String,
    pub heading: Option<String>,
    pub heading_found: bool,
    pub is_mmd: bool,
    pub content: String,
    pub text: String,
    pub blocks: Vec<EmbedBlockInfo>,
    pub media: Vec<crate::media::MediaBlock>,
    pub math: Vec<crate::math::MathBlock>,
    pub diagrams: Vec<EmbedDiagramInfo>,
    pub links: Vec<ResolvedLinkInfo>,
    pub tags: Vec<InlineTagInfo>,
    pub candidates: Vec<String>,
}

impl From<LinkResolution> for LinkResolutionInfo {
    fn from(value: LinkResolution) -> Self {
        let (kind, resolved_path, candidates) = match value {
            LinkResolution::Note(path) => ("note", Some(path), vec![]),
            LinkResolution::Attachment => ("attachment", None, vec![]),
            LinkResolution::Ambiguous(paths) => ("ambiguous", None, paths),
            LinkResolution::Broken => ("broken", None, vec![]),
        };
        Self {
            resolution_kind: kind.into(),
            resolved_path,
            candidates,
        }
    }
}

// 顺序转换源码偏移，复用上一个位置的字节与 UTF-16 计数。
#[derive(Default)]
struct UTF16Offset {
    bytes: usize,
    units: u64,
}

impl UTF16Offset {
    fn range(&mut self, text: &str, start: usize, end: usize) -> (u64, u64) {
        if start < self.bytes {
            *self = Self::default();
        }
        self.units += text[self.bytes..start].encode_utf16().count() as u64;
        self.bytes = start;
        (self.units, text[start..end].encode_utf16().count() as u64)
    }

    fn byte_at(&mut self, text: &str, units: u64) -> usize {
        if units < self.units {
            *self = Self::default();
        }
        let mut characters = text[self.bytes..].chars();
        while self.units < units {
            let character = characters.next().expect("UTF-16 offset within source");
            self.units += character.len_utf16() as u64;
            self.bytes += character.len_utf8();
        }
        debug_assert_eq!(self.units, units);
        self.bytes
    }
}

fn resolved_links(text: &str, links: Vec<ResolvedLink>) -> Vec<ResolvedLinkInfo> {
    let mut offsets = UTF16Offset::default();
    links
        .into_iter()
        .map(|link| {
            let (start_utf16, length_utf16) = offsets.range(text, link.start, link.end);
            let info = LinkResolutionInfo::from(link.resolution);
            ResolvedLinkInfo {
                target: link.target,
                heading: link.heading,
                alias: link.alias,
                embed: link.embed,
                in_diagram: link.in_diagram,
                attachment: link.attachment,
                resolution_kind: info.resolution_kind,
                resolved_path: info.resolved_path,
                candidates: info.candidates,
                line: link.line as u64,
                start_utf16,
                length_utf16,
            }
        })
        .collect()
}

fn rename_plan_files(planned: &BTreeMap<String, PlannedFile>) -> Vec<RenameLinkFileInfo> {
    planned
        .iter()
        .map(|(path, file)| {
            let mut offsets = UTF16Offset::default();
            let edits = file
                .edits
                .iter()
                .map(|edit| {
                    let (start_utf16, length_utf16) =
                        offsets.range(&file.text, edit.start, edit.end);
                    RenameLinkEditInfo {
                        source_path: path.clone(),
                        line: edit.line as u64,
                        before: edit.before.clone(),
                        after: edit.after.clone(),
                        start_utf16,
                        length_utf16,
                        replacement: edit.after.clone(),
                    }
                })
                .collect();
            RenameLinkFileInfo {
                source_path: path.clone(),
                edits,
            }
        })
        .collect()
}

fn embed_title(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(dot) = name.rfind('.') else {
        return name.to_owned();
    };
    if dot == 0
        || !matches!(
            name[dot + 1..].to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mmd"
        )
    {
        return name.to_owned();
    }
    name[..dot].to_owned()
}

fn utf16_units(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn embed_blocks(content: &str, model: &hortus_document::reading::Model) -> Vec<EmbedBlockInfo> {
    let mut offsets = UTF16Offset::default();
    let mut display = 0;
    model
        .blocks
        .iter()
        .map(|block| {
            // 阅读模型的块范围按整篇正文的 UTF-16 计数。
            let source_start_utf16 = block.source.start as u64;
            let source_length_utf16 = block.source.len() as u64;
            let start = offsets.byte_at(content, source_start_utf16);
            let end = offsets.byte_at(content, block.source.end as u64);
            let runs = block
                .content
                .runs
                .iter()
                .map(|run| EmbedRunInfo {
                    start: run.display.start as u64,
                    length: run.display.len() as u64,
                    // 阅读模型的 run 源码范围已经是块内 UTF-16。
                    source_start_utf16: run.source.start as u64,
                    source_length_utf16: run.source.len() as u64,
                    mask: run.mask,
                })
                .collect();
            let info = EmbedBlockInfo {
                id: block.id,
                source_start_byte: start as u64,
                source_length_byte: (end - start) as u64,
                source_start_utf16,
                source_length_utf16,
                display_start: display as u64,
                display_length: block.content.display_len() as u64,
                diagram: block.content.diagram,
                runs,
            };
            display += block.content.display_len();
            info
        })
        .collect()
}

fn embed_math(model: &hortus_document::reading::Model) -> Vec<crate::math::MathBlock> {
    let mut result = Vec::new();
    let mut display = 0;
    for block in &model.blocks {
        for item in &block.content.math {
            result.push(crate::math::MathBlock {
                mask: item.mask,
                block_id: block.id,
                source_start: item.source.start as u64,
                source_length: item.source.len() as u64,
                display_start: (display + item.display.start) as u64,
                display_length: item.display.len() as u64,
                display_source: item.display_source.clone(),
                latex: item.latex.clone(),
                style: match item.style {
                    hortus_document::math::Style::Display => crate::math::MathStyle::Display,
                    hortus_document::math::Style::Text => crate::math::MathStyle::Text,
                },
                block: item.block,
            });
        }
        display += block.content.display_len();
    }
    result
}

#[derive(Default)]
pub(crate) struct RenamePlans {
    pending: BTreeMap<(String, String), RenamePreview>,
    folder_pending: BTreeMap<(String, String), FolderRenamePreview>,
}

struct RenamePreview {
    target_text: String,
    files: BTreeMap<String, PlannedFile>,
}

struct FolderRenamePreview {
    moves: BTreeMap<String, String>,
    files: BTreeMap<String, PlannedFile>,
}

struct PlannedFile {
    text: String,
    edits: Vec<LinkRewrite>,
}

impl VaultHandle {
    pub(crate) fn index_paths(&self, upserted: Vec<String>, removed: Vec<String>) {
        let updates = upserted
            .into_iter()
            .map(|path| {
                let text = self.vault.read_note(&path).ok();
                (path, text)
            })
            .collect::<Vec<_>>();
        self.index_contents(removed.into_iter().map(|path| (path, None)).chain(updates));
    }

    pub(crate) fn index_contents(
        &self,
        updates: impl IntoIterator<Item = (String, Option<String>)>,
    ) {
        let mut search = recover_lock(&self.search);
        let mut links = recover_lock(&self.links);
        for (path, text) in updates {
            if let Some(text) = text {
                if links.text(&path) == Some(&text) {
                    continue;
                }
                links.upsert(path.clone(), text.clone());
                search.upsert(path, text);
            } else {
                links.remove(&path);
                search.remove(&path);
            }
        }
    }

    fn rewrite_rename(
        &self,
        old_path: String,
        new_path: String,
        skip_paths: Vec<String>,
        temporary: &Path,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        let root = self.vault.root();
        let old_absolute = note::note_path(root, &old_path)?;
        let new_absolute = note::note_path(root, &new_path)?;
        if !new_absolute.is_file() || old_path == new_path {
            return Err(CoreError::failure("vault.invalid_path"));
        }
        // APFS 的仅大小写变化在旧拼写下也能读取，以磁盘实际名称为准。
        if let Some(parent) = old_absolute.parent()
            && parent.exists()
            && std::fs::read_dir(parent)?.any(|entry| {
                entry.is_ok_and(|entry| {
                    Some(entry.file_name().as_os_str()) == old_absolute.file_name()
                })
            })
        {
            return Err(CoreError::failure("knowledge.rename_required"));
        }
        let _update = recover_lock(&self.index_update);
        let preview = recover_lock(&self.rename_plans)
            .pending
            .remove(&(old_path.clone(), new_path.clone()))
            .ok_or_else(|| CoreError::failure("knowledge.rename_plan_missing"))?;
        if self.vault.read_note(&new_path)? != preview.target_text {
            return Err(CoreError::failure("knowledge.changed_on_disk"));
        }
        let skip_paths = skip_paths.into_iter().collect::<BTreeSet<_>>();
        let mut result = RenameLinksResultInfo {
            updated: vec![],
            total_rewrites: 0,
            skipped: vec![],
            failed: vec![],
        };
        for (source, file) in preview.files {
            let path = if source == old_path {
                &new_path
            } else {
                &source
            };
            if skip_paths.contains(&source) || skip_paths.contains(path) {
                result.skipped.push(RenameLinkFailureInfo {
                    path: path.to_owned(),
                    reason: "knowledge.unsaved".into(),
                });
                continue;
            }
            match self.rewrite_file(path, &file, temporary) {
                Ok(()) => {
                    result.updated.push(path.to_owned());
                    result.total_rewrites += file.edits.len() as u64;
                }
                Err(error) => result.failed.push(RenameLinkFailureInfo {
                    path: path.to_owned(),
                    reason: error.to_string(),
                }),
            }
        }
        let mut changed = result.updated.clone();
        if !changed.contains(&new_path) {
            changed.push(new_path);
        }
        self.index_paths(changed, vec![old_path]);
        Ok(result)
    }

    /// 文件夹改名后执行已确认的链接计划。受影响的来源可能在刚搬走的文件夹里，
    /// 写盘前先按 `moves` 映射到实际新路径，`skip_paths` 新旧写法都接受。
    fn rewrite_folder_rename(
        &self,
        old_dir: String,
        new_dir: String,
        skip_paths: Vec<String>,
        temporary: &Path,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        let root = self.vault.root();
        let old_absolute = note::note_path(root, &old_dir)?;
        let new_absolute = note::note_path(root, &new_dir)?;
        if old_dir == new_dir || !new_absolute.is_dir() {
            return Err(CoreError::failure("vault.invalid_path"));
        }
        if new_dir.starts_with(&format!("{old_dir}/")) {
            return Err(CoreError::failure("vault.invalid_path"));
        }
        // 旧目录名仍在磁盘上（含 APFS 仅大小写变化）说明改名尚未发生。
        if let Some(parent) = old_absolute.parent()
            && parent.exists()
            && std::fs::read_dir(parent)?.any(|entry| {
                entry.is_ok_and(|entry| {
                    Some(entry.file_name().as_os_str()) == old_absolute.file_name()
                })
            })
        {
            return Err(CoreError::failure("knowledge.rename_required"));
        }
        let _update = recover_lock(&self.index_update);
        let preview = recover_lock(&self.rename_plans)
            .folder_pending
            .remove(&(old_dir.clone(), new_dir.clone()))
            .ok_or_else(|| CoreError::failure("knowledge.rename_plan_missing"))?;
        let skip_paths = skip_paths.into_iter().collect::<BTreeSet<_>>();
        let mut result = RenameLinksResultInfo {
            updated: vec![],
            total_rewrites: 0,
            skipped: vec![],
            failed: vec![],
        };
        for (source, file) in &preview.files {
            let path = preview.moves.get(source).unwrap_or(source);
            if skip_paths.contains(source) || skip_paths.contains(path) {
                result.skipped.push(RenameLinkFailureInfo {
                    path: path.clone(),
                    reason: "knowledge.unsaved".into(),
                });
                continue;
            }
            match self.rewrite_file(path, file, temporary) {
                Ok(()) => {
                    result.updated.push(path.clone());
                    result.total_rewrites += file.edits.len() as u64;
                }
                Err(error) => result.failed.push(RenameLinkFailureInfo {
                    path: path.clone(),
                    reason: error.to_string(),
                }),
            }
        }
        // 未改写的搬移笔记也要把索引换成新路径，否则改名后的嵌入与反链会暂时断掉。
        let mut upserted = result.updated.clone();
        for path in preview.moves.values() {
            if !upserted.contains(path) {
                upserted.push(path.clone());
            }
        }
        self.index_paths(upserted, preview.moves.keys().cloned().collect());
        Ok(result)
    }

    fn rewrite_file(
        &self,
        path: &str,
        file: &PlannedFile,
        temporary: &Path,
    ) -> Result<(), CoreError> {
        let root = self.vault.root();
        let absolute = note::note_path(root, path)?;
        let (safe, directory) = self.vault.resolve_trash_target(path)?;
        if directory || safe != absolute {
            return Err(CoreError::failure("vault.invalid_path"));
        }
        let baseline = note::read(&absolute)?.ok_or_else(|| CoreError::failure("io.not_found"))?;
        let (format, normalized, valid) = note::Format::decode(&baseline.bytes);
        if !valid || normalized != file.text {
            return Err(CoreError::failure("knowledge.changed_on_disk"));
        }
        // 以原始字节写回。CRLF、LF（含混用）与 BOM 都保持原样。
        let mut bytes = baseline.bytes.clone();
        let mut displacement = if format.bom { 3 } else { 0 };
        let mut raw = displacement;
        let mut normalized_pos = 0;
        let mut edits = Vec::with_capacity(file.edits.len());
        for edit in &file.edits {
            while normalized_pos < edit.start {
                if bytes[raw..].starts_with(b"\r\n") {
                    displacement += 1;
                    raw += 1;
                }
                raw += 1;
                normalized_pos += 1;
            }
            let range = edit.start + displacement..edit.end + displacement;
            if bytes.get(range.clone()) != Some(edit.before.as_bytes()) {
                return Err(CoreError::failure("knowledge.changed_on_disk"));
            }
            edits.push((range, &edit.after));
        }
        for (range, replacement) in edits.into_iter().rev() {
            bytes.splice(range, replacement.bytes());
        }
        note::atomic_replace(&absolute, &bytes, Some(&baseline), temporary, root, None).map_err(
            |error| match error {
                WriteFailure::Changed => CoreError::failure("knowledge.changed_on_disk"),
                WriteFailure::Missing => CoreError::failure("io.not_found"),
                WriteFailure::Io(error) => error.into(),
            },
        )?;
        Ok(())
    }
}

#[uniffi::export]
impl VaultHandle {
    pub fn backlinks(&self, path: String) -> Vec<BacklinkInfo> {
        let index = recover_lock(&self.links);
        let mut offsets = UTF16Offset::default();
        let mut source = String::new();
        index
            .backlinks(&path)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|link| {
                if source != link.source {
                    source.clone_from(&link.source);
                    offsets = UTF16Offset::default();
                }
                let text = index.text(&source)?;
                let (start_utf16, length_utf16) = offsets.range(text, link.start, link.end);
                Some(BacklinkInfo {
                    source: link.source,
                    target: link.target,
                    heading: link.heading,
                    alias: link.alias,
                    embed: link.embed,
                    in_diagram: link.in_diagram,
                    context: link.context,
                    line: link.line as u64,
                    start_utf16,
                    length_utf16,
                })
            })
            .collect()
    }

    pub fn links(&self, path: String) -> Vec<ResolvedLinkInfo> {
        let index = recover_lock(&self.links);
        resolved_links(
            index.text(&path).unwrap_or_default(),
            index.links(&path).unwrap_or_default(),
        )
    }

    pub fn knowledge_markup(&self, path: String, text: String) -> KnowledgeMarkupInfo {
        let (links, tags) = recover_lock(&self.links).markup(&path, &text);
        let mut offsets = UTF16Offset::default();
        let tags = tags
            .into_iter()
            .map(|tag| {
                let (start_utf16, length_utf16) = offsets.range(&text, tag.start, tag.end);
                InlineTagInfo {
                    tag: tag.key,
                    start_utf16,
                    length_utf16,
                }
            })
            .collect();
        KnowledgeMarkupInfo {
            links: resolved_links(&text, links),
            tags,
        }
    }

    pub fn tags(&self) -> Vec<TagSummaryInfo> {
        recover_lock(&self.links)
            .tags()
            .into_iter()
            .map(|tag| TagSummaryInfo {
                tag: tag.tag,
                raw: tag.raw,
                count: tag.count as u64,
                notes: tag.notes,
            })
            .collect()
    }

    pub fn notes_with_tag(&self, tag: String) -> Vec<String> {
        recover_lock(&self.links).notes_with_tag(&tag)
    }

    pub fn resolve_link(&self, source_path: String, raw_target: String) -> LinkResolutionInfo {
        recover_lock(&self.links)
            .resolve(&source_path, &raw_target)
            .into()
    }

    pub fn link_completions_at(
        &self,
        text: String,
        link_start: u64,
        query: String,
        limit: u32,
    ) -> Vec<LinkCompletionInfo> {
        let mut offsets = UTF16Offset::default();
        let valid = hortus_links::parse_note(&text)
            .links
            .into_iter()
            .any(|link| {
                !link.in_diagram && offsets.range(&text, link.start, link.end).0 == link_start
            });
        if valid {
            self.link_completions(query, limit)
        } else {
            vec![]
        }
    }

    pub fn link_completions(&self, query: String, limit: u32) -> Vec<LinkCompletionInfo> {
        recover_lock(&self.links)
            .completions(&query, limit as usize)
            .into_iter()
            .map(|hit| LinkCompletionInfo {
                path: hit.path,
                title: hit.title,
                insert_target: hit.insert_target,
                matched_alias: hit.matched_alias,
            })
            .collect()
    }

    pub fn plan_rename_links(&self, old_path: String, new_path: String) -> RenameLinkPlanInfo {
        let index = recover_lock(&self.links);
        let mut plans = recover_lock(&self.rename_plans);
        let mut planned: BTreeMap<String, PlannedFile> = BTreeMap::new();
        if note::note_path(self.vault.root(), &old_path).is_ok()
            && note::note_path(self.vault.root(), &new_path).is_ok()
        {
            for edit in index.plan_rename_links(&old_path, &new_path) {
                let Some(text) = index.text(&edit.source) else {
                    continue;
                };
                planned
                    .entry(edit.source.clone())
                    .or_insert_with(|| PlannedFile {
                        text: text.to_owned(),
                        edits: vec![],
                    })
                    .edits
                    .push(edit);
            }
        }
        let files: Vec<_> = planned
            .iter()
            .map(|(path, file)| {
                let mut offsets = UTF16Offset::default();
                let edits = file
                    .edits
                    .iter()
                    .map(|edit| {
                        let (start_utf16, length_utf16) =
                            offsets.range(&file.text, edit.start, edit.end);
                        RenameLinkEditInfo {
                            source_path: path.clone(),
                            line: edit.line as u64,
                            before: edit.before.clone(),
                            after: edit.after.clone(),
                            start_utf16,
                            length_utf16,
                            replacement: edit.after.clone(),
                        }
                    })
                    .collect();
                RenameLinkFileInfo {
                    source_path: path.clone(),
                    edits,
                }
            })
            .collect();
        plans.pending.retain(|(old, _), _| old != &old_path);
        if !planned.is_empty()
            && let Some(target_text) = index.text(&old_path)
        {
            plans.pending.insert(
                (old_path, new_path),
                RenamePreview {
                    target_text: target_text.into(),
                    files: planned,
                },
            );
        }
        RenameLinkPlanInfo {
            affected_notes: files.len() as u64,
            total_rewrites: files.iter().map(|file| file.edits.len() as u64).sum(),
            files,
        }
    }

    pub fn apply_rename_links(
        &self,
        old_path: String,
        new_path: String,
        skip_paths: Vec<String>,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        self.rewrite_rename(old_path, new_path, skip_paths, &std::env::temp_dir())
    }

    /// macOS 为外接卷提供与笔记同卷的系统替换目录。
    pub fn apply_rename_links_in(
        &self,
        old_path: String,
        new_path: String,
        skip_paths: Vec<String>,
        temporary: String,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        self.rewrite_rename(old_path, new_path, skip_paths, Path::new(&temporary))
    }

    /// 阅读视图的只读嵌入解析；不写盘，目标内容按当前索引或磁盘最新正文切片。
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_embed(
        &self,
        source_path: String,
        raw_target: String,
        heading: Option<String>,
        alias: Option<String>,
        ancestors: Vec<String>,
        max_depth: u32,
        width: f64,
        size: f64,
    ) -> Result<EmbedContentInfo, CoreError> {
        let normalized_source =
            hortus_links::normalize_note_path(&source_path).unwrap_or_else(|| source_path.clone());
        let (resolution, text) = {
            let index = recover_lock(&self.links);
            let resolution = index.resolve(&normalized_source, &raw_target);
            let text = match &resolution {
                LinkResolution::Note(path) => index.text(path).map(str::to_owned),
                _ => None,
            };
            (resolution, text)
        };
        let base = EmbedContentInfo {
            kind: String::new(),
            resolved_path: None,
            display_title: String::new(),
            heading: heading.clone(),
            heading_found: true,
            is_mmd: false,
            content: String::new(),
            text: String::new(),
            blocks: Vec::new(),
            media: Vec::new(),
            math: Vec::new(),
            diagrams: Vec::new(),
            links: Vec::new(),
            tags: Vec::new(),
            candidates: Vec::new(),
        };
        let resolved = match resolution {
            LinkResolution::Note(path) => path,
            LinkResolution::Attachment => {
                return Ok(EmbedContentInfo {
                    kind: "attachment".into(),
                    ..base
                });
            }
            LinkResolution::Ambiguous(candidates) => {
                return Ok(EmbedContentInfo {
                    kind: "ambiguous".into(),
                    candidates,
                    ..base
                });
            }
            LinkResolution::Broken => {
                return Ok(EmbedContentInfo {
                    kind: "broken".into(),
                    ..base
                });
            }
        };
        let cycle = resolved == normalized_source
            || ancestors.iter().any(|path| {
                hortus_links::normalize_note_path(path).as_deref() == Some(resolved.as_str())
            });
        let display_title = alias
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| embed_title(&resolved));
        if cycle {
            return Ok(EmbedContentInfo {
                kind: "cycle".into(),
                resolved_path: Some(resolved),
                display_title,
                ..base
            });
        }
        if ancestors.len() > max_depth as usize {
            return Ok(EmbedContentInfo {
                kind: "depth_exceeded".into(),
                resolved_path: Some(resolved),
                display_title,
                ..base
            });
        }
        let text = match text {
            Some(text) => text,
            None => self.vault.read_note(&resolved)?,
        };
        let title = alias
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| embed_title(&resolved));
        let is_mmd = hortus_links::is_mmd_path(&resolved);
        if is_mmd {
            return Ok(EmbedContentInfo {
                kind: "diagram".into(),
                resolved_path: Some(resolved),
                display_title: title,
                heading,
                heading_found: true,
                is_mmd: true,
                diagrams: vec![EmbedDiagramInfo {
                    ordinal: 1,
                    start_utf16: 0,
                    length_utf16: utf16_units(&text),
                }],
                content: text,
                ..base
            });
        }
        let (content, heading_found) =
            if let Some(target) = heading.as_deref().filter(|value| !value.trim().is_empty()) {
                match embeds::heading_section(&text, target) {
                    Some(section) => (text[section.range].to_owned(), true),
                    None => {
                        return Ok(EmbedContentInfo {
                            kind: "heading_missing".into(),
                            resolved_path: Some(resolved),
                            display_title: title,
                            heading,
                            heading_found: false,
                            ..base
                        });
                    }
                }
            } else {
                (text[embeds::body_start(&text)..].to_owned(), true)
            };
        let mut document = Document::new(&content);
        let model = document.reading(false)?;
        let (links, tags) = recover_lock(&self.links).markup(&resolved, &content);
        let links = resolved_links(&content, links);
        let mut tag_offsets = UTF16Offset::default();
        let tags = tags
            .into_iter()
            .map(|tag| {
                let (start_utf16, length_utf16) = tag_offsets.range(&content, tag.start, tag.end);
                InlineTagInfo {
                    tag: tag.key,
                    start_utf16,
                    length_utf16,
                }
            })
            .collect();
        let width = if width.is_finite() && width > 0.0 {
            width
        } else {
            700.0
        };
        let size = if size.is_finite() && size > 0.0 {
            size
        } else {
            15.0
        };
        let resolver = hortus_document::media::ImageResolver::default();
        let media = crate::media::blocks(
            &model,
            self.vault.root(),
            Path::new(&resolved),
            &resolver,
            width,
            size,
        )?;
        Ok(EmbedContentInfo {
            kind: "note".into(),
            resolved_path: Some(resolved),
            display_title: title,
            heading,
            heading_found,
            is_mmd: false,
            blocks: embed_blocks(&content, &model),
            media,
            math: embed_math(&model),
            diagrams: Document::mermaid_ranges(&content)
                .into_iter()
                .map(|(ordinal, start, length)| EmbedDiagramInfo {
                    ordinal,
                    start_utf16: start,
                    length_utf16: length,
                })
                .collect(),
            links,
            tags,
            content,
            text: model
                .blocks
                .iter()
                .map(|block| block.content.text.as_str())
                .collect(),
            ..base
        })
    }

    pub fn plan_folder_rename_links(&self, old_dir: String, new_dir: String) -> RenameLinkPlanInfo {
        let index = recover_lock(&self.links);
        let mut plans = recover_lock(&self.rename_plans);
        let plan = if note::note_path(self.vault.root(), &old_dir).is_ok()
            && note::note_path(self.vault.root(), &new_dir).is_ok()
        {
            index.plan_folder_rename_links(&old_dir, &new_dir)
        } else {
            Default::default()
        };
        let mut planned: BTreeMap<String, PlannedFile> = BTreeMap::new();
        for edit in plan.rewrites {
            let Some(text) = index.text(&edit.source) else {
                continue;
            };
            planned
                .entry(edit.source.clone())
                .or_insert_with(|| PlannedFile {
                    text: text.to_owned(),
                    edits: vec![],
                })
                .edits
                .push(edit);
        }
        let files = rename_plan_files(&planned);
        let moves = plan
            .moves
            .into_iter()
            .map(|item| (item.old_path, item.new_path))
            .collect::<BTreeMap<_, _>>();
        plans.folder_pending.retain(|(old, _), _| old != &old_dir);
        if !moves.is_empty() || !planned.is_empty() {
            plans.folder_pending.insert(
                (old_dir, new_dir),
                FolderRenamePreview {
                    moves,
                    files: planned,
                },
            );
        }
        RenameLinkPlanInfo {
            affected_notes: files.len() as u64,
            total_rewrites: files.iter().map(|file| file.edits.len() as u64).sum(),
            files,
        }
    }

    pub fn apply_folder_rename_links(
        &self,
        old_dir: String,
        new_dir: String,
        skip_paths: Vec<String>,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        self.rewrite_folder_rename(old_dir, new_dir, skip_paths, &std::env::temp_dir())
    }

    pub fn apply_folder_rename_links_in(
        &self,
        old_dir: String,
        new_dir: String,
        skip_paths: Vec<String>,
        temporary: String,
    ) -> Result<RenameLinksResultInfo, CoreError> {
        self.rewrite_folder_rename(old_dir, new_dir, skip_paths, Path::new(&temporary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn poison<T>(lock: &Mutex<T>) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = lock.lock().unwrap();
            panic!("inject lock poison");
        }));
        assert!(lock.is_poisoned());
    }

    #[test]
    fn poisoned_vault_locks_recover_for_queries_and_rename() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("vault");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.md"), "[[b]] #tag").unwrap();
        std::fs::write(root.join("b.md"), "").unwrap();
        let core = crate::WorkspaceCore::in_directories(
            temporary.path().join("config").to_string_lossy().into(),
            temporary.path().join("cache").to_string_lossy().into(),
        )
        .unwrap();
        let handle = core
            .open_vault(root.to_string_lossy().into_owned())
            .unwrap();
        handle.update_search_index(vec!["a.md".into(), "b.md".into()], vec![]);

        poison(&handle.search);
        poison(&handle.links);
        poison(&handle.index_update);
        poison(&handle.rename_plans);

        assert_eq!(handle.backlinks("b.md".into()).len(), 1);
        assert_eq!(handle.links("a.md".into()).len(), 1);
        assert_eq!(
            handle
                .knowledge_markup("a.md".into(), "[[b]] #fresh".into())
                .tags
                .len(),
            1
        );
        assert_eq!(handle.tags().len(), 1);
        assert_eq!(handle.notes_with_tag("tag".into()), vec!["a.md"]);
        assert_eq!(
            handle
                .resolve_link("a.md".into(), "b".into())
                .resolved_path
                .as_deref(),
            Some("b.md")
        );
        assert_eq!(handle.link_completions("b".into(), 5).len(), 1);
        assert_eq!(
            handle
                .link_completions_at("[[b]]".into(), 0, "b".into(), 5)
                .len(),
            1
        );

        let plan = handle.plan_rename_links("b.md".into(), "c.md".into());
        assert_eq!(plan.total_rewrites, 1);
        handle.rename_entry("b.md".into(), "c.md".into()).unwrap();
        let result = handle
            .apply_rename_links("b.md".into(), "c.md".into(), vec![])
            .unwrap();
        assert_eq!(result.updated, vec!["a.md"]);
        assert_eq!(result.total_rewrites, 1);
        assert_eq!(
            std::fs::read_to_string(root.join("a.md")).unwrap(),
            "[[c]] #tag"
        );
        assert!(!handle.search.is_poisoned());
        assert!(!handle.links.is_poisoned());
        assert!(!handle.index_update.is_poisoned());
        assert!(!handle.rename_plans.is_poisoned());
        handle.close();
    }
}
