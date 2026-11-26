// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 窗口主菜单、主题切换、关于对话框与关闭确认弹窗。

use crate::core::{APP_ID, APP_VERSION, ColorSchemeChoice, PanelRegion, StaticToolKind};
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

impl WindowInner {
    pub fn show_about_dialog(&self) {
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

    pub fn ask_unsaved_close(self: &Rc<Self>, page: &adw::TabPage, path: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(crate::i18n::tr(crate::i18n::Key::UnsavedHeading))
            .body(crate::i18n::fmt1(
                crate::i18n::Key::UnsavedTabCloseBody,
                path,
            ))
            .build();
        dialog.add_response("cancel", crate::i18n::tr(crate::i18n::Key::Cancel));
        dialog.add_response(
            "close",
            crate::i18n::tr(crate::i18n::Key::BtnDiscardAndClose),
        );
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        {
            let mut st = self.state.borrow_mut();
            st.last_alert = Some(dialog.heading().unwrap_or_default().to_string());
            st.last_alert_dialog.set(Some(&dialog));
        }
        let weak = Rc::downgrade(self);
        let page = page.clone();
        dialog.connect_response(None, move |_, response| {
            if let Some(win) = weak.upgrade() {
                win.state.borrow_mut().last_alert_dialog.set(None);
                if response == "close" {
                    win.tab_view.close_page_finish(&page, true);
                } else {
                    win.tab_view.close_page_finish(&page, false);
                }
            }
        });
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

pub fn current_color_scheme() -> ColorSchemeChoice {
    match adw::StyleManager::default().color_scheme() {
        adw::ColorScheme::PreferLight | adw::ColorScheme::ForceLight => ColorSchemeChoice::Light,
        adw::ColorScheme::PreferDark | adw::ColorScheme::ForceDark => ColorSchemeChoice::Dark,
        _ => ColorSchemeChoice::System,
    }
}

pub fn build_primary_menu(
    profiles: &[crate::services::config_service::AgentProfileSummary],
) -> gio::Menu {
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
    if profiles.len() > 1 {
        for profile in profiles {
            let item = gio::MenuItem::new(
                Some(&fmt1(Key::MenuLaunchAgentProfile, &profile.name)),
                None,
            );
            item.set_action_and_target_value(
                Some("win.launch-agent-profile"),
                Some(&profile.id.to_variant()),
            );
            agent_sec.append_item(&item);
        }
    }
    agent_sec.append(
        Some(tr(Key::MenuCustomAgentCommand)),
        Some("win.launch-custom-agent"),
    );
    agent_sec.append(
        Some(tr(Key::MenuSendPathToTerminal)),
        Some("win.send-path-to-terminal"),
    );
    agent_sec.append(
        Some(tr(Key::MenuSendSelectionToTerminal)),
        Some("win.send-selection-to-terminal"),
    );
    agent_sec.append(Some(tr(Key::MenuSettings)), Some("win.open-settings"));
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
