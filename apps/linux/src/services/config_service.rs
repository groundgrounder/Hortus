// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 配置快照管理、`config.toml` 目录监听防抖与跨窗口热重载广播。

use crate::platform::style::update_editor_font_size_css;
use crate::services::window_registry::WindowRegistry;
use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;
use hortus_ffi::{EditableConfigInfo, WorkspaceCore};
use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub const FONT_SIZE_MIN: f64 = 9.0;
pub const FONT_SIZE_MAX: f64 = 40.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSnapshot {
    pub config_path: String,
    pub font_size: f64,
    pub default_view: String,
    pub terminal_font: Option<String>,
    pub agent_configured: bool,
    pub agent_command: Option<String>,
    pub agent_args: Vec<String>,
}

impl Default for ConfigSnapshot {
    fn default() -> Self {
        Self {
            config_path: String::new(),
            font_size: 15.0,
            default_view: "reading".to_string(),
            terminal_font: None,
            agent_configured: false,
            agent_command: None,
            agent_args: Vec::new(),
        }
    }
}

impl From<&EditableConfigInfo> for ConfigSnapshot {
    fn from(info: &EditableConfigInfo) -> Self {
        Self {
            config_path: info.config_path.clone(),
            font_size: if info.font_size.is_finite()
                && (FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&info.font_size)
            {
                info.font_size
            } else {
                15.0
            },
            default_view: if info.default_view == "editing" {
                "editing".to_string()
            } else {
                "reading".to_string()
            },
            terminal_font: info
                .terminal_font
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            agent_configured: info.agent_configured,
            agent_command: info.agent_command.clone(),
            agent_args: info.agent_args.clone(),
        }
    }
}

struct MonitorState {
    _monitor: gio::FileMonitor,
    debounce: Option<glib::SourceId>,
}

thread_local! {
    static CONFIG_MONITOR: RefCell<Option<MonitorState>> = const { RefCell::new(None) };
}

pub struct ConfigService {
    core: Arc<WorkspaceCore>,
    windows: Arc<WindowRegistry>,
    snapshot: RwLock<ConfigSnapshot>,
}

impl ConfigService {
    pub fn new(core: Arc<WorkspaceCore>, windows: Arc<WindowRegistry>) -> Arc<Self> {
        let initial = core
            .editable_config()
            .map(|info| ConfigSnapshot::from(&info))
            .unwrap_or_default();
        Arc::new(Self {
            core,
            windows,
            snapshot: RwLock::new(initial),
        })
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 主线程应用当前字号 CSS 并启动 `config.toml` 监听（幂等）。
    pub fn ensure_monitor_and_css(self: &Arc<Self>) {
        let snap = self.snapshot();
        update_editor_font_size_css(snap.font_size);

        let already = CONFIG_MONITOR.with(|slot| slot.borrow().is_some());
        if already || snap.config_path.is_empty() {
            return;
        }
        let config_path = Path::new(&snap.config_path);
        let Some(parent_dir) = config_path.parent() else {
            return;
        };
        let target_name = config_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.toml")
            .to_string();
        let dir_file = gio::File::for_path(parent_dir);
        let Ok(monitor) =
            dir_file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let svc_weak = Arc::downgrade(self);
        monitor.connect_changed(move |_, file, other_file, event| {
            let matches_name = |f: &gio::File| {
                f.basename()
                    .and_then(|n| n.into_os_string().into_string().ok())
                    .is_some_and(|name| name == target_name)
            };
            let relevant = matches_name(file) || other_file.is_some_and(matches_name);
            if !relevant {
                return;
            }
            if !matches!(
                event,
                gio::FileMonitorEvent::Changed
                    | gio::FileMonitorEvent::ChangesDoneHint
                    | gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::Renamed
                    | gio::FileMonitorEvent::MovedIn
            ) {
                return;
            }
            let Some(svc) = svc_weak.upgrade() else {
                return;
            };
            CONFIG_MONITOR.with(|slot| {
                let mut guard = slot.borrow_mut();
                let Some(state) = guard.as_mut() else {
                    return;
                };
                if let Some(old) = state.debounce.take() {
                    old.remove();
                }
                let svc_deb = Arc::downgrade(&svc);
                let src = glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    CONFIG_MONITOR.with(|inner_slot| {
                        if let Some(s) = inner_slot.borrow_mut().as_mut() {
                            s.debounce = None;
                        }
                    });
                    if let Some(svc_now) = svc_deb.upgrade() {
                        svc_now.reload_from_disk_async();
                    }
                });
                state.debounce = Some(src);
            });
        });

        CONFIG_MONITOR.with(|slot| {
            *slot.borrow_mut() = Some(MonitorState {
                _monitor: monitor,
                debounce: None,
            });
        });
    }

    pub fn reload_from_disk_async(self: &Arc<Self>) {
        let svc_weak = Arc::downgrade(self);
        let core = Arc::clone(&self.core);
        glib::MainContext::default().spawn_local(async move {
            let loaded = gio::spawn_blocking(move || core.editable_config())
                .await
                .ok()
                .and_then(Result::ok);
            let Some(info) = loaded else {
                return;
            };
            let Some(svc) = svc_weak.upgrade() else {
                return;
            };
            svc.apply_info_if_changed(&info, false);
        });
    }

    pub fn apply_saved(&self, info: &EditableConfigInfo) {
        self.apply_info_if_changed(info, true);
    }

    fn apply_info_if_changed(&self, info: &EditableConfigInfo, force_broadcast: bool) {
        let next = ConfigSnapshot::from(info);
        let changed = {
            let mut guard = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
            if !force_broadcast && *guard == next {
                false
            } else {
                *guard = next.clone();
                true
            }
        };
        if changed {
            update_editor_font_size_css(next.font_size);
            self.windows.broadcast_config_changed(&next);
        }
    }
}
