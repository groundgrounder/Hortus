// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 默认只读 Reading View：消费内核 `ReadingModel`、`MathBlock`、`MediaBlock`、Mermaid 图表与嵌套嵌入卡片。

use crate::core::{Utf16Mapper, utf16_slice};
use crate::diagram_render::{
    DiagramCallbacks, DiagramExportKind, DiagramExportMeta, RenderedDiagram, build_diagram_card,
    format_diagram_failure, render_block_with_engine,
};
use crate::embed_render::{
    EmbedCallbacks, EmbedRequest, PreparedEmbed, build_embed_card, prepare_embed_bg,
};
use crate::math_render::{
    MathRenderService, PreparedMathBlock, build_display_math_widget, build_inline_math_widget,
};
use crate::media_render::{
    PreparedMedia, build_image_widget, build_table_widget, prepare_media_blocks_bg,
    summarize_media_stats,
};
use crate::style::{apply_mask_tags, create_tag_table};
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, DiagramIdentity, DocumentSession, EmbedContentInfo, KnowledgeMarkupInfo,
    ReadingModel, ResolvedLinkInfo, VaultHandle, WorkspaceCore,
};
use libadwaita as adw;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
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
        .ok_or_else(|| "ReadingModel 数据截断".to_string())?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u64_le(data: &[u8], off: usize) -> Result<u64, String> {
    let slice = data
        .get(off..off + 8)
        .ok_or_else(|| "ReadingModel 数据截断".to_string())?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

impl DecodedReadingModel {
    pub fn decode(wire: &ReadingModel) -> Result<Self, String> {
        let b_data = &wire.packed_blocks;
        let r_data = &wire.packed_runs;
        if b_data.len() < 8 || r_data.len() < 8 {
            return Err("ReadingModel 头部长度不足".into());
        }
        let b_ver = read_u32_le(b_data, 0)?;
        let b_cnt = read_u32_le(b_data, 4)? as usize;
        let b_rec = match b_ver {
            1 => 64usize,
            2 => 36usize,
            _ => return Err(format!("不支持的 block 版本：{b_ver}")),
        };
        if b_data.len() != 8 + b_cnt * b_rec {
            return Err("ReadingModel packed_blocks 长度不匹配".into());
        }

        let r_ver = read_u32_le(r_data, 0)?;
        let r_cnt = read_u32_le(r_data, 4)? as usize;
        let r_rec = match r_ver {
            1 => 40usize,
            2 => 20usize,
            _ => return Err(format!("不支持的 run 版本：{r_ver}")),
        };
        if r_data.len() != 8 + r_cnt * r_rec {
            return Err("ReadingModel packed_runs 长度不匹配".into());
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
                return Err("ReadingModel run 索引越界".into());
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
    pub source_text: String,
    pub model: DecodedReadingModel,
    pub diagrams: Vec<RenderedDiagram>,
    pub math: Vec<PreparedMathBlock>,
    pub media: Vec<PreparedMedia>,
    pub knowledge: Option<KnowledgeMarkupInfo>,
    pub embeds: HashMap<usize, PreparedEmbed>,
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
        .map_err(|e| format!("读取 ReadingModel 失败：{e}"))?;
    let model = DecodedReadingModel::decode(&wire)?;
    let snapshot = req
        .session
        .snapshot()
        .map_err(|e| format!("读取文档快照失败：{e}"))?;
    let source_text = snapshot.text;

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
    if is_mmd {
        let collapsed = req
            .core
            .collapsed_nodes(req.vault_root.clone(), req.note_path.clone(), 1)
            .unwrap_or_default();
        match render_block_with_engine(
            &req.diagram_engine,
            &source_text,
            true,
            req.content_width.max(240.0),
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
                req.content_width.max(240.0),
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
            .prepare_blocks(&math_blocks, &source_text, req.font_size, false)
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
        source_text,
        model,
        diagrams,
        math: prepared_math,
        media: prepared_media,
        knowledge,
        embeds,
        prepared_off_main_thread: std::thread::current().id() != req.main_thread,
    })
}

#[derive(Clone)]
pub struct ReadingEnv {
    pub note_path: String,
    pub vault_root: String,
    pub core: Arc<WorkspaceCore>,
    pub handle: Arc<VaultHandle>,
    pub diagram_engine: Arc<DiagramEngine>,
    pub math_service: Arc<MathRenderService>,
    pub font_size: f64,
    pub content_width: f64,
    pub ancestors: Vec<String>,
    pub depth: u32,
    pub parent_window: Option<gtk4::Window>,
    pub callbacks: EmbedCallbacks,
    pub on_open_source: Rc<dyn Fn(u64, u64)>,
    /// 阅读视图内点击 `#标签` 时切到标签面板并筛选。
    pub on_open_tag: Rc<dyn Fn(String)>,
}

#[derive(Clone)]
enum AnchorKind {
    Widget(gtk4::Widget),
    InlineWidget {
        widget: gtk4::Widget,
        rise_pango: i32,
    },
}

#[derive(Clone)]
enum LinkAction {
    Wiki {
        target: String,
        heading: Option<String>,
        resolved_path: Option<String>,
    },
    ExternalUrl(String),
    Tag(String),
}

#[derive(Clone)]
struct LinkSpan {
    char_start: i32,
    char_end: i32,
    action: LinkAction,
}

#[derive(Clone)]
pub struct ReadingSurfaceHandle {
    pub root: gtk4::Widget,
    pub scrolled: gtk4::ScrolledWindow,
    pub text_view: Option<gtk4::TextView>,
    source_char_anchors: Vec<(u64, i32)>,
    link_spans: Rc<Vec<LinkSpan>>,
    click_gesture: Option<gtk4::GestureClick>,
    /// 内联块控件引用：宽度适配自动化读取实际分配宽。
    pub diagram_widgets: Vec<gtk4::Widget>,
    pub table_widgets: Vec<gtk4::Widget>,
    pub image_widgets: Vec<gtk4::Widget>,
    pub embed_widgets: Vec<gtk4::Widget>,
    pub stats: NoteReadingStats,
}

impl ReadingSurfaceHandle {
    /// 自动化回归：按可见文字命中阅读视图双链，并触发真实的 `GestureClick::released`。
    /// 返回被点击链接的 `(target, heading)`；文字不在双链范围内时返回 `None`。
    pub fn click_wikilink(&self, label: &str) -> Option<(String, Option<String>)> {
        let text_view = self.text_view.as_ref()?;
        let gesture = self.click_gesture.as_ref()?;
        let buffer = text_view.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let byte = text.find(label)?;
        let char_off = text[..byte].chars().count() as i32;
        let action = self
            .link_spans
            .iter()
            .find(|span| char_off >= span.char_start && char_off < span.char_end)?
            .action
            .clone();
        let LinkAction::Wiki {
            target, heading, ..
        } = action
        else {
            return None;
        };
        let mut iter = buffer.iter_at_offset(char_off);
        text_view.scroll_to_iter(&mut iter, 0.0, false, 0.0, 0.0);
        let rect = text_view.iter_location(&iter);
        let (wx, wy) = text_view.buffer_to_window_coords(
            gtk4::TextWindowType::Widget,
            rect.x() + 2,
            rect.y() + rect.height().max(1) / 2,
        );
        let (x, y) = (f64::from(wx), f64::from(wy));
        gesture.emit_by_name::<()>("released", &[&1i32, &x, &y]);
        Some((target, heading))
    }

    pub fn scroll_to_source_utf16(&self, target_utf16: u64) {
        let Some(ref tv) = self.text_view else { return };
        let char_off = map_source_utf16_to_reading_char(&self.source_char_anchors, target_utf16);
        let buf = tv.buffer();
        let iter = buf.iter_at_offset(char_off.clamp(0, buf.char_count()));
        let rect = tv.iter_location(&iter);
        let adj = self.scrolled.vadjustment();
        let target_y = (f64::from(rect.y()) - 12.0).max(0.0);
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value(target_y.min(max_y));
    }

    pub fn current_source_utf16(&self) -> u64 {
        let Some(ref tv) = self.text_view else {
            return 0;
        };
        let top_y = self.scrolled.vadjustment().value().max(0.0) as i32;
        let (iter, _) = tv.line_at_y(top_y + 12);
        let off = iter.offset();
        map_reading_char_to_source_utf16(&self.source_char_anchors, off)
    }

    pub fn scroll_fraction(&self, fraction: f64) {
        let adj = self.scrolled.vadjustment();
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value((max_y * fraction.clamp(0.0, 1.0)).max(0.0));
    }

    /// 当前相对滚动位置；动态重排前读取，重建阅读表面后恢复。
    pub fn scroll_fraction_value(&self) -> f64 {
        let adj = self.scrolled.vadjustment();
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        if max_y <= 0.0 {
            0.0
        } else {
            (adj.value() / max_y).clamp(0.0, 1.0)
        }
    }
}

fn map_source_utf16_to_reading_char(anchors: &[(u64, i32)], target: u64) -> i32 {
    if anchors.is_empty() {
        return 0;
    }
    let idx = anchors.partition_point(|(src, _)| *src < target);
    if idx < anchors.len() {
        anchors[idx].1
    } else {
        anchors.last().map(|(_, c)| *c).unwrap_or(0)
    }
}

fn map_reading_char_to_source_utf16(anchors: &[(u64, i32)], char_off: i32) -> u64 {
    if anchors.is_empty() {
        return 0;
    }
    let idx = anchors.partition_point(|(_, c)| *c < char_off);
    if idx < anchors.len() {
        anchors[idx].0
    } else {
        anchors.last().map(|(s, _)| *s).unwrap_or(0)
    }
}

/// 在 GTK 主线程组装整篇阅读表面（纯消费后台准备好的数据，零阻塞计算）。
pub fn build_reading_surface(
    prepared: &PreparedReadingNote,
    env: &ReadingEnv,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
) -> ReadingSurfaceHandle {
    let mut stats = compute_stats(prepared);

    if prepared.is_mmd {
        let diag = prepared
            .diagrams
            .first()
            .cloned()
            .unwrap_or(RenderedDiagram {
                ordinal: 1,
                source_start: 0,
                source_end: prepared.source_text.len() as u64,
                scene: None,
                failure: Some("未生成 Mermaid 图表场景".into()),
                collapsed: Vec::new(),
                previous: None,
            });
        let diag_card = build_diagram_card(
            prepared.path.clone(),
            prepared.source_text.clone(),
            true,
            diag,
            Arc::clone(&env.diagram_engine),
            make_diagram_callbacks(env, prepared.path.clone(), on_diagram_collapse),
            env.content_width,
        );
        let box_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        box_wrap.set_margin_top(24);
        box_wrap.set_margin_bottom(36);
        box_wrap.set_margin_start(32);
        box_wrap.set_margin_end(32);
        box_wrap.append(&diag_card);

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&box_wrap)
            .build();
        return ReadingSurfaceHandle {
            root: scrolled.clone().upcast(),
            scrolled,
            text_view: None,
            source_char_anchors: Vec::new(),
            link_spans: Rc::new(Vec::new()),
            click_gesture: None,
            diagram_widgets: Vec::new(),
            table_widgets: Vec::new(),
            image_widgets: Vec::new(),
            embed_widgets: Vec::new(),
            stats,
        };
    }

    let assembled = assemble_reading_buffer_data(
        &prepared.model,
        &prepared.source_text,
        prepared.knowledge.as_ref(),
        &prepared.diagrams,
        &prepared.math,
        &prepared.media,
        &prepared.embeds,
        env,
        Some(&mut stats),
        on_diagram_collapse,
    );

    let (text_view, scrolled, click_gesture, link_spans) = install_text_view_with_anchors(
        &assembled.text,
        &assembled.runs,
        &assembled.anchors,
        &assembled.link_spans,
        env,
        false,
    );

    ReadingSurfaceHandle {
        root: scrolled.clone().upcast(),
        scrolled,
        text_view: Some(text_view),
        source_char_anchors: assembled.source_char_anchors,
        link_spans,
        click_gesture,
        diagram_widgets: assembled.diagram_widgets,
        table_widgets: assembled.table_widgets,
        image_widgets: assembled.image_widgets,
        embed_widgets: assembled.embed_widgets,
        stats,
    }
}

/// 为嵌入卡片（`"note"` 类型）构建内联只读正文部件。
pub fn build_embedded_note_body(
    env: &ReadingEnv,
    info: &EmbedContentInfo,
    media: &[PreparedMedia],
    math: &[PreparedMathBlock],
    diagrams: &[RenderedDiagram],
) -> gtk4::Widget {
    let converted = convert_embed_to_model(info);
    let empty_embeds: HashMap<usize, PreparedEmbed> = HashMap::new();
    let know = KnowledgeMarkupInfo {
        links: info.links.clone(),
        tags: info.tags.clone(),
    };
    let on_collapse = Rc::new({
        let core = Arc::clone(&env.core);
        let vault = env.vault_root.clone();
        let note = env.note_path.clone();
        move |ordinal: u32, collapsed: Vec<String>, _| {
            let _ = core.remember_diagram(vault.clone(), note.clone(), ordinal, collapsed);
        }
    });
    let assembled = assemble_reading_buffer_data(
        &converted,
        &info.content,
        Some(&know),
        diagrams,
        math,
        media,
        &empty_embeds,
        env,
        None,
        on_collapse,
    );

    let (text_view, _, _, _) = install_text_view_with_anchors(
        &assembled.text,
        &assembled.runs,
        &assembled.anchors,
        &assembled.link_spans,
        env,
        true,
    );
    text_view.upcast()
}

fn convert_embed_to_model(info: &EmbedContentInfo) -> DecodedReadingModel {
    let mut blocks = Vec::with_capacity(info.blocks.len());
    let mut runs = Vec::new();
    for blk in &info.blocks {
        let start = runs.len();
        for run in &blk.runs {
            runs.push(DecodedRun {
                start: run.start,
                length: run.length,
                source_start: run.source_start_utf16,
                source_length: run.source_length_utf16,
                mask: run.mask,
            });
        }
        blocks.push(DecodedBlock {
            id: blk.id,
            source_start: blk.source_start_utf16,
            source_length: blk.source_length_utf16,
            display_start: blk.display_start,
            display_length: blk.display_length,
            run_start: start,
            run_len: blk.runs.len(),
            diagram: blk.diagram,
        });
    }
    DecodedReadingModel {
        text: info.text.clone(),
        blocks,
        runs,
    }
}

fn compute_stats(prepared: &PreparedReadingNote) -> NoteReadingStats {
    let mut s = NoteReadingStats {
        prepared_off_main_thread: prepared.prepared_off_main_thread,
        diagram_count: prepared.diagrams.len(),
        math_feedback_rounds: prepared.math_feedback_rounds_placeholder(),
        ..Default::default()
    };
    let mut seen_kinds = HashSet::new();
    for d in &prepared.diagrams {
        if let Some(ref sc) = d.scene {
            s.diagram_rendered += 1;
            s.diagram_collapsed_count += d.collapsed.len();
            if let Some(ref gm) = sc.graph_metrics
                && !gm.stability_reason.is_empty()
                && seen_kinds.insert(gm.stability_reason.clone())
            {
                s.diagram_kinds.push(gm.stability_reason.clone());
            }
        } else {
            s.diagram_fallback += 1;
        }
    }
    s.math_total = prepared.math.len();
    for m in &prepared.math {
        if m.math.is_some() {
            s.math_rendered += 1;
        } else {
            s.math_fallback_error += 1;
        }
    }
    for m in &prepared.media {
        for cell in &m.cell_math {
            for cm in cell {
                s.math_total += 1;
                if cm.math.is_some() {
                    s.math_rendered += 1;
                } else {
                    s.math_fallback_error += 1;
                }
            }
        }
    }
    s.math_cjk_fallback_codepoints = prepared.diagram_math_codepoints().unwrap_or_default();

    let (tables, local_img, remote_img, ext_refs, scripts) = summarize_media_stats(&prepared.media);
    s.table_count = tables;
    s.local_image_count = local_img;
    s.remote_image_count = remote_img;
    s.sanitized_svg_external_refs = ext_refs;
    s.sanitized_svg_scripts = scripts;

    if let Some(ref k) = prepared.knowledge {
        s.wikilink_count = k.links.iter().filter(|l| !l.embed).count();
    }
    let mut ek = HashSet::new();
    for pre in prepared.embeds.values() {
        s.embed_count += 1;
        if ek.insert(pre.info.kind.clone()) {
            s.embed_kinds.push(pre.info.kind.clone());
        }
    }
    s
}

impl PreparedReadingNote {
    fn diagram_math_codepoints(&self) -> Option<Vec<u32>> {
        None
    }
    fn math_feedback_rounds_placeholder(&self) -> usize {
        self.math
            .iter()
            .filter_map(|m| m.math.as_ref())
            .map(|m| m.missing_metrics_feedbacks)
            .sum()
    }
}

#[derive(Clone)]
struct AdjustedRun {
    disp_start_u16: usize,
    disp_len_u16: usize,
    mask: u32,
}

struct AssembledReadingBuffer {
    text: String,
    runs: Vec<AdjustedRun>,
    anchors: Vec<(usize, AnchorKind)>,
    source_char_anchors: Vec<(u64, i32)>,
    link_spans: Vec<LinkSpan>,
    /// 内联块控件引用：宽度适配自动化直接读取其实际分配宽。
    diagram_widgets: Vec<gtk4::Widget>,
    table_widgets: Vec<gtk4::Widget>,
    image_widgets: Vec<gtk4::Widget>,
    embed_widgets: Vec<gtk4::Widget>,
}

#[allow(clippy::too_many_arguments)]
fn assemble_reading_buffer_data(
    model: &DecodedReadingModel,
    source_text: &str,
    knowledge: Option<&KnowledgeMarkupInfo>,
    diagrams: &[RenderedDiagram],
    math: &[PreparedMathBlock],
    media: &[PreparedMedia],
    precomputed_embeds: &HashMap<usize, PreparedEmbed>,
    env: &ReadingEnv,
    stats: Option<&mut NoteReadingStats>,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
) -> AssembledReadingBuffer {
    let mut text = model.text.clone();
    let has_props = text.starts_with('\u{e000}');
    let prop_label = "属性";
    let prop_delta_u16: isize = if has_props {
        text.remove(0);
        text.insert_str(0, prop_label);
        prop_label.encode_utf16().count() as isize - 1
    } else {
        0
    };

    let disp_mapper = Utf16Mapper::new(&text);

    // 建立索引：block.id -> block_idx，以及 knowledge 未解析/已解析双链清单
    let mut block_index_by_id = HashMap::new();
    let mut block_disp_starts_u16 = Vec::with_capacity(model.blocks.len());
    for (idx, blk) in model.blocks.iter().enumerate() {
        block_index_by_id.insert(blk.id, idx);
        let shifted = if idx == 0 {
            blk.display_start as usize
        } else {
            (blk.display_start as isize + prop_delta_u16).max(0) as usize
        };
        block_disp_starts_u16.push(shifted);
    }

    let wiki_links: Vec<&ResolvedLinkInfo> = knowledge
        .map(|k| {
            k.links
                .iter()
                .filter(|l| !l.embed && !l.in_diagram)
                .collect()
        })
        .unwrap_or_default();
    let embed_links: Vec<&ResolvedLinkInfo> = knowledge
        .map(|k| {
            k.links
                .iter()
                .filter(|l| l.embed && !l.attachment && !l.in_diagram)
                .collect()
        })
        .unwrap_or_default();

    let mut adjusted_runs = Vec::new();
    let mut anchors: Vec<(usize, AnchorKind)> = Vec::new();
    let mut link_spans: Vec<LinkSpan> = Vec::new();
    let mut source_char_anchors: Vec<(u64, i32)> = Vec::new();
    let mut seen_hr_chars = HashSet::new();
    let mut diagram_widgets: Vec<gtk4::Widget> = Vec::new();
    let mut table_widgets: Vec<gtk4::Widget> = Vec::new();
    let mut image_widgets: Vec<gtk4::Widget> = Vec::new();
    let mut embed_widgets: Vec<gtk4::Widget> = Vec::new();

    for (idx, blk) in model.blocks.iter().enumerate() {
        let blk_disp_u16 = block_disp_starts_u16[idx];
        let blk_src_u16 = blk.source_start;
        let blk_char = disp_mapper.utf16_to_char(blk_disp_u16) as i32;
        source_char_anchors.push((blk_src_u16, blk_char));

        let slice = &model.runs[blk.run_start..blk.run_start + blk.run_len];
        for run in slice {
            let mut r_start = blk_disp_u16 + run.start as usize;
            let mut r_len = run.length as usize;
            if has_props && idx == 0 {
                if run.start == 0 && run.mask == 16384 {
                    r_len = (r_len as isize + prop_delta_u16).max(0) as usize;
                } else if run.start >= 1 {
                    r_start = (r_start as isize + prop_delta_u16).max(0) as usize;
                }
            }
            let r_char_start = disp_mapper.utf16_to_char(r_start) as i32;
            let r_char_end = disp_mapper.utf16_to_char(r_start + r_len) as i32;

            let mut final_mask = run.mask;
            let abs_src_u16 = blk_src_u16 + run.source_start;

            // 分隔线 `\u{200b}` -> 原生水平 Separator 挂件
            if final_mask & 32768 != 0
                && let Some(zw_byte) = find_char_near_utf16(&text, r_start, r_len, '\u{200b}')
            {
                let zw_char = text[..zw_byte].chars().count();
                if seen_hr_chars.insert(zw_char) {
                    let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
                    sep.add_css_class("hortus-reading-hr");
                    sep.set_hexpand(true);
                    anchors.push((zw_char, AnchorKind::Widget(sep.upcast())));
                }
            }

            // WikiLink 点击路由与失效标记
            if final_mask & (1 << 18) != 0 {
                if let Some(link) = wiki_links.iter().find(|l| {
                    abs_src_u16 >= l.start_utf16 && abs_src_u16 < l.start_utf16 + l.length_utf16
                }) {
                    if link.resolved_path.is_none() {
                        final_mask |= 1 << 19;
                    }
                    if r_char_end > r_char_start {
                        link_spans.push(LinkSpan {
                            char_start: r_char_start,
                            char_end: r_char_end,
                            action: LinkAction::Wiki {
                                target: link.target.clone(),
                                heading: link.heading.clone(),
                                resolved_path: link.resolved_path.clone(),
                            },
                        });
                    }
                }
            } else if final_mask & 128 != 0 {
                // 标准 Markdown 外部链接 `[title](url)`
                if let Some(url) =
                    extract_markdown_url_around_utf16(source_text, abs_src_u16, run.source_length)
                    && r_char_end > r_char_start
                {
                    link_spans.push(LinkSpan {
                        char_start: r_char_start,
                        char_end: r_char_end,
                        action: LinkAction::ExternalUrl(url),
                    });
                }
            }

            // 行内 `#标签`：点击切换左侧标签面板并筛选该标签。
            if final_mask & (512 | 8192) != 0 && r_char_end > r_char_start {
                let tag_text: String = text
                    .chars()
                    .skip(r_char_start as usize)
                    .take((r_char_end - r_char_start) as usize)
                    .collect();
                let tag = tag_text.trim_start_matches('#').trim().to_string();
                if !tag.is_empty() {
                    link_spans.push(LinkSpan {
                        char_start: r_char_start,
                        char_end: r_char_end,
                        action: LinkAction::Tag(tag),
                    });
                }
            }

            adjusted_runs.push(AdjustedRun {
                disp_start_u16: r_start,
                disp_len_u16: r_len,
                mask: final_mask,
            });
        }
    }

    // 安装 Mermaid 图块 `\u{fffc}` 占位锚点
    let mut diag_ord = 0u32;
    for (idx, blk) in model.blocks.iter().enumerate() {
        if !blk.diagram {
            continue;
        }
        diag_ord += 1;
        let blk_disp_u16 = block_disp_starts_u16[idx];
        let blk_len_u16 = blk.display_length as usize;
        if let Some(byte_pos) = find_char_near_utf16(&text, blk_disp_u16, blk_len_u16, '\u{fffc}') {
            let char_pos = text[..byte_pos].chars().count();
            let diag = diagrams
                .iter()
                .find(|d| d.ordinal == diag_ord)
                .cloned()
                .unwrap_or(RenderedDiagram {
                    ordinal: diag_ord,
                    source_start: 0,
                    source_end: 0,
                    scene: None,
                    failure: Some("图表尚未完成排版".into()),
                    collapsed: Vec::new(),
                    previous: None,
                });
            let w = build_diagram_card(
                env.note_path.clone(),
                source_text.to_string(),
                false,
                diag,
                Arc::clone(&env.diagram_engine),
                make_diagram_callbacks(env, env.note_path.clone(), on_diagram_collapse.clone()),
                env.content_width,
            );
            diagram_widgets.push(w.clone());
            anchors.push((char_pos, AnchorKind::Widget(w)));
        }
    }

    // 安装数学公式 `\u{fffc}` 占位锚点
    for m in math {
        let Some(&blk_idx) = block_index_by_id.get(&m.block.block_id) else {
            continue;
        };
        let local_u16 = m
            .block
            .display_start
            .saturating_sub(model.blocks[blk_idx].display_start) as usize;
        let disp_u16 = block_disp_starts_u16[blk_idx] + local_u16;
        if let Some(byte_pos) = find_char_near_utf16(
            &text,
            disp_u16,
            m.block.display_length.max(1) as usize,
            '\u{fffc}',
        ) {
            let char_pos = text[..byte_pos].chars().count();
            if m.block.block {
                let widget = build_display_math_widget(m, env.content_width);
                anchors.push((char_pos, AnchorKind::Widget(widget)));
            } else {
                let widget = build_inline_math_widget(m);
                let rise_pango = m
                    .math
                    .as_ref()
                    .map(|pm| {
                        let px = pm.descent_px;
                        -((px * 0.75 * f64::from(pango::SCALE)).round() as i32)
                    })
                    .unwrap_or(0);
                anchors.push((char_pos, AnchorKind::InlineWidget { widget, rise_pango }));
            }
        }
    }

    // 安装媒体 / 表格 / 嵌入卡片 `\u{fffc}` 占位锚点
    for (media_idx, item) in media.iter().enumerate() {
        let Some(&blk_idx) = block_index_by_id.get(&item.block.block_id) else {
            continue;
        };
        let local_u16 = item
            .block
            .display_start
            .saturating_sub(model.blocks[blk_idx].display_start) as usize;
        let disp_u16 = block_disp_starts_u16[blk_idx] + local_u16;
        let Some(byte_pos) = find_char_near_utf16(
            &text,
            disp_u16,
            item.block.display_length.max(1) as usize,
            '\u{fffc}',
        ) else {
            continue;
        };
        let char_pos = text[..byte_pos].chars().count();

        if item.block.image_wiki && item.reason.starts_with("embed.") {
            let src_u16 = item.block.source_start;
            if let Some(link) = embed_links
                .iter()
                .find(|l| src_u16 >= l.start_utf16 && src_u16 < l.start_utf16 + l.length_utf16)
            {
                let embed_req = EmbedRequest {
                    source_path: env.note_path.clone(),
                    target: link.target.clone(),
                    heading: link.heading.clone(),
                    alias: link.alias.clone(),
                    ancestors: env.ancestors.clone(),
                    depth: env.depth,
                    width: env.content_width,
                    font_size: env.font_size,
                    dark: adw::StyleManager::default().is_dark(),
                    vault_root: env.vault_root.clone(),
                    handle: Arc::clone(&env.handle),
                    core: Arc::clone(&env.core),
                    diagram_engine: Arc::clone(&env.diagram_engine),
                    math_service: Arc::clone(&env.math_service),
                };
                let pre = precomputed_embeds.get(&media_idx).map(|p| PreparedEmbed {
                    info: p.info.clone(),
                    media: p.media.clone(),
                    math: p.math.clone(),
                    diagrams: p.diagrams.clone(),
                });
                let card = build_embed_card(embed_req, pre, env.callbacks.clone());
                embed_widgets.push(card.clone());
                anchors.push((char_pos, AnchorKind::Widget(card)));
                continue;
            }
        }

        if !item.block.columns.is_empty() {
            let tbl = build_table_widget(
                item,
                env.content_width,
                Some(env.callbacks.on_open_note.clone()),
                Some(env.callbacks.on_open_external.clone()),
            );
            table_widgets.push(tbl.clone());
            anchors.push((char_pos, AnchorKind::Widget(tbl)));
        } else {
            let img = build_image_widget(item, env.content_width, env.parent_window.as_ref());
            image_widgets.push(img.clone());
            anchors.push((char_pos, AnchorKind::Widget(img)));
        }
    }

    if let Some(_s) = stats {
        // 已由 compute_stats 填充主字段
    }

    anchors.sort_by_key(|a| a.0);
    anchors.dedup_by(|a, b| a.0 == b.0);
    source_char_anchors.sort_by_key(|a| a.0);

    AssembledReadingBuffer {
        text,
        runs: adjusted_runs,
        anchors,
        source_char_anchors,
        link_spans,
        diagram_widgets,
        table_widgets,
        image_widgets,
        embed_widgets,
    }
}

fn install_text_view_with_anchors(
    display_text: &str,
    runs: &[AdjustedRun],
    anchors: &[(usize, AnchorKind)],
    link_spans: &[LinkSpan],
    env: &ReadingEnv,
    embedded: bool,
) -> (
    gtk4::TextView,
    gtk4::ScrolledWindow,
    Option<gtk4::GestureClick>,
    Rc<Vec<LinkSpan>>,
) {
    let tag_table = create_tag_table();
    let buffer = gtk4::TextBuffer::new(Some(&tag_table));
    buffer.set_text(display_text);
    let spans = Rc::new(link_spans.to_vec());
    let mut click_gesture: Option<gtk4::GestureClick> = None;

    let mapper = Utf16Mapper::new(display_text);
    for run in runs {
        if run.disp_len_u16 == 0 || run.mask == 0 {
            continue;
        }
        let cs = mapper.utf16_to_char(run.disp_start_u16) as i32;
        let ce = mapper.utf16_to_char(run.disp_start_u16 + run.disp_len_u16) as i32;
        if cs < ce {
            apply_mask_tags(
                &buffer,
                &buffer.iter_at_offset(cs),
                &buffer.iter_at_offset(ce),
                run.mask,
            );
        }
    }

    let text_view = gtk4::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .left_margin(if embedded { 8 } else { 44 })
        .right_margin(if embedded { 8 } else { 44 })
        .top_margin(if embedded { 6 } else { 28 })
        .bottom_margin(if embedded { 8 } else { 48 })
        .pixels_above_lines(2)
        .pixels_below_lines(4)
        .vexpand(!embedded)
        .hexpand(true)
        .build();
    text_view.add_css_class(if embedded {
        "hortus-embedded-reading-view"
    } else {
        "hortus-reading-view"
    });

    // 逆序替换 1 个字符占位符 (`\u{fffc}` / `\u{200b}`) 为 ChildAnchor：总字符数保持不变，不破坏已有标签与链接范围
    for &(char_off, ref kind) in anchors.iter().rev() {
        let off = char_off as i32;
        if off < 0 || off >= buffer.char_count() {
            continue;
        }
        let mut s_it = buffer.iter_at_offset(off);
        let mut e_it = buffer.iter_at_offset(off + 1);
        buffer.delete(&mut s_it, &mut e_it);
        let anchor = buffer.create_child_anchor(&mut s_it);
        match kind {
            AnchorKind::Widget(w) => {
                text_view.add_child_at_anchor(w, &anchor);
            }
            AnchorKind::InlineWidget { widget, rise_pango } => {
                text_view.add_child_at_anchor(widget, &anchor);
                if *rise_pango != 0 {
                    let tag = gtk4::TextTag::builder().rise(*rise_pango).build();
                    tag_table.add(&tag);
                    let s_anchor = buffer.iter_at_child_anchor(&anchor);
                    let mut e_anchor = s_anchor;
                    e_anchor.forward_char();
                    buffer.apply_tag(&tag, &s_anchor, &e_anchor);
                }
            }
        }
    }

    // 绑定正文双链与外部 URL 点击与手型光标
    if !link_spans.is_empty() {
        let motion = gtk4::EventControllerMotion::new();
        {
            let tv_m = text_view.clone();
            let spans_m = spans.clone();
            motion.connect_motion(move |_, x, y| {
                let (bx, by) =
                    tv_m.window_to_buffer_coords(gtk4::TextWindowType::Widget, x as i32, y as i32);
                let over_link = tv_m.iter_at_location(bx, by).is_some_and(|it| {
                    let off = it.offset();
                    spans_m
                        .iter()
                        .any(|s| off >= s.char_start && off < s.char_end)
                });
                tv_m.set_cursor_from_name(if over_link { Some("pointer") } else { None });
            });
        }
        text_view.add_controller(motion);

        let click = gtk4::GestureClick::new();
        click.set_button(1);
        {
            let tv_c = text_view.clone();
            let spans_c = spans.clone();
            let cbs = env.callbacks.clone();
            let handle = Arc::clone(&env.handle);
            let src_note = env.note_path.clone();
            let open_tag = env.on_open_tag.clone();
            click.connect_released(move |_, _, x, y| {
                let (bx, by) =
                    tv_c.window_to_buffer_coords(gtk4::TextWindowType::Widget, x as i32, y as i32);
                let Some(it) = tv_c.iter_at_location(bx, by) else {
                    return;
                };
                let off = it.offset();
                let Some(span) = spans_c
                    .iter()
                    .find(|s| off >= s.char_start && off < s.char_end)
                else {
                    return;
                };
                match &span.action {
                    LinkAction::Wiki {
                        target,
                        heading,
                        resolved_path,
                    } => {
                        if let Some(p) = resolved_path {
                            (cbs.on_open_note)(p.clone(), heading.clone());
                        } else {
                            let res = handle.resolve_link(src_note.clone(), target.clone());
                            if let Some(p) = res.resolved_path {
                                (cbs.on_open_note)(p, heading.clone());
                            } else {
                                (cbs.on_open_note)(target.clone(), heading.clone());
                            }
                        }
                    }
                    LinkAction::ExternalUrl(url) => {
                        (cbs.on_open_external)(url.clone());
                    }
                    LinkAction::Tag(tag) => {
                        (open_tag)(tag.clone());
                    }
                }
            });
        }
        text_view.add_controller(click.clone());
        click_gesture = Some(click);
    }

    // 水平策略 Automatic 让外层滚动容器不把正文/锚点的最小宽向上传播到窗口：
    // 窗口可缩到窄平铺宽度而不触发 AdwToastOverlay 最小宽警告；正文仍由 TextView
    // 按视口宽 WordChar 换行，仅在内联块瞬时超出视口时短暂使用横向滚动。
    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(!embedded)
        .hexpand(true)
        .child(&text_view)
        .build();

    (text_view, scrolled, click_gesture, spans)
}

fn make_diagram_callbacks(
    env: &ReadingEnv,
    note_path: String,
    on_collapse_changed: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
) -> DiagramCallbacks {
    let handle = Arc::clone(&env.handle);
    let open_note = env.callbacks.on_open_note.clone();
    let export_cb = env.callbacks.on_export_diagram.clone();
    let focus_cb = env.callbacks.on_open_focus.clone();
    let open_src = env.on_open_source.clone();
    let src_note = note_path.clone();
    let note_focus = note_path;

    DiagramCallbacks {
        on_collapse_changed,
        on_open_link: Rc::new(move |target, heading| {
            let res = handle.resolve_link(src_note.clone(), target);
            if let Some(resolved) = res.resolved_path {
                (open_note)(resolved, heading);
            }
        }),
        on_open_source: open_src,
        on_focus: Rc::new(move |ordinal| (focus_cb)(note_focus.clone(), ordinal)),
        on_export: Rc::new(move |kind: DiagramExportKind, meta: DiagramExportMeta| {
            (export_cb)(kind, meta);
        }),
    }
}

fn find_char_near_utf16(
    text: &str,
    disp_start_u16: usize,
    disp_len_u16: usize,
    needle: char,
) -> Option<usize> {
    let mapper = Utf16Mapper::new(text);
    let b_start = mapper.utf16_to_byte(disp_start_u16);
    let b_end = mapper.utf16_to_byte(disp_start_u16 + disp_len_u16.max(1));
    text.get(b_start..b_end.min(text.len()))?
        .find(needle)
        .map(|rel| b_start + rel)
}

fn extract_markdown_url_around_utf16(source: &str, start_u16: u64, len_u16: u64) -> Option<String> {
    let mapper = Utf16Mapper::new(source);
    let b_start = mapper.utf16_to_byte(start_u16 as usize);
    let b_end = mapper.utf16_to_byte((start_u16 + len_u16) as usize);
    let tail = source.get(b_end.min(source.len())..)?;
    if let Some(rest) = tail.strip_prefix("](")
        && let Some(close) = rest.find(')')
    {
        let candidate = rest[..close].split_whitespace().next().unwrap_or("");
        if candidate.starts_with("http://") || candidate.starts_with("https://") {
            return Some(candidate.to_string());
        }
    }
    let slice = utf16_slice(source, start_u16 as usize, len_u16 as usize)?;
    if slice.starts_with("http://") || slice.starts_with("https://") {
        return Some(slice);
    }
    let _ = b_start;
    None
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
