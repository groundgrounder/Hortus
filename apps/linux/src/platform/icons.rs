// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 底栏开关专属 GTK4 原生 `SymbolicPaintable` 符号（与左右侧栏 symbolic 图标同族）。

use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

const ICON_SIZE: i32 = 16;

fn draw_bottom_panel_icon(snapshot: &gdk::Snapshot, width: f64, height: f64, fg: gdk::RGBA) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let Some(snapshot) = snapshot.downcast_ref::<gtk4::Snapshot>() else {
        return;
    };

    snapshot.save();
    snapshot.scale((width / 16.0) as f32, (height / 16.0) as f32);

    // 外框：x ∈ [0, 16], y ∈ [1, 15]，外圆角 r = 3，边框宽 2px（内腔 x ∈ [2, 14], y ∈ [3, 13]，内圆角 r = 1）
    let outer = gsk::RoundedRect::from_rect(graphene::Rect::new(0.0, 1.0, 16.0, 14.0), 3.0);
    snapshot.append_border(&outer, &[2.0, 2.0, 2.0, 2.0], &[fg, fg, fg, fg]);

    // 水平分隔线：x ∈ [2, 14], y ∈ [9, 10]（高 1px）
    snapshot.append_color(&fg, &graphene::Rect::new(2.0, 9.0, 12.0, 1.0));

    // 底部面板填充区：x ∈ [2, 14], y ∈ [10, 13]（底部两角圆角 r = 1，前景色 alpha * 0.35）
    let bottom_bounds = graphene::Rect::new(2.0, 10.0, 12.0, 3.0);
    let bottom_rounded = gsk::RoundedRect::new(
        bottom_bounds,
        graphene::Size::zero(),
        graphene::Size::zero(),
        graphene::Size::new(1.0, 1.0),
        graphene::Size::new(1.0, 1.0),
    );
    let fill = gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), fg.alpha() * 0.35);
    snapshot.push_rounded_clip(&bottom_rounded);
    snapshot.append_color(&fill, &bottom_bounds);
    snapshot.pop();

    snapshot.restore();
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BottomPanelSymbolicIcon;

    #[glib::object_subclass]
    impl ObjectSubclass for BottomPanelSymbolicIcon {
        const NAME: &'static str = "HortusBottomPanelSymbolicIcon";
        type Type = super::BottomPanelSymbolicIcon;
        type Interfaces = (gdk::Paintable, gtk4::SymbolicPaintable);
    }

    impl ObjectImpl for BottomPanelSymbolicIcon {}

    impl PaintableImpl for BottomPanelSymbolicIcon {
        fn flags(&self) -> gdk::PaintableFlags {
            gdk::PaintableFlags::STATIC_SIZE
        }

        fn intrinsic_width(&self) -> i32 {
            ICON_SIZE
        }

        fn intrinsic_height(&self) -> i32 {
            ICON_SIZE
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            draw_bottom_panel_icon(snapshot, width, height, gdk::RGBA::BLACK);
        }
    }

    impl SymbolicPaintableImpl for BottomPanelSymbolicIcon {
        fn snapshot_symbolic(
            &self,
            snapshot: &gdk::Snapshot,
            width: f64,
            height: f64,
            colors: &[gdk::RGBA],
        ) {
            let fg = colors.first().copied().unwrap_or(gdk::RGBA::BLACK);
            draw_bottom_panel_icon(snapshot, width, height, fg);
        }
    }
}

glib::wrapper! {
    pub struct BottomPanelSymbolicIcon(ObjectSubclass<imp::BottomPanelSymbolicIcon>)
        @implements gdk::Paintable, gtk4::SymbolicPaintable;
}

impl Default for BottomPanelSymbolicIcon {
    fn default() -> Self {
        Self::new()
    }
}

impl BottomPanelSymbolicIcon {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

/// 创建与左/右侧栏开关同族对齐的底部面板切换按钮。
pub fn create_bottom_panel_toggle(tooltip: &str) -> gtk4::ToggleButton {
    let paintable = BottomPanelSymbolicIcon::new();
    let image = gtk4::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(ICON_SIZE);
    let button = gtk4::ToggleButton::builder()
        .child(&image)
        .tooltip_text(tooltip)
        .active(false)
        .build();
    button.add_css_class("image-button");
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_panel_symbolic_icon_dimensions_and_toggle_widget() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping BottomPanelSymbolicIcon test: GTK 初始化失败或非主线程");
            return;
        }
        let icon = BottomPanelSymbolicIcon::new();
        assert_eq!(icon.intrinsic_width(), 16);
        assert_eq!(icon.intrinsic_height(), 16);

        let toggle = create_bottom_panel_toggle("Toggle Bottom Panel");
        assert!(!toggle.is_active());
        assert!(toggle.has_css_class("image-button"));
        assert!(toggle.icon_name().is_none());
        let img = toggle
            .child()
            .and_downcast::<gtk4::Image>()
            .expect("bottom toggle child should be gtk4::Image");
        assert_eq!(img.pixel_size(), 16);
        assert!(img.icon_name().is_none());
        assert!(img.paintable().is_some());

        // 验证 snapshot_symbolic 与 snapshot 兜底绘制路径不崩溃
        let snapshot = gtk4::Snapshot::new();
        let fg = gdk::RGBA::new(0.2, 0.2, 0.2, 1.0);
        icon.snapshot_symbolic(snapshot.upcast_ref::<gdk::Snapshot>(), 16.0, 16.0, &[fg]);
        icon.snapshot(snapshot.upcast_ref::<gdk::Snapshot>(), 16.0, 16.0);
        assert!(snapshot.to_node().is_some());
    }
}
