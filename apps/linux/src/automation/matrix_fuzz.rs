// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! Layer 1 全库矩阵不变量巡检 (`--matrix-audit-json`) 与 Layer 2 确定性有种子 GUI Fuzzer (`--fuzz-json`)。

use crate::automation::driver::wait_for;
use crate::core::ColorSchemeChoice;
use crate::platform::{READING_COLUMN_MAX_WIDTH, effective_reading_column_width};
use crate::window::shell::WindowInner;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound <= 1 {
            return 0;
        }
        (self.next_u64() % (bound as u64)) as usize
    }

    fn next_i32_range(&mut self, lo: i32, hi_inclusive: i32) -> i32 {
        if hi_inclusive <= lo {
            return lo;
        }
        let span = (hi_inclusive - lo + 1) as u64;
        lo + (self.next_u64() % span) as i32
    }
}

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

fn collect_markdown_notes(vault_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![vault_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                && let Ok(rel) = path.strip_prefix(vault_root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

async fn resize_window_to(win: &Rc<WindowInner>, w: i32, h: i32) -> bool {
    win.window.set_resizable(false);
    win.window.set_size_request(w, h);
    win.window.set_default_size(w, h);
    win.window.queue_resize();
    let reached = wait_for(2500, || {
        (win.window.width() - w).abs() <= 16 && (win.window.height() - h).abs() <= 16
    })
    .await;
    glib::timeout_future(Duration::from_millis(80)).await;
    reached
}

async fn ensure_note_open_and_ready(win: &Rc<WindowInner>, relpath: &str) -> bool {
    win.open_note_at(relpath, None);
    let loaded = wait_for(12000, || {
        let st = win.state.borrow();
        win.active_tab_path().as_deref() == Some(relpath)
            && st
                .tabs
                .get(relpath)
                .is_some_and(|tab| tab.source_loaded && tab.reading_loaded)
    })
    .await;
    if !loaded {
        return false;
    }
    let _ = wait_for(3000, || {
        win.state.borrow().tabs.get(relpath).is_some_and(|tab| {
            if tab.editing {
                tab.edit_host.width() > 0 && tab.edit_view.width() > 0
            } else {
                tab.reading_viewport.width() > 0
            }
        })
    })
    .await;
    glib::timeout_future(Duration::from_millis(50)).await;
    true
}

fn expected_edit_side_offset(host_w: i32) -> i32 {
    let (_, raw) = effective_reading_column_width(host_w, Some(READING_COLUMN_MAX_WIDTH));
    let max_side = (host_w - 120).max(0) / 2;
    raw.min(max_side)
}

fn check_pango_font_size(edit_view: &gtk4::TextView, configured_px: f64) -> Result<f64, String> {
    let pango_ctx = edit_view.pango_context();
    let Some(desc) = pango_ctx.font_description() else {
        return Err("edit_view 缺少 Pango FontDescription".into());
    };
    let raw = f64::from(desc.size()) / f64::from(pango::SCALE);
    let actual_px = if desc.is_size_absolute() {
        raw
    } else {
        raw * 96.0 / 72.0
    };
    if !actual_px.is_finite() || (actual_px - configured_px).abs() > 2.5 {
        return Err(format!(
            "edit_view Pango 字体大小异常：actual={actual_px:.2}px configured={configured_px:.2}px"
        ));
    }
    let buf = edit_view.buffer();
    let start = buf.start_iter();
    let (_, line_h) = edit_view.line_yrange(&start);
    if line_h <= 0 || line_h > (configured_px * 3.5).round() as i32 {
        return Err(format!(
            "edit_view 首行高度异常：line_h={line_h}px (font={configured_px:.1}px)"
        ));
    }
    Ok(actual_px)
}

fn check_vadjustment_finite(
    adj: &gtk4::Adjustment,
    label: &str,
) -> Result<(f64, f64, f64), String> {
    let v = adj.value();
    let lower = adj.lower();
    let upper = adj.upper();
    let page = adj.page_size();
    if !v.is_finite() || !lower.is_finite() || !upper.is_finite() || !page.is_finite() {
        return Err(format!(
            "{label} vadjustment 出现非有限值：val={v} lower={lower} upper={upper} page={page}"
        ));
    }
    if upper < page - 1.5 {
        return Err(format!(
            "{label} vadjustment upper < page_size：upper={upper:.2} page={page:.2}"
        ));
    }
    let max_v = (upper - page).max(lower);
    if v < lower - 1.5 || v > max_v + 1.5 {
        return Err(format!(
            "{label} vadjustment value 越界：val={v:.2} range=[{lower:.2}, {max_v:.2}]"
        ));
    }
    Ok((v, upper, page))
}

fn find_first_h1_char_offset(text: &str) -> Option<usize> {
    let mut char_pos = 0usize;
    let mut in_frontmatter = false;
    let mut seen_first = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !seen_first {
            seen_first = true;
            if trimmed == "---" {
                in_frontmatter = true;
                char_pos += line.chars().count();
                continue;
            }
        } else if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            char_pos += line.chars().count();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ")
            && !rest.trim().is_empty()
        {
            return Some(char_pos + 2);
        }
        char_pos += line.chars().count();
    }
    None
}

fn find_first_quote_or_callout_line_char(text: &str) -> Option<usize> {
    let mut char_pos = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with("> ") {
            return Some(char_pos + 2);
        }
        char_pos += line.chars().count();
    }
    None
}

fn check_edit_line_within_column(
    edit_view: &gtk4::TextView,
    char_off: usize,
    left_m: i32,
    col_w: i32,
    label: &str,
) -> Result<(), String> {
    let buf = edit_view.buffer();
    let clamped = (char_off as i32).clamp(0, buf.char_count());
    let iter = buf.iter_at_offset(clamped);
    let loc = edit_view.iter_location(&iter);
    let (wx, _) = edit_view.buffer_to_window_coords(gtk4::TextWindowType::Widget, loc.x(), loc.y());
    if wx < left_m - 2 || wx > left_m + col_w + 2 {
        return Err(format!(
            "{label} 行位置越出居中列：wx={wx} expected=[{}, {}]",
            left_m,
            left_m + col_w
        ));
    }
    Ok(())
}

pub fn execute_matrix_audit(win: &Rc<WindowInner>, report_path: PathBuf) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_matrix_audit(&this).await;
        write_json_atomic(&report_path, &report);
        this.close_now_clean();
    });
}

#[allow(clippy::too_many_lines)]
async fn run_matrix_audit(win: &Rc<WindowInner>) -> serde_json::Value {
    win.window.present();
    let vault_root = win.state.borrow().vault_root().unwrap_or_default();
    let configured_font = win
        .state
        .borrow()
        .services
        .as_ref()
        .map(|s| s.config.snapshot().font_size)
        .unwrap_or(15.0);
    let notes = collect_markdown_notes(Path::new(&vault_root));

    let required_notes = [
        "Start here.md",
        "Learning/Learning map.md",
        "Markdown field guide.md",
        "Learning/Study loop.md",
        "Project/Project workflow.md",
        "README.md",
    ];
    let mut failures: Vec<serde_json::Value> = Vec::new();
    for req in required_notes {
        if !notes.iter().any(|n| n == req) {
            failures.push(serde_json::json!({
                "stage": "scan",
                "reason": format!("vault 缺少必需笔记 {req}"),
            }));
        }
    }

    struct MatrixConfig<'a> {
        name: &'a str,
        width: i32,
        height: i32,
        sidebars: bool,
        dark: bool,
        notes: Vec<String>,
    }

    let configs = vec![
        MatrixConfig {
            name: "default-light",
            width: 1280,
            height: 820,
            sidebars: true,
            dark: false,
            notes: notes.clone(),
        },
        MatrixConfig {
            name: "default-dark",
            width: 1280,
            height: 820,
            sidebars: true,
            dark: true,
            notes: notes.clone(),
        },
        MatrixConfig {
            name: "wide-light",
            width: 1440,
            height: 860,
            sidebars: false,
            dark: false,
            notes: notes.clone(),
        },
        MatrixConfig {
            name: "wide-dark",
            width: 1440,
            height: 860,
            sidebars: false,
            dark: true,
            notes: notes.clone(),
        },
        MatrixConfig {
            name: "narrow-860-light",
            width: 860,
            height: 780,
            sidebars: true,
            dark: false,
            notes: vec![
                "Start here.md".into(),
                "Learning/Learning map.md".into(),
                "Markdown field guide.md".into(),
            ],
        },
    ];

    let mut cases = Vec::new();
    for cfg in configs {
        let cfg_name = cfg.name;
        let win_w = cfg.width;
        let win_h = cfg.height;
        let show_sidebars = cfg.sidebars;
        let is_dark = cfg.dark;
        let target_notes = cfg.notes;
        win.apply_color_scheme_and_sync(if is_dark {
            ColorSchemeChoice::Dark
        } else {
            ColorSchemeChoice::Light
        });
        win.left_split.set_show_sidebar(show_sidebars);
        win.right_split.set_show_sidebar(show_sidebars);
        let resized_ok = resize_window_to(win, win_w, win_h).await;
        if !resized_ok {
            failures.push(serde_json::json!({
                "config": cfg_name,
                "reason": format!("窗口未能缩放到 {win_w}x{win_h}，实测 {}x{}", win.window.width(), win.window.height()),
            }));
        }
        glib::timeout_future(Duration::from_millis(120)).await;

        for note in target_notes {
            if !notes.iter().any(|n| n == &note) {
                continue;
            }
            let case_res =
                audit_single_matrix_case(win, cfg_name, &note, show_sidebars, configured_font)
                    .await;
            if let Some(err) = case_res["error"].as_str() {
                failures.push(serde_json::json!({
                    "config": cfg_name,
                    "note": note,
                    "reason": err,
                    "details": case_res,
                }));
            }
            cases.push(case_res);

            // 保持标签页数量可控，避免过多后台 surface 占用过多内存
            let extra_tabs: Vec<String> =
                win.tab_paths().into_iter().filter(|p| p != &note).collect();
            for p in extra_tabs {
                win.state.borrow_mut().force_close.insert(p.clone());
                win.close_tab_by_path(&p);
            }
            glib::timeout_future(Duration::from_millis(20)).await;
        }
    }

    serde_json::json!({
        "passed": failures.is_empty(),
        "vault_root": vault_root,
        "note_count": notes.len(),
        "notes": notes,
        "cases_total": cases.len(),
        "failures_count": failures.len(),
        "failures": failures,
        "cases": cases,
    })
}

async fn audit_single_matrix_case(
    win: &Rc<WindowInner>,
    cfg_name: &str,
    note: &str,
    show_sidebars: bool,
    configured_font: f64,
) -> serde_json::Value {
    let mut err_msgs: Vec<String> = Vec::new();
    if !ensure_note_open_and_ready(win, note).await {
        return serde_json::json!({
            "config": cfg_name,
            "note": note,
            "error": "笔记加载超时",
        });
    }

    // 确保起点为阅读态顶位
    let initially_editing = win.state.borrow().tabs.get(note).is_some_and(|t| t.editing);
    if initially_editing {
        win.switch_edit_mode();
        let _ = wait_for(3000, || {
            win.state
                .borrow()
                .tabs
                .get(note)
                .is_some_and(|t| !t.editing && t.reading_loaded)
        })
        .await;
        glib::timeout_future(Duration::from_millis(60)).await;
    }

    if let Some(surface) = win.reading_surface(note) {
        surface.scrolled.vadjustment().set_value(0.0);
    }
    glib::timeout_future(Duration::from_millis(40)).await;

    let read_top_before = win
        .reading_surface(note)
        .map(|s| s.scrolled.vadjustment().value())
        .unwrap_or(-1.0);
    if !(-0.5..=1.0).contains(&read_top_before) {
        err_msgs.push(format!("初始阅读顶位异常：{read_top_before:.2}"));
    }

    // 阅读顶位 -> 切编辑态
    win.switch_edit_mode();
    let edit_ready = wait_for(4000, || {
        win.state.borrow().tabs.get(note).is_some_and(|t| {
            t.editing
                && t.edit_view.is_mapped()
                && t.edit_view.width() > 0
                && t.edit_host.width() > 0
        })
    })
    .await;
    glib::timeout_future(Duration::from_millis(90)).await;
    if !edit_ready {
        err_msgs.push("切入编辑态后未完成映射与宽度分配".into());
    }

    let (
        edit_top_val,
        line0_y,
        vis_y,
        vis_h,
        host_w,
        view_w,
        left_m,
        right_m,
        exp_m,
        col_w,
        pango_px,
        edit_upper,
        edit_page,
    ) = {
        let st = win.state.borrow();
        let Some(tab) = st.tabs.get(note) else {
            return serde_json::json!({
                "config": cfg_name,
                "note": note,
                "error": "切编辑态后标签页丢失",
            });
        };
        let adj = tab.edit_scrolled.vadjustment();
        let v_info = check_vadjustment_finite(&adj, "edit");
        if let Err(e) = &v_info {
            err_msgs.push(e.clone());
        }
        let (val, upper, page) = v_info.unwrap_or((adj.value(), adj.upper(), adj.page_size()));
        if val > 1.0 {
            err_msgs.push(format!(
                "顶位切编辑后 edit vadjustment 未归零：val={val:.2}"
            ));
        }
        let start_iter = tab.edit_buffer.start_iter();
        let loc0 = tab.edit_view.iter_location(&start_iter);
        let vis = tab.edit_view.visible_rect();
        if loc0.y() < 0 || loc0.y() < vis.y() || loc0.y() >= vis.y() + vis.height() {
            err_msgs.push(format!(
                "顶位切编辑后第 0 行不在可见顶区：loc0.y={} vis=({}, h={})",
                loc0.y(),
                vis.y(),
                vis.height()
            ));
        }

        let hw = tab.edit_host.width();
        let vw = tab.edit_view.width();
        let lm = tab.edit_view.left_margin();
        let rm = tab.edit_view.right_margin();
        let cw = vw - lm - rm;
        let exp = expected_edit_side_offset(hw);
        if (lm - exp).abs() > 2 || lm != rm {
            err_msgs.push(format!(
                "编辑态居中边距异常：left={lm} right={rm} expected={exp} host_w={hw}"
            ));
        }
        if !show_sidebars && hw >= 1008 && (cw - 920).abs() > 2 {
            err_msgs.push(format!("宽屏编辑内容列宽应为 920px，实测 {cw}px"));
        }
        if cw < 120 {
            err_msgs.push(format!("编辑内容列宽不足 120px：实测 {cw}px"));
        }

        if let Err(e) = check_edit_line_within_column(&tab.edit_view, 0, lm, cw, "首行") {
            err_msgs.push(e);
        }
        if let Some(h1_off) = find_first_h1_char_offset(&tab.edit_state.borrow().text) {
            if let Err(e) = check_edit_line_within_column(&tab.edit_view, h1_off, lm, cw, "H1标题")
            {
                err_msgs.push(e);
            }
            let tbl = tab.edit_buffer.tag_table();
            if let (Some(h1_tag), Some(h2_tag)) = (tbl.lookup("h1"), tbl.lookup("h2")) {
                let it = tab.edit_buffer.iter_at_offset(h1_off as i32);
                if !it.has_tag(&h1_tag) || it.has_tag(&h2_tag) {
                    err_msgs.push(format!(
                        "H1 标题标签错误：has_h1={} has_h2={}",
                        it.has_tag(&h1_tag),
                        it.has_tag(&h2_tag)
                    ));
                }
            }
        }
        if let Some(q_off) = find_first_quote_or_callout_line_char(&tab.edit_state.borrow().text)
            && let Err(e) =
                check_edit_line_within_column(&tab.edit_view, q_off, lm, cw, "quote/callout")
        {
            err_msgs.push(e);
        }
        if note == "Start here.md" {
            let text_now = tab.edit_state.borrow().text.clone();
            if let Some(byte_pos) = text_now.find("Learning map") {
                let char_off = text_now[..byte_pos].chars().count() as i32;
                let it = tab.edit_buffer.iter_at_offset(char_off);
                let tbl = tab.edit_buffer.tag_table();
                let link_tag = tbl.lookup("link");
                let quote_tag = tbl.lookup("quote_block");
                let callout_tag = tbl.lookup("callout_block");
                match (link_tag, quote_tag, callout_tag) {
                    (Some(lt), Some(qt), Some(ct)) => {
                        if !it.has_tag(&lt) {
                            err_msgs.push(
                                "Start here.md callout 内 [[Learning map]] 缺少 link 标签".into(),
                            );
                        }
                        if lt.priority() <= qt.priority() || lt.priority() <= ct.priority() {
                            err_msgs.push(format!(
                                "link 优先级未高于 quote_block/callout_block：link={} quote={} callout={}",
                                lt.priority(),
                                qt.priority(),
                                ct.priority()
                            ));
                        }
                    }
                    _ => err_msgs.push("tag_table 缺少 link/quote_block/callout_block".into()),
                }
            }
        }

        let font_px = match check_pango_font_size(&tab.edit_view, configured_font) {
            Ok(px) => px,
            Err(e) => {
                err_msgs.push(e);
                0.0
            }
        };

        (
            val,
            loc0.y(),
            vis.y(),
            vis.height(),
            hw,
            vw,
            lm,
            rm,
            exp,
            cw,
            font_px,
            upper,
            page,
        )
    };

    // 编辑态滚动向下再回顶
    let mut edit_scroll_tested = false;
    let mut edit_scrolled_mid = 0.0;
    let mut edit_scrolled_back = 0.0;
    let mut edit_outline_top = None;
    let mut edit_outline_mid = None;
    let mut edit_outline_back = None;
    let check_outline = note == "Markdown field guide.md";
    if check_outline {
        let st = win.state.borrow();
        edit_outline_top = st.active_outline_index;
        if edit_outline_top != Some(0) {
            err_msgs.push(format!(
                "编辑态顶位大纲高亮应为 Some(0)，实测 {edit_outline_top:?}"
            ));
        } else if let Some((_, btn)) = st.outline_items.first()
            && !btn.has_css_class("hortus-outline-active")
        {
            err_msgs.push("编辑态顶位大纲第 0 项缺少 hortus-outline-active 样式类".into());
        }
    }
    if edit_upper > edit_page + 24.0 {
        edit_scroll_tested = true;
        let max_y = (edit_upper - edit_page).max(0.0);
        if let Some(adj) = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|t| t.edit_scrolled.vadjustment())
        {
            adj.set_value(max_y * 0.5);
        }
        glib::timeout_future(Duration::from_millis(60)).await;
        if check_outline {
            let st = win.state.borrow();
            edit_outline_mid = st.active_outline_index;
            if edit_outline_mid.is_none_or(|i| i == 0) {
                err_msgs.push(format!(
                    "编辑态滚动中段后大纲高亮未前移：mid={edit_outline_mid:?}"
                ));
            }
        }
        if let Some(adj) = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|t| t.edit_scrolled.vadjustment())
        {
            edit_scrolled_mid = adj.value();
            if edit_scrolled_mid <= 1.0 {
                err_msgs.push(format!(
                    "编辑态无法向下滚动：mid={edit_scrolled_mid:.2} max={max_y:.2}"
                ));
            }
            adj.set_value(max_y);
        }
        glib::timeout_future(Duration::from_millis(60)).await;
        if let Some(adj) = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|t| t.edit_scrolled.vadjustment())
        {
            let bot = adj.value();
            if bot < max_y * 0.85 {
                err_msgs.push(format!("编辑态滚动到底段不足：bot={bot:.2} max={max_y:.2}"));
            }
            adj.set_value(0.0);
        }
        glib::timeout_future(Duration::from_millis(60)).await;
        if check_outline {
            let st = win.state.borrow();
            edit_outline_back = st.active_outline_index;
            if edit_outline_back != Some(0) {
                err_msgs.push(format!(
                    "编辑态滚回顶部后大纲高亮未恢复 Some(0)：back={edit_outline_back:?}"
                ));
            }
        }
        if let Some(adj) = win
            .state
            .borrow()
            .tabs
            .get(note)
            .map(|t| t.edit_scrolled.vadjustment())
        {
            edit_scrolled_back = adj.value();
            if edit_scrolled_back > 1.0 {
                err_msgs.push(format!("编辑态滚回顶部失败：val={edit_scrolled_back:.2}"));
            }
        }
    }

    // 切回阅读态并等待 reading_scrolled 高度稳定（避免 TextView 刚 re-map 时行高树未验证导致虚高 upper）
    win.switch_edit_mode();
    let back_reading_ok = wait_for(4000, || {
        win.state.borrow().tabs.get(note).is_some_and(|t| {
            !t.editing
                && t.reading_loaded
                && t.reading.as_ref().is_some_and(|s| {
                    s.scrolled.is_mapped()
                        && s.scrolled.height() > 0
                        && s.scrolled.vadjustment().page_size() > 100.0
                        && s.content_box.height() > 0
                })
        })
    })
    .await;
    let mut prev_upper = -1.0_f64;
    let mut stable_ticks = 0u8;
    for _ in 0..20 {
        glib::timeout_future(Duration::from_millis(30)).await;
        let cur_upper = win
            .reading_surface(note)
            .map(|s| s.scrolled.vadjustment().upper())
            .unwrap_or(-1.0);
        if cur_upper > 0.0 && (cur_upper - prev_upper).abs() <= 1.0 {
            stable_ticks += 1;
            if stable_ticks >= 2 {
                break;
            }
        } else {
            stable_ticks = 0;
        }
        prev_upper = cur_upper;
    }
    if !back_reading_ok {
        err_msgs.push("切回阅读态失败".into());
    }

    let read_top_after = win
        .reading_surface(note)
        .map(|s| s.scrolled.vadjustment().value())
        .unwrap_or(-1.0);
    if !(-0.5..=1.0).contains(&read_top_after) {
        err_msgs.push(format!(
            "编辑顶位切回阅读后未保留顶位：val={read_top_after:.2}"
        ));
    }

    // 校检阅读态字体/H1标签与滚动健康度
    let mut read_scroll_tested = false;
    let mut read_outline_top = None;
    let mut read_outline_mid = None;
    let mut read_outline_back = None;
    if check_outline {
        let st = win.state.borrow();
        read_outline_top = st.active_outline_index;
        if read_outline_top != Some(0) {
            err_msgs.push(format!(
                "阅读态顶位大纲高亮应为 Some(0)，实测 {read_outline_top:?}"
            ));
        }
    }
    if let Some(surface) = win.reading_surface(note) {
        let adj = surface.scrolled.vadjustment();
        match check_vadjustment_finite(&adj, "reading") {
            Ok((_, r_upper, r_page)) => {
                if r_upper > r_page + 24.0 {
                    adj.set_value((r_upper - r_page) * 0.5);
                    glib::timeout_future(Duration::from_millis(60)).await;
                    let upper_settled = adj.upper();
                    let page_settled = adj.page_size();
                    if upper_settled > page_settled + 24.0 {
                        read_scroll_tested = true;
                        if adj.value() <= 1.0 {
                            err_msgs.push(format!(
                                "阅读态无法向下滚动：val={:.2} upper={upper_settled:.2} page={page_settled:.2}",
                                adj.value()
                            ));
                        }
                        if check_outline {
                            let st = win.state.borrow();
                            read_outline_mid = st.active_outline_index;
                            if read_outline_mid.is_none_or(|i| i == 0) {
                                err_msgs.push(format!(
                                    "阅读态滚动中段后大纲高亮未前移：mid={read_outline_mid:?}"
                                ));
                            }
                        }
                        adj.set_value(0.0);
                        glib::timeout_future(Duration::from_millis(50)).await;
                        if adj.value() > 1.0 {
                            err_msgs.push(format!("阅读态滚回顶部失败：val={:.2}", adj.value()));
                        }
                        if check_outline {
                            let st = win.state.borrow();
                            read_outline_back = st.active_outline_index;
                            if read_outline_back != Some(0) {
                                err_msgs.push(format!(
                                    "阅读态滚回顶部后大纲高亮未恢复 Some(0)：back={read_outline_back:?}"
                                ));
                            }
                        }
                    } else {
                        adj.set_value(0.0);
                    }
                }
            }
            Err(e) => err_msgs.push(e),
        }
        if matches!(note, "Start here.md" | "Learning/Learning map.md")
            && let Some((_, first_buf)) = surface.text_views().into_iter().next()
        {
            let tbl = first_buf.tag_table();
            if let (Some(h1_tag), Some(h2_tag)) = (tbl.lookup("h1"), tbl.lookup("h2")) {
                let text_seg = first_buf
                    .text(&first_buf.start_iter(), &first_buf.end_iter(), false)
                    .to_string();
                let needle = if note == "Start here.md" {
                    "Learning lab"
                } else {
                    "Learning map"
                };
                let found_h1 = text_seg.match_indices(needle).any(|(b, _)| {
                    let ch = text_seg[..b].chars().count() as i32;
                    let it = first_buf.iter_at_offset(ch);
                    it.has_tag(&h1_tag) && !it.has_tag(&h2_tag)
                });
                if !found_h1 {
                    err_msgs.push(format!("{note} 阅读态主标题未应用 h1（或错应用 h2）"));
                }
            }
        }
    }

    let mut out = serde_json::json!({
        "config": cfg_name,
        "note": note,
        "ok": err_msgs.is_empty(),
        "read_top_before": read_top_before,
        "edit_top_val": edit_top_val,
        "line0_y": line0_y,
        "visible_rect_y": vis_y,
        "visible_rect_h": vis_h,
        "edit_host_w": host_w,
        "edit_view_w": view_w,
        "edit_left_margin": left_m,
        "edit_right_margin": right_m,
        "expected_margin": exp_m,
        "edit_column_w": col_w,
        "pango_font_px": pango_px,
        "edit_scroll_tested": edit_scroll_tested,
        "edit_scrolled_mid": edit_scrolled_mid,
        "edit_scrolled_back": edit_scrolled_back,
        "read_top_after": read_top_after,
        "read_scroll_tested": read_scroll_tested,
        "edit_outline_indices": [edit_outline_top, edit_outline_mid, edit_outline_back],
        "read_outline_indices": [read_outline_top, read_outline_mid, read_outline_back],
    });
    if !err_msgs.is_empty()
        && let Some(obj) = out.as_object_mut()
    {
        obj.insert("error".into(), serde_json::json!(err_msgs.join("; ")));
    }
    out
}

pub fn execute_fuzz(
    win: &Rc<WindowInner>,
    report_path: PathBuf,
    seed: u64,
    steps: usize,
    media_dir: Option<PathBuf>,
) {
    let weak = Rc::downgrade(win);
    glib::MainContext::default().spawn_local(async move {
        let Some(this) = weak.upgrade() else { return };
        let report = run_fuzz(&this, &report_path, seed, steps, media_dir.as_deref()).await;
        write_json_atomic(&report_path, &report);
        this.close_now_clean();
    });
}

#[allow(clippy::too_many_lines)]
async fn run_fuzz(
    win: &Rc<WindowInner>,
    report_path: &Path,
    seed: u64,
    steps: usize,
    media_dir: Option<&Path>,
) -> serde_json::Value {
    win.window.present();
    let vault_root = win.state.borrow().vault_root().unwrap_or_default();
    let configured_font = win
        .state
        .borrow()
        .services
        .as_ref()
        .map(|s| s.config.snapshot().font_size)
        .unwrap_or(15.0);
    let notes = collect_markdown_notes(Path::new(&vault_root));
    let scratch_note = notes
        .iter()
        .find(|n| n.ends_with("FuzzScratch.md"))
        .cloned();
    let diagram_notes: Vec<String> = notes
        .iter()
        .filter(|n| {
            matches!(
                n.as_str(),
                "Learning/Learning map.md"
                    | "Learning/Study loop.md"
                    | "Project/Project workflow.md"
                    | "Markdown field guide.md"
            )
        })
        .cloned()
        .collect();

    let mut rng = SplitMix64::new(seed);
    let mut action_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut recent: VecDeque<serde_json::Value> = VecDeque::with_capacity(30);

    for step in 0..steps {
        let roll = rng.next_usize(100);
        let action_name: &str;
        let detail: String;

        if roll < 16 && !notes.is_empty() {
            action_name = "open_note";
            let idx = rng.next_usize(notes.len());
            let target = notes[idx].clone();
            let _ = ensure_note_open_and_ready(win, &target).await;
            detail = target;
        } else if roll < 42 {
            action_name = "toggle_edit";
            let active = win.active_tab_path().unwrap_or_default();
            let double_toggle = rng.next_usize(4) == 0;
            win.switch_edit_mode();
            if double_toggle {
                glib::timeout_future(Duration::from_millis(20)).await;
                win.switch_edit_mode();
                detail = format!("{active} (double)");
            } else {
                detail = active.clone();
            }
            let _ = wait_for(2500, || {
                win.state.borrow().tabs.get(&active).is_some_and(|t| {
                    if t.editing {
                        t.edit_view.is_mapped() && t.edit_host.width() > 0
                    } else {
                        t.reading_loaded
                    }
                })
            })
            .await;
        } else if roll < 57 {
            action_name = "scroll_view";
            let active = win.active_tab_path().unwrap_or_default();
            let mode = rng.next_usize(3);
            let frac = match mode {
                0 => 0.0,
                1 => 0.2 + (rng.next_usize(60) as f64) / 100.0,
                _ => 1.0,
            };
            if let Some(adj) = win.state.borrow().tabs.get(&active).and_then(|t| {
                if t.editing {
                    Some(t.edit_scrolled.vadjustment())
                } else {
                    t.reading.as_ref().map(|r| r.scrolled.vadjustment())
                }
            }) {
                let max_v = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value((max_v * frac).clamp(0.0, max_v));
            }
            detail = format!("{active} frac={frac:.2}");
        } else if roll < 67 {
            action_name = "toggle_panel";
            match rng.next_usize(3) {
                0 => {
                    let next = !win.left_split.shows_sidebar();
                    win.left_split.set_show_sidebar(next);
                    glib::timeout_future(Duration::from_millis(280)).await;
                    detail = format!("left={next}");
                }
                1 => {
                    let next = !win.right_split.shows_sidebar();
                    win.right_split.set_show_sidebar(next);
                    glib::timeout_future(Duration::from_millis(280)).await;
                    detail = format!("right={next}");
                }
                _ => {
                    let next = !win.state.borrow().bottom_revealed;
                    win.set_bottom_panel_revealed(next);
                    glib::timeout_future(Duration::from_millis(80)).await;
                    detail = format!("bottom={next}");
                }
            }
        } else if roll < 75 {
            action_name = "toggle_theme";
            let next_dark = !libadwaita::StyleManager::default().is_dark();
            win.apply_color_scheme_and_sync(if next_dark {
                ColorSchemeChoice::Dark
            } else {
                ColorSchemeChoice::Light
            });
            glib::timeout_future(Duration::from_millis(60)).await;
            detail = format!("dark={next_dark}");
        } else if roll < 85 {
            action_name = "resize_window";
            let w = rng.next_i32_range(820, 1440);
            let h = rng.next_i32_range(740, 900);
            let _ = resize_window_to(win, w, h).await;
            detail = format!("{w}x{h}");
        } else if roll < 92 {
            action_name = "edit_undo_redo";
            if let Some(ref target) = scratch_note {
                let _ = ensure_note_open_and_ready(win, target).await;
                let is_ed = win
                    .state
                    .borrow()
                    .tabs
                    .get(target)
                    .is_some_and(|t| t.editing);
                if !is_ed {
                    win.switch_edit_mode();
                    glib::timeout_future(Duration::from_millis(50)).await;
                }
                let op = rng.next_usize(3);
                if let Some(buf) = win
                    .state
                    .borrow()
                    .tabs
                    .get(target)
                    .map(|t| t.edit_buffer.clone())
                {
                    match op {
                        0 => {
                            buf.begin_user_action();
                            let mut end = buf.end_iter();
                            buf.insert(&mut end, &format!("\nfuzz-{step}"));
                            buf.end_user_action();
                            detail = format!("{target} insert");
                        }
                        1 => {
                            if buf.can_undo() {
                                buf.undo();
                            }
                            detail = format!("{target} undo");
                        }
                        _ => {
                            if buf.can_redo() {
                                buf.redo();
                            }
                            detail = format!("{target} redo");
                        }
                    }
                } else {
                    detail = "missing scratch tab".into();
                }
            } else {
                detail = "no scratch note".into();
            }
        } else if roll < 96 {
            action_name = "overlays";
            match rng.next_usize(3) {
                0 => {
                    win.find_open(false);
                    let find_opt = win.active_tab_path().and_then(|active| {
                        win.state.borrow().tabs.get(&active).map(|t| t.find.clone())
                    });
                    if let Some(find) = find_opt {
                        find.set_query("the");
                        glib::timeout_future(Duration::from_millis(30)).await;
                        find.close();
                    }
                    detail = "find_bar".into();
                }
                1 => {
                    win.open_quick_open();
                    glib::timeout_future(Duration::from_millis(220)).await;
                    win.quick_open.close_if_open();
                    glib::timeout_future(Duration::from_millis(180)).await;
                    detail = "quick_open".into();
                }
                _ => {
                    if let Some(dn) = diagram_notes
                        .get(rng.next_usize(diagram_notes.len().max(1)))
                        .cloned()
                    {
                        let _ = ensure_note_open_and_ready(win, &dn).await;
                        win.open_focus_mode(&dn, 1);
                        glib::timeout_future(Duration::from_millis(40)).await;
                        win.focus.close();
                        detail = format!("focus {dn}");
                    } else {
                        detail = "focus skipped".into();
                    }
                }
            }
        } else {
            action_name = "close_reopen_tab";
            let tabs = win.tab_paths();
            if tabs.len() > 1 {
                let active = win.active_tab_path();
                let victim = tabs
                    .iter()
                    .find(|p| {
                        Some((*p).as_str()) != active.as_deref()
                            && scratch_note.as_ref() != Some(*p)
                    })
                    .or_else(|| tabs.first())
                    .cloned();
                if let Some(v) = victim {
                    win.state.borrow_mut().force_close.insert(v.clone());
                    win.close_tab_by_path(&v);
                    glib::timeout_future(Duration::from_millis(30)).await;
                    let _ = ensure_note_open_and_ready(win, &v).await;
                    detail = v;
                } else {
                    detail = "no victim".into();
                }
            } else {
                detail = "single tab".into();
            }
        }

        *action_counts.entry(action_name.to_string()).or_insert(0) += 1;
        let active_after = win.active_tab_path().unwrap_or_default();
        let editing_after = win
            .state
            .borrow()
            .tabs
            .get(&active_after)
            .is_some_and(|t| t.editing);
        if recent.len() == 30 {
            recent.pop_front();
        }
        recent.push_back(serde_json::json!({
            "step": step,
            "action": action_name,
            "detail": detail,
            "active_note": active_after,
            "editing": editing_after,
            "window_width": win.window.width(),
            "window_height": win.window.height(),
        }));

        glib::timeout_future(Duration::from_millis(45)).await;
        if let Err(reason) = verify_fuzz_invariants(win, configured_font).await {
            let png_path = match media_dir {
                Some(dir) => dir.join(format!("fuzz-fail-step-{step}.png")),
                None => {
                    let dir = report_path.parent().unwrap_or_else(|| Path::new("/tmp"));
                    dir.join(format!("fuzz-fail-step-{step}.png"))
                }
            };
            let shot_saved = win.capture_window_png(&png_path).is_ok();
            return serde_json::json!({
                "passed": false,
                "seed": seed,
                "steps_requested": steps,
                "steps_completed": step,
                "failed_step": step,
                "failure_reason": reason,
                "failure_screenshot": if shot_saved { Some(png_path.to_string_lossy().into_owned()) } else { None },
                "recent_30_actions": recent,
                "action_counts": action_counts,
            });
        }
    }

    serde_json::json!({
        "passed": true,
        "seed": seed,
        "steps_requested": steps,
        "steps_completed": steps,
        "note_count": notes.len(),
        "scratch_note": scratch_note,
        "action_counts": action_counts,
        "recent_30_actions": recent,
    })
}

async fn verify_fuzz_invariants(win: &Rc<WindowInner>, configured_font: f64) -> Result<(), String> {
    if win.state.borrow().disposing {
        return Err("窗口进入 disposing 状态".into());
    }
    let Some(active) = win.active_tab_path() else {
        return Err("活跃标签页缺失".into());
    };
    let loaded = wait_for(6000, || {
        win.state
            .borrow()
            .tabs
            .get(&active)
            .is_some_and(|t| t.source_loaded && t.reading_loaded)
    })
    .await;
    if !loaded {
        return Err(format!("活跃标签 {active} 未完成加载"));
    }

    let is_editing = win
        .state
        .borrow()
        .tabs
        .get(&active)
        .is_some_and(|t| t.editing);
    if is_editing {
        let _ = wait_for(1200, || {
            win.state.borrow().tabs.get(&active).is_some_and(|t| {
                let hw = t.edit_host.width();
                let vw = t.edit_view.width();
                if hw <= 0 || vw <= 0 {
                    return false;
                }
                let exp = expected_edit_side_offset(hw);
                (t.edit_view.left_margin() - exp).abs() <= 2
                    && t.edit_view.left_margin() == t.edit_view.right_margin()
            })
        })
        .await;
    }

    let st = win.state.borrow();
    let Some(tab) = st.tabs.get(&active) else {
        return Err(format!("标签 {active} 突然消失"));
    };

    if tab.editing {
        let host_w = tab.edit_host.width();
        let view_w = tab.edit_view.width();
        if host_w > 0 && view_w > 0 {
            let exp = expected_edit_side_offset(host_w);
            let lm = tab.edit_view.left_margin();
            let rm = tab.edit_view.right_margin();
            let col_w = view_w - lm - rm;
            if (lm - exp).abs() > 2 || lm != rm {
                return Err(format!(
                    "{active} 编辑态边距失配：left={lm} right={rm} expected={exp} host_w={host_w}"
                ));
            }
            if col_w < 120 {
                return Err(format!("{active} 编辑态正文列宽过窄：{col_w}px"));
            }
        }
        let _ = check_pango_font_size(&tab.edit_view, configured_font)?;
        let (_, upper, page) = check_vadjustment_finite(&tab.edit_scrolled.vadjustment(), "edit")?;
        if upper > page + 24.0 && upper - page <= 24.0 {
            return Err(format!("{active} 编辑态长文档可滚区间异常"));
        }
    } else if let Some(ref surface) = tab.reading {
        let _ = check_vadjustment_finite(&surface.scrolled.vadjustment(), "reading")?;
    } else {
        return Err(format!("{active} 处于阅读态但 reading surface 为空"));
    }

    let edit = tab.edit_state.borrow();
    if edit.dirty {
        let buf_text = tab
            .edit_buffer
            .text(
                &tab.edit_buffer.start_iter(),
                &tab.edit_buffer.end_iter(),
                false,
            )
            .to_string();
        if edit.text != buf_text {
            return Err(format!(
                "{active} 脏编辑态内存文本与缓冲区不一致：state_len={} buf_len={}",
                edit.text.len(),
                buf_text.len()
            ));
        }
    }

    Ok(())
}
