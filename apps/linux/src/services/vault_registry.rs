// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

use crate::core::{NotifyQueue, is_note_file_name};
use hortus_ffi::{TreeBatch, TreeEntry, VaultHandle, WorkspaceCore};
use std::collections::{BTreeMap, HashMap};
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

#[derive(Default)]
struct CompactedTreeSnapshot {
    entries: BTreeMap<String, TreeEntry>,
    scan_complete: bool,
    scan_micros: u64,
}

impl CompactedTreeSnapshot {
    fn apply_batches(&mut self, batches: &[TreeBatch]) {
        for batch in batches {
            if batch.scan_complete {
                self.scan_complete = true;
            }
            self.scan_micros = self.scan_micros.max(batch.scan_micros);
            for removed in &batch.removed {
                self.entries.remove(removed);
            }
            for entry in &batch.upserted {
                self.entries.insert(entry.path.clone(), clone_entry(entry));
            }
        }
    }

    fn to_replay_batches(&self) -> Vec<TreeBatch> {
        if self.entries.is_empty() && !self.scan_complete {
            return Vec::new();
        }
        vec![TreeBatch {
            upserted: self.entries.values().map(clone_entry).collect(),
            removed: Vec::new(),
            scan_complete: self.scan_complete,
            scan_micros: self.scan_micros,
            errors: Vec::new(),
        }]
    }
}

#[derive(Default)]
struct VaultContextState {
    snapshot: CompactedTreeSnapshot,
    subscribers: Vec<Subscriber>,
}

/// 知识库上下文：按规范路径复用同一 `VaultHandle` 与单一后台监听线程，并在最后引用释放时关闭。
pub struct VaultContext {
    pub root: String,
    pub handle: Arc<VaultHandle>,
    state: Mutex<VaultContextState>,
    next_id: AtomicU64,
}

impl VaultContext {
    fn new(handle: Arc<VaultHandle>) -> Arc<Self> {
        let root = handle.root();
        let ctx = Arc::new(Self {
            root,
            handle: Arc::clone(&handle),
            state: Mutex::new(VaultContextState::default()),
            next_id: AtomicU64::new(1),
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
                let mut st = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
                st.snapshot.apply_batches(&batches);
                st.subscribers.retain(|sub| {
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
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let replay = st.snapshot.to_replay_batches();
        if !replay.is_empty() {
            queue.push(replay);
        }
        st.subscribers.push(Subscriber {
            id,
            queue: Arc::downgrade(&queue),
        });
        (id, queue)
    }

    pub fn unsubscribe(&self, id: u64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.subscribers.retain(|sub| {
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
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for sub in &st.subscribers {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacted_snapshot_deduplicates_repeated_updates_and_removals() {
        let mut snap = CompactedTreeSnapshot::default();
        for i in 0..50u64 {
            snap.apply_batches(&[TreeBatch {
                upserted: vec![
                    TreeEntry {
                        path: "a.md".into(),
                        parent: String::new(),
                        name: "a.md".into(),
                        directory: false,
                    },
                    TreeEntry {
                        path: "b.md".into(),
                        parent: String::new(),
                        name: "b.md".into(),
                        directory: false,
                    },
                ],
                removed: Vec::new(),
                scan_complete: i == 0,
                scan_micros: i,
                errors: Vec::new(),
            }]);
        }
        snap.apply_batches(&[TreeBatch {
            upserted: Vec::new(),
            removed: vec!["b.md".into()],
            scan_complete: false,
            scan_micros: 0,
            errors: Vec::new(),
        }]);
        let replay = snap.to_replay_batches();
        assert_eq!(replay.len(), 1);
        assert!(replay[0].scan_complete);
        assert_eq!(replay[0].upserted.len(), 1);
        assert_eq!(replay[0].upserted[0].path, "a.md");
    }
}
