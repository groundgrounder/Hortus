//! 与平台无关的源码语义；范围同时保留 UTF-8 字节和原生 UTF-16 单位。
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use std::ops::Range;

pub(crate) struct LinesInclusive<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> Iterator for LinesInclusive<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.text.len() {
            return None;
        }
        let bytes = self.text.as_bytes();
        let start = self.offset;
        let mut end = start;
        while end < bytes.len() && !matches!(bytes[end], b'\r' | b'\n') {
            end += 1;
        }
        if end < bytes.len() {
            end += 1;
            if bytes[end - 1] == b'\r' && bytes.get(end) == Some(&b'\n') {
                end += 1;
            }
        }
        self.offset = end;
        Some(&self.text[start..end])
    }
}

pub(crate) fn split_lines_inclusive(text: &str) -> LinesInclusive<'_> {
    LinesInclusive { text, offset: 0 }
}

fn line_start(text: &str, position: usize) -> usize {
    text[..position]
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, '\r' | '\n'))
        .map_or(0, |(index, character)| index + character.len_utf8())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Marker,
    Heading(u8),
    Strong,
    Emphasis,
    Strike,
    InlineCode,
    CodeBlock,
    Quote,
    OrderedList,
    UnorderedList,
    Task,
    Link,
    Image,
    WikiLink,
    Embed,
    Tag,
    Highlight,
    Comment,
    Callout,
    Footnote,
    Frontmatter,
    Rule,
    FencedCode,
}
impl Kind {
    pub fn mask(self) -> u32 {
        match self {
            Self::Marker => 1,
            Self::Heading(n) => u32::from(n) << 24,
            Self::Strong => 2,
            Self::Emphasis => 4,
            Self::Strike => 8,
            Self::InlineCode => 16,
            Self::CodeBlock => 32,
            Self::Quote => 64,
            Self::Link | Self::WikiLink => 128,
            Self::Image | Self::Embed => 256,
            Self::Tag => 512,
            Self::Highlight => 1024,
            Self::Comment => 2048,
            Self::Callout => 4096,
            Self::Footnote => 8192,
            Self::Frontmatter => 16384,
            Self::Rule => 32768,
            Self::OrderedList | Self::UnorderedList | Self::Task => 0,
            Self::FencedCode => 65536,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub kind: Kind,
    pub bytes: Range<usize>,
    pub utf16: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub range: Range<usize>,
    pub mask: u32,
}

trait SemanticSink {
    fn insert(&mut self, kind: Kind, range: Range<usize>);
    fn clear_ranges(&mut self, ranges: &[Range<usize>]);
}

// 注释按源码顺序互不重叠。字节索引直接定位第一个相交注释，
// 每个语义范围只访问会切出新片段的注释，不为每条注释重建整个集合。
fn clear_ranges(spans: &mut Vec<(Kind, Range<usize>)>, removed: &[Range<usize>], reading: bool) {
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in removed {
        if let Some(last) = merged.last_mut().filter(|last| last.end == range.start) {
            last.end = range.end;
        } else {
            merged.push(range.clone());
        }
    }
    let Some(last) = merged.last() else {
        return;
    };
    let mut next = vec![merged.len(); last.end + 1];
    let mut end = 0;
    for (index, range) in merged.iter().enumerate() {
        next[end..range.end].fill(index);
        end = range.end;
    }
    let mut kept = Vec::with_capacity(spans.len());
    for (kind, range) in spans.drain(..) {
        // 这些范围指向完整结构的起点；阅读时不能把注释后的片段当成新结构。
        if reading && kind == Kind::Callout {
            kept.push((kind, range));
            continue;
        }
        let mut start = range.start;
        for comment in &merged[next[start.min(next.len() - 1)]..] {
            if comment.start >= range.end {
                break;
            }
            if start < comment.start {
                kept.push((kind, start..comment.start));
            }
            start = comment.end;
            if start >= range.end {
                break;
            }
        }
        if start < range.end {
            kept.push((kind, start..range.end));
        }
    }
    *spans = kept;
}

impl SemanticSink for Vec<(Kind, Range<usize>)> {
    fn insert(&mut self, kind: Kind, range: Range<usize>) {
        self.push((kind, range));
    }
    fn clear_ranges(&mut self, removed: &[Range<usize>]) {
        clear_ranges(self, removed, false);
    }
}
// 阅读中的结构范围不能被注释切开；显示样式仍由普通 sink 清除注释覆盖。
#[derive(Default)]
struct ReadingSink(Vec<(Kind, Range<usize>)>);
impl SemanticSink for ReadingSink {
    fn insert(&mut self, kind: Kind, range: Range<usize>) {
        self.0.push((kind, range));
    }
    fn clear_ranges(&mut self, ranges: &[Range<usize>]) {
        clear_ranges(&mut self.0, ranges, true);
    }
}
/// 显示路径直接叠加掩码，不先分配数十万个带范围的语义对象。
struct MaskSink<'a> {
    masks: &'a mut [u32],
    coverage: usize,
    exceeded: bool,
}
impl SemanticSink for MaskSink<'_> {
    fn insert(&mut self, kind: Kind, range: Range<usize>) {
        let mask = kind.mask();
        if mask == 0 || self.exceeded {
            return;
        }
        self.coverage = self.coverage.saturating_add(range.len());
        if self.coverage > self.masks.len().saturating_mul(8) {
            self.exceeded = true;
            return;
        }
        for value in &mut self.masks[range] {
            *value |= mask;
        }
    }
    fn clear_ranges(&mut self, ranges: &[Range<usize>]) {
        if !self.exceeded {
            for range in ranges {
                self.masks[range.clone()].fill(0);
            }
        }
    }
}

fn push(spans: &mut impl SemanticSink, kind: Kind, range: Range<usize>) {
    if !range.is_empty() {
        spans.insert(kind, range);
    }
}
fn wrapped(spans: &mut impl SemanticSink, kind: Kind, r: Range<usize>, left: usize, right: usize) {
    if r.len() >= left + right {
        push(spans, Kind::Marker, r.start..r.start + left);
        push(spans, kind, r.start + left..r.end - right);
        push(spans, Kind::Marker, r.end - right..r.end);
    }
}
fn escaped(bytes: &[u8], pos: usize) -> bool {
    bytes[..pos]
        .iter()
        .rev()
        .take_while(|&&b| b == b'\\')
        .count()
        % 2
        == 1
}

pub(crate) fn body_start(text: &str) -> usize {
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let first = text.find('\n').unwrap() + 1;
        let mut offset = first;
        for line in split_lines_inclusive(&text[first..]) {
            offset += line.len();
            if matches!(line.trim_end(), "---" | "...") {
                return offset;
            }
        }
    }
    0
}

fn analyze_into(
    text: &str,
    spans: &mut impl SemanticSink,
    protected: &mut Vec<bool>,
    math: bool,
    html_comments: bool,
) {
    if protected.capacity() < text.len() {
        protected.reserve(text.len() - protected.len() + (text.len() / 8).max(4096));
    }
    protected.resize(text.len(), false);
    protected.fill(false);
    // frontmatter 必须从文档首行开始且有闭合线；不完整的输入不遮蔽后续正文。
    let body_start = body_start(text);
    if body_start > 0 {
        let first = text.find('\n').unwrap() + 1;
        let last = text[..body_start]
            .trim_end_matches('\n')
            .rfind('\n')
            .map_or(0, |i| i + 1);
        wrapped(
            spans,
            Kind::Frontmatter,
            0..body_start,
            first,
            body_start - last,
        );
        protected[..body_start].fill(true);
    }
    let mut options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    if math {
        options |= Options::ENABLE_MATH;
    }
    let mut quote_prefix_ends = std::collections::HashMap::new();
    let mut list_prefixes: Vec<Range<usize>> = Vec::new();
    let mut math_ranges = Vec::new();
    let mut html_ranges = Vec::new();
    let mut callout_lines = Vec::new();
    for (event, local) in Parser::new_ext(&text[body_start..], options).into_offset_iter() {
        let r = local.start + body_start..local.end + body_start;
        let source = &text[r.clone()];
        match event {
            event @ (Event::Start(Tag::HtmlBlock) | Event::InlineHtml(_))
                if html_comments && source.starts_with("<!--") =>
            {
                let short = if source.starts_with("<!-->") {
                    Some(5)
                } else if source.starts_with("<!--->") {
                    Some(6)
                } else {
                    None
                };
                if let Some(length) = short {
                    // Obsidian 的短注释只在 HTML 块里隐藏，行内保留原文。
                    if matches!(event, Event::Start(Tag::HtmlBlock)) {
                        html_ranges.push((r.start..r.start + length, 0, 0));
                    }
                } else if let Some(close) = source.find("-->") {
                    html_ranges.push((r.start..r.start + close + 3, 4, 3));
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let trimmed = source.trim_end_matches(['\n', '\r']);
                let leading = trimmed.len() - trimmed.trim_start().len();
                let hashes = trimmed[leading..]
                    .bytes()
                    .take_while(|&b| b == b'#')
                    .count();
                if hashes > 0 {
                    let content_start = leading
                        + hashes
                        + trimmed[leading + hashes..]
                            .bytes()
                            .take_while(u8::is_ascii_whitespace)
                            .count();
                    let tail = trimmed.trim_end_matches([' ', '\t']);
                    let hashes_start = tail.trim_end_matches('#').len();
                    let end = if hashes_start < tail.len()
                        && hashes_start >= content_start
                        && hashes_start > 0
                        && tail.as_bytes()[hashes_start - 1].is_ascii_whitespace()
                        && !escaped(tail.as_bytes(), hashes_start)
                    {
                        hashes_start - 1
                    } else {
                        trimmed.len()
                    };
                    push(
                        spans,
                        Kind::Heading(level as u8),
                        r.start..r.start + trimmed.len(),
                    );
                    push(spans, Kind::Marker, r.start..r.start + content_start);
                    if end < trimmed.len() {
                        push(spans, Kind::Marker, r.start + end..r.start + trimmed.len());
                    }
                } else if let Some(i) = trimmed.rfind('\n') {
                    push(spans, Kind::Heading(level as u8), r.start..r.start + i);
                    push(
                        spans,
                        Kind::Marker,
                        r.start + i + 1..r.start + trimmed.len(),
                    );
                }
            }
            Event::Start(Tag::Strong) => wrapped(spans, Kind::Strong, r, 2, 2),
            Event::Start(Tag::Emphasis) => wrapped(spans, Kind::Emphasis, r, 1, 1),
            Event::Start(Tag::Strikethrough) => wrapped(spans, Kind::Strike, r, 2, 2),
            Event::Code(_) => {
                let ticks = source.bytes().take_while(|&b| b == b'`').count();
                protected[r.clone()].fill(true);
                wrapped(spans, Kind::InlineCode, r, ticks, ticks);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let r = r.start..r.end + usize::from(text.as_bytes().get(r.end) == Some(&b'\n'));
                let source = &text[r.clone()];
                protected[r.clone()].fill(true);
                match kind {
                    CodeBlockKind::Fenced(_) => {
                        push(spans, Kind::FencedCode, r.clone());
                        let first = source.find('\n').map_or(source.len(), |i| i + 1);
                        let tail = source.trim_end_matches(['\n', '\r']);
                        let last = tail.rfind('\n').map_or(0, |i| i + 1);
                        let opening = source.trim_start_matches(' ');
                        let fence = opening.as_bytes()[0];
                        let minimum = opening.bytes().take_while(|&b| b == fence).count();
                        let closing = tail[last..].trim_start_matches(' ');
                        let count = closing.bytes().take_while(|&b| b == fence).count();
                        let closed =
                            last >= first && count >= minimum && closing[count..].trim().is_empty();
                        push(spans, Kind::CodeBlock, r.clone());
                        push(spans, Kind::Marker, r.start..r.start + first);
                        if closed {
                            push(spans, Kind::Marker, r.start + last..r.end);
                        }
                    }
                    CodeBlockKind::Indented => push(spans, Kind::CodeBlock, r),
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let mut offset = r.start;
                let mut line_start = line_start(text, r.start);
                for (index, line) in split_lines_inclusive(source).enumerate() {
                    // 内层引用从外层已消费的前缀之后继续，不能再次隐藏同一个 >。
                    let start = offset.max(*quote_prefix_ends.get(&line_start).unwrap_or(&offset));
                    let rest = &text[start..offset + line.len()];
                    let indent = rest.len() - rest.trim_start_matches(' ').len();
                    if rest.as_bytes().get(indent) == Some(&b'>') {
                        let n = indent
                            + 1
                            + usize::from(rest.as_bytes().get(indent + 1) == Some(&b' '));
                        let end = start + n;
                        quote_prefix_ends.insert(line_start, end);
                        push(spans, Kind::Marker, start..end);
                        push(
                            spans,
                            Kind::Quote,
                            end..offset + line.trim_end_matches(['\n', '\r']).len(),
                        );
                        if index == 0 {
                            callout_lines.push(start + n..offset + line.len());
                        }
                    }
                    offset += line.len();
                    line_start = offset;
                }
            }
            Event::Start(Tag::Item) => {
                let n = source.bytes().take_while(|b| b.is_ascii_digit()).count();
                let marker = if n > 0 && matches!(source.as_bytes().get(n), Some(b'.' | b')')) {
                    n + 1
                } else {
                    1
                };
                let prefix = marker
                    + source[marker..]
                        .bytes()
                        .take_while(|b| matches!(b, b' ' | b'\t'))
                        .count();
                push(
                    spans,
                    if n > 0 {
                        Kind::OrderedList
                    } else {
                        Kind::UnorderedList
                    },
                    r.start + prefix..r.end,
                );
                push(spans, Kind::Marker, r.start..r.start + prefix);
                if math {
                    let line_start = line_start(text, r.start);
                    let columns = |value: &str| {
                        value.chars().fold(0, |column, c| {
                            if c == '\t' {
                                column + 4 - column % 4
                            } else {
                                column + 1
                            }
                        })
                    };
                    let empty_marker = source[prefix..].starts_with(['\n', '\r']);
                    let target = columns(&text[line_start..r.start + prefix])
                        + usize::from(empty_marker && prefix == marker);
                    let mut offset = r.start;
                    for (index, line) in split_lines_inclusive(source).enumerate() {
                        if index > 0 {
                            let mut column = 0;
                            let count = line
                                .bytes()
                                .take_while(|byte| {
                                    if column >= target || !matches!(byte, b' ' | b'\t' | b'>') {
                                        return false;
                                    }
                                    column += if *byte == b'\t' { 4 - column % 4 } else { 1 };
                                    true
                                })
                                .count();
                            if count > 0 {
                                list_prefixes.push(offset..offset + count);
                            }
                        }
                        offset += line.len();
                    }
                }
            }
            Event::TaskListMarker(_) => {
                push(spans, Kind::Task, r.clone());
                push(spans, Kind::Marker, r);
            }
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => {
                let image = source.starts_with('!');
                let left = usize::from(image) + 1;
                if let Some(end) = source[left..].find(']').map(|i| i + left) {
                    push(spans, Kind::Marker, r.start..r.start + left);
                    push(
                        spans,
                        if image { Kind::Image } else { Kind::Link },
                        r.start + left..r.start + end,
                    );
                    push(spans, Kind::Marker, r.start + end..r.end);
                    protected[r.start + end..r.end].fill(true);
                } else {
                    push(spans, Kind::Link, r);
                }
            }
            Event::FootnoteReference(_) => wrapped(spans, Kind::Footnote, r, 2, 1),
            Event::Start(Tag::FootnoteDefinition(_)) => {
                if let Some(end) = source.find("]: ").or_else(|| source.find("]:")) {
                    push(spans, Kind::Marker, r.start..r.start + 2);
                    push(spans, Kind::Footnote, r.start + 2..r.start + end);
                    push(spans, Kind::Marker, r.start + end..r.start + end + 2);
                }
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                math_ranges.push(r.clone());
                protected[r].fill(true);
            }
            Event::Rule => {
                push(spans, Kind::Rule, r.clone());
                push(spans, Kind::Marker, r);
            }
            _ => {}
        }
    }
    // 合并嵌套列表的重复前缀，再线性扫描公式范围；只隐藏源码范围以内的缩进。
    // 公式前紧邻的缩进仍参与普通正文和块级附件的布局。
    list_prefixes.sort_unstable_by_key(|range| range.start);
    let mut prefixes: Vec<Range<usize>> = Vec::new();
    for prefix in list_prefixes {
        if let Some(last) = prefixes.last_mut().filter(|last| prefix.start <= last.end) {
            last.end = last.end.max(prefix.end);
        } else {
            prefixes.push(prefix);
        }
    }
    let (mut prefix, mut formula) = (0, 0);
    while prefix < prefixes.len() && formula < math_ranges.len() {
        let left = &prefixes[prefix];
        let right = &math_ranges[formula];
        let start = left.start.max(right.start);
        let end = left.end.min(right.end);
        if start < end {
            push(spans, Kind::Marker, start..end);
        }
        if left.end <= right.end {
            prefix += 1;
        } else {
            formula += 1;
        }
    }
    // 闭合注释允许跨行；代码中的起始符不参与配对，未闭合时保留正常正文。
    let mut comments = Vec::new();
    let mut position = body_start;
    let mut html = html_ranges.into_iter().peekable();
    let mut percent = text[body_start..]
        .match_indices("%%")
        .map(|(offset, _)| offset + body_start)
        .filter(|&offset| !protected[offset] && !escaped(text.as_bytes(), offset))
        .peekable();
    while position < text.len() {
        while html
            .peek()
            .is_some_and(|(range, _, _)| range.start < position)
        {
            html.next();
        }
        while percent.peek().is_some_and(|&offset| offset < position) {
            percent.next();
        }
        if html
            .peek()
            .is_some_and(|(range, _, _)| percent.peek().is_none_or(|&p| range.start <= p))
        {
            let comment = html.next().unwrap();
            position = comment.0.end;
            comments.push(comment);
            continue;
        }
        let Some(start) = percent.next() else {
            break;
        };
        if let Some(close) = percent.next() {
            position = close + 2;
            comments.push((start..position, 2, 2));
        } else {
            position = start + 2;
        }
    }
    let mut hidden = vec![false; text.len()];
    for (range, _, _) in &comments {
        hidden[range.clone()].fill(true);
    }
    for line in callout_lines {
        if let Some((end, _)) = callout_header(&text[line.clone()], &hidden[line.clone()]) {
            push(spans, Kind::Callout, line.clone());
            push(spans, Kind::Marker, line.start..line.start + end);
        }
    }
    spans.clear_ranges(
        &comments
            .iter()
            .map(|(range, _, _)| range.clone())
            .collect::<Vec<_>>(),
    );
    for (range, left, right) in comments {
        if range.len() == left + right {
            push(spans, Kind::Comment, range.clone());
        }
        wrapped(spans, Kind::Comment, range.clone(), left, right);
        protected[range].fill(true);
    }
    // 扩展在代码/目标地址之外逐行匹配；未闭合的扩展不会跨行吞正文。
    let mut base = body_start;
    for line in split_lines_inclusive(&text[body_start..]) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let Some(offset) = bytes[i..]
                .iter()
                .position(|b| matches!(b, b'!' | b'[' | b'=' | b'%' | b'#'))
            else {
                break;
            };
            i += offset;
            if protected[base + i] {
                i += 1;
                continue;
            }
            if escaped(bytes, i) {
                i += line[i..].chars().next().unwrap().len_utf8();
                continue;
            }
            let rest = &line[i..];
            let pair = if rest.starts_with("![[") {
                Some(("![[", "]]", Kind::Embed))
            } else if rest.starts_with("[[") {
                Some(("[[", "]]", Kind::WikiLink))
            } else if rest.starts_with("==") {
                Some(("==", "==", Kind::Highlight))
            } else {
                None
            };
            if let Some((open, close, kind)) = pair
                && let Some(end) = rest[open.len()..]
                    .find(close)
                    .map(|n| i + open.len() + n + close.len())
                && !protected[base + i..base + end].iter().any(|&p| p)
            {
                wrapped(spans, kind, base + i..base + end, open.len(), close.len());
                i = end;
                continue;
            }
            if bytes[i] == b'#'
                && (i == 0
                    || line[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_whitespace() || matches!(c, '(' | '（')))
            {
                let len: usize = rest[1..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
                    .map(char::len_utf8)
                    .sum();
                if len > 0 {
                    push(spans, Kind::Marker, base + i..base + i + 1);
                    push(spans, Kind::Tag, base + i + 1..base + i + 1 + len);
                    i += len + 1;
                    continue;
                }
            }
            i += line[i..].chars().next().unwrap().len_utf8();
        }
        base += line.len();
    }
}

/// 注释内的右括号不结束标题；返回原始源码里的闭括号边界。
pub(crate) fn callout_header(text: &str, hidden: &[bool]) -> Option<(usize, String)> {
    let mut visible = text.char_indices().filter(|(index, _)| !hidden[*index]);
    if visible.next()?.1 != '[' || visible.next()?.1 != '!' {
        return None;
    }
    let mut label = String::new();
    for (index, character) in visible {
        if character == ']' {
            return Some((index + 1, label));
        }
        if matches!(character, '\r' | '\n') {
            return None;
        }
        label.push(character);
    }
    None
}

fn sort_spans(spans: &mut Vec<(Kind, Range<usize>)>) {
    // 原生语法与扩展各自大致有序，稳定排序可利用已有连续区间。
    spans.sort_by_key(|(_, r)| (r.start, r.end));
    spans.dedup();
}

/// 只记录非 ASCII 字符造成的偏移；大篇 ASCII 笔记不分配逐字节映射。
struct Utf16Offsets(Vec<(usize, usize)>);
impl Utf16Offsets {
    fn new(text: &str) -> Self {
        let mut offsets = Vec::new();
        let mut difference = 0;
        for (i, c) in text.char_indices().filter(|(_, c)| !c.is_ascii()) {
            difference += c.len_utf8() - c.len_utf16();
            offsets.push((i + c.len_utf8(), difference));
        }
        Self(offsets)
    }
    fn at(&self, byte: usize) -> usize {
        let index = self.0.partition_point(|&(position, _)| position <= byte);
        byte - index.checked_sub(1).map_or(0, |i| self.0[i].1)
    }
}

pub fn analyze(text: &str) -> Vec<Span> {
    let mapping = Utf16Offsets::new(text);
    analyze_bytes(text)
        .into_iter()
        .map(|(kind, bytes)| Span {
            kind,
            utf16: mapping.at(bytes.start)..mapping.at(bytes.end),
            bytes,
        })
        .collect()
}

pub(crate) fn analyze_bytes(text: &str) -> Vec<(Kind, Range<usize>)> {
    let mut spans = Vec::new();
    analyze_into(text, &mut spans, &mut Vec::new(), false, true);
    sort_spans(&mut spans);
    spans
}

pub(crate) fn analyze_bytes_with_math(text: &str, math: bool) -> Vec<(Kind, Range<usize>)> {
    let mut spans = Vec::new();
    analyze_into(text, &mut spans, &mut Vec::new(), math, false);
    sort_spans(&mut spans);
    spans
}

pub(crate) fn reading_spans(text: &str, math: bool) -> Vec<(Kind, Range<usize>)> {
    let mut sink = ReadingSink::default();
    analyze_into(text, &mut sink, &mut Vec::new(), math, true);
    sort_spans(&mut sink.0);
    sink.0
}

/// 文档持有分析缓冲，避免每次编辑重新分配大型语义片段数组。
#[derive(Default)]
pub(crate) struct Analyzer {
    spans: Vec<(Kind, Range<usize>)>,
    protected: Vec<bool>,
    masks: Vec<u32>,
    spare_runs: Vec<Run>,
}
impl Analyzer {
    pub(crate) fn recycle(&mut self, mut previous: Vec<Run>, current: &[Run]) {
        previous.clear();
        // 首次打开就建立另一份可写缓冲；之后新旧结果交替使用，避免每键分配大数组。
        if previous.capacity() < current.len() {
            previous.extend_from_slice(current);
            previous.clear();
        }
        self.spare_runs = previous;
    }
    pub(crate) fn runs(&mut self, text: &str) -> Vec<Run> {
        use std::{cmp::Reverse, collections::BinaryHeap};
        let mapping = Utf16Offsets::new(text);
        // 为连续编辑预留空间，避免首次插字时复制整篇文档的分析缓冲。
        if self.masks.capacity() < text.len() {
            self.masks
                .reserve(text.len() - self.masks.len() + (text.len() / 8).max(4096));
        }
        self.masks.resize(text.len(), 0);
        self.masks.fill(0);
        let mut sink = MaskSink {
            masks: &mut self.masks,
            coverage: 0,
            exceeded: false,
        };
        analyze_into(text, &mut sink, &mut self.protected, false, true);
        let exceeded = sink.exceeded;
        let mut result = std::mem::take(&mut self.spare_runs);
        // 常见笔记的叠加深度很小，按字节合并后线性压缩，省去排序和事件堆。
        // 覆盖总长超过正文 8 倍时退回事件扫描，避免深层嵌套造成平方级工作。
        if !exceeded {
            let mut start = 0;
            while start < text.len() {
                let mask = self.masks[start];
                let mut end = start + 1;
                while end < text.len() && self.masks[end] == mask {
                    end += 1;
                }
                result.push(Run {
                    range: mapping.at(start)..mapping.at(end),
                    mask,
                });
                start = end;
            }
            return result;
        }
        // 超过预算时再收集片段；首轮直接叠加的工作量始终不超过正文的 8 倍。
        self.spans.clear();
        analyze_into(text, &mut self.spans, &mut self.protected, false, true);
        sort_spans(&mut self.spans);
        let mut spans = self
            .spans
            .iter()
            .filter(|(kind, _)| kind.mask() != 0)
            .map(|(kind, range)| (kind.mask(), mapping.at(range.start)..mapping.at(range.end)))
            .peekable();
        let length = mapping.at(text.len());
        let mut active: BinaryHeap<Reverse<(usize, u32)>> = BinaryHeap::new();
        let mut counts = [0i32; 32];
        let mut mask = 0u32;
        let mut position = 0;
        while position < length {
            let mut update = |bits: u32, delta: i32| {
                let mut bits = bits;
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    counts[bit] += delta;
                    if counts[bit] > 0 {
                        mask |= 1 << bit;
                    } else {
                        mask &= !(1 << bit);
                    }
                }
            };
            while active
                .peek()
                .is_some_and(|Reverse((end, _))| *end <= position)
            {
                let Reverse((_, mask)) = active.pop().unwrap();
                update(mask, -1);
            }
            while spans.peek().is_some_and(|(_, r)| r.start <= position) {
                let (mask, range) = spans.next().unwrap();
                update(mask, 1);
                active.push(Reverse((range.end, mask)));
            }
            let end = spans
                .peek()
                .map_or(length, |(_, r)| r.start)
                .min(active.peek().map_or(length, |Reverse((end, _))| *end));
            if let Some(last) = result.last_mut().filter(|r| r.mask == mask) {
                last.range.end = end;
            } else {
                result.push(Run {
                    range: position..end,
                    mask,
                });
            }
            position = end;
        }
        result
    }
}

pub fn runs(text: &str) -> Vec<Run> {
    Analyzer::default().runs(text)
}
