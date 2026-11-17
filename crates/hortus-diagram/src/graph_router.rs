//! 基于节点外侧可见网格的确定性正交路由；已占用通道作为代价。
use crate::{Direction, Graph, LayoutError, Point, Rect, Shape};

const CLEARANCE: f64 = 10.0;

/// 无标签端口组在节点同侧占用的半幅比例（以边中点为原点）。
pub(crate) const PORT_SPAN_RATIO: f64 = 0.22;
/// 同侧相邻无标签端口的最小中心间距；0.25px 余量吸收浮点误差。
pub(crate) const MIN_PORT_GAP: f64 = 6.0;
const PORT_GAP_MARGIN: f64 = 0.25;

mod route_grid;
mod route_plan;
use route_grid::route_grid;
use route_plan::{EdgeSide, Ends, compute_ends, plan_edges};

/// 同侧 `ports` 个端口保持最小间距所需的最短节点边长。
pub(crate) fn min_port_side(ports: usize) -> f64 {
    ports.saturating_sub(1) as f64 * (MIN_PORT_GAP + PORT_GAP_MARGIN) / (2.0 * PORT_SPAN_RATIO)
}

#[derive(Clone, Copy)]
struct ChannelOwner {
    first: usize,
    common_from: usize,
    common_to: usize,
    labelled: bool,
}
impl ChannelOwner {
    const EMPTY: Self = Self {
        first: usize::MAX,
        common_from: usize::MAX,
        common_to: usize::MAX,
        labelled: false,
    };
    fn occupy(&mut self, index: usize, graph: &Graph) {
        let edge = &graph.edges[index];
        if self.first == usize::MAX {
            *self = Self {
                first: index,
                common_from: edge.from,
                common_to: edge.to,
                labelled: edge.label.is_some(),
            };
        } else {
            if self.common_from != edge.from {
                self.common_from = usize::MAX;
            }
            if self.common_to != edge.to {
                self.common_to = usize::MAX;
            }
            self.labelled |= edge.label.is_some();
        }
    }
}

pub(crate) struct Routes {
    pub paths: Vec<Vec<Point>>,
    pub labels: Vec<Option<Rect>>,
}

#[derive(Clone, Copy)]
pub(crate) struct RoutePolicy<'a> {
    pub accepted: &'a [bool],
    pub lr_bands: Option<&'a [usize]>,
}

pub(crate) fn route(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    policy: RoutePolicy<'_>,
    width: f64,
    height: f64,
    label_widths: &[Option<f64>],
    previous: Option<&crate::PreviousGraph>,
) -> Result<Routes, LayoutError> {
    // 同源前缀和同目的后缀共用总线；先重建该平面嵌入，避免局部旧通道形成交叉。
    if let Some(paths) = branch_routes(graph, nodes, width) {
        let seeds = paths
            .into_iter()
            .map(|p| (!p.is_empty()).then_some(p))
            .collect::<Vec<_>>();
        if seeds.iter().all(Option::is_some) {
            return Ok(Routes {
                paths: seeds.into_iter().map(Option::unwrap).collect(),
                labels: vec![None; graph.edges.len()],
            });
        }
        return route_grid(
            graph,
            nodes,
            policy,
            (width, height),
            label_widths,
            &seeds,
            None,
        );
    }
    let seeds = retained_paths(graph, nodes, width, height, previous);
    if seeds.iter().all(Option::is_some) {
        return Ok(Routes {
            paths: seeds.into_iter().map(Option::unwrap).collect(),
            labels: vec![None; graph.edges.len()],
        });
    }
    let mut best = route_grid(
        graph,
        nodes,
        policy,
        (width, height),
        label_widths,
        &seeds,
        None,
    )?;
    if nodes.len() <= 16 && graph.edges.len() <= 32 {
        let mut score = route_conflicts(graph, &best.paths).0;
        for _ in 0..3 {
            let initial_score = score;
            let conflicts = route_conflicts(graph, &best.paths).1;
            // 只拆除冲突边；其它有效通道显式传回路由器，避免全图反复搜索。
            for edge in conflicts.into_iter().rev().take(8) {
                if score == 0 {
                    break;
                }
                for side in 0..16 {
                    let fixed = best
                        .paths
                        .iter()
                        .enumerate()
                        .map(|(i, path)| {
                            (i != edge && label_widths[i].is_none()).then(|| path.clone())
                        })
                        .collect::<Vec<_>>();
                    if let Ok(candidate) = route_grid(
                        graph,
                        nodes,
                        policy,
                        (width, height),
                        label_widths,
                        &fixed,
                        Some((edge, side)),
                    ) {
                        let next = route_conflicts(graph, &candidate.paths).0;
                        if next < score {
                            best = candidate;
                            score = next;
                        }
                        if score == 0 {
                            break;
                        }
                    }
                }
            }
            if score == initial_score {
                break;
            }
        }
    }
    if nodes.len() <= 16 && graph.edges.len() <= 32 && route_conflicts(graph, &best.paths).0 > 0 {
        let conflicts = route_conflicts(graph, &best.paths).1;
        let mut score = route_conflicts(graph, &best.paths).0;
        for (i, edge) in conflicts.iter().take(6).enumerate() {
            for other in conflicts.iter().skip(i + 1).take(6) {
                for side in 0..16 {
                    let fixed = best
                        .paths
                        .iter()
                        .enumerate()
                        .map(|(n, p)| {
                            (n != *edge && n != *other && label_widths[n].is_none())
                                .then(|| p.clone())
                        })
                        .collect::<Vec<_>>();
                    if let Ok(candidate) = route_grid(
                        graph,
                        nodes,
                        policy,
                        (width, height),
                        label_widths,
                        &fixed,
                        Some((*edge, side)),
                    ) {
                        let next = route_conflicts(graph, &candidate.paths).0;
                        if next < score {
                            best = candidate;
                            score = next;
                        }
                    }
                    if score == 0 {
                        break;
                    }
                }
                if score == 0 {
                    break;
                }
            }
            if score == 0 {
                break;
            }
        }
    }
    Ok(best)
}

/// 旧通道只有在端点、真实轮廓和当前全部障碍仍有效时才能复用。
fn retained_paths(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    width: f64,
    height: f64,
    previous: Option<&crate::PreviousGraph>,
) -> Vec<Option<Vec<Point>>> {
    let Some(previous) = previous.filter(|old| {
        old.direction == graph.direction && !graph.edges.iter().any(|edge| edge.label.is_some())
    }) else {
        return vec![None; graph.edges.len()];
    };
    graph
        .edges
        .iter()
        .map(|edge| {
            if edge.label.is_some() {
                return None;
            }
            let mut points = previous
                .edges
                .iter()
                .find(|old| old.edge_id == edge.id)?
                .points
                .clone();
            if points.len() < 2 {
                return None;
            }
            for (node, at, adjacent) in [
                (edge.from, 0, 1),
                (edge.to, points.len() - 1, points.len() - 2),
            ] {
                let old = previous
                    .nodes
                    .iter()
                    .find(|n| n.node_id == graph.nodes[node].id)?
                    .rect;
                let (rect, shape) = nodes[node];
                let tip = points[at];
                let next = points[adjacent];
                let vertical = (tip.x - next.x).abs() < 0.001;
                let positive = if vertical {
                    next.y > tip.y
                } else {
                    next.x > tip.x
                };
                let fixed = if vertical {
                    rect.x + (tip.x - old.x) / old.width * rect.width
                } else {
                    rect.y + (tip.y - old.y) / old.height * rect.height
                };
                let tip = crate::shape::port(shape, rect, vertical, fixed, positive);
                points[at] = tip;
                if points.len() > 2 {
                    if vertical {
                        points[adjacent].x = tip.x;
                    } else {
                        points[adjacent].y = tip.y;
                    }
                }
                let next = points[adjacent];
                if if vertical {
                    (next.y - tip.y) * if positive { 1.0 } else { -1.0 } <= 0.0
                } else {
                    (next.x - tip.x) * if positive { 1.0 } else { -1.0 } <= 0.0
                } {
                    return None;
                }
            }
            if points.iter().any(|p| {
                !p.x.is_finite()
                    || !p.y.is_finite()
                    || p.x < 1.0
                    || p.x > width - 1.0
                    || p.y < 1.0
                    || p.y > height - 1.0
            }) {
                return None;
            }
            for (i, pair) in points.windows(2).enumerate() {
                if (pair[0].x - pair[1].x).abs() > 0.001 && (pair[0].y - pair[1].y).abs() > 0.001 {
                    return None;
                }
                if nodes.iter().enumerate().any(|(n, (rect, _))| {
                    !(i == 0 && n == edge.from || i == points.len() - 2 && n == edge.to)
                        && segment_intersects(pair[0], pair[1], *rect)
                }) {
                    return None;
                }
            }
            Some(points)
        })
        .collect()
}

fn route_conflicts(graph: &Graph, paths: &[Vec<Point>]) -> (usize, Vec<usize>) {
    let mut score = 0;
    let mut conflicts = Vec::new();
    for (i, a) in paths.iter().enumerate() {
        for (j, b) in paths.iter().enumerate().skip(i + 1) {
            for a in a.windows(2) {
                for b in b.windows(2) {
                    let ah = (a[0].y - a[1].y).abs() < 0.001;
                    let bh = (b[0].y - b[1].y).abs() < 0.001;
                    if ah == bh {
                        let overlap = if ah {
                            (a[0].y - b[0].y).abs() < 0.001
                                && a[0].x.min(a[1].x) < b[0].x.max(b[1].x) - 0.001
                                && b[0].x.min(b[1].x) < a[0].x.max(a[1].x) - 0.001
                        } else {
                            (a[0].x - b[0].x).abs() < 0.001
                                && a[0].y.min(a[1].y) < b[0].y.max(b[1].y) - 0.001
                                && b[0].y.min(b[1].y) < a[0].y.max(a[1].y) - 0.001
                        };
                        let (first, second) = (&graph.edges[i], &graph.edges[j]);
                        if overlap
                            && (first.label.is_some()
                                || second.label.is_some()
                                || first.from != second.from && first.to != second.to
                                || first.from == second.from && first.to == second.to)
                        {
                            score += 1000;
                            for edge in [i, j] {
                                if !conflicts.contains(&edge) {
                                    conflicts.push(edge);
                                }
                            }
                        }
                        continue;
                    }
                    let (h, v) = if ah { (a, b) } else { (b, a) };
                    if v[0].x > h[0].x.min(h[1].x) + 0.001
                        && v[0].x < h[0].x.max(h[1].x) - 0.001
                        && h[0].y > v[0].y.min(v[1].y) + 0.001
                        && h[0].y < v[0].y.max(v[1].y) - 0.001
                    {
                        score += 1;
                        for edge in [i, j] {
                            if !conflicts.contains(&edge) {
                                conflicts.push(edge);
                            }
                        }
                    }
                }
            }
        }
    }
    (score, conflicts)
}

fn clear_label_port(
    (rect, shape): (Rect, Shape),
    original: (Point, Point),
    labels: &[Rect],
) -> (Point, Point) {
    let clear = |(port, stub): (Point, Point)| {
        !labels
            .iter()
            .any(|r| r.contains(stub) || segment_intersects(port, stub, *r))
    };
    if clear(original) {
        return original;
    }
    for (vertical, positive) in [(true, false), (true, true), (false, false), (false, true)] {
        let fixed = if vertical {
            rect.x + rect.width / 2.0
        } else {
            rect.y + rect.height / 2.0
        };
        let port = crate::shape::port(shape, rect, vertical, fixed, positive);
        let stub = if vertical {
            Point {
                x: fixed,
                y: if positive {
                    rect.y + rect.height + CLEARANCE
                } else {
                    rect.y - CLEARANCE
                },
            }
        } else {
            Point {
                x: if positive {
                    rect.x + rect.width + CLEARANCE
                } else {
                    rect.x - CLEARANCE
                },
                y: fixed,
            }
        };
        if clear((port, stub)) {
            return (port, stub);
        }
    }
    original
}

/// 单分叉、多条完整链、单汇合的平面嵌入。输入和输出分别沿两侧，带内按端口次序留通道。
fn branch_routes(graph: &Graph, nodes: &[(Rect, Shape)], width: f64) -> Option<Vec<Vec<Point>>> {
    if graph.direction != Direction::TopToBottom
        || graph.edges.iter().any(|edge| {
            edge.label.is_some()
                || edge.connector.start != crate::Marker::None
                || edge.connector.line != crate::EdgeLine::Solid
        })
    {
        return None;
    }
    let mut incoming = vec![Vec::new(); nodes.len()];
    let mut outgoing = vec![Vec::new(); nodes.len()];
    for edge in &graph.edges {
        incoming[edge.to].push(edge.from);
        outgoing[edge.from].push(edge.to);
    }
    let roots: Vec<_> = (0..nodes.len())
        .filter(|i| incoming[*i].is_empty())
        .collect();
    if roots.len() != 1 {
        return None;
    }
    let root = roots[0];
    let branches = outgoing[root].len();
    let merges: Vec<_> = (0..nodes.len())
        .filter(|i| incoming[*i].len() >= 3)
        .collect();
    if branches < 3 || merges.len() != 1 {
        return None;
    }
    let sink = merges[0];
    if (0..nodes.len())
        .any(|i| i != root && i != sink && (incoming[i].len() != 1 || outgoing[i].len() > 2))
        || outgoing[sink].len() > 1
    {
        return None;
    }
    let mut result = Vec::new();
    for edge in &graph.edges {
        let (a, ashape) = nodes[edge.from];
        let (b, bshape) = nodes[edge.to];
        let points = if edge.from == root {
            let y = b.y - 16.0;
            let lane = 14.0;
            let source = crate::shape::port(ashape, a, false, a.y + a.height / 2.0, false);
            let target = crate::shape::port(bshape, b, true, b.x + b.width / 2.0, false);
            vec![
                source,
                Point {
                    x: lane,
                    y: source.y,
                },
                Point { x: lane, y },
                Point { x: target.x, y },
                target,
            ]
        } else if edge.to == sink {
            let y = a.y + a.height + 16.0;
            let lane = width - 14.0;
            let source = crate::shape::port(ashape, a, true, a.x + a.width / 2.0, true);
            let target = crate::shape::port(bshape, b, false, b.y + b.height / 2.0, true);
            vec![
                source,
                Point { x: source.x, y },
                Point { x: lane, y },
                Point {
                    x: lane,
                    y: target.y,
                },
                target,
            ]
        } else {
            let source = crate::shape::port(ashape, a, true, a.x + a.width / 2.0, true);
            let target = crate::shape::port(bshape, b, true, b.x + b.width / 2.0, false);
            if (source.x - target.x).abs() > 0.01 {
                result.push(Vec::new());
                continue;
            }
            vec![source, target]
        };
        // 通道不足时交还一般路由，不能穿节点作为回退。
        if points.windows(2).any(|pair| {
            nodes.iter().enumerate().any(|(i, (r, _))| {
                i != edge.from && i != edge.to && segment_intersects(pair[0], pair[1], *r)
            })
        }) {
            return None;
        }
        result.push(simplify(points));
    }
    Some(result)
}

fn orthogonal_cross(first: &[Point], second: &[Point]) -> bool {
    first.windows(2).any(|a| {
        second.windows(2).any(|b| {
            let (a0, a1) = (a[0], a[1]);
            let (b0, b1) = (b[0], b[1]);
            if (a0.y - a1.y).abs() < 0.001 && (b0.x - b1.x).abs() < 0.001 {
                a0.y > b0.y.min(b1.y) + 0.001
                    && a0.y < b0.y.max(b1.y) - 0.001
                    && b0.x > a0.x.min(a1.x) + 0.001
                    && b0.x < a0.x.max(a1.x) - 0.001
            } else if (a0.x - a1.x).abs() < 0.001 && (b0.y - b1.y).abs() < 0.001 {
                a0.x > b0.x.min(b1.x) + 0.001
                    && a0.x < b0.x.max(b1.x) - 0.001
                    && b0.y > a0.y.min(a1.y) + 0.001
                    && b0.y < a0.y.max(a1.y) - 0.001
            } else {
                false
            }
        })
    })
}

fn segment_intersects(a: Point, b: Point, r: Rect) -> bool {
    if (a.x - b.x).abs() < 0.001 {
        a.x > r.x && a.x < r.x + r.width && a.y.min(b.y) < r.y + r.height && a.y.max(b.y) > r.y
    } else {
        a.y > r.y && a.y < r.y + r.height && a.x.min(b.x) < r.x + r.width && a.x.max(b.x) > r.x
    }
}

fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut result: Vec<Point> = Vec::new();
    for point in points {
        if result
            .last()
            .is_some_and(|last| (last.x - point.x).abs() + (last.y - point.y).abs() < 0.001)
        {
            continue;
        }
        while result.len() >= 2 {
            let a = result[result.len() - 2];
            let b = result[result.len() - 1];
            if ((a.x - b.x).abs() < 0.001
                && (b.x - point.x).abs() < 0.001
                && (b.y - a.y) * (point.y - b.y) >= 0.0)
                || ((a.y - b.y).abs() < 0.001
                    && (b.y - point.y).abs() < 0.001
                    && (b.x - a.x) * (point.x - b.x) >= 0.0)
            {
                result.pop();
            } else {
                break;
            }
        }
        result.push(point);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_fanout_channels_are_reusable() {
        let crate::Diagram::Graph(graph) = crate::parse_diagram(include_str!(
            "../../../fixtures/mermaid/flowchart/fanout.mmd"
        ))
        .unwrap() else {
            panic!("expected graph")
        };
        let before = crate::layout_graph(
            &graph,
            &crate::LayoutOptions::default(),
            &mut crate::SystemFonts::new().unwrap(),
        )
        .unwrap();
        let nodes = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (before.previous.nodes[i].rect, node.shape))
            .collect::<Vec<_>>();
        let seeds = retained_paths(
            &graph,
            &nodes,
            before.scene.width,
            before.scene.height,
            Some(&before.previous),
        );
        assert_eq!(
            seeds.iter().filter(|p| p.is_some()).count(),
            graph.edges.len()
        );
    }
    #[test]
    fn fanout_edits_preserve_clear_channels() {
        let source = include_str!("../../../fixtures/mermaid/flowchart/fanout.mmd");
        let crate::Diagram::Graph(graph) = crate::parse_diagram(source).unwrap() else {
            panic!("expected graph")
        };
        let mut fonts = crate::SystemFonts::new().unwrap();
        for width in [700.0, 1100.0] {
            let before = crate::layout_graph(
                &graph,
                &crate::LayoutOptions {
                    width,
                    ..Default::default()
                },
                &mut fonts,
            )
            .unwrap();
            for edited in [
                format!("{source}\nJ1 --> added[新增叶]\n"),
                source.replacen("J1 --> done([汇合])", "", 1),
            ] {
                let crate::Diagram::Graph(graph) = crate::parse_diagram(&edited).unwrap() else {
                    panic!("expected graph")
                };
                let after = crate::layout_graph(
                    &graph,
                    &crate::LayoutOptions {
                        width,
                        previous: Some(before.previous.clone()),
                        ..Default::default()
                    },
                    &mut fonts,
                )
                .unwrap();
                let paths = after
                    .previous
                    .edges
                    .iter()
                    .map(|edge| edge.points.clone())
                    .collect::<Vec<_>>();
                let (score, _) = route_conflicts(&graph, &paths);
                assert_eq!(score, 0, "width={width} source={edited} paths={paths:?}");
            }
        }
    }
}
