//! 顶层 subgraph 容器的内部布局与场景合成。

use super::*;

pub(super) struct ContainerContent {
    scene: Scene,
    bounds: Option<Rect>,
}

pub(super) fn style_for_node(graph: &Graph, node: &crate::GraphNode) -> crate::Style {
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

pub(super) fn layout_container_graph(
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
            let result = layout_graph_with_context(
                &inner_graph,
                &LayoutOptions {
                    width: inner_width,
                    theme: options.theme,
                    ..LayoutOptions::default()
                },
                fonts,
                true,
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
    let outer = layout_graph_with_context(
        &outer_graph,
        &LayoutOptions {
            width: options.width,
            theme: options.theme,
            previous: options.previous.clone(),
            ..LayoutOptions::default()
        },
        fonts,
        false,
    )?;
    let outer_rects = outer
        .scene
        .hit_regions
        .iter()
        .map(|hit| (hit.node_id.clone(), hit.rect))
        .collect::<HashMap<_, _>>();
    Ok(compose_container_scene(
        graph,
        options,
        fonts,
        contents,
        outer,
        outer_rects,
    ))
}

fn compose_container_scene(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
    contents: Vec<ContainerContent>,
    outer: GraphScene,
    outer_rects: HashMap<String, Rect>,
) -> GraphScene {
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
    GraphScene {
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
    }
}
