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
        context.coordinator.update(outline, version: workspace.treeVersion, selected: workspace.selected, root: workspace.root)
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
        func outlineViewSelectionDidChange(_ notification: Notification) {
            guard !updating, let outline = notification.object as? NSOutlineView,
                  let node = outline.item(atRow: outline.selectedRow) as? Node, !node.entry.directory else { return }
            workspace.openNote(node.entry.path)
        }
    }
}

@MainActor final class DrawingOutlineView: NSOutlineView {
    var didDraw: (() -> Void)?
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if numberOfRows > 0 { DispatchQueue.main.async { [weak self] in self?.didDraw?() } }
    }
}
