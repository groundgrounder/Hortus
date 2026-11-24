// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 标签页创建、后台首屏加载、页面关闭销毁与活跃标签界面同步。

use crate::core::model::HeadingItem;
use crate::editor::{EditState, FindBar, LinkCompletion};
use crate::knowledge::clear_box;
use crate::platform::HortusReadingViewport;
use crate::platform::style::{apply_source_styles, create_tag_table};
use crate::reading::{PrepareReadingRequest, build_reading_surface, prepare_reading_note_bg};
use crate::window::commands::WindowCommand;
use crate::window::shell::WindowInner;
use crate::window::state::OpenNoteTab;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::DocumentSession;
use libadwaita as adw;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

impl WindowInner {
    pub fn open_note_at(self: &Rc<Self>, relpath: &str, heading: Option<String>) {
        let already_open = {
            let mut st = self.state.borrow_mut();
            if let Some(tab) = st.tabs.get_mut(relpath) {
                let page = tab.page.clone();
                let loaded = tab.reading_loaded;
                if let Some(ref h) = heading {
                    tab.pending_heading = Some(h.clone());
                }
                drop(st);
                self.tab_view.set_selected_page(&page);
                self.center_stack.set_visible_child_name("tabs");
                if loaded && let Some(ref h) = heading {
                    self.scroll_to_heading_title(relpath, h);
                }
                true
            } else {
                false
            }
        };
        if already_open {
            return;
        }

        let (services, vault_root, vault_handle) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_root(), st.vault_handle())
        };
        let (Some(services), Some(vault_root)) = (services, vault_root) else {
            return;
        };

        let content_stack = gtk4::Stack::new();
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);

        let reading_slot = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        reading_slot.set_vexpand(true);
        reading_slot.set_hexpand(true);
        let loading_lbl = gtk4::Label::new(Some("正在渲染阅读视图…"));
        loading_lbl.add_css_class("dim-label");
        loading_lbl.set_margin_top(36);
        reading_slot.append(&loading_lbl);

        let reading_viewport = HortusReadingViewport::new();
        reading_viewport.set_child(Some(&reading_slot));
        {
            let weak = Rc::downgrade(self);
            let rel_vp = relpath.to_string();
            reading_viewport.connect_relayout_width(move |avail_w| {
                if let Some(win) = weak.upgrade()
                    && win.active_tab_path().as_deref() == Some(rel_vp.as_str())
                {
                    win.send_command(WindowCommand::RelayoutActiveTab(avail_w));
                }
            });
        }
        let reading_scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&reading_viewport)
            .build();
        content_stack.add_named(&reading_scrolled, Some("reading"));

        let tag_table = create_tag_table();
        let edit_buffer = gtk4::TextBuffer::new(Some(&tag_table));
        edit_buffer.set_enable_undo(true);
        edit_buffer.set_text("正在加载…");
        let edit_view = gtk4::TextView::builder()
            .buffer(&edit_buffer)
            .editable(true)
            .cursor_visible(true)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .left_margin(44)
            .right_margin(44)
            .top_margin(28)
            .bottom_margin(48)
            .pixels_above_lines(2)
            .pixels_below_lines(4)
            .vexpand(true)
            .hexpand(true)
            .build();
        edit_view.add_css_class("hortus-note-view");
        let edit_scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&edit_view)
            .build();
        content_stack.add_named(&edit_scrolled, Some("edit"));
        content_stack.set_visible_child_name("reading");

        let notice_label = gtk4::Label::new(None);
        notice_label.set_wrap(true);
        notice_label.set_xalign(0.0);
        notice_label.set_hexpand(true);
        let notice_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        notice_actions.set_halign(gtk4::Align::Start);
        let notice_row = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        notice_row.add_css_class("hortus-notice");
        notice_row.append(&notice_label);
        notice_row.append(&notice_actions);
        let notice = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .visible(false)
            .child(&notice_row)
            .build();

        let find = FindBar::new();
        let page_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        page_wrap.append(&notice);
        page_wrap.append(find.root());
        page_wrap.append(&content_stack);

        let tab_overlay = gtk4::Overlay::new();
        tab_overlay.set_child(Some(&page_wrap));
        let completion = LinkCompletion::new();
        completion.bind(&edit_view, &edit_buffer, &tab_overlay);
        if let Some(ref handle) = vault_handle {
            completion.set_handle(Arc::clone(handle));
        }

        let page = self.tab_view.append(&tab_overlay);
        let file_title = Path::new(relpath)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(relpath);
        page.set_title(file_title);
        page.set_tooltip(relpath);

        self.state.borrow_mut().tabs.insert(
            relpath.to_string(),
            OpenNoteTab {
                path: relpath.to_string(),
                page: page.clone(),
                content_stack,
                reading_scrolled,
                reading_viewport,
                reading_slot,
                reading: None,
                prepared_diagrams: Vec::new(),
                layout_width: 0.0,
                reading_generation: 0,
                reading_revision: 0,
                surface_rebuilds: 0,
                in_place_relayouts: 0,
                edit_scrolled,
                edit_view,
                edit_buffer,
                text: String::new(),
                headings: Vec::new(),
                session: None,
                editing: false,
                source_loaded: false,
                reading_loaded: false,
                pending_heading: heading,
                pending_scroll: None,
                notice,
                notice_label,
                notice_actions,
                find,
                completion,
                edit_state: Rc::new(RefCell::new(EditState::new(relpath))),
            },
        );
        self.tab_view.set_selected_page(&page);
        self.center_stack.set_visible_child_name("tabs");
        self.sync_active_tab_ui();

        let content_width = self.reading_content_width();
        let dark = adw::StyleManager::default().is_dark();
        let main_thread = std::thread::current().id();
        let weak = Rc::downgrade(self);
        let rel = relpath.to_string();
        let core_bg = Arc::clone(&services.core);
        let diag_bg = Arc::clone(&services.diagram_engine);
        let math_bg = Arc::clone(&services.math_service);

        glib::MainContext::default().spawn_local(async move {
            let rel_bg = rel.clone();
            let vault_bg = vault_root.clone();
            let load_result = gio::spawn_blocking(move || {
                let session = core_bg
                    .open_document(vault_bg.clone(), rel_bg.clone())
                    .map_err(|e| e.to_string())?;
                let initial = session.initial().map_err(|e| e.to_string())?;
                let text = initial.text.unwrap_or_default();
                let outline = session.outline().map_err(|e| e.to_string())?;
                let headings: Vec<HeadingItem> = outline
                    .headings
                    .into_iter()
                    .map(HeadingItem::from)
                    .collect();
                let styles = session
                    .source_styles(u64::MAX, true)
                    .map_err(|e| e.to_string())?;

                let prepared = prepare_reading_note_bg(&PrepareReadingRequest {
                    note_path: rel_bg,
                    vault_root: vault_bg,
                    session: Arc::clone(&session),
                    core: Arc::clone(&core_bg),
                    handle: vault_handle,
                    diagram_engine: diag_bg,
                    math_service: math_bg,
                    content_width,
                    font_size: 15.0,
                    dark,
                    main_thread,
                })?;
                Ok::<_, String>((session, text, headings, styles.packed_runs, prepared))
            })
            .await
            .unwrap_or_else(|_| Err("读取笔记线程异常".into()));

            let Some(win) = weak.upgrade() else { return };
            if win.state.borrow().disposing {
                return;
            }
            match load_result {
                Ok((session, text, headings, packed_runs, prepared)) => {
                    win.install_loaded_tab(
                        &rel,
                        session,
                        text,
                        headings,
                        &packed_runs,
                        prepared,
                        content_width,
                    );
                    win.sync_active_tab_ui();
                    #[cfg(feature = "automation")]
                    crate::automation::maybe_schedule_automation(&win);
                }
                Err(err) => {
                    let widgets = {
                        let mut st = win.state.borrow_mut();
                        st.tabs.get_mut(&rel).map(|tab| {
                            tab.source_loaded = true;
                            tab.reading_loaded = true;
                            (tab.edit_buffer.clone(), tab.reading_slot.clone())
                        })
                    };
                    if let Some((edit_buffer, reading_slot)) = widgets {
                        edit_buffer.set_text(&format!("读取笔记失败：{err}"));
                        clear_box(&reading_slot);
                        let err_lbl = gtk4::Label::new(Some(&format!("读取笔记失败：{err}")));
                        err_lbl.set_margin_top(24);
                        reading_slot.append(&err_lbl);
                    }
                    win.show_toast(&format!("打开笔记失败：{err}"));
                    #[cfg(feature = "automation")]
                    crate::automation::maybe_schedule_automation(&win);
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn install_loaded_tab(
        self: &Rc<Self>,
        rel: &str,
        session: Arc<DocumentSession>,
        text: String,
        headings: Vec<HeadingItem>,
        packed_runs: &[u8],
        prepared: crate::reading::PreparedReadingNote,
        content_width: f64,
    ) {
        let (services, vault_root, vault_handle) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_root(), st.vault_handle())
        };
        let (Some(services), Some(vault_root), Some(handle)) = (services, vault_root, vault_handle)
        else {
            return;
        };

        let env = self.make_reading_env(
            rel,
            vault_root.clone(),
            Arc::clone(&services.core),
            handle,
            Arc::clone(&services.diagram_engine),
            Arc::clone(&services.math_service),
        );
        let weak_collapse = Rc::downgrade(self);
        let rel_collapse = rel.to_string();
        let vault_collapse = vault_root;
        let core_collapse = Arc::clone(&services.core);
        let weak_scene = Rc::downgrade(self);
        let rel_scene = rel.to_string();

        let surface = build_reading_surface(
            &prepared,
            &env,
            Rc::new(move |ordinal, collapsed, new_scene| {
                let _ = core_collapse.remember_diagram(
                    vault_collapse.clone(),
                    rel_collapse.clone(),
                    ordinal,
                    collapsed.clone(),
                );
                if let Some(win) = weak_collapse.upgrade()
                    && let Some(tab) = win.state.borrow_mut().tabs.get_mut(&rel_collapse)
                    && let Some(item) = tab
                        .prepared_diagrams
                        .iter_mut()
                        .find(|d| d.ordinal == ordinal)
                {
                    item.collapsed = collapsed;
                    item.previous = new_scene.previous.clone();
                    item.scene = Some(new_scene);
                }
            }),
            Rc::new(move |ordinal, new_scene| {
                if let Some(win) = weak_scene.upgrade()
                    && let Some(tab) = win.state.borrow_mut().tabs.get_mut(&rel_scene)
                    && let Some(item) = tab
                        .prepared_diagrams
                        .iter_mut()
                        .find(|d| d.ordinal == ordinal)
                {
                    item.previous = new_scene.previous.clone();
                    item.scene = Some(new_scene);
                }
            }),
        );

        let revision = prepared.revision;
        let (edit_view, edit_buffer, edit_state, reading_slot) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(rel) else {
                return;
            };
            (
                tab.edit_view.clone(),
                tab.edit_buffer.clone(),
                tab.edit_state.clone(),
                tab.reading_slot.clone(),
            )
        };
        {
            let mut edit = edit_state.borrow_mut();
            edit.suppress = true;
            edit.text = text.clone();
            edit.revision = revision;
            edit.dirty = false;
            edit.issue = None;
        }
        let detach_for_load = text.chars().count() > 262_144;
        if detach_for_load {
            edit_view.set_buffer(None::<&gtk4::TextBuffer>);
        }
        edit_buffer.set_text(&text);
        let style_started = Instant::now();
        apply_source_styles(&edit_buffer, &text, packed_runs);
        let load_style_micros = style_started.elapsed().as_micros() as u64;
        if detach_for_load {
            edit_view.set_buffer(Some(&edit_buffer));
        }
        {
            let mut edit = edit_state.borrow_mut();
            edit.suppress = false;
            edit.load_style_micros = load_style_micros;
            edit.last_style_micros = load_style_micros;
        }
        clear_box(&reading_slot);
        reading_slot.append(&surface.root);
        let (pending_heading, pending_scroll, viewport, old) = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(rel) else {
                return;
            };
            let old_reading = tab.reading.replace(surface);
            let old_diagrams = std::mem::replace(&mut tab.prepared_diagrams, prepared.diagrams);
            tab.layout_width = content_width;
            tab.reading_generation = tab.reading_generation.wrapping_add(1);
            tab.reading_revision = revision;
            tab.text = text;
            tab.headings = headings;
            tab.session = Some(session);
            tab.source_loaded = true;
            tab.reading_loaded = true;
            (
                tab.pending_heading.take(),
                tab.pending_scroll.take(),
                tab.reading_viewport.clone(),
                (old_reading, old_diagrams),
            )
        };
        drop(old);

        let alloc_w = viewport.last_allocated_width();
        if alloc_w > 240 {
            let clamped = f64::from(alloc_w).max(260.0);
            if (clamped - content_width).abs() < 16.0 {
                viewport.set_settled_width(alloc_w);
            } else {
                viewport.set_settled_width(alloc_w);
                self.relayout_reading_width(rel, clamped);
            }
        }

        self.bind_edit_sync(rel);
        self.start_checkpoint_timer(rel);
        self.update_disk_stamp(rel);
        self.update_tab_notice(rel);
        self.update_tab_title(rel);

        if pending_heading.is_some() || pending_scroll.is_some() {
            let weak_h = Rc::downgrade(self);
            let rel_h = rel.to_string();
            glib::idle_add_local_once(move || {
                let Some(win) = weak_h.upgrade() else { return };
                if let Some(h) = pending_heading {
                    win.scroll_to_heading_title(&rel_h, &h);
                }
                if let Some(offset) = pending_scroll {
                    win.scroll_to_utf16_offset(&rel_h, offset);
                }
            });
        }
    }

    pub fn handle_tab_closed(self: &Rc<Self>, closed_page: &adw::TabPage) {
        let mut removed = None;
        {
            let mut st = self.state.borrow_mut();
            if let Some(key) = st
                .tabs
                .iter()
                .find(|(_, tab)| &tab.page == closed_page)
                .map(|(k, _)| k.clone())
            {
                removed = st.tabs.remove(&key);
            }
        }
        if let Some(tab) = removed {
            tab.edit_state.borrow_mut().cancel_timers();
            tab.reading_viewport.clear_timers();
            tab.find.teardown();
            tab.completion.teardown();
            if self.state.borrow().tabs.is_empty() {
                self.center_stack.set_visible_child_name("empty");
                self.edit_badge.set_visible(false);
                self.render_outline_empty("未打开笔记");
            } else {
                self.sync_active_tab_ui();
            }
        }
    }

    pub fn active_tab_path(&self) -> Option<String> {
        let page = self.tab_view.selected_page()?;
        self.state
            .borrow()
            .tabs
            .values()
            .find(|t| t.page == page)
            .map(|t| t.path.clone())
    }

    pub fn sync_active_tab_ui(self: &Rc<Self>) {
        let active_path = self.active_tab_path();
        let Some(path) = active_path else {
            self.edit_badge.set_visible(false);
            self.render_outline_empty("未打开笔记");
            return;
        };
        let (editing, headings, loaded) = {
            let st = self.state.borrow();
            if let Some(tab) = st.tabs.get(&path) {
                (tab.editing, tab.headings.clone(), tab.source_loaded)
            } else {
                return;
            }
        };
        self.edit_badge.set_visible(editing);
        self.mode_toggle_btn.set_icon_name(if editing {
            "view-reveal-symbolic"
        } else {
            "document-edit-symbolic"
        });
        self.vault_tree.select_path(&path);
        if loaded {
            self.render_outline(&path, &headings);
        } else {
            self.render_outline_empty("正在解析大纲…");
        }
        self.refresh_backlinks(false);
        let completion = self
            .state
            .borrow()
            .tabs
            .get(&path)
            .map(|tab| tab.completion.clone());
        if !editing && let Some(completion) = completion {
            completion.hide();
        }
    }
}
