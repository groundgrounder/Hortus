import AppKit
import SwiftUI
import Testing
@testable import Hortus

@Suite @MainActor
struct EditorTabsTests {
    @Test func capsuleFitsContentAndOverflowScrollsWithinTheColumn() async throws {
        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md")]
        workspace.selected = "note.md"
        var width: CGFloat = 0
        let host = NSHostingView(rootView: HStack(spacing: 0) {
            EditorTabs(maximumWidth: 500)
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            Spacer(minLength: 0)
        }.environment(workspace))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 44),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await settle(host)
        #expect(width > 50 && width < 200)

        workspace.selected = "long-note-title-11.md"
        workspace.tabs = (0..<12).map { NoteTab(path: "long-note-title-\($0).md") }
        try await settle(host)
        #expect(width > 490 && width <= 500)
        let scroll = try #require(scrollViews(in: host).first { ($0.documentView?.frame.width ?? 0) > $0.contentSize.width })
        #expect(scroll.frame.width <= 500)
        // 允许末端胶囊内边距，最后一个标签本身必须完整可见。
        #expect(scroll.documentVisibleRect.maxX >= (scroll.documentView?.frame.maxX ?? 0) - 8)
        scroll.documentView?.scroll(.zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        let before = scroll.documentVisibleRect.minX
        scroll.documentView?.scroll(NSPoint(x: 200, y: 0))
        scroll.reflectScrolledClipView(scroll.contentView)
        #expect(scroll.documentVisibleRect.minX > before)

        workspace.selected = "long-note-title-0.md"
        try await settle(host)
        #expect(scroll.documentVisibleRect.minX < 1)

        for tab in workspace.tabs.dropFirst() { workspace.closeTab(tab.path) }
        try await settle(host)
        #expect(width > 50 && width < 250)
        workspace.closeTab()
        try await settle(host)
        #expect(workspace.tabs.isEmpty)
    }

    private func settle(_ host: NSView) async throws {
        // 仅等待一次系统布局提交，不用于收取文件库批次。
        try await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()
        host.displayIfNeeded()
    }

    private func scrollViews(in view: NSView) -> [NSScrollView] {
        (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap { scrollViews(in: $0) }
    }
}
