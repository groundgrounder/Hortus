//! 逐行解析状态图，保留定义、边、标签和多行便签的原始字节位置。

use crate::parser::syntax;
use crate::state::*;
use crate::syntax::{is_id_char, unquote, word_tail};
use crate::{DiagramError, Direction};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone)]
struct Line<'a> {
    at: usize,
    text: &'a str,
}

impl<'a> Line<'a> {
    fn new(at: usize, raw: &'a str) -> Self {
        Self {
            at: at + raw.len() - raw.trim_start().len(),
            text: raw.trim(),
        }
    }

    fn range(&self) -> Range<usize> {
        self.at..self.at + self.text.len()
    }

    fn error(&self, reason: &str) -> DiagramError {
        syntax(self.range(), reason)
    }
}

fn lines(source: &str) -> impl Iterator<Item = Line<'_>> {
    let mut at = 0;
    source.split_inclusive('\n').map(move |raw| {
        let line = Line::new(at, raw);
        at += raw.len();
        line
    })
}

fn outside_quotes(text: &str, token: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if !quoted && text[index..].starts_with(token) {
            return Some(index);
        }
    }
    None
}

fn statement<'a>(mut line: Line<'a>, directive: &mut bool) -> Line<'a> {
    loop {
        if *directive {
            let Some(end) = line.text.find("}%%") else {
                return Line::new(line.at, "");
            };
            *directive = false;
            line = Line::new(line.at + end + 3, &line.text[end + 3..]);
        } else if let Some(rest) = line.text.strip_prefix("%%{") {
            *directive = true;
            line = Line::new(line.at + 3, rest);
        } else {
            let end = outside_quotes(line.text, "%%").unwrap_or(line.text.len());
            let text =
                line.text[..end].trim_start_matches(|ch: char| ch == ';' || ch.is_whitespace());
            return Line::new(line.at + end - text.len(), text.trim_end_matches(';'));
        }
    }
}

fn header(line: &str) -> bool {
    matches!(
        line.split(|ch: char| ch.is_whitespace() || ch == ';')
            .next(),
        Some("stateDiagram" | "stateDiagram-v2")
    )
}

/// 使用与状态图解析相同的注释和指令规则识别图头。
pub fn is_state_diagram(source: &str) -> bool {
    let mut directive = false;
    lines(source)
        .map(|line| statement(line, &mut directive))
        .find(|line| !line.text.is_empty())
        .is_some_and(|line| header(line.text))
}

#[derive(Default)]
struct Region {
    data: StateRegion,
    ids: HashMap<String, usize>,
    defined: Vec<bool>,
}

impl Region {
    fn node(&mut self, id: &str, range: Range<usize>, defined: bool) -> &mut StateNode {
        let index = *self.ids.entry(id.to_owned()).or_insert_with(|| {
            let index = self.data.nodes.len();
            let (kind, label) = match id {
                "[*] start" => (StateKind::Start, "[*]"),
                "[*] end" => (StateKind::End, "[*]"),
                _ => (StateKind::Ordinary, id),
            };
            self.data.nodes.push(StateNode {
                id: id.to_owned(),
                label: label.to_owned(),
                descriptions: Vec::new(),
                kind,
                class: None,
                source_range: range.clone(),
            });
            self.defined.push(defined);
            index
        });
        let node = &mut self.data.nodes[index];
        if defined && !self.defined[index] {
            node.source_range = range;
            self.defined[index] = true;
        }
        node
    }
}

struct Declaration<'a> {
    id: &'a str,
    class: Option<&'a str>,
    label: Option<String>,
    kind: StateKind,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(is_id_char)
}

fn endpoint(text: &str) -> Option<(&str, Option<&str>)> {
    let (id, class) = text
        .trim()
        .split_once(":::")
        .map_or((text.trim(), None), |(id, class)| {
            (id.trim_end(), Some(class.trim()))
        });
    (valid_id(id) && class.is_none_or(valid_id) || id == "[*]" && class.is_none())
        .then_some((id, class))
}

fn declaration(text: &str) -> Option<Declaration<'_>> {
    let mut label = None;
    let mut rest = text;
    if let Some(quoted) = rest.strip_prefix('"') {
        let mut escaped = false;
        let closing = quoted.char_indices().find_map(|(index, ch)| {
            let closes = !escaped && ch == '"';
            escaped = !escaped && ch == '\\';
            closes.then_some(index)
        })?;
        label = Some(unquote(&rest[..closing + 2]));
        rest = word_tail(quoted[closing + 1..].trim_start(), "as")?;
    }
    let kind = [
        ("<<choice>>", StateKind::Choice),
        ("[[choice]]", StateKind::Choice),
        ("<<fork>>", StateKind::Fork),
        ("<<join>>", StateKind::Join),
    ]
    .into_iter()
    .find_map(|(suffix, kind)| rest.strip_suffix(suffix).map(|id| (id.trim_end(), kind)));
    let (id, kind) = kind.unwrap_or((rest, StateKind::Ordinary));
    let (id, class) = endpoint(id)?;
    valid_id(id).then_some(Declaration {
        id,
        class,
        label,
        kind,
    })
}

fn property(text: &str) -> (&str, Option<&str>) {
    // 三个冒号是内联 class，单个冒号开启任意文本。
    let mut cursor = 0;
    while let Some(index) = outside_quotes(&text[cursor..], ":") {
        let at = cursor + index;
        if text[at..].starts_with(":::") {
            cursor = at + 3;
        } else {
            return (text[..at].trim_end(), Some(text[at + 1..].trim()));
        }
    }
    (text, None)
}

struct Parser<'a> {
    source: &'a str,
    lines: std::vec::IntoIter<Line<'a>>,
    pending: Option<Line<'a>>,
    diagram: StateDiagram,
    classes: HashMap<String, String>,
    directive: bool,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<Line<'a>> {
        loop {
            let inline = self.pending.is_some();
            let line = self.pending.take().or_else(|| {
                self.lines
                    .next()
                    .map(|line| statement(line, &mut self.directive))
            })?;
            if line.text.is_empty() {
                continue;
            }
            let text = line.text;
            let opening = word_tail(text, "state").and_then(|_| outside_quotes(text, "{"));
            // 正文中的冒号开启文字。在同一行的复合块中，未引用的右括号仍结束块。
            let delimited = property(text).1.is_none();
            let closing = (inline || delimited)
                .then(|| outside_quotes(text, "}"))
                .flatten();
            let semicolon = delimited.then(|| outside_quotes(text, ";")).flatten();
            let split = [
                opening.map(|at| (at + 1, at + 1)),
                closing.map(|at| if at == 0 { (1, 1) } else { (at, at) }),
                semicolon.map(|at| (at, at + 1)),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some((end, next)) = split {
                self.pending = Some(Line::new(line.at + next, &text[next..]));
                let first = Line::new(line.at, &text[..end]);
                if !first.text.is_empty() {
                    return Some(first);
                }
                continue;
            }
            return Some(line);
        }
    }

    fn regions(
        &mut self,
        opening: Option<&Line<'a>>,
        depth: usize,
    ) -> Result<(Vec<StateRegion>, usize), DiagramError> {
        let mut regions = Vec::new();
        let mut region = Region::default();
        while let Some(line) = self.next() {
            let text = line.text;
            if text == "}" {
                if opening.is_none() {
                    return Err(line.error("state_unexpected_close"));
                }
                regions.push(region.data);
                return Ok((regions, line.range().end));
            }
            if text == "--" && opening.is_some() {
                let direction = region.data.direction;
                regions.push(region.data);
                region = Region::default();
                region.data.direction = direction;
            } else if let Some(value) = word_tail(text, "direction") {
                region.data.direction = match value {
                    "TB" | "TD" => Direction::TopToBottom,
                    "BT" => Direction::BottomToTop,
                    "LR" => Direction::LeftToRight,
                    "RL" => Direction::RightToLeft,
                    _ => return Err(line.error("state_invalid_direction")),
                };
            } else if let Some(value) = word_tail(text, "title") {
                self.diagram.title = Some(unquote(value));
            } else if let Some(value) = word_tail(text, "classDef") {
                let (name, properties) = value
                    .split_once(char::is_whitespace)
                    .filter(|(name, _)| valid_id(name))
                    .ok_or_else(|| line.error("state_invalid_class_definition"))?;
                let at = subslice_offset(self.source, properties);
                let style = crate::graph_parser::parse_style(properties, at, None)?;
                self.diagram.class_defs.insert(name.to_owned(), style);
            } else if let Some(value) = word_tail(text, "class") {
                let (ids, class) = value
                    .rsplit_once(char::is_whitespace)
                    .filter(|(_, class)| valid_id(class))
                    .ok_or_else(|| line.error("state_invalid_class_assignment"))?;
                for id in ids.split(',').map(str::trim) {
                    if !valid_id(id) {
                        return Err(line.error("state_invalid_class_assignment"));
                    }
                    self.classes.insert(id.to_owned(), class.to_owned());
                }
            } else if let Some(value) = word_tail(text, "state") {
                let definition = value.strip_suffix('{').map(str::trim_end);
                let Declaration {
                    id,
                    class,
                    label,
                    mut kind,
                } = declaration(definition.unwrap_or(value))
                    .ok_or_else(|| line.error("state_invalid_declaration"))?;
                let mut range = line.range();
                if definition.is_some() {
                    if depth >= 64 {
                        return Err(line.error("state_nesting_limit"));
                    }
                    let (children, end) = self.regions(Some(&line), depth + 1)?;
                    range.end = end;
                    kind = StateKind::Composite(children);
                }
                let node = region.node(id, range.clone(), true);
                if let Some(label) = label {
                    node.label = label;
                }
                if let Some(class) = class {
                    node.class = Some(class.to_owned());
                }
                if matches!(node.kind, StateKind::Ordinary) || kind != StateKind::Ordinary {
                    node.kind = kind;
                    node.source_range = range;
                }
            } else if let Some(value) = word_tail(text, "note") {
                self.note(&mut region, &line, value)?;
            } else {
                self.relation(&mut region, &line)?;
            }
        }
        if let Some(opening) = opening {
            return Err(opening.error("state_unclosed_composite"));
        }
        regions.push(region.data);
        Ok((regions, self.source.len()))
    }

    fn note(
        &mut self,
        region: &mut Region,
        line: &Line<'a>,
        value: &str,
    ) -> Result<(), DiagramError> {
        let (head, label) = property(value);
        let words: Vec<_> = head.split_whitespace().collect();
        let (side, id) = match words.as_slice() {
            ["left", "of", id] => (StateNoteSide::Left, *id),
            ["right", "of", id] => (StateNoteSide::Right, *id),
            _ => return Err(line.error("state_invalid_note")),
        };
        if !valid_id(id) {
            return Err(line.error("state_invalid_note_target"));
        }
        let at = subslice_offset(self.source, id);
        region.node(id, at..at + id.len(), false);
        let mut range = line.range();
        let text = if let Some(label) = label {
            unquote(label)
        } else {
            let body: Vec<_> = self
                .lines
                .by_ref()
                .take_while(|next| {
                    if next.text == "end note" {
                        range.end = next.range().end;
                        false
                    } else {
                        true
                    }
                })
                .map(|next| next.text)
                .collect();
            if range.end == line.range().end {
                return Err(line.error("state_unclosed_note"));
            }
            body.join("\n")
        };
        region.data.notes.push(StateNote {
            target: region.ids[id],
            side,
            text,
            source_range: range,
        });
        Ok(())
    }

    fn relation(&self, region: &mut Region, line: &Line<'a>) -> Result<(), DiagramError> {
        let (head, label) = property(line.text);
        if let Some((a, b)) = head.split_once("-->") {
            let mut indices = [0; 2];
            for (index, text) in [a, b].into_iter().enumerate() {
                let (id, class) =
                    endpoint(text).ok_or_else(|| line.error("state_invalid_transition"))?;
                let at = subslice_offset(self.source, id);
                let range = at..at + id.len();
                let id = if id == "[*]" {
                    ["[*] start", "[*] end"][index]
                } else {
                    id
                };
                let node = region.node(id, range, false);
                if let Some(class) = class {
                    node.class = Some(class.to_owned());
                }
                indices[index] = region.ids[id];
            }
            region.data.transitions.push(StateTransition {
                from: indices[0],
                to: indices[1],
                label: label.map(unquote),
                label_range: label.map(|text| {
                    let at = subslice_offset(self.source, text);
                    at..at + text.len()
                }),
                source_range: line.range(),
            });
        } else {
            let (id, class) = endpoint(head)
                .filter(|(id, class)| valid_id(id) && (label.is_some() || class.is_some()))
                .ok_or_else(|| line.error("state_invalid_statement"))?;
            let node = region.node(id, line.range(), label.is_some());
            if let Some(class) = class {
                node.class = Some(class.to_owned());
            }
            if let Some(label) = label {
                node.descriptions.push(unquote(label));
            }
        }
        Ok(())
    }
}

fn subslice_offset(source: &str, child: &str) -> usize {
    child.as_ptr() as usize - source.as_ptr() as usize
}

fn apply_classes(region: &mut StateRegion, classes: &HashMap<String, String>) {
    for node in &mut region.nodes {
        if node.class.is_none() {
            node.class = classes.get(&node.id).cloned();
        }
        if let StateKind::Composite(regions) = &mut node.kind {
            for region in regions {
                apply_classes(region, classes);
            }
        }
    }
}

pub(crate) fn parse(source: &str) -> Result<StateDiagram, DiagramError> {
    let mut parser = Parser {
        source,
        lines: lines(source).collect::<Vec<_>>().into_iter(),
        pending: None,
        diagram: StateDiagram::default(),
        classes: HashMap::new(),
        directive: false,
    };
    let header = parser.next().ok_or_else(|| syntax(0..0, "missing_type"))?;
    if !matches!(header.text, "stateDiagram" | "stateDiagram-v2") {
        return Err(header.error("state_invalid_header"));
    }
    let (mut regions, _) = parser.regions(None, 0)?;
    let mut root = regions.pop().unwrap_or_default();
    if root.nodes.is_empty() {
        return Err(header.error("state_empty"));
    }
    apply_classes(&mut root, &parser.classes);
    parser.diagram.root = root;
    Ok(parser.diagram)
}
