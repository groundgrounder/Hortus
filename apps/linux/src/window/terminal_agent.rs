// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 窗口级终端、Agent 启动闸门、「本次会话改动」面板、统一工具区回调与设置对话框协调。

use crate::agent::{AgentLaunchCallbacks, ChangesCallbacks, launch_agent_with_snapshot_gate};
use crate::core::{ColorSchemeChoice, PanelRegion};
use crate::dock::DockCallbacks;
use crate::services::ConfigSnapshot;
use crate::settings::{SettingsDialog, SettingsPage};
use crate::terminal::TerminalHostCallbacks;
use crate::window::actions::apply_color_scheme;
use crate::window::commands::WindowCommand;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

impl WindowInner {
    pub fn init_terminal_and_changes_panels(self: &Rc<Self>) {
        let weak_create = Rc::downgrade(self);
        let weak_update = Rc::downgrade(self);
        let weak_select = Rc::downgrade(self);
        let weak_close = Rc::downgrade(self);
        let weak_exit = Rc::downgrade(self);
        self.terminal_host.set_callbacks(TerminalHostCallbacks {
            on_tab_created: Rc::new(move |id, region, widget, title, icon| {
                if let Some(win) = weak_create.upgrade() {
                    win.tool_dock
                        .insert_terminal_tab(id, region, &widget, &title, icon);
                }
            }),
            on_tab_updated: Rc::new(move |id, title, icon| {
                if let Some(win) = weak_update.upgrade() {
                    win.tool_dock.update_terminal_tab(id, &title, icon);
                }
            }),
            on_request_select_tab: Rc::new(move |id| {
                if let Some(win) = weak_select.upgrade() {
                    win.tool_dock.select_terminal_tab(id);
                }
            }),
            on_request_close_tab: Rc::new(move |id| {
                if let Some(win) = weak_close.upgrade() {
                    win.tool_dock.close_terminal_tab(id);
                }
            }),
            on_agent_exited: Rc::new(move |sid, code| {
                if let Some(win) = weak_exit.upgrade() {
                    win.handle_agent_process_exited(sid, code);
                }
            }),
        });

        let weak_close_t = Rc::downgrade(self);
        let weak_emptied = Rc::downgrade(self);
        let weak_reveal = Rc::downgrade(self);
        let weak_collapse = Rc::downgrade(self);
        let weak_term_sel = Rc::downgrade(self);
        let weak_term_mov = Rc::downgrade(self);
        let weak_chg = Rc::downgrade(self);
        let weak_layout = Rc::downgrade(self);
        self.tool_dock.set_callbacks(DockCallbacks {
            on_request_close_terminal: Rc::new(move |tid| {
                if let Some(win) = weak_close_t.upgrade() {
                    win.terminal_host.close_tab_from_dock(tid);
                }
            }),
            on_region_emptied: Rc::new(move |region| {
                if let Some(win) = weak_emptied.upgrade() {
                    win.set_region_revealed(region, false);
                }
            }),
            on_region_reveal: Rc::new(move |region| {
                if let Some(win) = weak_reveal.upgrade() {
                    win.set_region_revealed(region, true);
                }
            }),
            on_region_collapse: Rc::new(move |region| {
                if let Some(win) = weak_collapse.upgrade() {
                    win.set_region_revealed(region, false);
                }
            }),
            on_terminal_selected: Rc::new(move |tid| {
                if let Some(win) = weak_term_sel.upgrade() {
                    win.terminal_host.mark_active_tab(tid);
                }
            }),
            on_terminal_moved: Rc::new(move |tid, region| {
                if let Some(win) = weak_term_mov.upgrade() {
                    win.terminal_host.set_tab_region(tid, region);
                }
            }),
            on_changes_shown: Rc::new(move || {
                if let Some(win) = weak_chg.upgrade() {
                    win.changes_panel.refresh();
                }
            }),
            on_layout_changed: Rc::new(move || {
                if let Some(win) = weak_layout.upgrade() {
                    win.schedule_layout_save();
                }
            }),
        });

        let weak_open = Rc::downgrade(self);
        let weak_rb = Rc::downgrade(self);
        let weak_c_toast = Rc::downgrade(self);
        let weak_win = Rc::downgrade(self);
        self.changes_panel.set_callbacks(ChangesCallbacks {
            on_open_note: Rc::new(move |relpath| {
                if let Some(win) = weak_open.upgrade() {
                    win.send_command(WindowCommand::OpenNote {
                        relpath,
                        heading: None,
                    });
                }
            }),
            on_rolled_back: Rc::new(move |paths: Vec<String>| {
                if let Some(win) = weak_rb.upgrade() {
                    win.handle_changes_rolled_back(&paths);
                }
            }),
            on_toast: Rc::new(move |msg: String| {
                if let Some(win) = weak_c_toast.upgrade() {
                    win.show_toast(&msg);
                }
            }),
            parent_window: Rc::new(move || weak_win.upgrade().map(|w| w.window.clone())),
        });
    }

    pub fn send_path_to_terminal(self: &Rc<Self>) {
        let Some(relpath) = self.active_tab_path() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSendPathNoNote));
            return;
        };
        let fallback = self.tool_dock.preferred_terminal_region();
        let (_id, region, quoted) = self.terminal_host.send_note_path(&relpath, fallback);
        self.set_region_revealed(region, true);
        self.show_toast(&crate::i18n::fmt1(crate::i18n::Key::ToastSentPath, quoted));
    }

    pub fn active_note_selection(&self) -> Option<String> {
        let active_path = self.active_tab_path()?;
        let st = self.state.borrow();
        let tab = st.tabs.get(&active_path)?;
        if tab.editing {
            let (start, end) = tab.edit_buffer.selection_bounds()?;
            let txt = start.text(&end).to_string();
            if txt.is_empty() { None } else { Some(txt) }
        } else {
            let reading = tab.reading.as_ref()?;
            let mut parts = Vec::new();
            for (_tv, buf) in reading.text_views() {
                if let Some((start, end)) = buf.selection_bounds() {
                    let s = start.text(&end).to_string();
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
    }

    pub fn send_selection_to_terminal(self: &Rc<Self>) {
        let Some(text) = self.active_note_selection() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSendSelEmpty));
            return;
        };
        let fallback = self.tool_dock.preferred_terminal_region();
        if let Some((_id, region, _)) = self.terminal_host.send_selection(&text, fallback) {
            self.set_region_revealed(region, true);
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSentSelection));
        } else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSelControlOnly));
        }
    }

    pub fn launch_agent_action(self: &Rc<Self>) {
        let region = self.tool_dock.preferred_terminal_region();
        self.launch_agent_in_region(region);
    }

    pub fn launch_agent_in_region(self: &Rc<Self>, target_region: PanelRegion) {
        let (services, login_path) = {
            let st = self.state.borrow();
            (st.services.clone(), st.login_path.clone())
        };
        let Some(services) = services else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastCoreNotReady));
            return;
        };
        let weak_toast = Rc::downgrade(self);
        let weak_settings = Rc::downgrade(self);
        let weak_changed = Rc::downgrade(self);
        let weak_alert = Rc::downgrade(self);
        let weak_vault = Rc::downgrade(self);
        let weak_disp = Rc::downgrade(self);
        launch_agent_with_snapshot_gate(
            &self.window,
            services,
            self.terminal_host.clone(),
            target_region,
            login_path,
            AgentLaunchCallbacks {
                on_toast: Rc::new(move |msg| {
                    if let Some(win) = weak_toast.upgrade() {
                        win.show_toast(&msg);
                    }
                }),
                on_open_agent_settings: Rc::new(move || {
                    if let Some(win) = weak_settings.upgrade() {
                        win.send_command(WindowCommand::OpenSettings(Some(SettingsPage::Agent)));
                    }
                }),
                on_session_changed: Rc::new(move || {
                    if let Some(win) = weak_changed.upgrade() {
                        win.changes_panel.refresh();
                    }
                }),
                on_alert: Rc::new(move |dlg: adw::AlertDialog| {
                    if let Some(win) = weak_alert.upgrade() {
                        let mut st = win.state.borrow_mut();
                        st.last_alert = Some(dlg.heading().unwrap_or_default().to_string());
                        st.last_alert_dialog.set(Some(&dlg));
                    }
                }),
                current_vault_root: Rc::new(move || {
                    weak_vault
                        .upgrade()
                        .and_then(|w| w.state.borrow().vault_root())
                }),
                is_disposing: Rc::new(move || {
                    weak_disp
                        .upgrade()
                        .is_none_or(|w| w.state.borrow().disposing)
                }),
            },
        );
    }

    pub fn build_settings_dialog(self: &Rc<Self>) -> Option<SettingsDialog> {
        let (services, login_path) = {
            let st = self.state.borrow();
            (st.services.clone(), st.login_path.clone())
        };
        let Some(services) = services else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastCoreNotReady));
            return None;
        };
        let scheme = match adw::StyleManager::default().color_scheme() {
            adw::ColorScheme::ForceLight => ColorSchemeChoice::Light,
            adw::ColorScheme::ForceDark => ColorSchemeChoice::Dark,
            _ => ColorSchemeChoice::System,
        };
        let weak_toast = Rc::downgrade(self);
        let weak_scheme = Rc::downgrade(self);
        Some(SettingsDialog::new(
            services,
            login_path,
            scheme,
            Rc::new(move |msg| {
                if let Some(win) = weak_toast.upgrade() {
                    win.show_toast(&msg);
                }
            }),
            Rc::new(move |choice| {
                apply_color_scheme(choice);
                if let Some(win) = weak_scheme.upgrade() {
                    win.refresh_all_reading_tabs();
                }
            }),
        ))
    }

    pub fn open_settings_dialog(self: &Rc<Self>, page: Option<SettingsPage>) {
        if let Some(dlg) = self.build_settings_dialog() {
            dlg.present(&self.window, page);
        }
    }

    pub fn apply_config_update(self: &Rc<Self>, cfg: &ConfigSnapshot) {
        self.terminal_host.apply_font(cfg.terminal_font.clone());
        self.refresh_all_reading_tabs();
    }

    fn handle_agent_process_exited(self: &Rc<Self>, sid: String, code: i32) {
        let core = self
            .state
            .borrow()
            .services
            .as_ref()
            .map(|s| Arc::clone(&s.core));
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            if let Some(core_bg) = core {
                let _ = gio::spawn_blocking(move || {
                    core_bg.mark_agent_session(sid, "exited".into(), Some(code))
                })
                .await;
            }
            let Some(win) = weak.upgrade() else {
                return;
            };
            if win.state.borrow().disposing {
                return;
            }
            win.changes_panel.refresh();
            win.refresh_vault_tabs_and_broadcast();
            win.show_toast(&crate::i18n::fmt1(crate::i18n::Key::ToastAgentExited, code));
        });
    }

    fn handle_changes_rolled_back(self: &Rc<Self>, paths: &[String]) {
        self.sync_paths_with_disk(paths);
        let (services, reg_id, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.window_reg_id, st.vault_root())
        };
        if let (Some(svcs), Some(root)) = (services, vault_root) {
            svcs.windows.notify_vault_written(reg_id, &root, paths);
        }
    }

    pub(crate) fn refresh_vault_tabs_and_broadcast(self: &Rc<Self>) {
        let open_paths: Vec<String> = self.state.borrow().tabs.keys().cloned().collect();
        self.sync_paths_with_disk(&open_paths);
        let (services, reg_id, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.window_reg_id, st.vault_root())
        };
        if let (Some(svcs), Some(root)) = (services, vault_root) {
            svcs.windows
                .notify_vault_written(reg_id, &root, &open_paths);
        }
    }

    pub(crate) fn sync_paths_with_disk(self: &Rc<Self>, paths: &[String]) {
        let Some(vault_root) = self.state.borrow().vault_root() else {
            return;
        };
        let targets: Vec<String> = if paths.is_empty() {
            self.state.borrow().tabs.keys().cloned().collect()
        } else {
            paths.to_vec()
        };
        let mut to_close = Vec::new();
        let mut to_refresh = Vec::new();
        {
            let st = self.state.borrow();
            for rel in targets {
                if !st.tabs.contains_key(&rel) {
                    continue;
                }
                let abs = Path::new(&vault_root).join(&rel);
                if abs.exists() {
                    to_refresh.push(rel);
                } else {
                    to_close.push(rel);
                }
            }
        }
        if !to_close.is_empty() {
            let mut st = self.state.borrow_mut();
            for rel in &to_close {
                st.force_close.insert(rel.clone());
            }
        }
        for rel in &to_close {
            self.close_tab_by_path(rel);
        }
        for rel in &to_refresh {
            self.refresh_document_tab(rel);
        }
        self.refresh_knowledge_after_write();
    }

    pub fn notify_sibling_windows_written(&self, paths: &[String]) {
        let (services, reg_id, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.window_reg_id, st.vault_root())
        };
        if let (Some(svcs), Some(root)) = (services, vault_root) {
            svcs.windows.notify_vault_written(reg_id, &root, paths);
        }
    }
}
