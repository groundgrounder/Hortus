//! 基于节点外侧可见网格的确定性正交路由；已占用通道作为代价。
use crate::{Direction, Graph, LayoutError, Point, Rect, Shape};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

const CLEARANCE: f64 = 10.0;

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

pub(crate) fn route(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    accepted: &[bool],
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
            accepted,
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
        accepted,
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
                        accepted,
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
                        accepted,
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

fn route_grid(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    accepted: &[bool],
    size: (f64, f64),
    label_widths: &[Option<f64>],
    seeds: &[Option<Vec<Point>>],
    preferred: Option<(usize, usize)>,
) -> Result<Routes, LayoutError> {
    let (width, height) = size;
    let vertical = matches!(
        graph.direction,
        Direction::TopToBottom | Direction::BottomToTop
    );
    let positive = matches!(
        graph.direction,
        Direction::TopToBottom | Direction::LeftToRight
    );
    let mut ends = Vec::new();
    let mut edge_sides = Vec::with_capacity(graph.edges.len());
    let mut peer_totals = BTreeMap::<(usize, usize), usize>::new();
    for edge in &graph.edges {
        *peer_totals.entry((edge.from, edge.to)).or_default() += 1;
    }
    let mut peer_seen = BTreeMap::<(usize, usize), usize>::new();
    let mut peer_counts = Vec::with_capacity(graph.edges.len());
    let mut peer_slots = Vec::with_capacity(graph.edges.len());
    for edge in &graph.edges {
        let key = (edge.from, edge.to);
        let slot = peer_seen.entry(key).or_default();
        peer_counts.push(peer_totals[&key]);
        peer_slots.push(*slot);
        *slot += 1;
    }
    let mut long_edge_slot = 0usize;
    let mut long_node_side: Vec<Option<bool>> = vec![None; nodes.len()];
    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, _) = nodes[edge.from];
        let (to, _) = nodes[edge.to];
        let mut a_vertical = vertical;
        let mut b_vertical = vertical;
        let mut a_positive = positive;
        let mut b_positive = !positive;
        if edge.from == edge.to {
            a_vertical = false;
            a_positive = true;
            b_vertical = true;
            b_positive = false;
        } else if !accepted[index] {
            a_vertical = false;
            b_vertical = false;
            a_positive = true;
            b_positive = true;
        } else if !vertical && (from.y - to.y).abs() > from.height.max(to.height) {
            // LR/RL 折回下一行，仍从下一行的源码起点继续阅读。
            a_vertical = true;
            b_vertical = true;
            a_positive = true;
            b_positive = false;
        } else if vertical && (from.y - to.y).abs() < 1.0 {
            a_vertical = false;
            b_vertical = false;
            a_positive = from.x < to.x;
            b_positive = !a_positive;
        }
        // 回边从靠近目标的竖直边出发，再沿目标朝向源节点的侧面汇入。这样
        // 不会与同节点的主链端口共用一条竖线，也不会为绕行额外挪动中心端。
        let backward =
            vertical && ((positive && to.y < from.y - 1.0) || (!positive && to.y > from.y + 1.0));
        if backward {
            a_vertical = true;
            a_positive = !positive;
            b_vertical = false;
            b_positive = from.x + from.width / 2.0 > to.x + to.width / 2.0;
        }
        let long_edge = !backward
            && nodes.len() <= 48
            && vertical
            && accepted[index]
            && (to.y - from.y).abs() > from.height + to.height + 60.0;
        if long_edge {
            // 连续的长边交替走左右外侧。相邻跳边的区间只在节点层边界相接，
            // 横向引线不会穿过上一条边的竖向通道。
            let source_side = long_node_side[edge.from].map(|side| !side);
            let target_side = long_node_side[edge.to].map(|side| !side);
            let side = source_side
                .or(target_side)
                .unwrap_or(long_edge_slot % 2 == 1);
            a_vertical = false;
            b_vertical = false;
            a_positive = side;
            b_positive = side;
            long_node_side[edge.from] = Some(side);
            long_node_side[edge.to] = Some(side);
            long_edge_slot += 1;
        }
        if long_edge {
            if graph
                .edges
                .iter()
                .filter(|other| other.from == edge.from)
                .count()
                >= 3
            {
                // 跨带扇出沿左侧，跨带汇合沿右侧，避免来回边共用一条总线。
                a_vertical = false;
                a_positive = false;
            }
            if graph
                .edges
                .iter()
                .filter(|other| other.to == edge.to)
                .count()
                >= 3
            {
                b_vertical = false;
                b_positive = true;
            }
        }
        if let Some((selected, sides)) = preferred
            && selected == index
        {
            if nodes.len() <= 24 {
                a_vertical = if sides & 2 == 0 { vertical } else { !vertical };
                b_vertical = if sides & 8 == 0 { vertical } else { !vertical };
                a_positive = sides & 1 != 0;
                b_positive = sides & 4 != 0;
            } else {
                a_vertical = !vertical;
                b_vertical = !vertical;
                a_positive = sides & 1 != 0;
                b_positive = sides & 2 != 0;
            }
        }
        if backward {
            a_vertical = true;
            a_positive = !positive;
            b_vertical = false;
            b_positive = from.x + from.width / 2.0 > to.x + to.width / 2.0;
        }
        // 重复边不合并成一条线。不同目标的同源前缀和同目的后缀允许共用端口。
        let peers = peer_counts[index];
        let slot = peer_slots[index];
        if peers > 1 && slot > 0 {
            // 平行边改走独立外侧通道，标签不能挤在相距数像素的共同前后缀上。
            a_vertical = !vertical;
            b_vertical = !vertical;
            a_positive = slot % 2 == 1;
            b_positive = a_positive;
        }
        edge_sides.push((a_vertical, a_positive, b_vertical, b_positive));
    }
    let mut port_groups = BTreeMap::<(usize, bool, bool, bool), Vec<usize>>::new();
    for (index, edge) in graph.edges.iter().enumerate() {
        let side = edge_sides[index];
        port_groups
            .entry((edge.from, true, side.0, side.1))
            .or_default()
            .push(index);
        port_groups
            .entry((edge.to, false, side.2, side.3))
            .or_default()
            .push(index);
    }
    for (&(_, outgoing, is_vertical, _), incident) in &mut port_groups {
        incident.sort_by(|a, b| {
            let first = &graph.edges[*a];
            let second = &graph.edges[*b];
            let first = if outgoing {
                nodes[first.to].0
            } else {
                nodes[first.from].0
            };
            let second = if outgoing {
                nodes[second.to].0
            } else {
                nodes[second.from].0
            };
            let coordinate = |rect: Rect| {
                if is_vertical {
                    rect.x + rect.width / 2.0
                } else {
                    rect.y + rect.height / 2.0
                }
            };
            coordinate(first)
                .total_cmp(&coordinate(second))
                .then(a.cmp(b))
        });
    }
    let port_delta = |edge_index: usize, outgoing: bool, is_vertical: bool, is_positive: bool| {
        let node_index = if outgoing {
            graph.edges[edge_index].from
        } else {
            graph.edges[edge_index].to
        };
        let incident = &port_groups[&(node_index, outgoing, is_vertical, is_positive)];
        if nodes.len() > 48
            || (incident.len() > 2
                && incident
                    .iter()
                    .all(|index| graph.edges[*index].label.is_none()))
        {
            return 0.0;
        }
        let position = incident
            .iter()
            .position(|index| *index == edge_index)
            .unwrap_or(0);
        (position as f64 - (incident.len() - 1) as f64 / 2.0) * 12.0
    };
    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, fs) = nodes[edge.from];
        let (to, ts) = nodes[edge.to];
        let (a_vertical, a_positive, b_vertical, b_positive) = edge_sides[index];
        let peers = peer_counts[index];
        let slot = peer_slots[index];
        let endpoint =
            |node_index: usize, rect: Rect, shape, is_vertical, is_positive, delta: f64| {
                let clearance = if peers > 1 && slot > 0 {
                    28.0 + slot as f64 * 4.0
                } else {
                    CLEARANCE
                };
                let fixed = if is_vertical {
                    rect.x + rect.width / 2.0 + delta.clamp(-rect.width * 0.22, rect.width * 0.22)
                } else {
                    rect.y
                        + rect.height / 2.0
                        + delta.clamp(-rect.height * 0.22, rect.height * 0.22)
                };
                let port = crate::shape::port(shape, rect, is_vertical, fixed, is_positive);
                // 平行边的外侧短段不能跨过邻近节点；其余路径仍由网格分配独立通道。
                let edge_axis = if is_vertical {
                    rect.y + if is_positive { rect.height } else { 0.0 }
                } else {
                    rect.x + if is_positive { rect.width } else { 0.0 }
                };
                let clearance = nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (other, _))| {
                        if index == node_index {
                            return None;
                        }
                        let (lo, hi, near) = if is_vertical {
                            (
                                other.x - 6.0,
                                other.x + other.width + 6.0,
                                if is_positive {
                                    other.y - 6.0
                                } else {
                                    other.y + other.height + 6.0
                                },
                            )
                        } else {
                            (
                                other.y - 6.0,
                                other.y + other.height + 6.0,
                                if is_positive {
                                    other.x - 6.0
                                } else {
                                    other.x + other.width + 6.0
                                },
                            )
                        };
                        let distance = if is_positive {
                            near - edge_axis
                        } else {
                            edge_axis - near
                        };
                        (fixed > lo && fixed < hi && distance > 0.0)
                            .then_some((distance - 1.0).max(1.0))
                    })
                    .fold(clearance, f64::min);
                let stub = if is_vertical {
                    Point {
                        x: fixed,
                        y: if is_positive {
                            rect.y + rect.height + clearance
                        } else {
                            rect.y - clearance
                        },
                    }
                } else {
                    Point {
                        x: if is_positive {
                            rect.x + rect.width + clearance
                        } else {
                            rect.x - clearance
                        },
                        y: fixed,
                    }
                };
                (port, stub)
            };
        ends.push((
            endpoint(
                edge.from,
                from,
                fs,
                a_vertical,
                a_positive,
                port_delta(index, true, a_vertical, a_positive),
            ),
            endpoint(
                edge.to,
                to,
                ts,
                b_vertical,
                b_positive,
                port_delta(index, false, b_vertical, b_positive),
            ),
        ));
    }
    let mut xs = vec![8.0, 18.0, width - 18.0, width - 8.0];
    let mut ys = vec![8.0, 18.0, height - 18.0, height - 8.0];
    for (rect, _) in nodes {
        xs.extend([
            rect.x - CLEARANCE,
            rect.x + rect.width / 2.0,
            rect.x + rect.width + CLEARANCE,
        ]);
        ys.extend([
            rect.y - CLEARANCE,
            rect.y + rect.height / 2.0,
            rect.y + rect.height + CLEARANCE,
        ]);
    }
    for ((source, a), (target, b)) in &ends {
        xs.extend([source.x, a.x, target.x, b.x]);
        ys.extend([source.y, a.y, target.y, b.y]);
    }
    for path in seeds.iter().flatten() {
        for point in path {
            xs.push(point.x);
            ys.push(point.y);
        }
    }
    let sort = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    };
    sort(&mut xs);
    sort(&mut ys);
    if nodes.len() <= 24 {
        // 走廊中线给相反方向的入边、出边各留一条通道。
        xs.extend(
            xs.clone()
                .windows(2)
                .filter(|p| p[1] - p[0] >= 8.0)
                .map(|p| (p[0] + p[1]) / 2.0),
        );
        ys.extend(
            ys.clone()
                .windows(2)
                .filter(|p| p[1] - p[0] >= 8.0)
                .map(|p| (p[0] + p[1]) / 2.0),
        );
        sort(&mut xs);
        sort(&mut ys);
    }
    let nx = xs.len();
    let ny = ys.len();
    let count = nx * ny;
    if count > 2_000_000 {
        return Err(LayoutError(
            "Graph routing grid exceeds the supported size".into(),
        ));
    }
    let mut horizontal = vec![false; count];
    let mut vertical_blocked = vec![false; count];
    for (rect, _) in nodes {
        let (left, right, top, bottom) = (
            rect.x - 6.0,
            rect.x + rect.width + 6.0,
            rect.y - 6.0,
            rect.y + rect.height + 6.0,
        );
        let x0 = xs.partition_point(|x| *x < left).saturating_sub(1);
        let x1 = xs.partition_point(|x| *x <= right).min(nx - 1);
        let y0 = ys.partition_point(|y| *y < top).saturating_sub(1);
        let y1 = ys.partition_point(|y| *y <= bottom).min(ny - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if x + 1 < nx && ys[y] > top && ys[y] < bottom && xs[x] < right && xs[x + 1] > left
                {
                    horizontal[y * nx + x] = true;
                }
                if y + 1 < ny && xs[x] > left && xs[x] < right && ys[y] < bottom && ys[y + 1] > top
                {
                    vertical_blocked[y * nx + x] = true;
                }
            }
        }
    }
    let mut h_use = vec![0u16; count];
    let mut v_use = vec![0u16; count];
    let mut h_owner = vec![ChannelOwner::EMPTY; count];
    let mut v_owner = vec![ChannelOwner::EMPTY; count];
    let mut h_block_prefix = vec![0usize; count];
    let mut v_block_prefix = vec![0usize; count];
    for y in 0..ny {
        for x in 0..nx {
            let i = y * nx + x;
            h_block_prefix[i] =
                usize::from(horizontal[i]) + if x > 0 { h_block_prefix[i - 1] } else { 0 };
            v_block_prefix[i] =
                usize::from(vertical_blocked[i]) + if y > 0 { v_block_prefix[i - nx] } else { 0 };
        }
    }
    let mut distances = vec![u64::MAX; count * 3];
    let mut generations = vec![0usize; count * 3];
    let mut previous = vec![usize::MAX; count * 3];
    let locate = |point: Point| {
        let x = xs.partition_point(|x| *x < point.x - 0.001).min(nx - 1);
        let y = ys.partition_point(|y| *y < point.y - 0.001).min(ny - 1);
        y * nx + x
    };
    let point = |index: usize| Point {
        x: xs[index % nx],
        y: ys[index / nx],
    };
    // 端口到网格的短段也占用通道；提前登记，防止较早的边遮住后续边的端点身份。
    for (edge_index, ((source, a), (target, b))) in ends.iter().enumerate() {
        if seeds[edge_index].is_some() {
            continue;
        }
        for (p, q) in [(*source, *a), (*target, *b)] {
            let (from, to) = (locate(p), locate(q));
            let h = from / nx == to / nx;
            let mut i = from.min(to);
            while i < from.max(to) {
                if h {
                    h_use[i] = h_use[i].saturating_add(1);
                    h_owner[i].occupy(edge_index, graph);
                } else {
                    v_use[i] = v_use[i].saturating_add(1);
                    v_owner[i].occupy(edge_index, graph);
                }
                i += if h { 1 } else { nx };
            }
        }
    }
    for (edge_index, path) in seeds
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
    {
        for pair in path.windows(2) {
            let (from, to) = (locate(pair[0]), locate(pair[1]));
            let h = from / nx == to / nx;
            let mut i = from.min(to);
            while i < from.max(to) {
                if h {
                    h_use[i] = h_use[i].saturating_add(1);
                    h_owner[i].occupy(edge_index, graph);
                } else {
                    v_use[i] = v_use[i].saturating_add(1);
                    v_owner[i].occupy(edge_index, graph);
                }
                i += if h { 1 } else { nx };
            }
        }
    }
    let mut result = seeds
        .iter()
        .map(|p| p.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    let mut label_rects = vec![None; ends.len()];
    let rects = nodes.iter().map(|(rect, _)| *rect).collect::<Vec<_>>();
    let mut protected = Vec::<Rect>::new();
    let mut routing_order = (0..ends.len())
        .filter(|i| seeds[*i].is_none())
        .collect::<Vec<_>>();
    routing_order.sort_by(|a, b| {
        label_widths[*a]
            .is_none()
            .cmp(&label_widths[*b].is_none())
            .then_with(|| {
                if nodes.len() > 48 {
                    return a.cmp(b);
                }
                let distance = |i: usize| {
                    let ((_, a), (_, b)) = ends[i];
                    (a.x - b.x).abs() + (a.y - b.y).abs()
                };
                (Some(*a) == preferred.map(|(i, _)| i))
                    .cmp(&(Some(*b) == preferred.map(|(i, _)| i)))
                    .then_with(|| distance(*a).total_cmp(&distance(*b)))
                    .then(a.cmp(b))
            })
    });
    for edge_index in routing_order {
        let (source, a) = clear_label_port(
            nodes[graph.edges[edge_index].from],
            ends[edge_index].0,
            &protected,
        );
        let (target, b) = clear_label_port(
            nodes[graph.edges[edge_index].to],
            ends[edge_index].1,
            &protected,
        );
        let generation = edge_index + 1;
        let start = locate(a);
        let goal = locate(b);
        // 优先比较确定的少弯路径。大图无需在每条边上遍历整个二维网格。
        let current = &graph.edges[edge_index];
        let compatible = |owner: ChannelOwner, _p: Point, _q: Point| {
            owner.first == usize::MAX || owner.first == edge_index || {
                (owner.common_from == current.from || owner.common_to == current.to)
                    && !(owner.common_from == current.from && owner.common_to == current.to)
                    && !owner.labelled
                    && current.label.is_none()
            }
        };
        let score_segment = |a: usize, b: usize| -> Option<f64> {
            let horizontal_axis = a / nx == b / nx;
            let (lo, hi, step, blocked, usage, other_use, owners) = if horizontal_axis {
                (
                    a.min(b),
                    a.max(b),
                    1,
                    &h_block_prefix,
                    &h_use,
                    &v_use,
                    &h_owner,
                )
            } else if a % nx == b % nx {
                (
                    a.min(b),
                    a.max(b),
                    nx,
                    &v_block_prefix,
                    &v_use,
                    &h_use,
                    &v_owner,
                )
            } else {
                return None;
            };
            if lo == hi {
                return Some(0.0);
            }
            let before = if horizontal_axis {
                lo % nx > 0
            } else {
                lo >= nx
            };
            let blocked_count = blocked[hi - step] - if before { blocked[lo - step] } else { 0 };
            if blocked_count != 0 {
                return None;
            }
            let p = point(a);
            let q = point(b);
            if protected.iter().any(|rect| segment_intersects(p, q, *rect)) {
                return None;
            }
            let mut cost = (p.x - q.x).abs() + (p.y - q.y).abs();
            let mut i = lo;
            while i < hi {
                if usage[i] > 0 && !compatible(owners[i], point(i), point(i + step)) {
                    cost += 1_000_000.0;
                }
                // 端点的共同前后缀不计作交叉；中间垂直占用必须绕开。
                if i != lo && other_use[i] > 0 {
                    // A candidate that crosses an occupied perpendicular channel is
                    // never preferable to a longer orthogonal detour on small graphs.
                    cost += 1_000_000.0;
                }
                i += step;
            }
            Some(cost)
        };
        let sx = start % nx;
        let sy = start / nx;
        let gx = goal % nx;
        let gy = goal / nx;
        let mut candidates = vec![
            vec![start, sy * nx + gx, goal],
            vec![start, gy * nx + sx, goal],
        ];
        let mut columns = vec![0, nx - 1, sx, gx, (sx + gx) / 2];
        let mut rows = vec![0, ny - 1, sy, gy, (sy + gy) / 2];
        for delta in 1..=3 {
            columns.extend([
                sx.saturating_sub(delta),
                (sx + delta).min(nx - 1),
                gx.saturating_sub(delta),
                (gx + delta).min(nx - 1),
            ]);
            rows.extend([
                sy.saturating_sub(delta),
                (sy + delta).min(ny - 1),
                gy.saturating_sub(delta),
                (gy + delta).min(ny - 1),
            ]);
        }
        columns.sort_unstable();
        columns.dedup();
        rows.sort_unstable();
        rows.dedup();
        for x in &columns {
            candidates.push(vec![start, sy * nx + x, gy * nx + x, goal]);
        }
        for y in &rows {
            candidates.push(vec![start, y * nx + sx, y * nx + gx, goal]);
        }
        if nodes.len() <= 48 {
            // 局部带与独立回边的少弯组合，先于完整网格搜索。
            for x in &columns {
                for y in &rows {
                    candidates.push(vec![start, sy * nx + x, y * nx + x, y * nx + gx, goal]);
                    candidates.push(vec![start, y * nx + sx, y * nx + x, gy * nx + x, goal]);
                }
            }
        }
        let mut best: Option<(f64, Vec<usize>)> = None;
        for candidate in candidates {
            let mut score = 8.0 * candidate.len() as f64;
            let mut valid = true;
            for segment in candidate.windows(2) {
                if let Some(cost) = score_segment(segment[0], segment[1]) {
                    score += cost;
                } else {
                    valid = false;
                    break;
                }
            }
            if valid && candidate.len() > 1 {
                // 源端口和目端口的 10px 短段不在候选网格内，也要计入转折次数；
                // 否则两条等长曼哈顿路径会因候选顺序而紧贴节点折返。
                let axis = |from: Point, to: Point| {
                    if (from.y - to.y).abs() < 0.001 {
                        Some(true)
                    } else if (from.x - to.x).abs() < 0.001 {
                        Some(false)
                    } else {
                        None
                    }
                };
                let source_axis = axis(source, a);
                let first_axis = axis(a, point(candidate[1]));
                let last_axis = axis(
                    point(candidate[candidate.len() - 2]),
                    point(candidate[candidate.len() - 1]),
                );
                let target_axis = axis(b, target);
                if source_axis != first_axis {
                    score += 8.0;
                }
                if last_axis != target_axis {
                    score += 8.0;
                }
            }
            if valid && let Some(label_width) = label_widths[edge_index] {
                let mut points = vec![source];
                points.extend(candidate.iter().copied().map(point));
                points.push(target);
                result[edge_index] = simplify(points);
                valid = crate::graph_layout::place_edge_label(
                    edge_index,
                    label_width,
                    &result,
                    &rects,
                    &protected,
                    width,
                )
                .is_some();
                result[edge_index].clear();
            }
            if valid && best.as_ref().is_none_or(|(old, _)| score < *old) {
                best = Some((score, candidate));
            }
        }
        let indices = if let Some((_, indices)) = best.filter(|(score, _)| {
            // 小图遇到已有边的独占通道时扩大搜索，不能仅因少弯候选可达就接受交叉。
            *score < 4_000.0 || nodes.len() > 48 || label_widths[edge_index].is_some()
        }) {
            indices
        } else {
            // 少数受阻路径才扩大到完整可见网格，禁止退化成穿过节点的直线。
            let heuristic = |index| {
                let p = point(index);
                ((p.x - b.x).abs() + (p.y - b.y).abs()) * 1000.0
            };
            let mut heap = BinaryHeap::new();
            let first = start * 3;
            distances[first] = 0;
            generations[first] = generation;
            previous[first] = usize::MAX;
            heap.push(Reverse((heuristic(start) as u64, 0u64, first)));
            let mut last = None;
            while let Some(Reverse((_, cost, state))) = heap.pop() {
                if generations[state] != generation || distances[state] != cost {
                    continue;
                }
                let index = state / 3;
                let axis = state % 3;
                if index == goal {
                    last = Some(state);
                    break;
                }
                let x = index % nx;
                let y = index / nx;
                for (valid, next, next_axis, segment) in [
                    (x > 0, index.saturating_sub(1), 1, index.saturating_sub(1)),
                    (x + 1 < nx, index + 1, 1, index),
                    (y > 0, index.saturating_sub(nx), 2, index.saturating_sub(nx)),
                    (y + 1 < ny, index + nx, 2, index),
                ] {
                    if !valid
                        || if next_axis == 1 {
                            horizontal[segment]
                        } else {
                            vertical_blocked[segment]
                        }
                    {
                        continue;
                    }
                    let p = point(index);
                    let q = point(next);
                    if protected.iter().any(|rect| segment_intersects(p, q, *rect)) {
                        continue;
                    }
                    let owner = if next_axis == 1 {
                        h_owner[segment]
                    } else {
                        v_owner[segment]
                    };
                    let penalty = if compatible(owner, p, q) {
                        0.0
                    } else {
                        1_000_000.0
                    };
                    let crosses = if next_axis == 1 {
                        v_use[index] > 0 || (index >= nx && v_use[index - nx] > 0)
                    } else {
                        h_use[index] > 0 || (index % nx > 0 && h_use[index - 1] > 0)
                    };
                    let crossing_penalty =
                        if nodes.len() <= 48 && index != start && index != goal && crosses {
                            1_000_000.0
                        } else {
                            0.0
                        };
                    let bend = if axis != 0 && axis != next_axis {
                        8.0
                    } else {
                        0.0
                    };
                    let next_cost = cost
                        + (((p.x - q.x).abs()
                            + (p.y - q.y).abs()
                            + bend
                            + penalty
                            + crossing_penalty)
                            * 1000.0) as u64;
                    let next_state = next * 3 + next_axis;
                    if generations[next_state] != generation || next_cost < distances[next_state] {
                        generations[next_state] = generation;
                        distances[next_state] = next_cost;
                        previous[next_state] = state;
                        heap.push(Reverse((
                            next_cost + heuristic(next) as u64,
                            next_cost,
                            next_state,
                        )));
                    }
                }
            }
            let Some(mut state) = last else {
                // 通道占用可能把相邻的短边完全封死，但只要两段折线不穿过
                // 其它节点，就仍然是可读的有效路径。仅在完整搜索无路时启用。
                let elbows = [Point { x: a.x, y: b.y }, Point { x: b.x, y: a.y }];
                let clear = |points: &[Point]| {
                    points.windows(2).all(|segment| {
                        !nodes.iter().enumerate().any(|(index, (rect, _))| {
                            index != current.from
                                && index != current.to
                                && segment_intersects(segment[0], segment[1], *rect)
                        }) && !protected
                            .iter()
                            .any(|rect| segment_intersects(segment[0], segment[1], *rect))
                    })
                };
                if let Some(elbow) = elbows.iter().find(|elbow| {
                    let points = [a, **elbow, b];
                    clear(&points)
                }) {
                    result[edge_index] = simplify(vec![source, a, *elbow, b, target]);
                    if let Some(label_width) = label_widths[edge_index] {
                        let rect = crate::graph_layout::place_edge_label(
                            edge_index,
                            label_width,
                            &result,
                            &rects,
                            &protected,
                            width,
                        )
                        .ok_or_else(|| {
                            LayoutError(format!("No label channel for edge {}", current.id))
                        })?;
                        label_rects[edge_index] = Some(rect);
                        protected.push(rect);
                    }
                    continue;
                }
                return Err(LayoutError(format!(
                    "No orthogonal route for edge {} ({} -> {}, {:?} -> {:?})",
                    current.id, graph.nodes[current.from].id, graph.nodes[current.to].id, a, b
                )));
            };
            let mut indices = Vec::new();
            loop {
                indices.push(state / 3);
                if previous[state] == usize::MAX {
                    break;
                }
                state = previous[state];
            }
            indices.reverse();
            indices
        };
        for pair in indices.windows(2) {
            let (mut index, end) = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            let h = pair[0] / nx == pair[1] / nx;
            while index < end {
                if h {
                    h_use[index] = h_use[index].saturating_add(1);
                    h_owner[index].occupy(edge_index, graph);
                } else {
                    v_use[index] = v_use[index].saturating_add(1);
                    v_owner[index].occupy(edge_index, graph);
                }
                index += if h { 1 } else { nx };
            }
        }
        let mut points = vec![source];
        points.extend(indices.into_iter().map(point));
        points.push(target);
        result[edge_index] = simplify(points);
        if let Some(label_width) = label_widths[edge_index] {
            let rect = crate::graph_layout::place_edge_label(
                edge_index,
                label_width,
                &result,
                &rects,
                &protected,
                width,
            )
            .ok_or_else(|| LayoutError(format!("No label channel for edge {}", current.id)))?;
            label_rects[edge_index] = Some(rect);
            protected.push(Rect {
                x: rect.x - 3.0,
                y: rect.y - 3.0,
                width: rect.width + 6.0,
                height: rect.height + 6.0,
            });
        }
    }
    Ok(Routes {
        paths: result,
        labels: label_rects,
    })
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
