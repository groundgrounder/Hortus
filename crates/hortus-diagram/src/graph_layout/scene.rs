//! 场景几何、标签排布与可见性统计。

use super::*;

pub(super) fn scene_content_bounds(scene: &Scene) -> Option<Rect> {
    let mut bounds = None::<Rect>;
    let mut include = |rect: Rect| {
        bounds = Some(match bounds {
            None => rect,
            Some(current) => {
                let x = current.x.min(rect.x);
                let y = current.y.min(rect.y);
                let right = (current.x + current.width).max(rect.x + rect.width);
                let bottom = (current.y + current.height).max(rect.y + rect.height);
                Rect {
                    x,
                    y,
                    width: right - x,
                    height: bottom - y,
                }
            }
        });
    };
    for hit in &scene.hit_regions {
        include(hit.rect);
    }
    for primitive in &scene.primitives {
        match primitive {
            Primitive::RoundedRect { .. } => {}
            Primitive::GraphShape { rect, .. } | Primitive::LabelBackground { rect, .. } => {
                include(*rect)
            }
            Primitive::GraphPath { points, .. } => {
                for point in points {
                    include(Rect {
                        x: point.x,
                        y: point.y,
                        width: 0.0,
                        height: 0.0,
                    });
                }
            }
            Primitive::Text(span) => include(Rect {
                x: span.baseline.x,
                y: span.baseline.y - span.font_size,
                width: span.width,
                height: span.font_size,
            }),
            Primitive::Line { from, to, .. } => include(Rect {
                x: from.x.min(to.x),
                y: from.y.min(to.y),
                width: (from.x - to.x).abs(),
                height: (from.y - to.y).abs(),
            }),
        }
    }
    bounds
}

pub(super) fn container_title_lines(
    title: &str,
    width: f64,
    fonts: &mut dyn FontMetrics,
) -> Vec<(String, crate::Measurement)> {
    wrap_label(title, width, EDGE_LABEL_SIZE, 600, fonts)
        .into_iter()
        .filter_map(|line| {
            let line = line.trim_end();
            (!line.is_empty())
                .then(|| (line.to_string(), fonts.measure(line, EDGE_LABEL_SIZE, 600)))
        })
        .collect()
}

pub(super) fn container_title_height(lines: &[(String, crate::Measurement)]) -> f64 {
    (lines.len().max(1) as f64 * 18.0 + 6.0).max(CONTAINER_TITLE_GAP)
}

pub(super) fn push_dashed_outline(
    primitives: &mut Vec<Primitive>,
    shape: Shape,
    rect: Rect,
    color: Color,
    width: f64,
    pattern: &[f64],
) {
    if pattern.is_empty() || pattern.iter().sum::<f64>() <= 0.001 {
        return;
    }
    let mut pattern = pattern.to_vec();
    if pattern.len() % 2 == 1 {
        pattern.extend_from_within(..);
    }
    let points = crate::shape::outline(shape, rect);
    if points.len() < 2 {
        return;
    }
    let mut pattern_index = 0usize;
    let mut remaining = pattern[0];
    let mut visible = true;
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let length = dx.hypot(dy);
        if length < 0.001 {
            continue;
        }
        let mut offset = 0.0;
        while offset < length - 0.001 {
            let step = remaining.min(length - offset);
            if visible && step > 0.001 {
                let start = offset / length;
                let end = (offset + step) / length;
                primitives.push(Primitive::Line {
                    from: Point {
                        x: a.x + dx * start,
                        y: a.y + dy * start,
                    },
                    to: Point {
                        x: a.x + dx * end,
                        y: a.y + dy * end,
                    },
                    width,
                    color,
                });
            }
            offset += step;
            remaining -= step;
            if remaining <= 0.001 {
                pattern_index = (pattern_index + 1) % pattern.len();
                remaining = pattern[pattern_index];
                visible = !visible;
            }
        }
    }
}

/// 容器标题的两个对比色固定为已发布场景中的 sRGB 值。
/// `Color::oklch` 在常量折叠与运行时 libm 之间会有 1 ULP 差异；固定值保证
/// 跨编译单元、跨版本输出完全一致。
const CONTRAST_DARK: Color = Color {
    r: 0.0936796325404568,
    g: 0.10485192623011494,
    b: 0.12288714748829563,
    a: 1.0,
};
const CONTRAST_LIGHT: Color = Color {
    r: 0.9198636694811972,
    g: 0.9360736346767901,
    b: 0.9622160300036441,
    a: 1.0,
};

pub(super) fn contrast_text_color(background: Color, fallback: Color) -> Color {
    if background.a < 0.15 {
        return fallback;
    }
    let linear = |value: f64| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.r)
        + 0.7152 * linear(background.g)
        + 0.0722 * linear(background.b);
    if luminance > 0.42 {
        CONTRAST_DARK
    } else {
        CONTRAST_LIGHT
    }
}

pub(super) fn scene_paths(scene: &Scene) -> Vec<Vec<Point>> {
    scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::GraphPath { points, .. } => Some(points.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn translated_primitive(primitive: &Primitive, dx: f64, dy: f64) -> Primitive {
    match primitive {
        Primitive::RoundedRect {
            rect,
            radius,
            color,
        } => Primitive::RoundedRect {
            rect: translated_rect(*rect, dx, dy),
            radius: *radius,
            color: *color,
        },
        Primitive::Line {
            from,
            to,
            width,
            color,
        } => Primitive::Line {
            from: Point {
                x: from.x + dx,
                y: from.y + dy,
            },
            to: Point {
                x: to.x + dx,
                y: to.y + dy,
            },
            width: *width,
            color: *color,
        },
        Primitive::GraphPath {
            edge_id,
            corner_radius,
            points,
            source_range,
            label_range,
            line,
            start,
            end,
            color,
            width,
        } => Primitive::GraphPath {
            edge_id: *edge_id,
            corner_radius: *corner_radius,
            points: points
                .iter()
                .map(|point| Point {
                    x: point.x + dx,
                    y: point.y + dy,
                })
                .collect(),
            source_range: source_range.clone(),
            label_range: label_range.clone(),
            line: *line,
            start: *start,
            end: *end,
            color: *color,
            width: *width,
        },
        Primitive::GraphShape {
            rect,
            shape,
            outline,
            details,
            fill,
            stroke,
            stroke_width,
        } => Primitive::GraphShape {
            rect: translated_rect(*rect, dx, dy),
            shape: *shape,
            outline: outline
                .iter()
                .map(|point| Point {
                    x: point.x + dx,
                    y: point.y + dy,
                })
                .collect(),
            details: details
                .iter()
                .map(|detail| {
                    detail
                        .iter()
                        .map(|point| Point {
                            x: point.x + dx,
                            y: point.y + dy,
                        })
                        .collect()
                })
                .collect(),
            fill: *fill,
            stroke: *stroke,
            stroke_width: *stroke_width,
        },
        Primitive::LabelBackground {
            edge_id,
            rect,
            color,
        } => Primitive::LabelBackground {
            edge_id: *edge_id,
            rect: translated_rect(*rect, dx, dy),
            color: *color,
        },
        Primitive::Text(span) => Primitive::Text(TextSpan {
            text: span.text.clone(),
            baseline: Point {
                x: span.baseline.x + dx,
                y: span.baseline.y + dy,
            },
            width: span.width,
            font_size: span.font_size,
            font_weight: span.font_weight,
            font_family: span.font_family.clone(),
            color: span.color,
        }),
    }
}

pub(super) fn translated_rect(rect: Rect, dx: f64, dy: f64) -> Rect {
    Rect {
        x: rect.x + dx,
        y: rect.y + dy,
        ..rect
    }
}

pub(super) fn wrap_label(
    text: &str,
    limit: f64,
    size: f64,
    weight: u16,
    fonts: &mut dyn FontMetrics,
) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut lines = Vec::new();
    let mut current = String::new();
    for grapheme in text.graphemes(true) {
        let candidate = format!("{current}{grapheme}");
        if !current.is_empty() && fonts.measure(&candidate, size, weight).width > limit {
            lines.push(std::mem::take(&mut current));
        }
        current.push_str(grapheme);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

pub(crate) fn place_edge_label(
    index: usize,
    width: f64,
    paths: &[Vec<Point>],
    nodes: &[Rect],
    labels: &[Rect],
    canvas_width: f64,
) -> Option<Rect> {
    place_sized_edge_label(
        index,
        (width, EDGE_LABEL_HEIGHT),
        paths,
        nodes,
        labels,
        (canvas_width, f64::INFINITY),
    )
}

pub(crate) fn place_sized_edge_label(
    index: usize,
    (width, height): (f64, f64),
    paths: &[Vec<Point>],
    nodes: &[Rect],
    labels: &[Rect],
    (canvas_width, canvas_height): (f64, f64),
) -> Option<Rect> {
    let mut candidates = Vec::new();
    for (segment, pair) in paths[index].windows(2).enumerate() {
        let length = (pair[1].x - pair[0].x).abs() + (pair[1].y - pair[0].y).abs();
        let horizontal = (pair[0].y - pair[1].y).abs() < 0.001;
        for fraction in [0.5, 0.35, 0.3, 0.65, 0.7, 0.15, 0.85] {
            let center = Point {
                x: pair[0].x + (pair[1].x - pair[0].x) * fraction,
                y: pair[0].y + (pair[1].y - pair[0].y) * fraction,
            };
            // 长标签在密集 LR 图里放不进边线正中；允许垂直/水平让位到相邻空白。
            let offsets = if horizontal {
                vec![0.0, 12.0, -12.0, 22.0, -22.0, 32.0, -32.0]
            } else {
                let side = width / 2.0 + EDGE_LABEL_PAD_X + EDGE_LABEL_SIDE_CLEARANCE;
                vec![side, -side]
            };
            for offset in offsets {
                let (dx, dy) = if horizontal {
                    (0.0, offset)
                } else {
                    (offset, 0.0)
                };
                let rect = Rect {
                    x: center.x + dx - width / 2.0 - EDGE_LABEL_PAD_X,
                    y: center.y + dy - height / 2.0,
                    width: width + EDGE_LABEL_PAD_X * 2.0,
                    height,
                };
                if rect.x < 4.0
                    || rect.x + rect.width > canvas_width - 4.0
                    || rect.y < 4.0
                    || rect.y + rect.height > canvas_height - 4.0
                {
                    continue;
                }
                let margin = Rect {
                    x: rect.x - 2.0,
                    y: rect.y - 2.0,
                    width: rect.width + 4.0,
                    height: rect.height + 4.0,
                };
                if nodes.iter().any(|node| rect.intersects(*node))
                    || labels.iter().any(|label| margin.intersects(*label))
                {
                    continue;
                }
                if !horizontal && crate::geometry::segment_intersects(pair[0], pair[1], margin) {
                    continue;
                }
                if marker_clearance(
                    paths[index].last(),
                    paths[index]
                        .len()
                        .checked_sub(2)
                        .and_then(|i| paths[index].get(i)),
                )
                .is_some_and(|marker| rect.intersects(marker))
                {
                    continue;
                }
                if paths.iter().enumerate().any(|(other, path)| {
                    other != index
                        && path.windows(2).any(|pair| {
                            crate::geometry::segment_intersects(pair[0], pair[1], margin)
                        })
                }) {
                    continue;
                }
                candidates.push((
                    -length
                        + (fraction - 0.5).abs() * 30.0
                        + offset.abs() * 1.5
                        + segment as f64 * 0.01,
                    rect,
                ));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    candidates.first().map(|(_, rect)| *rect)
}

pub(super) fn marker_clearance(point: Option<&Point>, next: Option<&Point>) -> Option<Rect> {
    let (point, next) = (point?, next?);
    let dx = point.x - next.x;
    let dy = point.y - next.y;
    let length = dx.hypot(dy);
    if length < 0.001 {
        return None;
    }
    let back = Point {
        x: point.x - 9.0 * dx / length,
        y: point.y - 9.0 * dy / length,
    };
    let side = Point {
        x: -dy / length * 4.0,
        y: dx / length * 4.0,
    };
    let vertices = [
        *point,
        Point {
            x: back.x + side.x,
            y: back.y + side.y,
        },
        Point {
            x: back.x - side.x,
            y: back.y - side.y,
        },
    ];
    let min_x = vertices
        .iter()
        .map(|point| point.x)
        .fold(f64::MAX, f64::min);
    let max_x = vertices
        .iter()
        .map(|point| point.x)
        .fold(f64::MIN, f64::max);
    let min_y = vertices
        .iter()
        .map(|point| point.y)
        .fold(f64::MAX, f64::min);
    let max_y = vertices
        .iter()
        .map(|point| point.y)
        .fold(f64::MIN, f64::max);
    Some(Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

pub(super) fn count_node_overlaps(nodes: &[BoxedNode]) -> usize {
    let mut overlaps = 0;
    for i in 0..nodes.len() {
        for j in i + 1..nodes.len() {
            if nodes[i].rect.intersects(nodes[j].rect) {
                overlaps += 1;
            }
        }
    }
    overlaps
}

pub(super) fn count_edge_crossings(paths: &[Vec<Point>]) -> usize {
    let mut crossings = 0;
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            for a in paths[i].windows(2) {
                for b in paths[j].windows(2) {
                    let ah = (a[0].y - a[1].y).abs() < 0.001;
                    let bh = (b[0].y - b[1].y).abs() < 0.001;
                    if ah == bh {
                        continue;
                    }
                    let (h, v) = if ah { (a, b) } else { (b, a) };
                    let x = v[0].x;
                    let y = h[0].y;
                    if x > h[0].x.min(h[1].x) + 0.001
                        && x < h[0].x.max(h[1].x) - 0.001
                        && y > v[0].y.min(v[1].y) + 0.001
                        && y < v[0].y.max(v[1].y) - 0.001
                    {
                        crossings += 1;
                    }
                }
            }
        }
    }
    crossings
}
