// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 宽度适配、X11 `ConfigureWindow` 实时拖拽握手，以及阅读双链点击回归。

use crate::automation::driver::wait_for;
use crate::window::shell::WindowInner;
use gtk4::glib;
use gtk4::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

fn write_json_atomic(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    let text =
        serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
    if std::fs::write(&tmp, text + "\n").is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

pub fn execute_layout_width_checks(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_layout_width_checks(&this).await;
        let json = serde_json::to_string_pretty(&report)
            .unwrap_or_else(|err| format!("{{\"error\":\"{err}\"}}"));
        let _ = std::fs::write(&report_path, json + "\n");
        this.close_now_clean();
    });
}

async fn run_layout_width_checks(win: &Rc<WindowInner>) -> serde_json::Value {
    let note = win.active_tab_path().unwrap_or_default();
    let ready = wait_for(20000, || {
        let st = win.state.borrow();
        st.scan_complete
            && st
                .tabs
                .get(&note)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    let allocated = wait_for(5000, || {
        win.state
            .borrow()
            .tabs
            .get(&note)
            .is_some_and(|tab| tab.reading_viewport.last_allocated_width() > 240)
    })
    .await;
    glib::timeout_future(Duration::from_millis(240)).await;
    let before = win.layout_snapshot(&note);
    let before_layout = before["layout_width"].as_f64().unwrap_or(0.0);
    let before_scene = before["diagram_scene_widths"]
        .as_array()
        .and_then(|scenes| scenes.first())
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);

    let mut sidebars_hidden = false;
    let mut reflowed = true;
    if win.options.layout_hide_sidebars {
        win.left_split.set_show_sidebar(false);
        win.right_split.set_show_sidebar(false);
        sidebars_hidden = true;
        reflowed = wait_for(12000, || {
            let st = win.state.borrow();
            let Some(tab) = st.tabs.get(&note) else {
                return false;
            };
            let target = f64::from(tab.reading_viewport.last_allocated_width()).max(260.0);
            let scene_grew = tab
                .prepared_diagrams
                .iter()
                .filter_map(|diagram| diagram.scene.as_ref())
                .any(|scene| scene.width > before_scene + 200.0);
            let pending = tab
                .reading
                .as_ref()
                .is_some_and(crate::reading::ReadingSurfaceHandle::has_pending_diagram_relayout);
            tab.layout_width > before_layout + 200.0
                && scene_grew
                && !pending
                && (tab.layout_width - target).abs() < 24.0
        })
        .await;
        glib::timeout_future(Duration::from_millis(220)).await;
    }
    let after = win.layout_snapshot(&note);
    if let Some(ref screenshot_path) = win.options.screenshot
        && let Err(err) = win.capture_window_png(screenshot_path)
    {
        eprintln!("screenshot failed: {err}");
    }
    serde_json::json!({
        "note": note,
        "ready": ready,
        "allocated": allocated,
        "sidebars_hidden": sidebars_hidden,
        "reflowed": reflowed,
        "first_text_scrolled_x": after["first_text_scrolled_x"],
        "left_margin_ok": after["first_text_scrolled_x"].as_f64().is_some_and(|x| x >= 20.0),
        "before": before,
        "after": after,
    })
}

pub fn execute_live_resize_checks(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_live_resize_checks(&this, &report_path).await;
        write_json_atomic(&report_path, &report);
        this.close_now_clean();
    });
}

async fn run_live_resize_checks(win: &Rc<WindowInner>, report_path: &Path) -> serde_json::Value {
    let note = win.active_tab_path().unwrap_or_default();
    let stage_path = report_path.with_extension("stage");
    let cmd_path = report_path.with_extension("cmd");

    let ready = wait_for(20000, || {
        let st = win.state.borrow();
        st.scan_complete
            && st
                .tabs
                .get(&note)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    let allocated = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(&note)
            .is_some_and(|tab| tab.reading_viewport.settled_width() > 240)
    })
    .await;
    glib::timeout_future(Duration::from_millis(240)).await;
    let init_snap = win.layout_snapshot(&note);
    write_json_atomic(
        &stage_path,
        &serde_json::json!({
            "seq": 0,
            "stage": "init",
            "ready": ready,
            "allocated": allocated,
            "snapshot": init_snap,
        }),
    );

    let mut steps = Vec::new();
    for expected_seq in 1..=3u64 {
        let got_cmd = wait_for(20000, || {
            let Ok(raw) = std::fs::read_to_string(&cmd_path) else {
                return false;
            };
            serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v["seq"].as_u64())
                .is_some_and(|s| s >= expected_seq)
        })
        .await;
        if !got_cmd {
            steps.push(serde_json::json!({
                "seq": expected_seq,
                "timeout": true,
            }));
            break;
        }
        let cmd: serde_json::Value = std::fs::read_to_string(&cmd_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(serde_json::Value::Null);
        let current_seq = expected_seq;
        let target_w = cmd["target_width"].as_i64().unwrap_or(0) as i32;
        let prev_reflows = win
            .state
            .borrow()
            .tabs
            .get(&note)
            .map(|t| t.reading_viewport.reflow_count())
            .unwrap_or(0);

        let resized = wait_for(12000, || {
            let w_ok = (win.window.width() - target_w).abs() <= 12;
            let st = win.state.borrow();
            let Some(tab) = st.tabs.get(&note) else {
                return false;
            };
            let vp_w = f64::from(tab.reading_viewport.last_allocated_width()).max(260.0);
            let pending = tab
                .reading
                .as_ref()
                .is_some_and(crate::reading::ReadingSurfaceHandle::has_pending_diagram_relayout);
            let reflow_done = tab.reading_viewport.reflow_count() > prev_reflows
                && !pending
                && (tab.layout_width - vp_w).abs() < 24.0;
            w_ok && reflow_done
        })
        .await;
        glib::timeout_future(Duration::from_millis(260)).await;
        if let Some(shot) = cmd["screenshot"].as_str()
            && !shot.is_empty()
            && let Err(err) = win.capture_window_png(Path::new(shot))
        {
            eprintln!("live resize screenshot failed ({shot}): {err}");
        }
        let snap = win.layout_snapshot(&note);
        let stage_obj = serde_json::json!({
            "seq": current_seq,
            "target_width": target_w,
            "resized_and_reflowed": resized,
            "left_margin_ok": snap["first_text_scrolled_x"].as_f64().is_some_and(|x| x >= 20.0),
            "snapshot": snap,
        });
        write_json_atomic(&stage_path, &stage_obj);
        steps.push(stage_obj);
    }

    serde_json::json!({
        "note": note,
        "ready": ready,
        "allocated": allocated,
        "init": init_snap,
        "steps": steps,
    })
}

pub fn execute_wikilink_checks(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_wikilink_checks(&this).await;
        let json = serde_json::to_string_pretty(&report)
            .unwrap_or_else(|err| format!("{{\"error\":\"{err}\"}}"));
        let _ = std::fs::write(&report_path, json + "\n");
        this.close_now_clean();
    });
}

async fn run_wikilink_checks(win: &Rc<WindowInner>) -> serde_json::Value {
    let start = "Start here.md";
    let map = "Learning/Learning map.md";
    let loop_note = "Learning/Study loop.md";
    let boundary = "L2-双链边界.md";
    let mut report = serde_json::Map::new();
    let mut stages: Vec<String> = Vec::new();
    stages.push("wait-initial".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));

    let initial_ready = wait_for(20000, || {
        let st = win.state.borrow();
        st.scan_complete
            && st
                .tabs
                .get(start)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    report.insert("initial_ready".into(), serde_json::json!(initial_ready));
    report.insert("initial_tabs".into(), serde_json::json!(win.tab_paths()));

    stages.push("click-learning-map".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    let click_map = win.click_reading_wikilink(start, "Learning map");
    report.insert(
        "learning_map_click".into(),
        serde_json::json!({
            "clicked": click_map.is_some(),
            "target": click_map.as_ref().map(|(target, _)| target.clone()),
            "heading": click_map.as_ref().and_then(|(_, heading)| heading.clone()),
        }),
    );
    let map_loaded = wait_for(12000, || {
        win.state
            .borrow()
            .tabs
            .get(map)
            .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    let map_active = win.active_tab_path();
    let map_nodes = win
        .state
        .borrow()
        .tabs
        .get(map)
        .and_then(|tab| {
            tab.prepared_diagrams
                .iter()
                .filter_map(|diagram| diagram.scene.as_ref())
                .map(|scene| scene.hit_regions.len())
                .max()
        })
        .unwrap_or(0);
    report.insert(
        "learning_map_open".into(),
        serde_json::json!({
            "loaded": map_loaded,
            "active_path": map_active,
            "active_is_target": map_active.as_deref() == Some(map),
            "tab_count": win.state.borrow().tabs.len(),
            "diagram_nodes": map_nodes,
            "diagram_ok": map_nodes > 0,
        }),
    );
    let widths_ready = wait_for(8000, || {
        win.layout_snapshot(map)["diagram_widget_widths"]
            .as_array()
            .and_then(|widths| widths.first())
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            >= 520
            && win.reading_surface(map).is_some_and(|surface| {
                !surface.has_pending_diagram_relayout()
                    && surface
                        .diagram_widgets()
                        .first()
                        .is_some_and(|w| w.height() >= 180)
            })
    })
    .await;
    glib::timeout_future(Duration::from_millis(120)).await;
    report.insert(
        "learning_map_widths_ready".into(),
        serde_json::json!(widths_ready),
    );
    report.insert("learning_map_widths".into(), win.layout_snapshot(map));
    if let Some(ref screenshot_path) = win.options.screenshot {
        win.window.queue_draw();
        glib::timeout_future(Duration::from_millis(500)).await;
        if let Err(err) = win.capture_window_png(screenshot_path) {
            eprintln!("screenshot failed: {err}");
        }
    }

    let _ = wait_for(4000, || {
        win.reading_surface(map)
            .and_then(|surface| surface.text_views().into_iter().next().map(|(v, _)| v))
            .is_some_and(|view| view.is_mapped() && view.width() > 0)
    })
    .await;
    glib::timeout_future(Duration::from_millis(180)).await;
    stages.push("click-study-loop".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    let click_study = win.click_reading_wikilink(map, "Study loop");
    let study_loaded = wait_for(12000, || {
        win.state
            .borrow()
            .tabs
            .get(loop_note)
            .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    let study_active = win.active_tab_path();
    report.insert(
        "study_loop_click".into(),
        serde_json::json!({
            "clicked": click_study.is_some(),
            "target": click_study.as_ref().map(|(target, _)| target.clone()),
            "loaded": study_loaded,
            "active_path": study_active,
            "active_is_target": study_active.as_deref() == Some(loop_note),
            "tab_count": win.state.borrow().tabs.len(),
        }),
    );

    stages.push("open-boundary".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    win.open_note_at(boundary, None);
    let boundary_loaded = wait_for(12000, || {
        win.state
            .borrow()
            .tabs
            .get(boundary)
            .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    report.insert("boundary_loaded".into(), serde_json::json!(boundary_loaded));
    let _ = wait_for(4000, || {
        win.reading_surface(boundary)
            .and_then(|surface| surface.text_views().into_iter().next().map(|(v, _)| v))
            .is_some_and(|view| view.is_mapped() && view.width() > 0)
    })
    .await;
    glib::timeout_future(Duration::from_millis(180)).await;

    stages.push("click-missing-target".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    let tabs_before_missing = win.state.borrow().tabs.len();
    let click_missing = win.click_reading_wikilink(boundary, "L2 不存在的目标");
    let expected_missing_prefix = crate::i18n::fmt1(crate::i18n::Key::ToastMissingLink, "L2");
    let missing_prefix_head = expected_missing_prefix.split("L2").next().unwrap_or("");
    let missing_toast = wait_for(4000, || {
        win.state
            .borrow()
            .last_toast
            .as_deref()
            .is_some_and(|toast| toast.contains(missing_prefix_head))
    })
    .await;
    report.insert(
        "missing_target_click".into(),
        serde_json::json!({
            "clicked": click_missing.is_some(),
            "target": click_missing.as_ref().map(|(target, _)| target.clone()),
            "toast_shown": missing_toast,
            "tab_count_unchanged": win.state.borrow().tabs.len() == tabs_before_missing,
            "active_path": win.active_tab_path(),
        }),
    );

    stages.push("click-ambiguous-target".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    let click_ambiguous = win.click_reading_wikilink(boundary, "同名");
    let expected_ambig = crate::i18n::fmt2(crate::i18n::Key::ToastAmbiguousLink, "同名", 2);
    let ambig_prefix = expected_ambig.split("同名").next().unwrap_or("");
    let ambiguous_toast = wait_for(4000, || {
        win.state
            .borrow()
            .last_toast
            .as_deref()
            .is_some_and(|toast| toast.contains(ambig_prefix))
    })
    .await;
    report.insert(
        "ambiguous_target_click".into(),
        serde_json::json!({
            "clicked": click_ambiguous.is_some(),
            "target": click_ambiguous.as_ref().map(|(target, _)| target.clone()),
            "toast_shown": ambiguous_toast,
            "active_path": win.active_tab_path(),
        }),
    );

    stages.push("click-heading-link".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    let click_heading = win.click_reading_wikilink(boundary, "带标题的链接");
    let heading_active = wait_for(4000, || win.active_tab_path().as_deref() == Some(map)).await;
    report.insert(
        "heading_link_click".into(),
        serde_json::json!({
            "clicked": click_heading.is_some(),
            "target": click_heading.as_ref().map(|(target, _)| target.clone()),
            "heading": click_heading.as_ref().and_then(|(_, heading)| heading.clone()),
            "active_is_map": heading_active,
            "active_path": win.active_tab_path(),
        }),
    );

    stages.push("bare-target-resolve".into());
    report.insert("stages".into(), serde_json::json!(stages.clone()));
    win.open_note_or_resolve(boundary, "Study loop", None);
    let bare_active = wait_for(4000, || win.active_tab_path().as_deref() == Some(loop_note)).await;
    report.insert(
        "bare_target_resolve".into(),
        serde_json::json!({
            "active_is_study_loop": bare_active,
            "active_path": win.active_tab_path(),
            "open_tabs": win.tab_paths(),
        }),
    );

    stages.push("done".into());
    report.insert("stages".into(), serde_json::json!(stages));
    report.insert(
        "last_toast".into(),
        serde_json::json!(win.state.borrow().last_toast),
    );
    report.insert("completed".into(), serde_json::json!(true));
    serde_json::Value::Object(report)
}
