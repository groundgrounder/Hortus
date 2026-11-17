//! 节点放置：分层换行、LR 容器 band 与分支成组。

use super::*;

pub(super) fn place_nodes(
    graph: &Graph,
    order: &[Vec<usize>],
    ranks: &[usize],
    accepted: &[bool],
    boxes: &mut [BoxedNode],
    available: f64,
    container_members: bool,
) -> (f64, Vec<usize>) {
    let mut groups = vec![0; boxes.len()];
    if container_members && graph.direction == Direction::LeftToRight {
        return place_lr_container_nodes(graph, order, ranks, accepted, boxes, available);
    }
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

/// 顶层 LR 容器的成员按宽度选择最少连续 band；所有 band 共用同一列起点。
fn place_lr_container_nodes(
    graph: &Graph,
    order: &[Vec<usize>],
    ranks: &[usize],
    accepted: &[bool],
    boxes: &mut [BoxedNode],
    available: f64,
) -> (f64, Vec<usize>) {
    let mut groups = vec![0; boxes.len()];
    if order.is_empty() {
        return (OUTER * 2.0, groups);
    }
    let widths = order
        .iter()
        .map(|rank| {
            rank.iter()
                .map(|node| boxes[*node].rect.width)
                .fold(0.0, f64::max)
        })
        .collect::<Vec<_>>();
    let weights = widths
        .iter()
        .map(|width| width + NODE_GAP)
        .collect::<Vec<_>>();
    let band_width = |range: &std::ops::Range<usize>| {
        widths[range.clone()].iter().sum::<f64>() + NODE_GAP * range.len().saturating_sub(1) as f64
    };
    let mut band_count = 1usize;
    let ranges = loop {
        let ranges = crate::partition(&weights, band_count);
        if ranges
            .iter()
            .all(|range| band_width(range) <= available + 0.01)
            || band_count == order.len()
        {
            break ranges;
        }
        band_count += 1;
    };
    let widest = ranges.iter().map(&band_width).fold(0.0, f64::max);
    let origin = OUTER + (available - widest).max(0.0) / 2.0;
    let mut top = OUTER;
    for (band, range) in ranges.into_iter().enumerate() {
        let mut x = origin;
        for rank in range.clone() {
            let mut node_y = top;
            for node in &order[rank] {
                boxes[*node].rect.x = x + (widths[rank] - boxes[*node].rect.width) / 2.0;
                boxes[*node].rect.y = node_y;
                groups[*node] = band;
                node_y += boxes[*node].rect.height + NODE_GAP;
            }
            x += widths[rank] + NODE_GAP;
        }

        // 先按 rank 从左到右设定汇点的垂直中心，再在同一列内消解重叠。
        for rank in range.clone() {
            let mut next_y = top;
            for node in &order[rank] {
                let parents = graph
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(index, edge)| {
                        accepted[*index]
                            && edge.to == *node
                            && edge.from != *node
                            && ranks[edge.from] < rank
                            && groups[edge.from] == band
                    })
                    .map(|(_, edge)| {
                        let rect = boxes[edge.from].rect;
                        rect.y + rect.height / 2.0
                    })
                    .collect::<Vec<_>>();
                let desired_top = if parents.is_empty() {
                    boxes[*node].rect.y
                } else {
                    parents.iter().sum::<f64>() / parents.len() as f64
                        - boxes[*node].rect.height / 2.0
                };
                boxes[*node].rect.y = desired_top.max(next_y);
                next_y = boxes[*node].rect.y + boxes[*node].rect.height + NODE_GAP;
            }
        }
        top = boxes
            .iter()
            .zip(&groups)
            .filter(|(_, group)| **group == band)
            .map(|(node, _)| node.rect.y + node.rect.height)
            .fold(top, f64::max)
            + LR_BAND_GAP;
    }
    (top - LR_BAND_GAP + OUTER, groups)
}

pub(super) fn place_branches(
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

pub(super) fn build_boxes(
    graph: &Graph,
    fonts: &mut dyn FontMetrics,
    palette: &Palette,
    container_members: bool,
    rank_count: usize,
    visible_width: f64,
) -> Vec<BoxedNode> {
    let mut boxes = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let label = crate::text::label_parts(&node.label);
        let main = format!("{}{}", label.main, label.rest);
        let horizontal = matches!(
            graph.direction,
            Direction::LeftToRight | Direction::RightToLeft
        );
        let limit = if horizontal && container_members && graph.direction == Direction::LeftToRight
        {
            let columns = rank_count.max(1) as f64;
            (((visible_width - NODE_GAP * (columns - 1.0)) / columns) - 24.0).max(48.0)
        } else if horizontal {
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
                .unwrap_or(palette.shape_fill),
            stroke: style
                .stroke
                .as_deref()
                .and_then(Color::parse_css)
                .unwrap_or(palette.shape_stroke),
            text_color: style
                .color
                .as_deref()
                .and_then(Color::parse_css)
                .unwrap_or(palette.text_color),
            stroke_width: style.stroke_width.unwrap_or(1.4),
            stroke_dasharray: style.stroke_dasharray.unwrap_or_default(),
            label_lines,
        });
    }

    // 高扇出和汇合给每条边留独立端口，避免端点装饰互相覆盖。
    for (index, boxed) in boxes.iter_mut().enumerate() {
        let incoming = graph.edges.iter().filter(|edge| edge.to == index).count();
        let degree = graph
            .edges
            .iter()
            .filter(|edge| edge.from == index)
            .count()
            .max(incoming);
        if degree >= 4 && graph.nodes.len() <= 48 {
            boxed.rect.height = boxed.rect.height.max((degree - 1) as f64 * 12.0 + 20.0);
        }
        if container_members && graph.direction == Direction::LeftToRight && incoming >= 2 {
            boxed.rect.height = boxed.rect.height.max((incoming as f64 - 1.0) * 14.0 + 20.0);
            // 顶/底入口只能占用宽度的 22%；按入口数量反推最小宽度，
            // 使任意数量的同侧无标签入口在端口中点仍保持最小间距。
            let widest = (visible_width - CONTAINER_PAD * 2.0).max(64.0);
            boxed.rect.width = boxed
                .rect
                .width
                .max(crate::graph_router::min_port_side(incoming).min(widest));
        }
    }
    boxes
}
