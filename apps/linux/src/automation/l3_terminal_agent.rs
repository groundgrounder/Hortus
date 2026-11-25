// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! L3 端到端自动化验证：统一单层三区工具标签栏、`+` 与右键/中键关闭及无损跨区迁移、
//! VTE4 终端安全发送、Agent 快照闸门与三级回滚、设置热重载、多窗口实时同步与会话恢复。

use crate::automation::driver::{capture_widget_png, wait_for};
use crate::core::{CliOptions, PanelRegion, StaticToolKind};
use crate::dock::DockTabId;
use crate::services::AppServices;
use crate::settings::SettingsPage;
use crate::terminal::process::is_pid_alive;
use crate::window::commands::WindowCommand;
use crate::window::session::{launch_initial_windows, parse_restorable_entries};
use crate::window::shell::{HortusWindow, WindowInner};
use crate::window::state::RestoredTabsOverride;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::EditableConfigInputInfo;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn render_window_texture(win: &adw::ApplicationWindow) -> Result<gtk4::gdk::Texture, String> {
    let width = win.width().max(1);
    let height = win.height().max(1);
    let paintable = gtk4::WidgetPaintable::new(Some(win));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or_else(|| "窗口渲染节点为空".to_string())?;
    let renderer = gtk4::prelude::NativeExt::renderer(win)
        .ok_or_else(|| "窗口 GskRenderer 不可用".to_string())?;
    let bounds = gtk4::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    Ok(renderer.render_texture(&node, Some(&bounds)))
}

fn capture_dual_window_png(
    win1: &adw::ApplicationWindow,
    win2: &adw::ApplicationWindow,
    out: &Path,
) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tex1 = render_window_texture(win1)?;
    let tex2 = render_window_texture(win2)?;
    let bytes1 = tex1.save_to_png_bytes();
    let bytes2 = tex2.save_to_png_bytes();
    let mut cur1 = Cursor::new(bytes1.as_ref());
    let mut cur2 = Cursor::new(bytes2.as_ref());
    let surf1 = gtk4::cairo::ImageSurface::create_from_png(&mut cur1)
        .map_err(|e| format!("读取窗口 1 PNG 失败：{e}"))?;
    let surf2 = gtk4::cairo::ImageSurface::create_from_png(&mut cur2)
        .map_err(|e| format!("读取窗口 2 PNG 失败：{e}"))?;
    let gap = 16;
    let total_w = surf1.width() + gap + surf2.width();
    let total_h = surf1.height().max(surf2.height());
    let canvas = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, total_w, total_h)
        .map_err(|e| format!("创建合成画布失败：{e}"))?;
    {
        let cr = gtk4::cairo::Context::new(&canvas).map_err(|e| e.to_string())?;
        cr.set_source_rgb(0.11, 0.12, 0.14);
        let _ = cr.paint();
        let _ = cr.set_source_surface(&surf1, 0.0, 0.0);
        let _ = cr.paint();
        let _ = cr.set_source_surface(&surf2, f64::from(surf1.width() + gap), 0.0);
        let _ = cr.paint();
    }
    let mut file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    canvas
        .write_to_png(&mut file)
        .map_err(|e| format!("写出双窗口 PNG 失败：{e}"))?;
    Ok(())
}

fn capture_window_with_menu_button_popover(
    win: &adw::ApplicationWindow,
    btn: &gtk4::MenuButton,
    out: &Path,
) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tex_win = render_window_texture(win)?;
    let bytes_win = tex_win.save_to_png_bytes();
    let mut cur_win = Cursor::new(bytes_win.as_ref());
    let surf_win = gtk4::cairo::ImageSurface::create_from_png(&mut cur_win)
        .map_err(|e| format!("读取窗口 PNG 失败：{e}"))?;
    let win_w = surf_win.width();
    let win_h = surf_win.height();
    let canvas = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, win_w, win_h)
        .map_err(|e| format!("创建合成画布失败：{e}"))?;
    {
        let cr = gtk4::cairo::Context::new(&canvas).map_err(|e| e.to_string())?;
        let _ = cr.set_source_surface(&surf_win, 0.0, 0.0);
        let _ = cr.paint();

        if let Some(popover) = btn.popover()
            && popover.is_visible()
            && let Some(renderer) = gtk4::prelude::NativeExt::renderer(&popover)
        {
            let pw = popover.width().max(1);
            let ph = popover.height().max(1);
            let paint = gtk4::WidgetPaintable::new(Some(&popover));
            let snap = gtk4::Snapshot::new();
            paint.snapshot(&snap, f64::from(pw), f64::from(ph));
            if let Some(node) = snap.to_node() {
                let bounds = gtk4::graphene::Rect::new(0.0, 0.0, pw as f32, ph as f32);
                let ptex = renderer.render_texture(&node, Some(&bounds));
                let pbytes = ptex.save_to_png_bytes();
                let mut pcur = Cursor::new(pbytes.as_ref());
                if let Ok(psurf) = gtk4::cairo::ImageSurface::create_from_png(&mut pcur)
                    && let Some(rect) = btn.compute_bounds(win)
                {
                    let px = (f64::from(rect.x() + rect.width()) - f64::from(pw))
                        .clamp(8.0, f64::from(win_w - pw - 8).max(8.0));
                    let py = f64::from(rect.y() + rect.height())
                        .clamp(8.0, f64::from(win_h - ph - 8).max(8.0));
                    let _ = cr.set_source_surface(&psurf, px, py);
                    let _ = cr.paint();
                }
            }
        }
    }
    let mut file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    canvas
        .write_to_png(&mut file)
        .map_err(|e| format!("写出带菜单 PNG 失败：{e}"))?;
    Ok(())
}

pub fn execute_terminal_agent_checks(
    win: &Rc<WindowInner>,
    json_path: PathBuf,
    media_dir: Option<PathBuf>,
    vault2_opt: Option<PathBuf>,
) {
    let win = Rc::clone(win);
    glib::MainContext::default().spawn_local(async move {
        let result = run_suite(&win, media_dir.as_deref(), vault2_opt.as_deref()).await;
        if let Some(parent) = json_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".into());
        let _ = std::fs::write(&json_path, body);
        win.close_now_clean();
    });
}

#[allow(clippy::too_many_lines)]
async fn run_suite(
    win: &Rc<WindowInner>,
    media_dir: Option<&Path>,
    vault2_opt: Option<&Path>,
) -> serde_json::Value {
    let ready = wait_for(5000, || {
        let st = win.state.borrow();
        st.services.is_some() && st.vault_ctx.is_some() && st.scan_complete
    })
    .await;
    if !ready {
        return serde_json::json!({ "ok": false, "error": "services_or_vault_timeout" });
    }

    let (services, vault_root, temp_dir) = {
        let st = win.state.borrow();
        (
            st.services.clone().unwrap(),
            st.vault_root().unwrap_or_default(),
            st.temp_dir.clone(),
        )
    };
    let vault_path = PathBuf::from(&vault_root);

    // -------------------------------------------------------------------------
    // Part 0 (L3b D1/D2/D3): 单层工具标签栏结构、+ 添加、× 关闭、自动收起与布局持久化
    // -------------------------------------------------------------------------
    let single_bar_left = win.tool_dock.region_visible_tab_bars(PanelRegion::Left) == 1;
    let single_bar_right = win.tool_dock.region_visible_tab_bars(PanelRegion::Right) == 1;
    let single_bar_bottom = win.tool_dock.region_visible_tab_bars(PanelRegion::Bottom) == 1;
    let all_regions_have_add = PanelRegion::ALL
        .iter()
        .all(|r| win.tool_dock.region_has_add_button(*r));
    let sidebar_max_480_ok = (win.left_split.max_sidebar_width() - 480.0).abs() < 0.1
        && (win.right_split.max_sidebar_width() - 480.0).abs() < 0.1
        && (win.left_split.min_sidebar_width() - 180.0).abs() < 0.1
        && (win.right_split.min_sidebar_width() - 180.0).abs() < 0.1;

    // -------------------------------------------------------------------------
    // Part 1: VTE4 原生终端 + 单层底栏 + 跨三区无损迁移 + 发送安全 + 退出/回收
    // -------------------------------------------------------------------------
    win.open_note_at("alpha.md", None);
    let alpha_loaded = wait_for(4000, || {
        win.state
            .borrow()
            .tabs
            .get("alpha.md")
            .is_some_and(|t| t.source_loaded && t.reading_loaded)
    })
    .await;

    win.set_bottom_panel_revealed(true);
    let shell_ready = wait_for(4000, || {
        let ids = win.terminal_host.tab_ids();
        ids.len() == 1
            && win
                .terminal_host
                .tab_snapshot(ids[0])
                .is_some_and(|s| s["pid"].as_i64().is_some())
    })
    .await;

    let shell_id = win.terminal_host.tab_ids().first().copied().unwrap_or(0);
    let bottom_titles_init = win.tool_dock.region_tab_titles(PanelRegion::Bottom);
    let bottom_single_bar_titles_ok = bottom_titles_init.len() == 1
        && win
            .terminal_host
            .tab_snapshot(shell_id)
            .is_some_and(|s| s["kind"] == "shell");
    let shell_snap_init = win.terminal_host.tab_snapshot(shell_id);
    let shell_pid = shell_snap_init
        .as_ref()
        .and_then(|s| s["pid"].as_i64())
        .unwrap_or(0) as i32;

    // Paned 底部面板可调节高度验证
    let paned_h = win.center_paned.height();
    if paned_h > 320 {
        win.center_paned.set_position(paned_h - 300);
        glib::timeout_future(Duration::from_millis(80)).await;
    }
    let paned_resizable_ok =
        paned_h <= 320 || (win.state.borrow().bottom_height - 300.0).abs() < 20.0;
    if paned_h > 320 {
        win.center_paned.set_position(paned_h - 255);
        glib::timeout_future(Duration::from_millis(80)).await;
    }

    let probe_file = temp_dir.join("l3_term_probe.txt");
    let _ = std::fs::remove_file(&probe_file);
    let probe_cmd = format!(
        "printf 'PWD=%s\\nLANG=%s\\nHORTUS=%s\\n' \"$PWD\" \"${{LANG:-}}\" \"$(command -v hortus || true)\" > '{}' && echo 'PTY_APPEND_LINE' >> alpha.md && echo '__L3B_MIGRATION_MARKER__'\n",
        probe_file.to_string_lossy()
    );
    let _ = win.terminal_host.feed_tab_command(shell_id, &probe_cmd);

    let probe_done = wait_for(6000, || {
        probe_file.exists()
            && win.terminal_host.tab_snapshot(shell_id).is_some_and(|s| {
                s["screen_text"]
                    .as_str()
                    .is_some_and(|txt| txt.contains("__L3B_MIGRATION_MARKER__"))
            })
    })
    .await;
    let probe_txt = std::fs::read_to_string(&probe_file).unwrap_or_default();
    let pwd_matches = probe_txt.contains(&format!("PWD={vault_root}"));
    let lang_utf8 = probe_txt.to_uppercase().contains("UTF-8");
    let hortus_found = probe_txt
        .lines()
        .find(|l| l.starts_with("HORTUS="))
        .is_some_and(|l| l.len() > "HORTUS=".len());

    // 终端写回笔记后，界面实时刷新打开的 alpha.md 标签
    win.refresh_document_tab("alpha.md");
    let pty_refreshed = wait_for(3000, || {
        win.state
            .borrow()
            .tabs
            .get("alpha.md")
            .is_some_and(|t| t.text.contains("PTY_APPEND_LINE"))
    })
    .await;

    // 测试发送笔记路径（含带空格路径引用）
    win.send_path_to_terminal();
    let (_, _, quoted_space_path) = win
        .terminal_host
        .send_note_path("folder/sub note.md", PanelRegion::Bottom);

    // 切编辑态构造含 \n + \x1b[201~ bracketed-paste spoof 的选区，验证不执行且仅作安全文本发送
    win.switch_edit_mode(false);
    let spoof_marker = temp_dir.join("l3_spoof_ran.txt");
    let _ = std::fs::remove_file(&spoof_marker);
    let dangerous_sel = format!(
        "selected-line-1\n\x1b[201~touch '{}'\nselected-line-2",
        spoof_marker.to_string_lossy()
    );
    if let Some(buf) = win
        .state
        .borrow()
        .tabs
        .get("alpha.md")
        .map(|t| t.edit_buffer.clone())
    {
        let prev = buf
            .text(&buf.start_iter(), &buf.end_iter(), false)
            .to_string();
        buf.set_text(&format!("{prev}\n{dangerous_sel}"));
        let end_off = buf.char_count();
        let start_off = end_off - dangerous_sel.chars().count() as i32;
        let s_it = buf.iter_at_offset(start_off.max(0));
        let e_it = buf.iter_at_offset(end_off);
        buf.select_range(&s_it, &e_it);
    }
    win.send_selection_to_terminal();
    glib::timeout_future(Duration::from_millis(350)).await;
    let spoof_blocked = !spoof_marker.exists();
    if let Some(buf) = win
        .state
        .borrow()
        .tabs
        .get("alpha.md")
        .map(|t| t.edit_buffer.clone())
    {
        let on_disk = std::fs::read_to_string(vault_path.join("alpha.md")).unwrap_or_default();
        buf.set_text(&on_disk);
    }
    win.switch_edit_mode(false);
    glib::timeout_future(Duration::from_millis(300)).await;

    // L3c D3-2: 标准 1280x800 窗口下量化断言左右边栏 3 个默认标签 100% 落在可视栏内、动态宽 < 120px，底栏标签不拉伸
    let left_strip_3 = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Left);
    let right_strip_3 = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Right);
    let bottom_strip_1 = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Bottom);
    let left_3tabs_fit = left_strip_3.as_ref().is_some_and(|m| {
        m.tabs.len() == 3
            && m.strip_width > 120.0
            && m.tabs.iter().all(|t| {
                t.fits_in_strip
                    && !t.ellipsized
                    && !t.has_close_button
                    && !t.has_motion_controller
                    && t.width > 24.0
                    && t.width < 140.0
            })
    });
    let right_3tabs_fit = right_strip_3.as_ref().is_some_and(|m| {
        m.tabs.len() == 3
            && m.strip_width > 120.0
            && m.tabs.iter().all(|t| {
                t.fits_in_strip
                    && !t.ellipsized
                    && !t.has_close_button
                    && !t.has_motion_controller
                    && t.width > 24.0
                    && t.width < 220.0
            })
    });
    let bottom_tab_compact = bottom_strip_1.as_ref().is_some_and(|m| {
        m.tabs.len() == 1
            && m.tabs[0].fits_in_strip
            && !m.tabs[0].has_close_button
            && !m.tabs[0].has_motion_controller
            && m.tabs[0].width < 220.0
    });

    // 截图 1 (v3): 标准宽度下左右边栏 3 标签完整显示不溢出 + 底部单层紧凑终端栏
    let shot_dynamic_sidebar = if let Some(dir) = media_dir {
        let p = dir.join("l3-v3-dynamic-sidebar-tabs.png");
        capture_widget_png(&win.window, &p).is_ok() && p.exists()
    } else {
        false
    };
    let _ = win.terminal_host.feed_tab_command(shell_id, "\x03\x15\n");
    glib::timeout_future(Duration::from_millis(120)).await;

    // L3c D3-1: 真实右键 PopoverMenu 迁移测试（改动 Right -> Bottom -> Left -> Right）
    let changes_id = DockTabId::Static(StaticToolKind::Changes);
    let changes_pop_opened_r = win.tool_dock.open_tab_context_menu(changes_id);
    let changes_pop_visible_r = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let move_left_lbl =
        crate::i18n::fmt1(crate::i18n::Key::DockMoveTabTo, PanelRegion::Left.title());
    let move_right_lbl =
        crate::i18n::fmt1(crate::i18n::Key::DockMoveTabTo, PanelRegion::Right.title());
    let move_bottom_lbl =
        crate::i18n::fmt1(crate::i18n::Key::DockMoveTabTo, PanelRegion::Bottom.title());
    let close_tab_lbl = crate::i18n::tr(crate::i18n::Key::DockCloseTab);
    let changes_menu_labels_r = win.tool_dock.active_context_menu_labels();
    let changes_menu_ok_r = changes_pop_opened_r
        && changes_pop_visible_r
        && changes_menu_labels_r.iter().any(|l| l == &move_bottom_lbl)
        && changes_menu_labels_r.iter().any(|l| l == &move_left_lbl)
        && changes_menu_labels_r.iter().any(|l| l == close_tab_lbl);
    let changes_click_to_bottom = win.tool_dock.activate_context_menu_item(&move_bottom_lbl);
    let changes_moved_to_bottom = wait_for(2000, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_none_or(|(_, p)| !p.is_visible())
            && win.tool_dock.tab_region(changes_id) == Some(PanelRegion::Bottom)
    })
    .await;

    let changes_pop_opened_b = win.tool_dock.open_tab_context_menu(changes_id);
    let changes_pop_visible_b = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let changes_menu_labels_b = win.tool_dock.active_context_menu_labels();
    let changes_menu_ok_b = changes_pop_opened_b
        && changes_pop_visible_b
        && changes_menu_labels_b.iter().any(|l| l == &move_left_lbl)
        && changes_menu_labels_b.iter().any(|l| l == &move_right_lbl)
        && !changes_menu_labels_b.iter().any(|l| l == &move_bottom_lbl);
    let changes_click_to_left = win.tool_dock.activate_context_menu_item(&move_left_lbl);
    let changes_moved_to_left = wait_for(2000, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_none_or(|(_, p)| !p.is_visible())
            && win.tool_dock.tab_region(changes_id) == Some(PanelRegion::Left)
    })
    .await;

    let changes_pop_opened_l = win.tool_dock.open_tab_context_menu(changes_id);
    let _ = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let changes_click_to_right = win.tool_dock.activate_context_menu_item(&move_right_lbl);
    let changes_moved_back_right = wait_for(2000, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_none_or(|(_, p)| !p.is_visible())
            && win.tool_dock.tab_region(changes_id) == Some(PanelRegion::Right)
    })
    .await;
    let changes_real_context_menu_ok = changes_menu_ok_r
        && changes_click_to_bottom
        && changes_moved_to_bottom
        && changes_menu_ok_b
        && changes_click_to_left
        && changes_moved_to_left
        && changes_pop_opened_l
        && changes_click_to_right
        && changes_moved_back_right;

    // 终端真实右键菜单检查与跨三区无损迁移（底 -> 右 -> 左 -> 底，全程 PID 与缓冲不变）
    let term_tab_id = DockTabId::Terminal(shell_id);
    let term_pop_opened = win.tool_dock.open_tab_context_menu(term_tab_id);
    let term_pop_visible = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let ctx_labels_bottom = win.tool_dock.active_context_menu_labels();
    let ctx_menu_bottom_ok = term_pop_opened
        && term_pop_visible
        && ctx_labels_bottom.iter().any(|l| l == &move_left_lbl)
        && ctx_labels_bottom.iter().any(|l| l == &move_right_lbl)
        && !ctx_labels_bottom.iter().any(|l| l == &move_bottom_lbl)
        && ctx_labels_bottom.iter().any(|l| l == close_tab_lbl);

    let term_click_to_right = win.tool_dock.activate_context_menu_item(&move_right_lbl);
    let term_moved_to_right = wait_for(2000, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_none_or(|(_, p)| !p.is_visible())
            && win.tool_dock.tab_region(term_tab_id) == Some(PanelRegion::Right)
    })
    .await;
    glib::timeout_future(Duration::from_millis(200)).await;
    let moved_right_pid = win
        .terminal_host
        .tab_snapshot(shell_id)
        .and_then(|s| s["pid"].as_i64())
        .unwrap_or(-1) as i32;
    let moved_right_buf = win
        .terminal_host
        .tab_snapshot(shell_id)
        .and_then(|s| s["screen_text"].as_str().map(str::to_string))
        .unwrap_or_default()
        .contains("__L3B_MIGRATION_MARKER__");
    let bottom_auto_collapsed_on_move = !win.is_bottom_panel_revealed();

    // 验证终端移入右侧栏后与 大纲 | 反链 | 改动 共 4 标签不溢出、无省略号，必要时非激活标签自动收为纯图标
    let right_strip_4 = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Right);
    let right_4tabs_fit = right_strip_4.as_ref().is_some_and(|m| {
        m.tabs.len() == 4
            && m.tabs.iter().all(|t| {
                t.fits_in_strip
                    && !t.ellipsized
                    && !t.has_close_button
                    && !t.has_motion_controller
                    && t.width > 24.0
            })
            && m.tabs.iter().any(|t| t.selected && t.label_visible)
    });
    let no_inline_close_buttons = [
        &left_strip_3,
        &right_strip_3,
        &bottom_strip_1,
        &right_strip_4,
    ]
    .into_iter()
    .all(|strip| {
        strip.as_ref().is_some_and(|m| {
            !m.tabs.is_empty()
                && m.tabs
                    .iter()
                    .all(|t| !t.has_close_button && !t.has_motion_controller)
        })
    });

    // 截图 2 (v3): 终端停靠在右侧栏并与大纲/反链/改动同层并列、动态缩窄不溢出
    let shot_right_sidebar_terminal = if let Some(dir) = media_dir {
        let p = dir.join("l3-v3-terminal-in-right-sidebar.png");
        capture_widget_png(&win.window, &p).is_ok() && p.exists()
    } else {
        false
    };

    let _ = win.tool_dock.open_tab_context_menu(term_tab_id);
    let _ = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let term_click_to_left = win.tool_dock.activate_context_menu_item(&move_left_lbl);
    let _ = wait_for(2000, || {
        win.tool_dock.tab_region(term_tab_id) == Some(PanelRegion::Left)
    })
    .await;
    glib::timeout_future(Duration::from_millis(120)).await;
    let moved_left_pid = win
        .terminal_host
        .tab_snapshot(shell_id)
        .and_then(|s| s["pid"].as_i64())
        .unwrap_or(-1) as i32;
    let moved_left_region = win.tool_dock.tab_region(term_tab_id);

    let _ = win.tool_dock.open_tab_context_menu(term_tab_id);
    let _ = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let term_click_to_bottom = win.tool_dock.activate_context_menu_item(&move_bottom_lbl);
    let _ = wait_for(2000, || {
        win.tool_dock.tab_region(term_tab_id) == Some(PanelRegion::Bottom)
    })
    .await;
    glib::timeout_future(Duration::from_millis(120)).await;
    let moved_back_pid = win
        .terminal_host
        .tab_snapshot(shell_id)
        .and_then(|s| s["pid"].as_i64())
        .unwrap_or(-1) as i32;
    let lossless_3_region_migration = shell_pid > 0
        && term_click_to_right
        && term_moved_to_right
        && term_click_to_left
        && term_click_to_bottom
        && moved_right_pid == shell_pid
        && moved_left_pid == shell_pid
        && moved_back_pid == shell_pid
        && is_pid_alive(shell_pid)
        && moved_right_buf
        && moved_left_region == Some(PanelRegion::Left)
        && bottom_auto_collapsed_on_move
        && win.tool_dock.tab_region(term_tab_id) == Some(PanelRegion::Bottom);

    // 测试工具栏右键「关闭标签」与 + 菜单添加/跨区移入
    let left_count_before = win.tool_dock.tab_count(PanelRegion::Left);
    let tags_id = DockTabId::Static(StaticToolKind::Tags);
    let tags_ctx_opened = win.tool_dock.open_tab_context_menu(tags_id);
    let tags_ctx_visible = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let tags_ctx_close_clicked = win.tool_dock.activate_context_menu_item(close_tab_lbl);
    let tags_closed = wait_for(2000, || {
        win.tool_dock.tab_count(PanelRegion::Left) + 1 == left_count_before
            && win
                .tool_dock
                .static_tool_region(StaticToolKind::Tags)
                .is_none()
    })
    .await
        && tags_ctx_opened
        && tags_ctx_visible
        && tags_ctx_close_clicked;

    let right_add_labels = win.tool_dock.add_menu_labels(PanelRegion::Right);
    let move_files_lbl = crate::i18n::fmt1(
        crate::i18n::Key::DockToolMoveHere,
        StaticToolKind::Files.title(),
    );
    let add_menu_items_ok = right_add_labels
        .iter()
        .any(|l| l == crate::i18n::tr(crate::i18n::Key::DockNewTerminal))
        && right_add_labels
            .iter()
            .any(|l| l == crate::i18n::tr(crate::i18n::Key::MenuLaunchAgent))
        && right_add_labels
            .iter()
            .any(|l| l == StaticToolKind::Tags.title())
        && right_add_labels.iter().any(|l| l == &move_files_lbl);

    // 弹出右侧栏 + 菜单截图展示工具添加与关闭效果
    win.tool_dock.popup_add_menu(PanelRegion::Right);
    if let Some(btn) = win.tool_dock.add_button_widget(PanelRegion::Right) {
        let _ = wait_for(1500, || {
            btn.popover()
                .is_some_and(|p| p.is_visible() && p.width() > 0 && p.height() > 0)
        })
        .await;
    }
    glib::timeout_future(Duration::from_millis(150)).await;
    let shot_tool_add_menu = if let Some(dir) = media_dir {
        let p = dir.join("l3-v3-tool-add-menu-and-close.png");
        let btn = win.tool_dock.add_button_widget(PanelRegion::Right);
        let mut ok = false;
        for _ in 0..3 {
            let res = if let Some(ref btn) = btn {
                capture_window_with_menu_button_popover(&win.window, btn, &p)
            } else {
                capture_widget_png(&win.window, &p)
            };
            if res.is_ok() && p.exists() {
                ok = true;
                break;
            }
            win.window.queue_draw();
            glib::timeout_future(Duration::from_millis(80)).await;
        }
        ok
    } else {
        false
    };
    win.tool_dock.popdown_add_menu(PanelRegion::Right);
    glib::timeout_future(Duration::from_millis(120)).await;

    let _ = gtk4::prelude::WidgetExt::activate_action(
        &win.window,
        "win.dock-add-tool",
        Some(&"right:tags".to_variant()),
    );
    glib::timeout_future(Duration::from_millis(150)).await;
    let tags_reopened_right =
        win.tool_dock.static_tool_region(StaticToolKind::Tags) == Some(PanelRegion::Right);

    // 移回左侧并立即持久化 layout.toml 验证不含动态终端 UUID
    let _ = gtk4::prelude::WidgetExt::activate_action(
        &win.window,
        "win.dock-add-tool",
        Some(&"left:tags".to_variant()),
    );
    win.persist_layout_now();
    glib::timeout_future(Duration::from_millis(250)).await;
    let layout_toml_path = Path::new(&services.config.snapshot().config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("layout.toml");
    let layout_toml_ok = std::fs::read_to_string(&layout_toml_path).is_ok_and(|txt| {
        txt.contains("files")
            && txt.contains("terminal")
            && !txt.contains(&format!("term:{shell_id}"))
    });

    // 测试从右侧栏 `+` 新建终端（win.dock-new-shell('right')）
    let right_shell_before = win.terminal_host.tab_ids().len();
    let _ = gtk4::prelude::WidgetExt::activate_action(
        &win.window,
        "win.dock-new-shell",
        Some(&"right".to_variant()),
    );
    let right_shell_spawned = wait_for(3000, || {
        win.terminal_host.tab_ids().len() > right_shell_before
            && win.terminal_host.tab_ids().iter().any(|id| {
                win.terminal_host
                    .tab_snapshot(*id)
                    .is_some_and(|s| s["region"] == "right" && s["pid"].as_i64().is_some())
            })
    })
    .await;
    if let Some(right_sid) =
        win.terminal_host.tab_ids().into_iter().find(|id| {
            win.tool_dock.tab_region(DockTabId::Terminal(*id)) == Some(PanelRegion::Right)
        })
    {
        win.terminal_host.close_tab_by_id(right_sid);
    }

    // 测试 Shell 退出非自动重启 + 关闭最后一个标签自动收起 + 再次展开自动新建终端
    let shell2_id = win.terminal_host.open_shell_in_region(PanelRegion::Bottom);
    let shell2_spawned = wait_for(3000, || {
        win.terminal_host
            .tab_snapshot(shell2_id)
            .is_some_and(|s| s["pid"].as_i64().is_some())
    })
    .await;
    let _ = win.terminal_host.feed_tab_command(shell2_id, "exit 42\n");
    let shell2_exited = wait_for(4000, || {
        win.terminal_host.tab_snapshot(shell2_id).is_some_and(|s| {
            s["phase"].as_str() == Some("exited")
                && s["exit_code"].as_i64() == Some(42)
                && s["status_visible"].as_bool() == Some(true)
        })
    })
    .await;
    glib::timeout_future(Duration::from_millis(350)).await;
    let shell2_no_autorestart = win
        .terminal_host
        .tab_snapshot(shell2_id)
        .is_some_and(|s| s["phase"].as_str() == Some("exited") && s["pid"].is_null());
    win.terminal_host.restart_tab_by_id(shell2_id);
    let shell2_restarted = wait_for(3000, || {
        win.terminal_host
            .tab_snapshot(shell2_id)
            .is_some_and(|s| s["phase"].as_str() == Some("running") && s["pid"].as_i64().is_some())
    })
    .await;
    let shell2_no_autorestart = shell2_no_autorestart && shell2_restarted;
    win.terminal_host.close_tab_by_id(shell2_id);
    win.terminal_host.close_tab_by_id(shell_id);
    let shell1_reaped = wait_for(2000, || shell_pid > 0 && !is_pid_alive(shell_pid)).await;
    let bottom_auto_collapsed_on_close = !win.is_bottom_panel_revealed();

    // -------------------------------------------------------------------------
    // Part 2: 启动闸门负面拦截测试（无有效 Agent 配置）
    // -------------------------------------------------------------------------
    let sessions_before_bad = services
        .core
        .list_agent_sessions(Some(vault_root.clone()))
        .map(|s| s.len())
        .unwrap_or(0);
    let bad_spec_err = services
        .core
        .agent_spec_with_path(None)
        .err()
        .map(|e| e.to_string());
    win.launch_agent_action();
    let alert_shown = wait_for(1500, || {
        win.state.borrow().last_alert_dialog.upgrade().is_some()
    })
    .await;
    if let Some(dlg) = win.state.borrow().last_alert_dialog.upgrade() {
        dlg.force_close();
    }
    win.state.borrow().last_alert_dialog.set(None);
    glib::timeout_future(Duration::from_millis(120)).await;
    let sessions_after_bad = services
        .core
        .list_agent_sessions(Some(vault_root.clone()))
        .map(|s| s.len())
        .unwrap_or(0);
    let unwritable_snapshot_blocked = {
        use std::os::unix::fs::PermissionsExt;
        let data_root = std::env::var("HORTUS_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| temp_dir.join("ro_data"));
        let sess_dir = data_root.join("agent-sessions");
        let _ = std::fs::create_dir_all(&data_root);
        let _ = std::fs::remove_dir_all(&sess_dir);
        let _ = std::fs::set_permissions(&data_root, std::fs::Permissions::from_mode(0o500));
        let blocked = services
            .core
            .begin_agent_session(vault_root.clone())
            .is_err();
        let _ = std::fs::set_permissions(&data_root, std::fs::Permissions::from_mode(0o700));
        blocked
    };
    let bad_gate_blocked = bad_spec_err.is_some()
        && alert_shown
        && sessions_after_bad == sessions_before_bad
        && unwritable_snapshot_blocked;

    // -------------------------------------------------------------------------
    // Part 3: 原生 PreferencesDialog 设置页 + 注释保留 + 冲突保护 + 热重载 + 指南
    // -------------------------------------------------------------------------
    let config_path = services.config.snapshot().config_path.clone();
    let mock_agent = std::env::var("HORTUS_L3_MOCK_AGENT").unwrap_or_default();

    win.open_settings_dialog(Some(SettingsPage::Agent));
    let dlg_tracked = wait_for(2000, || win.settings_dialog().is_some()).await;
    let dlg = win.settings_dialog().expect("settings dialog");
    let config_subtitle_resolved = wait_for(3000, || {
        let sub = dlg.config_path_subtitle();
        !sub.is_empty()
            && !sub.contains("Loading")
            && !sub.contains("读取")
            && sub.contains("config.toml")
    })
    .await;
    glib::timeout_future(Duration::from_millis(200)).await;
    let discovered_ids = dlg.discovered_ids();
    if let Some(first_id) = discovered_ids.first() {
        let _ = dlg.fill_discovered_agent(first_id);
    }
    dlg.set_agent(&mock_agent, &["{guide}".into()]);
    let probe_ok = wait_for(3000, || dlg.last_probe().is_some_and(|p| p.available)).await;
    glib::timeout_future(Duration::from_millis(200)).await;

    dlg.set_font_size(19.0);
    dlg.set_terminal_font("Monospace 13");
    dlg.set_default_view(true);
    dlg.save_config_now(false);
    let config_saved = wait_for(3000, || {
        let snap = services.config.snapshot();
        (snap.font_size - 19.0).abs() < 0.01
            && snap.default_view == "editing"
            && snap.terminal_font.as_deref() == Some("Monospace 13")
    })
    .await;
    let toml_after_save = std::fs::read_to_string(&config_path).unwrap_or_default();
    let comments_preserved = toml_after_save.contains("# L3-PRESERVED-COMMENT");

    let mut modified_toml = toml_after_save.replace("font_size = 19", "font_size = 17");
    if modified_toml == toml_after_save {
        modified_toml.push_str("\n# external bump\n");
    }
    let _ = std::fs::write(&config_path, &modified_toml);
    dlg.save_config_now(false);
    let conflict_detected = wait_for(2500, || dlg.last_conflict()).await;

    dlg.set_default_view(false);
    dlg.set_font_size(16.0);
    dlg.save_config_now(true);
    let force_saved = wait_for(2500, || {
        (services.config.snapshot().font_size - 16.0).abs() < 0.01
            && services.config.snapshot().default_view == "reading"
    })
    .await;

    dlg.set_guide_text("# L3 自定义 Agent 指南\n请遵循白名单约束。\n");
    dlg.save_guide_now();
    let guide_overridden = wait_for(2000, || {
        dlg.guide_overridden() && dlg.guide_text().contains("L3 自定义 Agent 指南")
    })
    .await;
    dlg.reset_guide_now();
    let guide_reset = wait_for(2000, || !dlg.guide_overridden()).await;
    dlg.close();
    drop(dlg);
    let dlg_cleared_after_close = wait_for(2500, || win.settings_dialog().is_none()).await;
    glib::timeout_future(Duration::from_millis(200)).await;

    let disk_edit = std::fs::read_to_string(&config_path)
        .unwrap_or_default()
        .replace("font_size = 16", "font_size = 18")
        .replace("default_view = \"reading\"", "default_view = \"editing\"");
    let _ = std::fs::write(&config_path, disk_edit);
    let monitor_hot_reloaded = wait_for(3000, || {
        let snap = services.config.snapshot();
        (snap.font_size - 18.0).abs() < 0.01 && snap.default_view == "editing"
    })
    .await;
    win.open_note_at("folder/sub note.md", None);
    let sub_opened_editing = wait_for(3000, || {
        win.state
            .borrow()
            .tabs
            .get("folder/sub note.md")
            .is_some_and(|t| t.source_loaded && t.editing)
    })
    .await;
    win.close_tab_by_path("folder/sub note.md");
    if let Ok(cur) = services.core.editable_config() {
        let _ = services.core.save_editable_config(
            EditableConfigInputInfo {
                font_size: 15.0,
                default_view: "reading".into(),
                terminal_font: cur.terminal_font,
                agent_command: Some(mock_agent.clone()),
                agent_args: vec!["{guide}".into()],
            },
            None,
        );
    }
    let _ = wait_for(2000, || {
        services.config.snapshot().default_view == "reading"
    })
    .await;
    // 等待前序 config.toml 写入引发的 120ms 监视防抖彻底落定，再单独测量主题切换刷新次数
    glib::timeout_future(Duration::from_millis(260)).await;

    // L5-12: 单次主题切换去重（先显式切到 Light 基线，避免宿主系统默认暗色导致 Dark 无变化）
    win.send_command(WindowCommand::ApplyColorScheme(
        crate::core::ColorSchemeChoice::Light,
    ));
    let _ = wait_for(2000, || !adw::StyleManager::default().is_dark()).await;
    glib::timeout_future(Duration::from_millis(260)).await;

    let refreshes_before_dark = win.state.borrow().theme_refreshes;
    win.send_command(WindowCommand::ApplyColorScheme(
        crate::core::ColorSchemeChoice::Dark,
    ));
    let _ = wait_for(2000, || {
        win.state.borrow().theme_refreshes > refreshes_before_dark
    })
    .await;
    glib::timeout_future(Duration::from_millis(180)).await;
    let refreshes_after_dark = win.state.borrow().theme_refreshes;
    let action_state_dark = win
        .window
        .lookup_action("color-scheme")
        .and_then(|a| a.state())
        .and_then(|v| v.str().map(str::to_string));
    let theme_switch_delta = refreshes_after_dark.saturating_sub(refreshes_before_dark);
    let theme_switch_single_refresh =
        theme_switch_delta == 1 && action_state_dark.as_deref() == Some("dark");
    win.send_command(WindowCommand::ApplyColorScheme(
        crate::core::ColorSchemeChoice::Light,
    ));
    let _ = wait_for(2000, || {
        win.state.borrow().theme_refreshes > refreshes_after_dark
    })
    .await;
    glib::timeout_future(Duration::from_millis(100)).await;

    // -------------------------------------------------------------------------
    // Part 4: 右侧栏直接启动 Agent + 快照闸门 + Changes 面板 + 红绿 Diff + 多窗口合成截图
    // -------------------------------------------------------------------------
    let _ = gtk4::prelude::WidgetExt::activate_action(
        &win.window,
        "win.dock-launch-agent",
        Some(&"right".to_variant()),
    );
    let agent_exited = wait_for(10000, || {
        win.changes_panel
            .sessions()
            .iter()
            .any(|s| s.state == "exited" && s.usable)
            && win
                .changes_panel
                .summary_snapshot()
                .is_some_and(|sum| sum.entries.len() >= 3)
    })
    .await;
    let right_agent_launched_ok =
        win.terminal_host.tab_ids().iter().any(|id| {
            win.tool_dock.tab_region(DockTabId::Terminal(*id)) == Some(PanelRegion::Right)
        });

    let agent_tab_ids = win.terminal_host.tab_ids();
    let agent_session_id = win
        .changes_panel
        .sessions()
        .first()
        .map(|s| s.id.clone())
        .unwrap_or_default();
    win.changes_panel.select_session(&agent_session_id);
    let _ = wait_for(3000, || {
        win.changes_panel
            .summary_snapshot()
            .is_some_and(|sum| sum.session_id == agent_session_id && sum.entries.len() >= 3)
    })
    .await;
    win.activate_static_tool(StaticToolKind::Changes);
    let selected_ok = win.changes_panel.select_entry("multi-hunk.md");
    let detail_loaded = wait_for(4000, || {
        win.changes_panel
            .detail_snapshot()
            .is_some_and(|d| d.entry.id == "multi-hunk.md" && d.hunks.len() >= 2)
    })
    .await;
    glib::timeout_future(Duration::from_millis(300)).await;
    let changes_title = win.changes_panel.title_text();

    // 截图 3 (v3): 通过真实右键菜单将「改动」移入底部面板并验证 diff/回滚界面完好
    let changes_live_ctx_open = win.tool_dock.open_tab_context_menu(changes_id);
    let _ = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let changes_live_ctx_move_bottom = win.tool_dock.activate_context_menu_item(&move_bottom_lbl);
    let changes_live_moved_bottom = wait_for(2000, || {
        win.tool_dock.tab_region(changes_id) == Some(PanelRegion::Bottom)
    })
    .await;
    glib::timeout_future(Duration::from_millis(220)).await;
    let changes_detail_after_ctx_move = win
        .changes_panel
        .detail_snapshot()
        .is_some_and(|d| d.entry.id == "multi-hunk.md" && d.hunks.len() >= 2);
    let shot_changes_context_moved = if let Some(dir) = media_dir {
        let p = dir.join("l3-v3-changes-context-menu-moved.png");
        capture_widget_png(&win.window, &p).is_ok() && p.exists()
    } else {
        false
    };
    let changes_live_ctx_open2 = win.tool_dock.open_tab_context_menu(changes_id);
    let _ = wait_for(1500, || {
        win.tool_dock
            .active_context_menu_popover()
            .is_some_and(|(_, p)| p.is_visible())
    })
    .await;
    let changes_live_ctx_move_right = win.tool_dock.activate_context_menu_item(&move_right_lbl);
    let changes_live_moved_back_right = wait_for(2000, || {
        win.tool_dock.tab_region(changes_id) == Some(PanelRegion::Right)
    })
    .await;
    let changes_ctx_with_diff_ok = changes_live_ctx_open
        && changes_live_ctx_move_bottom
        && changes_live_moved_bottom
        && changes_detail_after_ctx_move
        && changes_live_ctx_open2
        && changes_live_ctx_move_right
        && changes_live_moved_back_right;
    glib::timeout_future(Duration::from_millis(150)).await;

    // -------------------------------------------------------------------------
    // Part 5: 多窗口同步（含 Agent 改动 + 双窗口同库同步合成截图）
    // -------------------------------------------------------------------------
    win.open_note_at("alpha.md", None);
    let mut multi_win_sync_ok = false;
    let mut same_services_ptr = false;
    let mut same_vault_ctx_ptr = false;
    let mut live_two_windows = false;
    let mut isolated_vault_ok = false;
    let mut win2_closed_clean = false;
    let mut win2_pty_reaped = false;
    let mut dirty_missing_preserved_across_windows = false;
    let mut shot_agent_changes_multi = false;

    if let Some(win2) = win.open_new_window_with_vault(None) {
        let win2_ready = wait_for(5000, || {
            let st = win2.inner.state.borrow();
            st.services.is_some() && st.vault_ctx.is_some() && st.scan_complete
        })
        .await;
        if win2_ready {
            same_services_ptr = {
                let s1 = win.state.borrow().services.as_ref().map(Arc::as_ptr);
                let s2 = win2.inner.state.borrow().services.as_ref().map(Arc::as_ptr);
                s1.is_some() && s1 == s2
            };
            same_vault_ctx_ptr = {
                let c1 = win.state.borrow().vault_ctx.as_ref().map(Arc::as_ptr);
                let c2 = win2
                    .inner
                    .state
                    .borrow()
                    .vault_ctx
                    .as_ref()
                    .map(Arc::as_ptr);
                c1.is_some() && c1 == c2
            };
            live_two_windows = services.windows.live_count() == 2;

            win2.inner.open_note_at("alpha.md", None);
            let _ = wait_for(3000, || {
                win2.inner
                    .state
                    .borrow()
                    .tabs
                    .get("alpha.md")
                    .is_some_and(|t| t.source_loaded && t.reading_loaded)
            })
            .await;

            win.open_note_at("alpha.md", None);
            win.switch_edit_mode(false);
            if let Some(buf) = win
                .state
                .borrow()
                .tabs
                .get("alpha.md")
                .map(|t| t.edit_buffer.clone())
            {
                let cur = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();
                buf.set_text(&format!("{cur}\nMULTI_WINDOW_SYNC_OK\n"));
            }
            win.send_command(WindowCommand::SaveNote {
                path: "alpha.md".into(),
                choice: hortus_ffi::SaveChoice::Normal,
                announce: false,
            });
            multi_win_sync_ok = wait_for(4000, || {
                win2.inner
                    .state
                    .borrow()
                    .tabs
                    .get("alpha.md")
                    .is_some_and(|t| t.text.contains("MULTI_WINDOW_SYNC_OK"))
            })
            .await;
            win.switch_edit_mode(false);
            glib::timeout_future(Duration::from_millis(300)).await;

            // 截图 4: Agent 退出后「本次会话改动」红绿 diff + 双窗口同库同步
            if let Some(dir) = media_dir {
                let p = dir.join("l3-v3-agent-changes-and-multi-window.png");
                shot_agent_changes_multi =
                    capture_dual_window_png(&win.window, &win2.inner.window, &p).is_ok()
                        && p.exists();
            }

            // L5-02 + L5-03: 双窗口同库外部删除未保存页不触发 RefCell 重入，且保留脏页标示 Missing
            let sub_rel = "folder/sub note.md";
            let sub_disk = vault_path.join(sub_rel);
            let sub_backup = std::fs::read_to_string(&sub_disk).unwrap_or_default();
            win2.inner.open_note_at(sub_rel, None);
            let sub_loaded_w2 = wait_for(4000, || {
                win2.inner
                    .state
                    .borrow()
                    .tabs
                    .get(sub_rel)
                    .is_some_and(|t| t.source_loaded)
            })
            .await;
            if sub_loaded_w2 {
                if !win2
                    .inner
                    .state
                    .borrow()
                    .tabs
                    .get(sub_rel)
                    .is_some_and(|t| t.editing)
                {
                    win2.inner.toggle_active_tab_edit_mode();
                }
                if let Some(buf) = win2
                    .inner
                    .state
                    .borrow()
                    .tabs
                    .get(sub_rel)
                    .map(|t| t.edit_buffer.clone())
                {
                    let cur = buf
                        .text(&buf.start_iter(), &buf.end_iter(), false)
                        .to_string();
                    buf.set_text(&format!("{cur}\nUNSAVED_IN_WIN2\n"));
                }
            }
            let _ = std::fs::remove_file(&sub_disk);
            let src_win_id = win.state.borrow().window_reg_id;
            services
                .windows
                .notify_vault_written(src_win_id, &vault_root, &[sub_rel.to_string()]);
            dirty_missing_preserved_across_windows = wait_for(4000, || {
                win2.inner
                    .state
                    .borrow()
                    .tabs
                    .get(sub_rel)
                    .is_some_and(|t| {
                        let edit = t.edit_state.borrow();
                        edit.issue == Some(hortus_ffi::DocumentIssue::Missing)
                            && edit.dirty
                            && edit.text.contains("UNSAVED_IN_WIN2")
                    })
            })
            .await;
            let _ = std::fs::write(&sub_disk, sub_backup);

            if let Some(v2) = vault2_opt {
                win2.inner.open_vault_path(v2.to_path_buf());
                let v2_ready = wait_for(4000, || {
                    win2.inner.state.borrow().scan_complete
                        && win2.inner.state.borrow().vault_root().as_deref()
                            == Some(v2.to_string_lossy().as_ref())
                })
                .await;
                win2.inner.set_bottom_panel_revealed(true);
                let win2_term_ok = wait_for(3000, || {
                    !win2.inner.terminal_host.all_child_pids().is_empty()
                })
                .await;
                let win2_term_dir = win2
                    .inner
                    .terminal_host
                    .tab_ids()
                    .first()
                    .and_then(|id| win2.inner.terminal_host.tab_snapshot(*id))
                    .and_then(|s| s["working_dir"].as_str().map(str::to_string))
                    .unwrap_or_default();
                isolated_vault_ok = v2_ready
                    && win2_term_ok
                    && win2_term_dir == v2.to_string_lossy()
                    && win.state.borrow().vault_root().as_deref() == Some(vault_root.as_str())
                    && win2.inner.changes_panel.sessions().is_empty();
            }

            let win2_pids = win2.inner.terminal_host.all_child_pids();
            let weak2 = win2.downgrade();
            win2.inner.close_now_clean();
            drop(win2);
            win2_closed_clean = wait_for(2500, || {
                weak2.strong_count() == 0 && services.windows.live_count() == 1
            })
            .await;
            win2_pty_reaped =
                wait_for(2000, || win2_pids.iter().all(|pid| !is_pid_alive(*pid))).await;
        }
    }

    // 执行三级安全回滚（单 hunk -> review_stale -> 单文件 -> 整会话 -> 删除快照）
    let hunk_rb_started = win.changes_panel.rollback_hunk_now(0);
    let hunk_rb_ok = wait_for(4000, || {
        win.changes_panel.last_rollback_report().as_deref() == Some("reverted")
            && std::fs::read_to_string(vault_path.join("multi-hunk.md"))
                .is_ok_and(|c| c.contains("TOP_ORIGINAL") && c.contains("BOT_AGENT_EDIT"))
    })
    .await;
    let _ = wait_for(3000, || {
        win.changes_panel
            .summary_snapshot()
            .is_some_and(|s| s.entries.iter().any(|e| e.id == "multi-hunk.md"))
    })
    .await;

    let multi_path = vault_path.join("multi-hunk.md");
    let before_stale = std::fs::read_to_string(&multi_path).unwrap_or_default();
    let _ = std::fs::write(
        &multi_path,
        format!("{before_stale}\nSTALE_EXTERNAL_MARKER\n"),
    );
    let _ = win.changes_panel.rollback_entry_now("multi-hunk.md");
    let stale_blocked = wait_for(3000, || {
        win.changes_panel.last_error().as_deref() == Some("history.review_stale")
    })
    .await;
    let stale_untouched =
        std::fs::read_to_string(&multi_path).is_ok_and(|c| c.contains("STALE_EXTERNAL_MARKER"));
    let _ = std::fs::write(&multi_path, &before_stale);
    win.changes_panel.refresh();
    let _ = wait_for(3000, || {
        win.changes_panel
            .summary_snapshot()
            .is_some_and(|s| s.entries.iter().any(|e| e.id == "agent-created.md"))
    })
    .await;

    let _ = win.changes_panel.rollback_entry_now("agent-created.md");
    let entry_rb_ok = wait_for(3500, || !vault_path.join("agent-created.md").exists()).await;
    let _ = wait_for(3000, || {
        win.changes_panel
            .summary_snapshot()
            .is_some_and(|s| !s.entries.iter().any(|e| e.id == "agent-created.md"))
    })
    .await;

    let _ = win.changes_panel.rollback_session_now();
    let session_rb_ok = wait_for(4000, || {
        vault_path.join("to-delete.md").exists()
            && std::fs::read_to_string(&multi_path)
                .is_ok_and(|c| c.contains("TOP_ORIGINAL") && c.contains("BOT_ORIGINAL"))
    })
    .await;

    win.changes_panel.delete_session_now(&agent_session_id);
    let session_deleted = wait_for(3000, || {
        !win.changes_panel
            .sessions()
            .iter()
            .any(|s| s.id == agent_session_id)
    })
    .await;
    let vault_intact_after_delete = vault_path.join("to-delete.md").exists();
    for id in agent_tab_ids {
        win.terminal_host.close_tab_by_id(id);
    }

    // L5-11: 手动关闭仍在运行的 Agent 终端标签页应收敛会话为 interrupted 并可删除
    if let Ok(cur) = services.core.editable_config() {
        let _ = services.core.save_editable_config(
            EditableConfigInputInfo {
                font_size: cur.font_size,
                default_view: cur.default_view,
                terminal_font: cur.terminal_font,
                agent_command: Some(mock_agent.clone()),
                agent_args: vec!["--sleep".into(), "{guide}".into()],
            },
            None,
        );
    }
    let _ = wait_for(2000, || {
        services
            .config
            .snapshot()
            .agent_args
            .first()
            .is_some_and(|a| a == "--sleep")
    })
    .await;
    let tabs_before_sleep: std::collections::HashSet<u64> =
        win.terminal_host.tab_ids().into_iter().collect();
    let sessions_before_sleep: std::collections::HashSet<String> = win
        .changes_panel
        .sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    win.launch_agent_action();
    let sleep_running = wait_for(5000, || {
        win.changes_panel
            .sessions()
            .iter()
            .any(|s| !sessions_before_sleep.contains(&s.id) && s.state == "running")
            && win
                .terminal_host
                .tab_ids()
                .into_iter()
                .any(|id| !tabs_before_sleep.contains(&id))
    })
    .await;
    let sleep_sid = win
        .changes_panel
        .sessions()
        .into_iter()
        .find(|s| !sessions_before_sleep.contains(&s.id))
        .map(|s| s.id.clone())
        .unwrap_or_default();
    if let Some(sleep_tid) = win
        .terminal_host
        .tab_ids()
        .into_iter()
        .find(|id| !tabs_before_sleep.contains(id))
    {
        win.terminal_host.close_tab_by_id(sleep_tid);
    }
    let running_agent_close_interrupted = sleep_running
        && !sleep_sid.is_empty()
        && wait_for(5000, || {
            win.changes_panel
                .sessions()
                .iter()
                .any(|s| s.id == sleep_sid && s.state != "running" && s.usable)
        })
        .await;
    if !sleep_sid.is_empty() {
        win.changes_panel.delete_session_now(&sleep_sid);
        let _ = wait_for(3000, || {
            !win.changes_panel
                .sessions()
                .iter()
                .any(|s| s.id == sleep_sid)
        })
        .await;
    }
    if let Ok(cur) = services.core.editable_config() {
        let _ = services.core.save_editable_config(
            EditableConfigInputInfo {
                font_size: cur.font_size,
                default_view: cur.default_view,
                terminal_font: cur.terminal_font,
                agent_command: Some(mock_agent.clone()),
                agent_args: vec!["{guide}".into()],
            },
            None,
        );
    }

    // -------------------------------------------------------------------------
    // Part 6 (L4c): 左右边栏平滑拖拽调宽、互不挤压、ToolDock/阅读区自适应与持久化恢复
    // -------------------------------------------------------------------------
    win.left_split.set_show_sidebar(true);
    win.right_split.set_show_sidebar(true);
    win.activate_static_tool(StaticToolKind::Outline);
    win.open_note_at("alpha.md", None);
    let _ = wait_for(2000, || {
        win.state
            .borrow()
            .tabs
            .get("alpha.md")
            .is_some_and(|t| t.reading_loaded && !t.editing)
    })
    .await;

    let handles_ready = win.left_resize_handle.width_request() == 6
        && win.right_resize_handle.width_request() == 6
        && win.left_resize_handle.halign() == gtk4::Align::End
        && win.right_resize_handle.halign() == gtk4::Align::Start
        && win.left_resize_handle.cursor().is_some()
        && win.right_resize_handle.cursor().is_some();

    win.apply_sidebar_widths(260.0, 220.0);
    let _ = wait_for(2000, || {
        (win.current_sidebar_width(PanelRegion::Left) - 260.0).abs() <= 4.0
            && (win.current_sidebar_width(PanelRegion::Right) - 220.0).abs() <= 4.0
    })
    .await;
    glib::timeout_future(Duration::from_millis(220)).await;
    let left_w_before = win.current_sidebar_width(PanelRegion::Left);
    let right_w_before = win.current_sidebar_width(PanelRegion::Right);
    let center_w_before = win
        .state
        .borrow()
        .tabs
        .get("alpha.md")
        .map(|t| t.reading_viewport.last_allocated_width())
        .unwrap_or(0);
    let right_strip_narrow = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Right);
    let right_narrow_icon_only_ok = right_strip_narrow.as_ref().is_some_and(|m| {
        m.tabs.len() == 3
            && m.tabs.iter().any(|t| t.selected && t.label_visible)
            && m.tabs.iter().any(|t| !t.selected && !t.label_visible)
    });

    // 拖拽左侧栏 260px -> 380px，验证左侧变宽且右侧不跟着缩水
    let left_drag_ok = win.simulate_sidebar_drag_window_delta(PanelRegion::Left, 120.0);
    let _ = wait_for(2000, || {
        (win.current_sidebar_width(PanelRegion::Left) - 380.0).abs() <= 4.0
    })
    .await;
    glib::timeout_future(Duration::from_millis(180)).await;
    let left_w_after_left_drag = win.current_sidebar_width(PanelRegion::Left);
    let right_w_after_left_drag = win.current_sidebar_width(PanelRegion::Right);
    let left_drag_no_right_drift = left_drag_ok
        && (left_w_after_left_drag - 380.0).abs() <= 4.0
        && (right_w_after_left_drag - right_w_before).abs() <= 4.0;

    // 拖拽右侧栏 220px -> 360px（左边缘向左拖 -140px），验证非激活标签自动恢复全文字且正文宽度自适应
    let right_drag_ok = win.simulate_sidebar_drag_window_delta(PanelRegion::Right, -140.0);
    let _ = wait_for(3000, || {
        let rw_ok = (win.current_sidebar_width(PanelRegion::Right) - 360.0).abs() <= 4.0;
        let vp_ok = win.state.borrow().tabs.get("alpha.md").is_some_and(|t| {
            let vp_w = f64::from(t.reading_viewport.last_allocated_width()).max(260.0);
            t.reading_viewport.last_allocated_width() < center_w_before - 180
                && (t.layout_width - vp_w).abs() < 24.0
        });
        rw_ok && vp_ok
    })
    .await;
    glib::timeout_future(Duration::from_millis(220)).await;
    let left_w_after_right_drag = win.current_sidebar_width(PanelRegion::Left);
    let right_w_after_right_drag = win.current_sidebar_width(PanelRegion::Right);
    let center_w_after_right_drag = win
        .state
        .borrow()
        .tabs
        .get("alpha.md")
        .map(|t| t.reading_viewport.last_allocated_width())
        .unwrap_or(0);
    let right_strip_wide = win.tool_dock.measure_strip_tab_bounds(PanelRegion::Right);
    let right_wide_restored_labels_ok = right_strip_wide.as_ref().is_some_and(|m| {
        m.tabs.len() == 3
            && m.tabs
                .iter()
                .all(|t| t.fits_in_strip && t.label_visible && !t.ellipsized)
    });
    let center_adapted_ok = center_w_after_right_drag < center_w_before - 180;
    let shot_sidebars_resized_wide = if let Some(dir) = media_dir {
        let p = dir.join("l4-sidebars-resized-wide.png");
        capture_widget_png(&win.window, &p).is_ok() && p.exists()
    } else {
        false
    };

    // 持久化到 layout.toml（含显隐 visible）并开新窗口验证真实启动恢复
    win.left_split.set_show_sidebar(false);
    win.persist_layout_now();
    let layout_persisted_ok = wait_for(3000, || {
        layout_toml_path.exists()
            && services
                .core
                .layout_json()
                .ok()
                .flatten()
                .and_then(|json| serde_json::from_str::<hortus_config::LayoutState>(&json).ok())
                .is_some_and(|st| {
                    let l = st.regions.get("left").map(|r| r.size).unwrap_or(0.0);
                    let r = st.regions.get("right").map(|r| r.size).unwrap_or(0.0);
                    let lv = st.regions.get("left").and_then(|r| r.visible);
                    let rv = st.regions.get("right").and_then(|r| r.visible);
                    (l - 380.0).abs() <= 4.0
                        && (r - 360.0).abs() <= 4.0
                        && lv == Some(false)
                        && rv == Some(true)
                })
    })
    .await;

    // L5-20: 主题偏好持久化到 linux-ui.toml
    win.apply_color_scheme_and_sync(crate::core::ColorSchemeChoice::Dark);
    let ui_prefs_persisted_ok = crate::services::ui_prefs::load_saved_color_scheme()
        == crate::core::ColorSchemeChoice::Dark;

    let mut layout_restored_ok = false;
    let mut layout_visibility_restored_ok = false;
    if let Some(app) = win
        .window
        .application()
        .and_then(|a| a.downcast::<adw::Application>().ok())
    {
        let opts_restore = CliOptions {
            vault: Some(vault_path.clone()),
            width: Some(1280),
            height: Some(800),
            ..Default::default()
        };
        let win_restore =
            HortusWindow::new_with_services(&app, opts_restore, Some(Arc::clone(&services)));
        win_restore.present();
        layout_restored_ok = wait_for(4000, || {
            let st = win_restore.inner.state.borrow();
            st.scan_complete
                && (win_restore.inner.current_sidebar_width(PanelRegion::Left) - 380.0).abs() <= 4.0
                && (win_restore.inner.current_sidebar_width(PanelRegion::Right) - 360.0).abs()
                    <= 4.0
        })
        .await;
        layout_visibility_restored_ok = !win_restore.inner.left_split.shows_sidebar()
            && win_restore.inner.right_split.shows_sidebar();
        win_restore.inner.close_now_clean();
        drop(win_restore);
    }
    win.left_split.set_show_sidebar(true);
    win.apply_color_scheme_and_sync(crate::core::ColorSchemeChoice::System);

    win.reset_sidebar_width(PanelRegion::Left);
    win.reset_sidebar_width(PanelRegion::Right);
    let double_click_reset_ok = (win.state.borrow().left_sidebar_width - 280.0).abs() < 0.1
        && (win.state.borrow().right_sidebar_width - 280.0).abs() < 0.1;
    win.persist_layout_now();
    glib::timeout_future(Duration::from_millis(160)).await;

    if let Some(entry) = win.build_session_entry() {
        crate::window::session::write_session_entries_to_core(&services.core, &[entry]);
    }

    serde_json::json!({
        "ok": true,
        "dock_v2": {
            "single_bar_left": single_bar_left,
            "single_bar_right": single_bar_right,
            "single_bar_bottom": single_bar_bottom,
            "all_regions_have_add": all_regions_have_add,
            "sidebar_max_480_ok": sidebar_max_480_ok,
            "bottom_single_bar_titles_ok": bottom_single_bar_titles_ok,
            "paned_resizable_ok": paned_resizable_ok,
            "ctx_menu_bottom_ok": ctx_menu_bottom_ok,
            "lossless_3_region_migration": lossless_3_region_migration,
            "tags_closed": tags_closed,
            "add_menu_items_ok": add_menu_items_ok,
            "tags_reopened_right": tags_reopened_right,
            "layout_toml_ok": layout_toml_ok,
            "right_shell_spawned": right_shell_spawned,
            "right_agent_launched_ok": right_agent_launched_ok,
            "bottom_auto_collapsed_on_close": bottom_auto_collapsed_on_close,
        },
        "dock_v3": {
            "left_3tabs_fit": left_3tabs_fit,
            "right_3tabs_fit": right_3tabs_fit,
            "bottom_tab_compact": bottom_tab_compact,
            "right_4tabs_fit": right_4tabs_fit,
            "no_inline_close_buttons": no_inline_close_buttons,
            "changes_real_context_menu_ok": changes_real_context_menu_ok,
            "changes_ctx_with_diff_ok": changes_ctx_with_diff_ok,
            "left_strip_3": left_strip_3,
            "right_strip_3": right_strip_3,
            "bottom_strip_1": bottom_strip_1,
            "right_strip_4": right_strip_4,
        },
        "terminal": {
            "alpha_loaded": alpha_loaded,
            "shell_ready": shell_ready,
            "probe_done": probe_done,
            "pwd_matches": pwd_matches,
            "lang_utf8": lang_utf8,
            "hortus_found": hortus_found,
            "pty_refreshed": pty_refreshed,
            "quoted_space_path": quoted_space_path,
            "spoof_blocked": spoof_blocked,
            "shell2_spawned": shell2_spawned,
            "shell2_exited": shell2_exited,
            "shell2_no_autorestart": shell2_no_autorestart,
            "shell1_reaped": shell1_reaped,
        },
        "settings": {
            "bad_gate_blocked": bad_gate_blocked,
            "dlg_tracked": dlg_tracked,
            "config_subtitle_resolved": config_subtitle_resolved,
            "dlg_cleared_after_close": dlg_cleared_after_close,
            "theme_switch_single_refresh": theme_switch_single_refresh,
            "theme_switch_delta": theme_switch_delta,
            "theme_action_state_dark": action_state_dark,
            "probe_ok": probe_ok,
            "discovered_ids": discovered_ids,
            "config_saved": config_saved,
            "comments_preserved": comments_preserved,
            "conflict_detected": conflict_detected,
            "force_saved": force_saved,
            "guide_overridden": guide_overridden,
            "guide_reset": guide_reset,
            "monitor_hot_reloaded": monitor_hot_reloaded,
            "sub_opened_editing": sub_opened_editing,
        },
        "agent_changes": {
            "agent_exited": agent_exited,
            "running_agent_close_interrupted": running_agent_close_interrupted,
            "selected_ok": selected_ok,
            "detail_loaded": detail_loaded,
            "changes_title": changes_title,
            "hunk_rb_started": hunk_rb_started,
            "hunk_rb_ok": hunk_rb_ok,
            "stale_blocked": stale_blocked,
            "stale_untouched": stale_untouched,
            "entry_rb_ok": entry_rb_ok,
            "session_rb_ok": session_rb_ok,
            "session_deleted": session_deleted,
            "vault_intact_after_delete": vault_intact_after_delete,
        },
        "multi_window": {
            "same_services_ptr": same_services_ptr,
            "same_vault_ctx_ptr": same_vault_ctx_ptr,
            "live_two_windows": live_two_windows,
            "multi_win_sync_ok": multi_win_sync_ok,
            "dirty_missing_preserved_across_windows": dirty_missing_preserved_across_windows,
            "isolated_vault_ok": isolated_vault_ok,
            "win2_closed_clean": win2_closed_clean,
            "win2_pty_reaped": win2_pty_reaped,
        },
        "sidebar_resize": {
            "handles_ready": handles_ready,
            "right_narrow_icon_only_ok": right_narrow_icon_only_ok,
            "left_drag_no_right_drift": left_drag_no_right_drift,
            "right_drag_ok": right_drag_ok,
            "right_wide_restored_labels_ok": right_wide_restored_labels_ok,
            "center_adapted_ok": center_adapted_ok,
            "layout_persisted_ok": layout_persisted_ok,
            "layout_restored_ok": layout_restored_ok,
            "layout_visibility_restored_ok": layout_visibility_restored_ok,
            "ui_prefs_persisted_ok": ui_prefs_persisted_ok,
            "double_click_reset_ok": double_click_reset_ok,
            "left_w_before": left_w_before,
            "right_w_before": right_w_before,
            "left_w_after_left_drag": left_w_after_left_drag,
            "right_w_after_left_drag": right_w_after_left_drag,
            "left_w_after_right_drag": left_w_after_right_drag,
            "right_w_after_right_drag": right_w_after_right_drag,
            "center_w_before": center_w_before,
            "center_w_after_right_drag": center_w_after_right_drag,
            "right_strip_narrow": right_strip_narrow,
            "right_strip_wide": right_strip_wide,
        },
        "screenshots": {
            "dynamic_sidebar": shot_dynamic_sidebar,
            "changes_context_moved": shot_changes_context_moved,
            "right_sidebar_terminal": shot_right_sidebar_terminal,
            "tool_add_menu": shot_tool_add_menu,
            "agent_changes_multi": shot_agent_changes_multi,
            "sidebars_resized_wide": shot_sidebars_resized_wide,
        },
    })
}

pub fn execute_session_restore_check(app: &adw::Application, opts: CliOptions) {
    let Some(out_path) = opts.restore_session_json.clone() else {
        app.quit();
        return;
    };
    let Ok((services, _)) = AppServices::init_blocking() else {
        let _ = std::fs::write(&out_path, r#"{"ok":false,"error":"init_failed"}"#);
        app.quit();
        return;
    };
    services.ensure_dark_listener();
    services.config.ensure_monitor_and_css();

    if opts.color_scheme.is_none() {
        crate::window::actions::apply_color_scheme(
            crate::services::ui_prefs::load_saved_color_scheme(),
        );
    }

    let entries = parse_restorable_entries(&services.core);
    for entry in &entries {
        crate::window::shell::bump_next_window_id(&entry.id);
    }
    let mut windows: Vec<HortusWindow> = Vec::new();
    for entry in &entries {
        let vault_path = PathBuf::from(&entry.path);
        let win_opts = CliOptions {
            vault: Some(vault_path.clone()),
            ..Default::default()
        };
        let valid_tabs: Vec<String> = entry
            .tabs
            .iter()
            .filter(|rel| vault_path.join(rel).is_file())
            .cloned()
            .collect();
        let selected = entry
            .selected
            .clone()
            .filter(|sel| valid_tabs.iter().any(|t| t == sel));
        let win = HortusWindow::new_with_services(app, win_opts, Some(Arc::clone(&services)));
        {
            let mut st = win.inner.state.borrow_mut();
            st.window_id = entry.id.clone();
            st.initial_tabs_override = Some(RestoredTabsOverride {
                tabs: valid_tabs,
                selected,
                views: entry.views.clone(),
            });
        }
        win.present();
        windows.push(win);
    }

    if windows.is_empty() {
        launch_initial_windows(app, CliOptions::default());
        app.quit();
        return;
    }

    let app_clone = app.clone();
    glib::MainContext::default().spawn_local(async move {
        let settled = wait_for(6000, || {
            windows.iter().all(|w| {
                w.inner.state.borrow().scan_complete && !w.inner.state.borrow().tabs.is_empty()
            })
        })
        .await;
        let report: Vec<serde_json::Value> = windows
            .iter()
            .map(|w| {
                let st = w.inner.state.borrow();
                let tab_names: Vec<String> = st.tabs.keys().cloned().collect();
                serde_json::json!({
                    "vault": st.vault_root(),
                    "tabs": tab_names,
                    "terminal_tabs": w.inner.terminal_host.tab_count(),
                })
            })
            .collect();
        let zero_pty = windows
            .iter()
            .all(|w| w.inner.terminal_host.tab_count() == 0);
        let payload = serde_json::json!({
            "ok": settled && zero_pty,
            "restored_count": windows.len(),
            "zero_pty_restored": zero_pty,
            "windows": report,
        });
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &out_path,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
        );
        for w in windows {
            w.inner.close_now_clean();
        }
        app_clone.quit();
    });
}
