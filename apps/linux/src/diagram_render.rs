// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Mermaid 图表的 Cairo + PangoCairo 原生矢量绘制、交互命中与 SVG/PNG/PDF 导出。

use crate::pangocairo_ffi;
use gtk4::cairo;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, DiagramFailure, DiagramIdentity, DiagramPrevious, SceneColor, SceneData,
    SceneHit, ScenePoint, ScenePrimitive, SceneRect, SceneText,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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
                "暂不支持该 Mermaid 图表类型".into()
            } else {
                format!("暂不支持的 Mermaid 图表类型：{diagram_type}")
            }
        }
        DiagramFailure::UnsupportedConstruct { line, kind, .. } => {
            format!("第 {line} 行使用了暂不支持的图表语法（{kind}）")
        }
        DiagramFailure::NotATree { multi_parent } => {
            if multi_parent.is_empty() {
                "该图表包含多父节点关系，无法折叠为树结构".into()
            } else {
                format!(
                    "节点存在多父级关系，无法转为树结构：{}",
                    multi_parent.join(", ")
                )
            }
        }
        DiagramFailure::Syntax { line, kind } => {
            format!("Mermaid 语法错误（第 {line} 行：{kind}）")
        }
        DiagramFailure::Engine { kind } => format!("图表引擎内部异常：{kind}"),
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
    let pango_ctx = pangocairo_ffi::create_pango_context(cr, font_map);

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
    pangocairo_ffi::update_layout(cr, &layout);

    set_color(cr, &span.color);
    if let Some(line) = layout.line(0) {
        cr.move_to(span.baseline.x, span.baseline.y);
        pangocairo_ffi::show_layout_line(cr, &line);
    } else {
        cr.move_to(span.baseline.x, span.baseline.y - span.font_size);
        pangocairo_ffi::show_layout(cr, &layout);
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
                // 用精确三次贝塞尔等价二次曲线（控制点权重 2/3）
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
        .map_err(|e| format!("创建 ImageSurface 失败：{e}"))?;
    {
        let cr =
            cairo::Context::new(&surface).map_err(|e| format!("创建 Cairo 上下文失败：{e}"))?;
        cr.scale(scale, scale);
        draw_scene(&cr, scene, None, None);
    }
    surface.flush();
    let mut out = Cursor::new(Vec::new());
    surface
        .write_to_png(&mut out)
        .map_err(|e| format!("编码 PNG 失败：{e}"))?;
    Ok(out.into_inner())
}

/// 将场景矢量输出为 PDF 字节（保留矢量文字与曲线，无栅格化降质）。
pub fn render_scene_to_pdf_bytes(scene: &SceneData) -> Result<Vec<u8>, String> {
    let w = scene.width.max(1.0);
    let h = scene.height.max(1.0);
    let surface = cairo::PdfSurface::for_stream(w, h, Vec::<u8>::new())
        .map_err(|e| format!("创建 PdfSurface 失败：{e}"))?;
    {
        let cr =
            cairo::Context::new(&surface).map_err(|e| format!("创建 PDF Cairo 上下文失败：{e}"))?;
        draw_scene(&cr, scene, None, None);
        let _ = cr.show_page();
    }
    let stream = surface
        .finish_output_stream()
        .map_err(|e| format!("完成 PDF 流失败：{}", e.error))?;
    stream
        .downcast::<Vec<u8>>()
        .map(|boxed| *boxed)
        .map_err(|_| "PDF 输出流类型不匹配".to_string())
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
                std::fs::write(&dest, svg.as_bytes()).map_err(|e| format!("写入文件失败：{e}"))?;
                Ok(dest)
            }
            DiagramExportKind::Png => {
                let bytes = render_scene_to_png_bytes(&meta.scene, 2.0)?;
                std::fs::write(&dest, &bytes).map_err(|e| format!("写入文件失败：{e}"))?;
                Ok(dest)
            }
            DiagramExportKind::Pdf => {
                let bytes = render_scene_to_pdf_bytes(&meta.scene)?;
                std::fs::write(&dest, &bytes).map_err(|e| format!("写入文件失败：{e}"))?;
                Ok(dest)
            }
        })
        .await
        .unwrap_or_else(|_| Err("导出线程异常".into()));
        on_done(res);
    });
}

/// 内联图表交互回调函数集。
#[derive(Clone)]
pub struct DiagramCallbacks {
    pub on_collapse_changed: Rc<dyn Fn(u32, Vec<String>, SceneData)>,
    pub on_open_link: Rc<dyn Fn(String, Option<String>)>,
    pub on_open_source: Rc<dyn Fn(u64, u64)>,
    pub on_focus: Rc<dyn Fn(u32)>,
    pub on_export: Rc<dyn Fn(DiagramExportKind, DiagramExportMeta)>,
}

/// 构建内联 Mermaid 图表控件（支持节点折叠、双链跳转、专注模式、SVG/PNG/PDF 导出及降级源码卡片）。
pub fn build_diagram_card(
    note_path: String,
    source: String,
    mmd: bool,
    initial: RenderedDiagram,
    engine: Arc<DiagramEngine>,
    callbacks: DiagramCallbacks,
    content_width: f64,
) -> gtk4::Widget {
    if let Some(reason) = initial.failure {
        return build_diagram_fallback_card(
            &source,
            initial.source_start,
            initial.source_end,
            &reason,
            content_width,
        );
    }
    let Some(scene) = initial.scene else {
        return build_diagram_fallback_card(
            &source,
            initial.source_start,
            initial.source_end,
            "图表场景为空",
            content_width,
        );
    };

    let ordinal = initial.ordinal;
    let scene_cell = Rc::new(RefCell::new(scene.clone()));
    let collapsed_cell = Rc::new(RefCell::new(initial.collapsed));
    let previous_cell = Rc::new(RefCell::new(initial.previous));
    let hovered_hit: Rc<RefCell<Option<SceneHit>>> = Rc::new(RefCell::new(None));
    let rerender_gen = Rc::new(Cell::new(0u64));

    let drawing = gtk4::DrawingArea::new();
    drawing.set_content_width(scene.width.ceil().max(120.0) as i32);
    drawing.set_content_height(scene.height.ceil().max(48.0) as i32);
    // 最小宽等于场景宽：若场景比目标栏宽略大，内层滚动容器可以真正横向滚动而不是裁切。
    drawing.set_size_request(scene.width.ceil().max(120.0) as i32, -1);
    drawing.set_hexpand(true);
    drawing.set_halign(gtk4::Align::Fill);

    {
        let scene_ref = scene_cell.clone();
        let hover_ref = hovered_hit.clone();
        drawing.set_draw_func(move |_, cr, _, _| {
            let current = scene_ref.borrow();
            let hl = hover_ref.borrow().as_ref().and_then(|hit| {
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
            draw_scene(cr, &current, None, hl.as_ref());
        });
    }

    let motion = gtk4::EventControllerMotion::new();
    {
        let drawing_c = drawing.clone();
        let scene_ref = scene_cell.clone();
        let hover_ref = hovered_hit.clone();
        motion.connect_motion(move |_, x, y| {
            let found = {
                let s = scene_ref.borrow();
                hit_test(&s, x, y).cloned()
            };
            let pointer = found
                .as_ref()
                .is_some_and(|h| h.can_collapse || h.link_target.is_some());
            drawing_c.set_cursor_from_name(if pointer { Some("pointer") } else { None });
            if let Some(ref hit) = found {
                let tip = if let Some(ref target) = hit.link_target {
                    Some(format!("打开笔记：{target}"))
                } else if hit.can_collapse {
                    Some(if hit.collapsed {
                        "点击展开分支".to_string()
                    } else {
                        "点击折叠分支".to_string()
                    })
                } else {
                    None
                };
                drawing_c.set_tooltip_text(tip.as_deref());
            } else {
                drawing_c.set_tooltip_text(None);
            }
            let changed = match (&*hover_ref.borrow(), &found) {
                (None, None) => false,
                (Some(a), Some(b)) => a.node_id != b.node_id,
                _ => true,
            };
            if changed {
                *hover_ref.borrow_mut() = found;
                drawing_c.queue_draw();
            }
        });
    }
    {
        let drawing_c = drawing.clone();
        let hover_ref = hovered_hit.clone();
        motion.connect_leave(move |_| {
            drawing_c.set_cursor_from_name(None);
            drawing_c.set_tooltip_text(None);
            if hover_ref.borrow_mut().take().is_some() {
                drawing_c.queue_draw();
            }
        });
    }
    drawing.add_controller(motion);

    let click = gtk4::GestureClick::new();
    click.set_button(1);
    {
        let scene_ref = scene_cell.clone();
        let collapsed_ref = collapsed_cell.clone();
        let previous_ref = previous_cell.clone();
        let gen_ref = rerender_gen.clone();
        let drawing_c = drawing.clone();
        let engine_c = engine.clone();
        let note_c = note_path.clone();
        let src_c = source.clone();
        let cbs = callbacks.clone();

        click.connect_pressed(move |_, _, x, y| {
            let hit = {
                let s = scene_ref.borrow();
                hit_test(&s, x, y).cloned()
            };
            let Some(hit) = hit else { return };
            if hit.can_collapse {
                let next_collapsed = {
                    let mut list = collapsed_ref.borrow_mut();
                    if let Some(pos) = list.iter().position(|id| id == &hit.node_id) {
                        list.remove(pos);
                    } else {
                        list.push(hit.node_id.clone());
                        list.sort();
                    }
                    list.clone()
                };
                let token = gen_ref.get().wrapping_add(1);
                gen_ref.set(token);
                let prev = previous_ref.borrow().clone();
                let cur_width = f64::from(drawing_c.width()).max(scene_ref.borrow().width);
                let dark = adw::StyleManager::default().is_dark();
                let engine_bg = engine_c.clone();
                let src_bg = src_c.clone();
                let note_bg = note_c.clone();
                let collapsed_bg = next_collapsed.clone();

                let gen_done = gen_ref.clone();
                let scene_done = scene_ref.clone();
                let prev_done = previous_ref.clone();
                let drawing_done = drawing_c.clone();
                let cb_done = cbs.on_collapse_changed.clone();

                glib::MainContext::default().spawn_local(async move {
                    let rendered = gio::spawn_blocking(move || {
                        render_block_with_engine(
                            &engine_bg,
                            &src_bg,
                            mmd,
                            cur_width,
                            &collapsed_bg,
                            DiagramIdentity {
                                document: note_bg,
                                block: ordinal,
                                revision: 1,
                                dark,
                                font_generation: 0,
                                request_generation: token,
                            },
                            prev,
                        )
                    })
                    .await;
                    if gen_done.get() != token {
                        return;
                    }
                    if let Ok(Ok(new_scene)) = rendered {
                        *prev_done.borrow_mut() = new_scene.previous.clone();
                        drawing_done.set_content_width(new_scene.width.ceil().max(120.0) as i32);
                        drawing_done.set_content_height(new_scene.height.ceil().max(48.0) as i32);
                        *scene_done.borrow_mut() = new_scene.clone();
                        drawing_done.queue_draw();
                        (cb_done)(ordinal, next_collapsed, new_scene);
                    }
                });
            } else if let Some(target) = hit.link_target {
                (cbs.on_open_link)(target, hit.link_heading);
            } else if hit.source_end > hit.source_start {
                (cbs.on_open_source)(hit.source_start, hit.source_end);
            }
        });
    }
    drawing.add_controller(click);

    // 悬停/固定微型工具栏：专注查看（Ctrl+Shift+D）与导出 SVG/PNG/PDF
    let tools = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    tools.add_css_class("hortus-diagram-tools");
    tools.set_halign(gtk4::Align::End);
    tools.set_valign(gtk4::Align::Start);
    tools.set_margin_top(6);
    tools.set_margin_end(6);

    let focus_btn = gtk4::Button::builder()
        .icon_name("view-fullscreen-symbolic")
        .tooltip_text("专注查看图表 (Ctrl+Shift+D)")
        .build();
    focus_btn.add_css_class("flat");
    focus_btn.add_css_class("circular");
    {
        let on_focus = callbacks.on_focus.clone();
        focus_btn.connect_clicked(move |_| (on_focus)(ordinal));
    }

    let export_popover = gtk4::Popover::new();
    let export_list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    export_list.set_margin_top(4);
    export_list.set_margin_bottom(4);
    export_list.set_margin_start(4);
    export_list.set_margin_end(4);
    for (label, kind) in [
        ("导出为 SVG…", DiagramExportKind::Svg),
        ("导出为 PNG (2x)…", DiagramExportKind::Png),
        ("导出为 PDF…", DiagramExportKind::Pdf),
    ] {
        let btn = gtk4::Button::with_label(label);
        btn.add_css_class("flat");
        let on_export = callbacks.on_export.clone();
        let scene_c = scene_cell.clone();
        let collapsed_c = collapsed_cell.clone();
        let note_c = note_path.clone();
        let src_c = source.clone();
        let pop_c = export_popover.clone();
        btn.connect_clicked(move |_| {
            pop_c.popdown();
            let meta = DiagramExportMeta {
                note_path: note_c.clone(),
                ordinal,
                source: src_c.clone(),
                mmd,
                collapsed: collapsed_c.borrow().clone(),
                scene: scene_c.borrow().clone(),
            };
            (on_export)(kind, meta);
        });
        export_list.append(&btn);
    }
    export_popover.set_child(Some(&export_list));

    let export_btn = gtk4::MenuButton::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("导出图表 (SVG / PNG / PDF)")
        .popover(&export_popover)
        .build();
    export_btn.add_css_class("flat");
    export_btn.add_css_class("circular");

    tools.append(&focus_btn);
    tools.append(&export_btn);

    // 画布最小宽度必须等于本次排版目标宽：TextView 按子锚点最小宽分配宽度，
    // 只有最小宽等于目标宽时图表才能完整铺满阅读栏。窗口最小宽由外层阅读滚动容器解耦。
    let canvas = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .child(&drawing)
        .build();
    canvas.add_css_class("hortus-diagram-canvas");
    canvas.set_size_request(content_width.round().max(120.0) as i32, -1);
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&canvas));
    overlay.add_overlay(&tools);
    overlay.set_hexpand(true);
    overlay.add_css_class("hortus-diagram-card");

    overlay.upcast()
}

fn build_diagram_fallback_card(
    full_source: &str,
    start_byte: u64,
    end_byte: u64,
    reason: &str,
    content_width: f64,
) -> gtk4::Widget {
    let snippet = full_source
        .get(start_byte as usize..end_byte as usize)
        .unwrap_or(full_source)
        .trim_end();
    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    card.add_css_class("hortus-diagram-fallback");
    card.set_halign(gtk4::Align::Fill);
    card.set_hexpand(true);
    card.set_size_request(content_width.round().max(240.0) as i32, -1);

    let header = gtk4::Label::new(Some(reason));
    header.set_halign(gtk4::Align::Start);
    header.add_css_class("dim-label");
    header.add_css_class("caption");
    header.set_wrap(true);
    card.append(&header);

    let tv = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .monospace(true)
        .build();
    tv.buffer().set_text(snippet);
    tv.add_css_class("hortus-fallback-code");
    card.append(&tv);
    card.upcast()
}
