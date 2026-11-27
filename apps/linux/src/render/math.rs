// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! RaTeX 公式排版场景、缺字度量回测与 Cairo/PangoCairo 原生绘制。

use crate::platform::fonts::{
    current_epoch, ensure_katex_fonts_registered_sync, fallback_font_description,
    katex_font_description,
};
use crate::platform::pangocairo;
use gtk4::cairo;
use gtk4::gdk;
use gtk4::pango;
use hortus_ffi::{MathBlock, MathEngine, MathFailure, MathGlyphMetrics, MathStyle};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub use crate::render::math_widgets::build_inline_math_widget;

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
            return Err(crate::i18n::fmt1(
                crate::i18n::Key::MathInternalErr,
                "length",
            ));
        }
        let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if version != 1 || len != bytes.len() - 8 {
            return Err(crate::i18n::fmt1(
                crate::i18n::Key::MathInternalErr,
                "header",
            ));
        }
        let scene: Self = serde_json::from_slice(&bytes[8..])
            .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::MathInternalErr, e))?;
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
            return Err(crate::i18n::fmt1(
                crate::i18n::Key::MathInternalErr,
                "bounds",
            ));
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

impl Default for MathRenderService {
    fn default() -> Self {
        Self::new()
    }
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

    #[cfg(feature = "automation")]
    pub fn measured_fallback_codepoints(&self) -> Vec<u32> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .measured_codepoints
            .iter()
            .copied()
            .collect()
    }

    #[cfg(feature = "automation")]
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
        body_size: f64,
        in_table_cell: bool,
    ) -> Vec<PreparedMathBlock> {
        blocks
            .iter()
            .filter(|b| !b.latex.trim().is_empty())
            .map(|block| {
                let size = if in_table_cell {
                    body_size
                } else {
                    Self::body_size_for_mask(body_size, block.mask)
                };
                match self.prepare_formula(&block.latex, block.style, size) {
                    Ok(math) => PreparedMathBlock {
                        block: block.clone(),
                        math: Some(math),
                        failure: None,
                    },
                    Err(reason) => PreparedMathBlock {
                        block: block.clone(),
                        math: None,
                        failure: Some(reason),
                    },
                }
            })
            .collect()
    }
}

fn format_math_failure(err: &MathFailure) -> String {
    match err {
        MathFailure::Parse { message } => {
            crate::i18n::fmt1(crate::i18n::Key::MathParseErr, message)
        }
        MathFailure::ResourceLimit { message } => {
            crate::i18n::fmt1(crate::i18n::Key::MathResourceErr, message)
        }
        MathFailure::Panic { message } => {
            crate::i18n::fmt1(crate::i18n::Key::MathInternalErr, message)
        }
        MathFailure::InvalidLayout { message } => {
            crate::i18n::fmt1(crate::i18n::Key::MathLayoutErr, message)
        }
    }
}

/// 在后台线程用 Cairo RecordingSurface + Pango 测量 CJK/Emoji/生僻缺失字形的归一化 `(width, height, depth)`（单位为 `em`）。
fn measure_fallback_glyph(codepoint: u32, size_px: f64) -> Result<MathGlyphMetrics, String> {
    let ch = char::from_u32(codepoint)
        .ok_or_else(|| crate::i18n::fmt1(crate::i18n::Key::MathInternalErr, codepoint))?;
    let surface = cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None)
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::MathInternalErr, e))?;
    let cr = cairo::Context::new(&surface)
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::MathInternalErr, e))?;
    let font_map = pangocairo::new_font_map();
    let pango_ctx = pangocairo::create_pango_context(&cr, Some(&font_map));
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
            *slot = Some((epoch, pangocairo::new_font_map()));
        }
        slot.as_ref().unwrap().1.clone()
    });

    let pango_ctx = pangocairo::create_pango_context(cr, Some(&font_map));
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
                pangocairo::update_layout(cr, &layout);

                set_math_color(cr, color, foreground);
                if let Some(line) = layout.line(0) {
                    cr.move_to(x * em, y * em);
                    pangocairo::show_layout_line(cr, &line);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!service.inner.lock().unwrap().measured_codepoints.is_empty());
        assert!(service.inner.lock().unwrap().feedback_rounds >= 1);

        let err = service.prepare_formula(r"\frac{1}{", MathStyle::Text, 15.0);
        assert!(err.is_err());
    }
}
