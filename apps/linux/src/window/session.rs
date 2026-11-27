// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 多窗口协调、跨窗口同库/配置桥接与会话持久化恢复（不持久化动态终端 PTY）。

use crate::core::CliOptions;
use crate::services::{AppServices, ConfigSnapshot, WindowBridge, WindowSessionEntry};
use crate::window::HortusWindow;
use crate::window::commands::WindowCommand;
use crate::window::shell::WindowInner;
use crate::window::state::RestoredTabsOverride;
use gtk4::prelude::*;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SavedWindowEntry {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub tabs: Vec<String>,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub views: HashMap<String, String>,
}

pub struct WindowInnerBridge {
    weak: Weak<WindowInner>,
}

impl WindowInnerBridge {
    pub fn new(weak: Weak<WindowInner>) -> Self {
        Self { weak }
    }
}

impl WindowBridge for WindowInnerBridge {
    fn on_dark_changed(&self) {
        if let Some(win) = self.weak.upgrade()
            && !win.state.borrow().disposing
        {
            win.refresh_all_reading_tabs();
        }
    }

    fn on_config_changed(&self, config: &ConfigSnapshot) {
        if let Some(win) = self.weak.upgrade()
            && !win.state.borrow().disposing
        {
            win.send_command(WindowCommand::ConfigChanged(config.clone()));
        }
    }

    fn on_vault_written(&self, vault_root: &str, changed_paths: &[String]) {
        if let Some(win) = self.weak.upgrade()
            && !win.state.borrow().disposing
            && win.state.borrow().vault_root().as_deref() == Some(vault_root)
        {
            win.send_command(WindowCommand::ExternalVaultRefresh(changed_paths.to_vec()));
        }
    }

    fn on_vault_renamed(&self, vault_root: &str, renames: &[(String, String)]) {
        if let Some(win) = self.weak.upgrade()
            && !win.state.borrow().disposing
            && win.state.borrow().vault_root().as_deref() == Some(vault_root)
        {
            win.send_command(WindowCommand::ExternalVaultRename(renames.to_vec()));
        }
    }

    fn session_entry(&self) -> Option<WindowSessionEntry> {
        let win = self.weak.upgrade()?;
        if win.state.borrow().disposing {
            return None;
        }
        win.build_session_entry()
    }

    fn is_alive(&self) -> bool {
        self.weak
            .upgrade()
            .is_some_and(|w| !w.state.borrow().disposing)
    }
}

impl WindowInner {
    pub fn build_session_entry(&self) -> Option<WindowSessionEntry> {
        let vault_root = self.state.borrow().vault_root()?;
        let selected = self.active_tab_path();
        let n_pages = self.tab_view.n_pages();
        let st = self.state.borrow();
        let mut tabs = Vec::new();
        let mut views = HashMap::new();
        for idx in 0..n_pages {
            let page = self.tab_view.nth_page(idx);
            if let Some((rel, tab)) = st.tabs.iter().find(|(_, t)| t.page == page) {
                tabs.push(rel.clone());
                views.insert(
                    rel.clone(),
                    if tab.editing {
                        "editing".to_string()
                    } else {
                        "reading".to_string()
                    },
                );
            }
        }
        Some(WindowSessionEntry {
            id: st.window_id.clone(),
            path: vault_root,
            tabs,
            selected,
            views,
            is_automation: self.options.has_automation_mode(),
        })
    }

    pub fn open_new_window(self: &Rc<Self>) {
        let _ = self.open_new_window_with_vault(None);
    }

    pub fn open_new_window_with_vault(
        self: &Rc<Self>,
        vault_override: Option<PathBuf>,
    ) -> Option<HortusWindow> {
        let Some(app) = self
            .window
            .application()
            .and_then(|a| a.downcast::<adw::Application>().ok())
        else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastCannotOpenNewWindow));
            return None;
        };
        let (services, vault_root) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_root())
        };
        let opts = CliOptions {
            vault: vault_override.or_else(|| vault_root.map(PathBuf::from)),
            ..Default::default()
        };
        let win = HortusWindow::new_with_services(&app, opts, services);
        win.present();
        self.persist_window_session();
        Some(win)
    }

    pub fn handle_external_vault_refresh(self: &Rc<Self>, paths: &[String]) {
        self.sync_paths_with_disk(paths, false);
        self.changes_panel.refresh();
    }

    pub fn handle_external_vault_rename(self: &Rc<Self>, renames: &[(String, String)]) {
        self.migrate_renamed_tabs(renames);
        self.refresh_knowledge_after_write();
        self.changes_panel.refresh();
    }

    pub fn persist_window_session(&self) {
        if self.options.has_automation_mode() {
            return;
        }
        let services = {
            let st = self.state.borrow();
            if st.disposing || st.suppressing_session_save {
                return;
            }
            st.services.clone()
        };
        let Some(services) = services else {
            return;
        };
        let entries = services.windows.collect_session_entries();
        write_session_entries_to_core(&services.core, &entries);
    }

    pub fn persist_session_before_close(&self) {
        if self.options.has_automation_mode() {
            return;
        }
        let (services, my_id) = {
            let st = self.state.borrow();
            (st.services.clone(), st.window_reg_id)
        };
        let Some(services) = services else {
            return;
        };
        let all = services.windows.collect_session_entries();
        let my_entry = self.build_session_entry();
        let others: Vec<WindowSessionEntry> = if all.len() > 1 {
            all.into_iter()
                .filter(|e| my_entry.as_ref().is_none_or(|m| m.id != e.id))
                .collect()
        } else if let Some(entry) = my_entry {
            // 关闭最后一个窗口时保留它的会话快照，确保下次无参启动可恢复。
            vec![entry]
        } else {
            all
        };
        let _ = my_id;
        write_session_entries_to_core(&services.core, &others);
    }
}

pub fn write_session_entries_to_core(
    core: &Arc<hortus_ffi::WorkspaceCore>,
    entries: &[WindowSessionEntry],
) {
    let Ok(existing_json) = core.session_json() else {
        return;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&existing_json) else {
        return;
    };
    let saved_windows: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id,
                "path": entry.path,
                "tabs": entry.tabs,
                "selected": entry.selected,
                "views": entry.views,
                "frame": serde_json::Value::Null,
                "visible": serde_json::Value::Null,
                "layout": serde_json::Value::Null,
            })
        })
        .collect();
    if let Some(first) = entries.first() {
        root["last_vault"] = serde_json::Value::String(first.path.clone());
    }
    root["windows"] = serde_json::Value::Array(saved_windows);
    if let Ok(serialized) = serde_json::to_string(&root) {
        let _ = core.save_window_session(serialized);
    }
}

pub fn launch_initial_windows(app: &adw::Application, opts: CliOptions) {
    #[cfg(feature = "automation")]
    if opts.restore_session_json.is_some() {
        crate::automation::l3_terminal_agent::execute_session_restore_check(app, opts);
        return;
    }

    if opts.vault.is_some() || opts.has_automation_mode() {
        let win = HortusWindow::new(app, opts);
        win.present();
        return;
    }

    let Ok((services, _)) = AppServices::init_blocking() else {
        let win = HortusWindow::new(app, opts);
        win.present();
        return;
    };
    services.ensure_dark_listener();
    services.config.ensure_monitor_and_css();

    if opts.color_scheme.is_none() {
        crate::window::actions::apply_color_scheme(
            crate::services::ui_prefs::load_color_scheme_with_core(&services.core),
        );
    }

    let saved = parse_restorable_entries(&services.core);
    if saved.is_empty() {
        let win = HortusWindow::new_with_services(app, opts, Some(services));
        win.present();
        return;
    }

    for entry in &saved {
        crate::window::shell::bump_next_window_id(&entry.id);
    }

    for entry in saved {
        let vault_path = PathBuf::from(&entry.path);
        let win_opts = CliOptions {
            vault: Some(vault_path.clone()),
            ..Default::default()
        };
        let valid_tabs: Vec<String> = entry
            .tabs
            .into_iter()
            .filter(|rel| vault_path.join(rel).is_file())
            .collect();
        let selected = entry
            .selected
            .filter(|sel| valid_tabs.iter().any(|t| t == sel));
        let win = HortusWindow::new_with_services(app, win_opts, Some(Arc::clone(&services)));
        {
            let mut st = win.inner.state.borrow_mut();
            st.window_id = entry.id;
            st.initial_tabs_override = Some(RestoredTabsOverride {
                tabs: valid_tabs,
                selected,
                views: entry.views,
            });
        }
        win.present();
    }
}

pub fn parse_restorable_entries(core: &hortus_ffi::WorkspaceCore) -> Vec<SavedWindowEntry> {
    let Ok(json) = core.session_json() else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&json) else {
        return Vec::new();
    };
    let Some(list) = root
        .get("windows")
        .and_then(|v| serde_json::from_value::<Vec<SavedWindowEntry>>(v.clone()).ok())
    else {
        return Vec::new();
    };
    list.into_iter()
        .filter(|entry| !entry.path.is_empty() && Path::new(&entry.path).is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::window::shell::bump_next_window_id_with;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn bump_next_window_id_prevents_collision_with_restored_windows() {
        let counter = AtomicU64::new(1);
        bump_next_window_id_with(&counter, "win-1");
        bump_next_window_id_with(&counter, "win-4");
        bump_next_window_id_with(&counter, "win-2");
        bump_next_window_id_with(&counter, "custom-id");
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 5);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 6);
    }
}
