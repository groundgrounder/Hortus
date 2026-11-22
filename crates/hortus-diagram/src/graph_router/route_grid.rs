//! 网格路由主循环：网格构建、端点短段预约、候选路径与 A* 回退。

use super::*;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

pub(super) fn route_grid(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    policy: RoutePolicy<'_>,
    size: (f64, f64),
    label_sizes: LabelSizes<'_>,
    seeds: &[Option<Vec<Point>>],
    preferred: Option<(usize, usize)>,
) -> Result<Routes, LayoutError> {
    let (width, height) = size;
    let accepted = policy.accepted;
    let lr_bands = policy
        .lr_bands
        .filter(|_| graph.direction == Direction::LeftToRight);
    let vertical = matches!(
        graph.direction,
        Direction::TopToBottom | Direction::BottomToTop
    );
    let positive = matches!(
        graph.direction,
        Direction::TopToBottom | Direction::LeftToRight
    );
    let mut plan = plan_edges(
        graph, nodes, accepted, lr_bands, vertical, positive, preferred,
    );
    if policy.distinct_ports {
        for (index, edge) in graph.edges.iter().enumerate() {
            if !accepted[index]
                && edge.from != edge.to
                && graph
                    .edges
                    .iter()
                    .any(|other| other.from == edge.to && other.to == edge.from)
            {
                // 逆向转换分别从节点侧面出入，主方向通道留给正向转换。
                plan.sides[index] = (!vertical, false, !vertical, false);
            }
        }
    }
    let ends = compute_ends(
        graph,
        nodes,
        accepted,
        lr_bands,
        &plan,
        policy.distinct_ports,
    );
    let edge_sides = &plan.sides;
    let mut seeds = seeds.to_vec();
    fill_parallel_paths(
        graph, nodes, &ends, edge_sides, accepted, lr_bands, &mut seeds,
    );
    let rects = nodes.iter().map(|(rect, _)| *rect).collect::<Vec<_>>();
    let (mut label_rects, mut protected) = (vec![None; ends.len()], Vec::new());
    if policy.distinct_ports {
        state_loops(
            graph,
            nodes,
            size,
            label_sizes,
            &mut seeds,
            &mut label_rects,
            &mut protected,
        );
    }
    let mut grid = RouteGrid::new(nodes, width, height, &ends, &seeds)?;
    grid.pre_register_stubs(&ends, &seeds, graph);
    grid.pre_register_seeds(&seeds, graph);
    let mut result = seeds
        .iter()
        .map(|p| p.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    let mut routing_order = (0..ends.len())
        .filter(|i| seeds[*i].is_none())
        .collect::<Vec<_>>();
    routing_order.sort_by(|a, b| {
        label_sizes.has(*b).cmp(&label_sizes.has(*a)).then_with(|| {
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
        let start = grid.locate(a);
        let goal = grid.locate(b);
        let current = &graph.edges[edge_index];
        let candidates = candidate_paths(start, goal, grid.nx, grid.ny, nodes.len());
        let mut best: Option<(f64, Vec<usize>)> = None;
        for candidate in candidates {
            let Some(score) = candidate_score(
                &grid, current, edge_index, &protected, source, a, b, target, &candidate,
            ) else {
                continue;
            };
            let mut valid = true;
            if label_sizes.has(edge_index) {
                let mut points = vec![source];
                points.extend(candidate.iter().copied().map(|index| grid.point(index)));
                points.push(target);
                result[edge_index] = simplify(points);
                valid = label_sizes
                    .place(edge_index, &result, &rects, &protected, size)
                    .is_some();
                result[edge_index].clear();
            }
            if valid && best.as_ref().is_none_or(|(old, _)| score < *old) {
                best = Some((score, candidate));
            }
        }
        let indices = if let Some((_, indices)) = best.filter(|(score, _)| {
            // 小图遇到已有边的独占通道时扩大搜索，不能仅因少弯候选可达就接受交叉。
            *score < 4_000.0 || nodes.len() > 48 || label_sizes.has(edge_index)
        }) {
            indices
        } else {
            match grid.astar(
                start,
                goal,
                b,
                generation,
                edge_index,
                current,
                &protected,
                nodes.len() <= 48,
            ) {
                Some(indices) => indices,
                None => {
                    if fallback_elbow(
                        nodes,
                        &mut result,
                        &mut label_rects,
                        &mut protected,
                        edge_index,
                        current,
                        source,
                        a,
                        b,
                        target,
                        label_sizes,
                        &rects,
                        size,
                    )? {
                        continue;
                    }
                    return Err(LayoutError(format!(
                        "No orthogonal route for edge {} ({} -> {}, {:?} -> {:?})",
                        current.id, graph.nodes[current.from].id, graph.nodes[current.to].id, a, b
                    )));
                }
            }
        };
        grid.commit(&indices, edge_index, graph);
        let mut points = vec![source];
        points.extend(indices.into_iter().map(|index| grid.point(index)));
        points.push(target);
        result[edge_index] = simplify(points);
        if label_sizes.has(edge_index) {
            let rect = label_sizes
                .place(edge_index, &result, &rects, &protected, size)
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

/// 自环优先沿状态侧边闭合，并提前为其文字保留空间。
fn state_loops(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    size: (f64, f64),
    labels: LabelSizes<'_>,
    seeds: &mut [Option<Vec<Point>>],
    label_rects: &mut [Option<Rect>],
    protected: &mut Vec<Rect>,
) {
    let obstacles: Vec<_> = nodes.iter().map(|(rect, _)| *rect).collect();
    let mut paths = vec![Vec::new(); graph.edges.len()];
    for (index, edge) in graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, edge)| edge.from == edge.to)
    {
        let (rect, shape) = nodes[edge.from];
        if rect.height < 18.0 {
            continue;
        }
        for right in [true, false] {
            let source = crate::shape::port(shape, rect, false, rect.y + rect.height * 0.73, right);
            let target = crate::shape::port(shape, rect, false, rect.y + rect.height * 0.27, right);
            let x = if right {
                rect.x + rect.width + 27.0
            } else {
                rect.x - 27.0
            };
            if x < 8.0 || x > size.0 - 8.0 {
                continue;
            }
            let candidate = vec![
                source,
                Point { x, y: source.y },
                Point { x, y: target.y },
                target,
            ];
            let blocked = candidate.windows(2).any(|pair| {
                obstacles
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != edge.from)
                    .map(|(_, obstacle)| obstacle)
                    .chain(protected.iter())
                    .any(|obstacle| segment_intersects(pair[0], pair[1], *obstacle))
            });
            if blocked || paths.iter().any(|path| path == &candidate) {
                continue;
            }
            paths[index] = candidate;
            let side_label = if let LabelSizes::Measured(sizes) = labels {
                sizes[index].and_then(|(width, height)| {
                    let width = width + 4.0;
                    let rect = Rect {
                        x: if right { x + 8.0 } else { x - width - 8.0 },
                        y: rect.y + (rect.height - height) / 2.0,
                        width,
                        height,
                    };
                    (rect.x >= 4.0
                        && rect.x + rect.width <= size.0 - 4.0
                        && rect.y >= 4.0
                        && rect.y + rect.height <= size.1 - 4.0
                        && !obstacles
                            .iter()
                            .chain(protected.iter())
                            .any(|other| rect.intersects(*other))
                        && !paths.iter().any(|path| {
                            path.windows(2)
                                .any(|pair| segment_intersects(pair[0], pair[1], rect))
                        }))
                    .then_some(rect)
                })
            } else {
                None
            };
            let label =
                side_label.or_else(|| labels.place(index, &paths, &obstacles, protected, size));
            if labels.has(index) && label.is_none() {
                paths[index].clear();
                continue;
            }
            if let Some(label) = label {
                protected.push(label);
                label_rects[index] = Some(label);
            }
            seeds[index] = Some(paths[index].clone());
            break;
        }
    }
}

/// 同一对节点在 LR 容器内的重复边按 band 方向单独编排通道：端口顺序与
/// 通道顺序一致，路径彼此平行、不交叉，再由网格为其它边让路。
fn fill_parallel_paths(
    graph: &Graph,
    nodes: &[(Rect, Shape)],
    ends: &[Ends],
    edge_sides: &[EdgeSide],
    accepted: &[bool],
    lr_bands: Option<&[usize]>,
    seeds: &mut [Option<Vec<Point>>],
) {
    let mut placed = seeds.iter().flatten().cloned().collect::<Vec<_>>();
    let mut parallel_groups = BTreeMap::<(usize, usize), Vec<usize>>::new();
    for (index, edge) in graph.edges.iter().enumerate() {
        if accepted[index] && lr_bands.is_some() && edge.from != edge.to && edge.label.is_none() {
            parallel_groups
                .entry((edge.from, edge.to))
                .or_default()
                .push(index);
        }
    }
    for group in parallel_groups.values() {
        if group.len() < 2 {
            continue;
        }
        let (a_vertical, _, b_vertical, _) = edge_sides[group[0]];
        if group.iter().any(|index| {
            let (a, _, b, _) = edge_sides[*index];
            a != a_vertical || b != b_vertical
        }) {
            continue;
        }
        let count = group.len();
        let margin = 4.0;
        let vertical = a_vertical && b_vertical;
        let horizontal = !a_vertical && !b_vertical;
        if !vertical && !horizontal {
            continue;
        }
        let (first_source, _) = ends[group[0]].0;
        let (first_target, _) = ends[group[0]].1;
        let span = if vertical {
            first_target.y - first_source.y
        } else {
            first_target.x - first_source.x
        };
        if span - margin * 2.0 < (count - 1) as f64 * 2.0 {
            continue;
        }
        let mut paths = Vec::with_capacity(count);
        for (slot, index) in group.iter().enumerate() {
            let (source, _) = ends[*index].0;
            let (target, _) = ends[*index].1;
            let step = slot as f64 / (count - 1) as f64;
            let path = if vertical {
                let y = source.y + margin + step * (target.y - source.y - margin * 2.0);
                simplify(vec![
                    source,
                    Point { x: source.x, y },
                    Point { x: target.x, y },
                    target,
                ])
            } else {
                // 上方的入口先折向更靠近目标的一侧，通道次序与端口次序相反。
                let x = target.x - margin - step * (target.x - source.x - margin * 2.0);
                simplify(vec![
                    source,
                    Point { x, y: source.y },
                    Point { x, y: target.y },
                    target,
                ])
            };
            paths.push((*index, path));
        }
        let clear = paths.iter().all(|(_, path)| {
            path.windows(2).all(|pair| {
                nodes.iter().enumerate().all(|(node, (rect, _))| {
                    node == graph.edges[group[0]].from
                        || node == graph.edges[group[0]].to
                        || !segment_intersects(pair[0], pair[1], *rect)
                })
            }) && placed.iter().all(|other| !orthogonal_cross(path, other))
        }) && paths.iter().enumerate().all(|(index, (_, path))| {
            paths[index + 1..]
                .iter()
                .all(|(_, other)| !orthogonal_cross(path, other))
        });
        if !clear {
            continue;
        }
        placed.extend(paths.iter().map(|(_, path)| path.clone()));
        for (index, path) in paths {
            seeds[index] = Some(path);
        }
    }
}

fn candidate_paths(
    start: usize,
    goal: usize,
    nx: usize,
    ny: usize,
    node_count: usize,
) -> Vec<Vec<usize>> {
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
    if node_count <= 48 {
        // 局部带与独立回边的少弯组合，先于完整网格搜索。
        for x in &columns {
            for y in &rows {
                candidates.push(vec![start, sy * nx + x, y * nx + x, y * nx + gx, goal]);
                candidates.push(vec![start, y * nx + sx, y * nx + x, gy * nx + x, goal]);
            }
        }
    }
    candidates
}

#[allow(clippy::too_many_arguments)]
fn candidate_score(
    grid: &RouteGrid,
    current: &crate::Edge,
    edge_index: usize,
    protected: &[Rect],
    source: Point,
    a: Point,
    b: Point,
    target: Point,
    candidate: &[usize],
) -> Option<f64> {
    let mut score = 8.0 * candidate.len() as f64;
    for segment in candidate.windows(2) {
        score += grid.score_segment(current, edge_index, protected, segment[0], segment[1])?;
    }
    if candidate.len() <= 1 {
        return None;
    }
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
    let first_axis = axis(a, grid.point(candidate[1]));
    let last_axis = axis(
        grid.point(candidate[candidate.len() - 2]),
        grid.point(candidate[candidate.len() - 1]),
    );
    let target_axis = axis(b, target);
    if source_axis != first_axis {
        score += 8.0;
    }
    if last_axis != target_axis {
        score += 8.0;
    }
    Some(score)
}

#[allow(clippy::too_many_arguments)]
fn fallback_elbow(
    nodes: &[(Rect, Shape)],
    result: &mut [Vec<Point>],
    label_rects: &mut [Option<Rect>],
    protected: &mut Vec<Rect>,
    edge_index: usize,
    current: &crate::Edge,
    source: Point,
    a: Point,
    b: Point,
    target: Point,
    label_sizes: LabelSizes<'_>,
    rects: &[Rect],
    size: (f64, f64),
) -> Result<bool, LayoutError> {
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
    let Some(elbow) = elbows.iter().find(|elbow| {
        let points = [a, **elbow, b];
        clear(&points)
    }) else {
        return Ok(false);
    };
    result[edge_index] = simplify(vec![source, a, *elbow, b, target]);
    if label_sizes.has(edge_index) {
        let rect = label_sizes
            .place(edge_index, result, rects, protected, size)
            .ok_or_else(|| LayoutError(format!("No label channel for edge {}", current.id)))?;
        label_rects[edge_index] = Some(rect);
        protected.push(rect);
    }
    Ok(true)
}

struct RouteGrid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    nx: usize,
    ny: usize,
    horizontal: Vec<bool>,
    vertical_blocked: Vec<bool>,
    h_use: Vec<u16>,
    v_use: Vec<u16>,
    h_owner: Vec<ChannelOwner>,
    v_owner: Vec<ChannelOwner>,
    h_block_prefix: Vec<usize>,
    v_block_prefix: Vec<usize>,
    distances: Vec<u64>,
    generations: Vec<usize>,
    previous: Vec<usize>,
}

impl RouteGrid {
    fn new(
        nodes: &[(Rect, Shape)],
        width: f64,
        height: f64,
        ends: &[Ends],
        seeds: &[Option<Vec<Point>>],
    ) -> Result<Self, LayoutError> {
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
        for ((source, a), (target, b)) in ends {
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
                    if x + 1 < nx
                        && ys[y] > top
                        && ys[y] < bottom
                        && xs[x] < right
                        && xs[x + 1] > left
                    {
                        horizontal[y * nx + x] = true;
                    }
                    if y + 1 < ny
                        && xs[x] > left
                        && xs[x] < right
                        && ys[y] < bottom
                        && ys[y + 1] > top
                    {
                        vertical_blocked[y * nx + x] = true;
                    }
                }
            }
        }
        let h_use = vec![0u16; count];
        let v_use = vec![0u16; count];
        let h_owner = vec![ChannelOwner::EMPTY; count];
        let v_owner = vec![ChannelOwner::EMPTY; count];
        let mut h_block_prefix = vec![0usize; count];
        let mut v_block_prefix = vec![0usize; count];
        for y in 0..ny {
            for x in 0..nx {
                let i = y * nx + x;
                h_block_prefix[i] =
                    usize::from(horizontal[i]) + if x > 0 { h_block_prefix[i - 1] } else { 0 };
                v_block_prefix[i] = usize::from(vertical_blocked[i])
                    + if y > 0 { v_block_prefix[i - nx] } else { 0 };
            }
        }
        Ok(Self {
            distances: vec![u64::MAX; count * 3],
            generations: vec![0usize; count * 3],
            previous: vec![usize::MAX; count * 3],
            xs,
            ys,
            nx,
            ny,
            horizontal,
            vertical_blocked,
            h_use,
            v_use,
            h_owner,
            v_owner,
            h_block_prefix,
            v_block_prefix,
        })
    }

    fn locate(&self, point: Point) -> usize {
        let x = self
            .xs
            .partition_point(|x| *x < point.x - 0.001)
            .min(self.nx - 1);
        let y = self
            .ys
            .partition_point(|y| *y < point.y - 0.001)
            .min(self.ny - 1);
        y * self.nx + x
    }

    fn point(&self, index: usize) -> Point {
        Point {
            x: self.xs[index % self.nx],
            y: self.ys[index / self.nx],
        }
    }

    fn compatible(&self, owner: ChannelOwner, edge_index: usize, current: &crate::Edge) -> bool {
        owner.first == usize::MAX || owner.first == edge_index || {
            (owner.common_from == current.from || owner.common_to == current.to)
                && !(owner.common_from == current.from && owner.common_to == current.to)
                && !owner.labelled
                && current.label.is_none()
        }
    }

    fn pre_register_stubs(&mut self, ends: &[Ends], seeds: &[Option<Vec<Point>>], graph: &Graph) {
        // 端口到网格的短段也占用通道；提前登记，防止较早的边遮住后续边的端点身份。
        for (edge_index, ((source, a), (target, b))) in ends.iter().enumerate() {
            if seeds[edge_index].is_some() {
                continue;
            }
            for (p, q) in [(*source, *a), (*target, *b)] {
                let (from, to) = (self.locate(p), self.locate(q));
                let h = from / self.nx == to / self.nx;
                let mut i = from.min(to);
                while i < from.max(to) {
                    if h {
                        self.h_use[i] = self.h_use[i].saturating_add(1);
                        self.h_owner[i].occupy(edge_index, graph);
                    } else {
                        self.v_use[i] = self.v_use[i].saturating_add(1);
                        self.v_owner[i].occupy(edge_index, graph);
                    }
                    i += if h { 1 } else { self.nx };
                }
            }
        }
    }

    fn pre_register_seeds(&mut self, seeds: &[Option<Vec<Point>>], graph: &Graph) {
        for (edge_index, path) in seeds
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
        {
            for pair in path.windows(2) {
                let (from, to) = (self.locate(pair[0]), self.locate(pair[1]));
                let h = from / self.nx == to / self.nx;
                let mut i = from.min(to);
                while i < from.max(to) {
                    if h {
                        self.h_use[i] = self.h_use[i].saturating_add(1);
                        self.h_owner[i].occupy(edge_index, graph);
                    } else {
                        self.v_use[i] = self.v_use[i].saturating_add(1);
                        self.v_owner[i].occupy(edge_index, graph);
                    }
                    i += if h { 1 } else { self.nx };
                }
            }
        }
    }

    fn score_segment(
        &self,
        current: &crate::Edge,
        edge_index: usize,
        protected: &[Rect],
        a: usize,
        b: usize,
    ) -> Option<f64> {
        let horizontal_axis = a / self.nx == b / self.nx;
        let (lo, hi, step, blocked, usage, other_use, owners) = if horizontal_axis {
            (
                a.min(b),
                a.max(b),
                1,
                &self.h_block_prefix,
                &self.h_use,
                &self.v_use,
                &self.h_owner,
            )
        } else if a % self.nx == b % self.nx {
            (
                a.min(b),
                a.max(b),
                self.nx,
                &self.v_block_prefix,
                &self.v_use,
                &self.h_use,
                &self.v_owner,
            )
        } else {
            return None;
        };
        if lo == hi {
            return Some(0.0);
        }
        let before = if horizontal_axis {
            lo % self.nx > 0
        } else {
            lo >= self.nx
        };
        let blocked_count = blocked[hi - step] - if before { blocked[lo - step] } else { 0 };
        if blocked_count != 0 {
            return None;
        }
        let p = self.point(a);
        let q = self.point(b);
        if protected.iter().any(|rect| segment_intersects(p, q, *rect)) {
            return None;
        }
        let mut cost = (p.x - q.x).abs() + (p.y - q.y).abs();
        let mut i = lo;
        while i < hi {
            if usage[i] > 0 && !self.compatible(owners[i], edge_index, current) {
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
    }

    #[allow(clippy::too_many_arguments)]
    fn astar(
        &mut self,
        start: usize,
        goal: usize,
        target: Point,
        generation: usize,
        edge_index: usize,
        current: &crate::Edge,
        protected: &[Rect],
        small_graph: bool,
    ) -> Option<Vec<usize>> {
        let heuristic = |grid: &Self, index| {
            let p = grid.point(index);
            ((p.x - target.x).abs() + (p.y - target.y).abs()) * 1000.0
        };
        let mut heap = BinaryHeap::new();
        let first = start * 3;
        self.distances[first] = 0;
        self.generations[first] = generation;
        self.previous[first] = usize::MAX;
        heap.push(Reverse((heuristic(self, start) as u64, 0u64, first)));
        let mut last = None;
        while let Some(Reverse((_, cost, state))) = heap.pop() {
            if self.generations[state] != generation || self.distances[state] != cost {
                continue;
            }
            let index = state / 3;
            let axis = state % 3;
            if index == goal {
                last = Some(state);
                break;
            }
            let x = index % self.nx;
            let y = index / self.nx;
            for (valid, next, next_axis, segment) in [
                (x > 0, index.saturating_sub(1), 1, index.saturating_sub(1)),
                (x + 1 < self.nx, index + 1, 1, index),
                (
                    y > 0,
                    index.saturating_sub(self.nx),
                    2,
                    index.saturating_sub(self.nx),
                ),
                (y + 1 < self.ny, index + self.nx, 2, index),
            ] {
                if !valid
                    || if next_axis == 1 {
                        self.horizontal[segment]
                    } else {
                        self.vertical_blocked[segment]
                    }
                {
                    continue;
                }
                let p = self.point(index);
                let q = self.point(next);
                if protected.iter().any(|rect| segment_intersects(p, q, *rect)) {
                    continue;
                }
                let owner = if next_axis == 1 {
                    self.h_owner[segment]
                } else {
                    self.v_owner[segment]
                };
                let penalty = if self.compatible(owner, edge_index, current) {
                    0.0
                } else {
                    1_000_000.0
                };
                let crosses = if next_axis == 1 {
                    self.v_use[index] > 0 || (index >= self.nx && self.v_use[index - self.nx] > 0)
                } else {
                    self.h_use[index] > 0 || (index % self.nx > 0 && self.h_use[index - 1] > 0)
                };
                let crossing_penalty = if small_graph && index != start && index != goal && crosses
                {
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
                    + (((p.x - q.x).abs() + (p.y - q.y).abs() + bend + penalty + crossing_penalty)
                        * 1000.0) as u64;
                let next_state = next * 3 + next_axis;
                if self.generations[next_state] != generation
                    || next_cost < self.distances[next_state]
                {
                    self.generations[next_state] = generation;
                    self.distances[next_state] = next_cost;
                    self.previous[next_state] = state;
                    heap.push(Reverse((
                        next_cost + heuristic(self, next) as u64,
                        next_cost,
                        next_state,
                    )));
                }
            }
        }
        let mut state = last?;
        let mut indices = Vec::new();
        loop {
            indices.push(state / 3);
            if self.previous[state] == usize::MAX {
                break;
            }
            state = self.previous[state];
        }
        indices.reverse();
        Some(indices)
    }

    fn commit(&mut self, indices: &[usize], edge_index: usize, graph: &Graph) {
        for pair in indices.windows(2) {
            let (mut index, end) = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            let h = pair[0] / self.nx == pair[1] / self.nx;
            while index < end {
                if h {
                    self.h_use[index] = self.h_use[index].saturating_add(1);
                    self.h_owner[index].occupy(edge_index, graph);
                } else {
                    self.v_use[index] = self.v_use[index].saturating_add(1);
                    self.v_owner[index].occupy(edge_index, graph);
                }
                index += if h { 1 } else { self.nx };
            }
        }
    }
}
