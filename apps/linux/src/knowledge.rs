// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 知识导航面板：全文搜索、层级标签、反链、快速打开与 `[[` 双链补全。
//! 所有内核查询都在后台线程执行，主线程只渲染结果；新查询会取消旧查询并丢弃过期结果。

use crate::core::Utf16Mapper;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    BacklinkInfo, LinkCompletionInfo, QuickOpenHit, SearchCancellation, SearchResult,
    TagSummaryInfo, VaultHandle,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub type OpenNoteCallback = Rc<dyn Fn(String, Option<u64>)>;

fn clear_box(container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn escape_markup(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// 把 `snippet` 中的匹配区间（UTF-16，相对整篇笔记）转换成 Pango 加粗标记。
fn snippet_markup(
    snippet: &str,
    snippet_start_utf16: u32,
    match_start_utf16: u32,
    length: u32,
) -> String {
    let mapper = Utf16Mapper::new(snippet);
    let rel_start = match_start_utf16.saturating_sub(snippet_start_utf16) as usize;
    let cs = mapper.utf16_to_char(rel_start);
    let ce = mapper.utf16_to_char(rel_start + length as usize);
    let chars: Vec<char> = snippet.chars().collect();
    let before: String = chars[..cs.min(chars.len())].iter().collect();
    let hit: String = chars[cs.min(chars.len())..ce.min(chars.len())]
        .iter()
        .collect();
    let after: String = chars[ce.min(chars.len())..].iter().collect();
    format!(
        "{}<b>{}</b>{}",
        escape_markup(&before),
        escape_markup(&hit),
        escape_markup(&after)
    )
}

fn panel_scroll(child: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(child)
        .build()
}

fn dim_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_wrap(true);
    label.set_xalign(0.0);
    label
}

fn entry_row(title: &str, subtitle: Option<&str>, meta: Option<&str>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let title_label = gtk4::Label::new(Some(title));
    title_label.add_css_class("hortus-result-title");
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    row.append(&title_label);
    if let Some(subtitle) = subtitle {
        let sub = gtk4::Label::new(None);
        sub.set_markup(subtitle);
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        sub.add_css_class("hortus-result-snippet");
        row.append(&sub);
    }
    if let Some(meta) = meta {
        let meta_label = gtk4::Label::new(Some(meta));
        meta_label.set_xalign(0.0);
        meta_label.add_css_class("hortus-result-meta");
        row.append(&meta_label);
    }
    row
}

// ---------------------------------------------------------------------------
// 全文搜索
// ---------------------------------------------------------------------------

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
}

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

pub struct SearchPanel {
    pub root: gtk4::Box,
    entry: gtk4::SearchEntry,
    list: gtk4::Box,
    status: gtk4::Label,
    state: Rc<RefCell<SearchState>>,
    on_open: Rc<RefCell<Option<OpenNoteCallback>>>,
    /// 供自动化点击的真实结果按钮，确保验证走与鼠标一致的事件路径。
    group_buttons: Rc<RefCell<Vec<gtk4::Button>>>,
}

impl SearchPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header.add_css_class("hortus-panel-header");
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("全文搜索（Ctrl+Shift+F）")
            .hexpand(true)
            .build();
        header.append(&entry);

        let status = gtk4::Label::new(Some("输入关键字开始搜索"));
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

        let state = Rc::new(RefCell::new(SearchState::default()));
        let on_open: Rc<RefCell<Option<OpenNoteCallback>>> = Rc::new(RefCell::new(None));
        let panel = Self {
            root,
            entry,
            list,
            status,
            state,
            on_open,
            group_buttons: Rc::new(RefCell::new(Vec::new())),
        };
        panel.bind_entry();
        panel
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        *self.on_open.borrow_mut() = Some(callback);
    }

    pub fn focus_entry(&self) {
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    pub fn set_query(&self, query: &str) {
        self.entry.set_text(query);
    }

    /// 索引随磁盘变化后重跑当前查询（`set_text` 相同文本不会触发信号）。
    pub fn rerun(&self) {
        if self.entry.text().is_empty() {
            return;
        }
        self.entry.emit_by_name::<()>("search-changed", &[]);
    }

    /// 立即执行一次查询（跳过输入防抖），用于并发取消验证与索引刷新后的重跑。
    pub fn run_query_now(&self, query: &str) {
        let state = self.state.clone();
        let list = self.list.clone();
        let status = self.status.clone();
        let group_buttons = self.group_buttons.clone();
        let on_open = self.on_open.clone();
        let query = query.to_string();
        let (handle, cancellation, generation) = {
            let mut st = state.borrow_mut();
            let Some(handle) = st.handle.clone() else {
                return;
            };
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
            st.last_query = query.clone();
            let cancellation = SearchCancellation::new();
            st.cancellation = Some(cancellation.clone());
            (handle, cancellation, st.generation)
        };
        glib::MainContext::default().spawn_local(async move {
            let query_bg = query.clone();
            let result = gio::spawn_blocking(move || handle.search(query_bg, 6, cancellation))
                .await
                .unwrap_or_default();
            let mut st = state.borrow_mut();
            if st.generation != generation {
                st.stale_dropped += 1;
                return;
            }
            st.pending = false;
            st.cancellation = None;
            drop(st);
            render_search_results(&list, &status, &on_open, &group_buttons, &query, &result);
            let mut st = state.borrow_mut();
            st.groups = result.len();
            st.matches = result.iter().map(|r| r.matches.len()).sum();
        });
    }

    pub fn stats(&self) -> SearchPanelStats {
        let st = self.state.borrow();
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

    pub fn click_group(&self, index: usize) -> bool {
        let Some(button) = self.group_buttons.borrow().get(index).cloned() else {
            return false;
        };
        button.emit_clicked();
        true
    }

    fn bind_entry(&self) {
        let state = self.state.clone();
        let list = self.list.clone();
        let status = self.status.clone();
        let on_open = self.on_open.clone();
        let group_buttons_bg = self.group_buttons.clone();
        let entry = self.entry.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            let generation = {
                let mut st = state.borrow_mut();
                if st.handle.is_none() {
                    return;
                }
                if let Some(old) = st.cancellation.take() {
                    old.cancel();
                    st.cancellations += 1;
                }
                st.generation += 1;
                st.queries += 1;
                if st.pending {
                    // 上一次防抖期内的查询被新输入取代：同样计为一次取消。
                    st.cancellations += 1;
                }
                st.pending = true;
                st.last_query = query.clone();
                st.generation
            };
            if query.trim().is_empty() {
                clear_box(&list);
                status.set_label("输入关键字开始搜索");
                let mut st = state.borrow_mut();
                st.groups = 0;
                st.matches = 0;
                st.pending = false;
                return;
            }
            status.set_label("正在搜索…");

            // 180ms 防抖：连续输入只对最后一次查询生效，同时新查询立即取消旧查询。
            let state_debounce = state.clone();
            let list_debounce = list.clone();
            let status_debounce = status.clone();
            let on_open_debounce = on_open.clone();
            let group_buttons_bg = group_buttons_bg.clone();
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                let (handle, cancellation) = {
                    let mut st = state_debounce.borrow_mut();
                    if st.generation != generation {
                        return;
                    }
                    let Some(handle) = st.handle.clone() else {
                        return;
                    };
                    let cancellation = SearchCancellation::new();
                    st.cancellation = Some(cancellation.clone());
                    (handle, cancellation)
                };
                let state_bg = state_debounce.clone();
                glib::MainContext::default().spawn_local(async move {
                    let query_bg = query.clone();
                    let cancel_bg = cancellation.clone();
                    let result = gio::spawn_blocking(move || handle.search(query_bg, 6, cancel_bg))
                        .await
                        .unwrap_or_default();
                    let current = {
                        let mut st = state_bg.borrow_mut();
                        if st.generation != generation {
                            st.stale_dropped += 1;
                            return;
                        }
                        st.pending = false;
                        st.cancellation = None;
                        st.last_query = query.clone();
                        true
                    };
                    if !current {
                        return;
                    }
                    render_search_results(
                        &list_debounce,
                        &status_debounce,
                        &on_open_debounce,
                        &group_buttons_bg,
                        &query,
                        &result,
                    );
                    let mut st = state_bg.borrow_mut();
                    st.groups = result.len();
                    st.matches = result.iter().map(|r| r.matches.len()).sum();
                });
            });
        });
    }
}

fn render_search_results(
    list: &gtk4::Box,
    status: &gtk4::Label,
    on_open: &Rc<RefCell<Option<OpenNoteCallback>>>,
    group_buttons: &Rc<RefCell<Vec<gtk4::Button>>>,
    query: &str,
    results: &[SearchResult],
) {
    clear_box(list);
    group_buttons.borrow_mut().clear();
    let total: usize = results.iter().map(|r| r.matches.len()).sum();
    let total_count: u64 = results.iter().map(|r| u64::from(r.count)).sum();
    status.set_label(&format!(
        "「{query}」：{} 篇笔记，{total} 处匹配（共 {total_count} 处）",
        results.len()
    ));
    for result in results {
        let group = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        let header = entry_row(
            &result.title,
            None,
            Some(&format!("{} · {} 处匹配", result.path, result.count)),
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
        let open = on_open.clone();
        header_btn.connect_clicked(move |_| {
            let cb = open.borrow().clone();
            if let Some(cb) = cb {
                cb(path.clone(), Some(start));
            }
        });
        group_buttons.borrow_mut().push(header_btn.clone());
        group.append(&header_btn);

        for m in result.matches.iter().take(3) {
            let markup = snippet_markup(&m.snippet, m.snippet_start, m.start, m.length);
            let row = entry_row(&format!("L{}", m.start), Some(&markup), None);
            let btn = gtk4::Button::builder()
                .child(&row)
                .halign(gtk4::Align::Fill)
                .build();
            btn.add_css_class("flat");
            btn.add_css_class("hortus-result-row");
            let path = result.path.clone();
            let start = u64::from(m.start);
            let open = on_open.clone();
            btn.connect_clicked(move |_| {
                let cb = open.borrow().clone();
                if let Some(cb) = cb {
                    cb(path.clone(), Some(start));
                }
            });
            group.append(&btn);
        }
        list.append(&group);
    }
}

// ---------------------------------------------------------------------------
// 标签
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TagsState {
    handle: Option<Arc<VaultHandle>>,
    tags: Vec<TagSummaryInfo>,
    selected: Option<String>,
    filter: String,
    visible_tags: usize,
    visible_notes: usize,
    on_open: Option<OpenNoteCallback>,
    programmatic_filter: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TagsPanelStats {
    pub total_tags: usize,
    pub visible_tags: usize,
    pub selected: Option<String>,
    pub visible_notes: usize,
    pub filter: String,
}

pub struct TagsPanel {
    pub root: gtk4::Box,
    filter_entry: gtk4::SearchEntry,
    list: gtk4::Box,
    status: gtk4::Label,
    state: Rc<RefCell<TagsState>>,
    tag_buttons: Rc<RefCell<Vec<(String, gtk4::Button)>>>,
    note_buttons: Rc<RefCell<Vec<(String, gtk4::Button)>>>,
}

impl TagsPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header.add_css_class("hortus-panel-header");
        let filter_entry = gtk4::SearchEntry::builder()
            .placeholder_text("过滤标签")
            .hexpand(true)
            .build();
        header.append(&filter_entry);

        let status = gtk4::Label::new(Some("尚未打开知识库"));
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

        let state = Rc::new(RefCell::new(TagsState::default()));
        let panel = Self {
            root,
            filter_entry,
            list,
            status,
            state,
            tag_buttons: Rc::new(RefCell::new(Vec::new())),
            note_buttons: Rc::new(RefCell::new(Vec::new())),
        };
        panel.bind_filter();
        panel
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.state.borrow_mut().handle = Some(handle);
        self.refresh();
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.state.borrow_mut().on_open = Some(callback);
    }

    pub fn stats(&self) -> TagsPanelStats {
        let st = self.state.borrow();
        TagsPanelStats {
            total_tags: st.tags.len(),
            visible_tags: st.visible_tags,
            selected: st.selected.clone(),
            visible_notes: st.visible_notes,
            filter: st.filter.clone(),
        }
    }

    pub fn filter_text(&self) -> String {
        self.filter_entry.text().to_string()
    }

    /// 选中标签并展开其笔记；阅读视图中的 `#标签` 点击也走这里。
    pub fn select_tag(&self, tag: &str) {
        {
            let mut st = self.state.borrow_mut();
            st.selected = Some(tag.trim_start_matches('#').to_string());
            st.filter = tag.to_string();
            // 由程序写入过滤框时保留选中标签，避免 search-changed 回调把选择清空。
            st.programmatic_filter = true;
        }
        self.filter_entry.set_text(tag);
        self.render();
    }

    pub fn refresh(&self) {
        let tags = {
            let st = self.state.borrow();
            let Some(handle) = st.handle.clone() else {
                return;
            };
            drop(st);
            handle.tags()
        };
        self.state.borrow_mut().tags = tags;
        self.render();
    }

    pub fn click_tag(&self, index: usize) -> bool {
        let Some((_, button)) = self.tag_buttons.borrow().get(index).cloned() else {
            return false;
        };
        button.emit_clicked();
        true
    }

    pub fn click_note(&self, index: usize) -> bool {
        let Some((_, button)) = self.note_buttons.borrow().get(index).cloned() else {
            return false;
        };
        button.emit_clicked();
        true
    }

    pub fn tag_labels(&self) -> Vec<String> {
        self.tag_buttons
            .borrow()
            .iter()
            .map(|(tag, _)| tag.clone())
            .collect()
    }

    fn bind_filter(&self) {
        let state = self.state.clone();
        let this = self.clone_handle();
        self.filter_entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            {
                let mut st = state.borrow_mut();
                st.filter = text;
                if st.programmatic_filter {
                    st.programmatic_filter = false;
                } else {
                    st.selected = None;
                }
            }
            this.render();
        });
    }

    fn clone_handle(&self) -> TagsPanel {
        Self {
            root: self.root.clone(),
            filter_entry: self.filter_entry.clone(),
            list: self.list.clone(),
            status: self.status.clone(),
            state: self.state.clone(),
            tag_buttons: self.tag_buttons.clone(),
            note_buttons: self.note_buttons.clone(),
        }
    }

    fn render(&self) {
        let (tags, filter, selected, handle, on_open) = {
            let st = self.state.borrow();
            (
                st.tags
                    .iter()
                    .map(|tag| (tag.tag.clone(), tag.raw.clone(), tag.count))
                    .collect::<Vec<_>>(),
                st.filter.clone(),
                st.selected.clone(),
                st.handle.clone(),
                st.on_open.clone(),
            )
        };
        clear_box(&self.list);
        self.tag_buttons.borrow_mut().clear();
        self.note_buttons.borrow_mut().clear();
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
                "该知识库尚无标签"
            } else {
                "没有匹配的标签"
            };
            self.list.append(&dim_label(text));
            let mut st = self.state.borrow_mut();
            st.visible_tags = 0;
            st.visible_notes = 0;
            self.status.set_label(text);
            return;
        }
        self.status.set_label(&format!(
            "{} 个标签{}",
            tags.len(),
            selected
                .as_deref()
                .map(|s| format!(" · 已选 #{s}"))
                .unwrap_or_default()
        ));

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
                // 选中态只用强调色胶囊；叠加 .flat 会把背景变透明，浅色主题下白字不可读。
                btn.add_css_class("suggested-action");
                btn.add_css_class("hortus-tag-chip-selected");
            } else {
                btn.add_css_class("flat");
            }
            let selected_tag = tag.0.clone();
            let selected_tag_cb = tag.0.clone();
            let panel = self.clone_handle();
            btn.connect_clicked(move |_| {
                let current = panel.state.borrow().selected.clone();
                if current.as_deref() == Some(selected_tag_cb.as_str()) {
                    panel.state.borrow_mut().selected = None;
                    panel.render();
                } else {
                    panel.state.borrow_mut().selected = Some(selected_tag_cb.clone());
                    panel.render();
                }
            });
            self.tag_buttons
                .borrow_mut()
                .push((selected_tag.clone(), btn.clone()));
            self.list.append(&btn);

            if is_selected && let Some(handle) = &handle {
                let notes = handle.notes_with_tag(tag.0.clone());
                visible_notes = notes.len();
                for note in notes {
                    let row = entry_row(&note, None, Some("打开并定位到标签"));
                    let open_btn = gtk4::Button::builder()
                        .child(&row)
                        .halign(gtk4::Align::Fill)
                        .build();
                    open_btn.add_css_class("flat");
                    open_btn.add_css_class("hortus-result-row");
                    open_btn.set_margin_start(14);
                    let path = note.clone();
                    let cb = on_open.clone();
                    open_btn.connect_clicked(move |_| {
                        if let Some(cb) = &cb {
                            cb(path.clone(), None);
                        }
                    });
                    self.note_buttons
                        .borrow_mut()
                        .push((note.clone(), open_btn.clone()));
                    self.list.append(&open_btn);
                }
                if visible_notes == 0 {
                    self.list.append(&dim_label("没有笔记使用该标签"));
                }
            }
        }
        let mut st = self.state.borrow_mut();
        st.visible_tags = rows;
        st.visible_notes = visible_notes;
    }
}

// ---------------------------------------------------------------------------
// 反链
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BacklinksState {
    handle: Option<Arc<VaultHandle>>,
    path: Option<String>,
    rows: usize,
    updates: u32,
    on_open: Option<OpenNoteCallback>,
}

#[derive(Clone, Debug, Default)]
pub struct BacklinksPanelStats {
    pub path: Option<String>,
    pub rows: usize,
    pub updates: u32,
}

pub struct BacklinksPanel {
    pub root: gtk4::Box,
    title: gtk4::Label,
    list: gtk4::Box,
    state: Rc<RefCell<BacklinksState>>,
    row_buttons: Rc<RefCell<Vec<(String, u64, gtk4::Button)>>>,
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
            root,
            title,
            list,
            state: Rc::new(RefCell::new(BacklinksState::default())),
            row_buttons: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// 触发第 `index` 行反链的真实点击回调；返回打开的源笔记与偏移。
    pub fn click_row(&self, index: usize) -> Option<(String, u64)> {
        let (source, start, button) = self.row_buttons.borrow().get(index).cloned()?;
        button.emit_clicked();
        Some((source, start))
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.state.borrow_mut().on_open = Some(callback);
    }

    pub fn stats(&self) -> BacklinksPanelStats {
        let st = self.state.borrow();
        BacklinksPanelStats {
            path: st.path.clone(),
            rows: st.rows,
            updates: st.updates,
        }
    }

    pub fn current_path(&self) -> Option<String> {
        self.state.borrow().path.clone()
    }

    /// 切换激活笔记或磁盘变化后刷新；相同路径且 `force` 为假时跳过重复查询。
    pub fn update(&self, path: Option<String>, force: bool) {
        let (handle, previous) = {
            let st = self.state.borrow();
            (st.handle.clone(), st.path.clone())
        };
        if !force && previous == path {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.path = path.clone();
            st.updates += 1;
        }
        clear_box(&self.list);
        self.row_buttons.borrow_mut().clear();
        let Some(path) = path else {
            self.title.set_label("未打开笔记");
            self.state.borrow_mut().rows = 0;
            return;
        };
        let Some(handle) = handle else {
            self.state.borrow_mut().rows = 0;
            return;
        };
        let links = handle.backlinks(path.clone());
        self.title
            .set_label(&format!("引用「{path}」的笔记（{} 条）", links.len()));
        if links.is_empty() {
            self.list.append(&dim_label("还没有笔记引用这篇笔记"));
        }
        let on_open = self.state.borrow().on_open.clone();
        for link in &links {
            let button = self.render_row(link, on_open.clone());
            self.row_buttons.borrow_mut().push((
                link.source.clone(),
                link.start_utf16,
                button.clone(),
            ));
            self.list.append(&button);
        }
        self.state.borrow_mut().rows = links.len();
    }

    fn render_row(&self, link: &BacklinkInfo, on_open: Option<OpenNoteCallback>) -> gtk4::Button {
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
        btn.connect_clicked(move |_| {
            if let Some(cb) = &on_open {
                cb(path.clone(), Some(start));
            }
        });
        btn
    }
}

// ---------------------------------------------------------------------------
// 快速打开
// ---------------------------------------------------------------------------

#[derive(Default)]
struct QuickState {
    handle: Option<Arc<VaultHandle>>,
    generation: u64,
    queries: u32,
    last_query: String,
    hits: Vec<QuickOpenHit>,
    on_open: Option<OpenNoteCallback>,
    selected: usize,
    open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct QuickOpenStats {
    pub queries: u32,
    pub last_query: String,
    pub hits: usize,
    pub first: Option<String>,
    pub open: bool,
}

pub struct QuickOpen {
    dialog: adw::Dialog,
    entry: gtk4::SearchEntry,
    list: gtk4::ListBox,
    state: Rc<RefCell<QuickState>>,
    window: RefCell<Option<gtk4::Window>>,
}

impl QuickOpen {
    pub fn new() -> Self {
        let dialog = adw::Dialog::builder()
            .title("快速打开")
            .content_width(500)
            .content_height(460)
            .build();
        dialog.add_css_class("hortus-quick-open");

        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("输入标题或路径，Enter 打开")
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

        let quick = Self {
            dialog: dialog.clone(),
            entry: entry.clone(),
            list: list.clone(),
            state: Rc::new(RefCell::new(QuickState::default())),
            window: RefCell::new(None),
        };
        quick.bind();
        quick
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.state.borrow_mut().handle = Some(handle);
    }

    pub fn set_on_open(&self, callback: OpenNoteCallback) {
        self.state.borrow_mut().on_open = Some(callback);
    }

    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }

    pub fn hits(&self) -> Vec<String> {
        self.state
            .borrow()
            .hits
            .iter()
            .map(|hit| hit.path.clone())
            .collect()
    }

    pub fn stats(&self) -> QuickOpenStats {
        let st = self.state.borrow();
        QuickOpenStats {
            queries: st.queries,
            last_query: st.last_query.clone(),
            hits: st.hits.len(),
            first: st.hits.first().map(|h| h.path.clone()),
            open: st.open,
        }
    }

    pub fn open(&self, parent: &gtk4::Window) {
        self.state.borrow_mut().open = true;
        *self.window.borrow_mut() = Some(parent.clone());
        self.entry.set_text("");
        {
            let mut st = self.state.borrow_mut();
            st.selected = 0;
        }
        self.dialog.present(Some(parent));
        self.entry.grab_focus();
        self.query_now("");
    }

    pub fn set_query(&self, query: &str) {
        self.entry.set_text(query);
    }

    pub fn activate_selected(&self) {
        self.open_selected();
    }

    fn bind(&self) {
        let state = self.state.clone();
        let list = self.list.clone();
        let entry = self.entry.clone();

        let state_changed = state.clone();
        let list_changed = list.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            let generation = {
                let mut st = state_changed.borrow_mut();
                if st.handle.is_none() {
                    return;
                }
                st.generation += 1;
                st.queries += 1;
                st.last_query = query.clone();
                st.generation
            };
            let state_debounce = state_changed.clone();
            let list_debounce = list_changed.clone();
            glib::timeout_add_local_once(Duration::from_millis(90), move || {
                let handle = {
                    let st = state_debounce.borrow();
                    if st.generation != generation {
                        return;
                    }
                    st.handle.clone()
                };
                let Some(handle) = handle else { return };
                let state_bg = state_debounce.clone();
                glib::MainContext::default().spawn_local(async move {
                    let result = gio::spawn_blocking(move || handle.quick_open(query, 30))
                        .await
                        .unwrap_or_default();
                    let mut st = state_bg.borrow_mut();
                    if st.generation != generation {
                        return;
                    }
                    st.hits = result;
                    st.selected = 0;
                    render_quick_hits(&list_debounce, &st);
                });
            });
        });

        // 键盘：Up/Down 选择，Enter 打开，Esc 关闭。
        let key = gtk4::EventControllerKey::new();
        let state_key = state.clone();
        let list_key = list.clone();
        let dialog_key = self.dialog.clone();
        key.connect_key_pressed(move |_, keyval, _, _| {
            use gtk4::gdk::Key;
            match keyval {
                Key::Down | Key::Up => {
                    let mut st = state_key.borrow_mut();
                    if st.hits.is_empty() {
                        return glib::Propagation::Proceed;
                    }
                    if keyval == Key::Down {
                        st.selected = (st.selected + 1).min(st.hits.len() - 1);
                    } else {
                        st.selected = st.selected.saturating_sub(1);
                    }
                    render_quick_hits(&list_key, &st);
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    open_selected_impl(&state_key);
                    state_key.borrow_mut().open = false;
                    dialog_key.close();
                    glib::Propagation::Stop
                }
                Key::Escape => glib::Propagation::Proceed,
                _ => glib::Propagation::Proceed,
            }
        });
        self.entry.add_controller(key);

        let state_activate = state.clone();
        let dialog_activate = self.dialog.clone();
        self.list.connect_row_activated(move |_, row| {
            let index = row.index().max(0) as usize;
            state_activate.borrow_mut().selected = index;
            open_selected_impl(&state_activate);
            state_activate.borrow_mut().open = false;
            dialog_activate.close();
        });
    }

    fn open_selected(&self) {
        open_selected_impl(&self.state);
        self.state.borrow_mut().open = false;
        self.dialog.close();
    }

    fn query_now(&self, query: &str) {
        let state = self.state.clone();
        let list = self.list.clone();
        let (handle, generation) = {
            let mut st = state.borrow_mut();
            let Some(handle) = st.handle.clone() else {
                return;
            };
            st.generation += 1;
            st.queries += 1;
            st.last_query = query.to_string();
            (handle, st.generation)
        };
        let query = query.to_string();
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || handle.quick_open(query, 30))
                .await
                .unwrap_or_default();
            let mut st = state.borrow_mut();
            if st.generation != generation {
                return;
            }
            st.hits = result;
            st.selected = 0;
            render_quick_hits(&list, &st);
        });
    }
}

fn render_quick_hits(list: &gtk4::ListBox, state: &QuickState) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    for (index, hit) in state.hits.iter().enumerate() {
        let row = gtk4::ListBoxRow::new();
        let label = entry_row(&hit.title, None, Some(&hit.path));
        label.set_margin_start(6);
        label.set_margin_end(6);
        row.set_child(Some(&label));
        if index == state.selected {
            row.add_css_class("suggested-action");
        }
        list.append(&row);
    }
}

fn open_selected_impl(state: &Rc<RefCell<QuickState>>) {
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

// ---------------------------------------------------------------------------
// `[[` 双链补全
// ---------------------------------------------------------------------------

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
}

#[derive(Clone, Debug, Default)]
pub struct CompletionStats {
    pub queries: u32,
    pub accepted: u32,
}

pub struct LinkCompletion {
    panel: gtk4::Box,
    list: gtk4::ListBox,
    state: Rc<RefCell<CompletionState>>,
    view: RefCell<Option<gtk4::TextView>>,
    overlay: RefCell<Option<gtk4::Overlay>>,
    queries: Rc<Cell<u32>>,
}

impl LinkCompletion {
    pub fn new() -> Self {
        // 用窗口内 Overlay 面板而不是 GtkPopover：不夺取键盘 grab，
        // 输入法与组合态始终由原生 GtkTextView 拥有，同时能被窗口截图捕获。
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
            panel,
            list,
            state: Rc::new(RefCell::new(CompletionState::default())),
            view: RefCell::new(None),
            overlay: RefCell::new(None),
            queries: Rc::new(Cell::new(0)),
        }
    }

    pub fn set_handle(&self, handle: Arc<VaultHandle>) {
        self.state.borrow_mut().handle = Some(handle);
    }

    pub fn stats(&self) -> CompletionStats {
        let st = self.state.borrow();
        CompletionStats {
            queries: self.queries.get(),
            accepted: st.accepted,
        }
    }

    pub fn hide(&self) {
        self.panel.set_visible(false);
        self.state.borrow_mut().items.clear();
        self.state.borrow_mut().link_start = None;
    }

    pub fn accept_selected(&self) {
        self.accept_impl();
    }

    pub fn is_visible(&self) -> bool {
        self.panel.is_visible()
    }

    /// 供自动化记录：补全弹层不获取键盘焦点。
    pub fn popover_focusable(&self) -> bool {
        self.panel.can_focus()
    }

    pub fn item_count(&self) -> usize {
        self.state.borrow().items.len()
    }

    pub fn first_insert_target(&self) -> Option<String> {
        self.state
            .borrow()
            .items
            .first()
            .map(|item| item.insert_target.clone())
    }

    pub fn move_selection(&self, down: bool) {
        let mut st = self.state.borrow_mut();
        if st.items.is_empty() {
            return;
        }
        if down {
            st.selected = (st.selected + 1).min(st.items.len() - 1);
        } else {
            st.selected = st.selected.saturating_sub(1);
        }
        render_completion_items(&self.list, &st);
    }

    /// 绑定编辑视图：随后每次文本/光标变化都会重新评估 `[[` 上下文。
    pub fn bind(&self, view: &gtk4::TextView, buffer: &gtk4::TextBuffer, overlay: &gtk4::Overlay) {
        *self.view.borrow_mut() = Some(view.clone());
        *self.overlay.borrow_mut() = Some(overlay.clone());
        overlay.add_overlay(&self.panel);

        let this = self.clone_handle();
        let buffer_changed = buffer.clone();
        buffer.connect_changed(move |_| {
            this.refresh_for_cursor(&buffer_changed);
        });

        let this = self.clone_handle();
        let view_key = view.clone();
        let key = gtk4::EventControllerKey::new();
        key.set_propagation_phase(gtk4::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _, _| {
            use gtk4::gdk::Key;
            if !this.panel.is_visible() {
                return glib::Propagation::Proceed;
            }
            match keyval {
                Key::Down => {
                    this.move_selection(true);
                    glib::Propagation::Stop
                }
                Key::Up => {
                    this.move_selection(false);
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter | Key::Tab => {
                    this.accept_selected();
                    glib::Propagation::Stop
                }
                Key::Escape => {
                    this.hide();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        view_key.add_controller(key);
    }

    fn clone_handle(&self) -> LinkCompletion {
        Self {
            panel: self.panel.clone(),
            list: self.list.clone(),
            state: self.state.clone(),
            view: RefCell::new(self.view.borrow().clone()),
            overlay: RefCell::new(self.overlay.borrow().clone()),
            queries: self.queries.clone(),
        }
    }

    fn refresh_for_cursor(&self, buffer: &gtk4::TextBuffer) {
        let Some(view) = self.view.borrow().clone() else {
            return;
        };
        let insert = buffer.iter_at_mark(&buffer.get_insert());
        let cursor_char = insert.offset().max(0) as usize;
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let chars: Vec<char> = text.chars().collect();
        // 仅在当前行内查找未闭合的 `[[`，避免跨行误触发。
        let mut line_start = 0usize;
        for idx in (0..cursor_char.min(chars.len())).rev() {
            if chars[idx] == '\n' {
                line_start = idx + 1;
                break;
            }
        }
        let prefix: String = chars[line_start..cursor_char.min(chars.len())]
            .iter()
            .collect();
        let Some(open) = prefix.rfind("[[") else {
            self.hide();
            return;
        };
        if prefix[open..].contains("]]") {
            self.hide();
            return;
        }
        let query = prefix[open + 2..].to_string();
        // 与 macOS 一致：查询词不允许出现结构字符；`![[` 的链接起点在 `!` 上。
        if query.chars().any(|c| "[]#|".contains(c)) {
            self.hide();
            return;
        }
        let embed = open > 0 && prefix.as_bytes().get(open - 1) == Some(&b'!');
        let link_start_char = line_start + open - usize::from(embed);
        let mapper = Utf16Mapper::new(&text);
        let link_start_u16 = mapper.char_to_utf16(link_start_char) as u64;
        // 未闭合的 `[[` 不会被解析成链接：在候选副本中补上 `]]` 再做内核校验。
        let has_closer = chars.get(cursor_char).is_some_and(|c| *c == ']')
            && chars.get(cursor_char + 1).is_some_and(|c| *c == ']');
        let mut candidate = text.clone();
        if !has_closer {
            let insert = if query.is_empty() { "x]]" } else { "]]" };
            let byte = mapper.char_to_byte(cursor_char);
            candidate.insert_str(byte, insert);
        } else if query.is_empty() {
            let byte = mapper.char_to_byte(cursor_char);
            candidate.insert(byte, 'x');
        }
        let generation = {
            let mut st = self.state.borrow_mut();
            if st.handle.is_none() {
                return;
            }
            st.generation += 1;
            st.link_start = Some(link_start_u16);
            st.last_query = query.clone();
            st.generation
        };
        self.queries.set(self.queries.get() + 1);
        let state = self.state.clone();
        let list = self.list.clone();
        let panel = self.panel.clone();
        let this = self.clone_handle();
        glib::timeout_add_local_once(Duration::from_millis(90), move || {
            let handle = {
                let st = state.borrow();
                if st.generation != generation {
                    return;
                }
                st.handle.clone()
            };
            let Some(handle) = handle else { return };
            glib::MainContext::default().spawn_local(async move {
                let text_bg = candidate.clone();
                let query_bg = query.clone();
                let items = gio::spawn_blocking(move || {
                    handle.link_completions_at(text_bg, link_start_u16, query_bg, 8)
                })
                .await
                .unwrap_or_default();
                let items: Vec<_> = items
                    .into_iter()
                    .filter(|item| !item.insert_target.contains(['[', ']', '#', '|']))
                    .collect();
                let mut st = state.borrow_mut();
                if st.generation != generation {
                    return;
                }
                st.items = items;
                st.selected = 0;
                if st.items.is_empty() {
                    drop(st);
                    panel.set_visible(false);
                    return;
                }
                render_completion_items(&list, &st);
                drop(st);
                this.show_at_cursor(&view);
            });
        });
    }

    fn show_at_cursor(&self, view: &gtk4::TextView) {
        let Some(overlay) = self.overlay.borrow().clone() else {
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
        self.panel.set_margin_start(margin_start);
        self.panel
            .set_margin_top((point.y().max(0.0) as i32).min(overlay.height()));
        self.panel.set_visible(true);
        self.panel.queue_resize();
    }

    fn accept_impl(&self) {
        let (target, link_start) = {
            let st = self.state.borrow();
            let Some(item) = st.items.get(st.selected) else {
                return;
            };
            let Some(link_start) = st.link_start else {
                return;
            };
            (item.insert_target.clone(), link_start)
        };
        let Some(view) = self.view.borrow().clone() else {
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
        self.hide();
        {
            let mut st = self.state.borrow_mut();
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

// ---------------------------------------------------------------------------
// 面板句柄复用：窗口与信号闭包共享同一组控件与状态。
// ---------------------------------------------------------------------------

impl Clone for SearchPanel {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            entry: self.entry.clone(),
            list: self.list.clone(),
            status: self.status.clone(),
            state: self.state.clone(),
            on_open: self.on_open.clone(),
            group_buttons: self.group_buttons.clone(),
        }
    }
}

impl Clone for TagsPanel {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            filter_entry: self.filter_entry.clone(),
            list: self.list.clone(),
            status: self.status.clone(),
            state: self.state.clone(),
            tag_buttons: self.tag_buttons.clone(),
            note_buttons: self.note_buttons.clone(),
        }
    }
}

impl Clone for BacklinksPanel {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            title: self.title.clone(),
            list: self.list.clone(),
            state: self.state.clone(),
            row_buttons: self.row_buttons.clone(),
        }
    }
}

impl Clone for QuickOpen {
    fn clone(&self) -> Self {
        Self {
            dialog: self.dialog.clone(),
            entry: self.entry.clone(),
            list: self.list.clone(),
            state: self.state.clone(),
            window: RefCell::new(self.window.borrow().clone()),
        }
    }
}

impl Clone for LinkCompletion {
    fn clone(&self) -> Self {
        self.clone_handle()
    }
}
