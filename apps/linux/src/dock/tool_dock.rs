// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 统一单层工具标签容器：左/右/底三区共用一套原生 GTK4 动态宽度标签条（`ToolTabStrip`）
//! 与底层 `adw::TabView` 内容宿主；静态工具与终端/Agent 实例平级同层、支持 `+` 添加、
//! 右键菜单 / 中键关闭、独立 PopoverMenu 右键迁移以及三区无损迁移（`transfer_page`）。

use crate::core::{PanelRegion, StaticToolKind};
use crate::i18n::{Key, fmt1, tr};
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_config::{LayoutState, PanelGroup, RegionLayout};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
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

struct TabStripChipInner {
    region: Cell<PanelRegion>,
    root: gtk4::Box,
    center_box: gtk4::Box,
    icon: gtk4::Image,
    label: gtk4::Label,
    selected: Cell<bool>,
}

#[derive(Clone)]
struct TabStripChip {
    inner: Rc<TabStripChipInner>,
}

impl TabStripChip {
    fn sync_visual(&self) {
        let selected = self.inner.selected.get();
        if selected {
            self.inner.root.add_css_class("selected");
        } else {
            self.inner.root.remove_css_class("selected");
        }
    }

    fn set_selected(&self, selected: bool) {
        self.inner.selected.set(selected);
        self.sync_visual();
    }

    fn configure_for_region(&self, region: PanelRegion) {
        self.inner.region.set(region);
        let is_bottom = region == PanelRegion::Bottom;
        self.inner.label.set_ellipsize(pango::EllipsizeMode::None);
        self.inner.label.set_max_width_chars(-1);
        if is_bottom {
            self.inner.root.remove_css_class("hortus-tool-tab-sidebar");
            self.inner.root.add_css_class("hortus-tool-tab-bottom");
            self.inner.root.set_hexpand(false);
            self.inner.center_box.set_hexpand(false);
            self.inner.label.set_visible(true);
        } else {
            self.inner.root.remove_css_class("hortus-tool-tab-bottom");
            self.inner.root.add_css_class("hortus-tool-tab-sidebar");
            self.inner.root.set_hexpand(true);
            self.inner.center_box.set_hexpand(true);
        }
    }

    fn apply_sidebar_fit(&self, label_visible: bool, root_hexpand: bool) {
        if self.inner.label.is_visible() != label_visible {
            self.inner.label.set_visible(label_visible);
        }
        if self.inner.root.hexpands() != root_hexpand {
            self.inner.root.set_hexpand(root_hexpand);
        }
    }

    fn update(&self, title: &str, icon_name: &str) {
        self.inner.icon.set_icon_name(Some(icon_name));
        self.inner.label.set_text(title);
        self.inner.root.set_tooltip_text(Some(title));
    }

    fn widget(&self) -> &gtk4::Box {
        &self.inner.root
    }
}

struct DockTabEntry {
    region: PanelRegion,
    page: adw::TabPage,
    title: String,
    chip: TabStripChip,
}

struct RegionWidgets {
    root: gtk4::Box,
    #[cfg(feature = "automation")]
    strip_bar: gtk4::Box,
    strip_scroll: gtk4::ScrolledWindow,
    tabs_box: gtk4::Box,
    tab_view: adw::TabView,
    content_stack: gtk4::Stack,
    add_btn: gtk4::MenuButton,
    add_menu: gio::Menu,
    collapse_btn: Option<gtk4::Button>,
    last_strip_width: Cell<i32>,
    relayout_scheduled: Cell<bool>,
}

struct ActiveContextMenu {
    #[cfg(feature = "automation")]
    tab_id: DockTabId,
    popover: gtk4::PopoverMenu,
    _menu: gio::Menu,
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
    active_context_menu: RefCell<Option<ActiveContextMenu>>,
}

#[derive(Clone)]
pub struct ToolDock {
    inner: Rc<ToolDockInner>,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, serde::Serialize)]
pub struct TabChipBounds {
    pub id: String,
    pub title: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub fits_in_strip: bool,
    pub selected: bool,
    pub has_close_button: bool,
    pub has_motion_controller: bool,
    pub label_visible: bool,
    pub ellipsized: bool,
}

#[cfg(feature = "automation")]
#[derive(Clone, Debug, serde::Serialize)]
pub struct StripLayoutMetrics {
    pub region: String,
    pub strip_width: f32,
    pub tabs: Vec<TabChipBounds>,
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
            active_context_menu: RefCell::new(None),
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

        // 原生 GTK4 动态宽度单层工具标签条：[ tabs_host (hexpand=true) | + | (Bottom) v ]
        let strip_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        strip_bar.add_css_class("hortus-tool-strip");
        strip_bar.set_widget_name(&format!("hortus-tool-strip-{}", region.id()));
        strip_bar.set_hexpand(true);

        let tabs_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        tabs_box.add_css_class("hortus-tool-tabs-box");
        tabs_box.set_hexpand(true);
        tabs_box.set_valign(gtk4::Align::Fill);

        let strip_scroll = gtk4::ScrolledWindow::new();
        strip_scroll.set_hexpand(true);
        strip_scroll.set_vexpand(false);
        strip_scroll.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::Never);
        strip_scroll.set_propagate_natural_width(false);
        strip_scroll.set_propagate_natural_height(true);
        strip_scroll.set_min_content_width(0);
        strip_scroll.set_child(Some(&tabs_box));

        let add_menu = gio::Menu::new();
        let add_btn = gtk4::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr(Key::DockAddTooltip))
            .menu_model(&add_menu)
            .valign(gtk4::Align::Center)
            .build();
        add_btn.add_css_class("flat");
        add_btn.add_css_class("hortus-tool-action-btn");

        let end_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        end_actions.set_valign(gtk4::Align::Center);
        end_actions.append(&add_btn);

        let collapse_btn = if region == PanelRegion::Bottom {
            let btn = gtk4::Button::builder()
                .icon_name("pan-down-symbolic")
                .tooltip_text(tr(Key::DockCollapseBottomTooltip))
                .valign(gtk4::Align::Center)
                .build();
            btn.add_css_class("flat");
            btn.add_css_class("hortus-tool-action-btn");
            btn.set_widget_name("hortus-bottom-collapse");
            end_actions.append(&btn);
            Some(btn)
        } else {
            None
        };

        strip_bar.append(&strip_scroll);
        strip_bar.append(&end_actions);

        let empty_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        empty_box.set_valign(gtk4::Align::Center);
        empty_box.set_halign(gtk4::Align::Center);
        empty_box.add_css_class("hortus-tool-empty");

        let empty_lbl = gtk4::Label::new(Some(tr(Key::DockEmptyLabel)));
        empty_lbl.add_css_class("dim-label");
        empty_lbl.add_css_class("caption");
        empty_lbl.set_wrap(true);
        empty_lbl.set_justify(gtk4::Justification::Center);
        let empty_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        empty_actions.set_halign(gtk4::Align::Center);
        let new_shell_btn = gtk4::Button::builder()
            .label(tr(Key::DockNewTerminal))
            .action_name("win.dock-new-shell")
            .action_target(&region.id().to_variant())
            .build();
        new_shell_btn.add_css_class("flat");
        new_shell_btn.add_css_class("caption");
        let new_agent_btn = gtk4::Button::builder()
            .label(tr(Key::MenuLaunchAgent))
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
        content_stack.set_hhomogeneous(false);
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);
        content_stack.add_named(&tab_view, Some("tabs"));
        content_stack.add_named(&empty_box, Some("empty"));
        content_stack.set_visible_child_name("empty");

        root.append(&strip_bar);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&content_stack);

        RegionWidgets {
            root,
            #[cfg(feature = "automation")]
            strip_bar,
            strip_scroll,
            tabs_box,
            tab_view,
            content_stack,
            add_btn,
            add_menu,
            collapse_btn,
            last_strip_width: Cell::new(-1),
            relayout_scheduled: Cell::new(false),
        }
    }

    fn build_tab_chip(
        inner: &Rc<ToolDockInner>,
        tab_id: DockTabId,
        region: PanelRegion,
        title: &str,
        icon_name: &str,
    ) -> TabStripChip {
        let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        root.add_css_class("hortus-tool-tab");
        root.set_overflow(gtk4::Overflow::Hidden);
        root.set_valign(gtk4::Align::Fill);
        root.set_margin_top(2);
        root.set_margin_bottom(2);
        root.set_tooltip_text(Some(title));

        let center_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        center_box.set_halign(gtk4::Align::Center);
        center_box.set_valign(gtk4::Align::Center);

        let icon = gtk4::Image::from_icon_name(icon_name);
        icon.set_pixel_size(14);
        icon.set_valign(gtk4::Align::Center);

        let label = gtk4::Label::new(Some(title));
        label.set_single_line_mode(true);
        label.set_ellipsize(pango::EllipsizeMode::None);
        label.set_xalign(0.5);
        label.set_valign(gtk4::Align::Center);
        label.add_css_class("caption");

        center_box.append(&icon);
        center_box.append(&label);

        root.append(&center_box);

        let chip = TabStripChip {
            inner: Rc::new(TabStripChipInner {
                region: Cell::new(region),
                root: root.clone(),
                center_box,
                icon,
                label,
                selected: Cell::new(false),
            }),
        };
        chip.configure_for_region(region);

        // 左键点击切换标签页（不注册悬停布局控制器，确保鼠标滑过标签栏纹丝不动）
        let left_click = gtk4::GestureClick::new();
        left_click.set_button(1);
        {
            let dock_weak = Rc::downgrade(inner);
            left_click.connect_released(move |_, _, _, _| {
                if let Some(this) = dock_weak.upgrade() {
                    Self::select_tab_impl(&this, tab_id);
                }
            });
        }
        root.add_controller(left_click);

        // 中键点击关闭标签页
        let mid_click = gtk4::GestureClick::new();
        mid_click.set_button(2);
        {
            let dock_weak = Rc::downgrade(inner);
            mid_click.connect_released(move |_, _, _, _| {
                if let Some(this) = dock_weak.upgrade() {
                    Self::close_tab_impl(&this, tab_id);
                }
            });
        }
        root.add_controller(mid_click);

        // 右键点击弹出独立 PopoverMenu（关闭时不清空 GMenu，由 idle_add_local_once 延迟卸载）
        let right_click = gtk4::GestureClick::new();
        right_click.set_button(3);
        {
            let dock_weak = Rc::downgrade(inner);
            right_click.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                if let Some(this) = dock_weak.upgrade() {
                    Self::show_context_menu_for_tab(&this, tab_id);
                }
            });
        }
        root.add_controller(right_click);

        chip
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
                    if this.state.borrow().disposing {
                        return;
                    }
                    Self::sync_strip_selection(&this, region);
                    if this.state.borrow().suppress_callbacks {
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
            if let Some(ref collapse_btn) = rw.collapse_btn {
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
            if region != PanelRegion::Bottom {
                let weak = Rc::downgrade(inner);
                rw.strip_scroll
                    .hadjustment()
                    .connect_page_size_notify(move |adj| {
                        let Some(this) = weak.upgrade() else { return };
                        if this.state.borrow().disposing {
                            return;
                        }
                        let w = adj.page_size().round() as i32;
                        if let Some(rw_cur) = this.regions.get(&region)
                            && (w - rw_cur.last_strip_width.get()).abs() >= 1
                        {
                            rw_cur.last_strip_width.set(w);
                            Self::schedule_strip_reconcile(&this, region);
                        }
                    });
            }
        }
    }

    fn schedule_strip_reconcile(inner: &Rc<ToolDockInner>, region: PanelRegion) {
        if region == PanelRegion::Bottom {
            return;
        }
        let Some(rw) = inner.regions.get(&region) else {
            return;
        };
        if rw.relayout_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(inner);
        glib::idle_add_local_once(move || {
            let Some(this) = weak.upgrade() else { return };
            if let Some(rw) = this.regions.get(&region) {
                rw.relayout_scheduled.set(false);
            }
            if !this.state.borrow().disposing {
                Self::reconcile_strip_labels(&this, region, None);
            }
        });
    }

    fn reconcile_strip_labels(
        inner: &Rc<ToolDockInner>,
        region: PanelRegion,
        width_override: Option<i32>,
    ) {
        let Some(rw) = inner.regions.get(&region) else {
            return;
        };
        if region == PanelRegion::Bottom {
            let st = inner.state.borrow();
            for entry in st.entries.values() {
                if entry.region == region {
                    entry.chip.apply_sidebar_fit(true, false);
                }
            }
            return;
        }
        let avail = width_override.unwrap_or_else(|| {
            let sw = rw.strip_scroll.width();
            if sw > 0 {
                sw
            } else {
                rw.strip_scroll.hadjustment().page_size().round() as i32
            }
        });
        if avail <= 0 {
            return;
        }

        let n_pages = rw.tab_view.n_pages();
        if n_pages == 0 {
            return;
        }
        let selected_page = rw.tab_view.selected_page();

        let st = inner.state.borrow();
        struct ChipMeasure {
            chip: TabStripChip,
            selected: bool,
            full_w: i32,
            icon_w: i32,
        }
        let mut items = Vec::with_capacity(n_pages as usize);
        for idx in 0..n_pages {
            let page = rw.tab_view.nth_page(idx);
            let Some(entry) = st.entries.values().find(|e| e.page == page) else {
                continue;
            };
            let is_sel = selected_page.as_ref() == Some(&entry.page);
            let layout = entry
                .chip
                .inner
                .label
                .create_pango_layout(Some(&entry.title));
            let text_w = layout.pixel_size().0 + 6;
            // padding(16) + icon(14) + center gap(4) + border/slack(4)
            let full_w = text_w + 16 + 14 + 4 + 4;
            let icon_w = 16 + 14 + 2;
            items.push(ChipMeasure {
                chip: entry.chip.clone(),
                selected: is_sel,
                full_w,
                icon_w,
            });
        }
        drop(st);

        let n = items.len();
        if n == 0 {
            return;
        }
        let spacing = 2 * (n as i32 - 1);
        let all_full_sum: i32 = items.iter().map(|m| m.full_w).sum::<i32>() + spacing;
        if all_full_sum <= avail {
            for item in items {
                item.chip.apply_sidebar_fit(true, true);
            }
            return;
        }

        let active_idx = items.iter().position(|m| m.selected).unwrap_or(0);
        let active_full_sum: i32 = items
            .iter()
            .enumerate()
            .map(|(i, m)| if i == active_idx { m.full_w } else { m.icon_w })
            .sum::<i32>()
            + spacing;
        if active_full_sum <= avail {
            for (i, item) in items.into_iter().enumerate() {
                let keep_label = i == active_idx;
                item.chip.apply_sidebar_fit(keep_label, keep_label);
            }
            return;
        }

        for item in items {
            item.chip.apply_sidebar_fit(false, true);
        }
    }

    fn sync_strip_selection(inner: &Rc<ToolDockInner>, region: PanelRegion) {
        let Some(rw) = inner.regions.get(&region) else {
            return;
        };
        let selected_page = rw.tab_view.selected_page();
        let mut selected_chip: Option<gtk4::Box> = None;
        {
            let st = inner.state.borrow();
            for entry in st.entries.values() {
                if entry.region == region {
                    let is_sel = selected_page.as_ref() == Some(&entry.page);
                    entry.chip.set_selected(is_sel);
                    if is_sel {
                        selected_chip = Some(entry.chip.widget().clone());
                    }
                }
            }
        }
        Self::reconcile_strip_labels(inner, region, None);
        if let Some(chip_widget) = selected_chip {
            Self::ensure_chip_visible(rw, &chip_widget);
        }
    }

    fn ensure_chip_visible(rw: &RegionWidgets, chip_widget: &gtk4::Box) {
        let Some(bounds) = chip_widget.compute_bounds(&rw.tabs_box) else {
            return;
        };
        let adj = rw.strip_scroll.hadjustment();
        let page_size = adj.page_size();
        if page_size <= 0.0 || adj.upper() <= page_size + 0.5 {
            return;
        }
        let left = f64::from(bounds.x());
        let right = left + f64::from(bounds.width());
        let cur = adj.value();
        if left < cur {
            adj.set_value(left.max(adj.lower()));
        } else if right > cur + page_size {
            adj.set_value((right - page_size).min(adj.upper() - page_size));
        }
    }

    fn select_tab_impl(inner: &Rc<ToolDockInner>, tab_id: DockTabId) {
        let info = inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| (e.region, e.page.clone()));
        let Some((region, page)) = info else {
            return;
        };
        if let Some(rw) = inner.regions.get(&region) {
            rw.tab_view.set_selected_page(&page);
        }
    }

    fn show_context_menu_for_tab(inner: &Rc<ToolDockInner>, tab_id: DockTabId) -> bool {
        if let Some(prev) = inner.active_context_menu.borrow_mut().take() {
            prev.popover.popdown();
            if prev.popover.parent().is_some() {
                prev.popover.unparent();
            }
        }
        let info = inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| (e.region, e.chip.widget().clone()));
        let Some((region, chip_widget)) = info else {
            return false;
        };
        let Some(rw) = inner.regions.get(&region) else {
            return false;
        };

        let menu = gio::Menu::new();
        Self::fill_context_menu_for_tab(&menu, region, tab_id);
        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(true);
        // 挂载到不随标签迁移销毁的区域 root 容器，防止 move_tab 移除旧芯片时触发子控件未解绑警告。
        popover.set_parent(&rw.root);
        if let Some(rect) = chip_widget.compute_bounds(&rw.root) {
            let gdk_rect = gtk4::gdk::Rectangle::new(
                rect.x().round() as i32,
                rect.y().round() as i32,
                rect.width().round().max(1.0) as i32,
                rect.height().round().max(1.0) as i32,
            );
            popover.set_pointing_to(Some(&gdk_rect));
        }

        // 关键防坑：Popover 关闭时绝不在动作派发前清空 menu，仅在下一帧空闲安全 unparent 并释放。
        let dock_weak = Rc::downgrade(inner);
        let pop_weak = popover.downgrade();
        popover.connect_closed(move |_| {
            let dock_weak = dock_weak.clone();
            let pop_weak = pop_weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(pop) = pop_weak.upgrade() {
                    if pop.parent().is_some() {
                        pop.unparent();
                    }
                    if let Some(this) = dock_weak.upgrade() {
                        let mut slot = this.active_context_menu.borrow_mut();
                        if slot.as_ref().is_some_and(|ctx| ctx.popover == pop) {
                            *slot = None;
                        }
                    }
                }
            });
        });

        *inner.active_context_menu.borrow_mut() = Some(ActiveContextMenu {
            #[cfg(feature = "automation")]
            tab_id,
            popover: popover.clone(),
            _menu: menu,
        });
        if rw.root.native().and_then(|n| n.surface()).is_some() {
            popover.popup();
        }
        true
    }

    fn fill_context_menu_for_tab(menu: &gio::Menu, current_region: PanelRegion, tab_id: DockTabId) {
        menu.remove_all();
        let move_sec = gio::Menu::new();
        for target in PanelRegion::ALL {
            if target == current_region {
                continue;
            }
            let item = gio::MenuItem::new(Some(&fmt1(Key::DockMoveTabTo, target.title())), None);
            item.set_action_and_target_value(
                Some("win.dock-move-tab"),
                Some(&format!("{}:{}", target.id(), tab_id.encode()).to_variant()),
            );
            move_sec.append_item(&item);
        }
        menu.append_section(None, &move_sec);

        let close_sec = gio::Menu::new();
        let close_item = gio::MenuItem::new(Some(tr(Key::DockCloseTab)), None);
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
            let shell_item = gio::MenuItem::new(Some(tr(Key::DockNewTerminal)), None);
            shell_item.set_action_and_target_value(
                Some("win.dock-new-shell"),
                Some(&region.id().to_variant()),
            );
            term_sec.append_item(&shell_item);

            let agent_item = gio::MenuItem::new(Some(tr(Key::MenuLaunchAgent)), None);
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
                    Some(cur) if *cur == region => fmt1(Key::DockToolHere, tool.title()),
                    Some(_) => fmt1(Key::DockToolMoveHere, tool.title()),
                };
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.dock-add-tool"),
                    Some(&format!("{}:{}", region.id(), tool.id()).to_variant()),
                );
                tools_sec.append_item(&item);
            }
            rw.add_menu
                .append_section(Some(tr(Key::DockToolsSection)), &tools_sec);
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
        for region in PanelRegion::ALL {
            Self::sync_strip_selection(&self.inner, region);
        }
        self.inner.state.borrow_mut().preferred_terminal_region = PanelRegion::Bottom;
        self.inner.state.borrow_mut().suppress_callbacks = false;
        Self::rebuild_add_menus(&self.inner);
    }

    /// 恢复已保存的 `LayoutState`（仅静态工具与 `"terminal"` 区域偏好，绝不恢复动态终端 PTY）。
    pub fn apply_layout_state(&self, layout: &LayoutState) -> (f64, f64, f64) {
        self.inner.state.borrow_mut().suppress_callbacks = true;
        // 先关闭全部静态工具页（保留 static_widgets），再按布局文件重新挂载。
        let static_pages: Vec<(PanelRegion, adw::TabPage, TabStripChip)> = {
            let mut st = self.inner.state.borrow_mut();
            let mut out = Vec::new();
            for kind in StaticToolKind::ALL {
                if let Some(entry) = st.entries.remove(&DockTabId::Static(kind)) {
                    out.push((entry.region, entry.page, entry.chip));
                }
            }
            out
        };
        for (region, page, chip) in static_pages {
            if let Some(rw) = self.inner.regions.get(&region) {
                rw.tabs_box.remove(chip.widget());
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
            Self::sync_strip_selection(&self.inner, region);
            Self::sync_region_empty_stack(&self.inner, region);
        }
        self.inner.state.borrow_mut().preferred_terminal_region = preferred_term;
        self.inner.state.borrow_mut().suppress_callbacks = false;
        Self::rebuild_add_menus(&self.inner);

        let left_size = layout
            .regions
            .get("left")
            .map(|r| r.size.clamp(180.0, 480.0))
            .unwrap_or(280.0);
        let right_size = layout
            .regions
            .get("right")
            .map(|r| r.size.clamp(180.0, 480.0))
            .unwrap_or(280.0);
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
        let chip =
            Self::build_tab_chip(inner, tab_id, target_region, kind.title(), kind.icon_name());
        rw.tabs_box.append(chip.widget());
        inner.state.borrow_mut().entries.insert(
            tab_id,
            DockTabEntry {
                region: target_region,
                page: page.clone(),
                title: kind.title().to_string(),
                chip,
            },
        );
        Self::sync_region_empty_stack(inner, target_region);
        if activate {
            rw.tab_view.set_selected_page(&page);
        }
        Self::sync_strip_selection(inner, target_region);
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
            .map(|e| (e.region, e.page.clone(), e.chip.clone()));
        let Some((src_region, page, chip)) = info else {
            return;
        };
        let Some(target_rw) = inner.regions.get(&target_region) else {
            return;
        };
        if src_region == target_region {
            target_rw.tab_view.set_selected_page(&page);
            Self::sync_strip_selection(inner, target_region);
            if reveal && let Some(ref cbs) = inner.state.borrow().callbacks {
                (cbs.on_region_reveal)(target_region);
            }
            return;
        }
        let Some(src_rw) = inner.regions.get(&src_region) else {
            return;
        };

        inner.state.borrow_mut().suppress_callbacks = true;
        src_rw.tabs_box.remove(chip.widget());
        let pos = target_rw.tab_view.n_pages();
        src_rw
            .tab_view
            .transfer_page(&page, &target_rw.tab_view, pos);
        chip.configure_for_region(target_region);
        target_rw.tabs_box.append(chip.widget());
        if let Some(entry) = inner.state.borrow_mut().entries.get_mut(&tab_id) {
            entry.region = target_region;
        }
        if matches!(tab_id, DockTabId::Terminal(_)) {
            inner.state.borrow_mut().preferred_terminal_region = target_region;
        }
        target_rw.tab_view.set_selected_page(&page);
        Self::sync_strip_selection(inner, src_region);
        Self::sync_strip_selection(inner, target_region);
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
        Self::close_tab_impl(&self.inner, tab_id);
    }

    fn close_tab_impl(inner: &Rc<ToolDockInner>, tab_id: DockTabId) {
        let info = inner
            .state
            .borrow()
            .entries
            .get(&tab_id)
            .map(|e| (e.region, e.page.clone()));
        let Some((region, page)) = info else {
            return;
        };
        if let Some(rw) = inner.regions.get(&region) {
            rw.tab_view.close_page(&page);
        }
    }

    fn handle_close_page(
        inner: &Rc<ToolDockInner>,
        region: PanelRegion,
        view: &adw::TabView,
        page: &adw::TabPage,
    ) {
        let closed = {
            let mut st = inner.state.borrow_mut();
            let id = st
                .entries
                .iter()
                .find(|(_, entry)| &entry.page == page)
                .map(|(k, _)| *k);
            id.and_then(|k| st.entries.remove(&k).map(|e| (k, e.chip)))
        };
        let tab_id = closed.as_ref().map(|(k, _)| *k);
        if let Some((_, chip)) = closed
            && let Some(rw) = inner.regions.get(&region)
        {
            rw.tabs_box.remove(chip.widget());
        }
        view.close_page_finish(page, true);
        Self::sync_strip_selection(inner, region);
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
        let tab_id = DockTabId::Terminal(term_id);
        let page = rw.tab_view.append(widget);
        page.set_title(title);
        page.set_icon(Some(&gio::ThemedIcon::new(icon_name)));
        page.set_tooltip(title);
        let chip = Self::build_tab_chip(&self.inner, tab_id, region, title, icon_name);
        rw.tabs_box.append(chip.widget());
        {
            let mut st = self.inner.state.borrow_mut();
            st.entries.insert(
                tab_id,
                DockTabEntry {
                    region,
                    page: page.clone(),
                    title: title.to_string(),
                    chip,
                },
            );
            if st.preferred_terminal_region != region {
                st.preferred_terminal_region = region;
            }
        }
        Self::sync_region_empty_stack(&self.inner, region);
        rw.tab_view.set_selected_page(&page);
        Self::sync_strip_selection(&self.inner, region);
        let cbs = self.inner.state.borrow().callbacks.clone();
        if let Some(ref cbs) = cbs {
            (cbs.on_region_reveal)(region);
            (cbs.on_layout_changed)();
        }
    }

    pub fn update_terminal_tab(&self, term_id: u64, title: &str, icon_name: &str) {
        let region = {
            let mut st = self.inner.state.borrow_mut();
            if let Some(entry) = st.entries.get_mut(&DockTabId::Terminal(term_id)) {
                entry.title = title.to_string();
                entry.page.set_title(title);
                entry.page.set_icon(Some(&gio::ThemedIcon::new(icon_name)));
                entry.page.set_tooltip(title);
                entry.chip.update(title, icon_name);
                Some(entry.region)
            } else {
                None
            }
        };
        if let Some(region) = region {
            Self::reconcile_strip_labels(&self.inner, region, None);
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
            Self::sync_strip_selection(&self.inner, region);
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

    #[cfg(any(test, feature = "automation"))]
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
        if let Some(ctx) = self.inner.active_context_menu.borrow_mut().take() {
            ctx.popover.popdown();
            if ctx.popover.parent().is_some() {
                ctx.popover.unparent();
            }
        }
        {
            let mut st = self.inner.state.borrow_mut();
            st.disposing = true;
            st.callbacks = None;
            for (_, entry) in st.entries.drain() {
                if let Some(rw) = self.inner.regions.get(&entry.region) {
                    rw.tabs_box.remove(entry.chip.widget());
                }
            }
            st.static_widgets.clear();
        }
        for rw in self.inner.regions.values() {
            rw.add_menu.remove_all();
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
        self.inner.regions.get(&region).is_some_and(|rw| {
            rw.add_btn.property::<bool>("visible")
                && rw.strip_bar.property::<bool>("visible")
                && rw.add_btn.parent().is_some()
        })
    }

    #[cfg(feature = "automation")]
    pub fn region_visible_tab_bars(&self, region: PanelRegion) -> usize {
        let Some(rw) = self.inner.regions.get(&region) else {
            return 0;
        };
        let adw_bars = count_adw_tab_bars_in_widget(rw.root.upcast_ref::<gtk4::Widget>());
        if adw_bars > 0 {
            return adw_bars + 100;
        }
        count_strips_in_widget(rw.root.upcast_ref::<gtk4::Widget>())
    }

    #[cfg(feature = "automation")]
    pub fn add_menu_labels(&self, region: PanelRegion) -> Vec<String> {
        Self::rebuild_add_menus(&self.inner);
        let Some(rw) = self.inner.regions.get(&region) else {
            return Vec::new();
        };
        collect_menu_labels(rw.add_menu.upcast_ref())
    }

    #[cfg(test)]
    pub fn context_menu_labels_for(&self, tab_id: DockTabId) -> Vec<String> {
        let Some(region) = self.tab_region(tab_id) else {
            return Vec::new();
        };
        let menu = gio::Menu::new();
        Self::fill_context_menu_for_tab(&menu, region, tab_id);
        collect_menu_labels(menu.upcast_ref())
    }

    #[cfg(feature = "automation")]
    pub fn open_tab_context_menu(&self, tab_id: DockTabId) -> bool {
        Self::show_context_menu_for_tab(&self.inner, tab_id)
    }

    #[cfg(feature = "automation")]
    pub fn active_context_menu_popover(&self) -> Option<(DockTabId, gtk4::PopoverMenu)> {
        self.inner
            .active_context_menu
            .borrow()
            .as_ref()
            .map(|ctx| (ctx.tab_id, ctx.popover.clone()))
    }

    #[cfg(feature = "automation")]
    pub fn activate_context_menu_item(&self, label_text: &str) -> bool {
        let Some(popover) = self
            .inner
            .active_context_menu
            .borrow()
            .as_ref()
            .map(|ctx| ctx.popover.clone())
        else {
            return false;
        };
        if !popover.is_visible() {
            return false;
        }
        let Some(model) = popover.menu_model() else {
            return false;
        };
        let Some((action_name, target_opt)) = find_menu_action(&model, label_text) else {
            return false;
        };
        if !action_name.starts_with("win.dock-") || target_opt.is_none() {
            return false;
        }
        if let Some(btn) = find_popover_item_widget(popover.upcast_ref(), label_text) {
            return btn.activate();
        }
        false
    }

    #[cfg(feature = "automation")]
    pub fn active_context_menu_labels(&self) -> Vec<String> {
        let Some(popover) = self
            .inner
            .active_context_menu
            .borrow()
            .as_ref()
            .map(|ctx| ctx.popover.clone())
        else {
            return Vec::new();
        };
        let Some(model) = popover.menu_model() else {
            return Vec::new();
        };
        collect_menu_labels(&model)
    }

    #[cfg(feature = "automation")]
    pub fn measure_strip_tab_bounds(&self, region: PanelRegion) -> Option<StripLayoutMetrics> {
        let rw = self.inner.regions.get(&region)?;
        let strip_width = rw.strip_scroll.width() as f32;
        let st = self.inner.state.borrow();
        let n_pages = rw.tab_view.n_pages();
        let mut tabs = Vec::with_capacity(n_pages as usize);
        for idx in 0..n_pages {
            let page = rw.tab_view.nth_page(idx);
            let Some((&tab_id, entry)) = st.entries.iter().find(|(_, e)| e.page == page) else {
                continue;
            };
            let chip_w = entry.chip.widget();
            if let Some(bounds) = chip_w.compute_bounds(&rw.strip_scroll) {
                let x = bounds.x();
                let y = bounds.y();
                let width = bounds.width();
                let height = bounds.height();
                let fits_in_strip = x >= -0.5 && x + width <= strip_width + 0.5;
                tabs.push(TabChipBounds {
                    id: tab_id.encode(),
                    title: entry.title.clone(),
                    x,
                    y,
                    width,
                    height,
                    fits_in_strip,
                    selected: entry.chip.inner.selected.get(),
                    has_close_button: widget_has_button_descendant(chip_w.upcast_ref()),
                    has_motion_controller: widget_has_motion_controller(chip_w.upcast_ref()),
                    label_visible: entry.chip.inner.label.is_visible(),
                    ellipsized: entry.chip.inner.label.ellipsize() != pango::EllipsizeMode::None,
                });
            }
        }
        Some(StripLayoutMetrics {
            region: region.id().to_string(),
            strip_width,
            tabs,
        })
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

#[cfg(any(test, feature = "automation"))]
fn widget_has_button_descendant(root: &gtk4::Widget) -> bool {
    if root.is::<gtk4::Button>() {
        return true;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if widget_has_button_descendant(&c) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

#[cfg(any(test, feature = "automation"))]
fn widget_has_motion_controller(widget: &gtk4::Widget) -> bool {
    let model = widget.observe_controllers();
    for idx in 0..model.n_items() {
        if let Some(obj) = model.item(idx)
            && obj.is::<gtk4::EventControllerMotion>()
        {
            return true;
        }
    }
    false
}

#[cfg(feature = "automation")]
fn count_adw_tab_bars_in_widget(root: &gtk4::Widget) -> usize {
    let mut count = 0usize;
    if root.is::<adw::TabBar>() {
        count += 1;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        count += count_adw_tab_bars_in_widget(&c);
        child = c.next_sibling();
    }
    count
}

#[cfg(feature = "automation")]
fn count_strips_in_widget(root: &gtk4::Widget) -> usize {
    let mut count = 0usize;
    if root.has_css_class("hortus-tool-strip") {
        count += 1;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        count += count_strips_in_widget(&c);
        child = c.next_sibling();
    }
    count
}

#[cfg(feature = "automation")]
fn find_menu_action(
    menu: &gio::MenuModel,
    target_label: &str,
) -> Option<(String, Option<glib::Variant>)> {
    for idx in 0..menu.n_items() {
        if let Some(val) = menu.item_attribute_value(idx, "label", Some(glib::VariantTy::STRING))
            && val.str() == Some(target_label)
        {
            let action = menu
                .item_attribute_value(idx, "action", Some(glib::VariantTy::STRING))
                .and_then(|v| v.str().map(str::to_string))?;
            let target = menu.item_attribute_value(idx, "target", None);
            return Some((action, target));
        }
        if let Some(sub) = menu.item_link(idx, gio::MENU_LINK_SECTION)
            && let Some(found) = find_menu_action(&sub, target_label)
        {
            return Some(found);
        }
        if let Some(sub) = menu.item_link(idx, gio::MENU_LINK_SUBMENU)
            && let Some(found) = find_menu_action(&sub, target_label)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(feature = "automation")]
fn find_popover_item_widget(root: &gtk4::Widget, target_label: &str) -> Option<gtk4::Widget> {
    if let Some(lbl) = root.downcast_ref::<gtk4::Label>()
        && lbl.text() == target_label
    {
        let mut cur = lbl.parent();
        while let Some(w) = cur {
            if w.type_().name() == "GtkModelButton" || w.is::<gtk4::Button>() {
                return Some(w);
            }
            cur = w.parent();
        }
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_popover_item_widget(&c, target_label) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

#[cfg(any(test, feature = "automation"))]
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
        let _ = ToolDock::show_context_menu_for_tab(
            &dock.inner,
            DockTabId::Static(StaticToolKind::Changes),
        );
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
            "Terminal 99",
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

    #[test]
    fn tool_dock_strip_chips_and_context_menu_all_tools() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let dock = ToolDock::new(dummy_static_widgets());
        dock.apply_default_layout();

        // 所有工具标签芯片均无内联 × 关闭按钮与悬停布局控制器，保持居中对称且悬停零位移
        let st = dock.inner.state.borrow();
        let files_entry = &st.entries[&DockTabId::Static(StaticToolKind::Files)];
        let search_entry = &st.entries[&DockTabId::Static(StaticToolKind::Search)];
        assert!(files_entry.chip.inner.selected.get());
        assert!(files_entry.chip.widget().has_css_class("selected"));
        assert!(!search_entry.chip.inner.selected.get());
        assert!(!search_entry.chip.widget().has_css_class("selected"));
        for entry in st.entries.values() {
            assert!(!widget_has_button_descendant(
                entry.chip.widget().upcast_ref()
            ));
            assert!(!widget_has_motion_controller(
                entry.chip.widget().upcast_ref()
            ));
            assert_eq!(entry.chip.inner.center_box.halign(), gtk4::Align::Center);
        }
        assert!(
            files_entry
                .chip
                .widget()
                .has_css_class("hortus-tool-tab-sidebar")
        );
        drop(st);

        // 全部 6 个静态工具右键菜单均包含其余两区移动项与关闭项
        for kind in StaticToolKind::ALL {
            let id = DockTabId::Static(kind);
            let region = dock.static_tool_region(kind).expect("tool in layout");
            let labels = dock.context_menu_labels_for(id);
            assert!(labels.iter().any(|l| l == tr(Key::DockCloseTab)));
            for target in PanelRegion::ALL {
                let move_lbl = fmt1(Key::DockMoveTabTo, target.title());
                if target == region {
                    assert!(!labels.iter().any(|l| l == &move_lbl));
                } else {
                    assert!(labels.iter().any(|l| l == &move_lbl));
                }
            }
        }

        // D2 动态宽度分级：宽条带全文字，窄条带激活保留全文字、非激活折叠为仅图标（无省略号），极窄全图标
        ToolDock::reconcile_strip_labels(&dock.inner, PanelRegion::Left, Some(520));
        {
            let st = dock.inner.state.borrow();
            for tool in [
                StaticToolKind::Files,
                StaticToolKind::Search,
                StaticToolKind::Tags,
            ] {
                let entry = &st.entries[&DockTabId::Static(tool)];
                assert!(entry.chip.inner.label.is_visible());
                assert_eq!(
                    entry.chip.inner.label.ellipsize(),
                    pango::EllipsizeMode::None
                );
            }
        }
        ToolDock::reconcile_strip_labels(&dock.inner, PanelRegion::Left, Some(180));
        {
            let st = dock.inner.state.borrow();
            assert!(
                st.entries[&DockTabId::Static(StaticToolKind::Files)]
                    .chip
                    .inner
                    .label
                    .is_visible()
            );
            assert!(
                !st.entries[&DockTabId::Static(StaticToolKind::Search)]
                    .chip
                    .inner
                    .label
                    .is_visible()
            );
            assert!(
                !st.entries[&DockTabId::Static(StaticToolKind::Tags)]
                    .chip
                    .inner
                    .label
                    .is_visible()
            );
        }
        ToolDock::reconcile_strip_labels(&dock.inner, PanelRegion::Left, Some(90));
        {
            let st = dock.inner.state.borrow();
            for tool in [
                StaticToolKind::Files,
                StaticToolKind::Search,
                StaticToolKind::Tags,
            ] {
                assert!(
                    !st.entries[&DockTabId::Static(tool)]
                        .chip
                        .inner
                        .label
                        .is_visible()
                );
            }
        }

        // 移动到 Bottom 后芯片切换为底部紧凑模式（hexpand=false），移回 Right 恢复自适应（hexpand=true）
        dock.move_tab(
            DockTabId::Static(StaticToolKind::Changes),
            PanelRegion::Bottom,
        );
        {
            let st = dock.inner.state.borrow();
            let chg = &st.entries[&DockTabId::Static(StaticToolKind::Changes)];
            assert!(!chg.chip.widget().hexpands());
            assert!(chg.chip.widget().has_css_class("hortus-tool-tab-bottom"));
        }
        dock.move_tab(
            DockTabId::Static(StaticToolKind::Changes),
            PanelRegion::Right,
        );
        {
            let st = dock.inner.state.borrow();
            let chg = &st.entries[&DockTabId::Static(StaticToolKind::Changes)];
            assert!(chg.chip.widget().hexpands());
            assert!(chg.chip.widget().has_css_class("hortus-tool-tab-sidebar"));
        }
        dock.teardown();
    }
}
