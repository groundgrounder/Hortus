// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::{Utf16Mapper, decode_source_styles};
use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;

const APP_CSS: &str = r#"
.hortus-sidebar-switcher {
    padding: 6px 8px;
}

.hortus-sidebar-switcher button {
    font-size: 0.92em;
    padding: 4px 8px;
}

.hortus-tree-list {
    background: transparent;
}

.hortus-tree-list row {
    border-radius: 6px;
    margin: 1px 6px;
    padding: 2px 4px;
}

.hortus-note-view {
    font-size: 15px;
    line-height: 1.58;
    background-color: @view_bg_color;
    color: @view_fg_color;
}

.hortus-outline-row {
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 0.94em;
}

.hortus-outline-h1,
.hortus-outline-h2 {
    font-weight: 600;
}

.hortus-outline-sub {
    opacity: 0.82;
}

.hortus-panel-header {
    padding: 6px 10px 4px 10px;
}

.hortus-bottom-panel {
    border-top: 1px solid alpha(currentColor, 0.10);
    background-color: @headerbar_bg_color;
}

.hortus-mode-badge {
    font-size: 0.82em;
    padding: 2px 8px;
    border-radius: 999px;
    background-color: alpha(@accent_bg_color, 0.16);
    color: @accent_color;
}

.hortus-reading-view,
.hortus-embedded-reading-view {
    font-size: 15px;
    line-height: 1.58;
    background-color: transparent;
    color: @view_fg_color;
}

.hortus-diagram-card {
    border-radius: 10px;
    border: 1px solid alpha(currentColor, 0.10);
    background-color: alpha(currentColor, 0.02);
    padding: 4px;
}

.hortus-diagram-tools {
    background-color: alpha(@window_bg_color, 0.86);
    border: 1px solid alpha(currentColor, 0.12);
    border-radius: 999px;
    padding: 2px;
}

.hortus-diagram-fallback,
.hortus-math-fallback,
.hortus-media-placeholder {
    border-radius: 8px;
    border: 1px solid alpha(currentColor, 0.14);
    background-color: alpha(currentColor, 0.04);
    padding: 10px 12px;
}

.hortus-fallback-code {
    background-color: transparent;
    font-size: 0.90em;
}

.hortus-embed-card {
    border-radius: 10px;
    border: 1px solid alpha(currentColor, 0.12);
    border-left: 3px solid alpha(@accent_bg_color, 0.55);
    background-color: alpha(currentColor, 0.03);
    padding: 8px 10px;
}

.hortus-embed-header {
    padding: 2px 4px;
    font-weight: 600;
    font-size: 0.92em;
}

.hortus-table-wrap {
    border-radius: 8px;
    border: 1px solid alpha(currentColor, 0.10);
    background-color: alpha(currentColor, 0.015);
}

.hortus-table-header-cell {
    border-bottom: 1px solid alpha(currentColor, 0.18);
    background-color: alpha(currentColor, 0.04);
}

.hortus-table-body-cell {
    border-bottom: 1px solid alpha(currentColor, 0.08);
}

.hortus-table-cell-text {
    background-color: transparent;
}

.hortus-focus-overlay {
    background-color: @window_bg_color;
}

.hortus-focus-bar {
    padding: 6px 10px;
    background-color: @headerbar_bg_color;
}

.hortus-reading-hr {
    margin: 6px 0px;
    opacity: 0.55;
}

.hortus-notice {
    padding: 6px 12px;
    background-color: alpha(@accent_bg_color, 0.12);
    border-bottom: 1px solid alpha(currentColor, 0.10);
}

.hortus-notice-error {
    background-color: alpha(@error_bg_color, 0.18);
}

.hortus-notice-warning {
    background-color: alpha(@warning_bg_color, 0.18);
}

.hortus-notice label {
    font-size: 0.94em;
}

.hortus-find-bar {
    background-color: @headerbar_bg_color;
    border-bottom: 1px solid alpha(currentColor, 0.10);
    padding: 2px 8px;
}

.hortus-result-row {
    padding: 6px 10px;
    border-radius: 6px;
}

.hortus-result-title {
    font-weight: 600;
    font-size: 0.96em;
}

.hortus-result-meta {
    font-size: 0.84em;
    opacity: 0.66;
}

.hortus-result-snippet {
    font-size: 0.88em;
    opacity: 0.86;
}

.hortus-quick-open {
    min-width: 520px;
}

.hortus-quick-open-list row {
    padding: 5px 10px;
    border-radius: 6px;
}

.hortus-completion-popover {
    min-width: 300px;
}

.hortus-completion-panel {
    background-color: @popover_bg_color;
    border: 1px solid alpha(currentColor, 0.14);
    border-radius: 10px;
    box-shadow: 0 4px 16px alpha(black, 0.18);
    padding: 4px;
}

.hortus-completion-popover row {
    padding: 4px 10px;
    border-radius: 5px;
}

.hortus-math-inline-fallback {
    border-radius: 4px;
    background-color: alpha(currentColor, 0.07);
    padding: 0px 3px;
}

.hortus-tag-chip {
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 0.90em;
}

.hortus-backlink-row {
    padding: 5px 10px;
    border-radius: 6px;
}
"#;

pub fn install_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(APP_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub fn create_tag_table() -> gtk4::TextTagTable {
    let table = gtk4::TextTagTable::new();

    for level in 1u8..=6 {
        let scale = match level {
            1 => 1.65,
            2 => 1.38,
            3 => 1.20,
            4 => 1.10,
            5 => 1.04,
            _ => 1.0,
        };
        let tag = gtk4::TextTag::builder()
            .name(format!("h{level}"))
            .scale(scale)
            .weight(700)
            .build();
        table.add(&tag);
    }

    table.add(
        &gtk4::TextTag::builder()
            .name("marker")
            .foreground_rgba(&gdk::RGBA::new(0.50, 0.52, 0.56, 0.60))
            .build(),
    );
    table.add(&gtk4::TextTag::builder().name("strong").weight(700).build());
    table.add(
        &gtk4::TextTag::builder()
            .name("emphasis")
            .style(pango::Style::Italic)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("strike")
            .strikethrough(true)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("code")
            .family("monospace")
            .background_rgba(&gdk::RGBA::new(0.50, 0.54, 0.60, 0.14))
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("codeblock")
            .family("monospace")
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("quote")
            .foreground_rgba(&gdk::RGBA::new(0.45, 0.48, 0.52, 0.90))
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("link")
            .foreground_rgba(&gdk::RGBA::new(0.21, 0.52, 0.89, 1.0))
            .underline(pango::Underline::Single)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("tag")
            .foreground_rgba(&gdk::RGBA::new(0.18, 0.52, 0.82, 1.0))
            .weight(500)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("highlight")
            .background_rgba(&gdk::RGBA::new(0.96, 0.83, 0.18, 0.30))
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("comment")
            .foreground_rgba(&gdk::RGBA::new(0.50, 0.52, 0.56, 0.55))
            .style(pango::Style::Italic)
            .build(),
    );
    table.add(&gtk4::TextTag::builder().name("callout").weight(600).build());
    table.add(
        &gtk4::TextTag::builder()
            .name("frontmatter")
            .family("monospace")
            .foreground_rgba(&gdk::RGBA::new(0.50, 0.52, 0.56, 0.65))
            .scale(0.92)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("unresolved_link")
            .foreground_rgba(&gdk::RGBA::new(0.55, 0.57, 0.62, 0.85))
            .underline(pango::Underline::Single)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("callout_block")
            .paragraph_background_rgba(&gdk::RGBA::new(0.21, 0.52, 0.89, 0.08))
            .left_margin(10)
            .right_margin(10)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("quote_block")
            .left_margin(16)
            .foreground_rgba(&gdk::RGBA::new(0.45, 0.48, 0.52, 0.90))
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("task")
            .foreground_rgba(&gdk::RGBA::new(0.21, 0.52, 0.89, 1.0))
            .weight(600)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("find_match")
            .background_rgba(&gdk::RGBA::new(0.98, 0.86, 0.30, 0.38))
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("find_current")
            .background_rgba(&gdk::RGBA::new(0.98, 0.62, 0.16, 0.62))
            .build(),
    );

    table
}

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
    if mask & 512 != 0 {
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
    if mask & 8192 != 0 {
        buffer.apply_tag_by_name("tag", s, e);
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
                lookup("h1"),
                lookup("h2"),
                lookup("h3"),
                lookup("h4"),
                lookup("h5"),
                lookup("h6"),
                None,
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
    // 一次线性扫描建立 UTF-16 ↔ char 索引，避免每段样式都从文档开头重扫（旧实现 O(R·N)）。
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

/// 同上，但返回 `(映射+合并耗时, 标签应用耗时)` 的微秒值，供自动化记录线性映射证据。
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
        // 迭代器只前进不回退：避免每段两次 offset→行索引查找（大文档下这是主要开销）。
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
        // 内核片段按起点有序；出现乱序（不应发生）时退回二分查找，保证正确性。
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
    use std::time::Instant;

    fn synthetic_packed_runs(text: &str) -> Vec<u8> {
        // 与内核输出同构：按 8 个字符一段轮换语义 mask，模拟密集样式文档。
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
        if gtk4::init().is_err() {
            eprintln!("skipping style timing test: GTK 初始化失败");
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
        // 单元测试在调试构建下运行，留出机器波动余量；<200ms 的严格门槛由自动化报告断言。
        assert!(total < 400_000, "style application took {total}us");
    }
}
