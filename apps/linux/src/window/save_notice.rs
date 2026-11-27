// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 标签页标题、状态通知条（冲突/缺失/恢复/失败）、原子保存与竞态安全重载。

use crate::core::Utf16Mapper;
use crate::core::model::HeadingItem;
use crate::editor::save_sync::read_disk_stamp;
use crate::files::operations::{
    resolve_save_temp, save_temp_for as resolve_save_temp_for, short_error,
};
use crate::knowledge::clear_box;
use crate::platform::style::apply_source_styles_with;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentIssue, DocumentPersistence, DocumentUpdate, SaveChoice};
use libadwaita as adw;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

impl WindowInner {
    pub fn update_tab_title(&self, path: &str) {
        let (page, base, dirty, issue) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit = tab.edit_state.borrow();
            let base = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string();
            (tab.page.clone(), base, edit.dirty, edit.issue)
        };
        let mut title = base;
        if dirty {
            title = format!("● {title}");
        }
        if matches!(
            issue,
            Some(DocumentIssue::Conflict | DocumentIssue::Missing | DocumentIssue::SaveFailed)
        ) {
            title = format!("{title} !");
        }
        page.set_title(&title);
    }

    pub fn update_tab_notice(self: &Rc<Self>, path: &str) {
        let (notice, label, actions, issue, message, edit_state) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit = tab.edit_state.borrow();
            (
                tab.notice.clone(),
                tab.notice_label.clone(),
                tab.notice_actions.clone(),
                edit.issue,
                edit.message.clone(),
                tab.edit_state.clone(),
            )
        };
        clear_box(&actions);
        let Some(row) = label.parent().and_downcast::<gtk4::Box>() else {
            return;
        };
        row.remove_css_class("hortus-notice-error");
        row.remove_css_class("hortus-notice-warning");
        let issue = match issue {
            Some(DocumentIssue::None) | None => {
                notice.set_reveal_child(false);
                notice.set_visible(false);
                return;
            }
            Some(other) => other,
        };
        let add_button = |text: &str, destructive: bool, handler: Box<dyn Fn()>| {
            let btn = gtk4::Button::with_label(text);
            btn.add_css_class("flat");
            btn.add_css_class("caption");
            if destructive {
                btn.add_css_class("destructive-action");
            }
            btn.connect_clicked(move |_| handler());
            actions.append(&btn);
        };
        match issue {
            DocumentIssue::Conflict => {
                label.set_text(&crate::i18n::fmt1(crate::i18n::Key::NoticeConflict, path));
                row.add_css_class("hortus-notice-warning");
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnKeepMine),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.request_save(&p, SaveChoice::KeepMine, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnUseDisk),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.conflict_use_disk(&p);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnConflictCopy),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.request_save(&p, SaveChoice::ConflictCopy, true);
                        }
                    }),
                );
            }
            DocumentIssue::Missing => {
                label.set_text(&crate::i18n::fmt1(crate::i18n::Key::NoticeMissing, path));
                row.add_css_class("hortus-notice-error");
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnRecreate),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.request_save(&p, SaveChoice::Recreate, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnCloseTab),
                    true,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.close_tab_by_path(&p);
                        }
                    }),
                );
            }
            DocumentIssue::Recovery => {
                label.set_text(&crate::i18n::fmt1(crate::i18n::Key::NoticeRecovery, path));
                row.add_css_class("hortus-notice-warning");
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnRestoreDraft),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.recover_draft(&p, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let es = edit_state.clone();
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnDiscardDraft),
                    true,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.recover_draft(&p, false);
                        }
                    }),
                );
            }
            DocumentIssue::SaveFailed => {
                let detail = match message.as_str() {
                    "document.save_failed" => crate::i18n::tr(crate::i18n::Key::SaveErrWrite),
                    "readonly.no_write_permission" | "document.read_only" => {
                        crate::i18n::tr(crate::i18n::Key::SaveErrPermission)
                    }
                    "readonly.invalid_utf8" => crate::i18n::tr(crate::i18n::Key::SaveErrUtf8),
                    "note.different_volume" => crate::i18n::tr(crate::i18n::Key::SaveErrMountRoot),
                    "" => crate::i18n::tr(crate::i18n::Key::SaveErrUnknown),
                    other => other,
                };
                label.set_text(&crate::i18n::fmt2(
                    crate::i18n::Key::NoticeSaveFailed,
                    path,
                    detail,
                ));
                row.add_css_class("hortus-notice-error");
                let weak = Rc::downgrade(self);
                let es = edit_state;
                add_button(
                    crate::i18n::tr(crate::i18n::Key::BtnRetry),
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            let p = es.borrow().path.clone();
                            win.request_save(&p, SaveChoice::Normal, true);
                        }
                    }),
                );
            }
            DocumentIssue::None => {}
        }
        notice.set_visible(true);
        notice.set_reveal_child(true);
    }

    pub fn save_temp_for(&self, vault_root: &str, rel: &str) -> PathBuf {
        resolve_save_temp_for(&self.state.borrow().temp_dir, vault_root, rel)
    }

    pub fn save_temp_info(&self, vault_root: &str, rel: &str) -> (PathBuf, bool) {
        resolve_save_temp(&self.state.borrow().temp_dir, vault_root, rel)
    }

    pub fn request_save(self: &Rc<Self>, path: &str, choice: SaveChoice, announce: bool) {
        let (session, edit_state, vault_root) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let vault_root = if !tab.vault_root.is_empty() {
                tab.vault_root.clone()
            } else {
                let Some(root) = st.vault_root() else {
                    return;
                };
                root
            };
            (session, tab.edit_state.clone(), vault_root)
        };
        let (save_temp, same_volume) = self.save_temp_info(&vault_root, path);
        let (saved_text, saved_seq) = {
            let mut edit = edit_state.borrow_mut();
            if edit.save_inflight {
                edit.save_queued = Some((choice, announce));
                return;
            }
            edit.save_inflight = true;
            edit.save_queued = None;
            (edit.text.clone(), edit.edit_seq)
        };
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let temp = save_temp.to_string_lossy().into_owned();
            let update = gio::spawn_blocking(move || session.save(temp, choice)).await;
            let Some(win) = weak.upgrade() else { return };
            let path_now = edit_state.borrow().path.clone();
            let queued = {
                let mut edit = edit_state.borrow_mut();
                edit.save_inflight = false;
                edit.save_queued.take()
            };
            match update {
                Ok(Ok(mut update)) => {
                    if !same_volume
                        && update.issue == DocumentIssue::SaveFailed
                        && update.message == "document.save_failed"
                    {
                        update.message = "note.different_volume".into();
                    }
                    let seq_matches = edit_state.borrow().edit_seq == saved_seq;
                    win.apply_save_update(&path_now, update, saved_text, saved_seq, announce);
                    if let Some((q_choice, q_announce)) = queued {
                        win.request_save(&path_now, q_choice, q_announce);
                    } else if !seq_matches {
                        win.request_save(&path_now, SaveChoice::Normal, false);
                    }
                }
                Ok(Err(err)) => {
                    {
                        let mut edit = edit_state.borrow_mut();
                        edit.issue = Some(DocumentIssue::SaveFailed);
                        edit.message = err.to_string();
                    }
                    win.update_tab_notice(&path_now);
                    if let Some((q_choice, q_announce)) = queued {
                        win.request_save(&path_now, q_choice, q_announce);
                    }
                }
                Err(_) => {
                    {
                        let mut edit = edit_state.borrow_mut();
                        edit.issue = Some(DocumentIssue::SaveFailed);
                        edit.message = "document.save_failed".into();
                    }
                    win.update_tab_notice(&path_now);
                    if let Some((q_choice, q_announce)) = queued {
                        win.request_save(&path_now, q_choice, q_announce);
                    }
                }
            }
        });
    }

    fn apply_save_update(
        self: &Rc<Self>,
        path: &str,
        update: DocumentUpdate,
        saved_text: String,
        saved_seq: u64,
        announce: bool,
    ) {
        let Some(edit_state) = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.edit_state.clone())
        else {
            return;
        };
        let seq_matches = {
            let mut edit = edit_state.borrow_mut();
            let seq_matches = edit.edit_seq == saved_seq;
            if seq_matches {
                edit.dirty = update.dirty;
                edit.revision = update.revision;
            } else {
                edit.dirty = true;
            }
            edit.issue = Some(update.issue);
            edit.message = update.message.clone();
            edit.last_save_micros = update.save_micros;
            edit.saves += 1;
            edit.last_saved_at = Some(Instant::now());
            seq_matches
        };
        let saved_clean = seq_matches && update.issue == DocumentIssue::None && !update.dirty;
        if saved_clean && let Some(handle) = self.state.borrow().vault_handle() {
            handle.upsert_note_content(path.to_string(), saved_text);
        }
        if let Some(copy) = update.copy_path.clone() {
            self.show_toast(&crate::i18n::fmt1(
                crate::i18n::Key::ToastConflictCopySaved,
                copy,
            ));
        } else if update.issue == DocumentIssue::SaveFailed
            && update.message == "note.different_volume"
        {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::SaveErrMountRoot));
        } else if announce && saved_clean {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSaved));
        }
        self.update_tab_title(path);
        self.update_tab_notice(path);
        if saved_clean {
            self.update_disk_stamp(path);
            self.refresh_knowledge_after_write();
            self.notify_sibling_windows_written(&[path.to_string()]);
        }
    }

    fn path_for_page(&self, page: &adw::TabPage) -> Option<String> {
        self.state
            .borrow()
            .tabs
            .values()
            .find(|tab| &tab.page == page)
            .map(|tab| tab.path.clone())
    }

    pub fn close_tab_by_path(&self, path: &str) {
        let page = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.page.clone());
        if let Some(page) = page {
            self.tab_view.close_page(&page);
        }
    }

    pub fn request_close_tab(self: &Rc<Self>, page: adw::TabPage) {
        let Some(path) = self.path_for_page(&page) else {
            self.tab_view.close_page_finish(&page, true);
            return;
        };
        if self.state.borrow_mut().force_close.remove(&path) {
            self.tab_view.close_page_finish(&page, true);
            return;
        }
        let (session, edit_state, tab_vault_root) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                self.tab_view.close_page_finish(&page, true);
                return;
            };
            let root = if !tab.vault_root.is_empty() {
                tab.vault_root.clone()
            } else {
                st.vault_root().unwrap_or_default()
            };
            (tab.session.clone(), tab.edit_state.clone(), root)
        };
        let needs_protection = session
            .as_ref()
            .map(|s| s.persistence() == DocumentPersistence::Unsaved)
            .unwrap_or(false)
            || edit_state.borrow().dirty;
        if !needs_protection {
            self.tab_view.close_page_finish(&page, true);
            return;
        }
        let weak = Rc::downgrade(self);
        let page_bg = page;
        let path_bg = path;
        glib::MainContext::default().spawn_local(async move {
            let Some(win) = weak.upgrade() else { return };
            let can_close = if let Some(session) = session {
                let vault_root = if !tab_vault_root.is_empty() {
                    tab_vault_root
                } else {
                    win.state.borrow().vault_root().unwrap_or_default()
                };
                let temp = if vault_root.is_empty() {
                    std::env::temp_dir()
                } else {
                    win.save_temp_for(&vault_root, &path_bg)
                };
                let temp = temp.to_string_lossy().into_owned();
                let fallback = Arc::clone(&session);
                let update = gio::spawn_blocking(move || session.prepare_close(temp, true))
                    .await
                    .unwrap_or_else(|_| fallback.failed_save("document.save_failed".into()));
                let can_close = update.persistence != DocumentPersistence::Unsaved;
                let mut edit = edit_state.borrow_mut();
                edit.dirty = update.dirty;
                edit.issue = Some(update.issue);
                edit.message = update.message.clone();
                can_close
            } else {
                true
            };
            let Some(win) = weak.upgrade() else { return };
            if can_close {
                win.tab_view.close_page_finish(&page_bg, true);
            } else {
                win.ask_unsaved_close(&page_bg, &path_bg);
            }
        });
    }

    fn conflict_use_disk(self: &Rc<Self>, path: &str) {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        };
        let Some(session) = session else { return };
        let weak = Rc::downgrade(self);
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let session_bg = Arc::clone(&session);
            let result = gio::spawn_blocking(move || {
                let proposed = session_bg.use_disk()?;
                session_bg.accept_external(proposed.revision)
            })
            .await;
            let Some(win) = weak.upgrade() else { return };
            match result {
                Ok(Ok(_)) => {
                    win.reload_tab_from_session(&path, true);
                    win.show_toast(crate::i18n::tr(crate::i18n::Key::ToastLoadedDisk));
                }
                Ok(Err(err)) => win.show_toast(&crate::i18n::fmt1(
                    crate::i18n::Key::ToastLoadDiskFailed,
                    err,
                )),
                Err(_) => win.show_toast(crate::i18n::tr(crate::i18n::Key::ErrThread)),
            }
        });
    }

    pub fn recover_draft(self: &Rc<Self>, path: &str, restore: bool) {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        };
        let Some(session) = session else { return };
        let weak = Rc::downgrade(self);
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let update = gio::spawn_blocking(move || session.recover(restore)).await;
            let Some(win) = weak.upgrade() else { return };
            match update {
                Ok(Ok(update)) => {
                    let ro = update.read_only_reason.clone();
                    if let Some(tab) = win.state.borrow().tabs.get(&path) {
                        tab.edit_state.borrow_mut().read_only_reason = ro.clone();
                        tab.edit_view.set_editable(ro.is_none());
                    }
                    win.reload_tab_from_session(&path, false);
                    win.show_toast(if restore {
                        crate::i18n::tr(crate::i18n::Key::ToastDraftRestored)
                    } else {
                        crate::i18n::tr(crate::i18n::Key::ToastDraftDiscarded)
                    });
                }
                Ok(Err(err)) => win.show_toast(&short_error(
                    crate::i18n::tr(crate::i18n::Key::ToastDraftFailed),
                    &err.to_string(),
                )),
                Err(_) => win.show_toast(crate::i18n::tr(crate::i18n::Key::ErrThread)),
            }
        });
    }

    pub fn reload_tab_from_session(self: &Rc<Self>, path: &str, preserve_scroll: bool) {
        let (session, edit_state, edit_seq_before, anchor) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let anchor = crate::editor::save_sync::capture_edit_reload_anchor(
                preserve_scroll,
                tab.reading.as_ref().map(|r| &r.scrolled),
                tab.editing,
                &tab.edit_scrolled,
                &tab.edit_buffer,
            );
            (
                tab.session.clone(),
                tab.edit_state.clone(),
                tab.edit_state.borrow().edit_seq,
                anchor,
            )
        };
        let Some(session) = session else { return };
        let weak = Rc::downgrade(self);
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let session_bg = Arc::clone(&session);
            let result = gio::spawn_blocking(move || {
                let snapshot = session_bg.snapshot().map_err(|e| e.to_string())?;
                let styles = session_bg
                    .source_styles(u64::MAX, true)
                    .map_err(|e| e.to_string())?;
                let outline = session_bg.outline().map_err(|e| e.to_string())?;
                let revision = snapshot.revision;
                Ok::<_, String>((snapshot.text, revision, styles, outline))
            })
            .await;
            let (Some(win), Ok(Ok((text, revision, styles, outline)))) = (weak.upgrade(), result)
            else {
                return;
            };
            if win.state.borrow().disposing {
                return;
            }
            let (buffer, seq_now) = {
                let st = win.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                (tab.edit_buffer.clone(), tab.edit_state.borrow().edit_seq)
            };
            if seq_now != edit_seq_before {
                return;
            }
            {
                let mut edit = edit_state.borrow_mut();
                edit.suppress = true;
                edit.text = text.clone();
                edit.revision = revision;
                if let Some(reason) = edit.read_only_reason.clone() {
                    edit.issue = Some(DocumentIssue::SaveFailed);
                    edit.message = reason;
                } else {
                    edit.issue = None;
                    edit.message.clear();
                }
                edit.dirty = false;
            }
            buffer.set_text(&text);
            let mapper = Utf16Mapper::new(&text);
            apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
            edit_state.borrow_mut().suppress = false;
            let locked = {
                let edit = edit_state.borrow();
                edit.issue == Some(DocumentIssue::Recovery) || edit.read_only_reason.is_some()
            };
            if let Some(tab) = win.state.borrow().tabs.get(&path) {
                tab.edit_view.set_editable(!locked);
                crate::editor::save_sync::restore_edit_reload_anchor(
                    anchor,
                    &tab.edit_scrolled,
                    &tab.edit_buffer,
                );
            }
            if let Some(find) = win
                .state
                .borrow()
                .tabs
                .get(&path)
                .map(|tab| tab.find.clone())
            {
                find.refresh_highlights();
            }
            let headings: Vec<HeadingItem> = outline
                .headings
                .into_iter()
                .map(HeadingItem::from)
                .collect();
            let is_active = win.active_tab_path().as_deref() == Some(path.as_str());
            if let Some(tab) = win.state.borrow_mut().tabs.get_mut(&path) {
                tab.text = text;
                tab.headings = headings.clone();
                tab.edit_state.borrow_mut().reading_reloads += 1;
            }
            if is_active {
                win.render_outline(&path, &headings);
            }
            win.refresh_reading_tab(
                &path,
                anchor
                    .reading_fraction
                    .map(crate::window::reading_layout::ReadingScrollTarget::Fraction),
            );
            win.update_disk_stamp(&path);
            win.update_tab_title(&path);
            win.update_tab_notice(&path);
        });
    }

    pub fn refresh_document_tab(self: &Rc<Self>, path: &str) {
        let (session, dirty_before, refresh_gen) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let mut edit = tab.edit_state.borrow_mut();
            edit.refresh_gen = edit.refresh_gen.wrapping_add(1);
            (tab.session.clone(), edit.dirty, edit.refresh_gen)
        };
        let Some(session) = session else { return };
        let weak = Rc::downgrade(self);
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let update = gio::spawn_blocking(move || session.refresh()).await;
            let (Some(win), Ok(Ok(update))) = (weak.upgrade(), update) else {
                return;
            };
            if win.state.borrow().disposing {
                return;
            }
            let issue = update.issue;
            let external_text = update.text.clone();
            let revision = update.revision;
            let read_only_reason = update.read_only_reason.clone();
            let (locked, effective_issue) = {
                let st = win.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                let mut edit = tab.edit_state.borrow_mut();
                if edit.refresh_gen != refresh_gen {
                    return;
                }
                if dirty_before && !edit.dirty && issue == DocumentIssue::Conflict {
                    return;
                }
                edit.read_only_reason = read_only_reason.clone();
                let effective_issue = if issue == DocumentIssue::None {
                    if let Some(ref reason) = read_only_reason {
                        edit.message = reason.clone();
                        DocumentIssue::SaveFailed
                    } else {
                        edit.message = update.message.clone();
                        DocumentIssue::None
                    }
                } else {
                    edit.message = update.message.clone();
                    issue
                };
                edit.issue = Some(effective_issue);
                edit.dirty = update.dirty;
                edit.revision = revision;
                edit.refreshes += 1;
                edit.last_refresh_issue = Some(effective_issue);
                let locked =
                    effective_issue == DocumentIssue::Recovery || read_only_reason.is_some();
                tab.edit_view.set_editable(!locked);
                (locked, effective_issue)
            };
            let _ = locked;
            match effective_issue {
                DocumentIssue::Conflict
                | DocumentIssue::Missing
                | DocumentIssue::Recovery
                | DocumentIssue::SaveFailed => {
                    win.update_tab_notice(&path);
                    win.update_tab_title(&path);
                }
                _ => {
                    if external_text.is_some() {
                        let session = win
                            .state
                            .borrow()
                            .tabs
                            .get(&path)
                            .and_then(|tab| tab.session.clone());
                        if let Some(session) = session {
                            let accepted =
                                gio::spawn_blocking(move || session.accept_external(revision))
                                    .await;
                            let Some(win) = weak.upgrade() else { return };
                            match accepted {
                                Ok(Ok(_)) => {
                                    win.reload_tab_from_session(&path, true);
                                    win.show_toast(crate::i18n::tr(
                                        crate::i18n::Key::ToastExternalRefreshed,
                                    ));
                                }
                                Ok(Err(err)) => win.show_toast(&crate::i18n::fmt1(
                                    crate::i18n::Key::ToastRefreshFailed,
                                    err,
                                )),
                                Err(_) => {
                                    win.show_toast(crate::i18n::tr(crate::i18n::Key::ErrThread))
                                }
                            }
                        }
                    } else {
                        win.update_tab_notice(&path);
                        win.update_tab_title(&path);
                    }
                }
            }
        });
    }

    pub fn update_disk_stamp(&self, path: &str) {
        let Some(root) = self.state.borrow().vault_root() else {
            return;
        };
        let stamp = read_disk_stamp(&root, path);
        let st = self.state.borrow();
        if let Some(tab) = st.tabs.get(path) {
            tab.edit_state.borrow_mut().disk_stamp = stamp;
        }
    }
}
