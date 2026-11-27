// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! Agent 安全启动闭环：校验配置档或自定义命令 → 后台全库快照（可取消，失败硬阻断）→ 启动 VTE4 标签。

use crate::core::PanelRegion;
use crate::i18n::{Key, fmt1, tr};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentLaunchTarget {
    DefaultProfile,
    Profile { id: String, name: String },
    CustomCommand(String),
}

#[derive(Clone)]
pub struct AgentLaunchCallbacks {
    pub on_toast: Rc<dyn Fn(String)>,
    pub on_open_agent_settings: Rc<dyn Fn()>,
    pub on_session_changed: Rc<dyn Fn()>,
    pub on_alert: Rc<dyn Fn(adw::AlertDialog)>,
    pub current_vault_root: Rc<dyn Fn() -> Option<String>>,
    pub is_disposing: Rc<dyn Fn() -> bool>,
}

struct PreparedAgentSpec {
    executable: String,
    args: Vec<String>,
    effective_path: String,
    guide_file: Option<String>,
    tab_title: Option<String>,
}

fn friendly_spec_error(err: &str) -> String {
    friendly_spec_error_for(crate::i18n::current_lang(), err)
}

fn friendly_spec_error_for(lang: crate::i18n::Lang, err: &str) -> String {
    match err {
        "agent.not_configured" => crate::i18n::tr_for(lang, Key::AgentNotConfigured).into(),
        "agent.command_missing" => crate::i18n::tr_for(lang, Key::AgentCmdMissing).into(),
        "agent.guide_missing" => crate::i18n::tr_for(lang, Key::AgentGuideMissing).into(),
        "agent.command_not_absolute" => crate::i18n::tr_for(lang, Key::AgentCmdNotAbsolute).into(),
        "agent.not_found" => crate::i18n::tr_for(lang, Key::AgentNotFound).into(),
        "agent.not_executable" => crate::i18n::tr_for(lang, Key::AgentNotExecutable).into(),
        "agent.profile_not_found" => crate::i18n::tr_for(lang, Key::AgentProfileNotFound).into(),
        "guide.empty" => crate::i18n::tr_for(lang, Key::GuideEmpty).into(),
        "guide.invalid_utf8" => crate::i18n::tr_for(lang, Key::GuideInvalidUtf8).into(),
        "guide.read_failed" => crate::i18n::tr_for(lang, Key::GuideReadFailed).into(),
        other => crate::i18n::fmt1_for(lang, Key::AgentInvalidConfig, other),
    }
}

/// 严格按安全闸门启动 Agent：
/// 1) 校验配置档或准备自定义命令只读指南文件（失败提示，不建快照、不启进程）；
/// 2) 打开准备占位标签并后台执行 `begin_agent_session`；
/// 3) 快照失败或被取消时硬性阻断启动，绝不静默降级；
/// 4) 快照成功后以 `executable + args` 与安全环境变量在终端启动并标记 `running`。
pub fn launch_agent_with_snapshot_gate(
    parent_window: &adw::ApplicationWindow,
    services: Arc<AppServices>,
    terminal: TerminalHost,
    target_region: PanelRegion,
    login_path: Option<String>,
    target: AgentLaunchTarget,
    callbacks: AgentLaunchCallbacks,
) {
    let Some(vault_root) = (callbacks.current_vault_root)() else {
        (callbacks.on_toast)(tr(Key::ToastOpenFolderBeforeAgent).into());
        return;
    };

    let win_weak = parent_window.downgrade();
    let core = Arc::clone(&services.core);
    let lp = login_path.clone();
    glib::MainContext::default().spawn_local(async move {
        let lp_bg = lp.clone();
        let core_bg = Arc::clone(&core);
        let target_bg = target.clone();
        let spec_res = gio::spawn_blocking(move || match target_bg {
            AgentLaunchTarget::DefaultProfile => {
                let spec = core_bg
                    .agent_spec_for_profile(None, lp_bg.clone())
                    .map_err(|e| e.to_string())?;
                let effective_path = core_bg.effective_agent_path(lp_bg);
                Ok::<_, String>(PreparedAgentSpec {
                    executable: spec.executable,
                    args: spec.args,
                    effective_path,
                    guide_file: None,
                    tab_title: None,
                })
            }
            AgentLaunchTarget::Profile { id, name } => {
                let spec = core_bg
                    .agent_spec_for_profile(Some(id), lp_bg.clone())
                    .map_err(|e| e.to_string())?;
                let effective_path = core_bg.effective_agent_path(lp_bg);
                let trimmed = name.trim();
                let tab_title = if trimmed.is_empty() {
                    None
                } else {
                    Some(fmt1(Key::TermTabAgentNamed, trimmed))
                };
                Ok::<_, String>(PreparedAgentSpec {
                    executable: spec.executable,
                    args: spec.args,
                    effective_path,
                    guide_file: None,
                    tab_title,
                })
            }
            AgentLaunchTarget::CustomCommand(command) => {
                let launch = core_bg
                    .prepare_custom_agent_launch(command.clone(), lp_bg)
                    .map_err(|e| e.to_string())?;
                let _ = core_bg.record_custom_agent_command(command);
                Ok::<_, String>(PreparedAgentSpec {
                    executable: launch.executable,
                    args: launch.args,
                    effective_path: launch.path,
                    guide_file: Some(launch.guide_file),
                    tab_title: None,
                })
            }
        })
        .await
        .unwrap_or_else(|_| Err("agent.thread_error".into()));

        if (callbacks.is_disposing)() {
            return;
        }
        let prepared = match spec_res {
            Ok(ok) => ok,
            Err(err) => {
                if let Some(win) = win_weak.upgrade() {
                    let dialog = adw::AlertDialog::builder()
                        .heading(tr(Key::CannotStartAgentHeading))
                        .body(friendly_spec_error(&err))
                        .build();
                    dialog.add_response("cancel", tr(Key::Cancel));
                    dialog.add_response("settings", tr(Key::OpenAgentSettingsBtn));
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

        let (tab_id, cancel_flag) = terminal.create_preparing_agent_tab_in_region(target_region);
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
                terminal.fail_preparing_agent_tab(tab_id, &fmt1(Key::SnapshotFailedPrefix, &err));
                (callbacks.on_toast)(fmt1(Key::SnapshotBlockedToast, &err));
                return;
            }
        };

        let mut argv = Vec::with_capacity(1 + prepared.args.len());
        argv.push(prepared.executable);
        argv.extend(prepared.args);
        let envv = build_terminal_env(
            Some(&vault_root),
            Some(&info.id),
            Some(&prepared.effective_path),
            prepared.guide_file.as_deref(),
        );
        let sid = info.id.clone();

        // 在启动进程之前先把快照状态推进到 running，避免短时运行的 Agent 竞态退出。
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

        match terminal.start_agent_in_tab(
            tab_id,
            sid.clone(),
            prepared.tab_title,
            vault_root,
            argv,
            envv,
        ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn friendly_spec_error_maps_guide_keys_in_both_languages() {
        let keys = ["guide.empty", "guide.invalid_utf8", "guide.read_failed"];
        for lang in [Lang::En, Lang::ZhHans] {
            for key in keys {
                let msg = friendly_spec_error_for(lang, key);
                assert!(!msg.trim().is_empty(), "empty message for {key}");
                assert!(
                    !msg.contains(key),
                    "raw key leaked for {key} ({lang:?}): {msg}"
                );
                assert!(
                    msg.contains("agent-guide.md"),
                    "expected guide file name in message for {key} ({lang:?}): {msg}"
                );
            }
        }
    }
}
