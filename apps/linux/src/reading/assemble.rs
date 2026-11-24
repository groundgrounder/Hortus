// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 阅读缓冲数据组装：锚点映射、链接区间、内嵌结构与统计指标计算。

use crate::core::Utf16Mapper;
#[cfg(feature = "automation")]
use crate::reading::prepare::NoteReadingStats;
#[cfg(feature = "automation")]
use crate::reading::prepare::PreparedReadingNote;
use crate::reading::prepare::{DecodedBlock, DecodedReadingModel, DecodedRun};
use crate::reading::surface::ReadingEnv;
use crate::render::diagram::{DiagramExportKind, DiagramExportMeta, RenderedDiagram};
use crate::render::diagram_card::{DiagramCallbacks, build_diagram_card};
use crate::render::embed::{EmbedBodyBuilder, EmbedRequest, PreparedEmbed, build_embed_card};
use crate::render::math::PreparedMathBlock;
use crate::render::math_widgets::{build_display_math_widget, build_inline_math_widget};
#[cfg(feature = "automation")]
use crate::render::media::summarize_media_stats;
use crate::render::media::{PreparedMedia, build_image_widget, build_table_widget};
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::{EmbedContentInfo, KnowledgeMarkupInfo, ResolvedLinkInfo};
use libadwaita as adw;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub enum AnchorKind {
    Widget(gtk4::Widget),
    InlineWidget {
        widget: gtk4::Widget,
        rise_pango: i32,
    },
}

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
pub struct AdjustedRun {
    pub disp_start_u16: usize,
    pub disp_len_u16: usize,
    pub mask: u32,
}

pub struct AssembledReadingBuffer {
    pub text: String,
    pub runs: Vec<AdjustedRun>,
    pub anchors: Vec<(usize, AnchorKind)>,
    pub source_char_anchors: Vec<(u64, i32)>,
    pub link_spans: Vec<LinkSpan>,
    #[cfg(feature = "automation")]
    pub diagram_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub table_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub image_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub embed_widgets: Vec<gtk4::Widget>,
}

#[allow(clippy::too_many_arguments)]
pub fn assemble_reading_buffer_data(
    model: &DecodedReadingModel,
    source_text: &str,
    knowledge: Option<&KnowledgeMarkupInfo>,
    diagrams: &[RenderedDiagram],
    math: &[PreparedMathBlock],
    media: &[PreparedMedia],
    precomputed_embeds: &HashMap<usize, PreparedEmbed>,
    env: &ReadingEnv,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
    embed_body_builder: &EmbedBodyBuilder,
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
    let src_mapper = Utf16Mapper::new(source_text);

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
    #[cfg(feature = "automation")]
    let mut diagram_widgets: Vec<gtk4::Widget> = Vec::new();
    #[cfg(feature = "automation")]
    let mut table_widgets: Vec<gtk4::Widget> = Vec::new();
    #[cfg(feature = "automation")]
    let mut image_widgets: Vec<gtk4::Widget> = Vec::new();
    #[cfg(feature = "automation")]
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

            if final_mask & 32768 != 0
                && let Some(zw_byte) =
                    find_char_near_utf16(&text, &disp_mapper, r_start, r_len, '\u{200b}')
            {
                let zw_char = text[..zw_byte].chars().count();
                if seen_hr_chars.insert(zw_char) {
                    let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
                    sep.add_css_class("hortus-reading-hr");
                    sep.set_hexpand(true);
                    anchors.push((zw_char, AnchorKind::Widget(sep.upcast())));
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
                    char_start: r_char_start,
                    char_end: r_char_end,
                    action: LinkAction::ExternalUrl(url),
                });
            }

            if final_mask & (512 | 8192) != 0 && r_char_end > r_char_start {
                let b0 = disp_mapper.char_to_byte(r_char_start as usize);
                let b1 = disp_mapper.char_to_byte(r_char_end as usize);
                let tag_text = text.get(b0..b1).unwrap_or("");
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
            #[cfg(feature = "automation")]
            diagram_widgets.push(w.clone());
            anchors.push((char_pos, AnchorKind::Widget(w)));
        }
    }

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
            &disp_mapper,
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
            &disp_mapper,
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
                let card = build_embed_card(
                    embed_req,
                    pre,
                    env.callbacks.clone(),
                    embed_body_builder.clone(),
                );
                #[cfg(feature = "automation")]
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
            #[cfg(feature = "automation")]
            table_widgets.push(tbl.clone());
            anchors.push((char_pos, AnchorKind::Widget(tbl)));
        } else {
            let img = build_image_widget(item, env.content_width, env.parent_window.as_ref());
            #[cfg(feature = "automation")]
            image_widgets.push(img.clone());
            anchors.push((char_pos, AnchorKind::Widget(img)));
        }
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
        #[cfg(feature = "automation")]
        diagram_widgets,
        #[cfg(feature = "automation")]
        table_widgets,
        #[cfg(feature = "automation")]
        image_widgets,
        #[cfg(feature = "automation")]
        embed_widgets,
    }
}

pub fn make_diagram_callbacks(
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
