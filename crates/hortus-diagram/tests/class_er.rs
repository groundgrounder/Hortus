use hortus_diagram::{
    ClassMarker, ClassMemberKind, ClassVisibility, DeterministicMetrics, Diagram, DiagramError,
    Direction, EdgeLine, ErCardinality, ErKey, LayoutOptions, Point, Primitive, Scene, Theme,
    layout_class, layout_er, mermaid_block, mermaid_blocks, parse_diagram, parse_diagram_at,
};
use std::collections::HashSet;

const DOMAIN: &str = include_str!("../../../fixtures/mermaid/class/domain-model.mmd");
const NAMESPACES: &str = include_str!("../../../fixtures/mermaid/class/namespaces-lr.mmd");
const ORDER_SCHEMA: &str = include_str!("../../../fixtures/mermaid/er/order-schema.mmd");
const INVENTORY: &str = include_str!("../../../fixtures/mermaid/er/inventory-lr.mmd");

fn class_diagram(source: &str) -> hortus_diagram::ClassDiagram {
    match parse_diagram(source).unwrap() {
        Diagram::Class(diagram) => diagram,
        other => panic!("expected class diagram, got {other:?}"),
    }
}

fn er_diagram(source: &str) -> hortus_diagram::ErDiagram {
    match parse_diagram(source).unwrap() {
        Diagram::Er(diagram) => diagram,
        other => panic!("expected ER diagram, got {other:?}"),
    }
}

fn scene(source: &str, width: f64, theme: Theme) -> Scene {
    let options = LayoutOptions {
        width,
        theme,
        ..LayoutOptions::default()
    };
    match parse_diagram(source).unwrap() {
        Diagram::Class(diagram) => {
            layout_class(&diagram, &options, &mut DeterministicMetrics).unwrap()
        }
        Diagram::Er(diagram) => layout_er(&diagram, &options, &mut DeterministicMetrics).unwrap(),
        other => panic!("unexpected diagram {other:?}"),
    }
}

fn assert_ranges(source: &str, ranges: impl IntoIterator<Item = std::ops::Range<usize>>) {
    for range in ranges {
        assert!(range.start <= range.end, "{range:?}");
        assert!(range.end <= source.len(), "{range:?} > {}", source.len());
        assert!(source.is_char_boundary(range.start), "{range:?}");
        assert!(source.is_char_boundary(range.end), "{range:?}");
    }
}

#[test]
fn class_declarations_members_stereotypes_and_generics() {
    let diagram = class_diagram(
        "classDiagram-v2\ndirection LR\ntitle 支付流程\nclass Order {\n  <<abstract>>\n  +id: UUID\n  #status: Status\n  +instance$: Shared\n  -recalculate() void\n  ~cache*() int\n}\nclass Repo~T~\nclass Storage[\"持久化\"]\n<<interface>> Port\nOrder : 提交() bool\n",
    );
    assert_eq!(diagram.direction, Direction::LeftToRight);
    assert_eq!(diagram.title.as_deref(), Some("支付流程"));
    let order = diagram
        .classes
        .iter()
        .find(|class| class.id == "Order")
        .unwrap();
    assert_eq!(order.stereotype.as_deref(), Some("abstract"));
    assert_eq!(order.fields.len(), 3);
    assert_eq!(order.methods.len(), 3);
    assert_eq!(order.fields[0].visibility, ClassVisibility::Public);
    assert_eq!(order.fields[1].visibility, ClassVisibility::Protected);
    assert!(order.fields[2].static_member && !order.fields[2].abstract_member);
    assert_eq!(order.methods[0].visibility, ClassVisibility::Private);
    assert_eq!(order.methods[1].visibility, ClassVisibility::Package);
    assert!(order.methods[1].abstract_member && !order.methods[1].static_member);
    assert_eq!(order.methods[1].kind, ClassMemberKind::Method);
    assert_eq!(order.methods[0].text, "-recalculate() void");
    let repo = diagram
        .classes
        .iter()
        .find(|class| class.id == "Repo")
        .unwrap();
    assert_eq!(repo.label, "Repo<T>");
    let storage = diagram
        .classes
        .iter()
        .find(|class| class.id == "Storage")
        .unwrap();
    assert_eq!(storage.label, "持久化");
    let port = diagram
        .classes
        .iter()
        .find(|class| class.id == "Port")
        .unwrap();
    assert_eq!(port.stereotype.as_deref(), Some("interface"));
    assert_eq!(diagram.classes.len(), 4);
}

#[test]
fn class_relations_cover_all_markers_lines_and_cardinalities() {
    let source = "classDiagram\nA <|-- B\nB --|> C\nD <|.. E\nF ..|> G\nH *-- I\nJ --* K\nL o-- M\nN --o O\nP --> Q\nR <-- S\nT <--> U\nV ..> W\nX <.. Y\nZ -- AA\nAB .. AC\nOrder \"1\" *-- \"1..*\" LineItem : contains\nNode --> Node : parent\n";
    let diagram = class_diagram(source);
    let relation = |index: usize| &diagram.relations[index];
    assert_eq!(
        (relation(0).start, relation(0).end, relation(0).line),
        (ClassMarker::Triangle, ClassMarker::None, EdgeLine::Solid)
    );
    assert_eq!(
        (relation(1).start, relation(1).end, relation(1).line),
        (ClassMarker::None, ClassMarker::Triangle, EdgeLine::Solid)
    );
    assert_eq!(
        (relation(2).start, relation(2).end, relation(2).line),
        (ClassMarker::Triangle, ClassMarker::None, EdgeLine::Dotted)
    );
    assert_eq!(
        (relation(3).start, relation(3).end, relation(3).line),
        (ClassMarker::None, ClassMarker::Triangle, EdgeLine::Dotted)
    );
    assert_eq!(
        (relation(4).start, relation(4).end),
        (ClassMarker::DiamondFilled, ClassMarker::None)
    );
    assert_eq!(
        (relation(5).start, relation(5).end),
        (ClassMarker::None, ClassMarker::DiamondFilled)
    );
    assert_eq!(
        (relation(6).start, relation(6).end),
        (ClassMarker::DiamondHollow, ClassMarker::None)
    );
    assert_eq!(
        (relation(7).start, relation(7).end),
        (ClassMarker::None, ClassMarker::DiamondHollow)
    );
    assert_eq!(
        (relation(8).start, relation(8).end),
        (ClassMarker::None, ClassMarker::Arrow)
    );
    assert_eq!(
        (relation(9).start, relation(9).end),
        (ClassMarker::Arrow, ClassMarker::None)
    );
    assert_eq!(
        (relation(10).start, relation(10).end),
        (ClassMarker::Arrow, ClassMarker::Arrow)
    );
    assert_eq!(
        (relation(11).start, relation(11).end, relation(11).line),
        (ClassMarker::None, ClassMarker::Arrow, EdgeLine::Dotted)
    );
    assert_eq!(
        (relation(12).start, relation(12).end, relation(12).line),
        (ClassMarker::Arrow, ClassMarker::None, EdgeLine::Dotted)
    );
    assert_eq!(relation(13).line, EdgeLine::Solid);
    assert_eq!(relation(14).line, EdgeLine::Dotted);
    let composed = relation(15);
    assert_eq!(composed.from_cardinality.as_deref(), Some("1"));
    assert_eq!(composed.to_cardinality.as_deref(), Some("1..*"));
    assert_eq!(composed.label.as_deref(), Some("contains"));
    assert_eq!(&source[composed.label_range.clone().unwrap()], "contains");
    let self_relation = relation(16);
    assert_eq!(self_relation.from, self_relation.to);
    assert_eq!(self_relation.label.as_deref(), Some("parent"));
}

#[test]
fn class_namespaces_notes_and_styles() {
    let diagram = class_diagram(NAMESPACES);
    assert_eq!(diagram.direction, Direction::LeftToRight);
    assert_eq!(diagram.namespaces.len(), 2);
    assert_eq!(diagram.namespaces[0].id, "Core");
    let gateway = diagram
        .classes
        .iter()
        .find(|class| class.id == "Gateway")
        .unwrap();
    assert_eq!(
        gateway
            .namespace
            .map(|index| diagram.namespaces[index].id.as_str()),
        Some("Core")
    );
    assert_eq!(diagram.notes.len(), 1);
    assert_eq!(
        diagram.notes[0].target,
        Some(
            diagram
                .classes
                .iter()
                .position(|class| class.id == "Gateway")
                .unwrap()
        )
    );
    let http = diagram
        .classes
        .iter()
        .find(|class| class.id == "HttpAdapter")
        .unwrap();
    assert_eq!(http.class.as_deref(), Some("edge"));
    let style = diagram.class_defs.get("edge").unwrap();
    assert!(style.fill.is_some() && style.stroke.is_some() && style.color.is_some());

    let styled = class_diagram(
        "classDiagram\nclass A\nclass B\nclass C\nclassDef key fill:#abc,stroke:#c33\nclass A,B key\ncssClass \"C\" key\nD:::key\nE\n",
    );
    assert_eq!(styled.classes[0].class.as_deref(), Some("key"));
    assert_eq!(styled.classes[1].class.as_deref(), Some("key"));
    assert_eq!(styled.classes[2].class.as_deref(), Some("key"));
    assert_eq!(styled.classes[3].class.as_deref(), Some("key"));

    let notes = class_diagram("classDiagram\nA\nnote \"全局说明\"\nnote for A \"附加说明\"\n");
    assert_eq!(notes.notes.len(), 2);
    assert_eq!(notes.notes[0].target, None);
    assert_eq!(notes.notes[0].text, "全局说明");
    assert_eq!(notes.notes[1].target, Some(0));
}

#[test]
fn class_ranges_survive_utf8_offsets_and_container_fences() {
    let source = "序😀\n\n> ~~~mermaid\n> classDiagram\n> class Doğrulama {\n>   +kontrol() bool\n> }\n> Doğrulama \"1\" --> \"1..*\" Kayıt : 记录\n> ~~~\n";
    let block = mermaid_block(source, 1).unwrap();
    assert!(block.container);
    let diagram = match block.parse_diagram().unwrap() {
        Diagram::Class(diagram) => diagram,
        other => panic!("{other:?}"),
    };
    let mut ranges = Vec::new();
    for class in &diagram.classes {
        ranges.push(class.source_range.clone());
        ranges.extend(
            class
                .fields
                .iter()
                .map(|member| member.source_range.clone()),
        );
        ranges.extend(
            class
                .methods
                .iter()
                .map(|member| member.source_range.clone()),
        );
    }
    for relation in &diagram.relations {
        ranges.push(relation.source_range.clone());
        ranges.push(relation.connector_range.clone());
        ranges.extend(relation.label_range.clone());
    }
    assert_ranges(source, ranges);
    let class = diagram
        .classes
        .iter()
        .find(|class| class.id == "Doğrulama")
        .unwrap();
    assert_eq!(&source[class.source_range.clone()], "class Doğrulama {");
    assert_eq!(
        &source[class.methods[0].source_range.clone()],
        "+kontrol() bool"
    );
    let relation = &diagram.relations[0];
    assert_eq!(&source[relation.label_range.clone().unwrap()], "记录");
    assert_eq!(&source[relation.connector_range.clone()], "-->");
    let implicit = diagram
        .classes
        .iter()
        .find(|class| class.id == "Kayıt")
        .unwrap();
    assert_eq!(&source[implicit.source_range.clone()], "Kayıt");

    let shifted = parse_diagram_at("classDiagram\nA --> B", 17).unwrap();
    let Diagram::Class(shifted) = shifted else {
        panic!("class")
    };
    assert!(
        shifted
            .classes
            .iter()
            .all(|class| { class.source_range.start >= 17 && class.source_range.end <= 17 + 21 })
    );
}

#[test]
fn class_errors_are_syntax_with_utf8_ranges() {
    let cases = [
        ("classDiagram", "class_empty"),
        ("classDiagram\nclass A {", "class_unclosed_block"),
        ("classDiagram\nA --> B\n}", "class_unexpected_close"),
        ("classDiagram\nA -->", "class_invalid_relation"),
        ("classDiagram\ndirection XX", "class_invalid_direction"),
        ("classDiagram\nnamespace N {", "class_unclosed_namespace"),
        ("classDiagram\nnote A", "class_invalid_note"),
    ];
    for (source, kind) in cases {
        let error = parse_diagram(source).unwrap_err();
        let DiagramError::Syntax {
            range,
            kind: actual,
        } = error
        else {
            panic!("{source:?}: {error:?}");
        };
        assert_eq!(actual, kind, "{source:?}");
        assert!(range.start <= range.end && range.end <= source.len());
        assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
    }
    let error = parse_diagram("classDiagram\nA -->").unwrap_err();
    assert_eq!(error.to_string().split(':').next(), Some("Syntax (13..18)"));
}

#[test]
fn er_entities_attributes_keys_aliases_and_ranges() {
    let diagram = er_diagram(ORDER_SCHEMA);
    assert_eq!(diagram.title.as_deref(), Some("订单库表"));
    assert_eq!(diagram.entities.len(), 4);
    let customer = diagram
        .entities
        .iter()
        .find(|entity| entity.id == "CUSTOMER")
        .unwrap();
    assert_eq!(customer.label, "CUSTOMER");
    assert_eq!(customer.attributes.len(), 3);
    assert_eq!(customer.attributes[0].kind, "bigint");
    assert_eq!(customer.attributes[0].name, "id");
    assert_eq!(customer.attributes[0].keys, vec![ErKey::Primary]);
    assert_eq!(customer.attributes[0].comment.as_deref(), Some("主键"));
    assert_eq!(customer.attributes[1].kind, "varchar(120)");
    assert_eq!(customer.attributes[2].keys, vec![ErKey::Unique]);
    let item = diagram
        .entities
        .iter()
        .find(|entity| entity.id == "ORDER-ITEM")
        .unwrap();
    assert_eq!(
        item.attributes[0].keys,
        vec![ErKey::Primary, ErKey::Foreign]
    );
    assert_eq!(diagram.relations.len(), 4);
    assert!(diagram.relations[0].identifying);
    assert!(!diagram.relations[2].identifying);
    let mut ranges = Vec::new();
    for entity in &diagram.entities {
        ranges.push(entity.source_range.clone());
        ranges.extend(
            entity
                .attributes
                .iter()
                .map(|attribute| attribute.source_range.clone()),
        );
    }
    for relation in &diagram.relations {
        ranges.push(relation.source_range.clone());
        ranges.push(relation.connector_range.clone());
        ranges.extend(relation.label_range.clone());
    }
    assert_ranges(ORDER_SCHEMA, ranges);
    let relation = &diagram.relations[0];
    assert_eq!(
        &ORDER_SCHEMA[relation.label_range.clone().unwrap()],
        "places"
    );
    assert_eq!(&ORDER_SCHEMA[relation.connector_range.clone()], "||--o{");

    let aliased = er_diagram(
        "erDiagram\nUSER[\"用户\"] {\n  bigint id PK\n}\nORDER-ITEM[\"行项目\"] {}\nEMPTY {}\nUSER ||--o{ ORDER-ITEM : 下单\n",
    );
    assert_eq!(aliased.entities[0].label, "用户");
    assert_eq!(aliased.entities[1].label, "行项目");
    assert!(aliased.entities[1].attributes.is_empty());
    assert!(aliased.entities[2].attributes.is_empty());
}

#[test]
fn er_cardinalities_lines_aliases_and_empty_labels() {
    let source = "erDiagram\nA ||--|| B : one\nB |o..o| C : zero\nC }|--|{ D : many\nD }o..o{ E : zero-many\nE one or zero -- only one F : aliases\nF zero or more .. one or more G\nH ||--o{ H : self\nI ||--o{ J : \"\"\n";
    let diagram = er_diagram(source);
    let relation = |index: usize| &diagram.relations[index];
    assert_eq!(relation(0).from_cardinality, ErCardinality::ExactlyOne);
    assert_eq!(relation(0).to_cardinality, ErCardinality::ExactlyOne);
    assert!(relation(0).identifying);
    assert_eq!(relation(1).from_cardinality, ErCardinality::ZeroOrOne);
    assert_eq!(relation(1).to_cardinality, ErCardinality::ZeroOrOne);
    assert!(!relation(1).identifying);
    assert_eq!(relation(2).from_cardinality, ErCardinality::OneOrMore);
    assert_eq!(relation(2).to_cardinality, ErCardinality::OneOrMore);
    assert_eq!(relation(3).from_cardinality, ErCardinality::ZeroOrMore);
    assert_eq!(relation(3).to_cardinality, ErCardinality::ZeroOrMore);
    assert_eq!(relation(4).from_cardinality, ErCardinality::ZeroOrOne);
    assert_eq!(relation(4).to_cardinality, ErCardinality::ExactlyOne);
    assert_eq!(relation(5).from_cardinality, ErCardinality::ZeroOrMore);
    assert_eq!(relation(5).to_cardinality, ErCardinality::OneOrMore);
    assert_eq!(relation(6).from, relation(6).to);
    assert_eq!(relation(7).label.as_deref(), Some(""));
    let implicit = diagram
        .entities
        .iter()
        .find(|entity| entity.id == "H")
        .unwrap();
    assert_eq!(&source[implicit.source_range.clone()], "H");
}

#[test]
fn er_errors_are_syntax_with_utf8_ranges() {
    let cases = [
        ("erDiagram", "er_empty"),
        ("erDiagram\nUSER {", "er_unclosed_block"),
        ("erDiagram\nUSER {}\n}", "er_unexpected_close"),
        ("erDiagram\nA xx--yy B", "er_invalid_relation"),
        ("erDiagram\nUSER {\nbigint\n}", "er_invalid_attribute"),
        ("erDiagram\ndirection XX", "er_invalid_direction"),
    ];
    for (source, kind) in cases {
        let error = parse_diagram(source).unwrap_err();
        let DiagramError::Syntax {
            range,
            kind: actual,
        } = error
        else {
            panic!("{source:?}: {error:?}");
        };
        assert_eq!(actual, kind, "{source:?}");
        assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
    }
}

fn check_layout(source: &str, kind: &str, widths: &[f64]) {
    for &width in widths {
        for theme in [Theme::Light, Theme::Dark] {
            let scene = scene(source, width, theme);
            assert_eq!(scene.width, width);
            assert_eq!(scene.metrics.diagram_kind, kind);
            assert_eq!(scene.metrics.node_overlaps, 0, "{kind} {width} {theme:?}");
            assert!(
                !scene.metrics.horizontal_overflow,
                "{kind} {width} {theme:?}"
            );
            assert!(
                scene.metrics.min_font_px >= 12.5,
                "{kind} {width} {theme:?}"
            );
            assert!(!scene.hit_regions.is_empty());
            let ids = scene
                .hit_regions
                .iter()
                .map(|hit| hit.node_id.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                ids.iter().collect::<HashSet<_>>().len(),
                ids.len(),
                "{kind} {width} {theme:?}: duplicate node ids"
            );
            assert!(scene.hit_regions.iter().all(|hit| !hit.can_collapse));
            for hit in &scene.hit_regions {
                assert!(scene.height + 0.001 >= hit.rect.y + hit.rect.height);
                assert!(scene.width + 0.001 >= hit.rect.x + hit.rect.width);
            }
            let paths = scene
                .primitives
                .iter()
                .filter(|primitive| matches!(primitive, Primitive::GraphPath { .. }))
                .count();
            assert!(paths >= 1, "{kind} {width} {theme:?}");
            assert!(
                scene
                    .primitives
                    .iter()
                    .any(|primitive| matches!(primitive, Primitive::LabelBackground { .. }))
            );
            assert_svg(&scene, kind);
        }
    }
}

fn assert_svg(scene: &Scene, kind: &str) {
    let svg = scene.to_svg();
    let label = if kind == "class" {
        "Mermaid class diagram"
    } else {
        "Mermaid entity-relationship diagram"
    };
    assert!(svg.contains(&format!("aria-label=\"{label}\"")), "{svg}");
    assert_eq!(
        svg.matches("<text ").count(),
        scene
            .primitives
            .iter()
            .filter(|primitive| matches!(primitive, Primitive::Text(_)))
            .count()
    );
}

#[test]
fn class_fixtures_layout_without_overlap_or_overflow() {
    for source in [DOMAIN, NAMESPACES] {
        check_layout(source, "class", &[700.0, 1100.0]);
    }
    let class_scene = scene(DOMAIN, 700.0, Theme::Light);
    assert!(
        class_scene
            .hit_regions
            .iter()
            .any(|hit| hit.node_id == "class-Repository" && hit.label == "Repository<T>")
    );
    assert!(
        class_scene
            .hit_regions
            .iter()
            .any(|hit| hit.node_id == "note-0")
    );
    let namespaces = scene(NAMESPACES, 700.0, Theme::Light);
    assert!(
        namespaces
            .hit_regions
            .iter()
            .any(|hit| hit.node_id == "namespace-Core")
    );
    assert!(
        namespaces
            .hit_regions
            .iter()
            .any(|hit| hit.node_id == "namespace-Adapters")
    );
}

#[test]
fn er_fixtures_layout_without_overlap_or_overflow() {
    for source in [ORDER_SCHEMA, INVENTORY] {
        check_layout(source, "er", &[700.0, 1100.0]);
    }
    let er_scene = scene(ORDER_SCHEMA, 700.0, Theme::Light);
    for id in ["CUSTOMER", "ORDERS", "ORDER-ITEM", "INVOICE"] {
        assert!(
            er_scene
                .hit_regions
                .iter()
                .any(|hit| hit.node_id == format!("entity-{id}"))
        );
    }
    assert_eq!(
        er_scene
            .hit_regions
            .iter()
            .filter(|hit| hit.node_id.starts_with("relation-"))
            .count(),
        4
    );
    let shapes = er_scene
        .primitives
        .iter()
        .filter(|primitive| matches!(primitive, Primitive::GraphShape { .. }))
        .count();
    assert!(shapes > 20, "crow's foot markers missing: {shapes}");
}

#[test]
fn class_and_er_mermaid_blocks_project_into_notes() {
    let note = "前言😀\n\n```mermaid\nclassDiagram\nOrder \"1\" *-- \"1..*\" LineItem : contains\n```\n\n```mermaid\nerDiagram\nUSER ||--o{ ORDERS : places\n```\n";
    let blocks = mermaid_blocks(note);
    assert_eq!(blocks.len(), 2);
    let class = match blocks[0].parse_diagram().unwrap() {
        Diagram::Class(class) => class,
        other => panic!("{other:?}"),
    };
    assert!(note.is_char_boundary(class.classes[0].source_range.start));
    assert_eq!(
        &note[class.relations[0].label_range.clone().unwrap()],
        "contains"
    );
    let er = match blocks[1].parse_diagram().unwrap() {
        Diagram::Er(er) => er,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        &note[er.relations[0].label_range.clone().unwrap()],
        "places"
    );
    assert_eq!(&note[er.relations[0].connector_range.clone()], "||--o{");
}

#[test]
fn diagram_header_inference_and_kinds() {
    assert_eq!(
        hortus_diagram::infer_diagram_header("%% 注释\n%%{init: {}}%%\nclassDiagram\nA"),
        Some("classDiagram")
    );
    assert_eq!(
        hortus_diagram::infer_diagram_header("erDiagram\nA ||--|| B : x"),
        Some("erDiagram")
    );
    assert!(matches!(
        parse_diagram("classDiagram\nA --> B").unwrap(),
        Diagram::Class(_)
    ));
    assert!(matches!(
        parse_diagram("erDiagram\nA ||--|| B : x").unwrap(),
        Diagram::Er(_)
    ));
    for source in ["gantt\ntitle x", "pie\n\"A\": 1"] {
        assert!(matches!(
            parse_diagram(source),
            Err(DiagramError::Unsupported { .. })
        ));
    }
}

#[test]
fn arrow_markers_are_vector_primitives_with_expected_orientation() {
    let scene = scene(
        "classDiagram\nA <|-- B\nC *-- D\nE o-- F\nG --> H\n",
        700.0,
        Theme::Light,
    );
    let mut triangles = 0;
    let mut filled = 0;
    let mut hollow = 0;
    let mut arrows = 0;
    for primitive in &scene.primitives {
        if let Primitive::GraphShape { outline, fill, .. } = primitive {
            match outline.len() {
                3 if fill.a > 0.0 => arrows += 1,
                3 => triangles += 1,
                4 if fill.a > 0.0 => filled += 1,
                4 => hollow += 1,
                _ => {}
            }
        }
    }
    assert_eq!((triangles, filled, hollow, arrows), (1, 1, 1, 1));
    let path_markers = scene
        .primitives
        .iter()
        .filter(|primitive| {
            matches!(
                primitive,
                Primitive::GraphPath {
                    end: hortus_diagram::Marker::None,
                    ..
                }
            )
        })
        .count();
    assert_eq!(path_markers, 4);
    let point = Point { x: 0.0, y: 0.0 };
    assert!(scene.hit_test(point).is_none());
}
