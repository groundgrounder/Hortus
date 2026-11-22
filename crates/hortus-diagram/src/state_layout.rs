//! 每个区域先度量子状态，再按拓扑层放置，并以共享网格路由转换。
mod node;

use crate::graph_layout::{assign_ranks, order_ranks};
use crate::graph_router::route_state;
use crate::state::*;
use crate::text::{Style as TextStyle, TextLayout, plain_wrap, translate};
use crate::{
    Color, Connector, Direction, Edge, EdgeLine, FontMetrics, Graph, GraphNode, HitRegion,
    LayoutError, LayoutOptions, Marker, Metrics, Point, Primitive, Rect, Scene, Shape, Theme,
};
use std::ops::Range;
use std::time::Instant;

const SIDE: f64 = 26.0;
const CELL_GAP: f64 = 30.0;
const TEXT_SIZE: f64 = 13.4;
const TEXT_LINE: f64 = 18.0;

#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
    background: Color,
    text: Color,
    muted: Color,
    edge: Color,
    line: Color,
}

impl Palette {
    fn new(theme: Theme) -> Self {
        let dark = theme == Theme::Dark;
        let tone = |day, night, chroma| Color::oklch(if dark { night } else { day }, chroma, 255.0);
        Self {
            dark,
            background: tone(0.982, 0.215, 0.004),
            text: tone(0.25, 0.91, 0.012),
            muted: tone(0.43, 0.76, 0.012),
            edge: tone(0.49, 0.67, 0.035),
            line: tone(0.86, 0.42, 0.015),
        }
    }

    fn tone(self, day: f64, night: f64, chroma: f64, hue: f64) -> Color {
        Color::oklch(if self.dark { night } else { day }, chroma, hue)
    }
}

#[derive(Default)]
struct Layer {
    width: f64,
    height: f64,
    primitives: Vec<Primitive>,
    hits: Vec<HitRegion>,
    overlaps: usize,
}

impl Layer {
    fn append(&mut self, mut other: Self, x: f64, y: f64) {
        translate(&mut other.primitives, x, y);
        self.primitives.append(&mut other.primitives);
        for mut hit in other.hits {
            hit.rect.x += x;
            hit.rect.y += y;
            self.hits.push(hit);
        }
        self.overlaps += other.overlaps;
    }

    fn text(&mut self, mut text: TextLayout, x: f64, y: f64) {
        translate(&mut text.primitives, x, y);
        self.primitives.append(&mut text.primitives);
    }

    fn hit(&mut self, node_id: String, label: String, rect: Rect, source_range: Range<usize>) {
        self.hit_with_link(node_id, label, rect, source_range, None);
    }

    fn hit_with_link(
        &mut self,
        node_id: String,
        label: String,
        rect: Rect,
        source_range: Range<usize>,
        link: Option<&crate::wikilink::WikiLink>,
    ) {
        self.hits.push(HitRegion {
            label,
            prefix: None,
            node_id,
            rect,
            can_collapse: false,
            collapsed: false,
            source_range,
            link_target: link.map(|link| link.target.clone()),
            link_heading: link.and_then(|link| link.heading.clone()),
        });
    }

    fn shape(&mut self, rect: Rect, shape: Shape, fill: Color, stroke: Color, stroke_width: f64) {
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

    fn rule(&mut self, from: Point, to: Point, color: Color) {
        self.primitives.push(Primitive::Line {
            from,
            to,
            width: 1.0,
            color,
        });
    }
}

struct Layout<'a> {
    diagram: &'a StateDiagram,
    fonts: &'a mut dyn FontMetrics,
    palette: Palette,
    next_path: usize,
}

fn text_width(text: &TextLayout) -> f64 {
    text.primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Text(span) => Some(span.baseline.x + span.width),
            _ => None,
        })
        .fold(0.0, f64::max)
}

fn hard_lines(text: &str) -> Vec<&str> {
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

impl Layout<'_> {
    fn text(
        &mut self,
        text: &str,
        max_width: f64,
        size: f64,
        weight: u16,
        color: Color,
    ) -> TextLayout {
        let style = TextStyle {
            size,
            weight,
            color,
        };
        let line_height = if size > TEXT_SIZE {
            size + 5.0
        } else {
            TEXT_LINE
        };
        let mut block = TextLayout {
            primitives: Vec::new(),
            height: 0.0,
        };
        if text.trim().is_empty() {
            return block;
        }
        for line in hard_lines(text) {
            let mut part = plain_wrap(&[(line.trim(), style)], max_width, line_height, self.fonts);
            translate(&mut part.primitives, 0.0, block.height);
            block.primitives.append(&mut part.primitives);
            block.height += part.height.max(line_height);
        }
        block
    }

    fn path(&mut self, layer: &mut Layer, points: Vec<Point>, mark: PathMark<'_>) -> usize {
        let edge_id = self.next_path;
        self.next_path += 1;
        let (source_range, label_range, line, end, color, width) = match mark {
            PathMark::Arrow(edge) => (
                edge.source_range.clone(),
                edge.label_range.clone(),
                EdgeLine::Solid,
                Marker::Arrow,
                self.palette.edge,
                1.4,
            ),
            PathMark::Dotted(source) => (
                source,
                None,
                EdgeLine::Dotted,
                Marker::None,
                self.palette.line,
                1.1,
            ),
        };
        layer.primitives.push(Primitive::GraphPath {
            edge_id,
            corner_radius: 6.0,
            points,
            source_range,
            label_range,
            line,
            start: Marker::None,
            end,
            color,
            width,
        });
        edge_id
    }

    fn prepare_region(
        &mut self,
        region: &StateRegion,
        prefix: &str,
        width: f64,
        background: Color,
    ) -> Result<Layer, LayoutError> {
        if region.nodes.is_empty() {
            return Ok(Layer::default());
        }
        let (vertical, reverse) = axes(region.direction);
        let graph = routing_graph(region)?;
        let (ranks, count, accepted) = assign_ranks(&graph);
        let order = order_ranks(&graph, &ranks, count);
        let inner = (width - SIDE * 2.0).max(72.0);
        let label_limit = (inner * if vertical { 0.39 } else { 0.25 }).clamp(50.0, 216.0);
        let labels: Vec<_> = region
            .transitions
            .iter()
            .map(|edge| {
                self.text(
                    edge.label.as_deref().unwrap_or(""),
                    label_limit,
                    TEXT_SIZE,
                    400,
                    self.palette.muted,
                )
            })
            .collect();
        let tallest_label = labels.iter().map(|text| text.height).fold(0.0, f64::max);
        let widest_label = labels.iter().map(text_width).fold(0.0, f64::max);
        let mut cells: Vec<_> = (0..region.nodes.len()).map(|_| None).collect();
        let layers_per_row = if vertical { 1 } else { count.min(3) };
        let mut notes = vec![Vec::new(); region.nodes.len()];
        for (index, note) in region.notes.iter().enumerate() {
            notes
                .get_mut(note.target)
                .ok_or_else(|| LayoutError("Invalid state note target".into()))?
                .push((index, note));
        }
        for rank in &order {
            let requested = if vertical { rank.len() } else { layers_per_row };
            let slots =
                requested.min(((inner + CELL_GAP) / (106.0 + CELL_GAP)).floor().max(1.0) as usize);
            let limit = ((inner + CELL_GAP) / slots as f64 - CELL_GAP).max(72.0);
            for &index in rank {
                let node = &region.nodes[index];
                let key = format!("{prefix}state-{}", node.id);
                cells[index] = Some(self.cell(node, &notes[index], key, limit, vertical)?);
            }
        }
        let cells: Vec<_> = cells.into_iter().flatten().collect();
        let (rows, gap_x, gap_y) = if vertical {
            let rows = order
                .into_iter()
                .flat_map(|rank| {
                    pack(
                        rank.into_iter().map(|index| vec![index]),
                        &cells,
                        inner,
                        CELL_GAP,
                    )
                })
                .collect();
            (rows, CELL_GAP, (tallest_label + 28.0).max(46.0))
        } else {
            let gap = (widest_label + 24.0).clamp(44.0, 115.0);
            (
                pack(order, &cells, inner, gap),
                gap,
                (tallest_label + 34.0).max(52.0),
            )
        };
        // 标签可占用首尾的留白，长反馈边也有容器外侧通道。
        let end_pad = if vertical {
            SIDE
        } else {
            SIDE + tallest_label / 2.0
        };
        let (mut layer, positions) =
            place_cells(&rows, &cells, (gap_x, gap_y), end_pad, (vertical, reverse));
        let previous_width = layer.width;
        if widest_label > 0.0 {
            let loop_room = region
                .transitions
                .iter()
                .filter(|edge| edge.from == edge.to)
                .map(|edge| cells[edge.from].rect.width / 2.0 + 28.0)
                .fold(0.0, f64::max);
            layer.width = layer
                .width
                .max((2.0 * (widest_label + loop_room + 20.0)).min(width));
        }
        let dx = (layer.width - previous_width) / 2.0;
        let positions: Vec<_> = positions
            .into_iter()
            .map(|point| Point {
                x: point.x + dx,
                ..point
            })
            .collect();
        self.emit_region(
            &mut layer, cells, positions, graph, &accepted, labels, region, prefix, background,
        )?;
        Ok(layer)
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_region(
        &mut self,
        layer: &mut Layer,
        cells: Vec<node::Cell>,
        positions: Vec<Point>,
        mut graph: Graph,
        accepted: &[bool],
        labels: Vec<TextLayout>,
        region: &StateRegion,
        prefix: &str,
        background: Color,
    ) -> Result<(), LayoutError> {
        let mut obstacles: Vec<_> = cells
            .iter()
            .zip(&positions)
            .map(|(cell, position)| (offset_rect(cell.rect, *position), cell.shape))
            .collect();
        for (cell, position) in cells.iter().zip(&positions) {
            for note in &cell.notes {
                obstacles.push((offset_rect(note.rect, *position), Shape::Rounded));
                graph.nodes.push(route_node("", note.source_range.clone()));
            }
        }
        layer.overlaps = obstacles
            .iter()
            .enumerate()
            .map(|(index, (rect, _))| {
                obstacles[..index]
                    .iter()
                    .filter(|(other, _)| rect.intersects(*other))
                    .count()
            })
            .sum();
        let label_sizes: Vec<_> = labels
            .iter()
            .map(|text| (text.height > 0.0).then(|| (text_width(text) + 8.0, text.height + 4.0)))
            .collect();
        let routes = if graph.edges.is_empty() {
            None
        } else {
            Some(route_state(
                &graph,
                &obstacles,
                accepted,
                (layer.width, layer.height),
                &label_sizes,
            )?)
        };
        let mut edge_hits = Layer::default();
        if let Some(routes) = routes {
            for (index, (points, label)) in routes.paths.into_iter().zip(labels).enumerate() {
                let transition = &region.transitions[index];
                let hit_rect = routes.labels[index].unwrap_or_else(|| path_hit_rect(&points));
                let edge_id = self.path(layer, points, PathMark::Arrow(transition));
                if let Some(rect) = routes.labels[index] {
                    layer.primitives.push(Primitive::LabelBackground {
                        edge_id,
                        rect,
                        color: background,
                    });
                    layer.text(label, rect.x + 6.0, rect.y + 2.0);
                }
                let label = transition
                    .label
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        format!(
                            "{} → {}",
                            region.nodes[transition.from].label, region.nodes[transition.to].label
                        )
                    });
                edge_hits.hit(
                    format!("{prefix}transition-{index}"),
                    label,
                    hit_rect,
                    transition.source_range.clone(),
                );
            }
        }
        for (cell, position) in cells.into_iter().zip(positions) {
            layer.append(cell.layer, position.x, position.y);
        }
        layer.hits.append(&mut edge_hits.hits);
        Ok(())
    }
}

enum PathMark<'a> {
    Arrow(&'a StateTransition),
    Dotted(Range<usize>),
}

fn axes(direction: Direction) -> (bool, bool) {
    match direction {
        Direction::TopToBottom => (true, false),
        Direction::BottomToTop => (true, true),
        Direction::LeftToRight => (false, false),
        Direction::RightToLeft => (false, true),
    }
}

fn route_node(id: &str, source_range: Range<usize>) -> GraphNode {
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

fn routing_graph(region: &StateRegion) -> Result<Graph, LayoutError> {
    let nodes: Vec<_> = region
        .nodes
        .iter()
        .map(|node| route_node(&node.id, node.source_range.clone()))
        .collect();
    let edges = region
        .transitions
        .iter()
        .enumerate()
        .map(|(id, edge)| {
            if edge.from >= nodes.len() || edge.to >= nodes.len() {
                return Err(LayoutError("Invalid state transition endpoint".into()));
            }
            Ok(Edge {
                id,
                from: edge.from,
                to: edge.to,
                source_range: edge.source_range.clone(),
                connector_range: edge.source_range.clone(),
                label: edge.label.clone(),
                label_range: edge.label_range.clone(),
                connector: Connector {
                    end: Marker::Arrow,
                    ..Connector::default()
                },
                visible: true,
                container: None,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Graph {
        direction: region.direction,
        nodes,
        edges,
        ..Graph::default()
    })
}

type Rows = Vec<Vec<Vec<usize>>>;

fn column_width(column: &[usize], cells: &[node::Cell]) -> f64 {
    column
        .iter()
        .map(|&i| cells[i].layer.width)
        .fold(0.0, f64::max)
}

fn pack(
    columns: impl IntoIterator<Item = Vec<usize>>,
    cells: &[node::Cell],
    limit: f64,
    gap: f64,
) -> Rows {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut width = 0.0;
    for column in columns {
        let next = column_width(&column, cells);
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

fn row_widths(rows: &Rows, cells: &[node::Cell], gap: f64) -> Vec<f64> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|column| column_width(column, cells))
                .sum::<f64>()
                + gap * row.len().saturating_sub(1) as f64
        })
        .collect()
}

fn place_cells(
    rows: &Rows,
    cells: &[node::Cell],
    (gap_x, gap_y): (f64, f64),
    end_pad: f64,
    (vertical, reverse): (bool, bool),
) -> (Layer, Vec<Point>) {
    let row_widths = row_widths(rows, cells, gap_x);
    let width = row_widths.iter().copied().fold(0.0, f64::max) + 2.0 * SIDE;
    let column_height = |column: &[usize]| {
        column.iter().map(|&i| cells[i].layer.height).sum::<f64>()
            + CELL_GAP * column.len().saturating_sub(1) as f64
    };
    let mut positions = vec![Point::default(); cells.len()];
    let mut top = end_pad;
    for (row, row_width) in rows.iter().zip(row_widths) {
        let row_height = row.iter().map(|col| column_height(col)).fold(0.0, f64::max);
        let mut x = (width - row_width) / 2.0;
        for column in row {
            let w = column_width(column, cells);
            let mut y = top + (row_height - column_height(column)) / 2.0;
            for &i in column {
                let cell = &cells[i].layer;
                let left = x + (w - cell.width) / 2.0;
                positions[i] = Point {
                    x: if !vertical && reverse {
                        width - left - cell.width
                    } else {
                        left
                    },
                    y,
                };
                y += cell.height + CELL_GAP;
            }
            x += w + gap_x;
        }
        top += row_height + gap_y;
    }
    let height = top - gap_y + end_pad;
    if vertical && reverse {
        for (point, cell) in positions.iter_mut().zip(cells) {
            point.y = height - point.y - cell.layer.height;
        }
    }
    (
        Layer {
            width,
            height,
            ..Layer::default()
        },
        positions,
    )
}

fn offset_rect(mut rect: Rect, offset: Point) -> Rect {
    rect.x += offset.x;
    rect.y += offset.y;
    rect
}

fn path_hit_rect(points: &[Point]) -> Rect {
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

pub fn layout_state(
    diagram: &StateDiagram,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<Scene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if diagram.root.nodes.is_empty() {
        return Err(LayoutError("State diagram has no states".into()));
    }
    let palette = Palette::new(options.theme);
    let mut layout = Layout {
        diagram,
        fonts,
        palette,
        next_path: 0,
    };
    let title = layout.text(
        diagram.title.as_deref().unwrap_or(""),
        options.width - 2.0 * SIDE,
        16.5,
        600,
        palette.text,
    );
    let region = layout.prepare_region(&diagram.root, "", options.width, palette.background)?;
    let title_top = if title.height > 0.0 { 18.0 } else { 0.0 };
    let height = title_top + title.height + region.height;
    let x = (options.width - region.width).max(0.0) / 2.0;
    let title_x = (options.width - text_width(&title)).max(0.0) / 2.0;
    let mut layer = Layer::default();
    layer.primitives.push(Primitive::RoundedRect {
        rect: Rect {
            width: options.width,
            height,
            ..Rect::default()
        },
        radius: 14.0,
        color: palette.background,
    });
    let horizontal_overflow =
        region.width > options.width + 0.001 || text_width(&title) > options.width;
    let region_top = title_top + title.height;
    layer.text(title, title_x, title_top);
    layer.append(region, x, region_top);
    let min_font_px = layer
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Text(span) => Some(span.font_size),
            _ => None,
        })
        .reduce(f64::min)
        .unwrap_or(TEXT_SIZE);
    Ok(Scene {
        width: options.width,
        height,
        primitives: layer.primitives,
        hit_regions: layer.hits,
        metrics: Metrics {
            columns: 1,
            diagram_height: height,
            min_font_px,
            horizontal_overflow,
            node_overlaps: layer.overlaps,
            fonts: layout.fonts.used_fonts(),
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue: None,
            branch_hues: Vec::new(),
            diagram_kind: "state".into(),
        },
    })
}
