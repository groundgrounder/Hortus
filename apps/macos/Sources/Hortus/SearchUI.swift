import AppKit
import SwiftUI
import HortusCore

struct SearchInput: NSViewRepresentable {
    let placeholder: String
    let text: String
    var focusOnAppear = false
    let onChange: (String, Bool) -> Void
    let onMove: (Int) -> Void
    let onSubmit: () -> Void
    let onDismiss: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onChange: onChange) }

    func makeNSView(context: Context) -> NativeSearchField {
        let field = NativeSearchField()
        field.placeholderString = L10n.string(placeholder)
        field.isBordered = false
        field.drawsBackground = false
        field.font = .systemFont(ofSize: 15)
        field.focusOnAppear = focusOnAppear
        field.delegate = context.coordinator
        field.onMove = onMove
        field.onSubmit = onSubmit
        field.onDismiss = onDismiss
        return field
    }

    func updateNSView(_ field: NativeSearchField, context: Context) {
        if field.stringValue != text { field.stringValue = text }
        field.placeholderString = L10n.string(placeholder)
        field.focusOnAppear = focusOnAppear
        field.onMove = onMove
        field.onSubmit = onSubmit
        field.onDismiss = onDismiss
        context.coordinator.onChange = onChange
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var onChange: (String, Bool) -> Void

        init(onChange: @escaping (String, Bool) -> Void) { self.onChange = onChange }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NativeSearchField else { return }
            let marked = (field.currentEditor() as? NSTextView)?.hasMarkedText() ?? false
            onChange(field.stringValue, marked)
        }

        func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
            guard let field = control as? NativeSearchField, !textView.hasMarkedText() else { return false }
            switch commandSelector {
            case #selector(NSResponder.moveDown(_:)):
                field.onMove(1)
                return true
            case #selector(NSResponder.moveUp(_:)):
                field.onMove(-1)
                return true
            case #selector(NSResponder.insertNewline(_:)):
                field.onSubmit()
                return true
            case #selector(NSResponder.cancelOperation(_:)):
                field.onDismiss()
                return true
            default:
                return false
            }
        }
    }
}

final class NativeSearchField: NSTextField {
    var focusOnAppear = false
    var onMove: (Int) -> Void = { _ in }
    var onSubmit: () -> Void = {}
    var onDismiss: () -> Void = {}

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard focusOnAppear else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.window != nil else { return }
            self.window?.makeFirstResponder(self)
        }
    }

}

struct SearchPanel: View {
    @Environment(Workspace.self) private var workspace

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                SearchInput(placeholder: "Search notes", text: workspace.searchQuery) { query, marked in
                    workspace.updateSearch(query: query, marked: marked)
                } onMove: { _ in } onSubmit: {} onDismiss: {}
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            Divider()
            if workspace.searchLoading {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if workspace.searchQuery.isEmpty {
                    Text(L10n.string("Search notes"))
                    .font(.callout).foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if workspace.searchResults.isEmpty {
                Text(L10n.string("No matching notes"))
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(workspace.searchResults.indices), id: \.self) { resultIndex in
                            let result = workspace.searchResults[resultIndex]
                            VStack(alignment: .leading, spacing: 3) {
                                HStack(spacing: 6) {
                                    Text(result.title).font(.callout.weight(.medium)).lineLimit(1)
                                    Spacer(minLength: 4)
                                    Text(verbatim: "\(result.count)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                                }
                                Text(result.path).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                ForEach(result.matches.indices, id: \.self) { matchIndex in
                                    let match = result.matches[matchIndex]
                                    Button {
                                        workspace.openSearchResult(path: result.path, start: Int(match.start), length: Int(match.length))
                                    } label: {
                                        Text(highlighted(match))
                                            .font(.callout)
                                            .lineLimit(2)
                                            .multilineTextAlignment(.leading)
                                            .frame(maxWidth: .infinity, alignment: .leading)
                                            .padding(.vertical, 3)
                                            .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .help(L10n.string("Open matching text"))
                                }
                            }
                            .padding(.horizontal, 12).padding(.vertical, 8)
                            if resultIndex + 1 < workspace.searchResults.count { Divider().padding(.leading, 12) }
                        }
                    }
                }
            }
        }
    }

    private func highlighted(_ match: SearchTextMatch) -> AttributedString {
        // 换行、制表符换成空格，摘要排成一段；都是一换一，UTF-16 偏移不变
        let flat = match.snippet.replacingOccurrences(of: "\r", with: " ")
            .replacingOccurrences(of: "\n", with: " ").replacingOccurrences(of: "\t", with: " ")
        let text = NSMutableAttributedString(string: flat, attributes: [.foregroundColor: NSColor.labelColor])
        let start = Int(match.start) - Int(match.snippetStart)
        let range = NSRange(location: start, length: Int(match.length))
        if start >= 0, NSMaxRange(range) <= text.length {
            let attributes: [NSAttributedString.Key: Any] = [
                .backgroundColor: NSColor.selectedTextBackgroundColor.withAlphaComponent(0.4),
                .foregroundColor: NSColor.selectedTextColor,
            ]
            text.addAttributes(attributes, range: range)
        }
        return AttributedString(text)
    }
}

struct QuickOpenOverlay: View {
    @Environment(Workspace.self) private var workspace

    var body: some View {
        if workspace.quickOpenVisible {
            ZStack(alignment: .top) {
                Color.black.opacity(0.12).ignoresSafeArea().onTapGesture { workspace.dismissQuickOpen() }
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        SearchInput(placeholder: "Search by title or path", text: workspace.quickOpenQuery, focusOnAppear: true) { query, marked in
                            workspace.updateQuickOpen(query: query, marked: marked)
                        } onMove: { workspace.moveQuickOpenSelection($0) } onSubmit: {
                            workspace.activateQuickOpenSelection()
                        } onDismiss: { workspace.dismissQuickOpen() }
                    }
                    .padding(.horizontal, 16).padding(.vertical, 12)
                    if !workspace.quickOpenResults.isEmpty {
                        Divider()
                        ScrollView {
                            LazyVStack(spacing: 2) {
                                ForEach(Array(workspace.quickOpenResults.indices), id: \.self) { index in
                                    let result = workspace.quickOpenResults[index]
                                    Button {
                                        workspace.quickOpenSelection = index
                                        workspace.activateQuickOpenSelection()
                                    } label: {
                                        HStack(spacing: 10) {
                                            Image(systemName: "doc.text").foregroundStyle(.secondary).frame(width: 18)
                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(highlighted(result.title, ranges: result.titleMatches))
                                                    .font(.callout.weight(.medium)).lineLimit(1)
                                                Text(highlighted(result.path, ranges: result.pathMatches))
                                                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                            }
                                            Spacer(minLength: 0)
                                        }
                                        .padding(.horizontal, 12).padding(.vertical, 7)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .background(index == workspace.quickOpenSelection ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 5))
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .onHover { if $0 { workspace.quickOpenSelection = index } }
                                }
                            }
                            .padding(6)
                        }
                        .frame(maxHeight: 360)
                    } else if workspace.scanning {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text(L10n.string("Loading notes")).font(.callout).foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading).padding(14)
                    } else if !workspace.quickOpenQuery.isEmpty {
                        Text(L10n.string("No matching notes")).font(.callout).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(14)
                    }
                }
                .frame(width: 620)
                .background(.regularMaterial, in: .rect(cornerRadius: 10))
                .overlay { RoundedRectangle(cornerRadius: 10).strokeBorder(.separator.opacity(0.5), lineWidth: 1) }
                .shadow(color: .black.opacity(0.16), radius: 18, y: 8)
                .padding(.top, 62)
                .frame(maxWidth: .infinity)
            }
            .zIndex(100)
        }
    }

    private func highlighted(_ value: String, ranges: [SearchMatchRange]) -> AttributedString {
        // 命中处只换颜色，字号字重跟随所在行（标题、路径字号不同），未命中的部分沿用外层的前景样式
        let text = NSMutableAttributedString(string: value)
        for match in ranges {
            let range = NSRange(location: Int(match.start), length: Int(match.length))
            if NSMaxRange(range) <= text.length {
                text.addAttribute(.foregroundColor, value: NSColor.controlAccentColor, range: range)
            }
        }
        return AttributedString(text)
    }
}
