// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

pub mod driver;
pub mod l0_smoke;
pub mod l1_reading;
pub mod l2_edit_knowledge;
pub mod l2_knowledge_files;
pub mod layout_width;
pub mod scenes;

use crate::window::shell::WindowInner;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

pub fn maybe_schedule_automation(win: &Rc<WindowInner>) {
    let ready = {
        let st = win.state.borrow();
        if st.automation_triggered || st.disposing {
            return;
        }
        if st.vault_ctx.is_some() && !st.scan_complete {
            return;
        }
        if !st.pending_open_notes.is_empty() {
            return;
        }
        if !st
            .tabs
            .values()
            .all(|t| t.source_loaded && t.reading_loaded)
        {
            return;
        }
        true
    };
    if !ready {
        return;
    }

    let opts = &win.options;
    if opts.smoke_json.is_none()
        && opts.reading_json.is_none()
        && opts.edit_knowledge_json.is_none()
        && opts.edit_knowledge_scene.is_none()
        && opts.wikilink_json.is_none()
        && opts.layout_width_json.is_none()
        && opts.live_resize_json.is_none()
        && opts.screenshot.is_none()
        && opts.focus_diagram.is_none()
        && !opts.edit_mode
        && opts.scroll_fraction.is_none()
    {
        if let Some(ms) = opts.quit_after_ms {
            let weak = Rc::downgrade(win);
            let src = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
                if let Some(w) = weak.upgrade() {
                    w.automation_source_done();
                    w.close_now_clean();
                }
            });
            win.state.borrow_mut().automation_source = Some(src);
        }
        return;
    }

    win.state.borrow_mut().automation_triggered = true;

    if opts.edit_mode
        && let Some(active) = win.active_tab_path()
    {
        let is_editing = win
            .state
            .borrow()
            .tabs
            .get(&active)
            .is_some_and(|t| t.editing);
        if !is_editing {
            win.toggle_active_tab_edit_mode();
        }
    }

    if let Some(ord) = opts.focus_diagram
        && let Some(active) = win.active_tab_path()
    {
        win.open_focus_mode(&active, ord);
    }

    if opts.overview {
        let total = win.tab_view.n_pages();
        let weak = Rc::downgrade(win);
        let index = Rc::new(Cell::new(0i32));
        let ov_src = glib::timeout_add_local(Duration::from_millis(90), move || {
            let Some(w) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if w.state.borrow().disposing {
                return glib::ControlFlow::Break;
            }
            let current = index.get();
            if current < total {
                let page = w.tab_view.nth_page(current);
                w.tab_view.set_selected_page(&page);
                page.child().queue_draw();
                index.set(current + 1);
                glib::ControlFlow::Continue
            } else {
                w.state.borrow_mut().overview_source = None;
                if total > 0 {
                    let first = w.tab_view.nth_page(0);
                    w.tab_view.set_selected_page(&first);
                }
                w.tab_overview.set_open(true);
                let weak_run = Rc::downgrade(&w);
                let src = glib::timeout_add_local_once(Duration::from_millis(450), move || {
                    if let Some(w2) = weak_run.upgrade() {
                        w2.automation_source_done();
                        run_automation(&w2);
                    }
                });
                w.state.borrow_mut().automation_source = Some(src);
                glib::ControlFlow::Break
            }
        });
        win.state.borrow_mut().overview_source = Some(ov_src);
        return;
    }

    let weak = Rc::downgrade(win);
    let src = glib::timeout_add_local_once(Duration::from_millis(500), move || {
        let Some(w) = weak.upgrade() else { return };
        w.automation_source_done();
        if let Some(frac) = w.options.scroll_fraction
            && let Some(active) = w.active_tab_path()
        {
            if let Some(reading) = w
                .state
                .borrow()
                .tabs
                .get(&active)
                .and_then(|t| t.reading.clone())
            {
                reading.scroll_fraction(frac);
            }
            let weak2 = Rc::downgrade(&w);
            let src2 = glib::timeout_add_local_once(Duration::from_millis(120), move || {
                if let Some(w2) = weak2.upgrade() {
                    w2.automation_source_done();
                    run_automation(&w2);
                }
            });
            w.state.borrow_mut().automation_source = Some(src2);
        } else {
            run_automation(&w);
        }
    });
    win.state.borrow_mut().automation_source = Some(src);
}

pub fn run_automation(win: &Rc<WindowInner>) {
    if let Some(ref json_path) = win.options.live_resize_json {
        layout_width::execute_live_resize_checks(win, json_path.clone());
        return;
    }
    if let Some(ref json_path) = win.options.layout_width_json {
        layout_width::execute_layout_width_checks(win, json_path.clone());
        return;
    }
    if let Some(ref json_path) = win.options.wikilink_json {
        layout_width::execute_wikilink_checks(win, json_path.clone());
        return;
    }
    if let Some(ref json_path) = win.options.edit_knowledge_json {
        l2_edit_knowledge::execute_edit_knowledge_checks(win, json_path.clone());
        return;
    }
    if let Some(scene) = win.options.edit_knowledge_scene.clone() {
        scenes::setup_l2_scene(win, &scene);
        let weak = Rc::downgrade(win);
        let started = std::time::Instant::now();
        let src = glib::timeout_add_local(Duration::from_millis(80), move || {
            let Some(w) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let elapsed = started.elapsed().as_millis();
            let layout_settled = if elapsed < 1100 {
                false
            } else if let Some(active) = w.active_tab_path() {
                w.state.borrow().tabs.get(&active).is_some_and(|tab| {
                    if tab.editing {
                        tab.source_loaded
                    } else {
                        tab.reading_loaded
                            && tab.reading.as_ref().is_some_and(|surface| {
                                surface.table_widgets().iter().all(|tw| tw.height() > 0)
                                    && surface.diagram_widgets().iter().all(|dw| dw.height() > 0)
                            })
                    }
                })
            } else {
                true
            };
            if !layout_settled && elapsed < 3500 {
                return glib::ControlFlow::Continue;
            }
            w.automation_source_done();
            let weak_cap = Rc::downgrade(&w);
            let src_cap = glib::timeout_add_local_once(Duration::from_millis(160), move || {
                let Some(w2) = weak_cap.upgrade() else { return };
                w2.automation_source_done();
                if let Some(ref screenshot_path) = w2.options.screenshot
                    && let Err(err) = scenes::capture_scene_pngs(&w2, screenshot_path)
                {
                    eprintln!("screenshot failed: {err}");
                }
                w2.close_now_clean();
            });
            w.state.borrow_mut().automation_source = Some(src_cap);
            glib::ControlFlow::Break
        });
        win.state.borrow_mut().automation_source = Some(src);
        return;
    }
    if let Some(ref screenshot_path) = win.options.screenshot
        && let Err(err) = win.capture_window_png(screenshot_path)
    {
        eprintln!("screenshot failed: {err}");
    }
    if let Some(ref reading_path) = win.options.reading_json {
        l1_reading::execute_reading_checks(win, reading_path.clone());
        return;
    }
    if let Some(ref smoke_path) = win.options.smoke_json {
        l0_smoke::execute_smoke_checks(win, smoke_path.clone());
        return;
    }
    if win.options.screenshot.is_some() {
        let delay = win.options.quit_after_ms.unwrap_or(80);
        let weak = Rc::downgrade(win);
        let src = glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if let Some(w) = weak.upgrade() {
                w.automation_source_done();
                w.close_now_clean();
            }
        });
        win.state.borrow_mut().automation_source = Some(src);
    }
}
