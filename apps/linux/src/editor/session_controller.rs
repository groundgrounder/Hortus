// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 每标签独立文档会话状态：增量编辑同步、世代防竞态与定时器生命周期管理。

use gtk4::glib;
use hortus_ffi::DocumentIssue;
use std::time::Instant;

/// 每个编辑标签页与内核同步的状态；拆出后避免信号回调重入窗口状态借用。
pub struct EditState {
    pub path: String,
    pub text: String,
    /// 程序加载/重置正文时暂停把缓冲区变更发给内核。
    pub suppress: bool,
    pub dirty: bool,
    pub revision: u64,
    pub issue: Option<DocumentIssue>,
    pub message: String,
    pub save_inflight: bool,
    pub last_save_micros: u64,
    pub restyle_gen: u64,
    pub autosave_gen: u64,
    pub refresh_gen: u64,
    pub edit_seq: u64,
    pub restyles: u32,
    pub saves: u32,
    pub reading_reloads: u32,
    pub last_style_micros: u64,
    pub load_style_micros: u64,
    pub last_saved_at: Option<Instant>,
    pub refreshes: u32,
    #[allow(dead_code)]
    pub deferred_refreshes: u32,
    pub poll_refreshes: u32,
    pub last_refresh_issue: Option<DocumentIssue>,
    pub disk_stamp: Option<(u64, u128)>,
    pub restyle_source: Option<glib::SourceId>,
    pub autosave_source: Option<glib::SourceId>,
    pub checkpoint_source: Option<glib::SourceId>,
}

impl EditState {
    pub fn new(path: &str) -> Self {
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
            refresh_gen: 0,
            edit_seq: 0,
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
            restyle_source: None,
            autosave_source: None,
            checkpoint_source: None,
        }
    }

    pub fn cancel_timers(&mut self) {
        if let Some(id) = self.restyle_source.take() {
            id.remove();
        }
        if let Some(id) = self.autosave_source.take() {
            id.remove();
        }
        if let Some(id) = self.checkpoint_source.take() {
            id.remove();
        }
    }
}

impl Drop for EditState {
    fn drop(&mut self) {
        self.cancel_timers();
    }
}
