// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! Mermaid 图表专注查看模式：Weak 解环（C-07 修复）与后台持久化（`gio::spawn_blocking`）。

use crate::render::diagram::{
    DiagramExportKind, DiagramExportMeta, DiagramHighlight, draw_scene, hit_test,
    render_block_with_engine,
};
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, DiagramIdentity, DiagramPrevious, SceneColor, SceneData, SceneHit, WorkspaceCore,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const MIN_SCALE: f64 = 0.25;
const MAX_SCALE: f64 = 4.0;

#[cfg(feature = "automation")]
#[derive(Clone, Debug, serde::Serialize)]
pub struct FocusSnapshot {
    pub note: String,
    pub ordinal: u32,
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub scene_width: f64,
    pub scene_height: f64,
    pub collapsed: Vec<String>,
}

pub struct FocusRequest {
    pub vault_root: String,
    pub note_path: String,
    pub ordinal: u32,
    pub source: String,
    pub mmd: bool,
    pub collapsed: Vec<String>,
    pub scene: SceneData,
    pub core: Arc<WorkspaceCore>,
    pub engine: Arc<DiagramEngine>,
}

#[derive(Clone)]
pub struct FocusCallbacks {
    pub on_collapse_changed: Rc<dyn Fn(u32, Vec<String>, SceneData)>,
    pub on_open_link: Rc<dyn Fn(String, Option<String>)>,
    pub on_export: Rc<dyn Fn(DiagramExportKind, DiagramExportMeta)>,
    pub on_exit: Rc<dyn Fn()>,
}

struct FocusState {
    vault_root: String,
    note_path: String,
    ordinal: u32,
    source: String,
    mmd: bool,
    collapsed: Vec<String>,
    scene: SceneData,
    previous: Option<DiagramPrevious>,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
    fit_on_resize: bool,
    drag_origin: Option<(f64, f64)>,
    hovered: Option<SceneHit>,
    core: Arc<WorkspaceCore>,
    engine: Arc<DiagramEngine>,
    callbacks: FocusCallbacks,
    persist_source: Option<glib::SourceId>,
    rerender_gen: u64,
    open_gen: u64,
    user_interacted: bool,
}

struct FocusInner {
    root: gtk4::Box,
    drawing: gtk4::DrawingArea,
    percent_label: gtk4::Label,
    open_gen_counter: Cell<u64>,
    session: RefCell<Option<FocusState>>,
}

impl Drop for FocusInner {
    fn drop(&mut self) {
        if let Some(mut st) = self.session.borrow_mut().take()
            && let Some(src) = st.persist_source.take()
        {
            src.remove();
        }
    }
}

/// 专注模式容器控制器（嵌入主窗口顶层 Overlay，支持截图采集与快捷键）。
#[derive(Clone)]
pub struct FocusController {
    inner: Rc<FocusInner>,
}

impl Default for FocusController {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusController {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.add_css_class("hortus-focus-overlay");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_visible(false);
        root.set_can_focus(true);

        let bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        bar.add_css_class("hortus-focus-bar");

        let title_label = gtk4::Label::new(Some(crate::i18n::tr(crate::i18n::Key::FocusTitle)));
        title_label.add_css_class("heading");
        title_label.set_margin_start(6);

        let percent_label = gtk4::Label::new(Some("100%"));
        percent_label.add_css_class("numeric");
        percent_label.add_css_class("dim-label");
        percent_label.set_width_chars(5);

        let zoom_out_btn = gtk4::Button::builder()
            .icon_name("zoom-out-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::FocusZoomOutTooltip))
            .build();
        zoom_out_btn.add_css_class("flat");

        let zoom_in_btn = gtk4::Button::builder()
            .icon_name("zoom-in-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::FocusZoomInTooltip))
            .build();
        zoom_in_btn.add_css_class("flat");

        let fit_btn = gtk4::Button::with_label(crate::i18n::tr(crate::i18n::Key::FocusFitBtn));
        fit_btn.set_tooltip_text(Some(crate::i18n::tr(crate::i18n::Key::FocusFitTooltip)));
        fit_btn.add_css_class("flat");

        let actual_btn = gtk4::Button::with_label("100%");
        actual_btn.set_tooltip_text(Some(crate::i18n::tr(crate::i18n::Key::FocusActualTooltip)));
        actual_btn.add_css_class("flat");

        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        let export_popover = gtk4::Popover::new();
        let export_menu = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        export_menu.set_margin_top(4);
        export_menu.set_margin_bottom(4);
        export_menu.set_margin_start(4);
        export_menu.set_margin_end(4);
        export_popover.set_child(Some(&export_menu));

        let export_btn = gtk4::MenuButton::builder()
            .label(crate::i18n::tr(crate::i18n::Key::FocusExportBtn))
            .popover(&export_popover)
            .build();
        export_btn.add_css_class("flat");

        let close_btn = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::FocusExitTooltip))
            .build();
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");

        bar.append(&title_label);
        bar.append(&percent_label);
        bar.append(&zoom_out_btn);
        bar.append(&zoom_in_btn);
        bar.append(&fit_btn);
        bar.append(&actual_btn);
        bar.append(&spacer);
        bar.append(&export_btn);
        bar.append(&close_btn);

        let drawing = gtk4::DrawingArea::new();
        drawing.set_hexpand(true);
        drawing.set_vexpand(true);
        drawing.set_focusable(true);

        root.append(&bar);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&drawing);

        let inner = Rc::new(FocusInner {
            root,
            drawing,
            percent_label,
            open_gen_counter: Cell::new(0),
            session: RefCell::new(None),
        });
        Self::bind_controllers(
            &inner,
            &export_popover,
            &export_menu,
            &zoom_in_btn,
            &zoom_out_btn,
            &fit_btn,
            &actual_btn,
            &close_btn,
        );
        Self { inner }
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_controllers(
        inner: &Rc<FocusInner>,
        export_popover: &gtk4::Popover,
        export_menu: &gtk4::Box,
        zoom_in_btn: &gtk4::Button,
        zoom_out_btn: &gtk4::Button,
        fit_btn: &gtk4::Button,
        actual_btn: &gtk4::Button,
        close_btn: &gtk4::Button,
    ) {
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
            let item_btn = gtk4::Button::with_label(label);
            item_btn.add_css_class("flat");
            let weak = Rc::downgrade(inner);
            let pop_weak = export_popover.downgrade();
            item_btn.connect_clicked(move |_| {
                if let Some(pop) = pop_weak.upgrade() {
                    pop.popdown();
                }
                if let Some(inner) = weak.upgrade() {
                    Self::trigger_export(&inner, kind);
                }
            });
            export_menu.append(&item_btn);
        }

        {
            let weak = Rc::downgrade(inner);
            zoom_in_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::zoom_factor_inner(&inner, 1.2);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            zoom_out_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::zoom_factor_inner(&inner, 1.0 / 1.2);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            fit_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::fit_to_view_inner(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            actual_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::actual_size_inner(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            close_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::close_inner(&inner);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.drawing.set_draw_func(move |area, cr, w, h| {
                let Some(inner) = weak.upgrade() else { return };
                let guard = inner.session.borrow();
                let Some(st) = guard.as_ref() else { return };

                let _ = area.color();
                let is_dark = adw::StyleManager::default().is_dark();
                if is_dark {
                    cr.set_source_rgba(0.11, 0.12, 0.14, 1.0);
                } else {
                    cr.set_source_rgba(0.98, 0.98, 0.99, 1.0);
                }
                cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
                let _ = cr.fill();

                let _ = cr.save();
                cr.translate(st.offset_x, st.offset_y);
                cr.scale(st.scale, st.scale);

                let hl = st.hovered.as_ref().and_then(|hit| {
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
                draw_scene(cr, &st.scene, None, hl.as_ref());
                let _ = cr.restore();
            });
        }

        let drag = gtk4::GestureDrag::new();
        drag.set_button(1);
        let did_drag = Rc::new(Cell::new(false));
        {
            let weak = Rc::downgrade(inner);
            let dd = did_drag.clone();
            drag.connect_drag_begin(move |_, _, _| {
                dd.set(false);
                let Some(inner) = weak.upgrade() else { return };
                if let Some(st) = inner.session.borrow_mut().as_mut() {
                    st.drag_origin = Some((st.offset_x, st.offset_y));
                    st.user_interacted = true;
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            let dd = did_drag.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let Some(inner) = weak.upgrade() else { return };
                if dx.hypot(dy) > 3.0 {
                    dd.set(true);
                }
                let changed = {
                    let mut guard = inner.session.borrow_mut();
                    if let Some(st) = guard.as_mut()
                        && let Some((ox, oy)) = st.drag_origin
                        && dd.get()
                    {
                        st.offset_x = ox + dx;
                        st.offset_y = oy + dy;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    inner.drawing.queue_draw();
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            let dd = did_drag.clone();
            drag.connect_drag_end(move |_, _, _| {
                let Some(inner) = weak.upgrade() else { return };
                if dd.get() {
                    Self::schedule_persist(&inner);
                }
                if let Some(st) = inner.session.borrow_mut().as_mut() {
                    st.drag_origin = None;
                }
            });
        }
        inner.drawing.add_controller(drag);

        let click = gtk4::GestureClick::new();
        click.set_button(1);
        {
            let weak = Rc::downgrade(inner);
            let dd = did_drag.clone();
            click.connect_released(move |_, _, x, y| {
                if dd.get() {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    Self::handle_canvas_click(&inner, x, y);
                }
            });
        }
        inner.drawing.add_controller(click);

        let motion = gtk4::EventControllerMotion::new();
        {
            let weak = Rc::downgrade(inner);
            motion.connect_motion(move |_, x, y| {
                let Some(inner) = weak.upgrade() else { return };
                let mut redraw = false;
                let mut pointer = false;
                {
                    let mut guard = inner.session.borrow_mut();
                    if let Some(st) = guard.as_mut() {
                        let sx = (x - st.offset_x) / st.scale;
                        let sy = (y - st.offset_y) / st.scale;
                        let hit = hit_test(&st.scene, sx, sy).cloned();
                        pointer = hit
                            .as_ref()
                            .is_some_and(|h| h.can_collapse || h.link_target.is_some());
                        redraw = match (&st.hovered, &hit) {
                            (None, None) => false,
                            (Some(a), Some(b)) => a.node_id != b.node_id,
                            _ => true,
                        };
                        st.hovered = hit;
                    }
                }
                inner
                    .drawing
                    .set_cursor_from_name(if pointer { Some("pointer") } else { None });
                if redraw {
                    inner.drawing.queue_draw();
                }
            });
        }
        inner.drawing.add_controller(motion);

        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
        let cursor_pos = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        {
            let cp = cursor_pos.clone();
            let motion2 = gtk4::EventControllerMotion::new();
            motion2.connect_motion(move |_, x, y| cp.set((x, y)));
            inner.drawing.add_controller(motion2);
        }
        {
            let weak = Rc::downgrade(inner);
            let cp = cursor_pos.clone();
            scroll.connect_scroll(move |ec, dx, dy| {
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let state = ec.current_event_state();
                if state.contains(gdk::ModifierType::CONTROL_MASK) {
                    let (ax, ay) = cp.get();
                    let factor = if dy < 0.0 { 1.12 } else { 1.0 / 1.12 };
                    Self::set_scale_around(&inner, factor, ax, ay);
                } else {
                    {
                        let mut guard = inner.session.borrow_mut();
                        if let Some(st) = guard.as_mut() {
                            st.offset_x -= dx * 28.0;
                            st.offset_y -= dy * 28.0;
                            st.user_interacted = true;
                        }
                    }
                    inner.drawing.queue_draw();
                    Self::schedule_persist(&inner);
                }
                glib::Propagation::Stop
            });
        }
        inner.drawing.add_controller(scroll);

        let key_ctrl = gtk4::EventControllerKey::new();
        {
            let weak = Rc::downgrade(inner);
            key_ctrl.connect_key_pressed(move |_, key, _, mods| {
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let ctrl_down = mods.contains(gdk::ModifierType::CONTROL_MASK);
                match key {
                    gdk::Key::Escape => {
                        Self::close_inner(&inner);
                        glib::Propagation::Stop
                    }
                    gdk::Key::_0 | gdk::Key::KP_0 if ctrl_down => {
                        Self::fit_to_view_inner(&inner);
                        glib::Propagation::Stop
                    }
                    gdk::Key::_1 | gdk::Key::KP_1 if ctrl_down => {
                        Self::actual_size_inner(&inner);
                        glib::Propagation::Stop
                    }
                    gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add if ctrl_down => {
                        Self::zoom_factor_inner(&inner, 1.2);
                        glib::Propagation::Stop
                    }
                    gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract if ctrl_down => {
                        Self::zoom_factor_inner(&inner, 1.0 / 1.2);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        inner.root.add_controller(key_ctrl);

        {
            let weak = Rc::downgrade(inner);
            inner.drawing.connect_resize(move |_, w, h| {
                let Some(inner) = weak.upgrade() else { return };
                if w > 10 && h > 10 {
                    let fitted = {
                        let mut guard = inner.session.borrow_mut();
                        if let Some(st) = guard.as_mut()
                            && st.fit_on_resize
                        {
                            let (s, ox, oy) = compute_fit(&st.scene, f64::from(w), f64::from(h));
                            st.scale = s;
                            st.offset_x = ox;
                            st.offset_y = oy;
                            st.fit_on_resize = false;
                            true
                        } else {
                            false
                        }
                    };
                    if fitted {
                        Self::update_percent_label(&inner);
                        inner.drawing.queue_draw();
                    }
                }
            });
        }
    }

    pub fn root(&self) -> &gtk4::Box {
        &self.inner.root
    }

    pub fn is_open(&self) -> bool {
        self.inner.root.is_visible() && self.inner.session.borrow().is_some()
    }

    pub fn open(&self, req: FocusRequest, callbacks: FocusCallbacks) {
        let previous = req.scene.previous.clone();
        let vw = if self.inner.drawing.width() > 10 {
            f64::from(self.inner.drawing.width())
        } else {
            1240.0
        };
        let vh = if self.inner.drawing.height() > 10 {
            f64::from(self.inner.drawing.height())
        } else {
            760.0
        };
        let fit_on_resize = self.inner.drawing.width() <= 10;
        let (scale, offset_x, offset_y) = compute_fit(&req.scene, vw, vh);

        let core_bg = Arc::clone(&req.core);
        let vault_bg = req.vault_root.clone();
        let note_bg = req.note_path.clone();
        let ordinal_bg = req.ordinal;

        let open_gen = {
            let next_gen = self.inner.open_gen_counter.get().wrapping_add(1).max(1);
            self.inner.open_gen_counter.set(next_gen);
            let mut guard = self.inner.session.borrow_mut();
            if let Some(old) = guard.take()
                && let Some(id) = old.persist_source
            {
                id.remove();
            }
            *guard = Some(FocusState {
                vault_root: req.vault_root,
                note_path: req.note_path,
                ordinal: req.ordinal,
                source: req.source,
                mmd: req.mmd,
                collapsed: req.collapsed,
                scene: req.scene,
                previous,
                scale,
                offset_x,
                offset_y,
                fit_on_resize,
                drag_origin: None,
                hovered: None,
                core: req.core,
                engine: req.engine,
                callbacks,
                persist_source: None,
                rerender_gen: 0,
                open_gen: next_gen,
                user_interacted: false,
            });
            next_gen
        };

        Self::update_percent_label(&self.inner);
        self.inner.root.set_visible(true);
        self.inner.drawing.queue_draw();
        self.inner.drawing.grab_focus();

        // 异步读取历史专注视口参数，不阻塞 GTK 主线程。
        let weak = Rc::downgrade(&self.inner);
        glib::MainContext::default().spawn_local(async move {
            let saved = gio::spawn_blocking(move || {
                core_bg
                    .diagram_focus(vault_bg, note_bg, ordinal_bg)
                    .ok()
                    .flatten()
            })
            .await
            .ok()
            .flatten();
            let Some(saved) = saved else { return };
            let Some(inner) = weak.upgrade() else { return };
            let cur_vw = if inner.drawing.width() > 10 {
                f64::from(inner.drawing.width())
            } else {
                vw
            };
            let cur_vh = if inner.drawing.height() > 10 {
                f64::from(inner.drawing.height())
            } else {
                vh
            };
            let applied = {
                let mut guard = inner.session.borrow_mut();
                if let Some(st) = guard.as_mut()
                    && st.open_gen == open_gen
                    && !st.user_interacted
                {
                    let (scale, ox, oy) = clamp_or_fit_restored_viewport(
                        &st.scene,
                        cur_vw,
                        cur_vh,
                        saved.scale,
                        saved.offset_x,
                        saved.offset_y,
                    );
                    st.scale = scale;
                    st.offset_x = ox;
                    st.offset_y = oy;
                    st.fit_on_resize = false;
                    true
                } else {
                    false
                }
            };
            if applied {
                Self::update_percent_label(&inner);
                inner.drawing.queue_draw();
            }
        });
    }

    pub fn refresh_theme(&self) {
        let params = {
            let mut guard = self.inner.session.borrow_mut();
            let Some(st) = guard.as_mut() else {
                return;
            };
            if !self.inner.root.is_visible() {
                return;
            }
            st.rerender_gen = st.rerender_gen.wrapping_add(1);
            (
                st.note_path.clone(),
                st.ordinal,
                st.source.clone(),
                st.mmd,
                st.collapsed.clone(),
                st.previous.clone(),
                Arc::clone(&st.engine),
                st.rerender_gen,
                st.scene.width.max(700.0),
            )
        };
        let (note_path, ordinal, source, mmd, collapsed, previous, engine, gen_token, render_w) =
            params;
        let dark = adw::StyleManager::default().is_dark();
        let weak = Rc::downgrade(&self.inner);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || {
                render_block_with_engine(
                    &engine,
                    &source,
                    mmd,
                    render_w,
                    &collapsed,
                    DiagramIdentity {
                        document: note_path,
                        block: ordinal,
                        revision: 1,
                        dark,
                        font_generation: 0,
                        request_generation: gen_token,
                    },
                    previous,
                )
            })
            .await;
            let Some(inner) = weak.upgrade() else { return };
            if let Ok(Ok(new_scene)) = res {
                let updated = {
                    let mut guard = inner.session.borrow_mut();
                    if let Some(st) = guard.as_mut()
                        && st.rerender_gen == gen_token
                    {
                        st.previous = new_scene.previous.clone();
                        st.scene = new_scene;
                        true
                    } else {
                        false
                    }
                };
                if updated {
                    inner.drawing.queue_draw();
                }
            }
        });
    }

    pub fn close(&self) {
        Self::close_inner(&self.inner);
    }

    pub fn teardown(&self) {
        if let Some(mut st) = self.inner.session.borrow_mut().take()
            && let Some(src) = st.persist_source.take()
        {
            src.remove();
        }
        self.inner.root.set_visible(false);
    }

    #[cfg(feature = "automation")]
    pub fn zoom_factor(&self, factor: f64) {
        Self::zoom_factor_inner(&self.inner, factor);
    }

    #[cfg(feature = "automation")]
    pub fn pan_by(&self, dx: f64, dy: f64) {
        {
            let mut guard = self.inner.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                st.offset_x += dx;
                st.offset_y += dy;
                st.user_interacted = true;
            }
        }
        self.inner.drawing.queue_draw();
    }

    #[cfg(feature = "automation")]
    pub async fn persist_now_async(&self) {
        let Some(p) = Self::take_persist_params(&self.inner) else {
            return;
        };
        let _ = gio::spawn_blocking(move || {
            let _ = p.core.remember_diagram_focus(
                p.vault, p.note, p.ordinal, p.scale, p.offset_x, p.offset_y,
            );
        })
        .await;
    }

    #[cfg(feature = "automation")]
    pub fn snapshot(&self) -> Option<FocusSnapshot> {
        self.inner
            .session
            .borrow()
            .as_ref()
            .map(|st| FocusSnapshot {
                note: st.note_path.clone(),
                ordinal: st.ordinal,
                scale: st.scale,
                offset_x: st.offset_x,
                offset_y: st.offset_y,
                scene_width: st.scene.width,
                scene_height: st.scene.height,
                collapsed: st.collapsed.clone(),
            })
    }

    fn close_inner(inner: &Rc<FocusInner>) {
        Self::persist_now_bg(inner);
        let exit_cb = {
            let mut guard = inner.session.borrow_mut();
            if let Some(mut st) = guard.take() {
                if let Some(src) = st.persist_source.take() {
                    src.remove();
                }
                Some(st.callbacks.on_exit.clone())
            } else {
                None
            }
        };
        inner.root.set_visible(false);
        if let Some(cb) = exit_cb {
            (cb)();
        }
    }

    fn fit_to_view_inner(inner: &Rc<FocusInner>) {
        let vw = f64::from(inner.drawing.width()).max(640.0);
        let vh = f64::from(inner.drawing.height()).max(420.0);
        {
            let mut guard = inner.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                let (s, ox, oy) = compute_fit(&st.scene, vw, vh);
                st.scale = s;
                st.offset_x = ox;
                st.offset_y = oy;
                st.user_interacted = true;
            }
        }
        Self::update_percent_label(inner);
        inner.drawing.queue_draw();
        Self::schedule_persist(inner);
    }

    fn actual_size_inner(inner: &Rc<FocusInner>) {
        let vw = f64::from(inner.drawing.width()).max(640.0);
        let vh = f64::from(inner.drawing.height()).max(420.0);
        {
            let mut guard = inner.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                st.scale = 1.0;
                st.offset_x = (vw - st.scene.width) / 2.0;
                st.offset_y = (vh - st.scene.height) / 2.0;
                st.user_interacted = true;
            }
        }
        Self::update_percent_label(inner);
        inner.drawing.queue_draw();
        Self::schedule_persist(inner);
    }

    fn zoom_factor_inner(inner: &Rc<FocusInner>, factor: f64) {
        let vw = f64::from(inner.drawing.width()).max(640.0);
        let vh = f64::from(inner.drawing.height()).max(420.0);
        Self::set_scale_around(inner, factor, vw / 2.0, vh / 2.0);
    }

    fn set_scale_around(inner: &Rc<FocusInner>, factor: f64, anchor_x: f64, anchor_y: f64) {
        {
            let mut guard = inner.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                let next_scale = (st.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
                let scene_x = (anchor_x - st.offset_x) / st.scale;
                let scene_y = (anchor_y - st.offset_y) / st.scale;
                st.scale = next_scale;
                st.offset_x = anchor_x - scene_x * next_scale;
                st.offset_y = anchor_y - scene_y * next_scale;
                st.user_interacted = true;
            }
        }
        Self::update_percent_label(inner);
        inner.drawing.queue_draw();
        Self::schedule_persist(inner);
    }

    fn update_percent_label(inner: &Rc<FocusInner>) {
        let pct = inner
            .session
            .borrow()
            .as_ref()
            .map(|st| (st.scale * 100.0).round() as i32);
        if let Some(pct) = pct {
            inner.percent_label.set_text(&format!("{pct}%"));
        }
    }

    fn trigger_export(inner: &Rc<FocusInner>, kind: DiagramExportKind) {
        let meta_and_cb = inner.session.borrow().as_ref().map(|st| {
            (
                DiagramExportMeta {
                    note_path: st.note_path.clone(),
                    ordinal: st.ordinal,
                    scene: st.scene.clone(),
                },
                st.callbacks.on_export.clone(),
            )
        });
        if let Some((meta, cb)) = meta_and_cb {
            (cb)(kind, meta);
        }
    }

    fn handle_canvas_click(inner: &Rc<FocusInner>, x: f64, y: f64) {
        enum ClickAction {
            Collapse {
                note_path: String,
                ordinal: u32,
                source: String,
                mmd: bool,
                collapsed: Vec<String>,
                previous: Option<DiagramPrevious>,
                engine: Arc<DiagramEngine>,
                gen_token: u64,
                render_w: f64,
            },
            Link {
                target: String,
                heading: Option<String>,
                cb: Rc<dyn Fn(String, Option<String>)>,
            },
        }

        let action = {
            let mut guard = inner.session.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            let sx = (x - st.offset_x) / st.scale;
            let sy = (y - st.offset_y) / st.scale;
            let Some(hit) = hit_test(&st.scene, sx, sy).cloned() else {
                return;
            };
            if hit.can_collapse {
                if let Some(pos) = st.collapsed.iter().position(|id| id == &hit.node_id) {
                    st.collapsed.remove(pos);
                } else {
                    st.collapsed.push(hit.node_id.clone());
                    st.collapsed.sort();
                }
                st.rerender_gen = st.rerender_gen.wrapping_add(1);
                Some(ClickAction::Collapse {
                    note_path: st.note_path.clone(),
                    ordinal: st.ordinal,
                    source: st.source.clone(),
                    mmd: st.mmd,
                    collapsed: st.collapsed.clone(),
                    previous: st.previous.clone(),
                    engine: Arc::clone(&st.engine),
                    gen_token: st.rerender_gen,
                    render_w: st.scene.width.max(700.0),
                })
            } else if let Some(target) = hit.link_target {
                Some(ClickAction::Link {
                    target,
                    heading: hit.link_heading,
                    cb: st.callbacks.on_open_link.clone(),
                })
            } else {
                None
            }
        };

        match action {
            Some(ClickAction::Link {
                target,
                heading,
                cb,
            }) => {
                Self::close_inner(inner);
                (cb)(target, heading);
            }
            Some(ClickAction::Collapse {
                note_path,
                ordinal,
                source,
                mmd,
                collapsed,
                previous,
                engine,
                gen_token,
                render_w,
            }) => {
                let dark = adw::StyleManager::default().is_dark();
                let weak = Rc::downgrade(inner);
                glib::MainContext::default().spawn_local(async move {
                    let collapsed_bg = collapsed.clone();
                    let note_bg = note_path.clone();
                    let res = gio::spawn_blocking(move || {
                        render_block_with_engine(
                            &engine,
                            &source,
                            mmd,
                            render_w,
                            &collapsed_bg,
                            DiagramIdentity {
                                document: note_bg,
                                block: ordinal,
                                revision: 1,
                                dark,
                                font_generation: 0,
                                request_generation: gen_token,
                            },
                            previous,
                        )
                    })
                    .await;

                    let Some(inner) = weak.upgrade() else { return };
                    if let Ok(Ok(new_scene)) = res {
                        let cb_opt = {
                            let mut guard = inner.session.borrow_mut();
                            if let Some(st) = guard.as_mut()
                                && st.rerender_gen == gen_token
                            {
                                st.previous = new_scene.previous.clone();
                                st.scene = new_scene.clone();
                                Some(st.callbacks.on_collapse_changed.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(cb) = cb_opt {
                            inner.drawing.queue_draw();
                            (cb)(ordinal, collapsed, new_scene);
                        }
                    }
                });
            }
            None => {}
        }
    }

    fn schedule_persist(inner: &Rc<FocusInner>) {
        let mut guard = inner.session.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        if let Some(old) = st.persist_source.take() {
            old.remove();
        }
        let weak = Rc::downgrade(inner);
        st.persist_source = Some(glib::timeout_add_local_once(
            Duration::from_millis(180),
            move || {
                if let Some(inner) = weak.upgrade() {
                    Self::persist_now_bg(&inner);
                }
            },
        ));
    }

    fn take_persist_params(inner: &Rc<FocusInner>) -> Option<PersistParams> {
        let mut guard = inner.session.borrow_mut();
        let st = guard.as_mut()?;
        if let Some(src) = st.persist_source.take() {
            src.remove();
        }
        Some(PersistParams {
            core: Arc::clone(&st.core),
            vault: st.vault_root.clone(),
            note: st.note_path.clone(),
            ordinal: st.ordinal,
            scale: st.scale,
            offset_x: st.offset_x,
            offset_y: st.offset_y,
        })
    }

    fn persist_now_bg(inner: &Rc<FocusInner>) {
        let Some(p) = Self::take_persist_params(inner) else {
            return;
        };
        gio::spawn_blocking(move || {
            let _ = p.core.remember_diagram_focus(
                p.vault, p.note, p.ordinal, p.scale, p.offset_x, p.offset_y,
            );
        });
    }
}

struct PersistParams {
    core: Arc<WorkspaceCore>,
    vault: String,
    note: String,
    ordinal: u32,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
}

fn clamp_or_fit_restored_viewport(
    scene: &SceneData,
    vw: f64,
    vh: f64,
    saved_scale: f64,
    saved_ox: f64,
    saved_oy: f64,
) -> (f64, f64, f64) {
    let scale = if saved_scale.is_finite() {
        saved_scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        return compute_fit(scene, vw, vh);
    };
    if !saved_ox.is_finite() || !saved_oy.is_finite() {
        return compute_fit(scene, vw, vh);
    }
    let sw = scene.width.max(1.0) * scale;
    let sh = scene.height.max(1.0) * scale;
    let margin = 40.0_f64.min(vw * 0.25).min(vh * 0.25);
    let visible_x = saved_ox + sw > margin && saved_ox < vw - margin;
    let visible_y = saved_oy + sh > margin && saved_oy < vh - margin;
    if visible_x && visible_y {
        (scale, saved_ox, saved_oy)
    } else {
        compute_fit(scene, vw, vh)
    }
}

fn compute_fit(scene: &SceneData, vw: f64, vh: f64) -> (f64, f64, f64) {
    let pad = 36.0;
    let avail_w = (vw - pad * 2.0).max(80.0);
    let avail_h = (vh - pad * 2.0).max(80.0);
    let scale = (avail_w / scene.width.max(1.0))
        .min(avail_h / scene.height.max(1.0))
        .clamp(MIN_SCALE, 1.5);
    let ox = (vw - scene.width * scale) / 2.0;
    let oy = (vh - scene.height * scale) / 2.0;
    (scale, ox, oy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_controller_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let focus = FocusController::new();
        let weak = Rc::downgrade(&focus.inner);
        assert_eq!(weak.strong_count(), 1);
        focus.teardown();
        drop(focus);
        assert_eq!(weak.strong_count(), 0);
    }
}
