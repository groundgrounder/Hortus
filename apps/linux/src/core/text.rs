// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleRun {
    pub start_utf16: u64,
    pub length_utf16: u64,
    pub mask: u32,
}

/// 将 `hortus_ffi::document::SourceStyles::packed_runs` 解码为样式片段列表。
/// 每条记录固定 20 字节小端序：`start:u64`、`length:u64`、`mask:u32`。
pub fn decode_source_styles(packed: &[u8]) -> Vec<StyleRun> {
    packed
        .chunks_exact(20)
        .map(|chunk| StyleRun {
            start_utf16: u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
            length_utf16: u64::from_le_bytes(chunk[8..16].try_into().unwrap()),
            mask: u32::from_le_bytes(chunk[16..20].try_into().unwrap()),
        })
        .collect()
}

/// 将源码 UTF-16 偏移转换为 `GtkTextBuffer` 使用的 Unicode 字符偏移。
pub fn utf16_offset_to_char_offset(text: &str, target_utf16: usize) -> usize {
    let mut utf16 = 0usize;
    let mut chars = 0usize;
    for ch in text.chars() {
        if utf16 >= target_utf16 {
            return chars;
        }
        utf16 += ch.len_utf16();
        chars += 1;
    }
    chars
}

/// 将源码 UTF-16 区间 `(start, length)` 转换为 `(char_start, char_end)`。
pub fn utf16_range_to_char_range(
    text: &str,
    start_utf16: usize,
    length_utf16: usize,
) -> (usize, usize) {
    let end_utf16 = start_utf16.saturating_add(length_utf16);
    let mut utf16 = 0usize;
    let mut chars = 0usize;
    let mut char_start = None;
    for ch in text.chars() {
        if char_start.is_none() && utf16 >= start_utf16 {
            char_start = Some(chars);
        }
        if utf16 >= end_utf16 {
            return (char_start.unwrap_or(chars), chars);
        }
        utf16 += ch.len_utf16();
        chars += 1;
    }
    (char_start.unwrap_or(chars), chars)
}

/// 从源码中提取给定 UTF-16 起点与长度的切片。
pub fn utf16_slice(text: &str, start_utf16: usize, length_utf16: usize) -> Option<String> {
    let mapper = Utf16Mapper::new(text);
    let b0 = mapper.utf16_to_byte(start_utf16);
    let b1 = mapper.utf16_to_byte(start_utf16.saturating_add(length_utf16));
    text.get(b0..b1).map(str::to_string)
}

/// 线性预扫描一次字符串，提供 O(log N) 精确 UTF-16 ↔ Unicode char ↔ UTF-8 byte 索引转换。
pub struct Utf16Mapper {
    utf16_offsets: Vec<usize>,
    byte_offsets: Vec<usize>,
}

impl Utf16Mapper {
    pub fn new(text: &str) -> Self {
        let mut utf16_offsets = Vec::with_capacity(text.chars().count() + 1);
        let mut byte_offsets = Vec::with_capacity(utf16_offsets.capacity());
        let mut u16 = 0usize;
        for (b, ch) in text.char_indices() {
            utf16_offsets.push(u16);
            byte_offsets.push(b);
            u16 += ch.len_utf16();
        }
        utf16_offsets.push(u16);
        byte_offsets.push(text.len());
        Self {
            utf16_offsets,
            byte_offsets,
        }
    }

    pub fn utf16_to_char(&self, target_utf16: usize) -> usize {
        self.utf16_offsets
            .partition_point(|&u| u < target_utf16)
            .min(self.utf16_offsets.len().saturating_sub(1))
    }

    pub fn utf16_to_byte(&self, target_utf16: usize) -> usize {
        let idx = self.utf16_to_char(target_utf16);
        self.byte_offsets[idx]
    }

    pub fn char_len(&self) -> usize {
        self.utf16_offsets.len().saturating_sub(1)
    }

    pub fn utf16_at(&self, char_index: usize) -> usize {
        self.utf16_offsets
            .get(char_index)
            .copied()
            .unwrap_or_else(|| *self.utf16_offsets.last().unwrap_or(&0))
    }

    pub fn char_to_utf16(&self, char_index: usize) -> usize {
        self.utf16_offsets
            .get(char_index)
            .copied()
            .unwrap_or_else(|| *self.utf16_offsets.last().unwrap_or(&0))
    }

    pub fn char_to_byte(&self, char_index: usize) -> usize {
        self.byte_offsets
            .get(char_index)
            .copied()
            .unwrap_or_else(|| *self.byte_offsets.last().unwrap_or(&0))
    }

    pub fn byte_to_utf16(&self, target_byte: usize) -> usize {
        let idx = self
            .byte_offsets
            .partition_point(|&b| b < target_byte)
            .min(self.utf16_offsets.len().saturating_sub(1));
        self.utf16_offsets[idx]
    }
}

/// 按当前行推断回车续写前缀：无序列表、有序列表与任务项（含缩进与已完成状态）。
/// 空列表项返回 `None`，让回车结束列表而不是无限续写。
pub fn line_list_continuation(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    let (marker, tail) = if let Some(tail) = rest.strip_prefix("- [ ] ") {
        ("- [ ] ", tail)
    } else if let Some(tail) = rest
        .strip_prefix("- [x] ")
        .or_else(|| rest.strip_prefix("- [X] "))
    {
        ("- [ ] ", tail)
    } else if let Some(tail) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        ("- ", tail)
    } else {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        let after = &rest[digits..];
        if digits > 0
            && let Some(tail) = after.strip_prefix(". ")
        {
            let next = rest[..digits].parse::<u64>().ok()?.saturating_add(1);
            let prefix = format!("{next}. ");
            if tail.trim().is_empty() {
                return None;
            }
            return Some(format!("{indent}{prefix}"));
        }
        if let Some(tail) = rest.strip_prefix("> ") {
            ("> ", tail)
        } else {
            return None;
        }
    };
    if tail.trim().is_empty() {
        return None;
    }
    Some(format!("{indent}{marker}"))
}

/// 在包含 `caret_char`（GtkTextBuffer 字符偏移）的行首定位任务勾选框并翻转。
/// 直接按切片扫描，避免整篇文档构造 `Vec<char>`。
pub fn task_toggle_at(text: &str, caret_char: usize) -> Option<(usize, usize, String)> {
    let mut line_start_char = 0usize;
    let mut line_start_byte = 0usize;
    let mut line_end_byte = text.len();
    let mut seen_caret = false;

    for (char_idx, (byte_idx, ch)) in text.char_indices().enumerate() {
        if char_idx == caret_char {
            seen_caret = true;
        }
        if ch == '\n' {
            if seen_caret {
                line_end_byte = byte_idx;
                break;
            }
            line_start_char = char_idx + 1;
            line_start_byte = byte_idx + 1;
        }
    }

    let line = &text[line_start_byte..line_end_byte];
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    // 仅识别行首标记，避免误改正文中的字面量。
    let open = ["- [ ] ", "- [x] ", "- [X] "]
        .into_iter()
        .find(|prefix| rest.starts_with(prefix))?;
    let start = line_start_char + line[..indent].chars().count();
    let end = start + open.chars().count();
    let replacement = if open == "- [ ] " { "- [x] " } else { "- [ ] " };
    Some((start, end, replacement.to_string()))
}

/// 在文本中按 ASCII 大小写不敏感查找全部匹配，返回字符偏移区间（零整篇 `Vec<char>` 分配）。
pub fn find_char_ranges(haystack: &str, needle: &str, limit: usize) -> Vec<(usize, usize)> {
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let needle_first = needle.chars().next().unwrap().to_ascii_lowercase();
    let needle_count = needle.chars().count();
    let mut found = Vec::new();
    let mut skip_until_char = 0usize;

    for (char_idx, (byte_idx, ch)) in haystack.char_indices().enumerate() {
        if char_idx >= skip_until_char && ch.to_ascii_lowercase() == needle_first {
            let tail = &haystack[byte_idx..];
            let mut matched_chars = 0usize;
            let mut matched = true;
            for (a, b) in tail.chars().zip(needle.chars()) {
                if !a.eq_ignore_ascii_case(&b) {
                    matched = false;
                    break;
                }
                matched_chars += 1;
            }
            if matched && matched_chars == needle_count {
                let end_char = char_idx + needle_count;
                found.push((char_idx, end_char));
                if found.len() >= limit {
                    break;
                }
                skip_until_char = end_char;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_continuation_and_task_toggle_follow_markdown_prefixes() {
        assert_eq!(line_list_continuation("  - 已有内容"), Some("  - ".into()));
        assert_eq!(line_list_continuation("1. 第一项"), Some("2. ".into()));
        assert_eq!(
            line_list_continuation("- [x] 完成项"),
            Some("- [ ] ".into())
        );
        assert_eq!(line_list_continuation("-   "), None);
        assert_eq!(line_list_continuation("普通段落"), None);

        let text = "# 标题\n- [ ] 待办\n- [x] 已完成\n";
        assert_eq!(task_toggle_at(text, 8), Some((5, 11, "- [x] ".into())));
        let second = text.find("已完成").unwrap();
        assert_eq!(
            task_toggle_at(text, text[..second].chars().count()),
            Some((14, 20, "- [ ] ".into()))
        );
        assert_eq!(task_toggle_at("没有任何任务", 2), None);
    }

    #[test]
    fn find_char_ranges_is_ascii_case_insensitive_across_cjk() {
        let text = "Hortus 中文 Search 搜索 search";
        let ranges = find_char_ranges(text, "search", 10);
        assert_eq!(ranges.len(), 2);
        let first: String = text
            .chars()
            .skip(ranges[0].0)
            .take(ranges[0].1 - ranges[0].0)
            .collect();
        assert_eq!(first, "Search");
        assert!(find_char_ranges("abc", "", 10).is_empty());
        assert_eq!(find_char_ranges("aaa", "aa", 10), vec![(0, 2)]);
    }

    #[test]
    fn utf16_offsets_map_accurately_across_cjk_and_surrogate_pairs() {
        let text = "# 标题😀\n正文";
        assert_eq!(utf16_offset_to_char_offset(text, 0), 0);
        assert_eq!(utf16_offset_to_char_offset(text, 4), 4);
        assert_eq!(utf16_offset_to_char_offset(text, 6), 5);
        assert_eq!(utf16_offset_to_char_offset(text, 7), 6);
        assert_eq!(utf16_range_to_char_range(text, 2, 4), (2, 5));
    }

    #[test]
    fn decode_source_styles_unpacks_little_endian_runs() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4u64.to_le_bytes());
        bytes.extend_from_slice(&9u64.to_le_bytes());
        bytes.extend_from_slice(&(2u32 << 24 | 1u32).to_le_bytes());
        let runs = decode_source_styles(&bytes);
        assert_eq!(
            runs,
            vec![StyleRun {
                start_utf16: 4,
                length_utf16: 9,
                mask: (2u32 << 24) | 1,
            }]
        );
    }
}
