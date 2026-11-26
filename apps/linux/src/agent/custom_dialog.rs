// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 轻量原生「自定义命令启动 Agent…」弹窗：自由 Shell 命令输入与最近 12 条缓存历史。

use crate::i18n::{Key, tr};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::WorkspaceCore;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct CustomAgentDialogState {
    history: Vec<String>,
    on_launch: Rc<dyn Fn(String)>,
}

struct CustomAgentDialogInner {
    dialog: adw::Dialog,
    command_entry: adw::EntryRow,
    launch_btn: gtk4::Button,
    history_group: adw::PreferencesGroup,
    history_rows: RefCell<Vec<adw::ActionRow>>,
    state: RefCell<CustomAgentDialogState>,
}

#[derive(Clone)]
pub struct CustomAgentDialog {
    inner: Rc<CustomAgentDialogInner>,
}

impl CustomAgentDialog {
    pub fn new(core: Arc<WorkspaceCore>, on_launch: Rc<dyn Fn(String)>) -> Self {
        let dialog = adw::Dialog::builder()
            .title(tr(Key::CustomAgentTitle))
            .content_width(520)
            .content_height(420)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);
        header.set_show_start_title_buttons(false);

        let cancel_btn = gtk4::Button::with_label(tr(Key::Cancel));
        let launch_btn = gtk4::Button::with_label(tr(Key::CustomAgentLaunchBtn));
        launch_btn.add_css_class("suggested-action");
        launch_btn.set_sensitive(false);
        header.pack_start(&cancel_btn);
        header.pack_end(&launch_btn);
        toolbar_view.add_top_bar(&header);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);

        let cmd_group = adw::PreferencesGroup::new();
        let command_entry = adw::EntryRow::builder()
            .title(tr(Key::CustomAgentCommandLabel))
            .activates_default(true)
            .build();
        cmd_group.add(&command_entry);
        content.append(&cmd_group);

        let history_group = adw::PreferencesGroup::builder()
            .title(tr(Key::CustomAgentHistoryTitle))
            .build();
        content.append(&history_group);

        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .child(&content)
            .build();
        toolbar_view.set_content(Some(&scroll));
        dialog.set_child(Some(&toolbar_view));

        let inner = Rc::new(CustomAgentDialogInner {
            dialog,
            command_entry,
            launch_btn,
            history_group,
            history_rows: RefCell::new(Vec::new()),
            state: RefCell::new(CustomAgentDialogState {
                history: Vec::new(),
                on_launch,
            }),
        });

        {
            let weak = Rc::downgrade(&inner);
            cancel_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    let _ = this.dialog.close();
                }
            });
        }
        {
            let weak = Rc::downgrade(&inner);
            inner.launch_btn.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::launch_current(&this);
                }
            });
        }
        {
            let weak = Rc::downgrade(&inner);
            inner.command_entry.connect_changed(move |entry| {
                if let Some(this) = weak.upgrade() {
                    let ready = !entry.text().trim().is_empty();
                    this.launch_btn.set_sensitive(ready);
                }
            });
        }
        {
            let weak = Rc::downgrade(&inner);
            inner.command_entry.connect_entry_activated(move |_| {
                if let Some(this) = weak.upgrade() {
                    Self::launch_current(&this);
                }
            });
        }

        Self::render_history(&inner, Vec::new());
        Self::load_history_async(&inner, core);
        Self { inner }
    }

    pub fn present(&self, parent: &impl IsA<gtk4::Widget>) {
        self.inner.dialog.present(Some(parent));
        self.inner.command_entry.grab_focus();
    }

    pub fn same_instance(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn connect_closed(&self, cb: impl Fn(&Self) + 'static) {
        let weak = Rc::downgrade(&self.inner);
        self.inner.dialog.connect_closed(move |_| {
            if let Some(inner) = weak.upgrade() {
                cb(&Self { inner });
            }
        });
    }

    pub fn force_close(&self) {
        self.inner.dialog.force_close();
    }

    fn launch_current(inner: &Rc<CustomAgentDialogInner>) {
        let cmd = inner.command_entry.text().trim().to_string();
        if cmd.is_empty() {
            return;
        }
        Self::launch_command(inner, cmd);
    }

    fn launch_command(inner: &Rc<CustomAgentDialogInner>, cmd: String) {
        let cb = inner.state.borrow().on_launch.clone();
        let _ = inner.dialog.close();
        cb(cmd);
    }

    fn load_history_async(inner: &Rc<CustomAgentDialogInner>, core: Arc<WorkspaceCore>) {
        let weak = Rc::downgrade(inner);
        glib::MainContext::default().spawn_local(async move {
            let items = gio::spawn_blocking(move || core.custom_agent_history())
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            if let Some(this) = weak.upgrade() {
                Self::render_history(&this, items);
            }
        });
    }

    fn render_history(inner: &Rc<CustomAgentDialogInner>, items: Vec<String>) {
        {
            let mut rows = inner.history_rows.borrow_mut();
            for row in rows.drain(..) {
                inner.history_group.remove(&row);
            }
        }
        inner.state.borrow_mut().history = items.clone();
        if items.is_empty() {
            let row = adw::ActionRow::builder()
                .title(tr(Key::CustomAgentHistoryEmpty))
                .build();
            inner.history_group.add(&row);
            inner.history_rows.borrow_mut().push(row);
            return;
        }
        for cmd in items {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&cmd))
                .activatable(true)
                .build();
            let use_btn = gtk4::Button::with_label(tr(Key::CustomAgentUseBtn));
            use_btn.add_css_class("flat");
            use_btn.set_valign(gtk4::Align::Center);
            {
                let weak = Rc::downgrade(inner);
                let cmd_fill = cmd.clone();
                use_btn.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade() {
                        this.command_entry.set_text(&cmd_fill);
                        this.command_entry.grab_focus();
                    }
                });
            }
            {
                let weak = Rc::downgrade(inner);
                let cmd_run = cmd.clone();
                row.connect_activated(move |_| {
                    if let Some(this) = weak.upgrade() {
                        Self::launch_command(&this, cmd_run.clone());
                    }
                });
            }
            row.add_suffix(&use_btn);
            inner.history_group.add(&row);
            inner.history_rows.borrow_mut().push(row);
        }
    }
}

#[cfg(feature = "automation")]
impl CustomAgentDialog {
    pub fn set_command(&self, command: &str) {
        self.inner.command_entry.set_text(command);
    }

    pub fn trigger_launch(&self) {
        Self::launch_current(&self.inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_agent_dialog_drops_without_reference_cycle() {
        if !crate::platform::style::ensure_gtk_test_thread() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .unwrap();
        let dlg = CustomAgentDialog::new(core, Rc::new(|_| {}));
        let weak = Rc::downgrade(&dlg.inner);
        assert_eq!(weak.strong_count(), 1);
        drop(dlg);
        assert_eq!(weak.strong_count(), 0);
    }
}
