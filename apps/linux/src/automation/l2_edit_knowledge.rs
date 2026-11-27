// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! L2 全流程自检前半段（A 编辑与原子保存、B 外部冲突与关闭保护、C 查找替换、D 1MiB 大文档）。

use crate::automation::driver::{collect_temp_leftovers, wait_for};
use crate::automation::l2_knowledge_files::run_knowledge_files_and_misc;
use crate::core::Utf16Mapper;
use crate::files::operations::count_dir_files;
use crate::platform::style::apply_source_styles_with;
use crate::window::shell::WindowInner;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{DocumentIssue, SaveChoice};
use libadwaita::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub fn execute_edit_knowledge_checks(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_edit_knowledge_checks(&this).await;
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&report_path, format!("{json}\n")) {
                    eprintln!("write edit-knowledge json failed: {err}");
                }
            }
            Err(err) => eprintln!("serialize edit-knowledge json failed: {err}"),
        }
        this.close_now_clean();
    });
}

#[allow(clippy::too_many_lines)]
async fn run_edit_knowledge_checks(win: &Rc<WindowInner>) -> serde_json::Value {
    let debug = std::env::var_os("HORTUS_DEBUG_L2").is_some();
    win.window.present();
    let automation_started = Instant::now();
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
    let root = win.state.borrow().vault_root().unwrap_or_default();
    let cache_dir = std::env::var("HORTUS_CACHE_DIR").unwrap_or_default();
    let real_home = std::env::var("HOME").unwrap_or_default();
    let real_trash = Path::new(&real_home).join(".local/share/Trash/files");
    let real_trash_before = count_dir_files(&real_trash).len();

    let mut report = serde_json::Map::new();
    report.insert("vault_root".into(), serde_json::json!(root));
    report.insert("cache_dir".into(), serde_json::json!(cache_dir));
    report.insert(
        "temp_dir".into(),
        serde_json::json!(win.state.borrow().temp_dir.to_string_lossy()),
    );
    report.insert(
        "temp_dir_outside_vault".into(),
        serde_json::json!(!win.state.borrow().temp_dir.starts_with(Path::new(&root))),
    );
    report.insert(
        "startup_ms".into(),
        serde_json::json!(win.state.borrow().started_at.elapsed().as_millis() as u64),
    );
    report.insert(
        "real_trash_before".into(),
        serde_json::json!(real_trash_before),
    );

    let accels: Vec<serde_json::Value> = win
        .window
        .application()
        .map(|app| {
            [
                "win.save-note",
                "win.find-in-note",
                "win.replace-in-note",
                "win.quick-open",
                "win.new-note",
                "win.focus-search",
                "win.align-table",
                "win.toggle-edit-mode",
            ]
            .into_iter()
            .map(|action| {
                serde_json::json!({
                    "action": action,
                    "accels": app.accels_for_action(action).iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                })
            })
            .collect()
        })
        .unwrap_or_default();
    report.insert("accels".into(), serde_json::json!(accels));

    let initial_ready = wait_for(20000, || {
        let st = win.state.borrow();
        st.scan_complete
            && !st.tabs.is_empty()
            && st
                .tabs
                .values()
                .all(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    report.insert("initial_ready".into(), serde_json::json!(initial_ready));
    stage!("initial-ready");

    stage!("A-edit");
    let note = "L2-编辑目标.md";
    win.open_note_at(note, None);
    let loaded = wait_for(8000, || {
        win.state
            .borrow()
            .tabs
            .get(note)
            .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    let mut edit = serde_json::Map::new();
    edit.insert("loaded".into(), serde_json::json!(loaded));
    let already_editing = win
        .state
        .borrow()
        .tabs
        .get(note)
        .is_some_and(|tab| tab.editing);
    if !already_editing {
        win.toggle_active_tab_edit_mode();
    }
    edit.insert(
        "entered".into(),
        serde_json::json!(
            win.state
                .borrow()
                .tabs
                .get(note)
                .is_some_and(|tab| tab.editing)
        ),
    );
    win.restore_or_clear_undo(note);
    let undo_baseline = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.edit_buffer.can_undo())
        .unwrap_or(false);
    win.insert_into_edit(note, "\n\nL2 增量输入标记\n");
    let typed_synced = wait_for(4000, || {
        win.state
            .borrow()
            .tabs
            .get(note)
            .is_some_and(|tab| tab.edit_state.borrow().text.contains("L2 增量输入标记"))
    })
    .await;
    let session_text_after_typing = win.session_text(note).await.unwrap_or_default();
    edit.insert("typed_synced".into(), serde_json::json!(typed_synced));
    edit.insert(
        "session_matches_buffer".into(),
        serde_json::json!(session_text_after_typing.contains("L2 增量输入标记")),
    );
    edit.insert(
        "dirty_marker".into(),
        serde_json::json!(
            win.tab_facts(note)
                .map(|f| f.title.starts_with('●'))
                .unwrap_or(false)
        ),
    );

    {
        let (buffer, session) = {
            let st = win.state.borrow();
            let Some(tab) = st.tabs.get(note) else {
                return serde_json::json!({"error": "tab missing"});
            };
            (tab.edit_buffer.clone(), tab.session.clone())
        };
        if let Some(session) = session {
            let text = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
            let mapper = Utf16Mapper::new(&text);
            for _ in 0..3 {
                if let Ok(styles) = session.source_styles(u64::MAX, true) {
                    apply_source_styles_with(&buffer, &mapper, &styles.packed_runs);
                }
            }
        }
    }
    let undo_after_styles = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.edit_buffer.can_undo())
        .unwrap_or(true);
    edit.insert(
        "styles_clear_of_undo".into(),
        serde_json::json!(!undo_after_styles || undo_after_styles == undo_baseline),
    );

    let saves_before = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.save-note", None);
    let saved = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before)
    })
    .await;
    let session_text = win.session_text(note).await.unwrap_or_default();
    let disk_text = win.read_disk_note(note).unwrap_or_default();
    let facts = win.tab_facts(note).unwrap_or_default();
    edit.insert("ctrl_s_saved".into(), serde_json::json!(saved));
    edit.insert(
        "facts".into(),
        serde_json::json!({
            "revision": facts.revision,
            "restyles": facts.restyles,
            "editing": facts.editing,
            "loaded": facts.loaded,
            "issue": format!("{:?}", facts.issue),
            "issue_message": facts.issue_message,
        }),
    );
    edit.insert(
        "disk_matches_session".into(),
        serde_json::json!(!disk_text.is_empty() && disk_text == session_text),
    );
    edit.insert("save_micros".into(), serde_json::json!(facts.style_micros));
    edit.insert(
        "temp_leftovers_in_vault".into(),
        serde_json::json!(collect_temp_leftovers(Path::new(&root))),
    );

    win.insert_into_edit(note, "\n\n恢复稿检查\n");
    let _ = wait_for(1200, || win.tab_facts(note).is_some_and(|f| f.dirty)).await;
    let session_ckpt = win
        .state
        .borrow()
        .tabs
        .get(note)
        .and_then(|tab| tab.session.clone());
    if let Some(session) = session_ckpt {
        let _ = gio::spawn_blocking(move || session.checkpoint()).await;
    }
    let checkpoint_after_dirty = win.checkpoint_count();
    let saves_before = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    win.request_save(note, SaveChoice::Normal, false);
    let _ = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before)
    })
    .await;
    let checkpoint_after_save = win.checkpoint_count();
    edit.insert(
        "checkpoint_written".into(),
        serde_json::json!(checkpoint_after_dirty > 0),
    );
    edit.insert(
        "checkpoint_cleared_after_save".into(),
        serde_json::json!(checkpoint_after_save == 0),
    );

    let before_undo = win.session_text(note).await.unwrap_or_default();
    if let Some(buffer) = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.edit_buffer.clone())
    {
        buffer.undo();
    }
    let undo_text = win.session_text(note).await.unwrap_or_default();
    if let Some(buffer) = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.edit_buffer.clone())
    {
        buffer.redo();
    }
    let redo_text = win.session_text(note).await.unwrap_or_default();
    edit.insert(
        "undo_redo".into(),
        serde_json::json!({
            "undo_changed": undo_text != before_undo,
            "redo_restored": redo_text == before_undo,
        }),
    );

    win.insert_into_edit(note, "\n- 续写条目");
    if let Some(buffer) = win
        .state
        .borrow()
        .tabs
        .get(note)
        .map(|tab| tab.edit_buffer.clone())
    {
        let end = buffer.end_iter();
        buffer.place_cursor(&end);
    }
    win.insert_into_edit(note, "\n");
    let list_continuation_live = wait_for(1500, || {
        win.tab_facts(note)
            .is_some_and(|f| f.text.contains("- 续写条目\n- "))
    })
    .await;
    edit.insert(
        "list_continuation_helper".into(),
        serde_json::json!(
            crate::core::line_list_continuation("- 待办条目").as_deref() == Some("- ")
        ),
    );
    edit.insert(
        "list_continuation_live".into(),
        serde_json::json!(list_continuation_live),
    );

    {
        let text = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        if let Some(byte) = text.find("- [ ] 待办任务一") {
            let char_index = text[..byte].chars().count() + 8;
            win.place_cursor_at_char(note, char_index);
        }
        let before = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.toggle-task", None);
        let _ = wait_for(600, || {
            win.tab_facts(note).is_some_and(|f| f.text != before)
        })
        .await;
        let after = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        edit.insert(
            "task_toggle_live".into(),
            serde_json::json!(after.contains("- [x] 待办任务一")),
        );
    }

    {
        let text = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        if let Some(byte) = text.find("| 规范指南 |") {
            let char_index = text[..byte].chars().count() + 3;
            win.place_cursor_at_char(note, char_index);
        }
        let before = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.align-table", None);
        let _ = wait_for(800, || {
            win.tab_facts(note).is_some_and(|f| f.text != before)
        })
        .await;
        let after = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        edit.insert("align_table_invoked".into(), serde_json::json!(true));
        edit.insert(
            "align_table_patch".into(),
            serde_json::json!(before != after),
        );
    }

    let saves_before = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    let autosave_started = Instant::now();
    win.insert_into_edit(note, "\n\n自动保存标记\n");
    let autosaved = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before)
    })
    .await;
    edit.insert("autosaved".into(), serde_json::json!(autosaved));
    edit.insert(
        "autosave_ms".into(),
        serde_json::json!(autosave_started.elapsed().as_millis() as u64),
    );

    // L2d-2：验证未保存与已保存编辑内容切回阅读视图后立即刷新预览及大纲。
    if !win.tab_facts(note).is_some_and(|f| f.editing) {
        win.switch_edit_mode();
        let _ = wait_for(2000, || win.tab_facts(note).is_some_and(|f| f.editing)).await;
    }
    let unsaved_marker = "L2d2-即时预览校验-未保存";
    win.insert_into_edit(note, &format!("\n\n{unsaved_marker}\n"));
    let rev_unsaved = win.tab_facts(note).map(|f| f.revision).unwrap_or(0);
    win.switch_edit_mode();
    let unsaved_synced = wait_for(4000, || {
        win.state.borrow().tabs.get(note).is_some_and(|tab| {
            !tab.editing
                && tab.reading_revision == rev_unsaved
                && tab
                    .reading
                    .as_ref()
                    .is_some_and(|s| s.plain_text().contains(unsaved_marker))
        })
    })
    .await;

    win.switch_edit_mode();
    let _ = wait_for(2000, || win.tab_facts(note).is_some_and(|f| f.editing)).await;
    let saved_heading = "L2d2-已保存大纲标题";
    let saved_marker = "L2d2-即时预览校验-已保存";
    win.insert_into_edit(note, &format!("\n\n## {saved_heading}\n\n{saved_marker}\n"));
    let saves_sync = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    win.request_save(note, SaveChoice::Normal, false);
    let saved_ok = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_sync)
    })
    .await;
    let rev_saved = win.tab_facts(note).map(|f| f.revision).unwrap_or(0);
    win.switch_edit_mode();
    let saved_synced = wait_for(4000, || {
        win.state.borrow().tabs.get(note).is_some_and(|tab| {
            !tab.editing
                && tab.reading_revision == rev_saved
                && tab
                    .reading
                    .as_ref()
                    .is_some_and(|s| s.plain_text().contains(saved_marker))
        })
    })
    .await;
    let outline_updated = win
        .state
        .borrow()
        .tabs
        .get(note)
        .is_some_and(|tab| tab.headings.iter().any(|h| h.title.contains(saved_heading)));
    edit.insert(
        "edit_preview_sync".into(),
        serde_json::json!({
            "unsaved_reflected": unsaved_synced,
            "saved_ok": saved_ok,
            "saved_reflected": saved_synced,
            "outline_updated": outline_updated,
        }),
    );

    let saves_before_queue = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    win.request_save(note, SaveChoice::Normal, false);
    win.insert_into_edit(note, "\nQUEUED_SAVE_TAIL\n");
    let queued_save_ok = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before_queue)
            && win
                .read_disk_note(note)
                .is_some_and(|d| d.contains("QUEUED_SAVE_TAIL"))
    })
    .await;
    edit.insert(
        "queued_save_followup".into(),
        serde_json::json!(queued_save_ok),
    );

    let _ = gio::spawn_blocking(|| {}).await;
    report.insert("edit".into(), serde_json::Value::Object(edit));

    stage!("B-external");
    let mut external = serde_json::Map::new();
    let _ = win.select_tab(note);
    win.request_save(note, SaveChoice::Normal, false);
    let _ = wait_for(3000, || win.tab_facts(note).is_some_and(|f| !f.dirty)).await;
    glib::timeout_future(Duration::from_millis(120)).await;
    if let Some(reading) = win
        .state
        .borrow()
        .tabs
        .get(note)
        .and_then(|tab| tab.reading.clone())
    {
        reading.scroll_fraction(0.4);
    }
    glib::timeout_future(Duration::from_millis(180)).await;
    let scroll_before = win
        .state
        .borrow()
        .tabs
        .get(note)
        .and_then(|tab| {
            tab.reading.as_ref().map(|r| {
                let adj = r.scrolled.vadjustment();
                let max = adj.upper() - adj.page_size();
                if max > 0.0 { adj.value() / max } else { 0.0 }
            })
        })
        .unwrap_or(0.0);
    let reloads_before = win.tab_facts(note).map(|f| f.reloads).unwrap_or(0);
    let base_text = win.read_disk_note(note).unwrap_or_default();
    let external_text = format!("{base_text}\n\n## 外部追加标题\n\n来自外部进程的追加内容。\n");
    let wrote_external = win.writing_ext(note, &external_text);
    let clean_reload = wait_for(9000, || {
        win.tab_facts(note)
            .is_some_and(|f| f.reloads > reloads_before && f.text.contains("外部追加标题"))
    })
    .await;
    glib::timeout_future(Duration::from_millis(320)).await;
    let scroll_after = win
        .state
        .borrow()
        .tabs
        .get(note)
        .and_then(|tab| {
            tab.reading.as_ref().map(|r| {
                let adj = r.scrolled.vadjustment();
                let max = adj.upper() - adj.page_size();
                if max > 0.0 { adj.value() / max } else { 0.0 }
            })
        })
        .unwrap_or(0.0);
    external.insert(
        "external_write_ok".into(),
        serde_json::json!(wrote_external),
    );
    external.insert("clean_reload".into(), serde_json::json!(clean_reload));
    external.insert("scroll_before".into(), serde_json::json!(scroll_before));
    external.insert("scroll_after".into(), serde_json::json!(scroll_after));
    external.insert(
        "scroll_preserved".into(),
        serde_json::json!((scroll_after - scroll_before).abs() < 0.12),
    );

    // L5-28: 编辑态外部干净重载保留纵向滚动比例与光标字符偏移
    if !win.tab_facts(note).is_some_and(|f| f.editing) {
        win.switch_edit_mode();
        let _ = wait_for(2000, || win.tab_facts(note).is_some_and(|f| f.editing)).await;
    }
    let (edit_cursor_target, edit_scroll_before) = {
        let st = win.state.borrow();
        if let Some(tab) = st.tabs.get(note) {
            let char_len = tab.edit_buffer.char_count();
            let target_off = (char_len / 2).clamp(1, char_len.max(1));
            let iter = tab.edit_buffer.iter_at_offset(target_off);
            tab.edit_buffer.place_cursor(&iter);
            let adj = tab.edit_scrolled.vadjustment();
            let max = (adj.upper() - adj.page_size()).max(0.0);
            if max > 0.0 {
                adj.set_value(max * 0.45);
            }
            let frac = if max > 0.0 { adj.value() / max } else { 0.0 };
            (target_off, frac)
        } else {
            (0, 0.0)
        }
    };
    glib::timeout_future(Duration::from_millis(100)).await;
    let reloads_before_edit = win.tab_facts(note).map(|f| f.reloads).unwrap_or(0);
    let edit_base_disk = win.read_disk_note(note).unwrap_or_default();
    let edit_ext_disk = format!("{edit_base_disk}\nL5-28 编辑态重载追加行\n");
    let _ = win.writing_ext(note, &edit_ext_disk);
    let edit_clean_reload = wait_for(9000, || {
        win.tab_facts(note).is_some_and(|f| {
            f.reloads > reloads_before_edit && f.text.contains("L5-28 编辑态重载追加行")
        })
    })
    .await;
    glib::timeout_future(Duration::from_millis(160)).await;
    let (edit_cursor_after, edit_scroll_after) = {
        let st = win.state.borrow();
        if let Some(tab) = st.tabs.get(note) {
            let mark = tab.edit_buffer.get_insert();
            let off = tab.edit_buffer.iter_at_mark(&mark).offset();
            let adj = tab.edit_scrolled.vadjustment();
            let max = (adj.upper() - adj.page_size()).max(0.0);
            let frac = if max > 0.0 { adj.value() / max } else { 0.0 };
            (off, frac)
        } else {
            (0, 0.0)
        }
    };
    external.insert(
        "edit_reload_preserve".into(),
        serde_json::json!({
            "reloaded": edit_clean_reload,
            "cursor_before": edit_cursor_target,
            "cursor_after": edit_cursor_after,
            "cursor_preserved": edit_clean_reload && edit_cursor_after == edit_cursor_target,
            "scroll_before": edit_scroll_before,
            "scroll_after": edit_scroll_after,
            "scroll_preserved": edit_clean_reload && (edit_scroll_before == 0.0 || (edit_scroll_after - edit_scroll_before).abs() < 0.15),
        }),
    );

    win.insert_into_edit(note, "\n\n冲突前本地输入\n");
    let _ = wait_for(600, || win.tab_facts(note).is_some_and(|f| f.dirty)).await;
    let _ = win.writing_ext(note, "# 外部改写\n\n覆盖内容，不应进入界面。\n");
    let conflict_shown = wait_for(9000, || {
        win.tab_facts(note)
            .is_some_and(|f| f.issue == Some(DocumentIssue::Conflict) && f.notice_visible)
    })
    .await;
    let conflict_local_preserved = win
        .tab_facts(note)
        .is_some_and(|f| f.text.contains("冲突前本地输入"));
    let saves_before = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    win.request_save(note, SaveChoice::KeepMine, true);
    let keep_mine_saved = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before)
    })
    .await;
    let disk_after_keep = win.read_disk_note(note).unwrap_or_default();
    external.insert(
        "conflict".into(),
        serde_json::json!({
            "shown": conflict_shown,
            "local_preserved": conflict_local_preserved,
            "keep_mine_saved": keep_mine_saved,
            "issue_message": win.tab_facts(note).map(|f| f.issue_message).unwrap_or_default(),
            "disk_has_local": disk_after_keep.contains("冲突前本地输入"),
            "disk_has_external_only": disk_after_keep.contains("覆盖内容，不应进入界面"),
        }),
    );

    if let Some(root_path) = win.state.borrow().vault_root() {
        let _ = std::fs::remove_file(Path::new(&root_path).join(note));
    }
    let missing_shown = wait_for(9000, || {
        win.tab_facts(note)
            .is_some_and(|f| f.issue == Some(DocumentIssue::Missing) && f.notice_visible)
    })
    .await;
    let missing_text_preserved = win
        .tab_facts(note)
        .is_some_and(|f| f.text.contains("冲突前本地输入"));
    let saves_before = win.tab_facts(note).map(|f| f.saves).unwrap_or(0);
    win.request_save(note, SaveChoice::Recreate, true);
    let recreated = wait_for(6000, || {
        win.tab_facts(note)
            .is_some_and(|f| !f.dirty && f.saves > saves_before)
    })
    .await;
    let recreated_disk = win.read_disk_note(note).unwrap_or_default();
    let missing_facts = win.tab_facts(note).unwrap_or_default();
    external.insert(
        "missing".into(),
        serde_json::json!({
            "shown": missing_shown,
            "text_preserved": missing_text_preserved,
            "recreated": recreated,
            "disk_recreated": recreated_disk.contains("冲突前本地输入"),
            "disk_len": recreated_disk.chars().count(),
            "saves_before": saves_before,
            "saves_after": missing_facts.saves,
            "issue_after": format!("{:?}", missing_facts.issue),
            "dirty_after": missing_facts.dirty,
            "refreshes": missing_facts.refreshes,
            "deferred_refreshes": missing_facts.deferred_refreshes,
            "last_refresh_issue": format!("{:?}", missing_facts.last_refresh_issue),
            "poll_refreshes": missing_facts.poll_refreshes,
        }),
    );

    let folder = "L2-外部文件夹";
    let folder_notes = ["L2-外部文件夹/甲.md", "L2-外部文件夹/乙.md"];
    {
        let dir = Path::new(&root).join(folder);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("甲.md"), "# 甲\n\n文件夹级删除样本甲。\n");
        let _ = std::fs::write(dir.join("乙.md"), "# 乙\n\n文件夹级删除样本乙。\n");
        if let Some(handle) = win.state.borrow().vault_handle() {
            handle.update_search_index(
                folder_notes.iter().map(|path| path.to_string()).collect(),
                vec![],
            );
        }
    }
    for rel in folder_notes {
        win.open_note_at(rel, None);
    }
    let folder_loaded = wait_for(9000, || {
        let st = win.state.borrow();
        folder_notes.iter().all(|rel| {
            st.tabs
                .get(*rel)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
        })
    })
    .await;
    let _ = std::fs::remove_dir_all(Path::new(&root).join(folder));
    win.process_file_changes(&[folder.to_string()]);
    let folder_missing = wait_for(9000, || {
        let st = win.state.borrow();
        folder_notes.iter().all(|rel| {
            st.tabs
                .get(*rel)
                .is_some_and(|tab| tab.edit_state.borrow().issue == Some(DocumentIssue::Missing))
        })
    })
    .await;
    let folder_tabs_kept = {
        let st = win.state.borrow();
        folder_notes.iter().all(|rel| st.tabs.contains_key(*rel))
    };
    external.insert(
        "folder_missing".into(),
        serde_json::json!({
            "folder": folder,
            "notes": folder_notes,
            "loaded": folder_loaded,
            "missing_shown": folder_missing,
            "tabs_kept": folder_tabs_kept,
        }),
    );

    // L5-17: RecoveryDraft（缺失笔记但恢复稿写入成功）应允许直接关闭，不弹阻塞对话框
    let rec_close_note = "L2-恢复稿关闭.md";
    let _ = std::fs::write(
        Path::new(&root).join(rec_close_note),
        "# 恢复稿关闭\n\n验证 RecoveryDraft 直接关闭行为。\n",
    );
    if let Some(handle) = win.state.borrow().vault_handle() {
        handle.update_search_index(vec![rec_close_note.into()], vec![]);
    }
    win.open_note_at(rec_close_note, None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(rec_close_note)
            .is_some_and(|tab| tab.source_loaded)
    })
    .await;
    if !win
        .state
        .borrow()
        .tabs
        .get(rec_close_note)
        .is_some_and(|tab| tab.editing)
    {
        win.toggle_active_tab_edit_mode();
    }
    win.insert_into_edit(rec_close_note, "\n\n可写入恢复稿的未保存内容\n");
    let _ = wait_for(800, || {
        win.tab_facts(rec_close_note).is_some_and(|f| f.dirty)
    })
    .await;
    let _ = std::fs::remove_file(Path::new(&root).join(rec_close_note));
    let _ = wait_for(7000, || {
        win.tab_facts(rec_close_note)
            .is_some_and(|f| f.issue == Some(DocumentIssue::Missing))
    })
    .await;
    {
        let mut st = win.state.borrow_mut();
        st.last_alert = None;
        st.last_alert_dialog.set(None);
    }
    win.close_tab_by_path(rec_close_note);
    let rec_draft_closed = wait_for(4000, || {
        !win.state.borrow().tabs.contains_key(rec_close_note)
    })
    .await;
    let rec_draft_alert = win.state.borrow().last_alert.is_some();
    external.insert(
        "recovery_draft_closes_cleanly".into(),
        serde_json::json!({
            "closed_cleanly": rec_draft_closed,
            "alert_shown": rec_draft_alert,
        }),
    );

    // 关闭保护（L5-17 真 Unsaved：磁盘与恢复稿均不可写时弹出原生保护对话框）
    #[cfg(unix)]
    let recovery_dir_guard = {
        let dir = std::env::var("HORTUS_CACHE_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(s).join("recovery"));
        if let Some(ref d) = dir {
            let _ = std::fs::create_dir_all(d);
        }
        dir
    };
    #[cfg(unix)]
    let lock_recovery_dir = |locked: bool| {
        use std::os::unix::fs::PermissionsExt;
        if let Some(ref d) = recovery_dir_guard {
            let mode = if locked { 0o555 } else { 0o755 };
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(mode));
        }
    };
    #[cfg(not(unix))]
    let lock_recovery_dir = |_locked: bool| {};

    let close_guard_note = "L2-关闭保护.md";
    let _ = std::fs::write(
        Path::new(&root).join(close_guard_note),
        "# 关闭保护\n\n用于验证未保存内容不会被静默丢弃。\n",
    );
    if let Some(handle) = win.state.borrow().vault_handle() {
        handle.update_search_index(vec![close_guard_note.into()], vec![]);
    }
    win.open_note_at(close_guard_note, None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(close_guard_note)
            .is_some_and(|tab| tab.source_loaded)
    })
    .await;
    if !win
        .state
        .borrow()
        .tabs
        .get(close_guard_note)
        .is_some_and(|tab| tab.editing)
    {
        win.toggle_active_tab_edit_mode();
    }
    lock_recovery_dir(true);
    win.insert_into_edit(close_guard_note, "\n\n未保存的关闭保护内容\n");
    let _ = wait_for(800, || {
        win.tab_facts(close_guard_note).is_some_and(|f| f.dirty)
    })
    .await;
    let _ = std::fs::remove_file(Path::new(&root).join(close_guard_note));
    win.process_file_changes(&[close_guard_note.to_string()]);
    let _ = wait_for(7000, || {
        win.tab_facts(close_guard_note)
            .is_some_and(|f| f.issue == Some(DocumentIssue::Missing))
    })
    .await;
    {
        let mut st = win.state.borrow_mut();
        st.last_alert = None;
        st.last_alert_dialog.set(None);
    }
    win.close_tab_by_path(close_guard_note);
    let guard_alert = wait_for(6000, || {
        let st = win.state.borrow();
        st.last_alert.is_some() && st.last_alert_dialog.upgrade().is_some()
    })
    .await;
    let guard_tab_kept = win.state.borrow().tabs.contains_key(close_guard_note);
    let guard_text_kept = win
        .tab_facts(close_guard_note)
        .is_some_and(|f| f.text.contains("未保存的关闭保护内容"));
    let dialog = win.state.borrow().last_alert_dialog.upgrade();
    if let Some(dialog) = dialog {
        dialog.close();
    }
    lock_recovery_dir(false);
    external.insert(
        "close_guard".into(),
        serde_json::json!({
            "alert_shown": guard_alert,
            "tab_kept": guard_tab_kept,
            "text_kept": guard_text_kept,
            "alert_title": win.state.borrow().last_alert,
        }),
    );
    let _ = wait_for(1200, || {
        win.state.borrow().last_alert_dialog.upgrade().is_none()
    })
    .await;
    win.request_save(close_guard_note, SaveChoice::Recreate, false);
    let _ = wait_for(4000, || {
        win.tab_facts(close_guard_note).is_some_and(|f| !f.dirty)
    })
    .await;
    win.close_tab_by_path(close_guard_note);

    let window_guard_note = "L2-窗口保护.md";
    let _ = std::fs::write(
        Path::new(&root).join(window_guard_note),
        "# 窗口保护\n\n验证关闭窗口时不会静默丢数据。\n",
    );
    if let Some(handle) = win.state.borrow().vault_handle() {
        handle.update_search_index(vec![window_guard_note.into()], vec![]);
    }
    win.open_note_at(window_guard_note, None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(window_guard_note)
            .is_some_and(|tab| tab.source_loaded)
    })
    .await;
    if !win
        .state
        .borrow()
        .tabs
        .get(window_guard_note)
        .is_some_and(|tab| tab.editing)
    {
        win.toggle_active_tab_edit_mode();
    }
    lock_recovery_dir(true);
    win.insert_into_edit(window_guard_note, "\n\n窗口关闭前的未保存输入\n");
    let _ = wait_for(800, || {
        win.tab_facts(window_guard_note).is_some_and(|f| f.dirty)
    })
    .await;
    let _ = std::fs::remove_file(Path::new(&root).join(window_guard_note));
    win.process_file_changes(&[window_guard_note.to_string()]);
    let _ = wait_for(7000, || {
        win.tab_facts(window_guard_note)
            .is_some_and(|f| f.issue == Some(DocumentIssue::Missing))
    })
    .await;
    {
        let mut st = win.state.borrow_mut();
        st.last_alert = None;
        st.last_alert_dialog.set(None);
    }
    let _close_signal_handled = win.handle_window_close();
    let window_guard_alert = wait_for(8000, || {
        let st = win.state.borrow();
        st.last_alert.is_some() && st.last_alert_dialog.upgrade().is_some()
    })
    .await;
    let window_kept = win.window.is_visible();
    let dialog = win.state.borrow().last_alert_dialog.upgrade();
    if let Some(dialog) = dialog {
        dialog.close();
    }
    lock_recovery_dir(false);
    let _ = wait_for(1200, || {
        win.state.borrow().last_alert_dialog.upgrade().is_none()
    })
    .await;
    external.insert(
        "window_close_guard".into(),
        serde_json::json!({
            "alert_shown": window_guard_alert,
            "window_kept": window_kept,
            "alert_title": win.state.borrow().last_alert,
            "close_requests": win.state.borrow().close_requests,
            "close_unsaved": win.state.borrow().close_unsaved,
        }),
    );
    win.request_save(window_guard_note, SaveChoice::Recreate, false);
    let _ = wait_for(4000, || {
        win.tab_facts(window_guard_note).is_some_and(|f| !f.dirty)
    })
    .await;
    win.close_tab_by_path(window_guard_note);

    // L5-06: 恢复稿未决期间编辑器只读锁定，恢复或丢弃后解锁
    let rec_note = "L2-恢复稿锁定.md";
    let _ = std::fs::write(
        Path::new(&root).join(rec_note),
        "# 恢复稿锁定\n\n初始正文。\n",
    );
    if let Some(handle) = win.state.borrow().vault_handle() {
        handle.update_search_index(vec![rec_note.into()], vec![]);
    }
    let rec_svc = win.state.borrow().services.clone();
    let seeded_recovery = if let Some(svc) = rec_svc {
        let root_c = root.clone();
        gio::spawn_blocking(move || {
            let Ok(sess) = svc.core.open_document(root_c, rec_note.into()) else {
                return false;
            };
            let _ = sess.replace(0, 0, "CRASH_RECOVERY_DRAFT\n".to_string());
            sess.checkpoint().is_ok()
        })
        .await
        .unwrap_or(false)
    } else {
        false
    };
    win.open_note_at(rec_note, None);
    let rec_locked = wait_for(6000, || {
        win.state.borrow().tabs.get(rec_note).is_some_and(|t| {
            t.source_loaded
                && t.edit_state.borrow().issue == Some(DocumentIssue::Recovery)
                && t.notice.reveals_child()
                && !t.edit_view.is_editable()
        })
    })
    .await;
    win.recover_draft(rec_note, true);
    let rec_unlocked = wait_for(4000, || {
        win.state.borrow().tabs.get(rec_note).is_some_and(|t| {
            t.edit_state.borrow().issue.is_none()
                && t.edit_view.is_editable()
                && t.edit_state.borrow().text.contains("CRASH_RECOVERY_DRAFT")
        })
    })
    .await;
    win.request_save(rec_note, SaveChoice::Normal, false);
    let _ = wait_for(4000, || win.tab_facts(rec_note).is_some_and(|f| !f.dirty)).await;
    win.close_tab_by_path(rec_note);
    external.insert(
        "recovery_readonly".into(),
        serde_json::json!({
            "seeded": seeded_recovery,
            "locked": rec_locked,
            "unlocked": rec_unlocked,
        }),
    );

    // L5-07: 磁盘只读时外部刷新锁定 edit_view，恢复写权限后解锁
    let ro_note = "L2-只读回滚.md";
    let ro_disk = Path::new(&root).join(ro_note);
    let _ = std::fs::write(&ro_disk, "# 只读锁定测试\n\n原始可写文本。\n");
    if let Some(handle) = win.state.borrow().vault_handle() {
        handle.update_search_index(vec![ro_note.into()], vec![]);
    }
    win.open_note_at(ro_note, None);
    let ro_opened = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(ro_note)
            .is_some_and(|t| t.source_loaded && t.edit_view.is_editable())
    })
    .await;
    #[cfg(unix)]
    let (ro_locked, ro_unlocked) = {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&ro_disk, std::fs::Permissions::from_mode(0o444));
        win.process_file_changes(&[ro_note.to_string()]);
        let locked = wait_for(6000, || {
            win.state
                .borrow()
                .tabs
                .get(ro_note)
                .is_some_and(|t| !t.edit_view.is_editable())
        })
        .await;
        let _ = std::fs::set_permissions(&ro_disk, std::fs::Permissions::from_mode(0o644));
        win.process_file_changes(&[ro_note.to_string()]);
        let unlocked = wait_for(6000, || {
            win.state
                .borrow()
                .tabs
                .get(ro_note)
                .is_some_and(|t| t.edit_view.is_editable())
        })
        .await;
        (locked, unlocked)
    };
    #[cfg(not(unix))]
    let (ro_locked, ro_unlocked) = (true, true);
    win.close_tab_by_path(ro_note);
    external.insert(
        "readonly_disk_lock".into(),
        serde_json::json!({
            "opened_editable": ro_opened,
            "locked": ro_locked,
            "unlocked": ro_unlocked,
        }),
    );

    report.insert("external".into(), serde_json::Value::Object(external));

    stage!("C-find");
    let mut find = serde_json::Map::new();
    let _ = win.select_tab(note);
    if !win.tab_facts(note).is_some_and(|f| f.editing) {
        win.switch_edit_mode();
        let _ = wait_for(2000, || win.tab_facts(note).is_some_and(|f| f.editing)).await;
    }
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.find-in-note", None);
    let find_bar = win.state.borrow().tabs.get(note).map(|t| t.find.clone());
    if let Some(find_bar) = find_bar {
        find_bar.set_query("Hortus");
        let got = wait_for(3000, || find_bar.stats().matches > 0).await;
        let stats = find_bar.stats();
        find.insert("opened".into(), serde_json::json!(stats.open));
        find.insert("editable".into(), serde_json::json!(stats.editable));
        find.insert("matches".into(), serde_json::json!(stats.matches));
        find.insert("matches_found".into(), serde_json::json!(got));
        let first = stats.current;
        find_bar.next();
        let second = find_bar.stats().current;
        find.insert(
            "next_advances".into(),
            serde_json::json!(stats.matches < 2 || second != first),
        );
        find_bar.set_replacement("L2Find");
        find_bar.replace_all();
        let _ = wait_for(1500, || find_bar.stats().matches == 0).await;
        let _ = wait_for(2500, || win.tab_facts(note).is_some_and(|f| !f.dirty)).await;
        let replaced_text = win.session_text(note).await.unwrap_or_default();
        find.insert(
            "replace_all".into(),
            serde_json::json!({
                "replaced": find_bar.stats().replaced,
                "session_has_replacement": replaced_text.contains("L2Find"),
                "session_has_no_query": !replaced_text.contains("Hortus"),
            }),
        );
        // L5-25: 保持查找条开启时修改正文，立即替换前自动校验/重算，避免错位覆盖新插入前缀
        find_bar.set_query("L2Find");
        let _ = wait_for(1500, || find_bar.stats().matches > 0).await;
        find_bar.set_replacement("L5FindOk");
        if let Some(buf) = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|t| t.edit_buffer.clone())
        {
            let mut start = buf.start_iter();
            buf.insert(&mut start, "STALE_GUARD_PREFIX ");
        }
        find_bar.replace_current();
        let after_stale = win.tab_facts(note).map(|f| f.text).unwrap_or_default();
        find.insert(
            "stale_guard_ok".into(),
            serde_json::json!(
                after_stale.starts_with("STALE_GUARD_PREFIX ") && after_stale.contains("L5FindOk")
            ),
        );
        find_bar.close();
        find.insert("closed".into(), serde_json::json!(!find_bar.is_open()));
        find.insert(
            "searches".into(),
            serde_json::json!(find_bar.stats().searches),
        );
    }

    win.open_note_at("Start here.md", None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get("Start here.md")
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.replace-in-note", None);
    let read_find = win
        .state
        .borrow()
        .tabs
        .get("Start here.md")
        .map(|tab| tab.find.clone());
    if let Some(read_find) = read_find {
        read_find.set_query("Learning");
        let _ = wait_for(2000, || read_find.stats().matches > 0).await;
        let stats = read_find.stats();
        find.insert(
            "reading_view".into(),
            serde_json::json!({
                "open": stats.open,
                "editable": stats.editable,
                "matches": stats.matches,
            }),
        );
        read_find.close();
    }

    // L2d-2：验证阅读视图跨多 TextSegment 的 Ctrl+F 查找与 Next 跨段跳转。
    let cross_note = "Markdown field guide.md";
    win.open_note_at(cross_note, None);
    let _ = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(cross_note)
            .is_some_and(|tab| tab.reading_loaded)
    })
    .await;
    let _ = gtk4::prelude::WidgetExt::activate_action(&win.window, "win.find-in-note", None);
    let cross_find = win
        .state
        .borrow()
        .tabs
        .get(cross_note)
        .map(|tab| tab.find.clone());
    if let Some(cross_find) = cross_find {
        cross_find.set_query("example");
        let _ = wait_for(2000, || {
            let st = cross_find.stats();
            st.matches >= 2 && st.segments >= 2
        })
        .await;
        let initial = cross_find.stats();
        let mut visited_segments = Vec::new();
        if let Some(seg) = initial.current_segment {
            visited_segments.push(seg);
        }
        let steps = initial.matches.clamp(1, 16);
        for _ in 0..steps {
            cross_find.next();
            let st = cross_find.stats();
            if let Some(seg) = st.current_segment
                && !visited_segments.contains(&seg)
            {
                visited_segments.push(seg);
            }
        }
        find.insert(
            "reading_cross_segment".into(),
            serde_json::json!({
                "open": initial.open,
                "editable": initial.editable,
                "matches": initial.matches,
                "segments": initial.segments,
                "visited_segments": visited_segments.len(),
            }),
        );
        cross_find.close();
    }
    report.insert("find".into(), serde_json::Value::Object(find));

    stage!("D-bigdoc");
    let mut big = serde_json::Map::new();
    let big_note = "L2-大文档.md";
    let open_started = Instant::now();
    win.open_note_at(big_note, None);
    let big_loaded = wait_for(20000, || {
        win.state
            .borrow()
            .tabs
            .get(big_note)
            .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    stage!("D-bigdoc-opened");
    big.insert(
        "open_ms".into(),
        serde_json::json!(open_started.elapsed().as_millis() as u64),
    );
    big.insert("loaded".into(), serde_json::json!(big_loaded));
    big.insert(
        "bytes".into(),
        serde_json::json!(
            std::fs::metadata(Path::new(&root).join(big_note))
                .map(|m| m.len())
                .unwrap_or(0)
        ),
    );
    if let Some((buffer, session)) = {
        let st = win.state.borrow();
        st.tabs.get(big_note).and_then(|tab| {
            tab.session
                .clone()
                .map(|session| (tab.edit_buffer.clone(), session))
        })
    } {
        let text = win.session_text(big_note).await.unwrap_or_default();
        let mapper = Utf16Mapper::new(&text);
        let mut best = u64::MAX;
        let mut worst = 0u64;
        let mut runs = 0usize;
        let mut analysis = 0u64;
        let mut plan_best = u64::MAX;
        let mut tag_best = u64::MAX;
        for _ in 0..3 {
            let started = Instant::now();
            let Ok(styles) = session.source_styles(u64::MAX, true) else {
                continue;
            };
            analysis = analysis.max(styles.analysis_micros);
            runs = styles.packed_runs.len() / 20;
            let (plan_micros, tag_micros) = crate::platform::style::apply_source_styles_timed(
                &buffer,
                &mapper,
                &styles.packed_runs,
            );
            plan_best = plan_best.min(plan_micros);
            tag_best = tag_best.min(tag_micros);
            let micros = started.elapsed().as_micros() as u64;
            best = best.min(micros);
            worst = worst.max(micros);
        }
        let big_facts = win.tab_facts(big_note).unwrap_or_default();
        big.insert(
            "load_style_micros".into(),
            serde_json::json!(big_facts.load_style_micros),
        );
        big.insert("style_runs".into(), serde_json::json!(runs));
        big.insert("style_best_micros".into(), serde_json::json!(best));
        big.insert("style_worst_micros".into(), serde_json::json!(worst));
        big.insert(
            "style_plan_best_micros".into(),
            serde_json::json!(plan_best),
        );
        big.insert("style_tag_best_micros".into(), serde_json::json!(tag_best));
        big.insert(
            "style_under_200ms".into(),
            serde_json::json!(worst < 200_000),
        );
        big.insert(
            "plan_under_200ms".into(),
            serde_json::json!(plan_best < 200_000),
        );
        big.insert(
            "load_style_under_200ms".into(),
            serde_json::json!(big_facts.load_style_micros < 200_000),
        );
        big.insert("analysis_micros".into(), serde_json::json!(analysis));
    }
    report.insert("big_doc".into(), serde_json::Value::Object(big));
    stage!("D-bigdoc-done");

    run_knowledge_files_and_misc(
        win,
        note,
        &root,
        &real_trash,
        real_trash_before,
        automation_started,
        debug,
        &mut report,
    )
    .await;
    serde_json::Value::Object(report)
}
