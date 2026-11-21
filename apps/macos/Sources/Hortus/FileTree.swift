import AppKit
import SwiftUI
import HortusCore

/// 原生大纲控件提供方向键、展开折叠、选择和 VoiceOver 树语义。
struct FileTree: NSViewRepresentable {
    @Environment(Workspace.self) private var workspace

    func makeCoordinator() -> Coordinator { Coordinator(workspace: workspace) }

    func makeNSView(context: Context) -> NSScrollView {
        let outline = DrawingOutlineView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.rowHeight = 26
        outline.intercellSpacing = NSSize(width: 0, height: 1)
        outline.indentationPerLevel = 15
        outline.style = .sourceList
        outline.backgroundColor = .clear
        outline.allowsEmptySelection = true
        outline.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        outline.delegate = context.coordinator
        outline.dataSource = context.coordinator
        outline.setAccessibilityLabel(L10n.string("File tree"))
        outline.didDraw = { [weak workspace] in workspace?.treeDidDraw() }
        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.contentInsets = NSEdgeInsets(top: 4, left: 4, bottom: 4, right: 4)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let outline = scroll.documentView as? DrawingOutlineView else { return }
        context.coordinator.update(outline, version: workspace.treeVersion, selected: workspace.folderSelection ?? workspace.selected, root: workspace.root)
    }

    @MainActor final class Node: NSObject {
        var entry: TreeEntry
        var children: [Node] = []
        init(_ entry: TreeEntry) { self.entry = entry }
    }

    @MainActor final class Coordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
        let workspace: Workspace
        var nodes: [String: Node] = [:]
        var roots: [Node] = []
        var version = -1
        var selected: String?
        var pendingSelection = false
        var root: String?
        var updating = false
        init(workspace: Workspace) { self.workspace = workspace }

        func update(_ outline: NSOutlineView, version: Int, selected: String?, root: String?) {
            updating = true
            defer { updating = false }
            if self.root != root {
                nodes = [:]; roots = []; self.root = root
                self.selected = nil
                pendingSelection = false
            }
            if self.version != version {
                let expanded = nodes.values.filter { outline.isItemExpanded($0) }.map { $0.entry.path }
                let focused = (outline.item(atRow: outline.selectedRow) as? Node)?.entry.path
                nodes = nodes.filter { workspace.entries[$0.key] != nil }
                for (path, entry) in workspace.entries {
                    if let node = nodes[path] { node.entry = entry; node.children = [] }
                    else { nodes[path] = Node(entry) }
                }
                roots = []
                for node in nodes.values {
                    if let parent = nodes[node.entry.parent] { parent.children.append(node) }
                    else if node.entry.parent.isEmpty { roots.append(node) }
                }
                func sort(_ list: inout [Node]) {
                    list.sort {
                        if $0.entry.directory != $1.entry.directory { return $0.entry.directory }
                        return $0.entry.name.localizedStandardCompare($1.entry.name) == .orderedAscending
                    }
                }
                sort(&roots)
                for node in nodes.values { sort(&node.children) }
                outline.reloadData()
                for path in expanded { if let node = nodes[path] { outline.expandItem(node) } }
                if let focused, let node = nodes[focused] {
                    let row = outline.row(forItem: node)
                    if row >= 0 { outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false) }
                }
                self.version = version
                outline.needsDisplay = true
            }
            if self.selected != selected {
                self.selected = selected
                pendingSelection = selected != nil
                if selected == nil { outline.deselectAll(nil) }
            }
            // 标签可能先于扫描结果恢复，等目标节点到达后再展开并选中。
            if pendingSelection, let selected, let node = nodes[selected] {
                var parent = node.entry.parent
                var chain: [Node] = []
                while let item = nodes[parent] { chain.insert(item, at: 0); parent = item.entry.parent }
                for item in chain { outline.expandItem(item) }
                let row = outline.row(forItem: node)
                if row >= 0 {
                    outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                    outline.scrollRowToVisible(row)
                    pendingSelection = false
                }
            }
        }

        func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
            (item as? Node)?.children.count ?? roots.count
        }
        func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
            ((item as? Node)?.children ?? roots)[index]
        }
        func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
            (item as? Node)?.entry.directory ?? false
        }
        func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
            guard let node = item as? Node else { return nil }
            let id = NSUserInterfaceItemIdentifier("file")
            let cell = outlineView.makeView(withIdentifier: id, owner: self) as? NSTableCellView ?? NSTableCellView()
            if cell.textField == nil {
                cell.identifier = id
                let text = NSTextField(labelWithString: "")
                text.font = .systemFont(ofSize: 13)
                text.lineBreakMode = .byTruncatingMiddle
                text.translatesAutoresizingMaskIntoConstraints = false
                let image = NSImageView()
                image.translatesAutoresizingMaskIntoConstraints = false
                image.contentTintColor = .secondaryLabelColor
                cell.addSubview(image); cell.addSubview(text)
                cell.textField = text; cell.imageView = image
                NSLayoutConstraint.activate([
                    image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                    image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                    image.widthAnchor.constraint(equalToConstant: 16), image.heightAnchor.constraint(equalToConstant: 16),
                    text.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
                    text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
                    text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                ])
            }
            cell.textField?.stringValue = node.entry.name
            cell.imageView?.image = NSImage(systemSymbolName: node.entry.directory ? "folder" : "doc.text", accessibilityDescription: nil)
            cell.setAccessibilityLabel(L10n.string("%@, %@", node.entry.name, node.entry.directory ? L10n.string("Folder") : L10n.string("Note")))
            cell.toolTip = node.entry.path
            return cell
        }
        /// 点选文件夹只更新新建目标（`folderSelection`）；点选笔记打开并把新建目标交回标签选中。
        /// 取消选中同样清空文件夹目标，避免顶栏按钮继续落在旧文件夹。
        func outlineViewSelectionDidChange(_ notification: Notification) {
            guard !updating, let outline = notification.object as? NSOutlineView else { return }
            guard outline.selectedRow >= 0,
                  let node = outline.item(atRow: outline.selectedRow) as? Node else {
                workspace.folderSelection = nil
                return
            }
            if node.entry.directory {
                workspace.folderSelection = node.entry.path
            } else {
                workspace.openNote(node.entry.path)
            }
        }

        /// 右键菜单：笔记项与文件夹项提供新建、重命名与移到废纸篓；
        /// 空白区域只提供库根的新建。右键先选中目标行，与 ⌘⌫ 保持一致。
        func menu(for event: NSEvent, outline: NSOutlineView) -> NSMenu? {
            let location = outline.convert(event.locationInWindow, from: nil)
            let row = outline.row(at: location)
            let node = row >= 0 ? outline.item(atRow: row) as? Node : nil
            if row >= 0, node != nil, outline.selectedRow != row {
                outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            }
            let entry = node?.entry
            let parent = entry.map { $0.directory ? $0.path : $0.parent } ?? ""
            let menu = NSMenu()
            let newNote = NSMenuItem(title: L10n.string("New Note"), action: #selector(newNoteUnder), keyEquivalent: "")
            newNote.target = self
            newNote.representedObject = parent
            menu.addItem(newNote)
            let newFolder = NSMenuItem(title: L10n.string("New Folder"), action: #selector(newFolderUnder), keyEquivalent: "")
            newFolder.target = self
            newFolder.representedObject = parent
            menu.addItem(newFolder)
            if let entry {
                menu.addItem(.separator())
                let rename = NSMenuItem(title: L10n.string("Rename"), action: #selector(renameEntry), keyEquivalent: "")
                rename.target = self
                rename.representedObject = entry.path
                menu.addItem(rename)
                let trash = NSMenuItem(title: L10n.string("Move to Trash"), action: #selector(trashEntry), keyEquivalent: "")
                trash.target = self
                trash.representedObject = entry.path
                menu.addItem(trash)
            }
            return menu.items.isEmpty ? nil : menu
        }

        @objc private func newNoteUnder(_ item: NSMenuItem) {
            workspace.requestNewNote(parent: item.representedObject as? String ?? "")
        }
        @objc private func newFolderUnder(_ item: NSMenuItem) {
            workspace.requestNewFolder(parent: item.representedObject as? String ?? "")
        }
        @objc private func renameEntry(_ item: NSMenuItem) {
            workspace.requestRename(path: item.representedObject as? String ?? "")
        }
        @objc private func trashEntry(_ item: NSMenuItem) {
            workspace.requestTrash(path: item.representedObject as? String ?? "")
        }

        /// 仅当大纲自身是 firstResponder 且有选中项时，⌘⌫ 触发移到废纸篓；
        /// 绝不干扰正文编辑器与终端的删除键。
        func trashSelection(outline: NSOutlineView) -> Bool {
            guard outline.selectedRow >= 0,
                  let node = outline.item(atRow: outline.selectedRow) as? Node else { return false }
            workspace.requestTrash(path: node.entry.path)
            return true
        }
     }
 }
 
 @MainActor final class DrawingOutlineView: NSOutlineView {
     var didDraw: (() -> Void)?
    override func menu(for event: NSEvent) -> NSMenu? {
        (delegate as? FileTree.Coordinator)?.menu(for: event, outline: self) ?? super.menu(for: event)
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.type == .keyDown, event.modifierFlags.contains(.command),
           event.charactersIgnoringModifiers == "\u{7f}",
           self === window?.firstResponder,
           let coordinator = delegate as? FileTree.Coordinator {
            return coordinator.trashSelection(outline: self)
        }
        return super.performKeyEquivalent(with: event)
    }
     override func draw(_ dirtyRect: NSRect) {
         super.draw(dirtyRect)
         if numberOfRows > 0 { DispatchQueue.main.async { [weak self] in self?.didDraw?() } }
     }
 }

