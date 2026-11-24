// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::{NotifyQueue, is_note_file_name};
use hortus_ffi::{TreeBatch, TreeEntry, VaultHandle, WorkspaceCore};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

fn clone_entry(entry: &TreeEntry) -> TreeEntry {
    TreeEntry {
        path: entry.path.clone(),
        parent: entry.parent.clone(),
        name: entry.name.clone(),
        directory: entry.directory,
    }
}

fn clone_batch(batch: &TreeBatch) -> TreeBatch {
    TreeBatch {
        upserted: batch.upserted.iter().map(clone_entry).collect(),
        removed: batch.removed.clone(),
        scan_complete: batch.scan_complete,
        scan_micros: batch.scan_micros,
        errors: batch.errors.clone(),
    }
}

fn clone_batches(batches: &[TreeBatch]) -> Vec<TreeBatch> {
    batches.iter().map(clone_batch).collect()
}

struct Subscriber {
    id: u64,
    queue: Weak<NotifyQueue<Vec<TreeBatch>>>,
}

/// 知识库上下文：按规范路径复用同一 `VaultHandle` 与单一后台监听线程，并在最后引用释放时关闭。
pub struct VaultContext {
    pub root: String,
    pub handle: Arc<VaultHandle>,
    subscribers: Mutex<Vec<Subscriber>>,
    next_id: AtomicU64,
    cached_batches: Mutex<Vec<TreeBatch>>,
}

impl VaultContext {
    fn new(handle: Arc<VaultHandle>) -> Arc<Self> {
        let root = handle.root();
        let ctx = Arc::new(Self {
            root,
            handle: Arc::clone(&handle),
            subscribers: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            cached_batches: Mutex::new(Vec::new()),
        });
        let weak_ctx = Arc::downgrade(&ctx);
        std::thread::spawn(move || {
            loop {
                let batches = handle.wait_for_batches();
                if batches.is_empty() {
                    break;
                }
                let mut upserted = Vec::new();
                let mut removed = Vec::new();
                for batch in &batches {
                    for entry in &batch.upserted {
                        if !entry.directory && is_note_file_name(&entry.path) {
                            upserted.push(entry.path.clone());
                        }
                    }
                    removed.extend(batch.removed.iter().cloned());
                }
                if !upserted.is_empty() || !removed.is_empty() {
                    handle.update_search_index(upserted, removed);
                }
                let Some(ctx) = weak_ctx.upgrade() else {
                    break;
                };
                {
                    let mut cached = ctx.cached_batches.lock().unwrap_or_else(|e| e.into_inner());
                    cached.extend(clone_batches(&batches));
                }
                let mut subs = ctx.subscribers.lock().unwrap_or_else(|e| e.into_inner());
                subs.retain(|sub| {
                    if let Some(q) = sub.queue.upgrade() {
                        q.push(clone_batches(&batches));
                        true
                    } else {
                        false
                    }
                });
            }
        });
        ctx
    }

    pub fn subscribe(&self) -> (u64, Arc<NotifyQueue<Vec<TreeBatch>>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let queue = NotifyQueue::new();
        let replay = {
            let cached = self
                .cached_batches
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            clone_batches(&cached)
        };
        if !replay.is_empty() {
            queue.push(replay);
        }
        let mut subs = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subs.push(Subscriber {
            id,
            queue: Arc::downgrade(&queue),
        });
        (id, queue)
    }

    pub fn unsubscribe(&self, id: u64) {
        let mut subs = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|sub| {
            if sub.id == id {
                if let Some(q) = sub.queue.upgrade() {
                    q.close();
                }
                false
            } else {
                sub.queue.strong_count() > 0
            }
        });
    }
}

impl Drop for VaultContext {
    fn drop(&mut self) {
        let subs = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        for sub in subs.iter() {
            if let Some(q) = sub.queue.upgrade() {
                q.close();
            }
        }
        let handle = Arc::clone(&self.handle);
        std::thread::spawn(move || handle.close());
    }
}

/// 进程级知识库注册表：同一规范路径在多窗口间复用 `VaultContext`。
#[derive(Default)]
pub struct VaultRegistry {
    contexts: Mutex<HashMap<String, Weak<VaultContext>>>,
}

impl VaultRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_or_reuse(
        &self,
        core: &Arc<WorkspaceCore>,
        path: &str,
    ) -> Result<Arc<VaultContext>, String> {
        let canonical_probe = std::fs::canonicalize(path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string());
        {
            let mut map = self.contexts.lock().unwrap_or_else(|e| e.into_inner());
            map.retain(|_, v| v.strong_count() > 0);
            if let Some(existing) = map.get(&canonical_probe).and_then(Weak::upgrade) {
                return Ok(existing);
            }
        }
        let handle = core
            .open_vault(path.to_string())
            .map_err(|e| e.to_string())?;
        let canonical = handle.root();
        let mut map = self.contexts.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, v| v.strong_count() > 0);
        if let Some(existing) = map.get(&canonical).and_then(Weak::upgrade) {
            std::thread::spawn(move || handle.close());
            return Ok(existing);
        }
        let ctx = VaultContext::new(handle);
        map.insert(canonical, Arc::downgrade(&ctx));
        Ok(ctx)
    }
}
