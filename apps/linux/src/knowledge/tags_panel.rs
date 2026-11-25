// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 层级标签浏览面板：`handle.tags()` 与 `notes_with_tag()` 均移至 `gio::spawn_blocking` 异步执行，弱引用解环（C-06）。

use crate::i18n::{Key, fmt1, fmt2, tr};
use crate::knowledge::widgets::{OpenNoteCallback, clear_box, dim_label, entry_row, panel_scroll};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{TagSummaryInfo, VaultHandle};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default)]
struct TagsState {
    handle: Option<Arc<VaultHandle>>,
    tags: Vec<TagSummaryInfo>,
    selected: Option<String>,
    selected_notes: Vec<String>,
    selected_notes_for: Option<String>,
    filter: String,
    visible_tags: usize,
    visible_notes: usize,
    on_open: Option<OpenNoteCallback>,
    programmatic_filter: bool,
    tags_gen: u64,
    notes_gen: u64,
    loading_tags: bool,
    loading_notes: bool,
    tag_buttons: Vec<(String, gtk4::Button)>,
    note_buttons: Vec<(String, gtk4::Button)>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct TagsPanelStats {
    pub total_tags: usize,
    pub visible_tags: usize,
    pub selected: Option<String>,
    pub visible_notes: usize,
    pub filter: String,
}

struct TagsPanelInner {
    root: gtk4::Box,
    filter_entry: gtk4::SearchEntry,
    list: gtk4::Box,
    status: gtk4::Label,
    state: RefCell<TagsState>,
}

#[derive(Clone)]
pub struct TagsPanel {
    inner: Rc<TagsPanelInner>,
}

impl Default for TagsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl TagsPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header.add_css_class("hortus-panel-header");
        let filter_entry = gtk4::SearchEntry::builder()
            .placeholder_text(tr(Key::TagsFilterPlaceholder))
            .hexpand(true)
            .build();
        header.append(&filter_entry);

        let status = gtk4::Label::new(Some(tr(Key::TagsNoFolder)));
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

        let inner = Rc::new(TagsPanelInner {
            root,
            filter_entry,
            list,
            status,
            state: RefCell::new(TagsState::default()),
        });
        Self::bind_filter(&inner);
        Self { inner }
    }

    pub fn root(&self) -> &gtk4::Box {
        &self.inner.root
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.inner.state.borrow_mut().handle = Some(handle);
        self.refresh();
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.inner.state.borrow_mut().on_open = Some(callback);
    }

    #[cfg(feature = "automation")]
    pub fn is_loading(&self) -> bool {
        let st = self.inner.state.borrow();
        st.loading_tags || st.loading_notes
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> TagsPanelStats {
        let st = self.inner.state.borrow();
        TagsPanelStats {
            total_tags: st.tags.len(),
            visible_tags: st.visible_tags,
            selected: st.selected.clone(),
            visible_notes: st.visible_notes,
            filter: st.filter.clone(),
        }
    }

    #[cfg(feature = "automation")]
    pub fn filter_text(&self) -> String {
        self.inner.filter_entry.text().to_string()
    }

    /// 选中标签并展开其笔记；阅读视图中的 `#标签` 点击也走这里。
    pub fn select_tag(&self, tag: &str) {
        let normalized = tag.trim_start_matches('#').to_string();
        {
            let mut st = self.inner.state.borrow_mut();
            st.selected = Some(normalized.clone());
            st.selected_notes.clear();
            st.selected_notes_for = None;
            st.filter = tag.to_string();
            st.programmatic_filter = true;
        }
        self.inner.filter_entry.set_text(tag);
        Self::load_notes_for_tag(&self.inner, normalized);
        Self::render(&self.inner);
    }

    /// 后台异步刷新全库标签列表（不阻塞主线程）。
    pub fn refresh(&self) {
        let (handle, generation) = {
            let mut st = self.inner.state.borrow_mut();
            let Some(handle) = st.handle.clone() else {
                return;
            };
            st.tags_gen = st.tags_gen.wrapping_add(1);
            st.loading_tags = true;
            (handle, st.tags_gen)
        };
        let weak = Rc::downgrade(&self.inner);
        glib::MainContext::default().spawn_local(async move {
            let tags = gio::spawn_blocking(move || handle.tags())
                .await
                .unwrap_or_default();
            let Some(inner) = weak.upgrade() else { return };
            let selected_to_reload = {
                let mut st = inner.state.borrow_mut();
                if st.tags_gen != generation {
                    return;
                }
                st.loading_tags = false;
                st.tags = tags;
                st.selected.clone()
            };
            if let Some(tag) = selected_to_reload {
                Self::load_notes_for_tag(&inner, tag);
            }
            Self::render(&inner);
        });
    }

    #[cfg(feature = "automation")]
    pub fn click_tag(&self, index: usize) -> bool {
        let button = self
            .inner
            .state
            .borrow()
            .tag_buttons
            .get(index)
            .map(|(_, b)| b.clone());
        let Some(button) = button else {
            return false;
        };
        button.emit_clicked();
        true
    }

    #[cfg(feature = "automation")]
    pub fn click_note(&self, index: usize) -> bool {
        let button = self
            .inner
            .state
            .borrow()
            .note_buttons
            .get(index)
            .map(|(_, b)| b.clone());
        let Some(button) = button else {
            return false;
        };
        button.emit_clicked();
        true
    }

    #[cfg(feature = "automation")]
    pub fn tag_labels(&self) -> Vec<String> {
        self.inner
            .state
            .borrow()
            .tag_buttons
            .iter()
            .map(|(tag, _)| tag.clone())
            .collect()
    }

    fn bind_filter(inner: &Rc<TagsPanelInner>) {
        let weak = Rc::downgrade(inner);
        inner.filter_entry.connect_search_changed(move |entry| {
            let Some(inner) = weak.upgrade() else { return };
            let text = entry.text().to_string();
            {
                let mut st = inner.state.borrow_mut();
                st.filter = text;
                if st.programmatic_filter {
                    st.programmatic_filter = false;
                } else {
                    st.selected = None;
                    st.selected_notes.clear();
                    st.selected_notes_for = None;
                }
            }
            Self::render(&inner);
        });
    }

    fn load_notes_for_tag(inner: &Rc<TagsPanelInner>, tag: String) {
        let (handle, generation) = {
            let mut st = inner.state.borrow_mut();
            let Some(handle) = st.handle.clone() else {
                return;
            };
            st.notes_gen = st.notes_gen.wrapping_add(1);
            st.loading_notes = true;
            (handle, st.notes_gen)
        };
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let tag_bg = tag.clone();
            let notes = gio::spawn_blocking(move || handle.notes_with_tag(tag_bg))
                .await
                .unwrap_or_default();
            let Some(inner) = weak.upgrade() else { return };
            {
                let mut st = inner.state.borrow_mut();
                if st.notes_gen != generation || st.selected.as_deref() != Some(tag.as_str()) {
                    return;
                }
                st.loading_notes = false;
                st.selected_notes = notes;
                st.selected_notes_for = Some(tag);
            }
            Self::render(&inner);
        });
    }

    fn render(inner: &Rc<TagsPanelInner>) {
        let (tags, filter, selected, selected_notes, selected_notes_for, loading_notes) = {
            let st = inner.state.borrow();
            (
                st.tags
                    .iter()
                    .map(|tag| (tag.tag.clone(), tag.raw.clone(), tag.count))
                    .collect::<Vec<_>>(),
                st.filter.clone(),
                st.selected.clone(),
                st.selected_notes.clone(),
                st.selected_notes_for.clone(),
                st.loading_notes,
            )
        };
        clear_box(&inner.list);
        {
            let mut st = inner.state.borrow_mut();
            st.tag_buttons.clear();
            st.note_buttons.clear();
        }
        let filter_lower = filter.trim().trim_start_matches('#').to_lowercase();
        let visible: Vec<&(String, String, u64)> = tags
            .iter()
            .filter(|t| {
                filter_lower.is_empty()
                    || t.0.to_lowercase().contains(&filter_lower)
                    || t.1.to_lowercase().contains(&filter_lower)
            })
            .collect();
        if visible.is_empty() {
            let text = if tags.is_empty() {
                tr(Key::TagsEmptyFolder)
            } else {
                tr(Key::TagsNoMatch)
            };
            inner.list.append(&dim_label(text));
            let mut st = inner.state.borrow_mut();
            st.visible_tags = 0;
            st.visible_notes = 0;
            inner.status.set_label(text);
            return;
        }
        let sel_suffix = selected
            .as_deref()
            .map(|s| fmt1(Key::TagsSelectedSuffix, s))
            .unwrap_or_default();
        inner
            .status
            .set_label(&fmt2(Key::TagsCount, tags.len(), sel_suffix));

        let mut visible_notes = 0usize;
        let mut rows = 0usize;
        for tag in visible {
            rows += 1;
            let label = format!("#{} ({})", tag.0, tag.2);
            let btn = gtk4::Button::with_label(&label);
            btn.add_css_class("hortus-tag-chip");
            btn.set_halign(gtk4::Align::Start);
            let is_selected = selected.as_deref() == Some(tag.0.as_str());
            if is_selected {
                btn.add_css_class("suggested-action");
                btn.add_css_class("hortus-tag-chip-selected");
            } else {
                btn.add_css_class("flat");
            }
            let selected_tag = tag.0.clone();
            let selected_tag_cb = tag.0.clone();
            let weak = Rc::downgrade(inner);
            btn.connect_clicked(move |_| {
                let Some(inner) = weak.upgrade() else { return };
                let current = inner.state.borrow().selected.clone();
                if current.as_deref() == Some(selected_tag_cb.as_str()) {
                    let mut st = inner.state.borrow_mut();
                    st.selected = None;
                    st.selected_notes.clear();
                    st.selected_notes_for = None;
                    st.loading_notes = false;
                    drop(st);
                    Self::render(&inner);
                } else {
                    {
                        let mut st = inner.state.borrow_mut();
                        st.selected = Some(selected_tag_cb.clone());
                        st.selected_notes.clear();
                        st.selected_notes_for = None;
                    }
                    Self::load_notes_for_tag(&inner, selected_tag_cb.clone());
                    Self::render(&inner);
                }
            });
            inner
                .state
                .borrow_mut()
                .tag_buttons
                .push((selected_tag, btn.clone()));
            inner.list.append(&btn);

            if is_selected {
                if selected_notes_for.as_deref() == Some(tag.0.as_str()) {
                    visible_notes = selected_notes.len();
                    for note in &selected_notes {
                        let row = entry_row(note, None, Some(tr(Key::TagsOpenAndJump)));
                        let open_btn = gtk4::Button::builder()
                            .child(&row)
                            .halign(gtk4::Align::Fill)
                            .build();
                        open_btn.add_css_class("flat");
                        open_btn.add_css_class("hortus-result-row");
                        open_btn.set_margin_start(14);
                        let path = note.clone();
                        let weak = Rc::downgrade(inner);
                        open_btn.connect_clicked(move |_| {
                            let cb = weak
                                .upgrade()
                                .and_then(|inner| inner.state.borrow().on_open.clone());
                            if let Some(cb) = cb {
                                cb(path.clone(), None);
                            }
                        });
                        inner
                            .state
                            .borrow_mut()
                            .note_buttons
                            .push((note.clone(), open_btn.clone()));
                        inner.list.append(&open_btn);
                    }
                    if visible_notes == 0 {
                        inner.list.append(&dim_label(tr(Key::TagsNoNotes)));
                    }
                } else if loading_notes {
                    inner.list.append(&dim_label(tr(Key::TagsLoadingNotes)));
                }
            }
        }
        let mut st = inner.state.borrow_mut();
        st.visible_tags = rows;
        st.visible_notes = visible_notes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_panel_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let panel = TagsPanel::new();
        let weak = Rc::downgrade(&panel.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(panel);
        assert_eq!(weak.strong_count(), 0);
    }
}
