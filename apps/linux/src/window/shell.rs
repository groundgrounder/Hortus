// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 主窗口外壳、生命周期解环（C-01/C-02/C-03）、库切换与关闭保护。

use crate::agent::ChangesPanel;
use crate::core::{CliOptions, CommandQueue, PanelRegion, StaticToolKind};
use crate::dock::{DockTabId, ToolDock};
use crate::files::{VaultTree, operations::prepare_edit_temp_dir};
use crate::knowledge::{BacklinksPanel, QuickOpen, SearchPanel, TagsPanel};
use crate::reading::FocusController;
use crate::render::math::MathRenderService;
use crate::services::AppServices;
use crate::terminal::TerminalHost;
use crate::window::actions::{apply_color_scheme, build_primary_menu};
use crate::window::commands::WindowCommand;
use crate::window::sidebar_resize::{
    DEFAULT_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, wrap_sidebar_with_resize_handle,
};
use crate::window::state::WindowState;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "automation")]
use std::time::Instant;

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

pub struct WindowInner {
    pub window: adw::ApplicationWindow,
    pub toast_overlay: adw::ToastOverlay,
    pub focus: FocusController,
    pub tab_overview: adw::TabOverview,
    pub toolbar_view: adw::ToolbarView,
    pub window_title: adw::WindowTitle,
    pub left_split: adw::OverlaySplitView,
    pub right_split: adw::OverlaySplitView,
    pub left_resize_handle: gtk4::Box,
    pub right_resize_handle: gtk4::Box,
    pub center_paned: gtk4::Paned,
    pub center_stack: gtk4::Stack,
    pub empty_status: adw::StatusPage,
    pub tab_view: adw::TabView,
    pub vault_tree: VaultTree,
    pub outline_box: gtk4::Box,
    pub bottom_toggle: gtk4::ToggleButton,
    pub edit_badge: gtk4::Label,
    pub mode_toggle_btn: gtk4::Button,
    pub tool_dock: ToolDock,
    pub terminal_host: TerminalHost,
    pub changes_panel: ChangesPanel,
    pub search_panel: SearchPanel,
    pub tags_panel: TagsPanel,
    pub backlinks_panel: BacklinksPanel,
    pub quick_open: QuickOpen,
    pub state: Rc<RefCell<WindowState>>,
    pub commands: CommandQueue<WindowCommand>,
    pub options: CliOptions,
}

#[derive(Clone)]
pub struct HortusWindow {
    pub(crate) inner: Rc<WindowInner>,
}

impl HortusWindow {
    pub fn new(app: &adw::Application, options: CliOptions) -> Self {
        Self::new_with_services(app, options, None)
    }

    pub fn new_with_services(
        app: &adw::Application,
        options: CliOptions,
        preloaded_services: Option<Arc<AppServices>>,
    ) -> Self {
        let initial_width = options.width.unwrap_or(1320);
        let initial_height = options.height.unwrap_or(840);
        let lock_size = options.locks_window_size();

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Hortus")
            .default_width(initial_width)
            .default_height(initial_height)
            .resizable(!lock_size)
            .build();

        if let Some(scheme) = options.color_scheme {
            apply_color_scheme(scheme);
        }

        let toast_overlay = adw::ToastOverlay::new();
        let root_overlay = gtk4::Overlay::new();
        let focus = FocusController::new();
        let tab_overview = adw::TabOverview::new();
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.set_top_bar_style(adw::ToolbarStyle::Flat);

        let window_title = adw::WindowTitle::new("Hortus", "");
        let header_bar = adw::HeaderBar::new();
        header_bar.set_title_widget(Some(&window_title));

        let left_toggle = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipToggleLeftSidebar))
            .active(true)
            .build();
        let open_vault_btn = gtk4::Button::builder()
            .icon_name("folder-open-symbolic")
            .label(crate::i18n::tr(crate::i18n::Key::OpenFolderBtn))
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipOpenFolder))
            .action_name("win.open-vault")
            .build();
        open_vault_btn.add_css_class("flat");
        header_bar.pack_start(&left_toggle);
        header_bar.pack_start(&open_vault_btn);

        let edit_badge = gtk4::Label::new(Some(crate::i18n::tr(crate::i18n::Key::EditBadge)));
        edit_badge.add_css_class("hortus-mode-badge");
        edit_badge.set_visible(false);

        let mode_toggle_btn = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipToggleEditMode))
            .action_name("win.toggle-edit-mode")
            .build();
        mode_toggle_btn.add_css_class("flat");

        let menu_button = gtk4::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipMainMenu))
            .primary(true)
            .menu_model(&build_primary_menu())
            .build();
        let bottom_toggle = gtk4::ToggleButton::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipToggleBottomPanel))
            .active(false)
            .build();
        let right_toggle = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipToggleRightSidebar))
            .active(true)
            .build();
        let overview_btn = gtk4::Button::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipTabOverview))
            .action_name("win.toggle-tab-overview")
            .build();

        header_bar.pack_end(&menu_button);
        header_bar.pack_end(&bottom_toggle);
        header_bar.pack_end(&right_toggle);
        header_bar.pack_end(&overview_btn);
        header_bar.pack_end(&mode_toggle_btn);
        header_bar.pack_end(&edit_badge);
        toolbar_view.add_top_bar(&header_bar);

        #[cfg(feature = "automation")]
        if options.hide_top_bar {
            toolbar_view.set_reveal_top_bars(false);
        }

        // 静态工具视图：文件 / 搜索 / 标签 / 大纲 / 反链 / 改动
        let vault_tree = VaultTree::new();
        let search_panel = SearchPanel::new();
        let tags_panel = TagsPanel::new();

        let files_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let files_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        files_header.add_css_class("hortus-panel-header");
        let files_title = gtk4::Label::new(Some(crate::i18n::tr(crate::i18n::Key::ToolFiles)));
        files_title.add_css_class("caption");
        files_title.add_css_class("dim-label");
        files_title.set_xalign(0.0);
        files_title.set_hexpand(true);
        let new_note_btn = gtk4::Button::builder()
            .icon_name("document-new-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipNewNote))
            .action_name("win.new-note")
            .build();
        new_note_btn.add_css_class("flat");
        let new_folder_btn = gtk4::Button::builder()
            .icon_name("folder-new-symbolic")
            .tooltip_text(crate::i18n::tr(crate::i18n::Key::TooltipNewFolder))
            .action_name("win.new-folder")
            .build();
        new_folder_btn.add_css_class("flat");
        files_header.append(&files_title);
        files_header.append(&new_note_btn);
        files_header.append(&new_folder_btn);
        files_page.append(&files_header);
        files_page.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        files_page.append(vault_tree.widget());

        let outline_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        let outline_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        outline_box.set_margin_top(8);
        outline_box.set_margin_bottom(12);
        outline_box.set_margin_start(6);
        outline_box.set_margin_end(6);
        outline_scroll.set_child(Some(&outline_box));

        let backlinks_panel = BacklinksPanel::new();
        let changes_panel = ChangesPanel::new();
        let terminal_host = TerminalHost::new();

        let mut static_widgets: HashMap<StaticToolKind, gtk4::Widget> = HashMap::new();
        static_widgets.insert(StaticToolKind::Files, files_page.upcast());
        static_widgets.insert(StaticToolKind::Search, search_panel.root().clone().upcast());
        static_widgets.insert(StaticToolKind::Tags, tags_panel.root().clone().upcast());
        static_widgets.insert(StaticToolKind::Outline, outline_scroll.upcast());
        static_widgets.insert(
            StaticToolKind::Backlinks,
            backlinks_panel.root().clone().upcast(),
        );
        static_widgets.insert(
            StaticToolKind::Changes,
            changes_panel.root().clone().upcast(),
        );

        let tool_dock = ToolDock::new(static_widgets);
        tool_dock.apply_default_layout();

        // 中央阅读/编辑标签区
        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);

        let tabs_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        tabs_container.set_vexpand(true);
        tabs_container.set_hexpand(true);
        tabs_container.append(&tab_bar);
        tabs_container.append(&tab_view);

        let open_empty_btn = gtk4::Button::builder()
            .label(crate::i18n::tr(crate::i18n::Key::OpenFolderEllipsis))
            .action_name("win.open-vault")
            .halign(gtk4::Align::Center)
            .build();
        open_empty_btn.add_css_class("suggested-action");
        open_empty_btn.add_css_class("pill");

        let empty_status = adw::StatusPage::builder()
            .icon_name("accessories-text-editor-symbolic")
            .title("Hortus")
            .description(crate::i18n::tr(crate::i18n::Key::EmptyStateDescription))
            .child(&open_empty_btn)
            .vexpand(true)
            .hexpand(true)
            .build();

        let center_stack = gtk4::Stack::new();
        center_stack.set_vexpand(true);
        center_stack.set_hexpand(true);
        center_stack.add_named(&empty_status, Some("empty"));
        center_stack.add_named(&tabs_container, Some("tabs"));
        center_stack.set_visible_child_name("empty");

        let bottom_panel_box = tool_dock.region_widget(PanelRegion::Bottom);
        bottom_panel_box.set_visible(false);
        bottom_panel_box.set_size_request(-1, 120);

        let center_paned = gtk4::Paned::builder()
            .orientation(gtk4::Orientation::Vertical)
            .vexpand(true)
            .hexpand(true)
            .resize_start_child(true)
            .shrink_start_child(false)
            .resize_end_child(false)
            .shrink_end_child(false)
            .start_child(&center_stack)
            .end_child(bottom_panel_box)
            .build();

        let (right_sidebar_wrap, right_resize_handle) = wrap_sidebar_with_resize_handle(
            tool_dock.region_widget(PanelRegion::Right),
            PanelRegion::Right,
        );
        let right_split = adw::OverlaySplitView::builder()
            .sidebar(&right_sidebar_wrap)
            .content(&center_paned)
            .sidebar_position(gtk4::PackType::End)
            .min_sidebar_width(MIN_SIDEBAR_WIDTH)
            .max_sidebar_width(MAX_SIDEBAR_WIDTH)
            .sidebar_width_fraction(0.24)
            .show_sidebar(true)
            .build();

        let (left_sidebar_wrap, left_resize_handle) = wrap_sidebar_with_resize_handle(
            tool_dock.region_widget(PanelRegion::Left),
            PanelRegion::Left,
        );
        let left_split = adw::OverlaySplitView::builder()
            .sidebar(&left_sidebar_wrap)
            .content(&right_split)
            .sidebar_position(gtk4::PackType::Start)
            .min_sidebar_width(MIN_SIDEBAR_WIDTH)
            .max_sidebar_width(MAX_SIDEBAR_WIDTH)
            .sidebar_width_fraction(0.24)
            .show_sidebar(true)
            .build();

        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            819.0,
            adw::LengthUnit::Px,
        ));
        breakpoint.add_setter(&left_split, "collapsed", Some(&true.to_value()));
        breakpoint.add_setter(&right_split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(breakpoint);

        left_split
            .bind_property("show-sidebar", &left_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();
        right_split
            .bind_property("show-sidebar", &right_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();

        toolbar_view.set_content(Some(&left_split));
        tab_overview.set_child(Some(&toolbar_view));
        tab_overview.set_view(Some(&tab_view));

        root_overlay.set_child(Some(&tab_overview));
        root_overlay.add_overlay(focus.root());
        toast_overlay.set_child(Some(&root_overlay));
        window.set_content(Some(&toast_overlay));

        let pending_open_notes = options.open_notes.clone();
        let math_service = preloaded_services
            .as_ref()
            .map(|s| Arc::clone(&s.math_service))
            .unwrap_or_else(|| Arc::new(MathRenderService::new()));
        let window_num = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let state = Rc::new(RefCell::new(WindowState {
            #[cfg(feature = "automation")]
            started_at: Instant::now(),
            self_keepalive: None,
            services: preloaded_services,
            vault_ctx: None,
            vault_sub_id: None,
            vault_open_gen: 0,
            window_reg_id: None,
            window_id: format!("win-{window_num}"),
            login_path: None,
            initial_tabs_override: None,
            initial_tab_views: HashMap::new(),
            suppressing_session_save: false,
            math_service,
            scan_complete: false,
            scan_micros: 0,
            pending_open_notes,
            tabs: HashMap::new(),
            left_sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            right_sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            pending_sidebar_widths: false,
            bottom_revealed: false,
            bottom_height: 260.0,
            pending_bottom_position: false,
            suppressing_bottom_pos: false,
            suppressing_layout_save: false,
            layout_save_source: None,
            #[cfg(feature = "automation")]
            automation_triggered: false,
            temp_dir: prepare_edit_temp_dir(),
            last_toast: None,
            files_busy: false,
            pending_file_changes: Vec::new(),
            file_change_source: None,
            disk_poll_source: None,
            #[cfg(feature = "automation")]
            automation_source: None,
            #[cfg(feature = "automation")]
            overview_source: None,
            close_approved: false,
            last_alert: None,
            last_alert_dialog: glib::WeakRef::new(),
            disposing: false,
            #[cfg(feature = "automation")]
            last_stage: String::new(),
            close_requests: 0,
            close_unsaved: 0,
            force_close: HashSet::new(),
            last_file_op: None,
        }));

        let inner = Rc::new(WindowInner {
            window,
            toast_overlay,
            focus,
            tab_overview,
            toolbar_view,
            window_title,
            left_split,
            right_split,
            left_resize_handle,
            right_resize_handle,
            center_paned,
            center_stack,
            empty_status,
            tab_view,
            vault_tree,
            outline_box,
            bottom_toggle,
            edit_badge,
            mode_toggle_btn,
            tool_dock,
            terminal_host,
            changes_panel,
            search_panel,
            tags_panel,
            backlinks_panel,
            quick_open: QuickOpen::new(),
            state,
            commands: CommandQueue::new(),
            options,
        });

        inner.bind_signals();
        inner.install_sidebar_resize_handlers();
        inner.apply_sidebar_widths(DEFAULT_SIDEBAR_WIDTH, DEFAULT_SIDEBAR_WIDTH);
        inner.register_actions();
        inner.init_knowledge_panels();
        inner.init_terminal_and_changes_panels();
        inner.install_tree_context_menu();
        inner.start_disk_poll_timer();
        inner.render_outline_empty(crate::i18n::tr(crate::i18n::Key::NoNoteOpen));
        inner.initialize_services_async();

        // 由自持字段 keepalive 在窗口打开期间维持控制器存活；teardown/destroy 时置 None 解环（C-01）。
        inner.state.borrow_mut().self_keepalive = Some(Rc::clone(&inner));
        let weak_destroy = Rc::downgrade(&inner);
        inner.window.connect_destroy(move |_| {
            if let Some(win) = weak_destroy.upgrade() {
                win.teardown();
            }
        });

        Self { inner }
    }

    pub fn present(&self) {
        self.inner.window.present();
    }

    #[cfg(any(test, feature = "automation"))]
    pub fn downgrade(&self) -> std::rc::Weak<WindowInner> {
        Rc::downgrade(&self.inner)
    }
}

impl WindowInner {
    pub fn send_command(self: &Rc<Self>, cmd: WindowCommand) {
        if self.state.borrow().disposing {
            return;
        }
        self.commands
            .dispatch(cmd, |item| self.handle_command(item));
    }

    fn handle_command(self: &Rc<Self>, cmd: WindowCommand) {
        match cmd {
            WindowCommand::OpenNote { relpath, heading } => self.open_note_at(&relpath, heading),
            WindowCommand::ResolveWikilink {
                source_note,
                raw_target,
                heading,
            } => self.open_note_or_resolve(&source_note, &raw_target, heading),
            WindowCommand::RelayoutActiveTab(width) => self.apply_relayout_width(width),
            WindowCommand::SwitchActiveEditMode { announce } => self.switch_edit_mode(announce),
            WindowCommand::SwitchToSourceAtByte {
                note_path,
                byte_start,
            } => self.switch_to_source_at_byte(&note_path, byte_start),
            WindowCommand::OpenFocus { note_path, ordinal } => {
                self.open_focus_mode(&note_path, ordinal)
            }
            WindowCommand::OpenTagFromReading(tag) => self.open_tag_from_reading(&tag),
            WindowCommand::FocusSearchPanel => self.focus_search_panel(),
            WindowCommand::OpenQuickOpen => self.open_quick_open(),
            WindowCommand::SaveNote {
                path,
                choice,
                announce,
            } => self.request_save(&path, choice, announce),
            WindowCommand::ToggleBottomPanel(revealed) => self.set_bottom_panel_revealed(revealed),
            WindowCommand::DockNewShell(region) => {
                self.terminal_host.open_shell_in_region(region);
            }
            WindowCommand::DockLaunchAgent(region) => self.launch_agent_in_region(region),
            WindowCommand::DockAddTool { region, tool } => {
                self.tool_dock.open_or_move_static_tool(tool, region, true);
            }
            WindowCommand::DockMoveTab { tab_id, region } => {
                self.tool_dock.move_tab(tab_id, region);
                if let DockTabId::Static(tool) = tab_id {
                    self.show_toast(&crate::i18n::fmt2(
                        crate::i18n::Key::ToastMovedTool,
                        tool.title(),
                        region.title(),
                    ));
                }
            }
            WindowCommand::DockCloseTab(tab_id) => self.tool_dock.close_tab(tab_id),
            WindowCommand::CloseTabPage(page) => self.request_close_tab(page),
            WindowCommand::ApplyColorScheme(scheme) => apply_color_scheme(scheme),
            WindowCommand::NewWindow => self.open_new_window(),
            WindowCommand::OpenSettings(page) => self.open_settings_dialog(page),
            WindowCommand::LaunchAgent => self.launch_agent_action(),
            WindowCommand::SendPathToTerminal => self.send_path_to_terminal(),
            WindowCommand::SendSelectionToTerminal => self.send_selection_to_terminal(),
            WindowCommand::ConfigChanged(cfg) => self.apply_config_update(&cfg),
            WindowCommand::ExternalVaultRefresh(paths) => {
                self.handle_external_vault_refresh(&paths)
            }
        }
    }

    fn bind_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.vault_tree.set_on_open_note(move |path| {
            if let Some(win) = weak.upgrade() {
                win.send_command(WindowCommand::OpenNote {
                    relpath: path,
                    heading: None,
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_selected_page_notify(move |_| {
            if let Some(win) = weak.upgrade() {
                win.sync_active_tab_ui();
                win.check_active_tab_width_alignment();
                win.persist_window_session();
            }
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_close_page(move |_, page| {
            let Some(win) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if win.state.borrow().disposing {
                return glib::Propagation::Proceed;
            }
            win.send_command(WindowCommand::CloseTabPage(page.clone()));
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_page_detached(move |_, page, _| {
            if let Some(win) = weak.upgrade() {
                win.handle_tab_closed(page);
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_toggle.connect_toggled(move |btn| {
            let Some(win) = weak.upgrade() else { return };
            let active = btn.is_active();
            if win.state.borrow().bottom_revealed != active {
                win.send_command(WindowCommand::ToggleBottomPanel(active));
            }
        });

        let weak = Rc::downgrade(self);
        self.center_paned.connect_position_notify(move |paned| {
            let Some(win) = weak.upgrade() else { return };
            let total_h = paned.height();
            let changed = {
                let mut st = win.state.borrow_mut();
                if !st.bottom_revealed || st.suppressing_bottom_pos || total_h <= 200 {
                    false
                } else {
                    let new_bh = f64::from(total_h - paned.position()).clamp(120.0, 520.0);
                    if (new_bh - st.bottom_height).abs() >= 2.0 {
                        st.bottom_height = new_bh;
                        true
                    } else {
                        false
                    }
                }
            };
            if changed {
                win.schedule_layout_save();
            }
        });

        let weak = Rc::downgrade(self);
        self.window.connect_fullscreened_notify(move |win| {
            let Some(this) = weak.upgrade() else { return };
            let fs = win.is_fullscreen();
            this.toolbar_view.set_reveal_top_bars(!fs);
            if fs {
                this.show_toast(crate::i18n::tr(
                    crate::i18n::Key::ToastFullscreenTopBarHidden,
                ));
            }
        });

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            weak.upgrade()
                .map(|win| win.handle_window_close())
                .unwrap_or(glib::Propagation::Proceed)
        });
    }

    pub fn teardown(&self) {
        let (keepalive, services, reg_id, vault_ctx, vault_sub_id, tabs) = {
            let mut st = self.state.borrow_mut();
            if st.disposing {
                return;
            }
            st.disposing = true;
            st.cancel_global_timers();
            st.last_alert_dialog.set(None);
            self.commands.clear();
            (
                st.self_keepalive.take(),
                st.services.take(),
                st.window_reg_id.take(),
                st.vault_ctx.take(),
                st.vault_sub_id.take(),
                std::mem::take(&mut st.tabs),
            )
        };
        if let (Some(ctx), Some(sub_id)) = (&vault_ctx, vault_sub_id) {
            ctx.unsubscribe(sub_id);
        }
        drop(vault_ctx);
        if let (Some(svcs), Some(id)) = (services, reg_id) {
            svcs.windows.unregister(id);
        }
        for (_, tab) in tabs {
            tab.edit_state.borrow_mut().cancel_timers();
            tab.reading_viewport.clear_timers();
            tab.edit_host.clear_timers();
            tab.find.teardown();
            tab.completion.teardown();
        }
        self.focus.teardown();
        self.quick_open.close_if_open();
        self.tool_dock.teardown();
        self.terminal_host.teardown();
        self.vault_tree.clear();
        drop(keepalive);
    }

    pub fn show_toast(&self, title: &str) {
        self.state.borrow_mut().last_toast = Some(title.to_string());
        let toast = adw::Toast::new(title);
        toast.set_timeout(3);
        self.toast_overlay.add_toast(toast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CliOptions;
    use gtk4::gio;

    #[test]
    fn window_teardown_breaks_reference_cycles() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            eprintln!("skipping HortusWindow test: GTK 初始化失败或非主线程");
            return;
        }
        let app = adw::Application::builder()
            .application_id("org.hortus.test.WindowTeardown")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        let weak_slot: Rc<RefCell<Option<std::rc::Weak<WindowInner>>>> =
            Rc::new(RefCell::new(None));
        let weak_slot_act = Rc::clone(&weak_slot);
        app.connect_activate(move |app| {
            let win = HortusWindow::new(app, CliOptions::default());
            let weak = win.downgrade();
            *weak_slot_act.borrow_mut() = Some(weak);
            win.present();
            let gtk_win = win.inner.window.clone();
            drop(win);
            let app_on_close = app.clone();
            let app_timeout = app.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(3000), move || {
                app_timeout.quit();
            });
            glib::idle_add_local_once(move || {
                gtk_win.close();
                glib::idle_add_local_once(move || app_on_close.quit());
            });
        });
        app.run_with_args(&Vec::<String>::new());
        let weak = weak_slot
            .borrow()
            .as_ref()
            .expect("activate should construct HortusWindow")
            .clone();
        assert_eq!(
            weak.strong_count(),
            0,
            "WindowInner remained strongly referenced after window.close()"
        );
    }
}
