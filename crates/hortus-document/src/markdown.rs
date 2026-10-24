//! 与平台无关的源码语义；范围同时保留 UTF-8 字节和原生 UTF-16 单位。
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use std::ops::Range;

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
    fn clear_range(&mut self, range: Range<usize>);
}
impl SemanticSink for Vec<(Kind, Range<usize>)> {
    fn insert(&mut self, kind: Kind, range: Range<usize>) {
        self.push((kind, range));
    }
    fn clear_range(&mut self, removed: Range<usize>) {
        let mut kept = Vec::with_capacity(self.len());
        for (kind, range) in self.drain(..) {
            if range.end <= removed.start || range.start >= removed.end {
                kept.push((kind, range));
            } else {
                if range.start < removed.start {
                    kept.push((kind, range.start..removed.start));
                }
                if range.end > removed.end {
                    kept.push((kind, removed.end..range.end));
                }
            }
        }
        *self = kept;
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
    fn clear_range(&mut self, range: Range<usize>) {
        if !self.exceeded {
            self.masks[range].fill(0);
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
        for line in text[first..].split_inclusive('\n') {
            offset += line.len();
            if matches!(line.trim_end(), "---" | "...") {
                return offset;
            }
        }
    }
    0
}

fn analyze_into(text: &str, spans: &mut impl SemanticSink, protected: &mut Vec<bool>) {
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
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    for (event, local) in Parser::new_ext(&text[body_start..], options).into_offset_iter() {
        let r = local.start + body_start..local.end + body_start;
        let source = &text[r.clone()];
        match event {
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
                for line in source.split_inclusive('\n') {
                    let indent = line.len() - line.trim_start_matches(' ').len();
                    if line.as_bytes().get(indent) == Some(&b'>') {
                        let n = indent
                            + 1
                            + usize::from(line.as_bytes().get(indent + 1) == Some(&b' '));
                        push(spans, Kind::Marker, offset..offset + n);
                        push(
                            spans,
                            Kind::Quote,
                            offset + n..offset + line.trim_end_matches(['\n', '\r']).len(),
                        );
                        let content = &line[n..];
                        if let Some(end) = content
                            .strip_prefix("[!")
                            .and_then(|s| s.find(']').map(|i| i + 3))
                        {
                            push(spans, Kind::Callout, offset + n..offset + line.len());
                            push(spans, Kind::Marker, offset + n..offset + n + end);
                        }
                    }
                    offset += line.len();
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
            Event::Rule => {
                push(spans, Kind::Rule, r.clone());
                push(spans, Kind::Marker, r);
            }
            _ => {}
        }
    }
    // 闭合注释允许跨行；代码中的起始符不参与配对，未闭合时保留正常正文。
    let mut position = body_start;
    while let Some(offset) = text[position..].find("%%") {
        position += offset;
        if !protected[position] && !escaped(text.as_bytes(), position) {
            let close = text[position + 2..]
                .match_indices("%%")
                .map(|(i, _)| i + position + 2)
                .find(|&i| !protected[i] && !escaped(text.as_bytes(), i));
            if let Some(close) = close {
                let end = close + 2;
                spans.clear_range(position..end);
                wrapped(spans, Kind::Comment, position..end, 2, 2);
                protected[position..end].fill(true);
                position = end;
                continue;
            }
        }
        position += 2;
    }
    // 扩展在代码/目标地址之外逐行匹配；未闭合的扩展不会跨行吞正文。
    let mut base = body_start;
    for line in text[body_start..].split_inclusive('\n') {
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
    analyze_into(text, &mut spans, &mut Vec::new());
    sort_spans(&mut spans);
    spans
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
        analyze_into(text, &mut sink, &mut self.protected);
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
        analyze_into(text, &mut self.spans, &mut self.protected);
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
