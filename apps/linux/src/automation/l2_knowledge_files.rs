// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! L2 全流程自检后半段（E 知识导航异步查询、F 文件管理与回收站、G L0 观察）。

use crate::automation::driver::{collect_temp_leftovers, wait_for};
use crate::core::{PanelRegion, StaticToolKind};
use crate::dock::DockTabId;
use crate::files::operations::count_dir_files;
use crate::render::media::markdown_cell_links;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
pub async fn run_knowledge_files_and_misc(
    win: &Rc<WindowInner>,
    note: &str,
    root: &str,
    real_trash: &Path,
    real_trash_before: usize,
    automation_started: Instant,
    debug: bool,
    report: &mut serde_json::Map<String, serde_json::Value>,
) {
    let xdg_data = std::env::var("XDG_DATA_HOME").unwrap_or_default();
    macro_rules! stage {
        ($name:expr) => {{
            win.state.borrow_mut().last_stage = $name.to_string();
            if debug {
                eprintln!(
                    "L2-STAGE {} at {}ms",
                    $name,
                    automation_started.elapsed().as_millis()
                );
            }
        }};
    }

    stage!("E-knowledge");
    let mut knowledge = serde_json::Map::new();
    win.activate_static_tool(StaticToolKind::Search);
    win.search_panel.run_query_now("Hortus");
    let _ = wait_for(6000, || {
        let stats = win.search_panel.stats();
        !stats.pending && stats.groups > 0
    })
    .await;
    let first_search = win.search_panel.stats();
    win.search_panel.run_query_now("Learning");
    win.search_panel.run_query_now("Hortus");
    let _ = wait_for(6000, || {
        let stats = win.search_panel.stats();
        !stats.pending && stats.last_query == "Hortus" && stats.groups > 0
    })
    .await;
    let final_search = win.search_panel.stats();
    let handle_search = win.state.borrow().vault_handle();
    let expected_groups = match handle_search {
        Some(handle) => gio::spawn_blocking(move || {
            handle
                .search("Hortus".into(), 6, hortus_ffi::SearchCancellation::new())
                .len()
        })
        .await
        .unwrap_or(0),
        None => 0,
    };
    let clicked_group = win.search_panel.click_group(0);
    let _ = wait_for(4000, || win.state.borrow().tabs.len() > 2).await;
    glib::timeout_future(Duration::from_millis(250)).await;
    knowledge.insert(
        "search".into(),
        serde_json::json!({
            "initial_groups": first_search.groups,
            "initial_matches": first_search.matches,
            "final_groups": final_search.groups,
            "final_query": final_search.last_query,
            "queries": final_search.queries,
            "cancellations": final_search.cancellations,
            "stale_dropped": final_search.stale_dropped,
            "expected_groups": expected_groups,
            "concurrent_result_matches": final_search.groups == expected_groups,
            "clicked_group": clicked_group,
        }),
    );

    win.activate_static_tool(StaticToolKind::Tags);
    win.tags_panel.refresh();
    let _ = wait_for(6000, || {
        !win.tags_panel.is_loading() && win.tags_panel.stats().total_tags > 0
    })
    .await;
    let tags_stats = win.tags_panel.stats();
    let tag_labels = win.tags_panel.tag_labels();
    let first_tag = tag_labels.first().cloned();
    let tag_clicked = win.tags_panel.click_tag(0);
    let _ = wait_for(6000, || {
        !win.tags_panel.is_loading() && win.tags_panel.stats().visible_notes > 0
    })
    .await;
    let tag_stats_after_click = win.tags_panel.stats();
    let tag_notes = tag_stats_after_click.visible_notes;
    let clicked_note = win.tags_panel.click_note(0);
    let _ = wait_for(3000, || win.state.borrow().tabs.len() > 2).await;
    let reading_tag_note = win.active_tab_path();
    if let Some(tag) = first_tag {
        win.open_tag_from_reading(&tag);
        let _ = wait_for(4000, || !win.tags_panel.is_loading()).await;
    }
    let tags_view_active = win
        .tool_dock
        .active_tab_in_region(PanelRegion::Left)
        .is_some_and(|id| id == DockTabId::Static(StaticToolKind::Tags));
    knowledge.insert(
        "tags".into(),
        serde_json::json!({
            "total_tags": tags_stats.total_tags,
            "sample_tags": tag_labels.iter().take(5).cloned().collect::<Vec<_>>(),
            "selected_notes": tag_notes,
            "tag_clicked": tag_clicked,
            "selected_after_click": tag_stats_after_click.selected,
            "filter_after_click": tag_stats_after_click.filter,
            "visible_tags": tag_stats_after_click.visible_tags,
            "clicked_note": clicked_note,
            "reading_click_switches_panel": tags_view_active,
            "filter_after_reading_click": win.tags_panel.filter_text(),
            "note_after_click": reading_tag_note,
        }),
    );

    for rel in ["Start here.md", "README.md"] {
        win.open_note_at(rel, None);
        let _ = wait_for(6000, || {
            win.state
                .borrow()
                .tabs
                .get(rel)
                .is_some_and(|tab| tab.reading_loaded)
        })
        .await;
    }
    win.open_note_at("Start here.md", None);
    let _ = wait_for(2000, || win.active_path_is("Start here.md")).await;
    win.refresh_backlinks(true);
    let _ = wait_for(4000, || {
        !win.backlinks_panel.is_loading()
            && win.backlinks_panel.current_path().as_deref() == Some("Start here.md")
    })
    .await;
    let backlinks_first = win.backlinks_panel.stats();
    let clicked_backlink = win.backlinks_panel.click_row(0);
    let _ = wait_for(4000, || {
        clicked_backlink
            .as_ref()
            .is_some_and(|(source, _)| win.active_path_is(source))
    })
    .await;
    win.open_note_at("README.md", None);
    let _ = wait_for(4000, || {
        !win.backlinks_panel.is_loading()
            && win.backlinks_panel.stats().path.as_deref() == Some("README.md")
    })
    .await;
    let backlinks_second = win.backlinks_panel.stats();
    knowledge.insert(
        "backlinks".into(),
        serde_json::json!({
            "first_path": "Start here.md",
            "first_rows": backlinks_first.rows,
            "second_path": win.backlinks_panel.current_path(),
            "second_rows": backlinks_second.rows,
            "updates": backlinks_second.updates,
            "clicked_row": clicked_backlink.as_ref().map(|(s, o)| serde_json::json!({"source": s, "offset": o})),
            "switched": backlinks_first.path != backlinks_second.path,
        }),
    );

    win.open_quick_open();
    win.quick_open.set_query("Start");
    let _ = wait_for(4000, || {
        let stats = win.quick_open.stats();
        stats.last_query == "Start" && stats.hits > 0
    })
    .await;
    let quick_stats = win.quick_open.stats();
    let quick_hits = win.quick_open.hits();
    win.quick_open.activate_selected();
    let quick_closed = wait_for(3000, || !win.quick_open.is_open()).await;
    let _ = wait_for(4000, || {
        quick_hits
            .first()
            .is_some_and(|path| win.active_path_is(path))
    })
    .await;
    knowledge.insert(
        "quick_open".into(),
        serde_json::json!({
            "queries": quick_stats.queries,
            "last_query": quick_stats.last_query,
            "open_before_activate": quick_stats.open,
            "hits": quick_stats.hits,
            "first": quick_stats.first,
            "closed_after_enter": quick_closed,
            "opened_first": quick_hits.first().is_some_and(|path| win.active_path_is(path)),
        }),
    );

    win.open_note_at(note, None);
    let _ = wait_for(6000, || win.active_path_is(note)).await;
    if !win
        .state
        .borrow()
        .tabs
        .get(note)
        .is_some_and(|tab| tab.editing)
    {
        win.toggle_active_tab_edit_mode();
    }
    let focus_baseline = {
        let (buffer, view) = {
            let st = win.state.borrow();
            match st.tabs.get(note) {
                Some(tab) => (Some(tab.edit_buffer.clone()), Some(tab.edit_view.clone())),
                None => (None, None),
            }
        };
        if let (Some(buffer), Some(view)) = (buffer, view) {
            let end = buffer.end_iter();
            buffer.place_cursor(&end);
            view.grab_focus();
        }
        glib::timeout_future(Duration::from_millis(120)).await;
        win.state
            .borrow()
            .tabs
            .get(note)
            .map(|tab| tab.edit_view.has_focus())
            .unwrap_or(false)
    };
    win.insert_into_edit(note, "\n\n[[Start");
    let completion = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.completion.clone());
    let mut completion_report = serde_json::json!({
        "visible": false,
        "items": 0,
        "typed_after_popup": false,
        "accepted": false,
        "target": null,
    });
    if let Some(completion) = completion {
        let visible = wait_for(4000, || {
            completion.is_visible() && completion.item_count() > 0
        })
        .await;
        let items_at_visible = completion.item_count();
        let view_focus = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|tab| tab.edit_view.has_focus())
            .unwrap_or(false);
        let window_active = win.window.is_active();
        win.insert_into_edit(note, "x");
        glib::timeout_future(Duration::from_millis(120)).await;
        let typed_kept = win
            .tab_facts(note)
            .is_some_and(|f| f.text.contains("[[Startx"));
        win.insert_into_edit(note, "\n\n[[Start");
        let re_visible = wait_for(4000, || {
            completion.is_visible() && completion.item_count() > 0
        })
        .await;
        let target = completion.first_insert_target();
        if re_visible {
            completion.accept_selected();
        }
        glib::timeout_future(Duration::from_millis(300)).await;
        let accepted_target = target.clone().unwrap_or_default();
        let accepted_text = !accepted_target.is_empty()
            && win
                .tab_facts(note)
                .is_some_and(|f| f.text.contains(&format!("[[{accepted_target}]]")));
        let accepted_in_session = !accepted_target.is_empty()
            && win
                .session_text(note)
                .await
                .unwrap_or_default()
                .contains(&format!("[[{accepted_target}]]"));
        completion_report = serde_json::json!({
            "visible": visible,
            "items": items_at_visible,
            "window_active": window_active,
            "focus_baseline": focus_baseline,
            "popover_focusable": completion.popover_focusable(),
            "typed_after_popup": typed_kept,
            "view_kept_focus": view_focus,
            "accepted": accepted_text && accepted_in_session && completion.stats().accepted > 0,
            "target": target,
            "stats": {
                "queries": completion.stats().queries,
                "accepted": completion.stats().accepted,
            },
        });
    }
    knowledge.insert("completion".into(), completion_report);
    report.insert("knowledge".into(), serde_json::Value::Object(knowledge));

    stage!("F-files");
    let mut files = serde_json::Map::new();
    let handle_files = win.state.borrow().vault_handle();
    if let Some(handle) = handle_files {
        let handle_bg = Arc::clone(&handle);
        let created =
            gio::spawn_blocking(move || handle_bg.create_note(String::new(), "L2-新笔记".into()))
                .await
                .ok()
                .and_then(|res| res.ok());
        let created_path = created.as_ref().map(|entry| entry.path.clone());
        if let Some(entry) = created {
            win.after_create_entry(entry);
        }
        let _ = wait_for(5000, || {
            win.state.borrow().tabs.contains_key("L2-新笔记.md")
        })
        .await;
        let created_folder = {
            let handle_bg = Arc::clone(&handle);
            gio::spawn_blocking(move || {
                handle_bg.create_folder(String::new(), "L2-新建文件夹".into())
            })
            .await
            .ok()
            .and_then(|res| res.ok())
        };
        let created_folder_path = created_folder.as_ref().map(|entry| entry.path.clone());
        if let Some(entry) = created_folder {
            win.after_create_entry(entry);
        }
        files.insert(
            "create".into(),
            serde_json::json!({
                "note": created_path,
                "folder": created_folder_path,
                "note_opened": win.state.borrow().tabs.contains_key("L2-新笔记.md"),
                "note_on_disk": Path::new(root).join("L2-新笔记.md").is_file(),
                "folder_on_disk": Path::new(root).join("L2-新建文件夹").is_dir(),
            }),
        );
    }

    win.state.borrow_mut().last_file_op = None;
    win.open_note_at("L2-被引用.md", None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get("L2-被引用.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    win.open_note_at("L2-引用者.md", None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get("L2-引用者.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    let _ = win.select_tab("L2-被引用.md");
    win.perform_rename("L2-被引用.md", "L2-被引用改名.md", true, false);
    let note_renamed = wait_for(9000, || {
        win.state.borrow().tabs.contains_key("L2-被引用改名.md")
    })
    .await;
    let _ = wait_for(2500, || win.state.borrow().last_file_op.is_some()).await;
    let rename_op = win.state.borrow().last_file_op.clone();
    let referrer_text = win.read_disk_note("L2-引用者.md").unwrap_or_default();
    files.insert(
        "rename_note".into(),
        serde_json::json!({
            "tab_migrated": note_renamed,
            "referrer_updated": referrer_text.contains("[[L2-被引用改名"),
            "referrer_keeps_old": referrer_text.contains("[[L2-被引用]]"),
            "op": rename_op,
            "open_paths": win.state.borrow().tabs.keys().cloned().collect::<Vec<_>>(),
        }),
    );

    win.state.borrow_mut().last_file_op = None;
    win.open_note_at("L2-组/子笔记.md", None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get("L2-组/子笔记.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    win.perform_rename("L2-组", "L2-组新", true, true);
    let folder_renamed = wait_for(9000, || {
        win.state.borrow().tabs.contains_key("L2-组新/子笔记.md")
    })
    .await;
    let _ = wait_for(2500, || win.state.borrow().last_file_op.is_some()).await;
    let folder_op = win.state.borrow().last_file_op.clone();
    let group_referrer = win.read_disk_note("L2-引用组.md").unwrap_or_default();
    files.insert(
        "rename_folder".into(),
        serde_json::json!({
            "tab_migrated": folder_renamed,
            "embed_updated": group_referrer.contains("![[L2-组新/子笔记"),
            "op": folder_op,
            "open_paths": win.state.borrow().tabs.keys().cloned().collect::<Vec<_>>(),
        }),
    );

    win.state.borrow_mut().last_file_op = None;
    win.open_note_at("L2-待删除.md", None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get("L2-待删除.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    win.perform_trash("L2-待删除.md", false);
    let trash_done = wait_for(9000, || win.state.borrow().last_file_op.is_some()).await;
    let trash_op = win.state.borrow().last_file_op.clone();
    let _ = wait_for(3000, || {
        !win.state.borrow().tabs.contains_key("L2-待删除.md")
    })
    .await;
    let gone = !Path::new(root).join("L2-待删除.md").exists();
    let tab_closed = !win.state.borrow().tabs.contains_key("L2-待删除.md");
    let real_trash_after = count_dir_files(real_trash).len();
    files.insert(
        "trash".into(),
        serde_json::json!({
            "done": trash_done,
            "gone": gone,
            "tab_closed": tab_closed,
            "op": trash_op,
            "real_trash_before": real_trash_before,
            "real_trash_after": real_trash_after,
            "real_trash_untouched": real_trash_before == real_trash_after,
            "xdg_data": xdg_data,
        }),
    );
    report.insert("files".into(), serde_json::Value::Object(files));

    stage!("G-misc");
    let mut misc = serde_json::Map::new();
    let _ = wait_for(2500, || {
        win.tab_facts("L2-编辑目标.md").is_some_and(|f| !f.dirty)
    })
    .await;
    win.open_note_at("L2-编辑目标.md", None);
    let _ = wait_for(4000, || {
        win.state
            .borrow()
            .tabs
            .get("L2-编辑目标.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    misc.insert(
        "inline_fallback_baseline".into(),
        serde_json::json!(win.inline_fallback_baseline_ok("L2-编辑目标.md")),
    );
    let table_text = win
        .tab_facts("L2-编辑目标.md")
        .map(|f| f.text)
        .unwrap_or_default();
    let table_links = markdown_cell_links(&table_text);
    misc.insert(
        "table_external_links".into(),
        serde_json::json!({
            "detected": table_links.len(),
            "sample": table_links.first().map(|(label, url)| serde_json::json!({"label": label, "url": url})),
        }),
    );
    report.insert("misc".into(), serde_json::Value::Object(misc));
    report.insert(
        "real_trash_after".into(),
        serde_json::json!(count_dir_files(real_trash).len()),
    );
    report.insert(
        "temp_leftovers_final".into(),
        serde_json::json!(collect_temp_leftovers(Path::new(root))),
    );
    report.insert(
        "last_toast".into(),
        serde_json::json!(win.state.borrow().last_toast),
    );
}
