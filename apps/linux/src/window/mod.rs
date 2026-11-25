// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod actions;
pub mod commands;
pub mod edit_sync;
pub mod file_ops;
pub mod menu;
pub mod outline;
pub mod reading_layout;
pub mod save_notice;
pub mod session;
pub mod shell;
pub mod sidebar_resize;
pub mod state;
pub mod tab_manager;
pub mod terminal_agent;
pub mod vault_lifecycle;

pub use shell::HortusWindow;
