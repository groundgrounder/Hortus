//! 媒体布局在文档队列中完成，平台只消费结构和几何。
use crate::{CoreError, document::DocumentSession};
use hortus_diagram::SystemFonts;
use hortus_document::{media, reading};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, uniffi::Record)]
pub struct MediaRun {
    pub start: u64,
    pub length: u64,
    pub mask: u32,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct TableCell {
    pub text: String,
    pub runs: Vec<MediaRun>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct MediaBlock {
    pub block_id: u64,
    pub source_start: u64,
    pub source_length: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub columns: Vec<f64>,
    pub minimum_column: f64,
    pub alignments: Vec<u8>,
    pub cells: Vec<TableCell>,
    pub rows: u64,
    pub image_target: String,
    pub image_alt: String,
    pub image_path: String,
    pub image_reason: String,
    pub image_wiki: bool,
    pub image_width: Option<u32>,
}

#[derive(uniffi::Record)]
pub struct AttachmentMetrics {
    pub files: u64,
    pub directories: u64,
    pub build_micros: u64,
    pub estimated_bytes: u64,
    pub builds: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct TablePatch {
    pub start: u64,
    pub length: u64,
    pub replacement: String,
}

pub(crate) fn blocks(
    model: &reading::Model,
    root: &std::path::Path,
    note: &std::path::Path,
    resolver: &media::ImageResolver,
    width: f64,
    size: f64,
) -> Result<Vec<MediaBlock>, CoreError> {
    static FONTS: OnceLock<Mutex<Option<SystemFonts>>> = OnceLock::new();
    let mut result = Vec::new();
    if model
        .blocks
        .iter()
        .all(|block| block.content.media.is_empty())
    {
        return Ok(result);
    }
    let mut display = 0;
    for block in &model.blocks {
        for item in &block.content.media {
            let mut wire = MediaBlock {
                block_id: block.id,
                source_start: (block.source.start + item.source.start) as u64,
                source_length: item.source.len() as u64,
                display_start: (display + item.display.start) as u64,
                display_length: item.display.len() as u64,
                columns: Vec::new(),
                minimum_column: 0.0,
                alignments: Vec::new(),
                cells: Vec::new(),
                rows: 0,
                image_target: String::new(),
                image_alt: String::new(),
                image_path: String::new(),
                image_reason: String::new(),
                image_wiki: false,
                image_width: None,
            };
            match &item.kind {
                media::Kind::Table(table) => {
                    let mut fonts = FONTS.get_or_init(|| Mutex::new(None)).lock().unwrap();
                    if fonts.is_none() {
                        *fonts = Some(
                            SystemFonts::new().map_err(|_| CoreError::failure("diagram.font"))?,
                        );
                    }
                    let layout = table.layout(width, size, fonts.as_mut().unwrap());
                    wire.columns = layout.widths;
                    wire.minimum_column = layout.minimum;
                    wire.alignments = table
                        .alignments
                        .iter()
                        .map(|align| match align {
                            media::Align::Left => 0,
                            media::Align::Center => 1,
                            media::Align::Right => 2,
                        })
                        .collect();
                    wire.rows = table.rows.len() as u64;
                    for row in &table.rows {
                        for column in 0..table.alignments.len() {
                            wire.cells.push(row.get(column).map_or_else(
                                || TableCell {
                                    text: String::new(),
                                    runs: Vec::new(),
                                },
                                |cell| {
                                    TableCell {
                                        text: cell.text.clone(),
                                        runs: cell
                                            .runs
                                            .iter()
                                            .map(|run| MediaRun {
                                                start: run.display.start as u64,
                                                length: run.display.len() as u64,
                                                mask: run.mask,
                                            })
                                            .collect(),
                                    }
                                },
                            ));
                        }
                    }
                }
                media::Kind::Image(image) => {
                    wire.image_target.clone_from(&image.target);
                    wire.image_alt.clone_from(&image.alt);
                    wire.image_wiki = image.wiki;
                    wire.image_width = image.width;
                    match resolver.resolve(root, note, &image.target, image.wiki) {
                        media::ImagePath::Local(path) => {
                            wire.image_path = path.to_string_lossy().into()
                        }
                        media::ImagePath::Rejected(reason) => wire.image_reason = reason.into(),
                    }
                }
            }
            result.push(wire);
        }
        display += block.content.display_len();
    }
    Ok(result)
}

#[uniffi::export]
impl DocumentSession {
    pub fn media_blocks(&self, width: f64, size: f64) -> Result<Vec<MediaBlock>, CoreError> {
        let mut state = self.state.lock().unwrap();
        let revision = state.document.revision();
        if state
            .reading_snapshot
            .as_ref()
            .is_none_or(|(cached, _)| *cached != revision)
        {
            let mmd = state.relative.to_lowercase().ends_with(".mmd");
            let model = state.document.reading(mmd)?;
            state.reading_snapshot = Some((revision, model));
        }
        let model = &state.reading_snapshot.as_ref().unwrap().1;
        blocks(
            model,
            &state.root,
            std::path::Path::new(&state.relative),
            &state.image_resolver,
            width,
            size,
        )
    }
    pub fn attachment_metrics(&self) -> AttachmentMetrics {
        let stats = self.state.lock().unwrap().image_resolver.stats();
        AttachmentMetrics {
            files: stats.files,
            directories: stats.directories,
            build_micros: stats.build_micros,
            estimated_bytes: stats.estimated_bytes,
            builds: stats.builds,
        }
    }
    pub fn align_table(&self, caret: u64) -> Result<Option<TablePatch>, CoreError> {
        let state = self.state.lock().unwrap();
        Ok(
            media::align_table(&state.document.text()?, caret as usize).map(|patch| TablePatch {
                start: patch.source.start as u64,
                length: patch.source.len() as u64,
                replacement: patch.replacement,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::WorkspaceCore;
    use std::fs;

    #[test]
    fn reopened_document_finds_external_attachment_changes_without_vault_state() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        fs::create_dir_all(root.join("attachments")).unwrap();
        fs::write(root.join("note.md"), "![[new.png|80]]\n\n![[Other note]]\n").unwrap();
        let core = WorkspaceCore::in_directories(
            directory.path().join("config").to_string_lossy().into(),
            directory.path().join("cache").to_string_lossy().into(),
        )
        .unwrap();
        let open = || {
            core.open_document(root.to_string_lossy().into(), "note.md".into())
                .unwrap()
        };
        let first = open();
        assert_eq!(
            first.media_blocks(700.0, 15.0).unwrap()[0].image_reason,
            "image.missing"
        );
        assert_eq!(first.attachment_metrics().builds, 1);
        fs::write(root.join("attachments/new.png"), []).unwrap();
        let reopened = open();
        let blocks = reopened.media_blocks(700.0, 15.0).unwrap();
        assert!(blocks[0].image_path.ends_with("/attachments/new.png"));
        assert_eq!(blocks[0].image_width, Some(80));
        assert_eq!(blocks[1].image_reason, "embed.note");
        assert_eq!(reopened.attachment_metrics().builds, 2);
        reopened.media_blocks(600.0, 15.0).unwrap();
        assert_eq!(reopened.attachment_metrics().builds, 2);
        fs::remove_file(root.join("attachments/new.png")).unwrap();
        assert_eq!(
            open().media_blocks(700.0, 15.0).unwrap()[0].image_reason,
            "image.missing"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert_eq!(fs::read_dir(root.join("attachments")).unwrap().count(), 0);
    }
}
