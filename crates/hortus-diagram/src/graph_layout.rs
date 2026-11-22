use crate::geometry::segment_intersects;
use crate::graph::{Direction, Graph, Shape};
use crate::{
    Color, FontInfo, FontMetrics, HitRegion, LayoutError, LayoutOptions, Metrics, Point, Primitive,
    Rect, Scene, TextSpan, Theme,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

mod column;
mod container;
mod place;
mod rank;
mod render;
mod retain;
mod scene;

use column::column_tree_layout;
use container::layout_container_graph;
use place::{build_boxes, place_nodes};
use rank::{align_feedback_ranks, permutations_of};
pub(crate) use rank::{assign_ranks, order_ranks};
use render::{LayoutState, assemble_scene};
use retain::retain_positions;
#[cfg(test)]
use retain::vertical_bands;
#[cfg(all(test, target_os = "macos"))]
use scene::marker_clearance;
use scene::{
    container_title_height, container_title_lines, contrast_text_color, count_edge_crossings,
    count_node_overlaps, push_dashed_outline, scene_content_bounds, scene_paths,
    translated_primitive, wrap_label,
};
pub(crate) use scene::{place_edge_label, place_sized_edge_label};

const NODE_GAP: f64 = 28.0;
const RANK_GAP_COMPACT: f64 = 28.0;
const RANK_GAP_DENSE: f64 = 44.0;
const EDGE_LABEL_SIZE: f64 = 13.6;
const EDGE_LABEL_HEIGHT: f64 = 22.0;
const EDGE_LABEL_PAD_X: f64 = 2.0;
const EDGE_LABEL_SIDE_CLEARANCE: f64 = 7.0;
const LR_BAND_GAP: f64 = 48.0;

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

struct Palette {
    background: Color,
    text_color: Color,
    edge_color: Color,
    shape_fill: Color,
    shape_stroke: Color,
}

impl Palette {
    fn new(theme: Theme) -> Self {
        let dark = theme == Theme::Dark;
        Self {
            background: Color::oklch(if dark { 0.215 } else { 0.982 }, 0.004, 260.0),
            text_color: Color::oklch(if dark { 0.91 } else { 0.25 }, 0.01, 260.0),
            edge_color: Color::oklch(if dark { 0.68 } else { 0.46 }, 0.025, 250.0),
            shape_fill: Color::oklch(if dark { 0.27 } else { 0.995 }, 0.008, 250.0),
            shape_stroke: Color::oklch(if dark { 0.66 } else { 0.43 }, 0.055, 250.0),
        }
    }
}

pub fn layout_graph(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<GraphScene, LayoutError> {
    layout_graph_with_context(graph, options, fonts, false)
}

fn layout_graph_with_context(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
    container_members: bool,
) -> Result<GraphScene, LayoutError> {
    if !graph.containers.is_empty() {
        return layout_container_graph(graph, options, fonts);
    }
    let initial = layout_graph_with_order(graph, options, fonts, None, container_members)?;
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
            if let Ok(candidate) =
                layout_graph_with_order(graph, options, fonts, Some(&order), container_members)
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

fn layout_graph_with_order(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
    order_override: Option<&[Vec<usize>]>,
    container_members: bool,
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

    let palette = Palette::new(options.theme);
    let visible_width = options.width - OUTER * 2.0;
    let (mut ranks, rank_count, accepted) = assign_ranks(graph);

    let mut boxes = build_boxes(
        graph,
        fonts,
        &palette,
        container_members,
        rank_count,
        visible_width,
    );

    align_feedback_ranks(graph, &mut ranks, &accepted);
    let order = order_override
        .map(<[Vec<usize>]>::to_vec)
        .unwrap_or_else(|| order_ranks(graph, &ranks, rank_count));
    let (mut height, mut groups) = place_nodes(
        graph,
        &order,
        &ranks,
        &accepted,
        &mut boxes,
        visible_width,
        container_members,
    );
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
    assemble_scene(
        graph,
        options,
        fonts,
        started,
        &palette,
        LayoutState {
            boxes,
            ranks,
            groups,
            accepted,
            rank_count,
            height,
            column_paths,
            retained_nodes,
            stability_reason,
        },
        container_members,
    )
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
            "../../../fixtures/mermaid/flowchart/release-flow.mmd"
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
                        .all(|pair| !segment_intersects(pair[0], pair[1], *rect))
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
