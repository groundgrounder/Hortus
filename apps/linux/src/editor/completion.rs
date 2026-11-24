// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 编辑态 `[[` 双链自动补全：仅读取当前光标行切片触发，Weak 解环（C-05 修复）。

use crate::core::Utf16Mapper;
use crate::knowledge::widgets::{dim_label, entry_row};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{LinkCompletionInfo, VaultHandle};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct CompletionState {
    handle: Option<Arc<VaultHandle>>,
    generation: u64,
    link_start: Option<u64>,
    items: Vec<LinkCompletionInfo>,
    selected: usize,
    accepted: u32,
    last_inserted: Option<String>,
    last_query: String,
    debounce_source: Option<glib::SourceId>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct CompletionStats {
    pub queries: u32,
    pub accepted: u32,
}

struct CompletionInner {
    panel: gtk4::Box,
    list: gtk4::ListBox,
    state: RefCell<CompletionState>,
    view: glib::WeakRef<gtk4::TextView>,
    overlay: glib::WeakRef<gtk4::Overlay>,
    queries: Cell<u32>,
}

impl Drop for CompletionInner {
    fn drop(&mut self) {
        if let Some(src) = self.state.borrow_mut().debounce_source.take() {
            src.remove();
        }
    }
}

#[derive(Clone)]
pub struct LinkCompletion {
    inner: Rc<CompletionInner>,
}

impl Default for LinkCompletion {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkCompletion {
    pub fn new() -> Self {
        let panel = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        panel.add_css_class("hortus-completion-popover");
        panel.add_css_class("hortus-completion-panel");
        panel.set_halign(gtk4::Align::Start);
        panel.set_valign(gtk4::Align::Start);
        panel.set_size_request(320, -1);
        panel.set_can_focus(false);
        panel.set_focusable(false);
        panel.set_visible(false);
        let header = dim_label("插入双链  Enter 确认 · Esc 关闭");
        header.set_margin_top(4);
        header.set_margin_bottom(4);
        header.set_margin_start(10);
        let list = gtk4::ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.set_focusable(false);
        panel.append(&header);
        panel.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        panel.append(&list);

        Self {
            inner: Rc::new(CompletionInner {
                panel,
                list,
                state: RefCell::new(CompletionState::default()),
                view: glib::WeakRef::new(),
                overlay: glib::WeakRef::new(),
                queries: Cell::new(0),
            }),
        }
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.inner.state.borrow_mut().handle = Some(handle);
    }

    pub fn hide(&self) {
        Self::hide_inner(&self.inner);
    }

    pub fn teardown(&self) {
        Self::hide_inner(&self.inner);
        self.inner.view.set(None);
        self.inner.overlay.set(None);
        if let Some(src) = self.inner.state.borrow_mut().debounce_source.take() {
            src.remove();
        }
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> CompletionStats {
        let st = self.inner.state.borrow();
        CompletionStats {
            queries: self.inner.queries.get(),
            accepted: st.accepted,
        }
    }

    #[cfg(feature = "automation")]
    pub fn accept_selected(&self) {
        Self::accept_impl(&self.inner);
    }

    #[cfg(feature = "automation")]
    pub fn is_visible(&self) -> bool {
        self.inner.panel.is_visible()
    }

    #[cfg(feature = "automation")]
    pub fn popover_focusable(&self) -> bool {
        self.inner.panel.can_focus()
    }

    #[cfg(feature = "automation")]
    pub fn item_count(&self) -> usize {
        self.inner.state.borrow().items.len()
    }

    #[cfg(feature = "automation")]
    pub fn first_insert_target(&self) -> Option<String> {
        self.inner
            .state
            .borrow()
            .items
            .first()
            .map(|item| item.insert_target.clone())
    }

    /// 绑定编辑视图：仅捕获 `Weak<CompletionInner>`，避免关闭标签页后循环泄漏。
    pub fn bind(&self, view: &gtk4::TextView, buffer: &gtk4::TextBuffer, overlay: &gtk4::Overlay) {
        self.inner.view.set(Some(view));
        self.inner.overlay.set(Some(overlay));
        overlay.add_overlay(&self.inner.panel);

        {
            let weak = Rc::downgrade(&self.inner);
            buffer.connect_changed(move |buf| {
                if let Some(inner) = weak.upgrade() {
                    Self::refresh_for_cursor(&inner, buf);
                }
            });
        }

        let key = gtk4::EventControllerKey::new();
        key.set_propagation_phase(gtk4::PropagationPhase::Capture);
        {
            let weak = Rc::downgrade(&self.inner);
            key.connect_key_pressed(move |_, keyval, _, _| {
                use gtk4::gdk::Key;
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !inner.panel.is_visible() {
                    return glib::Propagation::Proceed;
                }
                match keyval {
                    Key::Down => {
                        Self::move_selection(&inner, true);
                        glib::Propagation::Stop
                    }
                    Key::Up => {
                        Self::move_selection(&inner, false);
                        glib::Propagation::Stop
                    }
                    Key::Return | Key::KP_Enter | Key::Tab => {
                        Self::accept_impl(&inner);
                        glib::Propagation::Stop
                    }
                    Key::Escape => {
                        Self::hide_inner(&inner);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        view.add_controller(key);
    }

    fn hide_inner(inner: &Rc<CompletionInner>) {
        inner.panel.set_visible(false);
        let mut st = inner.state.borrow_mut();
        st.items.clear();
        st.link_start = None;
    }

    fn move_selection(inner: &Rc<CompletionInner>, down: bool) {
        let mut st = inner.state.borrow_mut();
        if st.items.is_empty() {
            return;
        }
        if down {
            st.selected = (st.selected + 1).min(st.items.len() - 1);
        } else {
            st.selected = st.selected.saturating_sub(1);
        }
        render_completion_items(&inner.list, &st);
    }

    fn refresh_for_cursor(inner: &Rc<CompletionInner>, buffer: &gtk4::TextBuffer) {
        let Some(view) = inner.view.upgrade() else {
            return;
        };
        let insert = buffer.iter_at_mark(&buffer.get_insert());
        let cursor_char = insert.offset().max(0) as usize;
        // 仅读取当前光标所在行切片（line_start..cursor），避免每次按键对整篇大文档构造 Vec<char>。
        let mut line_start_iter = insert;
        line_start_iter.set_line_offset(0);
        let prefix = buffer.text(&line_start_iter, &insert, false).to_string();
        let Some(open) = prefix.rfind("[[") else {
            Self::hide_inner(inner);
            return;
        };
        if prefix[open..].contains("]]") {
            Self::hide_inner(inner);
            return;
        }
        let query = prefix[open + 2..].to_string();
        if query.chars().any(|c| "[]#|".contains(c)) {
            Self::hide_inner(inner);
            return;
        }
        let embed = open > 0 && prefix.as_bytes().get(open - 1) == Some(&b'!');
        let prefix_chars_before_open = prefix[..open].chars().count() - usize::from(embed);
        let link_start_char = line_start_iter.offset().max(0) as usize + prefix_chars_before_open;

        // 仅在确认处于未闭合 `[[` 上下文时才构造补全候选文本。
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let mapper = Utf16Mapper::new(&text);
        let link_start_u16 = mapper.char_to_utf16(link_start_char) as u64;
        let cursor_byte = mapper.char_to_byte(cursor_char);
        let has_closer = text.get(cursor_byte..).is_some_and(|s| s.starts_with("]]"));
        let mut candidate = text;
        if !has_closer {
            let ins = if query.is_empty() { "x]]" } else { "]]" };
            candidate.insert_str(cursor_byte, ins);
        } else if query.is_empty() {
            candidate.insert(cursor_byte, 'x');
        }

        let generation = {
            let mut st = inner.state.borrow_mut();
            if st.handle.is_none() {
                return;
            }
            if let Some(old) = st.debounce_source.take() {
                old.remove();
            }
            st.generation += 1;
            st.link_start = Some(link_start_u16);
            st.last_query = query.clone();
            st.generation
        };
        inner.queries.set(inner.queries.get() + 1);

        let weak_timer = Rc::downgrade(inner);
        let view_weak = view.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(90), move || {
            let Some(inner) = weak_timer.upgrade() else {
                return;
            };
            inner.state.borrow_mut().debounce_source.take();
            let handle = {
                let st = inner.state.borrow();
                if st.generation != generation {
                    return;
                }
                st.handle.clone()
            };
            let Some(handle) = handle else { return };
            let weak_done = Rc::downgrade(&inner);
            let view_weak = view_weak.clone();
            glib::MainContext::default().spawn_local(async move {
                let items = gio::spawn_blocking(move || {
                    handle.link_completions_at(candidate, link_start_u16, query, 8)
                })
                .await
                .unwrap_or_default();
                let items: Vec<_> = items
                    .into_iter()
                    .filter(|item| !item.insert_target.contains(['[', ']', '#', '|']))
                    .collect();
                let Some(inner) = weak_done.upgrade() else {
                    return;
                };
                {
                    let mut st = inner.state.borrow_mut();
                    if st.generation != generation {
                        return;
                    }
                    st.items = items;
                    st.selected = 0;
                    if st.items.is_empty() {
                        drop(st);
                        inner.panel.set_visible(false);
                        return;
                    }
                    render_completion_items(&inner.list, &st);
                }
                if let Some(view) = view_weak.upgrade() {
                    Self::show_at_cursor(&inner, &view);
                }
            });
        });
        inner.state.borrow_mut().debounce_source = Some(id);
    }

    fn show_at_cursor(inner: &Rc<CompletionInner>, view: &gtk4::TextView) {
        let Some(overlay) = inner.overlay.upgrade() else {
            return;
        };
        let buffer = view.buffer();
        let iter = buffer.iter_at_mark(&buffer.get_insert());
        let rect = view.iter_location(&iter);
        let (x, y) = view.buffer_to_window_coords(
            gtk4::TextWindowType::Widget,
            rect.x(),
            rect.y() + rect.height(),
        );
        let origin = gtk4::graphene::Point::new(x as f32, y as f32);
        let Some(point) = view.compute_point(&overlay, &origin) else {
            return;
        };
        let margin_start = (point.x().max(0.0) as i32).min((overlay.width() - 340).max(0));
        inner.panel.set_margin_start(margin_start);
        inner
            .panel
            .set_margin_top((point.y().max(0.0) as i32).min(overlay.height()));
        inner.panel.set_visible(true);
        inner.panel.queue_resize();
    }

    fn accept_impl(inner: &Rc<CompletionInner>) {
        let (target, link_start) = {
            let st = inner.state.borrow();
            let Some(item) = st.items.get(st.selected) else {
                return;
            };
            let Some(link_start) = st.link_start else {
                return;
            };
            (item.insert_target.clone(), link_start)
        };
        let Some(view) = inner.view.upgrade() else {
            return;
        };
        let buffer = view.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let mapper = Utf16Mapper::new(&text);
        let start_char = mapper.utf16_to_char(link_start as usize);
        let cursor_char = buffer.iter_at_mark(&buffer.get_insert()).offset().max(0) as usize;
        let has_closer = buffer.iter_at_offset(cursor_char as i32).char() == ']'
            && buffer.iter_at_offset(cursor_char as i32 + 1).char() == ']';
        let delete_end = cursor_char + if has_closer { 2 } else { 0 };
        let insert = format!("[[{target}]]");
        let insert_len = insert.chars().count();
        buffer.begin_user_action();
        {
            let mut s = buffer.iter_at_offset(start_char as i32);
            let mut e = buffer.iter_at_offset(delete_end as i32);
            buffer.delete(&mut s, &mut e);
            buffer.insert(&mut buffer.iter_at_offset(start_char as i32), &insert);
        }
        buffer.end_user_action();
        Self::hide_inner(inner);
        {
            let mut st = inner.state.borrow_mut();
            st.accepted += 1;
            st.last_inserted = Some(insert);
        }
        let cursor = buffer.iter_at_offset((start_char + insert_len) as i32);
        buffer.place_cursor(&cursor);
        view.grab_focus();
    }
}

fn render_completion_items(list: &gtk4::ListBox, state: &CompletionState) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    for (index, item) in state.items.iter().enumerate() {
        let row = gtk4::ListBoxRow::new();
        row.set_activatable(true);
        row.set_focusable(false);
        let label = entry_row(&item.title, None, Some(&item.path));
        label.set_margin_start(6);
        label.set_margin_end(6);
        row.set_child(Some(&label));
        if index == state.selected {
            row.add_css_class("suggested-action");
        }
        list.append(&row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_completion_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let overlay = gtk4::Overlay::new();
        let buffer = gtk4::TextBuffer::new(None);
        let view = gtk4::TextView::with_buffer(&buffer);
        overlay.set_child(Some(&view));

        let completion = LinkCompletion::new();
        completion.bind(&view, &buffer, &overlay);
        let weak = Rc::downgrade(&completion.inner);
        assert_eq!(weak.strong_count(), 1);
        completion.teardown();
        drop(completion);
        assert_eq!(weak.strong_count(), 0);
    }
}
