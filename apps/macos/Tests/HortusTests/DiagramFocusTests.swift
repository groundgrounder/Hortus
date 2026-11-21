import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct DiagramFocusTests {
    @Test func zoomIsStrictlyClampedToQuarterAndFour() {
        #expect(DiagramFocusLimits.clamp(0.01) == 0.25)
        #expect(DiagramFocusLimits.clamp(0.25) == 0.25)
        #expect(DiagramFocusLimits.clamp(1.75) == 1.75)
        #expect(DiagramFocusLimits.clamp(4) == 4)
        #expect(DiagramFocusLimits.clamp(12) == 4)
    }

    private func preparedScene() throws -> PreparedScene {
        let source = "mindmap\n  root((Root))\n    first(First)\n      second(Second)\n        third(Third)\n          fourth(Fourth)\n    other(Other)\n"
        let scene = try DiagramEngine().render(source: source, mmd: true, block: 1, width: 420,
                                               dark: false, collapsed: [])
        let offsets = DiagramSourceOffsets(source)
        var ranges: [String: NSRange] = [:]
        for hit in scene.hitRegions {
            if let range = offsets.range(start: hit.sourceStart, end: hit.sourceEnd) { ranges[hit.nodeId] = range }
        }
        return PreparedScene(scene: try DiagramScene(scene), sourceRanges: ranges)
    }

    @Test func highlightOnlyInvalidatesDrawingAndKeepsTheScene() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse) else {
            Issue.record("没有可折叠节点")
            return
        }
        view.needsDisplay = false
        view.highlightedNodeID = hit.nodeId
        #expect(view.prepared === prepared)
        #expect(view.highlightedNodeID == hit.nodeId)
        view.highlightedNodeID = nil
        #expect(view.highlightedNodeID == nil)
    }

    @Test func accessibilityCustomActionSelectsNodeSource() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse),
              let expected = prepared.sourceRanges[hit.nodeId] else {
            Issue.record("没有可折叠节点")
            return
        }
        var jumped: NSRange?
        view.onSourceRange = { jumped = $0 }
        let children = view.accessibilityChildren() as? [NSAccessibilityElement] ?? []
        guard let action = children.prefix(view.accessibilityNodeCount)
            .compactMap({ $0.accessibilityCustomActions()?.first }).first else {
            Issue.record("节点没有跳到源码动作")
            return
        }
        #expect(action.name == L10n.string("Jump to Source"))
        #expect(action.handler?() == true)
        #expect(jumped == expected)
    }

    @Test func commandClickOnCollapsibleNodeJumpsInsteadOfToggling() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        view.frame = NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view
        defer { window.close() }
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse),
              let expected = prepared.sourceRanges[hit.nodeId] else {
            Issue.record("没有可折叠节点")
            return
        }
        var toggled = false, jumped: NSRange?
        view.onToggle = { _ in toggled = true }
        view.onSourceRange = { jumped = $0 }
        let point = view.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        guard let event = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [.command],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1) else {
            Issue.record("无法构造鼠标事件")
            return
        }
        view.mouseDown(with: event)
        #expect(!toggled)
        #expect(jumped == expected)
        // 普通单击仍折叠可折叠节点，不回归。
        guard let plain = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1) else { return }
        toggled = false
        view.mouseDown(with: plain)
        #expect(toggled)
    }

    @Test func contextMenuOffersJumpCollapseAndFocus() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        view.frame = NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view
        defer { window.close() }
        var focused = false
        view.onFocus = { focused = true }
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse) else {
            Issue.record("没有可折叠节点")
            return
        }
        let point = view.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        guard let event = NSEvent.mouseEvent(with: .rightMouseDown, location: point, modifierFlags: [],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1),
              let menu = view.menu(for: event) else {
            Issue.record("没有右键菜单")
            return
        }
        let titles = menu.items.filter { !$0.isSeparatorItem }.map(\.title)
        #expect(titles.contains(L10n.string("Jump to Source")))
        #expect(titles.contains(L10n.string("Collapse")))
        #expect(titles.contains(L10n.string("Focus Mode")))
        if let item = menu.items.first(where: { $0.title == L10n.string("Focus Mode") }),
           let action = item.action, let target = item.target as? NSObject {
            _ = target.perform(action, with: item)
        }
        #expect(focused)
    }
}
