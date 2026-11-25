// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 库内文件管理对话框：新建笔记/文件夹、重命名（含双链批量更新预览）与回收站二次确认。

use crate::i18n::{Key, fmt1, fmt2, tr};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use hortus_ffi::{RenameLinkPlanInfo, TreeEntry, VaultHandle};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub type CreateDone = Rc<dyn Fn(Result<TreeEntry, String>)>;
pub type RenameConfirm = Rc<dyn Fn(String, bool)>;
pub type TrashConfirm = Rc<dyn Fn()>;

fn build_dialog(title: &str) -> (adw::Dialog, adw::HeaderBar, gtk4::Box) {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(400)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    body.set_margin_top(6);
    body.set_margin_bottom(16);
    body.set_margin_start(16);
    body.set_margin_end(16);
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    dialog.set_child(Some(&root));
    (dialog, header, body)
}

/// 新建笔记或文件夹；成功后回车即可继续在标签页中编辑。
pub fn prompt_new_entry(
    window: &adw::ApplicationWindow,
    handle: Arc<VaultHandle>,
    parent_dir: String,
    directory: bool,
    on_done: CreateDone,
) {
    let title = if directory {
        tr(Key::CreateFolder)
    } else {
        tr(Key::CreateNote)
    };
    let (dialog, header, body) = build_dialog(title);
    let subtitle = if parent_dir.is_empty() {
        tr(Key::LocationRoot).to_string()
    } else {
        fmt1(Key::LocationSub, &parent_dir)
    };
    let hint = gtk4::Label::new(Some(&subtitle));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.set_xalign(0.0);
    body.append(&hint);

    let entry = gtk4::Entry::builder()
        .placeholder_text(if directory {
            tr(Key::FolderNamePlaceholder)
        } else {
            tr(Key::NoteNamePlaceholder)
        })
        .activates_default(true)
        .build();
    body.append(&entry);

    let error = gtk4::Label::new(None);
    error.add_css_class("error");
    error.add_css_class("caption");
    error.set_xalign(0.0);
    error.set_visible(false);
    body.append(&error);

    let cancel = gtk4::Button::with_label(tr(Key::Cancel));
    let create = gtk4::Button::with_label(if directory {
        tr(Key::CreateFolder)
    } else {
        tr(Key::CreateNote)
    });
    create.add_css_class("suggested-action");
    header.pack_start(&cancel);
    header.pack_end(&create);
    {
        let dlg_weak = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    let dlg_weak = dialog.downgrade();
    let entry_weak = entry.downgrade();
    let error_weak = error.downgrade();
    let submit = move || {
        let (Some(dialog_done), Some(entry_ref), Some(error_done)) = (
            dlg_weak.upgrade(),
            entry_weak.upgrade(),
            error_weak.upgrade(),
        ) else {
            return;
        };
        let name = entry_ref.text().to_string();
        if name.trim().is_empty() {
            error_done.set_label(tr(Key::ErrNameEmpty));
            error_done.set_visible(true);
            return;
        }
        error_done.set_visible(false);
        let handle_bg = Arc::clone(&handle);
        let parent_bg = parent_dir.clone();
        let name_bg = name.clone();
        let dlg_async = dialog_done.downgrade();
        let err_async = error_done.downgrade();
        let on_done_done = on_done.clone();
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || {
                let res = if directory {
                    handle_bg.create_folder(parent_bg, name_bg)
                } else {
                    handle_bg.create_note(parent_bg, name_bg)
                };
                res.map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err(tr(Key::ErrThread).into()));
            match result {
                Ok(entry) => {
                    if let Some(d) = dlg_async.upgrade() {
                        let _ = d.close();
                    }
                    on_done_done(Ok(entry));
                }
                Err(err) => {
                    if let Some(e) = err_async.upgrade() {
                        e.set_label(&fmt1(Key::CreateFailed, err));
                        e.set_visible(true);
                    }
                }
            }
        });
    };
    let submit_btn = submit.clone();
    create.connect_clicked(move |_| submit_btn());
    entry.connect_activate(move |_| submit());
    dialog.present(Some(window));
    entry.grab_focus();
}

/// 重命名对话框：输入时后台预计算受影响的笔记数/改链数，默认勾选同步更新双链与嵌入。
pub fn prompt_rename(
    window: &adw::ApplicationWindow,
    handle: Arc<VaultHandle>,
    path: String,
    is_dir: bool,
    initial_name: Option<String>,
    on_confirm: RenameConfirm,
) {
    let (dialog, header, body) = build_dialog(if is_dir {
        tr(Key::RenameFolderTitle)
    } else {
        tr(Key::RenameNoteTitle)
    });
    let current_name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let parent = path
        .rsplit_once('/')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default();

    let hint = gtk4::Label::new(Some(&if parent.is_empty() {
        tr(Key::LocationRoot).to_string()
    } else {
        fmt1(Key::LocationSub, &parent)
    }));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.set_xalign(0.0);
    body.append(&hint);

    let entry = gtk4::Entry::builder()
        .text(initial_name.as_deref().unwrap_or(&current_name))
        .activates_default(true)
        .build();
    entry.select_region(0, -1);
    body.append(&entry);

    let update_switch = gtk4::Switch::new();
    update_switch.set_active(true);
    update_switch.set_valign(gtk4::Align::Center);
    let switch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    let switch_label = gtk4::Label::new(Some(tr(Key::RenameUpdateLinksSwitch)));
    switch_label.set_xalign(0.0);
    switch_label.set_hexpand(true);
    switch_row.append(&switch_label);
    switch_row.append(&update_switch);
    body.append(&switch_row);

    let preview = gtk4::Label::new(Some(tr(Key::RenameCalculatingLinks)));
    preview.add_css_class("dim-label");
    preview.add_css_class("caption");
    preview.set_xalign(0.0);
    preview.set_wrap(true);
    body.append(&preview);

    let cancel = gtk4::Button::with_label(tr(Key::Cancel));
    let confirm = gtk4::Button::with_label(tr(Key::RenameConfirm));
    confirm.add_css_class("suggested-action");
    header.pack_start(&cancel);
    header.pack_end(&confirm);
    {
        let dlg_weak = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    let generation = Rc::new(Cell::new(0u64));
    let recompute: Rc<dyn Fn()> = {
        let generation = generation.clone();
        let handle = Arc::clone(&handle);
        let entry_weak = entry.downgrade();
        let preview_weak = preview.downgrade();
        let switch_weak = update_switch.downgrade();
        let parent = parent.clone();
        let current_name = current_name.clone();
        let path = path.clone();
        Rc::new(move || {
            let (Some(entry), Some(preview), Some(update_switch)) = (
                entry_weak.upgrade(),
                preview_weak.upgrade(),
                switch_weak.upgrade(),
            ) else {
                return;
            };
            let name = entry.text().to_string();
            let new_path = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            generation.set(generation.get() + 1);
            let my_generation = generation.get();
            if name.trim().is_empty() {
                preview.set_label(tr(Key::ErrNameEmpty));
                return;
            }
            if name == current_name {
                preview.set_label(tr(Key::RenameNameUnchanged));
                return;
            }
            if !update_switch.is_active() {
                preview.set_label(tr(Key::RenameNoLinkUpdates));
                return;
            }
            preview.set_label(tr(Key::RenameCalculatingLinks));
            let handle_bg = Arc::clone(&handle);
            let entry_path = path.clone();
            let new_path_bg = new_path;
            let gen_weak = Rc::downgrade(&generation);
            let preview_done = preview.downgrade();
            glib::MainContext::default().spawn_local(async move {
                glib::timeout_future(Duration::from_millis(180)).await;
                if gen_weak.upgrade().is_none_or(|g| g.get() != my_generation) {
                    return;
                }
                let result = gio::spawn_blocking(move || {
                    if is_dir {
                        handle_bg.plan_folder_rename_links(entry_path, new_path_bg)
                    } else {
                        handle_bg.plan_rename_links(entry_path, new_path_bg)
                    }
                })
                .await
                .unwrap_or(RenameLinkPlanInfo {
                    affected_notes: 0,
                    total_rewrites: 0,
                    files: Vec::new(),
                });
                if gen_weak.upgrade().is_none_or(|g| g.get() != my_generation) {
                    return;
                }
                let Some(preview_done) = preview_done.upgrade() else {
                    return;
                };
                let text = if result.affected_notes == 0 {
                    tr(Key::RenameNoReferrers).to_string()
                } else {
                    fmt2(
                        Key::RenameWillUpdateLinks,
                        result.affected_notes,
                        result.total_rewrites,
                    )
                };
                preview_done.set_label(&text);
            });
        })
    };
    {
        let recompute = recompute.clone();
        entry.connect_changed(move |_| recompute());
    }
    {
        let recompute = recompute.clone();
        update_switch.connect_active_notify(move |_| recompute());
    }
    recompute();

    {
        let dlg_weak = dialog.downgrade();
        let entry_weak = entry.downgrade();
        let switch_weak = update_switch.downgrade();
        let confirm_weak = confirm.downgrade();
        let submit = move || {
            let (Some(dialog_done), Some(entry_done), Some(switch_done), Some(confirm_btn)) = (
                dlg_weak.upgrade(),
                entry_weak.upgrade(),
                switch_weak.upgrade(),
                confirm_weak.upgrade(),
            ) else {
                return;
            };
            let name = entry_done.text().to_string();
            if name.trim().is_empty() || name == current_name {
                return;
            }
            confirm_btn.set_sensitive(false);
            let _ = dialog_done.close();
            on_confirm(name, switch_done.is_active());
        };
        let submit_btn = submit.clone();
        confirm.connect_clicked(move |_| submit_btn());
        entry.connect_activate(move |_| submit());
    }
    dialog.present(Some(window));
    entry.grab_focus();
}

/// 移到回收站前的二次确认；`dirty_tabs` 非零时明确提示会先保存哪些打开中的改动。
pub fn confirm_trash(
    window: &adw::ApplicationWindow,
    path: String,
    is_dir: bool,
    dirty_tabs: usize,
    on_confirm: TrashConfirm,
) {
    let title = if is_dir {
        tr(Key::TrashFolderHeading)
    } else {
        tr(Key::TrashNoteHeading)
    };
    let mut body = if is_dir {
        fmt1(Key::TrashBodyFolder, &path)
    } else {
        fmt1(Key::TrashBodyNote, &path)
    };
    if dirty_tabs > 0 {
        body.push_str(&fmt1(Key::TrashBodyDirtySuffix, dirty_tabs));
    }
    let dialog = adw::AlertDialog::builder()
        .heading(title)
        .body(&body)
        .build();
    dialog.add_response("cancel", tr(Key::Cancel));
    dialog.add_response("trash", tr(Key::MoveToTrash));
    dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "trash" {
            on_confirm();
        }
    });
    dialog.present(Some(window));
}
