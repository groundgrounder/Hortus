use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagramError {
    Unsupported { diagram_type: String },
    NotATree { multi_parent: Vec<String> },
    Syntax { range: Range<usize>, kind: String },
    MissingBlock { requested: usize },
}

impl fmt::Display for DiagramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { diagram_type } => write!(f, "Unsupported: {diagram_type}"),
            Self::NotATree { multi_parent } => write!(f, "NotATree: multi_parent={multi_parent:?}"),
            Self::Syntax { range, kind } => {
                write!(f, "Syntax ({}..{}): {kind}", range.start, range.end)
            }
            Self::MissingBlock { requested } => write!(f, "MissingBlock: requested={requested}"),
        }
    }
}

impl std::error::Error for DiagramError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassStyle {
    pub stroke: Option<String>,
    pub fill: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub label: String,
    /// 入边标签，在节点标题前以弱化文字显示。
    pub prefix: Option<String>,
    /// 完整节点定义的 UTF-8 字节范围；只有引用时使用首次引用。
    pub source_range: Range<usize>,
    pub class: Option<String>,
    pub children: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: usize,
    pub class_defs: HashMap<String, ClassStyle>,
    pub inline_styles: HashMap<String, ClassStyle>,
}

impl Tree {
    pub fn root_style(&self) -> Option<&ClassStyle> {
        let root = &self.nodes[self.root];
        self.inline_styles.get(&root.id).or_else(|| {
            root.class
                .as_ref()
                .and_then(|class| self.class_defs.get(class))
        })
    }
}

#[derive(Clone, Debug)]
pub struct MermaidBlock<'a> {
    pub fence_range: Range<usize>,
    /// 图表正文；引用块和列表容器中的围栏会去掉每行的容器前缀。
    pub source: Cow<'a, str>,
    /// 正文在原文中的绝对范围；容器内的正文不是连续切片，节点范围见 [`MermaidBlock::parse`]。
    pub source_range: Range<usize>,
    /// 围栏位于引用块、列表等容器内，`source` 已去掉容器前缀。
    pub container: bool,
    projection: Projection,
}

/// 把去掉容器前缀后的正文偏移映射回原文的绝对 UTF-8 偏移。
#[derive(Clone, Debug, Default)]
struct Projection {
    /// (正文起点, 原文起点, 原文中可一一对应的字节数)
    segments: Vec<(usize, usize, usize)>,
    length: usize,
    end: usize,
}

impl Projection {
    fn map(&self, offset: usize) -> usize {
        if offset >= self.length {
            return self.end;
        }
        let index = self
            .segments
            .partition_point(|(start, _, _)| *start <= offset)
            .saturating_sub(1);
        let (start, raw, length) = self.segments[index];
        raw + (offset - start).min(length)
    }

    fn map_range(&self, range: Range<usize>) -> Range<usize> {
        self.map(range.start)..self.map(range.end)
    }
}

impl MermaidBlock<'_> {
    /// 解析图表，返回的节点范围指向整篇原文。
    pub fn parse(&self) -> Result<Tree, DiagramError> {
        let map = |range| self.projection.map_range(range);
        match parse_relative(&self.source) {
            Ok(mut tree) => {
                tree.map_ranges(map);
                Ok(tree)
            }
            Err(error) => Err(error.map_ranges(map)),
        }
    }
}

struct FenceBuilder {
    fence_start: usize,
    body_start: usize,
    body: String,
    segments: Vec<(usize, usize, usize)>,
    last_text_end: usize,
    container: bool,
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index + 1)
}

/// 识别全部围栏；保留完整围栏范围，供原生文本布局直接映射。
/// 围栏可位于引用块、Obsidian callout 或列表项内，正文按 CommonMark 去掉容器前缀。
pub fn mermaid_blocks(markdown: &str) -> Vec<MermaidBlock<'_>> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    let mut fence: Option<FenceBuilder> = None;
    for (event, range) in Parser::new_ext(markdown, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                if info.split_whitespace().next() == Some("mermaid") {
                    let body_start = line_end(markdown, range.start);
                    fence = Some(FenceBuilder {
                        fence_start: line_start(markdown, range.start),
                        body_start,
                        body: String::new(),
                        segments: Vec::new(),
                        last_text_end: body_start,
                        container: depth > 0,
                    });
                }
                depth += 1;
            }
            Event::End(TagEnd::CodeBlock) => {
                depth = depth.saturating_sub(1);
                if let Some(builder) = fence.take() {
                    // 关闭围栏之后还有内容；未闭合的围栏 End 就落在正文末尾。
                    let closed = range.end > builder.last_text_end;
                    let fence_end = if closed {
                        line_end(markdown, range.end)
                    } else {
                        markdown.len()
                    };
                    let source_range = builder.body_start..builder.last_text_end;
                    let length = builder.body.len();
                    let source = match markdown.get(source_range.clone()) {
                        Some(raw) if raw == builder.body => Cow::Borrowed(raw),
                        _ => Cow::Owned(builder.body),
                    };
                    blocks.push(MermaidBlock {
                        fence_range: builder.fence_start..fence_end,
                        source,
                        source_range,
                        container: builder.container,
                        projection: Projection {
                            segments: builder.segments,
                            length,
                            end: builder.last_text_end,
                        },
                    });
                }
            }
            Event::Text(text) if fence.is_some() => {
                let builder = fence.as_mut().unwrap();
                builder
                    .segments
                    .push((builder.body.len(), range.start, range.len()));
                builder.body.push_str(&text);
                builder.last_text_end = range.end;
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    blocks
}

pub fn mermaid_block(markdown: &str, requested: usize) -> Result<MermaidBlock<'_>, DiagramError> {
    mermaid_blocks(markdown)
        .into_iter()
        .nth(requested.wrapping_sub(1))
        .ok_or(DiagramError::MissingBlock { requested })
}

pub fn parse(source: &str) -> Result<Tree, DiagramError> {
    parse_relative(source)
}

/// 解析相对 `offset` 的图表源码；返回的节点与错误范围都加上 `offset`。
pub fn parse_at(source: &str, offset: usize) -> Result<Tree, DiagramError> {
    let shift = |range: Range<usize>| range.start + offset..range.end + offset;
    match parse_relative(source) {
        Ok(mut tree) => {
            tree.map_ranges(shift);
            Ok(tree)
        }
        Err(error) => Err(error.map_ranges(shift)),
    }
}

fn parse_relative(source: &str) -> Result<Tree, DiagramError> {
    let statements = statements(source);
    let Some((header_at, header)) = statements.first().copied() else {
        return Err(syntax(0..0, "missing_type"));
    };
    let kind = header.split_whitespace().next().unwrap_or("");
    if !matches!(kind, "flowchart" | "graph" | "mindmap") {
        return Err(DiagramError::Unsupported {
            diagram_type: kind.into(),
        });
    }
    let mut builder = Builder::default();
    if kind == "mindmap" {
        parse_mindmap(
            &source[header_at + header.len()..],
            header_at + header.len(),
            &mut builder,
        )?;
    } else {
        let direction = header.strip_prefix(kind).unwrap().trim();
        if !matches!(direction, "TB" | "TD" | "BT" | "LR" | "RL") {
            return Err(syntax(
                header_at..header_at + header.len(),
                "invalid_direction",
            ));
        }
        for (at, statement) in statements.into_iter().skip(1) {
            parse_flow_statement(statement, at, &mut builder)?;
        }
    }
    builder.finish()
}

impl Tree {
    fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize>) {
        for node in &mut self.nodes {
            node.source_range = map(node.source_range.clone());
        }
    }
}

impl DiagramError {
    fn map_ranges(self, map: impl Fn(Range<usize>) -> Range<usize>) -> Self {
        match self {
            Self::Syntax { range, kind } => Self::Syntax {
                range: map(range),
                kind,
            },
            other => other,
        }
    }
}

fn syntax(range: Range<usize>, kind: &str) -> DiagramError {
    DiagramError::Syntax {
        range,
        kind: kind.into(),
    }
}

// 只在形状、引号以外切分语句，保留原始字节位置。注释中的分号不影响解析。
fn statements(source: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut comment = false;
    let bytes = source.as_bytes();
    for (i, ch) in source.char_indices() {
        if comment {
            if ch == '\n' {
                comment = false;
                start = i + 1;
            }
            continue;
        }
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
        if stack.is_empty() && bytes[i..].starts_with(b"%%") {
            push_statement(&mut result, source, start, i);
            comment = true;
            continue;
        }
        match ch {
            '[' | '(' | '{' => stack.push(ch),
            ']' | ')' | '}' => {
                stack.pop();
            }
            '\n' | ';' if stack.is_empty() => {
                push_statement(&mut result, source, start, i);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !comment {
        push_statement(&mut result, source, start, source.len());
    }
    result
}

fn push_statement<'a>(
    result: &mut Vec<(usize, &'a str)>,
    source: &'a str,
    start: usize,
    end: usize,
) {
    let raw = &source[start..end];
    let text = raw.trim();
    if !text.is_empty() {
        result.push((start + raw.len() - raw.trim_start().len(), text));
    }
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    ids: HashMap<String, usize>,
    edges: Vec<(usize, usize, Option<String>)>,
    classes: HashMap<String, ClassStyle>,
    inline_styles: HashMap<String, ClassStyle>,
}

impl Builder {
    fn node(
        &mut self,
        id: &str,
        label: Option<String>,
        class: Option<String>,
        range: Range<usize>,
    ) -> usize {
        let index = *self.ids.entry(id.into()).or_insert_with(|| {
            self.nodes.push(Node {
                id: id.into(),
                label: id.into(),
                prefix: None,
                source_range: range.clone(),
                class: None,
                children: Vec::new(),
            });
            self.nodes.len() - 1
        });
        let node = &mut self.nodes[index];
        if let Some(label) = label {
            node.label = label;
            node.source_range = range;
        }
        if class.is_some() {
            node.class = class;
        }
        index
    }

    fn finish(mut self) -> Result<Tree, DiagramError> {
        let mut parents = vec![HashSet::new(); self.nodes.len()];
        for (from, to, prefix) in self.edges {
            if parents[to].insert(from) {
                self.nodes[from].children.push(to);
                if prefix.is_some() {
                    self.nodes[to].prefix = prefix;
                }
            } else if self.nodes[to].prefix.is_none() {
                self.nodes[to].prefix = prefix;
            }
        }
        let multi_parent = parents
            .iter()
            .enumerate()
            .filter(|(_, p)| p.len() > 1)
            .map(|(i, _)| self.nodes[i].id.clone())
            .collect::<Vec<_>>();
        let roots = parents
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_empty())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if roots.len() != 1 || !multi_parent.is_empty() {
            return Err(DiagramError::NotATree { multi_parent });
        }
        let root = roots[0];
        let mut seen = HashSet::new();
        let mut todo = vec![root];
        while let Some(i) = todo.pop() {
            if !seen.insert(i) {
                break;
            }
            todo.extend(&self.nodes[i].children);
        }
        if seen.len() != self.nodes.len() {
            return Err(DiagramError::NotATree { multi_parent });
        }
        Ok(Tree {
            nodes: self.nodes,
            root,
            class_defs: self.classes,
            inline_styles: self.inline_styles,
        })
    }
}

fn parse_flow_statement(text: &str, at: usize, builder: &mut Builder) -> Result<(), DiagramError> {
    if text.starts_with("subgraph")
        && text["subgraph".len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        return Err(DiagramError::Unsupported {
            diagram_type: "subgraph".into(),
        });
    }
    if ["linkStyle", "click", "accTitle", "accDescr"]
        .iter()
        .any(|directive| is_directive(text, directive))
    {
        return Ok(());
    }
    if let Some(rest) = text
        .strip_prefix("classDef")
        .filter(|s| s.starts_with(char::is_whitespace))
        .map(str::trim_start)
    {
        let Some((name, properties)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "classdef_missing_properties"));
        };
        let style = parse_style_properties(properties);
        for name in name.split(',') {
            builder.classes.insert(name.into(), style.clone());
        }
        return Ok(());
    }
    if let Some(rest) = text
        .strip_prefix("style")
        .filter(|s| s.starts_with(char::is_whitespace))
        .map(str::trim_start)
    {
        let Some((id, properties)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "style_missing_properties"));
        };
        if id.is_empty() || properties.trim().is_empty() {
            return Err(syntax(at..at + text.len(), "style_missing_target"));
        }
        builder
            .inline_styles
            .insert(id.into(), parse_style_properties(properties));
        return Ok(());
    }
    if let Some(rest) = text
        .strip_prefix("class")
        .filter(|s| s.starts_with(char::is_whitespace))
        .map(str::trim_start)
    {
        let Some((ids, class)) = rest.split_once(char::is_whitespace) else {
            return Err(syntax(at..at + text.len(), "class_missing_name"));
        };
        for id in ids.split(',') {
            builder.node(id, None, Some(class.trim().into()), at..at + text.len());
        }
        return Ok(());
    }
    let mut cursor = 0;
    let mut previous = vec![parse_ref(text, &mut cursor, at, builder)?];
    loop {
        skip_space(text, &mut cursor);
        if !text[cursor..].starts_with('&') {
            break;
        }
        cursor += 1;
        previous.push(parse_ref(text, &mut cursor, at, builder)?);
    }
    loop {
        skip_space(text, &mut cursor);
        if cursor == text.len() {
            return Ok(());
        }
        let prefix = parse_connector(text, &mut cursor, at)?;
        let mut targets = Vec::new();
        loop {
            let next = parse_ref(text, &mut cursor, at, builder)?;
            for &from in &previous {
                builder.edges.push((from, next, prefix.clone()));
            }
            targets.push(next);
            skip_space(text, &mut cursor);
            if text[cursor..].starts_with('&') {
                cursor += 1;
                continue;
            }
            break;
        }
        previous = targets;
    }
}

fn is_directive(text: &str, directive: &str) -> bool {
    text == directive
        || text
            .strip_prefix(directive)
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with(char::is_whitespace))
}

fn parse_style_properties(properties: &str) -> ClassStyle {
    let mut style = ClassStyle::default();
    for property in split_style_properties(properties) {
        if let Some((key, value)) = property.split_once(':') {
            match key.trim() {
                "stroke" => style.stroke = Some(value.trim().into()),
                "fill" => style.fill = Some(value.trim().into()),
                _ => {}
            }
        }
    }
    style
}

fn split_style_properties(properties: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth: usize = 0;
    for (i, ch) in properties.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(properties[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(properties[start..].trim());
    result
}

fn parse_connector(
    text: &str,
    cursor: &mut usize,
    at: usize,
) -> Result<Option<String>, DiagramError> {
    let start = *cursor;
    if text[start..].starts_with('<') {
        return Err(syntax(
            at + start..at + text.len(),
            "unsupported_reverse_edge",
        ));
    }

    if text[start..].starts_with("--")
        && text[start + 2..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        let label_start = start + 2;
        let Some(relative_arrow) = text[label_start..].find("-->") else {
            return Err(syntax(
                at + start..at + text.len(),
                "edge_label_missing_arrow",
            ));
        };
        let label_end = label_start + relative_arrow;
        let label = text[label_start..label_end].trim();
        if label.is_empty() {
            return Err(syntax(at + start..at + label_end + 3, "edge_label_empty"));
        }
        *cursor = label_end + 3;
        return Ok(Some(label.into()));
    }

    if text[start..].starts_with("-.") {
        let dots = text[start + 1..].bytes().take_while(|b| *b == b'.').count();
        let end = start + 1 + dots;
        if !text[end..].starts_with('-') {
            return Err(syntax(
                at + start..at + text.len(),
                "dotted_edge_missing_end",
            ));
        }
        *cursor = end + 1;
        if text[*cursor..].starts_with('>') {
            *cursor += 1;
        }
    } else if text[start..].starts_with('-') {
        let count = text[start..].bytes().take_while(|b| *b == b'-').count();
        let after = start + count;
        if count >= 2 && text[after..].starts_with('>') {
            *cursor = after + 1;
        } else if count >= 3 {
            *cursor = after;
        } else {
            return Err(syntax(at + start..at + text.len(), "unrecognized_edge"));
        }
    } else if text[start..].starts_with('=') {
        let count = text[start..].bytes().take_while(|b| *b == b'=').count();
        let after = start + count;
        if text[after..].starts_with('>') {
            *cursor = after + 1;
        } else if count >= 3 {
            *cursor = after;
        } else {
            return Err(syntax(at + start..at + text.len(), "unrecognized_edge"));
        }
    } else {
        return Err(syntax(at + start..at + text.len(), "tree_edges_only"));
    }

    skip_space(text, cursor);
    if text[*cursor..].starts_with('|') {
        let label_start = *cursor + 1;
        let Some(end) = text[label_start..].find('|') else {
            return Err(syntax(at + *cursor..at + text.len(), "edge_label_unclosed"));
        };
        let label_end = label_start + end;
        let label = text[label_start..label_end].trim().to_string();
        *cursor = label_end + 1;
        return Ok((!label.is_empty()).then_some(label));
    }
    Ok(None)
}

fn skip_space(text: &str, cursor: &mut usize) {
    *cursor += text[*cursor..].len() - text[*cursor..].trim_start().len();
}

fn parse_ref(
    text: &str,
    cursor: &mut usize,
    at: usize,
    builder: &mut Builder,
) -> Result<usize, DiagramError> {
    skip_space(text, cursor);
    let start = *cursor;
    while let Some(ch) = text[*cursor..].chars().next() {
        if connector_start(&text[*cursor..]) || !(ch.is_alphanumeric() || ch == '_' || ch == '-') {
            break;
        }
        *cursor += ch.len_utf8();
    }
    if start == *cursor {
        return Err(syntax(at + start..at + text.len(), "missing_node_id"));
    }
    let id = &text[start..*cursor];
    skip_space(text, cursor);
    let label = parse_shape(text, cursor, at)?;
    let class = parse_class(text, cursor);
    Ok(builder.node(id, label, class, at + start..at + *cursor))
}

fn connector_start(text: &str) -> bool {
    text.starts_with("<--")
        || text.starts_with("--")
        || text.starts_with("-.")
        || text.starts_with("==")
}

const SHAPES: &[(&str, &str)] = &[
    ("))", "(("),
    (")", "("),
    ("((", "))"),
    ("([", "])"),
    ("[[", "]]"),
    ("[(", ")]"),
    ("{{", "}}"),
    ("[", "]"),
    ("(", ")"),
    ("{", "}"),
    (">", "]"),
];

fn parse_shape(text: &str, cursor: &mut usize, at: usize) -> Result<Option<String>, DiagramError> {
    let Some(&(open, close)) = SHAPES
        .iter()
        .find(|(open, _)| text[*cursor..].starts_with(open))
    else {
        return Ok(None);
    };
    let start = *cursor;
    let body = start + open.len();
    let mut quoted = false;
    let mut escaped = false;
    let mut nested = Vec::new();
    for (relative, ch) in text[body..].char_indices() {
        let i = body + relative;
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
        if nested.is_empty() && text[i..].starts_with(close) {
            *cursor = i + close.len();
            return Ok(Some(unquote(&text[body..i])));
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
        at + start..at + text.len(),
        "node_shape_or_quote_unclosed",
    ))
}

fn unquote(text: &str) -> String {
    let text = text.trim();
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

fn parse_class(text: &str, cursor: &mut usize) -> Option<String> {
    skip_space(text, cursor);
    if !text[*cursor..].starts_with(":::") {
        return None;
    }
    *cursor += 3;
    let start = *cursor;
    while let Some(ch) = text[*cursor..].chars().next() {
        if !(ch.is_alphanumeric() || ch == '_' || ch == '-') {
            break;
        }
        *cursor += ch.len_utf8();
    }
    Some(text[start..*cursor].into())
}

fn mindmap_explicit_ids(source: &str) -> HashSet<String> {
    let mut ids = HashSet::new();
    for line in source.split_inclusive('\n') {
        let raw = line.trim_end_matches(['\r', '\n']);
        let text = without_comment(raw.trim());
        if text.is_empty() || text.starts_with(":::") || text.starts_with("::icon(") {
            continue;
        }
        let mut cursor = 0;
        while let Some(ch) = text[cursor..].chars().next() {
            if !(ch.is_alphanumeric() || ch == '_' || ch == '-') {
                break;
            }
            cursor += ch.len_utf8();
        }
        if cursor > 0
            && SHAPES
                .iter()
                .any(|(open, _)| text[cursor..].starts_with(open))
        {
            ids.insert(text[..cursor].into());
        }
    }
    ids
}

fn parse_mindmap(source: &str, at: usize, builder: &mut Builder) -> Result<(), DiagramError> {
    let explicit = mindmap_explicit_ids(source);
    let mut offset = 0;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for line in source.split_inclusive('\n') {
        let raw = line.trim_end_matches(['\r', '\n']);
        let text = without_comment(raw.trim());
        let start = at + offset + raw.len() - raw.trim_start().len();
        offset += line.len();
        if text.is_empty() {
            continue;
        }
        if text.starts_with(":::") || text.starts_with("::icon(") {
            if let Some(class) = mindmap_annotations(text, &mut 0, start)? {
                let Some((_, node)) = stack.last() else {
                    return Err(syntax(start..start + text.len(), "class_without_node"));
                };
                builder.nodes[*node].class = Some(class);
            }
            continue;
        }
        let indent: usize = raw[..raw.len() - raw.trim_start().len()]
            .chars()
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();
        let mut cursor = 0;
        while let Some(ch) = text[cursor..].chars().next() {
            if !(ch.is_alphanumeric() || ch == '_' || ch == '-') {
                break;
            }
            cursor += ch.len_utf8();
        }
        let prefix = &text[..cursor];
        let shape = parse_shape(text, &mut cursor, start)?;
        let explicit_id = shape.is_some() && !prefix.is_empty();
        let (id, label) = if let Some(label) = shape {
            let id = if explicit_id {
                prefix.into()
            } else {
                auto_mindmap_id(&explicit, builder)
            };
            (id, label)
        } else {
            cursor = annotation_start(text);
            (
                auto_mindmap_id(&explicit, builder),
                unquote(&text[..cursor]),
            )
        };
        let class = mindmap_annotations(text, &mut cursor, start)?;
        if explicit_id && builder.ids.contains_key(&id) {
            return Err(syntax(start..start + text.len(), "duplicate_mindmap_id"));
        }
        let node = builder.node(&id, Some(label), class, start..start + text.len());
        while stack.last().is_some_and(|(level, _)| *level >= indent) {
            stack.pop();
        }
        if let Some((_, parent)) = stack.last() {
            builder.edges.push((*parent, node, None));
        }
        stack.push((indent, node));
    }
    Ok(())
}

fn auto_mindmap_id(explicit: &HashSet<String>, builder: &Builder) -> String {
    // 先沿用按当前节点数生成的历史 ID，只有与显式 ID 冲突时才递增，
    // 这样普通笔记的折叠状态键保持不变。
    let mut candidate = builder.nodes.len();
    loop {
        let id = format!("mindmap-{candidate}");
        candidate += 1;
        if !explicit.contains(&id) && !builder.ids.contains_key(&id) {
            return id;
        }
    }
}

// 注释和附加标记只在引号外识别，不能吞掉标签里的字面文字。
fn without_comment(text: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for (i, ch) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
        } else if depth == 0 && text[i..].starts_with("%%") {
            return text[..i].trim_end();
        } else if matches!(ch, '[' | '(' | '{') {
            depth += 1;
        } else if matches!(ch, ']' | ')' | '}') {
            depth = depth.saturating_sub(1);
        }
    }
    text
}

fn annotation_start(text: &str) -> usize {
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
        } else if text[i..].starts_with(":::") || text[i..].starts_with("::icon(") {
            return i;
        }
    }
    text.len()
}

fn mindmap_annotations(
    text: &str,
    cursor: &mut usize,
    at: usize,
) -> Result<Option<String>, DiagramError> {
    let mut class = None;
    loop {
        skip_space(text, cursor);
        if *cursor == text.len() {
            return Ok(class);
        }
        if text[*cursor..].starts_with(":::") {
            class = parse_class(text, cursor);
            if class.as_ref().is_some_and(String::is_empty) {
                return Err(syntax(at + *cursor..at + text.len(), "missing_class_name"));
            }
        } else if text[*cursor..].starts_with("::icon(") {
            let Some(end) = text[*cursor..].find(')') else {
                return Err(syntax(at + *cursor..at + text.len(), "icon_unclosed"));
            };
            *cursor += end + 1;
        } else {
            return Err(syntax(
                at + *cursor..at + text.len(),
                "unrecognized_mindmap_suffix",
            ));
        }
    }
}
