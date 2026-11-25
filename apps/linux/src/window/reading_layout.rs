// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 阅读表面重排、真宽度校正、双链解析及阅读/编辑模式切换。

use crate::core::Utf16Mapper;
use crate::knowledge::clear_box;
use crate::reading::{
    PrepareReadingRequest, ReadingEnv, ReadingSurfaceHandle, build_reading_surface,
    prepare_reading_note_bg,
};
use crate::render::embed::EmbedCallbacks;
use crate::render::math::MathRenderService;
use crate::window::commands::WindowCommand;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DiagramEngine, VaultHandle, WorkspaceCore};
use libadwaita as adw;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum ReadingScrollTarget {
    Fraction(f64),
    SourceUtf16(u64),
}

impl WindowInner {
    pub fn make_reading_env(
        self: &Rc<Self>,
        note_path: &str,
        vault_root: String,
        core: Arc<WorkspaceCore>,
        handle: Arc<VaultHandle>,
        diagram_engine: Arc<DiagramEngine>,
        math_service: Arc<MathRenderService>,
    ) -> ReadingEnv {
        let src_note = note_path.to_string();
        let weak_open = Rc::downgrade(self);
        let weak_ext = Rc::downgrade(self);
        let weak_tag = Rc::downgrade(self);
        let weak_exp = Rc::downgrade(self);
        let weak_focus = Rc::downgrade(self);
        let weak_src = Rc::downgrade(self);
        let weak_tag2 = Rc::downgrade(self);

        let callbacks = EmbedCallbacks {
            on_open_note: Rc::new(move |target, heading| {
                if let Some(win) = weak_open.upgrade() {
                    win.send_command(WindowCommand::ResolveWikilink {
                        source_note: src_note.clone(),
                        raw_target: target,
                        heading,
                    });
                }
            }),
            on_open_external: Rc::new(move |url| {
                if let Some(win) = weak_ext.upgrade()
                    && (url.starts_with("http://") || url.starts_with("https://"))
                {
                    let launcher = gtk4::UriLauncher::new(&url);
                    launcher.launch(Some(&win.window), gio::Cancellable::NONE, |_| {});
                }
            }),
            on_open_tag: Rc::new(move |tag| {
                if let Some(win) = weak_tag.upgrade() {
                    win.send_command(WindowCommand::OpenTagFromReading(tag));
                }
            }),
            on_export_diagram: Rc::new(move |kind, meta| {
                if let Some(win) = weak_exp.upgrade() {
                    win.prompt_and_export_diagram(kind, meta);
                }
            }),
            on_open_focus: Rc::new(move |target_note, ordinal| {
                if let Some(win) = weak_focus.upgrade() {
                    win.send_command(WindowCommand::OpenFocus {
                        note_path: target_note,
                        ordinal,
                    });
                }
            }),
        };

        let rel_src = note_path.to_string();
        let scrolled = self
            .state
            .borrow()
            .tabs
            .get(note_path)
            .map(|tab| tab.reading_scrolled.clone());
        let font_size = self
            .state
            .borrow()
            .services
            .as_ref()
            .map(|s| s.config.snapshot().font_size)
            .unwrap_or(15.0);
        ReadingEnv {
            note_path: note_path.to_string(),
            vault_root,
            core,
            handle,
            diagram_engine,
            math_service,
            font_size,
            content_width: self.reading_content_width(),
            ancestors: vec![note_path.to_string()],
            depth: 0,
            parent_window: Some(self.window.clone().upcast()),
            scrolled,
            callbacks,
            on_open_source: Rc::new(move |byte_start, _| {
                if let Some(win) = weak_src.upgrade() {
                    win.send_command(WindowCommand::SwitchToSourceAtByte {
                        note_path: rel_src.clone(),
                        byte_start,
                    });
                }
            }),
            on_open_tag: Rc::new(move |tag| {
                if let Some(win) = weak_tag2.upgrade() {
                    win.send_command(WindowCommand::OpenTagFromReading(tag));
                }
            }),
        }
    }

    pub fn open_note_or_resolve(
        self: &Rc<Self>,
        source_note: &str,
        raw_target: &str,
        heading: Option<String>,
    ) {
        let lower = raw_target.trim().to_ascii_lowercase();
        let has_path_hint = raw_target.contains('/')
            || lower.ends_with(".md")
            || lower.ends_with(".markdown")
            || lower.ends_with(".mmd");
        if has_path_hint {
            let known = self
                .state
                .borrow()
                .vault_handle()
                .and_then(|handle| handle.read_note(raw_target.to_string()).ok())
                .is_some();
            if known {
                self.open_note_at(raw_target, heading);
                return;
            }
        }
        let resolved = self
            .state
            .borrow()
            .vault_handle()
            .map(|handle| handle.resolve_link(source_note.to_string(), raw_target.to_string()));
        if let Some(resolved) = resolved {
            if let Some(path) = resolved.resolved_path {
                self.open_note_at(&path, heading);
            } else if resolved.candidates.len() > 1 {
                self.show_toast(&crate::i18n::fmt2(
                    crate::i18n::Key::ToastAmbiguousLink,
                    raw_target,
                    resolved.candidates.len(),
                ));
            } else {
                self.show_toast(&crate::i18n::fmt1(
                    crate::i18n::Key::ToastMissingLink,
                    raw_target,
                ));
            }
        } else {
            self.open_note_at(raw_target, heading);
        }
    }

    pub fn refresh_all_reading_tabs(self: &Rc<Self>) {
        self.terminal_host.apply_theme();
        let paths: Vec<String> = self.state.borrow().tabs.keys().cloned().collect();
        for p in paths {
            self.refresh_reading_tab(&p, None);
        }
    }

    pub fn refresh_reading_tab(
        self: &Rc<Self>,
        note_path: &str,
        restore_scroll: Option<ReadingScrollTarget>,
    ) {
        let (session, generation) = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(note_path) else {
                return;
            };
            tab.reading_generation = tab.reading_generation.wrapping_add(1);
            (tab.session.clone(), tab.reading_generation)
        };
        let (services, vault_root, vault_handle) = {
            let st = self.state.borrow();
            (st.services.clone(), st.vault_root(), st.vault_handle())
        };
        let (Some(services), Some(vault_root), Some(session)) = (services, vault_root, session)
        else {
            return;
        };

        let content_width = self.reading_content_width();
        let font_size = services.config.snapshot().font_size;
        let dark = adw::StyleManager::default().is_dark();
        let main_thread = std::thread::current().id();
        let weak = Rc::downgrade(self);
        let rel = note_path.to_string();
        let core_bg = Arc::clone(&services.core);
        let diag_bg = Arc::clone(&services.diagram_engine);
        let math_bg = Arc::clone(&services.math_service);

        glib::MainContext::default().spawn_local(async move {
            let rel_bg = rel.clone();
            let vault_bg = vault_root.clone();
            let res = gio::spawn_blocking(move || {
                prepare_reading_note_bg(&PrepareReadingRequest {
                    note_path: rel_bg,
                    vault_root: vault_bg,
                    session,
                    core: core_bg,
                    handle: vault_handle,
                    diagram_engine: diag_bg,
                    math_service: math_bg,
                    content_width,
                    font_size,
                    dark,
                    main_thread,
                })
            })
            .await;

            let Some(win) = weak.upgrade() else { return };
            if win.state.borrow().disposing {
                return;
            }
            if let Ok(Ok(prepared)) = res {
                let current = win
                    .state
                    .borrow()
                    .tabs
                    .get(&rel)
                    .map(|tab| tab.reading_generation);
                if current != Some(generation) {
                    return;
                }
                let Some(handle) = win.state.borrow().vault_handle() else {
                    return;
                };
                let env = win.make_reading_env(
                    &rel,
                    vault_root.clone(),
                    Arc::clone(&services.core),
                    handle,
                    Arc::clone(&services.diagram_engine),
                    Arc::clone(&services.math_service),
                );
                let weak_c = Rc::downgrade(&win);
                let rel_c = rel.clone();
                let vault_c = vault_root;
                let core_c = Arc::clone(&services.core);
                let weak_scene = Rc::downgrade(&win);
                let rel_scene = rel.clone();
                let surface = build_reading_surface(
                    &prepared,
                    &env,
                    Rc::new(move |ordinal, collapsed, new_scene| {
                        let _ = core_c.remember_diagram(
                            vault_c.clone(),
                            rel_c.clone(),
                            ordinal,
                            collapsed.clone(),
                        );
                        if let Some(win_c) = weak_c.upgrade()
                            && let Some(tab) = win_c.state.borrow_mut().tabs.get_mut(&rel_c)
                            && let Some(d) = tab
                                .prepared_diagrams
                                .iter_mut()
                                .find(|d| d.ordinal == ordinal)
                        {
                            d.collapsed = collapsed;
                            d.previous = new_scene.previous.clone();
                            d.scene = Some(new_scene);
                        }
                    }),
                    Rc::new(move |ordinal, new_scene| {
                        if let Some(win_s) = weak_scene.upgrade()
                            && let Some(tab) = win_s.state.borrow_mut().tabs.get_mut(&rel_scene)
                            && let Some(d) = tab
                                .prepared_diagrams
                                .iter_mut()
                                .find(|d| d.ordinal == ordinal)
                        {
                            d.previous = new_scene.previous.clone();
                            d.scene = Some(new_scene);
                        }
                    }),
                );
                let reading_slot = win
                    .state
                    .borrow()
                    .tabs
                    .get(&rel)
                    .map(|tab| tab.reading_slot.clone());
                let Some(reading_slot) = reading_slot else {
                    return;
                };
                clear_box(&reading_slot);
                reading_slot.append(&surface.root);
                let updated_headings = prepared.headings.clone();
                let (find_bar, editing_now, old) = {
                    let mut st = win.state.borrow_mut();
                    let Some(tab) = st.tabs.get_mut(&rel) else {
                        return;
                    };
                    let old_reading = tab.reading.replace(surface.clone());
                    let old_diagrams =
                        std::mem::replace(&mut tab.prepared_diagrams, prepared.diagrams);
                    tab.reading_loaded = true;
                    tab.reading_revision = prepared.revision;
                    tab.text = prepared.source_text;
                    tab.headings = updated_headings.clone();
                    tab.layout_width = content_width;
                    tab.surface_rebuilds = tab.surface_rebuilds.saturating_add(1);
                    let alloc_w = tab.reading_viewport.last_allocated_width();
                    if alloc_w > 240 {
                        tab.reading_viewport.set_settled_width(alloc_w);
                    }
                    (tab.find.clone(), tab.editing, (old_reading, old_diagrams))
                };
                drop(old);
                if !editing_now && find_bar.is_open() {
                    find_bar.bind_reading(&surface);
                    find_bar.recompute_if_open();
                }
                if win.active_tab_path().as_deref() == Some(rel.as_str()) {
                    win.render_outline(&rel, &updated_headings);
                }
                if let Some(target) = restore_scroll {
                    let expected_token = surface.scroll_token();
                    schedule_reading_scroll_restore(
                        Rc::downgrade(&win),
                        rel.clone(),
                        target,
                        expected_token,
                        0.0,
                        0,
                        12,
                    );
                }
            }
        });
    }

    pub fn reading_content_width(&self) -> f64 {
        let viewport_alloc = self
            .active_tab_path()
            .and_then(|path| {
                let st = self.state.borrow();
                st.tabs
                    .get(&path)
                    .map(|tab| tab.reading_viewport.last_allocated_width())
            })
            .unwrap_or(0);
        if viewport_alloc > 240 {
            return f64::from(viewport_alloc).max(260.0);
        }
        let tab_view_width = f64::from(self.tab_view.width());
        if tab_view_width > 160.0 {
            let pad = f64::from(crate::platform::viewport::responsive_horizontal_padding(
                tab_view_width as i32,
            ));
            return (tab_view_width - pad).max(260.0);
        }
        let window_w = f64::from(self.window.width()).max(720.0);
        let sidebar_width = |w: f64| (w * 0.24).clamp(220.0, 480.0);
        let mut content = window_w;
        if self.left_split.shows_sidebar() && !self.left_split.is_collapsed() {
            content -= sidebar_width(window_w);
        }
        if self.right_split.shows_sidebar() && !self.right_split.is_collapsed() {
            content -= sidebar_width(window_w);
        }
        let pad = f64::from(crate::platform::viewport::responsive_horizontal_padding(
            content as i32,
        ));
        (content - pad).max(260.0)
    }

    pub fn relayout_reading_width(&self, note_path: &str, target_width: f64) {
        let reading = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(note_path) else {
                return;
            };
            if !tab.reading_loaded {
                return;
            }
            tab.layout_width = target_width;
            tab.in_place_relayouts = tab.in_place_relayouts.saturating_add(1);
            tab.reading.clone()
        };
        if let Some(surface) = reading {
            surface.relayout_to_width(target_width);
        }
    }

    pub fn apply_relayout_width(self: &Rc<Self>, avail_w: i32) {
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let target = f64::from(avail_w).max(260.0);
        let (loaded, editing, current, viewport) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            (
                tab.reading_loaded,
                tab.editing,
                tab.layout_width,
                tab.reading_viewport.clone(),
            )
        };
        if !loaded || editing {
            return;
        }
        if viewport.settled_width() == 0 {
            viewport.set_settled_width(avail_w);
        }
        if current > 0.0 && (target - current).abs() < 16.0 {
            return;
        }
        self.relayout_reading_width(&path, target);
    }

    pub fn check_active_tab_width_alignment(self: &Rc<Self>) {
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let (loaded, editing, current, alloc_w, viewport) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            (
                tab.reading_loaded,
                tab.editing,
                tab.layout_width,
                tab.reading_viewport.last_allocated_width(),
                tab.reading_viewport.clone(),
            )
        };
        if !loaded || editing || alloc_w <= 240 {
            return;
        }
        let target = f64::from(alloc_w).max(260.0);
        if current > 0.0 && (target - current).abs() >= 16.0 {
            viewport.mark_reflowed_width(alloc_w);
            self.relayout_reading_width(&path, target);
        }
    }

    pub fn copy_active_note_plain_text(&self) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastNoNoteOpen));
            return;
        };
        let text = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            if !tab.editing
                && let Some(ref surface) = tab.reading
            {
                let plain = surface.plain_text();
                if !plain.is_empty() {
                    plain
                } else {
                    tab.edit_state.borrow().text.clone()
                }
            } else {
                tab.edit_state.borrow().text.clone()
            }
        };
        if let Some(display) = gtk4::gdk::Display::default() {
            display.clipboard().set_text(&text);
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastCopiedPlainText));
        }
    }

    pub fn switch_to_source_at_byte(self: &Rc<Self>, note_path: &str, byte_start: u64) {
        let mapper_u16 = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(note_path) else {
                return;
            };
            let u16_off = Utf16Mapper::new(&tab.text).byte_to_utf16(byte_start as usize) as u64;
            tab.editing = true;
            Some((tab.edit_view.clone(), u16_off, tab.content_stack.clone()))
        };
        if let Some((ev, u16_off, content_stack)) = mapper_u16 {
            content_stack.set_visible_child_name("edit");
            self.sync_active_tab_ui();
            self.scroll_to_utf16_offset(note_path, u16_off);
            ev.grab_focus();
        }
    }

    #[cfg(feature = "automation")]
    pub fn toggle_active_tab_edit_mode(self: &Rc<Self>) {
        self.switch_edit_mode(true);
    }

    pub fn switch_edit_mode(self: &Rc<Self>, announce: bool) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastNoNoteOpen));
            return;
        };
        let (new_editing, needs_reading_refresh, anchor_u16, edit_view, content_stack) = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(&path) else {
                return;
            };
            let (target_u16, needs_refresh) = if !tab.editing {
                let u16_off = tab
                    .reading
                    .as_ref()
                    .map(ReadingSurfaceHandle::current_source_utf16)
                    .unwrap_or(0);
                (u16_off, false)
            } else {
                let top_y = tab.edit_scrolled.vadjustment().value().max(0.0) as i32;
                let (iter, _) = tab.edit_view.line_at_y(top_y + 12);
                let char_off = iter.offset().max(0) as usize;
                let edit = tab.edit_state.borrow();
                let u16_off: u64 = edit
                    .text
                    .chars()
                    .take(char_off)
                    .map(|c| c.len_utf16() as u64)
                    .sum();
                let edit_text = edit.text.clone();
                let edit_rev = edit.revision;
                drop(edit);
                tab.text = edit_text;
                let dirty_rev = edit_rev != tab.reading_revision || !tab.reading_loaded;
                (u16_off, dirty_rev)
            };
            tab.editing = !tab.editing;
            (
                tab.editing,
                needs_refresh,
                target_u16,
                tab.edit_view.clone(),
                tab.content_stack.clone(),
            )
        };
        content_stack.set_visible_child_name(if new_editing { "edit" } else { "reading" });
        self.sync_active_tab_ui();
        if !new_editing {
            if needs_reading_refresh {
                self.refresh_reading_tab(&path, Some(ReadingScrollTarget::SourceUtf16(anchor_u16)));
            } else {
                self.check_active_tab_width_alignment();
                self.scroll_to_utf16_offset(&path, anchor_u16);
            }
        } else {
            self.scroll_to_utf16_offset(&path, anchor_u16);
            edit_view.grab_focus();
        }
        if announce {
            if new_editing {
                self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSwitchedToEdit));
            } else {
                self.show_toast(crate::i18n::tr(crate::i18n::Key::ToastSwitchedToReading));
            }
        }
    }
}

fn schedule_reading_scroll_restore(
    weak_win: std::rc::Weak<WindowInner>,
    rel: String,
    target: ReadingScrollTarget,
    expected_token: u64,
    last_upper: f64,
    stable_ticks: u8,
    attempts_left: u8,
) {
    glib::idle_add_local_once(move || {
        let Some(win) = weak_win.upgrade() else {
            return;
        };
        let reading = win
            .state
            .borrow()
            .tabs
            .get(&rel)
            .and_then(|tab| tab.reading.clone());
        let Some(reading) = reading else {
            return;
        };
        if reading.scroll_token() != expected_token {
            return;
        }
        let adj = reading.scrolled.vadjustment();
        let cur_upper = adj.upper();
        let unsettled_height = reading.content_box.height() <= 0 || cur_upper <= 0.0;
        let next_stable = if !unsettled_height && (cur_upper - last_upper).abs() <= 1.0 {
            stable_ticks.saturating_add(1)
        } else {
            0
        };
        if (unsettled_height || next_stable < 2) && attempts_left > 0 {
            glib::timeout_add_local_once(Duration::from_millis(24), move || {
                schedule_reading_scroll_restore(
                    weak_win,
                    rel,
                    target,
                    expected_token,
                    cur_upper,
                    next_stable,
                    attempts_left - 1,
                );
            });
            return;
        }
        match target {
            ReadingScrollTarget::Fraction(fraction) => reading.scroll_fraction(fraction),
            ReadingScrollTarget::SourceUtf16(u16_off) => reading.scroll_to_source_utf16(u16_off),
        }
    });
}
