// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

type DarkCallback = Box<dyn Fn() -> bool>;

struct WindowEntry {
    id: u64,
    on_dark_changed: DarkCallback,
}

thread_local! {
    static WINDOW_ENTRIES: RefCell<Vec<WindowEntry>> = const { RefCell::new(Vec::new()) };
}

/// 存活窗口弱注册表：仅保存弱回调，窗口关闭/析构时自动注销并回收。
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

    pub fn register(&self, id: u64, on_dark_changed: impl Fn() -> bool + 'static) {
        WINDOW_ENTRIES.with(|list| {
            let mut entries = list.borrow_mut();
            entries.retain(|e| e.id != id);
            entries.push(WindowEntry {
                id,
                on_dark_changed: Box::new(on_dark_changed),
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
            let mut entries = list.borrow_mut();
            entries.retain(|e| (e.on_dark_changed)());
        });
    }

    #[allow(dead_code)]
    pub fn live_count(&self) -> usize {
        WINDOW_ENTRIES.with(|list| list.borrow().len())
    }
}
