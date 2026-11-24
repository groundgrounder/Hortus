// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod async_bus;
pub mod cli;
pub mod model;
pub mod text;

pub use async_bus::{CommandQueue, NotifyQueue};
pub use cli::CliOptions;
pub use model::{
    APP_ID, APP_VERSION, ColorSchemeChoice, PanelPlacement, PanelRegion, PlaceholderKind,
    TreeEntryItem, is_note_file_name, sort_tree_entries,
};
pub use text::{
    Utf16Mapper, decode_source_styles, find_char_ranges, line_list_continuation, task_toggle_at,
    utf16_offset_to_char_offset,
};
