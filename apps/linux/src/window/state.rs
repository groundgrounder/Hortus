// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::model::HeadingItem;
use crate::editor::{EditState, FindBar, LinkCompletion};
use crate::platform::HortusReadingViewport;
use crate::reading::ReadingSurfaceHandle;
use crate::render::diagram::RenderedDiagram;
use crate::render::math::MathRenderService;
use crate::services::{AppServices, VaultContext};
use gtk4::glib;
use hortus_ffi::DocumentSession;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(feature = "automation")]
use std::time::Instant;

pub struct OpenNoteTab {
    pub path: String,
    pub page: adw::TabPage,
    pub content_stack: gtk4::Stack,
    pub reading_scrolled: gtk4::ScrolledWindow,
    pub reading_viewport: HortusReadingViewport,
    pub reading_slot: gtk4::Box,
    pub reading: Option<ReadingSurfaceHandle>,
    pub prepared_diagrams: Vec<RenderedDiagram>,
    /// 当前阅读表面使用的排版内容宽；用于防抖阈值判断与自动化报告。
    pub layout_width: f64,
    /// 阅读表面重建代号：丢弃过期的后台重排结果。
    pub reading_generation: u64,
    /// 当前阅读表面对应的文档修订号：用于判断切回阅读态是否需要刷新预览。
    pub reading_revision: u64,
    /// 阅读表面整棵销毁重建次数（窗口缩放期间应保持为 0）。
    pub surface_rebuilds: u32,
    /// 原地宽度重排次数。
    pub in_place_relayouts: u32,
    pub edit_scrolled: gtk4::ScrolledWindow,
    pub edit_view: gtk4::TextView,
    pub edit_buffer: gtk4::TextBuffer,
    pub text: String,
    pub headings: Vec<HeadingItem>,
    pub session: Option<Arc<DocumentSession>>,
    pub editing: bool,
    pub source_loaded: bool,
    pub reading_loaded: bool,
    pub pending_heading: Option<String>,
    pub pending_scroll: Option<u64>,
    pub notice: gtk4::Revealer,
    pub notice_label: gtk4::Label,
    pub notice_actions: gtk4::Box,
    pub find: FindBar,
    pub completion: LinkCompletion,
    pub edit_state: Rc<RefCell<EditState>>,
}

pub struct RestoredTabsOverride {
    pub tabs: Vec<String>,
    pub selected: Option<String>,
    pub views: HashMap<String, String>,
}

pub struct WindowState {
    #[cfg(feature = "automation")]
    pub started_at: Instant,
    pub self_keepalive: Option<Rc<crate::window::shell::WindowInner>>,
    pub services: Option<Arc<AppServices>>,
    pub vault_ctx: Option<Arc<VaultContext>>,
    pub vault_sub_id: Option<u64>,
    pub vault_open_gen: u64,
    pub window_reg_id: Option<u64>,
    pub window_id: String,
    pub login_path: Option<String>,
    pub initial_tabs_override: Option<RestoredTabsOverride>,
    pub initial_tab_views: HashMap<String, String>,
    pub suppressing_session_save: bool,
    pub math_service: Arc<MathRenderService>,
    pub scan_complete: bool,
    pub scan_micros: u64,
    pub pending_open_notes: Vec<String>,
    pub tabs: HashMap<String, OpenNoteTab>,
    pub left_sidebar_width: f64,
    pub right_sidebar_width: f64,
    pub pending_sidebar_widths: bool,
    pub bottom_revealed: bool,
    pub bottom_height: f64,
    pub pending_bottom_position: bool,
    pub suppressing_bottom_pos: bool,
    pub suppressing_layout_save: bool,
    pub layout_save_source: Option<glib::SourceId>,
    #[cfg(feature = "automation")]
    pub automation_triggered: bool,
    /// 原子保存与冲突副本的临时目录，始终位于笔记库之外。
    pub temp_dir: PathBuf,
    pub last_toast: Option<String>,
    /// 重命名/回收站执行期间暂停外部文件刷新，避免把自身操作误判为外部改动。
    pub files_busy: bool,
    pub pending_file_changes: Vec<String>,
    pub file_change_source: Option<glib::SourceId>,
    pub disk_poll_source: Option<glib::SourceId>,
    #[cfg(feature = "automation")]
    pub automation_source: Option<glib::SourceId>,
    #[cfg(feature = "automation")]
    pub overview_source: Option<glib::SourceId>,
    pub close_approved: bool,
    pub last_alert: Option<String>,
    pub last_alert_dialog: glib::WeakRef<adw::AlertDialog>,
    /// 窗口销毁期间不再拦截 close-page，避免对已关闭页面调用 close_page_finish。
    pub disposing: bool,
    #[cfg(feature = "automation")]
    pub last_stage: String,
    pub close_requests: u32,
    pub close_unsaved: u32,
    /// 回收站等已确认删除的路径：绕过关闭保护直接关闭对应标签页。
    pub force_close: HashSet<String>,
    pub last_file_op: Option<serde_json::Value>,
}

impl WindowState {
    pub fn vault_root(&self) -> Option<String> {
        self.vault_ctx.as_ref().map(|ctx| ctx.root.clone())
    }

    pub fn vault_handle(&self) -> Option<Arc<hortus_ffi::VaultHandle>> {
        self.vault_ctx.as_ref().map(|ctx| Arc::clone(&ctx.handle))
    }

    pub fn cancel_global_timers(&mut self) {
        if let Some(id) = self.disk_poll_source.take() {
            id.remove();
        }
        if let Some(id) = self.file_change_source.take() {
            id.remove();
        }
        if let Some(id) = self.layout_save_source.take() {
            id.remove();
        }
        #[cfg(feature = "automation")]
        {
            if let Some(id) = self.automation_source.take() {
                id.remove();
            }
            if let Some(id) = self.overview_source.take() {
                id.remove();
            }
        }
    }
}
