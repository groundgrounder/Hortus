// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::{PanelRegion, PlaceholderKind};
use crate::window::shell::WindowInner;
use gtk4::glib;
use gtk4::prelude::*;
use serde::Serialize;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

#[derive(Serialize)]
struct SmokeTab {
    path: String,
    title: String,
    char_count: i32,
}

#[derive(Serialize)]
struct SmokeHeading {
    level: u8,
    title: String,
    start: u64,
}

#[derive(Serialize)]
struct SmokeReport {
    vault_root: String,
    scan_complete: bool,
    scan_micros: u64,
    entry_count: usize,
    note_count: usize,
    note_paths: Vec<String>,
    open_tabs: Vec<SmokeTab>,
    active_note: Option<String>,
    outline_headings: Vec<SmokeHeading>,
    outline_scroll_moved: bool,
    outline_scroll_before: f64,
    outline_scroll_after: f64,
    tab_overview_opened: bool,
    tab_overview_listed_count: usize,
    wide_width: i32,
    wide_left_collapsed: bool,
    wide_right_collapsed: bool,
    narrow_width: i32,
    narrow_left_collapsed: bool,
    narrow_right_collapsed: bool,
    top_bar_hidden_ok: bool,
    top_bar_restored_ok: bool,
    edit_mode_toggled_on: bool,
    edit_mode_toggled_off: bool,
    edit_buffer_modified_in_memory: bool,
    panel_move_verified: bool,
    startup_ms: u64,
}

pub fn execute_smoke_checks(win: &Rc<WindowInner>, smoke_path: PathBuf) {
    let (entry_count, note_count, note_paths) = win.vault_tree.stats();
    let (vault_root, scan_complete, scan_micros, startup_ms) = {
        let st = win.state.borrow();
        (
            st.vault_root().unwrap_or_default(),
            st.scan_complete,
            st.scan_micros,
            st.started_at.elapsed().as_millis() as u64,
        )
    };

    let mut open_tabs = Vec::new();
    for idx in 0..win.tab_view.n_pages() {
        let page = win.tab_view.nth_page(idx);
        let facts = {
            let st = win.state.borrow();
            st.tabs
                .values()
                .find(|t| t.page == page)
                .map(|tab| (tab.path.clone(), tab.edit_buffer.char_count()))
        };
        if let Some((path, char_count)) = facts {
            open_tabs.push(SmokeTab {
                path,
                title: page.title().to_string(),
                char_count,
            });
        }
    }

    let active_note = win.active_tab_path();
    let mut outline_headings: Vec<SmokeHeading> = active_note
        .as_ref()
        .and_then(|p| {
            win.state.borrow().tabs.get(p).map(|t| {
                t.headings
                    .iter()
                    .map(|h| SmokeHeading {
                        level: h.level,
                        title: h.title.clone(),
                        start: h.start,
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    let (outline_scroll_before, outline_scroll_after, outline_scroll_moved) =
        if let Some(ref active) = active_note {
            let before = win
                .state
                .borrow()
                .tabs
                .get(active)
                .and_then(|t| t.reading.as_ref().map(|r| r.scrolled.vadjustment().value()))
                .unwrap_or(0.0);
            if let Some(last_heading) = outline_headings.last() {
                win.scroll_to_utf16_offset(active, last_heading.start);
            }
            let after = win
                .state
                .borrow()
                .tabs
                .get(active)
                .and_then(|t| t.reading.as_ref().map(|r| r.scrolled.vadjustment().value()))
                .unwrap_or(0.0);
            (before, after, after > before)
        } else {
            (0.0, 0.0, false)
        };

    win.tab_overview.set_open(true);
    let tab_overview_opened = win.tab_overview.is_open();
    let tab_overview_listed_count = win.tab_view.n_pages() as usize;
    win.tab_overview.set_open(false);

    win.toolbar_view.set_reveal_top_bars(false);
    let top_bar_hidden_ok = !win.toolbar_view.reveals_top_bars();
    win.toolbar_view.set_reveal_top_bars(true);
    let top_bar_restored_ok = win.toolbar_view.reveals_top_bars();

    win.toggle_active_tab_edit_mode();
    let edit_mode_toggled_on = active_note
        .as_ref()
        .and_then(|p| {
            win.state
                .borrow()
                .tabs
                .get(p)
                .map(|t| t.editing && t.edit_view.is_editable())
        })
        .unwrap_or(false);
    let edit_buffer_modified_in_memory = if let Some(ref active) = active_note {
        let st = win.state.borrow();
        if let Some(tab) = st.tabs.get(active) {
            let before_chars = tab.edit_buffer.char_count();
            let mut end_iter = tab.edit_buffer.end_iter();
            tab.edit_buffer
                .insert(&mut end_iter, "\n[内存输入法原型测试]");
            tab.edit_buffer.char_count() > before_chars
        } else {
            false
        }
    } else {
        false
    };
    win.toggle_active_tab_edit_mode();
    let edit_mode_toggled_off = active_note
        .as_ref()
        .and_then(|p| {
            win.state.borrow().tabs.get(p).map(|t| {
                !t.editing && t.content_stack.visible_child_name().as_deref() == Some("reading")
            })
        })
        .unwrap_or(false);

    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.toggle-bottom-panel", None);
    let bottom_synced_on = win.bottom_toggle.is_active() && win.bottom_revealer.reveals_child();
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.toggle-bottom-panel", None);
    let bottom_synced_off = !win.bottom_toggle.is_active() && !win.bottom_revealer.reveals_child();

    win.toggle_placeholder_region(PlaceholderKind::Backlinks);
    let moved_to_bottom = win
        .state
        .borrow()
        .placement
        .region_of(PlaceholderKind::Backlinks)
        == PanelRegion::Bottom
        && win.bottom_toggle.is_active();
    win.toggle_placeholder_region(PlaceholderKind::Backlinks);
    let moved_back_to_right = win
        .state
        .borrow()
        .placement
        .region_of(PlaceholderKind::Backlinks)
        == PanelRegion::Right;
    let panel_move_verified =
        moved_to_bottom && moved_back_to_right && bottom_synced_on && bottom_synced_off;

    let wide_width = win.window.width();
    let wide_left_collapsed = win.left_split.is_collapsed();
    let wide_right_collapsed = win.right_split.is_collapsed();

    win.window.set_resizable(false);
    win.window.set_size_request(680, 800);
    win.window.set_default_size(680, 800);
    win.window.queue_resize();

    let weak = Rc::downgrade(win);
    let elapsed_ms = Rc::new(Cell::new(0u32));
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let Some(this) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let waited = elapsed_ms.get() + 50;
        elapsed_ms.set(waited);
        let cur_width = this.window.width();
        if (cur_width < 820 && this.left_split.is_collapsed()) || waited >= 2500 {
            let report = SmokeReport {
                vault_root: vault_root.clone(),
                scan_complete,
                scan_micros,
                entry_count,
                note_count,
                note_paths: note_paths.clone(),
                open_tabs: std::mem::take(&mut open_tabs),
                active_note: active_note.clone(),
                outline_headings: std::mem::take(&mut outline_headings),
                outline_scroll_moved,
                outline_scroll_before,
                outline_scroll_after,
                tab_overview_opened,
                tab_overview_listed_count,
                wide_width,
                wide_left_collapsed,
                wide_right_collapsed,
                narrow_width: cur_width,
                narrow_left_collapsed: this.left_split.is_collapsed(),
                narrow_right_collapsed: this.right_split.is_collapsed(),
                top_bar_hidden_ok,
                top_bar_restored_ok,
                edit_mode_toggled_on,
                edit_mode_toggled_off,
                edit_buffer_modified_in_memory,
                panel_move_verified,
                startup_ms,
            };
            if let Some(parent) = smoke_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match serde_json::to_string_pretty(&report) {
                Ok(json) => {
                    if let Err(err) = std::fs::write(&smoke_path, format!("{json}\n")) {
                        eprintln!("write smoke json failed: {err}");
                    }
                }
                Err(err) => eprintln!("serialize smoke json failed: {err}"),
            }
            this.close_now_clean();
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
