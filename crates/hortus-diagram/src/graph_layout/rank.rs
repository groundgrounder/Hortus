//! 分层、层内排序与小图排列枚举。

use crate::Graph;
use std::collections::VecDeque;

pub(super) fn permutations_of(values: &mut [usize], start: usize, output: &mut Vec<Vec<usize>>) {
    if start == values.len() {
        output.push(values.to_vec());
        return;
    }
    for index in start..values.len() {
        values.swap(start, index);
        permutations_of(values, start + 1, output);
        values.swap(start, index);
    }
}

pub(super) fn assign_ranks(graph: &Graph) -> (Vec<usize>, usize, Vec<bool>) {
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

pub(super) fn order_ranks(graph: &Graph, ranks: &[usize], count: usize) -> Vec<Vec<usize>> {
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

/// 回到上游的终止反馈分支与判断节点同高，缩短回路。
pub(super) fn align_feedback_ranks(graph: &Graph, ranks: &mut [usize], accepted: &[bool]) {
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
}
