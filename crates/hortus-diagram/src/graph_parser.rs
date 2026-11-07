use crate::Color;
use crate::graph::{
    Connector, Direction, Edge, EdgeLine, Graph, GraphContainer, GraphNode, GraphNodeKind, Marker,
    Shape, Style,
};
use crate::parser::{DiagramError, syntax, unsupported};
use std::collections::HashMap;
use std::ops::Range;

pub(crate) fn parse(source: &str) -> Result<Graph, DiagramError> {
    parse_mode(source, false)
}

pub(crate) fn tree_probe(source: &str) -> Result<Graph, DiagramError> {
    parse_mode(source, true)
}

fn parse_mode(source: &str, tree_probe: bool) -> Result<Graph, DiagramError> {
    let statements = graph_statements(source);
    let Some((header_at, header)) = statements.first().copied() else {
        return Err(syntax(0..0, "missing_type"));
    };
    let kind = header.split_whitespace().next().unwrap_or("");
    if !matches!(kind, "flowchart" | "graph") {
        return Err(DiagramError::Unsupported {
            diagram_type: kind.into(),
        });
    }
    let direction = match header[kind.len()..].trim() {
        "TB" | "TD" => Direction::TopToBottom,
        "BT" => Direction::BottomToTop,
        "LR" => Direction::LeftToRight,
        "RL" => Direction::RightToLeft,
        _ => {
            return Err(syntax(
                header_at..header_at + header.len(),
                "invalid_direction",
            ));
        }
    };

    let mut builder = Builder {
        direction,
        tree_probe,
        ..Builder::default()
    };
    let mut open_container = None;
    for (at, statement) in statements.into_iter().skip(1) {
        if starts_word(statement, "subgraph") {
            builder.owner = None;
            if open_container.is_some() {
                return Err(unsupported(at..at + statement.len(), "subgraph"));
            }
            open_container = Some(parse_subgraph(statement, at, &mut builder)?);
        } else if statement == "end" {
            let Some(container) = open_container.take() else {
                return Err(unsupported(at..at + statement.len(), "subgraph"));
            };
            finish_subgraph(container, at + statement.len(), &mut builder);
        } else if starts_word(statement, "direction") {
            let Some(container) = open_container else {
                return Err(syntax(
                    at..at + statement.len(),
                    "direction_outside_subgraph",
                ));
            };
            let value = word_tail(statement, "direction").unwrap_or("");
            builder.containers[container].direction = match value.trim() {
                "TB" | "TD" => Direction::TopToBottom,
                "BT" => Direction::BottomToTop,
                "LR" => Direction::LeftToRight,
                "RL" => Direction::RightToLeft,
                _ => return Err(syntax(at..at + statement.len(), "invalid_direction")),
            };
        } else {
            builder.owner = open_container;
            parse_statement(statement, at, open_container, &mut builder)?;
        }
    }
    if let Some(container) = open_container {
        let range = builder.containers[container].source_range.clone();
        return Err(unsupported(range, "subgraph"));
    }
    Ok(builder.finish())
}

#[derive(Default)]
struct Builder {
    direction: Direction,
    tree_probe: bool,
    warnings: Vec<(Range<usize>, String)>,
    nodes: Vec<GraphNode>,
    ids: HashMap<String, usize>,
    defined: Vec<bool>,
    edges: Vec<Edge>,
    class_defs: HashMap<String, Style>,
    inline_styles: HashMap<String, Style>,
    containers: Vec<GraphContainer>,
    owner: Option<usize>,
}

#[derive(Clone)]
struct NodeRef {
    index: usize,
    range: Range<usize>,
}

#[derive(Clone)]
struct ParsedConnector {
    connector: Connector,
    label: Option<String>,
    label_range: Option<Range<usize>>,
    reverse: bool,
    visible: bool,
}

type ParsedLabel = Option<(String, Range<usize>)>;

impl Builder {
    fn node(
        &mut self,
        id: &str,
        id_range: Range<usize>,
        shape: Shape,
        label: Option<(String, Range<usize>)>,
        class: Option<String>,
        source_range: Range<usize>,
    ) -> usize {
        let index = if let Some(index) = self.ids.get(id).copied() {
            index
        } else {
            let index = self.nodes.len();
            self.ids.insert(id.into(), index);
            self.defined.push(false);
            self.nodes.push(GraphNode {
                id: id.into(),
                label: id.into(),
                id_range: id_range.clone(),
                label_range: id_range.clone(),
                source_range: id_range.clone(),
                shape: Shape::Rectangle,
                class: None,
                kind: GraphNodeKind::Node,
                owner: self.owner,
                preferred_size: None,
            });
            index
        };
        let node = &mut self.nodes[index];
        if node.kind == GraphNodeKind::Container {
            if class.is_some() {
                node.class = class;
            }
            return index;
        }
        if node.owner.is_none() {
            node.owner = self.owner;
        }
        if let Some(owner) = node.owner
            && let Some(container) = self.containers.get_mut(owner)
            && !container.members.contains(&index)
        {
            container.members.push(index);
        }
        if let Some((label, range)) = label {
            node.label = label;
            node.label_range = range;
            node.shape = shape;
            node.id_range = id_range.clone();
            node.source_range = source_range.clone();
            self.defined[index] = true;
        } else if !self.defined[index] && node.source_range.is_empty() {
            node.id_range = id_range.clone();
            node.label_range = id_range.clone();
            node.source_range = id_range.clone();
        }
        if class.is_some() {
            node.class = class;
        }
        index
    }

    fn finish(mut self) -> Graph {
        let parents = self
            .edges
            .iter()
            .filter(|edge| {
                edge.connector.line == EdgeLine::Solid
                    && edge.connector.start == Marker::None
                    && edge.connector.end == Marker::Arrow
            })
            .fold(
                vec![Vec::<usize>::new(); self.nodes.len()],
                |mut parents, edge| {
                    if !parents[edge.to].contains(&edge.from) {
                        parents[edge.to].push(edge.from);
                    }
                    parents
                },
            );
        let roots = parents.iter().filter(|parents| parents.is_empty()).count();
        let mut seen = vec![false; self.nodes.len()];
        let mut pending: Vec<_> = parents
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.is_empty().then_some(i))
            .collect();
        while let Some(node) = pending.pop() {
            if std::mem::replace(&mut seen[node], true) {
                continue;
            }
            pending.extend(
                parents
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.contains(&node).then_some(i)),
            );
        }
        // 身份由端点、语义和同类边序号决定，插入无关边不改变已有边的身份。
        let mut occurrences = HashMap::new();
        for edge in &mut self.edges {
            let mut key = format!(
                "{}\0{}\0{:?}\0{:?}",
                self.nodes[edge.from].id, self.nodes[edge.to].id, edge.connector, edge.label
            );
            // 已有可见边维持原身份；隐藏边单独命名，避免与同端点的实线冲突。
            if !edge.visible {
                key.push_str("\0invisible");
            }
            let occurrence = occurrences.entry(key.clone()).or_insert(0usize);
            let mut hash = 0xcbf29ce484222325u64;
            for byte in key.bytes().chain(occurrence.to_le_bytes()) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
            edge.id = hash as usize;
            *occurrence += 1;
        }
        Graph {
            direction: self.direction,
            warnings: self.warnings,
            nodes: self.nodes,
            edges: self.edges,
            class_defs: self.class_defs,
            inline_styles: self.inline_styles,
            forest: roots > 1
                && seen.iter().all(|seen| *seen)
                && parents.iter().all(|parents| parents.len() <= 1),
            containers: self.containers,
        }
    }
}

fn parse_subgraph(text: &str, at: usize, builder: &mut Builder) -> Result<usize, DiagramError> {
    let rest = word_tail(text, "subgraph")
        .ok_or_else(|| syntax(at..at + text.len(), "subgraph_missing_id"))?;
    let rest_at = at + text.len() - rest.len();
    let mut cursor = 0;
    while let Some(ch) = rest[cursor..].chars().next() {
        if !is_id_char(ch) {
            break;
        }
        cursor += ch.len_utf8();
    }
    if cursor == 0 {
        return Err(syntax(at..at + text.len(), "subgraph_missing_id"));
    }
    let id = &rest[..cursor];
    let id_range = rest_at..rest_at + cursor;
    skip_space(rest, &mut cursor);
    let (title, title_range) = if cursor < rest.len() {
        let (shape, label) = parse_shape(rest, &mut cursor, rest_at)?;
        if shape != Shape::Rectangle {
            return Err(syntax(
                rest_at + cursor..rest_at + rest.len(),
                "subgraph_title_shape",
            ));
        }
        label.map_or_else(
            || (id.to_string(), id_range.clone()),
            |(title, range)| (title, rest_at + range.start..rest_at + range.end),
        )
    } else {
        (id.to_string(), id_range.clone())
    };
    let class = parse_inline_class(rest, &mut cursor);
    if !rest[cursor..].trim().is_empty() {
        return Err(syntax(
            rest_at + cursor..rest_at + rest.len(),
            "subgraph_trailing_tokens",
        ));
    }
    let existing = builder.ids.get(id).copied();
    let index = builder.node(
        id,
        id_range.clone(),
        Shape::Rectangle,
        Some((title.clone(), title_range.clone())),
        class,
        at..at + text.len(),
    );
    if builder.nodes[index].kind == GraphNodeKind::Container
        || existing
            .is_some_and(|index| builder.defined[index] || builder.nodes[index].owner.is_some())
    {
        return Err(syntax(id_range.clone(), "subgraph_duplicate_id"));
    }
    builder.nodes[index].kind = GraphNodeKind::Container;
    builder.nodes[index].owner = None;
    builder.nodes[index].source_range = at..at + text.len();
    builder.defined[index] = true;
    let container = builder.containers.len();
    builder.containers.push(GraphContainer {
        node_index: index,
        direction: builder.direction,
        members: Vec::new(),
        id_range,
        title_range,
        source_range: at..at + text.len(),
    });
    Ok(container)
}

fn finish_subgraph(container: usize, end: usize, builder: &mut Builder) {
    let value = &mut builder.containers[container];
    value.source_range.end = end;
    let node = &mut builder.nodes[value.node_index];
    node.source_range.end = end;
}

fn parse_statement(
    text: &str,
    at: usize,
    container: Option<usize>,
    builder: &mut Builder,
) -> Result<(), DiagramError> {
    if builder.tree_probe
        && ["classDef", "style", "linkStyle"]
            .iter()
            .any(|word| starts_word(text, word))
    {
        return Ok(());
    }
    if starts_word(text, "linkStyle") {
        return Err(unsupported(at..at + text.len(), "linkStyle"));
    }
    if starts_word(text, "click") || starts_word(text, "accTitle") || starts_word(text, "accDescr")
    {
        return Ok(());
    }
    if let Some(rest) = word_tail(text, "classDef") {
        let Some((names, properties)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "classdef_missing_properties"));
        };
        let style = parse_style(
            properties,
            at + text.len() - properties.len(),
            Some(&mut builder.warnings),
        )?;
        for name in names.split(',') {
            if name.is_empty() {
                return Err(syntax(at..at + text.len(), "classdef_empty_name"));
            }
            builder.class_defs.insert(name.into(), style.clone());
        }
        return Ok(());
    }
    if let Some(rest) = word_tail(text, "style") {
        let Some((id, properties)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "style_missing_properties"));
        };
        let style = parse_style(
            properties,
            at + text.len() - properties.len(),
            Some(&mut builder.warnings),
        )?;
        let id_at = at + text.len() - rest.len();
        builder.node(
            id,
            id_at..id_at + id.len(),
            Shape::Rectangle,
            None,
            None,
            id_at..id_at + id.len(),
        );
        builder.inline_styles.insert(id.into(), style);
        return Ok(());
    }
    if let Some(rest) = word_tail(text, "class") {
        let Some((ids, class)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "class_missing_name"));
        };
        let mut id_at = at + text.len() - rest.len();
        for id in ids.split(',') {
            if id.is_empty() || class.trim().is_empty() {
                return Err(syntax(at..at + text.len(), "class_missing_target"));
            }
            builder.node(
                id,
                id_at..id_at + id.len(),
                Shape::Rectangle,
                None,
                Some(class.trim().into()),
                id_at..id_at + id.len(),
            );
            id_at += id.len() + 1;
        }
        return Ok(());
    }

    let mut cursor = 0;
    let mut previous = parse_node_set(text, &mut cursor, at, builder)?;
    loop {
        skip_space(text, &mut cursor);
        if cursor == text.len() {
            return Ok(());
        }
        let connector_at = cursor;
        let parsed = parse_connector(text, &mut cursor, at)?;
        let connector_range = at + connector_at..at + cursor;
        let targets = parse_node_set(text, &mut cursor, at, builder)?;
        for from in &previous {
            for to in &targets {
                let (from_index, to_index, edge_connector) = if parsed.reverse {
                    (
                        to.index,
                        from.index,
                        Connector {
                            start: parsed.connector.end,
                            end: parsed.connector.start,
                            ..parsed.connector
                        },
                    )
                } else {
                    (from.index, to.index, parsed.connector)
                };
                builder.edges.push(Edge {
                    id: builder.edges.len(),
                    from: from_index,
                    to: to_index,
                    source_range: from_range(
                        &previous,
                        &targets,
                        from_index,
                        to_index,
                        parsed.reverse,
                    ),
                    connector_range: connector_range.clone(),
                    label: parsed.label.clone(),
                    label_range: parsed.label_range.clone(),
                    connector: edge_connector,
                    visible: parsed.visible,
                    container: match container {
                        Some(container)
                            if builder.nodes[from_index].owner == Some(container)
                                && builder.nodes[to_index].owner == Some(container) =>
                        {
                            Some(container)
                        }
                        _ => None,
                    },
                });
            }
        }
        previous = targets;
    }
}

fn from_range(
    previous: &[NodeRef],
    targets: &[NodeRef],
    from: usize,
    to: usize,
    reverse: bool,
) -> Range<usize> {
    let (left, right) = if reverse { (to, from) } else { (from, to) };
    let source = previous
        .iter()
        .find(|node| node.index == left)
        .expect("parsed source");
    let target = targets
        .iter()
        .find(|node| node.index == right)
        .expect("parsed target");
    source.range.start..target.range.end
}

fn parse_node_set(
    text: &str,
    cursor: &mut usize,
    at: usize,
    builder: &mut Builder,
) -> Result<Vec<NodeRef>, DiagramError> {
    let mut nodes = vec![parse_node(text, cursor, at, builder)?];
    loop {
        skip_space(text, cursor);
        if !text[*cursor..].starts_with('&') {
            return Ok(nodes);
        }
        *cursor += 1;
        nodes.push(parse_node(text, cursor, at, builder)?);
    }
}

fn parse_node(
    text: &str,
    cursor: &mut usize,
    at: usize,
    builder: &mut Builder,
) -> Result<NodeRef, DiagramError> {
    skip_space(text, cursor);
    let start = *cursor;
    while let Some(ch) = text[*cursor..].chars().next() {
        if connector_start(&text[*cursor..]) || !is_id_char(ch) {
            break;
        }
        *cursor += ch.len_utf8();
    }
    if *cursor == start {
        return Err(syntax(at + start..at + text.len(), "missing_node_id"));
    }
    let id_end = *cursor;
    let id = &text[start..id_end];
    skip_space(text, cursor);
    let (shape, label) = parse_shape(text, cursor, at)?;
    let class = parse_inline_class(text, cursor);
    let end = text[..*cursor].trim_end().len();
    let index = builder.node(
        id,
        at + start..at + id_end,
        shape,
        label.map(|(label, range)| (label, at + range.start..at + range.end)),
        class,
        at + start..at + end,
    );
    Ok(NodeRef {
        index,
        range: at + start..at + end,
    })
}

fn parse_shape(
    text: &str,
    cursor: &mut usize,
    at: usize,
) -> Result<(Shape, ParsedLabel), DiagramError> {
    let forms = [
        ("(((", ")))", Shape::DoubleCircle),
        ("((", "))", Shape::Circle),
        ("([", "])", Shape::Stadium),
        ("[[", "]]", Shape::Subroutine),
        ("[(", ")]", Shape::Cylinder),
        ("{{", "}}", Shape::Hexagon),
        ("[/", "/]", Shape::Parallelogram),
        ("[\\", "\\]", Shape::ParallelogramReverse),
        ("(", ")", Shape::Rounded),
        ("{", "}", Shape::Diamond),
        (">", "]", Shape::Asymmetric),
        ("[", "]", Shape::Rectangle),
    ];
    let Some((open, close, shape)) = forms
        .iter()
        .find(|(open, _, _)| text[*cursor..].starts_with(open))
    else {
        return Ok((Shape::Rectangle, None));
    };
    let body = *cursor + open.len();
    let mut quoted = false;
    let mut escaped = false;
    let mut nested = Vec::new();
    for (relative, ch) in text[body..].char_indices() {
        let index = body + relative;
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            continue;
        }
        if nested.is_empty() && text[index..].starts_with(close) {
            let raw = &text[body..index];
            let trimmed = raw.trim();
            let left = raw.len() - raw.trim_start().len();
            let label_start = body + left;
            let label_end = label_start + trimmed.len();
            *cursor = index + close.len();
            return Ok((*shape, Some((unquote(trimmed), label_start..label_end))));
        }
        match ch {
            '[' | '(' | '{' => nested.push(ch),
            ']' | ')' | '}' => {
                nested.pop();
            }
            _ => {}
        }
    }
    Err(syntax(
        at + *cursor..at + text.len(),
        "node_shape_or_quote_unclosed",
    ))
}

fn parse_connector(
    text: &str,
    cursor: &mut usize,
    at: usize,
) -> Result<ParsedConnector, DiagramError> {
    let start = *cursor;
    let mut label = None;
    let mut label_range = None;
    // 中缀标签先于普通连线读取，不能把标签中的 > 或 <br> 当作箭头。
    if text[start..].starts_with("--") && text[start + 2..].starts_with(char::is_whitespace) {
        let body = start + 2;
        let mut quoted = false;
        let mut escaped = false;
        let mut terminator = None;
        for (relative, ch) in text[body..].char_indices() {
            let index = body + relative;
            if quoted {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    quoted = false;
                }
                continue;
            }
            if ch == '"' {
                quoted = true;
                continue;
            }
            if text[index..].starts_with("-->") {
                terminator = Some(index);
                break;
            }
        }
        let end = terminator
            .ok_or_else(|| syntax(at + start..at + text.len(), "edge_label_missing_arrow"))?;
        let raw = &text[body..end];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(syntax(at + body..at + end, "edge_label_empty"));
        }
        let first = body + raw.len() - raw.trim_start().len();
        label = Some(unquote(trimmed));
        label_range = Some(at + first..at + first + trimmed.len());
        *cursor = end + 3;
        return Ok(ParsedConnector {
            connector: Connector {
                line: EdgeLine::Solid,
                start: Marker::None,
                end: Marker::Arrow,
            },
            label,
            label_range,
            reverse: false,
            visible: true,
        });
    }
    if text[start..].starts_with("~~~") {
        *cursor += 3;
        return Ok(ParsedConnector {
            connector: Connector {
                line: EdgeLine::Solid,
                start: Marker::None,
                end: Marker::None,
            },
            label: None,
            label_range: None,
            reverse: false,
            visible: false,
        });
    }
    let mut start_marker = Marker::None;
    if text[*cursor..].starts_with('<') {
        start_marker = Marker::Arrow;
        *cursor += 1;
    }
    let body = *cursor;
    let line;
    let count;
    if text[body..].starts_with("-.") {
        let dots = text[body + 1..]
            .bytes()
            .take_while(|byte| *byte == b'.')
            .count();
        let end = body + 1 + dots;
        if !text[end..].starts_with('-') {
            return Err(syntax(
                at + start..at + text.len(),
                "dotted_edge_missing_end",
            ));
        }
        *cursor = end + 1;
        line = EdgeLine::Dotted;
        count = 3;
    } else {
        let byte = text.as_bytes().get(body).copied().unwrap_or(0);
        if !matches!(byte, b'-' | b'=') {
            return Err(syntax(at + start..at + text.len(), "unrecognized_edge"));
        }
        count = text[body..]
            .bytes()
            .take_while(|candidate| *candidate == byte)
            .count();
        if count < 2 {
            return Err(syntax(at + start..at + text.len(), "unrecognized_edge"));
        }
        *cursor += count;
        line = if byte == b'=' {
            EdgeLine::Thick
        } else {
            EdgeLine::Solid
        };
    }
    let end_marker = match text.as_bytes().get(*cursor) {
        Some(b'>') => Marker::Arrow,
        Some(b'o') => Marker::Circle,
        Some(b'x') => Marker::Cross,
        _ => Marker::None,
    };
    if end_marker != Marker::None {
        *cursor += 1;
    } else if start_marker == Marker::None && count < 3 {
        return Err(syntax(at + start..at + *cursor, "edge_missing_end"));
    }
    skip_space(text, cursor);
    if text[*cursor..].starts_with('|') {
        let body = *cursor + 1;
        let mut quoted = false;
        let mut escaped = false;
        let mut end = None;
        for (relative, ch) in text[body..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                quoted = !quoted;
            }
            if ch == '|' && !quoted {
                end = Some(body + relative);
                break;
            }
        }
        let end =
            end.ok_or_else(|| syntax(at + *cursor..at + text.len(), "edge_label_unclosed"))?;
        let raw = &text[body..end];
        let trimmed = raw.trim();
        let first = body + raw.len() - raw.trim_start().len();
        label = (!trimmed.is_empty()).then(|| unquote(trimmed));
        label_range = label
            .as_ref()
            .map(|_| at + first..at + first + trimmed.len());
        *cursor = end + 1;
    }
    Ok(ParsedConnector {
        connector: Connector {
            line,
            start: start_marker,
            end: end_marker,
        },
        label,
        label_range,
        reverse: start_marker == Marker::Arrow && end_marker == Marker::None,
        visible: true,
    })
}

fn connector_start(text: &str) -> bool {
    text.starts_with("--")
        || text.starts_with("-.")
        || text.starts_with("==")
        || text.starts_with("<--")
        || text.starts_with("~~~")
}

fn parse_inline_class(text: &str, cursor: &mut usize) -> Option<String> {
    skip_space(text, cursor);
    if !text[*cursor..].starts_with(":::") {
        return None;
    }
    *cursor += 3;
    let start = *cursor;
    while let Some(ch) = text[*cursor..].chars().next() {
        if !is_id_char(ch) {
            break;
        }
        *cursor += ch.len_utf8();
    }
    (start < *cursor).then(|| text[start..*cursor].into())
}

fn parse_style(
    text: &str,
    at: usize,
    mut warnings: Option<&mut Vec<(Range<usize>, String)>>,
) -> Result<Style, DiagramError> {
    let mut style = Style::default();
    // CSS 函数的逗号属于同一属性；即使不支持该颜色也保留完整诊断范围。
    let mut depth = 0usize;
    let items = text.split(|ch| match ch {
        '(' => {
            depth += 1;
            false
        }
        ')' => {
            depth = depth.saturating_sub(1);
            false
        }
        ',' => depth == 0,
        _ => false,
    });
    for item in items {
        let Some((key, value)) = item.split_once(':') else {
            return Err(syntax(at..at + text.len(), "style_invalid_property"));
        };
        let value = value.trim();
        let key = key.trim();
        let parsed = match key {
            "fill" | "stroke" | "color" => Color::parse_css(value).is_some(),
            "stroke-width" => parse_px(value).is_some_and(|width| width >= 0.0),
            "stroke-dasharray" => parse_dasharray(value).is_some(),
            _ => false,
        };
        if !parsed {
            let offset = item.as_ptr() as usize - text.as_ptr() as usize;
            let range = at + offset..at + offset + item.len();
            // 范围外属性只作可诊断降级，保留已支持属性和原图结构。
            if let Some(warnings) = warnings.as_deref_mut() {
                warnings.push((range, format!("ignored_style_property:{key}")));
            }
            continue;
        }
        match key {
            "fill" => style.fill = Some(value.into()),
            "stroke" => style.stroke = Some(value.into()),
            "color" => style.color = Some(value.into()),
            "stroke-width" => style.stroke_width = parse_px(value),
            "stroke-dasharray" => style.stroke_dasharray = parse_dasharray(value),
            _ => unreachable!(),
        }
    }
    Ok(style)
}

fn parse_px(value: &str) -> Option<f64> {
    value
        .trim()
        .strip_suffix("px")
        .unwrap_or(value.trim())
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn parse_dasharray(value: &str) -> Option<Vec<f64>> {
    let values = value
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(parse_px)
        .collect::<Option<Vec<_>>>()?;
    (!values.is_empty()
        && values.iter().all(|value| *value >= 0.0)
        && values.iter().any(|value| *value > 0.0))
    .then_some(values)
}

fn starts_word(text: &str, word: &str) -> bool {
    text == word
        || text
            .strip_prefix(word)
            .is_some_and(|tail| tail.starts_with(char::is_whitespace) || tail.starts_with(':'))
}

fn word_tail<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    text.strip_prefix(word)
        .filter(|tail| tail.starts_with(char::is_whitespace))
        .map(str::trim_start)
}

fn is_id_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

fn skip_space(text: &str, cursor: &mut usize) {
    *cursor += text[*cursor..].len() - text[*cursor..].trim_start().len();
}

fn unquote(text: &str) -> String {
    let text = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(text);
    text.replace("\\\"", "\"")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

// 普通图的词法边界独立于旧树入口，避免改变旧场景与错误契约。
fn graph_statements(source: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut pipe = false;
    let mut infix = false;
    let mut comment = false;
    for (index, ch) in source.char_indices() {
        if comment {
            if ch == '\n' {
                comment = false;
                start = index + 1;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            if ch == '\\' {
                escaped = true;
            }
            if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            continue;
        }
        if depth == 0 && ch == '|' {
            pipe = !pipe;
            continue;
        }
        if pipe {
            continue;
        }
        if depth == 0 && source[index..].starts_with("-->") {
            infix = false;
        }
        if depth == 0
            && (index == 0 || source.as_bytes()[index - 1] != b'-')
            && source[index..].starts_with("--")
            && source[index + 2..].starts_with(char::is_whitespace)
        {
            infix = true;
        }
        if !infix && depth == 0 && source[index..].starts_with("%%") {
            push_graph_statement(&mut result, source, start, index);
            comment = true;
            continue;
        }
        if infix {
            continue;
        }
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            '\n' | ';' if depth == 0 => {
                push_graph_statement(&mut result, source, start, index);
                start = index + 1;
            }
            _ => {}
        }
    }
    if !comment {
        push_graph_statement(&mut result, source, start, source.len());
    }
    result
}
fn push_graph_statement<'a>(
    result: &mut Vec<(usize, &'a str)>,
    source: &'a str,
    start: usize,
    end: usize,
) {
    let raw = &source[start..end];
    if !raw.trim().is_empty() {
        result.push((start + raw.len() - raw.trim_start().len(), raw.trim()));
    }
}
