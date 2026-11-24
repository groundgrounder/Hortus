// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 文内查找与替换：支持单编辑器缓冲区与阅读视图跨分段扫描（弱引用解环，零整篇 `Vec<char>` 分配）。

use crate::core::find_char_ranges;
use crate::reading::ReadingSurfaceHandle;
use gtk4::glib;
use gtk4::graphene;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const MAX_HIGHLIGHTS: usize = 4000;

#[derive(Clone)]
struct FindTarget {
    view: glib::WeakRef<gtk4::TextView>,
    buffer: glib::WeakRef<gtk4::TextBuffer>,
}

#[derive(Clone, Copy)]
struct FindMatch {
    target_idx: usize,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct FindState {
    targets: Vec<FindTarget>,
    scrolled: glib::WeakRef<gtk4::ScrolledWindow>,
    content_box: glib::WeakRef<gtk4::Box>,
    editable: bool,
    matches: Vec<FindMatch>,
    current: usize,
    query: String,
    replaced: u32,
    searches: u32,
    debounce_source: Option<glib::SourceId>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct FindStats {
    pub open: bool,
    pub matches: usize,
    pub current: usize,
    pub current_segment: Option<usize>,
    pub segments: usize,
    pub editable: bool,
    pub replaced: u32,
    pub searches: u32,
}

struct FindBarInner {
    bar: gtk4::SearchBar,
    entry: gtk4::SearchEntry,
    replace_entry: gtk4::Entry,
    replace_box: gtk4::Box,
    status: gtk4::Label,
    state: RefCell<FindState>,
}

impl Drop for FindBarInner {
    fn drop(&mut self) {
        if let Some(src) = self.state.borrow_mut().debounce_source.take() {
            src.remove();
        }
    }
}

#[derive(Clone)]
pub struct FindBar {
    inner: Rc<FindBarInner>,
}

impl Default for FindBar {
    fn default() -> Self {
        Self::new()
    }
}

impl FindBar {
    pub fn new() -> Self {
        let bar = gtk4::SearchBar::new();
        bar.add_css_class("hortus-find-bar");
        bar.set_show_close_button(true);

        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        let find_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("查找 (Ctrl+F)")
            .hexpand(true)
            .build();
        let status = gtk4::Label::new(Some(""));
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_width_chars(10);

        let prev = gtk4::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("上一个匹配 (Shift+Enter)")
            .build();
        prev.add_css_class("flat");
        let next = gtk4::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("下一个匹配 (Enter)")
            .build();
        next.add_css_class("flat");

        find_row.append(&entry);
        find_row.append(&status);
        find_row.append(&prev);
        find_row.append(&next);

        let replace_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let replace_entry = gtk4::Entry::builder()
            .placeholder_text("替换为 (Ctrl+H)")
            .hexpand(true)
            .build();
        let replace_one = gtk4::Button::with_label("替换");
        replace_one.add_css_class("flat");
        let replace_all = gtk4::Button::with_label("全部替换");
        replace_all.add_css_class("flat");
        replace_box.append(&replace_entry);
        replace_box.append(&replace_one);
        replace_box.append(&replace_all);
        replace_box.set_visible(false);

        root.append(&find_row);
        root.append(&replace_box);
        bar.set_child(Some(&root));

        let inner = Rc::new(FindBarInner {
            bar,
            entry,
            replace_entry,
            replace_box,
            status,
            state: RefCell::new(FindState::default()),
        });
        Self::bind_signals(&inner, &prev, &next, &replace_one, &replace_all);
        Self { inner }
    }

    fn bind_signals(
        inner: &Rc<FindBarInner>,
        prev: &gtk4::Button,
        next: &gtk4::Button,
        replace_one: &gtk4::Button,
        replace_all: &gtk4::Button,
    ) {
        {
            let weak = Rc::downgrade(inner);
            inner.bar.connect_search_mode_enabled_notify(move |bar| {
                if !bar.is_search_mode() {
                    let view = weak.upgrade().and_then(|inner| {
                        inner
                            .state
                            .borrow()
                            .targets
                            .first()
                            .and_then(|t| t.view.upgrade())
                    });
                    if let Some(view) = view {
                        view.grab_focus();
                    }
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_search_changed(move |entry| {
                let Some(inner) = weak.upgrade() else { return };
                let query = entry.text().to_string();
                {
                    let mut st = inner.state.borrow_mut();
                    st.query = query.clone();
                    if let Some(old) = st.debounce_source.take() {
                        old.remove();
                    }
                }
                let weak_timer = Rc::downgrade(&inner);
                let id = glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    let Some(inner) = weak_timer.upgrade() else {
                        return;
                    };
                    inner.state.borrow_mut().debounce_source.take();
                    if inner.state.borrow().query != query {
                        return;
                    }
                    Self::recompute(&inner, true);
                });
                inner.state.borrow_mut().debounce_source = Some(id);
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_activate(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::next_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_next_match(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::next_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_previous_match(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::prev_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.entry.connect_stop_search(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::close_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            prev.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::prev_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            next.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::next_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            replace_one.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::replace_current_impl(&inner);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            replace_all.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self::replace_all_impl(&inner);
                }
            });
        }
    }

    pub fn root(&self) -> &gtk4::SearchBar {
        &self.inner.bar
    }

    /// 绑定单一原生正文控件（编辑视图）。
    pub fn bind(&self, view: &gtk4::TextView, buffer: &gtk4::TextBuffer, editable: bool) {
        {
            let mut st = self.inner.state.borrow_mut();
            let v_ref = glib::WeakRef::new();
            v_ref.set(Some(view));
            let b_ref = glib::WeakRef::new();
            b_ref.set(Some(buffer));
            st.targets = vec![FindTarget {
                view: v_ref,
                buffer: b_ref,
            }];
            st.scrolled.set(None);
            st.content_box.set(None);
            st.editable = editable;
            st.matches.clear();
            st.current = 0;
            st.query = self.inner.entry.text().to_string();
        }
        self.inner.replace_box.set_visible(editable);
    }

    /// 绑定分段块级阅读表面，顺序扫描全部 `TextSegment`。
    pub fn bind_reading(&self, surface: &ReadingSurfaceHandle) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.targets = surface
                .text_views()
                .into_iter()
                .map(|(view, buffer)| {
                    let v_ref = glib::WeakRef::new();
                    v_ref.set(Some(&view));
                    let b_ref = glib::WeakRef::new();
                    b_ref.set(Some(&buffer));
                    FindTarget {
                        view: v_ref,
                        buffer: b_ref,
                    }
                })
                .collect();
            st.scrolled.set(Some(&surface.scrolled));
            st.content_box.set(Some(&surface.content_box));
            st.editable = false;
            st.matches.clear();
            st.current = 0;
            st.query = self.inner.entry.text().to_string();
        }
        self.inner.replace_box.set_visible(false);
    }

    pub fn open(&self, editable: bool) {
        self.inner.replace_box.set_visible(editable);
        self.inner.bar.set_search_mode(true);
        self.inner.entry.grab_focus();
        self.inner.entry.select_region(0, -1);
        Self::recompute(&self.inner, true);
    }

    #[cfg(feature = "automation")]
    pub fn close(&self) {
        Self::close_impl(&self.inner);
    }

    pub fn is_open(&self) -> bool {
        self.inner.bar.is_search_mode()
    }

    #[cfg(feature = "automation")]
    pub fn set_query(&self, query: &str) {
        self.inner.entry.set_text(query);
        self.inner.state.borrow_mut().query = query.to_string();
        Self::recompute(&self.inner, true);
    }

    #[cfg(feature = "automation")]
    pub fn set_replacement(&self, text: &str) {
        self.inner.replace_entry.set_text(text);
    }

    /// 源码样式重放会清空所有标签；查找条打开时重新施加匹配高亮。
    pub fn refresh_highlights(&self) {
        if self.inner.bar.is_search_mode() {
            Self::highlight_and_scroll(&self.inner);
        }
    }

    /// 表面重建后若查找条仍处于打开状态，重新在各分段中计算高亮。
    pub fn recompute_if_open(&self) {
        if self.inner.bar.is_search_mode() {
            Self::recompute(&self.inner, true);
        }
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> FindStats {
        let st = self.inner.state.borrow();
        FindStats {
            open: self.inner.bar.is_search_mode(),
            matches: st.matches.len(),
            current: st.current,
            current_segment: st.matches.get(st.current).map(|m| m.target_idx),
            segments: st.targets.len(),
            editable: st.editable,
            replaced: st.replaced,
            searches: st.searches,
        }
    }

    #[cfg(feature = "automation")]
    pub fn next(&self) {
        Self::next_impl(&self.inner);
    }

    #[cfg(feature = "automation")]
    pub fn replace_all(&self) {
        Self::replace_all_impl(&self.inner);
    }

    pub fn teardown(&self) {
        let mut st = self.inner.state.borrow_mut();
        if let Some(src) = st.debounce_source.take() {
            src.remove();
        }
        st.targets.clear();
        st.scrolled.set(None);
        st.content_box.set(None);
        st.matches.clear();
    }

    fn close_impl(inner: &Rc<FindBarInner>) {
        inner.bar.set_search_mode(false);
        let view = inner
            .state
            .borrow()
            .targets
            .first()
            .and_then(|t| t.view.upgrade());
        if let Some(view) = view {
            view.grab_focus();
        }
    }

    fn next_impl(inner: &Rc<FindBarInner>) {
        {
            let mut st = inner.state.borrow_mut();
            if st.matches.is_empty() {
                return;
            }
            st.current = (st.current + 1) % st.matches.len();
        }
        Self::highlight_and_scroll(inner);
    }

    fn prev_impl(inner: &Rc<FindBarInner>) {
        {
            let mut st = inner.state.borrow_mut();
            if st.matches.is_empty() {
                return;
            }
            let len = st.matches.len();
            st.current = (st.current + len - 1) % len;
        }
        Self::highlight_and_scroll(inner);
    }

    fn replace_current_impl(inner: &Rc<FindBarInner>) {
        let (buffer, m) = {
            let st = inner.state.borrow();
            if !st.editable {
                return;
            }
            let Some(&m) = st.matches.get(st.current) else {
                return;
            };
            let Some(buffer) = st
                .targets
                .get(m.target_idx)
                .and_then(|t| t.buffer.upgrade())
            else {
                return;
            };
            (buffer, m)
        };
        let replacement = inner.replace_entry.text().to_string();
        buffer.begin_user_action();
        {
            let mut s = buffer.iter_at_offset(m.start as i32);
            let mut e = buffer.iter_at_offset(m.end as i32);
            buffer.delete(&mut s, &mut e);
            buffer.insert(&mut buffer.iter_at_offset(m.start as i32), &replacement);
        }
        buffer.end_user_action();
        inner.state.borrow_mut().replaced += 1;
        Self::recompute(inner, true);
        inner.entry.grab_focus();
    }

    fn replace_all_impl(inner: &Rc<FindBarInner>) {
        let (buffer, ranges, replacement) = {
            let st = inner.state.borrow();
            if !st.editable {
                return;
            }
            let Some(buffer) = st.targets.first().and_then(|t| t.buffer.upgrade()) else {
                return;
            };
            let ranges: Vec<(usize, usize)> = st
                .matches
                .iter()
                .filter(|m| m.target_idx == 0)
                .map(|m| (m.start, m.end))
                .collect();
            (buffer, ranges, inner.replace_entry.text().to_string())
        };
        if ranges.is_empty() {
            return;
        }
        buffer.begin_user_action();
        for (start, end) in ranges.iter().rev() {
            let mut s = buffer.iter_at_offset(*start as i32);
            let mut e = buffer.iter_at_offset(*end as i32);
            buffer.delete(&mut s, &mut e);
            buffer.insert(&mut buffer.iter_at_offset(*start as i32), &replacement);
        }
        buffer.end_user_action();
        inner.state.borrow_mut().replaced += ranges.len() as u32;
        Self::recompute(inner, true);
    }

    fn recompute(inner: &Rc<FindBarInner>, keep_current: bool) {
        let (buffers, query, keep) = {
            let st = inner.state.borrow();
            let buffers: Vec<(usize, gtk4::TextBuffer)> = st
                .targets
                .iter()
                .enumerate()
                .filter_map(|(idx, t)| t.buffer.upgrade().map(|buf| (idx, buf)))
                .collect();
            (
                buffers,
                inner.entry.text().to_string(),
                if keep_current { st.current } else { 0 },
            )
        };
        if buffers.is_empty() {
            return;
        }
        let mut matches = Vec::new();
        if !query.is_empty() {
            for (target_idx, buffer) in buffers {
                let remaining = MAX_HIGHLIGHTS.saturating_sub(matches.len());
                if remaining == 0 {
                    break;
                }
                let text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                for (start, end) in find_char_ranges(&text, &query, remaining) {
                    matches.push(FindMatch {
                        target_idx,
                        start,
                        end,
                    });
                }
            }
        }
        let (matches_len, current_idx, is_empty_query) = {
            let mut st = inner.state.borrow_mut();
            st.matches = matches;
            st.query = query.clone();
            st.searches += 1;
            if !st.matches.is_empty() {
                st.current = keep.min(st.matches.len() - 1);
            } else {
                st.current = 0;
            }
            (st.matches.len(), st.current, st.query.is_empty())
        };
        Self::highlight_and_scroll(inner);
        if matches_len == 0 {
            inner
                .status
                .set_label(if is_empty_query { "" } else { "无匹配" });
        } else {
            inner
                .status
                .set_label(&format!("{}/{}", current_idx + 1, matches_len));
        }
    }

    fn highlight_and_scroll(inner: &Rc<FindBarInner>) {
        let (targets, matches, current, scrolled, content_box) = {
            let st = inner.state.borrow();
            (
                st.targets.clone(),
                st.matches.clone(),
                st.current,
                st.scrolled.upgrade(),
                st.content_box.upgrade(),
            )
        };
        Self::apply_highlights(inner, &targets, &matches, current);
        let Some(m) = matches.get(current) else {
            return;
        };
        let Some(target) = targets.get(m.target_idx) else {
            return;
        };
        let (Some(view), Some(buffer)) = (target.view.upgrade(), target.buffer.upgrade()) else {
            return;
        };
        let mut iter = buffer.iter_at_offset(m.start as i32);
        if let (Some(scrolled), Some(content_box)) = (scrolled, content_box) {
            let rect = view.iter_location(&iter);
            let (wx, wy) =
                view.buffer_to_window_coords(gtk4::TextWindowType::Widget, rect.x(), rect.y());
            if let Some(pt) =
                view.compute_point(&content_box, &graphene::Point::new(wx as f32, wy as f32))
            {
                let adj = scrolled.vadjustment();
                let target_y = (f64::from(pt.y()) - adj.page_size() * 0.3).max(0.0);
                let max_y = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value(target_y.min(max_y));
            }
        } else {
            view.scroll_to_iter(&mut iter, 0.2, false, 0.0, 0.3);
        }
    }

    fn apply_highlights(
        inner: &Rc<FindBarInner>,
        targets: &[FindTarget],
        matches: &[FindMatch],
        current: usize,
    ) {
        for (target_idx, target) in targets.iter().enumerate() {
            let Some(buffer) = target.buffer.upgrade() else {
                continue;
            };
            buffer.begin_irreversible_action();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            buffer.remove_tag_by_name("find_match", &start, &end);
            buffer.remove_tag_by_name("find_current", &start, &end);
            for (global_idx, m) in matches.iter().enumerate().take(MAX_HIGHLIGHTS) {
                if m.target_idx != target_idx {
                    continue;
                }
                let s = buffer.iter_at_offset(m.start as i32);
                let e = buffer.iter_at_offset(m.end as i32);
                if global_idx == current {
                    buffer.apply_tag_by_name("find_current", &s, &e);
                } else {
                    buffer.apply_tag_by_name("find_match", &s, &e);
                }
            }
            buffer.end_irreversible_action();
        }
        if !matches.is_empty() {
            inner
                .status
                .set_label(&format!("{}/{}", current + 1, matches.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_bar_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let find = FindBar::new();
        let weak = Rc::downgrade(&find.inner);
        assert_eq!(weak.strong_count(), 1);
        find.teardown();
        drop(find);
        assert_eq!(weak.strong_count(), 0);
    }
}
