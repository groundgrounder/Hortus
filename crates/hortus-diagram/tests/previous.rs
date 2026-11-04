//! 近树图在初排、同内容 previous、改宽/恢复和加边时的公开几何回归。
//!
//! 三个 seed × 五个场景的输入与 v4 二级探针相同；断言交叉、路径数、
//! 共同成员行带、分组和解析平移残余。设置 `HORTUS_PREVIOUS_EVIDENCE_DIR`
//! 时同时写出逐场景 Scene 和 summary.jsonl，供交付证据重算。
use hortus_diagram::{
    Diagram, Graph, GraphAnchor, GraphScene, LayoutOptions, Point, Primitive, Rect, Scene,
    SystemFonts, layout_graph, parse_diagram,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn synthetic(count: usize, mut seed: u64) -> String {
    let mut result = String::from("flowchart TB\n");
    for n in 0..count {
        result.push_str(&format!("N{n}[Node {n}]\n"));
    }
    for n in 1..count {
        result.push_str(&format!("N{} --> N{n}\n", (n - 1) / 3));
    }
    for _ in 0..count / 4 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = (seed >> 32) as usize % count;
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let b = (seed >> 32) as usize % count;
        result.push_str(&format!("N{a} --> N{b}\n"));
    }
    result
}

fn graph(source: &str) -> Graph {
    match parse_diagram(source).expect("synthetic probe must parse") {
        Diagram::Graph(graph) => graph,
        _ => panic!("synthetic probe did not produce Graph"),
    }
}

fn paths(scene: &Scene) -> Vec<(usize, Vec<Point>)> {
    scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::GraphPath {
                edge_id, points, ..
            } => Some((*edge_id, points.clone())),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
struct Geometry {
    crossings: Vec<(usize, usize, Point)>,
    crossing_paths: Vec<(Vec<Point>, Vec<Point>)>,
    edge_node: usize,
    nonorthogonal: usize,
    overflow: bool,
}

fn geometry(scene: &Scene) -> Geometry {
    let paths = paths(scene);
    let mut result = Geometry::default();
    for (_, path) in &paths {
        for (index, pair) in path.windows(2).enumerate() {
            if (pair[0].x - pair[1].x).abs() > 0.001 && (pair[0].y - pair[1].y).abs() > 0.001 {
                result.nonorthogonal += 1;
            }
            for hit in &scene.hit_regions {
                if (index == 0 && hit.rect.contains(pair[0]))
                    || (index == path.len() - 2 && hit.rect.contains(*path.last().unwrap()))
                {
                    continue;
                }
                if segment_hits(pair, hit.rect) {
                    result.edge_node += 1;
                }
            }
        }
        if path
            .iter()
            .any(|point| point.x < -0.01 || point.x > scene.width + 0.01)
        {
            result.overflow = true;
        }
    }
    for primitive in &scene.primitives {
        let outside = match primitive {
            Primitive::GraphShape { rect, .. } | Primitive::LabelBackground { rect, .. } => {
                rect.x < -0.01 || rect.x + rect.width > scene.width + 0.01
            }
            Primitive::Text(span) => {
                span.baseline.x < -0.01 || span.baseline.x + span.width > scene.width + 0.01
            }
            _ => false,
        };
        result.overflow |= outside;
    }
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            for a in paths[i].1.windows(2) {
                for b in paths[j].1.windows(2) {
                    let ah = (a[0].y - a[1].y).abs() < 0.001;
                    let bh = (b[0].y - b[1].y).abs() < 0.001;
                    if ah == bh {
                        continue;
                    }
                    let (horizontal, vertical) = if ah { (a, b) } else { (b, a) };
                    let x = vertical[0].x;
                    let y = horizontal[0].y;
                    if x > horizontal[0].x.min(horizontal[1].x) + 0.001
                        && x < horizontal[0].x.max(horizontal[1].x) - 0.001
                        && y > vertical[0].y.min(vertical[1].y) + 0.001
                        && y < vertical[0].y.max(vertical[1].y) - 0.001
                    {
                        result
                            .crossings
                            .push((paths[i].0, paths[j].0, Point { x, y }));
                        result
                            .crossing_paths
                            .push((paths[i].1.clone(), paths[j].1.clone()));
                    }
                }
            }
        }
    }
    result
}

fn segment_hits(pair: &[Point], rect: Rect) -> bool {
    if (pair[0].x - pair[1].x).abs() < 0.001 {
        pair[0].x > rect.x + 0.001
            && pair[0].x < rect.x + rect.width - 0.001
            && pair[0].y.min(pair[1].y) < rect.y + rect.height - 0.001
            && pair[0].y.max(pair[1].y) > rect.y + 0.001
    } else {
        pair[0].y > rect.y + 0.001
            && pair[0].y < rect.y + rect.height - 0.001
            && pair[0].x.min(pair[1].x) < rect.x + rect.width - 0.001
            && pair[0].x.max(pair[1].x) > rect.x + 0.001
    }
}

/// 与 graph-matrix 相同的行带定义：共同成员按垂直重叠 ≥ 较矮节点一半聚类。
fn row_band_changes(common: &[(&GraphAnchor, &GraphAnchor)]) -> Option<usize> {
    if common.len() <= 1 {
        return None;
    }
    let bands = |nodes: &[&GraphAnchor]| {
        let mut bands = vec![usize::MAX; nodes.len()];
        let mut next = 0;
        for start in 0..nodes.len() {
            if bands[start] != usize::MAX {
                continue;
            }
            bands[start] = next;
            let mut cursor = start;
            while cursor < nodes.len() {
                if bands[cursor] == next {
                    for other in start + 1..nodes.len() {
                        if bands[other] != usize::MAX {
                            continue;
                        }
                        let a = nodes[cursor].rect;
                        let b = nodes[other].rect;
                        let overlap = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
                        if overlap >= a.height.min(b.height) * 0.5 {
                            bands[other] = next;
                        }
                    }
                }
                cursor += 1;
            }
            next += 1;
        }
        bands
    };
    let old = common.iter().map(|(anchor, _)| *anchor).collect::<Vec<_>>();
    let new = common.iter().map(|(_, anchor)| *anchor).collect::<Vec<_>>();
    let old_bands = bands(&old);
    let new_bands = bands(&new);
    Some(
        (0..common.len())
            .filter(|index| {
                (0..common.len()).any(|other| {
                    index != &other
                        && (old_bands[*index] == old_bands[other])
                            != (new_bands[*index] == new_bands[other])
                })
            })
            .count(),
    )
}

struct Case {
    name: &'static str,
    scene: Scene,
    previous: hortus_diagram::PreviousGraph,
    crossings: usize,
    paths: usize,
    retained: usize,
    reason: String,
}

fn record(name: &'static str, result: &GraphScene) -> Case {
    let geometry = geometry(&result.scene);
    Case {
        name,
        scene: result.scene.clone(),
        previous: result.previous.clone(),
        crossings: geometry.crossings.len(),
        paths: paths(&result.scene).len(),
        retained: result.metrics.retained_nodes,
        reason: result.metrics.stability_reason.clone(),
    }
}

fn previous_at(path: &Path, seed: u64, case: &str, scene: &Scene) -> std::io::Result<()> {
    let _ = (path, seed, case);
    std::fs::write(
        path.join(format!("probe-{seed}-{case}.json")),
        serde_json::to_vec_pretty(scene)?,
    )
}

fn write_summary(path: &Path, rows: &[String]) -> std::io::Result<()> {
    std::fs::write(path.join("summary.jsonl"), rows.join("\n") + "\n")
}

fn displacement(common: &[(&GraphAnchor, &GraphAnchor)]) -> (f64, f64, f64, f64) {
    let deltas = common
        .iter()
        .map(|(before, after)| {
            (
                after.rect.x + after.rect.width / 2.0 - before.rect.x - before.rect.width / 2.0,
                after.rect.y + after.rect.height / 2.0 - before.rect.y - before.rect.height / 2.0,
            )
        })
        .collect::<Vec<_>>();
    let mut distances = deltas.iter().map(|(x, y)| x.hypot(*y)).collect::<Vec<_>>();
    distances.sort_by(f64::total_cmp);
    let anchor = common
        .iter()
        .position(|(before, _)| before.rect.y < 600.0)
        .map(|index| deltas[index]);
    let mut residual = deltas
        .iter()
        .map(|(x, y)| anchor.map_or(0.0, |(dx, dy)| (x - dx).hypot(y - dy)))
        .collect::<Vec<_>>();
    residual.sort_by(f64::total_cmp);
    let median = |values: &[f64]| {
        if values.len().is_multiple_of(2) {
            (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
        } else {
            values[values.len() / 2]
        }
    };
    (
        median(&distances),
        *distances.last().unwrap(),
        median(&residual),
        *residual.last().unwrap(),
    )
}

fn common<'a>(before: &'a Case, after: &'a Case) -> Vec<(&'a GraphAnchor, &'a GraphAnchor)> {
    before
        .previous
        .nodes
        .iter()
        .filter_map(|a| {
            after
                .previous
                .nodes
                .iter()
                .find(|b| a.node_id == b.node_id)
                .map(|b| (a, b))
        })
        .collect()
}

fn groups(case: &Case) -> BTreeMap<usize, usize> {
    let mut groups = BTreeMap::new();
    for node in &case.previous.nodes {
        *groups.entry(node.group).or_insert(0usize) += 1;
    }
    groups
}

fn assert_geometry(seed: u64, case: &Case, expected_paths: usize) {
    let geometry = geometry(&case.scene);
    let mut problems = Vec::new();
    if !geometry.crossings.is_empty() {
        problems.push(format!(
            "{} crossings: {:?}",
            geometry.crossings.len(),
            geometry
                .crossings
                .iter()
                .take(8)
                .map(|(a, b, point)| format!("{a}/{b}@({:.1},{:.1})", point.x, point.y))
                .collect::<Vec<_>>()
        ));
        for (first, second) in geometry.crossing_paths.iter().take(4) {
            problems.push(format!(
                "paths {:?} / {:?}",
                first
                    .iter()
                    .map(|p| format!("({:.1},{:.1})", p.x, p.y))
                    .collect::<Vec<_>>(),
                second
                    .iter()
                    .map(|p| format!("({:.1},{:.1})", p.x, p.y))
                    .collect::<Vec<_>>()
            ));
        }
    }
    if case.crossings != geometry.crossings.len() {
        problems.push(format!(
            "metric crossings {} != recomputed {}",
            case.crossings,
            geometry.crossings.len()
        ));
    }
    if case.paths != expected_paths {
        problems.push(format!("paths {} != {expected_paths}", case.paths));
    }
    if case.scene.metrics.node_overlaps != 0 {
        problems.push(format!(
            "node overlaps {}",
            case.scene.metrics.node_overlaps
        ));
    }
    if geometry.edge_node != 0 {
        problems.push(format!("edge/node hits {}", geometry.edge_node));
    }
    if geometry.nonorthogonal != 0 {
        problems.push(format!(
            "non-orthogonal segments {}",
            geometry.nonorthogonal
        ));
    }
    if case.scene.metrics.horizontal_overflow || geometry.overflow {
        problems.push("horizontal overflow".into());
    }
    assert!(
        problems.is_empty(),
        "seed {seed} {}: {}",
        case.name,
        problems.join("; ")
    );
}

fn assert_previous(seed: u64, before: &Case, after: &Case) {
    let common = common(before, after);
    assert_eq!(
        common.len(),
        before.previous.nodes.len(),
        "seed {seed} {}: common nodes {} != {}",
        after.name,
        common.len(),
        before.previous.nodes.len()
    );
    let group_changes = common.iter().filter(|(a, b)| a.group != b.group).count();
    let bands = row_band_changes(&common);
    let (median, max, residual_median, residual_max) = displacement(&common);
    assert_eq!(
        group_changes,
        0,
        "seed {seed} {}: group changes {group_changes}; groups {:?} -> {:?}",
        after.name,
        groups(before),
        groups(after)
    );
    assert_eq!(
        bands,
        Some(0),
        "seed {seed} {}: row band changes {bands:?}",
        after.name
    );
    if matches!(after.name, "resize-900-previous" | "restore-700-previous") {
        assert!(
            residual_median <= 8.0 && residual_max <= 48.0,
            "seed {seed} {}: translation residual median {residual_median:.3} max {residual_max:.3} (raw {median:.3}/{max:.3})",
            after.name
        );
    }
}

fn run_scenarios() {
    let Ok(mut fonts) = SystemFonts::new() else {
        eprintln!("系统字体不可用：跳过近树 previous 回归");
        return;
    };
    let evidence = std::env::var_os("HORTUS_PREVIOUS_EVIDENCE_DIR").map(PathBuf::from);
    if let Some(directory) = &evidence {
        std::fs::create_dir_all(directory).unwrap();
    }
    let mut rows = Vec::new();
    for seed in [17u64, 42, 2026] {
        let source = synthetic(20, seed);
        let initial = layout_graph(&graph(&source), &LayoutOptions::default(), &mut fonts)
            .expect("initial layout");
        let initial = record("initial", &initial);
        assert_geometry(seed, &initial, 24);
        assert_eq!(initial.crossings, 0, "seed {seed} initial crossings");
        let same = layout_graph(
            &graph(&source),
            &LayoutOptions {
                previous: Some(initial.previous.clone()),
                ..LayoutOptions::default()
            },
            &mut fonts,
        )
        .expect("same-previous layout");
        let same = record("same-previous", &same);
        assert_geometry(seed, &same, 24);
        assert_previous(seed, &initial, &same);
        // 同内容在 previous 下保持旧坐标与旧分组。
        let exact = common(&initial, &same)
            .iter()
            .all(|(a, b)| a.rect == b.rect && a.group == b.group);
        assert!(exact, "seed {seed} same-previous changed geometry");
        let resized = layout_graph(
            &graph(&source),
            &LayoutOptions {
                width: 900.0,
                previous: Some(initial.previous.clone()),
                ..LayoutOptions::default()
            },
            &mut fonts,
        )
        .expect("resized layout");
        let resized = record("resize-900-previous", &resized);
        assert_geometry(seed, &resized, 24);
        assert_previous(seed, &initial, &resized);
        let restored = layout_graph(
            &graph(&source),
            &LayoutOptions {
                width: 700.0,
                previous: Some(resized.previous.clone()),
                ..LayoutOptions::default()
            },
            &mut fonts,
        )
        .expect("restored layout");
        let restored = record("restore-700-previous", &restored);
        assert_geometry(seed, &restored, 24);
        assert_previous(seed, &resized, &restored);
        let edited = layout_graph(
            &graph(&format!("{source}N0 --> N19\n")),
            &LayoutOptions {
                previous: Some(initial.previous.clone()),
                ..LayoutOptions::default()
            },
            &mut fonts,
        )
        .expect("add-edge layout");
        let edited = record("add-edge-previous", &edited);
        assert_geometry(seed, &edited, 25);
        assert_previous(seed, &initial, &edited);
        for case in [&initial, &same, &resized, &restored, &edited] {
            let (displacement_median, displacement_max, residual_median, residual_max) =
                displacement(&common(&initial, case));
            let row_band = row_band_changes(&common(&initial, case));
            let group_changes = common(&initial, case)
                .iter()
                .filter(|(a, b)| a.group != b.group)
                .count();
            let row = json!({
                "seed": seed,
                "case": case.name,
                "crossings": case.crossings,
                "paths": case.paths,
                "scene_edges": case.previous.edges.len(),
                "height": case.scene.height,
                "node_overlaps": case.scene.metrics.node_overlaps,
                "horizontal_overflow": case.scene.metrics.horizontal_overflow,
                "groups": groups(case).values().collect::<Vec<_>>(),
                "group_changes": group_changes,
                "row_band_changes": row_band,
                "displacement_median_px": displacement_median,
                "displacement_max_px": displacement_max,
                "residual_median_px": residual_median,
                "residual_max_px": residual_max,
                "retained_nodes": case.retained,
                "stability_reason": case.reason,
            });
            rows.push(row.to_string());
            if let Some(directory) = &evidence {
                previous_at(directory, seed, case.name, &case.scene)
                    .expect("write previous probe scene");
            }
        }
    }
    if let Some(directory) = &evidence {
        write_summary(directory, &rows).expect("write previous probe summary");
    }
    for row in &rows {
        println!("{row}");
    }
}

#[test]
fn previous_scenarios_hold() {
    run_scenarios();
}
