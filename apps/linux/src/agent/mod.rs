// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod changes_panel;
pub mod diff_view;
pub mod launcher;

pub use changes_panel::{ChangesCallbacks, ChangesPanel};
pub use launcher::{AgentLaunchCallbacks, launch_agent_with_snapshot_gate};
