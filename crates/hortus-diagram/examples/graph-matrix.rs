//! 产品场景的公开矩阵和可复核负载；输出目录由调用方明确指定。
use hortus_diagram::*;
use serde_json::{Value, json};
use std::{env, fs, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let output = Path::new(args.get(1).ok_or("missing output directory")?);
    let media = Path::new(args.get(2).ok_or("missing media directory")?);
    fs::create_dir_all(output)?;
    fs::create_dir_all(media)?;
    let mut fonts = SystemFonts::new()?;
    if args.iter().any(|arg| arg == "--probe-only") {
        for seed in [17, 42, 2026] {
            let source = synthetic(20, seed);
            let start = Instant::now();
            match render(&source, &LayoutOptions::default(), &mut fonts) {
                Ok((scene, _)) => {
                    fs::write(
                        output.join(format!("probe-{seed}.json")),
                        serde_json::to_vec_pretty(&scene)?,
                    )?;
                    println!(
                        "seed={seed} ms={} geometry={}",
                        start.elapsed().as_secs_f64() * 1000.0,
                        geometry(&scene)
                    );
                }
                Err(error) => println!("seed={seed} error={error}"),
            }
        }
        return Ok(());
    }
    let mut rows = Vec::new();
    let mut stability_rows = Vec::new();
    let mut window_rows = Vec::new();
    let mut files = fs::read_dir("fixtures/mermaid/flowchart")?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|s| s == "mmd"))
        .collect::<Vec<_>>();
    files.sort();
    if !args.iter().any(|arg| arg == "--bench-only") {
        for file in files {
            let source = fs::read_to_string(&file)?;
            let name = file.file_stem().unwrap().to_str().unwrap();
            if let Ok(Diagram::Graph(graph)) = parse_diagram(&source) {
                window_rows.extend(window_stability(name, &graph, &mut fonts));
                for width in [700.0, 1100.0] {
                    stability_rows.extend(stability(name, &graph, width, &mut fonts));
                }
            }
            for width in [700.0, 900.0, 1100.0] {
                for theme in [Theme::Light, Theme::Dark] {
                    let tag = format!("{name}-{}-{theme:?}", width as u32);
                    let options = LayoutOptions {
                        width,
                        theme,
                        ..LayoutOptions::default()
                    };
                    let result = render(&source, &options, &mut fonts);
                    match result {
                    Ok((scene,previous)) => {
                        let mut stable = scene.clone(); stable.metrics.layout_ms=0.0;
                        let baseline=serde_json::to_value(&stable)?;
                        let deterministic=(0..9).all(|_| render(&source,&options,&mut fonts).is_ok_and(|(mut scene,_)| {scene.metrics.layout_ms=0.0; serde_json::to_value(&scene).unwrap()==baseline}));
                        fs::write(media.join(format!("{tag}.svg")),scene.to_svg())?;
                        fs::write(output.join(format!("{tag}.json")),serde_json::to_vec_pretty(&scene)?)?;
                        rows.push(json!({"name":name,"width":width,"theme":format!("{theme:?}"),"height":scene.height,"nodes":scene.hit_regions.len(),"geometry":geometry(&scene),"deterministic_10":deterministic,"previous":previous}));
                    }
                    Err(error) => rows.push(json!({"name":name,"width":width,"theme":format!("{theme:?}"),"error":error})),
                }
                }
            }
        }
        fs::write(
            output.join("matrix.json"),
            serde_json::to_vec_pretty(&rows)?,
        )?;
        fs::write(
            output.join("stability.json"),
            serde_json::to_vec_pretty(&stability_rows)?,
        )?;
        fs::write(
            output.join("window-stability.json"),
            serde_json::to_vec_pretty(&window_rows)?,
        )?;
    }
    if args.iter().any(|arg| arg == "--matrix-only") {
        return Ok(());
    }
    let mut bench = Vec::new();
    for count in [20, 50, 100, 200] {
        for seed in [17u64, 42, 2026] {
            let source = synthetic(count, seed);
            let mut rounds = Vec::new();
            let mut failures = Vec::new();
            for _ in 0..3 {
                let mut samples = Vec::new();
                for _ in 0..31 {
                    let start = Instant::now();
                    if let Err(error) = render(&source, &LayoutOptions::default(), &mut fonts) {
                        failures.push(error);
                    }
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                rounds.push(samples);
            }
            let mut sorted = rounds.iter().flatten().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            let row = json!({"nodes":count,"seed":seed,"rounds_ms":rounds,"median_ms":sorted[sorted.len()/2],"p95_ms":sorted[(sorted.len()-1)*95/100],"max_ms":sorted.last(),"errors":failures});
            println!(
                "nodes={count} seed={seed} median={:.3} p95={:.3}",
                row["median_ms"].as_f64().unwrap(),
                row["p95_ms"].as_f64().unwrap()
            );
            bench.push(row);
            fs::write(output.join(format!("load-{count}-{seed}.mmd")), source)?;
        }
    }
    fs::write(
        output.join("benchmark.json"),
        serde_json::to_vec_pretty(&bench)?,
    )?;
    Ok(())
}

fn row_band_changes(common: &[(&GraphAnchor, &GraphAnchor)]) -> usize {
    let same_row = |a: &GraphAnchor, b: &GraphAnchor| {
        (a.rect.y + a.rect.height / 2.0 - b.rect.y - b.rect.height / 2.0).abs() < 1.0
    };
    common
        .iter()
        .enumerate()
        .filter(|(i, (a, b))| {
            common
                .iter()
                .enumerate()
                .any(|(j, (c, d))| i != &j && same_row(a, c) != same_row(b, d))
        })
        .count()
}

fn window_stability(name: &str, graph: &Graph, fonts: &mut dyn FontMetrics) -> Vec<Value> {
    let mut previous: Option<PreviousGraph> = None;
    let mut rows = Vec::new();
    for width in [700.0, 900.0, 1100.0, 900.0, 700.0] {
        let viewport = previous.as_ref().map(|old| GraphViewport {
            visible: old
                .nodes
                .iter()
                .filter(|n| n.rect.y < 600.0)
                .map(|n| ViewportAnchor {
                    node_id: n.node_id.clone(),
                    screen_center: Point {
                        x: n.rect.x + n.rect.width / 2.0,
                        y: n.rect.y + n.rect.height / 2.0,
                    },
                })
                .collect(),
        });
        let after = match layout_graph(
            graph,
            &LayoutOptions {
                width,
                previous: previous.clone(),
                viewport,
                ..Default::default()
            },
            fonts,
        ) {
            Ok(after) => after,
            Err(error) => {
                rows.push(json!({"name": name, "width": width, "error": error.to_string()}));
                continue;
            }
        };
        if let Some(before) = &previous {
            let common = before
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
                .collect::<Vec<_>>();
            let deltas = common
                .iter()
                .map(|(a, b)| {
                    (
                        b.rect.x + b.rect.width / 2.0 - a.rect.x - a.rect.width / 2.0,
                        b.rect.y + b.rect.height / 2.0 - a.rect.y - a.rect.height / 2.0,
                    )
                })
                .collect::<Vec<_>>();
            let mut distances = deltas.iter().map(|(x, y)| x.hypot(*y)).collect::<Vec<_>>();
            distances.sort_by(f64::total_cmp);
            let anchor = common
                .iter()
                .position(|(a, _)| a.rect.y < 600.0)
                .map(|i| deltas[i]);
            let mut anchored = deltas
                .iter()
                .map(|(x, y)| anchor.map_or(0.0, |(dx, dy)| (x - dx).hypot(y - dy)))
                .collect::<Vec<_>>();
            anchored.sort_by(f64::total_cmp);
            let groups = common.iter().filter(|(a, b)| a.group != b.group).count();
            rows.push(json!({"name":name,"from_width":before.width,"width":width,"nodes":common.len(),"median_displacement_px":distances.get(distances.len()/2),"max_displacement_px":distances.last(),"analytical_translation_residual_median_px":anchored.get(anchored.len()/2),"analytical_translation_residual_max_px":anchored.last(),"group_changes":groups,"row_band_changes":row_band_changes(&common),"reason":after.metrics.stability_reason,"geometry":geometry(&after.scene)}));
        }
        previous = Some(after.previous);
    }
    rows
}
fn render(
    source: &str,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<(Scene, Option<PreviousGraph>), String> {
    match parse_diagram(source).map_err(|e| e.to_string())? {
        Diagram::Tree(tree) => layout(&tree, options, fonts).map(|scene| (scene, None)),
        Diagram::Graph(graph) => {
            layout_graph(&graph, options, fonts).map(|result| (result.scene, Some(result.previous)))
        }
    }
    .map_err(|e| e.to_string())
}
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
fn geometry(scene: &Scene) -> Value {
    let paths = scene
        .primitives
        .iter()
        .filter_map(|p| {
            if let Primitive::GraphPath {
                edge_id, points, ..
            } = p
            {
                Some((*edge_id, points))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let labels = scene
        .primitives
        .iter()
        .filter_map(|p| {
            if let Primitive::LabelBackground { edge_id, rect, .. } = p {
                Some((*edge_id, *rect))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut piercing = 0;
    let mut nonorthogonal = 0;
    let mut crossings = 0;
    let mut crossing_details = Vec::new();
    let mut shared = 0;
    let mut foreign_shared = 0;
    for (_, path) in &paths {
        for (i, pair) in path.windows(2).enumerate() {
            if (pair[0].x - pair[1].x).abs() > 0.001 && (pair[0].y - pair[1].y).abs() > 0.001 {
                nonorthogonal += 1;
            }
            for hit in &scene.hit_regions {
                if (i == 0 && hit.rect.contains(path[0]))
                    || (i == path.len() - 2 && hit.rect.contains(*path.last().unwrap()))
                {
                    continue;
                }
                if intersects(pair, hit.rect) {
                    piercing += 1;
                }
            }
        }
    }
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            for a in paths[i].1.windows(2) {
                for b in paths[j].1.windows(2) {
                    let ah = (a[0].y - a[1].y).abs() < 0.001;
                    let bh = (b[0].y - b[1].y).abs() < 0.001;
                    if ah != bh {
                        let (h, v) = if ah { (a, b) } else { (b, a) };
                        if v[0].x > h[0].x.min(h[1].x) + 0.001
                            && v[0].x < h[0].x.max(h[1].x) - 0.001
                            && h[0].y > v[0].y.min(v[1].y) + 0.001
                            && h[0].y < v[0].y.max(v[1].y) - 0.001
                        {
                            crossings += 1;
                            crossing_details.push(json!({"edge_a": paths[i].0, "edge_b": paths[j].0,
                                "point": {"x": v[0].x, "y": h[0].y}, "path_a": paths[i].1, "path_b": paths[j].1}));
                        }
                    } else if (ah
                        && (a[0].y - b[0].y).abs() < 0.001
                        && a[0].x.min(a[1].x) < b[0].x.max(b[1].x) - 0.001
                        && b[0].x.min(b[1].x) < a[0].x.max(a[1].x) - 0.001)
                        || (!ah
                            && (a[0].x - b[0].x).abs() < 0.001
                            && a[0].y.min(a[1].y) < b[0].y.max(b[1].y) - 0.001
                            && b[0].y.min(b[1].y) < a[0].y.max(a[1].y) - 0.001)
                    {
                        shared += 1;
                        let endpoint = |path: &[Point], first: bool| {
                            let p = if first {
                                path[0]
                            } else {
                                *path.last().unwrap()
                            };
                            scene.hit_regions.iter().position(|hit| {
                                let r = hit.rect;
                                p.x >= r.x - 0.01
                                    && p.x <= r.x + r.width + 0.01
                                    && p.y >= r.y - 0.01
                                    && p.y <= r.y + r.height + 0.01
                            })
                        };
                        let same_source = endpoint(paths[i].1, true).is_some()
                            && endpoint(paths[i].1, true) == endpoint(paths[j].1, true);
                        let same_target = endpoint(paths[i].1, false).is_some()
                            && endpoint(paths[i].1, false) == endpoint(paths[j].1, false);
                        let labelled = labels
                            .iter()
                            .any(|(id, _)| *id == paths[i].0 || *id == paths[j].0);
                        if labelled || !(same_source || same_target) {
                            foreign_shared += 1;
                        }
                    }
                }
            }
        }
    }
    json!({"node_overlaps":scene.metrics.node_overlaps,"edge_node":piercing,"nonorthogonal":nonorthogonal,"crossings":crossings,"crossing_details":crossing_details,"shared_segments":shared,"foreign_or_labelled_shared_segments":foreign_shared,"label_node":labels.iter().map(|(_,r)|scene.hit_regions.iter().filter(|h|h.rect.intersects(*r)).count()).sum::<usize>(),"label_edge":labels.iter().map(|(id,r)|paths.iter().filter(|(other,p)|other!=id && p.windows(2).any(|s|intersects(s,*r))).count()).sum::<usize>(),"label_label":labels.iter().enumerate().map(|(i,(_,a))|labels[i+1..].iter().filter(|(_,b)|a.intersects(*b)).count()).sum::<usize>(),"horizontal_overflow":scene.metrics.horizontal_overflow})
}
fn intersects(p: &[Point], r: Rect) -> bool {
    if (p[0].x - p[1].x).abs() < 0.001 {
        p[0].x > r.x + 0.001
            && p[0].x < r.x + r.width - 0.001
            && p[0].y.min(p[1].y) < r.y + r.height - 0.001
            && p[0].y.max(p[1].y) > r.y + 0.001
    } else {
        p[0].y > r.y + 0.001
            && p[0].y < r.y + r.height - 0.001
            && p[0].x.min(p[1].x) < r.x + r.width - 0.001
            && p[0].x.max(p[1].x) > r.x + 0.001
    }
}

fn stability(name: &str, graph: &Graph, width: f64, fonts: &mut dyn FontMetrics) -> Vec<Value> {
    let options = LayoutOptions {
        width,
        ..LayoutOptions::default()
    };
    let Ok(before) = layout_graph(graph, &options, fonts) else {
        return vec![];
    };
    let viewport = GraphViewport {
        visible: before
            .previous
            .nodes
            .iter()
            .filter(|n| n.rect.y < 600.0)
            .map(|n| ViewportAnchor {
                node_id: n.node_id.clone(),
                screen_center: Point {
                    x: n.rect.x + n.rect.width / 2.0,
                    y: n.rect.y + n.rect.height / 2.0,
                },
            })
            .collect(),
    };
    let options = LayoutOptions {
        previous: Some(before.previous.clone()),
        viewport: Some(viewport),
        ..options
    };
    let mut edits = Vec::new();
    // 以实际排版框宽选择 30–50% 增长，排除已经达到换行上限的节点。
    let mut widened = None;
    'outer: for (index, old) in before.previous.nodes.iter().enumerate() {
        for count in 2..24 {
            let mut edited = graph.clone();
            edited.nodes[index].label =
                format!("{}{}", graph.nodes[index].label, "W".repeat(count));
            if let Ok(measured) = layout_graph(
                &edited,
                &LayoutOptions {
                    width,
                    ..LayoutOptions::default()
                },
                fonts,
            ) {
                let new = &measured.previous.nodes[index];
                let ratio = new.rect.width / old.rect.width;
                if (1.3..=1.5).contains(&ratio) {
                    widened = Some((edited, old.node_id.clone(), ratio));
                    break 'outer;
                }
            }
        }
    }
    if let Some((edited, id, ratio)) = widened {
        edits.push(("width", edited, Some(id), Some(ratio)));
    }
    let mut added = graph.clone();
    let id = "P131_added_leaf".to_string();
    let from = graph.nodes.len().saturating_sub(1);
    added.nodes.push(GraphNode {
        id: id.clone(),
        label: "新增叶".into(),
        id_range: 0..0,
        label_range: 0..0,
        source_range: 0..0,
        shape: Shape::Rectangle,
        class: None,
    });
    added.edges.push(Edge {
        connector_range: 0..0,
        id: 0,
        from,
        to: graph.nodes.len(),
        connector: Connector {
            line: EdgeLine::Solid,
            start: Marker::None,
            end: Marker::Arrow,
        },
        label: None,
        label_range: None,
        source_range: 0..0,
    });
    edits.push(("add_leaf", added, Some(id), None));
    let mut seen = vec![false; graph.nodes.len()];
    let redundant = graph.edges.iter().enumerate().find_map(|(index, edge)| {
        let extra = seen[edge.to] || edge.from == edge.to;
        seen[edge.to] = true;
        extra.then_some(index)
    });
    if let Some(index) = redundant {
        let mut edited = graph.clone();
        edited.edges.remove(index);
        edits.push(("delete_non_tree_edge", edited, None, None));
    }
    let mut rows = Vec::new();
    if redundant.is_none() {
        rows.push(json!({"name":name,"width":width,"edit":"delete_non_tree_edge","result":null}));
    }
    for (kind, edited, changed, ratio) in edits {
        let after = match layout_graph(&edited, &options, fonts) {
            Ok(scene) => scene,
            Err(e) => {
                rows.push(json!({"name":name,"width":width,"edit":kind,"error":e.to_string()}));
                continue;
            }
        };
        let common = before
            .previous
            .nodes
            .iter()
            .filter(|a| changed.as_deref() != Some(&a.node_id))
            .filter_map(|a| {
                after
                    .previous
                    .nodes
                    .iter()
                    .find(|b| a.node_id == b.node_id)
                    .map(|b| (a, b))
            })
            .collect::<Vec<_>>();
        let mut distances = common
            .iter()
            .map(|(a, b)| {
                ((b.rect.x + b.rect.width / 2.0 - a.rect.x - a.rect.width / 2.0).powi(2)
                    + (b.rect.y + b.rect.height / 2.0 - a.rect.y - a.rect.height / 2.0).powi(2))
                .sqrt()
            })
            .collect::<Vec<_>>();
        distances.sort_by(f64::total_cmp);
        let group_changes = common.iter().filter(|(a, b)| a.group != b.group).count();
        let mut inversions = 0;
        for (i, (a, b)) in common.iter().enumerate() {
            for (c, d) in &common[i + 1..] {
                if a.rank == c.rank && (a.rect.x - c.rect.x) * (b.rect.x - d.rect.x) < -0.01 {
                    inversions += 1;
                }
            }
        }
        let mut residual = Vec::new();
        for (a, b) in &common {
            let members = common
                .iter()
                .filter(|(c, _)| c.group == a.group)
                .collect::<Vec<_>>();
            if members.len() > 1 {
                let dx = members
                    .iter()
                    .map(|(c, d)| d.rect.x + d.rect.width / 2.0 - c.rect.x - c.rect.width / 2.0)
                    .sum::<f64>()
                    / members.len() as f64;
                let dy = members
                    .iter()
                    .map(|(c, d)| d.rect.y + d.rect.height / 2.0 - c.rect.y - c.rect.height / 2.0)
                    .sum::<f64>()
                    / members.len() as f64;
                residual.push(
                    ((b.rect.x + b.rect.width / 2.0 - a.rect.x - a.rect.width / 2.0 - dx).powi(2)
                        + (b.rect.y + b.rect.height / 2.0 - a.rect.y - a.rect.height / 2.0 - dy)
                            .powi(2))
                    .sqrt(),
                );
            }
        }
        rows.push(json!({"name":name,"width":width,"edit":kind,"actual_width_ratio":ratio,"unchanged_nodes":common.len(),"median_displacement_px":distances.get(distances.len()/2),"max_displacement_px":distances.last(),"group_changes":group_changes,"within_rank_inversions":inversions,"row_band_changes":row_band_changes(&common),"anchor_residual_max_px":residual.into_iter().max_by(f64::total_cmp),"reason":after.metrics.stability_reason,"geometry":geometry(&after.scene)}));
    }
    rows
}
