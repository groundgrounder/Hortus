// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::services::config_service::ConfigSnapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub struct WindowSessionEntry {
    pub id: String,
    pub path: String,
    pub tabs: Vec<String>,
    pub selected: Option<String>,
    pub views: HashMap<String, String>,
    pub is_automation: bool,
}

pub trait WindowBridge {
    fn on_dark_changed(&self);
    fn on_config_changed(&self, config: &ConfigSnapshot);
    fn on_vault_written(&self, vault_root: &str, changed_paths: &[String]);
    fn session_entry(&self) -> Option<WindowSessionEntry>;
    fn is_alive(&self) -> bool;
}

struct WindowEntry {
    id: u64,
    bridge: Box<dyn WindowBridge>,
}

thread_local! {
    static WINDOW_ENTRIES: RefCell<Vec<WindowEntry>> = const { RefCell::new(Vec::new()) };
}

/// 存活窗口弱注册表：仅保存弱桥接，窗口关闭/析构时自动注销并回收。
#[derive(Default)]
pub struct WindowRegistry {
    next_id: AtomicU64,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn register(&self, id: u64, bridge: impl WindowBridge + 'static) {
        WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.id != id);
            entries.push(WindowEntry {
                id,
                bridge: Box::new(bridge),
            });
        });
    }

    pub fn unregister(&self, id: u64) {
        WINDOW_ENTRIES.with(|list| {
            list.borrow_mut().retain(|e| e.id != id);
        });
    }

    pub fn broadcast_dark_changed(&self) {
        WINDOW_ENTRIES.with(|list| {
            list.borrow_mut().retain(|e| e.bridge.is_alive());
            let count = list.borrow().len();
            for idx in 0..count {
                let alive = list.borrow().get(idx).is_some_and(|e| e.bridge.is_alive());
                if alive {
                    // 按索引逐个借用前先读状态并在外层回调，防止重入修改 list panic。
                    // WindowBridge::on_dark_changed 只向 CommandQueue / spawn_local 入队。
                    if let Some(entry) = list.borrow().get(idx) {
                        entry.bridge.on_dark_changed();
                    }
                }
            }
        });
    }

    pub fn broadcast_config_changed(&self, config: &ConfigSnapshot) {
        WINDOW_ENTRIES.with(|list| {
            list.borrow_mut().retain(|e| e.bridge.is_alive());
            let count = list.borrow().len();
            for idx in 0..count {
                if let Some(entry) = list.borrow().get(idx) {
                    entry.bridge.on_config_changed(config);
                }
            }
        });
    }

    pub fn notify_vault_written(
        &self,
        source_window_id: Option<u64>,
        vault_root: &str,
        paths: &[String],
    ) {
        WINDOW_ENTRIES.with(|list| {
            list.borrow_mut().retain(|e| e.bridge.is_alive());
            let count = list.borrow().len();
            for idx in 0..count {
                if let Some(entry) = list.borrow().get(idx)
                    && Some(entry.id) != source_window_id
                {
                    entry.bridge.on_vault_written(vault_root, paths);
                }
            }
        });
    }

    pub fn collect_session_entries(&self) -> Vec<WindowSessionEntry> {
        WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries
                .iter()
                .filter_map(|e| e.bridge.session_entry())
                .filter(|entry| !entry.is_automation)
                .collect()
        })
    }

    #[cfg(feature = "automation")]
    pub fn live_count(&self) -> usize {
        WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries.len()
        })
    }
}
