// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 阅读分段模型组装：合并连续文本段与行内公式，遇到块级非文本元素切分原生子控件。

use crate::core::Utf16Mapper;
#[cfg(feature = "automation")]
use crate::reading::prepare::NoteReadingStats;
#[cfg(feature = "automation")]
use crate::reading::prepare::PreparedReadingNote;
use crate::reading::prepare::{DecodedBlock, DecodedReadingModel, DecodedRun};
use crate::reading::surface::ReadingEnv;
use crate::render::diagram::{DiagramExportKind, DiagramExportMeta, RenderedDiagram};
use crate::render::diagram_card::{DiagramCallbacks, DiagramCard, build_diagram_card};
use crate::render::embed::{
    EmbedBodyBuilder, EmbedCard, EmbedRequest, PreparedEmbed, build_embed_card,
};
use crate::render::math::PreparedMathBlock;
use crate::render::math_widgets::{
    DisplayMathBlock, build_display_math_widget, build_inline_math_widget,
};
#[cfg(feature = "automation")]
use crate::render::media::summarize_media_stats;
use crate::render::media::{
    ImageBlock, PreparedMedia, TableBlock, build_image_widget, build_table_widget,
};
use gtk4::pango;
use hortus_ffi::{EmbedContentInfo, KnowledgeMarkupInfo, ResolvedLinkInfo};
use libadwaita as adw;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub enum LinkAction {
    Wiki {
        target: String,
        heading: Option<String>,
        resolved_path: Option<String>,
    },
    ExternalUrl(String),
    Tag(String),
}

#[derive(Clone)]
pub struct LinkSpan {
    pub char_start: i32,
    pub char_end: i32,
    pub action: LinkAction,
}

#[derive(Clone)]
pub struct InlineAnchor {
    pub local_char_pos: usize,
    pub widget: gtk4::Widget,
    pub rise_pango: i32,
}

pub struct TextSegmentData {
    pub text: String,
    pub runs: Vec<(i32, i32, u32)>,
    pub inline_anchors: Vec<InlineAnchor>,
    pub link_spans: Vec<LinkSpan>,
    pub source_anchors: Vec<(u64, i32)>,
    pub source_start: u64,
    pub source_end: u64,
}

pub enum AssembledSegment {
    Text(TextSegmentData),
    Diagram {
        card: DiagramCard,
        source_start: u64,
        source_end: u64,
    },
    Table {
        table: TableBlock,
        source_start: u64,
        source_end: u64,
    },
    Image {
        image: ImageBlock,
        source_start: u64,
        source_end: u64,
    },
    DisplayMath {
        math: DisplayMathBlock,
        source_start: u64,
        source_end: u64,
    },
    Embed {
        embed: EmbedCard,
        source_start: u64,
        source_end: u64,
    },
    Hr {
        source_start: u64,
        source_end: u64,
    },
}

enum BlockBreakKind {
    Diagram(DiagramCard),
    Table(TableBlock),
    Image(ImageBlock),
    DisplayMath(DisplayMathBlock),
    Embed(EmbedCard),
    Hr,
}

struct BlockBreak {
    char_pos: usize,
    source_start: u64,
    source_end: u64,
    kind: BlockBreakKind,
}

#[derive(Clone)]
struct CharRun {
    char_start: usize,
    char_end: usize,
    mask: u32,
}

#[allow(clippy::too_many_arguments)]
pub fn assemble_reading_segments(
    model: &DecodedReadingModel,
    source_text: &str,
    knowledge: Option<&KnowledgeMarkupInfo>,
    diagrams: &[RenderedDiagram],
    math: &[PreparedMathBlock],
    media: &[PreparedMedia],
    precomputed_embeds: &HashMap<usize, PreparedEmbed>,
    env: &ReadingEnv,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
    on_diagram_scene_updated: Rc<dyn Fn(u32, hortus_ffi::SceneData)>,
    embed_body_builder: &EmbedBodyBuilder,
) -> Vec<AssembledSegment> {
    let mut text = model.text.clone();
    let has_props = text.starts_with('\u{e000}');
    let prop_label = crate::i18n::tr(crate::i18n::Key::ReadingPropertiesLabel);
    let prop_delta_u16: isize = if has_props {
        text.remove(0);
        text.insert_str(0, prop_label);
        prop_label.encode_utf16().count() as isize - 1
    } else {
        0
    };

    let disp_mapper = Utf16Mapper::new(&text);
    let src_mapper = Utf16Mapper::new(source_text);

    let mut block_index_by_id = HashMap::with_capacity(model.blocks.len());
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

    let mut char_runs: Vec<CharRun> = Vec::with_capacity(model.runs.len());
    let mut block_breaks: Vec<BlockBreak> = Vec::new();
    let mut inline_anchors: Vec<(usize, gtk4::Widget, i32)> = Vec::new();
    let mut link_spans: Vec<LinkSpan> = Vec::new();
    let mut source_char_anchors: Vec<(u64, usize)> = Vec::new();
    let mut seen_hr_chars = HashSet::new();

    for (idx, blk) in model.blocks.iter().enumerate() {
        let blk_disp_u16 = block_disp_starts_u16[idx];
        let blk_src_u16 = blk.source_start;
        let blk_char = disp_mapper.utf16_to_char(blk_disp_u16);
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
            let r_char_start = disp_mapper.utf16_to_char(r_start);
            let r_char_end = disp_mapper.utf16_to_char(r_start + r_len);

            let mut final_mask = run.mask;
            let abs_src_u16 = blk_src_u16 + run.source_start;

            if final_mask & 32768 != 0
                && let Some(zw_byte) =
                    find_char_near_utf16(&text, &disp_mapper, r_start, r_len, '\u{200b}')
            {
                let zw_char = disp_mapper.byte_to_char(zw_byte);
                if seen_hr_chars.insert(zw_char) {
                    block_breaks.push(BlockBreak {
                        char_pos: zw_char,
                        source_start: blk_src_u16,
                        source_end: blk_src_u16 + blk.source_length,
                        kind: BlockBreakKind::Hr,
                    });
                }
            }

            if final_mask & (1 << 18) != 0 {
                if let Some(link) = wiki_links.iter().find(|l| {
                    abs_src_u16 >= l.start_utf16 && abs_src_u16 < l.start_utf16 + l.length_utf16
                }) {
                    if link.resolved_path.is_none() {
                        final_mask |= 1 << 19;
                    }
                    if r_char_end > r_char_start {
                        link_spans.push(LinkSpan {
                            char_start: r_char_start as i32,
                            char_end: r_char_end as i32,
                            action: LinkAction::Wiki {
                                target: link.target.clone(),
                                heading: link.heading.clone(),
                                resolved_path: link.resolved_path.clone(),
                            },
                        });
                    }
                }
            } else if final_mask & 128 != 0
                && let Some(url) = extract_markdown_url_around_utf16(
                    source_text,
                    &src_mapper,
                    abs_src_u16,
                    run.source_length,
                )
                && r_char_end > r_char_start
            {
                link_spans.push(LinkSpan {
                    char_start: r_char_start as i32,
                    char_end: r_char_end as i32,
                    action: LinkAction::ExternalUrl(url),
                });
            }

            if final_mask & (512 | 8192) != 0 && r_char_end > r_char_start {
                let b0 = disp_mapper.char_to_byte(r_char_start);
                let b1 = disp_mapper.char_to_byte(r_char_end);
                let tag_text = text.get(b0..b1).unwrap_or("");
                let tag = tag_text.trim_start_matches('#').trim().to_string();
                if !tag.is_empty() {
                    link_spans.push(LinkSpan {
                        char_start: r_char_start as i32,
                        char_end: r_char_end as i32,
                        action: LinkAction::Tag(tag),
                    });
                }
            }

            if r_char_end > r_char_start && final_mask != 0 {
                char_runs.push(CharRun {
                    char_start: r_char_start,
                    char_end: r_char_end,
                    mask: final_mask,
                });
            }
        }
    }

    let mut diag_ord = 0u32;
    for (idx, blk) in model.blocks.iter().enumerate() {
        if !blk.diagram {
            continue;
        }
        diag_ord += 1;
        let blk_disp_u16 = block_disp_starts_u16[idx];
        let blk_len_u16 = blk.display_length as usize;
        if let Some(byte_pos) =
            find_char_near_utf16(&text, &disp_mapper, blk_disp_u16, blk_len_u16, '\u{fffc}')
        {
            let char_pos = disp_mapper.byte_to_char(byte_pos);
            let diag = diagrams
                .iter()
                .find(|d| d.ordinal == diag_ord)
                .cloned()
                .unwrap_or(RenderedDiagram {
                    ordinal: diag_ord,
                    source_start: 0,
                    source_end: 0,
                    scene: None,
                    failure: Some(crate::i18n::tr(crate::i18n::Key::DiagramNotReady).into()),
                    collapsed: Vec::new(),
                    previous: None,
                });
            let card = build_diagram_card(
                env.note_path.clone(),
                source_text.to_string(),
                false,
                diag,
                Arc::clone(&env.diagram_engine),
                make_diagram_callbacks(
                    env,
                    env.note_path.clone(),
                    on_diagram_collapse.clone(),
                    on_diagram_scene_updated.clone(),
                ),
                env.content_width,
            );
            block_breaks.push(BlockBreak {
                char_pos,
                source_start: blk.source_start,
                source_end: blk.source_start + blk.source_length,
                kind: BlockBreakKind::Diagram(card),
            });
        }
    }

    for m in math {
        let Some(&blk_idx) = block_index_by_id.get(&m.block.block_id) else {
            continue;
        };
        let blk = &model.blocks[blk_idx];
        let local_u16 = m.block.display_start.saturating_sub(blk.display_start) as usize;
        let disp_u16 = block_disp_starts_u16[blk_idx] + local_u16;
        if let Some(byte_pos) = find_char_near_utf16(
            &text,
            &disp_mapper,
            disp_u16,
            m.block.display_length.max(1) as usize,
            '\u{fffc}',
        ) {
            let char_pos = disp_mapper.byte_to_char(byte_pos);
            if m.block.block {
                let math_block = build_display_math_widget(m, env.content_width);
                let src_start = blk.source_start + m.block.source_start;
                block_breaks.push(BlockBreak {
                    char_pos,
                    source_start: src_start,
                    source_end: src_start + m.block.source_length,
                    kind: BlockBreakKind::DisplayMath(math_block),
                });
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
                inline_anchors.push((char_pos, widget, rise_pango));
            }
        }
    }

    for (media_idx, item) in media.iter().enumerate() {
        let Some(&blk_idx) = block_index_by_id.get(&item.block.block_id) else {
            continue;
        };
        let blk = &model.blocks[blk_idx];
        let local_u16 = item.block.display_start.saturating_sub(blk.display_start) as usize;
        let disp_u16 = block_disp_starts_u16[blk_idx] + local_u16;
        let Some(byte_pos) = find_char_near_utf16(
            &text,
            &disp_mapper,
            disp_u16,
            item.block.display_length.max(1) as usize,
            '\u{fffc}',
        ) else {
            continue;
        };
        let char_pos = disp_mapper.byte_to_char(byte_pos);

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
                let card = build_embed_card(
                    embed_req,
                    pre,
                    env.callbacks.clone(),
                    embed_body_builder.clone(),
                );
                block_breaks.push(BlockBreak {
                    char_pos,
                    source_start: src_u16,
                    source_end: src_u16 + link.length_utf16,
                    kind: BlockBreakKind::Embed(card),
                });
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
            block_breaks.push(BlockBreak {
                char_pos,
                source_start: item.block.source_start,
                source_end: item
                    .block
                    .source_start
                    .max(blk.source_start + blk.source_length),
                kind: BlockBreakKind::Table(tbl),
            });
        } else {
            let img = build_image_widget(item, env.content_width, env.parent_window.as_ref());
            block_breaks.push(BlockBreak {
                char_pos,
                source_start: item.block.source_start,
                source_end: item.block.source_start + 1,
                kind: BlockBreakKind::Image(img),
            });
        }
    }

    block_breaks.sort_by_key(|b| b.char_pos);
    block_breaks.dedup_by_key(|b| b.char_pos);
    inline_anchors.sort_by_key(|a| a.0);
    inline_anchors.dedup_by_key(|a| a.0);
    source_char_anchors.sort_by_key(|a| a.0);

    split_into_segments(
        &text,
        &disp_mapper,
        &char_runs,
        &inline_anchors,
        &link_spans,
        &source_char_anchors,
        block_breaks,
    )
}

fn split_into_segments(
    text: &str,
    disp_mapper: &Utf16Mapper,
    char_runs: &[CharRun],
    inline_anchors: &[(usize, gtk4::Widget, i32)],
    link_spans: &[LinkSpan],
    source_char_anchors: &[(u64, usize)],
    block_breaks: Vec<BlockBreak>,
) -> Vec<AssembledSegment> {
    let total_chars = disp_mapper.char_len();
    let mut segments = Vec::with_capacity(block_breaks.len() * 2 + 1);
    let mut raw_start = 0usize;
    let mut last_src_u16 = source_char_anchors.first().map(|a| a.0).unwrap_or(0);

    for brk in block_breaks {
        let raw_end = brk.char_pos.min(total_chars);
        if raw_end > raw_start
            && let Some(seg) = build_text_segment_slice(
                text,
                disp_mapper,
                char_runs,
                inline_anchors,
                link_spans,
                source_char_anchors,
                raw_start,
                raw_end,
                last_src_u16,
                brk.source_start,
            )
        {
            last_src_u16 = seg.source_end;
            segments.push(AssembledSegment::Text(seg));
        }

        let brk_seg = match brk.kind {
            BlockBreakKind::Diagram(card) => AssembledSegment::Diagram {
                card,
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
            BlockBreakKind::Table(table) => AssembledSegment::Table {
                table,
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
            BlockBreakKind::Image(image) => AssembledSegment::Image {
                image,
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
            BlockBreakKind::DisplayMath(math) => AssembledSegment::DisplayMath {
                math,
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
            BlockBreakKind::Embed(embed) => AssembledSegment::Embed {
                embed,
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
            BlockBreakKind::Hr => AssembledSegment::Hr {
                source_start: brk.source_start,
                source_end: brk.source_end,
            },
        };
        last_src_u16 = brk.source_end.max(last_src_u16);
        segments.push(brk_seg);
        raw_start = (brk.char_pos + 1).min(total_chars);
    }

    if raw_start < total_chars {
        let end_src_u16 = source_char_anchors
            .last()
            .map(|a| a.0)
            .unwrap_or(last_src_u16)
            .max(last_src_u16);
        if let Some(seg) = build_text_segment_slice(
            text,
            disp_mapper,
            char_runs,
            inline_anchors,
            link_spans,
            source_char_anchors,
            raw_start,
            total_chars,
            last_src_u16,
            end_src_u16,
        ) {
            segments.push(AssembledSegment::Text(seg));
        }
    }

    segments
}

#[allow(clippy::too_many_arguments)]
fn build_text_segment_slice(
    text: &str,
    disp_mapper: &Utf16Mapper,
    char_runs: &[CharRun],
    inline_anchors: &[(usize, gtk4::Widget, i32)],
    link_spans: &[LinkSpan],
    source_char_anchors: &[(u64, usize)],
    raw_start: usize,
    raw_end: usize,
    fallback_src_start: u64,
    fallback_src_end: u64,
) -> Option<TextSegmentData> {
    let raw_b0 = disp_mapper.char_to_byte(raw_start);
    let raw_b1 = disp_mapper.char_to_byte(raw_end);
    let raw_slice = text.get(raw_b0..raw_b1)?;

    let lead_trim_bytes = raw_slice
        .bytes()
        .take_while(|&b| b == b'\n' || b == b'\r')
        .count();
    let tail_trim_bytes = raw_slice[lead_trim_bytes..]
        .bytes()
        .rev()
        .take_while(|&b| b == b'\n' || b == b'\r')
        .count();

    let seg_b0 = raw_b0 + lead_trim_bytes;
    let seg_b1 = raw_b1 - tail_trim_bytes;
    if seg_b0 >= seg_b1 {
        return None;
    }
    let seg_text = text.get(seg_b0..seg_b1)?;
    if seg_text.trim().is_empty() {
        return None;
    }

    let seg_start = disp_mapper.byte_to_char(seg_b0);
    let seg_end = disp_mapper.byte_to_char(seg_b1);
    if seg_start >= seg_end {
        return None;
    }

    let seg_runs: Vec<(i32, i32, u32)> = char_runs
        .iter()
        .filter(|r| r.char_end > seg_start && r.char_start < seg_end)
        .filter_map(|r| {
            let cs = r.char_start.max(seg_start) - seg_start;
            let ce = r.char_end.min(seg_end) - seg_start;
            (cs < ce).then_some((cs as i32, ce as i32, r.mask))
        })
        .collect();

    let seg_inline: Vec<InlineAnchor> = inline_anchors
        .iter()
        .filter(|(pos, _, _)| *pos >= seg_start && *pos < seg_end)
        .map(|(pos, w, rise)| InlineAnchor {
            local_char_pos: *pos - seg_start,
            widget: w.clone(),
            rise_pango: *rise,
        })
        .collect();

    let seg_links: Vec<LinkSpan> = link_spans
        .iter()
        .filter(|l| (l.char_end as usize) > seg_start && (l.char_start as usize) < seg_end)
        .filter_map(|l| {
            let cs = (l.char_start as usize).max(seg_start) - seg_start;
            let ce = (l.char_end as usize).min(seg_end) - seg_start;
            (cs < ce).then(|| LinkSpan {
                char_start: cs as i32,
                char_end: ce as i32,
                action: l.action.clone(),
            })
        })
        .collect();

    let mut seg_anchors: Vec<(u64, i32)> = source_char_anchors
        .iter()
        .filter(|(_, ch)| *ch >= raw_start && *ch <= raw_end)
        .map(|&(src, ch)| (src, (ch.clamp(seg_start, seg_end) - seg_start) as i32))
        .collect();
    if seg_anchors.is_empty() {
        seg_anchors.push((fallback_src_start, 0));
    }

    let src_start = seg_anchors
        .first()
        .map(|a| a.0)
        .unwrap_or(fallback_src_start);
    let src_end = seg_anchors
        .last()
        .map(|a| a.0)
        .unwrap_or(fallback_src_end)
        .max(fallback_src_end)
        .max(src_start);

    Some(TextSegmentData {
        text: seg_text.to_string(),
        runs: seg_runs,
        inline_anchors: seg_inline,
        link_spans: seg_links,
        source_anchors: seg_anchors,
        source_start: src_start,
        source_end: src_end,
    })
}

pub fn make_diagram_callbacks(
    env: &ReadingEnv,
    note_path: String,
    on_collapse_changed: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
    on_scene_updated: Rc<dyn Fn(u32, hortus_ffi::SceneData)>,
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
        on_scene_updated,
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

pub fn convert_embed_to_model(info: &EmbedContentInfo) -> DecodedReadingModel {
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

#[cfg(feature = "automation")]
pub fn compute_stats(prepared: &PreparedReadingNote) -> NoteReadingStats {
    let mut s = NoteReadingStats {
        prepared_off_main_thread: prepared.prepared_off_main_thread,
        diagram_count: prepared.diagrams.len(),
        math_feedback_rounds: prepared
            .math
            .iter()
            .filter_map(|m| m.math.as_ref())
            .map(|m| m.missing_metrics_feedbacks)
            .sum(),
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

fn find_char_near_utf16(
    text: &str,
    mapper: &Utf16Mapper,
    disp_start_u16: usize,
    disp_len_u16: usize,
    needle: char,
) -> Option<usize> {
    let b_start = mapper.utf16_to_byte(disp_start_u16);
    let b_end = mapper.utf16_to_byte(disp_start_u16 + disp_len_u16.max(1));
    text.get(b_start..b_end.min(text.len()))?
        .find(needle)
        .map(|rel| b_start + rel)
}

fn extract_markdown_url_around_utf16(
    source: &str,
    mapper: &Utf16Mapper,
    start_u16: u64,
    len_u16: u64,
) -> Option<String> {
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
    let slice = source.get(b_start..b_end.min(source.len()))?;
    if slice.starts_with("http://") || slice.starts_with("https://") {
        return Some(slice.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_text_segment_slice_trims_and_rebases_runs_and_links() {
        let text = "\n\nHello world!\n";
        let mapper = Utf16Mapper::new(text);
        let char_runs = vec![CharRun {
            char_start: 2,
            char_end: 7,
            mask: 4,
        }];
        let links = vec![LinkSpan {
            char_start: 8,
            char_end: 13,
            action: LinkAction::Tag("demo".into()),
        }];
        let anchors = vec![(0, 2), (10, 8)];
        let seg = build_text_segment_slice(
            text,
            &mapper,
            &char_runs,
            &[],
            &links,
            &anchors,
            0,
            text.chars().count(),
            0,
            20,
        )
        .expect("segment should exist");
        assert_eq!(seg.text, "Hello world!");
        assert_eq!(seg.runs, vec![(0, 5, 4)]);
        assert_eq!(seg.link_spans.len(), 1);
        assert_eq!(seg.link_spans[0].char_start, 6);
        assert_eq!(seg.link_spans[0].char_end, 11);
        assert_eq!(seg.source_anchors, vec![(0, 0), (10, 6)]);
    }
}
