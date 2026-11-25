// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 知识库文件树右键菜单、新建、重命名改链、回收站及监听批次防抖处理。

use crate::core::is_note_file_name;
use crate::files;
#[cfg(feature = "automation")]
use crate::files::operations::count_dir_files;
use crate::files::operations::short_error;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentPersistence, TreeBatch};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

impl WindowInner {
    pub fn maybe_refresh_tab(self: &Rc<Self>, path: &str) {
        let (session, should_refresh) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit = tab.edit_state.borrow();
            let recent = edit
                .last_saved_at
                .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
            (tab.session.clone(), !edit.save_inflight && !recent)
        };
        if session.is_some() && should_refresh {
            self.refresh_document_tab(path);
        }
    }

    pub fn handle_vault_batches(self: &Rc<Self>, batches: &[TreeBatch]) {
        let mut changed = Vec::new();
        for batch in batches {
            for entry in &batch.upserted {
                if !entry.directory && is_note_file_name(&entry.path) {
                    changed.push(entry.path.clone());
                }
            }
            changed.extend(batch.removed.iter().cloned());
        }
        if changed.is_empty() {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            if st.files_busy || st.disposing {
                return;
            }
            st.pending_file_changes.extend(changed);
            if st.file_change_source.is_some() {
                return;
            }
        }
        let weak = Rc::downgrade(self);
        let src_id = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let Some(win) = weak.upgrade() else { return };
            let (paths, busy, disposing) = {
                let mut st = win.state.borrow_mut();
                st.file_change_source = None;
                (
                    std::mem::take(&mut st.pending_file_changes),
                    st.files_busy,
                    st.disposing,
                )
            };
            if busy || disposing {
                return;
            }
            win.process_file_changes(&paths);
        });
        self.state.borrow_mut().file_change_source = Some(src_id);
    }

    pub fn process_file_changes(self: &Rc<Self>, paths: &[String]) {
        let mut seen = HashSet::new();
        for path in paths {
            if !seen.insert(path.as_str()) {
                continue;
            }
            self.maybe_refresh_tab(path);
        }
        let (open_paths, vault_root) = {
            let st = self.state.borrow();
            (st.tabs.keys().cloned().collect::<Vec<_>>(), st.vault_root())
        };
        for rel in open_paths {
            if seen.contains(rel.as_str()) {
                continue;
            }
            let missing = vault_root
                .as_deref()
                .is_some_and(|root| !root.is_empty() && !Path::new(root).join(&rel).exists());
            if missing {
                self.maybe_refresh_tab(&rel);
            }
        }
        self.tags_panel.refresh();
        self.search_panel.rerun_now();
        self.refresh_backlinks(true);
    }

    pub fn install_tree_context_menu(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.vault_tree
            .set_on_context_menu(move |widget, path, is_dir| {
                if let Some(win) = weak.upgrade() {
                    win.show_tree_context_menu(widget, &path, is_dir);
                }
            });
    }

    fn show_tree_context_menu(self: &Rc<Self>, anchor: gtk4::Widget, path: &str, is_dir: bool) {
        let popover = gtk4::Popover::new();
        popover.set_has_arrow(true);
        popover.set_parent(&anchor);
        let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        box_v.set_margin_top(4);
        box_v.set_margin_bottom(4);
        box_v.set_margin_start(4);
        box_v.set_margin_end(4);
        let parent_dir = if is_dir {
            path.to_string()
        } else {
            path.rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default()
        };
        let add_item = |label: &str, handler: Box<dyn Fn()>| {
            let btn = gtk4::Button::with_label(label);
            btn.add_css_class("flat");
            if let Some(child) = btn.child().and_downcast::<gtk4::Label>() {
                child.set_xalign(0.0);
            }
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                handler();
            });
            box_v.append(&btn);
        };
        let weak = Rc::downgrade(self);
        let pd = parent_dir.clone();
        add_item(
            crate::i18n::tr(crate::i18n::Key::TreeNewNoteHere),
            Box::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.prompt_new_entry_in(&pd, false);
                }
            }),
        );
        let weak = Rc::downgrade(self);
        let pd = parent_dir;
        add_item(
            crate::i18n::tr(crate::i18n::Key::TreeNewFolderHere),
            Box::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.prompt_new_entry_in(&pd, true);
                }
            }),
        );
        let weak = Rc::downgrade(self);
        let p = path.to_string();
        add_item(
            crate::i18n::tr(crate::i18n::Key::TreeRename),
            Box::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.begin_rename(&p, is_dir);
                }
            }),
        );
        let weak = Rc::downgrade(self);
        let p = path.to_string();
        add_item(
            crate::i18n::tr(crate::i18n::Key::TreeMoveToTrash),
            Box::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.prompt_trash(&p, is_dir);
                }
            }),
        );
        let weak = Rc::downgrade(self);
        let p = path.to_string();
        add_item(
            crate::i18n::tr(crate::i18n::Key::TreeRevealInFileManager),
            Box::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.reveal_in_file_manager(&p);
                }
            }),
        );
        popover.set_child(Some(&box_v));
        popover.connect_closed(|p| p.unparent());
        popover.popup();
    }

    fn current_parent_dir(&self) -> String {
        if let Some((dir, _)) = self.vault_tree.selected_directory() {
            return dir;
        }
        self.active_tab_path()
            .and_then(|p| p.rsplit_once('/').map(|(parent, _)| parent.to_string()))
            .unwrap_or_default()
    }

    pub fn prompt_new_entry(self: &Rc<Self>, directory: bool) {
        let parent = self.current_parent_dir();
        self.prompt_new_entry_in(&parent, directory);
    }

    fn prompt_new_entry_in(self: &Rc<Self>, parent: &str, directory: bool) {
        let Some(handle) = self.state.borrow().vault_handle() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenFolderFirst));
            return;
        };
        let weak = Rc::downgrade(self);
        files::dialogs::prompt_new_entry(
            &self.window,
            handle,
            parent.to_string(),
            directory,
            Rc::new(move |res| match res {
                Ok(entry) => {
                    if let Some(win) = weak.upgrade() {
                        win.after_create_entry(entry);
                    }
                }
                Err(err) => {
                    if let Some(win) = weak.upgrade() {
                        win.show_toast(&short_error(
                            crate::i18n::tr(crate::i18n::Key::CreateFailed),
                            &err,
                        ));
                    }
                }
            }),
        );
    }

    pub fn after_create_entry(self: &Rc<Self>, entry: hortus_ffi::TreeEntry) {
        if let Some(handle) = self.state.borrow().vault_handle()
            && !entry.directory
            && is_note_file_name(&entry.path)
        {
            handle.update_search_index(vec![entry.path.clone()], Vec::new());
        }
        if !entry.directory && is_note_file_name(&entry.path) {
            self.open_note_at(&entry.path, None);
        } else {
            self.show_toast(&crate::i18n::fmt1(
                crate::i18n::Key::ToastFolderCreated,
                &entry.name,
            ));
        }
        self.refresh_knowledge_after_write();
    }

    pub fn begin_rename(self: &Rc<Self>, path: &str, is_dir: bool) {
        if self.state.borrow().files_busy {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastFileOpBusy));
            return;
        }
        let Some(handle) = self.state.borrow().vault_handle() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenFolderFirst));
            return;
        };
        let weak = Rc::downgrade(self);
        let old = path.to_string();
        files::dialogs::prompt_rename(
            &self.window,
            handle,
            old.clone(),
            is_dir,
            None,
            Rc::new(move |new_name, update_links| {
                if let Some(win) = weak.upgrade() {
                    win.perform_rename(&old, &new_name, update_links, is_dir);
                }
            }),
        );
    }

    fn affected_open_tabs(&self, path: &str) -> Vec<String> {
        let prefix = format!("{path}/");
        self.state
            .borrow()
            .tabs
            .keys()
            .filter(|open| *open == path || open.starts_with(&prefix))
            .cloned()
            .collect()
    }

    pub fn perform_rename(
        self: &Rc<Self>,
        old_path: &str,
        new_name: &str,
        update_links: bool,
        is_dir: bool,
    ) {
        let (services, handle, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_handle(), st.vault_root())
        };
        let (Some(services), Some(handle), Some(vault_root)) = (services, handle, vault_root)
        else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenFolderFirst));
            return;
        };
        let parent = old_path
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        let new_path = if parent.is_empty() {
            new_name.to_string()
        } else {
            format!("{parent}/{new_name}")
        };
        if new_path == old_path {
            return;
        }
        let affected = self.affected_open_tabs(old_path);
        let sessions: Vec<(String, Arc<hortus_ffi::DocumentSession>, String)> = {
            let st = self.state.borrow();
            affected
                .iter()
                .filter_map(|p| {
                    st.tabs.get(p).and_then(|tab| {
                        tab.session.as_ref().map(|session| {
                            let temp = self.save_temp_for(&vault_root, p);
                            (
                                p.clone(),
                                Arc::clone(session),
                                temp.to_string_lossy().into_owned(),
                            )
                        })
                    })
                })
                .collect()
        };
        self.state.borrow_mut().files_busy = true;
        let weak = Rc::downgrade(self);
        let old_path = old_path.to_string();
        let name_owned = new_name.to_string();
        let temp_bg = self
            .save_temp_for(&vault_root, &old_path)
            .to_string_lossy()
            .into_owned();
        let core_bg = Arc::clone(&services.core);
        glib::MainContext::default().spawn_local(async move {
            let old_bg = old_path.clone();
            let new_bg = new_path.clone();
            let name_bg = name_owned;
            let root_bg = vault_root;
            let handle_bg = Arc::clone(&handle);
            let result = gio::spawn_blocking(move || {
                for (path, session, temp) in &sessions {
                    let update = session.prepare_close(temp.clone(), true);
                    if update.persistence != DocumentPersistence::Persisted {
                        return Err(format!("rename.unsaved:{path}"));
                    }
                }
                let entry = handle_bg
                    .rename_entry(old_bg.clone(), name_bg)
                    .map_err(|e| e.to_string())?;
                let mut rewrites = 0u64;
                let mut updated_notes = 0usize;
                if update_links {
                    let _ = if is_dir {
                        handle_bg.plan_folder_rename_links(old_bg.clone(), entry.path.clone())
                    } else {
                        handle_bg.plan_rename_links(old_bg.clone(), entry.path.clone())
                    };
                    let applied = if is_dir {
                        handle_bg.apply_folder_rename_links_in(
                            old_bg.clone(),
                            entry.path.clone(),
                            Vec::new(),
                            temp_bg,
                        )
                    } else {
                        handle_bg.apply_rename_links_in(
                            old_bg.clone(),
                            entry.path.clone(),
                            Vec::new(),
                            temp_bg,
                        )
                    }
                    .map_err(|e| e.to_string())?;
                    rewrites = applied.total_rewrites;
                    updated_notes = applied.updated.len();
                }
                handle_bg.update_search_index(vec![entry.path.clone()], vec![old_bg.clone()]);
                let _ = core_bg.rename_diagram_state(
                    root_bg,
                    old_bg.clone(),
                    entry.path.clone(),
                    is_dir,
                );
                let mut moved = Vec::new();
                for (path, session, _) in &sessions {
                    let new_rel = if *path == old_bg {
                        new_bg.clone()
                    } else {
                        let suffix = path.strip_prefix(&old_bg).unwrap_or("");
                        format!("{new_bg}{suffix}")
                    };
                    let _ = session.relocate(new_rel.clone());
                    moved.push((path.clone(), new_rel));
                }
                Ok::<_, String>((entry, rewrites, updated_notes, moved))
            })
            .await;
            let Some(win) = weak.upgrade() else { return };
            win.state.borrow_mut().files_busy = false;
            match result {
                Ok(Ok((entry, rewrites, updated_notes, moved))) => {
                    let mut moved_tabs = Vec::new();
                    {
                        let mut st = win.state.borrow_mut();
                        for (old, new) in &moved {
                            if let Some(mut tab) = st.tabs.remove(old) {
                                tab.path = new.clone();
                                tab.edit_state.borrow_mut().path = new.clone();
                                tab.page.set_tooltip(new);
                                st.tabs.insert(new.clone(), tab);
                                moved_tabs.push((old.clone(), new.clone()));
                            }
                        }
                    }
                    for (_, new) in &moved_tabs {
                        win.update_tab_title(new);
                        win.refresh_document_tab(new);
                    }
                    if let Some(h) = win.state.borrow().vault_handle() {
                        let removed: Vec<String> =
                            moved_tabs.iter().map(|(old, _)| old.clone()).collect();
                        let added: Vec<String> =
                            moved_tabs.iter().map(|(_, new)| new.clone()).collect();
                        h.update_search_index(added, removed);
                    }
                    win.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "rename",
                        "old": old_path,
                        "new": entry.path,
                        "is_dir": is_dir,
                        "update_links": update_links,
                        "updated_notes": updated_notes,
                        "rewrites": rewrites,
                        "moved_tabs": moved_tabs.iter().map(|(_, new)| new.clone()).collect::<Vec<_>>(),
                    }));
                    if update_links {
                        win.show_toast(&crate::i18n::fmt2(
                            crate::i18n::Key::ToastRenamedWithLinks,
                            updated_notes,
                            rewrites,
                        ));
                    } else {
                        win.show_toast(crate::i18n::tr(crate::i18n::Key::ToastRenamed));
                    }
                    win.refresh_knowledge_after_write();
                    let mut changed_paths: Vec<String> = moved_tabs
                        .iter()
                        .flat_map(|(old, new)| [old.clone(), new.clone()])
                        .collect();
                    changed_paths.push(old_path);
                    changed_paths.push(entry.path);
                    win.notify_sibling_windows_written(&changed_paths);
                }
                Ok(Err(err)) => {
                    win.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "rename",
                        "error": err,
                    }));
                    if err.starts_with("rename.unsaved:") {
                        let msg = crate::i18n::tr(crate::i18n::Key::ToastRenameCancelledUnsaved);
                        win.show_toast(msg);
                        win.state.borrow_mut().last_alert = Some(msg.into());
                    } else {
                        win.show_toast(&short_error(
                            crate::i18n::tr(crate::i18n::Key::ErrRenameFailed),
                            &err,
                        ));
                    }
                }
                Err(_) => win.show_toast(crate::i18n::tr(crate::i18n::Key::ErrThread)),
            }
        });
    }

    fn prompt_trash(self: &Rc<Self>, path: &str, is_dir: bool) {
        if self.state.borrow().files_busy {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastFileOpBusy));
            return;
        }
        let dirty = self
            .affected_open_tabs(path)
            .iter()
            .filter(|open| {
                self.state
                    .borrow()
                    .tabs
                    .get(*open)
                    .is_some_and(|tab| tab.edit_state.borrow().dirty)
            })
            .count();
        let weak = Rc::downgrade(self);
        let p = path.to_string();
        files::dialogs::confirm_trash(
            &self.window,
            p.clone(),
            is_dir,
            dirty,
            Rc::new(move || {
                if let Some(win) = weak.upgrade() {
                    win.perform_trash(&p, is_dir);
                }
            }),
        );
    }

    pub fn perform_trash(self: &Rc<Self>, path: &str, is_dir: bool) {
        let (services, handle, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_handle(), st.vault_root())
        };
        let (Some(services), Some(handle), Some(vault_root)) = (services, handle, vault_root)
        else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenFolderFirst));
            return;
        };
        let affected = self.affected_open_tabs(path);
        let sessions: Vec<(String, Arc<hortus_ffi::DocumentSession>, String)> = {
            let st = self.state.borrow();
            affected
                .iter()
                .filter_map(|open| {
                    st.tabs.get(open).and_then(|tab| {
                        tab.session.as_ref().map(|session| {
                            let temp = self.save_temp_for(&vault_root, open);
                            (
                                open.clone(),
                                Arc::clone(session),
                                temp.to_string_lossy().into_owned(),
                            )
                        })
                    })
                })
                .collect()
        };
        self.state.borrow_mut().files_busy = true;
        let weak = Rc::downgrade(self);
        let path_bg = path.to_string();
        let handle_bg = Arc::clone(&handle);
        let core_bg = Arc::clone(&services.core);
        glib::MainContext::default().spawn_local(async move {
            let handle_job = Arc::clone(&handle_bg);
            let path_job = path_bg.clone();
            let result = gio::spawn_blocking(move || {
                for (open, session, temp) in &sessions {
                    let update = session.prepare_close(temp.clone(), true);
                    if update.persistence != DocumentPersistence::Persisted {
                        return Err(format!("trash.unsaved:{open}"));
                    }
                }
                let absolute = handle_job
                    .resolve_trash_target(path_job)
                    .map_err(|e| e.to_string())?;
                let file = gio::File::for_path(Path::new(&absolute));
                file.trash(gio::Cancellable::NONE)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(absolute)
            })
            .await;
            let Some(win) = weak.upgrade() else { return };
            win.state.borrow_mut().files_busy = false;
            match result {
                Ok(Ok(absolute)) => {
                    let removed: Vec<String> = win.affected_open_tabs(&path_bg);
                    {
                        let mut st = win.state.borrow_mut();
                        for open in &removed {
                            st.force_close.insert(open.clone());
                        }
                    }
                    for open in &removed {
                        win.close_tab_by_path(open);
                    }
                    handle_bg.update_search_index(Vec::new(), removed.clone());
                    let _ = core_bg.remove_diagram_state(vault_root, path_bg.clone(), is_dir);
                    #[cfg(feature = "automation")]
                    {
                        let xdg_data = std::env::var("XDG_DATA_HOME").unwrap_or_default();
                        let trash_dir = Path::new(&xdg_data).join("Trash/files");
                        win.state.borrow_mut().last_file_op = Some(serde_json::json!({
                            "op": "trash",
                            "path": path_bg,
                            "absolute": absolute,
                            "is_dir": is_dir,
                            "trash_dir": trash_dir.to_string_lossy(),
                            "trash_files": count_dir_files(&trash_dir),
                            "closed_tabs": removed,
                        }));
                    }
                    #[cfg(not(feature = "automation"))]
                    let _ = absolute;
                    win.show_toast(crate::i18n::tr(crate::i18n::Key::ToastMovedToTrash));
                    win.refresh_knowledge_after_write();
                    let mut notify_paths = removed;
                    notify_paths.push(path_bg);
                    win.notify_sibling_windows_written(&notify_paths);
                }
                Ok(Err(err)) => {
                    win.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "trash",
                        "error": err,
                    }));
                    if err.starts_with("trash.unsaved:") {
                        let msg = crate::i18n::tr(crate::i18n::Key::ToastTrashCancelledUnsaved);
                        win.show_toast(msg);
                        win.state.borrow_mut().last_alert = Some(msg.into());
                    } else {
                        win.show_toast(&short_error(
                            crate::i18n::tr(crate::i18n::Key::ErrTrashFailed),
                            &err,
                        ));
                    }
                }
                Err(_) => win.show_toast(crate::i18n::tr(crate::i18n::Key::ErrThread)),
            }
        });
    }

    fn reveal_in_file_manager(&self, path: &str) {
        let Some(root) = self.state.borrow().vault_root() else {
            return;
        };
        let target = Path::new(&root).join(path);
        let dir = if target.is_dir() {
            target
        } else {
            target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(PathBuf::from(root))
        };
        let uri = glib::filename_to_uri(&dir, None)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file://{}", dir.display()));
        gtk4::UriLauncher::new(&uri).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
    }
}
