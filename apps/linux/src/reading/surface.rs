// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 分段块级 Reading View 表面组装：合并连续文本段（随 `HortusReadingViewport` 统一居中列宽）+ 原生块级控件，
//! 支持原地宽度重排（零销毁重建）、精准跨段滚动锚点与双链坐标命中。

use crate::platform::style::{MaskTagSet, apply_mask_tags_cached, create_tag_table};
#[cfg(feature = "automation")]
use crate::reading::assemble::compute_stats;
use crate::reading::assemble::{
    AssembledSegment, LinkAction, LinkSpan, TextSegmentData, assemble_reading_segments,
    convert_embed_to_model, make_diagram_callbacks,
};
#[cfg(feature = "automation")]
use crate::reading::prepare::NoteReadingStats;
use crate::reading::prepare::PreparedReadingNote;
use crate::render::diagram::RenderedDiagram;
use crate::render::diagram_card::{DiagramCard, build_diagram_card};
use crate::render::embed::{
    EmbedBodyBuilder, EmbedBodyResult, EmbedCallbacks, EmbedCard, EmbedRequest, PreparedEmbed,
};
use crate::render::math::{MathRenderService, PreparedMathBlock};
use crate::render::math_widgets::DisplayMathBlock;
use crate::render::media::{ImageBlock, PreparedMedia, TableBlock};
use gtk4::gio;
use gtk4::graphene;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, EmbedContentInfo, KnowledgeMarkupInfo, VaultHandle, WorkspaceCore,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct ReadingEnv {
    pub note_path: String,
    pub shared_note_path: Rc<RefCell<String>>,
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
    pub scrolled: Option<gtk4::ScrolledWindow>,
    pub callbacks: EmbedCallbacks,
    pub on_open_source: Rc<dyn Fn(u64, u64)>,
    /// 阅读视图内点击 `#标签` 时切到标签面板并筛选。
    pub on_open_tag: Rc<dyn Fn(String)>,
}

#[derive(Clone)]
pub struct TextSegment {
    pub view: gtk4::TextView,
    pub buffer: gtk4::TextBuffer,
    pub source_anchors: Vec<(u64, i32)>,
    pub source_start: u64,
    pub source_end: u64,
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub link_spans: Rc<Vec<LinkSpan>>,
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub link_dispatcher: Option<Rc<LinkDispatcher>>,
}

#[derive(Clone)]
pub enum ReadingSegment {
    Text(TextSegment),
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
        separator: gtk4::Separator,
        source_start: u64,
        source_end: u64,
    },
}

impl ReadingSegment {
    fn widget(&self) -> &gtk4::Widget {
        match self {
            Self::Text(seg) => seg.view.upcast_ref(),
            Self::Diagram { card, .. } => card.widget(),
            Self::Table { table, .. } => table.widget(),
            Self::Image { image, .. } => image.widget(),
            Self::DisplayMath { math, .. } => math.widget(),
            Self::Embed { embed, .. } => embed.widget(),
            Self::Hr { separator, .. } => separator.upcast_ref(),
        }
    }

    fn source_start(&self) -> u64 {
        match self {
            Self::Text(seg) => seg.source_start,
            Self::Diagram { source_start, .. }
            | Self::Table { source_start, .. }
            | Self::Image { source_start, .. }
            | Self::DisplayMath { source_start, .. }
            | Self::Embed { source_start, .. }
            | Self::Hr { source_start, .. } => *source_start,
        }
    }

    fn source_end(&self) -> u64 {
        match self {
            Self::Text(seg) => seg.source_end,
            Self::Diagram { source_end, .. }
            | Self::Table { source_end, .. }
            | Self::Image { source_end, .. }
            | Self::DisplayMath { source_end, .. }
            | Self::Embed { source_end, .. }
            | Self::Hr { source_end, .. } => *source_end,
        }
    }
}

#[derive(Clone)]
pub struct ReadingSurfaceHandle {
    pub root: gtk4::Widget,
    pub content_box: gtk4::Box,
    pub scrolled: gtk4::ScrolledWindow,
    pub segments: Vec<ReadingSegment>,
    scroll_token: Rc<std::cell::Cell<u64>>,
    #[cfg(feature = "automation")]
    pub stats: NoteReadingStats,
}

#[derive(Clone)]
pub struct LinkDispatcher {
    spans: Rc<Vec<LinkSpan>>,
    callbacks: EmbedCallbacks,
    handle: Arc<VaultHandle>,
    src_note: Rc<RefCell<String>>,
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
                    let src = self.src_note.borrow().clone();
                    let res = self.handle.resolve_link(src, target.clone());
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
    pub fn text_views(&self) -> Vec<(gtk4::TextView, gtk4::TextBuffer)> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Text(t) => Some((t.view.clone(), t.buffer.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn plain_text(&self) -> String {
        let mut parts = Vec::new();
        for seg in &self.segments {
            if let ReadingSegment::Text(t) = seg {
                let raw = t
                    .buffer
                    .text(&t.buffer.start_iter(), &t.buffer.end_iter(), false)
                    .to_string();
                let cleaned = raw.replace('\u{fffc}', "");
                if !cleaned.trim().is_empty() {
                    parts.push(cleaned);
                }
            }
        }
        parts.join("\n\n")
    }

    pub fn relayout_to_width(&self, avail_w: f64) {
        for seg in &self.segments {
            match seg {
                ReadingSegment::Diagram { card, .. } => card.relayout_to_width(avail_w),
                ReadingSegment::Embed { embed, .. } => embed.relayout_to_width(avail_w),
                _ => {}
            }
        }
    }

    #[cfg(feature = "automation")]
    pub fn has_pending_diagram_relayout(&self) -> bool {
        self.segments.iter().any(|seg| match seg {
            ReadingSegment::Diagram { card, .. } => card.is_relayout_pending(),
            ReadingSegment::Embed { embed, .. } => embed.is_relayout_pending(),
            _ => false,
        })
    }

    /// 自动化回归：按可见文字命中阅读视图双链，并走真实坐标命中与路由逻辑。
    #[cfg(feature = "automation")]
    pub fn click_wikilink(&self, label: &str) -> Option<(String, Option<String>)> {
        for seg in &self.segments {
            let ReadingSegment::Text(t) = seg else {
                continue;
            };
            let text = t
                .buffer
                .text(&t.buffer.start_iter(), &t.buffer.end_iter(), false)
                .to_string();
            let Some(byte) = text.find(label) else {
                continue;
            };
            let char_off = text[..byte].chars().count() as i32;
            let Some(span) = t
                .link_spans
                .iter()
                .find(|s| char_off >= s.char_start && char_off < s.char_end)
            else {
                continue;
            };
            let LinkAction::Wiki {
                target, heading, ..
            } = span.action.clone()
            else {
                continue;
            };
            let iter = t.buffer.iter_at_offset(char_off);
            let rect = t.view.iter_location(&iter);
            let (wx, wy) = t.view.buffer_to_window_coords(
                gtk4::TextWindowType::Widget,
                rect.x() + 2,
                rect.y() + rect.height().max(1) / 2,
            );
            if let Some(pt) = t.view.compute_point(
                &self.content_box,
                &graphene::Point::new(wx as f32, wy as f32),
            ) {
                let adj = self.scrolled.vadjustment();
                let target_y = (f64::from(pt.y()) - adj.page_size() * 0.25).max(0.0);
                let max_y = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value(target_y.min(max_y));
            }
            if let Some(ref dispatcher) = t.link_dispatcher {
                dispatcher.activate_at_point(&t.view, f64::from(wx), f64::from(wy));
            }
            return Some((target, heading));
        }
        None
    }

    pub fn scroll_token(&self) -> u64 {
        self.scroll_token.get()
    }

    fn bump_scroll_token(&self) {
        self.scroll_token
            .set(self.scroll_token.get().wrapping_add(1));
    }

    pub fn scroll_to_source_utf16(&self, target_utf16: u64) {
        self.bump_scroll_token();
        if self.segments.is_empty() {
            return;
        }
        let seg = self
            .segments
            .iter()
            .find(|s| target_utf16 <= s.source_end())
            .or_else(|| self.segments.last())
            .unwrap();

        let target_y = match seg {
            ReadingSegment::Text(t) => {
                let char_off = map_source_utf16_to_reading_char(&t.source_anchors, target_utf16);
                let iter = t
                    .buffer
                    .iter_at_offset(char_off.clamp(0, t.buffer.char_count()));
                let rect = t.view.iter_location(&iter);
                let (wx, wy) = t.view.buffer_to_window_coords(
                    gtk4::TextWindowType::Widget,
                    rect.x(),
                    rect.y(),
                );
                t.view
                    .compute_point(
                        &self.content_box,
                        &graphene::Point::new(wx as f32, wy as f32),
                    )
                    .map(|pt| f64::from(pt.y()))
                    .unwrap_or(0.0)
            }
            other => other
                .widget()
                .compute_point(&self.content_box, &graphene::Point::new(0.0, 0.0))
                .map(|pt| f64::from(pt.y()))
                .unwrap_or(0.0),
        };

        let adj = self.scrolled.vadjustment();
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value((target_y - 12.0).max(0.0).min(max_y));
    }

    pub fn current_source_utf16(&self) -> u64 {
        let top_y = self.scrolled.vadjustment().value().max(0.0) + 12.0;
        for seg in &self.segments {
            let w = seg.widget();
            if let Some(bounds) = w.compute_bounds(&self.content_box) {
                let bottom = f64::from(bounds.y() + bounds.height());
                if bottom >= top_y {
                    return match seg {
                        ReadingSegment::Text(t) => {
                            let local_y = self
                                .content_box
                                .compute_point(&t.view, &graphene::Point::new(0.0, top_y as f32))
                                .map(|pt| f64::from(pt.y()).max(0.0))
                                .unwrap_or(0.0);
                            let (_, by) = t.view.window_to_buffer_coords(
                                gtk4::TextWindowType::Widget,
                                0,
                                local_y as i32,
                            );
                            let (iter, _) = t.view.line_at_y(by.max(0));
                            map_reading_char_to_source_utf16(&t.source_anchors, iter.offset())
                        }
                        _ => seg.source_start(),
                    };
                }
            }
        }
        0
    }

    pub fn scroll_fraction(&self, fraction: f64) {
        self.bump_scroll_token();
        let adj = self.scrolled.vadjustment();
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value((max_y * fraction.clamp(0.0, 1.0)).max(0.0));
    }

    #[cfg(feature = "automation")]
    pub fn diagram_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Diagram { card, .. } => Some(card.widget().clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn table_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Table { table, .. } => Some(table.widget().clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn image_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Image { image, .. } => Some(image.measured_widget().clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn embed_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Embed { embed, .. } => Some(embed.widget().clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn math_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::DisplayMath { math, .. } => Some(math.widget().clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn hr_widgets(&self) -> Vec<gtk4::Widget> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Hr { separator, .. } => Some(separator.clone().upcast()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn diagram_horizontal_overflows(&self) -> Vec<bool> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Diagram { card, .. } => Some(card.has_horizontal_overflow()),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn text_segment_layouts(&self) -> Vec<(i32, f64)> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Text(t) => {
                    let x = t
                        .view
                        .compute_point(&self.content_box, &graphene::Point::new(0.0, 0.0))
                        .map(|pt| f64::from(pt.x()))
                        .unwrap_or(0.0);
                    Some((t.view.width(), x))
                }
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn text_segment_scrolled_offsets(&self) -> Vec<f64> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                ReadingSegment::Text(t) => t
                    .view
                    .compute_point(&self.scrolled, &graphene::Point::new(0.0, 0.0))
                    .map(|pt| f64::from(pt.x())),
                _ => None,
            })
            .collect()
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
            note_path: note_rel.clone(),
            shared_note_path: Rc::new(RefCell::new(note_rel)),
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
            scrolled: None,
            callbacks: callbacks.clone(),
            on_open_source: Rc::new(|_, _| {}),
            on_open_tag: callbacks.on_open_tag.clone(),
        };
        build_embedded_note_body(&env, info, media, math, diagrams)
    })
}

/// 在 GTK 主线程组装分段块级阅读表面（纯消费后台准备好的数据，零阻塞计算）。
pub fn build_reading_surface(
    prepared: &PreparedReadingNote,
    env: &ReadingEnv,
    on_diagram_collapse: Rc<dyn Fn(u32, Vec<String>, hortus_ffi::SceneData)>,
    on_diagram_scene_updated: Rc<dyn Fn(u32, hortus_ffi::SceneData)>,
) -> ReadingSurfaceHandle {
    #[cfg(feature = "automation")]
    let stats = compute_stats(prepared);

    let scrolled = env.scrolled.clone().unwrap_or_else(|| {
        gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .build()
    });

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    content_box.add_css_class("hortus-reading-box");
    content_box.set_margin_top(24);
    content_box.set_margin_bottom(48);
    content_box.set_hexpand(true);
    content_box.set_vexpand(true);

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
                failure: Some(crate::i18n::tr(crate::i18n::Key::DiagramNotReady).into()),
                collapsed: Vec::new(),
                previous: None,
            });
        let card = build_diagram_card(
            prepared.path.clone(),
            prepared.source_text.clone(),
            true,
            diag,
            Arc::clone(&env.diagram_engine),
            make_diagram_callbacks(
                env,
                prepared.path.clone(),
                on_diagram_collapse,
                on_diagram_scene_updated,
            ),
            env.content_width,
        );
        content_box.append(card.widget());
        let segments = vec![ReadingSegment::Diagram {
            card,
            source_start: 0,
            source_end: prepared.source_text.encode_utf16().count() as u64,
        }];
        return ReadingSurfaceHandle {
            root: content_box.clone().upcast(),
            content_box,
            scrolled,
            segments,
            scroll_token: Rc::new(std::cell::Cell::new(0)),
            #[cfg(feature = "automation")]
            stats,
        };
    }

    let embed_builder = make_embed_body_builder();
    let assembled = assemble_reading_segments(
        &prepared.model,
        &prepared.source_text,
        prepared.knowledge.as_ref(),
        &prepared.diagrams,
        &prepared.math,
        &prepared.media,
        &prepared.embeds,
        env,
        on_diagram_collapse,
        on_diagram_scene_updated,
        &embed_builder,
    );

    let segments = mount_assembled_segments(&content_box, assembled, env, false);

    ReadingSurfaceHandle {
        root: content_box.clone().upcast(),
        content_box,
        scrolled,
        segments,
        scroll_token: Rc::new(std::cell::Cell::new(0)),
        #[cfg(feature = "automation")]
        stats,
    }
}

/// 为嵌入卡片（`"note"` 类型）构建内联只读正文部件（同样走分段块级逻辑）。
pub fn build_embedded_note_body(
    env: &ReadingEnv,
    info: &EmbedContentInfo,
    media: &[PreparedMedia],
    math: &[PreparedMathBlock],
    diagrams: &[RenderedDiagram],
) -> EmbedBodyResult {
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
    let assembled = assemble_reading_segments(
        &converted,
        &info.content,
        Some(&know),
        diagrams,
        math,
        media,
        &empty_embeds,
        env,
        on_collapse,
        Rc::new(|_, _| {}),
        &embed_builder,
    );

    let body_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    body_box.set_hexpand(true);
    body_box.set_halign(gtk4::Align::Fill);

    let segments = mount_assembled_segments(&body_box, assembled, env, true);
    let nested_diagrams = segments
        .into_iter()
        .filter_map(|seg| match seg {
            ReadingSegment::Diagram { card, .. } => Some(card),
            _ => None,
        })
        .collect();

    EmbedBodyResult {
        widget: body_box.upcast(),
        diagrams: nested_diagrams,
    }
}

fn mount_assembled_segments(
    container: &gtk4::Box,
    assembled: Vec<AssembledSegment>,
    env: &ReadingEnv,
    embedded: bool,
) -> Vec<ReadingSegment> {
    let mut segments = Vec::with_capacity(assembled.len());
    for item in assembled {
        match item {
            AssembledSegment::Text(data) => {
                let (widget_to_append, seg) = install_text_segment(data, env, embedded);
                container.append(&widget_to_append);
                segments.push(ReadingSegment::Text(seg));
            }
            AssembledSegment::Diagram {
                card,
                source_start,
                source_end,
            } => {
                container.append(card.widget());
                segments.push(ReadingSegment::Diagram {
                    card,
                    source_start,
                    source_end,
                });
            }
            AssembledSegment::Table {
                table,
                source_start,
                source_end,
            } => {
                container.append(table.widget());
                segments.push(ReadingSegment::Table {
                    table,
                    source_start,
                    source_end,
                });
            }
            AssembledSegment::Image {
                image,
                source_start,
                source_end,
            } => {
                container.append(image.widget());
                segments.push(ReadingSegment::Image {
                    image,
                    source_start,
                    source_end,
                });
            }
            AssembledSegment::DisplayMath {
                math,
                source_start,
                source_end,
            } => {
                container.append(math.widget());
                segments.push(ReadingSegment::DisplayMath {
                    math,
                    source_start,
                    source_end,
                });
            }
            AssembledSegment::Embed {
                embed,
                source_start,
                source_end,
            } => {
                container.append(embed.widget());
                segments.push(ReadingSegment::Embed {
                    embed,
                    source_start,
                    source_end,
                });
            }
            AssembledSegment::Hr {
                source_start,
                source_end,
            } => {
                let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
                sep.add_css_class("hortus-reading-hr");
                sep.set_hexpand(true);
                container.append(&sep);
                segments.push(ReadingSegment::Hr {
                    separator: sep,
                    source_start,
                    source_end,
                });
            }
        }
    }
    segments
}

fn install_text_segment(
    data: TextSegmentData,
    env: &ReadingEnv,
    embedded: bool,
) -> (gtk4::Widget, TextSegment) {
    let tag_table = create_tag_table();
    let tag_set = MaskTagSet::new(&tag_table);
    let buffer = gtk4::TextBuffer::new(Some(&tag_table));
    buffer.set_text(&data.text);
    let spans = Rc::new(data.link_spans);

    for (cs, ce, mask) in data.runs {
        if cs < ce {
            apply_mask_tags_cached(
                &buffer,
                &tag_set,
                &buffer.iter_at_offset(cs),
                &buffer.iter_at_offset(ce),
                mask,
            );
        }
    }

    if let Some(blank_tag) = tag_table.lookup("blank_line") {
        let codeblock_tag = tag_table.lookup("codeblock");
        let mut cursor = buffer.start_iter();
        for segment in data.text.split_inclusive('\n') {
            let seg_chars = segment.chars().count() as i32;
            let mut next = cursor;
            next.forward_chars(seg_chars);
            if segment.ends_with('\n') && segment.trim().is_empty() {
                let in_codeblock = codeblock_tag.as_ref().is_some_and(|ct| cursor.has_tag(ct));
                if !in_codeblock {
                    buffer.apply_tag(&blank_tag, &cursor, &next);
                }
            }
            cursor = next;
        }
    }

    let text_view = gtk4::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .left_margin(if embedded { 4 } else { 0 })
        .right_margin(if embedded { 4 } else { 0 })
        .top_margin(if embedded { 2 } else { 0 })
        .bottom_margin(if embedded { 2 } else { 0 })
        .pixels_above_lines(0)
        .pixels_below_lines(2)
        .pixels_inside_wrap(1)
        .vexpand(false)
        .hexpand(true)
        .build();
    text_view.add_css_class(if embedded {
        "hortus-embedded-reading-view"
    } else {
        "hortus-reading-view"
    });

    // 按 0.5px 精度在当前分段 tag_table 内缓存去重 rise 标签，避免每个行内公式新建匿名 TextTag。
    let mut rise_tags: HashMap<i32, gtk4::TextTag> = HashMap::new();
    let rise_step = (pango::SCALE / 2).max(1);
    for anchor_item in data.inline_anchors.iter().rev() {
        let off = anchor_item.local_char_pos as i32;
        if off < 0 || off >= buffer.char_count() {
            continue;
        }
        let mut s_it = buffer.iter_at_offset(off);
        let mut e_it = buffer.iter_at_offset(off + 1);
        buffer.delete(&mut s_it, &mut e_it);
        let anchor = buffer.create_child_anchor(&mut s_it);
        text_view.add_child_at_anchor(&anchor_item.widget, &anchor);
        if anchor_item.rise_pango != 0 {
            let q_rise = ((f64::from(anchor_item.rise_pango) / f64::from(rise_step)).round()
                as i32)
                * rise_step;
            if q_rise != 0 {
                let tag = rise_tags.entry(q_rise).or_insert_with(|| {
                    let t = gtk4::TextTag::builder().rise(q_rise).build();
                    tag_table.add(&t);
                    t
                });
                let s_anchor = buffer.iter_at_child_anchor(&anchor);
                let mut e_anchor = s_anchor;
                e_anchor.forward_char();
                buffer.apply_tag(tag, &s_anchor, &e_anchor);
            }
        }
    }

    let mut link_dispatcher = None;
    if !spans.is_empty() {
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
            src_note: Rc::clone(&env.shared_note_path),
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

    let seg = TextSegment {
        view: text_view.clone(),
        buffer,
        source_anchors: data.source_anchors,
        source_start: data.source_start,
        source_end: data.source_end,
        link_spans: spans,
        link_dispatcher,
    };
    (text_view.upcast(), seg)
}
