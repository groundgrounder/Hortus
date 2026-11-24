// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 统一单层工具标签容器：左/右/底三区共用一套 `adw::TabView` + `adw::TabBar`，
//! 静态工具（文件/搜索/标签/大纲/反链/改动）与每个终端/Agent 实例完全平级同层、
//! 支持 `+` 添加/新建、`×` 关闭及三区无损迁移（`transfer_page`）。

use crate::core::{PanelRegion, StaticToolKind};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_config::{LayoutState, PanelGroup, RegionLayout};
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DockTabId {
    Static(StaticToolKind),
    Terminal(u64),
}

impl DockTabId {
    pub fn encode(self) -> String {
        match self {
            Self::Static(kind) => format!("static:{}", kind.id()),
            Self::Terminal(id) => format!("term:{id}"),
        }
    }

    pub fn decode(raw: &str) -> Option<Self> {
        if let Some(rest) = raw.strip_prefix("static:") {
            return StaticToolKind::parse(rest).map(Self::Static);
        }
        if let Some(rest) = raw.strip_prefix("term:") {
            return rest.parse::<u64>().ok().map(Self::Terminal);
        }
        None
    }
}

#[derive(Clone)]
pub struct DockCallbacks {
    pub on_request_close_terminal: Rc<dyn Fn(u64)>,
    pub on_region_emptied: Rc<dyn Fn(PanelRegion)>,
    pub on_region_reveal: Rc<dyn Fn(PanelRegion)>,
    pub on_region_collapse: Rc<dyn Fn(PanelRegion)>,
    pub on_terminal_selected: Rc<dyn Fn(u64)>,
    pub on_terminal_moved: Rc<dyn Fn(u64, PanelRegion)>,
    pub on_changes_shown: Rc<dyn Fn()>,
    pub on_layout_changed: Rc<dyn Fn()>,
}

struct DockTabEntry {
    region: PanelRegion,
    page: adw::TabPage,
}

struct RegionWidgets {
    root: gtk4::Box,
    tab_bar: adw::TabBar,
    tab_view: adw::TabView,
    content_stack: gtk4::Stack,
    add_btn: gtk4::MenuButton,
    add_menu: gio::Menu,
    context_menu: gio::Menu,
}

struct ToolDockState {
    static_widgets: HashMap<StaticToolKind, gtk4::Widget>,
    entries: HashMap<DockTabId, DockTabEntry>,
    preferred_terminal_region: PanelRegion,
    callbacks: Option<DockCallbacks>,
    suppress_callbacks: bool,
    disposing: bool,
}

struct ToolDockInner {
    regions: HashMap<PanelRegion, RegionWidgets>,
    state: RefCell<ToolDockState>,
}

#[derive(Clone)]
pub struct ToolDock {
    inner: Rc<ToolDockInner>,
}

impl ToolDock {
    pub fn new(static_widgets: HashMap<StaticToolKind, gtk4::Widget>) -> Self {
        let mut regions = HashMap::new();
        for region in PanelRegion::ALL {
            regions.insert(region, Self::build_region_widgets(region));
        }

        let inner = Rc::new(ToolDockInner {
            regions,
            state: RefCell::new(ToolDockState {
                static_widgets,
                entries: HashMap::new(),
                preferred_terminal_region: PanelRegion::Bottom,
                callbacks: None,
                suppress_callbacks: false,
                disposing: false,
            }),
        });

        Self::bind_region_signals(&inner);
        Self::rebuild_add_menus(&inner);
        Self { inner }
    }

    fn build_region_widgets(region: PanelRegion) -> RegionWidgets {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.set_vexpand(true);
        root.set_hexpand(true);
        root.add_css_class("hortus-tool-region");
        if region == PanelRegion::Bottom {
            root.add_css_class("hortus-bottom-panel");
        }

        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);
        tab_view.remove_shortcuts(adw::TabViewShortcuts::ALL_SHORTCUTS);

        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);
        tab_bar.add_css_class("inline");
        tab_bar.add_css_class("hortus-tool-tabbar");

        let add_menu = gio::Menu::new();
        let add_btn = gtk4::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("添加工具或新建终端")
            .menu_model(&add_menu)
            .build();
        add_btn.add_css_class("flat");

        let end_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        end_actions.set_margin_end(4);
        end_actions.append(&add_btn);

        if region == PanelRegion::Bottom {
            let collapse_btn = gtk4::Button::builder()
                .icon_name("pan-down-symbolic")
                .tooltip_text("收起底部面板 (Ctrl+J)")
                .build();
            collapse_btn.add_css_class("flat");
            collapse_btn.set_widget_name("hortus-bottom-collapse");
            end_actions.append(&collapse_btn);
        }

        tab_bar.set_end_action_widget(Some(&end_actions));

        let context_menu = gio::Menu::new();
        tab_view.set_menu_model(Some(&context_menu));

        let empty_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        empty_box.set_valign(gtk4::Align::Center);
        empty_box.set_halign(gtk4::Align::Center);
        empty_box.add_css_class("hortus-tool-empty");

        let empty_lbl = gtk4::Label::new(Some("此区域暂无工具"));
        empty_lbl.add_css_class("dim-label");
        empty_lbl.add_css_class("caption");
        let empty_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        empty_actions.set_halign(gtk4::Align::Center);
        let new_shell_btn = gtk4::Button::builder()
            .label("新建终端")
            .action_name("win.dock-new-shell")
            .action_target(&region.id().to_variant())
            .build();
        new_shell_btn.add_css_class("flat");
        new_shell_btn.add_css_class("caption");
        let new_agent_btn = gtk4::Button::builder()
            .label("启动 Agent")
            .action_name("win.dock-launch-agent")
            .action_target(&region.id().to_variant())
            .build();
        new_agent_btn.add_css_class("flat");
        new_agent_btn.add_css_class("caption");
        empty_actions.append(&new_shell_btn);
        empty_actions.append(&new_agent_btn);
        empty_box.append(&empty_lbl);
        empty_box.append(&empty_actions);

        let content_stack = gtk4::Stack::new();
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);
        content_stack.add_named(&tab_view, Some("tabs"));
        content_stack.add_named(&empty_box, Some("empty"));
        content_stack.set_visible_child_name("empty");

        root.append(&tab_bar);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&content_stack);

        RegionWidgets {
            root,
            tab_bar,
            tab_view,
            content_stack,
            add_btn,
            add_menu,
            context_menu,
        }
    }

    fn bind_region_signals(inner: &Rc<ToolDockInner>) {
        for (&region, rw) in &inner.regions {
            {
                let weak = Rc::downgrade(inner);
                rw.tab_view.connect_close_page(move |view, page| {
                    let Some(this) = weak.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    if this.state.borrow().disposing {
                        return glib::Propagation::Proceed;
                    }
                    Self::handle_close_page(&this, region, view, page);
                    glib::Propagation::Stop
                });
            }
            {
                let weak = Rc::downgrade(inner);
                rw.tab_view.connect_selected_page_notify(move |view| {
                    let Some(this) = weak.upgrade() else { return };
                    let (suppress, disposing) = {
                        let st = this.state.borrow();
                        (st.suppress_callbacks, st.disposing)
                    };
                    if suppress || disposing {
                        return;
                    }
                    let Some(page) = view.selected_page() else {
                        return;
                    };
                    let tab_id = this
                        .state
                        .borrow()
                        .entries
                        .iter()
                        .find(|(_, entry)| entry.page == page)
                        .map(|(id, _)| *id);
                    if let Some(tab_id) = tab_id {
                        let cbs = this.state.borrow().callbacks.clone();
                        match tab_id {
                            DockTabId::Terminal(tid) => {
                                if let Some(ref cbs) = cbs {
                                    (cbs.on_terminal_selected)(tid);
                                }
                            }
                            DockTabId::Static(kind) => {
                                if kind == StaticToolKind::Changes
                                    && let Some(ref cbs) = cbs
                                {
                                    (cbs.on_changes_shown)();
                                }
                                if let Some(ref cbs) = cbs {
                                    (cbs.on_layout_changed)();
                                }
                            }
                        }
                    }
                });
            }
            {
                let weak = Rc::downgrade(inner);
                rw.tab_view.connect_setup_menu(move |_, page_opt| {
                    let Some(this) = weak.upgrade() else { return };
                    Self::populate_context_menu(&this, region, page_opt);
                });
            }
            if region == PanelRegion::Bottom
                && let Some(end_box) = rw.tab_bar.end_action_widget().and_downcast::<gtk4::Box>()
                && let Some(collapse_btn) = end_box.last_child().and_downcast::<gtk4::Button>()
            {
                let weak = Rc::downgrade(inner);
                collapse_btn.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade()
                        && let Some(ref cbs) = this.state.borrow().callbacks
                    {
                        (cbs.on_region_collapse)(PanelRegion::Bottom);
                    }
                });
            }
            if let Some(popover) = rw.add_btn.popover() {
                let weak = Rc::downgrade(inner);
                popover.connect_show(move |_| {
                    if let Some(this) = weak.upgrade() {
                        Self::rebuild_add_menus(&this);
                    }
                });
            }
        }
    }

    fn populate_context_menu(
        inner: &Rc<ToolDockInner>,
        region: PanelRegion,
        page_opt: Option<&adw::TabPage>,
    ) {
        let Some(rw) = inner.regions.get(&region) else {
            return;
        };
        rw.context_menu.remove_all();
        let Some(page) = page_opt else {
            return;
        };
        let tab_id = inner
            .state
            .borrow()
            .entries
            .iter()
            .find(|(_, entry)| &entry.page == page)
            .map(|(id, _)| *id);
        let Some(tab_id) = tab_id else {
            return;
        };
        Self::fill_context_menu_for_tab(&rw.context_menu, region, tab_id);
    }

    fn fill_context_menu_for_tab(menu: &gio::Menu, current_region: PanelRegion, tab_id: DockTabId) {
        menu.remove_all();
        let move_sec = gio::Menu::new();
        for target in PanelRegion::ALL {
            if target == current_region {
                continue;
            }
            let item = gio::MenuItem::new(Some(&format!("移动到「{}」", target.title())), None);
            item.set_action_and_target_value(
                Some("win.dock-move-tab"),
                Some(&format!("{}:{}", target.id(), tab_id.encode()).to_variant()),
            );
            move_sec.append_item(&item);
        }
        menu.append_section(None, &move_sec);

        let close_sec = gio::Menu::new();
        let close_item = gio::MenuItem::new(Some("关闭标签"), None);
        close_item.set_action_and_target_value(
            Some("win.dock-close-tab"),
            Some(&tab_id.encode().to_variant()),
        );
        close_sec.append_item(&close_item);
        menu.append_section(None, &close_sec);
    }

    fn rebuild_add_menus(inner: &Rc<ToolDockInner>) {
        let locations: HashMap<StaticToolKind, PanelRegion> = {
            let st = inner.state.borrow();
            StaticToolKind::ALL
                .into_iter()
                .filter_map(|k| {
                    st.entries
                        .get(&DockTabId::Static(k))
                        .map(|entry| (k, entry.region))
                })
                .collect()
        };
        for (&region, rw) in &inner.regions {
            rw.add_menu.remove_all();

            let term_sec = gio::Menu::new();
            let shell_item = gio::MenuItem::new(Some("新建终端"), None);
            shell_item.set_action_and_target_value(
                Some("win.dock-new-shell"),
                Some(&region.id().to_variant()),
            );
            term_sec.append_item(&shell_item);

            let agent_item = gio::MenuItem::new(Some("启动 Agent"), None);
            agent_item.set_action_and_target_value(
                Some("win.dock-launch-agent"),
                Some(&region.id().to_variant()),
            );
            term_sec.append_item(&agent_item);
            rw.add_menu.append_section(None, &term_sec);

            let tools_sec = gio::Menu::new();
            for tool in StaticToolKind::ALL {
                let label = match locations.get(&tool) {
                    None => tool.title().to_string(),
                    Some(cur) if *cur == region => format!("{}（当前区域）", tool.title()),
                    Some(_) => format!("{}（移至此处）", tool.title()),
                };
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.dock-add-tool"),
                    Some(&format!("{}:{}", region.id(), tool.id()).to_variant()),
                );
                tools_sec.append_item(&item);
            }
            rw.add_menu.append_section(Some("工具面板"), &tools_sec);
        }
    }

    pub fn set_callbacks(&self, callbacks: DockCallbacks) {
        self.inner.state.borrow_mut().callbacks = Some(callbacks);
    }

    pub fn region_widget(&self, region: PanelRegion) -> &gtk4::Box {
        &self.inner.regions[&region].root
    }

    pub fn apply_default_layout(&self) {
        self.inner.state.borrow_mut().suppress_callbacks = true;
        for kind in StaticToolKind::ALL {
            Self::insert_or_move_static_impl(
                &self.inner,
                kind,
                kind.default_region(),
                false,
                false,
            );
        }
        if let Some(first) = self
            .inner
            .state
            .borrow()
            .entries
            .get(&DockTabId::Static(StaticToolKind::Files))
            .map(|e| e.page.clone())
        {
            self.inner.regions[&PanelRegion::Left]
                .tab_view
                .set_selected_page(&first);
        }
        if let Some(first) = self
            .inner
            .state
            .borrow()
            .entries
            .get(&DockTabId::Static(StaticToolKind::Outline))
            .map(|e| e.page.clone())
        {
            self.inner.regions[&PanelRegion::Right]
                .tab_view
                .set_selected_page(&first);
        }
        self.inner.state.borrow_mut().preferred_terminal_region = PanelRegion::Bottom;
        self.inner.state.borrow_mut().suppress_callbacks = false;
        Self::rebuild_add_menus(&self.inner);
    }

    /// 恢复已保存的 `LayoutState`（仅静态工具与 `"terminal"` 区域偏好，绝不恢复动态终端 PTY）。
    pub fn apply_layout_state(&self, layout: &LayoutState) -> (f64, f64, f64) {
        self.inner.state.borrow_mut().suppress_callbacks = true;
        // 先关闭全部静态工具页（保留 static_widgets），再按布局文件重新挂载。
        let static_pages: Vec<(StaticToolKind, PanelRegion, adw::TabPage)> = {
            let mut st = self.inner.state.borrow_mut();
            let mut out = Vec::new();
            for kind in StaticToolKind::ALL {
                if let Some(entry) = st.entries.remove(&DockTabId::Static(kind)) {
                    out.push((kind, entry.region, entry.page));
                }
            }
            out
        };
        for (_, region, page) in static_pages {
            if let Some(rw) = self.inner.regions.get(&region) {
                rw.tab_view.close_page(&page);
            }
        }

        let mut preferred_term = PanelRegion::Bottom;
        for region in PanelRegion::ALL {
            let Some(rl) = layout.regions.get(region.id()) else {
                continue;
            };
            let mut selected_static: Option<StaticToolKind> = None;
            for group in &rl.groups {
                for tab_name in &group.tabs {
                    if tab_name == "terminal" {
                        preferred_term = region;
                        continue;
                    }
                    if let Some(kind) = StaticToolKind::parse(tab_name) {
                        Self::insert_or_move_static_impl(&self.inner, kind, region, false, false);
                        if tab_name == &group.selected {
                            selected_static = Some(kind);
                        }
                    }
                }
            }
            if let Some(sel_kind) = selected_static
                && let Some(page) = self
                    .inner
                    .state
                    .borrow()
                    .entries
                    .get(&DockTabId::Static(sel_kind))
                    .map(|e| e.page.clone())
                && let Some(rw) = self.inner.regions.get(&region)
            {
                rw.tab_view.set_selected_page(&page);
            }
        }

        for region in PanelRegion::ALL {
            Self::sync_region_empty_stack(&self.inner, region);
        }
        self.inner.state.borrow_mut().preferred_terminal_region = preferred_term;
        self.inner.state.borrow_mut().suppress_callbacks = false;
        Self::rebuild_add_menus(&self.inner);

        let left_size = layout
            .regions
            .get("left")
            .map(|r| r.size.clamp(180.0, 480.0))
            .unwrap_or(290.0);
        let right_size = layout
            .regions
            .get("right")
            .map(|r| r.size.clamp(180.0, 480.0))
            .unwrap_or(290.0);
        let bottom_size = layout
            .regions
            .get("bottom")
            .map(|r| r.size.clamp(120.0, 520.0))
            .unwrap_or(260.0);
        (left_size, right_size, bottom_size)
    }

    pub fn export_layout_state(
        &self,
        left_size: f64,
        right_size: f64,
        bottom_size: f64,
    ) -> LayoutState {
        let st = self.inner.state.borrow();
        let mut regions = BTreeMap::new();
        for region in PanelRegion::ALL {
            let rw = &self.inner.regions[&region];
            let n_pages = rw.tab_view.n_pages();
            let mut tabs = Vec::new();
            let mut selected_tab: Option<String> = None;
            let active_page = rw.tab_view.selected_page();
            for idx in 0..n_pages {
                let page = rw.tab_view.nth_page(idx);
                if let Some((DockTabId::Static(kind), _)) =
                    st.entries.iter().find(|(_, entry)| entry.page == page)
                {
                    let id_str = kind.id().to_string();
                    if active_page.as_ref() == Some(&page) {
                        selected_tab = Some(id_str.clone());
                    }
                    tabs.push(id_str);
                }
            }
            if st.preferred_terminal_region == region {
                tabs.push("terminal".to_string());
                if selected_tab.is_none() {
                    selected_tab = Some("terminal".to_string());
                }
            }
            let groups = if tabs.is_empty() {
                Vec::new()
            } else {
                let selected = selected_tab.unwrap_or_else(|| tabs[0].clone());
                vec![PanelGroup {
                    id: format!("{}-main", region.id()),
                    tabs,
                    selected,
                }]
            };
            let size = match region {
                PanelRegion::Left => left_size.clamp(180.0, 480.0),
                PanelRegion::Right => right_size.clamp(180.0, 480.0),
                PanelRegion::Bottom => bottom_size.clamp(120.0, 520.0),
            };
            regions.insert(
                region.id().to_string(),
                RegionLayout {
                    groups,
                    visible: None,
                    size,
                },
            );
        }
        LayoutState { regions }
    }

    pub fn open_or_move_static_tool(
        &self,
        kind: StaticToolKind,
        target_region: PanelRegion,
        reveal: bool,
    ) {
        Self::insert_or_move_static_impl(&self.inner, kind, target_region, true, reveal);
    }

    pub fn activate_static_tool(&self, kind: StaticToolKind) {
        let region = self
            .static_tool_region(kind)
            .unwrap_or_else(|| kind.default_region());
        Self::insert_or_move_static_impl(&self.inner, kind, region, true, true);
    }

    fn insert_or_move_static_impl(
        inner: &Rc<ToolDockInner>,
        kind: StaticToolKind,
        target_region: PanelRegion,
        activate: bool,
        reveal: bool,
    ) {
        let tab_id = DockTabId::Static(kind);
        let existing_region = inner.state.borrow().entries.get(&tab_id).map(|e| e.region);
        if let Some(src_region) = existing_region {
            if src_region == target_region {
                if activate
                    && let Some(page) = inner
                        .state
                        .borrow()
                        .entries
                        .get(&tab_id)
                        .map(|e| e.page.clone())
                    && let Some(rw) = inner.regions.get(&target_region)
                {
                    rw.tab_view.set_selected_page(&page);
                }
                if reveal && let Some(ref cbs) = inner.state.borrow().callbacks {
                    (cbs.on_region_reveal)(target_region);
                }
                if kind == StaticToolKind::Changes
                    && activate
                    && let Some(ref cbs) = inner.state.borrow().callbacks
                {
                    (cbs.on_changes_shown)();
                }
                return;
            }
            Self::move_tab_impl(inner, tab_id, target_region, reveal);
            return;
        }

        let widget = inner.state.borrow().static_widgets.get(&kind).cloned();
        let Some(widget) = widget else {
            return;
        };
        let Some(rw) = inner.regions.get(&target_region) else {
            return;
        };
        let page = rw.tab_view.append(&widget);
        page.set_title(kind.title());
        page.set_icon(Some(&gio::ThemedIcon::new(kind.icon_name())));
        page.set_tooltip(kind.title());
        inner.state.borrow_mut().entries.insert(
            tab_id,
            DockTabEntry {
                region: target_region,
                page: page.clone(),
            },
        );
        Self::sync_region_empty_stack(inner, target_region);
        if activate {
            rw.tab_view.set_selected_page(&page);
        }
        let suppress = inner.state.borrow().suppress_callbacks;
        if !suppress {
            Self::rebuild_add_menus(inner);
            let cbs = inner.state.borrow().callbacks.clone();
            if let Some(ref cbs) = cbs {
                if reveal {
                    (cbs.on_region_reveal)(target_region);
                }
                if kind == StaticToolKind::Changes && activate {
                    (cbs.on_changes_shown)();
                }
                (cbs.on_layout_changed)();
            }
        }
    }

    pub fn move_tab(&self, tab_id: DockTabId, target_region: PanelRegion) {
        Self::move_tab_impl(&self.inner, tab_id, target_region, true);
    }

    fn move_tab_impl(
        inner: &Rc<ToolDockInner>,
        tab_id: DockTabId,
        target_region: PanelRegion,
        reveal: bool,
    ) {
        let info = inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| (e.region, e.page.clone()));
        let Some((src_region, page)) = info else {
            return;
        };
        let Some(target_rw) = inner.regions.get(&target_region) else {
            return;
        };
        if src_region == target_region {
            target_rw.tab_view.set_selected_page(&page);
            if reveal && let Some(ref cbs) = inner.state.borrow().callbacks {
                (cbs.on_region_reveal)(target_region);
            }
            return;
        }
        let Some(src_rw) = inner.regions.get(&src_region) else {
            return;
        };

        inner.state.borrow_mut().suppress_callbacks = true;
        let pos = target_rw.tab_view.n_pages();
        src_rw
            .tab_view
            .transfer_page(&page, &target_rw.tab_view, pos);
        if let Some(entry) = inner.state.borrow_mut().entries.get_mut(&tab_id) {
            entry.region = target_region;
        }
        if matches!(tab_id, DockTabId::Terminal(_)) {
            inner.state.borrow_mut().preferred_terminal_region = target_region;
        }
        target_rw.tab_view.set_selected_page(&page);
        Self::sync_region_empty_stack(inner, src_region);
        Self::sync_region_empty_stack(inner, target_region);
        inner.state.borrow_mut().suppress_callbacks = false;

        Self::rebuild_add_menus(inner);
        let src_empty = src_rw.tab_view.n_pages() == 0;
        let cbs = inner.state.borrow().callbacks.clone();
        if let Some(ref cbs) = cbs {
            if src_empty {
                (cbs.on_region_emptied)(src_region);
            }
            if reveal {
                (cbs.on_region_reveal)(target_region);
            }
            match tab_id {
                DockTabId::Terminal(tid) => {
                    (cbs.on_terminal_moved)(tid, target_region);
                    (cbs.on_terminal_selected)(tid);
                }
                DockTabId::Static(StaticToolKind::Changes) => {
                    (cbs.on_changes_shown)();
                }
                DockTabId::Static(_) => {}
            }
            (cbs.on_layout_changed)();
        }
    }

    pub fn close_tab(&self, tab_id: DockTabId) {
        let info = self
            .inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| (e.region, e.page.clone()));
        let Some((region, page)) = info else {
            return;
        };
        if let Some(rw) = self.inner.regions.get(&region) {
            rw.tab_view.close_page(&page);
        }
    }

    fn handle_close_page(
        inner: &Rc<ToolDockInner>,
        region: PanelRegion,
        view: &adw::TabView,
        page: &adw::TabPage,
    ) {
        let tab_id = {
            let mut st = inner.state.borrow_mut();
            let id = st
                .entries
                .iter()
                .find(|(_, entry)| &entry.page == page)
                .map(|(k, _)| *k);
            if let Some(k) = id {
                st.entries.remove(&k);
            }
            id
        };
        view.close_page_finish(page, true);
        Self::sync_region_empty_stack(inner, region);
        if inner.state.borrow().suppress_callbacks {
            return;
        }
        Self::rebuild_add_menus(inner);

        let cbs = inner.state.borrow().callbacks.clone();
        if let Some(ref cbs) = cbs {
            match tab_id {
                Some(DockTabId::Terminal(tid)) => {
                    (cbs.on_request_close_terminal)(tid);
                }
                Some(DockTabId::Static(_)) => {
                    (cbs.on_layout_changed)();
                }
                None => {}
            }
            if view.n_pages() == 0 {
                (cbs.on_region_emptied)(region);
            }
        }
    }

    pub fn insert_terminal_tab(
        &self,
        term_id: u64,
        region: PanelRegion,
        widget: &gtk4::Widget,
        title: &str,
        icon_name: &str,
    ) {
        let Some(rw) = self.inner.regions.get(&region) else {
            return;
        };
        let page = rw.tab_view.append(widget);
        page.set_title(title);
        page.set_icon(Some(&gio::ThemedIcon::new(icon_name)));
        page.set_tooltip(title);
        {
            let mut st = self.inner.state.borrow_mut();
            st.entries.insert(
                DockTabId::Terminal(term_id),
                DockTabEntry {
                    region,
                    page: page.clone(),
                },
            );
            if st.preferred_terminal_region != region {
                st.preferred_terminal_region = region;
            }
        }
        Self::sync_region_empty_stack(&self.inner, region);
        rw.tab_view.set_selected_page(&page);
        let cbs = self.inner.state.borrow().callbacks.clone();
        if let Some(ref cbs) = cbs {
            (cbs.on_region_reveal)(region);
            (cbs.on_layout_changed)();
        }
    }

    pub fn update_terminal_tab(&self, term_id: u64, title: &str, icon_name: &str) {
        let page = self
            .inner
            .state
            .borrow()
            .entries
            .get(&DockTabId::Terminal(term_id))
            .map(|e| e.page.clone());
        if let Some(page) = page {
            page.set_title(title);
            page.set_icon(Some(&gio::ThemedIcon::new(icon_name)));
            page.set_tooltip(title);
        }
    }

    pub fn select_terminal_tab(&self, term_id: u64) {
        let info = self
            .inner
            .state
            .borrow()
            .entries
            .get(&DockTabId::Terminal(term_id))
            .map(|e| (e.region, e.page.clone()));
        let Some((region, page)) = info else {
            return;
        };
        if let Some(rw) = self.inner.regions.get(&region) {
            rw.tab_view.set_selected_page(&page);
        }
        if let Some(ref cbs) = self.inner.state.borrow().callbacks {
            (cbs.on_region_reveal)(region);
        }
    }

    pub fn close_terminal_tab(&self, term_id: u64) {
        self.close_tab(DockTabId::Terminal(term_id));
    }

    fn sync_region_empty_stack(inner: &Rc<ToolDockInner>, region: PanelRegion) {
        if let Some(rw) = inner.regions.get(&region) {
            let name = if rw.tab_view.n_pages() > 0 {
                "tabs"
            } else {
                "empty"
            };
            rw.content_stack.set_visible_child_name(name);
        }
    }

    pub fn tab_count(&self, region: PanelRegion) -> usize {
        self.inner
            .regions
            .get(&region)
            .map(|rw| rw.tab_view.n_pages() as usize)
            .unwrap_or(0)
    }

    pub fn static_tool_region(&self, kind: StaticToolKind) -> Option<PanelRegion> {
        self.inner
            .state
            .borrow()
            .entries
            .get(&DockTabId::Static(kind))
            .map(|e| e.region)
    }

    #[cfg(feature = "automation")]
    pub fn tab_region(&self, tab_id: DockTabId) -> Option<PanelRegion> {
        self.inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| e.region)
    }

    #[cfg(feature = "automation")]
    pub fn active_tab_in_region(&self, region: PanelRegion) -> Option<DockTabId> {
        let page = self.inner.regions.get(&region)?.tab_view.selected_page()?;
        self.inner
            .state
            .borrow()
            .entries
            .iter()
            .find(|(_, e)| e.region == region && e.page == page)
            .map(|(id, _)| *id)
    }

    pub fn preferred_terminal_region(&self) -> PanelRegion {
        self.inner.state.borrow().preferred_terminal_region
    }

    pub fn teardown(&self) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.disposing = true;
            st.callbacks = None;
            st.entries.clear();
            st.static_widgets.clear();
        }
        for rw in self.inner.regions.values() {
            rw.add_menu.remove_all();
            rw.context_menu.remove_all();
        }
    }

    #[cfg(feature = "automation")]
    pub fn region_tab_titles(&self, region: PanelRegion) -> Vec<String> {
        let Some(rw) = self.inner.regions.get(&region) else {
            return Vec::new();
        };
        (0..rw.tab_view.n_pages())
            .map(|idx| rw.tab_view.nth_page(idx).title().to_string())
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn region_has_add_button(&self, region: PanelRegion) -> bool {
        self.inner
            .regions
            .get(&region)
            .is_some_and(|rw| rw.tab_bar.end_action_widget().is_some())
    }

    #[cfg(feature = "automation")]
    pub fn region_visible_tab_bars(&self, region: PanelRegion) -> usize {
        let Some(rw) = self.inner.regions.get(&region) else {
            return 0;
        };
        count_tab_bars_in_widget(rw.root.upcast_ref::<gtk4::Widget>())
    }

    #[cfg(feature = "automation")]
    pub fn add_menu_labels(&self, region: PanelRegion) -> Vec<String> {
        Self::rebuild_add_menus(&self.inner);
        let Some(rw) = self.inner.regions.get(&region) else {
            return Vec::new();
        };
        collect_menu_labels(rw.add_menu.upcast_ref())
    }

    #[cfg(feature = "automation")]
    pub fn context_menu_labels_for(&self, tab_id: DockTabId) -> Vec<String> {
        let Some(region) = self.tab_region(tab_id) else {
            return Vec::new();
        };
        let menu = gio::Menu::new();
        Self::fill_context_menu_for_tab(&menu, region, tab_id);
        collect_menu_labels(menu.upcast_ref())
    }

    #[cfg(feature = "automation")]
    pub fn popup_add_menu(&self, region: PanelRegion) -> bool {
        Self::rebuild_add_menus(&self.inner);
        let Some(rw) = self.inner.regions.get(&region) else {
            return false;
        };
        rw.add_btn.popup();
        true
    }

    #[cfg(feature = "automation")]
    pub fn popdown_add_menu(&self, region: PanelRegion) {
        if let Some(rw) = self.inner.regions.get(&region) {
            rw.add_btn.popdown();
        }
    }

    #[cfg(feature = "automation")]
    pub fn add_button_widget(&self, region: PanelRegion) -> Option<gtk4::MenuButton> {
        self.inner.regions.get(&region).map(|rw| rw.add_btn.clone())
    }
}

#[cfg(feature = "automation")]
fn count_tab_bars_in_widget(root: &gtk4::Widget) -> usize {
    let mut count = 0usize;
    if root.is::<adw::TabBar>() {
        count += 1;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        count += count_tab_bars_in_widget(&c);
        child = c.next_sibling();
    }
    count
}

#[cfg(feature = "automation")]
fn collect_menu_labels(menu: &gio::MenuModel) -> Vec<String> {
    let mut out = Vec::new();
    for idx in 0..menu.n_items() {
        if let Some(val) = menu.item_attribute_value(idx, "label", Some(glib::VariantTy::STRING))
            && let Some(s) = val.str()
        {
            out.push(s.to_string());
        }
        if let Some(sub) = menu.item_link(idx, gio::MENU_LINK_SECTION) {
            out.extend(collect_menu_labels(&sub));
        }
        if let Some(sub) = menu.item_link(idx, gio::MENU_LINK_SUBMENU) {
            out.extend(collect_menu_labels(&sub));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn dummy_static_widgets() -> HashMap<StaticToolKind, gtk4::Widget> {
        StaticToolKind::ALL
            .into_iter()
            .map(|k| {
                let b = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                b.set_widget_name(k.id());
                (k, b.upcast::<gtk4::Widget>())
            })
            .collect()
    }

    #[test]
    fn tool_dock_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let dock = ToolDock::new(dummy_static_widgets());
        dock.apply_default_layout();
        let weak = Rc::downgrade(&dock.inner);
        assert_eq!(weak.strong_count(), 1);
        dock.teardown();
        drop(dock);
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn tool_dock_transfer_preserves_widget_identity_and_auto_collapses() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let widgets = dummy_static_widgets();
        let backlinks_ptr = widgets[&StaticToolKind::Backlinks].as_ptr();
        let dock = ToolDock::new(widgets);
        dock.apply_default_layout();

        let emptied_regions: Rc<RefCell<Vec<PanelRegion>>> = Rc::new(RefCell::new(Vec::new()));
        let emptied_cb = Rc::clone(&emptied_regions);
        dock.set_callbacks(DockCallbacks {
            on_request_close_terminal: Rc::new(|_| {}),
            on_region_emptied: Rc::new(move |r| emptied_cb.borrow_mut().push(r)),
            on_region_reveal: Rc::new(|_| {}),
            on_region_collapse: Rc::new(|_| {}),
            on_terminal_selected: Rc::new(|_| {}),
            on_terminal_moved: Rc::new(|_, _| {}),
            on_changes_shown: Rc::new(|| {}),
            on_layout_changed: Rc::new(|| {}),
        });

        assert_eq!(
            dock.static_tool_region(StaticToolKind::Backlinks),
            Some(PanelRegion::Right)
        );
        dock.move_tab(
            DockTabId::Static(StaticToolKind::Backlinks),
            PanelRegion::Bottom,
        );
        assert_eq!(
            dock.static_tool_region(StaticToolKind::Backlinks),
            Some(PanelRegion::Bottom)
        );
        assert_eq!(dock.tab_count(PanelRegion::Bottom), 1);

        // 移动回去后清空 Bottom，必须触发 Bottom 自动收起回调。
        dock.move_tab(
            DockTabId::Static(StaticToolKind::Backlinks),
            PanelRegion::Left,
        );
        assert_eq!(
            dock.static_tool_region(StaticToolKind::Backlinks),
            Some(PanelRegion::Left)
        );
        assert_eq!(dock.tab_count(PanelRegion::Bottom), 0);
        assert_eq!(&*emptied_regions.borrow(), &[PanelRegion::Bottom]);

        let page_child_ptr = dock.inner.state.borrow().entries
            [&DockTabId::Static(StaticToolKind::Backlinks)]
            .page
            .child()
            .as_ptr();
        assert_eq!(page_child_ptr, backlinks_ptr);

        dock.teardown();
    }

    #[test]
    fn tool_dock_layout_state_roundtrip_excludes_dynamic_terminals() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let dock = ToolDock::new(dummy_static_widgets());
        dock.apply_default_layout();
        let term_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        dock.insert_terminal_tab(
            99,
            PanelRegion::Right,
            term_box.upcast_ref(),
            "终端 99",
            "utilities-terminal-symbolic",
        );

        let layout = dock.export_layout_state(300.0, 340.0, 260.0);
        assert!(layout.validate().is_ok());
        let json = serde_json::to_string(&layout).unwrap();
        assert!(!json.contains("99"));
        assert!(json.contains("\"terminal\""));

        let closed_count = Rc::new(Cell::new(0usize));
        let closed_cb = Rc::clone(&closed_count);
        dock.set_callbacks(DockCallbacks {
            on_request_close_terminal: Rc::new(move |_| {
                closed_cb.set(closed_cb.get() + 1);
            }),
            on_region_emptied: Rc::new(|_| {}),
            on_region_reveal: Rc::new(|_| {}),
            on_region_collapse: Rc::new(|_| {}),
            on_terminal_selected: Rc::new(|_| {}),
            on_terminal_moved: Rc::new(|_, _| {}),
            on_changes_shown: Rc::new(|| {}),
            on_layout_changed: Rc::new(|| {}),
        });
        dock.close_terminal_tab(99);
        assert_eq!(closed_count.get(), 1);
        dock.teardown();
    }
}
