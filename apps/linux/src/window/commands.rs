// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 窗口级串行指令：将 GTK 回调与跨模块动作收敛到单一非重入队列。

use crate::core::{ColorSchemeChoice, PanelRegion, StaticToolKind};
use crate::dock::DockTabId;
use crate::services::ConfigSnapshot;
use crate::settings::SettingsPage;
use hortus_ffi::SaveChoice;
use libadwaita as adw;

pub enum WindowCommand {
    OpenNote {
        relpath: String,
        heading: Option<String>,
    },
    ResolveWikilink {
        source_note: String,
        raw_target: String,
        heading: Option<String>,
    },
    RelayoutActiveTab(i32),
    SwitchActiveEditMode,
    SwitchToSourceAtByte {
        note_path: String,
        byte_start: u64,
    },
    OpenFocus {
        note_path: String,
        ordinal: u32,
    },
    OpenTagFromReading(String),
    FocusSearchPanel,
    OpenQuickOpen,
    SaveNote {
        path: String,
        choice: SaveChoice,
        announce: bool,
    },
    ToggleBottomPanel(bool),
    DockNewShell(PanelRegion),
    DockLaunchAgent(PanelRegion),
    DockLaunchAgentProfile {
        region: PanelRegion,
        profile_id: String,
    },
    DockLaunchCustomAgent(PanelRegion),
    DockAddTool {
        region: PanelRegion,
        tool: StaticToolKind,
    },
    DockMoveTab {
        tab_id: DockTabId,
        region: PanelRegion,
    },
    DockCloseTab(DockTabId),
    CloseTabPage(adw::TabPage),
    ApplyColorScheme(ColorSchemeChoice),
    NewWindow,
    OpenSettings(Option<SettingsPage>),
    LaunchAgent,
    LaunchAgentProfile(String),
    OpenCustomAgentDialog(Option<PanelRegion>),
    SendPathToTerminal,
    SendSelectionToTerminal,
    ConfigChanged(ConfigSnapshot),
    ExternalVaultRefresh(Vec<String>),
    ExternalVaultRename(Vec<(String, String)>),
}
