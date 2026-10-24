//! 标题来自与阅读视图相同的解析语义；偏移始终使用源码 UTF-16 单位。
use crate::{markdown, reading};
use pulldown_cmark::{Event, Options, Parser, Tag};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub title: String,
    pub start: usize,
}

pub(crate) fn parse(text: &str) -> Vec<Heading> {
    // 注释会把标题语义拆成多段；结构仍以解析器的一整个标题为准。
    let front = markdown::body_start(text);
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TABLES;
    let headings = Parser::new_ext(&text[front..], options)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::Heading { level, .. }) => {
                Some((level as u8, range.start + front..range.end + front))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if headings.is_empty() {
        return Vec::new();
    }
    // 普通 ATX 标题的行内语义只依赖自身；跨段注释、引用定义和 setext 保留全文语境。
    let spans = if !text.contains("%%")
        && !text.contains("]:")
        && headings
            .iter()
            .all(|(_, range)| text[range.clone()].starts_with('#'))
    {
        headings
            .iter()
            .flat_map(|(_, range)| {
                markdown::analyze_bytes(&text[range.clone()])
                    .into_iter()
                    .map(|(kind, local)| (kind, local.start + range.start..local.end + range.start))
            })
            .collect::<Vec<_>>()
    } else {
        markdown::analyze_bytes(text)
    };
    let footnotes = reading::footnotes(text);
    let mut result = Vec::new();
    let mut active = Vec::new();
    let mut next = 0;
    let mut byte = 0;
    let mut utf16 = 0;
    for (level, range) in headings {
        active.retain(|&index: &usize| spans[index].1.end > range.start);
        while next < spans.len() && spans[next].1.start < range.end {
            if spans[next].1.end > range.start {
                active.push(next);
            }
            next += 1;
        }
        if !active
            .iter()
            .any(|&index| matches!(spans[index].0, markdown::Kind::Heading(_)))
        {
            continue;
        }
        utf16 += text[byte..range.start].encode_utf16().count();
        byte = range.start;
        let local = active
            .iter()
            .map(|&index| {
                let (kind, span) = &spans[index];
                (
                    *kind,
                    span.start.max(range.start) - range.start
                        ..span.end.min(range.end) - range.start,
                )
            })
            .collect::<Vec<_>>();
        let first = footnotes.partition_point(|(span, _)| span.end <= range.start);
        let refs = footnotes[first..]
            .iter()
            .take_while(|(span, _)| span.start < range.end)
            .filter(|(span, _)| span.start >= range.start && span.end <= range.end)
            .map(|(span, value)| {
                (
                    span.start - range.start,
                    span.end - range.start,
                    value.clone(),
                )
            })
            .collect::<Vec<_>>();
        let title = reading::heading_text(&text[range.clone()], &local, &refs);
        result.push(Heading {
            level,
            title,
            start: utf16,
        });
    }
    result
}
