//! 美元符边界完全由 pulldown 决定；阅读层将非空公式投影为原子元素。
use pulldown_cmark::{Event, Options, Parser};
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

pub fn scan(text: &str) -> Vec<Formula> {
    let front = crate::markdown::body_start(text);
    Parser::new_ext(
        &text[front..],
        Options::ENABLE_MATH
            | Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_TASKLISTS,
    )
    .into_offset_iter()
    .filter_map(|(event, range)| {
        let (latex, style) = match event {
            Event::InlineMath(latex) => (latex, Style::Text),
            Event::DisplayMath(latex) => (latex, Style::Display),
            _ => return None,
        };
        Some(Formula {
            source: range.start + front..range.end + front,
            latex: latex.into_string(),
            style,
            block: style == Style::Display,
        })
    })
    .collect()
}
