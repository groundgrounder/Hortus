// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod fonts;
pub mod icons;
pub mod pangocairo;
pub mod source_styles;
pub mod style;
pub mod viewport;

pub use viewport::{
    HortusEditHost, HortusReadingViewport, READING_COLUMN_MAX_WIDTH, effective_reading_column_width,
};
