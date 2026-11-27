// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 编辑缓冲区实时同步、列表续写、节流样式重放、自动保存、检查点与文内动作。

use crate::core::Utf16Mapper;
use crate::core::model::HeadingItem;
use crate::core::{line_list_continuation, task_toggle_at};
use crate::platform::style::apply_source_styles_with;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentIssue, SaveChoice};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

impl WindowInner {
    pub fn bind_edit_sync(self: &Rc<Self>, rel: &str) {
        let (edit_buffer, edit_view, edit_state, session) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(rel) else {
                return;
            };
            (
                tab.edit_buffer.clone(),
                tab.edit_view.clone(),
                tab.edit_state.clone(),
                tab.session.clone(),
            )
        };
        let Some(session) = session else {
            return;
        };
        let mut edit = edit_state.borrow_mut();
        edit.suppress = false;
        let locked = edit.issue == Some(DocumentIssue::Recovery) || edit.read_only_reason.is_some();
        edit_view.set_editable(!locked);
        drop(edit);
        self.update_tab_notice(rel);

        {
            let weak = Rc::downgrade(self);
            let edit_state_ins = edit_state.clone();
            let session_ins = Arc::clone(&session);
            let weak_buf = edit_buffer.downgrade();
            let weak_view = edit_view.downgrade();
            edit_buffer.connect_insert_text(move |_, location, text| {
                let Some(win) = weak.upgrade() else { return };
                let mut edit = edit_state_ins.borrow_mut();
                if edit.suppress {
                    return;
                }
                let char_off = location.offset().max(0) as usize;
                let mapper = Utf16Mapper::new(&edit.text);
                let start_u16 = mapper.char_to_utf16(char_off) as u64;
                let byte = mapper.char_to_byte(char_off);
                let path_now = edit.path.clone();
                match session_ins.replace(start_u16, 0, text.to_string()) {
                    Ok(revision) => {
                        edit.text.insert_str(byte, text);
                        edit.edit_seq = edit.edit_seq.wrapping_add(1);
                        edit.revision = revision;
                        edit.dirty = true;
                        let prefix = if text == "\n" {
                            let before = &edit.text[..byte];
                            let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                            line_list_continuation(&before[line_start..])
                        } else {
                            None
                        };
                        let insert_at = char_off + text.chars().count();
                        drop(edit);
                        win.note_edited(&path_now);
                        if let Some(prefix) = prefix {
                            let weak_b = weak_buf.clone();
                            glib::idle_add_local_once(move || {
                                let Some(buffer) = weak_b.upgrade() else {
                                    return;
                                };
                                let caret = buffer.iter_at_mark(&buffer.get_insert()).offset();
                                if caret != insert_at as i32 {
                                    return;
                                }
                                buffer.begin_user_action();
                                buffer.insert(&mut buffer.iter_at_offset(caret), &prefix);
                                buffer.end_user_action();
                            });
                        }
                    }
                    Err(err) => {
                        let expected_text =
                            record_replace_error(&mut edit, &weak_view, &err.to_string());
                        drop(edit);
                        schedule_buffer_rollback(&win, &edit_state_ins, &weak_buf, expected_text);
                    }
                }
            });
        }

        {
            let weak = Rc::downgrade(self);
            let edit_state_del = edit_state.clone();
            let session_del = Arc::clone(&session);
            let weak_buf = edit_buffer.downgrade();
            let weak_view = edit_view.downgrade();
            edit_buffer.connect_delete_range(move |_, start, end| {
                let Some(win) = weak.upgrade() else { return };
                let mut edit = edit_state_del.borrow_mut();
                if edit.suppress {
                    return;
                }
                let char_start = start.offset().max(0) as usize;
                let char_end = end.offset().max(0) as usize;
                if char_end <= char_start {
                    return;
                }
                let mapper = Utf16Mapper::new(&edit.text);
                let start_u16 = mapper.char_to_utf16(char_start) as u64;
                let end_u16 = mapper.char_to_utf16(char_end) as u64;
                let byte_start = mapper.char_to_byte(char_start);
                let byte_end = mapper.char_to_byte(char_end);
                let path_now = edit.path.clone();
                match session_del.replace(start_u16, end_u16 - start_u16, String::new()) {
                    Ok(revision) => {
                        edit.text.replace_range(byte_start..byte_end, "");
                        edit.edit_seq = edit.edit_seq.wrapping_add(1);
                        edit.revision = revision;
                        edit.dirty = true;
                        drop(edit);
                        win.note_edited(&path_now);
                    }
                    Err(err) => {
                        let expected_text =
                            record_replace_error(&mut edit, &weak_view, &err.to_string());
                        drop(edit);
                        schedule_buffer_rollback(&win, &edit_state_del, &weak_buf, expected_text);
                    }
                }
            });
        }

        {
            let weak = Rc::downgrade(self);
            let click = gtk4::GestureClick::new();
            click.set_button(1);
            let weak_view = edit_view.downgrade();
            let edit_state_click = edit_state;
            click.connect_released(move |gesture, _, x, y| {
                let (Some(win), Some(view_c)) = (weak.upgrade(), weak_view.upgrade()) else {
                    return;
                };
                let (bx, by) = view_c.window_to_buffer_coords(
                    gtk4::TextWindowType::Widget,
                    x as i32,
                    y as i32,
                );
                let Some(iter) = view_c.iter_at_location(bx, by) else {
                    return;
                };
                let path_now = edit_state_click.borrow().path.clone();
                let Some((start, end, _)) =
                    win.task_toggle_span(&path_now, iter.offset().max(0) as usize)
                else {
                    return;
                };
                if iter.offset() >= start as i32 && iter.offset() <= end as i32 {
                    win.perform_toggle_task();
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                }
            });
            edit_view.add_controller(click);
        }
    }

    pub fn task_toggle_span(&self, path: &str, caret: usize) -> Option<(usize, usize, String)> {
        let text = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.edit_state.borrow().text.clone())?;
        task_toggle_at(&text, caret)
    }

    fn note_edited(self: &Rc<Self>, path: &str) {
        let is_active = self.active_tab_path().as_deref() == Some(path);
        let completion = if is_active {
            self.state
                .borrow()
                .tabs
                .get(path)
                .map(|tab| tab.completion.clone())
        } else {
            None
        };
        if let Some(completion) = completion {
            completion.hide();
        }
        self.update_tab_title(path);
        self.schedule_restyle(path);
        self.schedule_autosave(path);
    }

    fn schedule_restyle(self: &Rc<Self>, path: &str) {
        let (session, edit_state, generation) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let edit_state = tab.edit_state.clone();
            let generation = {
                let mut edit = edit_state.borrow_mut();
                if let Some(id) = edit.restyle_source.take() {
                    id.remove();
                }
                edit.restyle_gen += 1;
                edit.restyle_gen
            };
            (session, edit_state, generation)
        };
        let weak = Rc::downgrade(self);
        let edit_timer = edit_state.clone();
        let src_id = glib::timeout_add_local_once(Duration::from_millis(260), move || {
            edit_timer.borrow_mut().restyle_source = None;
            let Some(win) = weak.upgrade() else { return };
            let path_now = edit_timer.borrow().path.clone();
            let current = win
                .state
                .borrow()
                .tabs
                .get(&path_now)
                .map(|tab| tab.edit_state.borrow().restyle_gen);
            if current != Some(generation) {
                return;
            }
            let session_bg = Arc::clone(&session);
            let edit_state_bg = edit_timer.clone();
            let weak_inner = Rc::downgrade(&win);
            glib::MainContext::default().spawn_local(async move {
                let started = Instant::now();
                let result = gio::spawn_blocking(move || {
                    let styles = session_bg
                        .source_styles(u64::MAX, true)
                        .map_err(|e| e.to_string())?;
                    let outline = session_bg.outline().map_err(|e| e.to_string())?;
                    Ok::<_, String>((styles, outline))
                })
                .await;
                let (Some(win), Ok(Ok((styles, outline)))) = (weak_inner.upgrade(), result) else {
                    return;
                };
                if win.state.borrow().disposing {
                    return;
                }
                let (text, revision, path_now) = {
                    let edit = edit_state_bg.borrow();
                    (edit.text.clone(), edit.revision, edit.path.clone())
                };
                if styles.revision != revision {
                    return;
                }
                let buffer = {
                    let st = win.state.borrow();
                    let Some(tab) = st.tabs.get(&path_now) else {
                        return;
                    };
                    tab.edit_buffer.clone()
                };
                let mapper = Utf16Mapper::new(&text);
                let apply_started = Instant::now();
                apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
                if let Some(find) = win
                    .state
                    .borrow()
                    .tabs
                    .get(&path_now)
                    .map(|tab| tab.find.clone())
                {
                    find.refresh_highlights();
                }
                let apply_micros = apply_started.elapsed().as_micros() as u64;
                {
                    let mut edit = edit_state_bg.borrow_mut();
                    edit.restyles += 1;
                    edit.last_style_micros = styles
                        .analysis_micros
                        .max(apply_micros)
                        .max(started.elapsed().as_micros() as u64);
                }
                let headings: Vec<HeadingItem> = outline
                    .headings
                    .into_iter()
                    .map(HeadingItem::from)
                    .collect();
                let is_active = win.active_tab_path().as_deref() == Some(path_now.as_str());
                if let Some(tab) = win.state.borrow_mut().tabs.get_mut(&path_now) {
                    tab.headings = headings.clone();
                }
                if is_active {
                    win.render_outline(&path_now, &headings);
                }
            });
        });
        edit_state.borrow_mut().restyle_source = Some(src_id);
    }

    pub fn reapply_styles_for_tab(self: &Rc<Self>, path: &str) {
        let (session, edit_state) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            (tab.session.clone(), tab.edit_state.clone())
        };
        let Some(session) = session else { return };
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let styles = gio::spawn_blocking(move || session.source_styles(u64::MAX, true)).await;
            let (Some(win), Ok(Ok(styles))) = (weak.upgrade(), styles) else {
                return;
            };
            let (text, revision, path_now) = {
                let edit = edit_state.borrow();
                (edit.text.clone(), edit.revision, edit.path.clone())
            };
            if styles.revision != revision {
                return;
            }
            if let Some(buffer) = win
                .state
                .borrow()
                .tabs
                .get(&path_now)
                .map(|t| t.edit_buffer.clone())
            {
                let mapper = Utf16Mapper::new(&text);
                apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
            }
        });
    }

    fn schedule_autosave(self: &Rc<Self>, path: &str) {
        let (edit_state, generation) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit_state = tab.edit_state.clone();
            let generation = {
                let mut edit = edit_state.borrow_mut();
                if let Some(id) = edit.autosave_source.take() {
                    id.remove();
                }
                edit.autosave_gen += 1;
                edit.autosave_gen
            };
            (edit_state, generation)
        };
        let weak = Rc::downgrade(self);
        let edit_timer = edit_state.clone();
        let src_id = glib::timeout_add_local_once(Duration::from_millis(1000), move || {
            edit_timer.borrow_mut().autosave_source = None;
            let Some(win) = weak.upgrade() else { return };
            let path_now = edit_timer.borrow().path.clone();
            let (current, dirty) = {
                let st = win.state.borrow();
                let Some(tab) = st.tabs.get(&path_now) else {
                    return;
                };
                let edit = tab.edit_state.borrow();
                (edit.autosave_gen, edit.dirty)
            };
            if current != generation || !dirty {
                return;
            }
            win.request_save(&path_now, SaveChoice::Normal, false);
        });
        edit_state.borrow_mut().autosave_source = Some(src_id);
    }

    pub fn start_checkpoint_timer(self: &Rc<Self>, rel: &str) {
        let Some(edit_state) = self
            .state
            .borrow()
            .tabs
            .get(rel)
            .map(|tab| tab.edit_state.clone())
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        let edit_timer = edit_state.clone();
        let src_id = glib::timeout_add_seconds_local(4, move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if win.state.borrow().disposing {
                return glib::ControlFlow::Break;
            }
            let path_now = edit_timer.borrow().path.clone();
            let session = {
                let st = win.state.borrow();
                let Some(tab) = st.tabs.get(&path_now) else {
                    return glib::ControlFlow::Break;
                };
                let mut edit = tab.edit_state.borrow_mut();
                if !edit.dirty || edit.edit_seq == edit.last_checkpoint_seq {
                    return glib::ControlFlow::Continue;
                }
                edit.last_checkpoint_seq = edit.edit_seq;
                tab.session.clone()
            };
            if let Some(session) = session {
                std::thread::spawn(move || {
                    let _ = session.checkpoint();
                });
            }
            glib::ControlFlow::Continue
        });
        edit_state.borrow_mut().checkpoint_source = Some(src_id);
    }

    pub fn find_open(&self, replace: bool) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastNoNoteOpen));
            return;
        };
        let (editing, find, edit_view, edit_buffer, reading) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            (
                tab.editing,
                tab.find.clone(),
                tab.edit_view.clone(),
                tab.edit_buffer.clone(),
                tab.reading.clone(),
            )
        };
        if editing {
            find.bind(&edit_view, &edit_buffer, true);
            find.open(replace);
        } else if let Some(surface) = reading {
            find.bind_reading(&surface);
            find.open(false);
        } else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastReadingNotReady));
        }
    }

    pub fn perform_toggle_task(&self) {
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let (editing, buffer, text) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            (
                tab.editing,
                tab.edit_buffer.clone(),
                tab.edit_state.borrow().text.clone(),
            )
        };
        if !editing {
            return;
        }
        let caret = buffer.iter_at_mark(&buffer.get_insert()).offset().max(0) as usize;
        let Some((start_char, end_char, replacement)) = task_toggle_at(&text, caret) else {
            return;
        };
        let mut s_it = buffer.iter_at_offset(start_char as i32);
        let mut e_it = buffer.iter_at_offset(end_char as i32);
        buffer.begin_user_action();
        buffer.delete(&mut s_it, &mut e_it);
        buffer.insert(&mut s_it, &replacement);
        buffer.end_user_action();
    }

    pub fn perform_align_table(&self) {
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let (editing, buffer, session, text) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            (
                tab.editing,
                tab.edit_buffer.clone(),
                tab.session.clone(),
                tab.edit_state.borrow().text.clone(),
            )
        };
        if !editing {
            self.show_toast(crate::i18n::tr(
                crate::i18n::Key::ToastAlignTableRequiresEdit,
            ));
            return;
        }
        let Some(session) = session else { return };
        let caret_char = buffer.iter_at_mark(&buffer.get_insert()).offset().max(0) as usize;
        let mapper = Utf16Mapper::new(&text);
        let caret_u16 = mapper.char_to_utf16(caret_char) as u64;
        let Ok(Some(patch)) = session.align_table(caret_u16) else {
            return;
        };
        let start = mapper.utf16_to_char(patch.start as usize);
        let end = mapper.utf16_to_char((patch.start + patch.length) as usize);
        let mut s_it = buffer.iter_at_offset(start as i32);
        let mut e_it = buffer.iter_at_offset(end as i32);
        buffer.begin_user_action();
        buffer.delete(&mut s_it, &mut e_it);
        buffer.insert(&mut s_it, &patch.replacement);
        buffer.end_user_action();
    }
}

fn record_replace_error(
    edit: &mut crate::editor::EditState,
    weak_view: &glib::WeakRef<gtk4::TextView>,
    reason: &str,
) -> String {
    edit.issue = Some(DocumentIssue::SaveFailed);
    edit.message = reason.to_string();
    if reason.contains("read_only") || reason.contains("readonly") {
        edit.read_only_reason = Some(reason.to_string());
        if let Some(view) = weak_view.upgrade() {
            view.set_editable(false);
        }
    }
    edit.text.clone()
}

fn schedule_buffer_rollback(
    win: &Rc<WindowInner>,
    edit_state: &Rc<std::cell::RefCell<crate::editor::EditState>>,
    weak_buf: &glib::WeakRef<gtk4::TextBuffer>,
    expected_text: String,
) {
    let weak_win = Rc::downgrade(win);
    let edit_rb = Rc::clone(edit_state);
    let weak_b = weak_buf.clone();
    glib::idle_add_local_once(move || {
        if let Some(buf) = weak_b.upgrade() {
            edit_rb.borrow_mut().suppress = true;
            buf.set_text(&expected_text);
            edit_rb.borrow_mut().suppress = false;
        }
        if let Some(w) = weak_win.upgrade() {
            let p = edit_rb.borrow().path.clone();
            w.reapply_styles_for_tab(&p);
            w.update_tab_notice(&p);
            w.update_tab_title(&p);
        }
    });
}
