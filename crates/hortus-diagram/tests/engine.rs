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
fn subgraphs_are_reported_as_unsupported_and_unknown_syntax_keeps_byte_ranges() {
    let error = parse("flowchart TB\nsubgraph Group\nA --> B\nend").unwrap_err();
    assert!(matches!(
        error,
        DiagramError::Unsupported { diagram_type }
            if diagram_type == "subgraph"
    ));

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
fn markdown_fences_and_absolute_source_mapping() {
    let markdown = "# Note\r\n````text\r\n```mermaid\r\nignored\r\n````\r\n  ~~~mermaid\r\nflowchart TB\r\nA[Root] --> B[Leaf]\r\n  ~~~~\r\n```mermaid\nmindmap\n  Root\n    Child\n";
    let first = mermaid_block(markdown, 1).unwrap();
    let tree = parse_at(first.source, first.source_range.start).unwrap();
    assert_eq!(&markdown[tree.nodes[0].source_range.clone()], "A[Root] ");
    assert_eq!(
        parse(mermaid_block(markdown, 2).unwrap().source)
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
        let tree = parse(mermaid_block(&markdown, 1).unwrap().source).unwrap();
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
        let tree = parse(mermaid_block(&markdown, 1).unwrap().source).unwrap();
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
