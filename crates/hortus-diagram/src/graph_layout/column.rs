//! 近树图的分列布局、旧列成员恢复与列稳定性。

use super::*;

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
pub(super) fn column_tree_layout(
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
    let paths = column_paths(
        graph,
        boxes,
        &positions,
        root,
        &branches,
        &parent,
        &groups,
        &lanes,
        &right_lanes,
        corridor_y,
        top_lane_y,
    )?;
    if count_edge_crossings(&paths) != 0 {
        return None;
    }
    let (retained, reason) = column_stability(graph, previous, &positions, boxes, &groups);
    Some((positions, height, groups, paths, retained, reason))
}

#[allow(clippy::too_many_arguments)]
fn column_paths(
    graph: &Graph,
    boxes: &[BoxedNode],
    positions: &[(f64, f64)],
    root: usize,
    branches: &[usize],
    parent: &[Option<usize>],
    groups: &[usize],
    lanes: &[f64],
    right_lanes: &[f64],
    corridor_y: f64,
    top_lane_y: f64,
) -> Option<Vec<Vec<Point>>> {
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
    Some(paths)
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
