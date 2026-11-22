//! 嵌入只使用阅读视图同一套标题语义：章节边界按真实标题层级截断。
use crate::{markdown, outline};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingSection {
    /// 目标标题所在源正文的字节范围（不含前置 frontmatter）。
    pub range: Range<usize>,
    pub level: u8,
}

/// 目标标题的比较口径：去首尾空白、去可选前导 `#`、ASCII 大小写不敏感。
fn heading_key(text: &str) -> String {
    text.trim().trim_start_matches('#').trim().to_lowercase()
}

/// 把标题的 UTF-16 起点换算成字节偏移；偏移越界时收敛到文末。
fn byte_offset(text: &str, units: usize) -> usize {
    let mut utf16 = 0;
    for (byte, character) in text.char_indices() {
        if utf16 >= units {
            return byte;
        }
        utf16 += character.len_utf16();
    }
    text.len()
}

/// 去除 YAML frontmatter 后的正文起点。
pub fn body_start(text: &str) -> usize {
    markdown::body_start(text)
}

/// 去除 YAML frontmatter 后的正文切片。
pub fn body(text: &str) -> &str {
    &text[body_start(text)..]
}

/// 从标题行起截到下一个同级或更高级标题之前；标题不存在时返回 `None`。
///
/// 标题来自与阅读视图相同的解析语义，代码块、注释里的 `#` 不会命中。
pub fn heading_section(text: &str, heading: &str) -> Option<HeadingSection> {
    let target = heading_key(heading);
    if target.is_empty() {
        return None;
    }
    let headings = outline::parse(text);
    let index = headings
        .iter()
        .position(|heading| heading_key(&heading.title) == target)?;
    let start = byte_offset(text, headings[index].start);
    let end = headings
        .iter()
        .skip(index + 1)
        .find(|next| next.level <= headings[index].level)
        .map_or(text.len(), |next| byte_offset(text, next.start));
    let trimmed = text[start..end].trim_end();
    Some(HeadingSection {
        range: start..start + trimmed.len(),
        level: headings[index].level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = concat!(
        "---\n",
        "title: ignored\n",
        "# Not a heading\n",
        "---\n",
        "# Title\n\n",
        "Intro text.\n\n",
        "## Section One\n\n",
        "One body.\n\n",
        "```text\n# fake heading\n```\n\n",
        "### Nested\n\n",
        "Nested body.\n\n",
        "%%\n## Comment heading\n%%\n\n",
        "## Section Two\n\n",
        "Two body.\n\n",
        "Setext Target\n",
        "-------------\n\n",
        "Setext body.\n\n",
        "# Later\n",
        "Later body.\n",
    );

    #[test]
    fn body_skips_frontmatter_only() {
        assert!(body(NOTE).starts_with("# Title"));
        assert!(!body(NOTE).contains("title: ignored"));
    }

    #[test]
    fn section_stops_at_same_or_higher_heading() {
        let section = heading_section(NOTE, "  ## section one  ").unwrap();
        let text = &NOTE[section.range.clone()];
        assert!(text.starts_with("## Section One"));
        assert!(text.contains("### Nested"));
        assert!(text.contains("Nested body."));
        assert!(!text.contains("## Section Two"));
    }

    #[test]
    fn section_for_top_level_stops_at_next_level_one() {
        let section = heading_section(NOTE, "# Title").unwrap();
        let text = &NOTE[section.range.clone()];
        assert!(text.starts_with("# Title"));
        assert!(text.contains("## Section Two"));
        assert!(!text.contains("# Later"));
    }

    #[test]
    fn setext_and_missing_headings_are_handled() {
        let section = heading_section(NOTE, "Setext Target").unwrap();
        assert_eq!(
            &NOTE[section.range],
            "Setext Target\n-------------\n\nSetext body."
        );
        assert!(heading_section(NOTE, "No Such Heading").is_none());
        assert!(heading_section(NOTE, "Not a heading").is_none());
        assert!(heading_section(NOTE, "Comment heading").is_none());
    }

    #[test]
    fn heading_level_is_reported() {
        assert_eq!(heading_section(NOTE, "Nested").unwrap().level, 3);
        assert_eq!(heading_section(NOTE, "Section Two").unwrap().level, 2);
    }
}
