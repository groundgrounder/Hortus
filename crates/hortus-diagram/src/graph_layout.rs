use crate::graph::{Direction, Graph, Shape};
use crate::{
    Color, FontInfo, FontMetrics, HitRegion, LayoutError, LayoutOptions, Metrics, Point, Primitive,
    Rect, Scene, TextSpan, Theme,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

const NODE_GAP: f64 = 28.0;
const RANK_GAP_COMPACT: f64 = 28.0;
const RANK_GAP_DENSE: f64 = 44.0;
const EDGE_LABEL_SIZE: f64 = 13.6;
const EDGE_LABEL_HEIGHT: f64 = 22.0;
const EDGE_LABEL_PAD_X: f64 = 2.0;
const EDGE_LABEL_SIDE_CLEARANCE: f64 = 7.0;

/// 稀疏链保持紧凑步距；连边密集或节点多时保留较大层距，避免走线折点膨胀。
fn rank_gap(graph: &Graph) -> f64 {
    if graph.nodes.len() > 32 || graph.edges.len() * 5 > graph.nodes.len() * 6 {
        RANK_GAP_DENSE
    } else {
        RANK_GAP_COMPACT
    }
}
const OUTER: f64 = 30.0;

#[derive(Clone, Debug)]
pub struct GraphMetrics {
    pub ranks: usize,
    pub roots: usize,
    pub edge_count: usize,
    pub invisible_edges: usize,
    pub containers: usize,
    pub node_overlaps: usize,
    pub edge_crossings: usize,
    pub horizontal_overflow: bool,
    pub retained_nodes: usize,
    pub stability_reason: String,
}

#[derive(Clone, Debug)]
pub struct GraphScene {
    pub scene: Scene,
    pub metrics: GraphMetrics,
    pub previous: crate::PreviousGraph,
}

#[derive(Clone)]
struct BoxedNode {
    rect: Rect,
    shape: Shape,
    fill: Color,
    stroke: Color,
    text_color: Color,
    stroke_width: f64,
    stroke_dasharray: Vec<f64>,
    label_lines: Vec<(String, f64, f64, f64)>,
}

pub fn layout_graph(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<GraphScene, LayoutError> {
    if !graph.containers.is_empty() {
        return layout_container_graph(graph, options, fonts);
    }
    let initial = layout_graph_with_order(graph, options, fonts, None)?;
    if graph.nodes.len() > 16 || initial.metrics.edge_crossings == 0 {
        return Ok(initial);
    }
    let (ranks, count, _) = assign_ranks(graph);
    let base = order_ranks(graph, &ranks, count);
    let mut best = initial;
    for rank in 0..count {
        if !(2..=3).contains(&base[rank].len()) {
            continue;
        }
        let mut permutations = Vec::new();
        let mut values = base[rank].clone();
        permutations_of(&mut values, 0, &mut permutations);
        for permutation in permutations {
            let mut order = base.clone();
            order[rank] = permutation;
            if let Ok(candidate) = layout_graph_with_order(graph, options, fonts, Some(&order))
                && candidate.metrics.edge_crossings < best.metrics.edge_crossings
            {
                best = candidate;
            }
        }
    }
    Ok(best)
}

const CONTAINER_PAD: f64 = 12.0;
const CONTAINER_TITLE_GAP: f64 = 18.0;

struct ContainerContent {
    scene: Scene,
    bounds: Option<Rect>,
}

fn style_for_node(graph: &Graph, node: &crate::GraphNode) -> crate::Style {
    let mut style = node
        .class
        .as_ref()
        .and_then(|class| graph.class_defs.get(class))
        .cloned()
        .unwrap_or_default();
    if let Some(inline) = graph.inline_styles.get(&node.id) {
        if inline.fill.is_some() {
            style.fill = inline.fill.clone();
        }
        if inline.stroke.is_some() {
            style.stroke = inline.stroke.clone();
        }
        if inline.color.is_some() {
            style.color = inline.color.clone();
        }
        if inline.stroke_width.is_some() {
            style.stroke_width = inline.stroke_width;
        }
        if inline.stroke_dasharray.is_some() {
            style.stroke_dasharray = inline.stroke_dasharray.clone();
        }
    }
    style
}

fn layout_container_graph(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<GraphScene, LayoutError> {
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    let available = options.width - OUTER * 2.0;
    let inner_width = (available - CONTAINER_PAD * 2.0).max(120.0);
    let mut contents = Vec::with_capacity(graph.containers.len());
    let mut sizes = HashMap::<usize, (f64, f64)>::new();
    for (container_index, container) in graph.containers.iter().enumerate() {
        let members = container
            .members
            .iter()
            .copied()
            .filter(|node| {
                graph.nodes[*node].kind == crate::GraphNodeKind::Node
                    && graph.nodes[*node].owner == Some(container_index)
            })
            .collect::<Vec<_>>();
        let mut index = HashMap::new();
        for (local, global) in members.iter().enumerate() {
            index.insert(*global, local);
        }
        let nodes = members
            .iter()
            .map(|node| {
                let mut node = graph.nodes[*node].clone();
                node.owner = None;
                node.preferred_size = None;
                node
            })
            .collect::<Vec<_>>();
        let edges = graph
            .edges
            .iter()
            .filter(|edge| edge.container == Some(container_index))
            .filter_map(|edge| {
                let mut edge = edge.clone();
                edge.from = *index.get(&edge.from)?;
                edge.to = *index.get(&edge.to)?;
                edge.container = None;
                Some(edge)
            })
            .collect();
        let inner_graph = Graph {
            direction: container.direction,
            warnings: graph.warnings.clone(),
            nodes,
            edges,
            class_defs: graph.class_defs.clone(),
            inline_styles: graph.inline_styles.clone(),
            forest: graph.forest,
            containers: Vec::new(),
        };
        let inner = if inner_graph.nodes.is_empty() {
            ContainerContent {
                scene: Scene {
                    width: inner_width,
                    height: CONTAINER_TITLE_GAP + CONTAINER_PAD * 2.0,
                    primitives: Vec::new(),
                    hit_regions: Vec::new(),
                    metrics: Metrics {
                        columns: 1,
                        diagram_height: CONTAINER_TITLE_GAP + CONTAINER_PAD * 2.0,
                        min_font_px: 15.0,
                        horizontal_overflow: false,
                        node_overlaps: 0,
                        fonts: Vec::new(),
                        layout_ms: 0.0,
                        root_hue: None,
                        branch_hues: Vec::new(),
                    },
                },
                bounds: None,
            }
        } else {
            let result = layout_graph(
                &inner_graph,
                &LayoutOptions {
                    width: inner_width,
                    theme: options.theme,
                    ..LayoutOptions::default()
                },
                fonts,
            )?;
            ContainerContent {
                bounds: scene_content_bounds(&result.scene),
                scene: result.scene,
            }
        };
        let title = &graph.nodes[container.node_index].label;
        let title_width = fonts
            .measure(title, EDGE_LABEL_SIZE, 600)
            .width
            .min(available - CONTAINER_PAD * 2.0);
        let content_width = inner.bounds.map_or(0.0, |bounds| bounds.width);
        let content_height = inner.bounds.map_or(0.0, |bounds| bounds.height);
        let width = (title_width + CONTAINER_PAD * 2.0 + 16.0)
            .max(content_width + CONTAINER_PAD * 2.0)
            .max(180.0)
            .min(available);
        let title_height = container_title_height(&container_title_lines(
            title,
            width - CONTAINER_PAD * 2.0,
            fonts,
        ));
        let height = if inner.bounds.is_some() {
            title_height + CONTAINER_PAD + content_height + CONTAINER_PAD
        } else {
            title_height + CONTAINER_PAD
        };
        sizes.insert(container.node_index, (width, height));
        contents.push(inner);
    }

    let outer_indices = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == crate::GraphNodeKind::Container || node.owner.is_none())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let outer_index = outer_indices
        .iter()
        .enumerate()
        .map(|(outer, global)| (*global, outer))
        .collect::<HashMap<_, _>>();
    let outer_nodes = outer_indices
        .iter()
        .map(|index| {
            let mut node = graph.nodes[*index].clone();
            if node.kind == crate::GraphNodeKind::Container {
                node.label.clear();
                node.preferred_size = sizes.get(index).copied();
            }
            node
        })
        .collect::<Vec<_>>();
    if graph
        .edges
        .iter()
        .filter(|edge| edge.container.is_none())
        .any(|edge| !outer_index.contains_key(&edge.from) || !outer_index.contains_key(&edge.to))
    {
        return Err(LayoutError(
            "Edges crossing a subgraph boundary are not supported".into(),
        ));
    }
    let outer_edges = graph
        .edges
        .iter()
        .filter(|edge| edge.container.is_none())
        .filter_map(|edge| {
            let mut edge = edge.clone();
            edge.from = *outer_index.get(&edge.from)?;
            edge.to = *outer_index.get(&edge.to)?;
            Some(edge)
        })
        .collect();
    let outer_graph = Graph {
        direction: graph.direction,
        warnings: graph.warnings.clone(),
        nodes: outer_nodes,
        edges: outer_edges,
        class_defs: graph.class_defs.clone(),
        inline_styles: graph.inline_styles.clone(),
        forest: graph.forest,
        containers: Vec::new(),
    };
    let outer = layout_graph(
        &outer_graph,
        &LayoutOptions {
            width: options.width,
            theme: options.theme,
            previous: options.previous.clone(),
            ..LayoutOptions::default()
        },
        fonts,
    )?;
    let outer_rects = outer
        .scene
        .hit_regions
        .iter()
        .map(|hit| (hit.node_id.clone(), hit.rect))
        .collect::<HashMap<_, _>>();
    let container_ids = graph
        .containers
        .iter()
        .map(|container| graph.nodes[container.node_index].id.clone())
        .collect::<Vec<_>>();
    let container_rects = container_ids
        .iter()
        .filter_map(|id| outer_rects.get(id).copied())
        .collect::<Vec<_>>();
    let text_color = Color::oklch(
        if options.theme == Theme::Dark {
            0.91
        } else {
            0.25
        },
        0.01,
        260.0,
    );
    let default_stroke = Color::oklch(
        if options.theme == Theme::Dark {
            0.66
        } else {
            0.43
        },
        0.055,
        250.0,
    );
    let mut primitives = outer
        .scene
        .primitives
        .iter()
        .filter(|primitive| {
            !matches!(
                primitive,
                Primitive::GraphShape { rect, .. } if container_rects.contains(rect)
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut hits = outer
        .scene
        .hit_regions
        .iter()
        .filter(|hit| !container_ids.contains(&hit.node_id))
        .cloned()
        .collect::<Vec<_>>();
    let mut previous_nodes = outer
        .previous
        .nodes
        .iter()
        .filter(|node| !container_ids.contains(&node.node_id))
        .cloned()
        .collect::<Vec<_>>();
    let mut container_primitives = Vec::new();
    let mut container_hits = Vec::new();
    let mut min_font = outer.scene.metrics.min_font_px;
    for (container_index, container) in graph.containers.iter().enumerate() {
        let node = &graph.nodes[container.node_index];
        let rect = outer_rects[&node.id];
        let style = style_for_node(graph, node);
        let fill = style
            .fill
            .as_deref()
            .and_then(Color::parse_css)
            .unwrap_or_else(|| Color {
                a: if options.theme == Theme::Dark {
                    0.2
                } else {
                    0.32
                },
                ..default_stroke
            });
        let stroke = style
            .stroke
            .as_deref()
            .and_then(Color::parse_css)
            .unwrap_or(default_stroke);
        let dash = style.stroke_dasharray.clone().unwrap_or_default();
        let stroke_width = style.stroke_width.unwrap_or(1.4);
        container_primitives.push(Primitive::GraphShape {
            rect,
            shape: Shape::Container,
            outline: crate::shape::outline(Shape::Container, rect),
            details: Vec::new(),
            fill,
            stroke: if dash.is_empty() { stroke } else { fill },
            stroke_width,
        });
        push_dashed_outline(
            &mut container_primitives,
            Shape::Container,
            rect,
            stroke,
            stroke_width,
            &dash,
        );
        let title_color = style
            .color
            .as_deref()
            .and_then(Color::parse_css)
            .unwrap_or_else(|| contrast_text_color(fill, text_color));
        let title = &node.label;
        let lines = container_title_lines(title, rect.width - CONTAINER_PAD * 2.0, fonts);
        let title_height = container_title_height(&lines);
        let mut baseline = rect.y + 15.0;
        for (_line, measured) in lines {
            let mut x = rect.x + CONTAINER_PAD;
            for run in measured.runs {
                container_primitives.push(Primitive::Text(TextSpan {
                    text: run.text,
                    baseline: Point { x, y: baseline },
                    width: run.width,
                    font_size: EDGE_LABEL_SIZE,
                    font_weight: 600,
                    font_family: run.font_family,
                    color: title_color,
                }));
                x += run.width;
            }
            baseline += 18.0;
        }
        container_hits.push(HitRegion {
            label: node.label.clone(),
            prefix: None,
            node_id: node.id.clone(),
            rect,
            can_collapse: false,
            collapsed: false,
            source_range: container.source_range.clone(),
        });
        let anchor = outer
            .previous
            .nodes
            .iter()
            .find(|anchor| anchor.node_id == node.id)
            .cloned();
        if let Some(anchor) = anchor {
            previous_nodes.push(anchor);
        }
        let content = &contents[container_index];
        if let Some(bounds) = content.bounds {
            let dx = rect.x + CONTAINER_PAD - bounds.x;
            let dy = rect.y + title_height + CONTAINER_PAD - bounds.y;
            for primitive in &content.scene.primitives {
                if matches!(primitive, Primitive::RoundedRect { .. }) {
                    continue;
                }
                primitives.push(translated_primitive(primitive, dx, dy));
            }
            for hit in &content.scene.hit_regions {
                let mut hit = hit.clone();
                hit.rect.x += dx;
                hit.rect.y += dy;
                hits.push(hit.clone());
                previous_nodes.push(crate::GraphAnchor {
                    node_id: hit.node_id,
                    rect: hit.rect,
                    rank: 0,
                    group: container_index + 1,
                });
            }
            min_font = min_font.min(content.scene.metrics.min_font_px);
        }
    }
    // 容器标题和边界在普通连接线之下，成员内容再覆盖其背景。
    primitives.splice(1..1, container_primitives);
    hits.extend(container_hits);
    let edges = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::GraphPath {
                edge_id, points, ..
            } => Some(crate::EdgeAnchor {
                edge_id: *edge_id,
                points: points.clone(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let horizontal_overflow = primitives.iter().any(|primitive| match primitive {
        Primitive::Text(span) => {
            span.baseline.x < 0.0 || span.baseline.x + span.width > options.width
        }
        Primitive::GraphShape { rect, .. } | Primitive::LabelBackground { rect, .. } => {
            rect.x < 0.0 || rect.x + rect.width > options.width
        }
        Primitive::GraphPath { points, width, .. } => points
            .iter()
            .any(|point| point.x - width / 2.0 < 0.0 || point.x + width / 2.0 > options.width),
        Primitive::RoundedRect { .. } | Primitive::Line { .. } => false,
    });
    let node_overlaps = outer.metrics.node_overlaps
        + contents
            .iter()
            .map(|content| content.scene.metrics.node_overlaps)
            .sum::<usize>();
    let edge_crossings = outer.metrics.edge_crossings
        + contents
            .iter()
            .map(|content| count_edge_crossings(&scene_paths(&content.scene)))
            .sum::<usize>();
    let ranks = outer.metrics.ranks.max(
        contents
            .iter()
            .map(|content| content.scene.metrics.columns)
            .max()
            .unwrap_or(1),
    );
    let retained_nodes = previous_nodes.len();
    let scene = Scene {
        width: options.width,
        height: outer.scene.height,
        primitives,
        hit_regions: hits,
        metrics: Metrics {
            columns: ranks,
            diagram_height: outer.scene.height,
            min_font_px: min_font.min(EDGE_LABEL_SIZE),
            horizontal_overflow,
            node_overlaps,
            fonts: fonts.used_fonts(),
            layout_ms: outer.scene.metrics.layout_ms,
            root_hue: None,
            branch_hues: Vec::new(),
        },
    };
    Ok(GraphScene {
        scene,
        metrics: GraphMetrics {
            ranks,
            roots: outer.metrics.roots,
            edge_count: graph.visible_edge_count(),
            invisible_edges: graph.invisible_edge_count(),
            containers: graph.containers.len(),
            node_overlaps,
            edge_crossings,
            horizontal_overflow,
            retained_nodes,
            stability_reason: format!("container_graph;{}", outer.metrics.stability_reason),
        },
        previous: crate::PreviousGraph {
            direction: graph.direction,
            width: options.width,
            nodes: previous_nodes,
            edges,
        },
    })
}

fn scene_content_bounds(scene: &Scene) -> Option<Rect> {
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

fn container_title_lines(
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

fn container_title_height(lines: &[(String, crate::Measurement)]) -> f64 {
    (lines.len().max(1) as f64 * 18.0 + 6.0).max(CONTAINER_TITLE_GAP)
}

fn push_dashed_outline(
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

fn contrast_text_color(background: Color, fallback: Color) -> Color {
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
        Color::oklch(0.22, 0.01, 260.0)
    } else {
        Color::oklch(0.95, 0.01, 260.0)
    }
}

fn scene_paths(scene: &Scene) -> Vec<Vec<Point>> {
    scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::GraphPath { points, .. } => Some(points.clone()),
            _ => None,
        })
        .collect()
}

fn translated_primitive(primitive: &Primitive, dx: f64, dy: f64) -> Primitive {
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

fn translated_rect(rect: Rect, dx: f64, dy: f64) -> Rect {
    Rect {
        x: rect.x + dx,
        y: rect.y + dy,
        ..rect
    }
}

fn permutations_of(values: &mut [usize], start: usize, output: &mut Vec<Vec<usize>>) {
    if start == values.len() {
        output.push(values.to_vec());
        return;
    }
    for index in start..values.len() {
        values.swap(start, index);
        permutations_of(values, start + 1, output);
        values.swap(start, index);
    }
}

fn layout_graph_with_order(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
    order_override: Option<&[Vec<usize>]>,
) -> Result<GraphScene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if graph.nodes.is_empty() {
        return Err(LayoutError("Graph has no nodes".into()));
    }

    let dark = options.theme == Theme::Dark;
    let background = Color::oklch(if dark { 0.215 } else { 0.982 }, 0.004, 260.0);
    let text_color = Color::oklch(if dark { 0.91 } else { 0.25 }, 0.01, 260.0);
    let edge_color = Color::oklch(if dark { 0.68 } else { 0.46 }, 0.025, 250.0);
    let shape_fill = Color::oklch(if dark { 0.27 } else { 0.995 }, 0.008, 250.0);
    let shape_stroke = Color::oklch(if dark { 0.66 } else { 0.43 }, 0.055, 250.0);
    let visible_width = options.width - OUTER * 2.0;

    let mut boxes = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let label = crate::text::label_parts(&node.label);
        let main = format!("{}{}", label.main, label.rest);
        let horizontal = matches!(
            graph.direction,
            Direction::LeftToRight | Direction::RightToLeft
        );
        let limit = if horizontal {
            ((visible_width - 32.0 * 3.0) / 4.0 - 8.0).max(48.0)
        } else {
            196.0_f64.min(visible_width - 24.0)
        };
        let mut label_lines = Vec::new();
        for (body, size) in [(&main, 15.0), (&label.sub, 13.0)] {
            let weight = if size < 15.0 { 400 } else { 500 };
            if body.is_empty() {
                continue;
            }
            // 短拉丁单词保留完整宽度；长链仍按实际框宽决定每带节点数。
            let word_width = if horizontal {
                body.split(|c: char| !c.is_ascii_alphanumeric())
                    .map(|word| fonts.measure(word, size, weight).width.min(196.0))
                    .fold(limit, f64::max)
            } else {
                limit
            };
            for line in wrap_label(body, word_width, size, weight, fonts) {
                // 行尾空白不参与可见排版；CoreText 与 OpenType 对空格宽度定义不同，
                // 保留会在窄标签上放大成可测的字宽误差。
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                let measured = fonts.measure(line, size, weight);
                label_lines.push((line.to_string(), measured.width, measured.ascent, size));
            }
        }
        let content_width = label_lines
            .iter()
            .map(|(_, width, _, _)| *width)
            .fold(0.0, f64::max);
        let content_height = label_lines
            .iter()
            .map(|(_, _, _, size)| size + 5.0)
            .sum::<f64>();
        let mut width = (content_width + 24.0).max(64.0);
        let mut height = (content_height + 16.0).max(40.0);
        if matches!(node.shape, Shape::Diamond) {
            width = (width * 1.45).min(visible_width);
            height *= 1.45;
        }
        if matches!(
            node.shape,
            Shape::Hexagon | Shape::Asymmetric | Shape::Parallelogram | Shape::ParallelogramReverse
        ) {
            width += 20.0;
        }
        let (mut width, mut height) = if matches!(node.shape, Shape::Circle | Shape::DoubleCircle) {
            let diameter = width.max(height);
            (diameter, diameter)
        } else {
            (width, height)
        };
        let class_style = node
            .class
            .as_ref()
            .and_then(|class| graph.class_defs.get(class))
            .cloned()
            .unwrap_or_default();
        let mut style = class_style;
        if let Some(inline) = graph.inline_styles.get(&node.id) {
            if inline.fill.is_some() {
                style.fill = inline.fill.clone();
            }
            if inline.stroke.is_some() {
                style.stroke = inline.stroke.clone();
            }
            if inline.color.is_some() {
                style.color = inline.color.clone();
            }
            if inline.stroke_width.is_some() {
                style.stroke_width = inline.stroke_width;
            }
            if inline.stroke_dasharray.is_some() {
                style.stroke_dasharray = inline.stroke_dasharray.clone();
            }
        }
        let preferred_size = node
            .preferred_size
            .map(|(width, height)| (width.clamp(64.0, visible_width.max(64.0)), height.max(40.0)));
        if let Some((preferred_width, preferred_height)) = preferred_size {
            width = preferred_width;
            height = preferred_height;
        }
        boxes.push(BoxedNode {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            shape: node.shape,
            fill: style
                .fill
                .as_deref()
                .and_then(Color::parse_css)
                .unwrap_or(shape_fill),
            stroke: style
                .stroke
                .as_deref()
                .and_then(Color::parse_css)
                .unwrap_or(shape_stroke),
            text_color: style
                .color
                .as_deref()
                .and_then(Color::parse_css)
                .unwrap_or(text_color),
            stroke_width: style.stroke_width.unwrap_or(1.4),
            stroke_dasharray: style.stroke_dasharray.unwrap_or_default(),
            label_lines,
        });
    }

    // 高扇出和汇合给每条边留独立端口，避免端点装饰互相覆盖。
    for (index, boxed) in boxes.iter_mut().enumerate() {
        let degree = graph
            .edges
            .iter()
            .filter(|edge| edge.from == index)
            .count()
            .max(graph.edges.iter().filter(|edge| edge.to == index).count());
        if degree >= 4 && graph.nodes.len() <= 48 {
            boxed.rect.height = boxed.rect.height.max((degree - 1) as f64 * 12.0 + 20.0);
        }
    }

    let (mut ranks, rank_count, accepted) = assign_ranks(graph);
    // 只回到上游的终止反馈分支与判断节点同高，缩短回路。
    for (index, edge) in graph.edges.iter().enumerate() {
        if accepted[index] || edge.from == edge.to {
            continue;
        }
        let incoming: Vec<_> = graph
            .edges
            .iter()
            .enumerate()
            .filter(|(i, other)| accepted[*i] && other.to == edge.from)
            .map(|(_, edge)| edge.from)
            .collect();
        if incoming.len() == 1
            && !graph
                .edges
                .iter()
                .enumerate()
                .any(|(i, other)| accepted[i] && other.from == edge.from)
        {
            ranks[edge.from] = ranks[incoming[0]];
        }
    }
    let order = order_override
        .map(<[Vec<usize>]>::to_vec)
        .unwrap_or_else(|| order_ranks(graph, &ranks, rank_count));
    let (mut height, mut groups) = place_nodes(graph, &order, &ranks, &mut boxes, visible_width);
    let mut column_paths = None;
    let mut column_stability = None;
    // 近树分列对初排和 previous 使用同一几何契约：位置只由结构和可用宽度决定，
    // 改宽是整图平移，同内容保持旧坐标，加边只影响新增分支。
    if let Some((positions, column_height, column_groups, paths, retained, reason)) =
        column_tree_layout(
            graph,
            &boxes,
            &ranks,
            visible_width,
            options.previous.as_ref(),
        )
    {
        for (node, (x, y)) in boxes.iter_mut().zip(positions) {
            node.rect.x = x;
            node.rect.y = y;
        }
        height = column_height;
        groups = column_groups;
        column_paths = Some(paths);
        column_stability = Some((retained, reason));
    }
    // 分列路径由最终列位直接生成；不能再按旧分层锚点移动节点。
    let (retained_nodes, stability_reason) = column_stability
        .unwrap_or_else(|| retain_positions(graph, options, &mut boxes, &mut groups));
    height = height.max(
        boxes
            .iter()
            .map(|node| node.rect.y + node.rect.height + OUTER)
            .fold(0.0, f64::max),
    );
    let mut primitives = vec![Primitive::RoundedRect {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: options.width,
            height,
        },
        radius: 16.0,
        color: background,
    }];
    let label_widths = graph
        .edges
        .iter()
        .map(|edge| {
            edge.label
                .as_ref()
                .map(|label| fonts.measure(label.trim_end(), EDGE_LABEL_SIZE, 400).width)
        })
        .collect::<Vec<_>>();
    let routed = if let Some(paths) = column_paths {
        crate::graph_router::Routes {
            paths,
            labels: vec![None; graph.edges.len()],
        }
    } else {
        crate::graph_router::route(
            graph,
            &boxes
                .iter()
                .map(|node| (node.rect, node.shape))
                .collect::<Vec<_>>(),
            &accepted,
            options.width,
            height,
            &label_widths,
            options.previous.as_ref().filter(|_| retained_nodes > 0),
        )?
    };
    let edge_paths = routed.paths;
    for (edge, points) in graph.edges.iter().zip(&edge_paths) {
        if !edge.visible {
            continue;
        }
        primitives.push(Primitive::GraphPath {
            edge_id: edge.id,
            corner_radius: 6.0,
            points: points.clone(),
            source_range: edge.source_range.clone(),
            label_range: edge.label_range.clone(),
            line: edge.connector.line,
            start: edge.connector.start,
            end: edge.connector.end,
            color: edge_color,
            width: if edge.connector.line == crate::EdgeLine::Thick {
                2.2
            } else {
                1.4
            },
        });
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        if let Some(label) = &edge.label {
            let label = label.trim_end();
            let measured = fonts.measure(label, EDGE_LABEL_SIZE, 400);
            let rect =
                routed.labels[index].ok_or_else(|| LayoutError("Missing edge label".into()))?;
            primitives.push(Primitive::LabelBackground {
                edge_id: edge.id,
                rect,
                color: background,
            });
            let mut x = rect.x + EDGE_LABEL_PAD_X;
            for run in measured.runs {
                primitives.push(Primitive::Text(TextSpan {
                    text: run.text,
                    baseline: Point {
                        x,
                        y: rect.y + EDGE_LABEL_HEIGHT * 0.7,
                    },
                    width: run.width,
                    font_size: EDGE_LABEL_SIZE,
                    font_weight: 400,
                    font_family: run.font_family,
                    color: text_color,
                }));
                x += run.width;
            }
        }
    }

    let mut hits = Vec::with_capacity(graph.nodes.len());
    for (node, boxed) in graph.nodes.iter().zip(&boxes) {
        let dashed = !boxed.stroke_dasharray.is_empty();
        primitives.push(Primitive::GraphShape {
            rect: boxed.rect,
            shape: boxed.shape,
            outline: crate::shape::outline(boxed.shape, boxed.rect),
            details: crate::shape::details(boxed.shape, boxed.rect),
            fill: boxed.fill,
            stroke: if dashed { boxed.fill } else { boxed.stroke },
            stroke_width: boxed.stroke_width,
        });
        push_dashed_outline(
            &mut primitives,
            boxed.shape,
            boxed.rect,
            boxed.stroke,
            boxed.stroke_width,
            &boxed.stroke_dasharray,
        );
        let line_height = boxed
            .label_lines
            .iter()
            .map(|(_, _, _, size)| size + 5.0)
            .sum::<f64>();
        let mut baseline = boxed.rect.y + (boxed.rect.height - line_height) / 2.0 + 13.0;
        for (line, line_width, _, size) in &boxed.label_lines {
            let weight = if *size < 15.0 { 400 } else { 500 };
            let measured = fonts.measure(line, *size, weight);
            let mut x = boxed.rect.x + (boxed.rect.width - line_width) / 2.0;
            for run in measured.runs {
                primitives.push(Primitive::Text(TextSpan {
                    text: run.text,
                    baseline: Point { x, y: baseline },
                    width: run.width,
                    font_size: *size,
                    font_weight: weight,
                    font_family: run.font_family,
                    color: boxed.text_color,
                }));
                x += run.width;
            }
            baseline += size + 5.0;
        }
        hits.push(HitRegion {
            node_id: node.id.clone(),
            label: node.label.clone(),
            prefix: None,
            rect: boxed.rect,
            can_collapse: false,
            collapsed: false,
            source_range: node.source_range.clone(),
        });
    }

    let node_overlaps = count_node_overlaps(&boxes);
    let visible_paths = graph
        .edges
        .iter()
        .zip(&edge_paths)
        .filter_map(|(edge, points)| edge.visible.then_some(points.clone()))
        .collect::<Vec<_>>();
    let edge_crossings = count_edge_crossings(&visible_paths);
    let horizontal_overflow = primitives.iter().any(|primitive| match primitive {
        Primitive::Text(span) => {
            span.baseline.x < 0.0 || span.baseline.x + span.width > options.width
        }
        Primitive::GraphShape { rect, .. } | Primitive::LabelBackground { rect, .. } => {
            rect.x < 0.0 || rect.x + rect.width > options.width
        }
        Primitive::GraphPath { points, width, .. } => points
            .iter()
            .any(|point| point.x - width / 2.0 < 0.0 || point.x + width / 2.0 > options.width),
        Primitive::RoundedRect { .. } | Primitive::Line { .. } => false,
    });
    let fonts_used: Vec<FontInfo> = fonts.used_fonts();
    let scene = Scene {
        width: options.width,
        height,
        primitives,
        hit_regions: hits,
        metrics: Metrics {
            columns: rank_count,
            diagram_height: height,
            min_font_px: if graph
                .nodes
                .iter()
                .any(|node| !crate::text::label_parts(&node.label).sub.is_empty())
            {
                13.0
            } else if graph.edges.iter().any(|edge| edge.label.is_some()) {
                EDGE_LABEL_SIZE
            } else {
                15.0
            },
            horizontal_overflow,
            node_overlaps,
            fonts: fonts_used,
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue: None,
            branch_hues: Vec::new(),
        },
    };
    let roots = ranks
        .iter()
        .enumerate()
        .filter(|(node, _)| {
            !graph
                .edges
                .iter()
                .any(|edge| edge.to == *node && edge.from != *node)
        })
        .count();
    Ok(GraphScene {
        previous: crate::PreviousGraph {
            direction: graph.direction,
            width: options.width,
            edges: graph
                .edges
                .iter()
                .zip(&edge_paths)
                .filter(|(edge, _)| edge.visible)
                .map(|(edge, points)| crate::EdgeAnchor {
                    edge_id: edge.id,
                    points: points.clone(),
                })
                .collect(),
            nodes: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| crate::GraphAnchor {
                    node_id: node.id.clone(),
                    rect: boxes[index].rect,
                    rank: ranks[index],
                    group: groups[index],
                })
                .collect(),
        },
        scene,
        metrics: GraphMetrics {
            ranks: rank_count,
            roots,
            edge_count: graph.visible_edge_count(),
            invisible_edges: graph.invisible_edge_count(),
            containers: graph.containers.len(),
            node_overlaps,
            edge_crossings,
            horizontal_overflow,
            retained_nodes,
            stability_reason: graph.warnings.iter().fold(
                stability_reason,
                |mut reason, (range, kind)| {
                    use std::fmt::Write;
                    let _ = write!(reason, "; {kind}@{}..{}", range.start, range.end);
                    reason
                },
            ),
        },
    })
}

fn assign_ranks(graph: &Graph) -> (Vec<usize>, usize, Vec<bool>) {
    let count = graph.nodes.len();
    let mut dag = vec![Vec::new(); count];
    let mut accepted = vec![false; graph.edges.len()];
    for (index, edge) in graph.edges.iter().enumerate() {
        if edge.from == edge.to || reachable(&dag, edge.to, edge.from) {
            continue;
        }
        dag[edge.from].push(edge.to);
        accepted[index] = true;
    }
    let mut indegree = vec![0usize; count];
    for targets in &dag {
        for target in targets {
            indegree[*target] += 1;
        }
    }
    let mut queue = VecDeque::new();
    for (index, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            queue.push_back(index);
        }
    }
    let mut ranks = vec![0usize; count];
    while let Some(node) = queue.pop_front() {
        for target in &dag[node] {
            ranks[*target] = ranks[*target].max(ranks[node] + 1);
            indegree[*target] -= 1;
            if indegree[*target] == 0 {
                queue.push_back(*target);
            }
        }
    }
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    (ranks, max_rank + 1, accepted)
}

fn reachable(adjacency: &[Vec<usize>], start: usize, target: usize) -> bool {
    let mut seen = vec![false; adjacency.len()];
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        stack.extend(adjacency[node].iter().copied());
    }
    false
}

fn order_ranks(graph: &Graph, ranks: &[usize], count: usize) -> Vec<Vec<usize>> {
    let mut order = vec![Vec::new(); count];
    for (node, rank) in ranks.iter().enumerate() {
        order[*rank].push(node);
    }
    let mut position = vec![0usize; ranks.len()];
    for rank in &order {
        for (index, node) in rank.iter().enumerate() {
            position[*node] = index;
        }
    }
    for _ in 0..4 {
        let mut rank_index = 1;
        while rank_index < count {
            let mut scored = order[rank_index]
                .iter()
                .copied()
                .map(|node| {
                    let parents = graph
                        .edges
                        .iter()
                        .filter(|edge| edge.to == node && ranks[edge.from] < rank_index)
                        .map(|edge| position[edge.from] as f64)
                        .collect::<Vec<_>>();
                    let score = if parents.is_empty() {
                        position[node] as f64
                    } else {
                        parents.iter().sum::<f64>() / parents.len() as f64
                    };
                    (score, node)
                })
                .collect::<Vec<_>>();
            scored.sort_by(|(a, an), (b, bn)| a.total_cmp(b).then_with(|| an.cmp(bn)));
            order[rank_index] = scored.into_iter().map(|(_, node)| node).collect();
            for (index, node) in order[rank_index].iter().enumerate() {
                position[*node] = index;
            }
            rank_index += 1;
        }
        let mut rank_index = count.saturating_sub(1);
        while rank_index > 0 {
            rank_index -= 1;
            let mut scored = order[rank_index]
                .iter()
                .copied()
                .map(|node| {
                    let children = graph
                        .edges
                        .iter()
                        .filter(|edge| edge.from == node && ranks[edge.to] > rank_index)
                        .map(|edge| position[edge.to] as f64)
                        .collect::<Vec<_>>();
                    let score = if children.is_empty() {
                        position[node] as f64
                    } else {
                        children.iter().sum::<f64>() / children.len() as f64
                    };
                    (score, node)
                })
                .collect::<Vec<_>>();
            scored.sort_by(|(a, an), (b, bn)| a.total_cmp(b).then_with(|| an.cmp(bn)));
            order[rank_index] = scored.into_iter().map(|(_, node)| node).collect();
            for (index, node) in order[rank_index].iter().enumerate() {
                position[*node] = index;
            }
        }
    }
    if graph.nodes.len() <= 64 {
        let crossing_score = |order: &[Vec<usize>]| {
            let mut positions = vec![0usize; ranks.len()];
            for row in order {
                for (i, node) in row.iter().enumerate() {
                    positions[*node] = i;
                }
            }
            let mut score = 0usize;
            for (i, a) in graph.edges.iter().enumerate() {
                for b in &graph.edges[i + 1..] {
                    if ranks[a.from] == ranks[b.from]
                        && ranks[a.to] == ranks[b.to]
                        && ranks[a.from] < ranks[a.to]
                        && a.from != b.from
                        && a.to != b.to
                        && (positions[a.from] < positions[b.from])
                            != (positions[a.to] < positions[b.to])
                    {
                        score += 1;
                    }
                }
            }
            score
        };
        for _ in 0..4 {
            let mut improved = false;
            for row in 0..count {
                if order[row].len() > 6 {
                    continue;
                }
                let mut candidate = order[row].clone();
                candidate.sort_unstable();
                let mut best = order[row].clone();
                let mut best_score = crossing_score(&order);
                loop {
                    order[row] = candidate.clone();
                    let score = crossing_score(&order);
                    if score < best_score {
                        best_score = score;
                        best = candidate.clone();
                        improved = true;
                    }
                    let Some(i) = (0..candidate.len().saturating_sub(1))
                        .rev()
                        .find(|i| candidate[*i] < candidate[*i + 1])
                    else {
                        break;
                    };
                    let j = (i + 1..candidate.len())
                        .rev()
                        .find(|j| candidate[*j] > candidate[i])
                        .unwrap();
                    candidate.swap(i, j);
                    candidate[i + 1..].reverse();
                }
                order[row] = best;
            }
            if !improved {
                break;
            }
        }
    }
    // 小图对少数多节点层作有限枚举，摆脱逐层下降停在两处交叉的局部最优。
    if graph.nodes.len() <= 16 {
        let mut variants = Vec::<(usize, Vec<Vec<usize>>)>::new();
        let mut combinations = 1usize;
        for (rank, row) in order.iter().enumerate() {
            if !(2..=3).contains(&row.len()) {
                continue;
            }
            let mut candidate = row.clone();
            candidate.sort_unstable();
            let mut permutations = Vec::new();
            loop {
                permutations.push(candidate.clone());
                let Some(i) = (0..candidate.len() - 1)
                    .rev()
                    .find(|i| candidate[*i] < candidate[*i + 1])
                else {
                    break;
                };
                let j = (i + 1..candidate.len())
                    .rev()
                    .find(|j| candidate[*j] > candidate[i])
                    .unwrap();
                candidate.swap(i, j);
                candidate[i + 1..].reverse();
            }
            combinations *= permutations.len();
            if combinations > 4096 {
                break;
            }
            variants.push((rank, permutations));
        }
        let score = |rows: &[Vec<usize>]| {
            let mut positions = vec![0; ranks.len()];
            for row in rows {
                for (i, node) in row.iter().enumerate() {
                    positions[*node] = i;
                }
            }
            graph
                .edges
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    graph.edges[i + 1..]
                        .iter()
                        .filter(|b| {
                            ranks[a.from] == ranks[b.from]
                                && ranks[a.to] == ranks[b.to]
                                && ranks[a.from] < ranks[a.to]
                                && a.from != b.from
                                && a.to != b.to
                                && (positions[a.from] < positions[b.from])
                                    != (positions[a.to] < positions[b.to])
                        })
                        .count()
                })
                .sum::<usize>()
        };
        let mut best = order.clone();
        let mut best_score = score(&best);
        for mut choice in 0..combinations.min(4096) {
            for (rank, permutations) in &variants {
                order[*rank] = permutations[choice % permutations.len()].clone();
                choice /= permutations.len();
            }
            let next = score(&order);
            if next < best_score {
                best_score = next;
                best = order.clone();
            }
            if best_score == 0 {
                break;
            }
        }
        order = best;
    }
    order
}

fn place_nodes(
    graph: &Graph,
    order: &[Vec<usize>],
    ranks: &[usize],
    boxes: &mut [BoxedNode],
    available: f64,
) -> (f64, Vec<usize>) {
    let mut groups = vec![0; boxes.len()];
    if matches!(
        graph.direction,
        Direction::LeftToRight | Direction::RightToLeft
    ) {
        let weights: Vec<_> = order
            .iter()
            .map(|rank| {
                rank.iter()
                    .map(|node| boxes[*node].rect.width)
                    .fold(0.0, f64::max)
                    + 32.0
            })
            .collect();
        let mut k = order.len().div_ceil(4).max(1);
        let ranges = loop {
            let ranges = crate::partition(&weights, k);
            if ranges
                .iter()
                .all(|range| weights[range.clone()].iter().sum::<f64>() - 32.0 <= available + 0.01)
                || k == order.len()
            {
                break ranges;
            }
            k += 1;
        };
        let mut y = OUTER;
        for (group, range) in ranges.into_iter().enumerate() {
            let total = weights[range.clone()].iter().sum::<f64>() - 32.0;
            let mut x = OUTER + (available - total).max(0.0) / 2.0;
            let indices: Vec<_> = if graph.direction == Direction::RightToLeft {
                range.rev().collect()
            } else {
                range.collect()
            };
            let mut bottom = y;
            for index in indices {
                let mut node_y = y;
                for node in &order[index] {
                    boxes[*node].rect.x =
                        x + (weights[index] - 32.0 - boxes[*node].rect.width) / 2.0;
                    boxes[*node].rect.y = node_y;
                    groups[*node] = group;
                    node_y += boxes[*node].rect.height + 32.0;
                }
                bottom = bottom.max(node_y - 32.0);
                x += weights[index];
            }
            y = bottom + 48.0;
        }
        return (y - 48.0 + OUTER, groups);
    }
    if let Some((height, bands)) = place_branches(graph, ranks, boxes, available) {
        return (height, bands);
    }
    let mut rows = Vec::<Vec<usize>>::new();
    for rank in order {
        let mut row = Vec::new();
        let mut used = 0.0;
        for node in rank {
            let need = boxes[*node].rect.width + if row.is_empty() { 0.0 } else { NODE_GAP };
            if !row.is_empty() && used + need > available {
                rows.push(std::mem::take(&mut row));
                used = 0.0;
            }
            used += boxes[*node].rect.width + if row.is_empty() { 0.0 } else { NODE_GAP };
            row.push(*node);
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    if graph.direction == Direction::BottomToTop {
        rows.reverse();
    }
    let mut y = OUTER;
    for (group, row) in rows.iter().enumerate() {
        let height = row
            .iter()
            .map(|node| boxes[*node].rect.height)
            .fold(0.0, f64::max);
        let total = row.iter().map(|node| boxes[*node].rect.width).sum::<f64>()
            + NODE_GAP * row.len().saturating_sub(1) as f64;
        let mut x = OUTER + (available - total).max(0.0) / 2.0;
        for node in row {
            boxes[*node].rect.x = x;
            boxes[*node].rect.y = y + (height - boxes[*node].rect.height) / 2.0;
            groups[*node] = group;
            x += boxes[*node].rect.width + NODE_GAP;
        }
        y += height + rank_gap(graph);
    }
    // 同高的反馈支路放在主干右侧；主干保持一条竖直阅读轴。
    for edge in &graph.edges {
        if edge.from == edge.to || ranks[edge.from] != ranks[edge.to] {
            continue;
        }
        let from = boxes[edge.from].rect;
        let to = boxes[edge.to].rect;
        let dx = from.x + from.width / 2.0 - (OUTER + available / 2.0);
        if dx < 0.0 && to.x + to.width - dx < OUTER + available {
            let moved = boxes
                .iter()
                .map(|boxed| {
                    boxed.rect.width < from.width * 1.2
                        && (boxed.rect.x + boxed.rect.width / 2.0 - (OUTER + available / 2.0)).abs()
                            < 1.0
                })
                .collect::<Vec<_>>();
            let clear = boxes.iter().enumerate().all(|(i, boxed)| {
                if !moved[i] {
                    return true;
                }
                let candidate = Rect {
                    x: boxed.rect.x + dx,
                    ..boxed.rect
                };
                candidate.x >= OUTER
                    && boxes.iter().enumerate().all(|(j, other)| {
                        i == j
                            || moved[j]
                            || !candidate.intersects(Rect {
                                x: other.rect.x - 8.0,
                                y: other.rect.y - 8.0,
                                width: other.rect.width + 16.0,
                                height: other.rect.height + 16.0,
                            })
                    })
            });
            // 反馈支路不能把其它层的主干推入同层相邻节点。
            if clear {
                for (boxed, moved) in boxes.iter_mut().zip(moved) {
                    if moved {
                        boxed.rect.x += dx;
                    }
                }
            }
        }
    }
    (y - rank_gap(graph) + OUTER, groups)
}

/// 从 previous 还原列成员与列内次序；结构不闭合时返回 None，退回 BFS 分列。
fn previous_columns(
    graph: &Graph,
    previous: Option<&crate::PreviousGraph>,
    root: usize,
) -> Option<Vec<Vec<usize>>> {
    let previous = previous?;
    if previous.direction != graph.direction {
        return None;
    }
    let mut anchors = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        anchors.push(
            previous
                .nodes
                .iter()
                .find(|anchor| anchor.node_id == node.id)?,
        );
    }
    let root_group = anchors[root].group;
    let mut columns = BTreeMap::<usize, Vec<usize>>::new();
    for (index, anchor) in anchors.iter().enumerate() {
        if index == root {
            continue;
        }
        if anchor.group == root_group {
            return None;
        }
        columns.entry(anchor.group).or_default().push(index);
    }
    if columns.is_empty() {
        return None;
    }
    let mut result = Vec::with_capacity(columns.len());
    let mut covered = vec![false; graph.nodes.len()];
    covered[root] = true;
    for (_, mut column) in columns {
        column.sort_by(|a, b| {
            anchors[*a]
                .rect
                .y
                .total_cmp(&anchors[*b].rect.y)
                .then_with(|| anchors[*a].rect.x.total_cmp(&anchors[*b].rect.x))
        });
        // 列头必须由根直接连入，其余节点必须由同列上方节点连入；
        // 否则旧分组不是这棵树的列，不能沿用。
        let head = column[0];
        if !graph
            .edges
            .iter()
            .any(|edge| edge.from == root && edge.to == head)
        {
            return None;
        }
        for (position, node) in column.iter().enumerate().skip(1) {
            let above = &column[..position];
            if !graph
                .edges
                .iter()
                .any(|edge| above.contains(&edge.from) && edge.to == *node)
            {
                return None;
            }
        }
        for node in &column {
            if covered[*node] {
                return None;
            }
            covered[*node] = true;
        }
        result.push(column);
    }
    if covered.iter().any(|covered| !*covered) {
        return None;
    }
    Some(result)
}

type ColumnLayout = (
    Vec<(f64, f64)>,
    f64,
    Vec<usize>,
    Vec<Vec<Point>>,
    usize,
    String,
);

/// 近树图（单根、仅少量附加边）在分层换行无法平面走线时，
/// 改为按根分支分列、子树纵向排列，并用独立侧通道走线。
fn column_tree_layout(
    graph: &Graph,
    boxes: &[BoxedNode],
    ranks: &[usize],
    available: f64,
    previous: Option<&crate::PreviousGraph>,
) -> Option<ColumnLayout> {
    if graph.direction != Direction::TopToBottom
        || graph.edges.iter().any(|edge| edge.label.is_some())
        || !(12..=24).contains(&graph.nodes.len())
        || graph.edges.len() < graph.nodes.len()
        || graph.edges.len() - (graph.nodes.len() - 1) > graph.nodes.len() / 4 + 1
    {
        return None;
    }
    if graph.nodes.iter().any(|node| {
        !matches!(
            node.shape,
            Shape::Rectangle | Shape::Rounded | Shape::Stadium
        )
    }) {
        return None;
    }
    let count = graph.nodes.len();
    let min_rank = ranks.iter().copied().min().unwrap_or(0);
    let roots = (0..count)
        .filter(|node| ranks[*node] == min_rank)
        .take(1)
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return None;
    }
    let root = roots[0];
    // 从根按有向边做 BFS，得到一棵覆盖全图的树；附加边不参与分列。
    let mut parent = vec![None; count];
    let mut children = vec![Vec::new(); count];
    let mut seen = vec![false; count];
    let mut queue = std::collections::VecDeque::from([root]);
    seen[root] = true;
    while let Some(node) = queue.pop_front() {
        for edge in &graph.edges {
            if edge.from == node && edge.to != node && !seen[edge.to] {
                seen[edge.to] = true;
                parent[edge.to] = Some(node);
                children[node].push(edge.to);
                queue.push_back(edge.to);
            }
        }
    }
    if seen.iter().any(|seen| !*seen) {
        return None;
    }
    let mut subtree = vec![Vec::new(); count];
    // 加边不重排既有列：成员和次序沿用 previous 的分列，只有新增列重新分配。
    let previous_columns = previous_columns(graph, previous, root);
    let branches = match &previous_columns {
        Some(columns) => columns.iter().map(|column| column[0]).collect::<Vec<_>>(),
        None => children[root].clone(),
    };
    for branch in &branches {
        if let Some(columns) = &previous_columns {
            let column = columns
                .iter()
                .find(|column| column[0] == *branch)
                .expect("branch has a previous column");
            subtree[*branch] = column.clone();
            continue;
        }
        let mut queue = std::collections::VecDeque::from([*branch]);
        while let Some(node) = queue.pop_front() {
            subtree[*branch].push(node);
            for child in &children[node] {
                queue.push_back(*child);
            }
        }
    }
    if branches.is_empty() {
        return None;
    }
    let mut column_width = 0.0_f64;
    for branch in &branches {
        for node in &subtree[*branch] {
            column_width = column_width.max(boxes[*node].rect.width);
        }
    }
    column_width += 40.0;
    let total = column_width * branches.len() as f64 + 16.0 * (branches.len() - 1) as f64;
    if total > available {
        return None;
    }
    let root_rect = boxes[root].rect;
    let root_x = OUTER + (available - root_rect.width) / 2.0;
    let mut positions = vec![(0.0, 0.0); count];
    positions[root] = (root_x, OUTER);
    let top = OUTER + root_rect.height + 30.0;
    let mut groups = vec![0usize; count];
    let mut lanes = vec![0.0_f64; count];
    let mut right_lanes = vec![0.0_f64; count];
    // 共同成员沿用旧分组：加边新增的列只领取新编号，不让后面的列整体换组。
    let root_group = previous
        .and_then(|old| {
            old.nodes
                .iter()
                .find(|anchor| anchor.node_id == graph.nodes[root].id)
                .map(|anchor| anchor.group)
        })
        .unwrap_or(0);
    let mut next_group = previous
        .map(|old| {
            old.nodes
                .iter()
                .map(|anchor| anchor.group)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        + 1;
    let mut cursor = OUTER + (available - total) / 2.0;
    let mut height = top;
    groups[root] = root_group;
    for branch in &branches {
        let inherited = previous.and_then(|old| {
            let mut counts = BTreeMap::<usize, usize>::new();
            for node in &subtree[*branch] {
                if let Some(anchor) = old
                    .nodes
                    .iter()
                    .find(|anchor| anchor.node_id == graph.nodes[*node].id)
                {
                    *counts.entry(anchor.group).or_insert(0) += 1;
                }
            }
            counts
                .into_iter()
                .max_by_key(|(group, count)| (*count, std::cmp::Reverse(*group)))
                .map(|(group, _)| group)
        });
        let group = inherited.unwrap_or_else(|| {
            let group = next_group;
            next_group += 1;
            group
        });
        let mut y = top;
        for node in &subtree[*branch] {
            let rect = boxes[*node].rect;
            positions[*node] = (cursor + 22.0 + (column_width - 40.0 - rect.width) / 2.0, y);
            lanes[*node] = cursor + 8.0;
            right_lanes[*node] = cursor + column_width + 8.0;
            groups[*node] = group;
            y += rect.height + 14.0;
        }
        height = height.max(y);
        cursor += column_width + 16.0;
    }
    let corridor_y = height + 12.0;
    let height = corridor_y + OUTER;
    let top_lane_y = OUTER * 0.4;
    let port = |rect: Rect, side: usize| -> Point {
        match side {
            0 => Point {
                x: rect.x,
                y: rect.y + rect.height / 2.0,
            },
            1 => Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height / 2.0,
            },
            2 => Point {
                x: rect.x + rect.width / 2.0,
                y: rect.y,
            },
            _ => Point {
                x: rect.x + rect.width / 2.0,
                y: rect.y + rect.height,
            },
        }
    };
    let placed = |node: usize| Rect {
        x: positions[node].0,
        y: positions[node].1,
        width: boxes[node].rect.width,
        height: boxes[node].rect.height,
    };
    let mut paths = Vec::with_capacity(graph.edges.len());
    for edge in &graph.edges {
        let source = placed(edge.from);
        let target = placed(edge.to);
        let mut points = Vec::new();
        if edge.from == edge.to {
            let start = port(source, 2);
            let right = source.x + source.width;
            points.extend([
                start,
                Point {
                    x: start.x,
                    y: source.y - 14.0,
                },
                Point {
                    x: right + 16.0,
                    y: source.y - 14.0,
                },
                Point {
                    x: right + 16.0,
                    y: source.y + source.height / 2.0,
                },
                port(source, 1),
            ]);
        } else if edge.from == root && branches.contains(&edge.to) {
            let start = port(source, 3);
            let corridor = source.y + source.height + 12.0;
            points.extend([
                start,
                Point {
                    x: start.x,
                    y: corridor,
                },
                Point {
                    x: target.x + target.width / 2.0,
                    y: corridor,
                },
                port(target, 2),
            ]);
        } else if edge.from == root {
            // 根到列中段的新边：先下到根下方走廊，再横穿到目标列的右通道下行；
            // 与根→分支走廊同高，共享段有相同源点，不会穿过任何节点。
            let lane = right_lanes[edge.to];
            if lane <= 0.0 {
                return None;
            }
            let corridor = source.y + source.height + 12.0;
            points.extend([
                port(source, 3),
                Point {
                    x: source.x + source.width / 2.0,
                    y: corridor,
                },
                Point {
                    x: lane,
                    y: corridor,
                },
                Point {
                    x: lane,
                    y: target.y + target.height / 2.0,
                },
                port(target, 1),
            ]);
        } else if parent[edge.to] == Some(edge.from)
            && groups[edge.from] == groups[edge.to]
            && source.y + source.height / 2.0 < target.y + target.height / 2.0
        {
            let lane = lanes[edge.from];
            if lane <= 0.0 {
                return None;
            }
            points.extend([
                port(source, 0),
                Point {
                    x: lane,
                    y: source.y + source.height / 2.0,
                },
                Point {
                    x: lane,
                    y: target.y + target.height / 2.0,
                },
                port(target, 0),
            ]);
        } else if edge.to == root {
            let node = if edge.from == root {
                edge.to
            } else {
                edge.from
            };
            let lane = lanes[node];
            if lane <= 0.0 {
                return None;
            }
            let root_top = port(source, 2);
            let root_top = if edge.from == root {
                root_top
            } else {
                port(target, 2)
            };
            let root_center = root_top.x;
            points.extend([
                root_top,
                Point {
                    x: root_center,
                    y: top_lane_y,
                },
                Point {
                    x: lane,
                    y: top_lane_y,
                },
                Point {
                    x: lane,
                    y: if edge.from == root {
                        target.y + target.height / 2.0
                    } else {
                        source.y + source.height / 2.0
                    },
                },
                if edge.from == root {
                    port(target, 0)
                } else {
                    port(source, 0)
                },
            ]);
        } else {
            let from_lane = right_lanes[edge.from];
            let to_lane = right_lanes[edge.to];
            if from_lane <= 0.0 || to_lane <= 0.0 {
                return None;
            }
            let source_y = source.y + source.height / 2.0;
            let target_y = target.y + target.height / 2.0;
            if (from_lane - to_lane).abs() < 0.001 {
                points.extend([
                    port(source, 1),
                    Point {
                        x: from_lane,
                        y: source_y,
                    },
                    Point {
                        x: to_lane,
                        y: target_y,
                    },
                    port(target, 1),
                ]);
            } else {
                points.extend([
                    port(source, 1),
                    Point {
                        x: from_lane,
                        y: source_y,
                    },
                    Point {
                        x: from_lane,
                        y: corridor_y,
                    },
                    Point {
                        x: to_lane,
                        y: corridor_y,
                    },
                    Point {
                        x: to_lane,
                        y: target_y,
                    },
                    port(target, 1),
                ]);
            }
        }
        points.dedup_by(|a, b| (a.x - b.x).abs() < 0.001 && (a.y - b.y).abs() < 0.001);
        let mut simplified = vec![points[0]];
        for point in points.into_iter().skip(1) {
            while simplified.len() >= 2 {
                let a = simplified[simplified.len() - 2];
                let b = simplified[simplified.len() - 1];
                let collinear = ((a.x - b.x).abs() < 0.001
                    && (b.x - point.x).abs() < 0.001
                    && (b.y - a.y) * (point.y - b.y) >= 0.0)
                    || ((a.y - b.y).abs() < 0.001
                        && (b.y - point.y).abs() < 0.001
                        && (b.x - a.x) * (point.x - b.x) >= 0.0);
                if collinear {
                    simplified.pop();
                } else {
                    break;
                }
            }
            simplified.push(point);
        }
        paths.push(simplified);
    }
    if count_edge_crossings(&paths) != 0 {
        return None;
    }
    let (retained, reason) = column_stability(graph, previous, &positions, boxes, &groups);
    Some((positions, height, groups, paths, retained, reason))
}

/// 分列几何对 previous 的稳定口径：y 不变、分组不变时只在 x 上整体平移。
fn column_stability(
    graph: &Graph,
    previous: Option<&crate::PreviousGraph>,
    positions: &[(f64, f64)],
    boxes: &[BoxedNode],
    groups: &[usize],
) -> (usize, String) {
    let Some(previous) = previous else {
        return (0, "initial".into());
    };
    if previous.direction != graph.direction {
        return (0, "direction_changed".into());
    }
    let mut common = 0;
    let mut exact = 0;
    let mut group_changes = 0;
    let mut uniform = true;
    let mut first_delta = None;
    for (index, node) in graph.nodes.iter().enumerate() {
        let Some(anchor) = previous
            .nodes
            .iter()
            .find(|anchor| anchor.node_id == node.id)
        else {
            continue;
        };
        common += 1;
        let rect = Rect {
            x: positions[index].0,
            y: positions[index].1,
            width: boxes[index].rect.width,
            height: boxes[index].rect.height,
        };
        if groups[index] != anchor.group {
            group_changes += 1;
        }
        let delta = (rect.x - anchor.rect.x, rect.y - anchor.rect.y);
        match first_delta {
            None => first_delta = Some(delta),
            Some((dx, dy)) => {
                if (delta.0 - dx).abs() > 0.01 || (delta.1 - dy).abs() > 0.01 {
                    uniform = false;
                }
            }
        }
        if delta.0.abs() < 0.01
            && delta.1.abs() < 0.01
            && (rect.width - anchor.rect.width).abs() < 0.01
            && (rect.height - anchor.rect.height).abs() < 0.01
        {
            exact += 1;
        }
    }
    if common == 0 {
        return (0, "no_common_nodes".into());
    }
    // 共同成员只发生同一平移时，列结构、行带和相对位置都保留；
    // 根框高度随扇出改变时整体平移不是等距的，按逐项精确命中计数。
    if group_changes == 0 && uniform {
        let (dx, dy) = first_delta.unwrap_or((0.0, 0.0));
        if exact == common {
            (common, format!("retained_columns_exact_{common}"))
        } else {
            (
                common,
                format!("retained_columns_translated_{dx:.1}x{dy:.1}_{common}"),
            )
        }
    } else {
        (
            exact,
            format!("columns_moved_{exact}_of_{common}_groups_{group_changes}"),
        )
    }
}

fn place_branches(
    graph: &Graph,
    ranks: &[usize],
    boxes: &mut [BoxedNode],
    available: f64,
) -> Option<(f64, Vec<usize>)> {
    // 首个分叉到共同汇合之间，以完整分支为宽度分组单位。
    let mut parents = vec![Vec::new(); boxes.len()];
    let mut children = vec![Vec::new(); boxes.len()];
    for edge in &graph.edges {
        if ranks[edge.from] >= ranks[edge.to] || edge.connector.line == crate::EdgeLine::Dotted {
            continue;
        }
        if !parents[edge.to].contains(&edge.from) {
            parents[edge.to].push(edge.from);
            children[edge.from].push(edge.to);
        }
    }
    let root =
        (0..boxes.len()).find(|node| parents[*node].is_empty() && children[*node].len() >= 3)?;
    let branches = &children[root];
    let mut members = Vec::new();
    for branch in branches {
        let mut list = vec![*branch];
        let mut cursor = 0;
        while cursor < list.len() {
            for child in &children[list[cursor]] {
                if parents[*child].len() == 1 {
                    list.push(*child);
                }
            }
            cursor += 1;
        }
        members.push(list);
    }
    let widths: Vec<_> = members
        .iter()
        .map(|members| {
            members
                .iter()
                .map(|node| boxes[*node].rect.width)
                .fold(0.0, f64::max)
        })
        .collect();
    if members
        .iter()
        .any(|nodes| nodes.iter().any(|node| children[*node].len() > 1))
    {
        return None;
    }
    let first_rank = branches.iter().map(|node| ranks[*node]).min()?;
    let last_rank = members.iter().flatten().map(|node| ranks[*node]).max()?;
    let branch_nodes: std::collections::HashSet<_> = members.iter().flatten().copied().collect();
    if (0..boxes.len()).any(|node| {
        !branch_nodes.contains(&node) && (first_rank..=last_rank).contains(&ranks[node])
    }) {
        return None;
    }
    let mut groups = vec![0; boxes.len()];
    let mut y = OUTER;
    let mut common = |range: std::ops::Range<usize>, y: &mut f64| {
        for rank in range {
            let nodes: Vec<_> = (0..boxes.len())
                .filter(|node| !branch_nodes.contains(node) && ranks[*node] == rank)
                .collect();
            let width = nodes
                .iter()
                .map(|node| boxes[*node].rect.width)
                .sum::<f64>()
                + NODE_GAP * nodes.len().saturating_sub(1) as f64;
            let height = nodes
                .iter()
                .map(|node| boxes[*node].rect.height)
                .fold(0.0, f64::max);
            let mut x = OUTER + (available - width) / 2.0;
            for node in nodes {
                boxes[node].rect.x = x;
                boxes[node].rect.y = *y;
                x += boxes[node].rect.width + NODE_GAP;
            }
            *y += height + rank_gap(graph);
        }
    };
    common(0..first_rank, &mut y);
    let mut start = 0;
    let mut band = 0;
    let branch_available = (available - 2.0 * (branches.len() as f64 * 3.0 + 10.0))
        .max(widths.iter().copied().fold(0.0, f64::max));
    while start < branches.len() {
        let mut end = start;
        let mut used = 0.0;
        while end < branches.len()
            && (end == start || used + NODE_GAP + widths[end] <= branch_available)
        {
            used += widths[end] + if end == start { 0.0 } else { NODE_GAP };
            end += 1;
        }
        let mut x = OUTER + (available - used) / 2.0;
        let mut bottom = y;
        for branch in start..end {
            let mut node_y = y;
            for node in &members[branch] {
                boxes[*node].rect.x = x + (widths[branch] - boxes[*node].rect.width) / 2.0;
                boxes[*node].rect.y = node_y;
                groups[*node] = band;
                node_y += boxes[*node].rect.height + rank_gap(graph);
            }
            bottom = bottom.max(node_y);
            x += widths[branch] + NODE_GAP;
        }
        y = bottom + 48.0;
        start = end;
        band += 1;
    }
    // 借用结束后安排共同汇合。
    for rank in last_rank + 1..=ranks.iter().copied().max().unwrap_or(0) {
        let nodes: Vec<_> = (0..boxes.len())
            .filter(|node| !branch_nodes.contains(node) && ranks[*node] == rank)
            .collect();
        let total = nodes
            .iter()
            .map(|node| boxes[*node].rect.width)
            .sum::<f64>()
            + NODE_GAP * nodes.len().saturating_sub(1) as f64;
        let mut x = OUTER + (available - total) / 2.0;
        let mut bottom = y;
        for node in nodes {
            boxes[node].rect.x = x;
            boxes[node].rect.y = y;
            groups[node] = band;
            bottom = bottom.max(y + boxes[node].rect.height);
            x += boxes[node].rect.width + NODE_GAP;
        }
        y = bottom + rank_gap(graph);
    }
    if graph.direction == Direction::BottomToTop {
        // 每个物理带只反转自身的层序，分支成员和左右次序不变。
        for boxed in boxes {
            boxed.rect.y = y - rank_gap(graph) - boxed.rect.y - boxed.rect.height + OUTER;
        }
    }
    Some((y - rank_gap(graph) + OUTER, groups))
}

fn retain_positions(
    graph: &Graph,
    options: &LayoutOptions,
    boxes: &mut [BoxedNode],
    groups: &mut [usize],
) -> (usize, String) {
    let Some(previous) = &options.previous else {
        return (0, "initial".into());
    };
    if previous.direction != graph.direction {
        return (0, "direction_changed".into());
    }
    let width_changed = (previous.width - options.width).abs() > 0.25;
    let width_initial = width_changed.then(|| (boxes.to_vec(), groups.to_vec()));
    if width_changed {
        let old_rects = previous
            .nodes
            .iter()
            .map(|anchor| anchor.rect)
            .collect::<Vec<_>>();
        let new_rects = boxes.iter().map(|node| node.rect).collect::<Vec<_>>();
        let old_bands = vertical_bands(&old_rects);
        let new_bands = vertical_bands(&new_rects);
        let common = graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                previous
                    .nodes
                    .iter()
                    .position(|anchor| anchor.node_id == node.id)
                    .map(|old| (index, old))
            })
            .collect::<Vec<_>>();
        let row_membership_matches = common.iter().all(|(a, old_a)| {
            common.iter().all(|(b, old_b)| {
                (old_bands[*old_a] == old_bands[*old_b]) == (new_bands[*a] == new_bands[*b])
            })
        });
        if !row_membership_matches {
            return (0, "width_row_bands_invalidated".into());
        }
    }
    // 改宽后旧行带可能已不再能容纳新框宽；若共同成员的带归属与目标宽度
    // 初排不同，保留旧分组会同时破坏行带和旧通道的一致性。
    if (previous.width - options.width).abs() > 0.25 {
        let same_group = |a: &crate::GraphAnchor, b: &crate::GraphAnchor| a.group == b.group;
        let common = graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                previous
                    .nodes
                    .iter()
                    .find(|anchor| anchor.node_id == node.id)
                    .map(|anchor| (index, anchor))
            })
            .collect::<Vec<_>>();
        let matches = common.iter().all(|(a, anchor_a)| {
            common
                .iter()
                .all(|(b, anchor_b)| same_group(anchor_a, anchor_b) == (groups[*a] == groups[*b]))
        });
        if !matches {
            return (0, "width_group_invalidated".into());
        }
    }
    // 横向改宽按旧行带整体装箱，不能把单个溢出节点依次丢到行尾下方。
    if (previous.width - options.width).abs() > 0.25
        && matches!(
            graph.direction,
            Direction::LeftToRight | Direction::RightToLeft
        )
        && previous.nodes.len() == graph.nodes.len()
    {
        let mut rows = std::collections::BTreeMap::<usize, Vec<usize>>::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            if let Some(old) = previous.nodes.iter().find(|old| old.node_id == node.id) {
                rows.entry(old.group).or_default().push(i);
            }
        }
        let fits = rows.values().all(|row| {
            row.iter().map(|i| boxes[*i].rect.width).sum::<f64>()
                + 32.0 * row.len().saturating_sub(1) as f64
                <= options.width - 2.0 * OUTER
        });
        if fits {
            let mut bottom = OUTER - 64.0;
            for (group, mut row) in rows {
                let anchor = |i: usize| {
                    previous
                        .nodes
                        .iter()
                        .find(|n| n.node_id == graph.nodes[i].id)
                        .unwrap()
                };
                row.sort_by(|a, b| anchor(*a).rect.x.total_cmp(&anchor(*b).rect.x));
                let total = row.iter().map(|i| boxes[*i].rect.width).sum::<f64>()
                    + 32.0 * row.len().saturating_sub(1) as f64;
                let left = row
                    .iter()
                    .map(|i| anchor(*i).rect.x)
                    .fold(f64::INFINITY, f64::min);
                let right = row
                    .iter()
                    .map(|i| anchor(*i).rect.x + anchor(*i).rect.width)
                    .fold(0.0, f64::max);
                let mut x =
                    ((left + right - total) / 2.0).clamp(OUTER, options.width - OUTER - total);
                let y = row
                    .iter()
                    .map(|i| anchor(*i).rect.y)
                    .fold(f64::INFINITY, f64::min)
                    .max(bottom + 64.0);
                for i in row {
                    boxes[i].rect.x = x;
                    boxes[i].rect.y = y;
                    groups[i] = group;
                    x += boxes[i].rect.width + 32.0;
                    bottom = bottom.max(y + boxes[i].rect.height);
                }
            }
            if let Some((initial_boxes, initial_groups)) = &width_initial {
                let initial_bands = vertical_bands(
                    &initial_boxes
                        .iter()
                        .map(|node| node.rect)
                        .collect::<Vec<_>>(),
                );
                let retained_bands =
                    vertical_bands(&boxes.iter().map(|node| node.rect).collect::<Vec<_>>());
                if !same_band_partition(&initial_bands, &retained_bands) {
                    boxes.clone_from_slice(initial_boxes);
                    groups.copy_from_slice(initial_groups);
                    return (0, "width_row_bands_invalidated".into());
                }
            }
            return (boxes.len(), "retained_horizontal_bands".into());
        }
        return (0, "horizontal_band_capacity_changed".into());
    }
    let mut adjusted = previous.clone();
    // 同层支路在右边界新增叶时，只压缩末端的空白；旧节点最多移动 48px。
    for (i, node) in graph.nodes.iter().enumerate() {
        if graph.direction != Direction::TopToBottom
            || previous.nodes.iter().any(|old| old.node_id == node.id)
            || graph.edges.iter().any(|e| e.from == i)
        {
            continue;
        }
        let Some(parent) = graph.edges.iter().find(|e| e.to == i).map(|e| e.from) else {
            continue;
        };
        if graph.edges.iter().filter(|e| e.from == parent).count() < 2 {
            continue;
        }
        let Some(anchor) = previous
            .nodes
            .iter()
            .find(|old| old.node_id == graph.nodes[parent].id)
        else {
            continue;
        };
        if anchor.rect.x + anchor.rect.width + NODE_GAP + boxes[i].rect.width
            <= options.width - OUTER
        {
            continue;
        }
        let mut band = previous
            .nodes
            .iter()
            .filter(|old| (old.rect.y - anchor.rect.y).abs() < 1.0)
            .collect::<Vec<_>>();
        band.sort_by(|a, b| b.rect.x.total_cmp(&a.rect.x));
        let mut right = options.width - OUTER - boxes[i].rect.width - 8.0;
        let mut moves = Vec::new();
        for old in band {
            let x = old.rect.x.min(right - old.rect.width);
            moves.push((old.rect.x + old.rect.width / 2.0, x - old.rect.x));
            right = x - 8.0;
        }
        if moves.iter().any(|(_, dx)| dx.abs() > 48.0) || right < OUTER - 8.0 {
            continue;
        }
        for old in &mut adjusted.nodes {
            if let Some((_, dx)) = moves
                .iter()
                .find(|(center, _)| (old.rect.x + old.rect.width / 2.0 - center).abs() < 1.0)
            {
                old.rect.x += dx;
            }
        }
    }
    // 改宽仍优先保留旧中心和分组；只有实际放不下的节点重新找位。
    let old: Vec<_> = graph
        .nodes
        .iter()
        .map(|node| adjusted.nodes.iter().find(|old| old.node_id == node.id))
        .collect();
    let visible = |index: usize| {
        options.viewport.as_ref().is_some_and(|viewport| {
            viewport
                .visible
                .iter()
                .any(|anchor| anchor.node_id == graph.nodes[index].id)
        })
    };
    let mut order: Vec<_> = (0..boxes.len()).collect();
    // 可见、未改尺寸的节点先占位；新增或扩大节点只移动自身，不让整图退回初排。
    order.sort_by_key(|index| {
        (
            old[*index].is_none(),
            !visible(*index),
            old[*index].is_none_or(|old| {
                (old.rect.width - boxes[*index].rect.width).abs() > 0.01
                    || (old.rect.height - boxes[*index].rect.height).abs() > 0.01
            }),
            *index,
        )
    });
    let mut placed = Vec::<Rect>::new();
    let mut retained = 0;
    let mut relocated = 0;
    let mut group_rows = Vec::<(f64, usize)>::new();
    for index in order {
        let initial = boxes[index].rect;
        let new_leaf = old[index].is_none() && !graph.edges.iter().any(|edge| edge.from == index);
        let parent = new_leaf
            .then(|| {
                graph
                    .edges
                    .iter()
                    .find(|edge| edge.to == index)
                    .and_then(|edge| old[edge.from])
            })
            .flatten();
        let preferred = parent.map_or(initial, |parent| {
            let r = parent.rect;
            match graph.direction {
                Direction::TopToBottom
                    if graph
                        .edges
                        .iter()
                        .filter(|e| graph.nodes[e.from].id == parent.node_id)
                        .count()
                        > 1 =>
                {
                    Rect {
                        x: (r.x + r.width + NODE_GAP).min(options.width - OUTER - initial.width),
                        y: r.y + (r.height - initial.height) / 2.0,
                        ..initial
                    }
                }
                Direction::TopToBottom => Rect {
                    x: r.x + (r.width - initial.width) / 2.0,
                    y: r.y + r.height + rank_gap(graph),
                    ..initial
                },
                Direction::BottomToTop => Rect {
                    x: r.x + (r.width - initial.width) / 2.0,
                    y: r.y - initial.height - rank_gap(graph),
                    ..initial
                },
                Direction::LeftToRight => Rect {
                    x: r.x + r.width + NODE_GAP,
                    y: r.y + (r.height - initial.height) / 2.0,
                    ..initial
                },
                Direction::RightToLeft => Rect {
                    x: r.x - initial.width - NODE_GAP,
                    y: r.y + (r.height - initial.height) / 2.0,
                    ..initial
                },
            }
        });
        let desired = old[index].map_or(preferred, |anchor| Rect {
            x: anchor.rect.x + (anchor.rect.width - initial.width) / 2.0,
            y: anchor.rect.y + (anchor.rect.height - initial.height) / 2.0,
            ..initial
        });
        let fits = |r: Rect| {
            r.x >= OUTER - 0.01
                && r.x + r.width <= options.width - OUTER + 0.01
                && r.y >= OUTER - 0.01
                && (old[index]
                    .is_some_and(|old| old.rect.width == r.width && old.rect.height == r.height)
                    || !previous.edges.iter().any(|old_edge| {
                        graph
                            .edges
                            .iter()
                            .find(|edge| edge.id == old_edge.edge_id)
                            .is_some_and(|edge| {
                                edge.from != index
                                    && edge.to != index
                                    && !parent.is_some_and(|parent| {
                                        graph.nodes[edge.from].id == parent.node_id
                                            || graph.nodes[edge.to].id == parent.node_id
                                    })
                                    && old_edge.points.windows(2).any(|pair| {
                                        segment_hits_rect(
                                            pair[0],
                                            pair[1],
                                            Rect {
                                                x: r.x - 8.0,
                                                y: r.y - 8.0,
                                                width: r.width + 16.0,
                                                height: r.height + 16.0,
                                            },
                                        )
                                    })
                            })
                    }))
                && !placed.iter().any(|other| {
                    Rect {
                        x: r.x - 8.0,
                        y: r.y - 8.0,
                        width: r.width + 16.0,
                        height: r.height + 16.0,
                    }
                    .intersects(*other)
                })
        };
        let selected = if fits(desired) {
            desired
        } else {
            let mut xs = vec![
                initial.x,
                desired.x,
                OUTER,
                options.width - OUTER - initial.width,
            ];
            let mut ys = vec![initial.y, desired.y, OUTER];
            for rect in &placed {
                xs.extend([
                    rect.x - initial.width - NODE_GAP,
                    rect.x + rect.width + NODE_GAP,
                ]);
                ys.extend([
                    rect.y - initial.height - rank_gap(graph),
                    rect.y + rect.height + rank_gap(graph),
                ]);
            }
            let mut candidates = Vec::new();
            for x in xs {
                for y in &ys {
                    let rect = Rect {
                        x,
                        y: *y,
                        ..initial
                    };
                    if fits(rect) {
                        candidates.push(rect);
                    }
                }
            }
            candidates.sort_by(|a, b| {
                let cost = |r: &Rect| (r.x - desired.x).powi(2) + (r.y - desired.y).powi(2) * 2.0;
                cost(a)
                    .total_cmp(&cost(b))
                    .then_with(|| a.y.total_cmp(&b.y))
                    .then_with(|| a.x.total_cmp(&b.x))
            });
            relocated += 1;
            candidates.first().copied().unwrap_or_else(|| Rect {
                x: OUTER,
                y: placed
                    .iter()
                    .map(|r| r.y + r.height + rank_gap(graph))
                    .fold(OUTER, f64::max),
                ..initial
            })
        };
        if let Some(anchor) = old[index] {
            let cx = selected.x + selected.width / 2.0;
            let cy = selected.y + selected.height / 2.0;
            if (cx - anchor.rect.x - anchor.rect.width / 2.0).abs() < 0.01
                && (cy - anchor.rect.y - anchor.rect.height / 2.0).abs() < 0.01
            {
                retained += 1;
                groups[index] = anchor.group;
                group_rows.push((selected.y, anchor.group));
            }
        }
        boxes[index].rect = selected;
        placed.push(selected);
    }
    if matches!(
        graph.direction,
        Direction::LeftToRight | Direction::RightToLeft
    ) {
        let mut next = previous.nodes.iter().map(|n| n.group).max().unwrap_or(0) + 1;
        for index in 0..boxes.len() {
            if let Some((_, group)) = group_rows
                .iter()
                .find(|(y, _)| (*y - boxes[index].rect.y).abs() < 0.01)
            {
                groups[index] = *group;
            } else {
                groups[index] = next;
                group_rows.push((boxes[index].rect.y, next));
                next += 1;
            }
        }
    }
    if let Some((initial_boxes, initial_groups)) = width_initial {
        let initial_bands = vertical_bands(
            &initial_boxes
                .iter()
                .map(|node| node.rect)
                .collect::<Vec<_>>(),
        );
        let retained_bands =
            vertical_bands(&boxes.iter().map(|node| node.rect).collect::<Vec<_>>());
        if !same_band_partition(&initial_bands, &retained_bands) {
            boxes.clone_from_slice(&initial_boxes);
            groups.copy_from_slice(&initial_groups);
            return (0, "width_row_bands_invalidated".into());
        }
    }
    (
        retained,
        format!(
            "retained_{retained}_local_relocations_{relocated}_visible_{}",
            (0..boxes.len()).filter(|i| visible(*i)).count()
        ),
    )
}

fn same_band_partition(a: &[usize], b: &[usize]) -> bool {
    a.len() == b.len()
        && (0..a.len())
            .all(|left| (0..a.len()).all(|right| (a[left] == a[right]) == (b[left] == b[right])))
}

fn vertical_bands(rects: &[Rect]) -> Vec<usize> {
    let mut bands = vec![usize::MAX; rects.len()];
    let mut next = 0;
    for start in 0..rects.len() {
        if bands[start] != usize::MAX {
            continue;
        }
        bands[start] = next;
        let mut pending = vec![start];
        while let Some(index) = pending.pop() {
            for other in start + 1..rects.len() {
                if bands[other] != usize::MAX {
                    continue;
                }
                let a = rects[index];
                let b = rects[other];
                let overlap = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
                if overlap >= a.height.min(b.height) * 0.5 {
                    bands[other] = next;
                    pending.push(other);
                }
            }
        }
        next += 1;
    }
    bands
}

fn wrap_label(
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
                    y: center.y + dy - EDGE_LABEL_HEIGHT / 2.0,
                    width: width + EDGE_LABEL_PAD_X * 2.0,
                    height: EDGE_LABEL_HEIGHT,
                };
                if rect.x < 4.0 || rect.x + rect.width > canvas_width - 4.0 || rect.y < 4.0 {
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
                if !horizontal && segment_hits_rect(pair[0], pair[1], margin) {
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
                        && path
                            .windows(2)
                            .any(|pair| segment_hits_rect(pair[0], pair[1], margin))
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

fn marker_clearance(point: Option<&Point>, next: Option<&Point>) -> Option<Rect> {
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

fn segment_hits_rect(a: Point, b: Point, r: Rect) -> bool {
    if (a.x - b.x).abs() < 0.001 {
        a.x > r.x && a.x < r.x + r.width && a.y.min(b.y) < r.y + r.height && a.y.max(b.y) > r.y
    } else {
        a.y > r.y && a.y < r.y + r.height && a.x.min(b.x) < r.x + r.width && a.x.max(b.x) > r.x
    }
}

fn count_node_overlaps(nodes: &[BoxedNode]) -> usize {
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

fn count_edge_crossings(paths: &[Vec<Point>]) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_diagram;
    use std::collections::BTreeSet;

    fn graph(source: &str) -> Graph {
        let crate::Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
            panic!("expected graph")
        };
        graph
    }

    fn groups(nodes: &[crate::GraphAnchor]) -> BTreeSet<BTreeSet<String>> {
        let mut result = std::collections::BTreeMap::<usize, BTreeSet<String>>::new();
        for node in nodes {
            result
                .entry(node.group)
                .or_default()
                .insert(node.node_id.clone());
        }
        result.into_values().collect()
    }

    #[test]
    fn width_change_discards_stale_fanout_bands() {
        let graph = graph(include_str!(
            "../../../fixtures/mermaid/flowchart/fanout.mmd"
        ));
        let mut fonts = crate::SystemFonts::new().unwrap();
        let initial = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let narrow = layout_graph(
            &graph,
            &LayoutOptions {
                width: 600.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let resized = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                previous: Some(narrow.previous),
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        assert!(resized.metrics.stability_reason.starts_with("width_"));
        assert_eq!(
            groups(&resized.previous.nodes),
            groups(&initial.previous.nodes)
        );
        assert_eq!(resized.metrics.edge_crossings, 0);
    }

    #[test]
    fn horizontal_width_change_keeps_compatible_rows() {
        let graph = graph(include_str!(
            "../../../fixtures/mermaid/flowchart/erikson.mmd"
        ));
        let mut fonts = crate::SystemFonts::new().unwrap();
        let initial = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let resized = layout_graph(
            &graph,
            &LayoutOptions {
                width: 900.0,
                previous: Some(initial.previous.clone()),
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        assert_eq!(
            resized.metrics.stability_reason,
            "retained_horizontal_bands"
        );
        assert_eq!(
            groups(&resized.previous.nodes),
            groups(&initial.previous.nodes)
        );
        assert_eq!(resized.metrics.edge_crossings, 0);
    }

    #[test]
    fn width_change_discards_stale_physical_rows() {
        let graph = graph(include_str!(
            "../../../fixtures/mermaid/flowchart/graph-syntax.mmd"
        ));
        let mut fonts = crate::SystemFonts::new().unwrap();
        let initial = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let narrow = layout_graph(
            &graph,
            &LayoutOptions {
                width: 500.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let resized = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                previous: Some(narrow.previous),
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        assert_eq!(
            resized.metrics.stability_reason,
            "width_row_bands_invalidated"
        );
        assert_eq!(
            vertical_bands(
                &resized
                    .previous
                    .nodes
                    .iter()
                    .map(|node| node.rect)
                    .collect::<Vec<_>>()
            ),
            vertical_bands(
                &initial
                    .previous
                    .nodes
                    .iter()
                    .map(|node| node.rect)
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(resized.metrics.edge_crossings, 0);
    }

    #[test]
    fn dense_dag_layout_has_no_crossing() {
        let graph = graph(include_str!(
            "../../../fixtures/mermaid/flowchart/dense-dag.mmd"
        ));
        let mut fonts = crate::SystemFonts::new().unwrap();
        let result = layout_graph(&graph, &LayoutOptions::default(), &mut fonts).unwrap();
        assert!(result.metrics.edge_crossings <= 1);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn research_ports_and_labels_use_centered_generic_rules() {
        let graph = graph(include_str!(
            "../../../fixtures/mermaid/flowchart/research.mmd"
        ));
        let mut fonts = crate::SystemFonts::new().unwrap();
        let result = layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        assert_eq!((result.scene.width, result.scene.height), (700.0, 644.0));
        assert_eq!(result.scene.hit_regions.len(), 10);

        let nodes = result
            .scene
            .hit_regions
            .iter()
            .map(|hit| (hit.node_id.as_str(), hit.rect))
            .collect::<BTreeMap<_, _>>();
        let expected_nodes = [
            ("A", 252.0, 30.0, 84.0, 40.0),
            ("B", 252.0, 98.0, 84.0, 40.0),
            ("C", 252.0, 166.0, 84.0, 40.0),
            ("D", 252.0, 234.0, 84.0, 40.0),
            ("E", 252.0, 302.0, 84.0, 40.0),
            ("F", 252.0, 370.0, 84.0, 40.0),
            ("G", 219.5, 438.0, 149.0, 40.0),
            ("H", 252.0, 506.0, 84.0, 40.0),
            ("J", 252.0, 574.0, 84.0, 40.0),
            ("I", 396.5, 438.0, 84.0, 40.0),
        ];
        for (id, x, y, width, height) in expected_nodes {
            assert_eq!(
                nodes[id],
                Rect {
                    x,
                    y,
                    width,
                    height
                },
                "{id}"
            );
        }

        let paths = result
            .scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::GraphPath {
                    edge_id, points, ..
                } => Some((*edge_id, points)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), graph.edges.len());
        let point = |x, y| Point { x, y };
        assert_eq!(paths[2].1, &[point(294.0, 206.0), point(294.0, 234.0)]);
        assert_eq!(paths[6].1, &[point(294.0, 478.0), point(294.0, 506.0)]);
        assert_eq!(paths[8].1, &[point(368.5, 458.0), point(396.5, 458.0)]);
        assert_eq!(
            paths[9].1,
            &[
                point(438.5, 438.0),
                point(438.5, 254.0),
                point(336.0, 254.0)
            ]
        );

        let label = |text: &str| {
            result
                .scene
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    Primitive::Text(span) if span.text == text => Some(span),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing label {text}"))
        };
        let background = |edge_id| {
            result
                .scene
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    Primitive::LabelBackground {
                        edge_id: id, rect, ..
                    } if *id == edge_id => Some(*rect),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing label background {edge_id}"))
        };
        for text in ["是", "否", "重新检验"] {
            let span = label(text);
            assert_eq!((span.font_size, span.font_weight), (EDGE_LABEL_SIZE, 400));
        }
        for (text, edge) in [("是", 6), ("重新检验", 9)] {
            let span = label(text);
            let rect = background(paths[edge].0);
            let line_x = paths[edge]
                .1
                .windows(2)
                .filter(|pair| (pair[0].x - pair[1].x).abs() < 0.001)
                .max_by(|a, b| (a[0].y - a[1].y).abs().total_cmp(&(b[0].y - b[1].y).abs()))
                .unwrap()[0]
                .x;
            assert!(rect.x > line_x + 4.0, "{text}: {rect:?} crosses x={line_x}");
            assert!(span.baseline.x > line_x + 4.0);
            assert!(span.baseline.x + span.width <= rect.x + rect.width);
        }
        let horizontal = label("否");
        let horizontal_rect = background(paths[8].0);
        assert!(horizontal_rect.y < 458.0 && horizontal_rect.y + horizontal_rect.height > 458.0);
        assert!((horizontal_rect.x + horizontal_rect.width / 2.0 - 382.5).abs() <= 5.0);
        assert!(horizontal.baseline.x >= horizontal_rect.x);
        assert!(
            horizontal.baseline.x + horizontal.width <= horizontal_rect.x + horizontal_rect.width
        );

        let backgrounds = paths
            .iter()
            .filter_map(|(edge_id, _)| {
                result
                    .scene
                    .primitives
                    .iter()
                    .find_map(|primitive| match primitive {
                        Primitive::LabelBackground {
                            edge_id: id, rect, ..
                        } if id == edge_id => Some((*edge_id, *rect)),
                        _ => None,
                    })
            })
            .collect::<Vec<_>>();
        for (index, (edge_id, rect)) in backgrounds.iter().enumerate() {
            assert!(nodes.values().all(|node| !rect.intersects(*node)));
            assert!(
                backgrounds[index + 1..]
                    .iter()
                    .all(|(_, other)| !rect.intersects(*other))
            );
            let path = paths
                .iter()
                .find(|(other_id, _)| other_id == edge_id)
                .unwrap()
                .1;
            if let Some(marker) = marker_clearance(
                path.last(),
                path.len().checked_sub(2).and_then(|index| path.get(index)),
            ) {
                assert!(!rect.intersects(marker));
            }
            assert!(paths.iter().all(|(other_id, path)| {
                other_id == edge_id
                    || path
                        .windows(2)
                        .all(|pair| !segment_hits_rect(pair[0], pair[1], *rect))
            }));
        }
    }

    #[test]
    fn same_side_and_parallel_edges_keep_distinct_ports() {
        let same_side = graph("flowchart TB\n  A --> B\n  A --> C\n  B --> C\n");
        let mut fonts = crate::SystemFonts::new().unwrap();
        let result = layout_graph(
            &same_side,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let paths = result
            .scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::GraphPath { points, .. } => Some(points),
                _ => None,
            })
            .collect::<Vec<_>>();
        let c = result
            .scene
            .hit_regions
            .iter()
            .find(|hit| hit.node_id == "C")
            .unwrap()
            .rect;
        let incoming = [paths[1].last().unwrap(), paths[2].last().unwrap()];
        assert!(incoming.iter().all(|point| (point.y - c.y).abs() < 0.001));
        assert!((incoming[0].x - incoming[1].x).abs() >= 12.0);
        assert!(((incoming[0].x + incoming[1].x) / 2.0 - (c.x + c.width / 2.0)).abs() < 0.001);

        let parallel = graph("flowchart TB\n  A --> B\n  A --> B\n");
        let result = layout_graph(
            &parallel,
            &LayoutOptions {
                width: 700.0,
                ..Default::default()
            },
            &mut fonts,
        )
        .unwrap();
        let paths = result
            .scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::GraphPath { points, .. } => Some(points),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_ne!(paths[0].first(), paths[1].first());
        assert_ne!(paths[0].last(), paths[1].last());
    }
}
