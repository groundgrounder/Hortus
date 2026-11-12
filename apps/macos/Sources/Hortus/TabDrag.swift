import AppKit
import SwiftUI

/// 一次拖动只有一个令牌；释放时再次核对源窗口与文档所有权。
struct TabDragPayload: Codable, Equatable {
    let token: UUID
    let window: String
    let path: String
}

extension ApplicationWorkspace {
    func beginTabDrag(workspace: Workspace, path: String) -> TabDragPayload? {
        guard let record = records.first(where: { $0.workspace === workspace }), !record.closing,
              let tab = workspace.tabs.first(where: { $0.path == path }), !tab.loading,
              let document = tab.document, !document.marked, !workspace.closingTabs.contains(path), !workspace.preparingClose,
              !draggingDocuments.contains(ObjectIdentifier(document)) else { return nil }
        let payload = TabDragPayload(token: UUID(), window: record.id, path: path)
        tabDrags[payload.token] = (payload, document)
        #if DEBUG
        tabDragTrace.append("begin \(payload.window) \(path) \(payload.token)")
        #endif
        return payload
    }

    var draggingDocuments: Set<ObjectIdentifier> { Set(tabDrags.values.map { ObjectIdentifier($0.1) }) }

    func cancelTabDrag(_ payload: TabDragPayload) {
        if tabDrags.removeValue(forKey: payload.token) != nil {
            #if DEBUG
            tabDragTrace.append("cancel \(payload.token)")
            #endif
        }
    }

    func cancelTabDrags(in workspace: Workspace, path: String? = nil) {
        guard let record = records.first(where: { $0.workspace === workspace }) else { return }
        for (payload, _) in tabDrags.values where payload.window == record.id && (path == nil || payload.path == path) {
            cancelTabDrag(payload)
        }
    }

    @discardableResult func finishTabDrag(_ payload: TabDragPayload, at point: NSPoint) -> WindowRecord? {
        #if DEBUG
        tabDragTrace.append("release \(payload.token) at \(NSStringFromPoint(point))")
        #endif
        guard let pending = tabDrags.removeValue(forKey: payload.token), pending.0 == payload,
              let source = records.first(where: { $0.id == payload.window }), !source.closing,
              let context = source.workspace.context,
              source.workspace.tabs.contains(where: { $0.path == payload.path && $0.document === pending.1 }),
              !NSApp.windows.contains(where: { $0.isVisible && !$0.isMiniaturized && $0.frame.contains(point) }),
              let tab = source.workspace.takeTab(payload.path) else { return nil }
        let target = WindowRecord(layout: makeLayout())
        configure(target)
        if let visible = recent.first(where: { $0.path == context.root })?.visible {
            for region in Region.allCases {
                target.layout.visible[region] = visible[region.rawValue] ?? (region != .bottom)
            }
        }
        target.lastVisible = Dictionary(uniqueKeysWithValues: target.layout.visible.map { ($0.key.rawValue, $0.value) })
        // 附着、转移与注册同步完成；异步 shell 初始化前已经只有一个所有者。
        target.workspace.context = context
        context.attach(target.workspace)
        target.workspace.tabs = [tab]; target.workspace.selected = tab.path
        if let document = tab.document { target.workspace.bind(document) }
        records.append(target)
        #if DEBUG
        tabDragTrace.append("commit \(payload.token) to \(target.id)")
        #endif
        target.workspace.terminals.setInitialRoot(context.root)
        Task { await target.workspace.ensureTerminalTools() }
        becameActive(target)
        showWindow?(target.id)
        scheduleSessionSave()
        return target
    }
}

struct TabDragButton: NSViewRepresentable {
    let workspace: Workspace
    let path: String
    let title: String
    let selected: Bool
    func makeNSView(context: Context) -> Button { Button() }
    func updateNSView(_ button: Button, context: Context) {
        button.workspace = workspace; button.path = path
        button.title = title
        button.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: selected ? .medium : .regular)
        button.contentTintColor = selected ? .labelColor : .secondaryLabelColor
        button.setAccessibilityLabel(L10n.string("Open note %@", title))
        button.setAccessibilityValue(selected ? 1 : 0)
        button.invalidateIntrinsicContentSize()
    }

    /// AppKit 跟踪循环显式区别 mouse-up 与 Escape/失活；取消没有提交落点。
    final class Button: NSButton {
        weak var workspace: Workspace?
        var path = ""
        override init(frame: NSRect) {
            super.init(frame: frame)
            isBordered = false; bezelStyle = .inline
            target = self; action = #selector(open)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        @objc private func open() { workspace?.openNote(path) }
        override func mouseDown(with event: NSEvent) {
            guard let workspace, let application = workspace.application, let sourceWindow = window else { return }
            #if DEBUG
            application.tabDragTrace.append("mouse-down \(path)")
            #endif
            let origin = event.locationInWindow
            var payload: TabDragPayload?
            var moved = false
            defer {
                if let payload { application.cancelTabDrag(payload) }
                NSCursor.arrow.set()
            }
            while sourceWindow.isVisible, NSApp.isActive,
                  application.records.contains(where: { $0.workspace === workspace && !$0.closing }) {
                guard let next = NSApp.nextEvent(matching: [.leftMouseDragged, .leftMouseUp, .keyDown, .appKitDefined],
                                                 until: Date(timeIntervalSinceNow: 0.05), inMode: .eventTracking, dequeue: true) else { continue }
                switch next.type {
                case .keyDown:
                    if next.keyCode == 53 { return }
                    NSApp.sendEvent(next)
                case .leftMouseDragged:
                    if !moved, hypot(next.locationInWindow.x - origin.x, next.locationInWindow.y - origin.y) >= 6 {
                        moved = true
                        payload = application.beginTabDrag(workspace: workspace, path: path)
                        NSCursor.closedHand.set()
                    }
                case .leftMouseUp:
                    if let payload { application.finishTabDrag(payload, at: NSEvent.mouseLocation) }
                    else if !moved, bounds.contains(convert(next.locationInWindow, from: nil)) { performClick(nil) }
                    return
                case .appKitDefined:
                    #if DEBUG
                    application.tabDragTrace.append("system-event \(next.subtype.rawValue)")
                    #endif
                    if next.subtype == .applicationDeactivated { return }
                    NSApp.sendEvent(next)
                default: break
                }
            }
        }
    }
}
