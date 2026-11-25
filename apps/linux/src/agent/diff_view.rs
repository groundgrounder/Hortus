// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 逐行 Unified Diff 渲染控件：原生着色 hunk 头与增删行，支持单 hunk 回滚。

use crate::i18n::{Key, fmt2, tr};
use gtk4::prelude::*;
use hortus_ffi::{ReviewDiffHunk, ReviewEntryDetail};
use std::rc::Rc;

pub fn render_entry_detail(
    container: &gtk4::Box,
    detail: &ReviewEntryDetail,
    on_rollback_hunk: Rc<dyn Fn(u32, ReviewDiffHunk)>,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    if detail.binary {
        let before_sz = detail.entry.before.as_ref().map(|m| m.size).unwrap_or(0);
        let after_sz = detail.entry.after.as_ref().map(|m| m.size).unwrap_or(0);
        let msg = gtk4::Label::new(Some(&fmt2(Key::DiffBinaryMsg, before_sz, after_sz)));
        msg.add_css_class("dim-label");
        msg.add_css_class("caption");
        msg.set_wrap(true);
        msg.set_xalign(0.0);
        container.append(&msg);
        return;
    }

    if detail.large {
        let msg = gtk4::Label::new(Some(tr(Key::DiffLargeMsg)));
        msg.add_css_class("dim-label");
        msg.add_css_class("caption");
        msg.set_wrap(true);
        msg.set_xalign(0.0);
        container.append(&msg);
        return;
    }

    if !detail.text {
        let none_lbl = tr(Key::DiffNoneLabel).to_string();
        let before_kind = detail
            .entry
            .before
            .as_ref()
            .map(|m| {
                format!(
                    "{}{}",
                    m.kind,
                    m.link
                        .as_deref()
                        .map(|l| format!(" -> {l}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| none_lbl.clone());
        let after_kind = detail
            .entry
            .after
            .as_ref()
            .map(|m| {
                format!(
                    "{}{}",
                    m.kind,
                    m.link
                        .as_deref()
                        .map(|l| format!(" -> {l}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or(none_lbl);
        let msg = gtk4::Label::new(Some(&fmt2(Key::DiffNonTextMsg, before_kind, after_kind)));
        msg.add_css_class("dim-label");
        msg.add_css_class("caption");
        msg.set_wrap(true);
        msg.set_xalign(0.0);
        container.append(&msg);
        return;
    }

    if detail.content_identical || detail.hunks.is_empty() {
        let msg = gtk4::Label::new(Some(if detail.content_identical {
            tr(Key::DiffIdenticalContent)
        } else {
            tr(Key::DiffNoTextChanges)
        }));
        msg.add_css_class("dim-label");
        msg.add_css_class("caption");
        msg.set_xalign(0.0);
        container.append(&msg);
        return;
    }

    let can_hunk_rollback = detail.entry.change == "modified";
    for (idx, hunk) in detail.hunks.iter().enumerate() {
        let hunk_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        hunk_box.set_margin_bottom(8);

        let header_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header_row.add_css_class("hortus-diff-hunk-header");
        let header_lbl = gtk4::Label::new(Some(&format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        )));
        header_lbl.set_xalign(0.0);
        header_lbl.set_hexpand(true);
        header_row.append(&header_lbl);

        if can_hunk_rollback {
            let rb_btn = gtk4::Button::with_label(tr(Key::DiffRestoreSectionBtn));
            rb_btn.add_css_class("flat");
            rb_btn.add_css_class("caption");
            let hunk_clone = hunk.clone();
            let cb = on_rollback_hunk.clone();
            rb_btn.connect_clicked(move |_| cb(idx as u32, hunk_clone.clone()));
            header_row.append(&rb_btn);
        }
        hunk_box.append(&header_row);

        for line in &hunk.lines {
            let prefix = match line.kind.as_str() {
                "added" => "+",
                "removed" => "-",
                _ => " ",
            };
            let old_no = line
                .old_line
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".into());
            let new_no = line
                .new_line
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".into());
            let text_clean = line.text.trim_end_matches(['\r', '\n']);
            let row_lbl =
                gtk4::Label::new(Some(&format!("{old_no} {new_no} {prefix}{text_clean}")));
            row_lbl.set_xalign(0.0);
            row_lbl.set_selectable(true);
            row_lbl.set_wrap(true);
            row_lbl.set_wrap_mode(gtk4::pango::WrapMode::Char);
            match line.kind.as_str() {
                "added" => row_lbl.add_css_class("hortus-diff-add"),
                "removed" => row_lbl.add_css_class("hortus-diff-del"),
                _ => row_lbl.add_css_class("hortus-diff-ctx"),
            }
            hunk_box.append(&row_lbl);
        }
        container.append(&hunk_box);
    }
}
