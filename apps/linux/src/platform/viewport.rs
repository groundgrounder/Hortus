// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! GTK4 真宽度感知阅读容器与编辑宿主：`WidgetImpl::size_allocate` + 统一 920px 列宽 + 空闲首对齐 + 90ms 防抖重排。

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub const READING_COLUMN_MAX_WIDTH: i32 = 920;

const H_PADDING_BREAKPOINT: i32 = 820;
const H_PADDING_NARROW: i32 = 24;
const H_PADDING_WIDE: i32 = 44;
const MIN_VIEWPORT_WIDTH: i32 = 240;
const REFLOW_THRESHOLD_PX: i32 = 16;
const DEBOUNCE_MS: u64 = 90;

pub fn responsive_horizontal_padding(width: i32) -> i32 {
    if width < H_PADDING_BREAKPOINT {
        H_PADDING_NARROW * 2
    } else {
        H_PADDING_WIDE * 2
    }
}

/// 统一计算阅读列与源码编辑区居中列宽和水平留白：`(col_w, side_offset)`。
pub fn effective_reading_column_width(viewport_w: i32, max_w: Option<i32>) -> (i32, i32) {
    let total_pad = responsive_horizontal_padding(viewport_w);
    let unclamped = (viewport_w - total_pad).max(1);
    let col_w = max_w
        .filter(|&m| m > 0)
        .map_or(unclamped, |m| unclamped.min(m));
    let side_offset = ((viewport_w - col_w) / 2).max(total_pad / 2);
    (col_w, side_offset)
}

type WidthCallback = Rc<dyn Fn(i32)>;

mod imp {
    use super::*;

    pub struct HortusReadingViewport {
        pub child: RefCell<Option<gtk4::Widget>>,
        pub max_content_width: Cell<i32>,
        pub horizontal_padding: Cell<i32>,
        pub last_viewport_w: Cell<i32>,
        pub last_allocated_w: Cell<i32>,
        pub settled_w: Cell<i32>,
        pub resize_alloc_count: Cell<u32>,
        pub reflow_count: Cell<u32>,
        pub viewport_idle_source: RefCell<Option<glib::SourceId>>,
        pub idle_source: RefCell<Option<glib::SourceId>>,
        pub debounce_source: RefCell<Option<glib::SourceId>>,
        pub on_relayout: RefCell<Option<WidthCallback>>,
        pub on_viewport_width: RefCell<Option<WidthCallback>>,
    }

    impl Default for HortusReadingViewport {
        fn default() -> Self {
            Self {
                child: RefCell::new(None),
                max_content_width: Cell::new(READING_COLUMN_MAX_WIDTH),
                horizontal_padding: Cell::new(H_PADDING_WIDE * 2),
                last_viewport_w: Cell::new(0),
                last_allocated_w: Cell::new(0),
                settled_w: Cell::new(0),
                resize_alloc_count: Cell::new(0),
                reflow_count: Cell::new(0),
                viewport_idle_source: RefCell::new(None),
                idle_source: RefCell::new(None),
                debounce_source: RefCell::new(None),
                on_relayout: RefCell::new(None),
                on_viewport_width: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HortusReadingViewport {
        const NAME: &'static str = "HortusReadingViewport";
        type Type = super::HortusReadingViewport;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for HortusReadingViewport {
        fn dispose(&self) {
            if let Some(src) = self.viewport_idle_source.borrow_mut().take() {
                src.remove();
            }
            if let Some(src) = self.idle_source.borrow_mut().take() {
                src.remove();
            }
            if let Some(src) = self.debounce_source.borrow_mut().take() {
                src.remove();
            }
            *self.on_relayout.borrow_mut() = None;
            *self.on_viewport_width.borrow_mut() = None;
            let old = self.child.borrow_mut().take();
            if let Some(child) = old {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for HortusReadingViewport {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            self.child
                .borrow()
                .as_ref()
                .map(|c| c.request_mode())
                .unwrap_or(gtk4::SizeRequestMode::ConstantSize)
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let child = self.child.borrow().clone();
            let Some(child) = child else {
                return (0, 0, -1, -1);
            };
            match orientation {
                gtk4::Orientation::Horizontal => {
                    let (min, nat, min_base, nat_base) = child.measure(orientation, for_size);
                    // 安全最小宽封顶 240px：绝不把内部宽块级控件顶破 AdwToastOverlay。
                    let safe_min = (min + H_PADDING_NARROW * 2).clamp(0, MIN_VIEWPORT_WIDTH);
                    let safe_nat = (nat + H_PADDING_WIDE * 2).max(safe_min);
                    (safe_min, safe_nat, min_base, nat_base)
                }
                _ => {
                    let inner_for = if for_size >= 0 {
                        let max_w = self.max_content_width.get();
                        let (col_w, _) =
                            effective_reading_column_width(for_size, (max_w > 0).then_some(max_w));
                        col_w
                    } else {
                        for_size
                    };
                    child.measure(orientation, inner_for)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let total_pad = responsive_horizontal_padding(width);
            self.horizontal_padding.set(total_pad);
            let max_w = self.max_content_width.get();
            let (col_w, side_offset) =
                effective_reading_column_width(width, (max_w > 0).then_some(max_w));
            let child = self.child.borrow().clone();
            if let Some(child) = child {
                let transform = gtk4::gsk::Transform::new()
                    .translate(&gtk4::graphene::Point::new(side_offset as f32, 0.0));
                child.allocate(col_w, height, baseline, Some(transform));
            }
            let avail_w = col_w.max(MIN_VIEWPORT_WIDTH);
            if width > 0 && self.last_viewport_w.get() != width {
                self.last_viewport_w.set(width);
                self.schedule_viewport_width_callback();
            }
            if self.last_allocated_w.get() != avail_w {
                self.last_allocated_w.set(avail_w);
                self.resize_alloc_count
                    .set(self.resize_alloc_count.get().saturating_add(1));
                self.schedule_width_callback(avail_w);
            }
        }
    }

    impl HortusReadingViewport {
        fn schedule_viewport_width_callback(&self) {
            if let Some(src) = self.viewport_idle_source.borrow_mut().take() {
                src.remove();
            }
            let weak = self.obj().downgrade();
            let id = glib::idle_add_local_once(move || {
                let Some(obj) = weak.upgrade() else { return };
                let imp = obj.imp();
                imp.viewport_idle_source.borrow_mut().take();
                let vp_w = obj.width().max(imp.last_viewport_w.get());
                let cb = imp.on_viewport_width.borrow().clone();
                if let Some(cb) = cb
                    && vp_w > 0
                {
                    cb(vp_w);
                }
            });
            *self.viewport_idle_source.borrow_mut() = Some(id);
        }

        fn cancel_pending(&self) {
            if let Some(src) = self.idle_source.borrow_mut().take() {
                src.remove();
            }
            if let Some(src) = self.debounce_source.borrow_mut().take() {
                src.remove();
            }
        }

        fn schedule_width_callback(&self, avail_w: i32) {
            self.cancel_pending();
            let weak = self.obj().downgrade();
            if self.settled_w.get() == 0 {
                let id = glib::idle_add_local_once(move || {
                    let Some(obj) = weak.upgrade() else { return };
                    let imp = obj.imp();
                    imp.idle_source.borrow_mut().take();
                    if imp.last_allocated_w.get() != avail_w {
                        return;
                    }
                    let cb = imp.on_relayout.borrow().clone();
                    if let Some(cb) = cb {
                        cb(avail_w);
                    }
                });
                *self.idle_source.borrow_mut() = Some(id);
            } else {
                let id =
                    glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
                        let Some(obj) = weak.upgrade() else { return };
                        let imp = obj.imp();
                        imp.debounce_source.borrow_mut().take();
                        if imp.last_allocated_w.get() != avail_w {
                            return;
                        }
                        let prev = imp.settled_w.get();
                        if (avail_w - prev).abs() < REFLOW_THRESHOLD_PX {
                            return;
                        }
                        imp.settled_w.set(avail_w);
                        imp.reflow_count
                            .set(imp.reflow_count.get().saturating_add(1));
                        let cb = imp.on_relayout.borrow().clone();
                        if let Some(cb) = cb {
                            cb(avail_w);
                        }
                    });
                *self.debounce_source.borrow_mut() = Some(id);
            }
        }
    }
}

glib::wrapper! {
    pub struct HortusReadingViewport(ObjectSubclass<imp::HortusReadingViewport>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for HortusReadingViewport {
    fn default() -> Self {
        Self::new()
    }
}

impl HortusReadingViewport {
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj.set_hexpand(true);
        obj.set_vexpand(true);
        obj
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk4::Widget>>) {
        let imp = self.imp();
        let old = imp.child.borrow_mut().take();
        if let Some(old_child) = old {
            old_child.unparent();
        }
        if let Some(child) = child {
            let widget: gtk4::Widget = child.clone().upcast();
            widget.set_parent(self);
            *imp.child.borrow_mut() = Some(widget);
        }
    }

    pub fn connect_relayout_width(&self, cb: impl Fn(i32) + 'static) {
        *self.imp().on_relayout.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn connect_viewport_width(&self, cb: impl Fn(i32) + 'static) {
        *self.imp().on_viewport_width.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn set_max_content_width(&self, max_w: Option<i32>) {
        let normalized = max_w.filter(|&w| w > 0).unwrap_or(0);
        let imp = self.imp();
        if imp.max_content_width.get() != normalized {
            imp.max_content_width.set(normalized);
            self.queue_resize();
        }
    }

    pub fn max_content_width(&self) -> Option<i32> {
        let w = self.imp().max_content_width.get();
        (w > 0).then_some(w)
    }

    pub fn last_allocated_width(&self) -> i32 {
        self.imp().last_allocated_w.get()
    }

    pub fn settled_width(&self) -> i32 {
        self.imp().settled_w.get()
    }

    pub fn set_settled_width(&self, width: i32) {
        self.imp().settled_w.set(width.max(0));
    }

    pub fn mark_reflowed_width(&self, width: i32) {
        let imp = self.imp();
        imp.settled_w.set(width.max(0));
        imp.reflow_count
            .set(imp.reflow_count.get().saturating_add(1));
    }

    pub fn resize_alloc_count(&self) -> u32 {
        self.imp().resize_alloc_count.get()
    }

    pub fn reflow_count(&self) -> u32 {
        self.imp().reflow_count.get()
    }

    pub fn clear_timers(&self) {
        let imp = self.imp();
        if let Some(src) = imp.viewport_idle_source.borrow_mut().take() {
            src.remove();
        }
        if let Some(src) = imp.idle_source.borrow_mut().take() {
            src.remove();
        }
        if let Some(src) = imp.debounce_source.borrow_mut().take() {
            src.remove();
        }
        *imp.on_relayout.borrow_mut() = None;
        *imp.on_viewport_width.borrow_mut() = None;
    }
}

mod edit_imp {
    use super::*;

    #[derive(Default)]
    pub struct HortusEditHost {
        pub child: RefCell<Option<gtk4::Widget>>,
        pub edit_view: RefCell<Option<glib::WeakRef<gtk4::TextView>>>,
        pub last_width: Cell<i32>,
        pub idle_source: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HortusEditHost {
        const NAME: &'static str = "HortusEditHost";
        type Type = super::HortusEditHost;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for HortusEditHost {
        fn dispose(&self) {
            if let Some(src) = self.idle_source.borrow_mut().take() {
                src.remove();
            }
            *self.edit_view.borrow_mut() = None;
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for HortusEditHost {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            self.child
                .borrow()
                .as_ref()
                .map(|c| c.request_mode())
                .unwrap_or(gtk4::SizeRequestMode::ConstantSize)
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let child = self.child.borrow().clone();
            let Some(child) = child else {
                return (0, 0, -1, -1);
            };
            child.measure(orientation, for_size)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let child = self.child.borrow().clone();
            if let Some(child) = child {
                child.allocate(width, height, baseline, None);
            }
            if width > 0 && self.last_width.get() != width {
                self.last_width.set(width);
                self.schedule_margin_sync();
            }
        }
    }

    impl HortusEditHost {
        fn schedule_margin_sync(&self) {
            if let Some(src) = self.idle_source.borrow_mut().take() {
                src.remove();
            }
            let weak = self.obj().downgrade();
            let id = glib::idle_add_local_once(move || {
                let Some(obj) = weak.upgrade() else { return };
                obj.imp().idle_source.borrow_mut().take();
                let w = obj.width().max(obj.imp().last_width.get());
                obj.sync_margins_for_width(w);
            });
            *self.idle_source.borrow_mut() = Some(id);
        }
    }
}

glib::wrapper! {
    /// 轻量编辑视图外包容器：保持 `edit_scrolled` 原生虚拟滚动，同步 `edit_view` 居中列宽留白。
    pub struct HortusEditHost(ObjectSubclass<edit_imp::HortusEditHost>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for HortusEditHost {
    fn default() -> Self {
        Self::new()
    }
}

impl HortusEditHost {
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj.set_hexpand(true);
        obj.set_vexpand(true);
        obj
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk4::Widget>>) {
        let imp = self.imp();
        let old = imp.child.borrow_mut().take();
        if let Some(old_child) = old {
            old_child.unparent();
        }
        if let Some(child) = child {
            let widget: gtk4::Widget = child.clone().upcast();
            widget.set_parent(self);
            *imp.child.borrow_mut() = Some(widget);
        }
    }

    pub fn set_edit_view(&self, view: Option<&gtk4::TextView>) {
        *self.imp().edit_view.borrow_mut() = view.map(glib::object::ObjectExt::downgrade);
    }

    pub fn sync_margins_for_width(&self, width: i32) {
        if width <= 0 {
            return;
        }
        let (_, side_offset) =
            effective_reading_column_width(width, Some(READING_COLUMN_MAX_WIDTH));
        let Some(view) = self
            .imp()
            .edit_view
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        else {
            return;
        };
        if view.left_margin() != side_offset {
            view.set_left_margin(side_offset);
        }
        if view.right_margin() != side_offset {
            view.set_right_margin(side_offset);
        }
    }

    pub fn clear_timers(&self) {
        let imp = self.imp();
        if let Some(src) = imp.idle_source.borrow_mut().take() {
            src.remove();
        }
        *imp.edit_view.borrow_mut() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_column_width_clamps_to_920_and_supports_unclamped_mmd() {
        assert_eq!(
            effective_reading_column_width(1440, Some(READING_COLUMN_MAX_WIDTH)),
            (920, 260)
        );
        assert_eq!(
            effective_reading_column_width(1000, Some(READING_COLUMN_MAX_WIDTH)),
            (912, 44)
        );
        assert_eq!(
            effective_reading_column_width(700, Some(READING_COLUMN_MAX_WIDTH)),
            (652, 24)
        );
        assert_eq!(effective_reading_column_width(1440, None), (1352, 44));
    }

    #[test]
    fn viewport_measure_caps_min_width_and_tracks_allocations() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping viewport test: GTK 初始化失败或非主线程");
            return;
        }
        let vp = HortusReadingViewport::new();
        assert_eq!(vp.max_content_width(), Some(READING_COLUMN_MAX_WIDTH));
        let wide_child = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        wide_child.set_size_request(900, 120);
        vp.set_child(Some(&wide_child));

        let (min_w, nat_w, _, _) = vp.measure(gtk4::Orientation::Horizontal, -1);
        assert!(min_w <= 240, "viewport min_w={min_w} exceeded 240");
        assert!(nat_w >= min_w);

        // 1440px 宽屏普通笔记：居中截断至 920px，左右各留 260px
        vp.allocate(1440, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), READING_COLUMN_MAX_WIDTH);
        assert_eq!(wide_child.width(), READING_COLUMN_MAX_WIDTH);
        let offset_clamped = wide_child
            .compute_point(&vp, &gtk4::graphene::Point::new(0.0, 0.0))
            .map(|pt| pt.x());
        assert_eq!(offset_clamped, Some(260.0));
        assert_eq!(vp.resize_alloc_count(), 1);

        // 独立 .mmd 模式（None）：铺满可用宽度（1440 - 88 = 1352）
        vp.set_max_content_width(None);
        assert_eq!(vp.max_content_width(), None);
        vp.allocate(1440, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), 1440 - H_PADDING_WIDE * 2);
        assert_eq!(wide_child.width(), 1440 - H_PADDING_WIDE * 2);
        let offset_mmd = wide_child
            .compute_point(&vp, &gtk4::graphene::Point::new(0.0, 0.0))
            .map(|pt| pt.x());
        assert_eq!(offset_mmd, Some(H_PADDING_WIDE as f32));
        assert_eq!(vp.resize_alloc_count(), 2);

        // 切回 920px 限制并缩窄到 700px 窄屏（24px 留白）
        vp.set_max_content_width(Some(READING_COLUMN_MAX_WIDTH));
        vp.set_settled_width(920);
        vp.allocate(700, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), 700 - H_PADDING_NARROW * 2);
        assert_eq!(wide_child.width(), 700 - H_PADDING_NARROW * 2);
        let offset_narrow = wide_child
            .compute_point(&vp, &gtk4::graphene::Point::new(0.0, 0.0))
            .map(|pt| pt.x());
        assert_eq!(offset_narrow, Some(H_PADDING_NARROW as f32));
        assert_eq!(vp.resize_alloc_count(), 3);

        vp.clear_timers();
        vp.set_child(None::<&gtk4::Widget>);
    }

    #[test]
    fn edit_host_syncs_margins_and_preserves_zero_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping edit host test: GTK 初始化失败或非主线程");
            return;
        }
        let host = HortusEditHost::new();
        let edit_view = gtk4::TextView::new();
        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&edit_view)
            .build();
        host.set_child(Some(&scrolled));
        host.set_edit_view(Some(&edit_view));

        host.allocate(1440, 800, -1, None);
        let ctx = glib::MainContext::default();
        while ctx.pending() {
            ctx.iteration(false);
        }
        assert_eq!(edit_view.left_margin(), 260);
        assert_eq!(edit_view.right_margin(), 260);
        assert_eq!(
            scrolled.width() - edit_view.left_margin() - edit_view.right_margin(),
            READING_COLUMN_MAX_WIDTH
        );

        host.allocate(700, 800, -1, None);
        while ctx.pending() {
            ctx.iteration(false);
        }
        assert_eq!(edit_view.left_margin(), H_PADDING_NARROW);
        assert_eq!(edit_view.right_margin(), H_PADDING_NARROW);
        assert_eq!(
            scrolled.width() - edit_view.left_margin() - edit_view.right_margin(),
            700 - H_PADDING_NARROW * 2
        );

        let weak_host = host.downgrade();
        let weak_view = edit_view.downgrade();
        host.clear_timers();
        host.set_child(None::<&gtk4::Widget>);
        scrolled.set_child(None::<&gtk4::Widget>);
        drop(edit_view);
        drop(scrolled);
        drop(host);
        assert!(weak_host.upgrade().is_none(), "HortusEditHost cycle leaked");
        assert!(weak_view.upgrade().is_none(), "TextView cycle leaked");
    }
}
