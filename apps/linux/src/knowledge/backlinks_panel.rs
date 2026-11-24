// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 反向链接面板：`handle.backlinks()` 移至 `gio::spawn_blocking` 异步执行，弱引用解环（C-06）。

use crate::knowledge::widgets::{OpenNoteCallback, clear_box, dim_label, panel_scroll};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{BacklinkInfo, VaultHandle};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default)]
struct BacklinksState {
    handle: Option<Arc<VaultHandle>>,
    path: Option<String>,
    rows: usize,
    updates: u32,
    generation: u64,
    loading: bool,
    on_open: Option<OpenNoteCallback>,
    row_buttons: Vec<(String, u64, gtk4::Button)>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, Default)]
pub struct BacklinksPanelStats {
    pub path: Option<String>,
    pub rows: usize,
    pub updates: u32,
}

struct BacklinksPanelInner {
    root: gtk4::Box,
    title: gtk4::Label,
    list: gtk4::Box,
    state: RefCell<BacklinksState>,
}

#[derive(Clone)]
pub struct BacklinksPanel {
    inner: Rc<BacklinksPanelInner>,
}

impl Default for BacklinksPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl BacklinksPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let title = gtk4::Label::new(Some("未打开笔记"));
        title.add_css_class("dim-label");
        title.add_css_class("caption");
        title.set_xalign(0.0);
        title.set_margin_start(12);
        title.set_margin_end(12);
        title.set_margin_top(8);
        title.set_margin_bottom(6);
        title.set_wrap(true);

        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        list.set_margin_start(6);
        list.set_margin_end(6);
        list.set_margin_bottom(12);
        let scroll = panel_scroll(&list);

        root.append(&title);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&scroll);

        Self {
            inner: Rc::new(BacklinksPanelInner {
                root,
                title,
                list,
                state: RefCell::new(BacklinksState::default()),
            }),
        }
    }

    pub fn root(&self) -> &gtk4::Box {
        &self.inner.root
    }

    /// 触发第 `index` 行反链的真实点击回调；返回打开的源笔记与偏移。
    #[cfg(feature = "automation")]
    pub fn click_row(&self, index: usize) -> Option<(String, u64)> {
        let (source, start, button) = self.inner.state.borrow().row_buttons.get(index).cloned()?;
        button.emit_clicked();
        Some((source, start))
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.inner.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.inner.state.borrow_mut().on_open = Some(callback);
    }

    #[cfg(feature = "automation")]
    pub fn is_loading(&self) -> bool {
        self.inner.state.borrow().loading
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> BacklinksPanelStats {
        let st = self.inner.state.borrow();
        BacklinksPanelStats {
            path: st.path.clone(),
            rows: st.rows,
            updates: st.updates,
        }
    }

    #[cfg(feature = "automation")]
    pub fn current_path(&self) -> Option<String> {
        self.inner.state.borrow().path.clone()
    }

    /// 切换激活笔记或磁盘变化后异步刷新；相同路径且 `force` 为假时跳过重复查询。
    pub fn update(&self, path: Option<String>, force: bool) {
        let (handle, generation) = {
            let mut st = self.inner.state.borrow_mut();
            if !force && st.path == path {
                return;
            }
            st.path = path.clone();
            st.updates += 1;
            st.generation = st.generation.wrapping_add(1);
            st.row_buttons.clear();
            st.rows = 0;
            (st.handle.clone(), st.generation)
        };
        clear_box(&self.inner.list);
        let Some(path) = path else {
            self.inner.state.borrow_mut().loading = false;
            self.inner.title.set_label("未打开笔记");
            return;
        };
        let Some(handle) = handle else {
            self.inner.state.borrow_mut().loading = false;
            return;
        };
        self.inner.state.borrow_mut().loading = true;
        self.inner
            .title
            .set_label(&format!("正在查询「{path}」的反链…"));

        let weak = Rc::downgrade(&self.inner);
        glib::MainContext::default().spawn_local(async move {
            let path_bg = path.clone();
            let links = gio::spawn_blocking(move || handle.backlinks(path_bg))
                .await
                .unwrap_or_default();
            let Some(inner) = weak.upgrade() else { return };
            {
                let mut st = inner.state.borrow_mut();
                if st.generation != generation {
                    return;
                }
                st.loading = false;
                st.rows = links.len();
                st.row_buttons.clear();
            }
            inner
                .title
                .set_label(&format!("引用「{path}」的笔记（{} 条）", links.len()));
            if links.is_empty() {
                inner.list.append(&dim_label("还没有笔记引用这篇笔记"));
                return;
            }
            for link in &links {
                let button = Self::render_row(&inner, link);
                inner.state.borrow_mut().row_buttons.push((
                    link.source.clone(),
                    link.start_utf16,
                    button.clone(),
                ));
                inner.list.append(&button);
            }
        });
    }

    fn render_row(inner: &Rc<BacklinksPanelInner>, link: &BacklinkInfo) -> gtk4::Button {
        let count_hint = if link.embed { "嵌入" } else { "双链" };
        let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        let title = gtk4::Label::new(Some(&link.source));
        title.add_css_class("hortus-result-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        row.append(&title);
        if !link.context.trim().is_empty() {
            let snippet = gtk4::Label::new(Some(&link.context));
            snippet.add_css_class("hortus-result-snippet");
            snippet.set_xalign(0.0);
            snippet.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&snippet);
        }
        let meta = gtk4::Label::new(Some(&format!("L{} · {count_hint}", link.line + 1)));
        meta.add_css_class("hortus-result-meta");
        meta.set_xalign(0.0);
        row.append(&meta);

        let btn = gtk4::Button::builder()
            .child(&row)
            .halign(gtk4::Align::Fill)
            .build();
        btn.add_css_class("flat");
        btn.add_css_class("hortus-backlink-row");
        let path = link.source.clone();
        let start = link.start_utf16;
        let weak = Rc::downgrade(inner);
        btn.connect_clicked(move |_| {
            let cb = weak
                .upgrade()
                .and_then(|inner| inner.state.borrow().on_open.clone());
            if let Some(cb) = cb {
                cb(path.clone(), Some(start));
            }
        });
        btn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlinks_panel_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let panel = BacklinksPanel::new();
        let weak = Rc::downgrade(&panel.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(panel);
        assert_eq!(weak.strong_count(), 0);
    }
}
