// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 自动化辅助驱动：统一状态查询、Widget 离屏截图与无强制强杀的安全退出（G-10）。

use crate::files::operations::count_dir_files;
use crate::reading::ReadingSurfaceHandle;
use crate::render::math_widgets::build_math_fallback_widget;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::DocumentIssue;
use libadwaita::prelude::*;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct TabFacts {
    pub dirty: bool,
    pub issue: Option<DocumentIssue>,
    pub text: String,
    pub revision: u64,
    pub saves: u32,
    pub restyles: u32,
    pub reloads: u32,
    pub style_micros: u64,
    pub load_style_micros: u64,
    pub refreshes: u32,
    pub deferred_refreshes: u32,
    pub poll_refreshes: u32,
    pub last_refresh_issue: Option<DocumentIssue>,
    pub editing: bool,
    pub loaded: bool,
    pub notice_visible: bool,
    pub title: String,
    pub issue_message: String,
}

pub async fn wait_for(timeout_ms: u64, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        glib::timeout_future(Duration::from_millis(25)).await;
    }
}

pub fn capture_widget_png<W: IsA<gtk4::Widget> + IsA<gtk4::Native>>(
    widget: &W,
    output_path: &Path,
) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let width = widget.width().max(1);
    let height = widget.height().max(1);
    let paintable = gtk4::WidgetPaintable::new(Some(widget));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or_else(|| "窗口渲染节点为空".to_string())?;
    let renderer = gtk4::prelude::NativeExt::renderer(widget)
        .ok_or_else(|| "窗口 GskRenderer 不可用".to_string())?;
    let bounds = gtk4::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&bounds));
    texture
        .save_to_png(output_path)
        .map_err(|e| format!("保存 PNG 失败：{e}"))?;
    Ok(())
}

pub fn collect_temp_leftovers(root: &Path) -> Vec<String> {
    let mut leftovers = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path.clone());
            }
            let lower = name.to_lowercase();
            if lower.contains("tmp")
                || lower.contains(".hortus")
                || lower.contains("temporaryitems")
                || lower.starts_with(".~")
            {
                leftovers.push(path.to_string_lossy().into_owned());
            }
        }
    }
    leftovers.sort();
    leftovers
}

impl WindowInner {
    pub fn automation_source_done(&self) {
        self.state.borrow_mut().automation_source = None;
    }

    /// G-10 修复：通过 `window.close()` + `app.quit()` 走完整清理链，不使用 `std::process::exit`。
    pub fn close_now_clean(self: &Rc<Self>) {
        let dialog = self.state.borrow().last_alert_dialog.upgrade();
        if let Some(dialog) = dialog {
            dialog.force_close();
        }
        self.state.borrow_mut().close_approved = true;
        self.window.close();
        self.teardown();
        if let Some(app) = self.window.application() {
            glib::idle_add_local_once(move || app.quit());
        }
    }

    pub fn capture_window_png(&self, output_path: &Path) -> Result<(), String> {
        capture_widget_png(&self.window, output_path)
    }

    pub fn tab_facts(&self, path: &str) -> Option<TabFacts> {
        let st = self.state.borrow();
        let tab = st.tabs.get(path)?;
        let edit = tab.edit_state.borrow();
        Some(TabFacts {
            dirty: edit.dirty,
            issue: edit.issue,
            text: edit.text.clone(),
            revision: edit.revision,
            saves: edit.saves,
            restyles: edit.restyles,
            reloads: edit.reading_reloads,
            style_micros: edit.last_style_micros,
            load_style_micros: edit.load_style_micros,
            refreshes: edit.refreshes,
            deferred_refreshes: edit.deferred_refreshes,
            poll_refreshes: edit.poll_refreshes,
            last_refresh_issue: edit.last_refresh_issue,
            editing: tab.editing,
            loaded: tab.source_loaded && tab.reading_loaded,
            notice_visible: tab.notice.reveals_child(),
            title: tab.page.title().to_string(),
            issue_message: edit.message.clone(),
        })
    }

    pub fn select_tab(&self, path: &str) -> bool {
        let page = self
            .state
            .borrow()
            .tabs
            .get(path)
            .map(|tab| tab.page.clone());
        if let Some(page) = page {
            self.tab_view.set_selected_page(&page);
            self.center_stack.set_visible_child_name("tabs");
            return true;
        }
        false
    }

    pub fn active_path_is(&self, path: &str) -> bool {
        self.active_tab_path().as_deref() == Some(path)
    }

    pub fn insert_into_edit(&self, path: &str, text: &str) {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            tab.edit_buffer.clone()
        };
        buffer.begin_user_action();
        {
            let mut end = buffer.end_iter();
            buffer.insert(&mut end, text);
        }
        buffer.end_user_action();
    }

    pub fn place_cursor_at_char(&self, path: &str, char_index: usize) -> bool {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return false;
            };
            tab.edit_buffer.clone()
        };
        let iter = buffer.iter_at_offset(char_index as i32);
        buffer.place_cursor(&iter);
        true
    }

    pub fn restore_or_clear_undo(&self, path: &str) {
        let buffer = {
            let st = self.state.borrow();
            let Some(tab) = st.tabs.get(path) else {
                return;
            };
            tab.edit_buffer.clone()
        };
        buffer.set_enable_undo(false);
        buffer.set_enable_undo(true);
    }

    pub async fn session_text(&self, path: &str) -> Option<String> {
        let session = {
            let st = self.state.borrow();
            st.tabs.get(path).and_then(|tab| tab.session.clone())
        }?;
        gio::spawn_blocking(move || session.snapshot().ok().map(|s| s.text))
            .await
            .ok()
            .flatten()
    }

    pub fn read_disk_note(&self, path: &str) -> Option<String> {
        let root = self.state.borrow().vault_root()?;
        std::fs::read_to_string(Path::new(&root).join(path)).ok()
    }

    pub fn writing_ext(&self, path: &str, content: &str) -> bool {
        let Some(root) = self.state.borrow().vault_root() else {
            return false;
        };
        std::fs::write(Path::new(&root).join(path), content).is_ok()
    }

    pub fn checkpoint_count(&self) -> usize {
        let cache = std::env::var("HORTUS_CACHE_DIR").unwrap_or_default();
        if cache.is_empty() {
            return 0;
        }
        count_dir_files(&Path::new(&cache).join("recovery")).len()
    }

    pub fn inline_fallback_baseline_ok(&self, _path: &str) -> bool {
        let widget =
            build_math_fallback_widget("\\unknowninlinecmd{x}", "公式语法错误", false, 0.0);
        widget.has_css_class("hortus-math-inline-fallback")
            && matches!(
                widget.valign(),
                gtk4::Align::Baseline | gtk4::Align::BaselineFill
            )
    }

    pub fn reading_surface(&self, note: &str) -> Option<ReadingSurfaceHandle> {
        self.state
            .borrow()
            .tabs
            .get(note)
            .and_then(|tab| tab.reading.clone())
    }

    pub fn click_reading_wikilink(
        &self,
        note: &str,
        label: &str,
    ) -> Option<(String, Option<String>)> {
        self.reading_surface(note)?.click_wikilink(label)
    }

    pub fn tab_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.state.borrow().tabs.keys().cloned().collect();
        paths.sort();
        paths
    }

    pub fn layout_snapshot(&self, note: &str) -> serde_json::Value {
        let window_width = self.window.width();
        let left_show = self.left_split.shows_sidebar();
        let left_collapsed = self.left_split.is_collapsed();
        let right_show = self.right_split.shows_sidebar();
        let right_collapsed = self.right_split.is_collapsed();
        let st = self.state.borrow();
        let Some(tab) = st.tabs.get(note) else {
            return serde_json::json!({
                "found": false,
                "window_width": window_width,
                "left_show": left_show,
                "left_collapsed": left_collapsed,
                "right_show": right_show,
                "right_collapsed": right_collapsed,
            });
        };
        let reading = tab.reading.as_ref();
        let scrolled_width = reading.map(|r| r.scrolled.width()).unwrap_or(0);
        let reading_box_width = reading.map(|r| r.content_box.width()).unwrap_or(0);
        let text_layouts = reading
            .map(ReadingSurfaceHandle::text_segment_layouts)
            .unwrap_or_default();
        let text_scrolled_offsets = reading
            .map(ReadingSurfaceHandle::text_segment_scrolled_offsets)
            .unwrap_or_default();
        let first_text_scrolled_x = text_scrolled_offsets.first().copied().unwrap_or(0.0);
        let text_widths: Vec<i32> = text_layouts.iter().map(|(w, _)| *w).collect();
        let text_offsets: Vec<f64> = text_layouts.iter().map(|(_, x)| *x).collect();
        let text_width = text_widths.first().copied().unwrap_or(0);
        let text_centered = !text_layouts.is_empty()
            && text_layouts.iter().all(|&(w, x)| {
                if reading_box_width <= w + 4 {
                    true
                } else {
                    let expected_x = f64::from(reading_box_width - w) / 2.0;
                    (x - expected_x).abs() <= 2.5
                }
            });
        let h_pad = crate::platform::viewport::responsive_horizontal_padding(scrolled_width);
        let max_content_width = tab.reading_viewport.max_content_width();
        let (effective_col_w, _) =
            crate::platform::effective_reading_column_width(scrolled_width, max_content_width);
        let content_width_measured = if scrolled_width > 0 {
            f64::from(effective_col_w)
        } else {
            0.0
        };
        let widths = |widgets: Vec<gtk4::Widget>| -> Vec<i32> {
            widgets.into_iter().map(|widget| widget.width()).collect()
        };
        let box_bounds = |widgets: Vec<gtk4::Widget>| -> Vec<serde_json::Value> {
            widgets
                .into_iter()
                .filter_map(|w| {
                    w.compute_bounds(&tab.reading_scrolled).map(|b| {
                        let x = f64::from(b.x());
                        let width = f64::from(b.width());
                        serde_json::json!({
                            "x": x,
                            "width": width,
                            "right": x + width,
                            "height": f64::from(b.height()),
                        })
                    })
                })
                .collect()
        };
        let edit_left_margin = tab.edit_view.left_margin();
        let edit_right_margin = tab.edit_view.right_margin();
        let edit_top_margin = tab.edit_view.top_margin();
        let edit_allocated_width = tab.edit_view.width();
        let edit_host_w = if edit_allocated_width > 0 {
            edit_allocated_width
        } else {
            tab.reading_viewport.width()
        };
        let edit_column_width = (edit_host_w - edit_left_margin - edit_right_margin).max(0);
        let mut snap = serde_json::json!({
            "found": true,
            "note": note,
            "window_width": window_width,
            "left_show": left_show,
            "left_collapsed": left_collapsed,
            "right_show": right_show,
            "right_collapsed": right_collapsed,
            "layout_width": tab.layout_width,
            "scrolled_width": scrolled_width,
            "reading_box_width": reading_box_width,
            "max_content_width": max_content_width,
            "text_view_width": text_width,
            "text_view_widths": text_widths,
            "text_view_offsets": text_offsets,
            "text_view_scrolled_offsets": text_scrolled_offsets,
            "first_text_scrolled_x": first_text_scrolled_x,
            "viewport_horizontal_padding": h_pad,
            "text_view_centered": text_centered,
            "content_width_measured": content_width_measured,
            "viewport_last_alloc": tab.reading_viewport.last_allocated_width(),
            "viewport_settled": tab.reading_viewport.settled_width(),
            "resize_alloc_count": tab.reading_viewport.resize_alloc_count(),
            "reflow_count": tab.reading_viewport.reflow_count(),
            "surface_rebuilds": tab.surface_rebuilds,
            "in_place_relayouts": tab.in_place_relayouts,
        });
        if let Some(obj) = snap.as_object_mut() {
            obj.insert(
                "diagram_scene_widths".into(),
                serde_json::json!(
                    tab.prepared_diagrams
                        .iter()
                        .filter_map(|diagram| diagram.scene.as_ref().map(|scene| scene.width))
                        .collect::<Vec<f64>>()
                ),
            );
            obj.insert(
                "diagram_scene_heights".into(),
                serde_json::json!(
                    tab.prepared_diagrams
                        .iter()
                        .filter_map(|diagram| diagram.scene.as_ref().map(|scene| scene.height))
                        .collect::<Vec<f64>>()
                ),
            );
            obj.insert(
                "diagram_horizontal_overflows".into(),
                serde_json::json!(
                    reading
                        .map(ReadingSurfaceHandle::diagram_horizontal_overflows)
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "diagram_widget_widths".into(),
                serde_json::json!(
                    reading
                        .map(|r| widths(r.diagram_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "table_widget_widths".into(),
                serde_json::json!(
                    reading
                        .map(|r| widths(r.table_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "image_widget_widths".into(),
                serde_json::json!(
                    reading
                        .map(|r| widths(r.image_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "embed_widget_widths".into(),
                serde_json::json!(
                    reading
                        .map(|r| widths(r.embed_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "text_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(
                            r.text_views()
                                .into_iter()
                                .map(|(v, _)| v.upcast())
                                .collect()
                        ))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "diagram_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.diagram_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "table_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.table_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "embed_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.embed_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "hr_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.hr_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "math_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.math_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "image_box_bounds".into(),
                serde_json::json!(
                    reading
                        .map(|r| box_bounds(r.image_widgets()))
                        .unwrap_or_default()
                ),
            );
            obj.insert(
                "edit_left_margin".into(),
                serde_json::json!(edit_left_margin),
            );
            obj.insert(
                "edit_right_margin".into(),
                serde_json::json!(edit_right_margin),
            );
            obj.insert("edit_top_margin".into(), serde_json::json!(edit_top_margin));
            obj.insert(
                "edit_allocated_width".into(),
                serde_json::json!(edit_allocated_width),
            );
            obj.insert(
                "edit_column_width".into(),
                serde_json::json!(edit_column_width),
            );
        }
        snap
    }
}
