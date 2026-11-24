// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 右侧栏「改动（Changes）」面板：文案严格称「本次会话改动」，支持会话选择、后台审阅摘要、
//! 按需逐行 diff、三级安全回滚（单 hunk / 单文件 / 整会话）、`review_stale` 保护与删除会话快照。

use crate::agent::diff_view::render_entry_detail;
use crate::core::is_note_file_name;
use crate::files::operations::save_temp_for;
use crate::knowledge::widgets::{clear_box, dim_label, panel_scroll};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{
    AgentSessionInfo, ReviewCancellation, ReviewDiffHunk, ReviewEntryDetail, ReviewEntryInfo,
    RollbackExpectationInfo, SessionReviewSummary, SessionRollbackReportInfo, WorkspaceCore,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct ChangesCallbacks {
    pub on_open_note: Rc<dyn Fn(String)>,
    pub on_rolled_back: Rc<dyn Fn(Vec<String>)>,
    pub on_toast: Rc<dyn Fn(String)>,
    pub parent_window: Rc<dyn Fn() -> Option<adw::ApplicationWindow>>,
}

#[derive(Default)]
struct ChangesState {
    core: Option<Arc<WorkspaceCore>>,
    vault_root: Option<String>,
    temp_dir: PathBuf,
    sessions: Vec<Arc<AgentSessionInfo>>,
    selected_session_id: Option<String>,
    selected_entry_id: Option<String>,
    summary: Option<Arc<SessionReviewSummary>>,
    detail: Option<Arc<ReviewEntryDetail>>,
    review_gen: u64,
    detail_gen: u64,
    active_cancel: Option<Arc<ReviewCancellation>>,
    callbacks: Option<ChangesCallbacks>,
    suppress_combo: bool,
    last_error: Option<String>,
    last_rollback_report: Option<String>,
}

struct ChangesPanelInner {
    root: gtk4::Box,
    title_lbl: gtk4::Label,
    session_combo: gtk4::DropDown,
    session_meta: gtk4::Label,
    delete_btn: gtk4::Button,
    rollback_all_btn: gtk4::Button,
    entries_box: gtk4::Box,
    detail_title: gtk4::Label,
    detail_box: gtk4::Box,
    state: RefCell<ChangesState>,
}

#[derive(Clone)]
pub struct ChangesPanel {
    inner: Rc<ChangesPanelInner>,
}

impl Default for ChangesPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangesPanel {
    pub fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        root.set_vexpand(true);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        header.add_css_class("hortus-panel-header");
        let title_lbl = gtk4::Label::new(Some("本次会话改动"));
        title_lbl.add_css_class("caption");
        title_lbl.add_css_class("dim-label");
        title_lbl.set_xalign(0.0);
        title_lbl.set_hexpand(true);

        let refresh_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("刷新会话与本次会话改动")
            .build();
        refresh_btn.add_css_class("flat");
        let delete_btn = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("删除选中会话快照")
            .build();
        delete_btn.add_css_class("flat");

        header.append(&title_lbl);
        header.append(&refresh_btn);
        header.append(&delete_btn);
        root.append(&header);

        let session_row = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        session_row.set_margin_start(8);
        session_row.set_margin_end(8);
        let session_combo = gtk4::DropDown::from_strings(&["暂无会话"]);
        session_combo.set_hexpand(true);
        let session_meta = gtk4::Label::new(Some("启动 Agent 后将在此显示本次会话改动。"));
        session_meta.add_css_class("caption");
        session_meta.add_css_class("dim-label");
        session_meta.set_xalign(0.0);
        session_meta.set_wrap(true);

        let rollback_all_btn = gtk4::Button::with_label("回滚整次会话改动");
        rollback_all_btn.add_css_class("destructive-action");
        rollback_all_btn.add_css_class("caption");
        rollback_all_btn.set_sensitive(false);

        session_row.append(&session_combo);
        session_row.append(&session_meta);
        session_row.append(&rollback_all_btn);
        root.append(&session_row);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

        let entries_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        entries_box.set_margin_start(6);
        entries_box.set_margin_end(6);
        entries_box.set_margin_top(4);
        entries_box.set_margin_bottom(4);
        let entries_scroll = panel_scroll(&entries_box);
        entries_scroll.set_min_content_height(130);

        let detail_title = gtk4::Label::new(Some("选择一个改动条目查看逐行 diff"));
        detail_title.add_css_class("caption");
        detail_title.add_css_class("dim-label");
        detail_title.set_xalign(0.0);
        detail_title.set_margin_start(8);
        detail_title.set_margin_end(8);
        detail_title.set_margin_top(4);
        detail_title.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

        let detail_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        detail_box.set_margin_start(6);
        detail_box.set_margin_end(6);
        detail_box.set_margin_bottom(8);
        let detail_scroll = panel_scroll(&detail_box);
        detail_scroll.set_min_content_height(140);

        let paned = gtk4::Paned::builder()
            .orientation(gtk4::Orientation::Vertical)
            .vexpand(true)
            .start_child(&entries_scroll)
            .end_child(&{
                let box_d = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                box_d.append(&detail_title);
                box_d.append(&detail_scroll);
                box_d
            })
            .build();
        root.append(&paned);

        let inner = Rc::new(ChangesPanelInner {
            root,
            title_lbl,
            session_combo,
            session_meta,
            delete_btn,
            rollback_all_btn,
            entries_box,
            detail_title,
            detail_box,
            state: RefCell::new(ChangesState::default()),
        });

        Self::bind_signals(&inner, &refresh_btn);
        Self { inner }
    }

    fn bind_signals(inner: &Rc<ChangesPanelInner>, refresh_btn: &gtk4::Button) {
        {
            let weak = Rc::downgrade(inner);
            refresh_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::refresh_impl(&this);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.delete_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::prompt_delete_session(&this);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.rollback_all_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::prompt_rollback_session(&this);
                }
            });
        }
        {
            let weak = Rc::downgrade(inner);
            inner.session_combo.connect_selected_notify(move |combo| {
                let Some(this) = weak.upgrade() else { return };
                let idx = combo.selected() as usize;
                let selected_id = {
                    let st = this.state.borrow();
                    if st.suppress_combo {
                        return;
                    }
                    st.sessions.get(idx).map(|s| s.id.clone())
                };
                if let Some(id) = selected_id {
                    this.state.borrow_mut().selected_session_id = Some(id.clone());
                    this.state.borrow_mut().selected_entry_id = None;
                    Self::review_selected_session(&this, &id);
                }
            });
        }
    }

    pub fn root(&self) -> &gtk4::Box {
        &self.inner.root
    }

    pub fn set_callbacks(&self, callbacks: ChangesCallbacks) {
        self.inner.state.borrow_mut().callbacks = Some(callbacks);
    }

    pub fn set_context(
        &self,
        core: Arc<WorkspaceCore>,
        vault_root: Option<String>,
        temp_dir: PathBuf,
    ) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.core = Some(core);
            st.vault_root = vault_root;
            st.temp_dir = temp_dir;
            st.selected_session_id = None;
            st.selected_entry_id = None;
            st.summary = None;
            st.detail = None;
        }
        Self::refresh_impl(&self.inner);
    }

    pub fn refresh(&self) {
        Self::refresh_impl(&self.inner);
    }

    fn refresh_impl(inner: &Rc<ChangesPanelInner>) {
        let (core, vault_root) = {
            let st = inner.state.borrow();
            (st.core.clone(), st.vault_root.clone())
        };
        let Some(core) = core else { return };
        let Some(vault) = vault_root.filter(|v| !v.is_empty()) else {
            inner.state.borrow_mut().sessions.clear();
            Self::render_no_sessions(inner, "未打开知识库");
            return;
        };

        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let v_bg = vault.clone();
            let list = gio::spawn_blocking(move || core.list_agent_sessions(Some(v_bg)))
                .await
                .unwrap_or_else(|_| Ok(Vec::new()))
                .unwrap_or_default();
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().vault_root.as_deref() != Some(vault.as_str()) {
                return;
            }
            Self::apply_session_list(&this, list);
        });
    }

    fn apply_session_list(inner: &Rc<ChangesPanelInner>, sessions: Vec<AgentSessionInfo>) {
        let usable_or_all: Vec<AgentSessionInfo> = sessions;
        if usable_or_all.is_empty() {
            inner.state.borrow_mut().sessions.clear();
            inner.state.borrow_mut().selected_session_id = None;
            inner.state.borrow_mut().summary = None;
            inner.state.borrow_mut().detail = None;
            Self::render_no_sessions(inner, "当前知识库暂无 Agent 会话快照");
            return;
        }

        let prev_sel = inner.state.borrow().selected_session_id.clone();
        let chosen_idx = prev_sel
            .as_deref()
            .and_then(|id| usable_or_all.iter().position(|s| s.id == id))
            .unwrap_or(0);
        let chosen_id = usable_or_all[chosen_idx].id.clone();

        let labels: Vec<String> = usable_or_all
            .iter()
            .map(|s| {
                format!(
                    "{} · {} ({} 项)",
                    short_id(&s.id),
                    state_label(&s.state),
                    s.entries
                )
            })
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        {
            let mut st = inner.state.borrow_mut();
            st.suppress_combo = true;
            st.sessions = usable_or_all.into_iter().map(Arc::new).collect();
            st.selected_session_id = Some(chosen_id.clone());
        }
        inner
            .session_combo
            .set_model(Some(&gtk4::StringList::new(&label_refs)));
        inner.session_combo.set_selected(chosen_idx as u32);
        inner.state.borrow_mut().suppress_combo = false;

        Self::review_selected_session(inner, &chosen_id);
    }

    fn render_no_sessions(inner: &Rc<ChangesPanelInner>, msg: &str) {
        inner.state.borrow_mut().suppress_combo = true;
        inner
            .session_combo
            .set_model(Some(&gtk4::StringList::new(&["暂无会话"])));
        inner.session_combo.set_selected(0);
        inner.state.borrow_mut().suppress_combo = false;
        inner.session_meta.set_text(msg);
        inner.delete_btn.set_sensitive(false);
        inner.rollback_all_btn.set_sensitive(false);
        clear_box(&inner.entries_box);
        clear_box(&inner.detail_box);
        inner.entries_box.append(&dim_label(msg));
        inner.detail_title.set_text("本次会话改动详情");
    }

    fn review_selected_session(inner: &Rc<ChangesPanelInner>, session_id: &str) {
        let (core, cancel, rev_gen, session_info) = {
            let mut st = inner.state.borrow_mut();
            if let Some(old) = st.active_cancel.take() {
                old.cancel();
            }
            let cancel = ReviewCancellation::new();
            st.active_cancel = Some(Arc::clone(&cancel));
            st.review_gen = st.review_gen.wrapping_add(1);
            st.summary = None;
            st.detail = None;
            let info = st.sessions.iter().find(|s| s.id == session_id).cloned();
            (st.core.clone(), cancel, st.review_gen, info)
        };
        let Some(core) = core else { return };
        if let Some(ref info) = session_info {
            let active = matches!(info.state.as_str(), "preparing" | "running");
            inner.delete_btn.set_sensitive(!active);
            inner.session_meta.set_text(&format!(
                "状态：{} · 快照 {} 项",
                state_label(&info.state),
                info.entries
            ));
            if !info.usable {
                inner.rollback_all_btn.set_sensitive(false);
                clear_box(&inner.entries_box);
                clear_box(&inner.detail_box);
                inner.entries_box.append(&dim_label(&format!(
                    "会话不可审阅（{}）",
                    state_label(&info.state)
                )));
                return;
            }
        }

        clear_box(&inner.entries_box);
        clear_box(&inner.detail_box);
        inner
            .entries_box
            .append(&dim_label("正在后台对比本次会话改动…"));
        inner.rollback_all_btn.set_sensitive(false);

        let weak = Rc::downgrade(inner);
        let sid = session_id.to_string();
        glib::MainContext::default().spawn_local(async move {
            let sid_bg = sid.clone();
            let res = gio::spawn_blocking(move || core.review_agent_session(sid_bg, cancel))
                .await
                .unwrap_or_else(|_| {
                    Err(hortus_ffi::CoreError::Failure {
                        kind: "history.review_thread_error".into(),
                    })
                });
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().review_gen != rev_gen {
                return;
            }
            match res {
                Ok(summary) => Self::render_summary(&this, summary),
                Err(err) => {
                    this.state.borrow_mut().last_error = Some(err.to_string());
                    clear_box(&this.entries_box);
                    this.entries_box
                        .append(&dim_label(&format!("审阅失败：{err}")));
                }
            }
        });
    }

    fn render_summary(inner: &Rc<ChangesPanelInner>, summary: SessionReviewSummary) {
        clear_box(&inner.entries_box);
        let c = &summary.counts;
        inner
            .title_lbl
            .set_text(&format!("本次会话改动（{}）", summary.entries.len()));
        inner.session_meta.set_text(&format!(
            "状态：{} · +{} / ~{} / -{} / 重命名 {}",
            state_label(&summary.state),
            c.added,
            c.modified,
            c.deleted,
            c.renamed
        ));
        inner
            .rollback_all_btn
            .set_sensitive(!summary.entries.is_empty());

        let prev_entry = inner.state.borrow().selected_entry_id.clone();
        if summary.entries.is_empty() {
            inner.state.borrow_mut().summary = Some(Arc::new(summary));
            inner
                .entries_box
                .append(&dim_label("本次会话尚未产生文件改动。"));
            clear_box(&inner.detail_box);
            inner.detail_title.set_text("无改动详情");
            return;
        }

        let sid = summary.session_id.clone();
        let entries = summary.entries.clone();
        inner.state.borrow_mut().summary = Some(Arc::new(summary));

        for entry in &entries {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            let badge = gtk4::Label::new(Some(change_badge(&entry.change)));
            badge.add_css_class("caption");
            badge.add_css_class("hortus-mode-badge");

            let open_btn = gtk4::Button::builder()
                .label(&entry.id)
                .hexpand(true)
                .build();
            open_btn.add_css_class("flat");
            open_btn.add_css_class("caption");
            if let Some(lbl) = open_btn.child().and_downcast::<gtk4::Label>() {
                lbl.set_xalign(0.0);
                lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            }

            let weak = Rc::downgrade(inner);
            let sid_c = sid.clone();
            let entry_c = entry.clone();
            open_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::load_entry_detail(&this, &sid_c, &entry_c);
                }
            });

            row.append(&badge);
            row.append(&open_btn);

            if entry.change != "deleted" && is_note_file_name(&entry.id) {
                let view_btn = gtk4::Button::builder()
                    .icon_name("document-open-symbolic")
                    .tooltip_text("打开笔记")
                    .build();
                view_btn.add_css_class("flat");
                let weak = Rc::downgrade(inner);
                let path = entry.id.clone();
                view_btn.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade()
                        && let Some(ref cbs) = this.state.borrow().callbacks
                    {
                        (cbs.on_open_note)(path.clone());
                    }
                });
                row.append(&view_btn);
            }

            let rb_btn = gtk4::Button::builder()
                .icon_name("edit-undo-symbolic")
                .tooltip_text("回滚此文件")
                .build();
            rb_btn.add_css_class("flat");
            let weak = Rc::downgrade(inner);
            let sid_r = sid.clone();
            let entry_r = entry.clone();
            rb_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::rollback_entry_impl(&this, sid_r.clone(), entry_r.clone());
                }
            });
            row.append(&rb_btn);

            inner.entries_box.append(&row);
        }

        let target_entry = prev_entry
            .as_deref()
            .and_then(|id| entries.iter().find(|e| e.id == id))
            .or_else(|| entries.first())
            .cloned();
        if let Some(entry) = target_entry {
            Self::load_entry_detail(inner, &sid, &entry);
        }
    }

    fn load_entry_detail(inner: &Rc<ChangesPanelInner>, session_id: &str, entry: &ReviewEntryInfo) {
        let (core, cancel, det_gen) = {
            let mut st = inner.state.borrow_mut();
            st.selected_entry_id = Some(entry.id.clone());
            st.detail_gen = st.detail_gen.wrapping_add(1);
            let cancel = st
                .active_cancel
                .clone()
                .unwrap_or_else(ReviewCancellation::new);
            (st.core.clone(), cancel, st.detail_gen)
        };
        let Some(core) = core else { return };
        inner
            .detail_title
            .set_text(&format!("{} · {}", change_badge(&entry.change), entry.id));
        clear_box(&inner.detail_box);
        inner.detail_box.append(&dim_label("正在加载 diff…"));

        let weak = Rc::downgrade(inner);
        let sid = session_id.to_string();
        let path = entry.id.clone();
        let old_path = entry.old_path.clone();
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || {
                core.review_agent_session_entry(sid, path, old_path, cancel)
            })
            .await
            .unwrap_or_else(|_| {
                Err(hortus_ffi::CoreError::Failure {
                    kind: "history.detail_thread_error".into(),
                })
            });
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().detail_gen != det_gen {
                return;
            }
            match res {
                Ok(detail) => Self::render_detail(&this, detail),
                Err(err) => {
                    clear_box(&this.detail_box);
                    this.detail_box
                        .append(&dim_label(&format!("加载详情失败：{err}")));
                }
            }
        });
    }

    fn render_detail(inner: &Rc<ChangesPanelInner>, detail: ReviewEntryDetail) {
        let sid = detail.entry.id.clone();
        let entry = detail.entry.clone();
        let session_id = inner
            .state
            .borrow()
            .selected_session_id
            .clone()
            .unwrap_or_default();
        let detail_arc = Arc::new(detail);
        inner.state.borrow_mut().detail = Some(Arc::clone(&detail_arc));
        let weak = Rc::downgrade(inner);
        render_entry_detail(
            &inner.detail_box,
            &detail_arc,
            Rc::new(move |hunk_idx, hunk| {
                if let Some(this) = weak.upgrade() {
                    Self::rollback_hunk_impl(
                        &this,
                        session_id.clone(),
                        entry.clone(),
                        hunk_idx,
                        hunk,
                    );
                }
            }),
        );
        let _ = sid;
    }

    fn rollback_temp_dir(inner: &Rc<ChangesPanelInner>) -> Option<String> {
        let st = inner.state.borrow();
        let vault_root = st.vault_root.as_deref()?;
        let dir = save_temp_for(&st.temp_dir, vault_root, "");
        Some(dir.to_string_lossy().into_owned())
    }

    fn expectation_for(entry: &ReviewEntryInfo) -> RollbackExpectationInfo {
        RollbackExpectationInfo {
            path: entry.id.clone(),
            old_path: entry.old_path.clone(),
            new_path: entry.new_path.clone(),
            change: entry.change.clone(),
            expected_before: entry.before_digest.clone(),
            expected_after: entry.after_digest.clone(),
        }
    }

    fn rollback_hunk_impl(
        inner: &Rc<ChangesPanelInner>,
        session_id: String,
        entry: ReviewEntryInfo,
        hunk_index: u32,
        hunk: ReviewDiffHunk,
    ) {
        let Some(core) = inner.state.borrow().core.clone() else {
            return;
        };
        let temp = Self::rollback_temp_dir(inner);
        let expectation = Self::expectation_for(&entry);
        let cancel = ReviewCancellation::new();
        let weak = Rc::downgrade(inner);
        let path_for_cb = entry.id.clone();
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || {
                core.rollback_agent_session_hunk(
                    session_id,
                    entry.id,
                    entry.old_path,
                    expectation,
                    hunk_index,
                    hunk,
                    temp,
                    cancel,
                )
            })
            .await
            .unwrap_or_else(|_| {
                Err(hortus_ffi::CoreError::Failure {
                    kind: "history.rollback_thread_error".into(),
                })
            });
            let Some(this) = weak.upgrade() else { return };
            Self::handle_single_rollback_result(&this, res, vec![path_for_cb]);
        });
    }

    fn rollback_entry_impl(
        inner: &Rc<ChangesPanelInner>,
        session_id: String,
        entry: ReviewEntryInfo,
    ) {
        let Some(core) = inner.state.borrow().core.clone() else {
            return;
        };
        let temp = Self::rollback_temp_dir(inner);
        let expectation = Self::expectation_for(&entry);
        let cancel = ReviewCancellation::new();
        let mut affected = vec![entry.id.clone()];
        if let Some(ref old) = entry.old_path {
            affected.push(old.clone());
        }
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || {
                core.rollback_agent_session_entry(session_id, expectation, temp, cancel)
            })
            .await
            .unwrap_or_else(|_| {
                Err(hortus_ffi::CoreError::Failure {
                    kind: "history.rollback_thread_error".into(),
                })
            });
            let Some(this) = weak.upgrade() else { return };
            Self::handle_single_rollback_result(&this, res, affected);
        });
    }

    fn handle_single_rollback_result(
        inner: &Rc<ChangesPanelInner>,
        res: Result<(), hortus_ffi::CoreError>,
        affected: Vec<String>,
    ) {
        match res {
            Ok(()) => {
                inner.state.borrow_mut().last_error = None;
                inner.state.borrow_mut().last_rollback_report = Some("reverted".into());
                if let Some(ref cbs) = inner.state.borrow().callbacks {
                    (cbs.on_toast)("已安全回滚到快照内容".into());
                    (cbs.on_rolled_back)(affected);
                }
                Self::refresh_impl(inner);
            }
            Err(err) => {
                let code = err.to_string();
                inner.state.borrow_mut().last_error = Some(code.clone());
                let msg = if code == "history.review_stale" {
                    "文件在审阅后已被外部修改（review_stale），已中止回滚，请先刷新审阅后再决定。"
                        .into()
                } else {
                    format!("回滚失败：{code}")
                };
                if let Some(ref cbs) = inner.state.borrow().callbacks {
                    (cbs.on_toast)(msg);
                }
            }
        }
    }

    fn prompt_rollback_session(inner: &Rc<ChangesPanelInner>) {
        let (sid, count, parent) = {
            let st = inner.state.borrow();
            let sid = st.selected_session_id.clone();
            let count = st.summary.as_ref().map(|s| s.entries.len()).unwrap_or(0);
            let parent = st.callbacks.as_ref().and_then(|c| (c.parent_window)());
            (sid, count, parent)
        };
        let (Some(sid), Some(win)) = (sid, parent) else {
            return;
        };
        if count == 0 {
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("回滚整次会话改动？")
            .body(format!(
                "将按本次审阅基线回滚 {count} 个受影响条目。磁盘上已被外部修改的条目会以冲突跳过，绝不静默覆盖。"
            ))
            .build();
        dialog.add_response("cancel", "取消");
        dialog.add_response("rollback", "全部回滚");
        dialog.set_response_appearance("rollback", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(inner);
        dialog.connect_response(None, move |_, resp| {
            if resp == "rollback"
                && let Some(this) = weak.upgrade()
            {
                Self::rollback_session_impl(&this, sid.clone());
            }
        });
        dialog.present(Some(&win));
    }

    fn rollback_session_impl(inner: &Rc<ChangesPanelInner>, session_id: String) {
        let (core, expectations) = {
            let st = inner.state.borrow();
            let Some(ref summary) = st.summary else {
                return;
            };
            let list: Vec<RollbackExpectationInfo> =
                summary.entries.iter().map(Self::expectation_for).collect();
            (st.core.clone(), list)
        };
        let Some(core) = core else { return };
        let temp = Self::rollback_temp_dir(inner);
        let cancel = ReviewCancellation::new();
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || {
                core.rollback_agent_session(session_id, expectations, temp, cancel)
            })
            .await
            .unwrap_or_else(|_| {
                Err(hortus_ffi::CoreError::Failure {
                    kind: "history.rollback_thread_error".into(),
                })
            });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(report) => Self::handle_session_rollback_report(&this, report),
                Err(err) => {
                    this.state.borrow_mut().last_error = Some(err.to_string());
                    if let Some(ref cbs) = this.state.borrow().callbacks {
                        (cbs.on_toast)(format!("整会话回滚失败：{err}"));
                    }
                }
            }
        });
    }

    fn handle_session_rollback_report(
        inner: &Rc<ChangesPanelInner>,
        report: SessionRollbackReportInfo,
    ) {
        let msg = format!(
            "回滚完成：成功 {} 项，过期冲突跳过 {} 项，失败 {} 项。",
            report.reverted_paths.len(),
            report.skipped_stale.len(),
            report.failed.len()
        );
        inner.state.borrow_mut().last_rollback_report = Some(msg.clone());
        let parent = inner
            .state
            .borrow()
            .callbacks
            .as_ref()
            .and_then(|c| (c.parent_window)());
        if (!report.skipped_stale.is_empty() || !report.failed.is_empty())
            && let Some(win) = parent
        {
            let mut detail = String::from(&msg);
            if !report.skipped_stale.is_empty() {
                detail.push_str("\n\n以下文件在审阅后已被磁盘外部修改（review_stale，已跳过）：\n");
                for item in &report.skipped_stale {
                    detail.push_str(&format!("· {} ({})\n", item.path, item.reason));
                }
            }
            if !report.failed.is_empty() {
                detail.push_str("\n以下文件回滚失败：\n");
                for item in &report.failed {
                    detail.push_str(&format!("· {} ({})\n", item.path, item.reason));
                }
            }
            let alert = adw::AlertDialog::builder()
                .heading("整会话回滚报告")
                .body(detail)
                .build();
            alert.add_response("ok", "知道了");
            alert.set_default_response(Some("ok"));
            alert.present(Some(&win));
        }
        if let Some(ref cbs) = inner.state.borrow().callbacks {
            (cbs.on_toast)(msg);
            if !report.reverted_paths.is_empty() {
                (cbs.on_rolled_back)(report.reverted_paths);
            }
        }
        Self::refresh_impl(inner);
    }

    fn prompt_delete_session(inner: &Rc<ChangesPanelInner>) {
        let (sid, active, parent) = {
            let st = inner.state.borrow();
            let sid = st.selected_session_id.clone();
            let active = sid.as_deref().is_some_and(|id| {
                st.sessions
                    .iter()
                    .find(|s| s.id == id)
                    .is_some_and(|s| matches!(s.state.as_str(), "preparing" | "running"))
            });
            let parent = st.callbacks.as_ref().and_then(|c| (c.parent_window)());
            (sid, active, parent)
        };
        if active {
            if let Some(ref cbs) = inner.state.borrow().callbacks {
                (cbs.on_toast)("运行中的 Agent 会话禁止删除快照".into());
            }
            return;
        }
        let (Some(sid), Some(win)) = (sid, parent) else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading("删除本次会话快照？")
            .body("仅删除应用数据目录中的历史快照，不会修改或删除知识库内的任何笔记文件。")
            .build();
        dialog.add_response("cancel", "取消");
        dialog.add_response("delete", "删除快照");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(inner);
        dialog.connect_response(None, move |_, resp| {
            if resp == "delete"
                && let Some(this) = weak.upgrade()
            {
                Self::delete_session_impl(&this, sid.clone());
            }
        });
        dialog.present(Some(&win));
    }

    fn delete_session_impl(inner: &Rc<ChangesPanelInner>, session_id: String) {
        let Some(core) = inner.state.borrow().core.clone() else {
            return;
        };
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || core.delete_agent_session(session_id))
                .await
                .unwrap_or_else(|_| {
                    Err(hortus_ffi::CoreError::Failure {
                        kind: "history.delete_thread_error".into(),
                    })
                });
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(()) => {
                    this.state.borrow_mut().selected_session_id = None;
                    if let Some(ref cbs) = this.state.borrow().callbacks {
                        (cbs.on_toast)("已删除会话快照（知识库文件未受影响）".into());
                    }
                    Self::refresh_impl(&this);
                }
                Err(err) => {
                    this.state.borrow_mut().last_error = Some(err.to_string());
                    if let Some(ref cbs) = this.state.borrow().callbacks {
                        (cbs.on_toast)(format!("删除快照失败：{err}"));
                    }
                }
            }
        });
    }
}

#[cfg(feature = "automation")]
impl ChangesPanel {
    pub fn sessions(&self) -> Vec<Arc<AgentSessionInfo>> {
        self.inner.state.borrow().sessions.clone()
    }

    pub fn select_session(&self, id: &str) {
        self.inner.state.borrow_mut().selected_session_id = Some(id.to_string());
        ChangesPanel::review_selected_session(&self.inner, id);
    }

    pub fn summary_snapshot(&self) -> Option<Arc<SessionReviewSummary>> {
        self.inner.state.borrow().summary.clone()
    }

    pub fn select_entry(&self, entry_id: &str) -> bool {
        let Some(summary) = self.inner.state.borrow().summary.clone() else {
            return false;
        };
        let Some(entry) = summary.entries.iter().find(|e| e.id == entry_id).cloned() else {
            return false;
        };
        ChangesPanel::load_entry_detail(&self.inner, &summary.session_id, &entry);
        true
    }

    pub fn detail_snapshot(&self) -> Option<Arc<ReviewEntryDetail>> {
        self.inner.state.borrow().detail.clone()
    }

    pub fn rollback_hunk_now(&self, hunk_index: u32) -> bool {
        let (sid, detail) = {
            let st = self.inner.state.borrow();
            (st.selected_session_id.clone(), st.detail.clone())
        };
        let (Some(sid), Some(detail)) = (sid, detail) else {
            return false;
        };
        let Some(hunk) = detail.hunks.get(hunk_index as usize).cloned() else {
            return false;
        };
        ChangesPanel::rollback_hunk_impl(&self.inner, sid, detail.entry.clone(), hunk_index, hunk);
        true
    }

    pub fn rollback_entry_now(&self, entry_id: &str) -> bool {
        let (sid, summary) = {
            let st = self.inner.state.borrow();
            (st.selected_session_id.clone(), st.summary.clone())
        };
        let (Some(sid), Some(summary)) = (sid, summary) else {
            return false;
        };
        let Some(entry) = summary.entries.iter().find(|e| e.id == entry_id).cloned() else {
            return false;
        };
        ChangesPanel::rollback_entry_impl(&self.inner, sid, entry);
        true
    }

    pub fn rollback_session_now(&self) -> bool {
        let Some(sid) = self.inner.state.borrow().selected_session_id.clone() else {
            return false;
        };
        ChangesPanel::rollback_session_impl(&self.inner, sid);
        true
    }

    pub fn delete_session_now(&self, id: &str) {
        ChangesPanel::delete_session_impl(&self.inner, id.to_string());
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.state.borrow().last_error.clone()
    }

    pub fn last_rollback_report(&self) -> Option<String> {
        self.inner.state.borrow().last_rollback_report.clone()
    }

    pub fn title_text(&self) -> String {
        self.inner.title_lbl.text().to_string()
    }
}

fn short_id(id: &str) -> &str {
    if id.len() > 8 { &id[..8] } else { id }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "preparing" => "准备快照",
        "ready" => "已就绪",
        "running" => "运行中",
        "exited" => "已退出",
        "interrupted" => "已中断",
        "cancelled" => "已取消",
        "failed" => "失败",
        _ => "未知",
    }
}

fn change_badge(change: &str) -> &'static str {
    match change {
        "added" => "新增",
        "modified" => "修改",
        "deleted" => "删除",
        "renamed" => "重命名",
        "type_changed" => "类型变化",
        _ => "改动",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_panel_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let panel = ChangesPanel::new();
        let weak = Rc::downgrade(&panel.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(panel);
        assert_eq!(weak.strong_count(), 0);
    }
}
