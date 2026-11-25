// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 左右边栏内侧平滑拖拽调宽手柄、窗口坐标系防抖换算与双击复位。

use crate::core::PanelRegion;
use crate::window::shell::WindowInner;
use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub const MIN_SIDEBAR_WIDTH: f64 = 180.0;
pub const MAX_SIDEBAR_WIDTH: f64 = 480.0;
pub const DEFAULT_SIDEBAR_WIDTH: f64 = 280.0;
pub const MIN_CENTER_CONTENT_WIDTH: f64 = 320.0;
const COLLAPSED_CENTER_RESERVE: f64 = 64.0;
const HANDLE_HIT_WIDTH: i32 = 6;
const HANDLE_LINE_WIDTH: i32 = 2;

/// 为左右边栏包装轻量 `gtk4::Overlay` 并在内侧边沿叠加 6px 透明拖拽手柄与 2px 悬停指示线。
pub fn wrap_sidebar_with_resize_handle(
    sidebar_widget: &impl IsA<gtk4::Widget>,
    region: PanelRegion,
) -> (gtk4::Overlay, gtk4::Box) {
    let overlay = gtk4::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(sidebar_widget));

    let handle = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    handle.add_css_class("hortus-sidebar-resize-handle");
    handle.set_widget_name(match region {
        PanelRegion::Left => "hortus-left-sidebar-resize-handle",
        PanelRegion::Right => "hortus-right-sidebar-resize-handle",
        PanelRegion::Bottom => "hortus-bottom-resize-handle",
    });
    handle.set_size_request(HANDLE_HIT_WIDTH, -1);
    handle.set_vexpand(true);
    handle.set_valign(gtk4::Align::Fill);
    let edge_align = match region {
        PanelRegion::Left => gtk4::Align::End,
        PanelRegion::Right => gtk4::Align::Start,
        PanelRegion::Bottom => gtk4::Align::Fill,
    };
    handle.set_halign(edge_align);

    let fallback_cursor = gdk::Cursor::from_name("ew-resize", None);
    let cursor = gdk::Cursor::from_name("col-resize", fallback_cursor.as_ref()).or(fallback_cursor);
    handle.set_cursor(cursor.as_ref());

    let indicator = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    indicator.add_css_class("hortus-sidebar-resize-line");
    indicator.set_size_request(HANDLE_LINE_WIDTH, -1);
    indicator.set_vexpand(true);
    indicator.set_valign(gtk4::Align::Fill);
    indicator.set_halign(edge_align);
    indicator.set_can_target(false);
    handle.append(&indicator);

    overlay.add_overlay(&handle);
    (overlay, handle)
}

pub fn compute_sidebar_max_width(window_w: f64, other_visible_w: f64, collapsed: bool) -> f64 {
    let room = if collapsed {
        (window_w - COLLAPSED_CENTER_RESERVE).max(MIN_SIDEBAR_WIDTH)
    } else {
        (window_w - other_visible_w - MIN_CENTER_CONTENT_WIDTH).max(MIN_SIDEBAR_WIDTH)
    };
    MAX_SIDEBAR_WIDTH.min(room).max(MIN_SIDEBAR_WIDTH)
}

/// 根据容器总宽计算左右边栏的真实夹紧宽度与互不挤压的 `OverlaySplitView` 比例。
pub fn compute_split_fractions(
    total_w: f64,
    left_w: f64,
    right_w: f64,
    left_visible: bool,
    right_visible: bool,
    left_collapsed: bool,
    right_collapsed: bool,
) -> (f64, f64, f64, f64) {
    let safe_total = total_w.max(MIN_SIDEBAR_WIDTH * 2.0);
    let clamped_left = left_w.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
    let clamped_right = right_w.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);

    let right_occupied_for_left = if right_visible && !right_collapsed && !left_collapsed {
        clamped_right
    } else {
        0.0
    };
    let left_max = compute_sidebar_max_width(safe_total, right_occupied_for_left, left_collapsed);
    let eff_left = clamped_left.min(left_max);
    let left_occupied = if left_visible && !left_collapsed {
        eff_left
    } else {
        0.0
    };
    let right_max = compute_sidebar_max_width(safe_total, left_occupied, right_collapsed);
    let eff_right = clamped_right.min(right_max);

    let left_fraction = (eff_left / safe_total).clamp(0.05, 0.90);
    let right_container_w = (safe_total - left_occupied).max(MIN_SIDEBAR_WIDTH);
    let right_fraction = (eff_right / right_container_w).clamp(0.05, 0.90);
    (eff_left, eff_right, left_fraction, right_fraction)
}

#[derive(Clone, Copy, Debug)]
struct DragOrigin {
    origin_win_x: f64,
    start_sx: f64,
    start_sy: f64,
    start_left_w: f64,
    start_right_w: f64,
    moved: bool,
}

impl WindowInner {
    pub fn current_sidebar_width(&self, region: PanelRegion) -> f64 {
        let (split, widget, fallback) = match region {
            PanelRegion::Left => (
                &self.left_split,
                self.tool_dock.region_widget(PanelRegion::Left),
                self.state.borrow().left_sidebar_width,
            ),
            PanelRegion::Right => (
                &self.right_split,
                self.tool_dock.region_widget(PanelRegion::Right),
                self.state.borrow().right_sidebar_width,
            ),
            PanelRegion::Bottom => return self.state.borrow().bottom_height,
        };
        if split.shows_sidebar() && widget.width() > 0 {
            f64::from(widget.width()).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
        } else {
            fallback.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
        }
    }

    pub fn apply_sidebar_widths(self: &Rc<Self>, desired_left_w: f64, desired_right_w: f64) {
        let split_w = self.left_split.width();
        let total_w = if split_w > 0 {
            f64::from(split_w)
        } else {
            f64::from(self.window.default_width().max(720))
        };
        let desired_left = desired_left_w.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        let desired_right = desired_right_w.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        let left_visible = self.left_split.shows_sidebar();
        let right_visible = self.right_split.shows_sidebar();
        let left_collapsed = self.left_split.is_collapsed();
        let right_collapsed = self.right_split.is_collapsed();

        let (eff_left, eff_right, left_frac, right_frac) = compute_split_fractions(
            total_w,
            desired_left,
            desired_right,
            left_visible,
            right_visible,
            left_collapsed,
            right_collapsed,
        );
        {
            let mut st = self.state.borrow_mut();
            st.desired_left_width = desired_left;
            st.desired_right_width = desired_right;
            st.left_sidebar_width = eff_left;
            st.right_sidebar_width = eff_right;
            st.pending_sidebar_widths = split_w <= 0;
        }
        if (self.left_split.sidebar_width_fraction() - left_frac).abs() > 1e-4 {
            self.left_split.set_sidebar_width_fraction(left_frac);
        }
        if (self.right_split.sidebar_width_fraction() - right_frac).abs() > 1e-4 {
            self.right_split.set_sidebar_width_fraction(right_frac);
        }
        if split_w <= 0 {
            self.schedule_pending_sidebar_widths();
        }
    }

    pub fn reapply_sidebar_widths(self: &Rc<Self>) {
        let (left_w, right_w) = {
            let st = self.state.borrow();
            (st.desired_left_width, st.desired_right_width)
        };
        self.apply_sidebar_widths(left_w, right_w);
    }

    pub fn install_width_probe(self: &Rc<Self>, probe: &gtk4::DrawingArea) {
        let last_width = Rc::new(Cell::new(0));
        let weak = Rc::downgrade(self);
        probe.connect_resize(move |_, width, _| {
            if width <= 0 || last_width.get() == width {
                return;
            }
            last_width.set(width);
            let Some(win) = weak.upgrade() else {
                return;
            };
            if win.state.borrow().disposing {
                return;
            }
            if let Some(old) = win.state.borrow_mut().resize_clamp_source.take() {
                old.remove();
            }
            let weak_timer = Rc::downgrade(&win);
            let src =
                glib::timeout_add_local_once(std::time::Duration::from_millis(45), move || {
                    let Some(win) = weak_timer.upgrade() else {
                        return;
                    };
                    win.state.borrow_mut().resize_clamp_source = None;
                    if !win.state.borrow().disposing {
                        win.reapply_sidebar_widths();
                    }
                });
            win.state.borrow_mut().resize_clamp_source = Some(src);
        });
    }

    fn schedule_pending_sidebar_widths(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.left_split.add_tick_callback(move |split, _| {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let pending = {
                let st = win.state.borrow();
                !st.disposing && st.pending_sidebar_widths
            };
            if !pending {
                return glib::ControlFlow::Break;
            }
            if split.width() > 0 {
                win.reapply_sidebar_widths();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn reset_sidebar_width(self: &Rc<Self>, region: PanelRegion) {
        let (cur_left, cur_right) = {
            let st = self.state.borrow();
            (st.desired_left_width, st.desired_right_width)
        };
        match region {
            PanelRegion::Left => self.apply_sidebar_widths(DEFAULT_SIDEBAR_WIDTH, cur_right),
            PanelRegion::Right => self.apply_sidebar_widths(cur_left, DEFAULT_SIDEBAR_WIDTH),
            PanelRegion::Bottom => return,
        }
        self.schedule_layout_save();
    }

    pub fn install_sidebar_resize_handlers(self: &Rc<Self>) {
        self.bind_single_sidebar_handle(&self.left_resize_handle, PanelRegion::Left);
        self.bind_single_sidebar_handle(&self.right_resize_handle, PanelRegion::Right);

        for (region, split) in [
            (PanelRegion::Left, &self.left_split),
            (PanelRegion::Right, &self.right_split),
        ] {
            let weak_show = Rc::downgrade(self);
            split.connect_show_sidebar_notify(move |split_view| {
                if let Some(win) = weak_show.upgrade()
                    && !win.state.borrow().disposing
                {
                    win.reapply_sidebar_widths();
                    if split_view.shows_sidebar() {
                        win.flush_visible_static_tool(region);
                    }
                    win.schedule_layout_save();
                }
            });
            let weak_col = Rc::downgrade(self);
            split.connect_collapsed_notify(move |_| {
                if let Some(win) = weak_col.upgrade()
                    && !win.state.borrow().disposing
                {
                    win.reapply_sidebar_widths();
                }
            });
        }
    }

    fn bind_single_sidebar_handle(self: &Rc<Self>, handle: &gtk4::Box, region: PanelRegion) {
        let origin_slot: Rc<RefCell<Option<DragOrigin>>> = Rc::new(RefCell::new(None));
        let suppress_drag = Rc::new(Cell::new(false));

        let double_click = gtk4::GestureClick::new();
        double_click.set_button(1);
        {
            let weak = Rc::downgrade(self);
            let suppress = Rc::clone(&suppress_drag);
            let origin_ref = Rc::clone(&origin_slot);
            let h_ref = handle.clone();
            double_click.connect_pressed(move |gesture, n_press, _, _| {
                if n_press == 2 {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    suppress.set(true);
                    *origin_ref.borrow_mut() = None;
                    h_ref.remove_css_class("dragging");
                    if let Some(win) = weak.upgrade() {
                        win.reset_sidebar_width(region);
                    }
                }
            });
        }
        handle.add_controller(double_click);

        let drag = gtk4::GestureDrag::new();
        drag.set_button(1);
        {
            let weak = Rc::downgrade(self);
            let origin_ref = Rc::clone(&origin_slot);
            let suppress = Rc::clone(&suppress_drag);
            let h_ref = handle.clone();
            drag.connect_drag_begin(move |_, sx, sy| {
                suppress.set(false);
                let Some(win) = weak.upgrade() else { return };
                let Some(pt) =
                    h_ref.compute_point(&win.window, &graphene::Point::new(sx as f32, sy as f32))
                else {
                    return;
                };
                let start_left_w = win.current_sidebar_width(PanelRegion::Left);
                let start_right_w = win.current_sidebar_width(PanelRegion::Right);
                *origin_ref.borrow_mut() = Some(DragOrigin {
                    origin_win_x: f64::from(pt.x()),
                    start_sx: sx,
                    start_sy: sy,
                    start_left_w,
                    start_right_w,
                    moved: false,
                });
                h_ref.add_css_class("dragging");
            });
        }
        {
            let weak = Rc::downgrade(self);
            let origin_ref = Rc::clone(&origin_slot);
            let suppress = Rc::clone(&suppress_drag);
            let h_ref = handle.clone();
            drag.connect_drag_update(move |_, ox, oy| {
                if suppress.get() {
                    return;
                }
                let Some(win) = weak.upgrade() else { return };
                let Some(mut origin) = *origin_ref.borrow() else {
                    return;
                };
                let cur_pt = graphene::Point::new(
                    (origin.start_sx + ox) as f32,
                    (origin.start_sy + oy) as f32,
                );
                let Some(win_pt) = h_ref.compute_point(&win.window, &cur_pt) else {
                    return;
                };
                let delta = f64::from(win_pt.x()) - origin.origin_win_x;
                if !origin.moved && delta.abs() < 0.5 {
                    return;
                }
                origin.moved = true;
                *origin_ref.borrow_mut() = Some(origin);
                win.apply_sidebar_drag_delta(
                    region,
                    origin.start_left_w,
                    origin.start_right_w,
                    delta,
                );
            });
        }
        {
            let weak = Rc::downgrade(self);
            let origin_ref = Rc::clone(&origin_slot);
            let suppress = Rc::clone(&suppress_drag);
            let h_ref = handle.clone();
            drag.connect_drag_end(move |_, _, _| {
                h_ref.remove_css_class("dragging");
                let origin = origin_ref.borrow_mut().take();
                if suppress.replace(false) {
                    return;
                }
                if origin.is_some_and(|o| o.moved)
                    && let Some(win) = weak.upgrade()
                {
                    win.schedule_layout_save();
                }
            });
        }
        {
            let origin_ref = Rc::clone(&origin_slot);
            let suppress = Rc::clone(&suppress_drag);
            let h_ref = handle.clone();
            drag.connect_cancel(move |_, _| {
                h_ref.remove_css_class("dragging");
                suppress.set(false);
                *origin_ref.borrow_mut() = None;
            });
        }
        handle.add_controller(drag);
    }

    pub fn apply_sidebar_drag_delta(
        self: &Rc<Self>,
        region: PanelRegion,
        start_left_w: f64,
        start_right_w: f64,
        delta: f64,
    ) {
        let window_w = f64::from(self.left_split.width().max(self.window.width()).max(720));
        match region {
            PanelRegion::Left => {
                let collapsed = self.left_split.is_collapsed();
                let other_visible = if !collapsed
                    && self.right_split.shows_sidebar()
                    && !self.right_split.is_collapsed()
                {
                    start_right_w
                } else {
                    0.0
                };
                let max_allowed = compute_sidebar_max_width(window_w, other_visible, collapsed);
                let next_left = (start_left_w + delta).clamp(MIN_SIDEBAR_WIDTH, max_allowed);
                let desired_right = self.state.borrow().desired_right_width;
                self.apply_sidebar_widths(next_left, desired_right);
            }
            PanelRegion::Right => {
                let collapsed = self.right_split.is_collapsed();
                let other_visible = if !collapsed
                    && self.left_split.shows_sidebar()
                    && !self.left_split.is_collapsed()
                {
                    start_left_w
                } else {
                    0.0
                };
                let max_allowed = compute_sidebar_max_width(window_w, other_visible, collapsed);
                let next_right = (start_right_w - delta).clamp(MIN_SIDEBAR_WIDTH, max_allowed);
                let desired_left = self.state.borrow().desired_left_width;
                self.apply_sidebar_widths(desired_left, next_right);
            }
            PanelRegion::Bottom => {}
        }
    }

    /// 以窗口坐标系 `compute_point` 驱动拖拽逻辑（供自动化验证完整路径）。
    #[cfg(any(test, feature = "automation"))]
    pub fn simulate_sidebar_drag_window_delta(
        self: &Rc<Self>,
        region: PanelRegion,
        window_delta_x: f64,
    ) -> bool {
        let handle = match region {
            PanelRegion::Left => &self.left_resize_handle,
            PanelRegion::Right => &self.right_resize_handle,
            PanelRegion::Bottom => return false,
        };
        let Some(origin_pt) = handle.compute_point(&self.window, &graphene::Point::new(3.0, 40.0))
        else {
            return false;
        };
        let Some(end_pt) = handle.compute_point(
            &self.window,
            &graphene::Point::new((3.0 + window_delta_x) as f32, 40.0),
        ) else {
            return false;
        };
        let start_left_w = self.current_sidebar_width(PanelRegion::Left);
        let start_right_w = self.current_sidebar_width(PanelRegion::Right);
        let delta = f64::from(end_pt.x()) - f64::from(origin_pt.x());
        self.apply_sidebar_drag_delta(region, start_left_w, start_right_w, delta);
        self.schedule_layout_save();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CliOptions;
    use gtk4::gio;
    use libadwaita as adw;

    #[test]
    fn split_fractions_keep_opposite_sidebar_stable_and_center_min_width() {
        let (l1, r1, lf1, rf1) =
            compute_split_fractions(1280.0, 260.0, 220.0, true, true, false, false);
        assert!((l1 - 260.0).abs() < 0.01);
        assert!((r1 - 220.0).abs() < 0.01);
        assert!((1280.0 * lf1 - 260.0).abs() < 0.5);
        assert!(((1280.0 - 260.0) * rf1 - 220.0).abs() < 0.5);

        // 左侧栏拉宽至 380px 时，右侧栏实际分配像素保持 220px 不随左侧缩水。
        let (l2, r2, lf2, rf2) =
            compute_split_fractions(1280.0, 380.0, 220.0, true, true, false, false);
        assert!((l2 - 380.0).abs() < 0.01);
        assert!((r2 - 220.0).abs() < 0.01);
        assert!((1280.0 * lf2 - 380.0).abs() < 0.5);
        assert!(((1280.0 - 380.0) * rf2 - 220.0).abs() < 0.5);

        // 相反侧栏隐藏时，不占用主栏预算。
        let (l_only, _, _, _) =
            compute_split_fractions(900.0, 420.0, 360.0, true, false, false, false);
        assert!((l_only - 420.0).abs() < 0.01);

        // 窄窗口双栏互相夹紧时不超正文下限；窗口放宽后可按同一期望值恢复。
        let (narrow_l, narrow_r, _, _) =
            compute_split_fractions(880.0, 380.0, 360.0, true, true, false, false);
        assert!(880.0 - narrow_l - narrow_r >= MIN_CENTER_CONTENT_WIDTH - 0.1);
        let (wide_l, wide_r, _, _) =
            compute_split_fractions(1280.0, 380.0, 360.0, true, true, false, false);
        assert!((wide_l - 380.0).abs() < 0.01);
        assert!((wide_r - 360.0).abs() < 0.01);

        // 中间正文区至少保留 320px。
        let max_left_on_900 = compute_sidebar_max_width(900.0, 350.0, false);
        assert!((max_left_on_900 - 230.0).abs() < 0.01);
        let max_collapsed = compute_sidebar_max_width(420.0, 0.0, true);
        assert!((max_collapsed - 356.0).abs() < 0.01);
    }

    #[test]
    fn sidebar_resize_handles_apply_drag_reset_and_preserve_zero_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping sidebar resize GTK test: GTK 初始化失败或非主线程");
            return;
        }
        let app = adw::Application::builder()
            .application_id("org.hortus.test.SidebarResize")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        let weak_slot: Rc<RefCell<Option<std::rc::Weak<WindowInner>>>> =
            Rc::new(RefCell::new(None));
        let metrics_ok = Rc::new(Cell::new(false));
        let weak_act = Rc::clone(&weak_slot);
        let ok_act = Rc::clone(&metrics_ok);
        app.connect_activate(move |app| {
            let win = crate::window::HortusWindow::new(
                app,
                CliOptions {
                    width: Some(1280),
                    height: Some(800),
                    ..Default::default()
                },
            );
            *weak_act.borrow_mut() = Some(win.downgrade());
            let inner = Rc::clone(&win.inner);
            drop(win);
            inner.window.present();

            assert!((inner.left_split.min_sidebar_width() - 180.0).abs() < 0.01);
            assert!((inner.left_split.max_sidebar_width() - 480.0).abs() < 0.01);
            assert!((inner.right_split.min_sidebar_width() - 180.0).abs() < 0.01);
            assert!((inner.right_split.max_sidebar_width() - 480.0).abs() < 0.01);
            assert_eq!(inner.left_resize_handle.halign(), gtk4::Align::End);
            assert_eq!(inner.right_resize_handle.halign(), gtk4::Align::Start);
            assert_eq!(inner.left_resize_handle.width_request(), 6);
            assert_eq!(inner.right_resize_handle.width_request(), 6);
            assert!(inner.left_resize_handle.cursor().is_some());
            assert!(inner.right_resize_handle.cursor().is_some());

            let inner_step = Rc::clone(&inner);
            let ok_step = Rc::clone(&ok_act);
            let app_done = app.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                inner_step.apply_sidebar_widths(260.0, 220.0);
                glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                    let left_before = inner_step.current_sidebar_width(PanelRegion::Left);
                    let right_before = inner_step.current_sidebar_width(PanelRegion::Right);
                    let dragged_left =
                        inner_step.simulate_sidebar_drag_window_delta(PanelRegion::Left, 120.0);
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(120),
                        move || {
                            let left_after = inner_step.current_sidebar_width(PanelRegion::Left);
                            let right_after = inner_step.current_sidebar_width(PanelRegion::Right);
                            inner_step.reset_sidebar_width(PanelRegion::Left);
                            let reset_left = inner_step.state.borrow().left_sidebar_width;
                            let pass = dragged_left
                                && (left_before - 260.0).abs() <= 4.0
                                && (right_before - 220.0).abs() <= 4.0
                                && (left_after - 380.0).abs() <= 4.0
                                && (right_after - right_before).abs() <= 4.0
                                && (reset_left - DEFAULT_SIDEBAR_WIDTH).abs() <= 0.1;
                            ok_step.set(pass);
                            inner_step.window.close();
                            glib::idle_add_local_once(move || app_done.quit());
                        },
                    );
                });
            });
            let app_to = app.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(4000), move || {
                app_to.quit();
            });
        });
        app.run_with_args(&Vec::<String>::new());
        assert!(
            metrics_ok.get(),
            "sidebar resize GTK integration check failed"
        );
        let weak = weak_slot.borrow().as_ref().expect("window").clone();
        assert_eq!(weak.strong_count(), 0);
    }
}
