//! Note parsing: frontmatter fields, inline tags and wiki links.
//!
//! The scanner is byte-oriented but only slices at character boundaries. Every
//! construct is identified with explicit UTF-8 byte ranges so later renames can
//! replace exactly the target text and nothing else.

use crate::{Link, ParsedNote, Tag};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag as MarkdownTag, TagEnd};
use std::collections::BTreeSet;
use std::ops::Range;

const CONTEXT_LIMIT: usize = 160;

pub(crate) fn parse_note(text: &str) -> ParsedNote {
    let mut note = parse_occurrences(text);
    let mut seen = BTreeSet::new();
    note.tags.retain(|tag| seen.insert(tag.key.clone()));
    note
}

pub(crate) fn parse_occurrences(text: &str) -> ParsedNote {
    let frontmatter = crate::frontmatter::parse(text);
    let body_start = frontmatter.as_ref().map_or(0, |front| front.end);
    let lines = Lines::new(text);
    let mut tags = Vec::new();
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    let mut source = None;
    if let Some(front) = &frontmatter {
        for (raw, line) in &front.tags {
            push_tag(&mut tags, &mut seen, raw, *line, true, text, &lines);
        }
        aliases = front.aliases.clone();
        source = front.source.clone();
    }

    let mut ignored = vec![false; text.len()];
    if body_start > 0 {
        mark(&mut ignored, 0..body_start.min(text.len()));
    }
    let mut diagrams = Vec::new();
    mask_blocks(text, body_start, &mut ignored, &mut diagrams);
    mask_inline(text, &mut ignored);

    let mut links = scan_links(text, &ignored, body_start, &lines);
    for range in diagrams {
        links.extend(scan_diagram_links(text, range, &lines));
    }
    links.sort_by_key(|link| link.start);

    // Markdown link targets and aliases are not tag sources.
    for link in &links {
        mark(&mut ignored, link.start..link.end);
    }

    scan_tags(text, &ignored, body_start, &lines, &mut tags);

    ParsedNote {
        tags,
        aliases,
        source,
        links,
    }
}

fn push_tag(
    tags: &mut Vec<Tag>,
    seen: &mut BTreeSet<String>,
    raw: &str,
    line: usize,
    frontmatter: bool,
    text: &str,
    lines: &Lines,
) {
    let raw = raw.trim().trim_start_matches('#').trim();
    if raw.is_empty() {
        return;
    }
    let key = raw.to_ascii_lowercase();
    if !seen.insert(key.clone()) {
        return;
    }
    let occurrence = if frontmatter {
        find_line(text, lines, line, raw)
    } else {
        None
    };
    let (start, end) = occurrence.unwrap_or((0, 0));
    tags.push(Tag {
        raw: raw.to_owned(),
        key,
        start,
        end,
        line,
        frontmatter,
    });
}

fn find_line(text: &str, lines: &Lines, line: usize, raw: &str) -> Option<(usize, usize)> {
    let start = *lines.starts.get(line.checked_sub(1)?)?;
    let end = line_end(text, start);
    let offset = text.get(start..end)?.find(raw)?;
    Some((start + offset, start + offset + raw.len()))
}

struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self { starts }
    }

    /// 1-based line number for a byte offset.
    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset).max(1)
    }

    fn start(&self, line: usize) -> usize {
        self.starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }
}

fn line_end(text: &str, offset: usize) -> usize {
    text[offset.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

fn next_line_start(text: &str, offset: usize) -> usize {
    (line_end(text, offset) + 1).min(text.len())
}

fn next_char_len(text: &str, index: usize) -> usize {
    text.get(index..)
        .and_then(|rest| rest.chars().next())
        .map_or(1, char::len_utf8)
}

fn mark(ignored: &mut [bool], range: Range<usize>) {
    let start = range.start.min(ignored.len());
    let end = range.end.min(ignored.len());
    if start < end {
        ignored[start..end].fill(true);
    }
}

fn context_at(text: &str, lines: &Lines, offset: usize) -> String {
    let line = lines.line_of(offset);
    let start = lines.start(line);
    let end = line_end(text, start);
    let line = text.get(start..end).unwrap_or("").trim();
    let mut context = String::new();
    for (count, character) in line.chars().enumerate() {
        if count == CONTEXT_LIMIT {
            context.push('…');
            break;
        }
        context.push(character);
    }
    context
}

fn mask_blocks(
    text: &str,
    body_start: usize,
    ignored: &mut [bool],
    diagrams: &mut Vec<Range<usize>>,
) {
    struct OpenBlock {
        start: usize,
        body_start: usize,
        last_text_end: usize,
        mermaid: bool,
    }
    let begin = body_start.min(text.len());
    let mut open: Option<OpenBlock> = None;
    let events = Parser::new_ext(&text[begin..], Options::empty())
        .into_offset_iter()
        .map(|(event, range)| (event, begin + range.start..begin + range.end));
    for (event, range) in events {
        match event {
            Event::Start(MarkdownTag::CodeBlock(kind)) => {
                let (body_start, mermaid) = match &kind {
                    CodeBlockKind::Fenced(info) => (
                        next_line_start(text, range.start),
                        info.split_whitespace().next() == Some("mermaid"),
                    ),
                    CodeBlockKind::Indented => (range.start, false),
                };
                open = Some(OpenBlock {
                    start: range.start,
                    body_start,
                    last_text_end: body_start,
                    mermaid,
                });
            }
            Event::Text(_) => {
                if let Some(block) = &mut open {
                    block.last_text_end = range.end;
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let Some(block) = open.take() else {
                    continue;
                };
                mark(ignored, block.start..range.end);
                if block.mermaid {
                    // 有闭合围栏时正文止于围栏行首；未闭合时止于块的实际结尾。
                    let closed = range.end > block.last_text_end;
                    let body_end = if closed {
                        line_start(text, range.end.saturating_sub(1))
                    } else {
                        range.end
                    };
                    diagrams.push(block.body_start.min(body_end)..body_end);
                }
            }
            _ => {}
        }
    }
}

fn mask_inline(text: &str, ignored: &mut [bool]) {
    let mut index = 0;
    while index < text.len() {
        if ignored[index] {
            index += next_char_len(text, index);
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '\\' {
            index += character.len_utf8();
            if index < text.len() {
                index += next_char_len(text, index);
            }
            continue;
        }
        if text[index..].starts_with("<!--") {
            let end = text[index + 4..].find("-->").map_or_else(
                || line_end(text, index),
                |position| index + 4 + position + 3,
            );
            let end = end.max(index + 4);
            mark(ignored, index..end);
            index = end;
            continue;
        }
        if text[index..].starts_with("%%") {
            let end = text[index + 2..].find("%%").map_or_else(
                || line_end(text, index),
                |position| index + 2 + position + 2,
            );
            let end = end.max(index + 2);
            mark(ignored, index..end);
            index = end.max(index + 2);
            continue;
        }
        if character == '`' {
            let run = count_run(text, index, b'`');
            let limit = line_end(text, index);
            if let Some(end) = find_run(text, index + run, b'`', run, limit) {
                mark(ignored, index..end);
                index = end;
            } else {
                mark(ignored, index..index + run);
                index += run;
            }
            continue;
        }
        if text[index..].starts_with("$$") {
            let end = text[index + 2..].find("$$").map_or_else(
                || line_end(text, index),
                |position| index + 2 + position + 2,
            );
            let end = end.max(index + 2);
            mark(ignored, index..end);
            index = end;
            continue;
        }
        if character == '$' {
            index = mask_inline_math(text, index, ignored);
            continue;
        }
        index += character.len_utf8();
    }
}

fn mask_inline_math(text: &str, index: usize, ignored: &mut [bool]) -> usize {
    let Some(next) = text[index + 1..].chars().next() else {
        return index + 1;
    };
    if next.is_whitespace() || next == '$' {
        return index + 1;
    }
    let end = line_end(text, index);
    let mut cursor = index + 1;
    while cursor < end {
        if text[cursor..].starts_with('$') && !text[cursor..].starts_with("$$") {
            let previous = text[..cursor].chars().next_back().unwrap_or(' ');
            if !previous.is_whitespace() && previous != '\\' {
                mark(ignored, index..cursor + 1);
                return cursor + 1;
            }
        }
        cursor += next_char_len(text, cursor);
    }
    index + 1
}

fn count_run(text: &str, index: usize, byte: u8) -> usize {
    text.as_bytes()
        .get(index..)
        .map(|bytes| {
            bytes
                .iter()
                .take_while(|candidate| **candidate == byte)
                .count()
        })
        .unwrap_or(0)
}

fn find_run(text: &str, from: usize, byte: u8, length: usize, limit: usize) -> Option<usize> {
    let mut index = from;
    while index < limit {
        let position = text[index..limit].find(byte as char)?;
        let at = index + position;
        let run = count_run(text, at, byte);
        if run == length {
            return Some(at + run);
        }
        index = at + run;
    }
    None
}

fn escaped_link_end(text: &str, index: usize, limit: usize) -> Option<usize> {
    let rest = text.get(index..limit)?;
    let skip = if rest.starts_with("\\[[") {
        3
    } else if rest.starts_with("\\![[") {
        4
    } else {
        return None;
    };
    let mut cursor = index + skip;
    while cursor < limit {
        if text[cursor..].starts_with('\n') || text[cursor..].starts_with('\r') {
            return None;
        }
        if text[cursor..].starts_with("]]") {
            return Some(cursor + 2);
        }
        cursor += next_char_len(text, cursor);
    }
    None
}

fn scan_links(text: &str, ignored: &[bool], body_start: usize, lines: &Lines) -> Vec<Link> {
    let mut links = Vec::new();
    let mut index = body_start.min(text.len());
    while index + 1 < text.len() {
        if ignored.get(index).copied().unwrap_or(false) {
            index += next_char_len(text, index);
            continue;
        }
        if text.as_bytes()[index] == b'\\' {
            if let Some(end) = escaped_link_end(text, index, text.len()) {
                index = end;
            } else {
                index += 1;
                if index < text.len() {
                    index += next_char_len(text, index);
                }
            }
            continue;
        }
        let embed = text[index..].starts_with("![[");
        if (embed || text[index..].starts_with("[["))
            && let Some(link) = parse_link_at(text, index, embed, false, lines)
        {
            index = link.end;
            links.push(link);
            continue;
        }
        index += next_char_len(text, index);
    }
    links
}

fn scan_diagram_links(text: &str, range: Range<usize>, lines: &Lines) -> Vec<Link> {
    let mut links = Vec::new();
    let mut index = range.start.min(text.len());
    let end = range.end.min(text.len());
    let mut quoted = false;
    let mut escaped = false;
    while index < end {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if escaped {
            escaped = false;
            index += character.len_utf8();
            continue;
        }
        if character == '\\' {
            if quoted {
                escaped = true;
                index += 1;
                continue;
            }
            if let Some(next) = escaped_link_end(text, index, end) {
                index = next;
            } else {
                index += 1;
                if index < end {
                    index += next_char_len(text, index);
                }
            }
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            index += 1;
            continue;
        }
        if !quoted && text[index..end].starts_with("%%") {
            index = skip_diagram_comment(text, index, end);
            continue;
        }
        let embed = text[index..end].starts_with("![[");
        if (embed || text[index..end].starts_with("[["))
            && let Some(link) = parse_link_at(text, index, embed, true, lines)
        {
            if link.end > end {
                break;
            }
            index = link.end;
            if is_state_pseudo_link(text, &link) {
                continue;
            }
            links.push(link);
            continue;
        }
        index += character.len_utf8();
    }
    links
}

/// Mermaid 的 `%%` 到行尾结束；`%%{...}%%` 指令才跨行配对。
fn skip_diagram_comment(text: &str, start: usize, end: usize) -> usize {
    let rest = &text[start + 2..end];
    if rest.starts_with('{') {
        if let Some(position) = rest.find("}%%") {
            return start + 2 + position + 3;
        }
        if let Some(position) = rest.find("%%") {
            return start + 2 + position + 2;
        }
        return end;
    }
    text[start..end]
        .find('\n')
        .map_or(end, |position| start + position)
}

/// `state X [[choice]]` 等状态伪状态后缀不是 Wiki 链接。
fn is_state_pseudo_link(text: &str, link: &Link) -> bool {
    if !matches!(link.raw_target.as_str(), "choice" | "fork" | "join")
        || link.heading.is_some()
        || link.alias.is_some()
    {
        return false;
    }
    let start = line_start(text, link.start);
    let end = line_end(text, link.end);
    let prefix = strip_container_prefix(&text[start..link.start]);
    let Some(rest) = prefix.strip_prefix("state") else {
        return false;
    };
    let suffix = text[link.end..end].trim_start();
    rest.starts_with(char::is_whitespace)
        && (suffix.is_empty() || suffix.starts_with("%%") || suffix.starts_with(':'))
}

fn strip_container_prefix(mut line: &str) -> &str {
    loop {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('>') {
            line = rest;
        } else {
            return trimmed;
        }
    }
}

fn parse_link_at(
    text: &str,
    start: usize,
    embed: bool,
    in_diagram: bool,
    lines: &Lines,
) -> Option<Link> {
    let content_start = start + if embed { 3 } else { 2 };
    if content_start > text.len() {
        return None;
    }
    let mut end = content_start;
    while end < text.len() {
        if text[end..].starts_with('\n') || text[end..].starts_with('\r') {
            return None;
        }
        if text[end..].starts_with("]]") {
            break;
        }
        end += next_char_len(text, end);
    }
    if end >= text.len() {
        return None;
    }
    let content = text.get(content_start..end)?;
    if content.trim().is_empty() {
        return None;
    }
    let (before_alias, alias) = match content.find('|') {
        Some(position) => (&content[..position], Some(content[position + 1..].trim())),
        None => (content, None),
    };
    let alias = alias.filter(|value| !value.is_empty()).map(str::to_owned);
    let (target_part, heading) = match before_alias.find('#') {
        Some(position) => (
            &before_alias[..position],
            Some(before_alias[position + 1..].trim()),
        ),
        None => (before_alias, None),
    };
    let heading = heading.filter(|value| !value.is_empty()).map(str::to_owned);
    let target = target_part.trim();
    let leading = target_part.len() - target_part.trim_start().len();
    let target_start = content_start + leading;
    Some(Link {
        raw_target: target.to_owned(),
        heading,
        alias,
        embed,
        in_diagram,
        start,
        end: end + 2,
        target_range: target_start..target_start + target.len(),
        line: lines.line_of(start),
        context: context_at(text, lines, start),
    })
}

fn scan_tags(text: &str, ignored: &[bool], body_start: usize, lines: &Lines, tags: &mut Vec<Tag>) {
    let mut index = body_start.min(text.len());
    while index < text.len() {
        if ignored.get(index).copied().unwrap_or(false) {
            index += next_char_len(text, index);
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '\\' {
            index += character.len_utf8();
            if index < text.len() {
                index += next_char_len(text, index);
            }
            continue;
        }
        if character != '#' {
            index += character.len_utf8();
            continue;
        }
        if index > 0 {
            let previous = text[..index].chars().next_back().unwrap_or(' ');
            if previous.is_alphanumeric() || matches!(previous, '_' | '/' | '\\') {
                index += 1;
                continue;
            }
        }
        if is_heading_marker(text, index) {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        let mut first = true;
        while cursor < text.len() {
            let Some(character) = text[cursor..].chars().next() else {
                break;
            };
            let allowed = if first {
                character.is_alphanumeric() || character == '_'
            } else {
                character.is_alphanumeric() || matches!(character, '_' | '-' | '/')
            };
            if !allowed {
                break;
            }
            first = false;
            cursor += character.len_utf8();
        }
        if cursor == index + 1 {
            index += 1;
            continue;
        }
        let raw = &text[index + 1..cursor];
        if !raw.chars().all(|character| character.is_ascii_digit()) {
            let key = raw.to_ascii_lowercase();
            tags.push(Tag {
                raw: raw.to_owned(),
                key,
                start: index,
                end: cursor,
                line: lines.line_of(index),
                frontmatter: false,
            });
        }
        index = cursor;
    }
}

fn is_heading_marker(text: &str, index: usize) -> bool {
    let line_start = text[..index].rfind('\n').map_or(0, |position| position + 1);
    let prefix = &text[line_start..index];
    if prefix.len() > 3 || !prefix.chars().all(|character| character == ' ') {
        return false;
    }
    text[index + 1..]
        .chars()
        .next()
        .is_some_and(|character| character == ' ' || character == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_frontmatter_fields_and_body_links() {
        let text = "---\ntags: [alpha, #nested/beta]\naliases: [First, Second]\nsource: spec.md\n---\n# Title\n\nSee [[Target|Alias]] and ![[picture.png]].\n";
        let note = parse_note(text);
        assert_eq!(
            note.tags
                .iter()
                .map(|tag| (tag.key.as_str(), tag.frontmatter))
                .collect::<Vec<_>>(),
            vec![("alpha", true), ("nested/beta", true)]
        );
        assert_eq!(note.aliases, vec!["First", "Second"]);
        assert_eq!(note.source.as_deref(), Some("spec.md"));
        assert_eq!(note.links.len(), 2);
        assert_eq!(note.links[0].raw_target, "Target");
        assert_eq!(note.links[0].alias.as_deref(), Some("Alias"));
        assert!(!note.links[0].embed);
        assert_eq!(note.links[1].raw_target, "picture.png");
        assert!(note.links[1].embed);
        assert!(note.links[1].is_attachment());
    }

    #[test]
    fn reads_inline_tags_with_prefixes_and_rejects_noise() {
        let text = "# Heading\n\n#rust #架构/索引 #dev_2026 #123\n\nhttps://example.com#anchor and foo#bar\n\n(#after) \\#escaped\n";
        let note = parse_note(text);
        assert_eq!(
            note.tags
                .iter()
                .map(|tag| tag.raw.as_str())
                .collect::<Vec<_>>(),
            vec!["rust", "架构/索引", "dev_2026", "after"]
        );
        assert_eq!(note.tags[0].line, 3);
    }

    #[test]
    fn skips_code_math_comments_and_formulas() {
        let text = concat!(
            "```rust\n[[code-link]] #code-tag\n```\n\n",
            "~~~mermaid\nflowchart TB\n  A[\"[[Diagram Note]]\"] --> B\n~~~\n\n",
            "    [[indented]] #indented-tag\n\n",
            "`[[inline-code]]` and `#inline-tag`\n\n",
            "$$\n[[math]] #math-tag\n$$\n\n",
            "$[[inline-math]]$ and $x + y$\n\n",
            "<!-- [[comment]] #comment-tag -->\n\n",
            "%% [[obsidian-comment]] #obsidian-tag %%\n\n",
            "[[live]] #live-tag\n",
        );
        let note = parse_note(text);
        assert_eq!(
            note.links
                .iter()
                .map(|link| link.raw_target.as_str())
                .collect::<Vec<_>>(),
            vec!["Diagram Note", "live"]
        );
        assert!(note.links[0].in_diagram);
        assert!(!note.links[1].in_diagram);
        assert_eq!(
            note.tags
                .iter()
                .map(|tag| tag.key.as_str())
                .collect::<Vec<_>>(),
            vec!["live-tag"]
        );
    }

    #[test]
    fn parses_headings_aliases_and_byte_ranges() {
        let text = "Links: [[dir/Target#Section|Label]] and [[#Local]].\n";
        let note = parse_note(text);
        assert_eq!(note.links.len(), 2);
        let link = &note.links[0];
        assert_eq!(link.raw_target, "dir/Target");
        assert_eq!(link.heading.as_deref(), Some("Section"));
        assert_eq!(link.alias.as_deref(), Some("Label"));
        assert_eq!(&text[link.target_range.clone()], "dir/Target");
        assert_eq!(&text[link.start..link.end], "[[dir/Target#Section|Label]]");
        assert_eq!(note.links[1].raw_target, "");
        assert_eq!(note.links[1].heading.as_deref(), Some("Local"));
    }

    #[test]
    fn escaped_links_are_literal() {
        let note = parse_note("\\[[escaped]] \\![[also-escaped]] \\\\[[real]]\n");
        assert_eq!(
            note.links
                .iter()
                .map(|link| link.raw_target.as_str())
                .collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    #[test]
    fn unclosed_and_multiline_constructs_never_panic() {
        let samples = [
            "[[",
            "[[unclosed",
            "```mermaid\n[[x]]",
            "$$\n[[x]]",
            "<!-- [[x]]",
            "%% [[x]]",
            "`[[x]]",
            "![[]]",
            "[[#]]",
            "#tag",
            "#",
            "中文😀[[目标#标题|别名]]😀",
            "\u{feff}---\ntags: [x]\n---\n[[a]]\n",
        ];
        for sample in samples {
            for boundary in 0..=sample.len() {
                if sample.is_char_boundary(boundary) {
                    let _ = parse_note(&sample[..boundary]);
                    let _ = parse_note(&sample[boundary..]);
                }
            }
        }
    }

    #[test]
    fn handles_bom_and_crlf_input() {
        let note = parse_note("\u{feff}---\r\ntags: [one]\r\n---\r\n[[Target]]\r\n");
        assert_eq!(note.tags[0].key, "one");
        assert_eq!(note.links[0].raw_target, "Target");
        assert_eq!(note.links[0].line, 4);
    }

    #[test]
    fn tags_inside_mermaid_are_not_inline_tags() {
        let note = parse_note("```mermaid\nflowchart TB\nA[#not-a-tag] --> B[\"[[Note]]\"]\n```\n");
        assert!(note.tags.is_empty());
        assert_eq!(note.links[0].raw_target, "Note");
    }

    #[test]
    fn mermaid_comment_at_eof_keeps_earlier_links() {
        let note = parse_note(
            "```mermaid\nflowchart TB\nA[\"[[Kept]]\"] --> B\n%% trailing without newline",
        );
        assert_eq!(note.links.len(), 1);
        assert_eq!(note.links[0].raw_target, "Kept");
        assert!(note.links[0].in_diagram);
    }
}
