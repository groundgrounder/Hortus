// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::automation::driver::capture_widget_png;
use crate::files;
use crate::window::shell::WindowInner;
use gtk4::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub fn setup_l2_scene(win: &Rc<WindowInner>, scene: &str) {
    match scene {
        "search-backlinks" => {
            win.left_split.set_show_sidebar(true);
            win.right_split.set_show_sidebar(true);
            win.switch_left_panel("search");
            win.search_panel.set_query("Hortus");
            win.right_stack.set_visible_child_name("backlinks");
            win.rebuild_panel_switchers();
            win.refresh_backlinks(true);
            win.focus_search_panel();
        }
        "tags" => {
            win.left_split.set_show_sidebar(true);
            win.switch_left_panel("tags");
            let first_tag = win
                .state
                .borrow()
                .vault_handle()
                .and_then(|handle| handle.tags().into_iter().next());
            if let Some(tag) = first_tag {
                win.tags_panel.select_tag(&tag.tag);
            }
        }
        "quick-open" => {
            win.open_quick_open();
            win.quick_open.set_query("Start");
        }
        "edit-find-completion" => {
            let editing = win
                .state
                .borrow()
                .tabs
                .get(&win.active_tab_path().unwrap_or_default())
                .is_some_and(|tab| tab.editing);
            if !editing {
                win.switch_edit_mode(false);
            }
            win.find_open(false);
            let Some(path) = win.active_tab_path() else {
                return;
            };
            let (buffer, view, find) = {
                let st = win.state.borrow();
                let Some(tab) = st.tabs.get(&path) else {
                    return;
                };
                (
                    tab.edit_buffer.clone(),
                    tab.edit_view.clone(),
                    tab.find.clone(),
                )
            };
            find.set_query("Hortus");
            {
                let end = buffer.end_iter();
                buffer.place_cursor(&end);
                view.grab_focus();
            }
            buffer.begin_user_action();
            {
                let mut end = buffer.end_iter();
                buffer.insert(&mut end, "\n\n[[Start");
            }
            buffer.end_user_action();
        }
        "rename-links" => {
            let Some(path) = win.active_tab_path() else {
                return;
            };
            let Some(handle) = win.state.borrow().vault_handle() else {
                return;
            };
            let weak = Rc::downgrade(win);
            let scene_path = path.clone();
            let stem = Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("note")
                .to_string();
            let ext = Path::new(&path)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("md")
                .to_string();
            files::dialogs::prompt_rename(
                &win.window,
                handle,
                path,
                false,
                Some(format!("{stem}-改名预览.{ext}")),
                Rc::new(move |new_name, update_links| {
                    if let Some(w) = weak.upgrade() {
                        w.perform_rename(&scene_path, &new_name, update_links, false);
                    }
                }),
            );
        }
        _ => {}
    }
}

pub fn capture_scene_pngs(win: &WindowInner, output_path: &Path) -> Result<Vec<PathBuf>, String> {
    win.capture_window_png(output_path)?;
    let mut saved = vec![output_path.to_path_buf()];
    let own: gtk4::Window = win.window.clone().upcast();
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene");
    let mut index = 0usize;
    for widget in gtk4::Window::list_toplevels() {
        let Ok(window) = widget.downcast::<gtk4::Window>() else {
            continue;
        };
        if window == own || !window.is_visible() {
            continue;
        }
        index += 1;
        let path = output_path.with_file_name(format!("{stem}-dialog{index}.png"));
        if capture_widget_png(&window, &path).is_ok() {
            saved.push(path);
        }
    }
    Ok(saved)
}
