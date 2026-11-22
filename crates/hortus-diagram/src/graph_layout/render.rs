//! 最终节点、连线和标签图元组装与场景指标。

use super::*;

pub(super) struct LayoutState {
    pub boxes: Vec<BoxedNode>,
    pub ranks: Vec<usize>,
    pub groups: Vec<usize>,
    pub accepted: Vec<bool>,
    pub rank_count: usize,
    pub height: f64,
    pub column_paths: Option<Vec<Vec<Point>>>,
    pub retained_nodes: usize,
    pub stability_reason: String,
}

pub(super) fn assemble_scene(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
    started: Instant,
    palette: &Palette,
    state: LayoutState,
    container_members: bool,
) -> Result<GraphScene, LayoutError> {
    let LayoutState {
        boxes,
        ranks,
        groups,
        accepted,
        rank_count,
        height,
        column_paths,
        retained_nodes,
        stability_reason,
    } = state;
    let background = palette.background;
    let text_color = palette.text_color;
    let edge_color = palette.edge_color;
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
        let lr_bands = (container_members && graph.direction == Direction::LeftToRight)
            .then_some(groups.as_slice());
        crate::graph_router::route(
            graph,
            &boxes
                .iter()
                .map(|node| (node.rect, node.shape))
                .collect::<Vec<_>>(),
            crate::graph_router::RoutePolicy {
                distinct_ports: false,
                accepted: &accepted,
                lr_bands,
                port_gap: 12.0,
            },
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
            label: boxed.display_label.clone(),
            prefix: None,
            rect: boxed.rect,
            can_collapse: false,
            collapsed: false,
            source_range: node.source_range.clone(),
            link_target: boxed.link_target.clone(),
            link_heading: boxed.link_heading.clone(),
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
            diagram_kind: "flowchart".into(),
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
