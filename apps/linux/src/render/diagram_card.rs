// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 内联 Mermaid 图表卡片控件（弱引用回调解环，支持折叠、双链、专注模式与导出）。

use crate::render::diagram::{
    DiagramExportKind, DiagramExportMeta, DiagramHighlight, RenderedDiagram, draw_scene, hit_test,
    render_block_with_engine,
};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, DiagramIdentity, DiagramPrevious, SceneColor, SceneData, SceneHit,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// 内联图表交互回调函数集。
#[derive(Clone)]
pub struct DiagramCallbacks {
    pub on_collapse_changed: Rc<dyn Fn(u32, Vec<String>, SceneData)>,
    pub on_scene_updated: Rc<dyn Fn(u32, SceneData)>,
    pub on_open_link: Rc<dyn Fn(String, Option<String>)>,
    pub on_open_source: Rc<dyn Fn(u64, u64)>,
    pub on_focus: Rc<dyn Fn(u32)>,
    pub on_export: Rc<dyn Fn(DiagramExportKind, DiagramExportMeta)>,
}

struct DiagramCardState {
    note_path: String,
    source: Arc<str>,
    mmd: bool,
    ordinal: u32,
    scene: RefCell<SceneData>,
    collapsed: RefCell<Vec<String>>,
    previous: RefCell<Option<DiagramPrevious>>,
    hovered: RefCell<Option<SceneHit>>,
    last_width: Cell<f64>,
    rerender_gen: Cell<u64>,
    pending_relayouts: Cell<u32>,
    drawing: gtk4::DrawingArea,
    engine: Arc<DiagramEngine>,
    callbacks: DiagramCallbacks,
}

/// `.hortus-diagram-card` CSS 水平 chrome（`4px padding + 1px border` × 2 = `10px`）。
pub const DIAGRAM_CARD_HORIZONTAL_CHROME: f64 = 10.0;

/// 计算传入图引擎的内容宽度：扣除外框 padding/border，避免多余 10px 横向滚动条。
pub fn diagram_render_width(avail_w: f64) -> f64 {
    (avail_w - DIAGRAM_CARD_HORIZONTAL_CHROME).max(240.0)
}

/// 原生块级 Mermaid 图表卡片句柄，支持原地宽度重排而不销毁重建控件。
#[derive(Clone)]
pub struct DiagramCard {
    root: gtk4::Widget,
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    canvas: Option<gtk4::ScrolledWindow>,
    state: Option<Rc<DiagramCardState>>,
}

impl DiagramCard {
    pub fn widget(&self) -> &gtk4::Widget {
        &self.root
    }

    #[cfg(feature = "automation")]
    pub fn is_relayout_pending(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|st| st.pending_relayouts.get() > 0)
    }

    #[cfg(feature = "automation")]
    pub fn has_horizontal_overflow(&self) -> bool {
        self.canvas.as_ref().is_some_and(|scroll| {
            let adj = scroll.hadjustment();
            adj.upper() > adj.page_size() + 1.0
        })
    }

    pub fn relayout_to_width(&self, avail_w: f64) {
        let Some(ref st) = self.state else {
            return;
        };
        let target_w = diagram_render_width(avail_w);
        if (target_w - st.last_width.get()).abs() < 4.0 {
            return;
        }
        st.last_width.set(target_w);
        let token = st.rerender_gen.get().wrapping_add(1);
        st.rerender_gen.set(token);
        st.pending_relayouts
            .set(st.pending_relayouts.get().saturating_add(1));

        let prev = st.previous.borrow().clone();
        let collapsed_bg = st.collapsed.borrow().clone();
        let dark = adw::StyleManager::default().is_dark();
        let engine_bg = Arc::clone(&st.engine);
        let src_bg = st.source.clone();
        let note_bg = st.note_path.clone();
        let mmd = st.mmd;
        let ordinal = st.ordinal;
        let weak_done = Rc::downgrade(st);

        glib::MainContext::default().spawn_local(async move {
            let rendered = gio::spawn_blocking(move || {
                render_block_with_engine(
                    &engine_bg,
                    &src_bg,
                    mmd,
                    target_w,
                    &collapsed_bg,
                    DiagramIdentity {
                        document: note_bg,
                        block: ordinal,
                        revision: 1,
                        dark,
                        font_generation: 0,
                        request_generation: token,
                    },
                    prev,
                )
            })
            .await;

            let Some(st) = weak_done.upgrade() else {
                return;
            };
            st.pending_relayouts
                .set(st.pending_relayouts.get().saturating_sub(1));
            if st.rerender_gen.get() != token {
                return;
            }
            if let Ok(Ok(new_scene)) = rendered {
                *st.previous.borrow_mut() = new_scene.previous.clone();
                st.drawing
                    .set_content_width(new_scene.width.ceil().max(120.0) as i32);
                st.drawing
                    .set_content_height(new_scene.height.ceil().max(48.0) as i32);
                *st.scene.borrow_mut() = new_scene.clone();
                st.drawing.queue_draw();
                (st.callbacks.on_scene_updated)(ordinal, new_scene);
            }
        });
    }
}

/// 构造内联 Mermaid 图表控件（支持节点折叠、双链跳转、专注模式、SVG/PNG/PDF 导出及降级源码卡片）。
pub fn build_diagram_card(
    note_path: String,
    source: Arc<str>,
    mmd: bool,
    initial: RenderedDiagram,
    engine: Arc<DiagramEngine>,
    callbacks: DiagramCallbacks,
    content_width: f64,
) -> DiagramCard {
    if let Some(reason) = initial.failure {
        return DiagramCard {
            root: build_diagram_fallback_card(
                &source,
                initial.source_start,
                initial.source_end,
                &reason,
                content_width,
            ),
            canvas: None,
            state: None,
        };
    }
    let Some(scene) = initial.scene else {
        return DiagramCard {
            root: build_diagram_fallback_card(
                &source,
                initial.source_start,
                initial.source_end,
                crate::i18n::tr(crate::i18n::Key::DiagramEmptyScene),
                content_width,
            ),
            canvas: None,
            state: None,
        };
    };

    let ordinal = initial.ordinal;
    let drawing = gtk4::DrawingArea::new();
    drawing.set_content_width(scene.width.ceil().max(120.0) as i32);
    drawing.set_content_height(scene.height.ceil().max(48.0) as i32);
    drawing.set_hexpand(true);
    drawing.set_halign(gtk4::Align::Fill);

    let state = Rc::new(DiagramCardState {
        note_path,
        source,
        mmd,
        ordinal,
        scene: RefCell::new(scene),
        collapsed: RefCell::new(initial.collapsed),
        previous: RefCell::new(initial.previous),
        hovered: RefCell::new(None),
        last_width: Cell::new(diagram_render_width(content_width)),
        rerender_gen: Cell::new(0),
        pending_relayouts: Cell::new(0),
        drawing: drawing.clone(),
        engine,
        callbacks,
    });

    {
        let weak = Rc::downgrade(&state);
        drawing.set_draw_func(move |_, cr, _, _| {
            let Some(st) = weak.upgrade() else { return };
            let current = st.scene.borrow();
            let hl = st.hovered.borrow().as_ref().and_then(|hit| {
                (hit.can_collapse || hit.link_target.is_some()).then(|| DiagramHighlight {
                    rect: hit.rect.clone(),
                    fill: SceneColor {
                        r: 0.21,
                        g: 0.52,
                        b: 0.89,
                        a: 0.10,
                    },
                    border: SceneColor {
                        r: 0.21,
                        g: 0.52,
                        b: 0.89,
                        a: 0.45,
                    },
                    border_width: 1.0,
                })
            });
            draw_scene(cr, &current, None, hl.as_ref());
        });
    }

    let motion = gtk4::EventControllerMotion::new();
    {
        let weak = Rc::downgrade(&state);
        motion.connect_motion(move |_, x, y| {
            let Some(st) = weak.upgrade() else { return };
            let found = {
                let s = st.scene.borrow();
                hit_test(&s, x, y).cloned()
            };
            let pointer = found
                .as_ref()
                .is_some_and(|h| h.can_collapse || h.link_target.is_some());
            st.drawing
                .set_cursor_from_name(if pointer { Some("pointer") } else { None });
            if let Some(ref hit) = found {
                let tip = if let Some(ref target) = hit.link_target {
                    Some(crate::i18n::fmt1(
                        crate::i18n::Key::DiagramOpenNoteTooltip,
                        target,
                    ))
                } else if hit.can_collapse {
                    Some(
                        if hit.collapsed {
                            crate::i18n::tr(crate::i18n::Key::DiagramExpandTooltip)
                        } else {
                            crate::i18n::tr(crate::i18n::Key::DiagramCollapseTooltip)
                        }
                        .to_string(),
                    )
                } else {
                    None
                };
                st.drawing.set_tooltip_text(tip.as_deref());
            } else {
                st.drawing.set_tooltip_text(None);
            }
            let changed = match (&*st.hovered.borrow(), &found) {
                (None, None) => false,
                (Some(a), Some(b)) => a.node_id != b.node_id,
                _ => true,
            };
            if changed {
                *st.hovered.borrow_mut() = found;
                st.drawing.queue_draw();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        motion.connect_leave(move |_| {
            let Some(st) = weak.upgrade() else { return };
            st.drawing.set_cursor_from_name(None);
            st.drawing.set_tooltip_text(None);
            if st.hovered.borrow_mut().take().is_some() {
                st.drawing.queue_draw();
            }
        });
    }
    drawing.add_controller(motion);

    let click = gtk4::GestureClick::new();
    click.set_button(1);
    {
        let weak = Rc::downgrade(&state);
        click.connect_pressed(move |_, _, x, y| {
            let Some(st) = weak.upgrade() else { return };
            let hit = {
                let s = st.scene.borrow();
                hit_test(&s, x, y).cloned()
            };
            let Some(hit) = hit else { return };
            if hit.can_collapse {
                let next_collapsed = {
                    let mut list = st.collapsed.borrow_mut();
                    if let Some(pos) = list.iter().position(|id| id == &hit.node_id) {
                        list.remove(pos);
                    } else {
                        list.push(hit.node_id.clone());
                        list.sort();
                    }
                    list.clone()
                };
                let token = st.rerender_gen.get().wrapping_add(1);
                st.rerender_gen.set(token);
                let prev = st.previous.borrow().clone();
                let cur_width = st
                    .last_width
                    .get()
                    .max(f64::from(st.drawing.width()))
                    .max(240.0);
                let dark = adw::StyleManager::default().is_dark();
                let engine_bg = Arc::clone(&st.engine);
                let src_bg = st.source.clone();
                let note_bg = st.note_path.clone();
                let mmd = st.mmd;
                let ordinal = st.ordinal;
                let collapsed_bg = next_collapsed.clone();
                let weak_done = Rc::downgrade(&st);

                glib::MainContext::default().spawn_local(async move {
                    let rendered = gio::spawn_blocking(move || {
                        render_block_with_engine(
                            &engine_bg,
                            &src_bg,
                            mmd,
                            cur_width,
                            &collapsed_bg,
                            DiagramIdentity {
                                document: note_bg,
                                block: ordinal,
                                revision: 1,
                                dark,
                                font_generation: 0,
                                request_generation: token,
                            },
                            prev,
                        )
                    })
                    .await;
                    let Some(st) = weak_done.upgrade() else {
                        return;
                    };
                    if st.rerender_gen.get() != token {
                        return;
                    }
                    if let Ok(Ok(new_scene)) = rendered {
                        *st.previous.borrow_mut() = new_scene.previous.clone();
                        st.drawing
                            .set_content_width(new_scene.width.ceil().max(120.0) as i32);
                        st.drawing
                            .set_content_height(new_scene.height.ceil().max(48.0) as i32);
                        *st.scene.borrow_mut() = new_scene.clone();
                        st.drawing.queue_draw();
                        (st.callbacks.on_collapse_changed)(ordinal, next_collapsed, new_scene);
                    }
                });
            } else if let Some(target) = hit.link_target {
                (st.callbacks.on_open_link)(target, hit.link_heading);
            } else if hit.source_end > hit.source_start {
                (st.callbacks.on_open_source)(hit.source_start, hit.source_end);
            }
        });
    }
    drawing.add_controller(click);

    // 悬停/固定微型工具栏：专注查看（Ctrl+Shift+D）与导出 SVG/PNG/PDF
    let tools = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    tools.add_css_class("hortus-diagram-tools");
    tools.set_halign(gtk4::Align::End);
    tools.set_valign(gtk4::Align::Start);
    tools.set_margin_top(6);
    tools.set_margin_end(6);

    let focus_btn = gtk4::Button::builder()
        .icon_name("view-fullscreen-symbolic")
        .tooltip_text(crate::i18n::tr(crate::i18n::Key::DiagramFocusBtnTooltip))
        .build();
    focus_btn.add_css_class("flat");
    focus_btn.add_css_class("circular");
    {
        let weak = Rc::downgrade(&state);
        focus_btn.connect_clicked(move |_| {
            if let Some(st) = weak.upgrade() {
                (st.callbacks.on_focus)(st.ordinal);
            }
        });
    }

    let export_popover = gtk4::Popover::new();
    let export_list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    export_list.set_margin_top(4);
    export_list.set_margin_bottom(4);
    export_list.set_margin_start(4);
    export_list.set_margin_end(4);
    for (label, kind) in [
        (
            crate::i18n::tr(crate::i18n::Key::ExportMenuSvg),
            DiagramExportKind::Svg,
        ),
        (
            crate::i18n::tr(crate::i18n::Key::ExportMenuPng),
            DiagramExportKind::Png,
        ),
        (
            crate::i18n::tr(crate::i18n::Key::ExportMenuPdf),
            DiagramExportKind::Pdf,
        ),
    ] {
        let btn = gtk4::Button::with_label(label);
        btn.add_css_class("flat");
        let weak = Rc::downgrade(&state);
        let pop_weak = export_popover.downgrade();
        btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            let Some(st) = weak.upgrade() else { return };
            let meta = DiagramExportMeta {
                note_path: st.note_path.clone(),
                ordinal: st.ordinal,
                scene: st.scene.borrow().clone(),
            };
            (st.callbacks.on_export)(kind, meta);
        });
        export_list.append(&btn);
    }
    export_popover.set_child(Some(&export_list));

    let export_btn = gtk4::MenuButton::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text(crate::i18n::tr(crate::i18n::Key::DiagramExportBtnTooltip))
        .popover(&export_popover)
        .build();
    export_btn.add_css_class("flat");
    export_btn.add_css_class("circular");

    tools.append(&focus_btn);
    tools.append(&export_btn);

    let canvas = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .min_content_width(120)
        .hexpand(true)
        .halign(gtk4::Align::Fill)
        .child(&drawing)
        .build();
    canvas.add_css_class("hortus-diagram-canvas");
    crate::platform::viewport::install_vertical_scroll_passthrough(&canvas);
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&canvas));
    overlay.add_overlay(&tools);
    overlay.set_hexpand(true);
    overlay.set_halign(gtk4::Align::Fill);
    overlay.add_css_class("hortus-diagram-card");

    DiagramCard {
        root: overlay.upcast(),
        canvas: Some(canvas),
        state: Some(state),
    }
}

fn build_diagram_fallback_card(
    full_source: &str,
    start_byte: u64,
    end_byte: u64,
    reason: &str,
    _content_width: f64,
) -> gtk4::Widget {
    let snippet = full_source
        .get(start_byte as usize..end_byte as usize)
        .unwrap_or(full_source)
        .trim_end();
    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    card.add_css_class("hortus-diagram-fallback");
    card.set_halign(gtk4::Align::Fill);
    card.set_hexpand(true);
    card.set_size_request(120, -1);

    let header = gtk4::Label::new(Some(reason));
    header.set_halign(gtk4::Align::Start);
    header.add_css_class("dim-label");
    header.add_css_class("caption");
    header.set_wrap(true);
    card.append(&header);

    let tv = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .monospace(true)
        .build();
    tv.buffer().set_text(snippet);
    tv.add_css_class("hortus-fallback-code");
    card.append(&tv);
    card.upcast()
}
