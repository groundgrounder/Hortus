// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Mermaid 图表的 Cairo + PangoCairo 原生矢量绘制、交互命中与 SVG/PNG/PDF 导出。

use crate::platform::pangocairo;
use gtk4::cairo;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use hortus_ffi::{
    DiagramEngine, DiagramFailure, DiagramIdentity, DiagramPrevious, SceneColor, SceneData,
    SceneHit, ScenePoint, ScenePrimitive, SceneRect, SceneText,
};
use libadwaita as adw;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct DiagramHighlight {
    pub rect: SceneRect,
    pub fill: SceneColor,
    pub border: SceneColor,
    pub border_width: f64,
}

#[derive(Clone)]
pub struct RenderedDiagram {
    pub ordinal: u32,
    pub source_start: u64,
    pub source_end: u64,
    pub scene: Option<SceneData>,
    pub failure: Option<String>,
    pub collapsed: Vec<String>,
    pub previous: Option<DiagramPrevious>,
}

pub fn format_diagram_failure(err: &DiagramFailure) -> String {
    match err {
        DiagramFailure::Unsupported { diagram_type } => {
            if diagram_type.trim().is_empty() {
                crate::i18n::tr(crate::i18n::Key::DiagramUnsupported).into()
            } else {
                crate::i18n::fmt1(crate::i18n::Key::DiagramUnsupportedTyped, diagram_type)
            }
        }
        DiagramFailure::UnsupportedConstruct { line, kind, .. } => {
            crate::i18n::fmt2(crate::i18n::Key::DiagramUnsupportedConstruct, line, kind)
        }
        DiagramFailure::NotATree { multi_parent } => {
            if multi_parent.is_empty() {
                crate::i18n::tr(crate::i18n::Key::DiagramNotTree).into()
            } else {
                crate::i18n::fmt1(
                    crate::i18n::Key::DiagramNotTreeNodes,
                    multi_parent.join(", "),
                )
            }
        }
        DiagramFailure::Syntax { line, kind } => {
            crate::i18n::fmt2(crate::i18n::Key::DiagramSyntaxErr, line, kind)
        }
        DiagramFailure::Engine { kind } => {
            crate::i18n::fmt1(crate::i18n::Key::DiagramEngineErr, kind)
        }
    }
}

pub fn render_block_with_engine(
    engine: &DiagramEngine,
    source: &str,
    mmd: bool,
    width: f64,
    collapsed: &[String],
    identity: DiagramIdentity,
    previous: Option<DiagramPrevious>,
) -> Result<SceneData, DiagramFailure> {
    engine.render_request(
        source.to_string(),
        mmd,
        width.max(120.0),
        collapsed.to_vec(),
        identity,
        previous,
        Vec::new(),
    )
}

/// 将 `SceneData` 矢量图元与文字基线精确提交到 Cairo 上下文。
pub fn draw_scene(
    cr: &cairo::Context,
    scene: &SceneData,
    font_map: Option<&pango::FontMap>,
    highlight: Option<&DiagramHighlight>,
) {
    let pango_ctx = pangocairo::create_pango_context(cr, font_map);

    for primitive in &scene.primitives {
        match primitive {
            ScenePrimitive::RoundedRect {
                rect,
                radius,
                color,
            } => {
                set_color(cr, color);
                rounded_rect_path(cr, rect.x, rect.y, rect.width, rect.height, *radius);
                let _ = cr.fill();
            }
            ScenePrimitive::Line {
                from,
                to,
                width,
                color,
            } => {
                set_color(cr, color);
                cr.set_line_width(*width);
                cr.set_dash(&[], 0.0);
                cr.move_to(from.x, from.y);
                cr.line_to(to.x, to.y);
                let _ = cr.stroke();
            }
            ScenePrimitive::GraphShape {
                rect,
                shape,
                outline,
                details,
                fill,
                stroke,
                stroke_width,
            } => {
                cr.set_dash(&[], 0.0);
                cr.set_line_width(*stroke_width);
                if outline.len() == 2 {
                    set_color(cr, stroke);
                    cr.move_to(outline[0].x, outline[0].y);
                    cr.line_to(outline[1].x, outline[1].y);
                    let _ = cr.stroke();
                } else {
                    if outline.is_empty() && (shape == "Rectangle" || shape == "Rounded") {
                        let radius = (rect.height / 2.0).min(10.0);
                        rounded_rect_path(cr, rect.x, rect.y, rect.width, rect.height, radius);
                    } else if outline.is_empty() && shape == "Stadium" {
                        let radius = rect.width.min(rect.height) / 2.0;
                        rounded_rect_path(cr, rect.x, rect.y, rect.width, rect.height, radius);
                    } else if !outline.is_empty() {
                        polygon_path(cr, outline, true);
                    }
                    if fill.a > 0.0 {
                        set_color(cr, fill);
                        let _ = cr.fill_preserve();
                    }
                    if stroke.a > 0.0 && *stroke_width > 0.0 {
                        set_color(cr, stroke);
                        let _ = cr.stroke();
                    } else {
                        cr.new_path();
                    }
                }
                if !details.is_empty() && stroke.a > 0.0 && *stroke_width > 0.0 {
                    set_color(cr, stroke);
                    for poly in details {
                        if poly.len() >= 2 {
                            polygon_path(cr, poly, false);
                            let _ = cr.stroke();
                        }
                    }
                }
            }
            ScenePrimitive::LabelBackground { rect, color, .. } => {
                set_color(cr, color);
                rounded_rect_path(cr, rect.x, rect.y, rect.width, rect.height, 4.0);
                let _ = cr.fill();
            }
            ScenePrimitive::GraphPath {
                corner_radius,
                points,
                line,
                start,
                end,
                color,
                width,
                ..
            } => {
                if points.len() < 2 {
                    continue;
                }
                set_color(cr, color);
                cr.set_line_width(*width);
                cr.set_line_join(cairo::LineJoin::Round);
                cr.set_line_cap(cairo::LineCap::Round);
                if line == "Dotted" {
                    cr.set_dash(&[4.0, 4.0], 0.0);
                } else {
                    cr.set_dash(&[], 0.0);
                }
                rounded_polyline_path(cr, points, *corner_radius);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                draw_marker(cr, start, points, true);
                draw_marker(cr, end, points, false);
            }
            ScenePrimitive::Text { .. } => {}
        }
    }

    if let Some(hl) = highlight {
        set_color(cr, &hl.fill);
        rounded_rect_path(cr, hl.rect.x, hl.rect.y, hl.rect.width, hl.rect.height, 7.0);
        let _ = cr.fill_preserve();
        set_color(cr, &hl.border);
        cr.set_line_width(hl.border_width);
        let _ = cr.stroke();
    }

    for primitive in &scene.primitives {
        if let ScenePrimitive::Text { span } = primitive {
            draw_scene_text(cr, &pango_ctx, span);
        }
    }
}

fn draw_scene_text(cr: &cairo::Context, pango_ctx: &pango::Context, span: &SceneText) {
    if span.text.is_empty() {
        return;
    }
    let layout = pango::Layout::new(pango_ctx);
    let mut desc = pango::FontDescription::new();
    let family = if span.font_family.trim().is_empty() {
        "sans-serif"
    } else {
        span.font_family.as_str()
    };
    desc.set_family(family);
    desc.set_weight(pango_weight(span.font_weight));
    desc.set_absolute_size(span.font_size.max(1.0) * f64::from(pango::SCALE));
    layout.set_font_description(Some(&desc));
    layout.set_text(&span.text);
    pangocairo::update_layout(cr, &layout);

    set_color(cr, &span.color);
    if let Some(line) = layout.line(0) {
        cr.move_to(span.baseline.x, span.baseline.y);
        pangocairo::show_layout_line(cr, &line);
    } else {
        cr.move_to(span.baseline.x, span.baseline.y - span.font_size);
        pangocairo::show_layout(cr, &layout);
    }
}

fn pango_weight(w: u16) -> pango::Weight {
    match w {
        0..=349 => pango::Weight::Light,
        350..=449 => pango::Weight::Normal,
        450..=549 => pango::Weight::Medium,
        550..=649 => pango::Weight::Semibold,
        650..=799 => pango::Weight::Bold,
        _ => pango::Weight::Heavy,
    }
}

fn set_color(cr: &cairo::Context, color: &SceneColor) {
    cr.set_source_rgba(color.r, color.g, color.b, color.a);
}

fn rounded_rect_path(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.01 {
        cr.rectangle(x, y, w, h);
        return;
    }
    let right = x + w;
    let bottom = y + h;
    cr.new_sub_path();
    cr.arc(right - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(right - r, bottom - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        bottom - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

fn polygon_path(cr: &cairo::Context, points: &[ScenePoint], closed: bool) {
    let Some(first) = points.first() else {
        return;
    };
    cr.move_to(first.x, first.y);
    for point in &points[1..] {
        cr.line_to(point.x, point.y);
    }
    if closed {
        cr.close_path();
    }
}

fn rounded_polyline_path(cr: &cairo::Context, points: &[ScenePoint], radius: f64) {
    let Some(first) = points.first() else {
        return;
    };
    cr.move_to(first.x, first.y);
    if points.len() > 2 {
        for i in 1..(points.len() - 1) {
            let a = &points[i - 1];
            let b = &points[i];
            let c = &points[i + 1];
            let incoming = (b.x - a.x).hypot(b.y - a.y);
            let outgoing = (c.x - b.x).hypot(c.y - b.y);
            let r = radius.min(incoming / 2.0).min(outgoing / 2.0);
            if r > 0.001 && incoming > 0.0 && outgoing > 0.0 {
                let p1x = b.x + (a.x - b.x) * r / incoming;
                let p1y = b.y + (a.y - b.y) * r / incoming;
                let p2x = b.x + (c.x - b.x) * r / outgoing;
                let p2y = b.y + (c.y - b.y) * r / outgoing;
                cr.line_to(p1x, p1y);
                let cp1x = p1x + (2.0 / 3.0) * (b.x - p1x);
                let cp1y = p1y + (2.0 / 3.0) * (b.y - p1y);
                let cp2x = p2x + (2.0 / 3.0) * (b.x - p2x);
                let cp2y = p2y + (2.0 / 3.0) * (b.y - p2y);
                cr.curve_to(cp1x, cp1y, cp2x, cp2y, p2x, p2y);
            } else {
                cr.line_to(b.x, b.y);
            }
        }
    }
    if let Some(last) = points.last() {
        cr.line_to(last.x, last.y);
    }
}

fn draw_marker(cr: &cairo::Context, kind: &str, points: &[ScenePoint], at_start: bool) {
    if kind == "None" || points.len() < 2 {
        return;
    }
    let (tip, next) = if at_start {
        (&points[0], &points[1])
    } else {
        (&points[points.len() - 1], &points[points.len() - 2])
    };
    let len = (tip.x - next.x).hypot(tip.y - next.y);
    if len <= 0.001 {
        return;
    }
    let ux = (tip.x - next.x) / len;
    let uy = (tip.y - next.y) / len;
    match kind {
        "Arrow" => {
            cr.move_to(tip.x, tip.y);
            cr.line_to(tip.x - ux * 9.0 - uy * 4.0, tip.y - uy * 9.0 + ux * 4.0);
            cr.line_to(tip.x - ux * 9.0 + uy * 4.0, tip.y - uy * 9.0 - ux * 4.0);
            cr.close_path();
            let _ = cr.fill();
        }
        "Circle" => {
            let cx = tip.x - ux * 4.25;
            let cy = tip.y - uy * 4.25;
            cr.set_line_width(1.5);
            cr.new_sub_path();
            cr.arc(cx, cy, 3.5, 0.0, std::f64::consts::TAU);
            let _ = cr.fill_preserve();
            let _ = cr.stroke();
        }
        "Cross" => {
            let cx = tip.x - ux * 5.0;
            let cy = tip.y - uy * 5.0;
            cr.set_line_width(1.5);
            for sign in [-1.0, 1.0] {
                cr.move_to(
                    cx - ux * 3.0 - uy * 3.0 * sign,
                    cy - uy * 3.0 + ux * 3.0 * sign,
                );
                cr.line_to(
                    cx + ux * 3.0 + uy * 3.0 * sign,
                    cy + uy * 3.0 - ux * 3.0 * sign,
                );
            }
            let _ = cr.stroke();
        }
        _ => {}
    }
}

pub fn hit_test(scene: &SceneData, x: f64, y: f64) -> Option<&SceneHit> {
    scene.hit_regions.iter().find(|hit| {
        x >= hit.rect.x
            && x <= hit.rect.x + hit.rect.width
            && y >= hit.rect.y
            && y <= hit.rect.y + hit.rect.height
    })
}

/// 将场景光栅化为 2x 高清 PNG 字节，与屏幕绘制共用一套 Cairo/PangoCairo 路径。
pub fn render_scene_to_png_bytes(scene: &SceneData, scale: f64) -> Result<Vec<u8>, String> {
    let scale = scale.clamp(1.0, 4.0);
    let px_w = (scene.width * scale).ceil().max(1.0) as i32;
    let px_h = (scene.height * scale).ceil().max(1.0) as i32;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, px_w, px_h)
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e))?;
    {
        let cr = cairo::Context::new(&surface)
            .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e))?;
        cr.scale(scale, scale);
        draw_scene(&cr, scene, None, None);
    }
    surface.flush();
    let mut out = Cursor::new(Vec::new());
    surface
        .write_to_png(&mut out)
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e))?;
    Ok(out.into_inner())
}

/// 将场景矢量输出为 PDF 字节（保留矢量文字与曲线，无栅格化降质）。
pub fn render_scene_to_pdf_bytes(scene: &SceneData) -> Result<Vec<u8>, String> {
    let w = scene.width.max(1.0);
    let h = scene.height.max(1.0);
    let surface = cairo::PdfSurface::for_stream(w, h, Vec::<u8>::new())
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e))?;
    {
        let cr = cairo::Context::new(&surface)
            .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e))?;
        draw_scene(&cr, scene, None, None);
        let _ = cr.show_page();
    }
    let stream = surface
        .finish_output_stream()
        .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, e.error))?;
    stream
        .downcast::<Vec<u8>>()
        .map(|boxed| *boxed)
        .map_err(|_| crate::i18n::fmt1(crate::i18n::Key::ExportImageFailed, "pdf stream"))
}

#[derive(Clone, Copy)]
pub enum DiagramExportKind {
    Svg,
    Png,
    Pdf,
}

impl DiagramExportKind {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

pub struct DiagramExportMeta {
    pub note_path: String,
    pub ordinal: u32,
    pub source: String,
    pub mmd: bool,
    pub collapsed: Vec<String>,
    pub scene: SceneData,
}

pub fn default_export_filename(note: &str, ordinal: u32, kind: DiagramExportKind) -> String {
    let base = Path::new(note)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Diagram");
    format!("{base}-diagram-{ordinal}.{}", kind.ext())
}

pub fn export_diagram_to_file(
    engine: Arc<DiagramEngine>,
    meta: DiagramExportMeta,
    kind: DiagramExportKind,
    dest: PathBuf,
    on_done: impl FnOnce(Result<PathBuf, String>) + 'static,
) {
    let dark = adw::StyleManager::default().is_dark();
    glib::MainContext::default().spawn_local(async move {
        let res = gio::spawn_blocking(move || match kind {
            DiagramExportKind::Svg => {
                let svg = engine
                    .render_svg(
                        meta.source,
                        meta.mmd,
                        meta.ordinal,
                        meta.scene.width.max(120.0),
                        dark,
                        meta.collapsed,
                    )
                    .map_err(|e| format_diagram_failure(&e))?;
                std::fs::write(&dest, svg.as_bytes())
                    .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::WriteFileFailed, e))?;
                Ok(dest)
            }
            DiagramExportKind::Png => {
                let bytes = render_scene_to_png_bytes(&meta.scene, 2.0)?;
                std::fs::write(&dest, &bytes)
                    .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::WriteFileFailed, e))?;
                Ok(dest)
            }
            DiagramExportKind::Pdf => {
                let bytes = render_scene_to_pdf_bytes(&meta.scene)?;
                std::fs::write(&dest, &bytes)
                    .map_err(|e| crate::i18n::fmt1(crate::i18n::Key::WriteFileFailed, e))?;
                Ok(dest)
            }
        })
        .await
        .unwrap_or_else(|_| Err(crate::i18n::tr(crate::i18n::Key::ErrThread).into()));
        on_done(res);
    });
}
