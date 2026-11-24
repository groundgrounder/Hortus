// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 大纲渲染、正文双向锚点滚动，以及图表专注覆盖层与矢量导出入口。

use crate::core::model::HeadingItem;
use crate::core::utf16_offset_to_char_offset;
use crate::files::operations::short_error;
use crate::knowledge::clear_box;
use crate::reading::{FocusCallbacks, FocusRequest};
use crate::render::diagram::{
    DiagramExportKind, DiagramExportMeta, default_export_filename, export_diagram_to_file,
};
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

impl WindowInner {
    pub fn render_outline_empty(&self, message: &str) {
        clear_box(&self.outline_box);
        let lbl = gtk4::Label::new(Some(message));
        lbl.add_css_class("dim-label");
        lbl.add_css_class("caption");
        lbl.set_margin_top(16);
        self.outline_box.append(&lbl);
    }

    pub fn render_outline(self: &Rc<Self>, note_path: &str, headings: &[HeadingItem]) {
        clear_box(&self.outline_box);
        if headings.is_empty() {
            self.render_outline_empty("本笔记暂无标题大纲");
            return;
        }
        for heading in headings {
            let indent = (heading.level.saturating_sub(1) as i32) * 12;
            let lbl = gtk4::Label::new(Some(&heading.title));
            lbl.set_xalign(0.0);
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl.set_margin_start(indent);
            lbl.add_css_class("caption");

            let btn = gtk4::Button::builder().child(&lbl).build();
            btn.add_css_class("flat");
            btn.add_css_class("hortus-outline-btn");

            let weak = Rc::downgrade(self);
            let path = note_path.to_string();
            let start = heading.start;
            btn.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.scroll_to_utf16_offset(&path, start);
                }
            });
            self.outline_box.append(&btn);
        }
    }

    pub fn scroll_to_heading_title(&self, note_path: &str, heading_title: &str) {
        let target = {
            let st = self.state.borrow();
            st.tabs.get(note_path).and_then(|t| {
                t.headings
                    .iter()
                    .find(|h| h.title.eq_ignore_ascii_case(heading_title))
                    .map(|h| h.start)
            })
        };
        if let Some(start) = target {
            self.scroll_to_utf16_offset(note_path, start);
        }
    }

    pub fn scroll_to_utf16_offset(&self, note_path: &str, start_utf16: u64) {
        let (editing, edit_view, edit_buffer, reading, char_off) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(note_path) else {
                return;
            };
            let char_off = if tab.editing {
                let edit = tab.edit_state.borrow();
                utf16_offset_to_char_offset(&edit.text, start_utf16 as usize)
            } else {
                utf16_offset_to_char_offset(&tab.text, start_utf16 as usize)
            };
            (
                tab.editing,
                tab.edit_view.clone(),
                tab.edit_buffer.clone(),
                tab.reading.clone(),
                char_off,
            )
        };
        if !editing && let Some(reading) = reading {
            reading.scroll_to_source_utf16(start_utf16);
            return;
        }
        let mut iter = edit_buffer.iter_at_offset(char_off as i32);
        edit_buffer.place_cursor(&iter);
        edit_view.scroll_to_iter(&mut iter, 0.15, false, 0.0, 0.12);
    }

    pub fn open_focus_mode(self: &Rc<Self>, note_path: &str, ordinal: u32) {
        let fields = {
            let st = self.state.borrow();
            st.tabs.get(note_path).map(|tab| {
                let diag = tab
                    .prepared_diagrams
                    .iter()
                    .find(|d| d.ordinal == ordinal && d.scene.is_some())
                    .cloned()
                    .or_else(|| {
                        tab.prepared_diagrams
                            .iter()
                            .find(|d| d.scene.is_some())
                            .cloned()
                    });
                (
                    st.services.clone(),
                    st.vault_root(),
                    tab.text.clone(),
                    note_path.to_ascii_lowercase().ends_with(".mmd"),
                    diag,
                )
            })
        };
        let Some((services_slot, vault_slot, source, mmd, diagram)) = fields else {
            self.show_toast("请先打开包含图表的笔记");
            return;
        };
        let (Some(services), Some(vault_root), Some(diagram)) =
            (services_slot, vault_slot, diagram)
        else {
            self.show_toast("当前笔记没有可用的 Mermaid 图表");
            return;
        };
        let Some(scene) = diagram.scene else { return };
        let req = FocusRequest {
            vault_root,
            note_path: note_path.to_string(),
            ordinal: diagram.ordinal,
            source,
            mmd,
            collapsed: diagram.collapsed,
            scene,
            core: Arc::clone(&services.core),
            engine: Arc::clone(&services.diagram_engine),
        };
        let weak_collapse = Rc::downgrade(self);
        let weak_link = Rc::downgrade(self);
        let weak_export = Rc::downgrade(self);
        let weak_exit = Rc::downgrade(self);
        let rel_collapse = note_path.to_string();
        let rel_link = note_path.to_string();
        self.focus.open(
            req,
            FocusCallbacks {
                on_collapse_changed: Rc::new(move |ordinal, collapsed, new_scene| {
                    if let Some(win) = weak_collapse.upgrade()
                        && let Some(tab) = win.state.borrow_mut().tabs.get_mut(&rel_collapse)
                        && let Some(item) = tab
                            .prepared_diagrams
                            .iter_mut()
                            .find(|d| d.ordinal == ordinal)
                    {
                        item.collapsed = collapsed;
                        item.previous = new_scene.previous.clone();
                        item.scene = Some(new_scene);
                    }
                }),
                on_open_link: Rc::new(move |target, heading| {
                    if let Some(win) = weak_link.upgrade() {
                        win.send_command(crate::window::commands::WindowCommand::ResolveWikilink {
                            source_note: rel_link.clone(),
                            raw_target: target,
                            heading,
                        });
                    }
                }),
                on_export: Rc::new(move |kind, meta| {
                    if let Some(win) = weak_export.upgrade() {
                        win.prompt_and_export_diagram(kind, meta);
                    }
                }),
                on_exit: Rc::new(move || {
                    let _ = weak_exit.upgrade();
                }),
            },
        );
    }

    pub fn prompt_and_export_diagram(
        self: &Rc<Self>,
        kind: DiagramExportKind,
        meta: DiagramExportMeta,
    ) {
        let dialog = gtk4::FileDialog::builder()
            .title(match kind {
                DiagramExportKind::Svg => "导出 SVG 矢量图",
                DiagramExportKind::Png => "导出高分 PNG 图片",
                DiagramExportKind::Pdf => "导出 PDF 文档",
            })
            .initial_name(default_export_filename(&meta.note_path, meta.ordinal, kind))
            .modal(true)
            .build();
        let Some(engine) = self
            .state
            .borrow()
            .services
            .as_ref()
            .map(|svcs| Arc::clone(&svcs.diagram_engine))
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(out_path) = file.path() else { return };
            let weak_done = weak.clone();
            export_diagram_to_file(engine, meta, kind, out_path, move |res| match res {
                Ok(saved_path) => {
                    let name = saved_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("diagram");
                    if let Some(win) = weak_done.upgrade() {
                        win.show_toast(&format!("已导出 {name}"));
                    }
                }
                Err(err) => {
                    if let Some(win) = weak_done.upgrade() {
                        win.show_toast(&short_error("导出失败", &err));
                    }
                }
            });
        });
    }
}
