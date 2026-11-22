import AppKit
import HortusCore
import SwiftUI

@MainActor
final class LinkCompletion {
    private struct Request: Equatable {
        let path: String
        let start: Int
        let query: String
        let caret: Int
        let hasCloser: Bool
        let embed: Bool

        var replacement: NSRange {
            NSRange(location: start + 2, length: caret - start - 2 + (hasCloser ? 2 : 0))
        }

        @MainActor init?(view: DiagramTextView) {
            let selection = view.selectedRange()
            let text = view.string as NSString
            guard selection.length == 0, selection.location >= 2, selection.location <= text.length else { return nil }
            let line = text.lineRange(for: NSRange(location: selection.location, length: 0))
            let prefix = NSRange(location: line.location, length: selection.location - line.location)
            let opening = text.range(of: "[[", options: .backwards, range: prefix)
            guard opening.location != NSNotFound else { return nil }
            let query = text.substring(with: NSRange(location: NSMaxRange(opening), length: selection.location - NSMaxRange(opening)))
            guard query.utf16.count <= 120, !query.contains(where: { "[]#|".contains($0) }) else { return nil }
            path = view.representedPath
            start = opening.location
            caret = selection.location
            self.query = query
            hasCloser = caret + 2 <= text.length
                && text.substring(with: NSRange(location: caret, length: 2)) == "]]"
            embed = start > 0 && text.character(at: start - 1) == 33
        }
    }

    private final class SuggestionPanel: NSPanel {
        override var canBecomeKey: Bool { false }
        override var canBecomeMain: Bool { false }
    }

    private var panel: SuggestionPanel?
    private weak var editor: DiagramTextView?
    private var request: Request?
    private var dismissed: Request?
    private var work: Task<Void, Never>?
    private(set) var results: [LinkCompletionInfo] = []
    private(set) var selected = 0
    var onScreen: Bool { panel?.isVisible == true }
    #if DEBUG
    var snapshotWindow: NSWindow? { panel }
    #endif

    func update(view: DiagramTextView, handle: VaultHandle?, edited: Bool) {
        guard view.isEditable, !view.compositionActive, let handle, let window = view.window,
              window.firstResponder === view, !view.representedPath.lowercased().hasSuffix(".mmd"),
              let next = Request(view: view) else {
            close()
            return
        }
        if !edited, request?.start != next.start { close(); return }
        if next == request || next == dismissed { return }
        dismissed = nil
        request = next
        editor = view
        view.onLinkCompletionState?(true)
        work?.cancel()
        hidePanel()
        let candidate = NSMutableString(string: view.string)
        if !next.hasCloser {
            candidate.insert(next.query.isEmpty ? "x]]" : "]]", at: next.caret)
        } else if next.query.isEmpty {
            candidate.insert("x", at: next.caret)
        }
        let text = candidate as String
        let start = UInt64(next.start - (next.embed ? 1 : 0))
        work = Task { [weak self, weak view] in
            do { try await Task.sleep(for: .milliseconds(35)) } catch { return }
            let hits = await Task.detached {
                handle.linkCompletionsAt(text: text, linkStart: start, query: next.query, limit: 7)
            }.value
            guard let self, let view, !Task.isCancelled, request == next,
                  !view.compositionActive, view.isEditable, Request(view: view) == next,
                  view.window?.firstResponder === view else { return }
            results = hits.filter { !$0.insertTarget.contains(where: { "[]#|".contains($0) }) }
            selected = 0
            if results.isEmpty { view.onLinkCompletionState?(false) }
            else { show(in: view) }
        }
    }

    func close() {
        work?.cancel(); work = nil
        request = nil; results = []
        editor?.onLinkCompletionState?(false)
        editor = nil
        hidePanel()
    }

    private func hidePanel() {
        if let panel {
            panel.parent?.removeChildWindow(panel)
            panel.orderOut(nil)
        }
        panel = nil
    }

    func keyDown(_ event: NSEvent, in view: DiagramTextView) -> Bool {
        guard !view.compositionActive, request != nil,
              event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty else { return false }
        switch event.keyCode {
        case 53:
            dismissed = request
            let visible = onScreen
            close()
            return visible
        case 125, 126:
            guard onScreen, !results.isEmpty else { return false }
            let delta = event.keyCode == 125 ? 1 : -1
            selected = (selected + delta + results.count) % results.count
            show(in: view)
            return true
        case 36, 48, 76:
            guard onScreen else { return false }
            return accept(selected, in: view)
        default: return false
        }
    }

    @discardableResult
    private func accept(_ row: Int, in view: DiagramTextView) -> Bool {
        guard let request, request == Request(view: view), results.indices.contains(row),
              view.isEditable, !view.compositionActive else { close(); return false }
        let replacement = results[row].insertTarget + "]]"
        let range = request.replacement
        close()
        view.breakUndoCoalescing()
        view.insertText(replacement, replacementRange: range)
        view.breakUndoCoalescing()
        return true
    }

    private func show(in view: DiagramTextView) {
        guard let window = view.window, let request else { return }
        let caret = view.firstRect(forCharacterRange: NSRange(location: request.caret, length: 0), actualRange: nil)
        guard !caret.isEmpty || caret.height > 0, let screen = window.screen?.visibleFrame else { return }
        let width: CGFloat = 260, height = CGFloat(results.count) * 30 + 8
        let origin = CGPoint(x: min(max(screen.minX, caret.minX), screen.maxX - width),
                             y: caret.minY - height - 4 >= screen.minY ? caret.minY - height - 4 : caret.maxY + 4)
        let rect = CGRect(origin: origin, size: CGSize(width: width, height: height))
        let popup: SuggestionPanel
        if let panel {
            popup = panel
        } else {
            popup = SuggestionPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel],
                                    backing: .buffered, defer: false)
            popup.isOpaque = false
            popup.backgroundColor = .clear
            popup.hasShadow = true
            popup.appearance = view.effectiveAppearance
            panel = popup
        }
        popup.contentView = NSHostingView(rootView: LinkCompletionRows(hits: results, selected: selected) { [weak self, weak view] index in
            if let view { self?.accept(index, in: view) }
        })
        popup.setFrame(rect, display: true)
        if popup.parent == nil { window.addChildWindow(popup, ordered: .above) }
        popup.orderFront(nil)
    }
}

private struct LinkCompletionRows: View {
    let hits: [LinkCompletionInfo]
    let selected: Int
    let choose: (Int) -> Void

    var body: some View {
        VStack(spacing: 0) {
            ForEach(hits.indices, id: \.self) { index in
                let hit = hits[index]
                Button { choose(index) } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "doc.text").foregroundStyle(.secondary)
                        Text(hit.title).lineLimit(1)
                        Spacer(minLength: 4)
                        if let alias = hit.matchedAlias {
                            Text(alias).foregroundStyle(.secondary).lineLimit(1)
                        } else if hit.path.contains("/") {
                            Text((hit.path as NSString).deletingLastPathComponent)
                                .foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                        }
                    }
                    .padding(.horizontal, 8).frame(height: 30)
                    .background(index == selected ? Color.accentColor.opacity(0.14) : .clear,
                                in: RoundedRectangle(cornerRadius: 5))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(index == selected ? .isSelected : [])
            }
        }
        .padding(4).font(.callout)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 9))
        .accessibilityLabel(L10n.string("Link suggestions"))
    }
}
