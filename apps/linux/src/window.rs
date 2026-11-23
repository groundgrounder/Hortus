// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::{
    APP_ID, APP_VERSION, CliOptions, ColorSchemeChoice, HeadingItem, NotifyQueue, PanelPlacement,
    PanelRegion, PlaceholderKind, SmokeHeading, SmokeReport, SmokeTab, utf16_offset_to_char_offset,
};
use crate::style::{apply_source_styles, create_tag_table};
use crate::tree::VaultTree;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentSession, VaultHandle, WorkspaceCore};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct OpenNoteTab {
    path: String,
    page: adw::TabPage,
    scrolled: gtk4::ScrolledWindow,
    text_view: gtk4::TextView,
    buffer: gtk4::TextBuffer,
    text: String,
    headings: Vec<HeadingItem>,
    session: Option<Arc<DocumentSession>>,
    editing: bool,
    loaded: bool,
}

struct PlaceholderWidgets {
    backlinks: gtk4::Box,
    changes: gtk4::Box,
    terminal: gtk4::Box,
}

impl PlaceholderWidgets {
    fn widget(&self, kind: PlaceholderKind) -> &gtk4::Box {
        match kind {
            PlaceholderKind::Backlinks => &self.backlinks,
            PlaceholderKind::Changes => &self.changes,
            PlaceholderKind::Terminal => &self.terminal,
        }
    }
}

struct AppState {
    started_at: Instant,
    core: Option<Arc<WorkspaceCore>>,
    vault_root: Option<String>,
    vault_handle: Option<Arc<VaultHandle>>,
    scan_complete: bool,
    scan_micros: u64,
    pending_open_notes: Vec<String>,
    tabs: HashMap<String, OpenNoteTab>,
    placement: PanelPlacement,
    bottom_revealed: bool,
    smoke_triggered: bool,
}

#[derive(Clone)]
pub struct HortusWindow {
    pub window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    tab_overview: adw::TabOverview,
    toolbar_view: adw::ToolbarView,
    window_title: adw::WindowTitle,
    left_split: adw::OverlaySplitView,
    right_split: adw::OverlaySplitView,
    center_stack: gtk4::Stack,
    empty_status: adw::StatusPage,
    tab_view: adw::TabView,
    vault_tree: VaultTree,
    outline_box: gtk4::Box,
    right_stack: gtk4::Stack,
    right_switcher: gtk4::Box,
    bottom_stack: gtk4::Stack,
    bottom_switcher: gtk4::Box,
    bottom_revealer: gtk4::Revealer,
    edit_badge: gtk4::Label,
    placeholders: Rc<PlaceholderWidgets>,
    state: Rc<RefCell<AppState>>,
    options: CliOptions,
}

impl HortusWindow {
    pub fn new(app: &adw::Application, options: CliOptions) -> Self {
        let initial_width = options.width.unwrap_or(1320);
        let initial_height = options.height.unwrap_or(840);
        let lock_size = options.width.is_some()
            && options.height.is_some()
            && (options.screenshot.is_some() || options.smoke_json.is_some());

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
        let tab_overview = adw::TabOverview::new();
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.set_top_bar_style(adw::ToolbarStyle::Flat);

        let window_title = adw::WindowTitle::new("Hortus", "");

        let header_bar = adw::HeaderBar::new();
        header_bar.set_title_widget(Some(&window_title));

        let left_toggle = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("切换左侧栏 (Ctrl+Shift+S)")
            .active(true)
            .build();
        let open_vault_btn = gtk4::Button::builder()
            .icon_name("folder-open-symbolic")
            .label("打开知识库")
            .tooltip_text("打开知识库文件夹 (Ctrl+O)")
            .action_name("win.open-vault")
            .build();
        open_vault_btn.add_css_class("flat");
        header_bar.pack_start(&left_toggle);
        header_bar.pack_start(&open_vault_btn);

        let edit_badge = gtk4::Label::new(Some("编辑原型（不落盘）"));
        edit_badge.add_css_class("hortus-mode-badge");
        edit_badge.set_visible(false);

        let menu_button = gtk4::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("主菜单")
            .primary(true)
            .menu_model(&build_primary_menu())
            .build();
        let bottom_toggle = gtk4::ToggleButton::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text("切换底部面板 (Ctrl+J)")
            .active(false)
            .build();
        let right_toggle = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("切换右侧栏 (Ctrl+Shift+I)")
            .active(true)
            .build();
        let overview_btn = gtk4::Button::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text("标签全景网格 (Ctrl+Shift+O)")
            .action_name("win.toggle-tab-overview")
            .build();

        header_bar.pack_end(&menu_button);
        header_bar.pack_end(&bottom_toggle);
        header_bar.pack_end(&right_toggle);
        header_bar.pack_end(&overview_btn);
        header_bar.pack_end(&edit_badge);
        toolbar_view.add_top_bar(&header_bar);

        if options.hide_top_bar {
            toolbar_view.set_reveal_top_bars(false);
        }

        // 左侧栏：文件 / 搜索(L2) / 标签(L2)
        let vault_tree = VaultTree::new();
        let left_stack = gtk4::Stack::new();
        left_stack.set_vexpand(true);
        left_stack.add_named(&vault_tree.widget, Some("files"));
        left_stack.add_named(
            &build_placeholder_box(
                "system-search-symbolic",
                "全文搜索与快速打开",
                "将在 L2 接入内核 SearchIndex，支持中英文检索与快速打开。",
            ),
            Some("search"),
        );
        left_stack.add_named(
            &build_placeholder_box(
                "view-list-bullet-symbolic",
                "层级标签浏览",
                "将在 L2 接入内核 KnowledgeIndex，支持按 #标签 过滤笔记。",
            ),
            Some("tags"),
        );
        left_stack.set_visible_child_name("files");

        let left_switcher = build_left_switcher(&left_stack);
        let left_sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        left_sidebar_box.append(&left_switcher);
        left_sidebar_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        left_sidebar_box.append(&left_stack);

        // 右侧栏：真实大纲 + 可移动占位页签
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

        let placeholders = Rc::new(PlaceholderWidgets {
            backlinks: build_placeholder_panel(PlaceholderKind::Backlinks),
            changes: build_placeholder_panel(PlaceholderKind::Changes),
            terminal: build_placeholder_panel(PlaceholderKind::Terminal),
        });

        let right_stack = gtk4::Stack::new();
        right_stack.set_vexpand(true);
        right_stack.add_named(&outline_scroll, Some("outline"));
        right_stack.add_named(
            &placeholders.backlinks,
            Some(PlaceholderKind::Backlinks.id()),
        );
        right_stack.add_named(&placeholders.changes, Some(PlaceholderKind::Changes.id()));
        right_stack.set_visible_child_name("outline");

        let right_switcher = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        right_switcher.add_css_class("hortus-sidebar-switcher");
        let right_sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        right_sidebar_box.append(&right_switcher);
        right_sidebar_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        right_sidebar_box.append(&right_stack);

        // 中间阅读/编辑区（AdwStatusPage + AdwTabBar + AdwTabView）
        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(true);

        let tabs_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        tabs_container.set_vexpand(true);
        tabs_container.set_hexpand(true);
        tabs_container.append(&tab_bar);
        tabs_container.append(&tab_view);

        let open_empty_btn = gtk4::Button::builder()
            .label("打开知识库…")
            .action_name("win.open-vault")
            .halign(gtk4::Align::Center)
            .build();
        open_empty_btn.add_css_class("suggested-action");
        open_empty_btn.add_css_class("pill");

        let empty_status = adw::StatusPage::builder()
            .icon_name("accessories-text-editor-symbolic")
            .title("Hortus")
            .description("打开知识库，开始阅读。")
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

        // 底部面板（默认收起，Ctrl+J 展开/收起）
        let bottom_stack = gtk4::Stack::new();
        bottom_stack.set_vexpand(false);
        bottom_stack.set_size_request(-1, 180);
        bottom_stack.add_named(&placeholders.terminal, Some(PlaceholderKind::Terminal.id()));

        let bottom_switcher = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        bottom_switcher.add_css_class("hortus-panel-header");

        let bottom_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        bottom_header.append(&bottom_switcher);
        let bottom_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        bottom_spacer.set_hexpand(true);
        bottom_header.append(&bottom_spacer);
        let bottom_close = gtk4::Button::builder()
            .icon_name("pan-down-symbolic")
            .tooltip_text("收起底部面板 (Ctrl+J)")
            .build();
        bottom_close.add_css_class("flat");
        bottom_header.append(&bottom_close);

        let bottom_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        bottom_box.add_css_class("hortus-bottom-panel");
        bottom_box.append(&bottom_header);
        bottom_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        bottom_box.append(&bottom_stack);

        let bottom_revealer = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideUp)
            .reveal_child(false)
            .visible(false)
            .child(&bottom_box)
            .build();
        bottom_revealer.connect_child_revealed_notify(|rev| {
            if !rev.reveals_child() && !rev.is_child_revealed() {
                rev.set_visible(false);
            }
        });

        let center_and_bottom = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        center_and_bottom.set_vexpand(true);
        center_and_bottom.set_hexpand(true);
        center_and_bottom.append(&center_stack);
        center_and_bottom.append(&bottom_revealer);

        // 右侧与左侧 AdwOverlaySplitView，配 AdwBreakpoint (< 820px 自动折叠)
        let right_split = adw::OverlaySplitView::builder()
            .sidebar(&right_sidebar_box)
            .content(&center_and_bottom)
            .sidebar_position(gtk4::PackType::End)
            .min_sidebar_width(220.0)
            .max_sidebar_width(320.0)
            .sidebar_width_fraction(0.22)
            .show_sidebar(true)
            .build();

        let left_split = adw::OverlaySplitView::builder()
            .sidebar(&left_sidebar_box)
            .content(&right_split)
            .sidebar_position(gtk4::PackType::Start)
            .min_sidebar_width(220.0)
            .max_sidebar_width(320.0)
            .sidebar_width_fraction(0.22)
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
        toast_overlay.set_child(Some(&tab_overview));
        window.set_content(Some(&toast_overlay));

        let state = Rc::new(RefCell::new(AppState {
            started_at: Instant::now(),
            core: None,
            vault_root: None,
            vault_handle: None,
            scan_complete: false,
            scan_micros: 0,
            pending_open_notes: options.open_notes.clone(),
            tabs: HashMap::new(),
            placement: PanelPlacement::default(),
            bottom_revealed: false,
            smoke_triggered: false,
        }));

        let this = Self {
            window,
            toast_overlay,
            tab_overview,
            toolbar_view,
            window_title,
            left_split,
            right_split,
            center_stack,
            empty_status,
            tab_view,
            vault_tree,
            outline_box,
            right_stack,
            right_switcher,
            bottom_stack,
            bottom_switcher,
            bottom_revealer,
            edit_badge,
            placeholders,
            state,
            options,
        };

        this.rebuild_panel_switchers();
        this.bind_signals(&bottom_toggle, &bottom_close);
        this.register_actions();
        this.render_outline_empty("未打开笔记");
        this.initialize_core_async();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn bind_signals(&self, bottom_toggle: &gtk4::ToggleButton, bottom_close: &gtk4::Button) {
        let this = self.clone();
        self.vault_tree.set_on_open_note(move |path| {
            this.open_note(&path);
        });

        let this = self.clone();
        self.tab_view.connect_selected_page_notify(move |_| {
            this.sync_active_tab_ui();
        });

        let this = self.clone();
        self.tab_view.connect_close_page(move |view, page| {
            this.handle_tab_closed(page);
            view.close_page_finish(page, true);
            glib::Propagation::Stop
        });

        let this = self.clone();
        bottom_toggle.connect_toggled(move |btn| {
            let active = btn.is_active();
            this.set_bottom_panel_revealed(active);
        });

        let this = self.clone();
        let bottom_toggle_for_close = bottom_toggle.clone();
        bottom_close.connect_clicked(move |_| {
            bottom_toggle_for_close.set_active(false);
            this.set_bottom_panel_revealed(false);
        });

        // 进入全屏自动隐藏顶栏；退出全屏恢复顶栏（除非用户明确保持隐藏）。
        let this = self.clone();
        self.window.connect_fullscreened_notify(move |win| {
            let fs = win.is_fullscreen();
            this.toolbar_view.set_reveal_top_bars(!fs);
            if fs {
                this.show_toast("已进入全屏并隐藏顶栏，按 F11 退出全屏");
            }
        });

        let state_on_close = self.state.clone();
        self.window.connect_close_request(move |_| {
            if let Some(handle) = state_on_close.borrow_mut().vault_handle.take() {
                std::thread::spawn(move || {
                    handle.close();
                });
            }
            glib::Propagation::Proceed
        });
    }

    fn register_actions(&self) {
        let this = self.clone();
        let open_vault = gio::SimpleAction::new("open-vault", None);
        open_vault.connect_activate(move |_, _| this.prompt_open_vault());
        self.window.add_action(&open_vault);

        let this = self.clone();
        let toggle_left = gio::SimpleAction::new("toggle-left-sidebar", None);
        toggle_left.connect_activate(move |_, _| {
            let next = !this.left_split.shows_sidebar();
            this.left_split.set_show_sidebar(next);
        });
        self.window.add_action(&toggle_left);

        let this = self.clone();
        let toggle_right = gio::SimpleAction::new("toggle-right-sidebar", None);
        toggle_right.connect_activate(move |_, _| {
            let next = !this.right_split.shows_sidebar();
            this.right_split.set_show_sidebar(next);
        });
        self.window.add_action(&toggle_right);

        let this = self.clone();
        let toggle_bottom = gio::SimpleAction::new("toggle-bottom-panel", None);
        toggle_bottom.connect_activate(move |_, _| {
            let next = !this.state.borrow().bottom_revealed;
            this.set_bottom_panel_revealed(next);
        });
        self.window.add_action(&toggle_bottom);

        let this = self.clone();
        let toggle_overview = gio::SimpleAction::new("toggle-tab-overview", None);
        toggle_overview.connect_activate(move |_, _| {
            let next = !this.tab_overview.is_open();
            this.tab_overview.set_open(next);
        });
        self.window.add_action(&toggle_overview);

        let this = self.clone();
        let toggle_edit = gio::SimpleAction::new("toggle-edit-mode", None);
        toggle_edit.connect_activate(move |_, _| this.toggle_active_tab_edit_mode());
        self.window.add_action(&toggle_edit);

        let this = self.clone();
        let toggle_top_bar = gio::SimpleAction::new("toggle-top-bar", None);
        toggle_top_bar.connect_activate(move |_, _| {
            let next = !this.toolbar_view.reveals_top_bars();
            this.toolbar_view.set_reveal_top_bars(next);
            if next {
                this.show_toast("已显示顶栏");
            } else {
                this.show_toast("已隐藏顶栏，按 Ctrl+Shift+H 恢复");
            }
        });
        self.window.add_action(&toggle_top_bar);

        let this = self.clone();
        let toggle_fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
        toggle_fullscreen.connect_activate(move |_, _| {
            if this.window.is_fullscreen() {
                this.window.unfullscreen();
            } else {
                this.window.fullscreen();
            }
        });
        self.window.add_action(&toggle_fullscreen);

        let this = self.clone();
        let close_tab = gio::SimpleAction::new("close-tab", None);
        close_tab.connect_activate(move |_, _| {
            if let Some(page) = this.tab_view.selected_page() {
                this.tab_view.close_page(&page);
            }
        });
        self.window.add_action(&close_tab);

        let initial_scheme = self
            .options
            .color_scheme
            .unwrap_or(ColorSchemeChoice::System)
            .as_str();
        let color_action = gio::SimpleAction::new_stateful(
            "color-scheme",
            Some(glib::VariantTy::STRING),
            &initial_scheme.to_variant(),
        );
        color_action.connect_activate(|action, param| {
            let Some(val) = param.and_then(|v| v.str()) else {
                return;
            };
            if let Ok(choice) = ColorSchemeChoice::parse(val) {
                apply_color_scheme(choice);
                action.set_state(&val.to_variant());
            }
        });
        self.window.add_action(&color_action);

        let this = self.clone();
        let move_panel = gio::SimpleAction::new("move-panel", Some(glib::VariantTy::STRING));
        move_panel.connect_activate(move |_, param| {
            let Some(id) = param.and_then(|v| v.str()) else {
                return;
            };
            let Some(kind) = PlaceholderKind::parse(id) else {
                return;
            };
            this.toggle_placeholder_region(kind);
        });
        self.window.add_action(&move_panel);

        let this = self.clone();
        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(move |_, _| this.show_about_dialog());
        self.window.add_action(&about);
    }

    fn initialize_core_async(&self) {
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(|| WorkspaceCore::new().map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|_| Err("初始化线程异常".into()));
            match result {
                Ok(core) => {
                    this.state.borrow_mut().core = Some(core);
                    if let Some(vault) = this.options.vault.clone() {
                        this.open_vault_path(vault);
                    } else {
                        this.maybe_schedule_automation();
                    }
                }
                Err(err) => {
                    this.show_toast(&format!("初始化内核失败：{err}"));
                }
            }
        });
    }

    fn prompt_open_vault(&self) {
        let dialog = gtk4::FileDialog::builder()
            .title("打开知识库文件夹")
            .modal(true)
            .build();
        let this = self.clone();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res
                && let Some(path) = file.path()
            {
                this.open_vault_path(path);
            }
        });
    }

    pub fn open_vault_path(&self, path: PathBuf) {
        let Some(core) = self.state.borrow().core.clone() else {
            self.show_toast("内核尚未就绪");
            return;
        };

        let old_handle = {
            let mut st = self.state.borrow_mut();
            st.scan_complete = false;
            st.scan_micros = 0;
            st.vault_handle.take()
        };
        if let Some(handle) = old_handle {
            std::thread::spawn(move || handle.close());
        }

        // 关闭现有标签页并清空树
        let existing_pages: Vec<adw::TabPage> = (0..self.tab_view.n_pages())
            .map(|i| self.tab_view.nth_page(i))
            .collect();
        for page in existing_pages {
            self.tab_view.close_page(&page);
        }
        self.vault_tree.clear();
        self.render_outline_empty("正在扫描知识库…");

        let this = self.clone();
        let path_str = path.to_string_lossy().into_owned();
        glib::MainContext::default().spawn_local(async move {
            let opened =
                gio::spawn_blocking(move || core.open_vault(path_str).map_err(|e| e.to_string()))
                    .await
                    .unwrap_or_else(|_| Err("打开知识库线程异常".into()));
            match opened {
                Ok(handle) => {
                    let canonical = handle.root();
                    let folder_name = Path::new(&canonical)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Hortus")
                        .to_string();
                    this.window_title.set_subtitle(&folder_name);
                    this.empty_status
                        .set_description(Some("从左侧文件树选择一篇笔记。"));
                    {
                        let mut st = this.state.borrow_mut();
                        st.vault_root = Some(canonical);
                        st.vault_handle = Some(handle.clone());
                    }

                    let pending: Vec<String> = {
                        let mut st = this.state.borrow_mut();
                        std::mem::take(&mut st.pending_open_notes)
                    };
                    for rel in &pending {
                        this.open_note(rel);
                    }
                    if let Some(first) = pending.first() {
                        this.open_note(first);
                    }

                    // 在后台线程阻塞等待扫描批次，通过 NotifyQueue 回传主循环，不阻塞 GTK。
                    let queue = NotifyQueue::new();
                    let queue_bg = queue.clone();
                    std::thread::spawn(move || {
                        loop {
                            let batches = handle.wait_for_batches();
                            if batches.is_empty() {
                                break;
                            }
                            queue_bg.push(batches);
                        }
                        queue_bg.close();
                    });

                    let this_ui = this.clone();
                    glib::MainContext::default().spawn_local(async move {
                        while let Some(batches) = queue.next().await {
                            let (complete, micros) = this_ui.vault_tree.apply_batches(&batches);
                            if complete {
                                {
                                    let mut st = this_ui.state.borrow_mut();
                                    st.scan_complete = true;
                                    st.scan_micros = micros;
                                }
                                this_ui.maybe_schedule_automation();
                            }
                        }
                    });
                }
                Err(err) => {
                    this.show_toast(&format!("无法打开知识库：{err}"));
                }
            }
        });
    }

    pub fn open_note(&self, relpath: &str) {
        if let Some(existing) = self.state.borrow().tabs.get(relpath) {
            self.tab_view.set_selected_page(&existing.page);
            self.center_stack.set_visible_child_name("tabs");
            return;
        }

        let (core, vault_root) = {
            let st = self.state.borrow();
            (st.core.clone(), st.vault_root.clone())
        };
        let (Some(core), Some(vault_root)) = (core, vault_root) else {
            return;
        };

        let tag_table = create_tag_table();
        let buffer = gtk4::TextBuffer::new(Some(&tag_table));
        buffer.set_text("正在加载…");

        let text_view = gtk4::TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
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
        text_view.add_css_class("hortus-note-view");

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&text_view)
            .build();

        let page = self.tab_view.append(&scrolled);
        let file_title = Path::new(relpath)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(relpath);
        page.set_title(file_title);
        page.set_tooltip(relpath);
        self.tab_view.set_selected_page(&page);
        self.center_stack.set_visible_child_name("tabs");

        self.state.borrow_mut().tabs.insert(
            relpath.to_string(),
            OpenNoteTab {
                path: relpath.to_string(),
                page,
                scrolled,
                text_view,
                buffer,
                text: String::new(),
                headings: Vec::new(),
                session: None,
                editing: false,
                loaded: false,
            },
        );

        let this = self.clone();
        let rel = relpath.to_string();
        glib::MainContext::default().spawn_local(async move {
            let rel_bg = rel.clone();
            let load_result = gio::spawn_blocking(move || {
                let session = core
                    .open_document(vault_root, rel_bg)
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
                Ok::<_, String>((session, text, headings, styles.packed_runs))
            })
            .await
            .unwrap_or_else(|_| Err("读取笔记线程异常".into()));

            match load_result {
                Ok((session, text, headings, packed_runs)) => {
                    {
                        let mut st = this.state.borrow_mut();
                        if let Some(tab) = st.tabs.get_mut(&rel) {
                            tab.buffer.set_text(&text);
                            apply_source_styles(&tab.buffer, &text, &packed_runs);
                            tab.text = text;
                            tab.headings = headings;
                            tab.session = Some(session);
                            tab.loaded = true;
                        }
                    }
                    this.sync_active_tab_ui();
                    this.maybe_schedule_automation();
                }
                Err(err) => {
                    if let Some(tab) = this.state.borrow_mut().tabs.get_mut(&rel) {
                        tab.buffer.set_text(&format!("读取笔记失败：{err}"));
                        tab.loaded = true;
                    }
                    this.show_toast(&format!("打开笔记失败：{err}"));
                }
            }
        });
    }

    fn handle_tab_closed(&self, closed_page: &adw::TabPage) {
        let mut removed_path = None;
        {
            let mut st = self.state.borrow_mut();
            if let Some(key) = st
                .tabs
                .iter()
                .find(|(_, tab)| &tab.page == closed_page)
                .map(|(k, _)| k.clone())
            {
                st.tabs.remove(&key);
                removed_path = Some(key);
            }
        }
        if removed_path.is_some() && self.state.borrow().tabs.is_empty() {
            self.center_stack.set_visible_child_name("empty");
            self.edit_badge.set_visible(false);
            self.render_outline_empty("未打开笔记");
        } else {
            self.sync_active_tab_ui();
        }
    }

    fn active_tab_path(&self) -> Option<String> {
        let page = self.tab_view.selected_page()?;
        self.state
            .borrow()
            .tabs
            .values()
            .find(|t| t.page == page)
            .map(|t| t.path.clone())
    }

    fn sync_active_tab_ui(&self) {
        let active_path = self.active_tab_path();
        let Some(path) = active_path else {
            self.edit_badge.set_visible(false);
            self.render_outline_empty("未打开笔记");
            return;
        };

        let (editing, headings, loaded) = {
            let st = self.state.borrow();
            if let Some(tab) = st.tabs.get(&path) {
                (tab.editing, tab.headings.clone(), tab.loaded)
            } else {
                return;
            }
        };

        self.edit_badge.set_visible(editing);
        self.vault_tree.select_path(&path);
        if loaded {
            self.render_outline(&path, &headings);
        } else {
            self.render_outline_empty("正在解析大纲…");
        }
    }

    fn toggle_active_tab_edit_mode(&self) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast("请先打开一篇笔记");
            return;
        };
        let mut new_editing = false;
        let mut target_view = None;
        {
            let mut st = self.state.borrow_mut();
            if let Some(tab) = st.tabs.get_mut(&path) {
                tab.editing = !tab.editing;
                new_editing = tab.editing;
                tab.text_view.set_editable(new_editing);
                tab.text_view.set_cursor_visible(new_editing);
                target_view = Some(tab.text_view.clone());
            }
        }
        self.edit_badge.set_visible(new_editing);
        if new_editing {
            if let Some(tv) = target_view {
                tv.grab_focus();
            }
            self.show_toast("已切换到内存原型编辑态（支持中文输入法，不写盘）");
        } else {
            self.show_toast("已切回只读浏览态");
        }
    }

    fn render_outline_empty(&self, message: &str) {
        while let Some(child) = self.outline_box.first_child() {
            self.outline_box.remove(&child);
        }
        let label = gtk4::Label::new(Some(message));
        label.add_css_class("dim-label");
        label.set_margin_top(24);
        self.outline_box.append(&label);
    }

    fn render_outline(&self, note_path: &str, headings: &[HeadingItem]) {
        while let Some(child) = self.outline_box.first_child() {
            self.outline_box.remove(&child);
        }
        if headings.is_empty() {
            self.render_outline_empty("该笔记没有标题");
            return;
        }
        for heading in headings {
            let label = gtk4::Label::new(Some(&heading.title));
            label.set_halign(gtk4::Align::Start);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_xalign(0.0);

            let btn = gtk4::Button::builder()
                .child(&label)
                .halign(gtk4::Align::Fill)
                .build();
            btn.add_css_class("flat");
            btn.add_css_class("hortus-outline-row");
            if heading.level <= 2 {
                btn.add_css_class("hortus-outline-h1");
            } else {
                btn.add_css_class("hortus-outline-sub");
            }
            let indent = i32::from(heading.level.saturating_sub(1)) * 14;
            btn.set_margin_start(indent);

            let this = self.clone();
            let target_note = note_path.to_string();
            let start_utf16 = heading.start;
            btn.connect_clicked(move |_| {
                this.scroll_to_utf16_offset(&target_note, start_utf16);
            });
            self.outline_box.append(&btn);
        }
    }

    fn scroll_to_utf16_offset(&self, note_path: &str, start_utf16: u64) {
        let st = self.state.borrow();
        let Some(tab) = st.tabs.get(note_path) else {
            return;
        };
        let char_offset = utf16_offset_to_char_offset(&tab.text, start_utf16 as usize);
        let mut iter = tab.buffer.iter_at_offset(char_offset as i32);
        tab.buffer.place_cursor(&iter);
        tab.text_view
            .scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.12);
        // 在无主循环空闲时也可通过 adjustment 立即落位，保障大纲跳转响应。
        let adj = tab.scrolled.vadjustment();
        let rect = tab.text_view.iter_location(&iter);
        let target_y = (f64::from(rect.y()) - 12.0).max(0.0);
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value(target_y.min(max_y));
    }

    fn set_bottom_panel_revealed(&self, revealed: bool) {
        self.state.borrow_mut().bottom_revealed = revealed;
        if revealed {
            self.bottom_revealer.set_visible(true);
        }
        self.bottom_revealer.set_reveal_child(revealed);
    }

    fn toggle_placeholder_region(&self, kind: PlaceholderKind) {
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
        let region_name = match next {
            PanelRegion::Right => "右侧栏",
            PanelRegion::Bottom => "底部面板",
        };
        self.show_toast(&format!("已将「{}」移至{}", kind.title(), region_name));
    }

    fn rebuild_panel_switchers(&self) {
        clear_box(&self.right_switcher);
        clear_box(&self.bottom_switcher);

        let placement = self.state.borrow().placement.clone();

        // 右侧栏固定保留「大纲」，外加停靠在右侧栏的占位页签
        let mut right_items: Vec<(&'static str, &'static str)> = vec![("outline", "大纲")];
        for kind in placement.views_in(PanelRegion::Right) {
            right_items.push((kind.id(), kind.title()));
        }
        populate_stack_switcher(&self.right_switcher, &self.right_stack, &right_items);
        if self.right_stack.visible_child_name().is_none() {
            self.right_stack.set_visible_child_name("outline");
        }

        // 底部面板展示停靠在底部的页签
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
                "面向阅读与 AI Agent 协作的原生 Markdown 知识库（Linux GTK4 + libadwaita 原型）。",
            )
            .build();
        dialog.present(Some(&self.window));
    }

    fn show_toast(&self, title: &str) {
        let toast = adw::Toast::new(title);
        toast.set_timeout(3);
        self.toast_overlay.add_toast(toast);
    }

    fn maybe_schedule_automation(&self) {
        let ready = {
            let st = self.state.borrow();
            if st.smoke_triggered {
                return;
            }
            if st.vault_root.is_some() && !st.scan_complete {
                return;
            }
            if !st.tabs.values().all(|t| t.loaded) {
                return;
            }
            true
        };
        if !ready {
            return;
        }
        if self.options.smoke_json.is_none() && self.options.screenshot.is_none() {
            if let Some(ms) = self.options.quit_after_ms {
                let win = self.window.clone();
                glib::timeout_add_local_once(Duration::from_millis(ms), move || {
                    win.close();
                });
            }
            return;
        }

        self.state.borrow_mut().smoke_triggered = true;
        if self.options.overview {
            // 先依次短暂切换并绘制每个标签页，确保 AdwTabOverview 为所有卡片生成真实缩略图。
            let total = self.tab_view.n_pages();
            let this = self.clone();
            let index = Rc::new(Cell::new(0i32));
            glib::timeout_add_local(Duration::from_millis(90), move || {
                let current = index.get();
                if current < total {
                    let page = this.tab_view.nth_page(current);
                    this.tab_view.set_selected_page(&page);
                    this.queue_render_redraw(&page);
                    index.set(current + 1);
                    glib::ControlFlow::Continue
                } else {
                    if total > 0 {
                        let first = this.tab_view.nth_page(0);
                        this.tab_view.set_selected_page(&first);
                    }
                    this.tab_overview.set_open(true);
                    let run_this = this.clone();
                    glib::timeout_add_local_once(Duration::from_millis(450), move || {
                        run_this.run_automation();
                    });
                    glib::ControlFlow::Break
                }
            });
            return;
        }

        let this = self.clone();
        // 等待 GTK4 布局绘制稳定后再截图/运行自检。
        glib::timeout_add_local_once(Duration::from_millis(450), move || {
            this.run_automation();
        });
    }

    fn queue_render_redraw(&self, page: &adw::TabPage) {
        page.child().queue_draw();
    }

    fn run_automation(&self) {
        if let Some(ref screenshot_path) = self.options.screenshot
            && let Err(err) = self.capture_window_png(screenshot_path)
        {
            eprintln!("screenshot failed: {err}");
            std::process::exit(1);
        }
        if let Some(ref smoke_path) = self.options.smoke_json {
            let smoke_path = smoke_path.clone();
            let this = self.clone();
            this.execute_smoke_checks(smoke_path);
            return;
        }
        if self.options.screenshot.is_some() {
            let delay = self.options.quit_after_ms.unwrap_or(80);
            let win = self.window.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                win.close();
            });
        }
    }

    fn capture_window_png(&self, output_path: &Path) -> Result<(), String> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let width = self.window.width().max(1);
        let height = self.window.height().max(1);
        let paintable = gtk4::WidgetPaintable::new(Some(&self.window));
        let snapshot = gtk4::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
        let node = snapshot
            .to_node()
            .ok_or_else(|| "窗口渲染节点为空".to_string())?;
        let renderer = gtk4::prelude::NativeExt::renderer(&self.window)
            .ok_or_else(|| "窗口 GskRenderer 不可用".to_string())?;
        let bounds = gtk4::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
        let texture = renderer.render_texture(&node, Some(&bounds));
        texture
            .save_to_png(output_path)
            .map_err(|e| format!("保存 PNG 失败：{e}"))?;
        Ok(())
    }

    fn execute_smoke_checks(&self, smoke_path: PathBuf) {
        // 阶段 1：收集宽屏与标签/大纲/顶栏/编辑态/移动面板检查，再切到窄屏测 AdwBreakpoint。
        let (entry_count, note_count, note_paths) = self.vault_tree.stats();
        let (vault_root, scan_complete, scan_micros, startup_ms) = {
            let st = self.state.borrow();
            (
                st.vault_root.clone().unwrap_or_default(),
                st.scan_complete,
                st.scan_micros,
                st.started_at.elapsed().as_millis() as u64,
            )
        };

        let mut open_tabs = Vec::new();
        for idx in 0..self.tab_view.n_pages() {
            let page = self.tab_view.nth_page(idx);
            if let Some(tab) = self.state.borrow().tabs.values().find(|t| t.page == page) {
                open_tabs.push(SmokeTab {
                    path: tab.path.clone(),
                    title: page.title().to_string(),
                    char_count: tab.buffer.char_count(),
                });
            }
        }

        let active_note = self.active_tab_path();
        let outline_headings: Vec<SmokeHeading> = active_note
            .as_ref()
            .and_then(|p| {
                self.state.borrow().tabs.get(p).map(|t| {
                    t.headings
                        .iter()
                        .map(|h| SmokeHeading {
                            level: h.level,
                            title: h.title.clone(),
                            start: h.start,
                        })
                        .collect()
                })
            })
            .unwrap_or_default();

        // 测试大纲点击定位（选最后一个标题确保滚动产生偏移）
        let (outline_scroll_before, outline_scroll_after, outline_scroll_moved) =
            if let Some(ref active) = active_note {
                let before = self
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .map(|t| t.scrolled.vadjustment().value())
                    .unwrap_or(0.0);
                if let Some(last_heading) = outline_headings.last() {
                    self.scroll_to_utf16_offset(active, last_heading.start);
                }
                let after = self
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .map(|t| t.scrolled.vadjustment().value())
                    .unwrap_or(0.0);
                (before, after, after > before)
            } else {
                (0.0, 0.0, false)
            };

        // 测试 AdwTabOverview 开关
        self.tab_overview.set_open(true);
        let tab_overview_opened = self.tab_overview.is_open();
        let tab_overview_listed_count = self.tab_view.n_pages() as usize;
        self.tab_overview.set_open(false);

        // 测试 AdwToolbarView 顶栏显隐
        self.toolbar_view.set_reveal_top_bars(false);
        let top_bar_hidden_ok = !self.toolbar_view.reveals_top_bars();
        self.toolbar_view.set_reveal_top_bars(true);
        let top_bar_restored_ok = self.toolbar_view.reveals_top_bars();

        // 测试 Ctrl+E 只读 <-> 内存编辑态切换与内存输入（不落盘）
        self.toggle_active_tab_edit_mode();
        let edit_mode_toggled_on = active_note
            .as_ref()
            .and_then(|p| {
                self.state
                    .borrow()
                    .tabs
                    .get(p)
                    .map(|t| t.editing && t.text_view.is_editable())
            })
            .unwrap_or(false);
        let edit_buffer_modified_in_memory = if let Some(ref active) = active_note {
            let st = self.state.borrow();
            if let Some(tab) = st.tabs.get(active) {
                let before_chars = tab.buffer.char_count();
                let mut end_iter = tab.buffer.end_iter();
                tab.buffer.insert(&mut end_iter, "\n[内存输入法原型测试]");
                tab.buffer.char_count() > before_chars
            } else {
                false
            }
        } else {
            false
        };
        self.toggle_active_tab_edit_mode();
        let edit_mode_toggled_off = active_note
            .as_ref()
            .and_then(|p| {
                self.state
                    .borrow()
                    .tabs
                    .get(p)
                    .map(|t| !t.editing && !t.text_view.is_editable())
            })
            .unwrap_or(false);

        // 测试占位面板在右侧栏与底部面板间移动
        self.toggle_placeholder_region(PlaceholderKind::Backlinks);
        let moved_to_bottom = self
            .state
            .borrow()
            .placement
            .region_of(PlaceholderKind::Backlinks)
            == PanelRegion::Bottom;
        self.toggle_placeholder_region(PlaceholderKind::Backlinks);
        let moved_back_to_right = self
            .state
            .borrow()
            .placement
            .region_of(PlaceholderKind::Backlinks)
            == PanelRegion::Right;
        let panel_move_verified = moved_to_bottom && moved_back_to_right;

        let wide_width = self.window.width();
        let wide_left_collapsed = self.left_split.is_collapsed();
        let wide_right_collapsed = self.right_split.is_collapsed();

        // 阶段 2：将窗口缩窄到 680×800，等待 Wayland 配置生效并检查 AdwBreakpoint。
        self.window.set_resizable(false);
        self.window.set_size_request(680, 800);
        self.window.set_default_size(680, 800);
        self.window.queue_resize();

        let this = self.clone();
        let elapsed_ms = Rc::new(Cell::new(0u32));
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let waited = elapsed_ms.get() + 50;
            elapsed_ms.set(waited);
            let cur_width = this.window.width();
            if (cur_width < 820 && this.left_split.is_collapsed()) || waited >= 2500 {
                let report = SmokeReport {
                    vault_root: vault_root.clone(),
                    scan_complete,
                    scan_micros,
                    entry_count,
                    note_count,
                    note_paths: note_paths.clone(),
                    open_tabs: open_tabs.clone(),
                    active_note: active_note.clone(),
                    outline_headings: outline_headings.clone(),
                    outline_scroll_moved,
                    outline_scroll_before,
                    outline_scroll_after,
                    tab_overview_opened,
                    tab_overview_listed_count,
                    wide_width,
                    wide_left_collapsed,
                    wide_right_collapsed,
                    narrow_width: cur_width,
                    narrow_left_collapsed: this.left_split.is_collapsed(),
                    narrow_right_collapsed: this.right_split.is_collapsed(),
                    top_bar_hidden_ok,
                    top_bar_restored_ok,
                    edit_mode_toggled_on,
                    edit_mode_toggled_off,
                    edit_buffer_modified_in_memory,
                    panel_move_verified,
                    startup_ms,
                };
                if let Some(parent) = smoke_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => {
                        if let Err(err) = std::fs::write(&smoke_path, format!("{json}\n")) {
                            eprintln!("write smoke json failed: {err}");
                        }
                    }
                    Err(err) => eprintln!("serialize smoke json failed: {err}"),
                }
                this.window.close();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

fn apply_color_scheme(choice: ColorSchemeChoice) {
    let style = adw::StyleManager::default();
    style.set_color_scheme(match choice {
        ColorSchemeChoice::System => adw::ColorScheme::Default,
        ColorSchemeChoice::Light => adw::ColorScheme::ForceLight,
        ColorSchemeChoice::Dark => adw::ColorScheme::ForceDark,
    });
}

fn build_primary_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let theme_section = gio::Menu::new();
    let theme_menu = gio::Menu::new();
    theme_menu.append(Some("跟随系统"), Some("win.color-scheme::system"));
    theme_menu.append(Some("浅色模式"), Some("win.color-scheme::light"));
    theme_menu.append(Some("深色模式"), Some("win.color-scheme::dark"));
    theme_section.append_submenu(Some("外观主题"), &theme_menu);
    menu.append_section(None, &theme_section);

    let view_section = gio::Menu::new();
    view_section.append(
        Some("只读 / 内存原型编辑 (Ctrl+E)"),
        Some("win.toggle-edit-mode"),
    );
    view_section.append(
        Some("标签全景网格 (Ctrl+Shift+O)"),
        Some("win.toggle-tab-overview"),
    );
    view_section.append(
        Some("显示 / 隐藏顶栏 (Ctrl+Shift+H)"),
        Some("win.toggle-top-bar"),
    );
    view_section.append(Some("全屏 (F11)"), Some("win.toggle-fullscreen"));
    menu.append_section(None, &view_section);

    let panel_section = gio::Menu::new();
    let move_menu = gio::Menu::new();
    move_menu.append(
        Some("切换「反链」所在区域"),
        Some("win.move-panel::backlinks"),
    );
    move_menu.append(
        Some("切换「改动」所在区域"),
        Some("win.move-panel::changes"),
    );
    move_menu.append(
        Some("切换「终端」所在区域"),
        Some("win.move-panel::terminal"),
    );
    panel_section.append_submenu(Some("移动占位面板区域"), &move_menu);
    menu.append_section(None, &panel_section);

    let app_section = gio::Menu::new();
    app_section.append(Some("关于 Hortus"), Some("win.about"));
    menu.append_section(None, &app_section);

    menu
}

fn build_left_switcher(stack: &gtk4::Stack) -> gtk4::Box {
    let switcher = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    switcher.add_css_class("hortus-sidebar-switcher");
    populate_stack_switcher(
        &switcher,
        stack,
        &[("files", "文件"), ("search", "搜索"), ("tags", "标签")],
    );
    switcher
}

fn clear_box(container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn populate_stack_switcher(
    container: &gtk4::Box,
    stack: &gtk4::Stack,
    items: &[(&'static str, &'static str)],
) {
    let mut group: Option<gtk4::ToggleButton> = None;
    let visible = stack
        .visible_child_name()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let has_visible = items.iter().any(|(id, _)| *id == visible);

    for (idx, (id, title)) in items.iter().enumerate() {
        let btn = gtk4::ToggleButton::with_label(title);
        btn.add_css_class("flat");
        if let Some(ref g) = group {
            btn.set_group(Some(g));
        } else {
            group = Some(btn.clone());
        }
        let active = if has_visible {
            *id == visible
        } else {
            idx == 0
        };
        btn.set_active(active);

        let stack_ref = stack.clone();
        let target_id = (*id).to_string();
        btn.connect_toggled(move |b| {
            if b.is_active() && stack_ref.child_by_name(&target_id).is_some() {
                stack_ref.set_visible_child_name(&target_id);
            }
        });
        container.append(&btn);
    }
}

fn build_placeholder_box(icon: &str, title: &str, description: &str) -> gtk4::Box {
    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    outer.set_valign(gtk4::Align::Center);
    outer.set_halign(gtk4::Align::Center);
    outer.set_vexpand(true);
    outer.set_margin_start(18);
    outer.set_margin_end(18);

    let img = gtk4::Image::from_icon_name(icon);
    img.set_pixel_size(32);
    img.add_css_class("dim-label");

    let title_label = gtk4::Label::new(Some(title));
    title_label.add_css_class("title-4");

    let desc_label = gtk4::Label::new(Some(description));
    desc_label.add_css_class("dim-label");
    desc_label.set_wrap(true);
    desc_label.set_justify(gtk4::Justification::Center);

    outer.append(&img);
    outer.append(&title_label);
    outer.append(&desc_label);
    outer
}

fn build_placeholder_panel(kind: PlaceholderKind) -> gtk4::Box {
    let outer = build_placeholder_box(kind.icon_name(), kind.title(), kind.milestone_note());
    let move_btn = gtk4::Button::builder()
        .label("在右侧栏 / 底部面板间移动")
        .halign(gtk4::Align::Center)
        .action_name("win.move-panel")
        .action_target(&kind.id().to_variant())
        .build();
    move_btn.add_css_class("pill");
    outer.append(&move_btn);
    outer
}
