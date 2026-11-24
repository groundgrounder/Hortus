// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::Utf16Mapper;
use gtk4::glib;
use gtk4::prelude::*;
use std::rc::Rc;

pub type OpenNoteCallback = Rc<dyn Fn(String, Option<u64>)>;

pub fn clear_box(container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

pub fn escape_markup(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// 把 `snippet` 中的匹配区间（UTF-16，相对整篇笔记）转换成 Pango 加粗标记。
pub fn snippet_markup(
    snippet: &str,
    snippet_start_utf16: u32,
    match_start_utf16: u32,
    length: u32,
) -> String {
    let mapper = Utf16Mapper::new(snippet);
    let rel_start = match_start_utf16.saturating_sub(snippet_start_utf16) as usize;
    let b0 = mapper.utf16_to_byte(rel_start).min(snippet.len());
    let b1 = mapper
        .utf16_to_byte(rel_start + length as usize)
        .min(snippet.len())
        .max(b0);
    let before = &snippet[..b0];
    let hit = &snippet[b0..b1];
    let after = &snippet[b1..];
    format!(
        "{}<b>{}</b>{}",
        escape_markup(before),
        escape_markup(hit),
        escape_markup(after)
    )
}

pub fn panel_scroll(child: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(child)
        .build()
}

pub fn dim_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_wrap(true);
    label.set_xalign(0.0);
    label
}

pub fn entry_row(title: &str, subtitle: Option<&str>, meta: Option<&str>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let title_label = gtk4::Label::new(Some(title));
    title_label.add_css_class("hortus-result-title");
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    row.append(&title_label);
    if let Some(subtitle) = subtitle {
        let sub = gtk4::Label::new(None);
        sub.set_markup(subtitle);
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        sub.add_css_class("hortus-result-snippet");
        row.append(&sub);
    }
    if let Some(meta) = meta {
        let meta_label = gtk4::Label::new(Some(meta));
        meta_label.set_xalign(0.0);
        meta_label.add_css_class("hortus-result-meta");
        row.append(&meta_label);
    }
    row
}
