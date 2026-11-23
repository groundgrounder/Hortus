//! 逐行解析类图，保留类、成员、关系与便签的原始字节位置。

use crate::class::*;
use crate::parser::syntax;
use crate::syntax::{unquote, word_tail};
use crate::uml_parser::{self, Line};
use crate::{DiagramError, Direction, EdgeLine};
use std::collections::HashMap;
use std::ops::Range;

pub(crate) fn is_class_diagram(source: &str) -> bool {
    matches!(
        crate::parser::infer_diagram_header(source),
        Some("classDiagram" | "classDiagram-v2")
    )
}

struct Parser<'a> {
    source: &'a str,
    lines: Vec<Line<'a>>,
    position: usize,
    directive: bool,
    diagram: ClassDiagram,
    ids: HashMap<String, usize>,
    defined: Vec<bool>,
    assignments: HashMap<String, String>,
    namespace: Option<usize>,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<Line<'a>> {
        loop {
            let line = *self.lines.get(self.position)?;
            self.position += 1;
            let line = uml_parser::statement(line, &mut self.directive);
            if !line.text.is_empty() {
                return Some(line);
            }
        }
    }

    fn node(&mut self, id: &str, range: Range<usize>, declared: bool) -> usize {
        let namespace = self.namespace;
        let index = *self.ids.entry(id.to_owned()).or_insert_with(|| {
            let index = self.diagram.classes.len();
            self.diagram.classes.push(ClassNode {
                id: id.to_owned(),
                label: id.to_owned(),
                stereotype: None,
                fields: Vec::new(),
                methods: Vec::new(),
                class: None,
                namespace,
                source_range: range.clone(),
            });
            self.defined.push(false);
            index
        });
        if declared && !self.defined[index] {
            let class = &mut self.diagram.classes[index];
            class.source_range = range;
            if class.namespace.is_none() {
                class.namespace = namespace;
            }
            self.defined[index] = true;
        }
        index
    }

    fn body(&mut self) -> Result<(), DiagramError> {
        while let Some(line) = self.next() {
            self.statement(line, false)?;
        }
        Ok(())
    }

    fn statement(&mut self, line: Line<'a>, in_namespace: bool) -> Result<(), DiagramError> {
        let text = line.text;
        if text == "}" {
            return Err(line.error("class_unexpected_close"));
        }
        if let Some(value) = word_tail(text, "direction") {
            self.diagram.direction = match value {
                "TB" | "TD" => Direction::TopToBottom,
                "BT" => Direction::BottomToTop,
                "LR" => Direction::LeftToRight,
                "RL" => Direction::RightToLeft,
                _ => return Err(line.error("class_invalid_direction")),
            };
            return Ok(());
        }
        if let Some(value) = word_tail(text, "title") {
            self.diagram.title = Some(unquote(value));
            return Ok(());
        }
        if let Some(value) = word_tail(text, "classDef") {
            let Some((name, properties)) = uml_parser::class_definition(value) else {
                return Err(line.error("class_invalid_class_definition"));
            };
            let at = uml_parser::subslice_offset(self.source, properties);
            let style = crate::graph_parser::parse_style(properties, at, None)?;
            self.diagram.class_defs.insert(name.to_owned(), style);
            return Ok(());
        }
        if let Some(value) = word_tail(text, "class") {
            return self.class_line(&line, value, in_namespace);
        }
        if let Some(value) = word_tail(text, "cssClass") {
            let Some((ids, class)) = uml_parser::assignments(value) else {
                return Err(line.error("class_invalid_class_assignment"));
            };
            for id in ids {
                self.assignments.insert(id, class.clone());
            }
            return Ok(());
        }
        if let Some(value) = word_tail(text, "namespace") {
            if in_namespace {
                return Err(line.error("class_nested_namespace"));
            }
            let Some(id) = namespace_header(value) else {
                return Err(line.error("class_invalid_namespace"));
            };
            let index = self.namespace_entry(id, line.range());
            let previous = self.namespace;
            self.namespace = Some(index);
            let result = self.namespace_body(&line);
            self.namespace = previous;
            return result;
        }
        if let Some(value) = word_tail(text, "note") {
            return self.note_line(&line, value);
        }
        // 只影响交互或外观的语句不执行脚本，也不改变图结构。
        if text.starts_with("click ")
            || text.starts_with("link ")
            || text.starts_with("callback ")
            || text.starts_with("style ")
            || text.starts_with("linkStyle ")
            || text.starts_with("accTitle")
            || text.starts_with("accDescr")
        {
            return Ok(());
        }
        if text.starts_with("<<") {
            return self.class_line(&line, text, in_namespace);
        }
        if let Some(relation) = relation(text, self.source) {
            let relation = relation.map_err(|_| line.error("class_invalid_relation"))?;
            self.add_relation(&line, relation);
            return Ok(());
        }
        let (head, member) = uml_parser::property(text);
        if let Some(member) = member {
            let mut cursor = 0;
            uml_parser::skip_space(head, &mut cursor);
            if let Some(id) = uml_parser::id(head, &mut cursor) {
                uml_parser::skip_space(head, &mut cursor);
                if cursor == head.len() {
                    let at = uml_parser::subslice_offset(self.source, id);
                    let index = self.node(id, at..at + id.len(), true);
                    let text = member.to_owned();
                    let range = member_offset(&line, member);
                    return self.member(index, &text, range);
                }
            }
        }
        if let Some((id, label, stereotype, class)) = declaration(text) {
            let index = self.node(&id, line.range(), true);
            let node = &mut self.diagram.classes[index];
            node.label = label;
            if stereotype.is_some() {
                node.stereotype = stereotype;
            }
            if class.is_some() {
                node.class = class;
            }
            return Ok(());
        }
        Err(line.error("class_invalid_statement"))
    }

    fn namespace_entry(&mut self, id: &str, range: Range<usize>) -> usize {
        if let Some(index) = self
            .diagram
            .namespaces
            .iter()
            .position(|namespace| namespace.id == id)
        {
            return index;
        }
        self.diagram.namespaces.push(ClassNamespace {
            id: id.to_owned(),
            source_range: range,
        });
        self.diagram.namespaces.len() - 1
    }

    fn namespace_body(&mut self, opening: &Line<'a>) -> Result<(), DiagramError> {
        loop {
            let Some(line) = self.next() else {
                return Err(opening.error("class_unclosed_namespace"));
            };
            if line.text == "}" {
                return Ok(());
            }
            if let Some(at) = uml_parser::outside_quotes(line.text, "}") {
                let before = line.text[..at].trim();
                if !before.is_empty() {
                    let end = line.at + at;
                    self.statement(
                        Line {
                            at: line.at,
                            text: before,
                        },
                        true,
                    )?;
                    let _ = end;
                }
                return Ok(());
            }
            self.statement(line, true)?;
        }
    }

    fn class_line(
        &mut self,
        line: &Line<'a>,
        value: &'a str,
        in_namespace: bool,
    ) -> Result<(), DiagramError> {
        let _ = in_namespace;
        if let Some(open) = uml_parser::outside_quotes(value, "{") {
            let Some((id, label, stereotype, class)) = declaration(value[..open].trim()) else {
                return Err(line.error("class_invalid_declaration"));
            };
            let index = self.node(&id, line.range(), true);
            let node = &mut self.diagram.classes[index];
            node.label = label;
            if stereotype.is_some() {
                node.stereotype = stereotype;
            }
            if class.is_some() {
                node.class = class;
            }
            let rest = value[open + 1..].trim();
            if let Some(close) = uml_parser::outside_quotes(rest, "}") {
                let content = rest[..close].trim();
                if !content.is_empty() {
                    for member in content.split(';') {
                        let member = member.trim();
                        if !member.is_empty() {
                            let at = uml_parser::subslice_offset(self.source, member);
                            self.member(index, member, at..at + member.len())?;
                        }
                    }
                }
                if !rest[close + 1..].trim().is_empty() {
                    return Err(line.error("class_unexpected_content"));
                }
                return Ok(());
            }
            if !rest.is_empty() {
                let at = uml_parser::subslice_offset(self.source, rest);
                self.member(index, rest, at..at + rest.len())?;
            }
            return self.block(line, index);
        }
        if value.contains(char::is_whitespace)
            && !value.contains('[')
            && !value.contains('"')
            && let Some((ids, class)) = uml_parser::assignments(value)
        {
            for id in ids {
                self.assignments.insert(id, class.clone());
            }
            return Ok(());
        }
        let Some((id, label, stereotype, class)) = declaration(value) else {
            return Err(line.error("class_invalid_declaration"));
        };
        let index = self.node(&id, line.range(), true);
        let node = &mut self.diagram.classes[index];
        node.label = label;
        if stereotype.is_some() {
            node.stereotype = stereotype;
        }
        if class.is_some() {
            node.class = class;
        }
        Ok(())
    }

    fn block(&mut self, opening: &Line<'a>, class: usize) -> Result<(), DiagramError> {
        loop {
            let Some(line) = self.next() else {
                return Err(opening.error("class_unclosed_block"));
            };
            if line.text == "}" {
                return Ok(());
            }
            if let Some(at) = uml_parser::outside_quotes(line.text, "}") {
                let before = line.text[..at].trim();
                if !before.is_empty() {
                    self.member(class, before, line.at..line.at + before.len())?;
                }
                if !line.text[at + 1..].trim().is_empty() {
                    return Err(line.error("class_unexpected_content"));
                }
                return Ok(());
            }
            self.member(class, line.text, line.range())?;
        }
    }

    fn member(
        &mut self,
        class: usize,
        text: &str,
        range: Range<usize>,
    ) -> Result<(), DiagramError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(syntax(range, "class_invalid_member"));
        }
        if let Some(inner) = text
            .strip_prefix("<<")
            .and_then(|text| text.strip_suffix(">>"))
        {
            let node = &mut self.diagram.classes[class];
            if node.stereotype.is_none() {
                node.stereotype = Some(inner.trim().to_owned());
                return Ok(());
            }
        }
        let first = text.chars().next().unwrap_or('+');
        let (visibility, body) = match first {
            '+' => (ClassVisibility::Public, &text[1..]),
            '-' => (ClassVisibility::Private, &text[1..]),
            '#' => (ClassVisibility::Protected, &text[1..]),
            '~' => (ClassVisibility::Package, &text[1..]),
            _ => (ClassVisibility::Public, text),
        };
        let modifier = |symbol: char| {
            body.contains(&format!("{symbol}("))
                || body.contains(&format!("{symbol}:"))
                || body.trim_end().ends_with(symbol)
        };
        let member = ClassMember {
            text: text.to_owned(),
            visibility,
            kind: if body.contains('(') {
                ClassMemberKind::Method
            } else {
                ClassMemberKind::Field
            },
            abstract_member: modifier('*'),
            static_member: modifier('$'),
            source_range: range,
        };
        let node = &mut self.diagram.classes[class];
        if member.kind == ClassMemberKind::Method {
            node.methods.push(member);
        } else {
            node.fields.push(member);
        }
        Ok(())
    }

    fn note_line(&mut self, line: &Line<'a>, value: &str) -> Result<(), DiagramError> {
        let value = value.trim();
        if let Some(rest) = value.strip_prefix("for") {
            let rest = rest.trim_start();
            let mut cursor = 0;
            let Some(target) = uml_parser::id(rest, &mut cursor) else {
                return Err(line.error("class_invalid_note"));
            };
            uml_parser::skip_space(rest, &mut cursor);
            let Some((range, _)) = uml_parser::quoted(rest, &mut cursor) else {
                return Err(line.error("class_invalid_note"));
            };
            if !rest[cursor..].trim().is_empty() {
                return Err(line.error("class_invalid_note"));
            }
            let index = self.node(target, line.range(), false);
            let at = uml_parser::subslice_offset(self.source, &rest[range.clone()]);
            self.diagram.notes.push(ClassNote {
                target: Some(index),
                text: unquote(&rest[range.clone()]),
                source_range: at..at + range.len(),
            });
            return Ok(());
        }
        let mut cursor = 0;
        let Some((range, _)) = uml_parser::quoted(value, &mut cursor) else {
            return Err(line.error("class_invalid_note"));
        };
        if !value[cursor..].trim().is_empty() {
            return Err(line.error("class_invalid_note"));
        }
        self.diagram.notes.push(ClassNote {
            target: None,
            text: unquote(&value[range]),
            source_range: line.range(),
        });
        Ok(())
    }

    fn add_relation(&mut self, line: &Line<'a>, relation: Relation<'_>) {
        let from = self.node(relation.from, relation.from_range, false);
        let to = self.node(relation.to, relation.to_range, false);
        if let Some(class) = relation.from_class {
            self.diagram.classes[from].class = Some(class.to_owned());
        }
        if let Some(class) = relation.to_class {
            self.diagram.classes[to].class = Some(class.to_owned());
        }
        self.diagram.relations.push(ClassRelation {
            from,
            to,
            start: relation.start,
            end: relation.end,
            line: relation.line,
            from_cardinality: relation.from_cardinality,
            to_cardinality: relation.to_cardinality,
            label: relation.label.map(unquote),
            label_range: relation.label.map(|label| {
                let at = uml_parser::subslice_offset(self.source, label);
                at..at + label.len()
            }),
            connector_range: {
                let at = uml_parser::subslice_offset(self.source, relation.connector);
                at..at + relation.connector.len()
            },
            source_range: line.range(),
        });
    }
}

fn namespace_header(value: &str) -> Option<&str> {
    let value = value.trim();
    let name = value.strip_suffix('{')?.trim_end();
    uml_parser::valid_id(name).then_some(name)
}

fn declaration(text: &str) -> Option<(String, String, Option<String>, Option<String>)> {
    let mut cursor = 0;
    uml_parser::skip_space(text, &mut cursor);
    let mut stereotype = None;
    if text[cursor..].starts_with("<<") {
        let close = text[cursor..].find(">>")? + cursor;
        stereotype = Some(text[cursor + 2..close].trim().to_owned());
        cursor = close + 2;
        uml_parser::skip_space(text, &mut cursor);
    }
    let id = uml_parser::id(text, &mut cursor)?.to_owned();
    let mut label = id.clone();
    while text[cursor..].starts_with('~') {
        let close = text[cursor + 1..].find('~')? + cursor + 1;
        label.push('<');
        label.push_str(text[cursor + 1..close].trim());
        label.push('>');
        cursor = close + 1;
    }
    if text[cursor..].starts_with('[') {
        let (display, _) = uml_parser::bracket_label(text, &mut cursor)?;
        label = display;
    }
    uml_parser::skip_space(text, &mut cursor);
    let class = uml_parser::class_suffix(text, &mut cursor);
    uml_parser::skip_space(text, &mut cursor);
    if text[cursor..].starts_with("<<") {
        let close = text[cursor..].find(">>")? + cursor;
        stereotype = Some(text[cursor + 2..close].trim().to_owned());
        cursor = close + 2;
        uml_parser::skip_space(text, &mut cursor);
    }
    (cursor == text.len()).then_some((id, label, stereotype, class.map(str::to_owned)))
}

struct Relation<'a> {
    from: &'a str,
    to: &'a str,
    from_range: Range<usize>,
    to_range: Range<usize>,
    from_class: Option<&'a str>,
    to_class: Option<&'a str>,
    start: ClassMarker,
    end: ClassMarker,
    line: EdgeLine,
    from_cardinality: Option<String>,
    to_cardinality: Option<String>,
    label: Option<&'a str>,
    connector: &'a str,
}

/// 返回 `None` 表示语句不是关系；`Some(Err)` 表示看起来是关系但语法不合法。
fn relation<'a>(text: &'a str, source: &str) -> Option<Result<Relation<'a>, ()>> {
    let (head, label) = uml_parser::property(text);
    if !head.contains("--") && !head.contains("..") {
        return None;
    }
    Some(parse_relation(head, label, source))
}

fn parse_relation<'a>(
    head: &'a str,
    label: Option<&'a str>,
    source: &str,
) -> Result<Relation<'a>, ()> {
    let mut cursor = 0;
    uml_parser::skip_space(head, &mut cursor);
    let from = uml_parser::id(head, &mut cursor).ok_or(())?;
    uml_parser::skip_space(head, &mut cursor);
    let from_class = uml_parser::class_suffix(head, &mut cursor);
    uml_parser::skip_space(head, &mut cursor);
    let from_cardinality = cardinality(head, &mut cursor);
    uml_parser::skip_space(head, &mut cursor);
    let connector_start = cursor;
    let (start, line, end) = connector(head, &mut cursor).ok_or(())?;
    let connector = &head[connector_start..cursor];
    uml_parser::skip_space(head, &mut cursor);
    let to_cardinality = cardinality(head, &mut cursor);
    uml_parser::skip_space(head, &mut cursor);
    let to = uml_parser::id(head, &mut cursor).ok_or(())?;
    uml_parser::skip_space(head, &mut cursor);
    let to_class = uml_parser::class_suffix(head, &mut cursor);
    uml_parser::skip_space(head, &mut cursor);
    if cursor != head.len() {
        return Err(());
    }
    Ok(Relation {
        from,
        to,
        from_range: {
            let at = uml_parser::subslice_offset(source, from);
            at..at + from.len()
        },
        to_range: {
            let at = uml_parser::subslice_offset(source, to);
            at..at + to.len()
        },
        from_class,
        to_class,
        start,
        end,
        line,
        from_cardinality,
        to_cardinality,
        label,
        connector,
    })
}

fn cardinality(text: &str, cursor: &mut usize) -> Option<String> {
    if !text[*cursor..].starts_with('"') {
        return None;
    }
    let (range, _) = uml_parser::quoted(text, cursor)?;
    Some(unquote(&text[range]))
}

fn connector(text: &str, cursor: &mut usize) -> Option<(ClassMarker, EdgeLine, ClassMarker)> {
    let rest = &text[*cursor..];
    let start = if rest.starts_with("<|") {
        *cursor += 2;
        ClassMarker::Triangle
    } else if rest.starts_with('*') {
        *cursor += 1;
        ClassMarker::DiamondFilled
    } else if rest.starts_with('o') {
        *cursor += 1;
        ClassMarker::DiamondHollow
    } else if rest.starts_with('<') {
        *cursor += 1;
        ClassMarker::Arrow
    } else {
        ClassMarker::None
    };
    let rest = &text[*cursor..];
    let line = if rest.starts_with("--") {
        *cursor += 2;
        EdgeLine::Solid
    } else if rest.starts_with("..") {
        *cursor += 2;
        EdgeLine::Dotted
    } else {
        return None;
    };
    let rest = &text[*cursor..];
    let end = if rest.starts_with("|>") {
        *cursor += 2;
        ClassMarker::Triangle
    } else if rest.starts_with('*') {
        *cursor += 1;
        ClassMarker::DiamondFilled
    } else if rest.starts_with('o') {
        *cursor += 1;
        ClassMarker::DiamondHollow
    } else if rest.starts_with('>') {
        *cursor += 1;
        ClassMarker::Arrow
    } else {
        ClassMarker::None
    };
    Some((start, line, end))
}

fn member_offset(line: &Line<'_>, member: &str) -> Range<usize> {
    let at = uml_parser::subslice_offset(line.text, member);
    line.at + at..line.at + at + member.len()
}

pub(crate) fn parse(source: &str) -> Result<ClassDiagram, DiagramError> {
    let mut parser = Parser {
        source,
        lines: uml_parser::lines(source),
        position: 0,
        directive: false,
        diagram: ClassDiagram::default(),
        ids: HashMap::new(),
        defined: Vec::new(),
        assignments: HashMap::new(),
        namespace: None,
    };
    let header = parser.next().ok_or_else(|| syntax(0..0, "missing_type"))?;
    if !matches!(header.text, "classDiagram" | "classDiagram-v2") {
        return Err(header.error("class_invalid_header"));
    }
    parser.body()?;
    if parser.diagram.classes.is_empty() {
        return Err(header.error("class_empty"));
    }
    for class in &mut parser.diagram.classes {
        if class.class.is_none() {
            class.class = parser.assignments.get(&class.id).cloned();
        }
    }
    Ok(parser.diagram)
}
