// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 原生设置对话框（`adw::PreferencesDialog`）：外观阅读、终端、Agent 与指南及外部修改冲突保护。

use crate::core::ColorSchemeChoice;
use crate::services::AppServices;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    AgentGuideInfo, AgentProbeInfo, DiscoveredAgentInfo, EditableConfigInfo,
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
    on_scheme: Rc<dyn Fn(ColorSchemeChoice)>,
    suppress_probe: bool,
    discovered: Vec<Arc<DiscoveredAgentInfo>>,
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
        on_scheme: Rc<dyn Fn(ColorSchemeChoice)>,
    ) -> Self {
        let dialog = adw::PreferencesDialog::builder()
            .title("偏好设置")
            .content_width(660)
            .content_height(620)
            .build();

        // 1. 外观与阅读页
        let appearance_page = adw::PreferencesPage::builder()
            .title("外观与阅读")
            .name(SettingsPage::Appearance.name())
            .icon_name("applications-graphics-symbolic")
            .build();

        let reading_group = adw::PreferencesGroup::builder()
            .title("阅读与排版")
            .description("修改会同步至全部已打开窗口。")
            .build();

        let font_adj = gtk4::Adjustment::new(15.0, 12.0, 24.0, 0.5, 1.0, 0.0);
        let font_spin = adw::SpinRow::builder()
            .title("正文字号")
            .subtitle("范围 12.0–24.0 pt")
            .adjustment(&font_adj)
            .digits(1)
            .build();

        let view_items = gtk4::StringList::new(&["阅读视图", "源码编辑"]);
        let default_view_combo = adw::ComboRow::builder()
            .title("默认打开视图")
            .subtitle("新打开笔记时的初始视图模式")
            .model(&view_items)
            .build();

        let scheme_items = gtk4::StringList::new(&["跟随系统", "浅色模式", "深色模式"]);
        let scheme_combo = adw::ComboRow::builder()
            .title("界面配色方案")
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
            .title("配置文件")
            .description("保存时原地更新已知字段，保留已有注释与其他自定义键。")
            .build();
        let config_path_row = adw::ActionRow::builder()
            .title("config.toml 路径")
            .subtitle("正在读取…")
            .subtitle_selectable(true)
            .build();
        config_group.add(&config_path_row);
        let save_row_app = Self::build_save_row();
        config_group.add(&save_row_app.0);
        appearance_page.add(&config_group);

        // 2. 终端页
        let terminal_page = adw::PreferencesPage::builder()
            .title("终端")
            .name(SettingsPage::Terminal.name())
            .icon_name("utilities-terminal-symbolic")
            .build();

        let term_group = adw::PreferencesGroup::builder()
            .title("VTE4 内置终端")
            .description("留空时跟随系统等宽字体（Monospace）。")
            .build();
        let terminal_font_entry = adw::EntryRow::builder().title("终端等宽字体族").build();
        term_group.add(&terminal_font_entry);
        let save_row_term = Self::build_save_row();
        term_group.add(&save_row_term.0);
        terminal_page.add(&term_group);

        // 3. Agent 与指南页
        let agent_page = adw::PreferencesPage::builder()
            .title("Agent 与指南")
            .name(SettingsPage::Agent.name())
            .icon_name("system-run-symbolic")
            .build();

        let discovered_group = adw::PreferencesGroup::builder()
            .title("自动发现的 Agent")
            .description("只读扫描系统与常见安装目录，绝不执行目标程序。")
            .build();
        let refresh_discover_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("重新扫描已安装的 Agent")
            .build();
        refresh_discover_btn.add_css_class("flat");
        discovered_group.set_header_suffix(Some(&refresh_discover_btn));
        agent_page.add(&discovered_group);

        let agent_cfg_group = adw::PreferencesGroup::builder()
            .title("Agent 启动模板")
            .description("每行一个 argv 元素（不做 shell 拆分）；参数中须包含 {guide} 占位符。")
            .build();

        let command_entry = adw::EntryRow::builder()
            .title("command（可执行文件或命令名）")
            .build();
        agent_cfg_group.add(&command_entry);

        let args_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        args_box.set_margin_top(8);
        args_box.set_margin_bottom(8);
        args_box.set_margin_start(12);
        args_box.set_margin_end(12);
        let args_label = gtk4::Label::new(Some("args（每行一个 argv 元素）"));
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
        let probe_badge = gtk4::Label::new(Some("未探测"));
        probe_badge.add_css_class("hortus-agent-badge-err");
        let probe_detail = gtk4::Label::new(Some("请填写 command 与包含 {guide} 的 args"));
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

        let guide_group = adw::PreferencesGroup::builder()
            .title("Agent 指南（agent-guide.md）")
            .description("启动 Agent 时自动替换 {guide}，也可通过 `hortus guide` 获取。")
            .build();
        let guide_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        guide_box.set_margin_top(8);
        guide_box.set_margin_bottom(8);
        guide_box.set_margin_start(12);
        guide_box.set_margin_end(12);
        let guide_meta = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let guide_status = gtk4::Label::new(Some("内置默认"));
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
        let guide_reset_btn = gtk4::Button::with_label("恢复默认指南");
        let guide_save_btn = gtk4::Button::with_label("保存指南");
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
                suppress_probe: false,
                discovered: Vec::new(),
                last_probe: None,
                conflict_dialog: glib::WeakRef::new(),
            }),
        });

        Self::bind_signals(
            &inner,
            &[save_row_app.1, save_row_term.1, save_row_agent.1],
            &refresh_discover_btn,
            &guide_save_btn,
            &guide_reset_btn,
        );
        Self::reload_all(&inner);
        Self { inner }
    }

    fn build_save_row() -> (adw::ActionRow, gtk4::Button) {
        let row = adw::ActionRow::builder()
            .title("保存 config.toml")
            .subtitle("写入后立即广播热重载到全部窗口")
            .build();
        let btn = gtk4::Button::with_label("保存设置");
        btn.add_css_class("suggested-action");
        btn.set_valign(gtk4::Align::Center);
        row.add_suffix(&btn);
        row.set_activatable_widget(Some(&btn));
        (row, btn)
    }

    fn bind_signals(
        inner: &Rc<SettingsInner>,
        save_buttons: &[gtk4::Button],
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
            inner.scheme_combo.connect_selected_notify(move |combo| {
                let Some(this) = weak.upgrade() else { return };
                let choice = match combo.selected() {
                    1 => ColorSchemeChoice::Light,
                    2 => ColorSchemeChoice::Dark,
                    _ => ColorSchemeChoice::System,
                };
                let cb = this.state.borrow().on_scheme.clone();
                cb(choice);
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.command_entry.connect_changed(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::probe_agent_async(&this);
                }
            });
        }

        {
            let weak = Rc::downgrade(inner);
            inner.args_buffer.connect_changed(move |_| {
                if let Some(this) = weak.upgrade() {
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

    pub fn present(&self, parent: &impl IsA<gtk4::Widget>, page: Option<SettingsPage>) {
        if let Some(page) = page {
            self.select_page(page);
        }
        self.inner.dialog.present(Some(parent));
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
                    .set_subtitle(&format!("读取配置失败：{err}")),
            }
        });
    }

    fn apply_editable_config(inner: &Rc<SettingsInner>, info: &EditableConfigInfo) {
        {
            let mut st = inner.state.borrow_mut();
            st.digest = info.digest.clone();
            st.suppress_probe = true;
        }
        inner.config_path_row.set_subtitle(&info.config_path);
        inner.font_spin.set_value(info.font_size.clamp(12.0, 24.0));
        inner
            .default_view_combo
            .set_selected(if info.default_view == "editing" { 1 } else { 0 });
        inner
            .terminal_font_entry
            .set_text(info.terminal_font.as_deref().unwrap_or(""));
        inner
            .command_entry
            .set_text(info.agent_command.as_deref().unwrap_or(""));
        inner.args_buffer.set_text(&info.agent_args.join("\n"));
        inner.state.borrow_mut().suppress_probe = false;
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
                .title("未发现内置预设 Agent")
                .subtitle("请在下方手动输入可执行文件路径或命令名")
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
            let fill_btn = gtk4::Button::with_label("填充");
            fill_btn.set_valign(gtk4::Align::Center);
            let weak = Rc::downgrade(inner);
            let exec = agent.executable.clone();
            let args = agent.suggested_args.clone();
            fill_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.state.borrow_mut().suppress_probe = true;
                    this.command_entry.set_text(&exec);
                    this.args_buffer.set_text(&args.join("\n"));
                    this.state.borrow_mut().suppress_probe = false;
                    Self::probe_agent_async(&this);
                }
            });
            row.add_suffix(&fill_btn);
            row.set_activatable_widget(Some(&fill_btn));
            inner.discovered_group.add(&row);
            inner.discovered_rows.borrow_mut().push(row);
        }
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
                this.probe_badge.set_label("可用");
                this.probe_badge.remove_css_class("hortus-agent-badge-err");
                this.probe_badge.add_css_class("hortus-agent-badge-ok");
                this.probe_detail
                    .set_label(status_arc.executable.as_deref().unwrap_or("已解析"));
            } else {
                this.probe_badge.set_label("不可用");
                this.probe_badge.remove_css_class("hortus-agent-badge-ok");
                this.probe_badge.add_css_class("hortus-agent-badge-err");
                let msg = match status_arc.error.as_deref() {
                    Some("agent.command_missing") => "未填写 command",
                    Some("agent.guide_missing") => "args 中缺少 {guide} 占位符",
                    Some("agent.command_not_absolute") => "含路径分隔符的命令须使用绝对路径",
                    Some("agent.not_found") => "未在 PATH 中找到可执行文件",
                    Some("agent.not_executable") => "目标不是可执行常规文件",
                    Some(other) => other,
                    None => "配置无效",
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
        inner.guide_buffer.set_text(&info.text);
        inner.guide_status.set_label(if info.overridden {
            "自定义覆盖"
        } else {
            "内置默认"
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
                    (this.state.borrow().on_toast)("已保存 agent-guide.md".into());
                }
                Err(err) => {
                    (this.state.borrow().on_toast)(format!("保存指南失败：{err}"));
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
                    (this.state.borrow().on_toast)("已恢复默认 Agent 指南".into());
                }
                Err(err) => {
                    (this.state.borrow().on_toast)(format!("恢复默认指南失败：{err}"));
                }
            }
        });
    }

    fn build_input(inner: &Rc<SettingsInner>) -> EditableConfigInputInfo {
        let font_size = (inner.font_spin.value() * 2.0).round() / 2.0;
        let default_view = if inner.default_view_combo.selected() == 1 {
            "editing".to_string()
        } else {
            "reading".to_string()
        };
        let terminal_font = {
            let raw = inner.terminal_font_entry.text().trim().to_string();
            if raw.is_empty() { None } else { Some(raw) }
        };
        let agent_command = {
            let raw = inner.command_entry.text().trim().to_string();
            if raw.is_empty() { None } else { Some(raw) }
        };
        let agent_args = Self::collect_args(&inner.args_buffer);
        EditableConfigInputInfo {
            font_size,
            default_view,
            terminal_font,
            agent_command,
            agent_args,
        }
    }

    fn save_config_impl(inner: &Rc<SettingsInner>, force_overwrite: bool) {
        let input = Self::build_input(inner);
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
            let res =
                gio::spawn_blocking(move || svc.core.save_editable_config(input, expected_digest))
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
                    this.save_status.set_label("配置已保存并热重载");
                    services.config.apply_saved(&saved);
                    (this.state.borrow().on_toast)("已保存配置并同步全部窗口".into());
                }
                Err(err) if err.to_string() == "config.externally_modified" => {
                    this.state.borrow_mut().last_conflict = true;
                    this.save_status
                        .set_label("保存冲突：config.toml 已被外部修改");
                    Self::prompt_external_conflict(&this);
                }
                Err(err) => {
                    this.save_status.set_label(&format!("保存失败：{err}"));
                    (this.state.borrow().on_toast)(format!("保存配置失败：{err}"));
                }
            }
        });
    }

    fn prompt_external_conflict(inner: &Rc<SettingsInner>) {
        let alert = adw::AlertDialog::builder()
            .heading("配置文件已被外部修改")
            .body("打开设置后，config.toml 在磁盘上被其他程序修改。请选择重新加载最新配置或覆盖保存当前设置。")
            .build();
        alert.add_response("reload", "重新加载最新配置");
        alert.add_response("overwrite", "覆盖保存");
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
                this.save_status.set_label("已重新加载磁盘最新配置");
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
        self.inner.state.borrow_mut().suppress_probe = true;
        self.inner.command_entry.set_text(&agent.executable);
        self.inner
            .args_buffer
            .set_text(&agent.suggested_args.join("\n"));
        self.inner.state.borrow_mut().suppress_probe = false;
        SettingsDialog::probe_agent_async(&self.inner);
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
        self.inner.guide_status.text() == "自定义覆盖"
    }

    pub fn save_guide_now(&self) {
        SettingsDialog::save_guide_impl(&self.inner);
    }

    pub fn reset_guide_now(&self) {
        SettingsDialog::reset_guide_impl(&self.inner);
    }

    pub fn close(&self) {
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
        let _ = self.inner.dialog.close();
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
            Rc::new(|_| {}),
        );
        let weak = Rc::downgrade(&dlg.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(dlg);
        assert_eq!(weak.strong_count(), 0);
    }
}
