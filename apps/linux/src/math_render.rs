// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! RaTeX 公式排版场景、缺字度量回测与 Cairo/PangoCairo 原生绘制及 GTK4 控件。

use crate::fonts::{
    current_epoch, ensure_katex_fonts_registered_sync, fallback_font_description,
    katex_font_description,
};
use crate::pangocairo_ffi;
use gtk4::cairo;
use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use hortus_ffi::{MathBlock, MathEngine, MathFailure, MathGlyphMetrics, MathStyle};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;

#[derive(Clone, Debug, Deserialize)]
pub struct MathColor {
    pub foreground: bool,
    pub rgba: [f64; 4],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum MathCommand {
    MoveTo {
        x: f64,
        y: f64,
    },
    LineTo {
        x: f64,
        y: f64,
    },
    CubicTo {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    QuadTo {
        x1: f64,
        y1: f64,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum MathItem {
    Glyph {
        x: f64,
        y: f64,
        scale: f64,
        font: String,
        codepoint: u32,
        fallback: bool,
        color: MathColor,
    },
    Line {
        x: f64,
        y: f64,
        width: f64,
        thickness: f64,
        dashed: bool,
        color: MathColor,
    },
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: MathColor,
    },
    Path {
        x: f64,
        y: f64,
        commands: Vec<MathCommand>,
        fill: bool,
        color: MathColor,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct MathScene {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub items: Vec<MathItem>,
    pub missing_metrics: Vec<u32>,
}

impl MathScene {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 8 || bytes.len() > 32 * 1024 * 1024 {
            return Err("公式载荷长度异常".into());
        }
        let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if version != 1 || len != bytes.len() - 8 {
            return Err("公式载荷协议头无效".into());
        }
        let scene: Self =
            serde_json::from_slice(&bytes[8..]).map_err(|e| format!("公式 JSON 解析失败：{e}"))?;
        if !scene.width.is_finite()
            || !scene.height.is_finite()
            || !scene.depth.is_finite()
            || scene.width < 0.0
            || scene.height < 0.0
            || scene.depth < 0.0
            || scene.width > 4096.0
            || scene.height > 4096.0
            || scene.depth > 4096.0
        {
            return Err("公式几何边界非法".into());
        }
        Ok(scene)
    }
}

#[derive(Clone, Debug)]
pub struct PreparedMath {
    pub scene: MathScene,
    pub em_px: f64,
    pub width_px: f64,
    pub height_px: f64,
    pub ascent_px: f64,
    pub descent_px: f64,
    pub missing_metrics_feedbacks: usize,
    pub estimated_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct PreparedMathBlock {
    pub block: MathBlock,
    pub math: Option<Arc<PreparedMath>>,
    pub failure: Option<String>,
}

struct MathCacheInner {
    engine: Arc<MathEngine>,
    metrics: HashMap<u32, (f64, f64, f64)>,
    entries: HashMap<String, Arc<PreparedMath>>,
    order: VecDeque<String>,
    cached_bytes: usize,
    measured_codepoints: BTreeSet<u32>,
    feedback_rounds: usize,
}

/// 线程安全、有界容量的 RaTeX 公式布局与缺字回测服务，完全在后台工作线程执行。
pub struct MathRenderService {
    inner: Mutex<MathCacheInner>,
}

impl MathRenderService {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MathCacheInner {
                engine: MathEngine::new(),
                metrics: HashMap::new(),
                entries: HashMap::new(),
                order: VecDeque::new(),
                cached_bytes: 0,
                measured_codepoints: BTreeSet::new(),
                feedback_rounds: 0,
            }),
        }
    }

    pub fn body_size_for_mask(base_size: f64, mask: u32) -> f64 {
        let heading = (mask >> 24) & 0xff;
        let factor = match heading {
            1 => 1.6,
            2 => 1.35,
            3 => 1.15,
            _ => 1.0,
        };
        base_size * factor
    }

    pub fn measured_fallback_codepoints(&self) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .measured_codepoints
            .iter()
            .copied()
            .collect()
    }

    pub fn feedback_rounds(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .feedback_rounds
    }

    /// 必须在后台工作线程调用：确保 KaTeX 字体就绪 -> 排版 -> 若缺度量用 Pango 测量并回填 -> 返回缓存结果。
    pub fn prepare_formula(
        &self,
        latex: &str,
        style: MathStyle,
        body_size: f64,
    ) -> Result<Arc<PreparedMath>, String> {
        ensure_katex_fonts_registered_sync()?;
        let em_px = body_size * hortus_ffi::math_typography_scale();
        let style_tag = match style {
            MathStyle::Display => 'D',
            MathStyle::Text => 'T',
        };
        let cache_key = format!("{style_tag}:{:.2}:{latex}", body_size);

        let (engine, existing_metrics) = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hit) = guard.entries.get(&cache_key) {
                return Ok(Arc::clone(hit));
            }
            (Arc::clone(&guard.engine), guard.metrics.clone())
        };

        let bytes = engine
            .render(latex.to_string(), style)
            .map_err(|e| format_math_failure(&e))?;
        let mut scene = MathScene::decode(&bytes)?;
        let mut feedback_count = 0usize;

        if !scene.missing_metrics.is_empty() {
            let mut scales: HashMap<u32, f64> = HashMap::new();
            for item in &scene.items {
                if let MathItem::Glyph {
                    codepoint,
                    scale,
                    fallback: true,
                    ..
                } = item
                {
                    scales.entry(*codepoint).or_insert(*scale);
                }
            }

            let mut to_submit = Vec::new();
            let mut newly_measured = Vec::new();
            for cp in &scene.missing_metrics {
                if let Some(&(width, height, depth)) = existing_metrics.get(cp) {
                    to_submit.push(MathGlyphMetrics {
                        codepoint: *cp,
                        width,
                        height,
                        depth,
                    });
                    continue;
                }
                let glyph_scale = scales.get(cp).copied().unwrap_or(1.0);
                let measured = measure_fallback_glyph(*cp, (em_px * glyph_scale).max(6.0))?;
                newly_measured.push((*cp, measured.width, measured.height, measured.depth));
                to_submit.push(measured);
            }

            if !to_submit.is_empty() {
                engine
                    .set_metrics(to_submit)
                    .map_err(|e| format_math_failure(&e))?;
                feedback_count += 1;
                let bytes2 = engine
                    .render(latex.to_string(), style)
                    .map_err(|e| format_math_failure(&e))?;
                scene = MathScene::decode(&bytes2)?;
            }

            if !newly_measured.is_empty() {
                let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                for (cp, width, height, depth) in newly_measured {
                    guard.measured_codepoints.insert(cp);
                    guard.metrics.insert(cp, (width, height, depth));
                }
                guard.feedback_rounds += 1;
            }
        }

        let estimated_bytes = scene.items.len() * 64 + latex.len();
        let prepared = Arc::new(PreparedMath {
            width_px: scene.width * em_px,
            ascent_px: scene.height * em_px,
            descent_px: scene.depth * em_px,
            height_px: (scene.height + scene.depth) * em_px,
            em_px,
            missing_metrics_feedbacks: feedback_count,
            estimated_bytes,
            scene,
        });

        if estimated_bytes <= MAX_CACHE_BYTES {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.entries.contains_key(&cache_key) {
                while guard.order.len() >= MAX_CACHE_ENTRIES
                    || guard.cached_bytes + estimated_bytes > MAX_CACHE_BYTES
                {
                    if let Some(old_key) = guard.order.pop_front() {
                        if let Some(removed) = guard.entries.remove(&old_key) {
                            guard.cached_bytes =
                                guard.cached_bytes.saturating_sub(removed.estimated_bytes);
                        }
                    } else {
                        break;
                    }
                }
                guard.cached_bytes += estimated_bytes;
                guard.order.push_back(cache_key.clone());
                guard.entries.insert(cache_key, Arc::clone(&prepared));
            }
        }

        Ok(prepared)
    }

    pub fn prepare_blocks(
        &self,
        blocks: &[MathBlock],
        source: &str,
        body_size: f64,
        in_table_cell: bool,
    ) -> Vec<PreparedMathBlock> {
        blocks
            .iter()
            .filter(|b| !b.latex.trim().is_empty())
            .filter_map(|block| {
                let _raw = crate::core::utf16_slice(
                    source,
                    block.source_start as usize,
                    block.source_length as usize,
                )?;
                let size = if in_table_cell {
                    body_size
                } else {
                    Self::body_size_for_mask(body_size, block.mask)
                };
                match self.prepare_formula(&block.latex, block.style, size) {
                    Ok(math) => Some(PreparedMathBlock {
                        block: block.clone(),
                        math: Some(math),
                        failure: None,
                    }),
                    Err(reason) => Some(PreparedMathBlock {
                        block: block.clone(),
                        math: None,
                        failure: Some(reason),
                    }),
                }
            })
            .collect()
    }
}

fn format_math_failure(err: &MathFailure) -> String {
    match err {
        MathFailure::Parse { message } => format!("公式语法错误：{message}"),
        MathFailure::ResourceLimit { message } => format!("公式超出资源限制：{message}"),
        MathFailure::Panic { message } => format!("公式引擎内部异常：{message}"),
        MathFailure::InvalidLayout { message } => format!("公式排版错误：{message}"),
    }
}

/// 在后台线程用 Cairo RecordingSurface + Pango 测量 CJK/Emoji/生僻缺失字形的归一化 `(width, height, depth)`（单位为 `em`）。
fn measure_fallback_glyph(codepoint: u32, size_px: f64) -> Result<MathGlyphMetrics, String> {
    let ch = char::from_u32(codepoint).ok_or_else(|| "无效字形码点".to_string())?;
    let surface = cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None)
        .map_err(|e| format!("创建度量表面失败：{e}"))?;
    let cr = cairo::Context::new(&surface).map_err(|e| format!("创建度量上下文失败：{e}"))?;
    let font_map = pangocairo_ffi::new_font_map();
    let pango_ctx = pangocairo_ffi::create_pango_context(&cr, Some(&font_map));
    let layout = pango::Layout::new(&pango_ctx);
    let desc = fallback_font_description(size_px);
    layout.set_font_description(Some(&desc));
    let mut text = String::new();
    text.push(ch);
    layout.set_text(&text);

    let (ink, logical) = layout.extents();
    let scale = f64::from(pango::SCALE);
    let baseline_px = f64::from(layout.baseline()) / scale;
    let ink_top = f64::from(ink.y()) / scale;
    let ink_bottom = f64::from(ink.y() + ink.height()) / scale;
    let ink_right = f64::from(ink.x() + ink.width()) / scale;
    let adv_px = (f64::from(logical.width()) / scale).max(ink_right);

    let width = (adv_px / size_px).max(0.0);
    let height = ((baseline_px - ink_top) / size_px).max(0.0);
    let depth = ((ink_bottom - baseline_px) / size_px).max(0.0);
    Ok(MathGlyphMetrics {
        codepoint,
        width,
        height,
        depth,
    })
}

/// 渲染已准备好的 `PreparedMath` 到 Cairo 上下文，`origin` 为公式盒子的左上角。
pub fn draw_prepared_math(
    cr: &cairo::Context,
    math: &PreparedMath,
    origin_x: f64,
    origin_y: f64,
    foreground: gdk::RGBA,
) {
    thread_local! {
        static DRAW_FONT_MAP: std::cell::RefCell<Option<(u64, pango::FontMap)>> = const { std::cell::RefCell::new(None) };
    }
    let epoch = current_epoch();
    let font_map = DRAW_FONT_MAP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|(e, _)| *e != epoch) {
            *slot = Some((epoch, pangocairo_ffi::new_font_map()));
        }
        slot.as_ref().unwrap().1.clone()
    });

    let pango_ctx = pangocairo_ffi::create_pango_context(cr, Some(&font_map));
    let em = math.em_px;

    let _ = cr.save();
    cr.translate(origin_x, origin_y);

    for item in &math.scene.items {
        match item {
            MathItem::Glyph {
                x,
                y,
                scale,
                font,
                codepoint,
                fallback,
                color,
            } => {
                let Some(ch) = char::from_u32(*codepoint) else {
                    continue;
                };
                let size_px = (*scale * em).max(1.0);
                let desc = if *fallback {
                    fallback_font_description(size_px)
                } else {
                    katex_font_description(font, size_px)
                        .unwrap_or_else(|| fallback_font_description(size_px))
                };
                let layout = pango::Layout::new(&pango_ctx);
                layout.set_font_description(Some(&desc));
                let mut text = String::new();
                text.push(ch);
                layout.set_text(&text);
                pangocairo_ffi::update_layout(cr, &layout);

                set_math_color(cr, color, foreground);
                if let Some(line) = layout.line(0) {
                    cr.move_to(x * em, y * em);
                    pangocairo_ffi::show_layout_line(cr, &line);
                }
            }
            MathItem::Line {
                x,
                y,
                width,
                thickness,
                dashed,
                color,
            } => {
                set_math_color(cr, color, foreground);
                let thick = (*thickness * em).max(0.75);
                if *dashed {
                    let dash = 4.0 * thick;
                    cr.set_line_width(thick);
                    cr.set_dash(&[dash, dash], 0.0);
                    cr.move_to(x * em, y * em);
                    cr.line_to((x + width) * em, y * em);
                    let _ = cr.stroke();
                    cr.set_dash(&[], 0.0);
                } else {
                    cr.rectangle(x * em, y * em - thick / 2.0, width * em, thick);
                    let _ = cr.fill();
                }
            }
            MathItem::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                set_math_color(cr, color, foreground);
                cr.rectangle(x * em, y * em, width * em, height * em);
                let _ = cr.fill();
            }
            MathItem::Path {
                x,
                y,
                commands,
                fill,
                color,
            } => {
                set_math_color(cr, color, foreground);
                let ox = x * em;
                let oy = y * em;
                let mut cx = ox;
                let mut cy = oy;
                cr.new_path();
                for cmd in commands {
                    match cmd {
                        MathCommand::MoveTo { x, y } => {
                            cx = ox + x * em;
                            cy = oy + y * em;
                            cr.move_to(cx, cy);
                        }
                        MathCommand::LineTo { x, y } => {
                            cx = ox + x * em;
                            cy = oy + y * em;
                            cr.line_to(cx, cy);
                        }
                        MathCommand::CubicTo {
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => {
                            let nx = ox + x * em;
                            let ny = oy + y * em;
                            cr.curve_to(
                                ox + x1 * em,
                                oy + y1 * em,
                                ox + x2 * em,
                                oy + y2 * em,
                                nx,
                                ny,
                            );
                            cx = nx;
                            cy = ny;
                        }
                        MathCommand::QuadTo { x1, y1, x, y } => {
                            let qx = ox + x1 * em;
                            let qy = oy + y1 * em;
                            let nx = ox + x * em;
                            let ny = oy + y * em;
                            cr.curve_to(
                                cx + (2.0 / 3.0) * (qx - cx),
                                cy + (2.0 / 3.0) * (qy - cy),
                                nx + (2.0 / 3.0) * (qx - nx),
                                ny + (2.0 / 3.0) * (qy - ny),
                                nx,
                                ny,
                            );
                            cx = nx;
                            cy = ny;
                        }
                        MathCommand::Close => {
                            cr.close_path();
                        }
                    }
                }
                if *fill {
                    let _ = cr.fill();
                } else {
                    cr.set_line_width(1.5);
                    let _ = cr.stroke();
                }
            }
        }
    }

    let _ = cr.restore();
}

fn set_math_color(cr: &cairo::Context, color: &MathColor, foreground: gdk::RGBA) {
    if color.foreground {
        cr.set_source_rgba(
            f64::from(foreground.red()),
            f64::from(foreground.green()),
            f64::from(foreground.blue()),
            f64::from(foreground.alpha()),
        );
    } else {
        cr.set_source_rgba(color.rgba[0], color.rgba[1], color.rgba[2], color.rgba[3]);
    }
}

/// 构造居中可横向滚动的块级公式控件，失败时局部降级展示原公式文本与原因。
pub fn build_display_math_widget(item: &PreparedMathBlock, max_width: f64) -> gtk4::Widget {
    let Some(ref math) = item.math else {
        return build_math_fallback_widget(
            &item.block.display_source,
            item.failure.as_deref().unwrap_or("公式渲染失败"),
            true,
            max_width,
        );
    };

    let vpad = 8.0;
    let hpad = 12.0;
    let natural_w = (math.width_px + 2.0 * hpad).ceil().max(48.0);
    let natural_h = (math.height_px + 2.0 * vpad).ceil().max(24.0);
    let draw_w = natural_w.max(max_width.max(120.0));

    let drawing = DisplayMathCanvas::new(Arc::clone(math), draw_w, natural_h, hpad, vpad);
    drawing.set_tooltip_text(Some(&item.block.latex));
    drawing.set_hexpand(true);
    drawing.add_css_class("hortus-math-block");
    drawing.upcast()
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
            item.failure.as_deref().unwrap_or("公式语法错误"),
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
    max_width: f64,
) -> gtk4::Widget {
    if block {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        container.add_css_class("hortus-math-fallback");
        container.set_halign(gtk4::Align::Start);
        container.set_size_request((max_width - 16.0).max(240.0).round() as i32, -1);
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
        // GtkLabel 会回报自身基线；使用 `Baseline` 让错误公式与周围正文共享同一基线，
        // 而不是 `BaselineCenter` 造成的整体上浮。
        badge.set_valign(gtk4::Align::Baseline);
        badge.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_error_fallback_uses_baseline_alignment() {
        if gtk4::init().is_err() {
            eprintln!("skipping GTK fallback baseline test: GTK 初始化失败");
            return;
        }
        let widget =
            build_math_fallback_widget("\\unknowninlinecmd{x}", "公式语法错误", false, 0.0);
        assert!(widget.has_css_class("hortus-math-inline-fallback"));
        // GTK 会把 BASELINE 归一化为 BASELINE_FILL（无扩展子控件时二者等价）。
        assert!(matches!(
            widget.valign(),
            gtk4::Align::Baseline | gtk4::Align::BaselineFill
        ));
    }

    #[test]
    fn ratex_display_inline_and_cjk_fallback_render_with_katex_fonts() {
        let service = MathRenderService::new();
        let display = service
            .prepare_formula(
                r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
                MathStyle::Display,
                15.0,
            )
            .expect("display math should render");
        assert!(display.width_px > 40.0);
        assert!(display.height_px > 20.0);
        assert!(!display.scene.items.is_empty());

        let cjk = service
            .prepare_formula(r"\text{能量 } E = mc^2", MathStyle::Text, 15.0)
            .expect("CJK fallback math should render");
        assert!(!cjk.scene.items.is_empty());
        assert!(!service.measured_fallback_codepoints().is_empty());
        assert!(service.feedback_rounds() >= 1);

        let err = service.prepare_formula(r"\frac{1}{", MathStyle::Text, 15.0);
        assert!(err.is_err());
    }
}
