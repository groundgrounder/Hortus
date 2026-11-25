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
use crate::window::reading_layout::ReadingScrollTarget;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

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
            self.render_outline_empty(crate::i18n::tr(crate::i18n::Key::OutlineEmpty));
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
        let (editing, edit_scrolled, edit_view, edit_buffer, reading, char_off) = {
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
                tab.edit_scrolled.clone(),
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
        if start_utf16 == 0 {
            let start_iter = edit_buffer.start_iter();
            edit_buffer.place_cursor(&start_iter);
            edit_scrolled.vadjustment().set_value(0.0);
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
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenDiagramNoteFirst));
            return;
        };
        let (Some(services), Some(vault_root), Some(diagram)) =
            (services_slot, vault_slot, diagram)
        else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastNoMermaidDiagram));
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
                DiagramExportKind::Svg => crate::i18n::tr(crate::i18n::Key::ExportTitleSvg),
                DiagramExportKind::Png => crate::i18n::tr(crate::i18n::Key::ExportTitlePng),
                DiagramExportKind::Pdf => crate::i18n::tr(crate::i18n::Key::ExportTitlePdf),
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
                        win.show_toast(&crate::i18n::fmt1(crate::i18n::Key::ToastExported, name));
                    }
                }
                Err(err) => {
                    if let Some(win) = weak_done.upgrade() {
                        win.show_toast(&short_error(
                            crate::i18n::tr(crate::i18n::Key::ErrExportFailed),
                            &err,
                        ));
                    }
                }
            });
        });
    }
}

pub(crate) fn launch_edit_scroll_sync(
    win: &Rc<WindowInner>,
    rel: &str,
    edit_buffer: &gtk4::TextBuffer,
    edit_scrolled: &gtk4::ScrolledWindow,
    anchor_u16: u64,
    scroll_gen: u64,
    attempts_left: u8,
) {
    if anchor_u16 == 0 {
        let start_iter = edit_buffer.start_iter();
        edit_buffer.place_cursor(&start_iter);
        edit_scrolled.vadjustment().set_value(0.0);
    } else {
        let text = win
            .state
            .borrow()
            .tabs
            .get(rel)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let byte_off =
            crate::core::text::Utf16Mapper::new(&text).utf16_to_byte(anchor_u16 as usize);
        let char_off = text[..byte_off.min(text.len())].chars().count() as i32;
        let iter = edit_buffer.iter_at_offset(char_off);
        edit_buffer.place_cursor(&iter);
    }
    let expected_cursor = edit_buffer.cursor_position();
    let initial_scroll = edit_scrolled.vadjustment().value();
    schedule_edit_scroll_sync(
        Rc::downgrade(win),
        rel.to_string(),
        anchor_u16,
        scroll_gen,
        expected_cursor,
        initial_scroll,
        attempts_left,
    );
}

pub(crate) fn schedule_edit_scroll_sync(
    weak_win: std::rc::Weak<WindowInner>,
    rel: String,
    anchor_u16: u64,
    expected_gen: u64,
    expected_cursor: i32,
    initial_scroll: f64,
    attempts_left: u8,
) {
    glib::idle_add_local_once(move || {
        let Some(win) = weak_win.upgrade() else {
            return;
        };
        if win.state.borrow().disposing {
            return;
        }
        let info = {
            let st = win.state.borrow();
            st.tabs.get(&rel).map(|tab| {
                (
                    tab.editing,
                    tab.edit_scroll_gen,
                    tab.edit_view.is_mapped(),
                    tab.edit_view.width(),
                    tab.edit_host.width(),
                    tab.edit_host.clone(),
                    tab.edit_scrolled.clone(),
                    tab.edit_buffer.clone(),
                )
            })
        };
        let Some((editing, cur_gen, mapped, view_w, host_w, edit_host, edit_scrolled, edit_buffer)) =
            info
        else {
            return;
        };
        if !editing || cur_gen != expected_gen {
            return;
        }
        // 若调用方/用户已在切入编辑态后显式移动光标或滚动位置，则不再回写旧锚点。
        if edit_buffer.cursor_position() != expected_cursor
            || (edit_scrolled.vadjustment().value() - initial_scroll).abs() > 2.0
        {
            return;
        }
        if (!mapped || view_w <= 0 || host_w <= 0) && attempts_left > 0 {
            let retry = move || {
                schedule_edit_scroll_sync(
                    weak_win,
                    rel,
                    anchor_u16,
                    expected_gen,
                    expected_cursor,
                    initial_scroll,
                    attempts_left - 1,
                );
            };
            if attempts_left > 6 {
                glib::idle_add_local_once(retry);
            } else {
                glib::timeout_add_local_once(Duration::from_millis(16), retry);
            }
            return;
        }
        if host_w > 0 {
            edit_host.sync_margins_for_width(host_w);
        }
        if anchor_u16 == 0 {
            let start_iter = edit_buffer.start_iter();
            edit_buffer.place_cursor(&start_iter);
            edit_scrolled.vadjustment().set_value(0.0);
        } else {
            win.scroll_to_utf16_offset(&rel, anchor_u16);
        }
    });
}

pub(crate) fn schedule_reading_scroll_restore(
    weak_win: std::rc::Weak<WindowInner>,
    rel: String,
    target: ReadingScrollTarget,
    expected_token: u64,
    last_upper: f64,
    stable_ticks: u8,
    attempts_left: u8,
) {
    glib::idle_add_local_once(move || {
        let Some(win) = weak_win.upgrade() else {
            return;
        };
        let reading = win
            .state
            .borrow()
            .tabs
            .get(&rel)
            .and_then(|tab| tab.reading.clone());
        let Some(reading) = reading else {
            return;
        };
        if reading.scroll_token() != expected_token {
            return;
        }
        let adj = reading.scrolled.vadjustment();
        let cur_upper = adj.upper();
        let unsettled_height = reading.content_box.height() <= 0 || cur_upper <= 0.0;
        let next_stable = if !unsettled_height && (cur_upper - last_upper).abs() <= 1.0 {
            stable_ticks.saturating_add(1)
        } else {
            0
        };
        if (unsettled_height || next_stable < 2) && attempts_left > 0 {
            glib::timeout_add_local_once(Duration::from_millis(24), move || {
                schedule_reading_scroll_restore(
                    weak_win,
                    rel,
                    target,
                    expected_token,
                    cur_upper,
                    next_stable,
                    attempts_left - 1,
                );
            });
            return;
        }
        match target {
            ReadingScrollTarget::Fraction(fraction) => reading.scroll_fraction(fraction),
            ReadingScrollTarget::SourceUtf16(u16_off) => reading.scroll_to_source_utf16(u16_off),
        }
    });
}
