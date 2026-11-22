use hortus_diagram::{
    DeterministicMetrics, Diagram, DiagramError, Direction, LayoutOptions, Marker, Point,
    Primitive, Rect, Shape, StateKind, StateNoteSide, StateRegion, Theme, layout_state,
    mermaid_blocks, parse_diagram, parse_diagram_at,
};
use std::ops::Range;

const LIFECYCLE: &str = include_str!("../../../fixtures/mermaid/state/lifecycle.mmd");
const COMPOSITE: &str = include_str!("../../../fixtures/mermaid/state/composite.mmd");
const ANNOTATED: &str = include_str!("../../../fixtures/mermaid/state/annotated-lr.mmd");

#[test]
fn parses_regions_pseudostates_notes_and_styles() {
    let Diagram::State(diagram) = parse_diagram(COMPOSITE).unwrap() else {
        panic!("state")
    };
    let node = diagram
        .root
        .nodes
        .iter()
        .find(|node| node.id == "Commit")
        .unwrap();
    let StateKind::Composite(regions) = &node.kind else {
        panic!("composite")
    };
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].direction, Direction::TopToBottom);
    assert_eq!(regions[0].nodes[0].kind, StateKind::Start);
    assert!(
        regions[0]
            .nodes
            .iter()
            .any(|node| node.kind == StateKind::Fork)
    );
    assert!(
        regions[0]
            .nodes
            .iter()
            .any(|node| node.kind == StateKind::Join)
    );
    assert!(
        regions[1]
            .nodes
            .iter()
            .any(|node| node.kind == StateKind::End)
    );
    let nested = regions[0]
        .nodes
        .iter()
        .find(|node| node.id == "Quorum")
        .unwrap();
    let StateKind::Composite(inner) = &nested.kind else {
        panic!("nested composite")
    };
    assert!(
        inner[0]
            .nodes
            .iter()
            .any(|node| node.kind == StateKind::Choice)
    );
    let end = inner[0]
        .nodes
        .iter()
        .position(|node| node.kind == StateKind::End)
        .unwrap();
    assert_eq!(
        inner[0]
            .transitions
            .iter()
            .filter(|edge| edge.to == end)
            .count(),
        2
    );

    let Diagram::State(annotated) = parse_diagram(ANNOTATED).unwrap() else {
        panic!("state")
    };
    assert_eq!(annotated.root.direction, Direction::LeftToRight);
    assert_eq!(annotated.title.as_deref(), Some("DMA 描述符队列"));
    assert_eq!(annotated.root.notes[0].side, StateNoteSide::Left);
    assert_eq!(annotated.root.notes[1].side, StateNoteSide::Right);
    assert!(annotated.root.notes[1].text.contains('\n'));
    assert!(
        annotated
            .root
            .nodes
            .iter()
            .any(|node| node.label.contains("<br/>"))
    );
    assert_eq!(
        annotated
            .root
            .nodes
            .iter()
            .find(|node| node.id == "Dispatch")
            .unwrap()
            .class
            .as_deref(),
        Some("busy")
    );
    assert_eq!(
        annotated
            .root
            .nodes
            .iter()
            .find(|node| node.id == "Reap")
            .unwrap()
            .class
            .as_deref(),
        Some("ring")
    );
    let style = &annotated.class_defs["busy"];
    assert_eq!(style.fill.as_deref(), Some("#c1e8ff"));
    assert_eq!(style.stroke.as_deref(), Some("#3485ae"));
    assert_eq!(style.color.as_deref(), Some("#154b65"));

    let Diagram::State(lifecycle) = parse_diagram(LIFECYCLE).unwrap() else {
        panic!("state")
    };
    assert_eq!(lifecycle.root.nodes[1].descriptions.len(), 2);
    assert!(
        lifecycle
            .root
            .transitions
            .iter()
            .any(|edge| edge.from == edge.to)
    );
    assert!(lifecycle.root.transitions.iter().any(|edge| {
        lifecycle
            .root
            .transitions
            .iter()
            .any(|other| edge.from == other.to && edge.to == other.from && edge.from != edge.to)
    }));
}

fn check_ranges(
    region: &StateRegion,
    expected: &StateRegion,
    source: &str,
    check: &impl Fn(Range<usize>, Range<usize>),
) {
    for (node, original) in region.nodes.iter().zip(&expected.nodes) {
        assert!(source.get(node.source_range.clone()).is_some());
        check(node.source_range.clone(), original.source_range.clone());
        if let (StateKind::Composite(regions), StateKind::Composite(before)) =
            (&node.kind, &original.kind)
        {
            for (child, old) in regions.iter().zip(before) {
                check_ranges(child, old, source, check);
            }
        }
    }
    for (edge, original) in region.transitions.iter().zip(&expected.transitions) {
        assert!(source.get(edge.source_range.clone()).is_some());
        check(edge.source_range.clone(), original.source_range.clone());
        if let Some(range) = &edge.label_range {
            assert!(source.get(range.clone()).is_some());
            check(range.clone(), original.label_range.clone().unwrap());
        }
    }
    for (note, original) in region.notes.iter().zip(&expected.notes) {
        assert!(source.get(note.source_range.clone()).is_some());
        check(note.source_range.clone(), original.source_range.clone());
    }
}

#[test]
fn projects_utf8_ranges_through_offsets_quotes_lists_and_crlf() {
    for text in [LIFECYCLE, COMPOSITE, ANNOTATED] {
        let Diagram::State(relative) = parse_diagram(text).unwrap() else {
            panic!("state")
        };
        let prefix = "序😀";
        let source = format!("{prefix}{text}");
        let Diagram::State(offset) = parse_diagram_at(text, prefix.len()).unwrap() else {
            panic!("state")
        };
        check_ranges(&offset.root, &relative.root, &source, &|range, original| {
            assert_eq!(
                range,
                original.start + prefix.len()..original.end + prefix.len()
            );
        });
        for (open, indent) in [("> ~~~mermaid", "> "), ("- ~~~mermaid", "  ")] {
            let mut markdown = format!("{prefix}\r\n\r\n{open}\r\n");
            for line in text.lines() {
                markdown.push_str(&format!("{indent}{line}\r\n"));
            }
            markdown.push_str(&format!("{indent}~~~\r\n"));
            let blocks = mermaid_blocks(&markdown);
            assert!(blocks[0].container);
            let Diagram::State(projected) = blocks[0].parse_diagram().unwrap() else {
                panic!("state")
            };
            check_ranges(
                &projected.root,
                &relative.root,
                &markdown,
                &|range, original| {
                    assert_eq!(
                        markdown[range].replace(&format!("\r\n{indent}"), "\n"),
                        text[original]
                    );
                },
            );
        }
    }
}

#[test]
fn accepts_compact_composites_and_multiline_directives() {
    let source =
        "%%{init:\n {}\n}%%\n;stateDiagram-v2; state Outer { state Inner { [*]-->RX; RX-->[*] } }";
    let Diagram::State(diagram) = parse_diagram(source).unwrap() else {
        panic!("state")
    };
    let outer = &diagram.root.nodes[0];
    assert_eq!(
        &source[outer.source_range.clone()],
        "state Outer { state Inner { [*]-->RX; RX-->[*] } }"
    );
    let StateKind::Composite(children) = &outer.kind else {
        panic!("outer")
    };
    let StateKind::Composite(inner) = &children[0].nodes[0].kind else {
        panic!("inner")
    };
    assert_eq!(inner[0].nodes.len(), 3);
    assert_eq!(inner[0].transitions.len(), 2);
}

#[test]
fn malformed_state_statements_report_the_offending_utf8_line() {
    for (tail, offending, want) in [
        ("", "stateDiagram-v2", "state_empty"),
        (
            "state 事务 {\n[*] --> B",
            "state 事务 {",
            "state_unclosed_composite",
        ),
        ("state 事务\n}", "}", "state_unexpected_close"),
        (
            "note left of 队列\n 内容😀",
            "note left of 队列",
            "state_unclosed_note",
        ),
        (
            "状态 --> --> B",
            "状态 --> --> B",
            "state_invalid_transition",
        ),
        ("状态 -> B", "状态 -> B", "state_invalid_statement"),
    ] {
        let source = format!("%% prelude\r\nstateDiagram-v2\r\n{tail}");
        let Err(DiagramError::Syntax { range, kind }) = parse_diagram(&source) else {
            panic!("{tail}")
        };
        assert_eq!(&source[range], offending);
        assert_eq!(kind, want);
    }
}

#[test]
fn all_directions_mirror_rank_positions() {
    for (direction, horizontal, reversed) in [
        ("TB", false, false),
        ("TD", false, false),
        ("BT", false, true),
        ("LR", true, false),
        ("RL", true, true),
    ] {
        let text = format!(
            "stateDiagram\ndirection {direction}\nstate Option [[choice]]\nA --> Option :\nOption --> B"
        );
        let Diagram::State(diagram) = parse_diagram(&text).unwrap() else {
            panic!("state")
        };
        assert_eq!(diagram.root.nodes[0].kind, StateKind::Choice);
        assert_eq!(diagram.root.transitions[0].label.as_deref(), Some(""));
        let scene = layout_state(
            &diagram,
            &LayoutOptions::default(),
            &mut DeterministicMetrics,
        )
        .unwrap();
        let point = |id| {
            scene
                .hit_regions
                .iter()
                .find(|node| node.node_id == id)
                .unwrap()
                .rect
        };
        let a = point("state-A");
        let b = point("state-B");
        let coord = |rect: Rect| if horizontal { rect.x } else { rect.y };
        assert_eq!(coord(a) > coord(b), reversed, "{direction}");
    }
}

fn crosses(a: Point, b: Point, r: Rect) -> bool {
    let inside = |v, low, length| v > low + 0.01 && v < low + length - 0.01;
    if (a.x - b.x).abs() < 0.001 {
        inside(a.x, r.x, r.width)
            && a.y.min(b.y) < r.y + r.height - 0.01
            && a.y.max(b.y) > r.y + 0.01
    } else {
        assert!((a.y - b.y).abs() < 0.001);
        inside(a.y, r.y, r.height)
            && a.x.min(b.x) < r.x + r.width - 0.01
            && a.x.max(b.x) > r.x + 0.01
    }
}

#[test]
fn fixture_scenes_preserve_geometry_and_regions_in_both_themes() {
    for (name, text) in [
        ("lifecycle", LIFECYCLE),
        ("composite", COMPOSITE),
        ("annotated", ANNOTATED),
    ] {
        let Diagram::State(diagram) = parse_diagram(text).unwrap() else {
            panic!("state")
        };
        for width in [700.0, 1100.0] {
            for theme in [Theme::Light, Theme::Dark] {
                let options = LayoutOptions {
                    width,
                    theme,
                    ..LayoutOptions::default()
                };
                let scene = layout_state(&diagram, &options, &mut DeterministicMetrics)
                    .unwrap_or_else(|error| panic!("{name} {width} {theme:?}: {error}"));
                assert_eq!(scene.metrics.columns, 1);
                assert!(!scene.metrics.horizontal_overflow, "{name}");
                assert_eq!(scene.metrics.node_overlaps, 0);
                assert!(scene.metrics.min_font_px >= 12.5);
                assert!(
                    scene
                        .to_svg()
                        .contains("aria-label=\"Mermaid state diagram\"")
                );
                assert!(
                    scene.hit_regions.iter().all(
                        |hit| !hit.can_collapse && text.get(hit.source_range.clone()).is_some()
                    )
                );
                let shapes: Vec<_> = scene
                    .primitives
                    .iter()
                    .filter_map(|primitive| match primitive {
                        Primitive::GraphShape { rect, shape, .. } => Some((*rect, *shape)),
                        _ => None,
                    })
                    .collect();
                let labels: Vec<_> = scene
                    .primitives
                    .iter()
                    .filter_map(|primitive| match primitive {
                        Primitive::LabelBackground { rect, .. } => Some(*rect),
                        _ => None,
                    })
                    .collect();
                for (index, label) in labels.iter().enumerate() {
                    assert!(
                        labels[..index]
                            .iter()
                            .all(|other| !label.intersects(*other))
                    );
                    assert!(shapes.iter().all(
                        |(rect, shape)| *shape == Shape::Container || !label.intersects(*rect)
                    ));
                }
                if name == "lifecycle" {
                    let loop_path = scene
                        .primitives
                        .iter()
                        .find_map(|primitive| match primitive {
                            Primitive::GraphPath {
                                points,
                                source_range,
                                ..
                            } if text[source_range.clone()].starts_with("Lease --> Lease") => {
                                Some(points)
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(loop_path.len(), 4);
                    assert!((loop_path[0].x - loop_path[3].x).abs() < 1.0);
                    assert!((loop_path[0].y - loop_path[3].y).abs() > 10.0);
                }
                for primitive in &scene.primitives {
                    if let Primitive::GraphPath {
                        points,
                        end: Marker::Arrow,
                        ..
                    } = primitive
                    {
                        assert!(points.len() >= 2);
                        for (r, shape) in &shapes {
                            if *shape == Shape::Container
                                || r.contains(points[0])
                                || r.contains(points[points.len() - 1])
                            {
                                continue;
                            }
                            assert!(
                                !points.windows(2).any(|p| crosses(p[0], p[1], *r)),
                                "{name}: {points:?} through {r:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
