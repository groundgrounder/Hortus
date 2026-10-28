//! 美元符边界完全由 pulldown 决定；只把纯公式段落作为块预览。
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Display,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formula {
    pub source: Range<usize>,
    pub latex: String,
    pub style: Style,
    pub block: bool,
}

#[derive(Default)]
struct Paragraph {
    formulas: Vec<usize>,
    other: bool,
    item: bool,
}

pub fn scan(text: &str) -> Vec<Formula> {
    let front = crate::markdown::body_start(text);
    // callout 首行属于容器标题，不把同段中的公式误判为混排正文。
    let mut offset = 0;
    let headers: Vec<_> = text
        .split_inclusive('\n')
        .filter_map(|line| {
            let start = offset;
            offset += line.len();
            let content = line.trim_start_matches([' ', '\t', '>']);
            (line.contains('>') && content.starts_with("[!") && content.contains(']'))
                .then_some(start..offset)
        })
        .collect();
    let mut result: Vec<Formula> = Vec::new();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    for (event, range) in Parser::new_ext(
        &text[front..],
        Options::ENABLE_MATH
            | Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_TASKLISTS,
    )
    .into_offset_iter()
    {
        match event {
            Event::Start(Tag::Paragraph | Tag::Item) => {
                let item = matches!(event, Event::Start(Tag::Item));
                paragraphs.push(Paragraph {
                    item,
                    ..Paragraph::default()
                });
            }
            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                if let Some(paragraph) = paragraphs.pop() {
                    if !paragraph.other {
                        for index in paragraph.formulas {
                            result[index].block = result[index].style == Style::Display;
                        }
                    }
                    if let Some(parent) = paragraphs.last_mut() {
                        parent.other = true;
                    }
                }
            }
            Event::InlineMath(ref latex) | Event::DisplayMath(ref latex) => {
                let style = if matches!(event, Event::DisplayMath(_)) {
                    Style::Display
                } else {
                    Style::Text
                };
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.formulas.push(result.len());
                    if style == Style::Text {
                        paragraph.other = true;
                    }
                }
                result.push(Formula {
                    source: range.start + front..range.end + front,
                    latex: latex.to_string(),
                    style,
                    block: false,
                });
            }
            Event::SoftBreak | Event::HardBreak | Event::TaskListMarker(_) => {}
            Event::Text(ref text) if text.trim().is_empty() => {}
            _ if headers.iter().any(|header| {
                header.start <= range.start + front && range.end + front <= header.end
            }) => {}
            Event::Start(Tag::List(_)) | Event::End(TagEnd::List(_))
                if paragraphs.last().is_some_and(|p| p.item) => {}
            _ => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.other = true;
                }
            }
        }
    }
    result
}
