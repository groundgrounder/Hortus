// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

use crate::services::config_service::{FONT_SIZE_MAX, FONT_SIZE_MIN};
use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::sync::Once;

#[cfg(feature = "automation")]
pub use crate::platform::source_styles::apply_source_styles_timed;
pub use crate::platform::source_styles::{
    MaskTagSet, apply_mask_tags_cached, apply_source_styles, apply_source_styles_with,
};

const BASE_CSS: &str = r#"
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
    line-height: 1.28;
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

.hortus-outline-btn {
    border-radius: 6px;
}

.hortus-outline-btn.hortus-outline-active {
    background-color: alpha(@accent_bg_color, 0.14);
    color: @accent_color;
    font-weight: 600;
}

.hortus-panel-header {
    padding: 6px 10px 4px 10px;
}

.hortus-bottom-panel {
    border-top: 1px solid alpha(currentColor, 0.10);
    background-color: @headerbar_bg_color;
}

.hortus-sidebar-resize-handle {
    background-color: transparent;
}

.hortus-sidebar-resize-line {
    min-width: 2px;
    background-color: transparent;
    transition: background-color 120ms ease;
}

.hortus-sidebar-resize-handle:hover .hortus-sidebar-resize-line,
.hortus-sidebar-resize-handle.dragging .hortus-sidebar-resize-line {
    background-color: alpha(@accent_bg_color, 0.55);
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
    line-height: 1.28;
    background-color: transparent;
    color: @view_fg_color;
}

.hortus-reading-box {
    padding: 0px;
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

.hortus-table-header {
    font-weight: 700;
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

.hortus-tag-chip.hortus-tag-chip-selected {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}

.hortus-tag-chip.hortus-tag-chip-selected:hover,
.hortus-tag-chip.hortus-tag-chip-selected:active {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}

.hortus-backlink-row {
    padding: 5px 10px;
    border-radius: 6px;
}

.hortus-tool-strip {
    min-height: 32px;
    padding: 2px 6px;
    background-color: transparent;
}

.hortus-tool-tab {
    min-width: 0;
    min-height: 26px;
    padding: 2px 8px;
    border-radius: 6px;
    transition: background-color 120ms ease;
}

.hortus-tool-tab.hortus-tool-tab-sidebar {
    min-width: 0;
    padding: 2px 8px;
}

.hortus-tool-tab.hortus-tool-tab-bottom {
    padding: 2px 10px;
}

.hortus-tool-tab:hover {
    background-color: alpha(currentColor, 0.06);
}

.hortus-tool-tab.selected {
    background-color: alpha(currentColor, 0.11);
    font-weight: 600;
}

.hortus-tool-action-btn,
.hortus-tool-action-btn > button,
.hortus-tool-action-btn button {
    min-width: 24px;
    min-height: 24px;
    padding: 2px;
    margin: 0;
}

.hortus-tool-empty {
    padding: 16px;
}

.hortus-terminal-status {
    padding: 4px 10px;
    background-color: alpha(@warning_bg_color, 0.16);
    border-bottom: 1px solid alpha(currentColor, 0.10);
}

.hortus-diff-hunk-header {
    padding: 4px 8px;
    border-radius: 6px;
    background-color: alpha(@accent_bg_color, 0.10);
    font-family: monospace;
    font-size: 0.88em;
}

.hortus-diff-add {
    background-color: alpha(@success_color, 0.16);
    color: @view_fg_color;
    font-family: monospace;
    font-size: 0.88em;
}

.hortus-diff-del {
    background-color: alpha(@error_color, 0.16);
    color: @view_fg_color;
    font-family: monospace;
    font-size: 0.88em;
}

.hortus-diff-ctx {
    font-family: monospace;
    font-size: 0.88em;
    opacity: 0.85;
}

.hortus-agent-badge-ok {
    padding: 2px 8px;
    border-radius: 999px;
    background-color: alpha(@success_color, 0.16);
    color: @success_color;
    font-size: 0.84em;
}

.hortus-agent-badge-err {
    padding: 2px 8px;
    border-radius: 999px;
    background-color: alpha(@error_color, 0.16);
    color: @error_color;
    font-size: 0.84em;
}
"#;

static CSS_INSTALL_ONCE: Once = Once::new();

thread_local! {
    static DYNAMIC_FONT_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

fn format_font_css(font_size_px: f64) -> String {
    let px = font_size_px.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
    format!(
        ".hortus-note-view, .hortus-reading-view, .hortus-embedded-reading-view {{ font-size: {px:.1}px; }}\n"
    )
}

/// 全局幂等安装基础 CSS 与可动态更新字号的 CSS Provider（G-08 修复）。
pub fn install_css() {
    CSS_INSTALL_ONCE.call_once(|| {
        let base_provider = gtk4::CssProvider::new();
        base_provider.load_from_string(BASE_CSS);
        let font_provider = gtk4::CssProvider::new();
        font_provider.load_from_string(&format_font_css(15.0));

        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &base_provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            gtk4::style_context_add_provider_for_display(
                &display,
                &font_provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
        DYNAMIC_FONT_PROVIDER.with(|slot| {
            *slot.borrow_mut() = Some(font_provider);
        });
    });
}

/// 实时刷新动态字号 `CssProvider`，不重复挂载全局样式表。
pub fn update_editor_font_size_css(font_size_px: f64) {
    install_css();
    DYNAMIC_FONT_PROVIDER.with(|slot| {
        if let Some(provider) = slot.borrow().as_ref() {
            provider.load_from_string(&format_font_css(font_size_px));
        }
    });
}

struct TagThemePalette {
    marker: gdk::RGBA,
    code_bg: gdk::RGBA,
    quote: gdk::RGBA,
    link: gdk::RGBA,
    tag: gdk::RGBA,
    highlight_bg: gdk::RGBA,
    comment: gdk::RGBA,
    frontmatter: gdk::RGBA,
    unresolved_link: gdk::RGBA,
    callout_bg: gdk::RGBA,
    task: gdk::RGBA,
    find_match_bg: gdk::RGBA,
    find_current_bg: gdk::RGBA,
}

fn palette_for_theme(is_dark: bool) -> TagThemePalette {
    if is_dark {
        TagThemePalette {
            marker: gdk::RGBA::new(0.66, 0.69, 0.75, 0.78),
            code_bg: gdk::RGBA::new(0.70, 0.75, 0.82, 0.18),
            quote: gdk::RGBA::new(0.72, 0.76, 0.82, 0.94),
            link: gdk::RGBA::new(0.47, 0.73, 1.0, 1.0),
            tag: gdk::RGBA::new(0.45, 0.76, 0.96, 1.0),
            highlight_bg: gdk::RGBA::new(0.88, 0.68, 0.12, 0.32),
            comment: gdk::RGBA::new(0.64, 0.68, 0.74, 0.78),
            frontmatter: gdk::RGBA::new(0.67, 0.70, 0.76, 0.85),
            unresolved_link: gdk::RGBA::new(0.66, 0.70, 0.77, 0.92),
            callout_bg: gdk::RGBA::new(0.38, 0.66, 0.98, 0.14),
            task: gdk::RGBA::new(0.47, 0.73, 1.0, 1.0),
            find_match_bg: gdk::RGBA::new(0.78, 0.62, 0.10, 0.42),
            find_current_bg: gdk::RGBA::new(0.90, 0.48, 0.10, 0.62),
        }
    } else {
        TagThemePalette {
            marker: gdk::RGBA::new(0.42, 0.45, 0.50, 0.72),
            code_bg: gdk::RGBA::new(0.50, 0.54, 0.60, 0.14),
            quote: gdk::RGBA::new(0.36, 0.40, 0.45, 0.95),
            link: gdk::RGBA::new(0.11, 0.44, 0.84, 1.0),
            tag: gdk::RGBA::new(0.12, 0.45, 0.76, 1.0),
            highlight_bg: gdk::RGBA::new(0.96, 0.83, 0.18, 0.34),
            comment: gdk::RGBA::new(0.43, 0.46, 0.51, 0.76),
            frontmatter: gdk::RGBA::new(0.42, 0.45, 0.50, 0.82),
            unresolved_link: gdk::RGBA::new(0.48, 0.51, 0.57, 0.92),
            callout_bg: gdk::RGBA::new(0.21, 0.52, 0.89, 0.08),
            task: gdk::RGBA::new(0.11, 0.44, 0.84, 1.0),
            find_match_bg: gdk::RGBA::new(0.98, 0.86, 0.30, 0.42),
            find_current_bg: gdk::RGBA::new(0.98, 0.62, 0.16, 0.62),
        }
    }
}

pub fn apply_tag_table_theme(table: &gtk4::TextTagTable, is_dark: bool) {
    let p = palette_for_theme(is_dark);
    let set_fg = |name: &str, rgba: &gdk::RGBA| {
        if let Some(tag) = table.lookup(name) {
            tag.set_foreground_rgba(Some(rgba));
        }
    };
    let set_bg = |name: &str, rgba: &gdk::RGBA| {
        if let Some(tag) = table.lookup(name) {
            tag.set_background_rgba(Some(rgba));
        }
    };
    set_fg("marker", &p.marker);
    set_bg("code", &p.code_bg);
    set_fg("quote", &p.quote);
    set_fg("quote_block", &p.quote);
    set_fg("link", &p.link);
    set_fg("tag", &p.tag);
    set_bg("highlight", &p.highlight_bg);
    set_fg("comment", &p.comment);
    set_fg("frontmatter", &p.frontmatter);
    set_fg("unresolved_link", &p.unresolved_link);
    if let Some(tag) = table.lookup("callout_block") {
        tag.set_paragraph_background_rgba(Some(&p.callout_bg));
    }
    set_fg("task", &p.task);
    set_bg("find_match", &p.find_match_bg);
    set_bg("find_current", &p.find_current_bg);
}

pub fn create_tag_table_for_mode(is_dark: bool, is_reading: bool) -> gtk4::TextTagTable {
    let table = gtk4::TextTagTable::new();

    // 块级/引用容器标签必须先于行内高亮与链接标签加入，确保链接蓝、查找高亮等覆盖引用灰。
    table.add(
        &gtk4::TextTag::builder()
            .name("frontmatter")
            .family("monospace")
            .scale(0.92)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("codeblock")
            .family("monospace")
            .build(),
    );
    if is_reading {
        table.add(
            &gtk4::TextTag::builder()
                .name("callout_block")
                .left_margin(10)
                .right_margin(10)
                .build(),
        );
        table.add(
            &gtk4::TextTag::builder()
                .name("quote_block")
                .left_margin(16)
                .build(),
        );
    } else {
        // 编辑态绝不在 TextTag 上设置绝对 left/right_margin，避免覆盖 TextView 的居中列宽。
        table.add(&gtk4::TextTag::builder().name("callout_block").build());
        table.add(&gtk4::TextTag::builder().name("quote_block").build());
    }
    table.add(&gtk4::TextTag::builder().name("quote").build());
    table.add(&gtk4::TextTag::builder().name("callout").weight(600).build());

    for level in 1u8..=6 {
        let (scale, above, below) = match level {
            1 => (1.65, 2, 0),
            2 => (1.38, 4, 0),
            3 => (1.20, 2, 0),
            4 => (1.10, 2, 0),
            5 => (1.04, 0, 0),
            _ => (1.0, 0, 0),
        };
        let tag = gtk4::TextTag::builder()
            .name(format!("h{level}"))
            .scale(scale)
            .weight(700)
            .pixels_above_lines(above)
            .pixels_below_lines(below)
            .build();
        table.add(&tag);
    }

    table.add(&gtk4::TextTag::builder().name("marker").build());
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
            .name("comment")
            .style(pango::Style::Italic)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("blank_line")
            .scale(0.38)
            .pixels_above_lines(0)
            .pixels_below_lines(0)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("code")
            .family("monospace")
            .build(),
    );
    table.add(&gtk4::TextTag::builder().name("highlight").build());
    table.add(&gtk4::TextTag::builder().name("task").weight(600).build());
    table.add(&gtk4::TextTag::builder().name("tag").weight(500).build());
    table.add(
        &gtk4::TextTag::builder()
            .name("unresolved_link")
            .underline(pango::Underline::Single)
            .build(),
    );
    table.add(
        &gtk4::TextTag::builder()
            .name("link")
            .underline(pango::Underline::Single)
            .build(),
    );
    table.add(&gtk4::TextTag::builder().name("find_match").build());
    table.add(&gtk4::TextTag::builder().name("find_current").build());

    apply_tag_table_theme(&table, is_dark);
    table
}

#[cfg(test)]
pub fn create_tag_table_for_theme(is_dark: bool) -> gtk4::TextTagTable {
    create_tag_table_for_mode(is_dark, true)
}

pub fn create_reading_tag_table() -> gtk4::TextTagTable {
    let is_dark = adw::is_initialized() && adw::StyleManager::default().is_dark();
    create_tag_table_for_mode(is_dark, true)
}

pub fn create_edit_tag_table() -> gtk4::TextTagTable {
    let is_dark = adw::is_initialized() && adw::StyleManager::default().is_dark();
    create_tag_table_for_mode(is_dark, false)
}

#[allow(dead_code)]
pub fn create_tag_table() -> gtk4::TextTagTable {
    create_reading_tag_table()
}

#[cfg(test)]
static GTK_INIT_RESULT: std::sync::OnceLock<(bool, std::thread::ThreadId)> =
    std::sync::OnceLock::new();

/// 确保多线程 `cargo test` 下 GTK/libadwaita 仅尝试初始化一次，避免无显示环境或跨线程重复调用 `gtk_init_check`。
#[cfg(test)]
pub fn ensure_gtk_test_thread() -> bool {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    let (ok, tid) = *GTK_INIT_RESULT.get_or_init(|| {
        let gtk_ok = gtk4::is_initialized() || gtk4::init().is_ok();
        let adw_ok = gtk_ok && (adw::is_initialized() || adw::init().is_ok());
        (adw_ok, std::thread::current().id())
    });
    ok && (gtk4::is_initialized_main_thread() || tid == std::thread::current().id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_table_switches_between_light_and_dark_themes() {
        if !ensure_gtk_test_thread() {
            return;
        }
        let table = create_tag_table_for_theme(false);
        let light_link = table.lookup("link").unwrap().foreground_rgba().unwrap();
        let light_callout = table
            .lookup("callout_block")
            .unwrap()
            .paragraph_background_rgba()
            .unwrap();
        apply_tag_table_theme(&table, true);
        let dark_link = table.lookup("link").unwrap().foreground_rgba().unwrap();
        let dark_callout = table
            .lookup("callout_block")
            .unwrap()
            .paragraph_background_rgba()
            .unwrap();
        assert_ne!(light_link, dark_link);
        assert_ne!(light_callout, dark_callout);
        assert_eq!(dark_link, palette_for_theme(true).link);
    }

    #[test]
    fn link_priority_beats_quote_and_callout_and_edit_clears_margins() {
        if !ensure_gtk_test_thread() {
            return;
        }
        for is_dark in [false, true] {
            let read_tbl = create_tag_table_for_mode(is_dark, true);
            let edit_tbl = create_tag_table_for_mode(is_dark, false);

            for tbl in [&read_tbl, &edit_tbl] {
                let link = tbl.lookup("link").unwrap();
                let unresolved = tbl.lookup("unresolved_link").unwrap();
                let quote = tbl.lookup("quote").unwrap();
                let quote_block = tbl.lookup("quote_block").unwrap();
                let callout_block = tbl.lookup("callout_block").unwrap();
                assert!(link.priority() > quote.priority());
                assert!(link.priority() > quote_block.priority());
                assert!(link.priority() > callout_block.priority());
                assert!(unresolved.priority() > quote_block.priority());
                assert!(unresolved.priority() > callout_block.priority());
            }

            let read_callout = read_tbl.lookup("callout_block").unwrap();
            let read_quote = read_tbl.lookup("quote_block").unwrap();
            assert!(read_callout.property::<bool>("left-margin-set"));
            assert_eq!(read_callout.left_margin(), 10);
            assert!(read_quote.property::<bool>("left-margin-set"));
            assert_eq!(read_quote.left_margin(), 16);

            let edit_callout = edit_tbl.lookup("callout_block").unwrap();
            let edit_quote = edit_tbl.lookup("quote_block").unwrap();
            assert!(!edit_callout.property::<bool>("left-margin-set"));
            assert!(!edit_callout.property::<bool>("right-margin-set"));
            assert!(!edit_quote.property::<bool>("left-margin-set"));
            assert!(!edit_quote.property::<bool>("right-margin-set"));
        }
    }
}
