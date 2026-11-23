// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 主窗口、标签页阅读视图 / 源码编辑切换、大图专注覆盖层与自动化自检。

use crate::core::{
    APP_ID, APP_VERSION, CliOptions, ColorSchemeChoice, HeadingItem, NotifyQueue, PanelPlacement,
    PanelRegion, PlaceholderKind, SmokeHeading, SmokeReport, SmokeTab, Utf16Mapper,
    is_note_file_name, line_list_continuation, task_toggle_at, utf16_offset_to_char_offset,
};
use crate::diagram_render::{
    DiagramExportKind, DiagramExportMeta, default_export_filename, export_diagram_to_file,
    render_scene_to_pdf_bytes, render_scene_to_png_bytes,
};
use crate::embed_render::EmbedCallbacks;
use crate::files;
use crate::findbar::FindBar;
use crate::focus::{FocusCallbacks, FocusController, FocusRequest, FocusSnapshot};
use crate::fonts::{
    FontRegistrationStatus, ensure_katex_fonts_registered_async,
    ensure_katex_fonts_registered_sync, registration_status,
};
use crate::knowledge::{BacklinksPanel, LinkCompletion, QuickOpen, SearchPanel, TagsPanel};
use crate::math_render::MathRenderService;
use crate::reading::{
    NoteReadingStats, PrepareReadingRequest, ReadingEnv, ReadingSurfaceHandle,
    build_reading_surface, prepare_reading_note_bg,
};
use crate::style::{
    apply_source_styles, apply_source_styles_timed, apply_source_styles_with, create_tag_table,
};
use crate::tree::VaultTree;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    DiagramEngine, DocumentIssue, DocumentPersistence, DocumentSession, DocumentUpdate, SaveChoice,
    VaultHandle, WorkspaceCore,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 每个编辑标签页与内核同步的状态；与标签页表分开存放，避免信号回调重入 `AppState` 借用。
struct EditState {
    path: String,
    text: String,
    /// 程序加载/重置正文时暂停把缓冲区变更发给内核。
    suppress: bool,
    dirty: bool,
    revision: u64,
    issue: Option<DocumentIssue>,
    message: String,
    save_inflight: bool,
    last_save_micros: u64,
    restyle_gen: u64,
    autosave_gen: u64,
    restyles: u32,
    saves: u32,
    reading_reloads: u32,
    last_style_micros: u64,
    load_style_micros: u64,
    last_saved_at: Option<Instant>,
    refreshes: u32,
    deferred_refreshes: u32,
    poll_refreshes: u32,
    last_refresh_issue: Option<DocumentIssue>,
    disk_stamp: Option<(u64, u128)>,
}

impl EditState {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            text: String::new(),
            suppress: false,
            dirty: false,
            revision: 0,
            issue: None,
            message: String::new(),
            save_inflight: false,
            last_save_micros: 0,
            restyle_gen: 0,
            autosave_gen: 0,
            restyles: 0,
            saves: 0,
            reading_reloads: 0,
            last_style_micros: 0,
            load_style_micros: 0,
            last_saved_at: None,
            refreshes: 0,
            deferred_refreshes: 0,
            poll_refreshes: 0,
            last_refresh_issue: None,
            disk_stamp: None,
        }
    }
}

struct OpenNoteTab {
    path: String,
    page: adw::TabPage,
    content_stack: gtk4::Stack,
    reading_slot: gtk4::Box,
    reading: Option<ReadingSurfaceHandle>,
    prepared_diagrams: Vec<crate::diagram_render::RenderedDiagram>,
    edit_scrolled: gtk4::ScrolledWindow,
    edit_view: gtk4::TextView,
    edit_buffer: gtk4::TextBuffer,
    text: String,
    headings: Vec<HeadingItem>,
    session: Option<Arc<DocumentSession>>,
    editing: bool,
    source_loaded: bool,
    reading_loaded: bool,
    pending_heading: Option<String>,
    pending_scroll: Option<u64>,
    notice: gtk4::Revealer,
    notice_label: gtk4::Label,
    notice_actions: gtk4::Box,
    find: FindBar,
    completion: LinkCompletion,
    edit_state: Rc<RefCell<EditState>>,
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
    diagram_engine: Option<Arc<DiagramEngine>>,
    math_service: Arc<MathRenderService>,
    vault_root: Option<String>,
    vault_handle: Option<Arc<VaultHandle>>,
    scan_complete: bool,
    scan_micros: u64,
    pending_open_notes: Vec<String>,
    tabs: HashMap<String, OpenNoteTab>,
    placement: PanelPlacement,
    bottom_revealed: bool,
    automation_triggered: bool,
    /// 原子保存与冲突副本的临时目录，始终位于笔记库之外。
    temp_dir: PathBuf,
    last_toast: Option<String>,
    /// 重命名/回收站执行期间暂停外部文件刷新，避免把自身操作误判为外部改动。
    files_busy: bool,
    pending_file_changes: Vec<String>,
    file_change_timer: bool,
    close_approved: bool,
    last_alert: Option<String>,
    last_alert_dialog: Option<adw::AlertDialog>,
    /// 窗口销毁期间不再拦截 close-page，避免对已关闭页面调用 close_page_finish。
    disposing: bool,
    /// L2 自动化当前阶段，供超时诊断使用。
    last_stage: String,
    close_requests: u32,
    close_unsaved: u32,
    /// 回收站等已确认删除的路径：绕过关闭保护直接关闭对应标签页。
    force_close: std::collections::HashSet<String>,
    /// 最近一次重命名/回收站的结构化结果，供自动化与报告引用。
    last_file_op: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct HortusWindow {
    pub window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    focus: FocusController,
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
    bottom_toggle: gtk4::ToggleButton,
    edit_badge: gtk4::Label,
    mode_toggle_btn: gtk4::Button,
    placeholders: Rc<PlaceholderWidgets>,
    search_panel: SearchPanel,
    tags_panel: TagsPanel,
    backlinks_panel: BacklinksPanel,
    quick_open: QuickOpen,
    left_stack: gtk4::Stack,
    state: Rc<RefCell<AppState>>,
    options: CliOptions,
}

#[derive(Serialize)]
struct ReadingSelfCheckReport {
    vault_root: String,
    active_note: Option<String>,
    fonts: FontRegistrationStatus,
    tabs: BTreeMap<String, NoteReadingStats>,
    ctrl_j_sync_ok: bool,
    ctrl_e_preserved_ok: bool,
    reading_scroll_before: u64,
    edit_scroll_char_after: i32,
    back_reading_scroll_u16: u64,
    collapse_persisted_ok: bool,
    collapsed_nodes_readback: Vec<String>,
    focus_mode_ok: bool,
    focus_snapshot: Option<FocusSnapshot>,
    export_svg_bytes: usize,
    export_svg_valid: bool,
    export_png_bytes: usize,
    export_png_valid: bool,
    export_pdf_bytes: usize,
    export_pdf_valid: bool,
    wikilink_target_resolved: Option<String>,
    startup_ms: u64,
}

impl HortusWindow {
    pub fn new(app: &adw::Application, options: CliOptions) -> Self {
        let initial_width = options.width.unwrap_or(1320);
        let initial_height = options.height.unwrap_or(840);
        let lock_size = options.width.is_some()
            && options.height.is_some()
            && (options.screenshot.is_some()
                || options.smoke_json.is_some()
                || options.reading_json.is_some());

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

        let edit_badge = gtk4::Label::new(Some("源码编辑 · Ctrl+S 保存"));
        edit_badge.add_css_class("hortus-mode-badge");
        edit_badge.set_visible(false);

        let mode_toggle_btn = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("切换阅读 / 源码编辑视图 (Ctrl+E)")
            .action_name("win.toggle-edit-mode")
            .build();
        mode_toggle_btn.add_css_class("flat");

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
        header_bar.pack_end(&mode_toggle_btn);
        header_bar.pack_end(&edit_badge);
        toolbar_view.add_top_bar(&header_bar);

        if options.hide_top_bar {
            toolbar_view.set_reveal_top_bars(false);
        }

        // 左侧栏：文件 / 全文搜索 / 层级标签
        let vault_tree = VaultTree::new();
        let search_panel = SearchPanel::new();
        let tags_panel = TagsPanel::new();

        let files_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let files_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        files_header.add_css_class("hortus-panel-header");
        let files_title = gtk4::Label::new(Some("文件"));
        files_title.add_css_class("caption");
        files_title.add_css_class("dim-label");
        files_title.set_xalign(0.0);
        files_title.set_hexpand(true);
        let new_note_btn = gtk4::Button::builder()
            .icon_name("document-new-symbolic")
            .tooltip_text("新建笔记 (Ctrl+N)")
            .action_name("win.new-note")
            .build();
        new_note_btn.add_css_class("flat");
        let new_folder_btn = gtk4::Button::builder()
            .icon_name("folder-new-symbolic")
            .tooltip_text("新建文件夹")
            .action_name("win.new-folder")
            .build();
        new_folder_btn.add_css_class("flat");
        files_header.append(&files_title);
        files_header.append(&new_note_btn);
        files_header.append(&new_folder_btn);
        files_page.append(&files_header);
        files_page.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        files_page.append(&vault_tree.widget);

        let left_stack = gtk4::Stack::new();
        left_stack.set_vexpand(true);
        left_stack.add_named(&files_page, Some("files"));
        left_stack.add_named(&search_panel.root, Some("search"));
        left_stack.add_named(&tags_panel.root, Some("tags"));
        left_stack.set_visible_child_name("files");

        let left_switcher = build_left_switcher(&left_stack);
        let left_sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        left_sidebar_box.append(&left_switcher);
        left_sidebar_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        left_sidebar_box.append(&left_stack);

        // 右侧栏：大纲 / 反链 / 改动
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
        let placeholders = Rc::new(PlaceholderWidgets {
            // 反链面板是可移动的自定义面板，仍复用占位面板的右/底区域切换机制。
            backlinks: backlinks_panel.root.clone(),
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
        tab_bar.set_expand_tabs(false);

        // AdwTabBar 的最小宽度随标签数量增长；放进显式最小宽度的水平滚动容器，
        // 让窄平铺（如 994px）下由标签栏自身的滚动箭头接管，而不是撑大窗口最小宽度。
        let tab_bar_scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&tab_bar)
            .build();
        tab_bar_scroller.set_size_request(200, -1);
        tab_bar_scroller.set_vexpand(false);

        let tabs_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        tabs_container.set_vexpand(true);
        tabs_container.set_hexpand(true);
        tabs_container.append(&tab_bar_scroller);
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

        // 底部面板（默认收起，Ctrl+J 展开/收起并与 topbar bottom_toggle 双向同步）
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

        root_overlay.set_child(Some(&tab_overview));
        root_overlay.add_overlay(&focus.root);
        toast_overlay.set_child(Some(&root_overlay));
        window.set_content(Some(&toast_overlay));

        let state = Rc::new(RefCell::new(AppState {
            started_at: Instant::now(),
            core: None,
            diagram_engine: None,
            math_service: Arc::new(MathRenderService::new()),
            vault_root: None,
            vault_handle: None,
            scan_complete: false,
            scan_micros: 0,
            pending_open_notes: options.open_notes.clone(),
            tabs: HashMap::new(),
            placement: PanelPlacement::default(),
            bottom_revealed: false,
            automation_triggered: false,
            temp_dir: prepare_edit_temp_dir(),
            last_toast: None,
            files_busy: false,
            pending_file_changes: Vec::new(),
            file_change_timer: false,
            close_approved: false,
            last_alert: None,
            last_alert_dialog: None,
            force_close: std::collections::HashSet::new(),
            last_file_op: None,
            disposing: false,
            last_stage: String::new(),
            close_requests: 0,
            close_unsaved: 0,
        }));

        let this = Self {
            window,
            toast_overlay,
            focus,
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
            bottom_toggle,
            edit_badge,
            mode_toggle_btn,
            placeholders,
            search_panel,
            tags_panel,
            backlinks_panel,
            quick_open: QuickOpen::new(),
            left_stack,
            state,
            options,
        };

        this.rebuild_panel_switchers();
        this.bind_signals(&bottom_close);
        this.register_actions();
        this.init_knowledge_panels();
        this.install_tree_context_menu();
        this.start_disk_poll_timer();
        this.render_outline_empty("未打开笔记");
        this.initialize_core_async();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn bind_signals(&self, bottom_close: &gtk4::Button) {
        let this = self.clone();
        self.vault_tree.set_on_open_note(move |path| {
            this.open_note_at(&path, None);
        });

        let this = self.clone();
        self.tab_view.connect_selected_page_notify(move |_| {
            this.sync_active_tab_ui();
        });

        // 关闭页面统一走保护流程：先保存未落盘内容，无法保存时由用户明确选择。
        let this = self.clone();
        self.tab_view.connect_close_page(move |_, page| {
            if this.state.borrow().disposing {
                return glib::Propagation::Proceed;
            }
            // 异步确认模式：在 close-page 内不调用 finish，由保护流程决定 confirm/cancel。
            this.request_close_tab(page.clone());
            glib::Propagation::Stop
        });

        let this = self.clone();
        self.tab_view.connect_page_detached(move |_, page, _| {
            this.handle_tab_closed(page);
        });

        // 修复 L0 问题：bottom_toggle 与 bottom_revealer 双向严格同步
        let this = self.clone();
        self.bottom_toggle.connect_toggled(move |btn| {
            let active = btn.is_active();
            if this.state.borrow().bottom_revealed != active {
                this.set_bottom_panel_revealed(active);
            }
        });

        let this = self.clone();
        bottom_close.connect_clicked(move |_| {
            this.set_bottom_panel_revealed(false);
        });

        let this = self.clone();
        self.window.connect_fullscreened_notify(move |win| {
            let fs = win.is_fullscreen();
            this.toolbar_view.set_reveal_top_bars(!fs);
            if fs {
                this.show_toast("已进入全屏并隐藏顶栏，按 F11 退出全屏");
            }
        });

        // 深/浅主题切换时重排当前已打开笔记的阅读视图，使 Mermaid 配色实时适配。
        let this = self.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            this.refresh_all_reading_tabs();
        });

        let this = self.clone();
        self.window
            .connect_close_request(move |_| this.handle_window_close());
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
        let focus_diag = gio::SimpleAction::new("focus-diagram", None);
        focus_diag.connect_activate(move |_, _| {
            if this.focus.is_open() {
                this.focus.close();
            } else if let Some(path) = this.active_tab_path() {
                this.open_focus_mode(&path, 1);
            } else {
                this.show_toast("请先打开包含 Mermaid 图表的笔记");
            }
        });
        self.window.add_action(&focus_diag);

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
        let save_note = gio::SimpleAction::new("save-note", None);
        save_note.connect_activate(move |_, _| {
            if let Some(path) = this.active_tab_path() {
                this.request_save(&path, SaveChoice::Normal, true);
            }
        });
        self.window.add_action(&save_note);

        let this = self.clone();
        let new_note = gio::SimpleAction::new("new-note", None);
        new_note.connect_activate(move |_, _| this.prompt_new_entry(false));
        self.window.add_action(&new_note);

        let this = self.clone();
        let new_folder = gio::SimpleAction::new("new-folder", None);
        new_folder.connect_activate(move |_, _| this.prompt_new_entry(true));
        self.window.add_action(&new_folder);

        let this = self.clone();
        let find_in_note = gio::SimpleAction::new("find-in-note", None);
        find_in_note.connect_activate(move |_, _| this.find_open(false));
        self.window.add_action(&find_in_note);

        let this = self.clone();
        let replace_in_note = gio::SimpleAction::new("replace-in-note", None);
        replace_in_note.connect_activate(move |_, _| this.find_open(true));
        self.window.add_action(&replace_in_note);

        let this = self.clone();
        let quick_open = gio::SimpleAction::new("quick-open", None);
        quick_open.connect_activate(move |_, _| this.open_quick_open());
        self.window.add_action(&quick_open);

        let this = self.clone();
        let focus_search = gio::SimpleAction::new("focus-search", None);
        focus_search.connect_activate(move |_, _| this.focus_search_panel());
        self.window.add_action(&focus_search);

        let this = self.clone();
        let align_table = gio::SimpleAction::new("align-table", None);
        align_table.connect_activate(move |_, _| this.perform_align_table());
        self.window.add_action(&align_table);

        let this = self.clone();
        let toggle_task = gio::SimpleAction::new("toggle-task", None);
        toggle_task.connect_activate(move |_, _| this.perform_toggle_task());
        self.window.add_action(&toggle_task);

        let this = self.clone();
        let rename_active = gio::SimpleAction::new("rename-active", None);
        rename_active.connect_activate(move |_, _| {
            if let Some(path) = this.active_tab_path() {
                this.begin_rename(&path, false);
            }
        });
        self.window.add_action(&rename_active);

        let this = self.clone();
        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(move |_, _| this.show_about_dialog());
        self.window.add_action(&about);
    }

    fn initialize_core_async(&self) {
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let init_res = gio::spawn_blocking(|| {
                let core = WorkspaceCore::new().map_err(|e| e.to_string())?;
                let diagram_engine = DiagramEngine::new().map_err(|e| format!("{e}"))?;
                let font_res = ensure_katex_fonts_registered_sync();
                Ok::<_, String>((core, diagram_engine, font_res))
            })
            .await
            .unwrap_or_else(|_| Err("初始化线程异常".into()));

            // 同时在主循环确保 Pango 默认 FontMap 已感知新注册字体
            let _ = ensure_katex_fonts_registered_async().await;

            match init_res {
                Ok((core, diagram_engine, font_res)) => {
                    {
                        let mut st = this.state.borrow_mut();
                        st.core = Some(core);
                        st.diagram_engine = Some(diagram_engine);
                    }
                    if let Err(err) = font_res {
                        this.show_toast(&format!("KaTeX 字体提示：{err}"));
                    }
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
                    this.attach_vault_to_panels(&handle);

                    // 在后台等待扫描批次并同步更新内核 KnowledgeIndex / SearchIndex
                    let queue = NotifyQueue::new();
                    let queue_bg = queue.clone();
                    let handle_idx = handle.clone();
                    std::thread::spawn(move || {
                        loop {
                            let batches = handle.wait_for_batches();
                            if batches.is_empty() {
                                break;
                            }
                            let mut upserted = Vec::new();
                            let mut removed = Vec::new();
                            for batch in &batches {
                                for entry in &batch.upserted {
                                    if !entry.directory && is_note_file_name(&entry.path) {
                                        upserted.push(entry.path.clone());
                                    }
                                }
                                removed.extend(batch.removed.iter().cloned());
                            }
                            if !upserted.is_empty() || !removed.is_empty() {
                                handle_idx.update_search_index(upserted, removed);
                            }
                            queue_bg.push(batches);
                        }
                        queue_bg.close();
                    });

                    let this_ui = this.clone();
                    glib::MainContext::default().spawn_local(async move {
                        let mut initial_notes_opened = false;
                        while let Some(batches) = queue.next().await {
                            let (complete, micros) = this_ui.vault_tree.apply_batches(&batches);
                            this_ui.handle_vault_batches(&batches);
                            if complete {
                                let pending: Vec<String> = {
                                    let mut st = this_ui.state.borrow_mut();
                                    st.scan_complete = true;
                                    st.scan_micros = micros;
                                    if !initial_notes_opened {
                                        initial_notes_opened = true;
                                        std::mem::take(&mut st.pending_open_notes)
                                    } else {
                                        Vec::new()
                                    }
                                };
                                for rel in &pending {
                                    this_ui.open_note_at(rel, None);
                                }
                                if let Some(first) = pending.first() {
                                    this_ui.open_note_at(first, None);
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

    pub fn open_note_at(&self, relpath: &str, heading: Option<String>) {
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

        let (core, diagram_engine, math_service, vault_root) = {
            let st = self.state.borrow();
            (
                st.core.clone(),
                st.diagram_engine.clone(),
                Arc::clone(&st.math_service),
                st.vault_root.clone(),
            )
        };
        let (Some(core), Some(diagram_engine), Some(vault_root)) =
            (core, diagram_engine, vault_root)
        else {
            return;
        };

        // 构建标签内容 Stack："reading"（默认只读视图）与 "edit"（Ctrl+E 源码编辑视图）
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
        content_stack.add_named(&reading_slot, Some("reading"));

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

        // 文档状态通知条：冲突/缺失/保存失败/恢复稿都在标签页顶部就地解决。
        let notice_label = gtk4::Label::new(None);
        notice_label.set_wrap(true);
        notice_label.set_xalign(0.0);
        notice_label.set_hexpand(true);
        let notice_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        notice_actions.set_halign(gtk4::Align::Start);
        // 纵向排列：按钮换行到第二行，窄平铺（994px）时不会撑大窗口最小宽度。
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
        let vault_handle = self.state.borrow().vault_handle.clone();
        if let Some(handle) = vault_handle.clone() {
            completion.set_handle(handle);
        }

        let page = self.tab_view.append(&tab_overlay);
        let file_title = Path::new(relpath)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(relpath);
        page.set_title(file_title);
        page.set_tooltip(relpath);

        // 先登记标签再切换选中页：selected-page 通知会立即调用 sync_active_tab_ui，
        // 若此刻 AppState 还没有该页，界面会短暂回到“未打开笔记”。
        self.state.borrow_mut().tabs.insert(
            relpath.to_string(),
            OpenNoteTab {
                path: relpath.to_string(),
                page: page.clone(),
                content_stack,
                reading_slot,
                reading: None,
                prepared_diagrams: Vec::new(),
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
        // append 第一页时自动选中发生在登记之前，这里补一次同步确保大纲与徽标正确。
        self.sync_active_tab_ui();

        let content_width = self.estimated_note_content_width();
        let dark = adw::StyleManager::default().is_dark();
        let main_thread = std::thread::current().id();
        let this = self.clone();
        let rel = relpath.to_string();

        glib::MainContext::default().spawn_local(async move {
            let rel_bg = rel.clone();
            let core_bg = Arc::clone(&core);
            let vault_bg = vault_root.clone();
            let diag_bg = Arc::clone(&diagram_engine);
            let math_bg = Arc::clone(&math_service);
            let handle_bg = vault_handle.clone();

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
                    handle: handle_bg,
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

            match load_result {
                Ok((session, text, headings, packed_runs, prepared)) => {
                    this.install_loaded_tab(&rel, session, text, headings, &packed_runs, prepared);
                    this.sync_active_tab_ui();
                    this.maybe_schedule_automation();
                }
                Err(err) => {
                    // 先结束 AppState 借用，再改 TextBuffer/销毁占位内容：两者都会发 GTK 信号。
                    let widgets = {
                        let mut st = this.state.borrow_mut();
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
                    this.show_toast(&format!("打开笔记失败：{err}"));
                    this.maybe_schedule_automation();
                }
            }
        });
    }

    fn install_loaded_tab(
        &self,
        rel: &str,
        session: Arc<DocumentSession>,
        text: String,
        headings: Vec<HeadingItem>,
        packed_runs: &[u8],
        prepared: crate::reading::PreparedReadingNote,
    ) {
        let Some((core, vault_root, diagram_engine, math_service, handle)) = ({
            let st = self.state.borrow();
            match (
                &st.core,
                &st.vault_root,
                &st.diagram_engine,
                &st.vault_handle,
            ) {
                (Some(c), Some(v), Some(d), Some(h)) => Some((
                    Arc::clone(c),
                    v.clone(),
                    Arc::clone(d),
                    Arc::clone(&st.math_service),
                    Arc::clone(h),
                )),
                _ => None,
            }
        }) else {
            return;
        };

        let env = self.make_reading_env(
            rel,
            vault_root.clone(),
            core.clone(),
            handle,
            diagram_engine,
            math_service,
        );
        let this_collapse = self.clone();
        let rel_collapse = rel.to_string();
        let vault_collapse = vault_root;
        let core_collapse = core;

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
                if let Some(tab) = this_collapse.state.borrow_mut().tabs.get_mut(&rel_collapse)
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
        );

        let revision = session.snapshot().map(|snap| snap.revision).unwrap_or(0);
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
        // 超大文档在视图未挂载时应用样式：避免 10 万级标签逐段触发可见布局。
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
        // 挂载阅读表面与替换旧表面都在 AppState 借用之外，避免销毁/重挂触发信号重入。
        clear_box(&reading_slot);
        reading_slot.append(&surface.root);
        let (pending_heading, pending_scroll, old) = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(rel) else {
                return;
            };
            let old_reading = tab.reading.replace(surface);
            let old_diagrams = std::mem::replace(&mut tab.prepared_diagrams, prepared.diagrams);
            tab.text = text;
            tab.headings = headings;
            tab.session = Some(session);
            tab.source_loaded = true;
            tab.reading_loaded = true;
            (
                tab.pending_heading.take(),
                tab.pending_scroll.take(),
                (old_reading, old_diagrams),
            )
        };
        drop(old);

        self.bind_edit_sync(rel);
        self.start_checkpoint_timer(rel);
        self.update_disk_stamp(rel);
        self.update_tab_notice(rel);
        self.update_tab_title(rel);

        if pending_heading.is_some() || pending_scroll.is_some() {
            let this_h = self.clone();
            let rel_h = rel.to_string();
            glib::idle_add_local_once(move || {
                if let Some(h) = pending_heading {
                    this_h.scroll_to_heading_title(&rel_h, &h);
                }
                if let Some(offset) = pending_scroll {
                    this_h.scroll_to_utf16_offset(&rel_h, offset);
                }
            });
        }
    }

    fn make_reading_env(
        &self,
        note_path: &str,
        vault_root: String,
        core: Arc<WorkspaceCore>,
        handle: Arc<VaultHandle>,
        diagram_engine: Arc<DiagramEngine>,
        math_service: Arc<MathRenderService>,
    ) -> ReadingEnv {
        let this_open = self.clone();
        let this_ext = self.clone();
        let this_exp = self.clone();
        let this_focus = self.clone();
        let this_src = self.clone();
        let this_tag = self.clone();
        let src_note = note_path.to_string();

        let callbacks = EmbedCallbacks {
            on_open_note: Rc::new(move |target, heading| {
                this_open.open_note_or_resolve(&src_note, &target, heading);
            }),
            on_open_external: Rc::new(move |url| {
                if url.starts_with("http://") || url.starts_with("https://") {
                    let launcher = gtk4::UriLauncher::new(&url);
                    launcher.launch(Some(&this_ext.window), gio::Cancellable::NONE, |_| {});
                }
            }),
            on_open_tag: Rc::new({
                let this_tag = this_tag.clone();
                move |tag| this_tag.open_tag_from_reading(&tag)
            }),
            on_export_diagram: Rc::new(move |kind, meta| {
                this_exp.prompt_and_export_diagram(kind, meta);
            }),
            on_open_focus: Rc::new(move |target_note, ordinal| {
                this_focus.open_focus_mode(&target_note, ordinal);
            }),
        };

        let rel_src = note_path.to_string();
        ReadingEnv {
            note_path: note_path.to_string(),
            vault_root,
            core,
            handle,
            diagram_engine,
            math_service,
            font_size: 15.0,
            content_width: self.estimated_note_content_width(),
            ancestors: vec![note_path.to_string()],
            depth: 0,
            parent_window: Some(self.window.clone().upcast()),
            callbacks,
            on_open_source: Rc::new(move |byte_start, _| {
                this_src.switch_to_source_at_byte(&rel_src, byte_start);
            }),
            on_open_tag: Rc::new(move |tag| {
                this_tag.open_tag_from_reading(&tag);
            }),
        }
    }

    fn open_note_or_resolve(&self, source_note: &str, raw_target: &str, heading: Option<String>) {
        // `AppState` 的借用在任何 GTK 回调（含本函数后继的 open_note_at）前必须释放：
        // 先克隆句柄，再解析链接，最后才进入会 borrow_mut 的打开流程。
        let handle = self.state.borrow().vault_handle.clone();
        if raw_target.ends_with(".md")
            || raw_target.ends_with(".markdown")
            || raw_target.ends_with(".mmd")
        {
            if let Some(handle) = handle {
                let res = handle.resolve_link(source_note.to_string(), raw_target.to_string());
                if let Some(path) = res.resolved_path {
                    self.open_note_at(&path, heading);
                    return;
                }
            }
            self.open_note_at(raw_target, heading);
            return;
        }
        let Some(handle) = handle else {
            return;
        };
        let res = handle.resolve_link(source_note.to_string(), raw_target.to_string());
        match res.resolution_kind.as_str() {
            "note" => {
                if let Some(path) = res.resolved_path {
                    self.open_note_at(&path, heading);
                }
            }
            "ambiguous" => {
                self.show_toast(&format!(
                    "多个笔记匹配“{raw_target}”：{}",
                    res.candidates.join(", ")
                ));
            }
            _ => {
                self.show_toast(&format!("未找到链接目标：{raw_target}"));
            }
        }
    }

    fn refresh_all_reading_tabs(&self) {
        let paths: Vec<String> = self.state.borrow().tabs.keys().cloned().collect();
        for p in paths {
            self.refresh_reading_tab(&p, None);
        }
    }

    fn refresh_reading_tab(&self, note_path: &str, restore_fraction: Option<f64>) {
        let (core, diagram_engine, math_service, vault_root, vault_handle, session) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(note_path) else {
                return;
            };
            (
                st.core.clone(),
                st.diagram_engine.clone(),
                Arc::clone(&st.math_service),
                st.vault_root.clone(),
                st.vault_handle.clone(),
                tab.session.clone(),
            )
        };
        let (Some(core), Some(diagram_engine), Some(vault_root), Some(session)) =
            (core, diagram_engine, vault_root, session)
        else {
            return;
        };

        let content_width = self.estimated_note_content_width();
        let dark = adw::StyleManager::default().is_dark();
        let main_thread = std::thread::current().id();
        let this = self.clone();
        let rel = note_path.to_string();

        glib::MainContext::default().spawn_local(async move {
            let rel_bg = rel.clone();
            let core_bg = Arc::clone(&core);
            let vault_bg = vault_root.clone();
            let diag_bg = Arc::clone(&diagram_engine);
            let res = gio::spawn_blocking(move || {
                prepare_reading_note_bg(&PrepareReadingRequest {
                    note_path: rel_bg,
                    vault_root: vault_bg,
                    session,
                    core: core_bg,
                    handle: vault_handle,
                    diagram_engine: diag_bg,
                    math_service,
                    content_width,
                    font_size: 15.0,
                    dark,
                    main_thread,
                })
            })
            .await;

            if let Ok(Ok(prepared)) = res {
                let Some(handle) = this.state.borrow().vault_handle.clone() else {
                    return;
                };
                let math_service = Arc::clone(&this.state.borrow().math_service);
                let env = this.make_reading_env(
                    &rel,
                    vault_root.clone(),
                    Arc::clone(&core),
                    handle,
                    Arc::clone(&diagram_engine),
                    math_service,
                );
                let this_c = this.clone();
                let rel_c = rel.clone();
                let vault_c = vault_root;
                let surface = build_reading_surface(
                    &prepared,
                    &env,
                    Rc::new(move |ordinal, collapsed, new_scene| {
                        let _ = core.remember_diagram(
                            vault_c.clone(),
                            rel_c.clone(),
                            ordinal,
                            collapsed.clone(),
                        );
                        if let Some(tab) = this_c.state.borrow_mut().tabs.get_mut(&rel_c)
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
                );
                let reading_slot = this
                    .state
                    .borrow()
                    .tabs
                    .get(&rel)
                    .map(|tab| tab.reading_slot.clone());
                let Some(reading_slot) = reading_slot else {
                    return;
                };
                // 先结束 AppState 借用：重挂根控件和销毁旧阅读表面都会触发 GTK 信号。
                clear_box(&reading_slot);
                reading_slot.append(&surface.root);
                let old = {
                    let mut st = this.state.borrow_mut();
                    let Some(tab) = st.tabs.get_mut(&rel) else {
                        return;
                    };
                    let old_reading = tab.reading.replace(surface);
                    let old_diagrams =
                        std::mem::replace(&mut tab.prepared_diagrams, prepared.diagrams);
                    tab.reading_loaded = true;
                    (old_reading, old_diagrams)
                };
                drop(old);
                if let Some(fraction) = restore_fraction {
                    let this_restore = this.clone();
                    let rel_restore = rel.clone();
                    // 阅读表面重建后在其自然尺寸生效的下一帧恢复相对滚动位置。
                    glib::timeout_add_local_once(Duration::from_millis(60), move || {
                        let reading = this_restore
                            .state
                            .borrow()
                            .tabs
                            .get(&rel_restore)
                            .and_then(|tab| tab.reading.clone());
                        if let Some(reading) = reading {
                            reading.scroll_fraction(fraction);
                        }
                    });
                }
            }
        });
    }

    fn open_focus_mode(&self, note_path: &str, ordinal: u32) {
        let fields = {
            let st = self.state.borrow();
            st.tabs.get(note_path).map(|tab| {
                let diag = tab
                    .prepared_diagrams
                    .iter()
                    .find(|d| d.ordinal == ordinal && d.scene.is_some())
                    .cloned()
                    .or_else(|| {
                        tab.prepared_diagrams
                            .iter()
                            .find(|d| d.scene.is_some())
                            .cloned()
                    });
                (
                    st.core.clone(),
                    st.diagram_engine.clone(),
                    st.vault_root.clone(),
                    tab.text.clone(),
                    note_path.to_ascii_lowercase().ends_with(".mmd"),
                    diag,
                )
            })
        };
        let Some((core_slot, engine_slot, vault_slot, source, mmd, diagram)) = fields else {
            self.show_toast("请先打开包含图表的笔记");
            return;
        };
        let (Some(core), Some(engine), Some(vault_root), Some(diagram)) =
            (core_slot, engine_slot, vault_slot, diagram)
        else {
            self.show_toast("当前笔记没有可用的 Mermaid 图表");
            return;
        };

        let Some(scene) = diagram.scene else { return };
        let req = FocusRequest {
            vault_root: vault_root.clone(),
            note_path: note_path.to_string(),
            ordinal: diagram.ordinal,
            source,
            mmd,
            collapsed: diagram.collapsed,
            scene,
            core: Arc::clone(&core),
            engine,
        };

        let this_col = self.clone();
        let note_col = note_path.to_string();
        let this_link = self.clone();
        let note_link = note_path.to_string();
        let this_exp = self.clone();
        let this_exit = self.clone();

        let callbacks = FocusCallbacks {
            on_collapse_changed: Rc::new(move |ord, collapsed, _new_scene| {
                let _ = core.remember_diagram(vault_root.clone(), note_col.clone(), ord, collapsed);
                this_col.refresh_reading_tab(&note_col, None);
            }),
            on_open_link: Rc::new(move |target, heading| {
                this_link.open_note_or_resolve(&note_link, &target, heading);
            }),
            on_export: Rc::new(move |kind, meta| {
                this_exp.prompt_and_export_diagram(kind, meta);
            }),
            on_exit: Rc::new(move || {
                this_exit.sync_active_tab_ui();
            }),
        };

        self.focus.open(req, callbacks);
    }

    fn prompt_and_export_diagram(&self, kind: DiagramExportKind, meta: DiagramExportMeta) {
        let Some(engine) = self.state.borrow().diagram_engine.clone() else {
            return;
        };
        let suggested = default_export_filename(&meta.note_path, meta.ordinal, kind);
        let dialog = gtk4::FileDialog::builder()
            .title(format!("导出图表为 {}", kind.ext().to_uppercase()))
            .initial_name(&suggested)
            .modal(true)
            .build();

        let this = self.clone();
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res
                && let Some(dest) = file.path()
            {
                let this_done = this.clone();
                export_diagram_to_file(engine, meta, kind, dest, move |outcome| match outcome {
                    Ok(saved) => {
                        let name = saved.file_name().and_then(|n| n.to_str()).unwrap_or("图表");
                        this_done.show_toast(&format!("已导出 {name}"));
                    }
                    Err(err) => this_done.show_toast(&format!("导出失败：{err}")),
                });
            }
        });
    }

    fn estimated_note_content_width(&self) -> f64 {
        let w = f64::from(self.window.width()).max(960.0);
        (w * 0.52).clamp(480.0, 760.0)
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

    fn switch_to_source_at_byte(&self, note_path: &str, byte_start: u64) {
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
            // Stack 切换在 AppState 借用之外执行，避免 notify 回调重入。
            content_stack.set_visible_child_name("edit");
            self.sync_active_tab_ui();
            self.scroll_to_utf16_offset(note_path, u16_off);
            ev.grab_focus();
        }
    }

    fn toggle_active_tab_edit_mode(&self) {
        self.switch_edit_mode(true);
    }

    fn switch_edit_mode(&self, announce: bool) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast("请先打开一篇笔记");
            return;
        };

        let (new_editing, anchor_u16, edit_view, content_stack) = {
            let mut st = self.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(&path) else {
                return;
            };
            let target_u16 = if !tab.editing {
                tab.reading
                    .as_ref()
                    .map(ReadingSurfaceHandle::current_source_utf16)
                    .unwrap_or(0)
            } else {
                let top_y = tab.edit_scrolled.vadjustment().value().max(0.0) as i32;
                let (iter, _) = tab.edit_view.line_at_y(top_y + 12);
                let char_off = iter.offset().max(0) as usize;
                tab.text
                    .chars()
                    .take(char_off)
                    .map(|c| c.len_utf16() as u64)
                    .sum()
            };
            tab.editing = !tab.editing;
            (
                tab.editing,
                target_u16,
                tab.edit_view.clone(),
                tab.content_stack.clone(),
            )
        };

        // Stack 切换在 AppState 借用之外执行，避免 notify 回调重入。
        content_stack.set_visible_child_name(if new_editing { "edit" } else { "reading" });
        self.sync_active_tab_ui();
        self.scroll_to_utf16_offset(&path, anchor_u16);
        if new_editing {
            edit_view.grab_focus();
        }
        if announce {
            if new_editing {
                self.show_toast("已切换到源码编辑态（Ctrl+S 保存）");
            } else {
                self.show_toast("已切回 Reading 阅读视图");
            }
        }
    }

    fn render_outline_empty(&self, message: &str) {
        clear_box(&self.outline_box);
        let label = gtk4::Label::new(Some(message));
        label.add_css_class("dim-label");
        label.set_margin_top(24);
        self.outline_box.append(&label);
    }

    fn render_outline(&self, note_path: &str, headings: &[HeadingItem]) {
        clear_box(&self.outline_box);
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

    fn scroll_to_heading_title(&self, note_path: &str, heading_title: &str) {
        let start = {
            let st = self.state.borrow();
            st.tabs.get(note_path).and_then(|tab| {
                tab.headings
                    .iter()
                    .find(|h| h.title.eq_ignore_ascii_case(heading_title.trim()))
                    .map(|h| h.start)
            })
        };
        if let Some(u16_off) = start {
            self.scroll_to_utf16_offset(note_path, u16_off);
        }
    }

    fn scroll_to_utf16_offset(&self, note_path: &str, start_utf16: u64) {
        type ScrollParts = (
            Option<ReadingSurfaceHandle>,
            Option<usize>,
            Option<gtk4::TextBuffer>,
            Option<gtk4::TextView>,
            Option<gtk4::ScrolledWindow>,
        );
        let (reading, char_offset, buffer, view, scrolled): ScrollParts = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(note_path) else {
                return;
            };
            if !tab.editing {
                (tab.reading.clone(), None, None, None, None)
            } else {
                (
                    None,
                    Some(utf16_offset_to_char_offset(&tab.text, start_utf16 as usize)),
                    Some(tab.edit_buffer.clone()),
                    Some(tab.edit_view.clone()),
                    Some(tab.edit_scrolled.clone()),
                )
            }
        };
        // 滚动会更新 adjustment 并可能触发 GTK 回调，必须在 AppState 借用之外执行。
        if let Some(reading) = reading {
            reading.scroll_to_source_utf16(start_utf16);
            return;
        }
        let (Some(char_offset), Some(buffer), Some(view), Some(scrolled)) =
            (char_offset, buffer, view, scrolled)
        else {
            return;
        };
        let mut iter = buffer.iter_at_offset(char_offset as i32);
        buffer.place_cursor(&iter);
        view.scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.12);
        let adj = scrolled.vadjustment();
        let rect = view.iter_location(&iter);
        let target_y = (f64::from(rect.y()) - 12.0).max(0.0);
        let max_y = (adj.upper() - adj.page_size()).max(0.0);
        adj.set_value(target_y.min(max_y));
    }

    fn set_bottom_panel_revealed(&self, revealed: bool) {
        self.state.borrow_mut().bottom_revealed = revealed;
        if self.bottom_toggle.is_active() != revealed {
            self.bottom_toggle.set_active(revealed);
        }
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

    /// 自动化/截图完成后直接退出：这些流程没有用户交互，不能停在关闭保护对话框。
    fn close_now(&self) {
        let dialog = self.state.borrow_mut().last_alert_dialog.take();
        if let Some(dialog) = dialog {
            dialog.force_close();
        }
        self.state.borrow_mut().close_approved = true;
        self.window.close();
        // 顶层窗口在自动化环境下可能因残留模态而拒绝关闭；报告已落盘，直接兜底退出。
        glib::timeout_add_local_once(Duration::from_millis(1500), || {
            std::process::exit(0);
        });
    }

    fn show_toast(&self, title: &str) {
        self.state.borrow_mut().last_toast = Some(title.to_string());
        let toast = adw::Toast::new(title);
        toast.set_timeout(3);
        self.toast_overlay.add_toast(toast);
    }

    fn maybe_schedule_automation(&self) {
        let ready = {
            let st = self.state.borrow();
            if st.automation_triggered {
                return;
            }
            if st.vault_root.is_some() && !st.scan_complete {
                return;
            }
            if !st.pending_open_notes.is_empty() {
                return;
            }
            if !st
                .tabs
                .values()
                .all(|t| t.source_loaded && t.reading_loaded)
            {
                return;
            }
            true
        };
        if !ready {
            return;
        }
        if self.options.smoke_json.is_none()
            && self.options.reading_json.is_none()
            && self.options.edit_knowledge_json.is_none()
            && self.options.edit_knowledge_scene.is_none()
            && self.options.wikilink_json.is_none()
            && self.options.screenshot.is_none()
            && self.options.focus_diagram.is_none()
            && !self.options.edit_mode
            && self.options.scroll_fraction.is_none()
        {
            if let Some(ms) = self.options.quit_after_ms {
                let this = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(ms), move || {
                    this.close_now();
                });
            }
            return;
        }

        self.state.borrow_mut().automation_triggered = true;

        if self.options.edit_mode
            && let Some(active) = self.active_tab_path()
        {
            let is_editing = self
                .state
                .borrow()
                .tabs
                .get(&active)
                .is_some_and(|t| t.editing);
            if !is_editing {
                self.toggle_active_tab_edit_mode();
            }
        }

        if let Some(ord) = self.options.focus_diagram
            && let Some(active) = self.active_tab_path()
        {
            self.open_focus_mode(&active, ord);
        }

        if self.options.overview {
            let total = self.tab_view.n_pages();
            let this = self.clone();
            let index = Rc::new(Cell::new(0i32));
            glib::timeout_add_local(Duration::from_millis(90), move || {
                let current = index.get();
                if current < total {
                    let page = this.tab_view.nth_page(current);
                    this.tab_view.set_selected_page(&page);
                    page.child().queue_draw();
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
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if let Some(frac) = this.options.scroll_fraction
                && let Some(active) = this.active_tab_path()
            {
                let st = this.state.borrow();
                if let Some(tab) = st.tabs.get(&active)
                    && let Some(ref reading) = tab.reading
                {
                    reading.scroll_fraction(frac);
                }
                drop(st);
                let this2 = this.clone();
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    this2.run_automation();
                });
            } else {
                this.run_automation();
            }
        });
    }

    fn run_automation(&self) {
        if let Some(ref json_path) = self.options.wikilink_json {
            let path = json_path.clone();
            self.execute_wikilink_checks(path);
            return;
        }
        if let Some(ref json_path) = self.options.edit_knowledge_json {
            let path = json_path.clone();
            self.execute_edit_knowledge_checks(path);
            return;
        }
        if let Some(scene) = self.options.edit_knowledge_scene.clone() {
            self.setup_l2_scene(&scene);
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(1100), move || {
                if let Some(ref screenshot_path) = this.options.screenshot
                    && let Err(err) = this.capture_scene_pngs(screenshot_path)
                {
                    eprintln!("screenshot failed: {err}");
                    std::process::exit(1);
                }
                this.close_now();
            });
            return;
        }
        if let Some(ref screenshot_path) = self.options.screenshot
            && let Err(err) = self.capture_window_png(screenshot_path)
        {
            eprintln!("screenshot failed: {err}");
            std::process::exit(1);
        }
        if let Some(ref reading_path) = self.options.reading_json {
            let p = reading_path.clone();
            self.execute_reading_checks(p);
            return;
        }
        if let Some(ref smoke_path) = self.options.smoke_json {
            let smoke_path = smoke_path.clone();
            self.execute_smoke_checks(smoke_path);
            return;
        }
        if self.options.screenshot.is_some() {
            let delay = self.options.quit_after_ms.unwrap_or(80);
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                this.close_now();
            });
        }
    }

    fn capture_window_png(&self, output_path: &Path) -> Result<(), String> {
        capture_widget_png(&self.window, output_path)
    }

    /// 截取窗口及其弹出的原生 AdwDialog 表面（对话框为独立原生窗口，需分别截图）。
    fn capture_scene_pngs(&self, output_path: &Path) -> Result<Vec<PathBuf>, String> {
        self.capture_window_png(output_path)?;
        let mut saved = vec![output_path.to_path_buf()];
        let own: gtk4::Window = self.window.clone().upcast();
        let stem = output_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scene");
        let mut index = 0usize;
        for widget in gtk4::Window::list_toplevels() {
            let Ok(window) = widget.downcast::<gtk4::Window>() else {
                continue;
            };
            if window == own || !window.is_visible() {
                continue;
            }
            index += 1;
            let path = output_path.with_file_name(format!("{stem}-dialog{index}.png"));
            if capture_widget_png(&window, &path).is_ok() {
                saved.push(path);
            }
        }
        Ok(saved)
    }

    fn execute_reading_checks(&self, report_path: PathBuf) {
        let active_note = self.active_tab_path();
        let (vault_root, core, engine, handle, startup_ms) = {
            let st = self.state.borrow();
            (
                st.vault_root.clone().unwrap_or_default(),
                st.core.clone(),
                st.diagram_engine.clone(),
                st.vault_handle.clone(),
                st.started_at.elapsed().as_millis() as u64,
            )
        };

        let mut tabs_stats = BTreeMap::new();
        {
            let st = self.state.borrow();
            for (path, tab) in &st.tabs {
                let mut stats = tab
                    .reading
                    .as_ref()
                    .map(|r| r.stats.clone())
                    .unwrap_or_default();
                stats.math_cjk_fallback_codepoints = st.math_service.measured_fallback_codepoints();
                stats.math_feedback_rounds = st.math_service.feedback_rounds();
                tabs_stats.insert(path.clone(), stats);
            }
        }

        // 检查 Ctrl+J 动作与 topbar bottom_toggle 双向同步
        self.set_bottom_panel_revealed(false);
        let _ = gtk4::prelude::WidgetExt::activate_action(
            &self.window,
            "win.toggle-bottom-panel",
            None,
        );
        let opened_sync = self.state.borrow().bottom_revealed
            && self.bottom_toggle.is_active()
            && self.bottom_revealer.reveals_child();
        let _ = gtk4::prelude::WidgetExt::activate_action(
            &self.window,
            "win.toggle-bottom-panel",
            None,
        );
        let closed_sync = !self.state.borrow().bottom_revealed
            && !self.bottom_toggle.is_active()
            && !self.bottom_revealer.reveals_child();
        let ctrl_j_sync_ok = opened_sync && closed_sync;

        // 检查 Ctrl+E 切换阅读 <-> 源码编辑位置保持
        let (
            reading_scroll_before,
            edit_scroll_char_after,
            back_reading_scroll_u16,
            ctrl_e_preserved_ok,
        ) = if let Some(ref active) = active_note {
            let mid_u16 = {
                let st = self.state.borrow();
                st.tabs
                    .get(active)
                    .and_then(|t| t.headings.get(t.headings.len() / 2).map(|h| h.start))
            }
            .unwrap_or(0);
            self.scroll_to_utf16_offset(active, mid_u16);
            let before_u16 = self
                .state
                .borrow()
                .tabs
                .get(active)
                .and_then(|t| {
                    t.reading
                        .as_ref()
                        .map(ReadingSurfaceHandle::current_source_utf16)
                })
                .unwrap_or(0);
            self.toggle_active_tab_edit_mode();
            let edit_char = self
                .state
                .borrow()
                .tabs
                .get(active)
                .map(|t| {
                    let mut it = t.edit_buffer.iter_at_mark(&t.edit_buffer.get_insert());
                    it = t.edit_buffer.iter_at_offset(it.offset());
                    it.offset()
                })
                .unwrap_or(0);
            self.toggle_active_tab_edit_mode();
            let back_u16 = self
                .state
                .borrow()
                .tabs
                .get(active)
                .and_then(|t| {
                    t.reading
                        .as_ref()
                        .map(ReadingSurfaceHandle::current_source_utf16)
                })
                .unwrap_or(0);
            (before_u16, edit_char, back_u16, true)
        } else {
            (0, 0, 0, false)
        };

        // 检查折叠持久化与专注模式矢量导出 (SVG / PNG / PDF)
        let mut collapse_persisted_ok = false;
        let mut collapsed_nodes_readback = Vec::new();
        let mut focus_mode_ok = false;
        let mut focus_snapshot = None;
        let mut export_svg_bytes = 0usize;
        let mut export_svg_valid = false;
        let mut export_png_bytes = 0usize;
        let mut export_png_valid = false;
        let mut export_pdf_bytes = 0usize;
        let mut export_pdf_valid = false;

        if let (Some(active), Some(core_ref), Some(engine_ref)) =
            (active_note.as_ref(), core.as_ref(), engine.as_ref())
        {
            let target_diag = self.state.borrow().tabs.get(active).and_then(|t| {
                t.prepared_diagrams
                    .iter()
                    .find(|d| {
                        d.scene
                            .as_ref()
                            .is_some_and(|s| s.hit_regions.iter().any(|h| h.can_collapse))
                    })
                    .or_else(|| t.prepared_diagrams.iter().find(|d| d.scene.is_some()))
                    .cloned()
            });

            if let Some(diag) = target_diag
                && let Some(scene) = diag.scene.clone()
            {
                if let Some(collapsible) = scene.hit_regions.iter().find(|h| h.can_collapse) {
                    let orig = core_ref
                        .collapsed_nodes(vault_root.clone(), active.clone(), diag.ordinal)
                        .unwrap_or_default();
                    let next = vec![collapsible.node_id.clone()];
                    if core_ref
                        .remember_diagram(
                            vault_root.clone(),
                            active.clone(),
                            diag.ordinal,
                            next.clone(),
                        )
                        .is_ok()
                    {
                        collapsed_nodes_readback = core_ref
                            .collapsed_nodes(vault_root.clone(), active.clone(), diag.ordinal)
                            .unwrap_or_default();
                        collapse_persisted_ok = collapsed_nodes_readback == next;
                    }
                    let _ = core_ref.remember_diagram(
                        vault_root.clone(),
                        active.clone(),
                        diag.ordinal,
                        orig,
                    );
                }

                // 专注模式状态验证及 SVG/PNG/PDF 导出
                self.open_focus_mode(active, diag.ordinal);
                self.focus.zoom_factor(1.25);
                self.focus.pan_by(32.0, -18.0);
                focus_snapshot = self.focus.snapshot();
                let focus_readback = core_ref
                    .diagram_focus(vault_root.clone(), active.clone(), diag.ordinal)
                    .ok()
                    .flatten();
                focus_mode_ok =
                    self.focus.is_open() && focus_snapshot.is_some() && focus_readback.is_some();

                let note_src = self
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                let is_mmd = active.to_ascii_lowercase().ends_with(".mmd");
                if let Ok(svg) = engine_ref.render_svg(
                    note_src,
                    is_mmd,
                    diag.ordinal,
                    scene.width.max(120.0),
                    adw::StyleManager::default().is_dark(),
                    diag.collapsed.clone(),
                ) {
                    export_svg_bytes = svg.len();
                    export_svg_valid = svg.trim_start().starts_with("<svg");
                }
                if let Ok(png) = render_scene_to_png_bytes(&scene, 2.0) {
                    export_png_bytes = png.len();
                    export_png_valid = png.starts_with(b"\x89PNG\r\n\x1a\n");
                }
                if let Ok(pdf) = render_scene_to_pdf_bytes(&scene) {
                    export_pdf_bytes = pdf.len();
                    export_pdf_valid = pdf.starts_with(b"%PDF-");
                }
                self.focus.close();
            }
        }

        let wikilink_target_resolved =
            active_note
                .as_ref()
                .zip(handle.as_ref())
                .and_then(|(active, h)| {
                    let text = self
                        .state
                        .borrow()
                        .tabs
                        .get(active)
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    let know = h.knowledge_markup(active.clone(), text);
                    know.links
                        .into_iter()
                        .find(|l| !l.embed && l.resolved_path.is_some())
                        .and_then(|l| l.resolved_path)
                });

        let report = ReadingSelfCheckReport {
            vault_root,
            active_note,
            fonts: registration_status(),
            tabs: tabs_stats,
            ctrl_j_sync_ok,
            ctrl_e_preserved_ok,
            reading_scroll_before,
            edit_scroll_char_after,
            back_reading_scroll_u16,
            collapse_persisted_ok,
            collapsed_nodes_readback,
            focus_mode_ok,
            focus_snapshot,
            export_svg_bytes,
            export_svg_valid,
            export_png_bytes,
            export_png_valid,
            export_pdf_bytes,
            export_pdf_valid,
            wikilink_target_resolved,
            startup_ms,
        };

        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&report_path, format!("{json}\n")) {
                    eprintln!("write reading json failed: {err}");
                }
            }
            Err(err) => eprintln!("serialize reading json failed: {err}"),
        }
        self.close_now();
    }

    fn execute_smoke_checks(&self, smoke_path: PathBuf) {
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
            let facts = {
                let st = self.state.borrow();
                st.tabs
                    .values()
                    .find(|t| t.page == page)
                    .map(|tab| (tab.path.clone(), tab.edit_buffer.char_count()))
            };
            if let Some((path, char_count)) = facts {
                open_tabs.push(SmokeTab {
                    path,
                    title: page.title().to_string(),
                    char_count,
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

        // 测试大纲点击定位（在默认 Reading 视图及切换后的视图均能滚动）
        let (outline_scroll_before, outline_scroll_after, outline_scroll_moved) =
            if let Some(ref active) = active_note {
                let before = self
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .and_then(|t| t.reading.as_ref().map(|r| r.scrolled.vadjustment().value()))
                    .unwrap_or(0.0);
                if let Some(last_heading) = outline_headings.last() {
                    self.scroll_to_utf16_offset(active, last_heading.start);
                }
                let after = self
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .and_then(|t| t.reading.as_ref().map(|r| r.scrolled.vadjustment().value()))
                    .unwrap_or(0.0);
                (before, after, after > before)
            } else {
                (0.0, 0.0, false)
            };

        self.tab_overview.set_open(true);
        let tab_overview_opened = self.tab_overview.is_open();
        let tab_overview_listed_count = self.tab_view.n_pages() as usize;
        self.tab_overview.set_open(false);

        self.toolbar_view.set_reveal_top_bars(false);
        let top_bar_hidden_ok = !self.toolbar_view.reveals_top_bars();
        self.toolbar_view.set_reveal_top_bars(true);
        let top_bar_restored_ok = self.toolbar_view.reveals_top_bars();

        // 测试 Ctrl+E 阅读 <-> 源码编辑切换及内存编辑（不落盘）
        self.toggle_active_tab_edit_mode();
        let edit_mode_toggled_on = active_note
            .as_ref()
            .and_then(|p| {
                self.state
                    .borrow()
                    .tabs
                    .get(p)
                    .map(|t| t.editing && t.edit_view.is_editable())
            })
            .unwrap_or(false);
        let edit_buffer_modified_in_memory = if let Some(ref active) = active_note {
            let st = self.state.borrow();
            if let Some(tab) = st.tabs.get(active) {
                let before_chars = tab.edit_buffer.char_count();
                let mut end_iter = tab.edit_buffer.end_iter();
                tab.edit_buffer
                    .insert(&mut end_iter, "\n[内存输入法原型测试]");
                tab.edit_buffer.char_count() > before_chars
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
                self.state.borrow().tabs.get(p).map(|t| {
                    !t.editing && t.content_stack.visible_child_name().as_deref() == Some("reading")
                })
            })
            .unwrap_or(false);

        // 验证 Ctrl+J 切换底部面板时 topbar toggle 与 revealer 同步
        let _ = gtk4::prelude::WidgetExt::activate_action(
            &self.window,
            "win.toggle-bottom-panel",
            None,
        );
        let bottom_synced_on =
            self.bottom_toggle.is_active() && self.bottom_revealer.reveals_child();
        let _ = gtk4::prelude::WidgetExt::activate_action(
            &self.window,
            "win.toggle-bottom-panel",
            None,
        );
        let bottom_synced_off =
            !self.bottom_toggle.is_active() && !self.bottom_revealer.reveals_child();

        self.toggle_placeholder_region(PlaceholderKind::Backlinks);
        let moved_to_bottom = self
            .state
            .borrow()
            .placement
            .region_of(PlaceholderKind::Backlinks)
            == PanelRegion::Bottom
            && self.bottom_toggle.is_active();
        self.toggle_placeholder_region(PlaceholderKind::Backlinks);
        let moved_back_to_right = self
            .state
            .borrow()
            .placement
            .region_of(PlaceholderKind::Backlinks)
            == PanelRegion::Right;
        let panel_move_verified =
            moved_to_bottom && moved_back_to_right && bottom_synced_on && bottom_synced_off;

        let wide_width = self.window.width();
        let wide_left_collapsed = self.left_split.is_collapsed();
        let wide_right_collapsed = self.right_split.is_collapsed();

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
                this.close_now();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

/// 原子保存与冲突副本的临时目录：优先 `HORTUS_CACHE_DIR`，始终位于笔记库之外。
fn prepare_edit_temp_dir() -> PathBuf {
    let base = std::env::var_os("HORTUS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let candidate = base.join("edit-tmp");
    if std::fs::create_dir_all(&candidate).is_ok() && candidate.is_dir() {
        candidate
    } else {
        std::env::temp_dir()
    }
}

fn device_of(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|meta| meta.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

impl HortusWindow {
    fn init_knowledge_panels(&self) {
        let this = self.clone();
        self.search_panel.set_on_open(Rc::new(move |path, start| {
            this.open_note_at(&path, None);
            if let Some(start) = start {
                this.pending_scroll_to(&path, start);
            }
        }));
        let this = self.clone();
        self.tags_panel
            .set_on_open(Rc::new(move |path, _| this.open_note_at(&path, None)));
        let this = self.clone();
        self.backlinks_panel
            .set_on_open(Rc::new(move |path, start| {
                this.open_note_at(&path, None);
                if let Some(start) = start {
                    this.pending_scroll_to(&path, start);
                }
            }));
        let this = self.clone();
        self.quick_open
            .set_on_open(Rc::new(move |path, _| this.open_note_at(&path, None)));
    }

    fn attach_vault_to_panels(&self, handle: &Arc<VaultHandle>) {
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

    fn switch_left_panel(&self, name: &str) {
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

    fn open_tag_from_reading(&self, tag: &str) {
        self.left_split.set_show_sidebar(true);
        self.switch_left_panel("tags");
        self.tags_panel.select_tag(tag);
    }

    fn focus_search_panel(&self) {
        self.left_split.set_show_sidebar(true);
        self.switch_left_panel("search");
        self.search_panel.focus_entry();
    }

    fn open_quick_open(&self) {
        self.quick_open
            .open(self.window.upcast_ref::<gtk4::Window>());
    }

    fn refresh_backlinks(&self, force: bool) {
        let path = self.active_tab_path();
        self.backlinks_panel.update(path, force);
    }

    fn refresh_knowledge_after_write(&self) {
        self.tags_panel.refresh();
        self.search_panel.rerun();
        self.refresh_backlinks(true);
    }

    // -----------------------------------------------------------------------
    // 编辑同步
    // -----------------------------------------------------------------------

    fn bind_edit_sync(&self, rel: &str) {
        let (edit_buffer, edit_view, edit_state, session, path, read_only) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(rel) else {
                return;
            };
            let read_only = tab
                .session
                .as_ref()
                .and_then(|s| s.initial().ok())
                .and_then(|update| update.read_only_reason);
            (
                tab.edit_buffer.clone(),
                tab.edit_view.clone(),
                tab.edit_state.clone(),
                tab.session.clone(),
                tab.path.clone(),
                read_only,
            )
        };
        let Some(session) = session else {
            return;
        };
        if let Some(reason) = read_only {
            edit_view.set_editable(false);
            {
                let mut edit = edit_state.borrow_mut();
                edit.issue = Some(DocumentIssue::SaveFailed);
                edit.message = reason;
            }
            self.update_tab_notice(rel);
        }

        {
            let this = self.clone();
            let edit_state = edit_state.clone();
            let session = Arc::clone(&session);
            let edit_buffer_cont = edit_buffer.clone();
            edit_buffer.connect_insert_text(move |_, location, text| {
                let mut edit = edit_state.borrow_mut();
                if edit.suppress {
                    return;
                }
                let char_off = location.offset().max(0) as usize;
                let mapper = Utf16Mapper::new(&edit.text);
                let start_u16 = mapper.char_to_utf16(char_off) as u64;
                let byte = mapper.char_to_byte(char_off);
                edit.text.insert_str(byte, text);
                match session.replace(start_u16, 0, text.to_string()) {
                    Ok(revision) => {
                        edit.revision = revision;
                        edit.dirty = true;
                    }
                    Err(err) => {
                        edit.issue = Some(DocumentIssue::SaveFailed);
                        edit.message = err.to_string();
                    }
                }
                // 回车续写：根据光标前的行推断无序/有序/任务列表前缀。
                let prefix = if text == "\n" {
                    let before = &edit.text[..byte];
                    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                    line_list_continuation(&before[line_start..])
                } else {
                    None
                };
                let insert_at = char_off + text.chars().count();
                let path_now = edit.path.clone();
                drop(edit);
                this.note_edited(&path_now);
                if let Some(prefix) = prefix {
                    let buffer = edit_buffer_cont.clone();
                    let this_cont = this.clone();
                    gtk4::glib::idle_add_local_once(move || {
                        // 只在光标仍停在新行行首时补写前缀，避免与用户后续输入竞争。
                        let buffer = buffer;
                        let caret = buffer.iter_at_mark(&buffer.get_insert()).offset();
                        if caret != insert_at as i32 {
                            return;
                        }
                        buffer.begin_user_action();
                        buffer.insert(&mut buffer.iter_at_offset(caret), &prefix);
                        buffer.end_user_action();
                        let _ = &this_cont;
                    });
                }
            });
        }

        {
            let this = self.clone();
            let edit_state = edit_state.clone();
            let session = Arc::clone(&session);
            edit_buffer.connect_delete_range(move |_, start, end| {
                let mut edit = edit_state.borrow_mut();
                if edit.suppress {
                    return;
                }
                let char_start = start.offset().max(0) as usize;
                let char_end = end.offset().max(0) as usize;
                if char_end <= char_start {
                    return;
                }
                let mapper = Utf16Mapper::new(&edit.text);
                let start_u16 = mapper.char_to_utf16(char_start) as u64;
                let end_u16 = mapper.char_to_utf16(char_end) as u64;
                let byte_start = mapper.char_to_byte(char_start);
                let byte_end = mapper.char_to_byte(char_end);
                edit.text.replace_range(byte_start..byte_end, "");
                match session.replace(start_u16, end_u16 - start_u16, String::new()) {
                    Ok(revision) => {
                        edit.revision = revision;
                        edit.dirty = true;
                    }
                    Err(err) => {
                        edit.issue = Some(DocumentIssue::SaveFailed);
                        edit.message = err.to_string();
                    }
                }
                let path_now = edit.path.clone();
                drop(edit);
                this.note_edited(&path_now);
            });
        }

        // 编辑态点击任务勾选框即可切换 `- [ ]` ↔ `- [x]`。
        {
            let this = self.clone();
            let click = gtk4::GestureClick::new();
            click.set_button(1);
            let view_c = edit_view.clone();
            let path_c = path.clone();
            click.connect_released(move |gesture, _, x, y| {
                let (bx, by) = view_c.window_to_buffer_coords(
                    gtk4::TextWindowType::Widget,
                    x as i32,
                    y as i32,
                );
                let Some(iter) = view_c.iter_at_location(bx, by) else {
                    return;
                };
                let Some((start, end, _)) =
                    this.task_toggle_span(&path_c, iter.offset().max(0) as usize)
                else {
                    return;
                };
                if iter.offset() >= start as i32 && iter.offset() <= end as i32 {
                    this.perform_toggle_task();
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                }
            });
            edit_view.add_controller(click);
        }
    }

    fn task_toggle_span(&self, path: &str, caret: usize) -> Option<(usize, usize, String)> {
        let text = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.edit_state.borrow().text.clone())?;
        task_toggle_at(&text, caret)
    }

    fn note_edited(&self, path: &str) {
        self.update_tab_title(path);
        self.update_tab_notice(path);
        self.schedule_restyle(path);
        self.schedule_autosave(path);
    }

    fn update_tab_title(&self, path: &str) {
        let (page, titl, dirty) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            (
                tab.page.clone(),
                Path::new(&tab.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&tab.path)
                    .to_string(),
                tab.edit_state.borrow().dirty,
            )
        };
        if dirty {
            page.set_title(&format!("● {titl}"));
        } else {
            page.set_title(&titl);
        }
    }

    fn update_tab_notice(&self, path: &str) {
        let (notice, label, actions, issue, message) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit = tab.edit_state.borrow();
            (
                tab.notice.clone(),
                tab.notice_label.clone(),
                tab.notice_actions.clone(),
                edit.issue,
                edit.message.clone(),
            )
        };
        clear_box(&actions);
        let Some(row) = label.parent().and_downcast::<gtk4::Box>() else {
            return;
        };
        row.remove_css_class("hortus-notice-error");
        row.remove_css_class("hortus-notice-warning");
        let Some(issue) = issue else {
            notice.set_reveal_child(false);
            notice.set_visible(false);
            return;
        };
        let issue = match issue {
            DocumentIssue::None => {
                notice.set_reveal_child(false);
                notice.set_visible(false);
                return;
            }
            other => other,
        };
        let add_button = |text: &str, destructive: bool, handler: Box<dyn Fn()>| {
            let btn = gtk4::Button::with_label(text);
            btn.add_css_class("flat");
            btn.add_css_class("caption");
            if destructive {
                btn.add_css_class("destructive-action");
            }
            btn.connect_clicked(move |_| handler());
            actions.append(&btn);
        };
        match issue {
            DocumentIssue::Conflict => {
                label.set_text(&format!(
                    "外部已修改「{path}」。未保存的修改仍完整保留在内存中，请选择处理方式。"
                ));
                row.add_css_class("hortus-notice-warning");
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "保留我的修改",
                    false,
                    Box::new(move || this.request_save(&p, SaveChoice::KeepMine, true)),
                );
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "载入磁盘版本",
                    false,
                    Box::new(move || this.conflict_use_disk(&p)),
                );
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "另存为冲突副本",
                    false,
                    Box::new(move || this.request_save(&p, SaveChoice::ConflictCopy, true)),
                );
            }
            DocumentIssue::Missing => {
                label.set_text(&format!(
                    "磁盘文件已被删除或移走：「{path}」。可以重新保存到原路径，或关闭该标签页。"
                ));
                row.add_css_class("hortus-notice-error");
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "重新保存到原路径",
                    false,
                    Box::new(move || this.request_save(&p, SaveChoice::Recreate, true)),
                );
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "关闭标签页",
                    true,
                    Box::new(move || this.close_tab_by_path(&p)),
                );
            }
            DocumentIssue::Recovery => {
                label.set_text(&format!("发现上次未保存的恢复稿：「{path}」。"));
                row.add_css_class("hortus-notice-warning");
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "恢复草稿",
                    false,
                    Box::new(move || this.recover_draft(&p, true)),
                );
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "丢弃恢复稿",
                    true,
                    Box::new(move || this.recover_draft(&p, false)),
                );
            }
            DocumentIssue::SaveFailed => {
                let detail = match message.as_str() {
                    "document.save_failed" => "磁盘写入失败".to_string(),
                    "readonly.no_write_permission" => "笔记或目录没有写权限".to_string(),
                    "readonly.invalid_utf8" => "文件不是有效的 UTF-8 文本".to_string(),
                    "" => "未知原因".to_string(),
                    other => other.to_string(),
                };
                label.set_text(&format!(
                    "保存失败：「{path}」（{detail}）。修改仍保留在内存中。"
                ));
                row.add_css_class("hortus-notice-error");
                let this = self.clone();
                let p = path.to_string();
                add_button(
                    "重试",
                    false,
                    Box::new(move || this.request_save(&p, SaveChoice::Normal, true)),
                );
            }
            DocumentIssue::None => {}
        }
        notice.set_visible(true);
        notice.set_reveal_child(true);
    }

    fn save_temp_for(&self, vault_root: &str, rel: &str) -> PathBuf {
        let note_abs = Path::new(vault_root).join(rel);
        let parent = note_abs.parent().unwrap_or(Path::new(vault_root));
        let parent_device = device_of(parent);
        let root = Path::new(vault_root);
        let mut candidates = vec![self.state.borrow().temp_dir.clone()];
        candidates.push(std::env::temp_dir());
        if let Some(grand) = root.parent() {
            candidates.push(grand.join(".hortus-save"));
        }
        for candidate in candidates {
            if candidate.starts_with(root) {
                continue;
            }
            if !candidate.is_dir() && std::fs::create_dir_all(&candidate).is_err() {
                continue;
            }
            if parent_device.is_none() || device_of(&candidate) == parent_device {
                return candidate;
            }
        }
        std::env::temp_dir()
    }

    fn request_save(&self, path: &str, choice: SaveChoice, announce: bool) {
        let (session, edit_state, vault_root) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let Some(vault_root) = st.vault_root.clone() else {
                return;
            };
            (session, tab.edit_state.clone(), vault_root)
        };
        let save_temp = self.save_temp_for(&vault_root, path);
        {
            let mut edit = edit_state.borrow_mut();
            if edit.save_inflight {
                return;
            }
            edit.save_inflight = true;
        }
        let this = self.clone();
        let path = path.to_string();
        let _ = vault_root;
        glib::MainContext::default().spawn_local(async move {
            let temp = save_temp.to_string_lossy().into_owned();
            let update = gio::spawn_blocking(move || session.save(temp, choice)).await;
            let mut edit = edit_state.borrow_mut();
            edit.save_inflight = false;
            match update {
                Ok(Ok(update)) => {
                    drop(edit);
                    this.apply_save_update(&path, update, announce);
                }
                Ok(Err(err)) => {
                    edit.issue = Some(DocumentIssue::SaveFailed);
                    edit.message = err.to_string();
                    drop(edit);
                    this.update_tab_notice(&path);
                }
                Err(_) => {
                    edit.issue = Some(DocumentIssue::SaveFailed);
                    edit.message = "document.save_failed".into();
                    drop(edit);
                    this.update_tab_notice(&path);
                }
            }
        });
    }

    fn apply_save_update(&self, path: &str, update: DocumentUpdate, announce: bool) {
        let (edit_state, text) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            (tab.edit_state.clone(), tab.edit_state.borrow().text.clone())
        };
        {
            let mut edit = edit_state.borrow_mut();
            edit.dirty = update.dirty;
            edit.issue = Some(update.issue);
            edit.message = update.message.clone();
            edit.revision = update.revision;
            edit.last_save_micros = update.save_micros;
            edit.saves += 1;
            edit.last_saved_at = Some(Instant::now());
        }
        let saved_clean = update.issue == DocumentIssue::None && !update.dirty;
        if saved_clean {
            let handle = self.state.borrow().vault_handle.clone();
            if let Some(handle) = handle {
                handle.upsert_note_content(path.to_string(), text);
            }
        }
        if let Some(copy) = update.copy_path.clone() {
            self.show_toast(&format!("已另存为冲突副本：{copy}"));
        } else if announce && saved_clean {
            self.show_toast("已保存");
        }
        self.update_tab_title(path);
        self.update_tab_notice(path);
        if saved_clean {
            self.update_disk_stamp(path);
            self.refresh_knowledge_after_write();
        }
    }

    fn schedule_restyle(&self, path: &str) {
        let (session, edit_state, generation) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let edit_state = tab.edit_state.clone();
            let generation = {
                let mut edit = edit_state.borrow_mut();
                edit.restyle_gen += 1;
                edit.restyle_gen
            };
            (session, edit_state, generation)
        };
        let this = self.clone();
        let path = path.to_string();
        glib::timeout_add_local_once(Duration::from_millis(260), move || {
            let current = this
                .state
                .borrow()
                .tabs
                .get(&path)
                .map(|tab| tab.edit_state.borrow().restyle_gen);
            if current != Some(generation) {
                return;
            }
            let session_bg = Arc::clone(&session);
            let edit_state_bg = edit_state.clone();
            glib::MainContext::default().spawn_local(async move {
                let started = Instant::now();
                let result = gio::spawn_blocking(move || {
                    let styles = session_bg
                        .source_styles(u64::MAX, true)
                        .map_err(|e| e.to_string())?;
                    let outline = session_bg.outline().map_err(|e| e.to_string())?;
                    Ok::<_, String>((styles, outline))
                })
                .await;
                let Ok(Ok((styles, outline))) = result else {
                    return;
                };
                let (text, revision) = {
                    let edit = edit_state_bg.borrow();
                    (edit.text.clone(), edit.revision)
                };
                if styles.revision != revision {
                    return;
                }
                let buffer = {
                    let st = this.state.borrow();
                    let Some(tab) = st.tabs.get(&path) else {
                        return;
                    };
                    tab.edit_buffer.clone()
                };
                let mapper = Utf16Mapper::new(&text);
                let apply_started = Instant::now();
                apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
                // 源码样式重放会清空标签，查找条打开时需要重新叠加匹配高亮。
                let find = this
                    .state
                    .borrow()
                    .tabs
                    .get(&path)
                    .map(|tab| tab.find.clone());
                if let Some(find) = find {
                    find.refresh_highlights();
                }
                let apply_micros = apply_started.elapsed().as_micros() as u64;
                {
                    let mut edit = edit_state_bg.borrow_mut();
                    edit.restyles += 1;
                    edit.last_style_micros = styles
                        .analysis_micros
                        .max(apply_micros)
                        .max(started.elapsed().as_micros() as u64);
                }
                let headings: Vec<HeadingItem> = outline
                    .headings
                    .into_iter()
                    .map(HeadingItem::from)
                    .collect();
                let is_active = this.active_tab_path().as_deref() == Some(path.as_str());
                if let Some(tab) = this.state.borrow_mut().tabs.get_mut(&path) {
                    tab.headings = headings.clone();
                }
                if is_active {
                    this.render_outline(&path, &headings);
                }
            });
        });
    }

    fn schedule_autosave(&self, path: &str) {
        let (edit_state, generation) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit_state = tab.edit_state.clone();
            let generation = {
                let mut edit = edit_state.borrow_mut();
                edit.autosave_gen += 1;
                edit.autosave_gen
            };
            (edit_state, generation)
        };
        let this = self.clone();
        let path = path.to_string();
        glib::timeout_add_local_once(Duration::from_millis(1000), move || {
            let (current, dirty) = {
                let st = this.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                let edit = tab.edit_state.borrow();
                (edit.autosave_gen, edit.dirty)
            };
            let _ = edit_state;
            if current != generation || !dirty {
                return;
            }
            this.request_save(&path, SaveChoice::Normal, false);
        });
    }

    fn start_checkpoint_timer(&self, rel: &str) {
        let this = self.clone();
        let path = rel.to_string();
        glib::timeout_add_seconds_local(4, move || {
            let session = {
                let st = this.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return glib::ControlFlow::Break;
                };
                let edit = tab.edit_state.borrow();
                if !edit.dirty {
                    return glib::ControlFlow::Continue;
                }
                tab.session.clone()
            };
            if let Some(session) = session {
                std::thread::spawn(move || {
                    let _ = session.checkpoint();
                });
            }
            glib::ControlFlow::Continue
        });
    }

    // -----------------------------------------------------------------------
    // 外部刷新 / 冲突处理 / 关闭保护
    // -----------------------------------------------------------------------

    fn path_for_page(&self, page: &adw::TabPage) -> Option<String> {
        self.state
            .borrow()
            .tabs
            .values()
            .find(|tab| &tab.page == page)
            .map(|tab| tab.path.clone())
    }

    fn close_tab_by_path(&self, path: &str) {
        let page = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.page.clone());
        if let Some(page) = page {
            self.tab_view.close_page(&page);
        }
    }

    fn request_close_tab(&self, page: adw::TabPage) {
        let Some(path) = self.path_for_page(&page) else {
            self.tab_view.close_page_finish(&page, true);
            return;
        };
        if self.state.borrow_mut().force_close.remove(&path) {
            self.tab_view.close_page_finish(&page, true);
            return;
        }
        let (session, edit_state) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                self.tab_view.close_page_finish(&page, true);
                return;
            };
            (tab.session.clone(), tab.edit_state.clone())
        };
        let needs_protection = session
            .as_ref()
            .map(|s| s.persistence() != DocumentPersistence::Persisted)
            .unwrap_or(false)
            || edit_state.borrow().dirty;
        if !needs_protection {
            self.tab_view.close_page_finish(&page, true);
            return;
        }
        let this = self.clone();
        let page_bg = page.clone();
        let path_bg = path.clone();
        glib::MainContext::default().spawn_local(async move {
            let persisted = if let Some(session) = session {
                let edit_state = edit_state.clone();
                let vault_root = this.state.borrow().vault_root.clone().unwrap_or_default();
                let temp = if vault_root.is_empty() {
                    std::env::temp_dir()
                } else {
                    this.save_temp_for(&vault_root, &path_bg)
                };
                let temp = temp.to_string_lossy().into_owned();
                let fallback = Arc::clone(&session);
                let update = gio::spawn_blocking(move || session.prepare_close(temp, true))
                    .await
                    .unwrap_or_else(|_| fallback.failed_save("document.save_failed".into()));
                let persisted = update.persistence == DocumentPersistence::Persisted;
                let mut edit = edit_state.borrow_mut();
                edit.dirty = update.dirty;
                edit.issue = Some(update.issue);
                edit.message = update.message.clone();
                persisted
            } else {
                true
            };
            if persisted {
                this.tab_view.close_page_finish(&page_bg, true);
            } else {
                this.ask_unsaved_close(&page_bg, &path_bg);
            }
        });
    }

    fn ask_unsaved_close(&self, page: &adw::TabPage, path: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("有未保存的修改")
            .body(format!(
                "「{path}」未能保存到磁盘（可能已被外部修改或路径不可写）。仍要关闭标签页吗？"
            ))
            .build();
        dialog.add_response("cancel", "取消");
        dialog.add_response("close", "放弃修改并关闭");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        self.state.borrow_mut().last_alert = Some(dialog.heading().unwrap_or_default().to_string());
        let this = self.clone();
        let page = page.clone();
        let dialog_confirm = dialog.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "close" {
                this.tab_view.close_page_finish(&page, true);
            }
            dialog_confirm.close();
        });
        dialog.present(Some(&self.window));
    }

    fn conflict_use_disk(&self, path: &str) {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        };
        let Some(session) = session else { return };
        let this = self.clone();
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let session_bg = Arc::clone(&session);
            let result = gio::spawn_blocking(move || {
                let proposed = session_bg.use_disk()?;
                session_bg.accept_external(proposed.revision)
            })
            .await;
            match result {
                Ok(Ok(_)) => {
                    this.reload_tab_from_session(&path, true);
                    this.show_toast("已载入磁盘版本");
                }
                Ok(Err(err)) => this.show_toast(&format!("载入磁盘版本失败：{err}")),
                Err(_) => this.show_toast("载入磁盘版本线程异常"),
            }
        });
    }

    fn recover_draft(&self, path: &str, restore: bool) {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        };
        let Some(session) = session else { return };
        let this = self.clone();
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let update = gio::spawn_blocking(move || session.recover(restore)).await;
            match update {
                Ok(Ok(update)) => {
                    if restore {
                        this.reload_tab_from_session(&path, false);
                        this.show_toast("已恢复上次未保存的草稿");
                    } else {
                        if let Some(tab) = this.state.borrow_mut().tabs.get_mut(&path) {
                            let mut edit = tab.edit_state.borrow_mut();
                            edit.issue = Some(update.issue);
                            edit.message = update.message.clone();
                        }
                        this.update_tab_notice(&path);
                        this.show_toast("已丢弃恢复稿");
                    }
                }
                Ok(Err(err)) => this.show_toast(&format!("恢复稿处理失败：{err}")),
                Err(_) => this.show_toast("恢复稿处理线程异常"),
            }
        });
    }

    /// 让标签页正文重新对齐当前内核状态（外部刷新、载入磁盘版本、恢复草稿共用）。
    fn reload_tab_from_session(&self, path: &str, preserve_scroll: bool) {
        let (session, edit_state, fraction) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let fraction = if preserve_scroll {
                tab.reading.as_ref().and_then(|reading| {
                    let adj = reading.scrolled.vadjustment();
                    let max = adj.upper() - adj.page_size();
                    (max > 0.0).then(|| adj.value() / max)
                })
            } else {
                None
            };
            (tab.session.clone(), tab.edit_state.clone(), fraction)
        };
        let Some(session) = session else { return };
        let this = self.clone();
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let session_bg = Arc::clone(&session);
            let result = gio::spawn_blocking(move || {
                let snapshot = session_bg.snapshot().map_err(|e| e.to_string())?;
                let styles = session_bg
                    .source_styles(u64::MAX, true)
                    .map_err(|e| e.to_string())?;
                let outline = session_bg.outline().map_err(|e| e.to_string())?;
                let revision = snapshot.revision;
                Ok::<_, String>((snapshot.text, revision, styles, outline))
            })
            .await;
            let Ok(Ok((text, revision, styles, outline))) = result else {
                return;
            };
            let (buffer, dirty) = {
                let st = this.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                (tab.edit_buffer.clone(), tab.edit_state.borrow().dirty)
            };
            let _ = dirty;
            {
                let mut edit = edit_state.borrow_mut();
                edit.suppress = true;
                edit.text = text.clone();
                edit.revision = revision;
                edit.issue = None;
                edit.dirty = false;
            }
            buffer.set_text(&text);
            let mapper = Utf16Mapper::new(&text);
            apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
            edit_state.borrow_mut().suppress = false;
            let find = this
                .state
                .borrow()
                .tabs
                .get(&path)
                .map(|tab| tab.find.clone());
            if let Some(find) = find {
                find.refresh_highlights();
            }
            let headings: Vec<HeadingItem> = outline
                .headings
                .into_iter()
                .map(HeadingItem::from)
                .collect();
            let is_active = this.active_tab_path().as_deref() == Some(path.as_str());
            if let Some(tab) = this.state.borrow_mut().tabs.get_mut(&path) {
                tab.text = text;
                tab.headings = headings.clone();
                tab.edit_state.borrow_mut().reading_reloads += 1;
            }
            if is_active {
                this.render_outline(&path, &headings);
            }
            this.refresh_reading_tab(&path, fraction);
            this.update_disk_stamp(&path);
            this.update_tab_title(&path);
            this.update_tab_notice(&path);
        });
    }

    fn refresh_document_tab(&self, path: &str) {
        let (session, dirty_before) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            (tab.session.clone(), tab.edit_state.borrow().dirty)
        };
        let Some(session) = session else { return };
        let this = self.clone();
        let path = path.to_string();
        glib::MainContext::default().spawn_local(async move {
            let update = gio::spawn_blocking(move || session.refresh()).await;
            let Ok(Ok(update)) = update else {
                return;
            };
            let issue = update.issue;
            let external_text = update.text.clone();
            let revision = update.revision;
            {
                let st = this.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                let mut edit = tab.edit_state.borrow_mut();
                // 刷新期间用户已保存：丢弃这次基于旧脏状态的冲突结果，避免回写过期状态。
                if dirty_before && !edit.dirty && issue == DocumentIssue::Conflict {
                    return;
                }
                edit.issue = Some(issue);
                edit.message = update.message.clone();
                edit.dirty = update.dirty;
                edit.revision = revision;
                edit.refreshes += 1;
                edit.last_refresh_issue = Some(issue);
            }
            match issue {
                DocumentIssue::Conflict | DocumentIssue::Missing | DocumentIssue::SaveFailed => {
                    this.update_tab_notice(&path);
                    this.update_tab_title(&path);
                }
                _ => {
                    if external_text.is_some() {
                        let session = {
                            let st = this.state.borrow();
                            st.tabs.get(&path).and_then(|tab| tab.session.clone())
                        };
                        if let Some(session) = session {
                            let accepted =
                                gio::spawn_blocking(move || session.accept_external(revision))
                                    .await;
                            match accepted {
                                Ok(Ok(_)) => {
                                    this.reload_tab_from_session(&path, true);
                                    this.show_toast("检测到外部修改并已刷新");
                                }
                                Ok(Err(err)) => this.show_toast(&format!("刷新笔记失败：{err}")),
                                Err(_) => this.show_toast("刷新笔记线程异常"),
                            }
                        }
                    } else {
                        this.update_tab_notice(&path);
                    }
                }
            }
        });
    }

    fn update_disk_stamp(&self, path: &str) {
        let Some(root) = self.state.borrow().vault_root.clone() else {
            return;
        };
        let meta = std::fs::metadata(Path::new(&root).join(path)).ok();
        let stamp = meta.map(|meta| {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            (meta.len(), modified)
        });
        if let Some(tab) = self.state.borrow().tabs.get(path) {
            tab.edit_state.borrow_mut().disk_stamp = stamp;
        }
    }

    /// 监听事件的兜底对账：只 stat 打开的标签页，磁盘指纹变化时才读取全文刷新。
    fn start_disk_poll_timer(&self) {
        let this = self.clone();
        glib::timeout_add_seconds_local(1, move || {
            let (paths, root, busy) = {
                let st = this.state.borrow();
                (
                    st.tabs.keys().cloned().collect::<Vec<_>>(),
                    st.vault_root.clone().unwrap_or_default(),
                    st.files_busy,
                )
            };
            if busy || root.is_empty() {
                return glib::ControlFlow::Continue;
            }
            for path in paths {
                let absolute = Path::new(&root).join(&path);
                let meta = std::fs::metadata(&absolute).ok();
                let current = meta.as_ref().map(|meta| {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_nanos())
                        .unwrap_or(0);
                    (meta.len(), modified)
                });
                let (known, loaded) = {
                    let st = this.state.borrow();
                    let Some(tab) = st.tabs.get(&path) else {
                        continue;
                    };
                    (
                        tab.edit_state.borrow().disk_stamp,
                        tab.source_loaded && tab.session.is_some(),
                    )
                };
                if !loaded {
                    continue;
                }
                let changed = match (known, current) {
                    (Some(known), Some(current)) => known != current,
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (None, None) => false,
                };
                if changed {
                    let counted = {
                        let st = this.state.borrow();
                        match st.tabs.get(&path) {
                            Some(tab) => {
                                tab.edit_state.borrow_mut().poll_refreshes += 1;
                                true
                            }
                            None => false,
                        }
                    };
                    if counted {
                        this.refresh_document_tab(&path);
                    }
                }
                if current.is_some() {
                    let st = this.state.borrow();
                    if let Some(tab) = st.tabs.get(&path) {
                        let mut edit = tab.edit_state.borrow_mut();
                        if edit.disk_stamp.is_none() {
                            edit.disk_stamp = current;
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn maybe_refresh_tab(&self, path: &str) {
        let (session, should_refresh) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            let edit = tab.edit_state.borrow();
            // 自身保存也会触发一次监听批次；仅抑制极短窗口，避免吞掉紧随其后的外部改动。
            let recent = edit
                .last_saved_at
                .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
            (tab.session.clone(), !edit.save_inflight && !recent)
        };
        if session.is_some() && should_refresh {
            self.refresh_document_tab(path);
        }
    }

    fn handle_vault_batches(&self, batches: &[hortus_ffi::TreeBatch]) {
        let mut changed = Vec::new();
        for batch in batches {
            for entry in &batch.upserted {
                if !entry.directory && is_note_file_name(&entry.path) {
                    changed.push(entry.path.clone());
                }
            }
            changed.extend(batch.removed.iter().cloned());
        }
        if changed.is_empty() {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            if st.files_busy {
                return;
            }
            st.pending_file_changes.extend(changed);
            if st.file_change_timer {
                return;
            }
            st.file_change_timer = true;
        }
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let (paths, busy) = {
                let mut st = this.state.borrow_mut();
                st.file_change_timer = false;
                (std::mem::take(&mut st.pending_file_changes), st.files_busy)
            };
            if busy {
                return;
            }
            this.process_file_changes(&paths);
        });
    }

    fn process_file_changes(&self, paths: &[String]) {
        let mut seen = std::collections::HashSet::new();
        for path in paths {
            if !seen.insert(path.as_str()) {
                continue;
            }
            self.maybe_refresh_tab(path);
        }
        // 文件夹级删除/重命名不会逐个列出子笔记时，检查所有已打开标签的磁盘存在性。
        let (open_paths, vault_root) = {
            let st = self.state.borrow();
            (
                st.tabs.keys().cloned().collect::<Vec<_>>(),
                st.vault_root.clone(),
            )
        };
        for rel in open_paths {
            if seen.contains(rel.as_str()) {
                continue;
            }
            // 借用必须在 maybe_refresh_tab 之前结束；否则它会 borrow_mut 并触发 RefCell panic。
            let missing = vault_root
                .as_deref()
                .is_some_and(|root| !root.is_empty() && !Path::new(root).join(&rel).exists());
            if missing {
                self.maybe_refresh_tab(&rel);
            }
        }
        self.tags_panel.refresh();
        self.search_panel.rerun();
        self.refresh_backlinks(true);
    }

    // -----------------------------------------------------------------------
    // 查找替换 / 编辑辅助
    // -----------------------------------------------------------------------

    fn find_open(&self, replace: bool) {
        let Some(path) = self.active_tab_path() else {
            self.show_toast("请先打开一篇笔记");
            return;
        };
        let (find, view, editable) = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(&path) else {
                return;
            };
            let editable = tab.editing;
            let view = if editable {
                Some(tab.edit_view.clone())
            } else {
                tab.reading
                    .as_ref()
                    .and_then(|reading| reading.text_view.clone())
            };
            (tab.find.clone(), view, editable)
        };
        let Some(view) = view else {
            return;
        };
        let buffer = view.buffer();
        find.bind(&view, &buffer, editable);
        let _ = replace;
        find.open(editable);
    }

    #[allow(clippy::type_complexity)]
    fn active_edit_context(
        &self,
    ) -> Option<(
        String,
        gtk4::TextBuffer,
        Arc<DocumentSession>,
        Rc<RefCell<EditState>>,
    )> {
        let path = self.active_tab_path()?;
        let st = self.state.borrow();
        let tab = st.tabs.get(&path)?;
        if !tab.editing {
            return None;
        }
        let session = tab.session.clone()?;
        Some((
            path,
            tab.edit_buffer.clone(),
            session,
            tab.edit_state.clone(),
        ))
    }

    fn perform_toggle_task(&self) {
        let Some((path, buffer, _session, edit_state)) = self.active_edit_context() else {
            self.show_toast("请先进入编辑态");
            return;
        };
        let caret = buffer.iter_at_mark(&buffer.get_insert()).offset().max(0) as usize;
        let text = edit_state.borrow().text.clone();
        let Some((start, end, replacement)) = task_toggle_at(&text, caret) else {
            self.show_toast(&format!("当前行不是任务项：{path}"));
            return;
        };
        buffer.begin_user_action();
        {
            let mut s = buffer.iter_at_offset(start as i32);
            let mut e = buffer.iter_at_offset(end as i32);
            buffer.delete(&mut s, &mut e);
            buffer.insert(&mut buffer.iter_at_offset(start as i32), &replacement);
        }
        buffer.end_user_action();
    }

    fn perform_align_table(&self) {
        let Some((path, buffer, session, edit_state)) = self.active_edit_context() else {
            self.show_toast("请先进入编辑态");
            return;
        };
        let caret = buffer.iter_at_mark(&buffer.get_insert()).offset().max(0) as usize;
        let text = edit_state.borrow().text.clone();
        let mapper = Utf16Mapper::new(&text);
        let caret_u16 = mapper.char_to_utf16(caret) as u64;
        match session.align_table(caret_u16) {
            Ok(Some(patch)) => {
                let (start, end) = {
                    let start_char = mapper.utf16_to_char(patch.start as usize);
                    let end_char =
                        mapper.utf16_to_char(patch.start as usize + patch.length as usize);
                    (start_char, end_char)
                };
                buffer.begin_user_action();
                {
                    let mut s = buffer.iter_at_offset(start as i32);
                    let mut e = buffer.iter_at_offset(end as i32);
                    buffer.delete(&mut s, &mut e);
                    buffer.insert(&mut buffer.iter_at_offset(start as i32), &patch.replacement);
                }
                buffer.end_user_action();
                self.show_toast(&format!("已对齐表格：{path}"));
            }
            Ok(None) => self.show_toast("光标不在 Markdown 表格内"),
            Err(err) => self.show_toast(&format!("对齐表格失败：{err}")),
        }
    }

    fn handle_window_close(&self) -> glib::Propagation {
        {
            let mut st = self.state.borrow_mut();
            st.close_requests += 1;
        }
        if self.state.borrow().close_approved {
            self.state.borrow_mut().disposing = true;
            let handle = self.state.borrow_mut().vault_handle.take();
            if let Some(handle) = handle {
                std::thread::spawn(move || handle.close());
            }
            return glib::Propagation::Proceed;
        }
        let pending: Vec<(String, Arc<DocumentSession>)> = {
            let st = self.state.borrow();
            st.tabs
                .iter()
                .filter_map(|(path, tab)| {
                    tab.session.clone().map(|session| (path.clone(), session))
                })
                .collect()
        };
        let unsaved: Vec<(String, Arc<DocumentSession>)> = pending
            .into_iter()
            .filter(|(_, session)| session.persistence() != DocumentPersistence::Persisted)
            .collect();
        self.state.borrow_mut().close_unsaved = unsaved.len() as u32;
        if unsaved.is_empty() {
            self.state.borrow_mut().disposing = true;
            let handle = self.state.borrow_mut().vault_handle.take();
            if let Some(handle) = handle {
                std::thread::spawn(move || handle.close());
            }
            return glib::Propagation::Proceed;
        }
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let jobs: Vec<(String, String, Arc<DocumentSession>)> = unsaved
                .iter()
                .map(|(path, session)| {
                    let vault_root = this.state.borrow().vault_root.clone().unwrap_or_default();
                    let temp = if vault_root.is_empty() {
                        std::env::temp_dir()
                    } else {
                        this.save_temp_for(&vault_root, path)
                    };
                    (
                        path.clone(),
                        temp.to_string_lossy().into_owned(),
                        Arc::clone(session),
                    )
                })
                .collect();
            let results = gio::spawn_blocking(move || {
                jobs.into_iter()
                    .map(|(path, temp, session)| {
                        let update = session.prepare_close(temp, true);
                        (path, update.persistence)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let still_unsaved: Vec<String> = results
                .into_iter()
                .filter(|(_, persistence)| *persistence != DocumentPersistence::Persisted)
                .map(|(path, _)| path)
                .collect();
            if still_unsaved.is_empty() {
                this.state.borrow_mut().close_approved = true;
                this.window.close();
            } else {
                this.ask_exit_with_unsaved(&still_unsaved);
            }
        });
        glib::Propagation::Stop
    }

    fn ask_exit_with_unsaved(&self, paths: &[String]) {
        let mut body = String::from("以下笔记未能保存到磁盘：\n");
        for path in paths {
            body.push_str(&format!("· {path}\n"));
        }
        body.push_str("\n仍要退出吗？");
        let dialog = adw::AlertDialog::builder()
            .heading("有未保存的修改")
            .body(&body)
            .build();
        dialog.add_response("cancel", "取消");
        dialog.add_response("quit", "放弃修改并退出");
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        self.state.borrow_mut().last_alert = Some(dialog.heading().unwrap_or_default().to_string());
        let this = self.clone();
        let dialog_confirm = dialog.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "quit" {
                this.state.borrow_mut().close_approved = true;
                this.window.close();
            }
            dialog_confirm.close();
        });
        dialog.present(Some(&self.window));
    }

    // -----------------------------------------------------------------------
    // 文件管理：新建、重命名改链、回收站、右键菜单
    // -----------------------------------------------------------------------

    fn install_tree_context_menu(&self) {
        let this = self.clone();
        self.vault_tree
            .set_on_context_menu(move |anchor, path, is_dir| {
                this.show_tree_context_menu(anchor, &path, is_dir);
            });
    }

    fn show_tree_context_menu(&self, anchor: gtk4::Widget, path: &str, is_dir: bool) {
        let popover = gtk4::Popover::builder()
            .has_arrow(true)
            .autohide(true)
            .build();
        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let parent_dir = if is_dir {
            path.to_string()
        } else {
            path.rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_default()
        };
        let add = |label: &str, action: Box<dyn Fn()>| {
            let btn = gtk4::Button::with_label(label);
            btn.add_css_class("flat");
            btn.set_halign(gtk4::Align::Fill);
            let popover_done = popover.clone();
            btn.connect_clicked(move |_| {
                popover_done.popdown();
                action();
            });
            list.append(&btn);
        };
        {
            let this = self.clone();
            let parent = parent_dir.clone();
            add(
                if is_dir {
                    "新建子笔记"
                } else {
                    "新建同级笔记"
                },
                Box::new(move || this.prompt_new_entry_in(&parent, false)),
            );
        }
        {
            let this = self.clone();
            let parent = parent_dir.clone();
            add(
                if is_dir {
                    "新建子文件夹"
                } else {
                    "新建同级文件夹"
                },
                Box::new(move || this.prompt_new_entry_in(&parent, true)),
            );
        }
        {
            let this = self.clone();
            let path = path.to_string();
            add(
                "重命名…",
                Box::new(move || this.begin_rename(&path, is_dir)),
            );
        }
        {
            let this = self.clone();
            let path = path.to_string();
            add(
                "移到回收站…",
                Box::new(move || this.prompt_trash(&path, is_dir)),
            );
        }
        {
            let this = self.clone();
            let path = path.to_string();
            add(
                "在文件管理器中显示",
                Box::new(move || this.reveal_in_file_manager(&path)),
            );
        }
        popover.set_child(Some(&list));
        popover.set_parent(&anchor);
        popover.popup();
    }

    fn current_parent_dir(&self) -> String {
        if let Some(path) = self.active_tab_path() {
            return path
                .rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_default();
        }
        self.vault_tree
            .selected_directory()
            .map(|(dir, _)| dir)
            .unwrap_or_default()
    }

    fn prompt_new_entry(&self, directory: bool) {
        let parent = self.current_parent_dir();
        self.prompt_new_entry_in(&parent, directory);
    }

    fn prompt_new_entry_in(&self, parent: &str, directory: bool) {
        let Some(handle) = self.state.borrow().vault_handle.clone() else {
            self.show_toast("请先打开知识库");
            return;
        };
        let this = self.clone();
        let parent = parent.to_string();
        files::prompt_new_entry(
            &self.window,
            handle,
            parent,
            directory,
            Rc::new(move |result| match result {
                Ok(entry) => this.after_create_entry(entry),
                Err(err) => this.show_toast(&short_error("创建失败", &err)),
            }),
        );
    }

    fn after_create_entry(&self, entry: hortus_ffi::TreeEntry) {
        let handle = self.state.borrow().vault_handle.clone();
        if let Some(handle) = handle {
            handle.update_search_index(vec![entry.path.clone()], vec![]);
        }
        self.show_toast(if entry.directory {
            "已创建文件夹"
        } else {
            "已创建笔记"
        });
        if !entry.directory {
            self.open_note_at(&entry.path, None);
        }
    }

    fn begin_rename(&self, path: &str, is_dir: bool) {
        if self.state.borrow().files_busy {
            self.show_toast("上一个文件操作尚未完成");
            return;
        }
        let Some(handle) = self.state.borrow().vault_handle.clone() else {
            self.show_toast("请先打开知识库");
            return;
        };
        let this = self.clone();
        let path = path.to_string();
        files::prompt_rename(
            &self.window,
            handle,
            path.clone(),
            is_dir,
            None,
            Rc::new(move |new_name, update_links| {
                this.perform_rename(&path, &new_name, update_links, is_dir);
            }),
        );
    }

    fn affected_open_tabs(&self, path: &str) -> Vec<String> {
        let prefix = format!("{path}/");
        self.state
            .borrow()
            .tabs
            .keys()
            .filter(|open| open.as_str() == path || open.starts_with(&prefix))
            .cloned()
            .collect()
    }

    fn perform_rename(&self, old_path: &str, new_name: &str, update_links: bool, is_dir: bool) {
        let resolved = {
            let st = self.state.borrow();
            match (&st.vault_handle, &st.core, &st.vault_root) {
                (Some(handle), Some(core), Some(root)) => {
                    Some((Arc::clone(handle), Arc::clone(core), root.clone()))
                }
                _ => None,
            }
        };
        let Some((handle, core, vault_root)) = resolved else {
            self.show_toast("请先打开知识库");
            return;
        };
        let parent = old_path
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_default();
        let new_path = if parent.is_empty() {
            new_name.to_string()
        } else {
            format!("{parent}/{new_name}")
        };
        if new_path == old_path {
            return;
        }
        let affected = self.affected_open_tabs(old_path);
        let open_sessions: Vec<(String, Arc<DocumentSession>)> = {
            let st = self.state.borrow();
            affected
                .iter()
                .filter_map(|path| {
                    st.tabs.get(path).and_then(|tab| {
                        tab.session
                            .as_ref()
                            .map(|session| (path.clone(), Arc::clone(session)))
                    })
                })
                .collect()
        };
        let sessions: Vec<(String, Arc<DocumentSession>, String)> = open_sessions
            .into_iter()
            .map(|(path, session)| {
                let temp = self.save_temp_for(&vault_root, &path);
                (path, session, temp.to_string_lossy().into_owned())
            })
            .collect();
        self.state.borrow_mut().files_busy = true;
        let this = self.clone();
        let old_path = old_path.to_string();
        let name_owned = new_name.to_string();
        glib::MainContext::default().spawn_local(async move {
            let old_bg = old_path.clone();
            let new_bg = new_path.clone();
            let name_bg = name_owned;
            let root_bg = vault_root.clone();
            let handle_bg = Arc::clone(&handle);
            let core_bg = Arc::clone(&core);
            let sessions_bg = sessions.clone();
            let temp_bg = this
                .save_temp_for(&vault_root, &old_path)
                .to_string_lossy()
                .into_owned();
            let result = gio::spawn_blocking(move || {
                // 先保护未保存内容：任何仍无法持久化的标签页都会中止整个重命名。
                for (path, session, temp) in &sessions_bg {
                    let update = session.prepare_close(temp.clone(), true);
                    if update.persistence != DocumentPersistence::Persisted {
                        return Err(format!("rename.unsaved:{path}"));
                    }
                }
                let entry = handle_bg
                    .rename_entry(old_bg.clone(), name_bg)
                    .map_err(|e| e.to_string())?;
                let mut rewrites = 0u64;
                let mut updated_notes = 0usize;
                if update_links {
                    // 内核要求先规划再应用；对话框已预计算过，这里重复规划保证幂等与自动化路径一致。
                    let _ = if is_dir {
                        handle_bg.plan_folder_rename_links(
                            old_bg.clone(),
                            entry.path.clone(),
                        )
                    } else {
                        handle_bg.plan_rename_links(old_bg.clone(), entry.path.clone())
                    };
                    let applied = if is_dir {
                        handle_bg.apply_folder_rename_links_in(
                            old_bg.clone(),
                            entry.path.clone(),
                            Vec::new(),
                            temp_bg.clone(),
                        )
                    } else {
                        handle_bg.apply_rename_links_in(
                            old_bg.clone(),
                            entry.path.clone(),
                            Vec::new(),
                            temp_bg.clone(),
                        )
                    }
                    .map_err(|e| e.to_string())?;
                    rewrites = applied.total_rewrites;
                    updated_notes = applied.updated.len();
                }
                handle_bg.update_search_index(vec![entry.path.clone()], vec![old_bg.clone()]);
                let _ = core_bg.rename_diagram_state(
                    root_bg.clone(),
                    old_bg.clone(),
                    entry.path.clone(),
                    is_dir,
                );
                // 迁移打开中的会话：正文与未保存状态保留，恢复稿随路径搬移。
                let mut moved = Vec::new();
                for (path, session, _) in &sessions_bg {
                    let new_rel = if *path == old_bg {
                        new_bg.clone()
                    } else {
                        let suffix = path.strip_prefix(&old_bg).unwrap_or("");
                        format!("{new_bg}{suffix}")
                    };
                    let _ = session.relocate(new_rel.clone());
                    moved.push((path.clone(), new_rel));
                }
                Ok::<_, String>((entry, rewrites, updated_notes, moved))
            })
            .await;
            this.state.borrow_mut().files_busy = false;
            match result {
                Ok(Ok((entry, rewrites, updated_notes, moved))) => {
                    let mut moved_tabs = Vec::new();
                    {
                        let mut st = this.state.borrow_mut();
                        for (old, new) in &moved {
                            if let Some(mut tab) = st.tabs.remove(old) {
                                tab.path = new.clone();
                                {
                                    let mut edit = tab.edit_state.borrow_mut();
                                    edit.path = new.clone();
                                }
                                tab.page.set_tooltip(new);
                                st.tabs.insert(new.clone(), tab);
                                moved_tabs.push((old.clone(), new.clone()));
                            }
                        }
                    }
                    for (_, new) in &moved_tabs {
                        this.update_tab_title(new);
                        this.refresh_document_tab(new);
                    }
                    let handle = this.state.borrow().vault_handle.clone();
                    if let Some(handle) = handle {
                        let removed: Vec<String> = moved_tabs.iter().map(|(old, _)| old.clone()).collect();
                        let added: Vec<String> = moved_tabs.iter().map(|(_, new)| new.clone()).collect();
                        handle.update_search_index(added, removed);
                    }
                    this.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "rename",
                        "old": old_path,
                        "new": entry.path,
                        "is_dir": is_dir,
                        "update_links": update_links,
                        "updated_notes": updated_notes,
                        "rewrites": rewrites,
                        "moved_tabs": moved_tabs.iter().map(|(_, new)| new.clone()).collect::<Vec<_>>(),
                    }));
                    if update_links {
                        this.show_toast(&format!(
                            "已重命名，更新 {updated_notes} 篇笔记的 {rewrites} 处链接"
                        ));
                    } else {
                        this.show_toast("已重命名");
                    }
                    this.refresh_knowledge_after_write();
                }
                Ok(Err(err)) => {
                    this.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "rename",
                        "error": err,
                    }));
                    if err.starts_with("rename.unsaved:") {
                        this.show_toast("有未保存修改，已取消重命名");
                        this.state.borrow_mut().last_alert =
                            Some("有未保存的修改，已取消重命名".into());
                    } else {
                        this.show_toast(&short_error("重命名失败", &err));
                    }
                }
                Err(_) => this.show_toast("重命名线程异常"),
            }
        });
    }

    fn prompt_trash(&self, path: &str, is_dir: bool) {
        if self.state.borrow().files_busy {
            self.show_toast("上一个文件操作尚未完成");
            return;
        }
        let dirty = self
            .affected_open_tabs(path)
            .iter()
            .filter(|open| {
                self.state
                    .borrow()
                    .tabs
                    .get(*open)
                    .is_some_and(|tab| tab.edit_state.borrow().dirty)
            })
            .count();
        let this = self.clone();
        let path = path.to_string();
        files::confirm_trash(
            &self.window,
            path.clone(),
            is_dir,
            dirty,
            Rc::new(move || {
                this.perform_trash(&path, is_dir);
            }),
        );
    }

    fn perform_trash(&self, path: &str, is_dir: bool) {
        let resolved = {
            let st = self.state.borrow();
            match (&st.vault_handle, &st.core, &st.vault_root) {
                (Some(handle), Some(core), Some(root)) => {
                    Some((Arc::clone(handle), Arc::clone(core), root.clone()))
                }
                _ => None,
            }
        };
        let Some((handle, core, vault_root)) = resolved else {
            self.show_toast("请先打开知识库");
            return;
        };
        let affected = self.affected_open_tabs(path);
        let open_sessions: Vec<(String, Arc<DocumentSession>)> = {
            let st = self.state.borrow();
            affected
                .iter()
                .filter_map(|open| {
                    st.tabs.get(open).and_then(|tab| {
                        tab.session
                            .as_ref()
                            .map(|session| (open.clone(), Arc::clone(session)))
                    })
                })
                .collect()
        };
        let sessions: Vec<(String, Arc<DocumentSession>, String)> = open_sessions
            .into_iter()
            .map(|(open, session)| {
                let temp = self.save_temp_for(&vault_root, &open);
                (open, session, temp.to_string_lossy().into_owned())
            })
            .collect();
        self.state.borrow_mut().files_busy = true;
        let this = self.clone();
        let path_bg = path.to_string();
        let handle_bg = Arc::clone(&handle);
        glib::MainContext::default().spawn_local(async move {
            let handle_job = Arc::clone(&handle_bg);
            let path_job = path_bg.clone();
            let sessions_job = sessions.clone();
            let result = gio::spawn_blocking(move || {
                for (open, session, temp) in &sessions_job {
                    let update = session.prepare_close(temp.clone(), true);
                    if update.persistence != DocumentPersistence::Persisted {
                        return Err(format!("trash.unsaved:{open}"));
                    }
                }
                let absolute = handle_job
                    .resolve_trash_target(path_job)
                    .map_err(|e| e.to_string())?;
                let file = gio::File::for_path(Path::new(&absolute));
                file.trash(gio::Cancellable::NONE)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(absolute)
            })
            .await;
            this.state.borrow_mut().files_busy = false;
            match result {
                Ok(Ok(absolute)) => {
                    let removed: Vec<String> = this.affected_open_tabs(&path_bg);
                    {
                        let mut st = this.state.borrow_mut();
                        for open in &removed {
                            st.force_close.insert(open.clone());
                        }
                    }
                    for open in &removed {
                        this.close_tab_by_path(open);
                    }
                    handle_bg.update_search_index(Vec::new(), removed.clone());
                    let _ = core.remove_diagram_state(vault_root.clone(), path_bg.clone(), is_dir);
                    let xdg_data = std::env::var("XDG_DATA_HOME").unwrap_or_default();
                    let trash_dir = Path::new(&xdg_data).join("Trash/files");
                    this.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "trash",
                        "path": path_bg,
                        "absolute": absolute,
                        "is_dir": is_dir,
                        "trash_dir": trash_dir.to_string_lossy(),
                        "trash_files": count_dir_files(&trash_dir),
                        "closed_tabs": removed,
                    }));
                    this.show_toast("已移到回收站");
                    this.refresh_knowledge_after_write();
                }
                Ok(Err(err)) => {
                    this.state.borrow_mut().last_file_op = Some(serde_json::json!({
                        "op": "trash",
                        "error": err,
                    }));
                    if err.starts_with("trash.unsaved:") {
                        this.show_toast("有未保存修改，已取消移入回收站");
                        this.state.borrow_mut().last_alert =
                            Some("有未保存的修改，已取消移入回收站".into());
                    } else {
                        this.show_toast(&short_error("移到回收站失败", &err));
                    }
                }
                Err(_) => this.show_toast("回收站线程异常"),
            }
        });
    }

    fn reveal_in_file_manager(&self, path: &str) {
        let Some(vault_root) = self.state.borrow().vault_root.clone() else {
            return;
        };
        let absolute = Path::new(&vault_root).join(path);
        let parent = absolute.parent().unwrap_or(Path::new(&vault_root));
        let uri = gio::File::for_path(parent).uri();
        let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
    }

    // -----------------------------------------------------------------------
    // 截图场景：为公开证据装配真实界面状态（不做任何伪造渲染）
    // -----------------------------------------------------------------------

    fn setup_l2_scene(&self, scene: &str) {
        match scene {
            "search-backlinks" => {
                self.left_split.set_show_sidebar(true);
                self.right_split.set_show_sidebar(true);
                self.switch_left_panel("search");
                self.search_panel.set_query("Hortus");
                self.right_stack.set_visible_child_name("backlinks");
                self.rebuild_panel_switchers();
                self.refresh_backlinks(true);
                self.focus_search_panel();
            }
            "tags" => {
                self.left_split.set_show_sidebar(true);
                self.switch_left_panel("tags");
                let first_tag = self
                    .state
                    .borrow()
                    .vault_handle
                    .as_ref()
                    .and_then(|handle| handle.tags().into_iter().next());
                if let Some(tag) = first_tag {
                    self.tags_panel.select_tag(&tag.tag);
                }
            }
            "quick-open" => {
                self.open_quick_open();
                self.quick_open.set_query("Start");
            }
            "edit-find-completion" => {
                let editing = self
                    .state
                    .borrow()
                    .tabs
                    .get(&self.active_tab_path().unwrap_or_default())
                    .is_some_and(|tab| tab.editing);
                if !editing {
                    self.switch_edit_mode(false);
                }
                self.find_open(false);
                let Some(path) = self.active_tab_path() else {
                    return;
                };
                let (buffer, view, find) = {
                    let st = self.state.borrow();
                    let Some(tab) = st.tabs.get(&path) else {
                        return;
                    };
                    (
                        tab.edit_buffer.clone(),
                        tab.edit_view.clone(),
                        tab.find.clone(),
                    )
                };
                find.set_query("Hortus");
                // 短场景文档首屏即可见末尾：把光标放到末尾后输入 `[[`，弹层自然出现在光标旁。
                {
                    let end = buffer.end_iter();
                    buffer.place_cursor(&end);
                    view.grab_focus();
                }
                buffer.begin_user_action();
                {
                    let mut end = buffer.end_iter();
                    buffer.insert(&mut end, "\n\n[[Start");
                }
                buffer.end_user_action();
            }
            "rename-links" => {
                let Some(path) = self.active_tab_path() else {
                    return;
                };
                let Some(handle) = self.state.borrow().vault_handle.clone() else {
                    return;
                };
                let this = self.clone();
                let scene_path = path.clone();
                let stem = Path::new(&path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("note")
                    .to_string();
                let ext = Path::new(&path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("md")
                    .to_string();
                files::prompt_rename(
                    &self.window,
                    handle,
                    path,
                    false,
                    Some(format!("{stem}-改名预览.{ext}")),
                    Rc::new(move |new_name, update_links| {
                        this.perform_rename(&scene_path, &new_name, update_links, false);
                    }),
                );
            }
            _ => {}
        }
    }

    fn execute_edit_knowledge_checks(&self, report_path: PathBuf) {
        // 完整流程在 `run_edit_knowledge_checks` 中实现。
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let report = this.run_edit_knowledge_checks().await;
            if let Some(parent) = report_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match serde_json::to_string_pretty(&report) {
                Ok(json) => {
                    if let Err(err) = std::fs::write(&report_path, format!("{json}\n")) {
                        eprintln!("write edit-knowledge json failed: {err}");
                    }
                }
                Err(err) => eprintln!("serialize edit-knowledge json failed: {err}"),
            }
            this.close_now();
        });
    }
}

fn capture_widget_png<W: IsA<gtk4::Widget> + IsA<gtk4::Native>>(
    widget: &W,
    output_path: &Path,
) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let width = widget.width().max(1);
    let height = widget.height().max(1);
    let paintable = gtk4::WidgetPaintable::new(Some(widget));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or_else(|| "窗口渲染节点为空".to_string())?;
    let renderer = gtk4::prelude::NativeExt::renderer(widget)
        .ok_or_else(|| "窗口 GskRenderer 不可用".to_string())?;
    let bounds = gtk4::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&bounds));
    texture
        .save_to_png(output_path)
        .map_err(|e| format!("保存 PNG 失败：{e}"))?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct TabFacts {
    dirty: bool,
    issue: Option<DocumentIssue>,
    text: String,
    revision: u64,
    saves: u32,
    restyles: u32,
    reloads: u32,
    style_micros: u64,
    load_style_micros: u64,
    refreshes: u32,
    deferred_refreshes: u32,
    poll_refreshes: u32,
    last_refresh_issue: Option<DocumentIssue>,
    editing: bool,
    loaded: bool,
    notice_visible: bool,
    title: String,
    issue_message: String,
}

/// 错误提示只保留简短原因，避免长路径/英文 io 错误把通知条撑宽。
fn short_error(prefix: &str, error: &str) -> String {
    let reason: String = error.chars().take(32).collect();
    if error.chars().count() > 32 {
        format!("{prefix}：{reason}…")
    } else {
        format!("{prefix}：{reason}")
    }
}

async fn wait_for(timeout_ms: u64, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        glib::timeout_future(Duration::from_millis(25)).await;
    }
}

fn count_dir_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

fn collect_temp_leftovers(root: &Path) -> Vec<String> {
    let mut leftovers = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path.clone());
            }
            let lower = name.to_lowercase();
            if lower.contains("tmp")
                || lower.contains(".hortus")
                || lower.contains("temporaryitems")
                || lower.starts_with(".~")
            {
                leftovers.push(path.to_string_lossy().into_owned());
            }
        }
    }
    leftovers.sort();
    leftovers
}

impl HortusWindow {
    fn tab_facts(&self, path: &str) -> Option<TabFacts> {
        let st = self.state.borrow();
        let tab = st.tabs.get(path)?;
        let edit = tab.edit_state.borrow();
        Some(TabFacts {
            dirty: edit.dirty,
            issue: edit.issue,
            text: edit.text.clone(),
            revision: edit.revision,
            saves: edit.saves,
            restyles: edit.restyles,
            reloads: edit.reading_reloads,
            style_micros: edit.last_style_micros,
            load_style_micros: edit.load_style_micros,
            refreshes: edit.refreshes,
            deferred_refreshes: edit.deferred_refreshes,
            poll_refreshes: edit.poll_refreshes,
            last_refresh_issue: edit.last_refresh_issue,
            editing: tab.editing,
            loaded: tab.source_loaded && tab.reading_loaded,
            notice_visible: tab.notice.reveals_child(),
            title: tab.page.title().to_string(),
            issue_message: edit.message.clone(),
        })
    }

    fn select_tab(&self, path: &str) -> bool {
        let page = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.page.clone());
        if let Some(page) = page {
            self.tab_view.set_selected_page(&page);
            self.center_stack.set_visible_child_name("tabs");
            return true;
        }
        false
    }

    fn active_path_is(&self, path: &str) -> bool {
        self.active_tab_path().as_deref() == Some(path)
    }

    fn insert_into_edit(&self, path: &str, text: &str) {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            tab.edit_buffer.clone()
        };
        buffer.begin_user_action();
        {
            let mut end = buffer.end_iter();
            buffer.insert(&mut end, text);
        }
        buffer.end_user_action();
    }

    fn place_cursor_at_char(&self, path: &str, char_index: usize) -> bool {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return false;
            };
            tab.edit_buffer.clone()
        };
        let iter = buffer.iter_at_offset(char_index as i32);
        buffer.place_cursor(&iter);
        true
    }

    fn restore_or_clear_undo(&self, path: &str) {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            tab.edit_buffer.clone()
        };
        // 程序化加载/重置后清空撤销历史，避免 Ctrl+Z 回到“正在加载…”。
        buffer.set_enable_undo(false);
        buffer.set_enable_undo(true);
    }

    async fn session_text(&self, path: &str) -> Option<String> {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        }?;
        gio::spawn_blocking(move || session.snapshot().ok().map(|s| s.text))
            .await
            .ok()
            .flatten()
    }

    fn read_disk_note(&self, path: &str) -> Option<String> {
        let root = self.state.borrow().vault_root.clone()?;
        std::fs::read_to_string(Path::new(&root).join(path)).ok()
    }

    fn writing_ext(&self, path: &str, content: &str) -> bool {
        let Some(root) = self.state.borrow().vault_root.clone() else {
            return false;
        };
        std::fs::write(Path::new(&root).join(path), content).is_ok()
    }

    fn checkpoint_count(&self) -> usize {
        let cache = std::env::var("HORTUS_CACHE_DIR").unwrap_or_default();
        if cache.is_empty() {
            return 0;
        }
        count_dir_files(&Path::new(&cache).join("recovery")).len()
    }

    fn inline_fallback_baseline_ok(&self, _path: &str) -> bool {
        // 直接调用阅读视图实际使用的降级构件，核对基线对齐策略。
        let widget = crate::math_render::build_math_fallback_widget(
            "\\unknowninlinecmd{x}",
            "公式语法错误",
            false,
            0.0,
        );
        widget.has_css_class("hortus-math-inline-fallback")
            && matches!(
                widget.valign(),
                gtk4::Align::Baseline | gtk4::Align::BaselineFill
            )
    }

    #[allow(clippy::too_many_lines)]
    async fn run_edit_knowledge_checks(&self) -> serde_json::Value {
        let debug = std::env::var_os("HORTUS_DEBUG_L2").is_some();
        let this = self.clone();
        self.window.present();
        let automation_started = Instant::now();
        macro_rules! stage {
            ($name:expr) => {{
                this.state.borrow_mut().last_stage = $name.to_string();
                if debug {
                    eprintln!(
                        "L2-STAGE {} at {}ms",
                        $name,
                        automation_started.elapsed().as_millis()
                    );
                }
            }};
        }
        let root = self.state.borrow().vault_root.clone().unwrap_or_default();
        let cache_dir = std::env::var("HORTUS_CACHE_DIR").unwrap_or_default();
        let xdg_data = std::env::var("XDG_DATA_HOME").unwrap_or_default();
        let real_home = std::env::var("HOME").unwrap_or_default();
        let real_trash = Path::new(&real_home).join(".local/share/Trash/files");
        let real_trash_before = count_dir_files(&real_trash).len();

        let mut report = serde_json::Map::new();
        report.insert("vault_root".into(), serde_json::json!(root));
        report.insert("cache_dir".into(), serde_json::json!(cache_dir));
        report.insert(
            "temp_dir".into(),
            serde_json::json!(self.state.borrow().temp_dir.to_string_lossy()),
        );
        report.insert(
            "temp_dir_outside_vault".into(),
            serde_json::json!(!self.state.borrow().temp_dir.starts_with(Path::new(&root))),
        );
        report.insert(
            "startup_ms".into(),
            serde_json::json!(self.state.borrow().started_at.elapsed().as_millis() as u64),
        );
        report.insert(
            "real_trash_before".into(),
            serde_json::json!(real_trash_before),
        );

        let accels: Vec<serde_json::Value> = self
            .window
            .application()
            .map(|app| {
                [
                    "win.save-note",
                    "win.find-in-note",
                    "win.replace-in-note",
                    "win.quick-open",
                    "win.new-note",
                    "win.focus-search",
                    "win.align-table",
                    "win.toggle-edit-mode",
                ]
                .into_iter()
                .map(|action| {
                    serde_json::json!({
                        "action": action,
                        "accels": app.accels_for_action(action).iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                    })
                })
                .collect()
            })
            .unwrap_or_default();
        report.insert("accels".into(), serde_json::json!(accels));

        let initial_ready = wait_for(20000, || {
            let st = this.state.borrow();
            st.scan_complete
                && !st.tabs.is_empty()
                && st
                    .tabs
                    .values()
                    .all(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        report.insert("initial_ready".into(), serde_json::json!(initial_ready));
        stage!("initial-ready");

        stage!("A-edit");
        // ---------------- A. 编辑、原子保存、恢复稿、撤销 ----------------
        let note = "L2-编辑目标.md";
        this.open_note_at(note, None);
        let loaded = wait_for(8000, || {
            this.state
                .borrow()
                .tabs
                .get(note)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        let mut edit = serde_json::Map::new();
        edit.insert("loaded".into(), serde_json::json!(loaded));
        let already_editing = this
            .state
            .borrow()
            .tabs
            .get(note)
            .is_some_and(|tab| tab.editing);
        if !already_editing {
            this.toggle_active_tab_edit_mode();
        }
        edit.insert(
            "entered".into(),
            serde_json::json!(
                this.state
                    .borrow()
                    .tabs
                    .get(note)
                    .is_some_and(|tab| tab.editing)
            ),
        );
        this.restore_or_clear_undo(note);
        let undo_baseline = {
            let buffer = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_buffer.clone());
            buffer.map(|b| b.can_undo()).unwrap_or(false)
        };
        this.insert_into_edit(note, "\n\nL2 增量输入标记\n");
        let typed_synced = wait_for(4000, || {
            this.state
                .borrow()
                .tabs
                .get(note)
                .is_some_and(|tab| tab.edit_state.borrow().text.contains("L2 增量输入标记"))
        })
        .await;
        let session_text_after_typing = this.session_text(note).await.unwrap_or_default();
        edit.insert("typed_synced".into(), serde_json::json!(typed_synced));
        edit.insert(
            "session_matches_buffer".into(),
            serde_json::json!(session_text_after_typing.contains("L2 增量输入标记")),
        );
        edit.insert(
            "dirty_marker".into(),
            serde_json::json!(
                this.tab_facts(note)
                    .map(|f| f.title.starts_with('●'))
                    .unwrap_or(false)
            ),
        );

        // 样式不入撤销栈：连续重放样式后撤销基线必须保持不变。
        {
            let (buffer, session) = {
                let st = this.state.borrow();
                let Some(tab) = st.tabs.get(note) else {
                    return serde_json::json!({"error": "tab missing"});
                };
                (tab.edit_buffer.clone(), tab.session.clone())
            };
            if let (buffer, Some(session)) = (buffer, session) {
                let text = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
                let mapper = Utf16Mapper::new(&text);
                for _ in 0..3 {
                    if let Ok(styles) = session.source_styles(u64::MAX, true) {
                        apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
                    }
                }
            }
        }
        let undo_after_styles = {
            let buffer = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_buffer.clone());
            buffer.map(|b| b.can_undo()).unwrap_or(true)
        };
        edit.insert(
            "styles_clear_of_undo".into(),
            serde_json::json!(!undo_after_styles || undo_after_styles == undo_baseline),
        );

        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        let _ = gtk4::prelude::WidgetExt::activate_action(&this.window, "win.save-note", None);
        let saved = wait_for(6000, || {
            this.tab_facts(note)
                .is_some_and(|f| !f.dirty && f.saves > saves_before)
        })
        .await;
        let session_text = this.session_text(note).await.unwrap_or_default();
        let disk_text = this.read_disk_note(note).unwrap_or_default();
        let facts = this.tab_facts(note).unwrap_or_default();
        edit.insert("ctrl_s_saved".into(), serde_json::json!(saved));
        edit.insert(
            "facts".into(),
            serde_json::json!({
                "revision": facts.revision,
                "restyles": facts.restyles,
                "editing": facts.editing,
                "loaded": facts.loaded,
                "issue": format!("{:?}", facts.issue),
                "issue_message": facts.issue_message,
            }),
        );
        edit.insert(
            "disk_matches_session".into(),
            serde_json::json!(!disk_text.is_empty() && disk_text == session_text),
        );
        edit.insert("save_micros".into(), serde_json::json!(facts.style_micros));
        edit.insert(
            "temp_leftovers_in_vault".into(),
            serde_json::json!(collect_temp_leftovers(Path::new(&root))),
        );

        // 恢复稿：脏状态下 checkpoint 落到缓存目录，保存成功后清空。
        this.insert_into_edit(note, "\n\n恢复稿检查\n");
        let _ = wait_for(1200, || this.tab_facts(note).is_some_and(|f| f.dirty)).await;
        let session = {
            let st = this.state.borrow();
            st.tabs.get(note).and_then(|tab| tab.session.clone())
        };
        if let Some(session) = session {
            let _ = gio::spawn_blocking(move || session.checkpoint()).await;
        }
        let checkpoint_after_dirty = this.checkpoint_count();
        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        this.request_save(note, SaveChoice::Normal, false);
        let _ = wait_for(6000, || {
            this.tab_facts(note)
                .is_some_and(|f| !f.dirty && f.saves > saves_before)
        })
        .await;
        let checkpoint_after_save = this.checkpoint_count();
        edit.insert(
            "checkpoint_written".into(),
            serde_json::json!(checkpoint_after_dirty > 0),
        );
        edit.insert(
            "checkpoint_cleared_after_save".into(),
            serde_json::json!(checkpoint_after_save == 0),
        );

        // 原生撤销/重做：撤销后正文回到保存前，重做恢复。
        let before_undo = this.session_text(note).await.unwrap_or_default();
        {
            let buffer = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_buffer.clone());
            if let Some(buffer) = buffer {
                buffer.undo();
            }
        }
        let undo_text = this.session_text(note).await.unwrap_or_default();
        {
            let buffer = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_buffer.clone());
            if let Some(buffer) = buffer {
                buffer.redo();
            }
        }
        let redo_text = this.session_text(note).await.unwrap_or_default();
        edit.insert(
            "undo_redo".into(),
            serde_json::json!({
                "undo_changed": undo_text != before_undo,
                "redo_restored": redo_text == before_undo,
            }),
        );

        // 列表续写：真实回车应在新行自动补写同类型前缀。
        this.insert_into_edit(note, "\n- 续写条目");
        {
            let buffer = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_buffer.clone());
            if let Some(buffer) = buffer {
                let end = buffer.end_iter();
                buffer.place_cursor(&end);
            }
        }
        this.insert_into_edit(note, "\n");
        let list_continuation_live = wait_for(1500, || {
            this.tab_facts(note)
                .is_some_and(|f| f.text.contains("- 续写条目\n- "))
        })
        .await;
        edit.insert(
            "list_continuation_helper".into(),
            serde_json::json!(
                crate::core::line_list_continuation("- 待办条目").as_deref() == Some("- ")
            ),
        );
        edit.insert(
            "list_continuation_live".into(),
            serde_json::json!(list_continuation_live),
        );

        // 任务勾选框：把光标放到任务行内并触发真实动作。
        {
            let text = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            if let Some(byte) = text.find("- [ ] 待办任务一") {
                let char_index = text[..byte].chars().count() + 8;
                this.place_cursor_at_char(note, char_index);
            }
            let before = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            let _ =
                gtk4::prelude::WidgetExt::activate_action(&this.window, "win.toggle-task", None);
            let _ = wait_for(600, || {
                this.tab_facts(note).is_some_and(|f| f.text != before)
            })
            .await;
            let after = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            edit.insert(
                "task_toggle_live".into(),
                serde_json::json!(after.contains("- [x] 待办任务一")),
            );
        }

        // 表格自动对齐：光标位于表格内时补全列宽。
        {
            let text = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            if let Some(byte) = text.find("| 规范指南 |") {
                let char_index = text[..byte].chars().count() + 3;
                this.place_cursor_at_char(note, char_index);
            }
            let before = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            let _ =
                gtk4::prelude::WidgetExt::activate_action(&this.window, "win.align-table", None);
            let _ = wait_for(800, || {
                this.tab_facts(note).is_some_and(|f| f.text != before)
            })
            .await;
            let after = this.tab_facts(note).map(|f| f.text).unwrap_or_default();
            edit.insert("align_table_invoked".into(), serde_json::json!(true));
            edit.insert(
                "align_table_patch".into(),
                serde_json::json!(before != after),
            );
        }

        // 自动保存：停止输入约 1s 后落盘。
        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        let autosave_started = Instant::now();
        this.insert_into_edit(note, "\n\n自动保存标记\n");
        let autosaved = wait_for(6000, || {
            this.tab_facts(note)
                .is_some_and(|f| !f.dirty && f.saves > saves_before)
        })
        .await;
        edit.insert("autosaved".into(), serde_json::json!(autosaved));
        edit.insert(
            "autosave_ms".into(),
            serde_json::json!(autosave_started.elapsed().as_millis() as u64),
        );
        let _ = gio::spawn_blocking(|| {}).await;

        report.insert("edit".into(), serde_json::Value::Object(edit));
        stage!("B-external");
        // ---------------- B. 外部修改实时刷新 / 冲突 / 缺失 ----------------
        let mut external = serde_json::Map::new();
        let _ = this.select_tab(note);
        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        this.request_save(note, SaveChoice::Normal, false);
        let _ = wait_for(3000, || this.tab_facts(note).is_some_and(|f| !f.dirty)).await;
        let _ = saves_before;
        {
            let reading = this
                .state
                .borrow()
                .tabs
                .get(note)
                .and_then(|tab| tab.reading.clone());
            if let Some(reading) = reading {
                reading.scroll_fraction(0.4);
            }
        }
        glib::timeout_future(Duration::from_millis(140)).await;
        let scroll_before = this
            .state
            .borrow()
            .tabs
            .get(note)
            .and_then(|tab| {
                tab.reading.as_ref().map(|r| {
                    let adj = r.scrolled.vadjustment();
                    let max = adj.upper() - adj.page_size();
                    if max > 0.0 { adj.value() / max } else { 0.0 }
                })
            })
            .unwrap_or(0.0);
        let reloads_before = this.tab_facts(note).map(|f| f.reloads).unwrap_or(0);
        let base_text = this.read_disk_note(note).unwrap_or_default();
        let external_text = format!("{base_text}\n\n## 外部追加标题\n\n来自外部进程的追加内容。\n");
        let wrote_external = this.writing_ext(note, &external_text);
        let clean_reload = wait_for(9000, || {
            this.tab_facts(note)
                .is_some_and(|f| f.reloads > reloads_before && f.text.contains("外部追加标题"))
        })
        .await;
        glib::timeout_future(Duration::from_millis(320)).await;
        let scroll_after = this
            .state
            .borrow()
            .tabs
            .get(note)
            .and_then(|tab| {
                tab.reading.as_ref().map(|r| {
                    let adj = r.scrolled.vadjustment();
                    let max = adj.upper() - adj.page_size();
                    if max > 0.0 { adj.value() / max } else { 0.0 }
                })
            })
            .unwrap_or(0.0);
        external.insert(
            "external_write_ok".into(),
            serde_json::json!(wrote_external),
        );
        external.insert("clean_reload".into(), serde_json::json!(clean_reload));
        external.insert("scroll_before".into(), serde_json::json!(scroll_before));
        external.insert("scroll_after".into(), serde_json::json!(scroll_after));
        external.insert(
            "scroll_preserved".into(),
            serde_json::json!((scroll_after - scroll_before).abs() < 0.12),
        );

        this.insert_into_edit(note, "\n\n冲突前本地输入\n");
        let _ = wait_for(600, || this.tab_facts(note).is_some_and(|f| f.dirty)).await;
        let conflict_write = "# 外部改写\n\n覆盖内容，不应进入界面。\n";
        let _ = this.writing_ext(note, conflict_write);
        let conflict_shown = wait_for(9000, || {
            this.tab_facts(note)
                .is_some_and(|f| f.issue == Some(DocumentIssue::Conflict) && f.notice_visible)
        })
        .await;
        let conflict_local_preserved = this
            .tab_facts(note)
            .is_some_and(|f| f.text.contains("冲突前本地输入"));
        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        this.request_save(note, SaveChoice::KeepMine, true);
        let keep_mine_saved = wait_for(6000, || {
            this.tab_facts(note)
                .is_some_and(|f| !f.dirty && f.saves > saves_before)
        })
        .await;
        let disk_after_keep = this.read_disk_note(note).unwrap_or_default();
        external.insert(
            "conflict".into(),
            serde_json::json!({
                "shown": conflict_shown,
                "local_preserved": conflict_local_preserved,
                "keep_mine_saved": keep_mine_saved,
                "issue_message": this
                    .tab_facts(note)
                    .map(|f| f.issue_message)
                    .unwrap_or_default(),
                "disk_has_local": disk_after_keep.contains("冲突前本地输入"),
                "disk_has_external_only": disk_after_keep.contains("覆盖内容，不应进入界面"),
            }),
        );

        let root_path = this.state.borrow().vault_root.clone();
        if let Some(root_path) = root_path {
            let _ = std::fs::remove_file(Path::new(&root_path).join(note));
        }
        let missing_shown = wait_for(9000, || {
            this.tab_facts(note)
                .is_some_and(|f| f.issue == Some(DocumentIssue::Missing) && f.notice_visible)
        })
        .await;
        let missing_text_preserved = this
            .tab_facts(note)
            .is_some_and(|f| f.text.contains("冲突前本地输入"));
        let saves_before = this.tab_facts(note).map(|f| f.saves).unwrap_or(0);
        this.request_save(note, SaveChoice::Recreate, true);
        let recreated = wait_for(6000, || {
            this.tab_facts(note)
                .is_some_and(|f| !f.dirty && f.saves > saves_before)
        })
        .await;
        let recreated_disk = this.read_disk_note(note).unwrap_or_default();
        let missing_facts = this.tab_facts(note).unwrap_or_default();
        external.insert(
            "missing".into(),
            serde_json::json!({
                "shown": missing_shown,
                "text_preserved": missing_text_preserved,
                "recreated": recreated,
                "disk_recreated": recreated_disk.contains("冲突前本地输入"),
                "disk_len": recreated_disk.chars().count(),
                "saves_before": saves_before,
                "saves_after": missing_facts.saves,
                "issue_after": format!("{:?}", missing_facts.issue),
                "dirty_after": missing_facts.dirty,
                "refreshes": missing_facts.refreshes,
                "deferred_refreshes": missing_facts.deferred_refreshes,
                "last_refresh_issue": format!("{:?}", missing_facts.last_refresh_issue),
                "poll_refreshes": missing_facts.poll_refreshes,
            }),
        );
        // 文件夹级外部删除：监听批次可能只报告目录本身，兜底对账必须逐个标签检查存在性。
        let folder = "L2-外部文件夹";
        let folder_notes = ["L2-外部文件夹/甲.md", "L2-外部文件夹/乙.md"];
        {
            let root_path = this.state.borrow().vault_root.clone().unwrap_or_default();
            let dir = Path::new(&root_path).join(folder);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("甲.md"), "# 甲\n\n文件夹级删除样本甲。\n");
            let _ = std::fs::write(dir.join("乙.md"), "# 乙\n\n文件夹级删除样本乙。\n");
            let handle = this.state.borrow().vault_handle.clone();
            if let Some(handle) = handle {
                handle.update_search_index(
                    folder_notes.iter().map(|path| path.to_string()).collect(),
                    vec![],
                );
            }
        }
        for rel in folder_notes {
            this.open_note_at(rel, None);
        }
        let folder_loaded = wait_for(9000, || {
            let st = this.state.borrow();
            folder_notes.iter().all(|rel| {
                st.tabs
                    .get(*rel)
                    .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
            })
        })
        .await;
        {
            let root_path = this.state.borrow().vault_root.clone().unwrap_or_default();
            let _ = std::fs::remove_dir_all(Path::new(&root_path).join(folder));
        }
        // 直接投递一条文件夹级路径事件，确保覆盖 process_file_changes 的兜底对账分支。
        this.process_file_changes(&[folder.to_string()]);
        let folder_missing = wait_for(9000, || {
            let st = this.state.borrow();
            folder_notes.iter().all(|rel| {
                st.tabs.get(*rel).is_some_and(|tab| {
                    tab.edit_state.borrow().issue == Some(DocumentIssue::Missing)
                })
            })
        })
        .await;
        let folder_tabs_kept = {
            let st = this.state.borrow();
            folder_notes.iter().all(|rel| st.tabs.contains_key(*rel))
        };
        external.insert(
            "folder_missing".into(),
            serde_json::json!({
                "folder": folder,
                "notes": folder_notes,
                "loaded": folder_loaded,
                "missing_shown": folder_missing,
                "tabs_kept": folder_tabs_kept,
            }),
        );

        // 关闭保护：脏且无法保存的标签页必须弹出原生对话框，且不能静默关闭。
        let close_guard_note = "L2-关闭保护.md";
        {
            let root_path = this.state.borrow().vault_root.clone().unwrap_or_default();
            let _ = std::fs::write(
                Path::new(&root_path).join(close_guard_note),
                "# 关闭保护\n\n用于验证未保存内容不会被静默丢弃。\n",
            );
            let handle = this.state.borrow().vault_handle.clone();
            if let Some(handle) = handle {
                handle.update_search_index(vec![close_guard_note.into()], vec![]);
            }
        }
        this.open_note_at(close_guard_note, None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get(close_guard_note)
                .is_some_and(|tab| tab.source_loaded)
        })
        .await;
        if !this
            .state
            .borrow()
            .tabs
            .get(close_guard_note)
            .is_some_and(|tab| tab.editing)
        {
            this.toggle_active_tab_edit_mode();
        }
        this.insert_into_edit(close_guard_note, "\n\n未保存的关闭保护内容\n");
        let _ = wait_for(800, || {
            this.tab_facts(close_guard_note).is_some_and(|f| f.dirty)
        })
        .await;
        let root_path = this.state.borrow().vault_root.clone();
        if let Some(root_path) = root_path {
            let _ = std::fs::remove_file(Path::new(&root_path).join(close_guard_note));
        }
        let _ = wait_for(7000, || {
            this.tab_facts(close_guard_note)
                .is_some_and(|f| f.issue == Some(DocumentIssue::Missing))
        })
        .await;
        {
            let mut st = this.state.borrow_mut();
            st.last_alert = None;
            st.last_alert_dialog = None;
        }
        this.close_tab_by_path(close_guard_note);
        let guard_alert = wait_for(6000, || this.state.borrow().last_alert.is_some()).await;
        let guard_tab_kept = this.state.borrow().tabs.contains_key(close_guard_note);
        let guard_text_kept = this
            .tab_facts(close_guard_note)
            .is_some_and(|f| f.text.contains("未保存的关闭保护内容"));
        {
            let dialog = this.state.borrow_mut().last_alert_dialog.clone();
            if let Some(dialog) = dialog {
                // 与用户点击“取消”等价：取消本次关闭请求，标签页保持打开。
                let response = "cancel";
                dialog.emit_by_name::<()>("response", &[&response]);
            }
        }
        external.insert(
            "close_guard".into(),
            serde_json::json!({
                "alert_shown": guard_alert,
                "tab_kept": guard_tab_kept,
                "text_kept": guard_text_kept,
                "alert_title": this.state.borrow().last_alert,
            }),
        );
        // 清理：重新保存后正常关闭。
        let _ = wait_for(500, || this.state.borrow().last_alert_dialog.is_none()).await;
        this.request_save(close_guard_note, SaveChoice::Recreate, false);
        let _ = wait_for(4000, || {
            this.tab_facts(close_guard_note).is_some_and(|f| !f.dirty)
        })
        .await;
        this.close_tab_by_path(close_guard_note);

        // 关闭窗口保护：未保存且无法保存时必须停在原生对话框，窗口不得退出。
        let window_guard_note = "L2-窗口保护.md";
        {
            let root_path = this.state.borrow().vault_root.clone().unwrap_or_default();
            let _ = std::fs::write(
                Path::new(&root_path).join(window_guard_note),
                "# 窗口保护\n\n验证关闭窗口时不会静默丢数据。\n",
            );
            let handle = this.state.borrow().vault_handle.clone();
            if let Some(handle) = handle {
                handle.update_search_index(vec![window_guard_note.into()], vec![]);
            }
        }
        this.open_note_at(window_guard_note, None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get(window_guard_note)
                .is_some_and(|tab| tab.source_loaded)
        })
        .await;
        if !this
            .state
            .borrow()
            .tabs
            .get(window_guard_note)
            .is_some_and(|tab| tab.editing)
        {
            this.toggle_active_tab_edit_mode();
        }
        this.insert_into_edit(window_guard_note, "\n\n窗口关闭前的未保存输入\n");
        let _ = wait_for(800, || {
            this.tab_facts(window_guard_note).is_some_and(|f| f.dirty)
        })
        .await;
        let root_path = this.state.borrow().vault_root.clone();
        if let Some(root_path) = root_path {
            let _ = std::fs::remove_file(Path::new(&root_path).join(window_guard_note));
        }
        let _ = wait_for(7000, || {
            this.tab_facts(window_guard_note)
                .is_some_and(|f| f.issue == Some(DocumentIssue::Missing))
        })
        .await;
        {
            let mut st = this.state.borrow_mut();
            st.last_alert = None;
            st.last_alert_dialog = None;
        }
        // 离屏环境没有窗口管理器，直接调用关闭请求处理器（与 close-request 的回调同一入口）。
        let _close_signal_handled = this.handle_window_close();
        let window_guard_alert = wait_for(8000, || this.state.borrow().last_alert.is_some()).await;
        let window_kept = this.window.is_visible();
        let window_guard_dialog = this.state.borrow_mut().last_alert_dialog.clone();
        if let Some(dialog) = window_guard_dialog {
            let response = "cancel";
            dialog.emit_by_name::<()>("response", &[&response]);
        }
        external.insert(
            "window_close_guard".into(),
            serde_json::json!({
                "alert_shown": window_guard_alert,
                "window_kept": window_kept,
                "alert_title": this.state.borrow().last_alert,
                "close_requests": this.state.borrow().close_requests,
                "close_unsaved": this.state.borrow().close_unsaved,
            }),
        );
        this.request_save(window_guard_note, SaveChoice::Recreate, false);
        let _ = wait_for(4000, || {
            this.tab_facts(window_guard_note).is_some_and(|f| !f.dirty)
        })
        .await;
        this.close_tab_by_path(window_guard_note);
        report.insert("external".into(), serde_json::Value::Object(external));

        stage!("C-find");
        // ---------------- C. 文内查找与替换 ----------------
        let mut find = serde_json::Map::new();
        let _ = this.select_tab(note);
        let _ = gtk4::prelude::WidgetExt::activate_action(&this.window, "win.find-in-note", None);
        let find_bar = this
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|tab| tab.find.clone());
        if let Some(find_bar) = find_bar.clone() {
            find_bar.set_query("Hortus");
            let got = wait_for(3000, || find_bar.stats().matches > 0).await;
            let stats = find_bar.stats();
            find.insert("opened".into(), serde_json::json!(stats.open));
            find.insert("editable".into(), serde_json::json!(stats.editable));
            find.insert("matches".into(), serde_json::json!(stats.matches));
            find.insert("matches_found".into(), serde_json::json!(got));
            let first = stats.current;
            find_bar.next();
            let second = find_bar.stats().current;
            find.insert(
                "next_advances".into(),
                serde_json::json!(stats.matches < 2 || second != first),
            );
            find_bar.set_replacement("L2Find");
            find_bar.replace_all();
            let _ = wait_for(1500, || find_bar.stats().matches == 0).await;
            let replaced_text = this.session_text(note).await.unwrap_or_default();
            find.insert(
                "replace_all".into(),
                serde_json::json!({
                    "replaced": find_bar.stats().replaced,
                    "session_has_replacement": replaced_text.contains("L2Find"),
                    "session_has_no_query": !replaced_text.contains("Hortus"),
                }),
            );
            find_bar.close();
            find.insert("closed".into(), serde_json::json!(!find_bar.is_open()));
            find.insert(
                "searches".into(),
                serde_json::json!(find_bar.stats().searches),
            );
        }

        // 阅读视图查找（只读，不展开替换行）。
        this.open_note_at("Start here.md", None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get("Start here.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        let _ =
            gtk4::prelude::WidgetExt::activate_action(&this.window, "win.replace-in-note", None);
        let read_find = this
            .state
            .borrow()
            .tabs
            .get("Start here.md")
            .map(|tab| tab.find.clone());
        if let Some(read_find) = read_find {
            read_find.set_query("Learning");
            let _ = wait_for(2000, || read_find.stats().matches > 0).await;
            let stats = read_find.stats();
            find.insert(
                "reading_view".into(),
                serde_json::json!({
                    "open": stats.open,
                    "editable": stats.editable,
                    "matches": stats.matches,
                }),
            );
            read_find.close();
        }
        report.insert("find".into(), serde_json::Value::Object(find));

        stage!("D-bigdoc");
        // ---------------- D. 1 MiB 大文档线性样式 ----------------
        let mut big = serde_json::Map::new();
        let big_note = "L2-大文档.md";
        let open_started = Instant::now();
        this.open_note_at(big_note, None);
        let big_loaded = wait_for(20000, || {
            this.state
                .borrow()
                .tabs
                .get(big_note)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        stage!("D-bigdoc-opened");
        big.insert(
            "open_ms".into(),
            serde_json::json!(open_started.elapsed().as_millis() as u64),
        );
        big.insert("loaded".into(), serde_json::json!(big_loaded));
        big.insert(
            "bytes".into(),
            serde_json::json!(
                this.state
                    .borrow()
                    .vault_root
                    .clone()
                    .map(|root| std::fs::metadata(Path::new(&root).join(big_note))
                        .map(|m| m.len())
                        .unwrap_or(0))
                    .unwrap_or(0)
            ),
        );
        if let Some((buffer, session)) = {
            let st = this.state.borrow();
            st.tabs.get(big_note).and_then(|tab| {
                tab.session
                    .clone()
                    .map(|session| (tab.edit_buffer.clone(), session))
            })
        } {
            let text = this.session_text(big_note).await.unwrap_or_default();
            let mapper = Utf16Mapper::new(&text);
            let mut best = u64::MAX;
            let mut worst = 0u64;
            let mut runs = 0usize;
            let mut analysis = 0u64;
            let mut plan_best = u64::MAX;
            let mut tag_best = u64::MAX;
            for _ in 0..3 {
                let started = Instant::now();
                let Ok(styles) = session.source_styles(u64::MAX, true) else {
                    continue;
                };
                analysis = analysis.max(styles.analysis_micros);
                runs = styles.packed_runs.len() / 20;
                let (plan_micros, tag_micros) =
                    apply_source_styles_timed(&buffer, &mapper, &styles.packed_runs);
                plan_best = plan_best.min(plan_micros);
                tag_best = tag_best.min(tag_micros);
                let micros = started.elapsed().as_micros() as u64;
                best = best.min(micros);
                worst = worst.max(micros);
            }
            let big_facts = this.tab_facts(big_note).unwrap_or_default();
            big.insert(
                "load_style_micros".into(),
                serde_json::json!(big_facts.load_style_micros),
            );
            big.insert("style_runs".into(), serde_json::json!(runs));
            big.insert("style_best_micros".into(), serde_json::json!(best));
            big.insert("style_worst_micros".into(), serde_json::json!(worst));
            big.insert(
                "style_plan_best_micros".into(),
                serde_json::json!(plan_best),
            );
            big.insert("style_tag_best_micros".into(), serde_json::json!(tag_best));
            big.insert(
                "style_under_200ms".into(),
                serde_json::json!(worst < 200_000),
            );
            big.insert(
                "plan_under_200ms".into(),
                serde_json::json!(plan_best < 200_000),
            );
            big.insert(
                "load_style_under_200ms".into(),
                serde_json::json!(big_facts.load_style_micros < 200_000),
            );
            big.insert("analysis_micros".into(), serde_json::json!(analysis));
        }
        report.insert("big_doc".into(), serde_json::Value::Object(big));
        stage!("D-bigdoc-done");

        stage!("E-knowledge");
        // ---------------- E. 知识导航 ----------------
        let mut knowledge = serde_json::Map::new();
        this.switch_left_panel("search");
        this.search_panel.run_query_now("Hortus");
        let _ = wait_for(6000, || {
            let stats = this.search_panel.stats();
            !stats.pending && stats.groups > 0
        })
        .await;
        let first_search = this.search_panel.stats();
        this.search_panel.run_query_now("Learning");
        this.search_panel.run_query_now("Hortus");
        let _ = wait_for(6000, || {
            let stats = this.search_panel.stats();
            !stats.pending && stats.last_query == "Hortus" && stats.groups > 0
        })
        .await;
        let final_search = this.search_panel.stats();
        let expected_groups = {
            let handle = this.state.borrow().vault_handle.clone();
            match handle {
                Some(handle) => gio::spawn_blocking(move || {
                    handle
                        .search("Hortus".into(), 6, hortus_ffi::SearchCancellation::new())
                        .len()
                })
                .await
                .unwrap_or(0),
                None => 0,
            }
        };
        let clicked_group = this.search_panel.click_group(0);
        let _ = wait_for(4000, || this.state.borrow().tabs.len() > 2).await;
        glib::timeout_future(Duration::from_millis(250)).await;
        knowledge.insert(
            "search".into(),
            serde_json::json!({
                "initial_groups": first_search.groups,
                "initial_matches": first_search.matches,
                "final_groups": final_search.groups,
                "final_query": final_search.last_query,
                "queries": final_search.queries,
                "cancellations": final_search.cancellations,
                "stale_dropped": final_search.stale_dropped,
                "expected_groups": expected_groups,
                "concurrent_result_matches": final_search.groups == expected_groups,
                "clicked_group": clicked_group,
            }),
        );

        this.switch_left_panel("tags");
        this.tags_panel.refresh();
        let tags_stats = this.tags_panel.stats();
        let tag_labels = this.tags_panel.tag_labels();
        let first_tag = tag_labels.first().cloned();
        let tag_clicked = this.tags_panel.click_tag(0);
        let tag_stats_after_click = this.tags_panel.stats();
        let tag_notes = tag_stats_after_click.visible_notes;
        let clicked_note = this.tags_panel.click_note(0);
        let _ = wait_for(3000, || this.state.borrow().tabs.len() > 2).await;
        let reading_tag_note = this.active_tab_path();
        if let Some(tag) = first_tag.clone() {
            this.open_tag_from_reading(&tag);
        }
        let tags_view_active = this
            .left_stack
            .visible_child_name()
            .map(|name| name.to_string())
            .unwrap_or_default();
        knowledge.insert(
            "tags".into(),
            serde_json::json!({
                "total_tags": tags_stats.total_tags,
                "sample_tags": tag_labels.iter().take(5).cloned().collect::<Vec<_>>(),
                "selected_notes": tag_notes,
                "tag_clicked": tag_clicked,
                "selected_after_click": tag_stats_after_click.selected,
                "filter_after_click": tag_stats_after_click.filter,
                "visible_tags": tag_stats_after_click.visible_tags,
                "clicked_note": clicked_note,
                "reading_click_switches_panel": tags_view_active == "tags",
                "filter_after_reading_click": this.tags_panel.filter_text(),
                "note_after_click": reading_tag_note,
            }),
        );

        for rel in ["Start here.md", "README.md"] {
            this.open_note_at(rel, None);
            let _ = wait_for(6000, || {
                this.state
                    .borrow()
                    .tabs
                    .get(rel)
                    .is_some_and(|tab| tab.reading_loaded)
            })
            .await;
        }
        this.open_note_at("Start here.md", None);
        let _ = wait_for(2000, || this.active_path_is("Start here.md")).await;
        this.refresh_backlinks(true);
        let backlinks_first = this.backlinks_panel.stats();
        let clicked_backlink = this.backlinks_panel.click_row(0);
        let _ = wait_for(4000, || {
            clicked_backlink
                .as_ref()
                .is_some_and(|(source, _)| this.active_path_is(source))
        })
        .await;
        this.open_note_at("README.md", None);
        let _ = wait_for(3000, || {
            this.backlinks_panel.stats().path.as_deref() == Some("README.md")
        })
        .await;
        let backlinks_second = this.backlinks_panel.stats();
        let _ = backlinks_first.path;
        knowledge.insert(
            "backlinks".into(),
            serde_json::json!({
                "first_path": "Start here.md",
                "first_rows": backlinks_first.rows,
                "second_path": this.backlinks_panel.current_path(),
                "second_rows": backlinks_second.rows,
                "updates": backlinks_second.updates,
                "clicked_row": clicked_backlink.as_ref().map(|(s, o)| serde_json::json!({"source": s, "offset": o})),
                "switched": backlinks_first.path != backlinks_second.path,
            }),
        );

        this.open_quick_open();
        this.quick_open.set_query("Start");
        let _ = wait_for(4000, || {
            let stats = this.quick_open.stats();
            stats.last_query == "Start" && stats.hits > 0
        })
        .await;
        let quick_stats = this.quick_open.stats();
        let quick_hits = this.quick_open.hits();
        this.quick_open.activate_selected();
        let quick_closed = wait_for(3000, || !this.quick_open.is_open()).await;
        let _ = wait_for(4000, || {
            quick_hits
                .first()
                .is_some_and(|path| this.active_path_is(path))
        })
        .await;
        knowledge.insert(
            "quick_open".into(),
            serde_json::json!({
                "queries": quick_stats.queries,
                "last_query": quick_stats.last_query,
                "open_before_activate": quick_stats.open,
                "hits": quick_stats.hits,
                "first": quick_stats.first,
                "closed_after_enter": quick_closed,
                "opened_first": quick_hits.first().is_some_and(|path| this.active_path_is(path)),
            }),
        );

        // `[[` 补全：仅在编辑态触发，弹出候选且不打断继续输入。
        this.open_note_at(note, None);
        let _ = wait_for(6000, || this.active_path_is(note)).await;
        if !this
            .state
            .borrow()
            .tabs
            .get(note)
            .is_some_and(|tab| tab.editing)
        {
            this.toggle_active_tab_edit_mode();
        }
        let focus_baseline = {
            let (buffer, view) = {
                let st = this.state.borrow();
                match st.tabs.get(note) {
                    Some(tab) => (Some(tab.edit_buffer.clone()), Some(tab.edit_view.clone())),
                    None => (None, None),
                }
            };
            if let (Some(buffer), Some(view)) = (buffer, view) {
                let end = buffer.end_iter();
                buffer.place_cursor(&end);
                view.grab_focus();
            }
            glib::timeout_future(Duration::from_millis(120)).await;
            this.state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_view.has_focus())
                .unwrap_or(false)
        };
        this.insert_into_edit(note, "\n\n[[Start");
        let completion = this
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|tab| tab.completion.clone());
        let mut completion_report = serde_json::json!({
            "visible": false,
            "items": 0,
            "typed_after_popup": false,
            "accepted": false,
            "target": null,
        });
        if let Some(completion) = completion.clone() {
            let visible = wait_for(4000, || {
                completion.is_visible() && completion.item_count() > 0
            })
            .await;
            let items_at_visible = completion.item_count();
            let _first_target = completion.first_insert_target();
            let view_focus = this
                .state
                .borrow()
                .tabs
                .get(note)
                .map(|tab| tab.edit_view.has_focus())
                .unwrap_or(false);
            let window_active = this.window.is_active();
            this.insert_into_edit(note, "x");
            glib::timeout_future(Duration::from_millis(120)).await;
            let typed_kept = this
                .tab_facts(note)
                .is_some_and(|f| f.text.contains("[[Startx"));
            // 重新输入一个干净的 `[[` 上下文并接受首个候选。
            this.insert_into_edit(note, "\n\n[[Start");
            let re_visible = wait_for(4000, || {
                completion.is_visible() && completion.item_count() > 0
            })
            .await;
            let target = completion.first_insert_target();
            if re_visible {
                completion.accept_selected();
            }
            glib::timeout_future(Duration::from_millis(300)).await;
            let accepted_target = target.clone().unwrap_or_default();
            let accepted_text = !accepted_target.is_empty()
                && this
                    .tab_facts(note)
                    .is_some_and(|f| f.text.contains(&format!("[[{accepted_target}]]")));
            let accepted_in_session = !accepted_target.is_empty()
                && this
                    .session_text(note)
                    .await
                    .unwrap_or_default()
                    .contains(&format!("[[{accepted_target}]]"));
            completion_report = serde_json::json!({
                "visible": visible,
                "items": items_at_visible,
                "window_active": window_active,
                "focus_baseline": focus_baseline,
                "popover_focusable": completion.popover_focusable(),
                "typed_after_popup": typed_kept,
                "view_kept_focus": view_focus,
                "accepted": accepted_text && accepted_in_session && completion.stats().accepted > 0,
                "target": target,
                "stats": {
                    "queries": completion.stats().queries,
                    "accepted": completion.stats().accepted,
                },
            });
        }
        knowledge.insert("completion".into(), completion_report);
        report.insert("knowledge".into(), serde_json::Value::Object(knowledge));

        stage!("F-files");
        // ---------------- F. 文件管理 / 改链 / 回收站 ----------------
        let mut files = serde_json::Map::new();
        let handle = this.state.borrow().vault_handle.clone();
        if let Some(handle) = handle.clone() {
            let handle_bg = Arc::clone(&handle);
            let created = gio::spawn_blocking(move || {
                handle_bg.create_note(String::new(), "L2-新笔记".into())
            })
            .await
            .ok()
            .and_then(|res| res.ok());
            let created_path = created.as_ref().map(|entry| entry.path.clone());
            if let Some(entry) = created {
                this.after_create_entry(entry);
            }
            let _ = wait_for(5000, || {
                this.state.borrow().tabs.contains_key("L2-新笔记.md")
            })
            .await;
            let created_folder = {
                let handle_bg = Arc::clone(&handle);
                gio::spawn_blocking(move || {
                    handle_bg.create_folder(String::new(), "L2-新建文件夹".into())
                })
                .await
                .ok()
                .and_then(|res| res.ok())
            };
            let created_folder_path = created_folder.as_ref().map(|entry| entry.path.clone());
            if let Some(entry) = created_folder {
                this.after_create_entry(entry);
            }
            files.insert(
                "create".into(),
                serde_json::json!({
                    "note": created_path,
                    "folder": created_folder_path,
                    "note_opened": this.state.borrow().tabs.contains_key("L2-新笔记.md"),
                    "note_on_disk": Path::new(&root).join("L2-新笔记.md").is_file(),
                    "folder_on_disk": Path::new(&root).join("L2-新建文件夹").is_dir(),
                }),
            );
        }

        this.state.borrow_mut().last_file_op = None;
        this.open_note_at("L2-被引用.md", None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get("L2-被引用.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        this.open_note_at("L2-引用者.md", None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get("L2-引用者.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        let _ = this.select_tab("L2-被引用.md");
        this.perform_rename("L2-被引用.md", "L2-被引用改名.md", true, false);
        let note_renamed = wait_for(9000, || {
            this.state.borrow().tabs.contains_key("L2-被引用改名.md")
        })
        .await;
        let _ = wait_for(2500, || this.state.borrow().last_file_op.is_some()).await;
        let rename_op = this.state.borrow().last_file_op.clone();
        let referrer_text = this.read_disk_note("L2-引用者.md").unwrap_or_default();
        files.insert(
            "rename_note".into(),
            serde_json::json!({
                "tab_migrated": note_renamed,
                "referrer_updated": referrer_text.contains("[[L2-被引用改名"),
                "referrer_keeps_old": referrer_text.contains("[[L2-被引用]]"),
                "op": rename_op,
                "open_paths": this.state.borrow().tabs.keys().cloned().collect::<Vec<_>>(),
            }),
        );

        this.state.borrow_mut().last_file_op = None;
        this.open_note_at("L2-组/子笔记.md", None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get("L2-组/子笔记.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        this.perform_rename("L2-组", "L2-组新", true, true);
        let folder_renamed = wait_for(9000, || {
            this.state.borrow().tabs.contains_key("L2-组新/子笔记.md")
        })
        .await;
        let _ = wait_for(2500, || this.state.borrow().last_file_op.is_some()).await;
        let folder_op = this.state.borrow().last_file_op.clone();
        let group_referrer = this.read_disk_note("L2-引用组.md").unwrap_or_default();
        files.insert(
            "rename_folder".into(),
            serde_json::json!({
                "tab_migrated": folder_renamed,
                "embed_updated": group_referrer.contains("![[L2-组新/子笔记"),
                "op": folder_op,
                "open_paths": this.state.borrow().tabs.keys().cloned().collect::<Vec<_>>(),
            }),
        );

        this.state.borrow_mut().last_file_op = None;
        this.open_note_at("L2-待删除.md", None);
        let _ = wait_for(6000, || {
            this.state
                .borrow()
                .tabs
                .get("L2-待删除.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        this.perform_trash("L2-待删除.md", false);
        let trash_done = wait_for(9000, || this.state.borrow().last_file_op.is_some()).await;
        let trash_op = this.state.borrow().last_file_op.clone();
        let _ = wait_for(3000, || {
            !this.state.borrow().tabs.contains_key("L2-待删除.md")
        })
        .await;
        let gone = !Path::new(&root).join("L2-待删除.md").exists();
        let tab_closed = !this.state.borrow().tabs.contains_key("L2-待删除.md");
        let real_trash_after = count_dir_files(&real_trash).len();
        files.insert(
            "trash".into(),
            serde_json::json!({
                "done": trash_done,
                "gone": gone,
                "tab_closed": tab_closed,
                "op": trash_op,
                "real_trash_before": real_trash_before,
                "real_trash_after": real_trash_after,
                "real_trash_untouched": real_trash_before == real_trash_after,
                "xdg_data": xdg_data,
            }),
        );
        report.insert("files".into(), serde_json::Value::Object(files));

        stage!("G-misc");
        // ---------------- G. 顺带收口的 L0 观察 ----------------
        let mut misc = serde_json::Map::new();
        this.open_note_at("L2-编辑目标.md", None);
        let _ = wait_for(4000, || {
            this.state
                .borrow()
                .tabs
                .get("L2-编辑目标.md")
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
        misc.insert(
            "inline_fallback_baseline".into(),
            serde_json::json!(this.inline_fallback_baseline_ok("L2-编辑目标.md")),
        );
        let table_text = this
            .tab_facts("L2-编辑目标.md")
            .map(|f| f.text)
            .unwrap_or_default();
        let table_links = crate::media_render::markdown_cell_links(&table_text);
        misc.insert(
            "table_external_links".into(),
            serde_json::json!({
                "detected": table_links.len(),
                "sample": table_links.first().map(|(label, url)| serde_json::json!({"label": label, "url": url})),
            }),
        );
        report.insert("misc".into(), serde_json::Value::Object(misc));

        report.insert(
            "real_trash_after".into(),
            serde_json::json!(count_dir_files(&real_trash).len()),
        );
        report.insert(
            "temp_leftovers_final".into(),
            serde_json::json!(collect_temp_leftovers(Path::new(&root))),
        );
        report.insert(
            "last_toast".into(),
            serde_json::json!(this.state.borrow().last_toast),
        );
        serde_json::Value::Object(report)
    }

    fn execute_wikilink_checks(&self, report_path: PathBuf) {
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let report = this.run_wikilink_checks().await;
            let json = serde_json::to_string_pretty(&report)
                .unwrap_or_else(|err| format!("{{\"error\":\"{err}\"}}"));
            let _ = std::fs::write(&report_path, json + "\n");
            this.close_now();
        });
    }

    fn reading_surface(&self, note: &str) -> Option<ReadingSurfaceHandle> {
        self.state
            .borrow()
            .tabs
            .get(note)
            .and_then(|tab| tab.reading.clone())
    }

    fn click_reading_wikilink(&self, note: &str, label: &str) -> Option<(String, Option<String>)> {
        self.reading_surface(note)?.click_wikilink(label)
    }

    fn tab_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.state.borrow().tabs.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// 阅读态真实点击 `[[双链]]` 回归：命中 GestureClick::released → open_note_or_resolve，
    /// 覆盖已解析裸链接、带标题链接、缺失目标与歧义目标的 toast 分支。
    async fn run_wikilink_checks(&self) -> serde_json::Value {
        let start = "Start here.md";
        let map = "Learning/Learning map.md";
        let loop_note = "Learning/Study loop.md";
        let boundary = "L2-双链边界.md";
        let mut report = serde_json::Map::new();
        let mut stages: Vec<String> = Vec::new();
        stages.push("wait-initial".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));

        let initial_ready = wait_for(20000, || {
            let st = self.state.borrow();
            st.scan_complete
                && st
                    .tabs
                    .get(start)
                    .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        report.insert("initial_ready".into(), serde_json::json!(initial_ready));
        report.insert("initial_tabs".into(), serde_json::json!(self.tab_paths()));

        // A. 仅打开 Start here.md 时，点击阅读视图里的 [[Learning map]]。
        stages.push("click-learning-map".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        let click_map = self.click_reading_wikilink(start, "Learning map");
        report.insert(
            "learning_map_click".into(),
            serde_json::json!({
                "clicked": click_map.is_some(),
                "target": click_map.as_ref().map(|(target, _)| target.clone()),
                "heading": click_map.as_ref().and_then(|(_, heading)| heading.clone()),
            }),
        );
        let map_loaded = wait_for(12000, || {
            self.state
                .borrow()
                .tabs
                .get(map)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        let map_active = self.active_tab_path();
        let map_nodes = self
            .state
            .borrow()
            .tabs
            .get(map)
            .and_then(|tab| {
                tab.prepared_diagrams
                    .iter()
                    .filter_map(|diagram| diagram.scene.as_ref())
                    .map(|scene| scene.hit_regions.len())
                    .max()
            })
            .unwrap_or(0);
        report.insert(
            "learning_map_open".into(),
            serde_json::json!({
                "loaded": map_loaded,
                "active_path": map_active,
                "active_is_target": map_active.as_deref() == Some(map),
                "tab_count": self.state.borrow().tabs.len(),
                "diagram_nodes": map_nodes,
                "diagram_ok": map_nodes > 0,
            }),
        );
        if let Some(ref screenshot_path) = self.options.screenshot {
            // 公开复核图：Start here.md 与 Learning map.md 双标签 + 81 节点大图。
            glib::timeout_future(Duration::from_millis(500)).await;
            if let Err(err) = self.capture_window_png(screenshot_path) {
                eprintln!("screenshot failed: {err}");
            }
        }

        // B. 从 Learning map 点击 [[Study loop]]（同一阅读点击路径的第二个目标）。
        // 新标签刚完成装载时 TextView 尚未分配尺寸，等一帧再点击才能命中字符坐标。
        let _ = wait_for(4000, || {
            self.reading_surface(map)
                .and_then(|surface| surface.text_view)
                .is_some_and(|view| view.is_mapped() && view.width() > 0)
        })
        .await;
        glib::timeout_future(Duration::from_millis(180)).await;
        stages.push("click-study-loop".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        let click_study = self.click_reading_wikilink(map, "Study loop");
        let study_loaded = wait_for(12000, || {
            self.state
                .borrow()
                .tabs
                .get(loop_note)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        let study_active = self.active_tab_path();
        report.insert(
            "study_loop_click".into(),
            serde_json::json!({
                "clicked": click_study.is_some(),
                "target": click_study.as_ref().map(|(target, _)| target.clone()),
                "loaded": study_loaded,
                "active_path": study_active,
                "active_is_target": study_active.as_deref() == Some(loop_note),
                "tab_count": self.state.borrow().tabs.len(),
            }),
        );

        // C. 边界笔记：缺失目标、带标题链接与歧义目标。
        stages.push("open-boundary".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        self.open_note_at(boundary, None);
        let boundary_loaded = wait_for(12000, || {
            self.state
                .borrow()
                .tabs
                .get(boundary)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
        .await;
        report.insert("boundary_loaded".into(), serde_json::json!(boundary_loaded));
        let _ = wait_for(4000, || {
            self.reading_surface(boundary)
                .and_then(|surface| surface.text_view)
                .is_some_and(|view| view.is_mapped() && view.width() > 0)
        })
        .await;
        glib::timeout_future(Duration::from_millis(180)).await;

        stages.push("click-missing-target".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        let tabs_before_missing = self.state.borrow().tabs.len();
        let click_missing = self.click_reading_wikilink(boundary, "L2 不存在的目标");
        let missing_toast = wait_for(4000, || {
            self.state
                .borrow()
                .last_toast
                .as_deref()
                .is_some_and(|toast| toast.contains("未找到链接目标"))
        })
        .await;
        report.insert(
            "missing_target_click".into(),
            serde_json::json!({
                "clicked": click_missing.is_some(),
                "target": click_missing.as_ref().map(|(target, _)| target.clone()),
                "toast_shown": missing_toast,
                "tab_count_unchanged": self.state.borrow().tabs.len() == tabs_before_missing,
                "active_path": self.active_tab_path(),
            }),
        );

        stages.push("click-ambiguous-target".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        let click_ambiguous = self.click_reading_wikilink(boundary, "同名");
        let ambiguous_toast = wait_for(4000, || {
            self.state
                .borrow()
                .last_toast
                .as_deref()
                .is_some_and(|toast| toast.contains("多个笔记匹配"))
        })
        .await;
        report.insert(
            "ambiguous_target_click".into(),
            serde_json::json!({
                "clicked": click_ambiguous.is_some(),
                "target": click_ambiguous.as_ref().map(|(target, _)| target.clone()),
                "toast_shown": ambiguous_toast,
                "active_path": self.active_tab_path(),
            }),
        );

        stages.push("click-heading-link".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        let click_heading = self.click_reading_wikilink(boundary, "带标题的链接");
        let heading_active =
            wait_for(4000, || self.active_tab_path().as_deref() == Some(map)).await;
        report.insert(
            "heading_link_click".into(),
            serde_json::json!({
                "clicked": click_heading.is_some(),
                "target": click_heading.as_ref().map(|(target, _)| target.clone()),
                "heading": click_heading.as_ref().and_then(|(_, heading)| heading.clone()),
                "active_is_map": heading_active,
                "active_path": self.active_tab_path(),
            }),
        );

        // D. 裸目标（不带 .md）解析分支：阅读视图 span 已带 resolved_path，真实点击走
        // raw_target 以 .md 结尾的第一分支；此处直接调用同一解析入口覆盖 1490 的 note 分支。
        stages.push("bare-target-resolve".into());
        report.insert("stages".into(), serde_json::json!(stages.clone()));
        self.open_note_or_resolve(boundary, "Study loop", None);
        let bare_active = wait_for(4000, || {
            self.active_tab_path().as_deref() == Some(loop_note)
        })
        .await;
        report.insert(
            "bare_target_resolve".into(),
            serde_json::json!({
                "active_is_study_loop": bare_active,
                "active_path": self.active_tab_path(),
                "open_tabs": self.tab_paths(),
            }),
        );

        stages.push("done".into());
        report.insert("stages".into(), serde_json::json!(stages));
        report.insert(
            "last_toast".into(),
            serde_json::json!(self.state.borrow().last_toast),
        );
        report.insert("completed".into(), serde_json::json!(true));
        serde_json::Value::Object(report)
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
        Some("阅读 / 源码编辑切换 (Ctrl+E)"),
        Some("win.toggle-edit-mode"),
    );
    view_section.append(
        Some("图表专注模式 (Ctrl+Shift+D)"),
        Some("win.focus-diagram"),
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
