// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod app_services;
pub mod config_service;
pub mod mcp_helper;
pub mod ui_prefs;
pub mod vault_registry;
pub mod window_registry;

pub use app_services::AppServices;
pub use config_service::ConfigSnapshot;
pub use vault_registry::VaultContext;
pub use window_registry::{WindowBridge, WindowSessionEntry};
