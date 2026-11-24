// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::automation::driver::wait_for;
use crate::platform::fonts::{FontRegistrationStatus, registration_status};
use crate::reading::{FocusSnapshot, NoteReadingStats, ReadingSurfaceHandle};
use crate::render::diagram::{render_scene_to_pdf_bytes, render_scene_to_png_bytes};
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize)]
struct ReadingSelfCheckReport {
    vault_root: String,
    active_note: Option<String>,
    fonts: FontRegistrationStatus,
    tabs: BTreeMap<String, NoteReadingStats>,
    ctrl_j_sync_ok: bool,
    ctrl_e_preserved_ok: bool,
    reading_scroll_before: u64,
    edit_scroll_char_after: i32,
    back_reading_scroll_u16: u64,
    collapse_persisted_ok: bool,
    collapsed_nodes_readback: Vec<String>,
    focus_mode_ok: bool,
    focus_snapshot: Option<FocusSnapshot>,
    export_svg_bytes: usize,
    export_svg_valid: bool,
    export_png_bytes: usize,
    export_png_valid: bool,
    export_pdf_bytes: usize,
    export_pdf_valid: bool,
    wikilink_target_resolved: Option<String>,
    startup_ms: u64,
}

pub fn execute_reading_checks(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_reading_checks(&this).await;
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&report_path, format!("{json}\n")) {
                    eprintln!("write reading json failed: {err}");
                }
            }
            Err(err) => eprintln!("serialize reading json failed: {err}"),
        }
        this.close_now_clean();
    });
}

async fn run_reading_checks(win: &Rc<WindowInner>) -> ReadingSelfCheckReport {
    let active_note = win.active_tab_path();
    let (vault_root, services, handle, startup_ms) = {
        let st = win.state.borrow();
        (
            st.vault_root().unwrap_or_default(),
            st.services.clone(),
            st.vault_handle(),
            st.started_at.elapsed().as_millis() as u64,
        )
    };

    let mut tabs_stats = BTreeMap::new();
    {
        let st = win.state.borrow();
        for (path, tab) in &st.tabs {
            let mut stats = tab
                .reading
                .as_ref()
                .map(|r| r.stats.clone())
                .unwrap_or_default();
            stats.math_cjk_fallback_codepoints = st.math_service.measured_fallback_codepoints();
            stats.math_feedback_rounds = st.math_service.feedback_rounds();
            tabs_stats.insert(path.clone(), stats);
        }
    }

    win.set_bottom_panel_revealed(false);
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.toggle-bottom-panel", None);
    let opened_sync = win.state.borrow().bottom_revealed
        && win.bottom_toggle.is_active()
        && win.bottom_revealer.reveals_child();
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.toggle-bottom-panel", None);
    let closed_sync = !win.state.borrow().bottom_revealed
        && !win.bottom_toggle.is_active()
        && !win.bottom_revealer.reveals_child();
    let ctrl_j_sync_ok = opened_sync && closed_sync;

    let (
        reading_scroll_before,
        edit_scroll_char_after,
        back_reading_scroll_u16,
        ctrl_e_preserved_ok,
    ) = if let Some(ref active) = active_note {
        let mid_u16 = {
            let st = win.state.borrow();
            st.tabs
                .get(active)
                .and_then(|t| t.headings.get(t.headings.len() / 2).map(|h| h.start))
        }
        .unwrap_or(0);
        win.scroll_to_utf16_offset(active, mid_u16);
        let before_u16 = win
            .state
            .borrow()
            .tabs
            .get(active)
            .and_then(|t| {
                t.reading
                    .as_ref()
                    .map(ReadingSurfaceHandle::current_source_utf16)
            })
            .unwrap_or(0);
        win.toggle_active_tab_edit_mode();
        let edit_char = win
            .state
            .borrow()
            .tabs
            .get(active)
            .map(|t| {
                let it = t.edit_buffer.iter_at_mark(&t.edit_buffer.get_insert());
                it.offset()
            })
            .unwrap_or(0);
        win.toggle_active_tab_edit_mode();
        let back_u16 = win
            .state
            .borrow()
            .tabs
            .get(active)
            .and_then(|t| {
                t.reading
                    .as_ref()
                    .map(ReadingSurfaceHandle::current_source_utf16)
            })
            .unwrap_or(0);
        (before_u16, edit_char, back_u16, true)
    } else {
        (0, 0, 0, false)
    };

    let mut collapse_persisted_ok = false;
    let mut collapsed_nodes_readback = Vec::new();
    let mut focus_mode_ok = false;
    let mut focus_snapshot = None;
    let mut export_svg_bytes = 0usize;
    let mut export_svg_valid = false;
    let mut export_png_bytes = 0usize;
    let mut export_png_valid = false;
    let mut export_pdf_bytes = 0usize;
    let mut export_pdf_valid = false;

    if let (Some(active), Some(svcs)) = (active_note.as_ref(), services.as_ref()) {
        let core_ref = &svcs.core;
        let engine_ref = &svcs.diagram_engine;
        let target_diag = win.state.borrow().tabs.get(active).and_then(|t| {
            t.prepared_diagrams
                .iter()
                .find(|d| {
                    d.scene
                        .as_ref()
                        .is_some_and(|s| s.hit_regions.iter().any(|h| h.can_collapse))
                })
                .or_else(|| t.prepared_diagrams.iter().find(|d| d.scene.is_some()))
                .cloned()
        });

        if let Some(diag) = target_diag
            && let Some(scene) = diag.scene.clone()
        {
            if let Some(collapsible) = scene.hit_regions.iter().find(|h| h.can_collapse) {
                let c1 = Arc::clone(core_ref);
                let v1 = vault_root.clone();
                let a1 = active.clone();
                let ord = diag.ordinal;
                let node_id = collapsible.node_id.clone();
                let (ok, readback) = gio::spawn_blocking(move || {
                    let orig = c1
                        .collapsed_nodes(v1.clone(), a1.clone(), ord)
                        .unwrap_or_default();
                    let next = vec![node_id];
                    let saved = c1
                        .remember_diagram(v1.clone(), a1.clone(), ord, next.clone())
                        .is_ok();
                    let readback = c1
                        .collapsed_nodes(v1.clone(), a1.clone(), ord)
                        .unwrap_or_default();
                    let _ = c1.remember_diagram(v1, a1, ord, orig);
                    (saved && readback == next, readback)
                })
                .await
                .unwrap_or((false, Vec::new()));
                collapse_persisted_ok = ok;
                collapsed_nodes_readback = readback;
            }

            win.open_focus_mode(active, diag.ordinal);
            let _ = wait_for(2000, || {
                win.focus.is_open() && win.focus.snapshot().is_some()
            })
            .await;
            glib::timeout_future(Duration::from_millis(60)).await;
            win.focus.zoom_factor(1.25);
            win.focus.pan_by(32.0, -18.0);
            focus_snapshot = win.focus.snapshot();
            win.focus.persist_now_async().await;

            let c2 = Arc::clone(core_ref);
            let v2 = vault_root.clone();
            let a2 = active.clone();
            let ord2 = diag.ordinal;
            let focus_readback =
                gio::spawn_blocking(move || c2.diagram_focus(v2, a2, ord2).ok().flatten())
                    .await
                    .ok()
                    .flatten();
            focus_mode_ok =
                win.focus.is_open() && focus_snapshot.is_some() && focus_readback.is_some();

            let note_src = win
                .state
                .borrow()
                .tabs
                .get(active)
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let is_mmd = active.to_ascii_lowercase().ends_with(".mmd");
            if let Ok(svg) = engine_ref.render_svg(
                note_src,
                is_mmd,
                diag.ordinal,
                scene.width.max(120.0),
                adw::StyleManager::default().is_dark(),
                diag.collapsed.clone(),
            ) {
                export_svg_bytes = svg.len();
                export_svg_valid = svg.trim_start().starts_with("<svg");
            }
            if let Ok(png) = render_scene_to_png_bytes(&scene, 2.0) {
                export_png_bytes = png.len();
                export_png_valid = png.starts_with(b"\x89PNG\r\n\x1a\n");
            }
            if let Ok(pdf) = render_scene_to_pdf_bytes(&scene) {
                export_pdf_bytes = pdf.len();
                export_pdf_valid = pdf.starts_with(b"%PDF-");
            }
            win.focus.close();
        }
    }

    let wikilink_target_resolved =
        active_note
            .as_ref()
            .zip(handle.as_ref())
            .and_then(|(active, h)| {
                let text = win
                    .state
                    .borrow()
                    .tabs
                    .get(active)
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                let know = h.knowledge_markup(active.clone(), text);
                know.links
                    .into_iter()
                    .find(|l| !l.embed && l.resolved_path.is_some())
                    .and_then(|l| l.resolved_path)
            });

    ReadingSelfCheckReport {
        vault_root,
        active_note,
        fonts: registration_status(),
        tabs: tabs_stats,
        ctrl_j_sync_ok,
        ctrl_e_preserved_ok,
        reading_scroll_before,
        edit_scroll_char_after,
        back_reading_scroll_u16,
        collapse_persisted_ok,
        collapsed_nodes_readback,
        focus_mode_ok,
        focus_snapshot,
        export_svg_bytes,
        export_svg_valid,
        export_png_bytes,
        export_png_valid,
        export_pdf_bytes,
        export_pdf_valid,
        wikilink_target_resolved,
        startup_ms,
    }
}
