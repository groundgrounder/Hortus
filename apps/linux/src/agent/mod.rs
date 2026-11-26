// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod changes_panel;
pub mod custom_dialog;
pub mod diff_view;
pub mod launcher;

pub use changes_panel::{ChangesCallbacks, ChangesPanel};
pub use custom_dialog::CustomAgentDialog;
pub use launcher::{AgentLaunchCallbacks, AgentLaunchTarget, launch_agent_with_snapshot_gate};
