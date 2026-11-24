// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 目录树视图与 `VaultNode`：彻底消除非安全 GObject quark 存储（G-09 修复），改由 `expander.list_row()` 取节点。

use crate::core::{TreeEntryItem, is_note_file_name, sort_tree_entries};
use gtk4::gio;
use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use hortus_ffi::TreeBatch;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VaultNode {
        pub path: RefCell<String>,
        pub name: RefCell<String>,
        pub directory: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VaultNode {
        const NAME: &'static str = "HortusVaultNode";
        type Type = super::VaultNode;
    }

    impl ObjectImpl for VaultNode {}
}

glib::wrapper! {
    pub struct VaultNode(ObjectSubclass<imp::VaultNode>);
}

impl VaultNode {
    pub fn new(entry: &TreeEntryItem) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().path.borrow_mut() = entry.path.clone();
        *obj.imp().name.borrow_mut() = entry.name.clone();
        *obj.imp().directory.borrow_mut() = entry.directory;
        obj
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }

    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn is_directory(&self) -> bool {
        *self.imp().directory.borrow()
    }
}

type OpenNoteCallback = Rc<dyn Fn(String)>;
/// 右键菜单回调：锚点控件、相对路径、是否目录。
type ContextMenuCallback = Rc<dyn Fn(gtk4::Widget, String, bool)>;

struct VaultTreeInner {
    widget: gtk4::ScrolledWindow,
    root_store: gio::ListStore,
    selection: gtk4::SingleSelection,
    entries: RefCell<BTreeMap<String, TreeEntryItem>>,
    dir_stores: RefCell<HashMap<String, gio::ListStore>>,
    on_open_note: RefCell<Option<OpenNoteCallback>>,
    on_context_menu: RefCell<Option<ContextMenuCallback>>,
}

#[derive(Clone)]
pub struct VaultTree {
    inner: Rc<VaultTreeInner>,
}

impl Default for VaultTree {
    fn default() -> Self {
        Self::new()
    }
}

impl VaultTree {
    pub fn new() -> Self {
        let root_store = gio::ListStore::new::<VaultNode>();
        let inner = Rc::new(VaultTreeInner {
            widget: gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .vexpand(true)
                .build(),
            root_store: root_store.clone(),
            selection: gtk4::SingleSelection::new(None::<gio::ListModel>),
            entries: RefCell::new(BTreeMap::new()),
            dir_stores: RefCell::new(HashMap::from([(String::new(), root_store.clone())])),
            on_open_note: RefCell::new(None),
            on_context_menu: RefCell::new(None),
        });

        let weak_for_model = Rc::downgrade(&inner);
        let tree_model = gtk4::TreeListModel::new(root_store, false, false, move |item| {
            let inner = weak_for_model.upgrade()?;
            let node = item.downcast_ref::<VaultNode>()?;
            if !node.is_directory() {
                return None;
            }
            let path = node.path();
            let store = {
                inner
                    .dir_stores
                    .borrow_mut()
                    .entry(path.clone())
                    .or_insert_with(gio::ListStore::new::<VaultNode>)
                    .clone()
            };
            let objects = {
                let entries = inner.entries.borrow();
                build_store_children(&path, &entries)
            };
            replace_store_items(&store, &objects);
            Some(store.upcast())
        });

        inner.selection.set_model(Some(&tree_model));
        inner.selection.set_autoselect(false);
        inner.selection.set_can_unselect(true);

        let factory = gtk4::SignalListItemFactory::new();
        {
            let weak_cb = Rc::downgrade(&inner);
            factory.connect_setup(move |_, list_item| {
                let Some(list_item) = list_item.downcast_ref::<gtk4::ListItem>() else {
                    return;
                };
                let expander = gtk4::TreeExpander::new();
                let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                row_box.set_margin_top(2);
                row_box.set_margin_bottom(2);
                let icon = gtk4::Image::new();
                let label = gtk4::Label::new(None);
                label.set_halign(gtk4::Align::Start);
                label.set_hexpand(true);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                row_box.append(&icon);
                row_box.append(&label);
                expander.set_child(Some(&row_box));
                list_item.set_child(Some(&expander));

                // 直接从 TreeExpander::list_row() 取 VaultNode，零 unsafe GObject quark。
                let click = gtk4::GestureClick::new();
                click.set_button(3);
                let weak_inner = weak_cb.clone();
                click.connect_pressed(move |gesture, _, _, _| {
                    let Some(inner) = weak_inner.upgrade() else {
                        return;
                    };
                    let Some(expander) = gesture.widget().and_downcast::<gtk4::TreeExpander>()
                    else {
                        return;
                    };
                    let Some(row) = expander.list_row() else {
                        return;
                    };
                    let Some(node) = row.item().and_downcast::<VaultNode>() else {
                        return;
                    };
                    let anchor = expander
                        .child()
                        .unwrap_or_else(|| expander.clone().upcast());
                    let handler = inner.on_context_menu.borrow().clone();
                    if let Some(handler) = handler {
                        handler(anchor, node.path(), node.is_directory());
                        gesture.set_state(gtk4::EventSequenceState::Claimed);
                    }
                });
                expander.add_controller(click);
            });
        }

        factory.connect_bind(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk4::ListItem>() else {
                return;
            };
            let Some(expander) = list_item.child().and_downcast::<gtk4::TreeExpander>() else {
                return;
            };
            let Some(tree_row) = list_item.item().and_downcast::<gtk4::TreeListRow>() else {
                return;
            };
            expander.set_list_row(Some(&tree_row));
            let Some(node) = tree_row.item().and_downcast::<VaultNode>() else {
                return;
            };
            let Some(row_box) = expander.child().and_downcast::<gtk4::Box>() else {
                return;
            };
            let Some(icon) = row_box.first_child().and_downcast::<gtk4::Image>() else {
                return;
            };
            let Some(label) = icon.next_sibling().and_downcast::<gtk4::Label>() else {
                return;
            };
            if node.is_directory() {
                icon.set_icon_name(Some("folder-symbolic"));
            } else {
                icon.set_icon_name(Some("x-office-document-symbolic"));
            }
            label.set_text(&node.name());
        });

        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk4::ListItem>()
                && let Some(expander) = list_item.child().and_downcast::<gtk4::TreeExpander>()
            {
                expander.set_list_row(None);
            }
        });

        let list_view = gtk4::ListView::new(Some(inner.selection.clone()), Some(factory));
        list_view.add_css_class("hortus-tree-list");
        list_view.add_css_class("navigation-sidebar");
        list_view.set_single_click_activate(true);

        {
            let weak_open = Rc::downgrade(&inner);
            list_view.connect_activate(move |_, pos| {
                let Some(inner) = weak_open.upgrade() else {
                    return;
                };
                let Some(tree_row) = inner
                    .selection
                    .item(pos)
                    .and_downcast::<gtk4::TreeListRow>()
                else {
                    return;
                };
                let Some(node) = tree_row.item().and_downcast::<VaultNode>() else {
                    return;
                };
                if node.is_directory() {
                    tree_row.set_expanded(!tree_row.is_expanded());
                } else if is_note_file_name(&node.name()) {
                    let cb = inner.on_open_note.borrow().clone();
                    if let Some(cb) = cb {
                        cb(node.path());
                    }
                }
            });
        }

        inner.widget.set_child(Some(&list_view));
        Self { inner }
    }

    pub fn widget(&self) -> &gtk4::ScrolledWindow {
        &self.inner.widget
    }

    pub fn set_on_context_menu(&self, callback: impl Fn(gtk4::Widget, String, bool) + 'static) {
        *self.inner.on_context_menu.borrow_mut() = Some(Rc::new(callback));
    }

    /// 当前选中的目录（用于新建笔记/文件夹的默认位置）；选中笔记时返回其父目录。
    pub fn selected_directory(&self) -> Option<(String, bool)> {
        let row = self
            .inner
            .selection
            .selected_item()
            .and_downcast::<gtk4::TreeListRow>()?;
        let node = row.item().and_downcast::<VaultNode>()?;
        let path = node.path();
        if node.is_directory() {
            return Some((path, true));
        }
        let parent = path
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        Some((parent, false))
    }

    pub fn set_on_open_note(&self, callback: impl Fn(String) + 'static) {
        *self.inner.on_open_note.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn clear(&self) {
        self.inner.entries.borrow_mut().clear();
        let stores: Vec<gio::ListStore> = {
            let mut map = self.inner.dir_stores.borrow_mut();
            let list = map.values().cloned().collect();
            map.clear();
            map.insert(String::new(), self.inner.root_store.clone());
            list
        };
        for store in stores {
            store.remove_all();
        }
    }

    pub fn apply_batches(&self, batches: &[TreeBatch]) -> (bool, u64) {
        let mut scan_complete = false;
        let mut scan_micros = 0u64;
        let mut dirty_parents: BTreeSet<String> = BTreeSet::new();

        {
            let mut entries = self.inner.entries.borrow_mut();
            let mut stores = self.inner.dir_stores.borrow_mut();
            for batch in batches {
                if batch.scan_complete {
                    scan_complete = true;
                    scan_micros = scan_micros.max(batch.scan_micros);
                }
                for removed in &batch.removed {
                    if let Some(old) = entries.remove(removed) {
                        dirty_parents.insert(old.parent);
                    }
                    stores.remove(removed);
                }
                for entry in &batch.upserted {
                    dirty_parents.insert(entry.parent.clone());
                    entries.insert(entry.path.clone(), TreeEntryItem::from(entry));
                }
            }
        }

        if !dirty_parents.is_empty() {
            let pending: Vec<(gio::ListStore, Vec<VaultNode>)> = {
                let entries = self.inner.entries.borrow();
                let stores = self.inner.dir_stores.borrow();
                dirty_parents
                    .into_iter()
                    .filter_map(|parent| {
                        stores
                            .get(&parent)
                            .cloned()
                            .map(|store| (store, build_store_children(&parent, &entries)))
                    })
                    .collect()
            };
            for (store, objects) in pending {
                replace_store_items(&store, &objects);
            }
        }

        (scan_complete, scan_micros)
    }

    #[cfg(feature = "automation")]
    pub fn stats(&self) -> (usize, usize, Vec<String>) {
        let entries = self.inner.entries.borrow();
        let mut note_paths: Vec<String> = entries
            .values()
            .filter(|e| !e.directory)
            .map(|e| e.path.clone())
            .collect();
        note_paths.sort();
        (entries.len(), note_paths.len(), note_paths)
    }

    pub fn select_path(&self, target_path: &str) {
        let Some(model) = self
            .inner
            .selection
            .model()
            .and_downcast::<gtk4::TreeListModel>()
        else {
            return;
        };
        let ancestors: Vec<String> = {
            let entries = self.inner.entries.borrow();
            let mut chain = Vec::new();
            let mut cur = entries.get(target_path).map(|e| e.parent.clone());
            while let Some(parent) = cur {
                if parent.is_empty() {
                    break;
                }
                chain.push(parent.clone());
                cur = entries.get(&parent).map(|e| e.parent.clone());
            }
            chain.reverse();
            chain
        };

        for ancestor in ancestors {
            let count = model.n_items();
            for idx in 0..count {
                if let Some(row) = model.item(idx).and_downcast::<gtk4::TreeListRow>()
                    && let Some(node) = row.item().and_downcast::<VaultNode>()
                    && node.path() == ancestor
                    && !row.is_expanded()
                {
                    row.set_expanded(true);
                    break;
                }
            }
        }

        let count = model.n_items();
        for idx in 0..count {
            if let Some(row) = model.item(idx).and_downcast::<gtk4::TreeListRow>()
                && let Some(node) = row.item().and_downcast::<VaultNode>()
                && node.path() == target_path
            {
                self.inner.selection.set_selected(idx);
                break;
            }
        }
    }
}

fn build_store_children(parent: &str, entries: &BTreeMap<String, TreeEntryItem>) -> Vec<VaultNode> {
    let mut children: Vec<TreeEntryItem> = entries
        .values()
        .filter(|entry| entry.parent == parent)
        .cloned()
        .collect();
    sort_tree_entries(&mut children);
    children.iter().map(VaultNode::new).collect()
}

fn replace_store_items(store: &gio::ListStore, objects: &[VaultNode]) {
    store.splice(0, store.n_items(), objects);
}
