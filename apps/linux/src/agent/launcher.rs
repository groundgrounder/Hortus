// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Agent 安全启动闭环：校验 `[agent]` 配置 → 后台全库快照（可取消，失败硬阻断）→ 直接按 argv 启动 VTE4 标签。

use crate::services::AppServices;
use crate::terminal::TerminalHost;
use crate::terminal::process::build_terminal_env;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[derive(Clone)]
pub struct AgentLaunchCallbacks {
    pub on_toast: Rc<dyn Fn(String)>,
    pub on_open_agent_settings: Rc<dyn Fn()>,
    pub on_session_changed: Rc<dyn Fn()>,
    pub on_reveal_terminal: Rc<dyn Fn()>,
    pub on_alert: Rc<dyn Fn(adw::AlertDialog)>,
    pub current_vault_root: Rc<dyn Fn() -> Option<String>>,
    pub is_disposing: Rc<dyn Fn() -> bool>,
}

fn friendly_spec_error(err: &str) -> String {
    match err {
        "agent.not_configured" => {
            "尚未配置 [agent] 启动模板。请在设置中配置 Agent 命令与参数。".into()
        }
        "agent.command_missing" => "Agent 配置缺少可执行命令（command）。".into(),
        "agent.guide_missing" => "Agent 启动参数缺少 {guide} 占位符。".into(),
        "agent.command_not_absolute" => "含路径分隔符的 Agent 命令必须使用绝对路径。".into(),
        "agent.not_found" => "未在 PATH 中找到配置的 Agent 可执行文件。".into(),
        "agent.not_executable" => "配置的 Agent 目标不是可执行常规文件。".into(),
        other => format!("Agent 配置校验失败：{other}"),
    }
}

/// 严格按安全闸门启动 Agent：
/// 1) 校验 `[agent]` 配置（失败提示并引导设置，不建快照、不启进程）；
/// 2) 打开“正在建立安全快照…”占位标签并后台执行 `begin_agent_session`；
/// 3) 快照失败或被取消时硬性阻断启动，绝不静默降级；
/// 4) 快照成功后以 `spec.executable + spec.args`（不经 shell 二次拆分）在 VTE4 启动并标记 `running`。
pub fn launch_agent_with_snapshot_gate(
    parent_window: &adw::ApplicationWindow,
    services: Arc<AppServices>,
    terminal: TerminalHost,
    login_path: Option<String>,
    callbacks: AgentLaunchCallbacks,
) {
    let Some(vault_root) = (callbacks.current_vault_root)() else {
        (callbacks.on_toast)("请先打开知识库后再启动 Agent".into());
        return;
    };

    let win_weak = parent_window.downgrade();
    let core = Arc::clone(&services.core);
    let lp = login_path.clone();
    glib::MainContext::default().spawn_local(async move {
        let lp_bg = lp.clone();
        let core_bg = Arc::clone(&core);
        let spec_res = gio::spawn_blocking(move || {
            let spec = core_bg
                .agent_spec_with_path(lp_bg.clone())
                .map_err(|e| e.to_string())?;
            let effective_path = core_bg.effective_agent_path(lp_bg);
            Ok::<_, String>((spec, effective_path))
        })
        .await
        .unwrap_or_else(|_| Err("agent.thread_error".into()));

        if (callbacks.is_disposing)() {
            return;
        }
        let (spec, effective_path) = match spec_res {
            Ok(ok) => ok,
            Err(err) => {
                if let Some(win) = win_weak.upgrade() {
                    let dialog = adw::AlertDialog::builder()
                        .heading("无法启动 Agent")
                        .body(friendly_spec_error(&err))
                        .build();
                    dialog.add_response("cancel", "取消");
                    dialog.add_response("settings", "打开设置 → Agent");
                    dialog.set_response_appearance("settings", adw::ResponseAppearance::Suggested);
                    dialog.set_default_response(Some("settings"));
                    dialog.set_close_response("cancel");
                    let open_settings = callbacks.on_open_agent_settings.clone();
                    dialog.connect_response(None, move |_, resp| {
                        if resp == "settings" {
                            open_settings();
                        }
                    });
                    (callbacks.on_alert)(dialog.clone());
                    dialog.present(Some(&win));
                } else {
                    (callbacks.on_toast)(friendly_spec_error(&err));
                }
                return;
            }
        };

        (callbacks.on_reveal_terminal)();
        let (tab_id, cancel_flag) = terminal.create_preparing_agent_tab();
        let core_snap = Arc::clone(&core);
        let vault_snap = vault_root.clone();
        let snap_res = gio::spawn_blocking(move || {
            core_snap
                .begin_agent_session(vault_snap)
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|_| Err("history.snapshot_thread_error".into()));

        if (callbacks.is_disposing)()
            || cancel_flag.load(Ordering::Acquire)
            || (callbacks.current_vault_root)().as_deref() != Some(vault_root.as_str())
        {
            if let Ok(info) = snap_res {
                let core_cancel = Arc::clone(&core);
                let sid = info.id;
                std::thread::spawn(move || {
                    let _ = core_cancel.mark_agent_session(sid, "cancelled".into(), None);
                });
            }
            return;
        }

        let info = match snap_res {
            Ok(info) => info,
            Err(err) => {
                terminal.fail_preparing_agent_tab(tab_id, &format!("快照失败：{err}"));
                (callbacks.on_toast)(format!("建立安全快照失败，已阻止启动 Agent：{err}"));
                return;
            }
        };

        let mut argv = Vec::with_capacity(1 + spec.args.len());
        argv.push(spec.executable);
        argv.extend(spec.args);
        let envv = build_terminal_env(Some(&vault_root), Some(&info.id), Some(&effective_path));
        let sid = info.id.clone();

        // 在启动进程之前先把快照状态推进到 running，避免短生命周期 Agent 竞态退出。
        let core_mark = Arc::clone(&core);
        let sid_run = sid.clone();
        let _ = gio::spawn_blocking(move || {
            core_mark.mark_agent_session(sid_run, "running".into(), None)
        })
        .await;

        if (callbacks.is_disposing)() || cancel_flag.load(Ordering::Acquire) {
            let core_cancel = Arc::clone(&core);
            std::thread::spawn(move || {
                let _ = core_cancel.mark_agent_session(sid, "cancelled".into(), None);
            });
            return;
        }

        match terminal.start_agent_in_tab(tab_id, sid.clone(), vault_root, argv, envv) {
            Ok(()) => {
                (callbacks.on_session_changed)();
            }
            Err(err) => {
                let core_fail = Arc::clone(&core);
                let sid_bg = sid;
                let on_changed = callbacks.on_session_changed.clone();
                glib::MainContext::default().spawn_local(async move {
                    let _ = gio::spawn_blocking(move || {
                        core_fail.mark_agent_session(sid_bg, "failed".into(), None)
                    })
                    .await;
                    on_changed();
                });
                terminal.fail_preparing_agent_tab(tab_id, &err);
            }
        }
    });
}
