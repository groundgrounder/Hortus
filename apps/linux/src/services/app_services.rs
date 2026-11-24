// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::platform::fonts::ensure_katex_fonts_registered_sync;
use crate::render::math::MathRenderService;
use crate::services::config_service::ConfigService;
use crate::services::vault_registry::VaultRegistry;
use crate::services::window_registry::WindowRegistry;
use hortus_ffi::{DiagramEngine, WorkspaceCore};
use libadwaita as adw;
use std::sync::{Arc, Once};

/// 进程唯一应用级共享服务：全部 `Send + Sync`，跨多窗口共享，不持有窗口强引用。
pub struct AppServices {
    pub core: Arc<WorkspaceCore>,
    pub diagram_engine: Arc<DiagramEngine>,
    pub math_service: Arc<MathRenderService>,
    pub vaults: Arc<VaultRegistry>,
    pub windows: Arc<WindowRegistry>,
    pub config: Arc<ConfigService>,
}

static DARK_NOTIFY_ONCE: Once = Once::new();

impl AppServices {
    pub fn init_blocking() -> Result<(Arc<Self>, Option<String>), String> {
        let core = WorkspaceCore::new().map_err(|e| e.to_string())?;
        // 启动时收敛上次异常退出遗留的 preparing/running 会话为 failed/interrupted。
        let _ = core.recover_agent_sessions();
        let diagram_engine = DiagramEngine::new().map_err(|e| format!("{e}"))?;
        let font_warning = ensure_katex_fonts_registered_sync().err();
        let windows = Arc::new(WindowRegistry::new());
        let config = ConfigService::new(Arc::clone(&core), Arc::clone(&windows));
        let services = Arc::new(Self {
            core,
            diagram_engine,
            math_service: Arc::new(MathRenderService::new()),
            vaults: Arc::new(VaultRegistry::new()),
            windows,
            config,
        });
        Ok((services, font_warning))
    }

    /// 全局单点注册 `adw::StyleManager::default().connect_dark_notify`，通过弱注册表广播给存活窗口。
    pub fn ensure_dark_listener(&self) {
        let windows = Arc::clone(&self.windows);
        DARK_NOTIFY_ONCE.call_once(move || {
            adw::StyleManager::default().connect_dark_notify(move |_| {
                windows.broadcast_dark_changed();
            });
        });
    }
}
