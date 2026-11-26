// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 原生设置对话框（`adw::PreferencesDialog`）：外观阅读、终端、Agent 多配置档、MCP 配置复制与指南及冲突保护。

use crate::core::ColorSchemeChoice;
use crate::i18n::{Key, fmt1, tr};
use crate::services::AppServices;
use crate::services::config_service::{FONT_SIZE_MAX, FONT_SIZE_MIN};
use crate::services::mcp_helper::mcp_config_json_for_vault;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    AgentGuideInfo, AgentProbeInfo, AgentProfileInfo, DiscoveredAgentInfo, EditableConfigInfo,
    EditableConfigInputInfo,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Appearance,
    Terminal,
    Agent,
}

impl SettingsPage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Terminal => "terminal",
            Self::Agent => "agent",
        }
    }
}

struct SettingsState {
    services: Arc<AppServices>,
    login_path: Option<String>,
    digest: String,
    probe_gen: u64,
    last_conflict: bool,
    on_toast: Rc<dyn Fn(String)>,
    on_scheme: Rc<dyn Fn(ColorSchemeChoice) -> Option<(String, String)>>,
    current_vault_root: Rc<dyn Fn() -> Option<String>>,
    suppress_probe: bool,
    suppress_scheme: bool,
    suppress_profile_sync: bool,
    guide_overridden: bool,
    discovered: Vec<Arc<DiscoveredAgentInfo>>,
    profiles: Vec<AgentProfileInfo>,
    default_profile: Option<String>,
    selected_profile: Option<usize>,
    last_probe: Option<Arc<AgentProbeInfo>>,
    conflict_dialog: glib::WeakRef<adw::AlertDialog>,
}

struct SettingsInner {
    dialog: adw::PreferencesDialog,
    appearance_page: adw::PreferencesPage,
    terminal_page: adw::PreferencesPage,
    agent_page: adw::PreferencesPage,
    font_spin: adw::SpinRow,
    default_view_combo: adw::ComboRow,
    scheme_combo: adw::ComboRow,
    config_path_row: adw::ActionRow,
    terminal_font_entry: adw::EntryRow,
    discovered_group: adw::PreferencesGroup,
    discovered_rows: RefCell<Vec<adw::ActionRow>>,
    profiles_group: adw::PreferencesGroup,
    profile_rows: RefCell<Vec<adw::ActionRow>>,
    profile_name_entry: adw::EntryRow,
    command_entry: adw::EntryRow,
    args_buffer: gtk4::TextBuffer,
    probe_badge: gtk4::Label,
    probe_detail: gtk4::Label,
    guide_status: gtk4::Label,
    guide_path_label: gtk4::Label,
    guide_buffer: gtk4::TextBuffer,
    save_status: gtk4::Label,
    state: RefCell<SettingsState>,
}

#[derive(Clone)]
pub struct SettingsDialog {
    inner: Rc<SettingsInner>,
}

impl SettingsDialog {
    pub fn new(
        services: Arc<AppServices>,
        login_path: Option<String>,
        initial_scheme: ColorSchemeChoice,
        on_toast: Rc<dyn Fn(String)>,
        on_scheme: Rc<dyn Fn(ColorSchemeChoice) -> Option<(String, String)>>,
        current_vault_root: Rc<dyn Fn() -> Option<String>>,
    ) -> Self {
        let dialog = adw::PreferencesDialog::builder()
            .title(tr(Key::SettingsTitle))
            .content_width(660)
            .content_height(640)
            .build();

        // 1. 外观与阅读页
        let appearance_page = adw::PreferencesPage::builder()
            .title(tr(Key::SettingsPageAppearance))
            .name(SettingsPage::Appearance.name())
            .icon_name("applications-graphics-symbolic")
            .build();

        let reading_group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(tr(Key::SettingsGroupReading)))
            .build();

        let font_adj = gtk4::Adjustment::new(15.0, FONT_SIZE_MIN, FONT_SIZE_MAX, 0.5, 1.0, 0.0);
        let font_spin = adw::SpinRow::builder()
            .title(tr(Key::SettingsFontSize))
            .adjustment(&font_adj)
            .digits(1)
            .build();

        let view_items =
            gtk4::StringList::new(&[tr(Key::SettingsViewReading), tr(Key::SettingsViewEditing)]);
        let default_view_combo = adw::ComboRow::builder()
            .title(tr(Key::SettingsDefaultView))
            .model(&view_items)
            .build();

        let scheme_items = gtk4::StringList::new(&[
            tr(Key::ThemeSystem),
            tr(Key::ThemeLight),
            tr(Key::ThemeDark),
        ]);
        let scheme_combo = adw::ComboRow::builder()
            .title(tr(Key::SettingsColorScheme))
            .model(&scheme_items)
            .selected(match initial_scheme {
                ColorSchemeChoice::System => 0,
                ColorSchemeChoice::Light => 1,
                ColorSchemeChoice::Dark => 2,
            })
            .build();

        reading_group.add(&font_spin);
        reading_group.add(&default_view_combo);
        reading_group.add(&scheme_combo);
        appearance_page.add(&reading_group);

        let config_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupConfig))
            .build();
        let config_path_row = adw::ActionRow::builder()
            .title(tr(Key::SettingsConfigPathTitle))
            .subtitle(tr(Key::SettingsLoading))
            .subtitle_selectable(true)
            .build();
        config_group.add(&config_path_row);
        let save_row_app = Self::build_save_row();
        config_group.add(&save_row_app.0);
        appearance_page.add(&config_group);

        // 2. 终端页
        let terminal_page = adw::PreferencesPage::builder()
            .title(tr(Key::SettingsPageTerminal))
            .name(SettingsPage::Terminal.name())
            .icon_name("utilities-terminal-symbolic")
            .build();

        let term_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupTerminal))
            .build();
        let terminal_font_entry = adw::EntryRow::builder()
            .title(tr(Key::SettingsTerminalFont))
            .build();
        term_group.add(&terminal_font_entry);
        let save_row_term = Self::build_save_row();
        term_group.add(&save_row_term.0);
        terminal_page.add(&term_group);

        // 3. Agent 与指南页
        let agent_page = adw::PreferencesPage::builder()
            .title(tr(Key::SettingsPageAgent))
            .name(SettingsPage::Agent.name())
            .icon_name("system-run-symbolic")
            .build();

        // MCP 配置一键复制入口
        let mcp_group = adw::PreferencesGroup::new();
        let mcp_row = adw::ActionRow::builder()
            .title(tr(Key::SettingsCopyMcpTitle))
            .build();
        let copy_mcp_btn = gtk4::Button::with_label(tr(Key::SettingsCopyMcpBtn));
        copy_mcp_btn.set_valign(gtk4::Align::Center);
        mcp_row.add_suffix(&copy_mcp_btn);
        mcp_row.set_activatable_widget(Some(&copy_mcp_btn));
        mcp_group.add(&mcp_row);
        agent_page.add(&mcp_group);

        // 多配置档列表
        let profiles_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupProfiles))
            .build();
        let add_profile_btn = gtk4::Button::with_label(tr(Key::SettingsProfileAddBtn));
        add_profile_btn.add_css_class("flat");
        profiles_group.set_header_suffix(Some(&add_profile_btn));
        agent_page.add(&profiles_group);

        // 启动命令编辑区（多配置档时编辑当前选中档；无配置档时兼容旧 `[agent]`）
        let agent_cfg_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupAgentLaunch))
            .build();

        let profile_name_entry = adw::EntryRow::builder()
            .title(tr(Key::SettingsProfileNameTitle))
            .visible(false)
            .build();
        agent_cfg_group.add(&profile_name_entry);

        let command_entry = adw::EntryRow::builder()
            .title(tr(Key::SettingsAgentCmdTitle))
            .build();
        agent_cfg_group.add(&command_entry);

        let args_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        args_box.set_margin_top(8);
        args_box.set_margin_bottom(8);
        args_box.set_margin_start(12);
        args_box.set_margin_end(12);
        let args_label = gtk4::Label::new(Some(tr(Key::SettingsAgentArgsLabel)));
        args_label.set_xalign(0.0);
        args_label.add_css_class("caption");
        args_label.add_css_class("dim-label");
        let args_buffer = gtk4::TextBuffer::new(None);
        let args_view = gtk4::TextView::builder()
            .buffer(&args_buffer)
            .monospace(true)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .height_request(76)
            .build();
        let args_frame = gtk4::Frame::new(None);
        let args_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&args_view)
            .min_content_height(76)
            .build();
        args_frame.set_child(Some(&args_scroll));
        args_box.append(&args_label);
        args_box.append(&args_frame);

        let probe_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let probe_badge = gtk4::Label::new(Some(tr(Key::SettingsProbeNotChecked)));
        probe_badge.add_css_class("hortus-agent-badge-err");
        let probe_detail = gtk4::Label::new(Some(tr(Key::SettingsProbePrompt)));
        probe_detail.add_css_class("caption");
        probe_detail.add_css_class("dim-label");
        probe_detail.set_xalign(0.0);
        probe_detail.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        probe_detail.set_hexpand(true);
        probe_row.append(&probe_badge);
        probe_row.append(&probe_detail);
        args_box.append(&probe_row);

        let args_row = adw::PreferencesRow::new();
        args_row.set_child(Some(&args_box));
        args_row.set_activatable(false);
        agent_cfg_group.add(&args_row);

        let save_row_agent = Self::build_save_row();
        agent_cfg_group.add(&save_row_agent.0);
        agent_page.add(&agent_cfg_group);

        let discovered_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupDetectedAgents))
            .build();
        let refresh_discover_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text(tr(Key::SettingsRescanAgentsTooltip))
            .build();
        refresh_discover_btn.add_css_class("flat");
        discovered_group.set_header_suffix(Some(&refresh_discover_btn));
        agent_page.add(&discovered_group);

        let guide_group = adw::PreferencesGroup::builder()
            .title(tr(Key::SettingsGroupGuide))
            .build();
        let guide_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        guide_box.set_margin_top(8);
        guide_box.set_margin_bottom(8);
        guide_box.set_margin_start(12);
        guide_box.set_margin_end(12);
        let guide_meta = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let guide_status = gtk4::Label::new(Some(tr(Key::SettingsGuideBuiltin)));
        guide_status.add_css_class("caption");
        guide_status.add_css_class("hortus-mode-badge");
        let guide_path_label = gtk4::Label::new(Some(""));
        guide_path_label.add_css_class("caption");
        guide_path_label.add_css_class("dim-label");
        guide_path_label.set_xalign(0.0);
        guide_path_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        guide_path_label.set_hexpand(true);
        guide_meta.append(&guide_status);
        guide_meta.append(&guide_path_label);

        let guide_buffer = gtk4::TextBuffer::new(None);
        let guide_view = gtk4::TextView::builder()
            .buffer(&guide_buffer)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .height_request(150)
            .build();
        let guide_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&guide_view)
            .min_content_height(150)
            .build();
        let guide_frame = gtk4::Frame::new(None);
        guide_frame.set_child(Some(&guide_scroll));

        let guide_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        guide_actions.set_halign(gtk4::Align::End);
        let guide_reset_btn = gtk4::Button::with_label(tr(Key::SettingsGuideResetBtn));
        let guide_save_btn = gtk4::Button::with_label(tr(Key::SettingsGuideSaveBtn));
        guide_save_btn.add_css_class("suggested-action");
        guide_actions.append(&guide_reset_btn);
        guide_actions.append(&guide_save_btn);

        guide_box.append(&guide_meta);
        guide_box.append(&guide_frame);
        guide_box.append(&guide_actions);
        let guide_row = adw::PreferencesRow::new();
        guide_row.set_child(Some(&guide_box));
        guide_row.set_activatable(false);
        guide_group.add(&guide_row);
        agent_page.add(&guide_group);

        let save_status = gtk4::Label::new(Some(""));
        save_status.add_css_class("caption");
        save_status.add_css_class("dim-label");
        save_status.set_xalign(0.0);
        let status_row = adw::PreferencesRow::new();
        status_row.set_child(Some(&save_status));
        status_row.set_activatable(false);
        config_group.add(&status_row);

        dialog.add(&appearance_page);
        dialog.add(&terminal_page);
        dialog.add(&agent_page);

        let inner = Rc::new(SettingsInner {
            dialog,
            appearance_page,
            terminal_page,
            agent_page,
            font_spin,
            default_view_combo,
            scheme_combo,
            config_path_row,
            terminal_font_entry,
            discovered_group,
            discovered_rows: RefCell::new(Vec::new()),
            profiles_group,
            profile_rows: RefCell::new(Vec::new()),
            profile_name_entry,
            command_entry,
            args_buffer,
            probe_badge,
            probe_detail,
            guide_status,
            guide_path_label,
            guide_buffer,
            save_status,
            state: RefCell::new(SettingsState {
                services,
                login_path,
                digest: String::new(),
                probe_gen: 0,
                last_conflict: false,
                on_toast,
                on_scheme,
                current_vault_root,
                suppress_probe: false,
                suppress_scheme: false,
                suppress_profile_sync: false,
                guide_overridden: false,
                discovered: Vec::new(),
                profiles: Vec::new(),
                default_profile: None,
                selected_profile: None,
                last_probe: None,
                conflict_dialog: glib::WeakRef::new(),
            }),
        });

        Self::bind_signals(
            &inner,
            &[save_row_app.1, save_row_term.1, save_row_agent.1],
            &copy_mcp_btn,
            &add_profile_btn,
            &refresh_discover_btn,
            &guide_save_btn,
            &guide_reset_btn,
        );
        Self::render_profiles(&inner);
        Self::reload_all(&inner);
        Self { inner }
    }

    fn build_save_row() -> (adw::ActionRow, gtk4::Button) {
        let row = adw::ActionRow::builder()
            .title(tr(Key::SettingsSaveRowTitle))
            .build();
        let btn = gtk4::Button::with_label(tr(Key::SettingsSaveBtn));
        btn.add_css_class("suggested-action");
        btn.set_valign(gtk4::Align::Center);
        row.add_suffix(&btn);
        row.set_activatable_widget(Some(&btn));
        (row, btn)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_signals(
        inner: &Rc<SettingsInner>,
        save_buttons: &[gtk4::Button],
        copy_mcp_btn: &gtk4::Button,
        add_profile_btn: &gtk4::Button,
        refresh_discover: &gtk4::Button,
        guide_save: &gtk4::Button,
        guide_reset: &gtk4::Button,
    ) {
        for btn in save_buttons {
            let weak = Rc::downgrade(inner);
            btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::save_config_impl(&this, false);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            copy_mcp_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::copy_mcp_config_impl(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            add_profile_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::add_profile_impl(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.scheme_combo.connect_selected_notify(move |combo| {
                let Some(this) = weak.upgrade() else { return };
                if this.state.borrow().suppress_scheme {
                    return;
                }
                let choice = match combo.selected() {
                    1 => ColorSchemeChoice::Light,
                    2 => ColorSchemeChoice::Dark,
                    _ => ColorSchemeChoice::System,
                };
                let cb = this.state.borrow().on_scheme.clone();
                let digests = cb(choice);
                Self::advance_digest_if_matching(
                    &this,
                    digests.as_ref().map(|(b, a)| (b.as_str(), a.as_str())),
                );
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.profile_name_entry.connect_changed(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::sync_selected_profile_from_fields(&this, true);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.command_entry.connect_changed(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::sync_selected_profile_from_fields(&this, true);
                    Self::probe_agent_async(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.args_buffer.connect_changed(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::sync_selected_profile_from_fields(&this, false);
                    Self::probe_agent_async(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            refresh_discover.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::discover_agents_async(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            guide_save.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::save_guide_impl(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            guide_reset.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::reset_guide_impl(&this);
                }
            });
        }
    }

    fn advance_digest_if_matching(inner: &Rc<SettingsInner>, digests: Option<(&str, &str)>) {
        if let Some((before, after)) = digests {
            let mut st = inner.state.borrow_mut();
            if st.digest == before {
                st.digest = after.to_string();
            }
        }
    }

    fn copy_mcp_config_impl(inner: &Rc<SettingsInner>) -> String {
        let vault_root = (inner.state.borrow().current_vault_root)();
        let json = mcp_config_json_for_vault(vault_root.as_deref());
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&json);
        }
        (inner.state.borrow().on_toast)(tr(Key::ToastCopiedMcpConfig).into());
        json
    }

    fn next_unique_profile_id(existing: &[AgentProfileInfo]) -> String {
        let mut n = existing.len() + 1;
        loop {
            let candidate = format!("profile-{n}");
            if !existing.iter().any(|p| p.id == candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    fn add_profile_impl(inner: &Rc<SettingsInner>) {
        Self::sync_selected_profile_from_fields(inner, false);
        let idx = {
            let mut st = inner.state.borrow_mut();
            // 首次从旧单 [agent] 配置升级为多配置档时，先自动保留当前命令与参数为默认配置档。
            if st.profiles.is_empty() {
                let raw_cmd = inner.command_entry.text().to_string();
                let trimmed_cmd = raw_cmd.trim();
                if !trimmed_cmd.is_empty() {
                    let first_name = std::path::Path::new(trimmed_cmd)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("Default")
                        .to_string();
                    let first_args = Self::collect_args(&inner.args_buffer);
                    st.profiles.push(AgentProfileInfo {
                        id: "default".to_string(),
                        name: first_name,
                        command: raw_cmd,
                        args: first_args,
                    });
                    st.default_profile = Some("default".to_string());
                }
            }
            let id = Self::next_unique_profile_id(&st.profiles);
            let name = fmt1(Key::SettingsProfileUntitled, st.profiles.len() + 1);
            let profile = AgentProfileInfo {
                id: id.clone(),
                name,
                command: String::new(),
                args: vec!["{guide}".to_string()],
            };
            st.profiles.push(profile);
            if st.default_profile.is_none() {
                st.default_profile = Some(id);
            }
            let idx = st.profiles.len() - 1;
            st.selected_profile = Some(idx);
            idx
        };
        Self::render_profiles(inner);
        Self::load_selected_profile_into_fields(inner, idx);
    }

    fn sync_selected_profile_from_fields(inner: &Rc<SettingsInner>, refresh_rows: bool) {
        let updated = {
            let mut st = inner.state.borrow_mut();
            if st.suppress_profile_sync {
                return;
            }
            let Some(idx) = st.selected_profile else {
                return;
            };
            let Some(profile) = st.profiles.get_mut(idx) else {
                return;
            };
            let new_name = inner.profile_name_entry.text().to_string();
            let new_cmd = inner.command_entry.text().to_string();
            let new_args = Self::collect_args(&inner.args_buffer);
            let meta_changed = profile.name != new_name || profile.command != new_cmd;
            profile.name = new_name;
            profile.command = new_cmd;
            profile.args = new_args;
            meta_changed
        };
        if refresh_rows && updated {
            Self::update_profile_row_labels(inner);
        }
    }

    fn update_profile_row_labels(inner: &Rc<SettingsInner>) {
        let rows = inner.profile_rows.borrow();
        let st = inner.state.borrow();
        for (idx, profile) in st.profiles.iter().enumerate() {
            if let Some(row) = rows.get(idx) {
                let is_default = st.default_profile.as_deref() == Some(profile.id.as_str());
                let title = if is_default {
                    format!(
                        "{} ({})",
                        profile.name,
                        tr(Key::SettingsProfileDefaultBadge)
                    )
                } else {
                    profile.name.clone()
                };
                row.set_title(&glib::markup_escape_text(&title));
                row.set_subtitle(&glib::markup_escape_text(&profile.command));
            }
        }
    }

    fn load_selected_profile_into_fields(inner: &Rc<SettingsInner>, idx: usize) {
        let profile = {
            let mut st = inner.state.borrow_mut();
            st.selected_profile = Some(idx);
            st.suppress_probe = true;
            st.suppress_profile_sync = true;
            st.profiles.get(idx).cloned()
        };
        if let Some(p) = profile {
            inner.profile_name_entry.set_visible(true);
            inner.profile_name_entry.set_text(&p.name);
            inner.command_entry.set_text(&p.command);
            inner.args_buffer.set_text(&p.args.join("\n"));
        }
        {
            let mut st = inner.state.borrow_mut();
            st.suppress_profile_sync = false;
            st.suppress_probe = false;
        }
        Self::probe_agent_async(inner);
    }

    fn render_profiles(inner: &Rc<SettingsInner>) {
        {
            let mut rows = inner.profile_rows.borrow_mut();
            for row in rows.drain(..) {
                inner.profiles_group.remove(&row);
            }
        }
        let (profiles, default_id, selected_idx) = {
            let st = inner.state.borrow();
            (
                st.profiles.clone(),
                st.default_profile.clone(),
                st.selected_profile,
            )
        };
        inner.profile_name_entry.set_visible(!profiles.is_empty());
        if profiles.is_empty() {
            let row = adw::ActionRow::builder()
                .title(tr(Key::SettingsProfileEmpty))
                .build();
            inner.profiles_group.add(&row);
            inner.profile_rows.borrow_mut().push(row);
            return;
        }
        for (idx, profile) in profiles.iter().enumerate() {
            let is_default = default_id.as_deref() == Some(profile.id.as_str());
            let title = if is_default {
                format!(
                    "{} ({})",
                    profile.name,
                    tr(Key::SettingsProfileDefaultBadge)
                )
            } else {
                profile.name.clone()
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle(glib::markup_escape_text(&profile.command))
                .activatable(true)
                .build();
            if selected_idx == Some(idx) {
                row.add_css_class("accent");
            }
            {
                let weak = Rc::downgrade(inner);
                row.connect_activated(move |_| {
                    if let Some(this) = weak.upgrade() {
                        Self::load_selected_profile_into_fields(&this, idx);
                        Self::render_profiles(&this);
                    }
                });
            }
            if !is_default {
                let default_btn = gtk4::Button::with_label(tr(Key::SettingsProfileSetDefaultBtn));
                default_btn.add_css_class("flat");
                default_btn.set_valign(gtk4::Align::Center);
                let weak = Rc::downgrade(inner);
                let pid = profile.id.clone();
                default_btn.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade() {
                        this.state.borrow_mut().default_profile = Some(pid.clone());
                        Self::render_profiles(&this);
                    }
                });
                row.add_suffix(&default_btn);
            }
            let del_btn = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr(Key::SettingsProfileDeleteBtn))
                .valign(gtk4::Align::Center)
                .build();
            del_btn.add_css_class("flat");
            {
                let weak = Rc::downgrade(inner);
                let pid = profile.id.clone();
                del_btn.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade() {
                        Self::delete_profile_impl(&this, &pid);
                    }
                });
            }
            row.add_suffix(&del_btn);
            inner.profiles_group.add(&row);
            inner.profile_rows.borrow_mut().push(row);
        }
    }

    fn delete_profile_impl(inner: &Rc<SettingsInner>, profile_id: &str) {
        let next_sel = {
            let mut st = inner.state.borrow_mut();
            st.profiles.retain(|p| p.id != profile_id);
            if st.default_profile.as_deref() == Some(profile_id) {
                st.default_profile = st.profiles.first().map(|p| p.id.clone());
            }
            if st.profiles.is_empty() {
                st.selected_profile = None;
                None
            } else {
                let idx = st
                    .selected_profile
                    .unwrap_or(0)
                    .min(st.profiles.len().saturating_sub(1));
                st.selected_profile = Some(idx);
                Some(idx)
            }
        };
        Self::render_profiles(inner);
        if let Some(idx) = next_sel {
            Self::load_selected_profile_into_fields(inner, idx);
        } else {
            let mut st = inner.state.borrow_mut();
            st.suppress_probe = true;
            st.suppress_profile_sync = true;
            drop(st);
            inner.profile_name_entry.set_text("");
            inner.command_entry.set_text("");
            inner.args_buffer.set_text("");
            let mut st = inner.state.borrow_mut();
            st.suppress_profile_sync = false;
            st.suppress_probe = false;
            drop(st);
            Self::probe_agent_async(inner);
        }
    }

    pub fn present(&self, parent: &impl IsA<gtk4::Widget>, page: Option<SettingsPage>) {
        if let Some(page) = page {
            self.select_page(page);
        }
        self.inner.dialog.present(Some(parent));
    }

    pub fn same_instance(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn connect_closed(&self, cb: impl Fn(&Self) + 'static) {
        let weak = Rc::downgrade(&self.inner);
        self.inner.dialog.connect_closed(move |_| {
            if let Some(inner) = weak.upgrade() {
                cb(&Self { inner });
            }
        });
    }

    pub fn sync_color_scheme_with_digests(
        &self,
        choice: ColorSchemeChoice,
        digests: Option<(&str, &str)>,
    ) {
        Self::advance_digest_if_matching(&self.inner, digests);
        let target = match choice {
            ColorSchemeChoice::System => 0,
            ColorSchemeChoice::Light => 1,
            ColorSchemeChoice::Dark => 2,
        };
        if self.inner.scheme_combo.selected() == target {
            return;
        }
        self.inner.state.borrow_mut().suppress_scheme = true;
        self.inner.scheme_combo.set_selected(target);
        self.inner.state.borrow_mut().suppress_scheme = false;
    }

    #[cfg(any(test, feature = "automation"))]
    pub fn select_color_scheme(&self, choice: ColorSchemeChoice) {
        let target = match choice {
            ColorSchemeChoice::System => 0,
            ColorSchemeChoice::Light => 1,
            ColorSchemeChoice::Dark => 2,
        };
        self.inner.scheme_combo.set_selected(target);
    }

    #[cfg(feature = "automation")]
    pub fn close(&self) {
        self.dismiss_conflict_dialog();
        let _ = self.inner.dialog.close();
    }

    pub fn force_close(&self) {
        self.dismiss_conflict_dialog();
        self.inner.dialog.force_close();
    }

    fn dismiss_conflict_dialog(&self) {
        let alert = {
            let mut st = self.inner.state.borrow_mut();
            st.last_conflict = false;
            let a = st.conflict_dialog.upgrade();
            st.conflict_dialog.set(None);
            a
        };
        if let Some(alert) = alert {
            alert.force_close();
        }
    }

    pub fn select_page(&self, page: SettingsPage) {
        let target = match page {
            SettingsPage::Appearance => &self.inner.appearance_page,
            SettingsPage::Terminal => &self.inner.terminal_page,
            SettingsPage::Agent => &self.inner.agent_page,
        };
        self.inner.dialog.set_visible_page(target);
    }

    fn reload_all(inner: &Rc<SettingsInner>) {
        Self::reload_config_async(inner);
        Self::reload_guide_async(inner);
        Self::discover_agents_async(inner);
    }

    fn reload_config_async(inner: &Rc<SettingsInner>) {
        let weak = Rc::downgrade(inner);
        let core = Arc::clone(&inner.state.borrow().services.core);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || core.editable_config())
                .await
                .unwrap_or_else(|_| {
                    Err(hortus_ffi::CoreError::Failure {
                        kind: "settings.thread_error".into(),
                    })
                });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(info) => Self::apply_editable_config(&this, &info),
                Err(err) => this
                    .config_path_row
                    .set_subtitle(&fmt1(Key::SettingsReadFailed, err)),
            }
        });
    }

    fn apply_editable_config(inner: &Rc<SettingsInner>, info: &EditableConfigInfo) {
        let selected_idx = {
            let mut st = inner.state.borrow_mut();
            st.digest = info.digest.clone();
            st.suppress_probe = true;
            st.suppress_profile_sync = true;
            st.profiles = info.agent_profiles.clone();
            st.default_profile = info.agent_default_profile.clone();
            let sel = if st.profiles.is_empty() {
                None
            } else {
                let def_idx = st
                    .default_profile
                    .as_deref()
                    .and_then(|id| st.profiles.iter().position(|p| p.id == id))
                    .unwrap_or(0);
                Some(def_idx)
            };
            st.selected_profile = sel;
            sel
        };
        inner.config_path_row.set_subtitle(&info.config_path);
        inner
            .font_spin
            .set_value(info.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX));
        inner
            .default_view_combo
            .set_selected(if info.default_view == "editing" { 1 } else { 0 });
        inner
            .terminal_font_entry
            .set_text(info.terminal_font.as_deref().unwrap_or(""));
        Self::render_profiles(inner);
        if let Some(idx) = selected_idx {
            let p = inner.state.borrow().profiles[idx].clone();
            inner.profile_name_entry.set_visible(true);
            inner.profile_name_entry.set_text(&p.name);
            inner.command_entry.set_text(&p.command);
            inner.args_buffer.set_text(&p.args.join("\n"));
        } else {
            inner.profile_name_entry.set_visible(false);
            inner.profile_name_entry.set_text("");
            inner
                .command_entry
                .set_text(info.agent_command.as_deref().unwrap_or(""));
            inner.args_buffer.set_text(&info.agent_args.join("\n"));
        }
        {
            let mut st = inner.state.borrow_mut();
            st.suppress_profile_sync = false;
            st.suppress_probe = false;
        }
        Self::probe_agent_async(inner);
    }

    fn discover_agents_async(inner: &Rc<SettingsInner>) {
        let weak = Rc::downgrade(inner);
        let (core, login_path) = {
            let st = inner.state.borrow();
            (Arc::clone(&st.services.core), st.login_path.clone())
        };
        glib::MainContext::default().spawn_local(async move {
            let agents = gio::spawn_blocking(move || core.discover_agents(login_path))
                .await
                .unwrap_or_default();
            let Some(this) = weak.upgrade() else { return };
            Self::render_discovered_agents(&this, agents);
        });
    }

    fn render_discovered_agents(inner: &Rc<SettingsInner>, agents: Vec<DiscoveredAgentInfo>) {
        {
            let mut rows = inner.discovered_rows.borrow_mut();
            for row in rows.drain(..) {
                inner.discovered_group.remove(&row);
            }
        }
        inner.state.borrow_mut().discovered = agents.into_iter().map(Arc::new).collect();
        let stored = inner.state.borrow().discovered.clone();
        if stored.is_empty() {
            let row = adw::ActionRow::builder()
                .title(tr(Key::SettingsNoAgentsFound))
                .subtitle(tr(Key::SettingsNoAgentsSubtitle))
                .build();
            inner.discovered_group.add(&row);
            inner.discovered_rows.borrow_mut().push(row);
            return;
        }
        for agent in stored {
            let row = adw::ActionRow::builder()
                .title(&agent.display_name)
                .subtitle(&agent.executable)
                .build();
            let fill_btn = gtk4::Button::with_label(tr(Key::SettingsFillBtn));
            fill_btn.set_valign(gtk4::Align::Center);
            let weak = Rc::downgrade(inner);
            let display_name = agent.display_name.clone();
            let exec = agent.executable.clone();
            let args = agent.suggested_args.clone();
            fill_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::fill_command_and_args(&this, &display_name, &exec, &args);
                }
            });
            row.add_suffix(&fill_btn);
            row.set_activatable_widget(Some(&fill_btn));
            inner.discovered_group.add(&row);
            inner.discovered_rows.borrow_mut().push(row);
        }
    }

    fn fill_command_and_args(
        inner: &Rc<SettingsInner>,
        display_name: &str,
        exec: &str,
        args: &[String],
    ) {
        let has_selected_profile = inner.state.borrow().selected_profile.is_some();
        inner.state.borrow_mut().suppress_probe = true;
        if has_selected_profile && inner.profile_name_entry.text().trim().is_empty() {
            inner.profile_name_entry.set_text(display_name);
        }
        inner.command_entry.set_text(exec);
        inner.args_buffer.set_text(&args.join("\n"));
        inner.state.borrow_mut().suppress_probe = false;
        Self::sync_selected_profile_from_fields(inner, true);
        Self::probe_agent_async(inner);
    }

    fn collect_args(buffer: &gtk4::TextBuffer) -> Vec<String> {
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        text.lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn probe_agent_async(inner: &Rc<SettingsInner>) {
        if inner.state.borrow().suppress_probe {
            return;
        }
        let (core, login_path, p_gen) = {
            let mut st = inner.state.borrow_mut();
            st.probe_gen = st.probe_gen.wrapping_add(1);
            (
                Arc::clone(&st.services.core),
                st.login_path.clone(),
                st.probe_gen,
            )
        };
        let command = inner.command_entry.text().to_string();
        let args = Self::collect_args(&inner.args_buffer);
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let status = gio::spawn_blocking(move || {
                core.probe_agent_launch_with_path(command, args, login_path)
            })
            .await
            .ok();
            let (Some(this), Some(status)) = (weak.upgrade(), status) else {
                return;
            };
            if this.state.borrow().probe_gen != p_gen {
                return;
            }
            let status_arc = Arc::new(status);
            this.state.borrow_mut().last_probe = Some(Arc::clone(&status_arc));
            if status_arc.available {
                this.probe_badge.set_label(tr(Key::SettingsProbeOk));
                this.probe_badge.remove_css_class("hortus-agent-badge-err");
                this.probe_badge.add_css_class("hortus-agent-badge-ok");
                this.probe_detail.set_label(
                    status_arc
                        .executable
                        .as_deref()
                        .unwrap_or(tr(Key::SettingsProbeResolved)),
                );
            } else {
                this.probe_badge.set_label(tr(Key::SettingsProbeErr));
                this.probe_badge.remove_css_class("hortus-agent-badge-ok");
                this.probe_badge.add_css_class("hortus-agent-badge-err");
                let msg = match status_arc.error.as_deref() {
                    Some("agent.command_missing") => tr(Key::SettingsProbeMissingCmd),
                    Some("agent.guide_missing") => tr(Key::SettingsProbeMissingGuide),
                    Some("agent.command_not_absolute") => tr(Key::SettingsProbeNotAbs),
                    Some("agent.not_found") => tr(Key::SettingsProbeNotFound),
                    Some("agent.not_executable") => tr(Key::SettingsProbeNotExec),
                    Some(other) => other,
                    None => tr(Key::SettingsProbeInvalid),
                };
                this.probe_detail.set_label(msg);
            }
        });
    }

    fn reload_guide_async(inner: &Rc<SettingsInner>) {
        let weak = Rc::downgrade(inner);
        let core = Arc::clone(&inner.state.borrow().services.core);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || core.agent_guide_state())
                .await
                .ok()
                .and_then(Result::ok);
            let (Some(this), Some(info)) = (weak.upgrade(), res) else {
                return;
            };
            Self::apply_guide_info(&this, &info);
        });
    }

    fn apply_guide_info(inner: &Rc<SettingsInner>, info: &AgentGuideInfo) {
        inner.state.borrow_mut().guide_overridden = info.overridden;
        inner.guide_buffer.set_text(&info.text);
        inner.guide_status.set_label(if info.overridden {
            tr(Key::SettingsGuideCustom)
        } else {
            tr(Key::SettingsGuideBuiltin)
        });
        inner.guide_path_label.set_label(&info.path);
    }

    fn save_guide_impl(inner: &Rc<SettingsInner>) {
        let text = inner
            .guide_buffer
            .text(
                &inner.guide_buffer.start_iter(),
                &inner.guide_buffer.end_iter(),
                false,
            )
            .to_string();
        let weak = Rc::downgrade(inner);
        let core = Arc::clone(&inner.state.borrow().services.core);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || core.save_agent_guide(text))
                .await
                .unwrap_or_else(|_| {
                    Err(hortus_ffi::CoreError::Failure {
                        kind: "guide.thread_error".into(),
                    })
                });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(info) => {
                    Self::apply_guide_info(&this, &info);
                    (this.state.borrow().on_toast)(tr(Key::SettingsGuideSavedToast).into());
                }
                Err(err) => {
                    (this.state.borrow().on_toast)(fmt1(Key::SettingsGuideSaveFailedToast, err));
                }
            }
        });
    }

    fn reset_guide_impl(inner: &Rc<SettingsInner>) {
        let weak = Rc::downgrade(inner);
        let core = Arc::clone(&inner.state.borrow().services.core);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || core.reset_agent_guide())
                .await
                .unwrap_or_else(|_| {
                    Err(hortus_ffi::CoreError::Failure {
                        kind: "guide.thread_error".into(),
                    })
                });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(info) => {
                    Self::apply_guide_info(&this, &info);
                    (this.state.borrow().on_toast)(tr(Key::SettingsGuideResetToast).into());
                }
                Err(err) => {
                    (this.state.borrow().on_toast)(fmt1(Key::SettingsGuideResetFailedToast, err));
                }
            }
        });
    }

    fn build_input(
        inner: &Rc<SettingsInner>,
    ) -> (
        EditableConfigInputInfo,
        Vec<AgentProfileInfo>,
        Option<String>,
    ) {
        Self::sync_selected_profile_from_fields(inner, false);
        let font_size =
            ((inner.font_spin.value() * 2.0).round() / 2.0).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        let default_view = if inner.default_view_combo.selected() == 1 {
            "editing".to_string()
        } else {
            "reading".to_string()
        };
        let terminal_font = {
            let raw = inner.terminal_font_entry.text().trim().to_string();
            if raw.is_empty() { None } else { Some(raw) }
        };
        let (profiles, default_profile) = {
            let st = inner.state.borrow();
            (st.profiles.clone(), st.default_profile.clone())
        };
        let (agent_command, agent_args) = if profiles.is_empty() {
            let cmd = {
                let raw = inner.command_entry.text().trim().to_string();
                if raw.is_empty() { None } else { Some(raw) }
            };
            (cmd, Self::collect_args(&inner.args_buffer))
        } else {
            (None, Vec::new())
        };
        (
            EditableConfigInputInfo {
                font_size,
                default_view,
                terminal_font,
                agent_command,
                agent_args,
            },
            profiles,
            default_profile,
        )
    }

    fn save_config_impl(inner: &Rc<SettingsInner>, force_overwrite: bool) {
        let (input, profiles, default_profile) = Self::build_input(inner);
        let (services, expected_digest) = {
            let st = inner.state.borrow();
            (
                Arc::clone(&st.services),
                if force_overwrite {
                    None
                } else {
                    Some(st.digest.clone())
                },
            )
        };
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let svc = Arc::clone(&services);
            let res = gio::spawn_blocking(move || {
                svc.core.save_editable_config_with_profiles(
                    input,
                    profiles,
                    default_profile,
                    expected_digest,
                )
            })
            .await
            .unwrap_or_else(|_| {
                Err(hortus_ffi::CoreError::Failure {
                    kind: "config.thread_error".into(),
                })
            });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(saved) => {
                    let alert = {
                        let mut st = this.state.borrow_mut();
                        st.last_conflict = false;
                        let a = st.conflict_dialog.upgrade();
                        st.conflict_dialog.set(None);
                        a
                    };
                    if let Some(alert) = alert {
                        alert.force_close();
                    }
                    Self::apply_editable_config(&this, &saved);
                    this.save_status.set_label(tr(Key::SettingsSavedStatus));
                    services.config.apply_saved(&saved);
                    (this.state.borrow().on_toast)(tr(Key::SettingsSavedToast).into());
                }
                Err(err) if err.to_string() == "config.externally_modified" => {
                    this.state.borrow_mut().last_conflict = true;
                    this.save_status.set_label(tr(Key::SettingsConflictStatus));
                    Self::prompt_external_conflict(&this);
                }
                Err(err) => {
                    this.save_status
                        .set_label(&fmt1(Key::SettingsSaveFailedStatus, &err));
                    (this.state.borrow().on_toast)(fmt1(Key::SettingsSaveFailedToast, err));
                }
            }
        });
    }

    fn prompt_external_conflict(inner: &Rc<SettingsInner>) {
        let alert = adw::AlertDialog::builder()
            .heading(tr(Key::SettingsConflictHeading))
            .body(tr(Key::SettingsConflictBody))
            .build();
        alert.add_response("reload", tr(Key::SettingsConflictReloadBtn));
        alert.add_response("overwrite", tr(Key::SettingsConflictOverwriteBtn));
        alert.set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("reload"));
        alert.set_close_response("reload");
        let weak = Rc::downgrade(inner);
        alert.connect_response(None, move |_, response| {
            let Some(this) = weak.upgrade() else { return };
            let still_active = {
                let st = this.state.borrow();
                st.conflict_dialog.set(None);
                st.last_conflict
            };
            if !still_active {
                return;
            }
            if response == "overwrite" {
                Self::save_config_impl(&this, true);
            } else {
                this.state.borrow_mut().last_conflict = false;
                Self::reload_config_async(&this);
                this.save_status.set_label(tr(Key::SettingsReloadedStatus));
            }
        });
        inner.state.borrow().conflict_dialog.set(Some(&alert));
        alert.present(Some(&inner.dialog));
    }
}

#[cfg(feature = "automation")]
impl SettingsDialog {
    pub fn set_font_size(&self, size: f64) {
        self.inner.font_spin.set_value(size);
    }

    pub fn set_terminal_font(&self, font: &str) {
        self.inner.terminal_font_entry.set_text(font);
    }

    pub fn set_default_view(&self, editing: bool) {
        self.inner
            .default_view_combo
            .set_selected(if editing { 1 } else { 0 });
    }

    pub fn set_agent(&self, command: &str, args: &[String]) {
        self.inner.command_entry.set_text(command);
        self.inner.args_buffer.set_text(&args.join("\n"));
    }

    pub fn set_profiles(&self, profiles: Vec<AgentProfileInfo>, default_profile: Option<String>) {
        let sel = if profiles.is_empty() {
            None
        } else {
            let def_idx = default_profile
                .as_deref()
                .and_then(|id| profiles.iter().position(|p| p.id == id))
                .unwrap_or(0);
            Some(def_idx)
        };
        {
            let mut st = self.inner.state.borrow_mut();
            st.profiles = profiles;
            st.default_profile = default_profile;
            st.selected_profile = sel;
        }
        SettingsDialog::render_profiles(&self.inner);
        if let Some(idx) = sel {
            SettingsDialog::load_selected_profile_into_fields(&self.inner, idx);
        }
    }

    pub fn copy_mcp_config_now(&self) -> String {
        SettingsDialog::copy_mcp_config_impl(&self.inner)
    }

    pub fn fill_discovered_agent(&self, id: &str) -> bool {
        let found = self
            .inner
            .state
            .borrow()
            .discovered
            .iter()
            .find(|a| a.id == id || a.binary_name == id)
            .cloned();
        let Some(agent) = found else {
            return false;
        };
        SettingsDialog::fill_command_and_args(
            &self.inner,
            &agent.display_name,
            &agent.executable,
            &agent.suggested_args,
        );
        true
    }

    pub fn discovered_ids(&self) -> Vec<String> {
        self.inner
            .state
            .borrow()
            .discovered
            .iter()
            .map(|a| a.id.clone())
            .collect()
    }

    pub fn last_probe(&self) -> Option<Arc<AgentProbeInfo>> {
        self.inner.state.borrow().last_probe.clone()
    }

    pub fn save_config_now(&self, force_overwrite: bool) {
        SettingsDialog::save_config_impl(&self.inner, force_overwrite);
    }

    pub fn last_conflict(&self) -> bool {
        self.inner.state.borrow().last_conflict
    }

    pub fn set_guide_text(&self, text: &str) {
        self.inner.guide_buffer.set_text(text);
    }

    pub fn guide_text(&self) -> String {
        self.inner
            .guide_buffer
            .text(
                &self.inner.guide_buffer.start_iter(),
                &self.inner.guide_buffer.end_iter(),
                false,
            )
            .to_string()
    }

    pub fn guide_overridden(&self) -> bool {
        self.inner.state.borrow().guide_overridden
    }

    pub fn save_guide_now(&self) {
        SettingsDialog::save_guide_impl(&self.inner);
    }

    pub fn reset_guide_now(&self) {
        SettingsDialog::reset_guide_impl(&self.inner);
    }

    pub fn config_path_subtitle(&self) -> String {
        self.inner
            .config_path_row
            .subtitle()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::config_service::ConfigService;
    use crate::services::vault_registry::VaultRegistry;
    use crate::services::window_registry::WindowRegistry;
    use hortus_ffi::WorkspaceCore;

    #[test]
    fn settings_dialog_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .unwrap();
        let windows = Arc::new(WindowRegistry::new());
        let config = ConfigService::new(Arc::clone(&core), Arc::clone(&windows));
        let services = Arc::new(AppServices {
            core,
            diagram_engine: hortus_ffi::DiagramEngine::new().unwrap(),
            math_service: Arc::new(crate::render::math::MathRenderService::new()),
            vaults: Arc::new(VaultRegistry::new()),
            windows,
            config,
        });
        let dlg = SettingsDialog::new(
            services,
            None,
            ColorSchemeChoice::System,
            Rc::new(|_| {}),
            Rc::new(|_| None),
            Rc::new(|| None),
        );
        let weak = Rc::downgrade(&dlg.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(dlg);
        assert_eq!(weak.strong_count(), 0);
    }

    fn make_test_services(tmp: &tempfile::TempDir) -> Arc<AppServices> {
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .unwrap();
        let windows = Arc::new(WindowRegistry::new());
        let config = ConfigService::new(Arc::clone(&core), Arc::clone(&windows));
        Arc::new(AppServices {
            core,
            diagram_engine: hortus_ffi::DiagramEngine::new().unwrap(),
            math_service: Arc::new(crate::render::math::MathRenderService::new()),
            vaults: Arc::new(VaultRegistry::new()),
            windows,
            config,
        })
    }

    fn pump_until(timeout_ms: u64, cond: impl Fn() -> bool) -> bool {
        let ctx = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            while ctx.iteration(false) {}
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        while ctx.iteration(false) {}
        cond()
    }

    #[test]
    fn add_profile_migrates_legacy_agent_config_without_data_loss() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let services = make_test_services(&tmp);
        let cfg_path =
            std::path::PathBuf::from(services.core.editable_config().unwrap().config_path);
        std::fs::write(
            &cfg_path,
            "font_size = 15.0\ndefault_view = \"reading\"\n\n[agent]\ncommand = \"/usr/local/bin/claude\"\nargs = [\"--print\", \"{guide}\"]\n",
        )
        .unwrap();

        let dlg = SettingsDialog::new(
            Arc::clone(&services),
            None,
            ColorSchemeChoice::System,
            Rc::new(|_| {}),
            Rc::new(|_| None),
            Rc::new(|| None),
        );
        assert!(
            pump_until(2000, || !dlg.inner.state.borrow().digest.is_empty()),
            "settings dialog should load initial config"
        );
        assert_eq!(dlg.inner.command_entry.text(), "/usr/local/bin/claude");
        assert!(dlg.inner.state.borrow().profiles.is_empty());

        SettingsDialog::add_profile_impl(&dlg.inner);

        {
            let st = dlg.inner.state.borrow();
            assert_eq!(st.profiles.len(), 2);
            assert_eq!(st.profiles[0].id, "default");
            assert_eq!(st.profiles[0].name, "claude");
            assert_eq!(st.profiles[0].command, "/usr/local/bin/claude");
            assert_eq!(st.profiles[0].args, vec!["--print", "{guide}"]);
            assert_eq!(st.default_profile.as_deref(), Some("default"));
            assert_eq!(st.selected_profile, Some(1));
        }

        // 填写第二个配置档并保存，验证原默认配置档完整保留
        dlg.inner.profile_name_entry.set_text("Codex");
        dlg.inner.command_entry.set_text("codex");
        dlg.inner.args_buffer.set_text("{guide}");
        SettingsDialog::save_config_impl(&dlg.inner, false);
        assert!(
            pump_until(2500, || {
                services
                    .core
                    .editable_config()
                    .is_ok_and(|c| c.agent_profiles.len() == 2)
            }),
            "saving migrated profiles should persist both profiles"
        );
        let saved = services.core.editable_config().unwrap();
        assert_eq!(saved.agent_default_profile.as_deref(), Some("default"));
        assert_eq!(saved.agent_profiles[0].id, "default");
        assert_eq!(saved.agent_profiles[0].command, "/usr/local/bin/claude");
        assert_eq!(saved.agent_profiles[0].args, vec!["--print", "{guide}"]);
        assert_eq!(saved.agent_profiles[1].command, "codex");
    }

    #[test]
    fn color_scheme_switch_preserves_external_conflict_and_allows_clean_saves() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let services = make_test_services(&tmp);
        let core_scheme = Arc::clone(&services.core);
        let dlg = SettingsDialog::new(
            Arc::clone(&services),
            None,
            ColorSchemeChoice::System,
            Rc::new(|_| {}),
            Rc::new(move |choice| {
                crate::services::ui_prefs::save_color_scheme_with_digests(&core_scheme, choice).ok()
            }),
            Rc::new(|| None),
        );
        assert!(
            pump_until(2000, || !dlg.inner.state.borrow().digest.is_empty()),
            "settings dialog should load initial config"
        );

        // (c) 无外部改动时连续切换主题两次再点保存，应正常保存且无冲突
        dlg.select_color_scheme(ColorSchemeChoice::Dark);
        dlg.select_color_scheme(ColorSchemeChoice::Light);
        dlg.inner.font_spin.set_value(17.0);
        SettingsDialog::save_config_impl(&dlg.inner, false);
        assert!(
            pump_until(2500, || {
                services
                    .core
                    .editable_config()
                    .is_ok_and(|c| (c.font_size - 17.0).abs() < 0.01)
            }),
            "clean save after repeated scheme switches should succeed"
        );
        assert!(!dlg.inner.state.borrow().last_conflict);
        assert_eq!(services.core.color_scheme().unwrap(), "light");

        // (b) 外部修改 config.toml 为配置 B -> 窗内切主题 + 主菜单切主题 -> 点保存，必须触发冲突并保留配置 B
        let cfg_path =
            std::path::PathBuf::from(services.core.editable_config().unwrap().config_path);
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let external_b = raw.replace("font_size = 17", "font_size = 19");
        assert_ne!(raw, external_b);
        std::fs::write(&cfg_path, &external_b).unwrap();

        // 1) 窗内切主题
        dlg.select_color_scheme(ColorSchemeChoice::Dark);
        // 2) 模拟主菜单切主题
        let menu_digests = crate::services::ui_prefs::save_color_scheme_with_digests(
            &services.core,
            ColorSchemeChoice::System,
        )
        .ok();
        dlg.sync_color_scheme_with_digests(
            ColorSchemeChoice::System,
            menu_digests.as_ref().map(|(b, a)| (b.as_str(), a.as_str())),
        );

        dlg.inner.font_spin.set_value(15.0);
        SettingsDialog::save_config_impl(&dlg.inner, false);
        assert!(
            pump_until(2500, || dlg.inner.state.borrow().last_conflict),
            "external modification before scheme switch must still trigger conflict"
        );
        let on_disk = services.core.editable_config().unwrap();
        assert!(
            (on_disk.font_size - 19.0).abs() < 0.01,
            "external config B must remain intact on disk, got {}",
            on_disk.font_size
        );
        dlg.force_close();
    }
}
