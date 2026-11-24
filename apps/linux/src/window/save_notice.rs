// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 标签页标题、状态通知条（冲突/缺失/恢复/失败）、原子保存与竞态安全重载。

use crate::core::Utf16Mapper;
use crate::core::model::HeadingItem;
use crate::editor::save_sync::read_disk_stamp;
use crate::files::operations::{save_temp_for as resolve_save_temp_for, short_error};
use crate::knowledge::clear_box;
use crate::platform::style::apply_source_styles_with;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentIssue, DocumentPersistence, DocumentUpdate, SaveChoice};
use libadwaita as adw;
use libadwaita::prelude::*;
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
        let (notice, label, actions, issue, message) = {
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
                label.set_text(&format!(
                    "外部已修改「{path}」。未保存的修改仍完整保留在内存中，请选择处理方式。"
                ));
                row.add_css_class("hortus-notice-warning");
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "保留我的修改",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.request_save(&p, SaveChoice::KeepMine, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "载入磁盘版本",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.conflict_use_disk(&p);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "另存为冲突副本",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.request_save(&p, SaveChoice::ConflictCopy, true);
                        }
                    }),
                );
            }
            DocumentIssue::Missing => {
                label.set_text(&format!(
                    "磁盘文件已被删除或移走：「{path}」。可以重新保存到原路径，或关闭该标签页。"
                ));
                row.add_css_class("hortus-notice-error");
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "重新保存到原路径",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.request_save(&p, SaveChoice::Recreate, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "关闭标签页",
                    true,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.close_tab_by_path(&p);
                        }
                    }),
                );
            }
            DocumentIssue::Recovery => {
                label.set_text(&format!("发现上次未保存的恢复稿：「{path}」。"));
                row.add_css_class("hortus-notice-warning");
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "恢复草稿",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.recover_draft(&p, true);
                        }
                    }),
                );
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "丢弃恢复稿",
                    true,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
                            win.recover_draft(&p, false);
                        }
                    }),
                );
            }
            DocumentIssue::SaveFailed => {
                let detail = match message.as_str() {
                    "document.save_failed" => "磁盘写入失败".to_string(),
                    "readonly.no_write_permission" => "笔记或目录没有写权限".to_string(),
                    "readonly.invalid_utf8" => "文件不是有效的 UTF-8 文本".to_string(),
                    "" => "未知原因".to_string(),
                    other => other.to_string(),
                };
                label.set_text(&format!(
                    "保存失败：「{path}」（{detail}）。修改仍保留在内存中。"
                ));
                row.add_css_class("hortus-notice-error");
                let weak = Rc::downgrade(self);
                let p = path.to_string();
                add_button(
                    "重试",
                    false,
                    Box::new(move || {
                        if let Some(win) = weak.upgrade() {
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

    pub fn request_save(self: &Rc<Self>, path: &str, choice: SaveChoice, announce: bool) {
        let (session, edit_state, vault_root) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let Some(vault_root) = st.vault_root() else {
                return;
            };
            (session, tab.edit_state.clone(), vault_root)
        };
        let save_temp = self.save_temp_for(&vault_root, path);
        let saved_text = {
            let mut edit = edit_state.borrow_mut();
            if edit.save_inflight {
                return;
            }
            edit.save_inflight = true;
            edit.text.clone()
        };
        let weak = Rc::downgrade(self);
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let temp = save_temp.to_string_lossy().into_owned();
            let update = gio::spawn_blocking(move || session.save(temp, choice)).await;
            let Some(win) = weak.upgrade() else { return };
            let mut edit = edit_state.borrow_mut();
            edit.save_inflight = false;
            match update {
                Ok(Ok(update)) => {
                    drop(edit);
                    win.apply_save_update(&path, update, saved_text, announce);
                }
                Ok(Err(err)) => {
                    edit.issue = Some(DocumentIssue::SaveFailed);
                    edit.message = err.to_string();
                    drop(edit);
                    win.update_tab_notice(&path);
                }
                Err(_) => {
                    edit.issue = Some(DocumentIssue::SaveFailed);
                    edit.message = "document.save_failed".into();
                    drop(edit);
                    win.update_tab_notice(&path);
                }
            }
        });
    }

    fn apply_save_update(
        self: &Rc<Self>,
        path: &str,
        update: DocumentUpdate,
        saved_text: String,
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
        {
            let mut edit = edit_state.borrow_mut();
            edit.dirty = update.dirty;
            edit.issue = Some(update.issue);
            edit.message = update.message.clone();
            edit.revision = update.revision;
            edit.last_save_micros = update.save_micros;
            edit.saves += 1;
            edit.last_saved_at = Some(Instant::now());
        }
        let saved_clean = update.issue == DocumentIssue::None && !update.dirty;
        if saved_clean && let Some(handle) = self.state.borrow().vault_handle() {
            handle.upsert_note_content(path.to_string(), saved_text);
        }
        if let Some(copy) = update.copy_path.clone() {
            self.show_toast(&format!("已另存为冲突副本：{copy}"));
        } else if announce && saved_clean {
            self.show_toast("已保存");
        }
        self.update_tab_title(path);
        self.update_tab_notice(path);
        if saved_clean {
            self.update_disk_stamp(path);
            self.refresh_knowledge_after_write();
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
        let (session, edit_state) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                self.tab_view.close_page_finish(&page, true);
                return;
            };
            (tab.session.clone(), tab.edit_state.clone())
        };
        let needs_protection = session
            .as_ref()
            .map(|s| s.persistence() != DocumentPersistence::Persisted)
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
            let persisted = if let Some(session) = session {
                let vault_root = win.state.borrow().vault_root().unwrap_or_default();
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
                let persisted = update.persistence == DocumentPersistence::Persisted;
                let mut edit = edit_state.borrow_mut();
                edit.dirty = update.dirty;
                edit.issue = Some(update.issue);
                edit.message = update.message.clone();
                persisted
            } else {
                true
            };
            let Some(win) = weak.upgrade() else { return };
            if persisted {
                win.tab_view.close_page_finish(&page_bg, true);
            } else {
                win.ask_unsaved_close(&page_bg, &path_bg);
            }
        });
    }

    fn ask_unsaved_close(self: &Rc<Self>, page: &adw::TabPage, path: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("有未保存的修改")
            .body(format!(
                "「{path}」未能保存到磁盘（可能已被外部修改或路径不可写）。仍要关闭标签页吗？"
            ))
            .build();
        dialog.add_response("cancel", "取消");
        dialog.add_response("close", "放弃修改并关闭");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        {
            let mut st = self.state.borrow_mut();
            st.last_alert = Some(dialog.heading().unwrap_or_default().to_string());
            st.last_alert_dialog.set(Some(&dialog));
        }
        let weak = Rc::downgrade(self);
        let page = page.clone();
        dialog.connect_response(None, move |_, response| {
            if let Some(win) = weak.upgrade() {
                win.state.borrow_mut().last_alert_dialog.set(None);
                if response == "close" {
                    win.tab_view.close_page_finish(&page, true);
                } else {
                    win.tab_view.close_page_finish(&page, false);
                }
            }
        });
        dialog.present(Some(&self.window));
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
                    win.show_toast("已载入磁盘版本");
                }
                Ok(Err(err)) => win.show_toast(&format!("载入磁盘版本失败：{err}")),
                Err(_) => win.show_toast("载入磁盘版本线程异常"),
            }
        });
    }

    fn recover_draft(self: &Rc<Self>, path: &str, restore: bool) {
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
                Ok(Ok(_)) => {
                    win.reload_tab_from_session(&path, false);
                    win.show_toast(if restore {
                        "已恢复未保存草稿"
                    } else {
                        "已丢弃恢复草稿"
                    });
                }
                Ok(Err(err)) => win.show_toast(&short_error("处理恢复稿失败", &err.to_string())),
                Err(_) => win.show_toast("处理恢复稿线程异常"),
            }
        });
    }

    pub fn reload_tab_from_session(self: &Rc<Self>, path: &str, preserve_scroll: bool) {
        let (session, edit_state, edit_seq_before, fraction) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let fraction = if preserve_scroll {
                tab.reading.as_ref().and_then(|reading| {
                    let adj = reading.scrolled.vadjustment();
                    let max = adj.upper() - adj.page_size();
                    (max > 0.0).then(|| adj.value() / max)
                })
            } else {
                None
            };
            (
                tab.session.clone(),
                tab.edit_state.clone(),
                tab.edit_state.borrow().edit_seq,
                fraction,
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
                edit.issue = None;
                edit.dirty = false;
            }
            buffer.set_text(&text);
            let mapper = Utf16Mapper::new(&text);
            apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
            edit_state.borrow_mut().suppress = false;
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
                fraction.map(crate::window::reading_layout::ReadingScrollTarget::Fraction),
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
            {
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
                edit.issue = Some(issue);
                edit.message = update.message.clone();
                edit.dirty = update.dirty;
                edit.revision = revision;
                edit.refreshes += 1;
                edit.last_refresh_issue = Some(issue);
            }
            match issue {
                DocumentIssue::Conflict | DocumentIssue::Missing | DocumentIssue::SaveFailed => {
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
                                    win.show_toast("检测到外部修改并已刷新");
                                }
                                Ok(Err(err)) => win.show_toast(&format!("刷新笔记失败：{err}")),
                                Err(_) => win.show_toast("刷新笔记线程异常"),
                            }
                        }
                    } else {
                        win.update_tab_notice(&path);
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
