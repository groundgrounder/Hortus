// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Mermaid 图表专注查看模式：矢量缩放/平移、适应/100%、折叠与双链跳转、SVG/PNG/PDF 导出及状态持久化。

use crate::diagram_render::{
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

/// 专注模式容器控制器（嵌入主窗口顶层 Overlay，支持截图采集与快捷键）。
#[derive(Clone)]
pub struct FocusController {
    pub root: gtk4::Box,
    drawing: gtk4::DrawingArea,
    percent_label: gtk4::Label,
    session: Rc<RefCell<Option<FocusState>>>,
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

        let title_label = gtk4::Label::new(Some("图表专注模式"));
        title_label.add_css_class("heading");
        title_label.set_margin_start(6);

        let percent_label = gtk4::Label::new(Some("100%"));
        percent_label.add_css_class("numeric");
        percent_label.add_css_class("dim-label");
        percent_label.set_width_chars(5);

        let zoom_out_btn = gtk4::Button::builder()
            .icon_name("zoom-out-symbolic")
            .tooltip_text("缩小 (Ctrl+-)")
            .build();
        zoom_out_btn.add_css_class("flat");

        let zoom_in_btn = gtk4::Button::builder()
            .icon_name("zoom-in-symbolic")
            .tooltip_text("放大 (Ctrl+=)")
            .build();
        zoom_in_btn.add_css_class("flat");

        let fit_btn = gtk4::Button::with_label("适应");
        fit_btn.set_tooltip_text(Some("适应画布 (Ctrl+0)"));
        fit_btn.add_css_class("flat");

        let actual_btn = gtk4::Button::with_label("100%");
        actual_btn.set_tooltip_text(Some("实际大小 100% (Ctrl+1)"));
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
            .label("导出")
            .popover(&export_popover)
            .build();
        export_btn.add_css_class("flat");

        let close_btn = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("退出专注模式 (Esc / Ctrl+Shift+D)")
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

        let session: Rc<RefCell<Option<FocusState>>> = Rc::new(RefCell::new(None));
        let ctrl = Self {
            root,
            drawing,
            percent_label,
            session,
        };

        // 绑定导出按钮项
        for (label, kind) in [
            ("导出 SVG…", DiagramExportKind::Svg),
            ("导出 PNG (2x)…", DiagramExportKind::Png),
            ("导出 PDF…", DiagramExportKind::Pdf),
        ] {
            let item_btn = gtk4::Button::with_label(label);
            item_btn.add_css_class("flat");
            let c = ctrl.clone();
            let pop = export_popover.clone();
            item_btn.connect_clicked(move |_| {
                pop.popdown();
                c.trigger_export(kind);
            });
            export_menu.append(&item_btn);
        }

        // 工具栏按钮事件
        {
            let c = ctrl.clone();
            zoom_in_btn.connect_clicked(move |_| c.zoom_factor(1.2));
        }
        {
            let c = ctrl.clone();
            zoom_out_btn.connect_clicked(move |_| c.zoom_factor(1.0 / 1.2));
        }
        {
            let c = ctrl.clone();
            fit_btn.connect_clicked(move |_| c.fit_to_view());
        }
        {
            let c = ctrl.clone();
            actual_btn.connect_clicked(move |_| c.actual_size());
        }
        {
            let c = ctrl.clone();
            close_btn.connect_clicked(move |_| c.close());
        }

        // 绘制函数：直接以 Cairo CTM 缩放平移，无位图重采样
        {
            let sess = ctrl.session.clone();
            ctrl.drawing.set_draw_func(move |area, cr, w, h| {
                let guard = sess.borrow();
                let Some(st) = guard.as_ref() else { return };

                // 背景填满工作区色调
                let bg = area.color();
                let is_dark = adw::StyleManager::default().is_dark();
                if is_dark {
                    cr.set_source_rgba(0.11, 0.12, 0.14, 1.0);
                } else {
                    cr.set_source_rgba(0.98, 0.98, 0.99, 1.0);
                }
                let _ = (bg, cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h)));
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

        // 左键拖拽平移（画布空白处）与滚动/缩放交互
        let drag = gtk4::GestureDrag::new();
        drag.set_button(1);
        let did_drag = Rc::new(Cell::new(false));
        {
            let sess = ctrl.session.clone();
            let dd = did_drag.clone();
            drag.connect_drag_begin(move |_, _, _| {
                dd.set(false);
                if let Some(st) = sess.borrow_mut().as_mut() {
                    st.drag_origin = Some((st.offset_x, st.offset_y));
                }
            });
        }
        {
            let c = ctrl.clone();
            let dd = did_drag.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                if dx.hypot(dy) > 3.0 {
                    dd.set(true);
                }
                let changed = {
                    let mut guard = c.session.borrow_mut();
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
                    c.drawing.queue_draw();
                }
            });
        }
        {
            let c = ctrl.clone();
            let dd = did_drag.clone();
            drag.connect_drag_end(move |_, _, _| {
                if dd.get() {
                    c.schedule_persist();
                }
                if let Some(st) = c.session.borrow_mut().as_mut() {
                    st.drag_origin = None;
                }
            });
        }
        ctrl.drawing.add_controller(drag);

        // 点击命中：折叠展开节点或打开节点双链
        let click = gtk4::GestureClick::new();
        click.set_button(1);
        {
            let c = ctrl.clone();
            let dd = did_drag.clone();
            click.connect_released(move |_, _, x, y| {
                if dd.get() {
                    return;
                }
                c.handle_canvas_click(x, y);
            });
        }
        ctrl.drawing.add_controller(click);

        // 悬停高亮与指针形状
        let motion = gtk4::EventControllerMotion::new();
        {
            let c = ctrl.clone();
            motion.connect_motion(move |_, x, y| {
                let mut redraw = false;
                let mut pointer = false;
                {
                    let mut guard = c.session.borrow_mut();
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
                c.drawing
                    .set_cursor_from_name(if pointer { Some("pointer") } else { None });
                if redraw {
                    c.drawing.queue_draw();
                }
            });
        }
        ctrl.drawing.add_controller(motion);

        // 滚轮平移 / Ctrl+滚轮光标锚点缩放
        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
        let cursor_pos = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        {
            let cp = cursor_pos.clone();
            let motion2 = gtk4::EventControllerMotion::new();
            motion2.connect_motion(move |_, x, y| cp.set((x, y)));
            ctrl.drawing.add_controller(motion2);
        }
        {
            let c = ctrl.clone();
            let cp = cursor_pos.clone();
            scroll.connect_scroll(move |ec, dx, dy| {
                let state = ec.current_event_state();
                if state.contains(gdk::ModifierType::CONTROL_MASK) {
                    let (ax, ay) = cp.get();
                    let factor = if dy < 0.0 { 1.12 } else { 1.0 / 1.12 };
                    c.set_scale_around(factor, ax, ay);
                } else {
                    {
                        let mut guard = c.session.borrow_mut();
                        if let Some(st) = guard.as_mut() {
                            st.offset_x -= dx * 28.0;
                            st.offset_y -= dy * 28.0;
                        }
                    }
                    c.drawing.queue_draw();
                    c.schedule_persist();
                }
                glib::Propagation::Stop
            });
        }
        ctrl.drawing.add_controller(scroll);

        // 键盘快捷键：Esc / Ctrl+0 / Ctrl+1 / Ctrl+= / Ctrl+-
        let key_ctrl = gtk4::EventControllerKey::new();
        {
            let c = ctrl.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, mods| {
                let ctrl_down = mods.contains(gdk::ModifierType::CONTROL_MASK);
                match key {
                    gdk::Key::Escape => {
                        c.close();
                        glib::Propagation::Stop
                    }
                    gdk::Key::_0 | gdk::Key::KP_0 if ctrl_down => {
                        c.fit_to_view();
                        glib::Propagation::Stop
                    }
                    gdk::Key::_1 | gdk::Key::KP_1 if ctrl_down => {
                        c.actual_size();
                        glib::Propagation::Stop
                    }
                    gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add if ctrl_down => {
                        c.zoom_factor(1.2);
                        glib::Propagation::Stop
                    }
                    gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract if ctrl_down => {
                        c.zoom_factor(1.0 / 1.2);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        ctrl.root.add_controller(key_ctrl);

        // 首次实际分配尺寸后自动以真实视口大小居中适应（若无历史持久状态）
        {
            let c = ctrl.clone();
            ctrl.drawing.connect_resize(move |_, w, h| {
                if w > 10 && h > 10 {
                    let fitted = {
                        let mut guard = c.session.borrow_mut();
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
                        c.update_percent_label();
                        c.drawing.queue_draw();
                    }
                }
            });
        }

        ctrl
    }

    pub fn is_open(&self) -> bool {
        self.root.is_visible() && self.session.borrow().is_some()
    }

    pub fn open(&self, req: FocusRequest, callbacks: FocusCallbacks) {
        let saved = req
            .core
            .diagram_focus(req.vault_root.clone(), req.note_path.clone(), req.ordinal)
            .ok()
            .flatten();

        let previous = req.scene.previous.clone();
        let vw = if self.drawing.width() > 10 {
            f64::from(self.drawing.width())
        } else {
            1240.0
        };
        let vh = if self.drawing.height() > 10 {
            f64::from(self.drawing.height())
        } else {
            760.0
        };
        let fit_on_resize = saved.is_none() && self.drawing.width() <= 10;
        let (scale, offset_x, offset_y) = if let Some(focus) = saved {
            (
                focus.scale.clamp(MIN_SCALE, MAX_SCALE),
                focus.offset_x,
                focus.offset_y,
            )
        } else {
            compute_fit(&req.scene, vw, vh)
        };

        {
            let mut guard = self.session.borrow_mut();
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
            });
        }

        self.update_percent_label();
        self.root.set_visible(true);
        self.drawing.queue_draw();
        self.drawing.grab_focus();
    }

    pub fn close(&self) {
        self.persist_now();
        let exit_cb = {
            let mut guard = self.session.borrow_mut();
            if let Some(mut st) = guard.take() {
                if let Some(src) = st.persist_source.take() {
                    src.remove();
                }
                Some(st.callbacks.on_exit.clone())
            } else {
                None
            }
        };
        self.root.set_visible(false);
        if let Some(cb) = exit_cb {
            (cb)();
        }
    }

    pub fn fit_to_view(&self) {
        let vw = f64::from(self.drawing.width()).max(640.0);
        let vh = f64::from(self.drawing.height()).max(420.0);
        {
            let mut guard = self.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                let (s, ox, oy) = compute_fit(&st.scene, vw, vh);
                st.scale = s;
                st.offset_x = ox;
                st.offset_y = oy;
            }
        }
        self.update_percent_label();
        self.drawing.queue_draw();
        self.schedule_persist();
    }

    pub fn actual_size(&self) {
        let vw = f64::from(self.drawing.width()).max(640.0);
        let vh = f64::from(self.drawing.height()).max(420.0);
        {
            let mut guard = self.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                st.scale = 1.0;
                st.offset_x = (vw - st.scene.width) / 2.0;
                st.offset_y = (vh - st.scene.height) / 2.0;
            }
        }
        self.update_percent_label();
        self.drawing.queue_draw();
        self.schedule_persist();
    }

    pub fn zoom_factor(&self, factor: f64) {
        let vw = f64::from(self.drawing.width()).max(640.0);
        let vh = f64::from(self.drawing.height()).max(420.0);
        self.set_scale_around(factor, vw / 2.0, vh / 2.0);
    }

    pub fn pan_by(&self, dx: f64, dy: f64) {
        {
            let mut guard = self.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                st.offset_x += dx;
                st.offset_y += dy;
            }
        }
        self.drawing.queue_draw();
        self.persist_now();
    }

    fn set_scale_around(&self, factor: f64, anchor_x: f64, anchor_y: f64) {
        {
            let mut guard = self.session.borrow_mut();
            if let Some(st) = guard.as_mut() {
                let next_scale = (st.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
                let scene_x = (anchor_x - st.offset_x) / st.scale;
                let scene_y = (anchor_y - st.offset_y) / st.scale;
                st.scale = next_scale;
                st.offset_x = anchor_x - scene_x * next_scale;
                st.offset_y = anchor_y - scene_y * next_scale;
            }
        }
        self.update_percent_label();
        self.drawing.queue_draw();
        self.schedule_persist();
    }

    fn update_percent_label(&self) {
        let pct = self
            .session
            .borrow()
            .as_ref()
            .map(|st| (st.scale * 100.0).round() as i32);
        if let Some(pct) = pct {
            self.percent_label.set_text(&format!("{pct}%"));
        }
    }

    pub fn snapshot(&self) -> Option<FocusSnapshot> {
        self.session.borrow().as_ref().map(|st| FocusSnapshot {
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

    fn trigger_export(&self, kind: DiagramExportKind) {
        let meta_and_cb = self.session.borrow().as_ref().map(|st| {
            (
                DiagramExportMeta {
                    note_path: st.note_path.clone(),
                    ordinal: st.ordinal,
                    source: st.source.clone(),
                    mmd: st.mmd,
                    collapsed: st.collapsed.clone(),
                    scene: st.scene.clone(),
                },
                st.callbacks.on_export.clone(),
            )
        });
        if let Some((meta, cb)) = meta_and_cb {
            (cb)(kind, meta);
        }
    }

    fn handle_canvas_click(&self, x: f64, y: f64) {
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
            let mut guard = self.session.borrow_mut();
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
                self.close();
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
                let ctrl = self.clone();
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

                    if let Ok(Ok(new_scene)) = res {
                        let cb_opt = {
                            let mut guard = ctrl.session.borrow_mut();
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
                            ctrl.drawing.queue_draw();
                            (cb)(ordinal, collapsed, new_scene);
                        }
                    }
                });
            }
            None => {}
        }
    }

    fn schedule_persist(&self) {
        let mut guard = self.session.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        if let Some(old) = st.persist_source.take() {
            old.remove();
        }
        let c = self.clone();
        st.persist_source = Some(glib::timeout_add_local_once(
            Duration::from_millis(180),
            move || c.persist_now(),
        ));
    }

    pub fn persist_now(&self) {
        let data = {
            let mut guard = self.session.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            if let Some(src) = st.persist_source.take() {
                src.remove();
            }
            (
                Arc::clone(&st.core),
                st.vault_root.clone(),
                st.note_path.clone(),
                st.ordinal,
                st.scale,
                st.offset_x,
                st.offset_y,
            )
        };
        let (core, vault, note, ordinal, scale, ox, oy) = data;
        let _ = core.remember_diagram_focus(vault, note, ordinal, scale, ox, oy);
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
