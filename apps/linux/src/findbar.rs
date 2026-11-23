// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 文内查找与替换：阅读视图只查找，编辑视图额外提供替换行。
//! 匹配高亮与原文样式一样包裹在不可撤销动作中，不会污染原生撤销栈。

use crate::core::find_char_ranges;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const MAX_HIGHLIGHTS: usize = 4000;

#[derive(Default)]
struct FindState {
    view: Option<gtk4::TextView>,
    buffer: Option<gtk4::TextBuffer>,
    editable: bool,
    matches: Vec<(usize, usize)>,
    current: usize,
    query: String,
    replaced: u32,
    searches: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FindStats {
    pub open: bool,
    pub query: String,
    pub matches: usize,
    pub current: usize,
    pub editable: bool,
    pub replaced: u32,
    pub searches: u32,
}

pub struct FindBar {
    pub bar: gtk4::SearchBar,
    entry: gtk4::SearchEntry,
    replace_entry: gtk4::Entry,
    replace_box: gtk4::Box,
    status: gtk4::Label,
    state: Rc<RefCell<FindState>>,
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

        let find = Self {
            bar,
            entry,
            replace_entry,
            replace_box,
            status,
            state: Rc::new(RefCell::new(FindState::default())),
        };

        {
            let state = find.state.clone();
            find.bar.connect_search_mode_enabled_notify(move |bar| {
                if !bar.is_search_mode()
                    && let Some(view) = state.borrow().view.clone()
                {
                    view.grab_focus();
                }
            });
        }
        {
            let this = find.clone_handle();
            find.entry.connect_search_changed(move |entry| {
                let query = entry.text().to_string();
                this.state.borrow_mut().query = query.clone();
                // 防抖：连续输入时只在停顿后重新计算，避免每次按键全量扫描长文档。
                let this_timer = this.clone_handle();
                gtk4::glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    if this_timer.state.borrow().query != query {
                        return;
                    }
                    this_timer.recompute(true);
                });
            });
        }
        {
            let this = find.clone_handle();
            find.entry.connect_activate(move |_| this.next());
        }
        {
            let this = find.clone_handle();
            find.entry.connect_next_match(move |_| this.next());
        }
        {
            let this = find.clone_handle();
            find.entry.connect_previous_match(move |_| this.prev());
        }
        {
            let this = find.clone_handle();
            find.entry.connect_stop_search(move |_| this.close());
        }
        {
            let this = find.clone_handle();
            prev.connect_clicked(move |_| this.prev());
        }
        {
            let this = find.clone_handle();
            next.connect_clicked(move |_| this.next());
        }
        {
            let this = find.clone_handle();
            replace_one.connect_clicked(move |_| {
                this.replace_current();
            });
        }
        {
            let this = find.clone_handle();
            replace_all.connect_clicked(move |_| {
                this.replace_all();
            });
        }
        find
    }

    fn clone_handle(&self) -> FindBar {
        Self {
            bar: self.bar.clone(),
            entry: self.entry.clone(),
            replace_entry: self.replace_entry.clone(),
            replace_box: self.replace_box.clone(),
            status: self.status.clone(),
            state: self.state.clone(),
        }
    }

    pub fn root(&self) -> &gtk4::SearchBar {
        &self.bar
    }

    /// 绑定当前可见的原生正文控件；阅读视图为只读，编辑视图展开替换行。
    pub fn bind(&self, view: &gtk4::TextView, buffer: &gtk4::TextBuffer, editable: bool) {
        let rebind = {
            let mut st = self.state.borrow_mut();
            let same = st.buffer.as_ref().is_some_and(|old| old == buffer);
            st.view = Some(view.clone());
            st.buffer = Some(buffer.clone());
            st.editable = editable;
            st.matches.clear();
            st.current = 0;
            st.query = self.entry.text().to_string();
            !same
        };
        self.replace_box.set_visible(editable);
        let _ = rebind;
        // 不监听 buffer 的 changed 信号：替换/样式重放发生在用户动作内，
        // 此时再包裹不可撤销动作会与 GTK 的用户动作状态冲突。
        // 查找结果改在输入、上下跳转与替换完成后显式重算。
    }

    pub fn is_open(&self) -> bool {
        self.bar.is_search_mode()
    }

    pub fn open(&self, editable: bool) {
        self.replace_box.set_visible(editable);
        self.bar.set_search_mode(true);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
        self.recompute(true);
    }

    pub fn close(&self) {
        self.bar.set_search_mode(false);
        let view = self.state.borrow().view.clone();
        if let Some(view) = view {
            view.grab_focus();
        }
    }

    pub fn set_query(&self, query: &str) {
        self.entry.set_text(query);
        self.state.borrow_mut().query = query.to_string();
        self.recompute(true);
    }

    pub fn set_replacement(&self, text: &str) {
        self.replace_entry.set_text(text);
    }

    /// 源码样式重放会清空所有标签；查找条打开时重新施加匹配高亮。
    pub fn refresh_highlights(&self) {
        if self.bar.is_search_mode() {
            self.highlight_and_scroll();
        }
    }

    pub fn stats(&self) -> FindStats {
        let st = self.state.borrow();
        FindStats {
            open: self.bar.is_search_mode(),
            query: st.query.clone(),
            matches: st.matches.len(),
            current: st.current,
            editable: st.editable,
            replaced: st.replaced,
            searches: st.searches,
        }
    }

    pub fn next(&self) {
        {
            let mut st = self.state.borrow_mut();
            if st.matches.is_empty() {
                return;
            }
            st.current = (st.current + 1) % st.matches.len();
        }
        self.highlight_and_scroll();
    }

    pub fn prev(&self) {
        {
            let mut st = self.state.borrow_mut();
            if st.matches.is_empty() {
                return;
            }
            let len = st.matches.len();
            st.current = (st.current + len - 1) % len;
        }
        self.highlight_and_scroll();
    }

    pub fn replace_current(&self) {
        let (view, buffer, range) = {
            let st = self.state.borrow();
            if !st.editable {
                return;
            }
            let Some(ref buffer) = st.buffer else { return };
            let Some(&range) = st.matches.get(st.current) else {
                return;
            };
            (st.view.clone(), buffer.clone(), range)
        };
        let replacement = self.replace_entry.text().to_string();
        buffer.begin_user_action();
        {
            let mut s = buffer.iter_at_offset(range.0 as i32);
            let mut e = buffer.iter_at_offset(range.1 as i32);
            buffer.delete(&mut s, &mut e);
            buffer.insert(&mut buffer.iter_at_offset(range.0 as i32), &replacement);
        }
        buffer.end_user_action();
        self.state.borrow_mut().replaced += 1;
        self.recompute(true);
        let _ = view;
        self.entry.grab_focus();
    }

    pub fn replace_all(&self) {
        let (buffer, ranges, replacement) = {
            let st = self.state.borrow();
            if !st.editable {
                return;
            }
            let Some(ref buffer) = st.buffer else { return };
            (
                buffer.clone(),
                st.matches.clone(),
                self.replace_entry.text().to_string(),
            )
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
        {
            let mut st = self.state.borrow_mut();
            st.replaced += ranges.len() as u32;
        }
        self.recompute(true);
    }

    fn recompute(&self, keep_current: bool) {
        let (buffer, query, keep) = {
            let st = self.state.borrow();
            (
                st.buffer.clone(),
                self.entry.text().to_string(),
                if keep_current { st.current } else { 0 },
            )
        };
        let Some(buffer) = buffer else {
            return;
        };
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let matches = if query.is_empty() {
            Vec::new()
        } else {
            find_char_ranges(&text, &query, MAX_HIGHLIGHTS)
        };
        {
            let mut st = self.state.borrow_mut();
            st.matches = matches;
            st.query = query;
            st.searches += 1;
            if !st.matches.is_empty() {
                st.current = keep.min(st.matches.len() - 1);
            } else {
                st.current = 0;
            }
        }
        self.highlight_and_scroll();
        let stats = self.stats();
        if stats.matches == 0 {
            self.status.set_label(if stats.query.is_empty() {
                ""
            } else {
                "无匹配"
            });
        } else {
            self.status
                .set_label(&format!("{}/{}", stats.current + 1, stats.matches));
        }
    }

    fn highlight_and_scroll(&self) {
        let (view, buffer, matches, current) = {
            let st = self.state.borrow();
            (
                st.view.clone(),
                st.buffer.clone(),
                st.matches.clone(),
                st.current,
            )
        };
        let (Some(view), Some(buffer)) = (view, buffer) else {
            return;
        };
        self.apply_highlights();
        if let Some(&(start, _)) = matches.get(current) {
            let mut iter = buffer.iter_at_offset(start as i32);
            view.scroll_to_iter(&mut iter, 0.2, false, 0.0, 0.3);
        }
    }

    fn apply_highlights(&self) {
        let (buffer, matches, current) = {
            let st = self.state.borrow();
            (st.buffer.clone(), st.matches.clone(), st.current)
        };
        let Some(buffer) = buffer else {
            return;
        };
        buffer.begin_irreversible_action();
        {
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            buffer.remove_tag_by_name("find_match", &start, &end);
            buffer.remove_tag_by_name("find_current", &start, &end);
        }
        for (index, (start, end)) in matches.iter().enumerate().take(MAX_HIGHLIGHTS) {
            let s = buffer.iter_at_offset(*start as i32);
            let e = buffer.iter_at_offset(*end as i32);
            if index == current {
                buffer.apply_tag_by_name("find_current", &s, &e);
            } else {
                buffer.apply_tag_by_name("find_match", &s, &e);
            }
        }
        buffer.end_irreversible_action();
        if !matches.is_empty() {
            self.status
                .set_label(&format!("{}/{}", current + 1, matches.len()));
        }
    }
}

impl Clone for FindBar {
    fn clone(&self) -> Self {
        self.clone_handle()
    }
}
