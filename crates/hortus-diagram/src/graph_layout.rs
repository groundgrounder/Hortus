use crate::graph::{Direction, Graph, Shape};
use crate::{
    Color, FontInfo, FontMetrics, HitRegion, LayoutError, LayoutOptions, Metrics, Point, Primitive,
    Rect, Scene, TextSpan, Theme,
};
use std::collections::VecDeque;
use std::time::Instant;

const NODE_GAP: f64 = 28.0;
const RANK_GAP: f64 = 44.0;
const OUTER: f64 = 30.0;

#[derive(Clone, Debug)]
pub struct GraphMetrics {
    pub ranks: usize,
    pub roots: usize,
    pub edge_count: usize,
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
    label_lines: Vec<(String, f64, f64, f64)>,
}

pub fn layout_graph(
    graph: &Graph,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
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
            ((visible_width - 32.0 * 3.0) / 4.0 - 24.0).max(48.0)
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
                let measured = fonts.measure(&line, size, weight);
                label_lines.push((line, measured.width, measured.ascent, size));
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
        let (width, height) = if matches!(node.shape, Shape::Circle | Shape::DoubleCircle) {
            let diameter = width.max(height);
            (diameter, diameter)
        } else {
            (width, height)
        };
        let style = node
            .class
            .as_ref()
            .and_then(|class| graph.class_defs.get(class))
            .cloned()
            .unwrap_or_default();
        let style = graph.inline_styles.get(&node.id).unwrap_or(&style);
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
    let order = order_ranks(graph, &ranks, rank_count);
    let (mut height, mut groups) = place_nodes(graph, &order, &ranks, &mut boxes, visible_width);
    let (retained_nodes, stability_reason) =
        retain_positions(graph, options, &mut boxes, &mut groups);
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
                .map(|label| fonts.measure(label, 13.0, 400).width)
        })
        .collect::<Vec<_>>();
    let routed = crate::graph_router::route(
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
    )?;
    let edge_paths = routed.paths;
    for (edge, points) in graph.edges.iter().zip(&edge_paths) {
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
            let measured = fonts.measure(label, 13.0, 400);
            let rect =
                routed.labels[index].ok_or_else(|| LayoutError("Missing edge label".into()))?;
            primitives.push(Primitive::LabelBackground {
                edge_id: edge.id,
                rect,
                color: background,
            });
            let mut x = rect.x + 5.0;
            for run in measured.runs {
                primitives.push(Primitive::Text(TextSpan {
                    text: run.text,
                    baseline: Point {
                        x,
                        y: rect.y + 14.0,
                    },
                    width: run.width,
                    font_size: 13.0,
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
        primitives.push(Primitive::GraphShape {
            rect: boxed.rect,
            shape: boxed.shape,
            outline: crate::shape::outline(boxed.shape, boxed.rect),
            details: crate::shape::details(boxed.shape, boxed.rect),
            fill: boxed.fill,
            stroke: boxed.stroke,
            stroke_width: 1.4,
        });
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
                    color: text_color,
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
    let edge_crossings = count_edge_crossings(&edge_paths);
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
                || graph.edges.iter().any(|edge| edge.label.is_some())
            {
                13.0
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
            edge_count: graph.edges.len(),
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
            y = bottom + 64.0;
        }
        return (y - 64.0 + OUTER, groups);
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
        y += height + RANK_GAP;
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
    (y - RANK_GAP + OUTER, groups)
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
            *y += height + RANK_GAP;
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
                node_y += boxes[*node].rect.height + RANK_GAP;
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
        y = bottom + RANK_GAP;
    }
    if graph.direction == Direction::BottomToTop {
        // 每个物理带只反转自身的层序，分支成员和左右次序不变。
        for boxed in boxes {
            boxed.rect.y = y - RANK_GAP - boxed.rect.y - boxed.rect.height + OUTER;
        }
    }
    Some((y - RANK_GAP + OUTER, groups))
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
                    y: r.y + r.height + RANK_GAP,
                    ..initial
                },
                Direction::BottomToTop => Rect {
                    x: r.x + (r.width - initial.width) / 2.0,
                    y: r.y - initial.height - RANK_GAP,
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
                    rect.y - initial.height - RANK_GAP,
                    rect.y + rect.height + RANK_GAP,
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
                    .map(|r| r.y + r.height + RANK_GAP)
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
    (
        retained,
        format!(
            "retained_{retained}_local_relocations_{relocated}_visible_{}",
            (0..boxes.len()).filter(|i| visible(*i)).count()
        ),
    )
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
        for fraction in [0.5, 0.3, 0.7, 0.15, 0.85] {
            let center = Point {
                x: pair[0].x + (pair[1].x - pair[0].x) * fraction,
                y: pair[0].y + (pair[1].y - pair[0].y) * fraction,
            };
            let rect = Rect {
                x: center.x - width / 2.0 - 5.0,
                y: center.y - 10.0,
                width: width + 10.0,
                height: 20.0,
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
            if nodes.iter().any(|node| margin.intersects(*node))
                || labels.iter().any(|label| margin.intersects(*label))
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
                -(length.min(100.0)) + (fraction - 0.5).abs() * 30.0 + segment as f64 * 0.01,
                rect,
            ));
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    candidates.first().map(|(_, rect)| *rect)
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
        assert_eq!(resized.metrics.stability_reason, "width_group_invalidated");
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
}
