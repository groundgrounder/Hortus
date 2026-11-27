// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 全文搜索面板：后台异步查询、输入防抖、并发取消与弱引用解环（G-07 / C-06 修复）。

use crate::i18n::{Key, fmt2, fmt4, tr};
use crate::knowledge::widgets::{
    OpenNoteCallback, clear_box, entry_row, panel_scroll, snippet_markup, snippet_row,
};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{SearchCancellation, SearchResult, VaultHandle};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct SearchState {
    handle: Option<Arc<VaultHandle>>,
    generation: u64,
    cancellation: Option<Arc<SearchCancellation>>,
    queries: u32,
    cancellations: u32,
    stale_dropped: u32,
    last_query: String,
    groups: usize,
    matches: usize,
    pending: bool,
    dirty: bool,
    on_open: Option<OpenNoteCallback>,
    group_buttons: Vec<gtk4::Button>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct SearchPanelStats {
    pub queries: u32,
    pub cancellations: u32,
    pub stale_dropped: u32,
    pub last_query: String,
    pub groups: usize,
    pub matches: usize,
    pub pending: bool,
}

struct SearchPanelInner {
    root: gtk4::Box,
    entry: gtk4::SearchEntry,
    list: gtk4::Box,
    status: gtk4::Label,
    state: RefCell<SearchState>,
}

#[derive(Clone)]
pub struct SearchPanel {
    inner: Rc<SearchPanelInner>,
}

impl Default for SearchPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header.add_css_class("hortus-panel-header");
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text(tr(Key::SearchPlaceholder))
            .hexpand(true)
            .build();
        header.append(&entry);

        let status = gtk4::Label::new(Some(tr(Key::SearchPrompt)));
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_xalign(0.0);
        status.set_margin_start(10);
        status.set_margin_end(10);
        status.set_margin_top(4);
        status.set_margin_bottom(6);

        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        list.set_margin_start(6);
        list.set_margin_end(6);
        list.set_margin_bottom(12);
        let scroll = panel_scroll(&list);
        scroll.set_margin_top(4);

        root.append(&header);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&status);
        root.append(&scroll);

        let inner = Rc::new(SearchPanelInner {
            root,
            entry,
            list,
            status,
            state: RefCell::new(SearchState::default()),
        });
        Self::bind_entry(&inner);
        Self { inner }
    }

    pub fn root(&self) -> &gtk4::Box {
        &self.inner.root
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.inner.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.inner.state.borrow_mut().on_open = Some(callback);
    }

    pub fn focus_entry(&self) {
        self.inner.entry.grab_focus();
        self.inner.entry.select_region(0, -1);
    }

    #[cfg(feature = "automation")]
    pub fn set_query(&self, query: &str) {
        self.inner.entry.set_text(query);
    }

    /// 索引随磁盘变化后显式重跑当前查询（G-07 修复：不再发射伪 `search-changed` 信号）。
    pub fn rerun_now(&self) {
        self.inner.state.borrow_mut().dirty = false;
        let query = self.inner.entry.text().to_string();
        if query.trim().is_empty() {
            return;
        }
        Self::schedule_query(&self.inner, query, false);
    }

    pub fn mark_dirty(&self) {
        self.inner.state.borrow_mut().dirty = true;
    }

    pub fn refresh_if_dirty(&self) {
        let dirty = {
            let mut st = self.inner.state.borrow_mut();
            let was = st.dirty;
            st.dirty = false;
            was
        };
        if dirty {
            self.rerun_now();
        }
    }

    /// 立即执行一次查询（跳过输入防抖），用于并发取消验证。
    #[cfg(feature = "automation")]
    pub fn run_query_now(&self, query: &str) {
        Self::schedule_query(&self.inner, query.to_string(), true);
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> SearchPanelStats {
        let st = self.inner.state.borrow();
        SearchPanelStats {
            queries: st.queries,
            cancellations: st.cancellations,
            stale_dropped: st.stale_dropped,
            last_query: st.last_query.clone(),
            groups: st.groups,
            matches: st.matches,
            pending: st.pending,
        }
    }

    #[cfg(feature = "automation")]
    pub fn click_group(&self, index: usize) -> bool {
        let button = self.inner.state.borrow().group_buttons.get(index).cloned();
        let Some(button) = button else {
            return false;
        };
        button.emit_clicked();
        true
    }

    fn bind_entry(inner: &Rc<SearchPanelInner>) {
        let weak = Rc::downgrade(inner);
        inner.entry.connect_search_changed(move |entry| {
            let Some(inner) = weak.upgrade() else { return };
            let query = entry.text().to_string();
            Self::schedule_query(&inner, query, false);
        });
    }

    fn schedule_query(inner: &Rc<SearchPanelInner>, query: String, immediate: bool) {
        let generation = {
            let mut st = inner.state.borrow_mut();
            if st.handle.is_none() {
                return;
            }
            if let Some(old) = st.cancellation.take() {
                old.cancel();
                st.cancellations += 1;
            }
            if st.pending {
                st.cancellations += 1;
            }
            st.generation += 1;
            st.queries += 1;
            st.pending = true;
            st.dirty = false;
            st.last_query = query.clone();
            st.generation
        };

        if query.trim().is_empty() {
            clear_box(&inner.list);
            inner.status.set_label(tr(Key::SearchPrompt));
            let mut st = inner.state.borrow_mut();
            st.group_buttons.clear();
            st.groups = 0;
            st.matches = 0;
            st.pending = false;
            return;
        }

        if !immediate {
            inner.status.set_label(tr(Key::SearchInProgress));
        }

        let weak = Rc::downgrade(inner);
        if immediate {
            Self::run_backend_search(weak, generation, query);
        } else {
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                Self::run_backend_search(weak, generation, query);
            });
        }
    }

    fn run_backend_search(weak: Weak<SearchPanelInner>, generation: u64, query: String) {
        let Some(inner) = weak.upgrade() else { return };
        let (handle, cancellation) = {
            let mut st = inner.state.borrow_mut();
            if st.generation != generation {
                return;
            }
            let Some(handle) = st.handle.clone() else {
                st.pending = false;
                return;
            };
            let cancellation = SearchCancellation::new();
            st.cancellation = Some(cancellation.clone());
            (handle, cancellation)
        };

        let weak_done = Rc::downgrade(&inner);
        glib::MainContext::default().spawn_local(async move {
            let query_bg = query.clone();
            let result = gio::spawn_blocking(move || handle.search(query_bg, 6, cancellation))
                .await
                .unwrap_or_default();
            let Some(inner) = weak_done.upgrade() else {
                return;
            };
            {
                let mut st = inner.state.borrow_mut();
                if st.generation != generation {
                    st.stale_dropped += 1;
                    return;
                }
                st.pending = false;
                st.cancellation = None;
                st.last_query = query.clone();
            }
            Self::render_search_results(&inner, &query, &result);
            let mut st = inner.state.borrow_mut();
            st.groups = result.len();
            st.matches = result.iter().map(|r| r.matches.len()).sum();
        });
    }

    fn render_search_results(inner: &Rc<SearchPanelInner>, query: &str, results: &[SearchResult]) {
        clear_box(&inner.list);
        inner.state.borrow_mut().group_buttons.clear();
        let total: usize = results.iter().map(|r| r.matches.len()).sum();
        let total_count: u64 = results.iter().map(|r| u64::from(r.count)).sum();
        inner.status.set_label(&fmt4(
            Key::SearchResultsSummary,
            query,
            results.len(),
            total,
            total_count,
        ));

        for result in results {
            let group = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            let header = entry_row(
                &result.title,
                None,
                Some(&fmt2(Key::SearchResultMeta, &result.path, result.count)),
            );
            let header_btn = gtk4::Button::builder()
                .child(&header)
                .halign(gtk4::Align::Fill)
                .build();
            header_btn.add_css_class("flat");
            header_btn.add_css_class("hortus-result-row");
            let path = result.path.clone();
            let start = result
                .matches
                .first()
                .map(|m| u64::from(m.start))
                .unwrap_or(0);
            let weak = Rc::downgrade(inner);
            header_btn.connect_clicked(move |_| {
                let cb = weak
                    .upgrade()
                    .and_then(|inner| inner.state.borrow().on_open.clone());
                if let Some(cb) = cb {
                    cb(path.clone(), Some(start));
                }
            });
            inner
                .state
                .borrow_mut()
                .group_buttons
                .push(header_btn.clone());
            group.append(&header_btn);

            for m in result.matches.iter().take(3) {
                let markup = snippet_markup(&m.snippet, m.snippet_start, m.start, m.length);
                let row = snippet_row(&markup);
                let btn = gtk4::Button::builder()
                    .child(&row)
                    .halign(gtk4::Align::Fill)
                    .build();
                btn.add_css_class("flat");
                btn.add_css_class("hortus-result-row");
                let path = result.path.clone();
                let start = u64::from(m.start);
                let weak = Rc::downgrade(inner);
                btn.connect_clicked(move |_| {
                    let cb = weak
                        .upgrade()
                        .and_then(|inner| inner.state.borrow().on_open.clone());
                    if let Some(cb) = cb {
                        cb(path.clone(), Some(start));
                    }
                });
                group.append(&btn);
            }
            inner.list.append(&group);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_panel_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let panel = SearchPanel::new();
        let weak = Rc::downgrade(&panel.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(panel);
        assert_eq!(weak.strong_count(), 0);
    }
}
