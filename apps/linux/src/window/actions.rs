// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 窗口动作、知识面板联动、可移动侧栏/底部面板及样式切换器。

use crate::core::{APP_ID, APP_VERSION, ColorSchemeChoice, PanelRegion, PlaceholderKind};
use crate::knowledge::clear_box;
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
                    win.show_toast("请先打开包含 Mermaid 图表的笔记");
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
                    "已显示顶栏"
                } else {
                    "已隐藏顶栏，按 Ctrl+Shift+H 恢复"
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
        let move_panel = gio::SimpleAction::new("move-panel", Some(glib::VariantTy::STRING));
        move_panel.connect_activate(move |_, param| {
            let Some(id) = param.and_then(|v| v.str()) else {
                return;
            };
            if let Some(kind) = PlaceholderKind::parse(id)
                && let Some(win) = weak.upgrade()
            {
                win.send_command(WindowCommand::TogglePlaceholderRegion(kind));
            }
        });
        self.window.add_action(&move_panel);

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

    pub fn switch_left_panel(&self, name: &str) {
        self.left_stack.set_visible_child_name(name);
        self.rebuild_left_switcher();
    }

    fn rebuild_left_switcher(&self) {
        let Some(switcher) = self.left_stack.parent().and_downcast::<gtk4::Box>() else {
            return;
        };
        let Some(switcher) = switcher.first_child().and_downcast::<gtk4::Box>() else {
            return;
        };
        clear_box(&switcher);
        populate_stack_switcher(
            &switcher,
            &self.left_stack,
            &[("files", "文件"), ("search", "搜索"), ("tags", "标签")],
        );
    }

    pub fn open_tag_from_reading(&self, tag: &str) {
        self.left_split.set_show_sidebar(true);
        self.switch_left_panel("tags");
        self.tags_panel.select_tag(tag);
    }

    pub fn focus_search_panel(&self) {
        self.left_split.set_show_sidebar(true);
        self.switch_left_panel("search");
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

    pub fn set_bottom_panel_revealed(&self, revealed: bool) {
        self.state.borrow_mut().bottom_revealed = revealed;
        if self.bottom_toggle.is_active() != revealed {
            self.bottom_toggle.set_active(revealed);
        }
        if revealed {
            self.bottom_revealer.set_visible(true);
            if self.bottom_stack.visible_child_name().as_deref()
                == Some(PlaceholderKind::Terminal.id())
            {
                self.terminal_host.ensure_default_shell();
            }
        }
        self.bottom_revealer.set_reveal_child(revealed);
    }

    pub fn toggle_placeholder_region(&self, kind: PlaceholderKind) {
        let next = self.state.borrow_mut().placement.toggle_region(kind);
        let widget = self.placeholders.widget(kind);
        if let Some(parent) = widget.parent().and_downcast::<gtk4::Stack>() {
            parent.remove(widget);
        }
        match next {
            PanelRegion::Right => {
                self.right_stack.add_named(widget, Some(kind.id()));
                self.right_stack.set_visible_child_name(kind.id());
                self.right_split.set_show_sidebar(true);
            }
            PanelRegion::Bottom => {
                self.bottom_stack.add_named(widget, Some(kind.id()));
                self.bottom_stack.set_visible_child_name(kind.id());
                self.set_bottom_panel_revealed(true);
            }
        }
        self.rebuild_panel_switchers();
        if kind == PlaceholderKind::Terminal {
            self.terminal_host.ensure_default_shell();
        } else if kind == PlaceholderKind::Changes {
            self.changes_panel.refresh();
        }
        let region_name = match next {
            PanelRegion::Right => "右侧栏",
            PanelRegion::Bottom => "底部面板",
        };
        self.show_toast(&format!("已将「{}」移至{}", kind.title(), region_name));
    }

    pub fn rebuild_panel_switchers(&self) {
        clear_box(&self.right_switcher);
        clear_box(&self.bottom_switcher);

        let placement = self.state.borrow().placement.clone();
        let mut right_items: Vec<(&'static str, &'static str)> = vec![("outline", "大纲")];
        for kind in placement.views_in(PanelRegion::Right) {
            right_items.push((kind.id(), kind.title()));
        }
        populate_stack_switcher(&self.right_switcher, &self.right_stack, &right_items);
        if self.right_stack.visible_child_name().is_none() {
            self.right_stack.set_visible_child_name("outline");
        }

        let bottom_views = placement.views_in(PanelRegion::Bottom);
        let bottom_items: Vec<(&'static str, &'static str)> =
            bottom_views.iter().map(|k| (k.id(), k.title())).collect();
        populate_stack_switcher(&self.bottom_switcher, &self.bottom_stack, &bottom_items);
        if let Some((first_id, _)) = bottom_items.first()
            && self
                .bottom_stack
                .child_by_name(
                    self.bottom_stack
                        .visible_child_name()
                        .as_deref()
                        .unwrap_or(""),
                )
                .is_none()
        {
            self.bottom_stack.set_visible_child_name(first_id);
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
            .comments(
                "面向阅读与 AI Agent 协作的原生 Markdown 知识库（Linux GTK4 + libadwaita 原生版）。",
            )
            .build();
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
    let menu = gio::Menu::new();

    let file_sec = gio::Menu::new();
    file_sec.append(Some("打开知识库…"), Some("win.open-vault"));
    file_sec.append(Some("新建窗口 (Ctrl+Shift+N)"), Some("win.new-window"));
    file_sec.append(Some("快速打开笔记 (Ctrl+P)"), Some("win.quick-open"));
    file_sec.append(Some("新建笔记 (Ctrl+N)"), Some("win.new-note"));
    file_sec.append(Some("新建文件夹"), Some("win.new-folder"));
    file_sec.append(Some("重命名当前笔记…"), Some("win.rename-active"));
    file_sec.append(Some("保存笔记 (Ctrl+S)"), Some("win.save-note"));
    file_sec.append(Some("复制整篇笔记纯文本"), Some("win.copy-note-text"));
    file_sec.append(Some("文内查找 (Ctrl+F)"), Some("win.find-in-note"));
    file_sec.append(Some("文内替换 (Ctrl+H)"), Some("win.replace-in-note"));
    file_sec.append(Some("全文搜索 (Ctrl+Shift+F)"), Some("win.focus-search"));
    file_sec.append(
        Some("切换阅读 / 编辑 (Ctrl+E)"),
        Some("win.toggle-edit-mode"),
    );
    file_sec.append(Some("关闭当前标签 (Ctrl+W)"), Some("win.close-tab"));
    menu.append_section(None, &file_sec);

    let agent_sec = gio::Menu::new();
    agent_sec.append(Some("启动 Agent"), Some("win.launch-agent"));
    agent_sec.append(
        Some("发送笔记路径到终端 (Ctrl+Shift+L)"),
        Some("win.send-path-to-terminal"),
    );
    agent_sec.append(
        Some("发送选区到终端 (Ctrl+Shift+K)"),
        Some("win.send-selection-to-terminal"),
    );
    agent_sec.append(Some("偏好设置… (Ctrl+,)"), Some("win.open-settings"));
    menu.append_section(None, &agent_sec);

    let view_sec = gio::Menu::new();
    view_sec.append(
        Some("标签全景网格 (Ctrl+Shift+O)"),
        Some("win.toggle-tab-overview"),
    );
    view_sec.append(
        Some("专注浏览图表 (Ctrl+Shift+D)"),
        Some("win.focus-diagram"),
    );
    view_sec.append(
        Some("切换左侧栏 (Ctrl+Shift+S)"),
        Some("win.toggle-left-sidebar"),
    );
    view_sec.append(
        Some("切换右侧栏 (Ctrl+Shift+I)"),
        Some("win.toggle-right-sidebar"),
    );
    view_sec.append(
        Some("切换底部面板 (Ctrl+J)"),
        Some("win.toggle-bottom-panel"),
    );
    view_sec.append(
        Some("显示 / 隐藏顶栏 (Ctrl+Shift+H)"),
        Some("win.toggle-top-bar"),
    );
    view_sec.append(Some("切换全屏 (F11)"), Some("win.toggle-fullscreen"));
    menu.append_section(None, &view_sec);

    let panel_sub = gio::Menu::new();
    panel_sub.append(
        Some("移动「反链」区域（右/底）"),
        Some("win.move-panel('backlinks')"),
    );
    panel_sub.append(
        Some("移动「改动」区域（右/底）"),
        Some("win.move-panel('changes')"),
    );
    panel_sub.append(
        Some("移动「终端」区域（右/底）"),
        Some("win.move-panel('terminal')"),
    );
    menu.append_submenu(Some("面板位置"), &panel_sub);

    let theme_sub = gio::Menu::new();
    theme_sub.append(Some("跟随系统"), Some("win.color-scheme('system')"));
    theme_sub.append(Some("浅色模式"), Some("win.color-scheme('light')"));
    theme_sub.append(Some("深色模式"), Some("win.color-scheme('dark')"));
    menu.append_submenu(Some("主题外观"), &theme_sub);

    let about_sec = gio::Menu::new();
    about_sec.append(Some("关于 Hortus"), Some("win.about"));
    menu.append_section(None, &about_sec);
    menu
}

pub fn build_left_switcher(stack: &gtk4::Stack) -> gtk4::Box {
    let switcher = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    switcher.add_css_class("hortus-sidebar-switcher");
    populate_stack_switcher(
        &switcher,
        stack,
        &[("files", "文件"), ("search", "搜索"), ("tags", "标签")],
    );
    switcher
}

pub fn populate_stack_switcher(container: &gtk4::Box, stack: &gtk4::Stack, items: &[(&str, &str)]) {
    let current = stack
        .visible_child_name()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut group_leader: Option<gtk4::ToggleButton> = None;
    for &(name, label) in items {
        let btn = gtk4::ToggleButton::with_label(label);
        btn.add_css_class("flat");
        btn.add_css_class("caption");
        btn.set_hexpand(true);
        btn.set_active(name == current);
        if let Some(ref leader) = group_leader {
            btn.set_group(Some(leader));
        } else {
            group_leader = Some(btn.clone());
        }
        let stack_weak = stack.downgrade();
        let name_owned = name.to_string();
        btn.connect_toggled(move |b| {
            if b.is_active()
                && let Some(stack_ref) = stack_weak.upgrade()
            {
                stack_ref.set_visible_child_name(&name_owned);
            }
        });
        container.append(&btn);
    }
}
