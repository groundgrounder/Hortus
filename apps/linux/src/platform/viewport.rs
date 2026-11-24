// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! GTK4 真宽度感知阅读容器：`WidgetImpl::size_allocate` + 空闲首对齐 + 90ms 防抖重排。

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const H_PADDING_BREAKPOINT: i32 = 820;
const H_PADDING_NARROW: i32 = 20;
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

type WidthCallback = Rc<dyn Fn(i32)>;

mod imp {
    use super::*;

    pub struct HortusReadingViewport {
        pub child: RefCell<Option<gtk4::Widget>>,
        pub horizontal_padding: Cell<i32>,
        pub last_allocated_w: Cell<i32>,
        pub settled_w: Cell<i32>,
        pub resize_alloc_count: Cell<u32>,
        pub reflow_count: Cell<u32>,
        pub idle_source: RefCell<Option<glib::SourceId>>,
        pub debounce_source: RefCell<Option<glib::SourceId>>,
        pub on_relayout: RefCell<Option<WidthCallback>>,
    }

    impl Default for HortusReadingViewport {
        fn default() -> Self {
            Self {
                child: RefCell::new(None),
                horizontal_padding: Cell::new(H_PADDING_WIDE * 2),
                last_allocated_w: Cell::new(0),
                settled_w: Cell::new(0),
                resize_alloc_count: Cell::new(0),
                reflow_count: Cell::new(0),
                idle_source: RefCell::new(None),
                debounce_source: RefCell::new(None),
                on_relayout: RefCell::new(None),
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
            if let Some(src) = self.idle_source.borrow_mut().take() {
                src.remove();
            }
            if let Some(src) = self.debounce_source.borrow_mut().take() {
                src.remove();
            }
            *self.on_relayout.borrow_mut() = None;
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
                        let pad = responsive_horizontal_padding(for_size);
                        (for_size - pad).max(1)
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
            let child = self.child.borrow().clone();
            if let Some(child) = child {
                child.allocate(width, height, baseline, None);
            }
            let avail_w = (width - total_pad).max(MIN_VIEWPORT_WIDTH);
            if self.last_allocated_w.get() != avail_w {
                self.last_allocated_w.set(avail_w);
                self.resize_alloc_count
                    .set(self.resize_alloc_count.get().saturating_add(1));
                self.schedule_width_callback(avail_w);
            }
        }
    }

    impl HortusReadingViewport {
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
        if let Some(src) = imp.idle_source.borrow_mut().take() {
            src.remove();
        }
        if let Some(src) = imp.debounce_source.borrow_mut().take() {
            src.remove();
        }
        *imp.on_relayout.borrow_mut() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_measure_caps_min_width_and_tracks_allocations() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping viewport test: GTK 初始化失败或非主线程");
            return;
        }
        let vp = HortusReadingViewport::new();
        let wide_child = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        wide_child.set_size_request(900, 120);
        vp.set_child(Some(&wide_child));

        let (min_w, nat_w, _, _) = vp.measure(gtk4::Orientation::Horizontal, -1);
        assert!(min_w <= 240, "viewport min_w={min_w} exceeded 240");
        assert!(nat_w >= min_w);

        vp.allocate(1200, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), 1200 - H_PADDING_WIDE * 2);
        assert_eq!(vp.resize_alloc_count(), 1);

        vp.set_settled_width(1200 - H_PADDING_WIDE * 2);
        vp.allocate(900, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), 900 - H_PADDING_WIDE * 2);
        assert_eq!(vp.resize_alloc_count(), 2);

        vp.allocate(700, 800, -1, None);
        assert_eq!(vp.last_allocated_width(), 700 - H_PADDING_NARROW * 2);
        assert_eq!(vp.resize_alloc_count(), 3);

        vp.clear_timers();
        vp.set_child(None::<&gtk4::Widget>);
    }
}
