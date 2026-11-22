//! 逐行解析 ER 图，保留实体、属性与关系的原始字节位置。

use crate::er::*;
use crate::parser::syntax;
use crate::syntax::{unquote, word_tail};
use crate::uml_parser::{self, Line};
use crate::{DiagramError, Direction};
use std::collections::HashMap;
use std::ops::Range;

pub(crate) fn is_er_diagram(source: &str) -> bool {
    matches!(
        crate::parser::infer_diagram_header(source),
        Some("erDiagram")
    )
}

struct Parser<'a> {
    source: &'a str,
    lines: Vec<Line<'a>>,
    position: usize,
    directive: bool,
    diagram: ErDiagram,
    ids: HashMap<String, usize>,
    defined: Vec<bool>,
    assignments: HashMap<String, String>,
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

    fn entity(&mut self, id: &str, range: Range<usize>, declared: bool) -> usize {
        let index = *self.ids.entry(id.to_owned()).or_insert_with(|| {
            let index = self.diagram.entities.len();
            self.diagram.entities.push(ErEntity {
                id: id.to_owned(),
                label: id.to_owned(),
                attributes: Vec::new(),
                class: None,
                source_range: range.clone(),
            });
            self.defined.push(false);
            index
        });
        if declared && !self.defined[index] {
            self.diagram.entities[index].source_range = range;
            self.defined[index] = true;
        }
        index
    }

    fn body(&mut self) -> Result<(), DiagramError> {
        while let Some(line) = self.next() {
            self.statement(line)?;
        }
        Ok(())
    }

    fn statement(&mut self, line: Line<'a>) -> Result<(), DiagramError> {
        let text = line.text;
        if text == "}" {
            return Err(line.error("er_unexpected_close"));
        }
        if let Some(value) = word_tail(text, "direction") {
            self.diagram.direction = match value {
                "TB" | "TD" => Direction::TopToBottom,
                "BT" => Direction::BottomToTop,
                "LR" => Direction::LeftToRight,
                "RL" => Direction::RightToLeft,
                _ => return Err(line.error("er_invalid_direction")),
            };
            return Ok(());
        }
        if let Some(value) = word_tail(text, "title") {
            self.diagram.title = Some(unquote(value));
            return Ok(());
        }
        if let Some(value) = word_tail(text, "classDef") {
            let Some((name, properties)) = uml_parser::class_definition(value) else {
                return Err(line.error("er_invalid_class_definition"));
            };
            let at = uml_parser::subslice_offset(self.source, properties);
            let style = crate::graph_parser::parse_style(properties, at, None)?;
            self.diagram.class_defs.insert(name.to_owned(), style);
            return Ok(());
        }
        if let Some(value) = word_tail(text, "class") {
            let Some((ids, class)) = uml_parser::assignments(value) else {
                return Err(line.error("er_invalid_class_assignment"));
            };
            for id in ids {
                self.assignments.insert(id, class.clone());
            }
            return Ok(());
        }
        if let Some(value) = word_tail(text, "cssClass") {
            let Some((ids, class)) = uml_parser::assignments(value) else {
                return Err(line.error("er_invalid_class_assignment"));
            };
            for id in ids {
                self.assignments.insert(id, class.clone());
            }
            return Ok(());
        }
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
        if let Some(header) = entity_header(text) {
            let header = header.map_err(|_| line.error("er_invalid_entity"))?;
            let index = self.entity(header.id, line.range(), true);
            let node = &mut self.diagram.entities[index];
            if header.label.is_some() {
                node.label = header.label.clone().unwrap();
            }
            if header.class.is_some() {
                node.class = header.class.clone();
            }
            match header.block {
                EntityBlock::None => Ok(()),
                EntityBlock::Empty => Ok(()),
                EntityBlock::Open => self.attributes(&line, index),
            }
        } else if let Some(relation) = relation(text, self.source) {
            let relation = relation.map_err(|_| line.error("er_invalid_relation"))?;
            self.add_relation(&line, relation);
            Ok(())
        } else {
            Err(line.error("er_invalid_statement"))
        }
    }

    fn attributes(&mut self, opening: &Line<'a>, entity: usize) -> Result<(), DiagramError> {
        loop {
            let Some(line) = self.next() else {
                return Err(opening.error("er_unclosed_block"));
            };
            if line.text == "}" {
                return Ok(());
            }
            if let Some(at) = uml_parser::outside_quotes(line.text, "}") {
                let before = line.text[..at].trim();
                if !before.is_empty() {
                    let attribute =
                        attribute(before).ok_or_else(|| line.error("er_invalid_attribute"))?;
                    let at = uml_parser::subslice_offset(self.source, before);
                    let mut attribute = attribute;
                    attribute.source_range = at..at + before.len();
                    self.diagram.entities[entity].attributes.push(attribute);
                }
                if !line.text[at + 1..].trim().is_empty() {
                    return Err(line.error("er_unexpected_content"));
                }
                return Ok(());
            }
            let attribute =
                attribute(line.text).ok_or_else(|| line.error("er_invalid_attribute"))?;
            let mut attribute = attribute;
            let at = uml_parser::subslice_offset(self.source, line.text);
            attribute.source_range = at..at + line.text.len();
            self.diagram.entities[entity].attributes.push(attribute);
        }
    }

    fn add_relation(&mut self, line: &Line<'a>, relation: Relation<'_>) {
        let from = self.entity(relation.from, relation.from_range, false);
        let to = self.entity(relation.to, relation.to_range, false);
        self.diagram.relations.push(ErRelation {
            from,
            to,
            from_cardinality: relation.from_cardinality,
            to_cardinality: relation.to_cardinality,
            identifying: relation.identifying,
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

struct EntityHeader<'a> {
    id: &'a str,
    label: Option<String>,
    class: Option<String>,
    block: EntityBlock,
}

enum EntityBlock {
    None,
    Empty,
    Open,
}

fn entity_header(text: &str) -> Option<Result<EntityHeader<'_>, ()>> {
    let mut cursor = 0;
    uml_parser::skip_space(text, &mut cursor);
    if !text[cursor..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
    {
        return None;
    }
    let id = uml_parser::id(text, &mut cursor)?;
    let mut label = None;
    if text[cursor..].starts_with('[') {
        label = Some(uml_parser::bracket_label(text, &mut cursor)?.0);
    }
    uml_parser::skip_space(text, &mut cursor);
    let class = uml_parser::class_suffix(text, &mut cursor);
    uml_parser::skip_space(text, &mut cursor);
    let rest = &text[cursor..];
    let block = if rest.is_empty() {
        EntityBlock::None
    } else if rest == "{}" {
        EntityBlock::Empty
    } else if rest == "{" {
        EntityBlock::Open
    } else {
        return None;
    };
    Some(Ok(EntityHeader {
        id,
        label,
        class: class.map(str::to_owned),
        block,
    }))
}

fn attribute(text: &str) -> Option<ErAttribute> {
    let mut cursor = 0;
    uml_parser::skip_space(text, &mut cursor);
    let start = cursor;
    while let Some(ch) = text[cursor..].chars().next() {
        if ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    if cursor == start {
        return None;
    }
    let kind = text[start..cursor].to_owned();
    uml_parser::skip_space(text, &mut cursor);
    let name = uml_parser::id(text, &mut cursor)?.to_owned();
    let mut keys = Vec::new();
    let mut comment = None;
    loop {
        uml_parser::skip_space(text, &mut cursor);
        if cursor == text.len() {
            break;
        }
        if text[cursor..].starts_with('"') {
            if comment.is_some() {
                return None;
            }
            let (range, after) = uml_parser::quoted(text, &mut cursor)?;
            comment = Some(unquote(&text[range]));
            cursor = after;
            continue;
        }
        let start = cursor;
        while let Some(ch) = text[cursor..].chars().next() {
            if ch.is_whitespace() {
                break;
            }
            cursor += ch.len_utf8();
        }
        for key in text[start..cursor].split(',') {
            keys.push(match key.trim() {
                "PK" => ErKey::Primary,
                "FK" => ErKey::Foreign,
                "UK" => ErKey::Unique,
                _ => return None,
            });
        }
    }
    Some(ErAttribute {
        kind,
        name,
        keys,
        comment,
        source_range: 0..text.len(),
    })
}

struct Relation<'a> {
    from: &'a str,
    to: &'a str,
    from_range: Range<usize>,
    to_range: Range<usize>,
    from_cardinality: ErCardinality,
    to_cardinality: ErCardinality,
    identifying: bool,
    label: Option<&'a str>,
    connector: &'a str,
}

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
    let connector_start = cursor;
    let from_cardinality = cardinality(head, &mut cursor).ok_or(())?;
    uml_parser::skip_space(head, &mut cursor);
    let rest = &head[cursor..];
    let identifying = if rest.starts_with("--") {
        cursor += 2;
        true
    } else if rest.starts_with("..") {
        cursor += 2;
        false
    } else {
        return Err(());
    };
    uml_parser::skip_space(head, &mut cursor);
    let to_cardinality = cardinality(head, &mut cursor).ok_or(())?;
    let connector = &head[connector_start..cursor];
    uml_parser::skip_space(head, &mut cursor);
    let to = uml_parser::id(head, &mut cursor).ok_or(())?;
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
        from_cardinality,
        to_cardinality,
        identifying,
        label,
        connector,
    })
}

fn cardinality(text: &str, cursor: &mut usize) -> Option<ErCardinality> {
    let rest = &text[*cursor..];
    let mut chars = rest.chars();
    let first = chars.next()?;
    let second = chars.next();
    if let Some(second) = second
        && "|o{}".contains(first)
        && "|o{}".contains(second)
    {
        let value = match (first, second) {
            ('|', '|') => ErCardinality::ExactlyOne,
            ('|', 'o') | ('o', '|') => ErCardinality::ZeroOrOne,
            ('}', '|') | ('|', '{') => ErCardinality::OneOrMore,
            ('}', 'o') | ('o', '{') => ErCardinality::ZeroOrMore,
            _ => return None,
        };
        *cursor += first.len_utf8() + second.len_utf8();
        return Some(value);
    }
    for (word, value) in [
        ("one or zero", ErCardinality::ZeroOrOne),
        ("zero or one", ErCardinality::ZeroOrOne),
        ("only one", ErCardinality::ExactlyOne),
        ("zero or more", ErCardinality::ZeroOrMore),
        ("one or more", ErCardinality::OneOrMore),
    ] {
        if rest.starts_with(word) {
            *cursor += word.len();
            return Some(value);
        }
    }
    None
}

pub(crate) fn parse(source: &str) -> Result<ErDiagram, DiagramError> {
    let mut parser = Parser {
        source,
        lines: uml_parser::lines(source),
        position: 0,
        directive: false,
        diagram: ErDiagram::default(),
        ids: HashMap::new(),
        defined: Vec::new(),
        assignments: HashMap::new(),
    };
    let header = parser.next().ok_or_else(|| syntax(0..0, "missing_type"))?;
    if header.text != "erDiagram" {
        return Err(header.error("er_invalid_header"));
    }
    parser.body()?;
    if parser.diagram.entities.is_empty() {
        return Err(header.error("er_empty"));
    }
    for entity in &mut parser.diagram.entities {
        if entity.class.is_none() {
            entity.class = parser.assignments.get(&entity.id).cloned();
        }
    }
    Ok(parser.diagram)
}
