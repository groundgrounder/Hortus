//! 路由前的端点方向推断、同源并行边槽位与端口偏移。

use super::*;
use std::collections::BTreeMap;

pub(super) type EdgeSide = (bool, bool, bool, bool);
pub(super) type Endpoint = (Point, Point);
pub(super) type Ends = (Endpoint, Endpoint);

pub(super) struct EdgePlan {
    pub sides: Vec<EdgeSide>,
    pub peer_counts: Vec<usize>,
    pub peer_slots: Vec<usize>,
}

pub(super) fn plan_edges(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    accepted: &[bool],
    lr_bands: Option<&[usize]>,
    vertical: bool,
    positive: bool,
    preferred: Option<(usize, usize)>,
) -> EdgePlan {
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
        } else if let Some(bands) = lr_bands {
            if bands[edge.from] < bands[edge.to] {
                // 真正跨 band 的边从源底部离开，并只在目标顶部汇入。
                a_vertical = true;
                b_vertical = true;
                a_positive = true;
                b_positive = false;
            } else if bands[edge.from] == bands[edge.to] {
                // 同一 band 的 LR 边固定走源右侧到目标左侧。
                a_vertical = false;
                b_vertical = false;
                a_positive = true;
                b_positive = false;
            }
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
        if let Some(bands) = lr_bands
            && accepted[index]
        {
            if bands[edge.from] < bands[edge.to] {
                a_vertical = true;
                b_vertical = true;
                a_positive = true;
                b_positive = false;
            } else if bands[edge.from] == bands[edge.to] {
                a_vertical = false;
                b_vertical = false;
                a_positive = true;
                b_positive = false;
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
        // LR 容器内的重复边由 band 决定方向，端口间距与网格通道负责分离；
        // 其余图仍改走独立外侧通道，标签不挤在相距数像素的共同前后缀上。
        let band_directed = lr_bands.is_some() && accepted[index];
        if peers > 1 && slot > 0 && !band_directed {
            a_vertical = !vertical;
            b_vertical = !vertical;
            a_positive = slot % 2 == 1;
            b_positive = a_positive;
        }
        edge_sides.push((a_vertical, a_positive, b_vertical, b_positive));
    }
    EdgePlan {
        sides: edge_sides,
        peer_counts,
        peer_slots,
    }
}

pub(super) fn compute_ends(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    accepted: &[bool],
    lr_bands: Option<&[usize]>,
    plan: &EdgePlan,
    distinct_ports: bool,
) -> Vec<Ends> {
    let edge_sides = &plan.sides;
    let peer_counts = &plan.peer_counts;
    let peer_slots = &plan.peer_slots;
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
        // LR 容器的汇点即使有三条以上无标签入边，也必须保留可分辨的端口。
        let lr_container_sink = lr_bands.is_some()
            && graph.direction == Direction::LeftToRight
            && !outgoing
            && incident
                .iter()
                .all(|index| graph.edges[*index].label.is_none());
        // LR 容器内同一对节点的重复边共用 band 方向，必须各自占用端口；
        // 不同目标的同源前缀和同目的后缀仍允许共用。
        let repeated = lr_bands.is_some()
            && incident.iter().any(|index| {
                incident.iter().any(|other| {
                    other != index
                        && graph.edges[*index].from == graph.edges[*other].from
                        && graph.edges[*index].to == graph.edges[*other].to
                })
            });
        if !distinct_ports
            && !lr_container_sink
            && !repeated
            && (nodes.len() > 48
                || (incident.len() > 2
                    && incident
                        .iter()
                        .all(|index| graph.edges[*index].label.is_none())))
        {
            return 0.0;
        }
        let position = incident
            .iter()
            .position(|index| *index == edge_index)
            .unwrap_or(0);
        let count = incident.len();
        let rect = nodes[node_index].0;
        // 固定 12px 间距经 ±22% 夹紧后不足 6px 时，按节点同侧可用范围均匀压缩。
        // 布局已按入边数扩展汇点高度，因此任意数量的无标签入口仍保持至少 6px。
        let unlabeled = incident
            .iter()
            .all(|index| graph.edges[*index].label.is_none());
        let span = (if is_vertical { rect.width } else { rect.height }) * PORT_SPAN_RATIO;
        let spacing = if distinct_ports && count > 1 {
            (2.0 * span / (count - 1) as f64).min(12.0)
        } else if !unlabeled || count < 2 || span >= MIN_PORT_GAP * (count as f64 - 2.0) {
            12.0
        } else {
            (2.0 * span / (count - 1) as f64).min(12.0)
        };
        (position as f64 - (count - 1) as f64 / 2.0) * spacing
    };
    let mut ends = Vec::with_capacity(graph.edges.len());
    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, fs) = nodes[edge.from];
        let (to, ts) = nodes[edge.to];
        let (a_vertical, a_positive, b_vertical, b_positive) = edge_sides[index];
        let peers = peer_counts[index];
        let slot = peer_slots[index];
        // LR 容器内的重复边共用 band 方向，端口已经分离，外侧短段不能越过目标端口。
        let band_directed = lr_bands.is_some() && accepted[index];
        let endpoint =
            |node_index: usize, rect: Rect, shape, is_vertical, is_positive, delta: f64| {
                let clearance = if peers > 1 && slot > 0 && !band_directed {
                    28.0 + slot as f64 * 4.0
                } else {
                    CLEARANCE
                };
                let fixed = if is_vertical {
                    rect.x
                        + rect.width / 2.0
                        + delta.clamp(-rect.width * PORT_SPAN_RATIO, rect.width * PORT_SPAN_RATIO)
                } else {
                    rect.y
                        + rect.height / 2.0
                        + delta.clamp(
                            -rect.height * PORT_SPAN_RATIO,
                            rect.height * PORT_SPAN_RATIO,
                        )
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
    ends
}
