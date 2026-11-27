// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

mod agent;
#[cfg(feature = "automation")]
mod automation;
mod core;
mod dock;
mod editor;
mod files;
mod i18n;
mod knowledge;
mod platform;
mod reading;
mod render;
mod services;
mod settings;
mod terminal;
mod window;

use crate::core::{APP_ID, CliOptions};
use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;

fn main() -> std::process::ExitCode {
    platform::fonts::mark_main_thread();
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

    let flags = if options.wants_non_unique() {
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
    app.set_accels_for_action("win.save-note", &["<Control>s"]);
    app.set_accels_for_action("win.new-note", &["<Control>n"]);
    app.set_accels_for_action("win.find-in-note", &["<Control>f"]);
    app.set_accels_for_action("win.replace-in-note", &["<Control>h"]);
    app.set_accels_for_action("win.quick-open", &["<Control>p"]);
    app.set_accels_for_action("win.focus-search", &["<Control><Shift>f"]);
    app.set_accels_for_action("win.align-table", &["<Control><Shift>t"]);
    app.set_accels_for_action("win.toggle-task", &["<Control>Return"]);
    app.set_accels_for_action("win.toggle-edit-mode", &["<Control>e"]);
    app.set_accels_for_action("win.focus-diagram", &["<Control><Shift>d"]);
    app.set_accels_for_action("win.toggle-left-sidebar", &["<Control><Shift>s"]);
    app.set_accels_for_action("win.toggle-right-sidebar", &["<Control><Shift>i"]);
    app.set_accels_for_action("win.toggle-bottom-panel", &["<Control>j"]);
    app.set_accels_for_action("win.toggle-tab-overview", &["<Control><Shift>o"]);
    app.set_accels_for_action("win.toggle-top-bar", &["<Control><Shift>h"]);
    app.set_accels_for_action("win.toggle-fullscreen", &["F11"]);
    app.set_accels_for_action("win.new-window", &["<Control><Shift>n"]);
    app.set_accels_for_action("win.open-settings", &["<Control>comma"]);
    app.set_accels_for_action("win.send-path-to-terminal", &["<Control><Shift>l"]);
    app.set_accels_for_action("win.send-selection-to-terminal", &["<Control><Shift>k"]);

    let options_cell = RefCell::new(Some(options));
    app.connect_activate(move |app| {
        platform::style::install_css();
        let options = options_cell.borrow_mut().take();
        if let Some(opts) = options {
            window::session::launch_initial_windows(app, opts);
        } else if let Some(win) = app.active_window() {
            win.present();
        }
    });

    app.run_with_args(&Vec::<String>::new());
    std::process::ExitCode::SUCCESS
}
