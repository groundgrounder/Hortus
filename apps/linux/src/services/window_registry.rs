// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::services::config_service::ConfigSnapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
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
    fn on_vault_renamed(&self, vault_root: &str, renames: &[(String, String)]);
    fn session_entry(&self) -> Option<WindowSessionEntry>;
    fn is_alive(&self) -> bool;
}

struct WindowEntry {
    id: u64,
    bridge: Rc<dyn WindowBridge>,
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
        let bridge: Rc<dyn WindowBridge> = Rc::new(bridge);
        WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive() && e.id != id);
            entries.push(WindowEntry { id, bridge });
        });
    }

    pub fn unregister(&self, id: u64) {
        WINDOW_ENTRIES.with(|list| {
            list.borrow_mut().retain(|e| e.id != id);
        });
    }

    pub fn broadcast_dark_changed(&self) {
        let bridges: Vec<Rc<dyn WindowBridge>> = WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries.iter().map(|e| Rc::clone(&e.bridge)).collect()
        });
        for bridge in bridges {
            if bridge.is_alive() {
                bridge.on_dark_changed();
            }
        }
    }

    pub fn broadcast_config_changed(&self, config: &ConfigSnapshot) {
        let bridges: Vec<Rc<dyn WindowBridge>> = WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries.iter().map(|e| Rc::clone(&e.bridge)).collect()
        });
        for bridge in bridges {
            if bridge.is_alive() {
                bridge.on_config_changed(config);
            }
        }
    }

    pub fn notify_vault_written(
        &self,
        source_window_id: Option<u64>,
        vault_root: &str,
        paths: &[String],
    ) {
        let bridges: Vec<Rc<dyn WindowBridge>> = WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries
                .iter()
                .filter(|e| Some(e.id) != source_window_id)
                .map(|e| Rc::clone(&e.bridge))
                .collect()
        });
        for bridge in bridges {
            if bridge.is_alive() {
                bridge.on_vault_written(vault_root, paths);
            }
        }
    }

    pub fn notify_vault_renamed(
        &self,
        source_window_id: Option<u64>,
        vault_root: &str,
        renames: &[(String, String)],
    ) {
        if renames.is_empty() {
            return;
        }
        let bridges: Vec<Rc<dyn WindowBridge>> = WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries
                .iter()
                .filter(|e| Some(e.id) != source_window_id)
                .map(|e| Rc::clone(&e.bridge))
                .collect()
        });
        for bridge in bridges {
            if bridge.is_alive() {
                bridge.on_vault_renamed(vault_root, renames);
            }
        }
    }

    pub fn collect_session_entries(&self) -> Vec<WindowSessionEntry> {
        let bridges: Vec<Rc<dyn WindowBridge>> = WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.bridge.is_alive());
            entries.iter().map(|e| Rc::clone(&e.bridge)).collect()
        });
        bridges
            .into_iter()
            .filter_map(|bridge| bridge.session_entry())
            .filter(|entry| !entry.is_automation)
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct ReentrantBridge {
        id: u64,
        registry: Arc<WindowRegistry>,
    }

    impl WindowBridge for ReentrantBridge {
        fn on_dark_changed(&self) {
            let _ = self.registry.collect_session_entries();
            self.registry.unregister(self.id);
        }

        fn on_config_changed(&self, _config: &ConfigSnapshot) {}

        fn on_vault_written(&self, _vault_root: &str, _changed_paths: &[String]) {
            let _ = self.registry.collect_session_entries();
            self.registry.unregister(self.id);
        }

        fn on_vault_renamed(&self, _vault_root: &str, _renames: &[(String, String)]) {}

        fn session_entry(&self) -> Option<WindowSessionEntry> {
            Some(WindowSessionEntry {
                id: format!("w-{}", self.id),
                path: "/tmp/vault".into(),
                tabs: vec!["A.md".into()],
                selected: Some("A.md".into()),
                views: HashMap::new(),
                is_automation: false,
            })
        }

        fn is_alive(&self) -> bool {
            true
        }
    }

    #[test]
    fn window_registry_allows_reentrant_reads_and_unregister_during_broadcast() {
        let registry = Arc::new(WindowRegistry::new());
        let id1 = registry.allocate_id();
        let id2 = registry.allocate_id();
        registry.register(
            id1,
            ReentrantBridge {
                id: id1,
                registry: Arc::clone(&registry),
            },
        );
        registry.register(
            id2,
            ReentrantBridge {
                id: id2,
                registry: Arc::clone(&registry),
            },
        );

        registry.notify_vault_written(None, "/tmp/vault", &["A.md".to_string()]);
        assert!(registry.collect_session_entries().is_empty());

        let id3 = registry.allocate_id();
        registry.register(
            id3,
            ReentrantBridge {
                id: id3,
                registry: Arc::clone(&registry),
            },
        );
        registry.broadcast_dark_changed();
        assert!(registry.collect_session_entries().is_empty());
    }
}
