// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 磁盘指纹对账、批量时间戳采集与热重载滚动/光标锚点辅助。

use gtk4::glib;
use gtk4::prelude::*;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
pub struct EditReloadAnchor {
    pub reading_fraction: Option<f64>,
    pub edit_fraction: Option<f64>,
    pub cursor_offset: Option<i32>,
}

pub fn read_disk_stamp(vault_root: &str, path: &str) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(Path::new(vault_root).join(path)).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified))
}

pub fn collect_disk_stamps(
    vault_root: &str,
    paths: &[String],
) -> Vec<(String, Option<(u64, u128)>)> {
    paths
        .iter()
        .map(|path| (path.clone(), read_disk_stamp(vault_root, path)))
        .collect()
}

pub fn capture_scrolled_fraction(scrolled: &gtk4::ScrolledWindow) -> Option<f64> {
    let adj = scrolled.vadjustment();
    let max = adj.upper() - adj.page_size();
    (max > 0.0).then(|| (adj.value() / max).clamp(0.0, 1.0))
}

pub fn capture_edit_reload_anchor(
    preserve_scroll: bool,
    reading_scrolled: Option<&gtk4::ScrolledWindow>,
    editing: bool,
    edit_scrolled: &gtk4::ScrolledWindow,
    edit_buffer: &gtk4::TextBuffer,
) -> EditReloadAnchor {
    if !preserve_scroll {
        return EditReloadAnchor::default();
    }
    let reading_fraction = reading_scrolled.and_then(capture_scrolled_fraction);
    let (edit_fraction, cursor_offset) = if editing {
        let insert = edit_buffer.get_insert();
        let offset = edit_buffer.iter_at_mark(&insert).offset();
        (capture_scrolled_fraction(edit_scrolled), Some(offset))
    } else {
        (None, None)
    };
    EditReloadAnchor {
        reading_fraction,
        edit_fraction,
        cursor_offset,
    }
}

pub fn restore_edit_reload_anchor(
    anchor: EditReloadAnchor,
    edit_scrolled: &gtk4::ScrolledWindow,
    edit_buffer: &gtk4::TextBuffer,
) {
    if let Some(offset) = anchor.cursor_offset {
        let clamped = offset.clamp(0, edit_buffer.char_count());
        let iter = edit_buffer.iter_at_offset(clamped);
        edit_buffer.place_cursor(&iter);
    }
    if let Some(fraction) = anchor.edit_fraction {
        let weak_scrolled = edit_scrolled.downgrade();
        glib::idle_add_local_once(move || {
            let Some(scrolled) = weak_scrolled.upgrade() else {
                return;
            };
            let adj = scrolled.vadjustment();
            let max = adj.upper() - adj.page_size();
            if max > 0.0 {
                adj.set_value((max * fraction).clamp(0.0, max));
            }
        });
    }
}
