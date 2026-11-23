// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

mod core;
mod style;
mod tree;
mod window;

use crate::core::{APP_ID, CliOptions};
use crate::window::HortusWindow;
use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;

fn main() -> std::process::ExitCode {
    let options = match CliOptions::parse(std::env::args()) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{}", CliOptions::usage());
            return std::process::ExitCode::FAILURE;
        }
    };
    if options.show_help {
        println!("{}", CliOptions::usage());
        return std::process::ExitCode::SUCCESS;
    }

    // 传入命令行参数启动时使用 NON_UNIQUE，确保多窗口/自动化/指定库参数互不干扰。
    let flags = if options.vault.is_some()
        || !options.open_notes.is_empty()
        || options.smoke_json.is_some()
        || options.screenshot.is_some()
        || options.color_scheme.is_some()
        || options.width.is_some()
        || options.height.is_some()
        || options.overview
        || options.hide_top_bar
    {
        gio::ApplicationFlags::NON_UNIQUE
    } else {
        gio::ApplicationFlags::FLAGS_NONE
    };

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();

    app.set_accels_for_action("win.open-vault", &["<Control>o"]);
    app.set_accels_for_action("win.close-tab", &["<Control>w"]);
    app.set_accels_for_action("win.toggle-edit-mode", &["<Control>e"]);
    app.set_accels_for_action("win.toggle-left-sidebar", &["<Control><Shift>s"]);
    app.set_accels_for_action("win.toggle-right-sidebar", &["<Control><Shift>i"]);
    app.set_accels_for_action("win.toggle-bottom-panel", &["<Control>j"]);
    app.set_accels_for_action("win.toggle-tab-overview", &["<Control><Shift>o"]);
    app.set_accels_for_action("win.toggle-top-bar", &["<Control><Shift>h"]);
    app.set_accels_for_action("win.toggle-fullscreen", &["F11"]);

    let options_cell = RefCell::new(Some(options));
    app.connect_activate(move |app| {
        style::install_css();
        if let Some(opts) = options_cell.borrow_mut().take() {
            let win = HortusWindow::new(app, opts);
            win.present();
        } else if let Some(win) = app.active_window() {
            win.present();
        }
    });

    let empty_args: [&str; 0] = [];
    let code = app.run_with_args(&empty_args);
    if code == gtk4::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
