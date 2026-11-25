// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 后台阅读模型解码与全量媒体/公式/Mermaid/嵌入准备（完全在工作线程执行）。

use crate::core::model::HeadingItem;
use crate::render::diagram::{RenderedDiagram, format_diagram_failure, render_block_with_engine};
use crate::render::diagram_card::diagram_render_width;
use crate::render::embed::{EmbedRequest, PreparedEmbed, prepare_embed_bg};
use crate::render::math::{MathRenderService, PreparedMathBlock};
use crate::render::media::{PreparedMedia, prepare_media_blocks_bg};
use hortus_ffi::{
    DiagramEngine, DiagramIdentity, DocumentSession, KnowledgeMarkupInfo, ReadingModel,
    ResolvedLinkInfo, VaultHandle, WorkspaceCore,
};
#[cfg(feature = "automation")]
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::ThreadId;

#[derive(Clone, Debug)]
pub struct DecodedBlock {
    pub id: u64,
    pub source_start: u64,
    pub source_length: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub run_start: usize,
    pub run_len: usize,
    pub diagram: bool,
}

#[derive(Clone, Debug)]
pub struct DecodedRun {
    pub start: u64,
    pub length: u64,
    pub source_start: u64,
    pub source_length: u64,
    pub mask: u32,
}

#[derive(Clone, Debug)]
pub struct DecodedReadingModel {
    pub text: String,
    pub blocks: Vec<DecodedBlock>,
    pub runs: Vec<DecodedRun>,
}

fn read_u32_le(data: &[u8], off: usize) -> Result<u32, String> {
    let slice = data
        .get(off..off + 4)
        .ok_or_else(|| crate::i18n::tr(crate::i18n::Key::ModelCorrupt).to_string())?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u64_le(data: &[u8], off: usize) -> Result<u64, String> {
    let slice = data
        .get(off..off + 8)
        .ok_or_else(|| crate::i18n::tr(crate::i18n::Key::ModelCorrupt).to_string())?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

impl DecodedReadingModel {
    pub fn decode(wire: &ReadingModel) -> Result<Self, String> {
        let b_data = &wire.packed_blocks;
        let r_data = &wire.packed_runs;
        if b_data.len() < 8 || r_data.len() < 8 {
            return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into());
        }
        let b_ver = read_u32_le(b_data, 0)?;
        let b_cnt = read_u32_le(b_data, 4)? as usize;
        let b_rec = match b_ver {
            1 => 64usize,
            2 => 36usize,
            _ => return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into()),
        };
        if b_data.len() != 8 + b_cnt * b_rec {
            return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into());
        }

        let r_ver = read_u32_le(r_data, 0)?;
        let r_cnt = read_u32_le(r_data, 4)? as usize;
        let r_rec = match r_ver {
            1 => 40usize,
            2 => 20usize,
            _ => return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into()),
        };
        if r_data.len() != 8 + r_cnt * r_rec {
            return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into());
        }

        let mut blocks = Vec::with_capacity(b_cnt);
        for i in 0..b_cnt {
            let base = 8 + i * b_rec;
            let (id, source_start, source_length, display_start, display_length, rs, rl, diagram) =
                if b_ver == 2 {
                    (
                        read_u64_le(b_data, base)?,
                        u64::from(read_u32_le(b_data, base + 8)?),
                        u64::from(read_u32_le(b_data, base + 12)?),
                        u64::from(read_u32_le(b_data, base + 16)?),
                        u64::from(read_u32_le(b_data, base + 20)?),
                        read_u32_le(b_data, base + 24)? as usize,
                        read_u32_le(b_data, base + 28)? as usize,
                        read_u32_le(b_data, base + 32)? != 0,
                    )
                } else {
                    (
                        read_u64_le(b_data, base)?,
                        read_u64_le(b_data, base + 8)?,
                        read_u64_le(b_data, base + 16)?,
                        read_u64_le(b_data, base + 24)?,
                        read_u64_le(b_data, base + 32)?,
                        read_u64_le(b_data, base + 40)? as usize,
                        read_u64_le(b_data, base + 48)? as usize,
                        read_u64_le(b_data, base + 56)? != 0,
                    )
                };
            if rs + rl > r_cnt {
                return Err(crate::i18n::tr(crate::i18n::Key::ModelCorrupt).into());
            }
            blocks.push(DecodedBlock {
                id,
                source_start,
                source_length,
                display_start,
                display_length,
                run_start: rs,
                run_len: rl,
                diagram,
            });
        }

        let mut runs = Vec::with_capacity(r_cnt);
        for i in 0..r_cnt {
            let base = 8 + i * r_rec;
            let (start, length, source_start, source_length, mask) = if r_ver == 2 {
                (
                    u64::from(read_u32_le(r_data, base)?),
                    u64::from(read_u32_le(r_data, base + 4)?),
                    u64::from(read_u32_le(r_data, base + 8)?),
                    u64::from(read_u32_le(r_data, base + 12)?),
                    read_u32_le(r_data, base + 16)?,
                )
            } else {
                (
                    read_u64_le(r_data, base)?,
                    read_u64_le(r_data, base + 8)?,
                    read_u64_le(r_data, base + 16)?,
                    read_u64_le(r_data, base + 24)?,
                    read_u64_le(r_data, base + 32)? as u32,
                )
            };
            runs.push(DecodedRun {
                start,
                length,
                source_start,
                source_length,
                mask,
            });
        }

        Ok(Self {
            text: wire.text.clone(),
            blocks,
            runs,
        })
    }
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default, Serialize)]
pub struct NoteReadingStats {
    pub diagram_count: usize,
    pub diagram_rendered: usize,
    pub diagram_fallback: usize,
    pub diagram_collapsed_count: usize,
    pub diagram_kinds: Vec<String>,
    pub math_total: usize,
    pub math_rendered: usize,
    pub math_fallback_error: usize,
    pub math_cjk_fallback_codepoints: Vec<u32>,
    pub math_feedback_rounds: usize,
    pub table_count: usize,
    pub local_image_count: usize,
    pub remote_image_count: usize,
    pub sanitized_svg_external_refs: usize,
    pub sanitized_svg_scripts: usize,
    pub embed_count: usize,
    pub embed_kinds: Vec<String>,
    pub wikilink_count: usize,
    pub prepared_off_main_thread: bool,
}

pub struct PreparedReadingNote {
    pub path: String,
    pub is_mmd: bool,
    pub revision: u64,
    pub source_text: String,
    pub headings: Vec<HeadingItem>,
    pub model: DecodedReadingModel,
    pub diagrams: Vec<RenderedDiagram>,
    pub math: Vec<PreparedMathBlock>,
    pub media: Vec<PreparedMedia>,
    pub knowledge: Option<KnowledgeMarkupInfo>,
    pub embeds: HashMap<usize, PreparedEmbed>,
    #[allow(dead_code)]
    pub prepared_off_main_thread: bool,
}

pub struct PrepareReadingRequest {
    pub note_path: String,
    pub vault_root: String,
    pub session: Arc<DocumentSession>,
    pub core: Arc<WorkspaceCore>,
    pub handle: Option<Arc<VaultHandle>>,
    pub diagram_engine: Arc<DiagramEngine>,
    pub math_service: Arc<MathRenderService>,
    pub content_width: f64,
    pub font_size: f64,
    pub dark: bool,
    pub main_thread: ThreadId,
}

/// 在后台线程准备整篇笔记的 Reading 数据：解码模型、渲染 Mermaid、排版公式、清洗/解码图片及解析嵌入。
pub fn prepare_reading_note_bg(req: &PrepareReadingRequest) -> Result<PreparedReadingNote, String> {
    let is_mmd = req.note_path.to_ascii_lowercase().ends_with(".mmd");
    let wire = req
        .session
        .reading_model()
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ReadNoteFailed, e))?;
    let model = DecodedReadingModel::decode(&wire)?;
    let snapshot = req
        .session
        .snapshot()
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ReadNoteFailed, e))?;
    let source_text = snapshot.text;
    let headings: Vec<HeadingItem> = req
        .session
        .outline()
        .map(|o| o.headings.into_iter().map(HeadingItem::from).collect())
        .unwrap_or_default();

    let math_blocks = req.session.math_blocks().unwrap_or_default();
    let raw_media_blocks = req
        .session
        .media_blocks(req.content_width.max(160.0), req.font_size)
        .unwrap_or_default();
    let media_blocks = crate::adapt_media_blocks!(&raw_media_blocks);
    let knowledge = req
        .handle
        .as_ref()
        .map(|h| h.knowledge_markup(req.note_path.clone(), source_text.clone()));

    let mut diagrams = Vec::new();
    let mut ordinal = 0u32;
    let diagram_width = diagram_render_width(req.content_width);
    if is_mmd {
        let collapsed = req
            .core
            .collapsed_nodes(req.vault_root.clone(), req.note_path.clone(), 1)
            .unwrap_or_default();
        match render_block_with_engine(
            &req.diagram_engine,
            &source_text,
            true,
            diagram_width,
            &collapsed,
            DiagramIdentity {
                document: req.note_path.clone(),
                block: 1,
                revision: wire.revision,
                dark: req.dark,
                font_generation: req.font_size.to_bits(),
                request_generation: 1,
            },
            None,
        ) {
            Ok(scene) => diagrams.push(RenderedDiagram {
                ordinal: 1,
                source_start: 0,
                source_end: source_text.len() as u64,
                previous: scene.previous.clone(),
                scene: Some(scene),
                failure: None,
                collapsed,
            }),
            Err(err) => diagrams.push(RenderedDiagram {
                ordinal: 1,
                source_start: 0,
                source_end: source_text.len() as u64,
                previous: None,
                scene: None,
                failure: Some(format_diagram_failure(&err)),
                collapsed,
            }),
        }
    } else {
        for (idx, blk) in model.blocks.iter().enumerate() {
            if !blk.diagram {
                continue;
            }
            ordinal += 1;
            let (s_start, s_end) = snapshot
                .blocks
                .get(idx)
                .map(|sb| (sb.start, sb.start + sb.length))
                .unwrap_or((blk.source_start, blk.source_start + blk.source_length));
            let collapsed = req
                .core
                .collapsed_nodes(req.vault_root.clone(), req.note_path.clone(), ordinal)
                .unwrap_or_default();
            match render_block_with_engine(
                &req.diagram_engine,
                &source_text,
                false,
                diagram_width,
                &collapsed,
                DiagramIdentity {
                    document: req.note_path.clone(),
                    block: ordinal,
                    revision: wire.revision,
                    dark: req.dark,
                    font_generation: req.font_size.to_bits(),
                    request_generation: 1,
                },
                None,
            ) {
                Ok(scene) => diagrams.push(RenderedDiagram {
                    ordinal,
                    source_start: s_start,
                    source_end: s_end,
                    previous: scene.previous.clone(),
                    scene: Some(scene),
                    failure: None,
                    collapsed,
                }),
                Err(err) => diagrams.push(RenderedDiagram {
                    ordinal,
                    source_start: s_start,
                    source_end: s_end,
                    previous: None,
                    scene: None,
                    failure: Some(format_diagram_failure(&err)),
                    collapsed,
                }),
            }
        }
    }

    let prepared_math = if !math_blocks.is_empty() {
        req.math_service
            .prepare_blocks(&math_blocks, req.font_size, false)
    } else {
        Vec::new()
    };

    let prepared_media = prepare_media_blocks_bg(
        &media_blocks,
        &req.vault_root,
        req.font_size,
        &req.math_service,
    );

    let mut embeds = HashMap::new();
    if let (Some(handle), Some(know)) = (&req.handle, &knowledge) {
        let embed_links: Vec<&ResolvedLinkInfo> = know
            .links
            .iter()
            .filter(|l| l.embed && !l.attachment && !l.in_diagram)
            .collect();
        for (idx, item) in prepared_media.iter().enumerate() {
            if item.block.image_wiki && item.reason.starts_with("embed.") {
                let src_u16 = item.block.source_start;
                if let Some(link) = embed_links
                    .iter()
                    .find(|l| src_u16 >= l.start_utf16 && src_u16 < l.start_utf16 + l.length_utf16)
                {
                    let embed_req = EmbedRequest {
                        source_path: req.note_path.clone(),
                        target: link.target.clone(),
                        heading: link.heading.clone(),
                        alias: link.alias.clone(),
                        ancestors: vec![req.note_path.clone()],
                        depth: 0,
                        width: req.content_width.max(240.0),
                        font_size: req.font_size,
                        dark: req.dark,
                        vault_root: req.vault_root.clone(),
                        handle: Arc::clone(handle),
                        core: Arc::clone(&req.core),
                        diagram_engine: Arc::clone(&req.diagram_engine),
                        math_service: Arc::clone(&req.math_service),
                    };
                    if let Ok(pre) = prepare_embed_bg(&embed_req) {
                        embeds.insert(idx, pre);
                    }
                }
            }
        }
    }

    Ok(PreparedReadingNote {
        path: req.note_path.clone(),
        is_mmd,
        revision: wire.revision,
        source_text,
        headings,
        model,
        diagrams,
        math: prepared_math,
        media: prepared_media,
        knowledge,
        embeds,
        prepared_off_main_thread: std::thread::current().id() != req.main_thread,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_embeds_and_error_math_after_cjk_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("EmbedTarget.md"), "# Embed\n\nBody\n").unwrap();
        std::fs::write(vault.join("Diag.mmd"), "flowchart LR\nA-->B\n").unwrap();
        let note = "---\ntitle: 中文\n---\n\n# 中文标题\n\n坏行内 $\\badcommand{\\frac{1}$ 与 $\\unknowncmd$ 文本\n\n$$\n\\badcmd{\\frac{1}}\n$$\n\n![[EmbedTarget]]\n\n![[EmbedTarget#Embed]]\n\n![[Diag.mmd]]\n";
        std::fs::write(vault.join("Main.md"), note).unwrap();

        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .unwrap();
        let handle = core
            .open_vault(vault.to_string_lossy().into_owned())
            .unwrap();
        handle.update_search_index(
            vec!["EmbedTarget.md".into(), "Diag.mmd".into(), "Main.md".into()],
            vec![],
        );
        let session = core
            .open_document(vault.to_string_lossy().into_owned(), "Main.md".into())
            .unwrap();
        let diag = DiagramEngine::new().unwrap();
        let math_svc = Arc::new(MathRenderService::new());
        let prep = prepare_reading_note_bg(&PrepareReadingRequest {
            note_path: "Main.md".into(),
            vault_root: vault.to_string_lossy().into_owned(),
            session,
            core,
            handle: Some(handle),
            diagram_engine: diag,
            math_service: math_svc,
            content_width: 680.0,
            font_size: 15.0,
            dark: false,
            main_thread: std::thread::current().id(),
        })
        .unwrap();
        assert_eq!(prep.embeds.len(), 3);
        let kinds: Vec<&str> = prep.embeds.values().map(|e| e.info.kind.as_str()).collect();
        assert!(kinds.contains(&"note"));
        assert!(kinds.contains(&"diagram"));
        assert!(prep.math.iter().any(|m| m.failure.is_some()));
    }
}
