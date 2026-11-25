// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 知识库打开/切换竞态防护、服务初始化、磁盘轮询（C-03）与窗口关闭保护。

use crate::platform::fonts::ensure_katex_fonts_registered_async;
use crate::services::AppServices;
use crate::terminal::process::resolve_login_path;
use crate::window::session::WindowInnerBridge;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::DocumentPersistence;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

impl WindowInner {
    pub fn initialize_services_async(self: &Rc<Self>) {
        let preloaded = self.state.borrow().services.clone();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let init_res = if let Some(svcs) = preloaded {
                gio::spawn_blocking(resolve_login_path)
                    .await
                    .map(|lp| (svcs, None, lp))
                    .map_err(|_| crate::i18n::tr(crate::i18n::Key::ErrThread).to_string())
            } else {
                gio::spawn_blocking(|| {
                    let (svcs, warn) = AppServices::init_blocking()?;
                    let lp = resolve_login_path();
                    Ok::<_, String>((svcs, warn, lp))
                })
                .await
                .unwrap_or_else(|_| Err(crate::i18n::tr(crate::i18n::Key::ErrThread).into()))
            };
            let _ = ensure_katex_fonts_registered_async().await;
            let Some(win) = weak.upgrade() else { return };
            if win.state.borrow().disposing {
                return;
            }
            match init_res {
                Ok((services, font_warning, login_path)) => {
                    services.ensure_dark_listener();
                    services.config.ensure_monitor_and_css();
                    let reg_id = services.windows.allocate_id();
                    services
                        .windows
                        .register(reg_id, WindowInnerBridge::new(Rc::downgrade(&win)));
                    let term_font = services.config.snapshot().terminal_font;
                    {
                        let mut st = win.state.borrow_mut();
                        st.math_service = Arc::clone(&services.math_service);
                        st.services = Some(Arc::clone(&services));
                        st.window_reg_id = Some(reg_id);
                        st.login_path = login_path;
                    }
                    win.terminal_host.apply_font(term_font);
                    win.load_saved_layout();
                    if let Some(warn) = font_warning {
                        win.show_toast(&crate::i18n::fmt1(
                            crate::i18n::Key::ToastKatexNotice,
                            warn,
                        ));
                    }
                    if let Some(vault) = win.options.vault.clone() {
                        win.open_vault_path(vault);
                    } else {
                        #[cfg(feature = "automation")]
                        crate::automation::maybe_schedule_automation(&win);
                    }
                }
                Err(err) => {
                    win.show_toast(&crate::i18n::fmt1(crate::i18n::Key::ToastInitFailed, err));
                }
            }
        });
    }

    pub fn prompt_open_vault(self: &Rc<Self>) {
        let dialog = gtk4::FileDialog::builder()
            .title(crate::i18n::tr(crate::i18n::Key::SelectFolderTitle))
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res
                && let Some(path) = file.path()
                && let Some(win) = weak.upgrade()
            {
                win.open_vault_path(path);
            }
        });
    }

    pub fn open_vault_path(self: &Rc<Self>, path: PathBuf) {
        let services = {
            let st = self.state.borrow();
            st.services.clone()
        };
        let Some(services) = services else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastCoreNotReady));
            return;
        };

        let existing_pages: Vec<adw::TabPage> = (0..self.tab_view.n_pages())
            .map(|i| self.tab_view.nth_page(i))
            .collect();
        for page in existing_pages {
            self.tab_view.close_page(&page);
        }

        let (old_ctx, old_sub_id, open_gen) = {
            let mut st = self.state.borrow_mut();
            st.vault_open_gen = st.vault_open_gen.wrapping_add(1);
            st.scan_complete = false;
            st.scan_micros = 0;
            (
                st.vault_ctx.take(),
                st.vault_sub_id.take(),
                st.vault_open_gen,
            )
        };
        if let (Some(ctx), Some(sub_id)) = (&old_ctx, old_sub_id) {
            ctx.unsubscribe(sub_id);
        }
        drop(old_ctx);

        self.vault_tree.clear();
        self.render_outline_empty(crate::i18n::tr(crate::i18n::Key::ScanningFolder));

        let weak = Rc::downgrade(self);
        let path_str = path.to_string_lossy().into_owned();
        let core = Arc::clone(&services.core);
        let vaults = Arc::clone(&services.vaults);
        glib::MainContext::default().spawn_local(async move {
            let opened = gio::spawn_blocking(move || vaults.open_or_reuse(&core, &path_str))
                .await
                .unwrap_or_else(|_| Err(crate::i18n::tr(crate::i18n::Key::ErrThread).into()));
            let Some(win) = weak.upgrade() else { return };
            if win.state.borrow().disposing || win.state.borrow().vault_open_gen != open_gen {
                return;
            }
            match opened {
                Ok(ctx) => {
                    let folder_name = Path::new(&ctx.root)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Hortus")
                        .to_string();
                    win.window_title.set_subtitle(&folder_name);
                    win.empty_status.set_description(Some(crate::i18n::tr(
                        crate::i18n::Key::EmptySelectNoteHint,
                    )));
                    let (sub_id, queue) = ctx.subscribe();
                    let temp_dir = {
                        let mut st = win.state.borrow_mut();
                        st.vault_ctx = Some(Arc::clone(&ctx));
                        st.vault_sub_id = Some(sub_id);
                        st.temp_dir.clone()
                    };
                    win.attach_vault_to_panels(&ctx.handle);
                    win.terminal_host.set_vault_root(Some(ctx.root.clone()));
                    win.changes_panel.set_context(
                        Arc::clone(&services.core),
                        Some(ctx.root.clone()),
                        temp_dir,
                    );

                    let weak_ui = Rc::downgrade(&win);
                    glib::MainContext::default().spawn_local(async move {
                        let mut initial_notes_opened = false;
                        while let Some(batches) = queue.next().await {
                            let Some(win_ui) = weak_ui.upgrade() else {
                                break;
                            };
                            if win_ui.state.borrow().disposing
                                || win_ui.state.borrow().vault_open_gen != open_gen
                            {
                                break;
                            }
                            let (complete, micros) = win_ui.vault_tree.apply_batches(&batches);
                            win_ui.handle_vault_batches(&batches);
                            if complete {
                                let (pending, override_state) = {
                                    let mut st = win_ui.state.borrow_mut();
                                    st.scan_complete = true;
                                    st.scan_micros = micros;
                                    if !initial_notes_opened {
                                        initial_notes_opened = true;
                                        (
                                            std::mem::take(&mut st.pending_open_notes),
                                            st.initial_tabs_override.take(),
                                        )
                                    } else {
                                        (Vec::new(), None)
                                    }
                                };
                                if let Some(restored) = override_state {
                                    win_ui.state.borrow_mut().initial_tab_views = restored.views;
                                    for rel in &restored.tabs {
                                        win_ui.open_note_at(rel, None);
                                    }
                                    if let Some(sel) = restored.selected {
                                        win_ui.open_note_at(&sel, None);
                                    }
                                } else {
                                    for rel in &pending {
                                        win_ui.open_note_at(rel, None);
                                    }
                                    if let Some(first) = pending.first() {
                                        win_ui.open_note_at(first, None);
                                    }
                                }
                                win_ui.persist_window_session();
                                #[cfg(feature = "automation")]
                                crate::automation::maybe_schedule_automation(&win_ui);
                            }
                        }
                    });
                }
                Err(err) => {
                    win.show_toast(&crate::i18n::fmt1(crate::i18n::Key::OpenFolderFailed, err));
                }
            }
        });
    }

    pub fn start_disk_poll_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let source_id = glib::timeout_add_seconds_local(1, move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let (paths, root, busy, disposing) = {
                let st = win.state.borrow();
                (
                    st.tabs.keys().cloned().collect::<Vec<_>>(),
                    st.vault_root().unwrap_or_default(),
                    st.files_busy,
                    st.disposing,
                )
            };
            if disposing {
                return glib::ControlFlow::Break;
            }
            if busy || root.is_empty() {
                return glib::ControlFlow::Continue;
            }
            for path in paths {
                let absolute = Path::new(&root).join(&path);
                let meta = std::fs::metadata(&absolute).ok();
                let current = meta.as_ref().map(|meta| {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_nanos())
                        .unwrap_or(0);
                    (meta.len(), modified)
                });
                let (known, loaded) = {
                    let st = win.state.borrow();
                    let Some(tab) = st.tabs.get(&path) else {
                        continue;
                    };
                    (
                        tab.edit_state.borrow().disk_stamp,
                        tab.source_loaded && tab.session.is_some(),
                    )
                };
                if !loaded {
                    continue;
                }
                let changed = match (known, current) {
                    (Some(known), Some(current)) => known != current,
                    (Some(_), None) => true,
                    _ => false,
                };
                if changed {
                    let counted = {
                        let st = win.state.borrow();
                        match st.tabs.get(&path) {
                            Some(tab) => {
                                tab.edit_state.borrow_mut().poll_refreshes += 1;
                                true
                            }
                            None => false,
                        }
                    };
                    if counted {
                        win.refresh_document_tab(&path);
                    }
                }
                if current.is_some() {
                    let st = win.state.borrow();
                    if let Some(tab) = st.tabs.get(&path) {
                        let mut edit = tab.edit_state.borrow_mut();
                        if edit.disk_stamp.is_none() {
                            edit.disk_stamp = current;
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
        self.state.borrow_mut().disk_poll_source = Some(source_id);
    }

    pub fn handle_window_close(self: &Rc<Self>) -> glib::Propagation {
        {
            let mut st = self.state.borrow_mut();
            st.close_requests += 1;
        }
        if self.state.borrow().close_approved {
            self.persist_session_before_close();
            self.teardown();
            return glib::Propagation::Proceed;
        }
        let pending: Vec<(String, String, Arc<hortus_ffi::DocumentSession>)> = {
            let st = self.state.borrow();
            let fallback_root = st.vault_root().unwrap_or_default();
            st.tabs
                .iter()
                .filter_map(|(path, tab)| {
                    let root = if tab.vault_root.is_empty() {
                        fallback_root.clone()
                    } else {
                        tab.vault_root.clone()
                    };
                    tab.session
                        .clone()
                        .map(|session| (path.clone(), root, session))
                })
                .collect()
        };
        let unsaved: Vec<(String, String, Arc<hortus_ffi::DocumentSession>)> = pending
            .into_iter()
            .filter(|(_, _, session)| session.persistence() != DocumentPersistence::Persisted)
            .collect();
        self.state.borrow_mut().close_unsaved = unsaved.len() as u32;
        if unsaved.is_empty() {
            self.persist_session_before_close();
            self.teardown();
            return glib::Propagation::Proceed;
        }
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Some(win) = weak.upgrade() else { return };
            let jobs: Vec<(String, String, Arc<hortus_ffi::DocumentSession>)> = unsaved
                .iter()
                .map(|(path, tab_root, session)| {
                    let temp = if tab_root.is_empty() {
                        std::env::temp_dir()
                    } else {
                        win.save_temp_for(tab_root, path)
                    };
                    (
                        path.clone(),
                        temp.to_string_lossy().into_owned(),
                        Arc::clone(session),
                    )
                })
                .collect();
            let results = gio::spawn_blocking(move || {
                jobs.into_iter()
                    .map(|(path, temp, session)| {
                        let update = session.prepare_close(temp, true);
                        (path, update.persistence)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let Some(win) = weak.upgrade() else { return };
            if win.state.borrow().disposing {
                return;
            }
            let still_unsaved: Vec<String> = results
                .into_iter()
                .filter(|(_, persistence)| *persistence != DocumentPersistence::Persisted)
                .map(|(path, _)| path)
                .collect();
            if still_unsaved.is_empty() {
                win.state.borrow_mut().close_approved = true;
                win.window.close();
            } else {
                win.ask_exit_with_unsaved(&still_unsaved);
            }
        });
        glib::Propagation::Stop
    }

    fn ask_exit_with_unsaved(self: &Rc<Self>, paths: &[String]) {
        let mut body = String::from(crate::i18n::tr(crate::i18n::Key::UnsavedQuitBodyHeader));
        for path in paths {
            body.push_str(&format!("· {path}\n"));
        }
        body.push_str(crate::i18n::tr(crate::i18n::Key::UnsavedQuitBodyFooter));
        let dialog = adw::AlertDialog::builder()
            .heading(crate::i18n::tr(crate::i18n::Key::UnsavedHeading))
            .body(&body)
            .build();
        dialog.add_response("cancel", crate::i18n::tr(crate::i18n::Key::Cancel));
        dialog.add_response("quit", crate::i18n::tr(crate::i18n::Key::BtnDiscardAndQuit));
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        {
            let mut st = self.state.borrow_mut();
            st.last_alert = Some(dialog.heading().unwrap_or_default().to_string());
            st.last_alert_dialog.set(Some(&dialog));
        }
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if let Some(win) = weak.upgrade() {
                win.state.borrow_mut().last_alert_dialog.set(None);
                if response == "quit" {
                    win.state.borrow_mut().close_approved = true;
                    win.window.close();
                }
            }
        });
        dialog.present(Some(&self.window));
    }
}
