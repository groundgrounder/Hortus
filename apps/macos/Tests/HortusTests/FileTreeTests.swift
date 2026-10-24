import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct FileTreeTests {
    @Test func selectionWaitsForScanAndPreservesKeyboardFolderFocus() {
        let workspace = Workspace()
        let coordinator = FileTree.Coordinator(workspace: workspace)
        let outline = NSOutlineView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.dataSource = coordinator
        outline.delegate = coordinator
        let path = "folder/note.md"
        coordinator.update(outline, version: 0, selected: path, root: "/test")
        #expect(outline.selectedRow == -1)
        workspace.entries = [
            "folder": TreeEntry(path: "folder", parent: "", name: "folder", directory: true),
            path: TreeEntry(path: path, parent: "folder", name: "note.md", directory: false),
        ]
        coordinator.update(outline, version: 1, selected: path, root: "/test")
        #expect(outline.isItemExpanded(coordinator.nodes["folder"]))
        #expect((outline.item(atRow: outline.selectedRow) as? FileTree.Node)?.entry.path == path)
        outline.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        coordinator.update(outline, version: 2, selected: path, root: "/test")
        #expect((outline.item(atRow: outline.selectedRow) as? FileTree.Node)?.entry.directory == true)
    }
}
