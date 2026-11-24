// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 默认只读 Reading View 表面组装：统一 `activate_link_at_widget_point`（G-06 修复）与弱回调解环。

use crate::core::Utf16Mapper;
use crate::platform::style::{apply_mask_tags, create_tag_table};
#[cfg(feature = "automation")]
use crate::reading::assemble::compute_stats;
use crate::reading::assemble::{
    AdjustedRun, AnchorKind, LinkAction, LinkSpan, assemble_reading_buffer_data,
    convert_embed_to_model, make_diagram_callbacks,
};
#[cfg(feature = "automation")]
use crate::reading::prepare::NoteReadingStats;
use crate::reading::prepare::PreparedReadingNote;
use crate::render::diagram::RenderedDiagram;
use crate::render::diagram_card::build_diagram_card;
use crate::render::embed::{EmbedBodyBuilder, EmbedCallbacks, EmbedRequest, PreparedEmbed};
use crate::render::math::{MathRenderService, PreparedMathBlock};
use crate::render::media::PreparedMedia;
use gtk4::gio;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, EmbedContentInfo, KnowledgeMarkupInfo, VaultHandle, WorkspaceCore,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

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
pub struct ReadingSurfaceHandle {
    pub root: gtk4::Widget,
    pub scrolled: gtk4::ScrolledWindow,
    pub text_view: Option<gtk4::TextView>,
    source_char_anchors: Vec<(u64, i32)>,
    #[allow(dead_code)]
    link_spans: Rc<Vec<LinkSpan>>,
    #[allow(dead_code)]
    link_dispatcher: Option<Rc<LinkDispatcher>>,
    #[cfg(feature = "automation")]
    pub diagram_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub table_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub image_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub embed_widgets: Vec<gtk4::Widget>,
    #[cfg(feature = "automation")]
    pub stats: NoteReadingStats,
}

#[derive(Clone)]
struct LinkDispatcher {
    spans: Rc<Vec<LinkSpan>>,
    callbacks: EmbedCallbacks,
    handle: Arc<VaultHandle>,
    src_note: String,
    open_tag: Rc<dyn Fn(String)>,
}

impl LinkDispatcher {
    fn activate_at_point(&self, text_view: &gtk4::TextView, x: f64, y: f64) -> bool {
        let (bx, by) =
            text_view.window_to_buffer_coords(gtk4::TextWindowType::Widget, x as i32, y as i32);
        let Some(it) = text_view.iter_at_location(bx, by) else {
            return false;
        };
        let off = it.offset();
        let Some(span) = self
            .spans
            .iter()
            .find(|s| off >= s.char_start && off < s.char_end)
        else {
            return false;
        };
        match &span.action {
            LinkAction::Wiki {
                target,
                heading,
                resolved_path,
            } => {
                if let Some(p) = resolved_path {
                    (self.callbacks.on_open_note)(p.clone(), heading.clone());
                } else {
                    let res = self
                        .handle
                        .resolve_link(self.src_note.clone(), target.clone());
                    if let Some(p) = res.resolved_path {
                        (self.callbacks.on_open_note)(p, heading.clone());
                    } else {
                        (self.callbacks.on_open_note)(target.clone(), heading.clone());
                    }
                }
            }
            LinkAction::ExternalUrl(url) => {
                (self.callbacks.on_open_external)(url.clone());
            }
            LinkAction::Tag(tag) => {
                (self.open_tag)(tag.clone());
            }
        }
        true
    }
}

impl ReadingSurfaceHandle {
    /// 与鼠标点击共用同一个坐标命中入口（G-06 修复：不再发射伪 `GestureClick::released`）。
    #[cfg(feature = "automation")]
    pub fn activate_link_at_widget_point(&self, x: f64, y: f64) -> bool {
        let (Some(tv), Some(dispatcher)) = (&self.text_view, &self.link_dispatcher) else {
            return false;
        };
        dispatcher.activate_at_point(tv, x, y)
    }

    /// 自动化回归：按可见文字命中阅读视图双链，并走真实坐标命中与路由逻辑。
    #[cfg(feature = "automation")]
    pub fn click_wikilink(&self, label: &str) -> Option<(String, Option<String>)> {
        let text_view = self.text_view.as_ref()?;
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
        self.activate_link_at_widget_point(f64::from(wx), f64::from(wy));
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

fn make_embed_body_builder() -> EmbedBodyBuilder {
    Rc::new(|req, info, media, math, diagrams, callbacks| {
        let mut next_ancestors = req.ancestors.clone();
        if let Some(ref p) = info.resolved_path {
            next_ancestors.push(p.clone());
        }
        let note_rel = info
            .resolved_path
            .clone()
            .unwrap_or_else(|| req.target.clone());
        let env = ReadingEnv {
            note_path: note_rel,
            vault_root: req.vault_root.clone(),
            core: Arc::clone(&req.core),
            handle: Arc::clone(&req.handle),
            diagram_engine: Arc::clone(&req.diagram_engine),
            math_service: Arc::clone(&req.math_service),
            font_size: req.font_size,
            content_width: EmbedRequest::content_width(req.width),
            ancestors: next_ancestors,
            depth: req.depth + 1,
            parent_window: None,
            callbacks: callbacks.clone(),
            on_open_source: Rc::new(|_, _| {}),
            on_open_tag: callbacks.on_open_tag.clone(),
        };
        build_embedded_note_body(&env, info, media, math, diagrams)
    })
}

/// 在 GTK 主线程组装整篇阅读表面（纯消费后台准备好的数据，零阻塞计算）。
pub fn build_reading_surface(
    prepared: &PreparedReadingNote,
    env: &ReadingEnv,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
) -> ReadingSurfaceHandle {
    #[cfg(feature = "automation")]
    let stats = compute_stats(prepared);

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
            link_dispatcher: None,
            #[cfg(feature = "automation")]
            diagram_widgets: Vec::new(),
            #[cfg(feature = "automation")]
            table_widgets: Vec::new(),
            #[cfg(feature = "automation")]
            image_widgets: Vec::new(),
            #[cfg(feature = "automation")]
            embed_widgets: Vec::new(),
            #[cfg(feature = "automation")]
            stats,
        };
    }

    let embed_builder = make_embed_body_builder();
    let assembled = assemble_reading_buffer_data(
        &prepared.model,
        &prepared.source_text,
        prepared.knowledge.as_ref(),
        &prepared.diagrams,
        &prepared.math,
        &prepared.media,
        &prepared.embeds,
        env,
        on_diagram_collapse,
        &embed_builder,
    );

    let (text_view, scrolled, link_spans, link_dispatcher) = install_text_view_with_anchors(
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
        link_dispatcher,
        #[cfg(feature = "automation")]
        diagram_widgets: assembled.diagram_widgets,
        #[cfg(feature = "automation")]
        table_widgets: assembled.table_widgets,
        #[cfg(feature = "automation")]
        image_widgets: assembled.image_widgets,
        #[cfg(feature = "automation")]
        embed_widgets: assembled.embed_widgets,
        #[cfg(feature = "automation")]
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
            let core_bg = Arc::clone(&core);
            let vault_bg = vault.clone();
            let note_bg = note.clone();
            gio::spawn_blocking(move || {
                let _ = core_bg.remember_diagram(vault_bg, note_bg, ordinal, collapsed);
            });
        }
    });
    let embed_builder = make_embed_body_builder();
    let assembled = assemble_reading_buffer_data(
        &converted,
        &info.content,
        Some(&know),
        diagrams,
        math,
        media,
        &empty_embeds,
        env,
        on_collapse,
        &embed_builder,
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
    Rc<Vec<LinkSpan>>,
    Option<Rc<LinkDispatcher>>,
) {
    let tag_table = create_tag_table();
    let buffer = gtk4::TextBuffer::new(Some(&tag_table));
    buffer.set_text(display_text);
    let spans = Rc::new(link_spans.to_vec());

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

    let mut link_dispatcher = None;
    if !link_spans.is_empty() {
        let motion = gtk4::EventControllerMotion::new();
        {
            let tv_weak = text_view.downgrade();
            let spans_m = spans.clone();
            motion.connect_motion(move |_, x, y| {
                let Some(tv_m) = tv_weak.upgrade() else {
                    return;
                };
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

        let dispatcher = Rc::new(LinkDispatcher {
            spans: spans.clone(),
            callbacks: env.callbacks.clone(),
            handle: Arc::clone(&env.handle),
            src_note: env.note_path.clone(),
            open_tag: env.on_open_tag.clone(),
        });

        let click = gtk4::GestureClick::new();
        click.set_button(1);
        {
            let tv_weak = text_view.downgrade();
            let disp = Rc::clone(&dispatcher);
            click.connect_released(move |_, _, x, y| {
                if let Some(tv) = tv_weak.upgrade() {
                    disp.activate_at_point(&tv, x, y);
                }
            });
        }
        text_view.add_controller(click);
        link_dispatcher = Some(dispatcher);
    }

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(!embedded)
        .hexpand(true)
        .child(&text_view)
        .build();

    (text_view, scrolled, spans, link_dispatcher)
}
