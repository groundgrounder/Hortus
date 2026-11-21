//! Mermaid `sequenceDiagram` 的 AST 与解析；自适应布局见 `sequence_layout`。
//!
//! 错误契约与树形/流程图一致：所有范围都是相对图源码的 UTF-8 字节，
//! 经 `parser::parse_diagram_at` 或容器投影后指向整篇原文。

use crate::parser::{DiagramError, syntax, unsupported};
use crate::syntax::{TREE_STATEMENTS, is_id_char, skip_space, starts_word, statements};
use crate::{Color, EdgeLine, Marker};
use std::collections::HashMap;
use std::ops::Range;

/// 一段图表文字与其在图源码中的字节范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceText {
    pub text: String,
    pub range: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct SequenceParticipant {
    pub id: String,
    /// 显示名；保留 `<br/>` 供布局拆行。
    pub display: String,
    /// `actor` 声明使用人形造型，`participant` 使用胶囊。
    pub actor: bool,
    pub id_range: Range<usize>,
    /// `as 显示名` 的范围；无别名的显式声明与隐式参与者为 `None`。
    pub display_range: Option<Range<usize>>,
    /// 显式声明语句，或首次出现语句（隐式参与者）的范围。
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct SequenceMessage {
    pub from: usize,
    pub to: usize,
    pub line: EdgeLine,
    pub marker: Marker,
    pub text: SequenceText,
    /// `+` 简写：接收方在本条消息后激活。
    pub activate_to: bool,
    /// `-` 简写：发送方在本条消息后去激活。
    pub deactivate_from: bool,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoteSide {
    LeftOf(usize),
    RightOf(usize),
    Over(Vec<usize>),
}

#[derive(Clone, Debug)]
pub struct SequenceNote {
    pub side: NoteSide,
    pub text: SequenceText,
    pub source_range: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentKind {
    Loop,
    Alt,
    Opt,
    Par,
    Critical,
    Break,
    Rect,
}

impl FragmentKind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Alt => "alt",
            Self::Opt => "opt",
            Self::Par => "par",
            Self::Critical => "critical",
            Self::Break => "break",
            Self::Rect => "rect",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SequenceRow {
    Message(SequenceMessage),
    Note(SequenceNote),
    Activate {
        participant: usize,
        source_range: Range<usize>,
    },
    Deactivate {
        participant: usize,
        source_range: Range<usize>,
    },
    FragmentStart {
        kind: FragmentKind,
        label: SequenceText,
        /// `rect rgb(...)` 的背景色；其余块为 `None`。
        fill: Option<Color>,
        source_range: Range<usize>,
    },
    /// `else` / `and` / `option` 分支分隔。
    FragmentDivider {
        label: SequenceText,
        source_range: Range<usize>,
    },
    FragmentEnd {
        source_range: Range<usize>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct SequenceDiagram {
    pub title: Option<SequenceText>,
    pub autonumber: bool,
    pub participants: Vec<SequenceParticipant>,
    pub rows: Vec<SequenceRow>,
}

/// 解析 `sequenceDiagram` 图；范围相对 `source` 的字节偏移。
pub(crate) fn parse_sequence(source: &str) -> Result<SequenceDiagram, DiagramError> {
    let statements = statements(source, TREE_STATEMENTS);
    let Some((header_at, header)) = statements.first().copied() else {
        return Err(syntax(0..0, "missing_type"));
    };
    if !starts_word(header, "sequenceDiagram") {
        return Err(DiagramError::Unsupported {
            diagram_type: header.split_whitespace().next().unwrap_or("").into(),
        });
    }
    let mut parser = Parser::default();
    for (at, statement) in statements.into_iter().skip(1) {
        parser.parse_statement(statement, at)?;
    }
    if let Some((kind, range)) = parser.open_fragments.first() {
        return Err(syntax(range.clone(), &format!("unclosed_{}", kind.word())));
    }
    let diagram = parser.diagram;
    if diagram.participants.is_empty() {
        return Err(syntax(
            header_at..header_at + header.len(),
            "empty_sequence",
        ));
    }
    Ok(diagram)
}

/// 参与者 ID 允许字母、数字、`-`、`_` 与全部非 ASCII 字符（中文 ID）。
fn id_char(ch: char) -> bool {
    is_id_char(ch) || !ch.is_ascii()
}

const ARROWS: &[(&str, EdgeLine, Marker)] = &[
    ("-->>", EdgeLine::Dotted, Marker::Arrow),
    ("-->", EdgeLine::Dotted, Marker::None),
    ("--x", EdgeLine::Dotted, Marker::Cross),
    ("--)", EdgeLine::Dotted, Marker::Circle),
    ("->>", EdgeLine::Solid, Marker::Arrow),
    ("->", EdgeLine::Solid, Marker::None),
    ("-x", EdgeLine::Solid, Marker::Cross),
    ("-)", EdgeLine::Solid, Marker::Circle),
];

fn find_arrow(text: &str) -> Option<(usize, &'static str, EdgeLine, Marker)> {
    for (at, ch) in text.char_indices() {
        if ch != '-' {
            continue;
        }
        for (token, line, marker) in ARROWS {
            if text[at..].starts_with(*token) {
                return Some((at, token, *line, *marker));
            }
        }
    }
    None
}

fn scan_id(text: &str, cursor: &mut usize) -> Option<Range<usize>> {
    skip_space(text, cursor);
    let start = *cursor;
    let end = start
        + text[start..]
            .chars()
            .take_while(|ch| id_char(*ch))
            .map(char::len_utf8)
            .sum::<usize>();
    *cursor = end;
    (end > start).then_some(start..end)
}

/// 语句尾部去掉首尾空白后的切片与相对语句起点的范围。
fn trimmed_tail(text: &str, from: usize) -> (&str, Range<usize>) {
    let raw = &text[from..];
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    (trimmed, from + lead..from + lead + trimmed.len())
}

#[derive(Default)]
struct Parser {
    diagram: SequenceDiagram,
    ids: HashMap<String, usize>,
    /// 与 `participants` 对齐的激活栈深；用于校验 deactivate 的平衡。
    activation: Vec<usize>,
    open_fragments: Vec<(FragmentKind, Range<usize>)>,
}

/// `Note` 大小写不敏感的词头；返回词长。
fn note_head(text: &str) -> Option<usize> {
    let head = text.split_whitespace().next()?;
    head.eq_ignore_ascii_case("note").then_some(head.len())
}

impl Parser {
    fn parse_statement(&mut self, text: &str, at: usize) -> Result<(), DiagramError> {
        let statement = at..at + text.len();
        if starts_word(text, "title") {
            let (value, range) = trimmed_tail(text, "title".len());
            self.diagram.title = Some(SequenceText {
                text: value.into(),
                range: at + range.start..at + range.end,
            });
            return Ok(());
        }
        if starts_word(text, "autonumber") {
            let (value, _) = trimmed_tail(text, "autonumber".len());
            self.diagram.autonumber = value.trim() != "off";
            return Ok(());
        }
        if starts_word(text, "participant") || starts_word(text, "actor") {
            return self.parse_participant(text, at, starts_word(text, "actor"));
        }
        if note_head(text).is_some() {
            return self.parse_note(text, at);
        }
        if starts_word(text, "activate") || starts_word(text, "deactivate") {
            return self.parse_activation(text, at, starts_word(text, "activate"));
        }
        for (word, kind) in [
            ("loop", FragmentKind::Loop),
            ("alt", FragmentKind::Alt),
            ("opt", FragmentKind::Opt),
            ("par", FragmentKind::Par),
            ("critical", FragmentKind::Critical),
            ("break", FragmentKind::Break),
            ("rect", FragmentKind::Rect),
        ] {
            if starts_word(text, word) {
                let (value, range) = trimmed_tail(text, word.len());
                let fill = (kind == FragmentKind::Rect)
                    .then(|| Color::parse_css(value))
                    .flatten();
                let label = if fill.is_some() { "" } else { value };
                self.open_fragments.push((kind, statement.clone()));
                self.diagram.rows.push(SequenceRow::FragmentStart {
                    kind,
                    label: SequenceText {
                        text: label.into(),
                        range: at + range.start..at + range.end,
                    },
                    fill,
                    source_range: statement,
                });
                return Ok(());
            }
        }
        for (word, open) in [("else", "alt"), ("and", "par"), ("option", "critical")] {
            if starts_word(text, word) {
                let mismatch = self
                    .open_fragments
                    .last()
                    .is_none_or(|(kind, _)| kind.word() != open);
                if mismatch {
                    return Err(syntax(statement, &format!("{word}_without_{open}")));
                }
                let (value, range) = trimmed_tail(text, word.len());
                self.diagram.rows.push(SequenceRow::FragmentDivider {
                    label: SequenceText {
                        text: value.into(),
                        range: at + range.start..at + range.end,
                    },
                    source_range: statement,
                });
                return Ok(());
            }
        }
        if text == "end" {
            if self.open_fragments.pop().is_none() {
                return Err(syntax(statement, "unexpected_end"));
            }
            self.diagram.rows.push(SequenceRow::FragmentEnd {
                source_range: statement,
            });
            return Ok(());
        }
        for word in ["box", "create", "destroy", "links", "link"] {
            if starts_word(text, word) {
                return Err(unsupported(statement, word));
            }
        }
        self.parse_message(text, at)
    }

    fn parse_participant(
        &mut self,
        text: &str,
        at: usize,
        actor: bool,
    ) -> Result<(), DiagramError> {
        let statement = at..at + text.len();
        let mut cursor = if actor {
            "actor".len()
        } else {
            "participant".len()
        };
        let Some(id_range) = scan_id(text, &mut cursor) else {
            return Err(syntax(statement, "participant_id"));
        };
        let id: String = text[id_range.clone()].into();
        skip_space(text, &mut cursor);
        let display = text[cursor..]
            .starts_with("as")
            .then(|| {
                text[cursor + "as".len()..]
                    .starts_with(char::is_whitespace)
                    .then(|| trimmed_tail(text, cursor + "as".len()))
            })
            .flatten();
        let (display, display_range) = match display {
            Some((value, range)) => (value.into(), Some(at + range.start..at + range.end)),
            None => (text[id_range.clone()].into(), None),
        };
        match self.ids.get(&id).copied() {
            // 重复声明同一 ID：别名与造型以显式声明覆盖，列不重复。
            Some(index) => {
                let participant = &mut self.diagram.participants[index];
                participant.display = display;
                participant.display_range = display_range;
                participant.actor = actor;
                participant.source_range = statement;
            }
            None => {
                let index = self.diagram.participants.len();
                self.diagram.participants.push(SequenceParticipant {
                    id: id.clone(),
                    display,
                    actor,
                    id_range: at + id_range.start..at + id_range.end,
                    display_range,
                    source_range: statement,
                });
                self.ids.insert(id, index);
                self.activation.push(0);
            }
        }
        Ok(())
    }

    /// 消息或 Note 中首次出现的参与者按出现顺序自动加入。
    fn mention(&mut self, id: &str, id_range: Range<usize>, statement: Range<usize>) -> usize {
        if let Some(&index) = self.ids.get(id) {
            return index;
        }
        let index = self.diagram.participants.len();
        self.diagram.participants.push(SequenceParticipant {
            id: id.into(),
            display: id.into(),
            actor: false,
            id_range,
            display_range: None,
            source_range: statement,
        });
        self.ids.insert(id.into(), index);
        self.activation.push(0);
        index
    }

    fn existing(&self, id: &str, statement: Range<usize>) -> Result<usize, DiagramError> {
        self.ids
            .get(id)
            .copied()
            .ok_or_else(|| syntax(statement, "unknown_participant"))
    }

    fn parse_note(&mut self, text: &str, at: usize) -> Result<(), DiagramError> {
        let statement = at..at + text.len();
        let head = note_head(text).unwrap();
        let mut cursor = head;
        skip_space(text, &mut cursor);
        let lower = text.to_ascii_lowercase();
        let over = if lower[cursor..].starts_with("left of") {
            cursor += "left of".len();
            false
        } else if lower[cursor..].starts_with("right of") {
            cursor += "right of".len();
            false
        } else if lower[cursor..].starts_with("over") {
            cursor += "over".len();
            true
        } else {
            return Err(syntax(statement, "note_placement"));
        };
        let left = lower[..cursor].ends_with("left of");
        let mut participants = Vec::new();
        loop {
            let Some(id_range) = scan_id(text, &mut cursor) else {
                return Err(syntax(statement, "note_participant"));
            };
            let id = &text[id_range.clone()];
            participants.push(self.mention(
                id,
                at + id_range.start..at + id_range.end,
                statement.clone(),
            ));
            if text[cursor..].starts_with(',') {
                cursor += 1;
                continue;
            }
            break;
        }
        let side = if over {
            NoteSide::Over(participants)
        } else {
            if participants.len() > 1 {
                return Err(syntax(statement, "note_participants"));
            }
            if left {
                NoteSide::LeftOf(participants[0])
            } else {
                NoteSide::RightOf(participants[0])
            }
        };
        skip_space(text, &mut cursor);
        if !text[cursor..].starts_with(':') {
            return Err(syntax(statement, "note_colon"));
        }
        let (value, range) = trimmed_tail(text, cursor + 1);
        self.diagram.rows.push(SequenceRow::Note(SequenceNote {
            side,
            text: SequenceText {
                text: value.into(),
                range: at + range.start..at + range.end,
            },
            source_range: statement,
        }));
        Ok(())
    }

    fn parse_activation(
        &mut self,
        text: &str,
        at: usize,
        activate: bool,
    ) -> Result<(), DiagramError> {
        let statement = at..at + text.len();
        let mut cursor = if activate {
            "activate".len()
        } else {
            "deactivate".len()
        };
        let Some(id_range) = scan_id(text, &mut cursor) else {
            return Err(syntax(statement, "activation_participant"));
        };
        let id = &text[id_range];
        let participant = self.existing(id, statement.clone())?;
        if activate {
            self.activation[participant] += 1;
            self.diagram.rows.push(SequenceRow::Activate {
                participant,
                source_range: statement,
            });
        } else {
            if self.activation[participant] == 0 {
                return Err(syntax(statement, "inactive_deactivate"));
            }
            self.activation[participant] -= 1;
            self.diagram.rows.push(SequenceRow::Deactivate {
                participant,
                source_range: statement,
            });
        }
        Ok(())
    }

    fn parse_message(&mut self, text: &str, at: usize) -> Result<(), DiagramError> {
        let statement = at..at + text.len();
        let Some((arrow_at, token, line, marker)) = find_arrow(text) else {
            return Err(syntax(statement, "unknown_statement"));
        };
        let from_raw = text[..arrow_at].trim();
        if from_raw.is_empty() || !from_raw.chars().all(id_char) {
            return Err(syntax(statement, "message_participant"));
        }
        let from = self.mention(
            from_raw,
            at + arrow_at - from_raw.len()..at + arrow_at,
            statement.clone(),
        );
        let mut cursor = arrow_at + token.len();
        skip_space(text, &mut cursor);
        let (activate_to, deactivate_from) = match text[cursor..].chars().next() {
            Some('+') => {
                cursor += 1;
                skip_space(text, &mut cursor);
                (true, false)
            }
            Some('-') => {
                cursor += 1;
                skip_space(text, &mut cursor);
                (false, true)
            }
            _ => (false, false),
        };
        let Some(to_range) = scan_id(text, &mut cursor) else {
            return Err(syntax(statement, "message_participant"));
        };
        let to = self.mention(
            &text[to_range.clone()],
            at + to_range.start..at + to_range.end,
            statement.clone(),
        );
        if !text[cursor..].starts_with(':') {
            return Err(syntax(statement, "message_colon"));
        }
        let (value, range) = trimmed_tail(text, cursor + 1);
        if deactivate_from && self.activation[from] == 0 {
            return Err(syntax(statement, "inactive_deactivate"));
        }
        if deactivate_from {
            self.activation[from] -= 1;
        }
        if activate_to {
            self.activation[to] += 1;
        }
        self.diagram
            .rows
            .push(SequenceRow::Message(SequenceMessage {
                from,
                to,
                line,
                marker,
                text: SequenceText {
                    text: value.into(),
                    range: at + range.start..at + range.end,
                },
                activate_to,
                deactivate_from,
                source_range: statement,
            }));
        Ok(())
    }
}

impl SequenceDiagram {
    /// 把图内全部范围映射到整篇原文；由 `parser::Diagram::map_ranges` 调用。
    pub(crate) fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        if let Some(title) = &mut self.title {
            title.range = map(title.range.clone());
        }
        for participant in &mut self.participants {
            participant.id_range = map(participant.id_range.clone());
            if let Some(range) = &mut participant.display_range {
                *range = map(range.clone());
            }
            participant.source_range = map(participant.source_range.clone());
        }
        for row in &mut self.rows {
            match row {
                SequenceRow::Message(message) => {
                    message.text.range = map(message.text.range.clone());
                    message.source_range = map(message.source_range.clone());
                }
                SequenceRow::Note(note) => {
                    note.text.range = map(note.text.range.clone());
                    note.source_range = map(note.source_range.clone());
                }
                SequenceRow::Activate { source_range, .. }
                | SequenceRow::Deactivate { source_range, .. }
                | SequenceRow::FragmentEnd { source_range } => {
                    *source_range = map(source_range.clone());
                }
                SequenceRow::FragmentStart {
                    label,
                    source_range,
                    ..
                }
                | SequenceRow::FragmentDivider {
                    label,
                    source_range,
                } => {
                    label.range = map(label.range.clone());
                    *source_range = map(source_range.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(source: &str) -> DiagramError {
        parse_sequence(source).unwrap_err()
    }

    #[test]
    fn declares_participants_in_order_with_aliases_and_actors() {
        let source =
            "sequenceDiagram\nparticipant U as 你\nactor M as Hortus\nU->>M: 发送\nM->>T: 转交\n";
        let diagram = parse_sequence(source).unwrap();
        let ids: Vec<&str> = diagram.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["U", "M", "T"]);
        assert_eq!(diagram.participants[0].display, "你");
        assert!(diagram.participants[0].display_range.is_some());
        assert!(!diagram.participants[0].actor);
        assert!(diagram.participants[1].actor);
        // T 是隐式参与者：显示名等于 ID，范围指向首次出现的消息。
        assert_eq!(diagram.participants[2].display, "T");
        assert_eq!(
            &source[diagram.participants[2].source_range.clone()],
            "M->>T: 转交"
        );
        // 重复声明：别名覆盖、不重复列。
        let diagram = parse_sequence(
            "sequenceDiagram\nA->>B: 首次\nparticipant A as 应用\nparticipant B as 备份\n",
        )
        .unwrap();
        assert_eq!(diagram.participants.len(), 2);
        assert_eq!(diagram.participants[0].display, "应用");
        assert_eq!(diagram.participants[1].display, "备份");
    }

    #[test]
    fn parses_all_eight_arrows_and_self_calls() {
        let source = concat!(
            "sequenceDiagram\n",
            "A->B: 无箭头\n",
            "A->>B: 实心\n",
            "A-xB: 叉号\n",
            "A-)B: 异步\n",
            "A-->B: 虚线无箭头\n",
            "A-->>B: 虚线实心\n",
            "A--xB: 虚线叉号\n",
            "A--)B: 虚线异步\n",
            "A->>A: 自调用\n",
            "A->>A:\n",
        );
        let diagram = parse_sequence(source).unwrap();
        let messages: Vec<&SequenceMessage> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), 10);
        let expected = [
            (EdgeLine::Solid, Marker::None),
            (EdgeLine::Solid, Marker::Arrow),
            (EdgeLine::Solid, Marker::Cross),
            (EdgeLine::Solid, Marker::Circle),
            (EdgeLine::Dotted, Marker::None),
            (EdgeLine::Dotted, Marker::Arrow),
            (EdgeLine::Dotted, Marker::Cross),
            (EdgeLine::Dotted, Marker::Circle),
            (EdgeLine::Solid, Marker::Arrow),
            (EdgeLine::Solid, Marker::Arrow),
        ];
        for (message, (line, marker)) in messages.iter().zip(expected) {
            assert_eq!(message.line, line);
            assert_eq!(message.marker, marker);
        }
        assert_eq!(messages[8].from, messages[8].to);
        assert_eq!(messages[9].text.text, "");
        for (index, message) in messages.iter().enumerate() {
            assert!(source.is_char_boundary(message.source_range.start));
            assert!(source.is_char_boundary(message.source_range.end));
            assert_eq!(
                &source[message.source_range.clone()],
                source.lines().nth(1 + index).unwrap().trim()
            );
        }
    }

    #[test]
    fn parses_activation_shorthand_statements_and_nesting() {
        let source = concat!(
            "sequenceDiagram\n",
            "A->>+B: 请求\n",
            "B->>+B: 自处理\n",
            "B-->>-A: 响应\n",
            "activate A\n",
            "A->>C: 传递\n",
            "deactivate A\n",
        );
        let diagram = parse_sequence(source).unwrap();
        let messages: Vec<&SequenceMessage> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert!(messages[0].activate_to);
        assert!(!messages[0].deactivate_from);
        assert!(messages[2].deactivate_from);
        let activations: Vec<(usize, bool)> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::Activate { participant, .. } => Some((*participant, true)),
                SequenceRow::Deactivate { participant, .. } => Some((*participant, false)),
                _ => None,
            })
            .collect();
        assert_eq!(activations, vec![(0, true), (0, false)]);
        // `-` 作用于发送方：B 已由 +B 激活，B-->>-A 合法配对。
        let source = "sequenceDiagram\nA->>+B: 请求\nB-->>-A: 响应\n";
        let diagram = parse_sequence(source).unwrap();
        let messages: Vec<&SequenceMessage> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert!(messages[1].deactivate_from);
        // 发送方从未激活时 `-` 报错，不静默吞掉。
        let source = "sequenceDiagram\nA->>B: 请求\nB-->>-A: 响应\n";
        assert!(matches!(
            err(source),
            DiagramError::Syntax { kind, .. } if kind == "inactive_deactivate"
        ));
    }

    #[test]
    fn rejects_deactivate_without_activation_or_unknown_participant() {
        for source in [
            "sequenceDiagram\nA->>B: x\ndeactivate A\n",
            "sequenceDiagram\nA-->>-B: x\n",
            "sequenceDiagram\ndeactivate Ghost\n",
            "sequenceDiagram\nactivate Ghost\n",
        ] {
            let error = err(source);
            assert!(
                matches!(&error, DiagramError::Syntax { range, .. } if !range.is_empty()),
                "{source} -> {error:?}"
            );
        }
    }

    #[test]
    fn parses_all_note_placements_with_ranges() {
        let source = concat!(
            "sequenceDiagram\n",
            "participant A\n",
            "participant B\n",
            "Note left of A: 左侧便签<br/>第二行\n",
            "Note right of B: 右侧便签\n",
            "Note over A: 单列便签\n",
            "Note over A,B: 跨列便签\n",
        );
        let diagram = parse_sequence(source).unwrap();
        let notes: Vec<&SequenceNote> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::Note(note) => Some(note),
                _ => None,
            })
            .collect();
        assert_eq!(notes.len(), 4);
        assert_eq!(notes[0].side, NoteSide::LeftOf(0));
        assert_eq!(notes[1].side, NoteSide::RightOf(1));
        assert_eq!(notes[2].side, NoteSide::Over(vec![0]));
        assert_eq!(notes[3].side, NoteSide::Over(vec![0, 1]));
        assert!(notes[0].text.text.contains("<br/>"));
        for note in notes {
            assert!(source.is_char_boundary(note.source_range.start));
            assert!(source.is_char_boundary(note.source_range.end));
        }
        // Note 中的新参与者按首次出现顺序自动加入。
        let diagram = parse_sequence("sequenceDiagram\nNote over 新角色: 说明\n").unwrap();
        assert_eq!(diagram.participants[0].id, "新角色");
    }

    #[test]
    fn parses_nested_fragments_dividers_and_rect_colors() {
        let source = concat!(
            "sequenceDiagram\n",
            "A->>B: 开始\n",
            "alt 可用\n",
            "  A->>B: 走主线\n",
            "else 不可用\n",
            "  par 并行一\n",
            "    B->>B: 自检\n",
            "  and 并行二\n",
            "    B-->>A: 汇报\n",
            "  end\n",
            "critical 关键段\n",
            "  option 兜底\n",
            "    A->>A: 降级\n",
            "  end\n",
            "end\n",
            "rect rgb(200, 210, 240)\n",
            "  A->>B: 后处理\n",
            "end\n",
            "opt 可选\n",
            "end\n",
            "break 中止\n",
            "end\n",
            "loop 循环\n",
            "end\n",
        );
        let diagram = parse_sequence(source).unwrap();
        let kinds: Vec<FragmentKind> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::FragmentStart { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                FragmentKind::Alt,
                FragmentKind::Par,
                FragmentKind::Critical,
                FragmentKind::Rect,
                FragmentKind::Opt,
                FragmentKind::Break,
                FragmentKind::Loop,
            ]
        );
        let rect = diagram
            .rows
            .iter()
            .find_map(|row| match row {
                SequenceRow::FragmentStart { fill, .. } => *fill,
                _ => None,
            })
            .unwrap();
        assert!((rect.r - 200.0 / 255.0).abs() < 0.01);
        let dividers: Vec<&str> = diagram
            .rows
            .iter()
            .filter_map(|row| match row {
                SequenceRow::FragmentDivider { label, .. } => Some(label.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dividers, vec!["不可用", "并行二", "兜底"]);
    }

    #[test]
    fn rejects_fragment_errors_with_accurate_ranges() {
        let unclosed = "sequenceDiagram\nA->>B: x\nalt 条件\nA->>B: y\n";
        assert!(matches!(
            err(unclosed),
            DiagramError::Syntax { range, kind } if kind == "unclosed_alt"
                && &unclosed[range.clone()] == "alt 条件"
        ));
        let stray = "sequenceDiagram\nA->>B: x\nend\n";
        assert!(matches!(
            err(stray),
            DiagramError::Syntax { range, .. } if &stray[range.clone()] == "end"
        ));
        let mismatch = "sequenceDiagram\nA->>B: x\nloop 循环\nelse 分支\nend\n";
        assert!(matches!(
            err(mismatch),
            DiagramError::Syntax { kind, .. } if kind == "else_without_alt"
        ));
        for source in [
            "sequenceDiagram",
            "sequenceDiagram\n%% 只有注释",
            "sequenceDiagram\ntitle 没有参与者",
        ] {
            assert!(
                matches!(err(source), DiagramError::Syntax { kind, .. } if kind == "empty_sequence"),
                "{source}"
            );
        }
        for source in [
            "sequenceDiagram\nA =>> B: 坏箭头\n",
            "sequenceDiagram\nA->>B 缺冒号\n",
            "sequenceDiagram\nA->> : 无接收方\n",
            "sequenceDiagram\n随机文字\n",
            "sequenceDiagram\nNote behind A: 未知位置\n",
        ] {
            assert!(
                matches!(err(source), DiagramError::Syntax { .. }),
                "{source} 应为语法错误"
            );
        }
    }

    #[test]
    fn reports_unsupported_constructs_with_ranges() {
        for (source, kind) in [
            ("sequenceDiagram\nbox 蓝色 分组\nA->>B: x\nend\n", "box"),
            ("sequenceDiagram\ncreate participant B\n", "create"),
            ("sequenceDiagram\nA->>B: x\ndestroy B\n", "destroy"),
            ("sequenceDiagram\nlinks A: {\"x\": \"y\"}\n", "links"),
        ] {
            assert!(
                matches!(err(source), DiagramError::UnsupportedConstruct { range, kind: k } if k == kind
                    && source.is_char_boundary(range.start)),
                "{source}"
            );
        }
    }

    #[test]
    fn accepts_titles_directives_comments_and_autonumber_off() {
        let source = concat!(
            "%%{init: {'theme':'dark'}}%%\n",
            "sequenceDiagram\n",
            "%% 注释行\n",
            "autonumber\n",
            "title 协作流程\n",
            "A->>B: 消息 %% 行内注释\n",
            "autonumber off\n",
        );
        let diagram = parse_sequence(source).unwrap();
        assert_eq!(diagram.title.as_ref().unwrap().text, "协作流程");
        assert_eq!(
            &source[diagram.title.as_ref().unwrap().range.clone()],
            "协作流程"
        );
        // 末行的 `autonumber off` 覆盖前面的开启。
        assert!(!diagram.autonumber);
        let message = match &diagram.rows[0] {
            SequenceRow::Message(message) => message,
            _ => panic!("首行应是消息"),
        };
        assert_eq!(message.text.text, "消息");
        assert_eq!(&source[message.source_range.clone()], "A->>B: 消息");
        assert!(
            parse_sequence("sequenceDiagram\nautonumber\nA->>B: x\n")
                .unwrap()
                .autonumber
        );
        assert!(
            !parse_sequence("sequenceDiagram\nA->>B: x\nautonumber off\n")
                .unwrap()
                .autonumber
        );
    }
}
