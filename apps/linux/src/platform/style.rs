// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use gtk4::gdk;
use gtk4::pango;
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
    background-color: alpha(#2ec27e, 0.16);
    color: @view_fg_color;
    font-family: monospace;
    font-size: 0.88em;
}

.hortus-diff-del {
    background-color: alpha(#e01b24, 0.16);
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
    background-color: alpha(#2ec27e, 0.18);
    color: #26a269;
    font-size: 0.84em;
}

.hortus-agent-badge-err {
    padding: 2px 8px;
    border-radius: 999px;
    background-color: alpha(#e01b24, 0.16);
    color: #c01c28;
    font-size: 0.84em;
}
"#;

static CSS_INSTALL_ONCE: Once = Once::new();

thread_local! {
    static DYNAMIC_FONT_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

fn format_font_css(font_size_px: f64) -> String {
    let px = font_size_px.clamp(10.0, 32.0);
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

pub fn create_tag_table() -> gtk4::TextTagTable {
    let table = gtk4::TextTagTable::new();

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
    table.add(
        &gtk4::TextTag::builder()
            .name("blank_line")
            .scale(0.38)
            .pixels_above_lines(0)
            .pixels_below_lines(0)
            .build(),
    );

    table
}

#[cfg(test)]
static GTK_INIT_RESULT: std::sync::OnceLock<(bool, std::thread::ThreadId)> =
    std::sync::OnceLock::new();

/// 确保多线程 `cargo test` 下 GTK 仅尝试初始化一次，避免无显示环境或跨线程重复调用 `gtk_init_check`。
#[cfg(test)]
pub fn ensure_gtk_test_thread() -> bool {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    let (ok, tid) = *GTK_INIT_RESULT.get_or_init(|| {
        let ok = if gtk4::is_initialized() {
            true
        } else {
            gtk4::init().is_ok()
        };
        (ok, std::thread::current().id())
    });
    ok && (gtk4::is_initialized_main_thread() || tid == std::thread::current().id())
}
