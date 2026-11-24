// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod assemble;
pub mod focus;
pub mod prepare;
pub mod surface;

#[cfg(feature = "automation")]
pub use focus::FocusSnapshot;
pub use focus::{FocusCallbacks, FocusController, FocusRequest};
#[cfg(feature = "automation")]
pub use prepare::NoteReadingStats;
pub use prepare::{PrepareReadingRequest, PreparedReadingNote, prepare_reading_note_bg};
pub use surface::{ReadingEnv, ReadingSurfaceHandle, build_reading_surface};
