// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 窗口动作、知识面板联动、三区统一工具面板显隐及布局持久化。

use crate::core::{APP_ID, APP_VERSION, ColorSchemeChoice, PanelRegion, StaticToolKind};
use crate::dock::DockTabId;
use crate::window::commands::WindowCommand;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{SaveChoice, VaultHandle};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

impl WindowInner {
    pub fn register_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let open_vault = gio::SimpleAction::new("open-vault", None);
        open_vault.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_open_vault();
            }
        });
        self.window.add_action(&open_vault);

        let weak = Rc::downgrade(self);
        let toggle_left = gio::SimpleAction::new("toggle-left-sidebar", None);
        toggle_left.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let next = !win.left_split.shows_sidebar();
                win.left_split.set_show_sidebar(next);
            }
        });
        self.window.add_action(&toggle_left);

        let weak = Rc::downgrade(self);
        let toggle_right = gio::SimpleAction::new("toggle-right-sidebar", None);
        toggle_right.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let next = !win.right_split.shows_sidebar();
                win.right_split.set_show_sidebar(next);
            }
        });
        self.window.add_action(&toggle_right);

        let weak = Rc::downgrade(self);
        let toggle_bottom = gio::SimpleAction::new("toggle-bottom-panel", None);
        toggle_bottom.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let next = !win.state.borrow().bottom_revealed;
                win.send_command(WindowCommand::ToggleBottomPanel(next));
            }
        });
        self.window.add_action(&toggle_bottom);

        let weak = Rc::downgrade(self);
        let toggle_overview = gio::SimpleAction::new("toggle-tab-overview", None);
        toggle_overview.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let next = !win.tab_overview.is_open();
                win.tab_overview.set_open(next);
            }
        });
        self.window.add_action(&toggle_overview);

        let weak = Rc::downgrade(self);
        let toggle_edit = gio::SimpleAction::new("toggle-edit-mode", None);
        toggle_edit.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::SwitchActiveEditMode { announce: true });
            }
        });
        self.window.add_action(&toggle_edit);

        let weak = Rc::downgrade(self);
        let focus_diag = gio::SimpleAction::new("focus-diagram", None);
        focus_diag.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                if win.focus.is_open() {
                    win.focus.close();
                } else if let Some(path) = win.active_tab_path() {
                    win.send_command(WindowCommand::OpenFocus {
                        note_path: path,
                        ordinal: 1,
                    });
                } else {
                    win.show_toast(crate::i18n::tr(crate::i18n::Key::ToastOpenDiagramNoteFirst));
                }
            }
        });
        self.window.add_action(&focus_diag);

        let weak = Rc::downgrade(self);
        let toggle_top_bar = gio::SimpleAction::new("toggle-top-bar", None);
        toggle_top_bar.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let next = !win.toolbar_view.reveals_top_bars();
                win.toolbar_view.set_reveal_top_bars(next);
                win.show_toast(if next {
                    crate::i18n::tr(crate::i18n::Key::ToastTopBarShown)
                } else {
                    crate::i18n::tr(crate::i18n::Key::ToastTopBarHidden)
                });
            }
        });
        self.window.add_action(&toggle_top_bar);

        let weak = Rc::downgrade(self);
        let toggle_fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
        toggle_fullscreen.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                if win.window.is_fullscreen() {
                    win.window.unfullscreen();
                } else {
                    win.window.fullscreen();
                }
            }
        });
        self.window.add_action(&toggle_fullscreen);

        let weak = Rc::downgrade(self);
        let close_tab = gio::SimpleAction::new("close-tab", None);
        close_tab.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade()
                && let Some(page) = win.tab_view.selected_page()
            {
                win.tab_view.close_page(&page);
            }
        });
        self.window.add_action(&close_tab);

        let initial_scheme = self
            .options
            .color_scheme
            .unwrap_or(ColorSchemeChoice::System)
            .as_str();
        let weak = Rc::downgrade(self);
        let color_action = gio::SimpleAction::new_stateful(
            "color-scheme",
            Some(glib::VariantTy::STRING),
            &initial_scheme.to_variant(),
        );
        color_action.connect_activate(move |action, param| {
            let Some(val) = param.and_then(|v| v.str()) else {
                return;
            };
            if let Ok(choice) = ColorSchemeChoice::parse(val)
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::ApplyColorScheme(choice));
                action.set_state(&val.to_variant());
            }
        });
        self.window.add_action(&color_action);

        let weak = Rc::downgrade(self);
        let dock_new_shell =
            gio::SimpleAction::new("dock-new-shell", Some(glib::VariantTy::STRING));
        dock_new_shell.connect_activate(move |_, param| {
            if let Some(reg) = param.and_then(|v| v.str()).and_then(PanelRegion::parse)
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::DockNewShell(reg));
            }
        });
        self.window.add_action(&dock_new_shell);

        let weak = Rc::downgrade(self);
        let dock_launch_agent =
            gio::SimpleAction::new("dock-launch-agent", Some(glib::VariantTy::STRING));
        dock_launch_agent.connect_activate(move |_, param| {
            if let Some(reg) = param.and_then(|v| v.str()).and_then(PanelRegion::parse)
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::DockLaunchAgent(reg));
            }
        });
        self.window.add_action(&dock_launch_agent);

        let weak = Rc::downgrade(self);
        let dock_add_tool = gio::SimpleAction::new("dock-add-tool", Some(glib::VariantTy::STRING));
        dock_add_tool.connect_activate(move |_, param| {
            let Some(raw) = param.and_then(|v| v.str()) else {
                return;
            };
            let Some((reg_str, tool_str)) = raw.split_once(':') else {
                return;
            };
            if let (Some(region), Some(tool)) =
                (PanelRegion::parse(reg_str), StaticToolKind::parse(tool_str))
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::DockAddTool { region, tool });
            }
        });
        self.window.add_action(&dock_add_tool);

        let weak = Rc::downgrade(self);
        let dock_move_tab = gio::SimpleAction::new("dock-move-tab", Some(glib::VariantTy::STRING));
        dock_move_tab.connect_activate(move |_, param| {
            let Some(raw) = param.and_then(|v| v.str()) else {
                return;
            };
            let Some((reg_str, tab_str)) = raw.split_once(':') else {
                return;
            };
            if let (Some(region), Some(tab_id)) =
                (PanelRegion::parse(reg_str), DockTabId::decode(tab_str))
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::DockMoveTab { tab_id, region });
            }
        });
        self.window.add_action(&dock_move_tab);

        let weak = Rc::downgrade(self);
        let dock_close_tab =
            gio::SimpleAction::new("dock-close-tab", Some(glib::VariantTy::STRING));
        dock_close_tab.connect_activate(move |_, param| {
            if let Some(tab_id) = param.and_then(|v| v.str()).and_then(DockTabId::decode)
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::DockCloseTab(tab_id));
            }
        });
        self.window.add_action(&dock_close_tab);

        let weak = Rc::downgrade(self);
        let save_note = gio::SimpleAction::new("save-note", None);
        save_note.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade()
                && let Some(path) = win.active_tab_path()
            {
                win.send_command(WindowCommand::SaveNote {
                    path,
                    choice: SaveChoice::Normal,
                    announce: true,
                });
            }
        });
        self.window.add_action(&save_note);

        let weak = Rc::downgrade(self);
        let new_note = gio::SimpleAction::new("new-note", None);
        new_note.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_new_entry(false);
            }
        });
        self.window.add_action(&new_note);

        let weak = Rc::downgrade(self);
        let new_folder = gio::SimpleAction::new("new-folder", None);
        new_folder.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_new_entry(true);
            }
        });
        self.window.add_action(&new_folder);

        let weak = Rc::downgrade(self);
        let find_in_note = gio::SimpleAction::new("find-in-note", None);
        find_in_note.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.find_open(false);
            }
        });
        self.window.add_action(&find_in_note);

        let weak = Rc::downgrade(self);
        let replace_in_note = gio::SimpleAction::new("replace-in-note", None);
        replace_in_note.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.find_open(true);
            }
        });
        self.window.add_action(&replace_in_note);

        let weak = Rc::downgrade(self);
        let quick_open = gio::SimpleAction::new("quick-open", None);
        quick_open.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenQuickOpen);
            }
        });
        self.window.add_action(&quick_open);

        let weak = Rc::downgrade(self);
        let new_window = gio::SimpleAction::new("new-window", None);
        new_window.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::NewWindow);
            }
        });
        self.window.add_action(&new_window);

        let weak = Rc::downgrade(self);
        let open_settings = gio::SimpleAction::new("open-settings", None);
        open_settings.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenSettings(None));
            }
        });
        self.window.add_action(&open_settings);

        let weak = Rc::downgrade(self);
        let launch_agent = gio::SimpleAction::new("launch-agent", None);
        launch_agent.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::LaunchAgent);
            }
        });
        self.window.add_action(&launch_agent);

        let weak = Rc::downgrade(self);
        let send_path = gio::SimpleAction::new("send-path-to-terminal", None);
        send_path.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::SendPathToTerminal);
            }
        });
        self.window.add_action(&send_path);

        let weak = Rc::downgrade(self);
        let send_sel = gio::SimpleAction::new("send-selection-to-terminal", None);
        send_sel.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::SendSelectionToTerminal);
            }
        });
        self.window.add_action(&send_sel);

        let weak = Rc::downgrade(self);
        let focus_search = gio::SimpleAction::new("focus-search", None);
        focus_search.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::FocusSearchPanel);
            }
        });
        self.window.add_action(&focus_search);

        let weak = Rc::downgrade(self);
        let align_table = gio::SimpleAction::new("align-table", None);
        align_table.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.perform_align_table();
            }
        });
        self.window.add_action(&align_table);

        let weak = Rc::downgrade(self);
        let toggle_task = gio::SimpleAction::new("toggle-task", None);
        toggle_task.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.perform_toggle_task();
            }
        });
        self.window.add_action(&toggle_task);

        let weak = Rc::downgrade(self);
        let rename_active = gio::SimpleAction::new("rename-active", None);
        rename_active.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade()
                && let Some(path) = win.active_tab_path()
            {
                win.begin_rename(&path, false);
            }
        });
        self.window.add_action(&rename_active);

        let weak = Rc::downgrade(self);
        let copy_note_text = gio::SimpleAction::new("copy-note-text", None);
        copy_note_text.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.copy_active_note_plain_text();
            }
        });
        self.window.add_action(&copy_note_text);

        let weak = Rc::downgrade(self);
        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.show_about_dialog();
            }
        });
        self.window.add_action(&about);
    }

    pub fn init_knowledge_panels(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.search_panel.set_on_open(Rc::new(move |path, start| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenNote {
                    relpath: path.clone(),
                    heading: None,
                });
                if let Some(start) = start {
                    win.pending_scroll_to(&path, start);
                }
            }
        }));
        let weak = Rc::downgrade(self);
        self.tags_panel.set_on_open(Rc::new(move |path, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenNote {
                    relpath: path,
                    heading: None,
                });
            }
        }));
        let weak = Rc::downgrade(self);
        self.backlinks_panel
            .set_on_open(Rc::new(move |path, start| {
                if let Some(win) = weak.upgrade() {
                    win.send_command(WindowCommand::OpenNote {
                        relpath: path.clone(),
                        heading: None,
                    });
                    if let Some(start) = start {
                        win.pending_scroll_to(&path, start);
                    }
                }
            }));
        let weak = Rc::downgrade(self);
        self.quick_open.set_on_open(Rc::new(move |path, _| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenNote {
                    relpath: path,
                    heading: None,
                });
            }
        }));
    }

    pub fn attach_vault_to_panels(&self, handle: &Arc<VaultHandle>) {
        self.search_panel.set_handle(Arc::clone(handle));
        self.tags_panel.set_handle(Arc::clone(handle));
        self.backlinks_panel.set_handle(Arc::clone(handle));
        self.quick_open.set_handle(Arc::clone(handle));
        for tab in self.state.borrow().tabs.values() {
            tab.completion.set_handle(Arc::clone(handle));
        }
    }

    fn pending_scroll_to(&self, path: &str, offset: u64) {
        let loaded = self
            .state
            .borrow()
            .tabs
            .get(path)
            .is_some_and(|tab| tab.source_loaded);
        if loaded {
            self.scroll_to_utf16_offset(path, offset);
        } else if let Some(tab) = self.state.borrow_mut().tabs.get_mut(path) {
            tab.pending_scroll = Some(offset);
        }
    }

    pub fn activate_static_tool(&self, kind: StaticToolKind) {
        self.tool_dock.activate_static_tool(kind);
    }

    pub fn open_tag_from_reading(&self, tag: &str) {
        self.activate_static_tool(StaticToolKind::Tags);
        self.tags_panel.select_tag(tag);
    }

    pub fn focus_search_panel(&self) {
        self.activate_static_tool(StaticToolKind::Search);
        self.search_panel.focus_entry();
    }

    pub fn open_quick_open(&self) {
        self.quick_open
            .open(self.window.upcast_ref::<gtk4::Window>());
    }

    pub fn refresh_backlinks(&self, force: bool) {
        let path = self.active_tab_path();
        self.backlinks_panel.update(path, force);
    }

    pub fn refresh_knowledge_after_write(&self) {
        self.tags_panel.refresh();
        self.search_panel.rerun_now();
        self.refresh_backlinks(true);
    }

    #[cfg(feature = "automation")]
    pub fn is_bottom_panel_revealed(&self) -> bool {
        self.state.borrow().bottom_revealed
            && self
                .tool_dock
                .region_widget(PanelRegion::Bottom)
                .is_visible()
    }

    pub fn set_region_revealed(self: &Rc<Self>, region: PanelRegion, revealed: bool) {
        match region {
            PanelRegion::Left => self.left_split.set_show_sidebar(revealed),
            PanelRegion::Right => self.right_split.set_show_sidebar(revealed),
            PanelRegion::Bottom => self.set_bottom_panel_revealed(revealed),
        }
    }

    pub fn set_bottom_panel_revealed(self: &Rc<Self>, revealed: bool) {
        let bottom_height = {
            let mut st = self.state.borrow_mut();
            st.bottom_revealed = revealed;
            st.bottom_height
        };
        if self.bottom_toggle.is_active() != revealed {
            self.bottom_toggle.set_active(revealed);
        }
        let bottom_box = self.tool_dock.region_widget(PanelRegion::Bottom);
        if revealed {
            bottom_box.set_visible(true);
            self.apply_bottom_paned_position(bottom_height);
            if self.tool_dock.tab_count(PanelRegion::Bottom) == 0 {
                self.terminal_host.open_shell_in_region(PanelRegion::Bottom);
            }
        } else {
            bottom_box.set_visible(false);
            self.state.borrow_mut().pending_bottom_position = false;
        }
    }

    fn apply_bottom_paned_position(self: &Rc<Self>, bottom_height: f64) {
        let total_h = self.center_paned.height();
        if total_h > 200 {
            self.state.borrow_mut().suppressing_bottom_pos = true;
            let pos = (total_h - bottom_height.round() as i32).max(120);
            self.center_paned.set_position(pos);
            self.state.borrow_mut().suppressing_bottom_pos = false;
            self.state.borrow_mut().pending_bottom_position = false;
            return;
        }
        self.state.borrow_mut().pending_bottom_position = true;
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(win) = weak.upgrade() else { return };
            let (revealed, pending, bh) = {
                let st = win.state.borrow();
                (
                    st.bottom_revealed,
                    st.pending_bottom_position,
                    st.bottom_height,
                )
            };
            if revealed && pending && win.center_paned.height() > 200 {
                win.apply_bottom_paned_position(bh);
            }
        });
    }

    pub fn load_saved_layout(self: &Rc<Self>) {
        let core = self
            .state
            .borrow()
            .services
            .as_ref()
            .map(|s| Arc::clone(&s.core));
        let Some(core) = core else { return };
        let Ok(Some(json)) = core.layout_json() else {
            return;
        };
        let Ok(layout) = serde_json::from_str::<hortus_config::LayoutState>(&json) else {
            return;
        };
        self.state.borrow_mut().suppressing_layout_save = true;
        let (left_w, right_w, bottom_h) = self.tool_dock.apply_layout_state(&layout);
        self.state.borrow_mut().bottom_height = bottom_h.clamp(120.0, 520.0);
        self.left_split
            .set_show_sidebar(self.tool_dock.tab_count(PanelRegion::Left) > 0);
        self.right_split
            .set_show_sidebar(self.tool_dock.tab_count(PanelRegion::Right) > 0);
        self.apply_sidebar_widths(left_w, right_w);
        self.state.borrow_mut().suppressing_layout_save = false;
    }

    pub fn schedule_layout_save(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            if st.disposing || st.suppressing_layout_save {
                return;
            }
            if let Some(old) = st.layout_save_source.take() {
                old.remove();
            }
        }
        let weak = Rc::downgrade(self);
        let src = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if let Some(win) = weak.upgrade() {
                win.state.borrow_mut().layout_save_source = None;
                win.persist_layout_now();
            }
        });
        self.state.borrow_mut().layout_save_source = Some(src);
    }

    pub fn persist_layout_now(&self) {
        let (core, left_w, right_w, bottom_height, suppressing) = {
            let st = self.state.borrow();
            (
                st.services.as_ref().map(|s| Arc::clone(&s.core)),
                st.left_sidebar_width,
                st.right_sidebar_width,
                st.bottom_height,
                st.disposing || st.suppressing_layout_save,
            )
        };
        if suppressing {
            return;
        }
        let Some(core) = core else { return };
        let layout = self
            .tool_dock
            .export_layout_state(left_w, right_w, bottom_height);
        if let Ok(json) = serde_json::to_string(&layout) {
            gio::spawn_blocking(move || {
                let _ = core.save_layout(json);
            });
        }
    }

    fn show_about_dialog(&self) {
        let dialog = adw::AboutDialog::builder()
            .application_name("Hortus")
            .application_icon(APP_ID)
            .version(APP_VERSION)
            .developer_name("groundgrounder")
            .copyright("© 2026 groundgrounder")
            .license_type(gtk4::License::Gpl30Only)
            .website("https://github.com/groundgrounder/Hortus")
            .build();
        dialog.add_legal_section(
            crate::i18n::tr(crate::i18n::Key::AboutThirdPartyNoticesTitle),
            None,
            gtk4::License::Custom,
            Some(&gtk4::glib::markup_escape_text(crate::i18n::tr(
                crate::i18n::Key::AboutThirdPartyNoticesSummary,
            ))),
        );
        dialog.present(Some(&self.window));
    }
}

pub fn apply_color_scheme(choice: ColorSchemeChoice) {
    let manager = adw::StyleManager::default();
    match choice {
        ColorSchemeChoice::System => manager.set_color_scheme(adw::ColorScheme::Default),
        ColorSchemeChoice::Light => manager.set_color_scheme(adw::ColorScheme::ForceLight),
        ColorSchemeChoice::Dark => manager.set_color_scheme(adw::ColorScheme::ForceDark),
    }
}

pub fn build_primary_menu() -> gio::Menu {
    use crate::i18n::{Key, fmt1, tr};
    let menu = gio::Menu::new();

    let file_sec = gio::Menu::new();
    file_sec.append(Some(tr(Key::MenuOpenFolder)), Some("win.open-vault"));
    file_sec.append(Some(tr(Key::MenuNewWindow)), Some("win.new-window"));
    file_sec.append(Some(tr(Key::MenuQuickOpen)), Some("win.quick-open"));
    file_sec.append(Some(tr(Key::MenuNewNote)), Some("win.new-note"));
    file_sec.append(Some(tr(Key::MenuNewFolder)), Some("win.new-folder"));
    file_sec.append(Some(tr(Key::MenuRenameNote)), Some("win.rename-active"));
    file_sec.append(Some(tr(Key::MenuSaveNote)), Some("win.save-note"));
    file_sec.append(Some(tr(Key::MenuCopyNoteText)), Some("win.copy-note-text"));
    file_sec.append(Some(tr(Key::MenuFindInNote)), Some("win.find-in-note"));
    file_sec.append(
        Some(tr(Key::MenuReplaceInNote)),
        Some("win.replace-in-note"),
    );
    file_sec.append(Some(tr(Key::MenuSearchFolder)), Some("win.focus-search"));
    file_sec.append(
        Some(tr(Key::MenuToggleReadingEditing)),
        Some("win.toggle-edit-mode"),
    );
    file_sec.append(Some(tr(Key::MenuCloseTab)), Some("win.close-tab"));
    menu.append_section(None, &file_sec);

    let agent_sec = gio::Menu::new();
    agent_sec.append(
        Some(tr(Key::MenuNewTerminalBottom)),
        Some("win.dock-new-shell('bottom')"),
    );
    agent_sec.append(Some(tr(Key::MenuLaunchAgent)), Some("win.launch-agent"));
    agent_sec.append(
        Some(tr(Key::MenuSendPathToTerminal)),
        Some("win.send-path-to-terminal"),
    );
    agent_sec.append(
        Some(tr(Key::MenuSendSelectionToTerminal)),
        Some("win.send-selection-to-terminal"),
    );
    agent_sec.append(Some(tr(Key::MenuPreferences)), Some("win.open-settings"));
    menu.append_section(None, &agent_sec);

    let view_sec = gio::Menu::new();
    view_sec.append(
        Some(tr(Key::MenuTabOverview)),
        Some("win.toggle-tab-overview"),
    );
    view_sec.append(Some(tr(Key::MenuFocusDiagram)), Some("win.focus-diagram"));
    view_sec.append(
        Some(tr(Key::MenuToggleLeftSidebar)),
        Some("win.toggle-left-sidebar"),
    );
    view_sec.append(
        Some(tr(Key::MenuToggleRightSidebar)),
        Some("win.toggle-right-sidebar"),
    );
    view_sec.append(
        Some(tr(Key::MenuToggleBottomPanel)),
        Some("win.toggle-bottom-panel"),
    );
    view_sec.append(Some(tr(Key::MenuToggleTopBar)), Some("win.toggle-top-bar"));
    view_sec.append(
        Some(tr(Key::MenuToggleFullscreen)),
        Some("win.toggle-fullscreen"),
    );
    menu.append_section(None, &view_sec);

    let panel_sub = gio::Menu::new();
    for tool in StaticToolKind::ALL {
        let tool_sub = gio::Menu::new();
        for region in PanelRegion::ALL {
            let item = gio::MenuItem::new(Some(&fmt1(Key::MenuMoveToRegion, region.title())), None);
            item.set_action_and_target_value(
                Some("win.dock-add-tool"),
                Some(&format!("{}:{}", region.id(), tool.id()).to_variant()),
            );
            tool_sub.append_item(&item);
        }
        panel_sub.append_submenu(Some(tool.title()), &tool_sub);
    }
    menu.append_submenu(Some(tr(Key::MenuPanelPosition)), &panel_sub);

    let theme_sub = gio::Menu::new();
    theme_sub.append(
        Some(tr(Key::ThemeSystem)),
        Some("win.color-scheme('system')"),
    );
    theme_sub.append(Some(tr(Key::ThemeLight)), Some("win.color-scheme('light')"));
    theme_sub.append(Some(tr(Key::ThemeDark)), Some("win.color-scheme('dark')"));
    menu.append_submenu(Some(tr(Key::MenuAppearance)), &theme_sub);

    let about_sec = gio::Menu::new();
    about_sec.append(Some(tr(Key::MenuAbout)), Some("win.about"));
    menu.append_section(None, &about_sec);
    menu
}
