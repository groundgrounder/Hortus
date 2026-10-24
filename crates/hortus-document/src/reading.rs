//! 阅读文本、语义类别和源码映射；缓存只保留当前文档的块，不持有平台视图。
use crate::markdown::{self, Kind};
use crate::media::{self, Image, Media, Table};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Run {
    pub display: Range<usize>,
    pub source: Range<usize>,
    pub mask: u32,
}
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub text: String,
    pub runs: Vec<Run>,
    pub diagram: bool,
    pub media: Vec<Media>,
}
impl Content {
    /// 显示片段连续覆盖正文，末尾边界即 UTF-16 长度，不必重复扫描长文。
    pub fn display_len(&self) -> usize {
        self.runs.last().map_or(0, |run| run.display.end)
    }
}
#[derive(Clone, Debug)]
pub struct Block {
    pub id: u64,
    pub source: Range<usize>,
    pub content: Arc<Content>,
}
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub blocks: Vec<Block>,
    pub rebuilt: usize,
}
type Semantics = Vec<(Kind, Range<usize>)>;
type Footnotes = Vec<(usize, usize, String)>;
type CachedBlock = (String, bool, Semantics, Footnotes, u64, Arc<Content>);

#[derive(Default)]
pub struct Cache {
    previous: Vec<CachedBlock>,
    previous_ranges: Vec<Range<usize>>,
    next_id: u64,
    previous_source: Option<String>,
    previous_mmd: bool,
    previous_context_safe: bool,
    previous_container_fences: bool,
    cached_blocks: Vec<Block>,
}

fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TABLES
}

fn has_reference_use(text: &str) -> bool {
    text.lines().any(|line| {
        let bytes = line.as_bytes();
        bytes.iter().enumerate().any(|(index, byte)| {
            if *byte != b']' {
                return false;
            }
            let mut next = index + 1;
            while bytes
                .get(next)
                .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
            {
                next += 1;
            }
            bytes.get(next) == Some(&b'[')
        })
    })
}

fn can_reuse_block_semantics(previous: Option<&str>, current: &str) -> bool {
    previous.is_some_and(|previous| context_is_safe(previous) && context_is_safe(current))
}

fn context_is_safe(text: &str) -> bool {
    !text.contains("%%")
        && !text.contains("[^")
        && !text.contains("]:")
        && !text.contains('<')
        && !has_reference_use(text)
}

fn byte_diff(previous: &str, current: &str) -> (usize, usize, usize) {
    let common = previous
        .as_bytes()
        .iter()
        .zip(current.as_bytes())
        .take_while(|(left, right)| left == right)
        .count();
    let mut prefix = common;
    while !previous.is_char_boundary(prefix) || !current.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = 0;
    while suffix < previous.len() - prefix
        && suffix < current.len() - prefix
        && previous.as_bytes()[previous.len() - suffix - 1]
            == current.as_bytes()[current.len() - suffix - 1]
    {
        suffix += 1;
    }
    while !previous.is_char_boundary(previous.len() - suffix)
        || !current.is_char_boundary(current.len() - suffix)
    {
        suffix -= 1;
    }
    (prefix, previous.len() - suffix, current.len() - suffix)
}

fn incremental_ranges(
    previous: &str,
    current: &str,
    previous_ranges: &[Range<usize>],
    previous_context_safe: bool,
    mmd: bool,
) -> Option<(Vec<Range<usize>>, usize)> {
    if mmd
        || !previous_context_safe
        || previous.starts_with("---\n")
        || previous.starts_with("---\r\n")
        || current.starts_with("---\n")
        || current.starts_with("---\r\n")
        || previous_ranges.is_empty()
    {
        return None;
    }
    let (prefix, previous_end, current_end) = byte_diff(previous, current);
    let index = previous_ranges
        .iter()
        .position(|range| range.start <= prefix && prefix < range.end && previous_end <= range.end)
        .or_else(|| {
            previous_ranges
                .last()
                .filter(|range| prefix == range.end && previous_end == prefix)
                .map(|_| previous_ranges.len() - 1)
        })?;
    let old_range = &previous_ranges[index];
    let new_block_end = old_range
        .end
        .checked_add_signed(current_end as isize - previous_end as isize)?;
    let new_range = old_range.start..new_block_end;
    let old_block = previous.get(old_range.clone())?;
    let new_block = current.get(new_range.clone())?;
    if !can_reuse_block_semantics(Some(old_block), new_block)
        || !has_single_top_level_block(new_block)
    {
        return None;
    }
    let delta = current_end as isize - previous_end as isize;
    let mut ranges = Vec::with_capacity(previous_ranges.len());
    for (block_index, range) in previous_ranges.iter().enumerate() {
        ranges.push(match block_index.cmp(&index) {
            std::cmp::Ordering::Less => range.clone(),
            std::cmp::Ordering::Equal => new_range.clone(),
            std::cmp::Ordering::Greater => {
                range.start.checked_add_signed(delta)?..range.end.checked_add_signed(delta)?
            }
        });
    }
    Some((ranges, index))
}

fn has_single_top_level_block(text: &str) -> bool {
    let mut depth = 0usize;
    let mut blocks = 0usize;
    let mut first_start = None;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    blocks += 1;
                    first_start = Some(range.start);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::Rule | Event::Html(_) if depth == 0 => {
                blocks += 1;
                first_start = Some(range.start);
            }
            _ => {}
        }
        if blocks > 1 {
            return false;
        }
    }
    blocks == 1 && first_start == Some(0)
}

/// 把包含容器围栏的顶层块按围栏边界拆开；返回的标记与范围一一对应。
/// 围栏两侧只有空白时并入围栏，避免多出空白块。
fn split_container_fences(
    ranges: Vec<Range<usize>>,
    text: &str,
    fences: &[Range<usize>],
) -> (Vec<Range<usize>>, Vec<bool>) {
    let mut output = Vec::with_capacity(ranges.len() + fences.len());
    let mut diagrams = Vec::with_capacity(output.capacity());
    for range in ranges {
        let mut parts = Vec::new();
        let mut start = range.start;
        let mut found = false;
        for fence in fences {
            if fence.start < range.start || fence.end > range.end {
                continue;
            }
            found = true;
            if fence.start > start {
                parts.push((start..fence.start, false));
            }
            parts.push((fence.clone(), true));
            start = fence.end;
        }
        if !found {
            output.push(range);
            diagrams.push(false);
            continue;
        }
        if start < range.end {
            parts.push((start..range.end, false));
        }
        let mut index = 0;
        while index < parts.len() {
            if !parts[index].1 && text[parts[index].0.clone()].trim().is_empty() {
                if index + 1 < parts.len() && parts[index + 1].1 {
                    parts[index + 1].0.start = parts[index].0.start;
                    parts.remove(index);
                    continue;
                }
                if index > 0 && parts[index - 1].1 {
                    parts[index - 1].0.end = parts[index].0.end;
                    parts.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        for (range, diagram) in parts {
            if range.start < range.end {
                output.push(range);
                diagrams.push(diagram);
            }
        }
    }
    (output, diagrams)
}

impl Cache {
    pub fn model(&mut self, text: &str, mmd: bool) -> Model {
        if self.previous_mmd == mmd && self.previous_source.as_deref() == Some(text) {
            return Model {
                blocks: self.cached_blocks.clone(),
                rebuilt: 0,
            };
        }
        let mut front = 0;
        if text.starts_with("---\n") || text.starts_with("---\r\n") {
            let first = text.find('\n').unwrap() + 1;
            let mut offset = first;
            for line in text[first..].split_inclusive('\n') {
                offset += line.len();
                if matches!(line.trim_end(), "---" | "...") {
                    front = offset;
                    break;
                }
            }
        }
        // 引用块和列表里的围栏由 pulldown 拆出正文，阅读块也要按围栏切开，
        // 才能与 mermaid_blocks 的序号一一对应。
        let fences = if mmd
            || !text.contains("mermaid")
            || !(text.contains("```") || text.contains("~~~"))
        {
            Vec::new()
        } else {
            hortus_diagram::mermaid_blocks(text)
                .into_iter()
                .filter(|block| block.container)
                .map(|block| block.fence_range)
                .collect::<Vec<_>>()
        };
        let incremental = self
            .previous_source
            .as_deref()
            .filter(|_| {
                self.previous_mmd == mmd
                    && front == 0
                    && fences.is_empty()
                    && !self.previous_container_fences
            })
            .and_then(|previous| {
                incremental_ranges(
                    previous,
                    text,
                    &self.previous_ranges,
                    self.previous_context_safe,
                    mmd,
                )
            });
        let ranges = if let Some((ranges, _)) = &incremental {
            ranges.clone()
        } else {
            let mut ranges = Vec::new();
            let mut depth = 0usize;
            let mut start = 0;
            if mmd {
                ranges.push(0..text.len());
            } else {
                if front > 0 {
                    ranges.push(0..front);
                }
                for (event, r) in Parser::new_ext(&text[front..], options()).into_offset_iter() {
                    match event {
                        Event::Start(_) => {
                            if depth == 0 {
                                start = r.start + front;
                            }
                            depth += 1;
                        }
                        Event::End(_) => {
                            depth -= 1;
                            if depth == 0 {
                                ranges.push(start..r.end + front);
                            }
                        }
                        Event::Rule | Event::Html(_) if depth == 0 => {
                            ranges.push(r.start + front..r.end + front)
                        }
                        _ => {}
                    }
                }
            }
            if ranges.is_empty() && !text.is_empty() {
                ranges.push(0..text.len());
            }
            // 块间空白归前一块，保留段落间距；前导空白也有可定位的块。
            if ranges.first().is_some_and(|r| r.start > 0) {
                ranges.insert(0, 0..ranges[0].start);
            }
            for i in 0..ranges.len() {
                ranges[i].end = ranges.get(i + 1).map_or(text.len(), |r| r.start);
            }
            ranges
        };
        let (ranges, diagrams) = split_container_fences(ranges, text, &fences);
        let incremental_index = incremental.as_ref().map(|(_, index)| *index);
        let mut old_blocks: Vec<Option<CachedBlock>> = std::mem::take(&mut self.previous)
            .into_iter()
            .map(Some)
            .collect();
        let mut reused_blocks = vec![None; ranges.len()];
        let mut fast_update = incremental.is_some()
            || (self.previous_mmd == mmd
                && old_blocks.len() == ranges.len()
                && can_reuse_block_semantics(self.previous_source.as_deref(), text));
        if let Some((_, changed_index)) = &incremental {
            for (index, reused) in reused_blocks.iter_mut().enumerate() {
                if index != *changed_index {
                    *reused = Some(index);
                }
            }
        } else if fast_update {
            let mut changed = 0;
            for (index, (previous, range)) in old_blocks.iter().zip(&ranges).enumerate() {
                if previous.as_ref().is_some_and(|block| {
                    block.0 == text[range.clone()] && block.1 == diagrams[index]
                }) {
                    reused_blocks[index] = Some(index);
                } else {
                    changed += 1;
                }
            }
            fast_update = changed <= 1;
            if !fast_update {
                reused_blocks.fill(None);
            }
        }
        let mut semantics = vec![Vec::new(); ranges.len()];
        if fast_update {
            for (index, range) in ranges.iter().enumerate() {
                if reused_blocks[index].is_none() {
                    semantics[index] = markdown::analyze_bytes(&text[range.clone()]);
                }
            }
        } else {
            // 全文扫描处理可能跨块的注释、脚注、引用式链接和 HTML 语境。
            for (kind, span) in markdown::analyze_bytes(text) {
                let first = ranges.partition_point(|r| r.end <= span.start);
                for i in first..ranges.len() {
                    if ranges[i].start >= span.end {
                        break;
                    }
                    semantics[i].push((
                        kind,
                        span.start.max(ranges[i].start) - ranges[i].start
                            ..span.end.min(ranges[i].end) - ranges[i].start,
                    ));
                }
            }
        }
        let mut replacements = vec![Vec::new(); ranges.len()];
        for (r, value) in footnotes(text) {
            let i = ranges.partition_point(|block| block.end <= r.start);
            if let Some(block) = ranges.get(i) {
                replacements[i].push((r.start - block.start, r.end - block.start, value));
            }
        }
        let mut old: HashMap<_, VecDeque<_>> = HashMap::new();
        if !fast_update {
            for (source, diagram, spans, refs, id, content) in old_blocks.drain(..).flatten() {
                old.entry((source, diagram, spans, refs))
                    .or_default()
                    .push_back((id, content));
            }
        }
        let mut result = Model::default();
        let mut byte_offset = 0;
        let mut source_offset = 0;
        let mut next_ranges = Vec::with_capacity(ranges.len());
        for (i, r) in ranges.into_iter().enumerate() {
            next_ranges.push(r.clone());
            if incremental_index.is_none() {
                source_offset += text[byte_offset..r.start].encode_utf16().count();
            }
            let source = &text[r.clone()];
            let diagram = diagrams[i];
            let reused = reused_blocks[i].and_then(|index| old_blocks[index].take());
            let (key, id, content) =
                if let Some((cached_source, cached_diagram, spans, refs, id, content)) = reused {
                    ((cached_source, cached_diagram, spans, refs), id, content)
                } else {
                    let key = (
                        source.to_owned(),
                        diagram,
                        std::mem::take(&mut semantics[i]),
                        std::mem::take(&mut replacements[i]),
                    );
                    let (id, content) =
                        if let Some(cached) = old.get_mut(&key).and_then(VecDeque::pop_front) {
                            cached
                        } else {
                            self.next_id += 1;
                            result.rebuilt += 1;
                            (
                                self.next_id,
                                Arc::new(render(
                                    source,
                                    mmd,
                                    r.start == 0 && front > 0,
                                    diagram,
                                    &key.2,
                                    &key.3,
                                )),
                            )
                        };
                    (key, id, content)
                };
            let length = source.encode_utf16().count();
            result.blocks.push(Block {
                id,
                source: source_offset..source_offset + length,
                content: content.clone(),
            });
            source_offset += length;
            byte_offset = r.end;
            self.previous
                .push((key.0, key.1, key.2, key.3, id, content));
        }
        self.previous_ranges = next_ranges;
        self.previous_source = Some(text.to_owned());
        self.previous_mmd = mmd;
        self.previous_context_safe = incremental.is_some() || context_is_safe(text);
        self.previous_container_fences = !fences.is_empty();
        self.cached_blocks.clone_from(&result.blocks);
        result
    }
}

pub(crate) fn footnotes(text: &str) -> Vec<(Range<usize>, String)> {
    let mut numbers = HashMap::new();
    let mut footnotes = Vec::new();
    if text.contains("[^") {
        for (event, r) in Parser::new_ext(text, options()).into_offset_iter() {
            if let Event::FootnoteReference(label) = event {
                let next = numbers.len() + 1;
                let number = *numbers.entry(label.to_string()).or_insert(next);
                footnotes.push((r, format!("[{number}]")));
            }
        }
        for (event, r) in Parser::new_ext(text, options()).into_offset_iter() {
            if let Event::Start(Tag::FootnoteDefinition(label)) = event {
                let next = numbers.len() + 1;
                let number = *numbers.entry(label.to_string()).or_insert(next);
                if let Some(end) = text[r.clone()].find("]:") {
                    footnotes.push((r.start..r.start + end + 2, format!("[{number}]")));
                }
            }
        }
    }
    footnotes.sort_by_key(|(range, _)| range.start);
    footnotes
}

pub(crate) fn heading_text(
    text: &str,
    spans: &[(Kind, Range<usize>)],
    refs: &[(usize, usize, String)],
) -> String {
    render(text, false, false, false, spans, refs)
        .text
        .trim()
        .replace(['\r', '\n'], " ")
}

fn render(
    text: &str,
    mmd: bool,
    front: bool,
    diagram: bool,
    spans: &[(Kind, Range<usize>)],
    refs: &[(usize, usize, String)],
) -> Content {
    let full_render = front
        || mmd
        || diagram
        || mermaid_fence(text)
        || !refs.is_empty()
        || text.contains("![")
        || text.lines().any(|line| line.contains('|'))
        || spans.iter().any(|(kind, _)| {
            matches!(
                kind,
                Kind::Comment
                    | Kind::OrderedList
                    | Kind::UnorderedList
                    | Kind::Task
                    | Kind::WikiLink
                    | Kind::Embed
                    | Kind::Callout
                    | Kind::Footnote
                    | Kind::Image
                    | Kind::Rule
            )
        });
    if !full_render {
        return render_simple(text, spans);
    }
    render_full(text, mmd, front, diagram, spans, refs)
}

fn render_simple(text: &str, spans: &[(Kind, Range<usize>)]) -> Content {
    let mut result = Content::default();
    let mut byte = 0;
    let mut source16 = 0;
    let mut display16 = 0;
    let mut first = 0;
    while byte < text.len() {
        while first < spans.len() && spans[first].1.end <= byte {
            first += 1;
        }
        let mut mask = 0;
        let mut index = first;
        while index < spans.len() && spans[index].1.start <= byte {
            if byte < spans[index].1.end {
                mask |= spans[index].0.mask();
            }
            index += 1;
        }
        let character = text[byte..].chars().next().unwrap();
        let source_end = source16 + character.len_utf16();
        if mask & 1 == 0 {
            let display_end = display16 + character.len_utf16();
            let style = mask & !1;
            if let Some(last) = result.runs.last_mut().filter(|last| {
                last.mask == style
                    && last.source.end == source16
                    && last.display.end == display16
                    && last.source.len() == last.display.len()
            }) {
                last.source.end = source_end;
                last.display.end = display_end;
            } else {
                result.runs.push(Run {
                    display: display16..display_end,
                    source: source16..source_end,
                    mask: style,
                });
            }
            result.text.push(character);
            display16 = display_end;
        }
        source16 = source_end;
        byte += character.len_utf8();
    }
    result
}

pub(crate) fn inline(text: &str) -> Content {
    let mut spans = media::inline_spans(text);
    for (index, _) in text.match_indices("\\|") {
        if text[..index]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 0
        {
            spans.push((Kind::Marker, index..index + 1));
        }
    }
    spans.sort_by_key(|(_, range)| (range.start, range.end));
    render_full(text, false, false, false, &spans, &[])
}

fn render_full(
    text: &str,
    mmd: bool,
    front: bool,
    diagram: bool,
    spans: &[(Kind, Range<usize>)],
    refs: &[(usize, usize, String)],
) -> Content {
    let mut result = Content::default();
    let mut masks = vec![0u32; text.len()];
    for (kind, r) in spans {
        for mask in &mut masks[r.clone()] {
            *mask |= kind.mask();
        }
    }
    let mut replacements: HashMap<usize, (usize, String, u32)> = HashMap::new();
    let mut suppress = vec![false; text.len()];
    let mut attachments: HashMap<usize, (Range<usize>, media::Kind)> = HashMap::new();
    for (kind, r) in spans {
        if *kind == Kind::Comment {
            suppress[r.clone()].fill(true);
            if r.start >= 2 && &text[r.start - 2..r.start] == "%%" {
                suppress[r.start - 2..r.start].fill(true);
            }
            if text.get(r.end..r.end + 2) == Some("%%") {
                suppress[r.end..r.end + 2].fill(true);
            }
        }
    }
    // 纯注释行连同换行符一起去掉；段落原有的空白间距仍保留。
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let end = line_start + line.len();
        if !line.trim().is_empty()
            && line
                .char_indices()
                .all(|(index, c)| c.is_whitespace() || suppress[line_start + index])
        {
            suppress[line_start..end].fill(true);
        }
        line_start = end;
    }
    if !mmd
        && suppress.iter().any(|value| *value)
        && text
            .char_indices()
            .all(|(index, c)| c.is_whitespace() || suppress[index])
    {
        return result;
    }
    if front {
        let mut fields = Vec::new();
        for line in text.lines().skip(1) {
            if matches!(line.trim(), "---" | "...") {
                break;
            }
            if !line.trim().is_empty() {
                fields.push(line.trim());
            }
        }
        replacements.insert(
            0,
            (
                text.len(),
                format!("\u{e000}  {}\n", fields.join("  ·  ")),
                16384,
            ),
        );
    } else if mmd || diagram || mermaid_fence(text) {
        result.diagram = true;
        // 成功的图由平台以单个占位段落绘制；失败时平台放回该块的完整源码。
        replacements.insert(0, (text.len(), "\u{fffc}\n".into(), 0));
    } else {
        for (start, end, value) in refs {
            replacements.insert(*start, (*end, value.clone(), 8192));
        }
        if let Some((range, table)) = text
            .contains('|')
            .then(|| Table::parse(text))
            .flatten()
            .filter(|(range, _)| !suppress[range.start])
        {
            replacements.insert(range.start, (range.end, "\u{fffc}\n".into(), 0));
            attachments.insert(range.start, (range, media::Kind::Table(table)));
        }
        if text.contains("![") {
            for (event, r) in Parser::new_ext(text, options()).into_offset_iter() {
                if let Event::Start(Tag::Image { dest_url, .. }) = event {
                    if suppress[r.start] || masks[r.start] & (16 | 32) != 0 {
                        continue;
                    }
                    if attachments
                        .values()
                        .any(|(span, _)| span.contains(&r.start))
                    {
                        continue;
                    }
                    let alt = Parser::new_ext(&text[r.clone()], options())
                        .filter_map(|event| match event {
                            Event::Text(text) | Event::Code(text) => Some(text.to_string()),
                            _ => None,
                        })
                        .collect::<String>();
                    replacements.insert(r.start, (r.end, "\n\u{fffc}\n".into(), 0));
                    attachments.insert(
                        r.start,
                        (
                            r,
                            media::Kind::Image(Image {
                                target: dest_url.into_string(),
                                alt,
                                wiki: false,
                                width: None,
                            }),
                        ),
                    );
                }
            }
        }
        for (_, range) in spans.iter().filter(|(kind, _)| *kind == Kind::Embed) {
            let range = range.start.saturating_sub(3)..(range.end + 2).min(text.len());
            let raw = &text[range.clone()];
            if let Some(inner) = raw.strip_prefix("![[").and_then(|s| s.strip_suffix("]]")) {
                replacements.insert(range.start, (range.end, "\n\u{fffc}\n".into(), 0));
                attachments.insert(
                    range.start,
                    (range.clone(), media::Kind::Image(Image::wiki(inner))),
                );
            }
        }
        for (kind, r) in spans {
            let r = r.clone();
            if attachments
                .values()
                .any(|(span, _)| span.contains(&r.start))
            {
                continue;
            }
            match kind {
                Kind::OrderedList | Kind::UnorderedList => {
                    let start = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
                    let marker = spans.iter().find(|(kind, marker)| {
                        *kind == Kind::Marker && marker.end == r.start && marker.start >= start
                    });
                    if let Some((_, marker)) = marker {
                        let value = if *kind == Kind::OrderedList {
                            text[marker.clone()].to_owned()
                        } else {
                            "• ".into()
                        };
                        replacements.insert(marker.start, (marker.end, value, 0));
                    }
                }
                Kind::Task => {
                    // 项目符号与任务框合并，只保留一个框。
                    if let Some((_, replacement, _)) = replacements
                        .values_mut()
                        .find(|(end, _, _)| *end == r.start)
                    {
                        replacement.clear();
                    }
                    replacements.insert(
                        r.start,
                        (
                            r.end,
                            if text[r.clone()].contains(['x', 'X']) {
                                "☑ "
                            } else {
                                "☐ "
                            }
                            .into(),
                            131072,
                        ),
                    );
                }
                Kind::WikiLink | Kind::Embed => {
                    if let Some(pipe) = text[r.clone()].find('|') {
                        suppress[r.start..r.start + pipe + 1].fill(true);
                    }
                }
                Kind::Callout => {
                    if let Some(end) = text[r.clone()].find(']') {
                        let kind = &text[r.start + 2..r.start + end];
                        replacements
                            .insert(r.start, (r.start + end + 1, format!("{kind} ·"), 4096 | 2));
                    }
                }
                Kind::Rule => {
                    replacements.insert(r.start, (r.end, "\u{200b}\n".into(), 32768));
                }
                _ => {}
            }
        }
        // 语义按范围排序，任务与列表可能先后任一顺序；最后合并项目符号。
        for (_, task) in spans.iter().filter(|(kind, _)| *kind == Kind::Task) {
            for (end, value, _) in replacements.values_mut() {
                if *end == task.start {
                    value.clear();
                }
            }
        }
    }
    let mut byte = 0;
    let mut source16 = 0;
    let mut display16 = 0;
    while byte < text.len() {
        let (end, value, mask) = if let Some((end, value, mask)) = replacements.remove(&byte) {
            (end, value, mask)
        } else {
            let c = text[byte..].chars().next().unwrap();
            let mask = masks[byte];
            let hidden = suppress[byte] || mask & (1 | 2048) != 0;
            (
                byte + c.len_utf8(),
                if hidden { String::new() } else { c.to_string() },
                mask & !1,
            )
        };
        let source_end = source16 + text[byte..end].encode_utf16().count();
        let display_end = display16 + value.encode_utf16().count();
        if let Some((span, kind)) = attachments.remove(&byte) {
            let leading = value.encode_utf16().take_while(|c| *c == 10).count();
            result.media.push(Media {
                source: source16..source16 + text[span].encode_utf16().count(),
                display: display16 + leading..display16 + leading + 1,
                kind,
            });
        }
        if !value.is_empty() {
            if let Some(last) = result.runs.last_mut().filter(|last| {
                last.mask == mask
                    && last.source.end == source16
                    && last.display.end == display16
                    && last.source.len() == last.display.len()
                    && source_end - source16 == display_end - display16
            }) {
                last.source.end = source_end;
                last.display.end = display_end;
            } else {
                result.runs.push(Run {
                    display: display16..display_end,
                    source: source16..source_end,
                    mask,
                });
            }
            result.text.push_str(&value);
        }
        source16 = source_end;
        display16 = display_end;
        byte = end;
    }
    result
}

fn mermaid_fence(text: &str) -> bool {
    let Some(line) = text.lines().next() else {
        return false;
    };
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let trimmed = &line[indent..];
    let Some(fence) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return false;
    };
    let length = trimmed.chars().take_while(|c| *c == fence).count();
    length >= 3 && trimmed[length..].split_whitespace().next() == Some("mermaid")
}

#[cfg(test)]
mod tests {
    use super::{markdown, render_full, render_simple};

    #[test]
    fn simple_renderer_preserves_text_styles_and_source_mapping() {
        let source = "# 标题😀\n\n正文 **加粗** *斜体* ~~删除~~ ==高亮== `代码` [链接](https://example.org) #标签\n\n> 引用\n\n```text\n代码块\n```\n";
        let spans = markdown::analyze_bytes(source);
        let simple = render_simple(source, &spans);
        let full = render_full(source, false, false, false, &spans, &[]);
        assert_eq!(simple.text, full.text);
        assert_eq!(simple.diagram, full.diagram);
        assert_eq!(simple.runs.len(), full.runs.len());
        for (simple, full) in simple.runs.iter().zip(&full.runs) {
            assert_eq!(simple.display, full.display);
            assert_eq!(simple.source, full.source);
            assert_eq!(simple.mask, full.mask);
        }
    }
}
