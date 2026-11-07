use hortus_diagram::*;
use std::path::PathBuf;

fn fake(source: &str, width: f64, collapsed: &[&str]) -> Scene {
    let scene = layout(
        &parse(source).unwrap(),
        &LayoutOptions {
            width,
            collapsed: collapsed.iter().map(|s| s.to_string()).collect(),
            ..LayoutOptions::default()
        },
        &mut DeterministicMetrics,
    )
    .unwrap();
    assert_text_within_nodes(&scene);
    scene
}

fn assert_text_within_nodes(scene: &Scene) {
    for primitive in &scene.primitives {
        if let Primitive::Text(text) = primitive {
            assert!(
                scene.hit_regions.iter().any(|hit| {
                    hit.rect.contains(text.baseline)
                        && text.baseline.x + text.width <= hit.rect.x + hit.rect.width + 0.001
                }),
                "文字超出节点：x={:.3}, y={:.3}, width={:.3}",
                text.baseline.x,
                text.baseline.y,
                text.width
            );
        }
    }
}

fn graph_hit<'a>(scene: &'a GraphScene, id: &str) -> &'a HitRegion {
    scene
        .scene
        .hit_regions
        .iter()
        .find(|hit| hit.node_id == id)
        .unwrap_or_else(|| panic!("missing hit {id}"))
}

fn graph_path(scene: &GraphScene, edge_id: usize) -> &Vec<Point> {
    scene
        .scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::GraphPath {
                edge_id: id,
                points,
                ..
            } if *id == edge_id => Some(points),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing edge path {edge_id}"))
}

#[test]
fn flow_shapes_chains_comments_and_utf8_ranges() {
    let source = "%% ignored\nflowchart TB; A[\"中文 --> ; %% 标签\"]:::key --> B(rounded) --> C((circle)); A --> D([stadium]); A --> E{{hexagon}}; A --> F[[subroutine]]; A --> G[(database)]; A --> H{decision}; A --> I>flag]; classDef key fill:#abc,stroke:#c33; %% trailing";
    let tree = parse_at(source, 17).unwrap();
    assert_eq!(tree.nodes.len(), 9);
    assert_eq!(tree.nodes[tree.root].children.len(), 7);
    assert_eq!(tree.nodes[0].label, "中文 --> ; %% 标签");
    assert_eq!(tree.root_style().unwrap().stroke.as_deref(), Some("#c33"));
    for node in &tree.nodes {
        assert!(source.is_char_boundary(node.source_range.start - 17));
        assert!(source.is_char_boundary(node.source_range.end - 17));
        assert!(
            source[node.source_range.start - 17..node.source_range.end - 17].starts_with(&node.id)
        );
    }
    let tree = parse("graph LR\nA --> B\nB[\"later definition\"]\nA --> B").unwrap();
    assert_eq!(tree.nodes.len(), 2);
    assert_eq!(tree.nodes[1].label, "later definition");
    assert_eq!(tree.nodes[0].children, vec![1]);
    assert_eq!(
        &"graph LR\nA --> B\nB[\"later definition\"]\nA --> B"[tree.nodes[1].source_range.clone()],
        "B[\"later definition\"]"
    );
}

#[test]
fn rejects_merges_cycles_forests_and_unsupported_types() {
    let error = parse("flowchart TB\nA --> B --> D\nA --> C --> D").unwrap_err();
    assert!(
        matches!(error, DiagramError::NotATree { multi_parent, .. } if multi_parent == vec!["D"])
    );
    for source in [
        "flowchart TB\nA --> A",
        "graph LR\nA --> B --> A",
        "flowchart TB\nA --> B\nC --> D --> C",
        "flowchart TB\nA\nB",
    ] {
        assert!(matches!(parse(source), Err(DiagramError::NotATree { .. })));
    }
    for kind in ["sequenceDiagram", "classDiagram", "stateDiagram-v2"] {
        assert!(matches!(parse(kind), Err(DiagramError::Unsupported { .. })));
    }
    for source in [
        "flowchart TB\nA[unclosed",
        "flowchart TB\nA -->",
        "flowchart TB\nA junk",
        "flowchart TB\nA <--> B",
        "flowchart TB\nA --o B",
        "flowchart TB\nA --x B",
    ] {
        assert!(matches!(parse(source), Err(DiagramError::Syntax { .. })));
    }
}

#[test]
fn general_diagram_parser_preserves_graph_structure_and_source_ranges() {
    let source =
        "flowchart LR\nA[\"开始 😀\"] -->|同意| C{决定}\nB(备用) --> C\nC -.-> A\nC --> C\n";
    let diagram = parse_diagram_at(source, 23).unwrap();
    let Diagram::Graph(graph) = diagram else {
        panic!("merge, cycle, and self-loop must use the graph AST");
    };
    assert_eq!(graph.direction, Direction::LeftToRight);
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.nodes[0].label, "开始 😀");
    assert_eq!(graph.nodes[0].shape, Shape::Rectangle);
    assert_eq!(graph.nodes[1].shape, Shape::Diamond);
    assert_eq!(graph.nodes[2].shape, Shape::Rounded);
    assert_eq!(graph.edges.len(), 4);
    assert_eq!(graph.edges[0].label.as_deref(), Some("同意"));
    assert_eq!(
        graph.edges[0].label_range.as_ref().unwrap().start,
        23 + source.find("同意").unwrap()
    );
    assert_eq!(graph.edges[2].connector.line, EdgeLine::Dotted);
    assert_eq!(graph.edges[3].to, graph.edges[3].from);
    for node in &graph.nodes {
        assert!(source.is_char_boundary(node.id_range.start - 23));
        assert!(source.is_char_boundary(node.id_range.end - 23));
        assert!(source.is_char_boundary(node.label_range.start - 23));
        assert!(source.is_char_boundary(node.label_range.end - 23));
    }
}

#[test]
fn graph_markdown_crlf_ranges_and_unclosed_fences() {
    let body = "flowchart LR\r\nA[中文😀 é] -->|是| C{决定}\r\nB(备用) --> C\r\nC -.-> A\r\n";
    for prefix in ["", "> ", "  "] {
        let source = format!(
            "前言😀\r\n\r\n{prefix}```mermaid\r\n{}{prefix}```\r\n",
            body.split_inclusive('\n')
                .map(|line| format!("{prefix}{line}"))
                .collect::<String>()
        );
        let block = mermaid_block(&source, 1).unwrap();
        let Diagram::Graph(graph) = block.parse_diagram().unwrap() else {
            panic!("expected graph")
        };
        assert_eq!(&source[graph.nodes[0].source_range.clone()], "A[中文😀 é]");
        assert_eq!(
            &source[graph.edges[0].source_range.clone()],
            "A[中文😀 é] -->|是| C{决定}"
        );
        for node in &graph.nodes {
            assert!(source.is_char_boundary(node.source_range.start));
            assert!(source.is_char_boundary(node.source_range.end));
        }
    }
    let block = mermaid_block("```mermaid\nflowchart TB\nA --> B\n", 1).unwrap();
    assert!(
        matches!(block.parse_diagram(), Err(DiagramError::Syntax { kind, .. }) if kind == "fence_unclosed")
    );
}

#[test]
fn general_diagram_parser_keeps_edge_identity_and_forest_relations() {
    let graph = match parse_diagram("flowchart TB\nA --> C\nB --> C\nA --> C\n") {
        Ok(Diagram::Graph(graph)) => graph,
        other => panic!("expected graph with a merge and duplicate edge: {other:?}"),
    };
    assert_eq!(graph.edges.len(), 3);
    assert_ne!(graph.edges[0].id, graph.edges[2].id);
    let Diagram::Graph(edited) =
        parse_diagram("flowchart TB\nX --> Y\nA --> C\nB --> C\nA --> C\n").unwrap()
    else {
        panic!("expected graph")
    };
    assert_eq!(graph.edges[0].id, edited.edges[1].id);
    assert_eq!(graph.edges[2].id, edited.edges[3].id);
    assert!(!graph.forest);

    let graph = match parse_diagram("flowchart TB\nA --> B\nC -.-> B\n") {
        Ok(Diagram::Graph(graph)) => graph,
        other => panic!("expected a forest with a relation edge: {other:?}"),
    };
    assert_eq!(graph.edges.len(), 2);
    assert!(graph.forest);
}

#[test]
fn general_diagram_parser_supports_bidirectional_connectors_and_common_shapes() {
    let source =
        "flowchart TB\nA --> B\nB <--> C\nC ---o D\nD --x E\nE ==> F\nF[/slant/]\nG[\\reverse\\]";
    let graph = match parse_diagram(source) {
        Ok(Diagram::Graph(graph)) => graph,
        other => panic!("expected graph: {other:?}"),
    };
    assert_eq!(graph.edges.len(), 5);
    assert_eq!(graph.edges[1].connector.start, Marker::Arrow);
    assert_eq!(graph.edges[1].connector.end, Marker::Arrow);
    assert_eq!(graph.edges[2].connector.end, Marker::Circle);
    assert_eq!(graph.edges[3].connector.end, Marker::Cross);
    assert_eq!(graph.edges[4].connector.line, EdgeLine::Thick);
    assert_eq!(graph.nodes[5].shape, Shape::Parallelogram);
    assert_eq!(graph.nodes[6].shape, Shape::ParallelogramReverse);
}

#[test]
fn graph_layout_returns_native_paths_nodes_and_stable_svg() {
    let graph =
        match parse_diagram("flowchart TB\nA -->|yes| C{Decision}\nB --> C\nC -.-> A\nC --> C") {
            Ok(Diagram::Graph(graph)) => graph,
            other => panic!("expected graph: {other:?}"),
        };
    let render =
        || layout_graph(&graph, &LayoutOptions::default(), &mut DeterministicMetrics).unwrap();
    let first = render();
    let second = render();
    assert_eq!(first.scene.to_svg(), second.scene.to_svg());
    assert_eq!(first.metrics.edge_count, 4);
    assert_eq!(first.metrics.ranks, 2);
    assert_eq!(first.scene.hit_regions.len(), 3);
    assert!(
        first
            .scene
            .primitives
            .iter()
            .any(|primitive| matches!(primitive, Primitive::GraphPath { .. }))
    );
    assert!(first.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::GraphShape {
            shape: Shape::Diamond,
            ..
        }
    )));
    assert!(
        first
            .scene
            .primitives
            .iter()
            .any(|primitive| matches!(primitive, Primitive::LabelBackground { .. }))
    );
    assert!(
        first
            .scene
            .to_svg()
            .contains(&format!("data-edge-id=\"{}\"", graph.edges[0].id))
    );
    assert!(!first.metrics.horizontal_overflow);
    assert_eq!(first.metrics.node_overlaps, 0);
}

#[test]
fn flowchart_connection_variants_and_ampersand_expansion() {
    assert!(matches!(parse("flowchart TB\nB & C --> D"),
        Err(DiagramError::NotATree { multi_parent, .. }) if multi_parent == vec!["D"]));
    for connector in ["-->", "---", "-.->", "-.-", "==>", "===", "--->", "====>"] {
        let tree = parse(&format!("flowchart TB\nA {connector} B")).unwrap();
        assert_eq!(tree.nodes.len(), 2, "{connector}");
        assert_eq!(tree.nodes[0].children, vec![1], "{connector}");
    }

    let tree = parse("flowchart TB\nA --> B & C\nB --> D & E").unwrap();
    assert_eq!(tree.nodes[0].children, vec![1, 2]);
    assert_eq!(tree.nodes[1].children, vec![3, 4]);
    assert_eq!(tree.nodes.len(), 5);

    assert!(matches!(
        parse("flowchart TB\nA --> B & C\nC --> B"),
        Err(DiagramError::NotATree { multi_parent, .. }) if multi_parent == vec!["B"]
    ));
}

#[test]
fn narrow_section_titles_allow_three_balanced_columns() {
    let scene = fake("mindmap\n  Root\n    One\n    Two\n    Three", 700.0, &[]);
    assert_eq!(scene.metrics.columns, 3);
    let ys: Vec<_> = scene
        .hit_regions
        .iter()
        .skip(1)
        .map(|hit| hit.rect.y)
        .collect();
    assert!(ys.iter().all(|y| *y == ys[0]));
    assert_eq!(
        fake("mindmap\n  Root\n    One\n    Two\n    Three", 380.0, &[])
            .metrics
            .columns,
        1
    );
}

#[test]
fn edge_labels_are_muted_prefixes_separate_from_node_labels() {
    for source in [
        "flowchart TB\nA -->|是| B[结论]",
        "flowchart TB\nA -- 是 --> B[结论]",
    ] {
        let tree = parse(source).unwrap();
        assert_eq!(tree.nodes[1].label, "结论");
        assert_eq!(tree.nodes[1].prefix.as_deref(), Some("是"));
        let scene = fake(source, 380.0, &[]);
        let prefix = scene
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                Primitive::Text(text) if text.text == "是：" => Some(text),
                _ => None,
            })
            .unwrap();
        let label = scene
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                Primitive::Text(text) if text.text == "结论" => Some(text),
                _ => None,
            })
            .unwrap();
        assert_ne!(prefix.color, label.color);
    }
}

#[test]
fn flowchart_style_directive_sets_root_accent_without_adding_a_node() {
    for source in [
        "flowchart TB\nA[Root] --> B[Leaf]\nstyle A fill:#fde9d9,stroke:#f00",
        "flowchart TB\nA[Root] --> B[Leaf]\nstyle A fill:rgba(255,0,0,.5),stroke:hsl(0,100%,50%)",
    ] {
        let tree = parse(source).unwrap();
        assert_eq!(tree.nodes.len(), 2);
        let style = tree.root_style().unwrap();
        assert!(style.stroke.is_some());
        assert!(style.fill.is_some());
        let scene = layout(&tree, &LayoutOptions::default(), &mut DeterministicMetrics).unwrap();
        assert_eq!(
            scene.metrics.root_hue,
            Some(Color::parse_css("red").unwrap().hue())
        );
    }
}

#[test]
fn visual_and_accessibility_directives_do_not_change_the_scene() {
    let base = "flowchart TB\nA[Root] --> B[Branch]\nB --> C[Leaf]";
    let decorated = format!(
        "{base}\nlinkStyle 0 stroke:#f00\nclick B \"https://example.test\"\naccTitle: Diagram\naccDescr: A description"
    );
    let before = fake(base, 700.0, &[]);
    let after = fake(&decorated, 700.0, &[]);
    assert_eq!(before.width, after.width);
    assert_eq!(before.height, after.height);
    assert_eq!(before.primitives, after.primitives);
    assert_eq!(before.hit_regions, after.hit_regions);
    assert_eq!(before.metrics.columns, after.metrics.columns);
    assert_eq!(before.metrics.min_font_px, after.metrics.min_font_px);
    assert_eq!(
        before.metrics.horizontal_overflow,
        after.metrics.horizontal_overflow
    );
    assert_eq!(before.metrics.node_overlaps, after.metrics.node_overlaps);
    assert_eq!(before.metrics.root_hue, after.metrics.root_hue);
    assert_eq!(before.metrics.branch_hues, after.metrics.branch_hues);
}

#[test]
fn nested_subgraphs_remain_unsupported_and_unknown_syntax_keeps_byte_ranges() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/subgraphs.mmd");
    let error = parse_diagram(source).unwrap_err();
    let DiagramError::UnsupportedConstruct { range, kind } = error else {
        panic!("nested subgraph must remain unsupported")
    };
    assert_eq!(kind, "subgraph");
    assert_eq!(&source[range.clone()], "subgraph validation[校验]");
    assert!(source.is_char_boundary(range.start));
    assert!(source.is_char_boundary(range.end));

    let source = "flowchart TB\nA --> B\n未知 语法 extra";
    let error = parse(source).unwrap_err();
    let DiagramError::Syntax { range, .. } = error else {
        panic!("应返回带源码范围的 Syntax")
    };
    assert_eq!(&source[range.clone()], "语法 extra");
    assert!(source.is_char_boundary(range.start));
    assert!(source.is_char_boundary(range.end));
}

#[test]
fn top_level_subgraphs_and_invisible_links_keep_layout_semantics() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/subgraphs-top-level.mmd");
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("expected graph")
    };
    assert_eq!(graph.ordinary_node_count(), 6);
    assert_eq!(graph.containers.len(), 2);
    assert_eq!(graph.visible_edge_count(), 3);
    assert_eq!(graph.invisible_edge_count(), 4);
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|edge| edge.container == Some(0))
            .count(),
        1
    );
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|edge| edge.container == Some(1))
            .count(),
        1
    );
    assert_eq!(graph.containers[0].direction, Direction::LeftToRight);
    assert_eq!(graph.containers[1].direction, Direction::LeftToRight);
    assert_eq!(
        graph.nodes[graph.containers[0].node_index].kind,
        GraphNodeKind::Container
    );
    for edge in graph.edges.iter().filter(|edge| !edge.visible) {
        assert!(source[edge.source_range.clone()].contains("~~~"));
        assert!(source.is_char_boundary(edge.source_range.start));
        assert!(source.is_char_boundary(edge.source_range.end));
    }

    let result = layout_graph(
        &graph,
        &LayoutOptions {
            width: 700.0,
            ..LayoutOptions::default()
        },
        &mut DeterministicMetrics,
    )
    .unwrap();
    assert_eq!(
        (result.metrics.edge_count, result.metrics.invisible_edges),
        (3, 4)
    );
    assert_eq!(result.metrics.containers, 2);
    assert_eq!(result.metrics.node_overlaps, 0);
    assert!(!result.metrics.horizontal_overflow);
    assert_eq!(result.scene.hit_regions.len(), 8);
    let paths = result
        .scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::GraphPath { source_range, .. } => Some(source_range),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 3);
    assert!(
        paths
            .iter()
            .all(|range| !source[(*range).clone()].contains("~~~"))
    );
    assert!(
        !result
            .scene
            .primitives
            .iter()
            .any(|primitive| matches!(primitive, Primitive::LabelBackground { .. }))
    );
    let hit = |id: &str| {
        result
            .scene
            .hit_regions
            .iter()
            .find(|hit| hit.node_id == id)
            .unwrap_or_else(|| panic!("missing hit {id}"))
    };
    assert!(hit("alpha").rect.y < hit("beta").rect.y);
    assert!(hit("A1").rect.x < hit("A2").rect.x);
    assert!(hit("B1").rect.x < hit("B2").rect.x);
    assert!(hit("A1").rect.y > hit("alpha").rect.y);
    assert!(hit("B2").rect.y < hit("beta").rect.y + hit("beta").rect.height);
    let alpha_source = &source[hit("alpha").source_range.clone()];
    assert!(alpha_source.starts_with("subgraph alpha"));
    assert!(alpha_source.trim_end().ends_with("end"));
    let alpha_container = &graph.containers[0];
    assert_eq!(&source[alpha_container.id_range.clone()], "alpha");
    assert_eq!(&source[alpha_container.title_range.clone()], "\"Alpha 😀\"");
    let alpha = hit("alpha").rect;
    let dashed = hit("A2").rect;
    assert!(result.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::GraphShape { rect, shape: Shape::Container, .. } if *rect == alpha
    )));
    assert!(result.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::GraphShape { rect, stroke_width, .. }
            if *rect == dashed && (*stroke_width - 2.0).abs() < 0.001
    )));
    assert!(result.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::Line { width, color, .. }
            if (*width - 2.0).abs() < 0.001
                && *color == Color::parse_css("#c33").unwrap()
    )));
    assert!(result.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::GraphShape { rect, fill, stroke_width, .. }
            if *rect == alpha && fill.a > 0.0 && (*stroke_width - 2.0).abs() < 0.001
    )));
    assert!(result.scene.primitives.iter().any(|primitive| matches!(
        primitive,
        Primitive::Text(span) if span.text.starts_with("Two")
            && span.color == Color::parse_css("#112233").unwrap()
    )));
    assert_eq!(
        result.scene.to_svg(),
        layout_graph(
            &graph,
            &LayoutOptions {
                width: 700.0,
                ..LayoutOptions::default()
            },
            &mut DeterministicMetrics
        )
        .unwrap()
        .scene
        .to_svg()
    );
}

#[test]
fn lr_subgraph_dag_keeps_forward_ranks_bands_and_sink_ports() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/subgraph-lr-dag.mmd");
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("expected graph")
    };
    let scene = |width| {
        layout_graph(
            &graph,
            &LayoutOptions {
                width,
                ..LayoutOptions::default()
            },
            &mut DeterministicMetrics,
        )
        .unwrap()
    };
    let narrow = scene(700.0);
    let wide = scene(1100.0);
    let edge = |from: &str, to: &str| {
        graph
            .edges
            .iter()
            .find(|edge| graph.nodes[edge.from].id == from && graph.nodes[edge.to].id == to)
            .unwrap_or_else(|| panic!("missing edge {from} -> {to}"))
    };
    let center = |scene: &GraphScene, id: &str| {
        let rect = graph_hit(scene, id).rect;
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    };
    let distinct_bands = |ids: &[&str]| {
        let mut tops = ids
            .iter()
            .map(|id| graph_hit(&narrow, id).rect.y)
            .collect::<Vec<_>>();
        tops.sort_by(f64::total_cmp);
        tops.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        tops
    };

    for id in ["C1", "C2", "C3", "C4", "C5"] {
        assert!(
            (graph_hit(&narrow, id).rect.y - graph_hit(&narrow, "C1").rect.y).abs() <= 0.5,
            "{id}: {:?}",
            ["C1", "C2", "C3", "C4", "C5"].map(|id| graph_hit(&narrow, id).rect)
        );
    }
    for (from, to) in [("C1", "C2"), ("C2", "C3"), ("C3", "C4"), ("C4", "C5")] {
        assert!(center(&narrow, from).0 < center(&narrow, to).0);
        let points = graph_path(&narrow, edge(from, to).id);
        assert!(
            (points[0].x - (graph_hit(&narrow, from).rect.x + graph_hit(&narrow, from).rect.width))
                .abs()
                < 0.01
        );
        assert!((points.last().unwrap().x - graph_hit(&narrow, to).rect.x).abs() < 0.01);
    }

    for (sink, predecessors) in [("M", vec!["L", "R"]), ("S", vec!["T1", "T2", "T3"])] {
        let centers = predecessors
            .iter()
            .map(|id| center(&narrow, id).1)
            .collect::<Vec<_>>();
        let expected = centers.iter().sum::<f64>() / centers.len() as f64;
        assert!((center(&narrow, sink).1 - expected).abs() <= 0.5);
        let mut ports = predecessors
            .iter()
            .map(|from| graph_path(&narrow, edge(from, sink).id).last().unwrap().y)
            .collect::<Vec<_>>();
        ports.sort_by(f64::total_cmp);
        assert!(ports.windows(2).all(|pair| pair[1] - pair[0] >= 6.0));
    }

    let wrapped = ["W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8"];
    assert_eq!(distinct_bands(&wrapped).len(), 2);
    let mut wide_tops = wrapped
        .iter()
        .map(|id| graph_hit(&wide, id).rect.y)
        .collect::<Vec<_>>();
    wide_tops.sort_by(f64::total_cmp);
    wide_tops.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    assert_eq!(wide_tops.len(), 1);
    let widths = wrapped
        .iter()
        .map(|id| graph_hit(&narrow, id).rect.width)
        .sum::<f64>()
        + 28.0 * (wrapped.len() - 1) as f64;
    assert!(widths > 616.0);
    let wide_widths = wrapped
        .iter()
        .map(|id| graph_hit(&wide, id).rect.width)
        .sum::<f64>()
        + 28.0 * (wrapped.len() - 1) as f64;
    assert!(wide_widths <= 1016.0);
    for (result, expected_down_entries) in [(&narrow, 1), (&wide, 0)] {
        assert_eq!(result.metrics.node_overlaps, 0);
        assert!(!result.metrics.horizontal_overflow);
        let mut down_entries = 0;
        for edge in &graph.edges {
            let points = graph_path(result, edge.id);
            let source = graph_hit(result, &graph.nodes[edge.from].id).rect;
            let target = graph_hit(result, &graph.nodes[edge.to].id).rect;
            let last = points.last().unwrap();
            let before = points[points.len() - 2];
            let vertical_end = (last.x - before.x).abs() < 0.01;
            if vertical_end {
                down_entries += 1;
                assert!((last.y - target.y).abs() < 0.01);
                assert!(target.y > source.y + source.height);
                assert!((points[0].y - (source.y + source.height)).abs() < 0.01);
            } else {
                assert!(
                    center(result, &graph.nodes[edge.to].id).0
                        > center(result, &graph.nodes[edge.from].id).0
                );
                assert!((points[0].x - (source.x + source.width)).abs() < 0.01);
                assert!((last.x - target.x).abs() < 0.01);
            }
            assert!(points.windows(2).all(|pair| {
                (pair[0].x - pair[1].x).abs() < 0.01 || (pair[0].y - pair[1].y).abs() < 0.01
            }));
            for (segment, pair) in points.windows(2).enumerate() {
                for node in &graph.nodes {
                    if node.id == graph.nodes[edge.from].id || node.id == graph.nodes[edge.to].id {
                        continue;
                    }
                    if node.kind == GraphNodeKind::Container {
                        continue;
                    }
                    let rect = graph_hit(result, &node.id).rect;
                    let intersects = if (pair[0].x - pair[1].x).abs() < 0.01 {
                        pair[0].x > rect.x + 0.01
                            && pair[0].x < rect.x + rect.width - 0.01
                            && pair[0].y.min(pair[1].y) < rect.y + rect.height - 0.01
                            && pair[0].y.max(pair[1].y) > rect.y + 0.01
                    } else {
                        pair[0].y > rect.y + 0.01
                            && pair[0].y < rect.y + rect.height - 0.01
                            && pair[0].x.min(pair[1].x) < rect.x + rect.width - 0.01
                            && pair[0].x.max(pair[1].x) > rect.x + 0.01
                    };
                    assert!(
                        !intersects,
                        "edge {} segment {segment} crosses {}",
                        edge.id, node.id
                    );
                }
            }
        }
        assert_eq!(down_entries, expected_down_entries);
    }
}

#[test]
fn lr_subgraph_edge_cases_separate_sink_ports_and_duplicate_channels() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/subgraph-lr-edge-cases.mmd");
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("expected graph")
    };
    let scene = |width| {
        layout_graph(
            &graph,
            &LayoutOptions {
                width,
                ..LayoutOptions::default()
            },
            &mut DeterministicMetrics,
        )
        .unwrap()
    };
    let narrow = scene(700.0);
    let wide = scene(1100.0);
    let paths_between = |result: &GraphScene, from: &str, to: &str| {
        graph
            .edges
            .iter()
            .filter(|edge| graph.nodes[edge.from].id == from && graph.nodes[edge.to].id == to)
            .map(|edge| graph_path(result, edge.id).clone())
            .collect::<Vec<_>>()
    };
    let sink_ports = |result: &GraphScene, sink: &str| {
        let rect = graph_hit(result, sink).rect;
        let mut ports = graph
            .edges
            .iter()
            .filter(|edge| graph.nodes[edge.to].id == sink)
            .map(|edge| *graph_path(result, edge.id).last().unwrap())
            .collect::<Vec<_>>();
        ports.sort_by(|a, b| a.y.total_cmp(&b.y));
        for port in &ports {
            assert!(
                (port.x - rect.x).abs() < 0.01 && port.y > rect.y && port.y < rect.y + rect.height,
                "{sink} 入口未落在左侧端口区：{port:?} / {rect:?}"
            );
        }
        assert!(
            ports.windows(2).all(|pair| pair[1].y - pair[0].y >= 6.0),
            "{sink} 相邻入口中心间距不足：{ports:?}"
        );
        ports
    };
    let shared_segment = |a: &[Point], b: &[Point]| {
        a.windows(2).any(|first| {
            b.windows(2).any(|second| {
                let (a0, a1) = (first[0], first[1]);
                let (b0, b1) = (second[0], second[1]);
                if (a0.y - a1.y).abs() < 0.01 && (b0.y - b1.y).abs() < 0.01 {
                    (a0.y - b0.y).abs() < 0.01
                        && a0.x.min(a1.x) < b0.x.max(b1.x) - 0.01
                        && b0.x.min(b1.x) < a0.x.max(a1.x) - 0.01
                } else if (a0.x - a1.x).abs() < 0.01 && (b0.x - b1.x).abs() < 0.01 {
                    (a0.x - b0.x).abs() < 0.01
                        && a0.y.min(a1.y) < b0.y.max(b1.y) - 0.01
                        && b0.y.min(b1.y) < a0.y.max(a1.y) - 0.01
                } else {
                    false
                }
            })
        })
    };
    for (result, cross_band) in [(&narrow, true), (&wide, false)] {
        assert_eq!(result.metrics.node_overlaps, 0);
        assert!(!result.metrics.horizontal_overflow);
        assert_eq!(sink_ports(result, "S5").len(), 5);
        assert_eq!(sink_ports(result, "T6").len(), 6);
        for (preds, sink) in [
            (["A1", "B1", "C1", "D1", "E1"].as_slice(), "S5"),
            (["A2", "B2", "C2", "D2", "E2", "F2"].as_slice(), "T6"),
        ] {
            let target = graph_hit(result, sink).rect;
            for from in preds {
                let path = &paths_between(result, from, sink)[0];
                let source = graph_hit(result, from).rect;
                assert!((path[0].x - (source.x + source.width)).abs() < 0.01);
                assert!((path.last().unwrap().x - target.x).abs() < 0.01);
            }
        }
        for (from, to, count) in [("Q1", "Q2", 2), ("R1", "R2", 3)] {
            let paths = paths_between(result, from, to);
            assert_eq!(paths.len(), count, "{from}->{to}");
            let source = graph_hit(result, from).rect;
            let target = graph_hit(result, to).rect;
            for (index, path) in paths.iter().enumerate() {
                assert!((path[0].x - (source.x + source.width)).abs() < 0.01);
                assert!((path.last().unwrap().x - target.x).abs() < 0.01);
                for other in &paths[index + 1..] {
                    assert_ne!(path, other);
                    assert!(!shared_segment(path, other));
                }
            }
            let mut ports = paths.iter().map(|path| path[0].y).collect::<Vec<_>>();
            ports.sort_by(f64::total_cmp);
            ports.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            assert_eq!(ports.len(), count, "{from}->{to} 端口未分离");
        }
        let wrapped = paths_between(result, "W3", "W4");
        assert_eq!(wrapped.len(), 3);
        assert_eq!(
            wrapped
                .iter()
                .map(|path| {
                    let port = path[0];
                    if cross_band {
                        (port.x * 1000.0) as i64
                    } else {
                        (port.y * 1000.0) as i64
                    }
                })
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
        for (index, path) in wrapped.iter().enumerate() {
            for other in &wrapped[index + 1..] {
                assert_ne!(path, other);
                assert!(!shared_segment(path, other));
            }
        }
        let mut targets = wrapped
            .iter()
            .map(|path| *path.last().unwrap())
            .collect::<Vec<_>>();
        targets.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        targets.dedup_by(|a, b| (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01);
        assert_eq!(targets.len(), 3, "W3->W4 目标端口未分离");
        let source = graph_hit(result, "W3").rect;
        let target = graph_hit(result, "W4").rect;
        for path in &wrapped {
            if cross_band {
                assert!((path[0].y - (source.y + source.height)).abs() < 0.01);
                assert!((path.last().unwrap().y - target.y).abs() < 0.01);
            } else {
                assert!((path[0].x - (source.x + source.width)).abs() < 0.01);
                assert!((path.last().unwrap().x - target.x).abs() < 0.01);
            }
        }
        for edge in &graph.edges {
            let path = graph_path(result, edge.id);
            assert!(path.windows(2).all(|pair| {
                (pair[0].x - pair[1].x).abs() < 0.01 || (pair[0].y - pair[1].y).abs() < 0.01
            }));
            for (segment, pair) in path.windows(2).enumerate() {
                for node in &graph.nodes {
                    if node.id == graph.nodes[edge.from].id || node.id == graph.nodes[edge.to].id {
                        continue;
                    }
                    if node.kind == GraphNodeKind::Container {
                        continue;
                    }
                    let rect = graph_hit(result, &node.id).rect;
                    let intersects = if (pair[0].x - pair[1].x).abs() < 0.01 {
                        pair[0].x > rect.x + 0.01
                            && pair[0].x < rect.x + rect.width - 0.01
                            && pair[0].y.min(pair[1].y) < rect.y + rect.height - 0.01
                            && pair[0].y.max(pair[1].y) > rect.y + 0.01
                    } else {
                        pair[0].y > rect.y + 0.01
                            && pair[0].y < rect.y + rect.height - 0.01
                            && pair[0].x.min(pair[1].x) < rect.x + rect.width - 0.01
                            && pair[0].x.max(pair[1].x) > rect.x + 0.01
                    };
                    assert!(
                        !intersects,
                        "edge {} segment {segment} crosses {}",
                        edge.id, node.id
                    );
                }
            }
        }
    }
}

#[test]
fn bare_subgraph_uses_its_id_as_the_title() {
    let source = "flowchart TB\nsubgraph plain\nA --> B\nend";
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("graph")
    };
    assert_eq!(graph.containers.len(), 1);
    let node = &graph.nodes[graph.containers[0].node_index];
    assert_eq!(node.id, "plain");
    assert_eq!(node.label, "plain");
    assert_eq!(node.kind, GraphNodeKind::Container);
    assert_eq!(&source[graph.containers[0].id_range.clone()], "plain");
    assert_eq!(&source[graph.containers[0].title_range.clone()], "plain");
    assert_eq!(graph.ordinary_node_count(), 2);
    let error = parse_diagram("flowchart TB\nA[Ordinary]\nsubgraph A\nB --> C\nend").unwrap_err();
    assert!(matches!(
        error,
        DiagramError::Syntax { kind, .. } if kind == "subgraph_duplicate_id"
    ));
}

#[test]
fn ordinary_invisible_links_order_without_routing_cycles() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/invisible-links.mmd");
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("expected graph")
    };
    let result =
        layout_graph(&graph, &LayoutOptions::default(), &mut DeterministicMetrics).unwrap();
    assert_eq!(result.metrics.edge_count, 1);
    assert_eq!(result.metrics.invisible_edges, 3);
    assert_eq!(
        result
            .scene
            .primitives
            .iter()
            .filter(|primitive| matches!(primitive, Primitive::GraphPath { .. }))
            .count(),
        1
    );
    let y = |id: &str| {
        result
            .scene
            .hit_regions
            .iter()
            .find(|hit| hit.node_id == id)
            .unwrap()
            .rect
            .y
    };
    assert!(y("A") < y("B") && y("B") < y("C"));

    let graph = match parse_diagram("flowchart TB\nA ~~~ B\nB ~~~ A").unwrap() {
        Diagram::Graph(graph) => graph,
        _ => panic!("graph"),
    };
    let result =
        layout_graph(&graph, &LayoutOptions::default(), &mut DeterministicMetrics).unwrap();
    assert_eq!(result.metrics.edge_count, 0);
    assert_eq!(result.metrics.invisible_edges, 2);
    assert!(
        !result
            .scene
            .primitives
            .iter()
            .any(|primitive| matches!(primitive, Primitive::GraphPath { .. }))
    );
}

#[test]
fn markdown_fences_and_absolute_source_mapping() {
    let markdown = "# Note\r\n````text\r\n```mermaid\r\nignored\r\n````\r\n  ~~~mermaid\r\nflowchart TB\r\nA[Root] --> B[Leaf]\r\n  ~~~~\r\n```mermaid\nmindmap\n  Root\n    Child\n";
    let first = mermaid_block(markdown, 1).unwrap();
    let tree = first.parse().unwrap();
    assert_eq!(&markdown[tree.nodes[0].source_range.clone()], "A[Root] ");
    assert_eq!(
        mermaid_block(markdown, 2)
            .unwrap()
            .parse()
            .unwrap()
            .nodes
            .len(),
        2
    );
    assert!(matches!(
        mermaid_block(markdown, 3),
        Err(DiagramError::MissingBlock { .. })
    ));
    assert!(mermaid_block(markdown, 0).is_err());
}

#[test]
fn container_fences_keep_absolute_utf8_ranges_and_stripped_bodies() {
    let markdown = concat!(
        "# Note\n\n",
        "> quoted\n",
        "> ```mermaid\n",
        "> flowchart TB\n",
        "> A[Quote root] --> B\n",
        "> ```\n\n",
        "> > nested\n",
        "> > ~~~mermaid\n",
        "> > mindmap\n",
        "> >   Root\n",
        "> >     Child\n",
        "> > ~~~\n\n",
        "> [!note]\n",
        "> callout\n",
        "> ```mermaid\n",
        "> graph LR\n",
        "> C --> D\n",
        "> ```\n\n",
        "```mermaid\n",
        "flowchart TB\n",
        "Top --> Level\n",
        "```\n\n",
        "- item\n\n",
        "    ```mermaid\n",
        "    flowchart TB\n",
        "    E --> F\n",
        "    ```\n\n",
        "    ```text\n",
        "    not a diagram\n",
        "    ```\n\n",
        "    ```mermaid\n",
        "    flowchart TB\n",
        "    G --> H\n",
        "    ```\n",
    );
    let blocks = mermaid_blocks(markdown);
    let expected = [
        ("flowchart TB\nA[Quote root] --> B\n", true),
        ("mindmap\n  Root\n    Child\n", true),
        ("graph LR\nC --> D\n", true),
        ("flowchart TB\nTop --> Level\n", false),
        ("flowchart TB\nE --> F\n", true),
        ("flowchart TB\nG --> H\n", true),
    ];
    assert_eq!(blocks.len(), expected.len());
    for (block, (source, container)) in blocks.iter().zip(expected) {
        assert_eq!(block.source, source);
        assert_eq!(block.container, container);
        assert!(markdown.is_char_boundary(block.fence_range.start));
        assert!(markdown.is_char_boundary(block.fence_range.end));
        assert!(block.fence_range.start <= block.source_range.start);
        assert!(block.source_range.end <= block.fence_range.end);
        let tree = block.parse().unwrap();
        assert!(!tree.nodes.is_empty());
        for node in &tree.nodes {
            assert!(block.fence_range.contains(&node.source_range.start));
            assert!(node.source_range.end <= block.fence_range.end);
            let raw = &markdown[node.source_range.clone()];
            if node.id.starts_with("mindmap-") {
                assert!(
                    raw.contains(&node.label),
                    "节点 {:?} 的范围没有指向原文",
                    node.id
                );
            } else {
                assert!(
                    raw.starts_with(&node.id),
                    "节点 {:?} 的范围没有指向原文",
                    node.id
                );
            }
        }
    }
    let quote = &blocks[0];
    assert!(markdown[quote.fence_range.clone()].starts_with("> ```mermaid"));
    assert!(markdown[quote.fence_range.clone()].ends_with("> ```\n"));
    let nested = &blocks[1];
    assert!(markdown[nested.fence_range.clone()].starts_with("> > ~~~mermaid"));
    assert_eq!(blocks[4].parse().unwrap().nodes[1].label, "F");

    let indented = "    ```mermaid\n    flowchart TB\n    A --> B\n    ```\n";
    assert!(mermaid_blocks(indented).is_empty());
    let quoted_after_paragraph = "正文\n\n    ```mermaid\n    flowchart TB\n    A --> B\n    ```\n";
    assert!(mermaid_blocks(quoted_after_paragraph).is_empty());

    let invalid = "> ```mermaid\n> flowchart TB\n> A ??? B\n> ```\n";
    let block = &mermaid_blocks(invalid)[0];
    let DiagramError::Syntax { range, .. } = block.parse().unwrap_err() else {
        panic!("应返回带源码范围的 Syntax")
    };
    assert!(invalid.is_char_boundary(range.start));
    assert!(invalid.is_char_boundary(range.end));
    assert_eq!(&invalid[range], "??? B");
}

#[test]
fn mindmap_auto_ids_avoid_all_explicit_ids_including_later_ones() {
    let report = "mindmap\n  Root\n    mindmap-2[Explicit]\n      Child";
    let tree = parse(report).unwrap();
    assert_eq!(tree.nodes.len(), 3);
    assert_eq!(tree.nodes[1].id, "mindmap-2");
    assert_eq!(tree.nodes[2].id, "mindmap-3");

    let later = "mindmap\n  Root\n    Child\n      mindmap-1[Later]\n        Grandchild";
    let tree = parse(later).unwrap();
    let ids = tree
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), tree.nodes.len());
    assert!(ids.contains("mindmap-1"));
    assert_eq!(tree.nodes[1].id, "mindmap-2");
    assert_eq!(tree.nodes[3].id, "mindmap-3");

    let duplicate = "mindmap\n  Root\n    A[One]\n    A[Two]";
    assert!(matches!(
        parse(duplicate),
        Err(DiagramError::Syntax { kind, .. }) if kind == "duplicate_mindmap_id"
    ));
}

fn layout_chain(depth: usize) -> String {
    let mut source = "flowchart TB\nA0".to_string();
    for i in 1..=depth {
        source.push_str(&format!(" --> A{i}"));
    }
    source.push('\n');
    source
}

fn layout_mindmap_chain(depth: usize) -> String {
    let mut source = "mindmap\n".to_string();
    for level in 0..depth {
        source.push_str(&format!("{}Level {level}\n", " ".repeat(level)));
    }
    source
}

fn digest(source: &str, width: f64, collapsed: &[&str]) -> u64 {
    let mut scene = layout(
        &parse(source).unwrap(),
        &LayoutOptions {
            width,
            collapsed: collapsed.iter().map(|s| s.to_string()).collect(),
            ..LayoutOptions::default()
        },
        &mut DeterministicMetrics,
    )
    .unwrap();
    // 排版耗时不参与比较，其余字段与旧实现逐字节一致。
    scene.metrics.layout_ms = 0.0;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in format!("{scene:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn iterative_layout_matches_the_previous_recursive_output() {
    for (source, expected) in [
        (layout_chain(64), 0x927e3b9690aa815fu64),
        (layout_chain(200), 0xe4003fef25ced52b),
        (layout_chain(512), 0x7751cdc05d652d14),
        (layout_mindmap_chain(200), 0xcd6976ae246192e1),
    ] {
        let width = if source.starts_with("mindmap") {
            380.0
        } else {
            700.0
        };
        assert_eq!(digest(&source, width, &[]), expected);
    }
    let branching = "flowchart TB\nR[Root] --> A[Alpha section] --> A1[Leaf one]\nA --> A2[Second leaf]\nR --> B[Beta section]:::hot --> B1[Leaf three]\nB --> B2[Leaf four]\nR --> C[Gamma]\nclassDef hot fill:#fde9d9,stroke:#c33";
    for (width, expected) in [
        (380.0, 0x3f313746a6e0d075u64),
        (700.0, 0x6c8c5ba9350b90d6),
        (1100.0, 0xd103e3a095eb5957),
    ] {
        assert_eq!(digest(branching, width, &[]), expected);
    }
    assert_eq!(digest(branching, 700.0, &["A", "B"]), 0x4ca0e0e9d9eab8c3);
    let labelled =
        "flowchart LR\nA[开始] -->|是| B{判断}\nB -- 否 --> C[结束]\nA --> D((圆)) --> E[[子程序]]";
    assert_eq!(digest(labelled, 380.0, &[]), 0xec46a1b1cbb3f259);
}

#[test]
fn deep_chain_layout_does_not_overflow_the_stack() {
    const CHILD: &str = "HORTUS_DIAGRAM_DEEP_CHAIN_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let tree = parse(&layout_chain(50_000)).unwrap();
        // 递归实现会在几百 KB 的线程栈上中止；显式栈应正常返回。
        let scene = std::thread::Builder::new()
            .name("deep-layout".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                layout(
                    &tree,
                    &LayoutOptions {
                        width: 700.0,
                        ..LayoutOptions::default()
                    },
                    &mut DeterministicMetrics,
                )
            })
            .unwrap()
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(scene.hit_regions.len(), 50_001);
        assert_eq!(scene.metrics.node_overlaps, 0);
        assert!(!scene.metrics.horizontal_overflow);
        assert!(scene.to_svg().starts_with("<svg"));
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "deep_chain_layout_does_not_overflow_the_stack",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "深链子进程失败：{:?}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn mindmaps_cover_shallow_deep_shapes_icons_and_classes() {
    for (source, count) in [
        (
            include_str!("../../../fixtures/mermaid/mindmap/shallow.mmd"),
            7,
        ),
        (
            include_str!("../../../fixtures/mermaid/mindmap/deep.mmd"),
            8,
        ),
        (
            include_str!("../../../fixtures/mermaid/mindmap/shapes-classes.mmd"),
            10,
        ),
    ] {
        let tree = parse(source).unwrap();
        assert_eq!(tree.nodes.len(), count);
        for width in [380.0, 700.0, 1100.0] {
            let scene = fake(source, width, &[]);
            assert_eq!(scene.hit_regions.len(), count);
            assert_eq!(scene.metrics.node_overlaps, 0);
            assert!(!scene.metrics.horizontal_overflow);
            assert!(scene.metrics.min_font_px >= 13.6);
        }
    }
    let tree = parse(include_str!(
        "../../../fixtures/mermaid/mindmap/shapes-classes.mmd"
    ))
    .unwrap();
    assert_eq!(tree.nodes[0].class.as_deref(), Some("central"));
    assert_eq!(tree.nodes.iter().filter(|n| n.class.is_some()).count(), 3);
    assert_eq!(tree.nodes[5].label, "Cloud");
    assert_eq!(tree.nodes[6].label, "Bang");
    let tree = parse("mindmap\n  r[\"Root %% ::icon()\"]\n    a[Child]::icon(fa fa-book):::blue %% comment\n    Plain child:::green::icon(fa fa-star)\n      ::icon(fa fa-code):::nested\n").unwrap();
    assert_eq!(tree.nodes.len(), 3);
    assert_eq!(tree.nodes[0].label, "Root %% ::icon()");
    assert_eq!(tree.nodes[1].class.as_deref(), Some("blue"));
    assert_eq!(tree.nodes[2].class.as_deref(), Some("nested"));
}

#[test]
fn contiguous_partition_is_optimal_and_ordered() {
    let heights = [70.0, 110.0, 40.0, 190.0, 50.0, 60.0];
    let groups = partition(&heights, 3);
    assert_eq!(
        groups.iter().cloned().flatten().collect::<Vec<_>>(),
        (0..heights.len()).collect::<Vec<_>>()
    );
    let cost = groups
        .iter()
        .map(|g| heights[g.clone()].iter().sum::<f64>())
        .fold(0.0, f64::max);
    let mut optimum = f64::INFINITY;
    for a in 1..heights.len() - 1 {
        for b in a + 1..heights.len() {
            optimum = optimum.min(
                [
                    heights[..a].iter().sum::<f64>(),
                    heights[a..b].iter().sum(),
                    heights[b..].iter().sum(),
                ]
                .into_iter()
                .fold(0.0, f64::max),
            );
        }
    }
    assert_eq!(cost, optimum);
    assert!(partition(&[], 3).is_empty());
}

#[test]
fn tied_partition_fills_earlier_columns_first() {
    // 8 个等高分支分 3 栏：3/3/2，短栏在最右
    let heights = [309.37; 8];
    let groups = partition(&heights, 3);
    assert_eq!(groups, vec![0..3, 3..6, 6..8]);
    let heights = [1.0; 7];
    assert_eq!(partition(&heights, 3), vec![0..3, 3..6, 6..7]);
}

#[test]
fn collapse_hit_regions_themes_and_badges() {
    let source = "flowchart TB\nA[Root] --> B[Branch]\nB --> C[First leaf]\nB --> D[Second leaf]\nA --> E[Another section]";
    let open = fake(source, 700.0, &[]);
    let closed = fake(source, 700.0, &["B"]);
    assert_eq!(closed.hit_regions.len(), 3);
    assert!(closed.height < open.height);
    let hit = closed
        .hit_regions
        .iter()
        .find(|h| h.node_id == "B")
        .unwrap();
    assert!(hit.can_collapse && hit.collapsed);
    assert_eq!(
        closed
            .hit_test(Point {
                x: hit.rect.x + 1.0,
                y: hit.rect.y + 1.0
            })
            .unwrap()
            .node_id,
        "B"
    );
    assert!(closed.to_svg().contains("+2"));
    assert_eq!(fake(source, 380.0, &["A"]).hit_regions.len(), 1);
    let dark = layout(
        &parse(source).unwrap(),
        &LayoutOptions {
            theme: Theme::Dark,
            ..LayoutOptions::default()
        },
        &mut DeterministicMetrics,
    )
    .unwrap();
    assert_ne!(open.primitives[0], dark.primitives[0]);
    assert_eq!(open.height, dark.height);
}

#[test]
fn changing_leaf_without_rewrapping_leaves_other_primitives_exactly_in_place() {
    let source = "graph TB\nR[Root] --> A[First section] --> L[Short leaf]\nA --> M[Other leaf]\nR --> B[Second section] --> N[Stable leaf]";
    let before = fake(source, 700.0, &[]);
    let after = fake(&source.replace("Short leaf", "A longer leaf"), 700.0, &[]);
    let hit = before
        .hit_regions
        .iter()
        .find(|h| h.node_id == "L")
        .unwrap();
    let keep = |p: &Primitive| !matches!(p, Primitive::Text(t) if hit.rect.contains(t.baseline));
    assert_eq!(
        before
            .primitives
            .iter()
            .filter(|p| keep(p))
            .collect::<Vec<_>>(),
        after
            .primitives
            .iter()
            .filter(|p| keep(p))
            .collect::<Vec<_>>()
    );
    for (a, b) in before.hit_regions.iter().zip(&after.hit_regions) {
        assert_eq!(a.rect, b.rect);
    }
}

#[test]
fn wrapping_keeps_punctuation_parentheses_and_graphemes_readable() {
    let source = "flowchart TB\nR[Typography] --> A[中文术语 中文说明，后续说明（内部 不要拆开）以及更多内容。]\nA --> B[English words remain the same color]\nA --> C[ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz]\nA --> D[组合字符 ééé 与家庭 👩‍👩‍👧‍👦]\nA --> E[Title<br/>Smaller subtitle]";
    let scene = fake(source, 260.0, &[]);
    assert!(!scene.metrics.horizontal_overflow);
    assert_eq!(scene.metrics.node_overlaps, 0);
    let mut lines: Vec<(f64, String)> = Vec::new();
    for p in &scene.primitives {
        if let Primitive::Text(t) = p {
            if let Some((_, value)) = lines.iter_mut().find(|(y, _)| *y == t.baseline.y) {
                value.push_str(&t.text);
            } else {
                lines.push((t.baseline.y, t.text.clone()));
            }
        }
    }
    assert!(
        lines
            .iter()
            .all(|(_, s)| !s.starts_with(['，', '。', '）']))
    );
    assert!(lines.iter().any(|(_, s)| s.contains("（内部 不要拆开）")));
    assert!(lines.iter().any(|(_, s)| s.contains("👩‍👩‍👧‍👦")));
    assert!(scene.metrics.min_font_px >= 13.6);
    assert!(scene.to_svg().contains("textLength="));
    let escaped = fake("flowchart TB\nA[\"<&quot;&amp;\"]", 380.0, &[]).to_svg();
    assert!(escaped.contains("&lt;&quot;&amp;"));
}

#[test]
fn invalid_widths_and_very_deep_trees() {
    let tree = parse("mindmap\n  Root\n    Child").unwrap();
    for width in [f64::NAN, f64::INFINITY, -1.0, 0.0, 119.0] {
        assert!(
            layout(
                &tree,
                &LayoutOptions {
                    width,
                    ..LayoutOptions::default()
                },
                &mut DeterministicMetrics
            )
            .is_err()
        );
    }
    let mut source = "mindmap\n".to_string();
    for level in 0..64 {
        source.push_str(&format!("{}Level {level}\n", " ".repeat(level)));
    }
    let scene = fake(&source, 380.0, &[]);
    assert_eq!(scene.hit_regions.len(), 64);
    assert!(!scene.metrics.horizontal_overflow);
}

#[test]
fn graph_chain_keeps_balanced_rows_after_window_resize() {
    let source = include_str!("../../../fixtures/mermaid/flowchart/erikson.mmd");
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("LR chain must preserve graph direction");
    };
    let mut previous = None;
    for width in [1100.0, 700.0, 1100.0] {
        let result = layout_graph(
            &graph,
            &LayoutOptions {
                width,
                previous,
                ..LayoutOptions::default()
            },
            &mut DeterministicMetrics,
        )
        .unwrap();
        let hits = &result.scene.hit_regions;
        assert_eq!(hits.len(), 8);
        for row in hits.chunks(4) {
            assert!(
                row.iter().all(|hit| hit.rect.y == row[0].rect.y),
                "width={width} rows={hits:?}"
            );
            assert!(row.windows(2).all(|pair| pair[0].rect.x < pair[1].rect.x));
        }
        assert!(hits[0].rect.y < hits[4].rect.y);
        assert_eq!(result.metrics.node_overlaps, 0);
        assert!(!result.metrics.horizontal_overflow);
        previous = Some(result.previous);
    }
}

fn sample(prefix: &str) -> Option<String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mermaid/tree");
    let path = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with(prefix))?
        .path();
    std::fs::read_to_string(path).ok()
}

#[test]
fn private_samples_structural_acceptance() {
    for (prefix, nodes) in [("01", 51), ("04", 132)] {
        let Some(markdown) = sample(prefix) else {
            eprintln!("跳过 {prefix}：本机私人样本不存在");
            continue;
        };
        let tree = mermaid_block(&markdown, 1).unwrap().parse().unwrap();
        assert_eq!(tree.nodes.len(), nodes);
        for (width, columns) in [(380.0, 1), (700.0, 2), (1100.0, 3)] {
            let scene = layout(
                &tree,
                &LayoutOptions {
                    width,
                    ..LayoutOptions::default()
                },
                &mut DeterministicMetrics,
            )
            .unwrap();
            assert_eq!(scene.metrics.columns, columns);
            assert_eq!(scene.hit_regions.len(), nodes);
            assert_text_within_nodes(&scene);
            assert!(!scene.metrics.horizontal_overflow);
            assert_eq!(scene.metrics.node_overlaps, 0);
            assert!(scene.metrics.min_font_px >= 13.6);
            if prefix == "01" {
                let root = scene.metrics.root_hue.unwrap();
                assert!(scene.metrics.branch_hues.iter().all(|h| {
                    let d = (h - root).abs();
                    d.min(360.0 - d) > 20.0
                }));
            }
        }
    }
}

#[test]
fn private_samples_real_font_height_acceptance() {
    let Ok(mut fonts) = SystemFonts::new() else {
        eprintln!("跳过真实高度：没有系统字体");
        return;
    };
    if !fonts.reference_fonts_available() {
        eprintln!("跳过真实高度：缺少系统 UI 字体或 PingFang SC");
        return;
    }
    for (prefix, targets, previous) in [
        ("01", [1789.0, 981.0, 724.0], [1789.0, 1006.0, 722.0]),
        ("04", [4163.0, 2175.0, 1580.0], [4177.35, 2183.35, 1584.35]),
    ] {
        let Some(markdown) = sample(prefix) else {
            eprintln!("跳过 {prefix}：本机私人样本不存在");
            continue;
        };
        let tree = mermaid_block(&markdown, 1).unwrap().parse().unwrap();
        for ((width, target), previous) in [380.0, 700.0, 1100.0]
            .into_iter()
            .zip(targets)
            .zip(previous)
        {
            let scene = layout(
                &tree,
                &LayoutOptions {
                    width,
                    ..LayoutOptions::default()
                },
                &mut fonts,
            )
            .unwrap();
            let delta = (scene.height - target).abs() / target;
            eprintln!(
                "{prefix} {width}: 高度 {:.2}，差异 {:.2}%",
                scene.height,
                delta * 100.0
            );
            assert!(delta <= 0.10);
            assert!((scene.height - previous).abs() < 0.01);
            assert_text_within_nodes(&scene);
            assert!(!scene.metrics.horizontal_overflow);
            assert_eq!(scene.metrics.node_overlaps, 0);
        }
    }
}

#[test]
fn root_accent_prefers_stroke_and_falls_back_to_fill() {
    for style in ["fill:#f00,stroke:#00f", "fill:#00f"] {
        let scene = fake(
            &format!("flowchart TB\nR[Root]:::key --> A[One]\nR --> B[Two]\nclassDef key {style}"),
            700.0,
            &[],
        );
        assert_eq!(
            scene.metrics.root_hue,
            Some(Color::parse_css("#00f").unwrap().hue())
        );
    }
}

#[test]
fn css_alpha_hsl_and_named_colors_produce_expected_hues() {
    let red = Color::parse_css("#ff0000").unwrap();
    for value in [
        "#f008",
        "#ff000080",
        "rgba(255, 0, 0, 0.5)",
        "hsla(0, 100%, 50%, 0.5)",
    ] {
        let color = Color::parse_css(value).unwrap_or_else(|| panic!("未解析 {value}"));
        assert_eq!(color.hue(), red.hue(), "{value}");
        assert!(color.a > 0.0 && color.a < 1.0, "{value}");
    }

    let green = Color::parse_css("#00ff00").unwrap();
    for value in ["hsl(120, 100%, 50%)", "hsla(120, 100%, 50%, 0.25)"] {
        assert_eq!(
            Color::parse_css(value).unwrap().hue(),
            green.hue(),
            "{value}"
        );
    }
    assert_eq!(
        Color::parse_css("rebeccapurple").unwrap().hue(),
        Color::parse_css("#663399").unwrap().hue()
    );
    assert!(Color::parse_css("not-a-color").is_none());
}

#[test]
fn long_dotted_edges_and_multiple_sources() {
    for connector in ["-..->", "-..-", "-...->", "-....-"] {
        let tree = parse(&format!("flowchart TB\nA {connector} B")).unwrap();
        assert_eq!(tree.nodes.len(), 2);
    }
    assert!(matches!(
        parse("flowchart TB\nB & C --> D"),
        Err(DiagramError::NotATree { .. })
    ));
}

#[test]
fn tree_dispatch_preserves_legacy_styles_before_graph_validation() {
    for style in [
        "linkStyle 0 stroke:#f00",
        "style A fill:rgba(255,0,0,0.5),stroke:hsl(120,50%,50%)",
        "classDef vivid fill:#f008,stroke:#1234,stroke-width:2px\nclass A vivid",
    ] {
        let source = format!("flowchart TB\nA[Root] --> B[Leaf]\n{style}\n");
        let legacy = parse(&source).unwrap();
        let Diagram::Tree(tree) = parse_diagram(&source).unwrap() else {
            panic!("ordinary tree")
        };
        assert_eq!(tree.nodes, legacy.nodes);
        assert_eq!(tree.class_defs, legacy.class_defs);
        assert_eq!(tree.inline_styles, legacy.inline_styles);
        let mut fonts = DeterministicMetrics;
        let mut actual = layout(&tree, &LayoutOptions::default(), &mut fonts).unwrap();
        let mut expected = layout(&legacy, &LayoutOptions::default(), &mut fonts).unwrap();
        actual.metrics.layout_ms = 0.0;
        expected.metrics.layout_ms = 0.0;
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }
}

#[test]
fn unsupported_graph_constructs_keep_utf8_ranges_and_class_fallback_is_diagnostic() {
    let source = "flowchart TB\nA[甲] --> C\nB --> C\nlinkStyle 0 stroke:#f00\n";
    let DiagramError::UnsupportedConstruct { range, .. } =
        parse_diagram_at(source, 17).unwrap_err()
    else {
        panic!("unsupported, not syntax")
    };
    assert_eq!(
        &source[range.start - 17..range.end - 17],
        "linkStyle 0 stroke:#f00"
    );
    let source = "flowchart TB\nA[甲]:::state --> C\nB --> C\nclassDef state fill:#ff0000,stroke-width:2px,stroke:#112233,font-size:18px";
    let Diagram::Graph(graph) = parse_diagram(source).unwrap() else {
        panic!("graph")
    };
    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.class_defs["state"].fill.as_deref(), Some("#ff0000"));
    assert_eq!(graph.class_defs["state"].stroke.as_deref(), Some("#112233"));
    assert_eq!(graph.class_defs["state"].stroke_width, Some(2.0));
    assert_eq!(graph.warnings.len(), 1);
    assert_eq!(&source[graph.warnings[0].0.clone()], "font-size:18px");
    assert_eq!(graph.warnings[0].1, "ignored_style_property:font-size");
}
