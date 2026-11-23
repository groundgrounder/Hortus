// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

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

type OpenNoteCallback = Rc<RefCell<Option<Box<dyn Fn(String)>>>>;
/// 右键菜单回调：锚点控件、相对路径、是否目录。
type ContextMenuCallback = Rc<dyn Fn(gtk4::Widget, String, bool)>;

#[derive(Clone)]
pub struct VaultTree {
    pub widget: gtk4::ScrolledWindow,
    list_view: gtk4::ListView,
    root_store: gio::ListStore,
    selection: gtk4::SingleSelection,
    entries: Rc<RefCell<BTreeMap<String, TreeEntryItem>>>,
    dir_stores: Rc<RefCell<HashMap<String, gio::ListStore>>>,
    on_open_note: OpenNoteCallback,
    on_context_menu: Rc<RefCell<Option<ContextMenuCallback>>>,
}

impl VaultTree {
    pub fn new() -> Self {
        let root_store = gio::ListStore::new::<VaultNode>();
        let entries: Rc<RefCell<BTreeMap<String, TreeEntryItem>>> =
            Rc::new(RefCell::new(BTreeMap::new()));
        let dir_stores: Rc<RefCell<HashMap<String, gio::ListStore>>> =
            Rc::new(RefCell::new(HashMap::new()));
        dir_stores
            .borrow_mut()
            .insert(String::new(), root_store.clone());

        let entries_for_model = entries.clone();
        let stores_for_model = dir_stores.clone();
        let tree_model = gtk4::TreeListModel::new(root_store.clone(), false, false, move |item| {
            let node = item.downcast_ref::<VaultNode>()?;
            if !node.is_directory() {
                return None;
            }
            let path = node.path();
            let store = {
                stores_for_model
                    .borrow_mut()
                    .entry(path.clone())
                    .or_insert_with(gio::ListStore::new::<VaultNode>)
                    .clone()
            };
            let objects = {
                let entries = entries_for_model.borrow();
                build_store_children(&path, &entries)
            };
            replace_store_items(&store, &objects);
            Some(store.upcast())
        });

        let selection = gtk4::SingleSelection::new(Some(tree_model));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let on_context_menu: Rc<RefCell<Option<ContextMenuCallback>>> = Rc::new(RefCell::new(None));

        let factory = gtk4::SignalListItemFactory::new();
        {
            let cb = on_context_menu.clone();
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

                // 右键菜单：每个行工厂持有一份当前路径，由列表项绑定阶段更新。
                let target: Rc<RefCell<Option<(String, bool)>>> = Rc::new(RefCell::new(None));
                unsafe {
                    list_item.set_data("hortus-context-target", target.clone());
                }
                let click = gtk4::GestureClick::new();
                click.set_button(3);
                let anchor = row_box.clone();
                let cb = cb.clone();
                click.connect_pressed(move |gesture, _, _, _| {
                    let Some((path, is_dir)) = target.borrow().clone() else {
                        return;
                    };
                    if let Some(handler) = cb.borrow().as_ref() {
                        handler(anchor.clone().upcast::<gtk4::Widget>(), path, is_dir);
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
            unsafe {
                if let Some(target) =
                    list_item.data::<Rc<RefCell<Option<(String, bool)>>>>("hortus-context-target")
                {
                    *target.as_ref().borrow_mut() = Some((node.path(), node.is_directory()));
                }
            }
            if node.is_directory() {
                icon.set_icon_name(Some("folder-symbolic"));
            } else {
                icon.set_icon_name(Some("x-office-document-symbolic"));
            }
            label.set_text(&node.name());
        });

        let list_view = gtk4::ListView::new(Some(selection.clone()), Some(factory));
        list_view.add_css_class("hortus-tree-list");
        list_view.add_css_class("navigation-sidebar");
        list_view.set_single_click_activate(true);

        let on_open_note: OpenNoteCallback = Rc::new(RefCell::new(None));
        let on_open_cb = on_open_note.clone();
        let selection_for_activate = selection.clone();
        list_view.connect_activate(move |_, pos| {
            let Some(tree_row) = selection_for_activate
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
            } else if is_note_file_name(&node.name())
                && let Some(cb) = on_open_cb.borrow().as_ref()
            {
                cb(node.path());
            }
        });

        let widget = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .child(&list_view)
            .build();

        Self {
            widget,
            list_view,
            root_store,
            selection,
            entries,
            dir_stores,
            on_open_note,
            on_context_menu,
        }
    }

    pub fn set_on_context_menu(&self, callback: impl Fn(gtk4::Widget, String, bool) + 'static) {
        *self.on_context_menu.borrow_mut() = Some(Rc::new(callback));
    }

    /// 当前选中的目录（用于新建笔记/文件夹的默认位置）；选中笔记时返回其父目录。
    pub fn selected_directory(&self) -> Option<(String, bool)> {
        let row = self
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
        *self.on_open_note.borrow_mut() = Some(Box::new(callback));
    }

    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
        let stores: Vec<gio::ListStore> = {
            let mut map = self.dir_stores.borrow_mut();
            let list = map.values().cloned().collect();
            map.clear();
            map.insert(String::new(), self.root_store.clone());
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
            let mut entries = self.entries.borrow_mut();
            let mut stores = self.dir_stores.borrow_mut();
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
            // 先在只读借用作用域内构建待替换的节点列表，避免 GListModel 回调重入 RefCell。
            let pending: Vec<(gio::ListStore, Vec<VaultNode>)> = {
                let entries = self.entries.borrow();
                let stores = self.dir_stores.borrow();
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

        let _ = &self.list_view;
        let _ = &self.selection;
        (scan_complete, scan_micros)
    }

    pub fn stats(&self) -> (usize, usize, Vec<String>) {
        let entries = self.entries.borrow();
        let mut note_paths: Vec<String> = entries
            .values()
            .filter(|e| !e.directory)
            .map(|e| e.path.clone())
            .collect();
        note_paths.sort();
        (entries.len(), note_paths.len(), note_paths)
    }

    pub fn select_path(&self, target_path: &str) {
        let Some(model) = self.selection.model().and_downcast::<gtk4::TreeListModel>() else {
            return;
        };
        // 先展开父目录链再选中节点，确保文件树和当前标签同步。
        let ancestors: Vec<String> = {
            let entries = self.entries.borrow();
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
                self.selection.set_selected(idx);
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
