// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! VTE4 原生终端管理器：直接托管于 `ToolDock` 单层工具标签栏（无内嵌双层 TabBar），
//! 支持普通 Shell 与独立 Agent 实例、最近聚焦终端追踪、主题/字体自适应、
//! 子进程退出非自动重启状态栏及进程组安全清理。

use crate::core::PanelRegion;
use crate::terminal::process::{
    build_terminal_env, is_pid_alive, signal_process_group, terminate_pids_sync, user_shell_program,
};
use crate::terminal::sanitize::{sanitize_selection_for_terminal, shell_quote_relative_path};
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use vte4::prelude::*;

static NEXT_TERM_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    Shell,
    Agent { session_id: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalPhase {
    Preparing,
    Running,
    Exited(i32),
    Failed(String),
}

#[derive(Clone)]
pub struct TerminalHostCallbacks {
    pub on_tab_created: Rc<dyn Fn(u64, PanelRegion, gtk4::Widget, String, &'static str)>,
    pub on_tab_updated: Rc<dyn Fn(u64, String, &'static str)>,
    pub on_request_select_tab: Rc<dyn Fn(u64)>,
    pub on_request_close_tab: Rc<dyn Fn(u64)>,
    pub on_agent_ended: Rc<dyn Fn(String, &'static str, Option<i32>)>,
}

struct TerminalTab {
    id: u64,
    kind: TerminalKind,
    phase: TerminalPhase,
    region: PanelRegion,
    title: String,
    stack: gtk4::Stack,
    vte: vte4::Terminal,
    status_bar: gtk4::Box,
    status_label: gtk4::Label,
    restart_btn: gtk4::Button,
    preparing_label: gtk4::Label,
    working_dir: String,
    child_pid: Option<i32>,
    argv: Vec<String>,
    envv: Vec<String>,
    cancel_flag: Arc<AtomicBool>,
    pending_feeds: Vec<PendingFeed>,
}

#[derive(Clone)]
enum PendingFeed {
    Raw(String),
    Paste(String),
}

#[derive(Default)]
struct HostState {
    vault_root: Option<String>,
    terminal_font: Option<String>,
    tabs: HashMap<u64, TerminalTab>,
    last_active_id: Option<u64>,
    callbacks: Option<TerminalHostCallbacks>,
    disposing: bool,
}

struct TerminalHostInner {
    state: RefCell<HostState>,
}

#[derive(Clone)]
pub struct TerminalHost {
    inner: Rc<TerminalHostInner>,
}

impl Default for TerminalHost {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalHost {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(TerminalHostInner {
                state: RefCell::new(HostState::default()),
            }),
        }
    }

    pub fn set_callbacks(&self, callbacks: TerminalHostCallbacks) {
        self.inner.state.borrow_mut().callbacks = Some(callbacks);
    }

    pub fn set_vault_root(&self, vault_root: Option<String>) {
        let prev = self.inner.state.borrow().vault_root.clone();
        if prev == vault_root {
            return;
        }
        self.close_all_tabs();
        self.inner.state.borrow_mut().vault_root = vault_root;
    }

    pub fn apply_font(&self, terminal_font: Option<String>) {
        self.inner.state.borrow_mut().terminal_font = terminal_font.clone();
        let desc = font_description_for(terminal_font.as_deref());
        let vtes: Vec<vte4::Terminal> = self
            .inner
            .state
            .borrow()
            .tabs
            .values()
            .map(|t| t.vte.clone())
            .collect();
        for vte in vtes {
            vte.set_font(Some(&desc));
        }
    }

    pub fn apply_theme(&self) {
        let dark = adw::StyleManager::default().is_dark();
        let vtes: Vec<vte4::Terminal> = self
            .inner
            .state
            .borrow()
            .tabs
            .values()
            .map(|t| t.vte.clone())
            .collect();
        for vte in vtes {
            apply_vte_colors(&vte, dark);
        }
    }

    pub fn open_shell_in_region(&self, region: PanelRegion) -> u64 {
        Self::open_shell_tab_impl(&self.inner, region)
    }

    pub fn ensure_terminal_in_region(&self, region: PanelRegion) -> (u64, PanelRegion) {
        if let Some(id) = self.last_active_tab_id()
            && let Some(cur_region) = self.tab_region(id)
        {
            return (id, cur_region);
        }
        let id = Self::open_shell_tab_impl(&self.inner, region);
        (id, region)
    }

    pub fn create_preparing_agent_tab_in_region(
        &self,
        region: PanelRegion,
    ) -> (u64, Arc<AtomicBool>) {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let working_dir = self.default_working_dir();
        let id = Self::insert_tab(
            &self.inner,
            region,
            TerminalKind::Agent { session_id: None },
            TerminalPhase::Preparing,
            working_dir,
            Vec::new(),
            Vec::new(),
            Arc::clone(&cancel_flag),
        );
        (id, cancel_flag)
    }

    pub fn start_agent_in_tab(
        &self,
        tab_id: u64,
        session_id: String,
        title: Option<String>,
        working_dir: String,
        argv: Vec<String>,
        envv: Vec<String>,
    ) -> Result<(), String> {
        let display_title = title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Agent")
            .to_string();
        let (vte, stack, pending_cancel, callbacks) = {
            let mut st = self.inner.state.borrow_mut();
            let cbs = st.callbacks.clone();
            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                return Err(crate::i18n::tr(crate::i18n::Key::TermAgentTabClosed).into());
            };
            if tab.cancel_flag.load(Ordering::Acquire) {
                return Err(crate::i18n::tr(crate::i18n::Key::TermAgentCancelled).into());
            }
            tab.kind = TerminalKind::Agent {
                session_id: Some(session_id.clone()),
            };
            tab.phase = TerminalPhase::Running;
            tab.title = display_title.clone();
            tab.working_dir = working_dir.clone();
            tab.argv = argv.clone();
            tab.envv = envv.clone();
            let out = (
                tab.vte.clone(),
                tab.stack.clone(),
                tab.cancel_flag.clone(),
                cbs,
            );
            st.last_active_id = Some(tab_id);
            out
        };
        if pending_cancel.load(Ordering::Acquire) {
            return Err(crate::i18n::tr(crate::i18n::Key::TermAgentCancelled).into());
        }
        if let Some(ref cbs) = callbacks {
            (cbs.on_tab_updated)(tab_id, display_title, "system-run-symbolic");
            (cbs.on_request_select_tab)(tab_id);
        }
        stack.set_visible_child_name("terminal");
        Self::spawn_in_vte(&self.inner, tab_id, &vte, &working_dir, &argv, &envv);
        vte.grab_focus();
        Ok(())
    }

    pub fn fail_preparing_agent_tab(&self, tab_id: u64, message: &str) {
        let mut st = self.inner.state.borrow_mut();
        if let Some(tab) = st.tabs.get_mut(&tab_id) {
            tab.phase = TerminalPhase::Failed(message.to_string());
            tab.preparing_label.set_text(&crate::i18n::fmt1(
                crate::i18n::Key::TermStartFailed,
                message,
            ));
            tab.status_label.set_text(&crate::i18n::fmt1(
                crate::i18n::Key::TermNotStarted,
                message,
            ));
            tab.status_bar.set_visible(true);
            tab.restart_btn.set_visible(false);
        }
    }

    pub fn set_tab_region(&self, tab_id: u64, region: PanelRegion) {
        let mut st = self.inner.state.borrow_mut();
        if let Some(tab) = st.tabs.get_mut(&tab_id) {
            tab.region = region;
        }
    }

    pub fn tab_region(&self, tab_id: u64) -> Option<PanelRegion> {
        self.inner
            .state
            .borrow()
            .tabs
            .get(&tab_id)
            .map(|t| t.region)
    }

    pub fn mark_active_tab(&self, tab_id: u64) {
        let mut st = self.inner.state.borrow_mut();
        if st.tabs.contains_key(&tab_id) {
            st.last_active_id = Some(tab_id);
        }
    }

    pub fn last_active_tab_id(&self) -> Option<u64> {
        let st = self.inner.state.borrow();
        if let Some(id) = st.last_active_id
            && st.tabs.contains_key(&id)
        {
            return Some(id);
        }
        st.tabs
            .values()
            .find(|t| t.phase == TerminalPhase::Running)
            .or_else(|| st.tabs.values().next())
            .map(|t| t.id)
    }

    pub fn send_note_path(
        &self,
        relpath: &str,
        fallback_region: PanelRegion,
    ) -> (u64, PanelRegion, String) {
        let quoted = shell_quote_relative_path(relpath);
        let (id, region) = self.ensure_terminal_in_region(fallback_region);
        self.deliver_to_tab(id, PendingFeed::Raw(quoted.clone()));
        (id, region, quoted)
    }

    pub fn send_selection(
        &self,
        text: &str,
        fallback_region: PanelRegion,
    ) -> Option<(u64, PanelRegion, String)> {
        let cleaned = sanitize_selection_for_terminal(text);
        if cleaned.is_empty() {
            return None;
        }
        let (id, region) = self.ensure_terminal_in_region(fallback_region);
        self.deliver_to_tab(id, PendingFeed::Paste(cleaned.clone()));
        Some((id, region, cleaned))
    }

    fn deliver_to_tab(&self, tab_id: u64, payload: PendingFeed) {
        let (vte, ready, callbacks) = {
            let mut st = self.inner.state.borrow_mut();
            let cbs = st.callbacks.clone();
            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                return;
            };
            let ready = tab.child_pid.is_some() && tab.phase == TerminalPhase::Running;
            if !ready {
                tab.pending_feeds.push(payload.clone());
            }
            let out = (tab.vte.clone(), ready, cbs);
            st.last_active_id = Some(tab_id);
            out
        };
        if let Some(ref cbs) = callbacks {
            (cbs.on_request_select_tab)(tab_id);
        }
        vte.grab_focus();
        if ready {
            Self::write_payload(&vte, &payload);
        }
    }

    fn write_payload(vte: &vte4::Terminal, payload: &PendingFeed) {
        match payload {
            PendingFeed::Raw(s) => vte.feed_child(s.as_bytes()),
            PendingFeed::Paste(s) => vte.paste_text(s),
        }
    }

    pub fn all_child_pids(&self) -> Vec<i32> {
        self.inner
            .state
            .borrow()
            .tabs
            .values()
            .filter_map(|t| t.child_pid)
            .collect()
    }

    #[cfg(feature = "automation")]
    pub fn tab_count(&self) -> usize {
        self.inner.state.borrow().tabs.len()
    }

    pub fn close_tab_from_dock(&self, tab_id: u64) {
        let (removed, callbacks) = {
            let mut st = self.inner.state.borrow_mut();
            if st.last_active_id == Some(tab_id) {
                st.last_active_id = None;
            }
            (st.tabs.remove(&tab_id), st.callbacks.clone())
        };
        if let Some(tab) = removed {
            tab.cancel_flag.store(true, Ordering::Release);
            if let Some(pid) = tab.child_pid {
                terminate_async_with_kill_fallback(pid);
            }
            if matches!(tab.phase, TerminalPhase::Preparing | TerminalPhase::Running)
                && let TerminalKind::Agent {
                    session_id: Some(sid),
                } = tab.kind
                && let Some(ref cbs) = callbacks
            {
                (cbs.on_agent_ended)(sid, "interrupted", None);
            }
        }
    }

    #[cfg(feature = "automation")]
    pub fn close_tab_by_id(&self, tab_id: u64) {
        let callbacks = self.inner.state.borrow().callbacks.clone();
        if let Some(ref cbs) = callbacks {
            (cbs.on_request_close_tab)(tab_id);
        } else {
            self.close_tab_from_dock(tab_id);
        }
    }

    pub fn close_all_tabs(&self) {
        let pids = self.all_child_pids();
        let (ids, interrupted_sids, callbacks, disposing) = {
            let mut st = self.inner.state.borrow_mut();
            let mut collected = Vec::new();
            let mut sids = Vec::new();
            for tab in st.tabs.values_mut() {
                tab.cancel_flag.store(true, Ordering::Release);
                collected.push(tab.id);
                if matches!(tab.phase, TerminalPhase::Preparing | TerminalPhase::Running)
                    && let TerminalKind::Agent {
                        session_id: Some(ref sid),
                    } = tab.kind
                {
                    sids.push(sid.clone());
                }
            }
            st.tabs.clear();
            st.last_active_id = None;
            (collected, sids, st.callbacks.clone(), st.disposing)
        };
        if let Some(ref cbs) = callbacks {
            for sid in interrupted_sids {
                (cbs.on_agent_ended)(sid, "interrupted", None);
            }
            if !disposing {
                for id in ids {
                    (cbs.on_request_close_tab)(id);
                }
            }
        }
        terminate_pids_sync(&pids);
    }

    pub fn teardown(&self) {
        self.inner.state.borrow_mut().disposing = true;
        self.close_all_tabs();
        self.inner.state.borrow_mut().callbacks = None;
    }

    fn default_working_dir(&self) -> String {
        self.inner
            .state
            .borrow()
            .vault_root
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
    }

    fn open_shell_tab_impl(inner: &Rc<TerminalHostInner>, region: PanelRegion) -> u64 {
        let shell = user_shell_program();
        Self::open_shell_tab_with_argv(inner, region, vec![shell])
    }

    fn open_shell_tab_with_argv(
        inner: &Rc<TerminalHostInner>,
        region: PanelRegion,
        argv: Vec<String>,
    ) -> u64 {
        let working_dir = inner
            .state
            .borrow()
            .vault_root
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()));
        let envv = build_terminal_env(inner.state.borrow().vault_root.as_deref(), None, None, None);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let id = Self::insert_tab(
            inner,
            region,
            TerminalKind::Shell,
            TerminalPhase::Running,
            working_dir.clone(),
            argv.clone(),
            envv.clone(),
            cancel_flag,
        );
        let vte = inner.state.borrow().tabs.get(&id).map(|t| t.vte.clone());
        if let Some(vte) = vte {
            Self::spawn_in_vte(inner, id, &vte, &working_dir, &argv, &envv);
            vte.grab_focus();
        }
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_tab(
        inner: &Rc<TerminalHostInner>,
        region: PanelRegion,
        kind: TerminalKind,
        phase: TerminalPhase,
        working_dir: String,
        argv: Vec<String>,
        envv: Vec<String>,
        cancel_flag: Arc<AtomicBool>,
    ) -> u64 {
        let id = NEXT_TERM_ID.fetch_add(1, Ordering::Relaxed);
        let vte = vte4::Terminal::new();
        vte.set_vexpand(true);
        vte.set_hexpand(true);
        vte.set_scrollback_lines(10_000);
        vte.set_scroll_on_keystroke(true);
        vte.set_allow_hyperlink(true);

        let dark = adw::StyleManager::default().is_dark();
        apply_vte_colors(&vte, dark);
        let font_opt = inner.state.borrow().terminal_font.clone();
        vte.set_font(Some(&font_description_for(font_opt.as_deref())));

        let focus_ctrl = gtk4::EventControllerFocus::new();
        {
            let weak = Rc::downgrade(inner);
            focus_ctrl.connect_enter(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.state.borrow_mut().last_active_id = Some(id);
                }
            });
        }
        vte.add_controller(focus_ctrl);

        let status_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        status_bar.add_css_class("hortus-terminal-status");
        let status_label = gtk4::Label::new(Some(""));
        status_label.set_xalign(0.0);
        status_label.set_hexpand(true);
        status_label.add_css_class("caption");
        let restart_btn =
            gtk4::Button::with_label(crate::i18n::tr(crate::i18n::Key::TermRestartBtn));
        restart_btn.add_css_class("flat");
        restart_btn.add_css_class("caption");
        let close_btn = gtk4::Button::with_label(crate::i18n::tr(crate::i18n::Key::BtnCloseTab));
        close_btn.add_css_class("flat");
        close_btn.add_css_class("caption");
        status_bar.append(&status_label);
        status_bar.append(&restart_btn);
        status_bar.append(&close_btn);
        status_bar.set_visible(false);

        let term_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        term_box.set_vexpand(true);
        term_box.set_hexpand(true);
        term_box.append(&status_bar);
        term_box.append(&vte);

        let prep_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        prep_box.set_valign(gtk4::Align::Center);
        prep_box.set_halign(gtk4::Align::Center);
        let spinner = gtk4::Spinner::new();
        spinner.set_spinning(true);
        let preparing_label =
            gtk4::Label::new(Some(crate::i18n::tr(crate::i18n::Key::TermPreparingMsg)));
        preparing_label.add_css_class("dim-label");
        let cancel_prep_btn = gtk4::Button::with_label(crate::i18n::tr(crate::i18n::Key::Cancel));
        prep_box.append(&spinner);
        prep_box.append(&preparing_label);
        prep_box.append(&cancel_prep_btn);

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_named(&term_box, Some("terminal"));
        stack.add_named(&prep_box, Some("preparing"));
        stack.set_visible_child_name(if phase == TerminalPhase::Preparing {
            "preparing"
        } else {
            "terminal"
        });

        let (title, icon_name) = match &kind {
            TerminalKind::Shell => (
                crate::i18n::fmt1(crate::i18n::Key::TermTabShellTitle, id),
                "utilities-terminal-symbolic",
            ),
            TerminalKind::Agent { .. } => (
                crate::i18n::tr(crate::i18n::Key::TermTabAgentPreparing).to_string(),
                "system-run-symbolic",
            ),
        };

        {
            let weak = Rc::downgrade(inner);
            vte.connect_child_exited(move |_, status| {
                if let Some(this) = weak.upgrade() {
                    Self::handle_child_exited(&this, id, status);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            restart_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::restart_tab(&this, id);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            close_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    let cbs = this.state.borrow().callbacks.clone();
                    if let Some(ref cbs) = cbs {
                        (cbs.on_request_close_tab)(id);
                    }
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            let flag = Arc::clone(&cancel_flag);
            cancel_prep_btn.connect_clicked(move |_| {
                flag.store(true, Ordering::Release);
                if let Some(this) = weak.upgrade() {
                    let cbs = this.state.borrow().callbacks.clone();
                    if let Some(ref cbs) = cbs {
                        (cbs.on_request_close_tab)(id);
                    }
                }
            });
        }

        let widget = stack.clone().upcast::<gtk4::Widget>();
        let callbacks = {
            let mut st = inner.state.borrow_mut();
            st.tabs.insert(
                id,
                TerminalTab {
                    id,
                    kind,
                    phase,
                    region,
                    title: title.clone(),
                    stack,
                    vte,
                    status_bar,
                    status_label,
                    restart_btn,
                    preparing_label,
                    working_dir,
                    child_pid: None,
                    argv,
                    envv,
                    cancel_flag,
                    pending_feeds: Vec::new(),
                },
            );
            st.last_active_id = Some(id);
            st.callbacks.clone()
        };
        if let Some(ref cbs) = callbacks {
            (cbs.on_tab_created)(id, region, widget, title, icon_name);
        }
        id
    }

    fn spawn_in_vte(
        inner: &Rc<TerminalHostInner>,
        tab_id: u64,
        vte: &vte4::Terminal,
        working_dir: &str,
        argv: &[String],
        envv: &[String],
    ) {
        if argv.is_empty() {
            return;
        }
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let env_refs: Vec<&str> = envv.iter().map(String::as_str).collect();
        let weak = Rc::downgrade(inner);
        let vte_weak = vte.downgrade();
        vte.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(working_dir),
            &argv_refs,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gio::Cancellable::NONE,
            move |res| {
                let Some(this) = weak.upgrade() else {
                    if let Ok(pid) = res {
                        terminate_async_with_kill_fallback(pid.0);
                    }
                    return;
                };
                match res {
                    Ok(pid) => {
                        let pid_i32 = pid.0;
                        let pending = {
                            let mut st = this.state.borrow_mut();
                            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                                drop(st);
                                terminate_async_with_kill_fallback(pid_i32);
                                return;
                            };
                            tab.child_pid = Some(pid_i32);
                            tab.phase = TerminalPhase::Running;
                            tab.status_bar.set_visible(false);
                            std::mem::take(&mut tab.pending_feeds)
                        };
                        if let Some(vte_now) = vte_weak.upgrade() {
                            for p in pending {
                                Self::write_payload(&vte_now, &p);
                            }
                        }
                    }
                    Err(err) => {
                        let msg = err.message().to_string();
                        let agent_cb = {
                            let mut st = this.state.borrow_mut();
                            let callbacks = st.callbacks.clone();
                            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                                return;
                            };
                            tab.phase = TerminalPhase::Failed(msg.clone());
                            tab.status_label.set_text(&crate::i18n::fmt1(
                                crate::i18n::Key::TermStartFailed,
                                &msg,
                            ));
                            tab.status_bar.set_visible(true);
                            tab.restart_btn
                                .set_visible(matches!(tab.kind, TerminalKind::Shell));
                            match &tab.kind {
                                TerminalKind::Agent {
                                    session_id: Some(sid),
                                } => Some((sid.clone(), callbacks.map(|c| c.on_agent_ended))),
                                _ => None,
                            }
                        };
                        if let Some((sid, Some(cb))) = agent_cb {
                            cb(sid, "failed", None);
                        }
                    }
                }
            },
        );
    }

    fn handle_child_exited(inner: &Rc<TerminalHostInner>, tab_id: u64, raw_status: i32) {
        let code = decode_wait_status(raw_status);
        let agent_cb = {
            let mut st = inner.state.borrow_mut();
            let callbacks = st.callbacks.clone();
            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                return;
            };
            tab.child_pid = None;
            tab.phase = TerminalPhase::Exited(code);
            // 退出后严禁自动重启，显示显式退出码与按钮。
            tab.status_label
                .set_text(&crate::i18n::fmt1(crate::i18n::Key::TermExitedCode, code));
            tab.status_bar.set_visible(true);
            tab.restart_btn
                .set_visible(matches!(tab.kind, TerminalKind::Shell));
            match &tab.kind {
                TerminalKind::Agent {
                    session_id: Some(sid),
                } => Some((sid.clone(), callbacks.map(|c| c.on_agent_ended))),
                _ => None,
            }
        };
        if let Some((sid, Some(cb))) = agent_cb {
            cb(sid, "exited", Some(code));
        }
    }

    fn restart_tab(inner: &Rc<TerminalHostInner>, tab_id: u64) {
        let info = {
            let mut st = inner.state.borrow_mut();
            let Some(tab) = st.tabs.get_mut(&tab_id) else {
                return;
            };
            if !matches!(tab.kind, TerminalKind::Shell) {
                return;
            }
            if tab.child_pid.is_some() {
                return;
            }
            tab.vte.reset(true, true);
            tab.status_bar.set_visible(false);
            tab.phase = TerminalPhase::Running;
            (
                tab.vte.clone(),
                tab.working_dir.clone(),
                tab.argv.clone(),
                tab.envv.clone(),
            )
        };
        Self::spawn_in_vte(inner, tab_id, &info.0, &info.1, &info.2, &info.3);
        info.0.grab_focus();
    }
}

#[cfg(feature = "automation")]
impl TerminalHost {
    pub fn tab_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.inner.state.borrow().tabs.keys().copied().collect();
        ids.sort();
        ids
    }

    pub fn tab_snapshot(&self, id: u64) -> Option<serde_json::Value> {
        let st = self.inner.state.borrow();
        let tab = st.tabs.get(&id)?;
        let kind_str = match &tab.kind {
            TerminalKind::Shell => "shell",
            TerminalKind::Agent { .. } => "agent",
        };
        let session_id = match &tab.kind {
            TerminalKind::Agent { session_id } => session_id.clone(),
            _ => None,
        };
        let (phase_str, exit_code) = match &tab.phase {
            TerminalPhase::Preparing => ("preparing", None),
            TerminalPhase::Running => ("running", None),
            TerminalPhase::Exited(c) => ("exited", Some(*c)),
            TerminalPhase::Failed(_) => ("failed", None),
        };
        let screen_text = tab
            .vte
            .text_format(vte4::Format::Text)
            .map(|s| s.to_string())
            .unwrap_or_default();
        let font_family = tab
            .vte
            .font()
            .and_then(|f| f.family())
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(serde_json::json!({
            "id": tab.id,
            "kind": kind_str,
            "region": tab.region.id(),
            "session_id": session_id,
            "phase": phase_str,
            "exit_code": exit_code,
            "status_visible": tab.status_bar.is_visible(),
            "status_text": tab.status_label.text().to_string(),
            "working_dir": tab.working_dir,
            "pid": tab.child_pid,
            "font_family": font_family,
            "screen_text": screen_text,
            "title": tab.title,
            "has_focus": tab.vte.has_focus(),
        }))
    }

    pub fn feed_tab_command(&self, id: u64, cmd: &str) -> bool {
        let vte = self
            .inner
            .state
            .borrow()
            .tabs
            .get(&id)
            .map(|t| t.vte.clone());
        let Some(vte) = vte else {
            return false;
        };
        vte.feed_child(cmd.as_bytes());
        true
    }

    pub fn restart_tab_by_id(&self, id: u64) {
        TerminalHost::restart_tab(&self.inner, id);
    }
}

fn decode_wait_status(raw: i32) -> i32 {
    #[cfg(unix)]
    {
        if libc::WIFEXITED(raw) {
            libc::WEXITSTATUS(raw)
        } else if libc::WIFSIGNALED(raw) {
            128 + libc::WTERMSIG(raw)
        } else {
            raw
        }
    }
    #[cfg(not(unix))]
    {
        raw
    }
}

fn terminate_async_with_kill_fallback(pid: i32) {
    signal_process_group(pid, false);
    glib::timeout_add_local_once(Duration::from_millis(220), move || {
        if is_pid_alive(pid) {
            signal_process_group(pid, true);
        }
    });
}

fn font_description_for(terminal_font: Option<&str>) -> pango::FontDescription {
    let mut desc = match terminal_font.map(str::trim).filter(|s| !s.is_empty()) {
        Some(spec) => {
            let mut d = pango::FontDescription::from_string(spec);
            if d.size() <= 0 {
                d.set_size(11 * pango::SCALE);
            }
            d
        }
        None => pango::FontDescription::from_string("Monospace 11"),
    };
    if desc.family().is_none() {
        desc.set_family("monospace");
    }
    desc
}

fn rgba(r: u8, g: u8, b: u8) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
}

fn apply_vte_colors(vte: &vte4::Terminal, dark: bool) {
    let (fg, bg, palette) = if dark {
        (
            rgba(235, 237, 240),
            rgba(28, 30, 34),
            [
                rgba(46, 52, 64),
                rgba(224, 108, 117),
                rgba(152, 195, 121),
                rgba(229, 192, 123),
                rgba(97, 175, 239),
                rgba(198, 120, 221),
                rgba(86, 182, 194),
                rgba(215, 218, 224),
                rgba(76, 86, 106),
                rgba(240, 128, 138),
                rgba(169, 212, 138),
                rgba(242, 207, 140),
                rgba(120, 193, 250),
                rgba(214, 143, 235),
                rgba(107, 201, 212),
                rgba(248, 249, 251),
            ],
        )
    } else {
        (
            rgba(36, 41, 47),
            rgba(250, 250, 252),
            [
                rgba(36, 41, 47),
                rgba(207, 34, 46),
                rgba(17, 99, 41),
                rgba(154, 103, 0),
                rgba(9, 105, 218),
                rgba(130, 80, 223),
                rgba(27, 124, 131),
                rgba(110, 119, 129),
                rgba(87, 96, 106),
                rgba(220, 56, 68),
                rgba(26, 127, 55),
                rgba(191, 135, 0),
                rgba(33, 139, 255),
                rgba(164, 117, 249),
                rgba(49, 146, 170),
                rgba(255, 255, 255),
            ],
        )
    };
    let refs: [&gdk::RGBA; 16] = std::array::from_fn(|i| &palette[i]);
    vte.set_colors(Some(&fg), Some(&bg), &refs);
    vte.set_color_cursor(Some(&fg));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_host_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let host = TerminalHost::new();
        let weak = Rc::downgrade(&host.inner);
        assert_eq!(weak.strong_count(), 1);
        host.teardown();
        drop(host);
        assert_eq!(weak.strong_count(), 0);
    }
}
