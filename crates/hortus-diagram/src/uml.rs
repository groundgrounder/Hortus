//! 类图与 ER 图共用的卡片度量、分层放置和矢量端点。
use crate::graph::{Edge, Graph, GraphNode, Shape, Style};
use crate::text::{Style as TextStyle, TextLayout, plain_wrap, translate};
use crate::{Color, Direction, FontMetrics, HitRegion, Point, Primitive, Rect, Theme};
use std::ops::Range;

pub(crate) const SIDE: f64 = 30.0;
pub(crate) const CELL_GAP: f64 = 34.0;
pub(crate) const CARD_PAD: f64 = 11.0;
/// 关系标签与端点基数的可用宽度上限。
pub(crate) const EDGE_LABEL_LIMIT: f64 = 168.0;

/// 浅色与深色主题的独立调和色板。
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub dark: bool,
    pub background: Color,
    pub text: Color,
    pub muted: Color,
    pub edge: Color,
    pub line: Color,
    pub accent: Color,
}

impl Palette {
    pub fn new(theme: Theme) -> Self {
        let dark = theme == Theme::Dark;
        let tone = |day, night, chroma| Color::oklch(if dark { night } else { day }, chroma, 255.0);
        Self {
            dark,
            background: tone(0.982, 0.215, 0.004),
            text: tone(0.25, 0.91, 0.012),
            muted: tone(0.43, 0.76, 0.012),
            edge: tone(0.47, 0.68, 0.032),
            line: tone(0.87, 0.42, 0.012),
            accent: tone(0.58, 0.72, 0.06),
        }
    }

    pub fn tone(self, day: f64, night: f64, chroma: f64, hue: f64) -> Color {
        Color::oklch(if self.dark { night } else { day }, chroma, hue)
    }

    /// 卡片底色、描边和正文色；`classDef` 只改对应通道，其余保持主题调和。
    pub fn card(self, style: Option<&Style>) -> (Color, Color, Color) {
        let hue = |value: Option<&String>, fallback| {
            value
                .and_then(|value| Color::parse_css(value))
                .map_or(fallback, Color::hue)
        };
        let fill_hue = hue(style.and_then(|s| s.fill.as_ref()), 255.0);
        let stroke_hue = hue(style.and_then(|s| s.stroke.as_ref()), fill_hue);
        let text_hue = hue(style.and_then(|s| s.color.as_ref()), 255.0);
        let fill = self.tone(0.995, 0.275, 0.008, fill_hue);
        let stroke = self.tone(0.82, 0.43, 0.018, stroke_hue);
        let text = self.tone(
            0.25,
            0.91,
            if style.is_some() { 0.04 } else { 0.012 },
            text_hue,
        );
        (fill, stroke, text)
    }
}

#[derive(Default)]
pub(crate) struct Layer {
    pub width: f64,
    pub height: f64,
    pub primitives: Vec<Primitive>,
    pub hits: Vec<HitRegion>,
    pub overlaps: usize,
}

impl Layer {
    pub fn append(&mut self, mut other: Self, x: f64, y: f64) {
        translate(&mut other.primitives, x, y);
        self.primitives.append(&mut other.primitives);
        for mut hit in other.hits {
            hit.rect.x += x;
            hit.rect.y += y;
            self.hits.push(hit);
        }
        self.overlaps += other.overlaps;
    }

    pub fn text(&mut self, mut text: TextLayout, x: f64, y: f64) {
        translate(&mut text.primitives, x, y);
        self.primitives.append(&mut text.primitives);
    }

    pub fn hit(&mut self, node_id: String, label: String, rect: Rect, source_range: Range<usize>) {
        self.hits.push(HitRegion {
            label,
            prefix: None,
            node_id,
            rect,
            can_collapse: false,
            collapsed: false,
            source_range,
        });
    }

    pub fn shape(
        &mut self,
        rect: Rect,
        shape: Shape,
        fill: Color,
        stroke: Color,
        stroke_width: f64,
    ) {
        self.primitives.push(Primitive::GraphShape {
            rect,
            shape,
            outline: crate::shape::outline(shape, rect),
            details: Vec::new(),
            fill,
            stroke,
            stroke_width,
        });
    }

    pub fn polygon(&mut self, points: Vec<Point>, fill: Color, stroke: Color, stroke_width: f64) {
        let rect = bounds(&points);
        self.primitives.push(Primitive::GraphShape {
            rect,
            shape: Shape::Diamond,
            outline: points,
            details: Vec::new(),
            fill,
            stroke,
            stroke_width,
        });
    }

    /// 不填充的线段；用退化轮廓表达，平台只描边。
    pub fn segment(&mut self, from: Point, to: Point, color: Color, width: f64) {
        let rect = bounds(&[from, to]);
        self.primitives.push(Primitive::GraphShape {
            rect,
            shape: Shape::Subroutine,
            outline: vec![from, to],
            details: Vec::new(),
            fill: Color { a: 0.0, ..color },
            stroke: color,
            stroke_width: width,
        });
    }

    pub fn rule(&mut self, from: Point, to: Point, color: Color) {
        self.primitives.push(Primitive::Line {
            from,
            to,
            width: 1.0,
            color,
        });
    }
}

fn bounds(points: &[Point]) -> Rect {
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for point in points {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

pub(crate) fn hard_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut start = 0;
    let mut offset = 0;
    while let Some(index) = lower[offset..].find('<') {
        let at = offset + index;
        let Some(end) = lower[at..].find('>').map(|end| at + end) else {
            break;
        };
        let tag = lower[at + 1..end].trim();
        if tag == "br"
            || tag
                .strip_suffix('/')
                .is_some_and(|name| name.trim_end() == "br")
        {
            lines.extend(text[start..at].split('\n'));
            start = end + 1;
        }
        offset = end + 1;
    }
    lines.extend(text[start..].split('\n'));
    lines
}

pub(crate) fn text_block(
    text: &str,
    max_width: f64,
    size: f64,
    weight: u16,
    color: Color,
    line_height: f64,
    fonts: &mut dyn FontMetrics,
) -> TextLayout {
    let style = TextStyle {
        size,
        weight,
        color,
    };
    let mut block = TextLayout {
        primitives: Vec::new(),
        height: 0.0,
    };
    if text.trim().is_empty() {
        return block;
    }
    for line in hard_lines(text) {
        let mut part = plain_wrap(&[(line.trim(), style)], max_width, line_height, fonts);
        translate(&mut part.primitives, 0.0, block.height);
        block.primitives.append(&mut part.primitives);
        block.height += part.height.max(line_height);
    }
    block
}

pub(crate) fn text_width(text: &TextLayout) -> f64 {
    text.primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Text(span) => Some(span.baseline.x + span.width),
            _ => None,
        })
        .fold(0.0, f64::max)
}

pub(crate) fn axes(direction: Direction) -> (bool, bool) {
    match direction {
        Direction::TopToBottom => (true, false),
        Direction::BottomToTop => (true, true),
        Direction::LeftToRight => (false, false),
        Direction::RightToLeft => (false, true),
    }
}

pub(crate) fn offset_rect(mut rect: Rect, offset: Point) -> Rect {
    rect.x += offset.x;
    rect.y += offset.y;
    rect
}

pub(crate) fn path_hit_rect(points: &[Point]) -> Rect {
    points
        .windows(2)
        .max_by(|a, b| {
            let length = |p: &[Point]| (p[0].x - p[1].x).abs() + (p[0].y - p[1].y).abs();
            length(a).total_cmp(&length(b))
        })
        .map_or(Rect::default(), |p| Rect {
            x: (p[0].x + p[1].x) / 2.0 - 7.0,
            y: (p[0].y + p[1].y) / 2.0 - 7.0,
            width: 14.0,
            height: 14.0,
        })
}

pub(crate) fn unit(from: Point, to: Point) -> Option<Point> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length = dx.hypot(dy);
    (length > 0.5).then(|| Point {
        x: dx / length,
        y: dy / length,
    })
}

pub(crate) fn route_node(id: &str, source_range: Range<usize>) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        label: String::new(),
        id_range: source_range.clone(),
        label_range: source_range.clone(),
        source_range,
        shape: Shape::Rectangle,
        class: None,
        kind: Default::default(),
        owner: None,
        preferred_size: None,
    }
}

/// 关系连线的共享路由图；节点顺序与卡片障碍数组一致。
pub(crate) fn routing_graph(
    diagram_direction: Direction,
    nodes: &[(String, Range<usize>)],
    edges: Vec<Edge>,
) -> Graph {
    Graph {
        direction: diagram_direction,
        nodes: nodes
            .iter()
            .map(|(id, range)| route_node(id, range.clone()))
            .collect(),
        edges,
        ..Graph::default()
    }
}

pub(crate) fn overlap_count(rects: &[Rect]) -> usize {
    rects
        .iter()
        .enumerate()
        .map(|(index, rect)| {
            rects[..index]
                .iter()
                .filter(|other| rect.intersects(**other))
                .count()
        })
        .sum()
}

type Rows = Vec<Vec<Vec<usize>>>;

fn column_width(column: &[usize], sizes: &[(f64, f64)]) -> f64 {
    column.iter().map(|&i| sizes[i].0).fold(0.0, f64::max)
}

fn pack(
    columns: impl IntoIterator<Item = Vec<usize>>,
    sizes: &[(f64, f64)],
    limit: f64,
    gap: f64,
) -> Rows {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut width = 0.0;
    for column in columns {
        let next = column_width(&column, sizes);
        if !row.is_empty() && width + gap + next > limit {
            rows.push(std::mem::take(&mut row));
            width = 0.0;
        }
        width += if row.is_empty() { next } else { gap + next };
        row.push(column);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn row_widths(rows: &Rows, sizes: &[(f64, f64)], gap: f64) -> Vec<f64> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|column| column_width(column, sizes))
                .sum::<f64>()
                + gap * row.len().saturating_sub(1) as f64
        })
        .collect()
}

fn place_cells(
    rows: &Rows,
    sizes: &[(f64, f64)],
    (gap_x, gap_y): (f64, f64),
    margin: f64,
    (vertical, reverse): (bool, bool),
) -> (f64, f64, Vec<Point>) {
    let widths = row_widths(rows, sizes, gap_x);
    let width = widths.iter().copied().fold(0.0, f64::max) + 2.0 * margin;
    let column_height = |column: &[usize]| {
        column.iter().map(|&i| sizes[i].1).sum::<f64>()
            + CELL_GAP * column.len().saturating_sub(1) as f64
    };
    let mut positions = vec![Point::default(); sizes.len()];
    let mut top = margin;
    for (row, row_width) in rows.iter().zip(widths) {
        let row_height = row.iter().map(|col| column_height(col)).fold(0.0, f64::max);
        let mut x = (width - row_width) / 2.0;
        for column in row {
            let w = column_width(column, sizes);
            let mut y = top + (row_height - column_height(column)) / 2.0;
            for &i in column {
                let (cell_width, cell_height) = sizes[i];
                let left = x + (w - cell_width) / 2.0;
                positions[i] = Point {
                    x: if !vertical && reverse {
                        width - left - cell_width
                    } else {
                        left
                    },
                    y,
                };
                y += cell_height + CELL_GAP;
            }
            x += w + gap_x;
        }
        top += row_height + gap_y;
    }
    let height = top - gap_y + margin;
    if vertical && reverse {
        for (point, size) in positions.iter_mut().zip(sizes) {
            point.y = height - point.y - size.1;
        }
    }
    (width, height, positions)
}

/// 一个独立块（无命名空间时的整图，或一个命名空间）的分层放置结果。
pub(crate) struct Placement {
    pub width: f64,
    pub height: f64,
    /// 与 `members` 一一对应的块内位置。
    pub positions: Vec<Point>,
}

pub(crate) fn place_block(
    members: &[usize],
    edges: impl IntoIterator<Item = (usize, usize)>,
    sizes: &[(f64, f64)],
    direction: Direction,
    width: f64,
    margin: f64,
) -> Placement {
    let mut index = vec![usize::MAX; sizes.len()];
    for (local, &global) in members.iter().enumerate() {
        index[global] = local;
    }
    let nodes: Vec<_> = members
        .iter()
        .map(|&global| route_node(&global.to_string(), 0..0))
        .collect();
    let edges = edges
        .into_iter()
        .filter_map(|(from, to)| {
            let (from, to) = (index[from], index[to]);
            (from != usize::MAX && to != usize::MAX && from != to).then(|| Edge {
                id: 0,
                from,
                to,
                source_range: 0..0,
                connector_range: 0..0,
                label: None,
                label_range: None,
                connector: Default::default(),
                visible: true,
                container: None,
            })
        })
        .collect::<Vec<_>>();
    let graph = Graph {
        direction,
        nodes,
        edges,
        ..Graph::default()
    };
    let (ranks, count, _) = crate::graph_layout::assign_ranks(&graph);
    let order = crate::graph_layout::order_ranks(&graph, &ranks, count);
    let sizes_local: Vec<_> = members.iter().map(|&global| sizes[global]).collect();
    let inner = (width - 2.0 * margin).max(72.0);
    let (vertical, reverse) = axes(direction);
    let (rows, gap_x, gap_y) = if vertical {
        let rows = pack(
            order
                .into_iter()
                .flat_map(|rank| rank.into_iter().map(|node| vec![node])),
            &sizes_local,
            inner,
            CELL_GAP,
        );
        (rows, CELL_GAP, 52.0)
    } else {
        (pack(order, &sizes_local, inner, 62.0), 62.0, 44.0)
    };
    let (width, height, positions) = place_cells(
        &rows,
        &sizes_local,
        (gap_x, gap_y),
        margin,
        (vertical, reverse),
    );
    Placement {
        width,
        height,
        positions,
    }
}
