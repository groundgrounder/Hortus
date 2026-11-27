// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 块级与行内公式 GTK4 自定义画布及降级提示控件。

use crate::render::math::{PreparedMath, PreparedMathBlock, draw_prepared_math};
use gtk4::glib;
use gtk4::graphene;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

/// 块级公式控件句柄：支持外层容器直接嵌入垂直阅读流。
#[derive(Clone)]
pub struct DisplayMathBlock {
    root: gtk4::Widget,
}

impl DisplayMathBlock {
    pub fn widget(&self) -> &gtk4::Widget {
        &self.root
    }
}

/// 构造居中可横向滚动的块级公式控件，失败时局部降级展示原公式文本与原因。
pub fn build_display_math_widget(item: &PreparedMathBlock, max_width: f64) -> DisplayMathBlock {
    let Some(ref math) = item.math else {
        return DisplayMathBlock {
            root: build_math_fallback_widget(
                &item.block.display_source,
                item.failure
                    .as_deref()
                    .unwrap_or(crate::i18n::tr(crate::i18n::Key::MathRenderFailed)),
                true,
                max_width,
            ),
        };
    };

    let vpad = 8.0;
    let hpad = 12.0;
    let natural_w = (math.width_px + 2.0 * hpad).ceil().max(48.0);
    let natural_h = (math.height_px + 2.0 * vpad).ceil().max(24.0);

    let drawing = DisplayMathCanvas::new(Arc::clone(math), natural_w, natural_h, hpad, vpad);
    drawing.set_tooltip_text(Some(&item.block.latex));
    drawing.set_hexpand(true);
    drawing.set_halign(gtk4::Align::Fill);
    drawing.add_css_class("hortus-math-block");

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .min_content_width(120)
        .hexpand(true)
        .halign(gtk4::Align::Fill)
        .child(&drawing)
        .build();
    crate::platform::viewport::install_vertical_scroll_passthrough(&scroll);
    DisplayMathBlock {
        root: scroll.upcast(),
    }
}

mod display_imp {
    use super::*;

    #[derive(Default)]
    pub struct DisplayMathCanvas {
        pub math: RefCell<Option<Arc<PreparedMath>>>,
        pub natural_width: Cell<f64>,
        pub natural_height: Cell<i32>,
        pub hpad: Cell<f64>,
        pub vpad: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DisplayMathCanvas {
        const NAME: &'static str = "HortusDisplayMathCanvas";
        type Type = super::DisplayMathCanvas;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for DisplayMathCanvas {}

    impl WidgetImpl for DisplayMathCanvas {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                // 最小宽度固定为 120：块级公式的自然宽度只影响自然尺寸，窄窗口时可收缩。
                gtk4::Orientation::Horizontal => (
                    120,
                    self.natural_width.get().ceil().max(120.0) as i32,
                    -1,
                    -1,
                ),
                _ => {
                    let height = self.natural_height.get().max(24);
                    (height, height, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let guard = self.math.borrow();
            let Some(ref math) = *guard else { return };
            let widget = self.obj();
            let w = widget.width().max(1) as f32;
            let h = widget.height().max(1) as f32;
            let rect = graphene::Rect::new(0.0, 0.0, w, h);
            let cr = snapshot.append_cairo(&rect);
            let fg = widget.color();
            let ox = ((f64::from(w) - math.width_px) / 2.0).max(self.hpad.get());
            draw_prepared_math(&cr, math, ox, self.vpad.get(), fg);
        }
    }
}

glib::wrapper! {
    pub struct DisplayMathCanvas(ObjectSubclass<display_imp::DisplayMathCanvas>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl DisplayMathCanvas {
    pub fn new(
        math: Arc<PreparedMath>,
        natural_width: f64,
        natural_height: f64,
        hpad: f64,
        vpad: f64,
    ) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().math.borrow_mut() = Some(math);
        obj.imp().natural_width.set(natural_width);
        obj.imp().natural_height.set(natural_height.ceil() as i32);
        obj.imp().hpad.set(hpad);
        obj.imp().vpad.set(vpad);
        obj
    }
}

mod inline_imp {
    use super::*;

    #[derive(Default)]
    pub struct InlineMathCanvas {
        pub math: RefCell<Option<Arc<PreparedMath>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for InlineMathCanvas {
        const NAME: &'static str = "HortusInlineMathCanvas";
        type Type = super::InlineMathCanvas;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for InlineMathCanvas {}

    impl WidgetImpl for InlineMathCanvas {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let guard = self.math.borrow();
            let Some(ref m) = *guard else {
                return (8, 8, -1, -1);
            };
            match orientation {
                gtk4::Orientation::Horizontal => {
                    let w = m.width_px.ceil().max(4.0) as i32;
                    (w, w, -1, -1)
                }
                _ => {
                    let h = m.height_px.ceil().max(12.0) as i32;
                    let baseline = (m.ascent_px.round() as i32).clamp(1, h);
                    (h, h, baseline, baseline)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let guard = self.math.borrow();
            let Some(ref m) = *guard else { return };
            let widget = self.obj();
            let w = widget.width().max(1) as f32;
            let h = widget.height().max(1) as f32;
            let rect = graphene::Rect::new(0.0, 0.0, w, h);
            let cr = snapshot.append_cairo(&rect);
            let fg = widget.color();
            draw_prepared_math(&cr, m, 0.0, 0.0, fg);
        }
    }
}

glib::wrapper! {
    pub struct InlineMathCanvas(ObjectSubclass<inline_imp::InlineMathCanvas>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl InlineMathCanvas {
    pub fn new(math: Arc<PreparedMath>) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().math.borrow_mut() = Some(math);
        obj
    }
}

/// 构造与正文精确基线对齐的行内公式控件（通过 `WidgetImpl::measure` 回报上伸基线）。
pub fn build_inline_math_widget(item: &PreparedMathBlock) -> gtk4::Widget {
    let Some(ref math) = item.math else {
        return build_math_fallback_widget(
            &item.block.display_source,
            item.failure
                .as_deref()
                .unwrap_or(crate::i18n::tr(crate::i18n::Key::MathSyntaxInvalid)),
            false,
            0.0,
        );
    };

    let canvas = InlineMathCanvas::new(Arc::clone(math));
    canvas.set_valign(gtk4::Align::BaselineFill);
    canvas.set_tooltip_text(Some(&item.block.latex));
    canvas.add_css_class("hortus-math-inline");
    canvas.upcast()
}

pub fn build_math_fallback_widget(
    source: &str,
    reason: &str,
    block: bool,
    _max_width: f64,
) -> gtk4::Widget {
    if block {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        container.add_css_class("hortus-math-fallback");
        container.set_halign(gtk4::Align::Fill);
        container.set_hexpand(true);
        container.set_size_request(120, -1);
        let msg = gtk4::Label::new(Some(reason));
        msg.add_css_class("dim-label");
        msg.add_css_class("caption");
        msg.set_halign(gtk4::Align::Start);
        msg.set_wrap(true);
        let code = gtk4::Label::new(Some(source.trim()));
        code.add_css_class("monospace");
        code.set_halign(gtk4::Align::Start);
        code.set_selectable(true);
        code.set_wrap(true);
        container.append(&msg);
        container.append(&code);
        container.upcast()
    } else {
        let badge = gtk4::Label::new(Some(source.trim()));
        badge.add_css_class("hortus-math-inline-fallback");
        badge.add_css_class("monospace");
        badge.set_tooltip_text(Some(reason));
        badge.set_valign(gtk4::Align::Baseline);
        badge.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_error_fallback_uses_baseline_alignment() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping GTK fallback baseline test: GTK 初始化失败或非主线程");
            return;
        }
        let widget =
            build_math_fallback_widget("\\unknowninlinecmd{x}", "公式语法错误", false, 0.0);
        assert!(widget.has_css_class("hortus-math-inline-fallback"));
        assert!(matches!(
            widget.valign(),
            gtk4::Align::Baseline | gtk4::Align::BaselineFill
        ));
    }
}
