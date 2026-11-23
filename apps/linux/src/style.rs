// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::{decode_source_styles, utf16_range_to_char_range};
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

pub fn apply_source_styles(buffer: &gtk4::TextBuffer, text: &str, packed_runs: &[u8]) {
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.remove_all_tags(&start, &end);

    for run in decode_source_styles(packed_runs) {
        if run.length_utf16 == 0 || run.mask == 0 {
            continue;
        }
        let (char_start, char_end) =
            utf16_range_to_char_range(text, run.start_utf16 as usize, run.length_utf16 as usize);
        if char_start >= char_end {
            continue;
        }
        let s = buffer.iter_at_offset(char_start as i32);
        let e = buffer.iter_at_offset(char_end as i32);

        let heading = (run.mask >> 24) & 0xff;
        if (1..=6).contains(&heading) {
            buffer.apply_tag_by_name(&format!("h{heading}"), &s, &e);
        }
        if run.mask & 1 != 0 {
            buffer.apply_tag_by_name("marker", &s, &e);
        }
        if run.mask & 2 != 0 {
            buffer.apply_tag_by_name("strong", &s, &e);
        }
        if run.mask & 4 != 0 {
            buffer.apply_tag_by_name("emphasis", &s, &e);
        }
        if run.mask & 8 != 0 {
            buffer.apply_tag_by_name("strike", &s, &e);
        }
        if run.mask & 16 != 0 {
            buffer.apply_tag_by_name("code", &s, &e);
        }
        if run.mask & (32 | 65536) != 0 {
            buffer.apply_tag_by_name("codeblock", &s, &e);
        }
        if run.mask & 64 != 0 {
            buffer.apply_tag_by_name("quote", &s, &e);
        }
        if run.mask & 128 != 0 {
            buffer.apply_tag_by_name("link", &s, &e);
        }
        if run.mask & 512 != 0 {
            buffer.apply_tag_by_name("tag", &s, &e);
        }
        if run.mask & 1024 != 0 {
            buffer.apply_tag_by_name("highlight", &s, &e);
        }
        if run.mask & 2048 != 0 {
            buffer.apply_tag_by_name("comment", &s, &e);
        }
        if run.mask & 4096 != 0 {
            buffer.apply_tag_by_name("callout", &s, &e);
        }
        if run.mask & 16384 != 0 {
            buffer.apply_tag_by_name("frontmatter", &s, &e);
        }
    }
}
