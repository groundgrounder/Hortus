//! previous 位置保持：改宽校验、水平 band 装箱与局部重排。

use super::*;

pub(super) fn retain_positions(
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
    let previous_index: HashMap<&str, usize> = previous
        .nodes
        .iter()
        .enumerate()
        .map(|(index, anchor)| (anchor.node_id.as_str(), index))
        .collect();
    if let Some(reason) = width_band_reason(
        graph,
        previous,
        &previous_index,
        boxes,
        groups,
        width_changed,
    ) {
        return (0, reason.into());
    }
    // 横向改宽按旧行带整体装箱，不能把单个溢出节点依次丢到行尾下方。
    if width_changed
        && matches!(
            graph.direction,
            Direction::LeftToRight | Direction::RightToLeft
        )
        && previous.nodes.len() == graph.nodes.len()
    {
        return retain_horizontal_bands(
            graph,
            options,
            boxes,
            groups,
            previous,
            &previous_index,
            width_initial.as_ref(),
        );
    }
    let mut adjusted = previous.clone();
    compact_right_edge_leaves(
        graph,
        options,
        boxes,
        previous,
        &previous_index,
        &mut adjusted,
    );
    // 改宽仍优先保留旧中心和分组；只有实际放不下的节点重新找位。
    let old: Vec<_> = graph
        .nodes
        .iter()
        .map(|node| {
            previous_index
                .get(node.id.as_str())
                .map(|index| &adjusted.nodes[*index])
        })
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
    let current_edges: HashMap<usize, &crate::Edge> =
        graph.edges.iter().fold(HashMap::new(), |mut map, edge| {
            map.entry(edge.id).or_insert(edge);
            map
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
                        current_edges.get(&old_edge.edge_id).is_some_and(|edge| {
                            edge.from != index
                                && edge.to != index
                                && !parent.is_some_and(|parent| {
                                    graph.nodes[edge.from].id == parent.node_id
                                        || graph.nodes[edge.to].id == parent.node_id
                                })
                                && old_edge.points.windows(2).any(|pair| {
                                    segment_intersects(
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

fn compact_right_edge_leaves(
    graph: &Graph,
    options: &LayoutOptions,
    boxes: &[BoxedNode],
    previous: &crate::PreviousGraph,
    previous_index: &HashMap<&str, usize>,
    adjusted: &mut crate::PreviousGraph,
) {
    // 同层支路在右边界新增叶时，只压缩末端的空白；旧节点最多移动 48px。
    for (i, node) in graph.nodes.iter().enumerate() {
        if graph.direction != Direction::TopToBottom
            || previous_index.contains_key(node.id.as_str())
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
        let Some(anchor) = previous_index
            .get(graph.nodes[parent].id.as_str())
            .map(|index| &previous.nodes[*index])
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
}

fn width_band_reason(
    graph: &Graph,
    previous: &crate::PreviousGraph,
    previous_index: &HashMap<&str, usize>,
    boxes: &[BoxedNode],
    groups: &[usize],
    width_changed: bool,
) -> Option<&'static str> {
    if !width_changed {
        return None;
    }
    let common = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            previous_index
                .get(node.id.as_str())
                .map(|old| (index, *old))
        })
        .collect::<Vec<_>>();
    let old_rects = previous
        .nodes
        .iter()
        .map(|anchor| anchor.rect)
        .collect::<Vec<_>>();
    let new_rects = boxes.iter().map(|node| node.rect).collect::<Vec<_>>();
    let old_bands = vertical_bands(&old_rects);
    let new_bands = vertical_bands(&new_rects);
    let row_membership_matches = common.iter().all(|(a, old_a)| {
        common.iter().all(|(b, old_b)| {
            (old_bands[*old_a] == old_bands[*old_b]) == (new_bands[*a] == new_bands[*b])
        })
    });
    if !row_membership_matches {
        return Some("width_row_bands_invalidated");
    }
    // 改宽后旧行带可能已不再能容纳新框宽；若共同成员的带归属与目标宽度
    // 初排不同，保留旧分组会同时破坏行带和旧通道的一致性。
    let same_group = |a: &crate::GraphAnchor, b: &crate::GraphAnchor| a.group == b.group;
    let matches = common.iter().all(|(a, old_a)| {
        common.iter().all(|(b, old_b)| {
            same_group(&previous.nodes[*old_a], &previous.nodes[*old_b])
                == (groups[*a] == groups[*b])
        })
    });
    if !matches {
        return Some("width_group_invalidated");
    }
    None
}

fn retain_horizontal_bands(
    graph: &Graph,
    options: &LayoutOptions,
    boxes: &mut [BoxedNode],
    groups: &mut [usize],
    previous: &crate::PreviousGraph,
    previous_index: &HashMap<&str, usize>,
    width_initial: Option<&(Vec<BoxedNode>, Vec<usize>)>,
) -> (usize, String) {
    let mut rows = BTreeMap::<usize, Vec<usize>>::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        if let Some(old) = previous_index.get(node.id.as_str()) {
            rows.entry(previous.nodes[*old].group).or_default().push(i);
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
                let index = previous_index
                    .get(graph.nodes[i].id.as_str())
                    .expect("row members come from previous");
                &previous.nodes[*index]
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
            let mut x = ((left + right - total) / 2.0).clamp(OUTER, options.width - OUTER - total);
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
                boxes.clone_from_slice(initial_boxes);
                groups.copy_from_slice(initial_groups);
                return (0, "width_row_bands_invalidated".into());
            }
        }
        return (boxes.len(), "retained_horizontal_bands".into());
    }
    (0, "horizontal_band_capacity_changed".into())
}

fn same_band_partition(a: &[usize], b: &[usize]) -> bool {
    a.len() == b.len()
        && (0..a.len())
            .all(|left| (0..a.len()).all(|right| (a[left] == a[right]) == (b[left] == b[right])))
}

pub(super) fn vertical_bands(rects: &[Rect]) -> Vec<usize> {
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
