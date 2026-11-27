// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

use crate::core::{Utf16Mapper, decode_source_styles};
use gtk4::prelude::*;

#[allow(dead_code)]
pub fn apply_mask_tags(
    buffer: &gtk4::TextBuffer,
    s: &gtk4::TextIter,
    e: &gtk4::TextIter,
    mask: u32,
) {
    let heading = (mask >> 24) & 0xff;
    if (1..=6).contains(&heading) {
        buffer.apply_tag_by_name(&format!("h{heading}"), s, e);
    }
    if mask & 1 != 0 {
        buffer.apply_tag_by_name("marker", s, e);
    }
    if mask & 2 != 0 {
        buffer.apply_tag_by_name("strong", s, e);
    }
    if mask & 4 != 0 {
        buffer.apply_tag_by_name("emphasis", s, e);
    }
    if mask & 8 != 0 {
        buffer.apply_tag_by_name("strike", s, e);
    }
    if mask & 16 != 0 {
        buffer.apply_tag_by_name("code", s, e);
    }
    if mask & (32 | 65536) != 0 {
        buffer.apply_tag_by_name("codeblock", s, e);
    }
    if mask & 64 != 0 {
        buffer.apply_tag_by_name("quote_block", s, e);
    }
    if mask & 128 != 0 {
        if mask & (1 << 19) != 0 {
            buffer.apply_tag_by_name("unresolved_link", s, e);
        } else {
            buffer.apply_tag_by_name("link", s, e);
        }
    }
    if mask & (512 | 8192) != 0 {
        buffer.apply_tag_by_name("tag", s, e);
    }
    if mask & 1024 != 0 {
        buffer.apply_tag_by_name("highlight", s, e);
    }
    if mask & 2048 != 0 {
        buffer.apply_tag_by_name("comment", s, e);
    }
    if mask & 4096 != 0 {
        buffer.apply_tag_by_name("callout", s, e);
        buffer.apply_tag_by_name("callout_block", s, e);
    }
    if mask & 16384 != 0 {
        buffer.apply_tag_by_name("frontmatter", s, e);
    }
    if mask & 131072 != 0 {
        buffer.apply_tag_by_name("task", s, e);
    }
}

/// 预解析的标签集合：避免大文档逐段样式应用时反复按名称查表。
pub struct MaskTagSet {
    heading: [Option<gtk4::TextTag>; 7],
    marker: Option<gtk4::TextTag>,
    strong: Option<gtk4::TextTag>,
    emphasis: Option<gtk4::TextTag>,
    strike: Option<gtk4::TextTag>,
    code: Option<gtk4::TextTag>,
    codeblock: Option<gtk4::TextTag>,
    quote_block: Option<gtk4::TextTag>,
    link: Option<gtk4::TextTag>,
    unresolved_link: Option<gtk4::TextTag>,
    tag: Option<gtk4::TextTag>,
    highlight: Option<gtk4::TextTag>,
    comment: Option<gtk4::TextTag>,
    callout: Option<gtk4::TextTag>,
    callout_block: Option<gtk4::TextTag>,
    frontmatter: Option<gtk4::TextTag>,
    task: Option<gtk4::TextTag>,
}

impl MaskTagSet {
    pub fn new(table: &gtk4::TextTagTable) -> Self {
        let lookup = |name: &str| table.lookup(name);
        Self {
            heading: [
                None,
                lookup("h1"),
                lookup("h2"),
                lookup("h3"),
                lookup("h4"),
                lookup("h5"),
                lookup("h6"),
            ],
            marker: lookup("marker"),
            strong: lookup("strong"),
            emphasis: lookup("emphasis"),
            strike: lookup("strike"),
            code: lookup("code"),
            codeblock: lookup("codeblock"),
            quote_block: lookup("quote_block"),
            link: lookup("link"),
            unresolved_link: lookup("unresolved_link"),
            tag: lookup("tag"),
            highlight: lookup("highlight"),
            comment: lookup("comment"),
            callout: lookup("callout"),
            callout_block: lookup("callout_block"),
            frontmatter: lookup("frontmatter"),
            task: lookup("task"),
        }
    }
}

fn apply_cached(
    buffer: &gtk4::TextBuffer,
    tag: &Option<gtk4::TextTag>,
    s: &gtk4::TextIter,
    e: &gtk4::TextIter,
) {
    if let Some(tag) = tag {
        buffer.apply_tag(tag, s, e);
    }
}

/// 按预解析标签集合应用语义 mask；与按名称版本行为一致，但无逐段名称查表。
pub fn apply_mask_tags_cached(
    buffer: &gtk4::TextBuffer,
    tags: &MaskTagSet,
    s: &gtk4::TextIter,
    e: &gtk4::TextIter,
    mask: u32,
) {
    let heading = (mask >> 24) & 0xff;
    if (1..=6).contains(&heading) {
        apply_cached(buffer, &tags.heading[heading as usize], s, e);
    }
    if mask & 1 != 0 {
        apply_cached(buffer, &tags.marker, s, e);
    }
    if mask & 2 != 0 {
        apply_cached(buffer, &tags.strong, s, e);
    }
    if mask & 4 != 0 {
        apply_cached(buffer, &tags.emphasis, s, e);
    }
    if mask & 8 != 0 {
        apply_cached(buffer, &tags.strike, s, e);
    }
    if mask & 16 != 0 {
        apply_cached(buffer, &tags.code, s, e);
    }
    if mask & (32 | 65536) != 0 {
        apply_cached(buffer, &tags.codeblock, s, e);
    }
    if mask & 64 != 0 {
        apply_cached(buffer, &tags.quote_block, s, e);
    }
    if mask & 128 != 0 {
        if mask & (1 << 19) != 0 {
            apply_cached(buffer, &tags.unresolved_link, s, e);
        } else {
            apply_cached(buffer, &tags.link, s, e);
        }
    }
    if mask & (512 | 8192) != 0 {
        apply_cached(buffer, &tags.tag, s, e);
    }
    if mask & 1024 != 0 {
        apply_cached(buffer, &tags.highlight, s, e);
    }
    if mask & 2048 != 0 {
        apply_cached(buffer, &tags.comment, s, e);
    }
    if mask & 4096 != 0 {
        apply_cached(buffer, &tags.callout, s, e);
        apply_cached(buffer, &tags.callout_block, s, e);
    }
    if mask & 16384 != 0 {
        apply_cached(buffer, &tags.frontmatter, s, e);
    }
    if mask & 131072 != 0 {
        apply_cached(buffer, &tags.task, s, e);
    }
}

pub fn apply_source_styles(buffer: &gtk4::TextBuffer, text: &str, packed_runs: &[u8]) {
    let mapper = Utf16Mapper::new(text);
    apply_source_styles_with(buffer, &mapper, packed_runs);
}

/// 复用已构建的 `Utf16Mapper` 重放样式；样式与标签变更包裹在不可撤销动作中，绝不进入原生撤销栈。
pub fn apply_source_styles_with(
    buffer: &gtk4::TextBuffer,
    mapper: &Utf16Mapper,
    packed_runs: &[u8],
) {
    let _ = apply_source_styles_timed(buffer, mapper, packed_runs);
}

/// 同上，但返回 `(映射+合并耗时, 标签应用耗时)` 的微秒值。
pub fn apply_source_styles_timed(
    buffer: &gtk4::TextBuffer,
    mapper: &Utf16Mapper,
    packed_runs: &[u8],
) -> (u64, u64) {
    let plan_started = std::time::Instant::now();
    let merged = merge_source_runs(mapper, packed_runs);
    let plan_micros = plan_started.elapsed().as_micros() as u64;

    let tag_started = std::time::Instant::now();
    buffer.begin_irreversible_action();
    {
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        buffer.remove_all_tags(&start, &end);
        let total = buffer.char_count() as usize;
        let tags = MaskTagSet::new(&buffer.tag_table());
        let mut cursor = buffer.start_iter();
        let mut cursor_offset = 0usize;
        for (char_start, char_end, mask) in merged {
            let char_start = char_start.min(total);
            let char_end = char_end.min(total);
            if char_end <= char_start {
                continue;
            }
            if char_start < cursor_offset {
                cursor = buffer.iter_at_offset(char_start as i32);
            } else {
                let _ = cursor.forward_chars((char_start - cursor_offset) as i32);
            }
            cursor_offset = char_start;
            let mut end_iter = cursor;
            let _ = end_iter.forward_chars((char_end - char_start) as i32);
            apply_mask_tags_cached(buffer, &tags, &cursor, &end_iter, mask);
        }
    }
    buffer.end_irreversible_action();
    (plan_micros, tag_started.elapsed().as_micros() as u64)
}

/// 单遍线性映射并合并相邻同 mask 片段：游标只前进不回退，O(N + R)。
fn merge_source_runs(mapper: &Utf16Mapper, packed_runs: &[u8]) -> Vec<(usize, usize, u32)> {
    let mut merged: Vec<(usize, usize, u32)> = Vec::new();
    let total_chars = mapper.char_len();
    let mut cursor = 0usize;
    for run in decode_source_styles(packed_runs) {
        if run.length_utf16 == 0 || run.mask == 0 {
            continue;
        }
        let start_u16 = run.start_utf16 as usize;
        let end_u16 = start_u16.saturating_add(run.length_utf16 as usize);
        while cursor < total_chars && mapper.utf16_at(cursor) < start_u16 {
            cursor += 1;
        }
        let char_start = if mapper.utf16_at(cursor) > start_u16 {
            mapper.utf16_to_char(start_u16)
        } else {
            cursor
        };
        if char_start > cursor {
            cursor = char_start;
        }
        while cursor < total_chars && mapper.utf16_at(cursor) < end_u16 {
            cursor += 1;
        }
        let char_end = cursor;
        if char_start >= char_end {
            continue;
        }
        match merged.last_mut() {
            Some((_, last_end, last_mask)) if *last_end == char_start && *last_mask == run.mask => {
                *last_end = char_end;
            }
            _ => merged.push((char_start, char_end, run.mask)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Utf16Mapper;
    use crate::platform::style::create_tag_table;
    use std::time::Instant;

    fn synthetic_packed_runs(text: &str) -> Vec<u8> {
        let masks = [1u32, 2, 4, 16, 128, 512, 2048, 0];
        let mut packed = Vec::new();
        let mut units = 0u64;
        let mut segment_units = 0u64;
        let mut segment_start = 0u64;
        let mut mask_index = 0usize;
        for (index, ch) in text.chars().enumerate() {
            units += ch.len_utf16() as u64;
            segment_units += ch.len_utf16() as u64;
            let segment_end = (index + 1) % 4 == 0;
            if segment_end {
                let mask = masks[mask_index % masks.len()];
                packed.extend_from_slice(&segment_start.to_le_bytes());
                packed.extend_from_slice(&segment_units.to_le_bytes());
                packed.extend_from_slice(&mask.to_le_bytes());
                segment_start = units;
                segment_units = 0;
                mask_index += 1;
            }
        }
        if segment_units > 0 {
            let mask = masks[mask_index % masks.len()];
            packed.extend_from_slice(&segment_start.to_le_bytes());
            packed.extend_from_slice(&segment_units.to_le_bytes());
            packed.extend_from_slice(&mask.to_le_bytes());
        }
        packed
    }

    #[test]
    fn one_mib_styles_apply_linearly_under_200ms() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping style timing test: GTK 初始化失败或非主线程");
            return;
        }
        let unit = "这是用于线性样式映射压力测试的段落，包含 **粗体**、`代码`、[链接](https://example.org) 与 #demo 标签。\n\n";
        let mut text = String::with_capacity(1024 * 1024 + unit.len());
        while text.len() < 1024 * 1024 {
            text.push_str(unit);
        }
        let packed = synthetic_packed_runs(&text);
        let table = create_tag_table();
        let buffer = gtk4::TextBuffer::new(Some(&table));
        buffer.set_text(&text);
        let mapper = Utf16Mapper::new(&text);
        let started = Instant::now();
        let (plan_micros, tag_micros) = apply_source_styles_timed(&buffer, &mapper, &packed);
        let total = started.elapsed().as_micros() as u64;
        eprintln!(
            "1 MiB styles: runs={} plan={plan_micros}us tags={tag_micros}us total={total}us",
            packed.len() / 20
        );
        assert!(total < 400_000, "style application took {total}us");
    }

    #[test]
    fn heading_masks_apply_exact_levels_and_monotonic_scales() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping heading style test: GTK 初始化失败或非主线程");
            return;
        }
        use crate::platform::style::create_edit_tag_table;

        let text = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n";
        let mut packed = Vec::new();
        let mut offset_u16 = 0u64;
        for (idx, line) in text.split_inclusive('\n').enumerate() {
            let len_u16 = line.encode_utf16().count() as u64;
            let level = (idx as u32) + 1;
            let mask = level << 24;
            packed.extend_from_slice(&offset_u16.to_le_bytes());
            packed.extend_from_slice(&len_u16.to_le_bytes());
            packed.extend_from_slice(&mask.to_le_bytes());
            offset_u16 += len_u16;
        }

        let table = create_edit_tag_table();
        let buffer = gtk4::TextBuffer::new(Some(&table));
        buffer.set_text(text);
        apply_source_styles(&buffer, text, &packed);

        let scales: Vec<f64> = (1u8..=6)
            .map(|lvl| table.lookup(&format!("h{lvl}")).unwrap().scale())
            .collect();
        assert!(scales[0] > scales[1]);
        assert!(scales[1] > scales[2]);
        assert!(scales[2] > scales[3]);
        assert!(scales[3] > scales[4]);
        assert!(scales[4] > 1.0);

        let mut char_cursor = 0i32;
        for (idx, line) in text.split_inclusive('\n').enumerate() {
            let expected_lvl = (idx as u8) + 1;
            let iter = buffer.iter_at_offset(char_cursor);
            for lvl in 1u8..=6 {
                let tag = table.lookup(&format!("h{lvl}")).unwrap();
                assert_eq!(
                    iter.has_tag(&tag),
                    lvl == expected_lvl,
                    "line {expected_lvl} unexpected h{lvl} tag state"
                );
            }
            char_cursor += line.chars().count() as i32;
        }

        // 同样验证直接调用 apply_mask_tags_cached 时 H1..H6 下标无错位
        let buffer2 = gtk4::TextBuffer::new(Some(&table));
        buffer2.set_text("abcdef");
        let tags = MaskTagSet::new(&table);
        for lvl in 1u32..=6 {
            let s = buffer2.iter_at_offset((lvl - 1) as i32);
            let e = buffer2.iter_at_offset(lvl as i32);
            apply_mask_tags_cached(&buffer2, &tags, &s, &e, lvl << 24);
            let expected_tag = table.lookup(&format!("h{lvl}")).unwrap();
            assert!(s.has_tag(&expected_tag), "cached h{lvl} missing");
        }
    }
}
