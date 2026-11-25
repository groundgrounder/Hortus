// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Ctrl+P 快速打开弹窗：Weak 父窗口与 Weak 内部引用解环（C-06 修复）。

use crate::knowledge::widgets::{OpenNoteCallback, entry_row};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{QuickOpenHit, VaultHandle};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct QuickState {
    handle: Option<Arc<VaultHandle>>,
    generation: u64,
    queries: u32,
    last_query: String,
    hits: Vec<QuickOpenHit>,
    on_open: Option<OpenNoteCallback>,
    selected: usize,
    pending: bool,
    open: bool,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct QuickOpenStats {
    pub queries: u32,
    pub last_query: String,
    pub hits: usize,
    pub first: Option<String>,
    pub pending: bool,
    pub open: bool,
}

struct QuickOpenInner {
    dialog: adw::Dialog,
    entry: gtk4::SearchEntry,
    list: gtk4::ListBox,
    state: RefCell<QuickState>,
    window: RefCell<glib::WeakRef<gtk4::Window>>,
}

#[derive(Clone)]
pub struct QuickOpen {
    inner: Rc<QuickOpenInner>,
}

impl Default for QuickOpen {
    fn default() -> Self {
        Self::new()
    }
}

impl QuickOpen {
    pub fn new() -> Self {
        let dialog = adw::Dialog::builder()
            .title(crate::i18n::tr(crate::i18n::Key::QuickOpenTitle))
            .content_width(500)
            .content_height(460)
            .build();
        dialog.add_css_class("hortus-quick-open");

        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text(crate::i18n::tr(crate::i18n::Key::QuickOpenPlaceholder))
            .margin_top(10)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        let list = gtk4::ListBox::new();
        list.add_css_class("hortus-quick-open-list");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .child(&list)
            .build();
        root.append(&entry);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&scroll);
        dialog.set_child(Some(&root));

        let inner = Rc::new(QuickOpenInner {
            dialog,
            entry,
            list,
            state: RefCell::new(QuickState::default()),
            window: RefCell::new(glib::WeakRef::new()),
        });
        Self::bind(&inner);
        Self { inner }
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.inner.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.inner.state.borrow_mut().on_open = Some(callback);
    }

    #[cfg(feature = "automation")]
    pub fn is_open(&self) -> bool {
        self.inner.state.borrow().open
    }

    #[cfg(feature = "automation")]
    pub fn hits(&self) -> Vec<String> {
        self.inner
            .state
            .borrow()
            .hits
            .iter()
            .map(|hit| hit.path.clone())
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> QuickOpenStats {
        let st = self.inner.state.borrow();
        QuickOpenStats {
            queries: st.queries,
            last_query: st.last_query.clone(),
            hits: st.hits.len(),
            first: st.hits.first().map(|h| h.path.clone()),
            pending: st.pending,
            open: st.open,
        }
    }

    pub fn open(&self, parent: &gtk4::Window) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.open = true;
            st.hits.clear();
            st.selected = 0;
            st.generation += 1;
            st.pending = st.handle.is_some();
        }
        render_quick_hits(&self.inner.list, &[], 0);
        self.inner.window.borrow().set(Some(parent));
        self.inner.entry.set_text("");
        self.inner.dialog.present(Some(parent));
        self.inner.entry.grab_focus();
        Self::query_now(&self.inner, "");
    }

    #[cfg(feature = "automation")]
    pub fn set_query(&self, query: &str) {
        self.inner.entry.set_text(query);
    }

    #[cfg(feature = "automation")]
    pub fn activate_selected(&self) {
        Self::open_selected(&self.inner);
    }

    pub fn close_if_open(&self) {
        if self.inner.state.borrow().open {
            self.inner.state.borrow_mut().open = false;
            self.inner.dialog.force_close();
        }
        self.inner.window.borrow().set(None);
    }

    fn bind(inner: &Rc<QuickOpenInner>) {
        {
            let weak = Rc::downgrade(inner);
            inner.dialog.connect_closed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.state.borrow_mut().open = false;
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_search_changed(move |entry| {
                let Some(inner) = weak.upgrade() else { return };
                let query = entry.text().to_string();
                let generation = {
                    let mut st = inner.state.borrow_mut();
                    if st.handle.is_none() {
                        return;
                    }
                    st.generation += 1;
                    st.queries += 1;
                    st.last_query = query.clone();
                    st.hits.clear();
                    st.selected = 0;
                    st.pending = true;
                    st.generation
                };
                render_quick_hits(&inner.list, &[], 0);
                let weak_debounce = Rc::downgrade(&inner);
                glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    let Some(inner) = weak_debounce.upgrade() else {
                        return;
                    };
                    let handle = {
                        let st = inner.state.borrow();
                        if st.generation != generation {
                            return;
                        }
                        st.handle.clone()
                    };
                    let Some(handle) = handle else { return };
                    let weak_done = Rc::downgrade(&inner);
                    glib::MainContext::default().spawn_local(async move {
                        let result = gio::spawn_blocking(move || handle.quick_open(query, 30))
                            .await
                            .unwrap_or_default();
                        let Some(inner) = weak_done.upgrade() else {
                            return;
                        };
                        let mut st = inner.state.borrow_mut();
                        if st.generation != generation {
                            return;
                        }
                        st.pending = false;
                        st.hits = result;
                        st.selected = 0;
                        render_quick_hits(&inner.list, &st.hits, st.selected);
                    });
                });
            });
        }

        let key = gtk4::EventControllerKey::new();
        {
            let weak = Rc::downgrade(inner);
            key.connect_key_pressed(move |_, keyval, _, _| {
                use gtk4::gdk::Key;
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                match keyval {
                    Key::Down | Key::Up => {
                        let mut st = inner.state.borrow_mut();
                        if st.pending || st.hits.is_empty() {
                            return glib::Propagation::Proceed;
                        }
                        if keyval == Key::Down {
                            st.selected = (st.selected + 1).min(st.hits.len() - 1);
                        } else {
                            st.selected = st.selected.saturating_sub(1);
                        }
                        render_quick_hits(&inner.list, &st.hits, st.selected);
                        glib::Propagation::Stop
                    }
                    Key::Return | Key::KP_Enter => {
                        if inner.state.borrow().pending {
                            return glib::Propagation::Stop;
                        }
                        Self::open_selected(&inner);
                        glib::Propagation::Stop
                    }
                    Key::Escape => glib::Propagation::Proceed,
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        inner.entry.add_controller(key);

        {
            let weak = Rc::downgrade(inner);
            inner.list.connect_row_activated(move |_, row| {
                let Some(inner) = weak.upgrade() else { return };
                let index = row.index().max(0) as usize;
                inner.state.borrow_mut().selected = index;
                Self::open_selected(&inner);
            });
        }
    }

    fn open_selected(inner: &Rc<QuickOpenInner>) {
        if inner.state.borrow().pending {
            return;
        }
        open_selected_impl(&inner.state);
        inner.state.borrow_mut().open = false;
        inner.dialog.close();
    }

    fn query_now(inner: &Rc<QuickOpenInner>, query: &str) {
        let (handle, generation) = {
            let mut st = inner.state.borrow_mut();
            let Some(handle) = st.handle.clone() else {
                st.pending = false;
                return;
            };
            st.generation += 1;
            st.queries += 1;
            st.last_query = query.to_string();
            st.hits.clear();
            st.selected = 0;
            st.pending = true;
            (handle, st.generation)
        };
        render_quick_hits(&inner.list, &[], 0);
        let query = query.to_string();
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || handle.quick_open(query, 30))
                .await
                .unwrap_or_default();
            let Some(inner) = weak.upgrade() else { return };
            let mut st = inner.state.borrow_mut();
            if st.generation != generation {
                return;
            }
            st.pending = false;
            st.hits = result;
            st.selected = 0;
            render_quick_hits(&inner.list, &st.hits, st.selected);
        });
    }
}

fn render_quick_hits(list: &gtk4::ListBox, hits: &[QuickOpenHit], selected: usize) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    for (index, hit) in hits.iter().enumerate() {
        let row = gtk4::ListBoxRow::new();
        let label = entry_row(&hit.title, None, Some(&hit.path));
        label.set_margin_start(6);
        label.set_margin_end(6);
        row.set_child(Some(&label));
        if index == selected {
            row.add_css_class("suggested-action");
        }
        list.append(&row);
    }
}

fn open_selected_impl(state: &RefCell<QuickState>) {
    let (hit, on_open) = {
        let st = state.borrow();
        (
            st.hits.get(st.selected).map(|hit| hit.path.clone()),
            st.on_open.clone(),
        )
    };
    if let (Some(path), Some(cb)) = (hit, on_open) {
        cb(path, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_open_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let quick = QuickOpen::new();
        let weak = Rc::downgrade(&quick.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(quick);
        assert_eq!(weak.strong_count(), 0);
    }
}
