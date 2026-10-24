import AppKit
import SwiftUI

struct EditorTabs: View {
    @Environment(Workspace.self) private var workspace

    var maximumWidth: CGFloat
    @State private var contentWidth: CGFloat = 100

    var body: some View {
        if !workspace.tabs.isEmpty {
            ScrollViewReader { proxy in
                ScrollView(.horizontal) {
                    tabContent
                        .fixedSize()
                        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { contentWidth = $0 }
                }
                .scrollIndicators(.hidden)
                .frame(width: min(contentWidth, maximumWidth))
                .fixedSize(horizontal: false, vertical: true)
                .onAppear {
                    if let path = workspace.selected { proxy.scrollTo(path, anchor: .trailing) }
                }
                .onChange(of: workspace.selected) { _, path in
                    if let path { proxy.scrollTo(path, anchor: .trailing) }
                }
                .onChange(of: maximumWidth) { _, _ in
                    if let path = workspace.selected { proxy.scrollTo(path, anchor: .trailing) }
                }
                .onChange(of: contentWidth) { _, _ in
                    if let path = workspace.selected { proxy.scrollTo(path, anchor: .trailing) }
                }
            }
        }
    }

    private var tabContent: some View {
        HStack(spacing: 2) {
            ForEach(workspace.tabs) { tab in
                HStack(spacing: 7) {
                    Button { workspace.openNote(tab.path) } label: {
                        Text(tab.title).lineLimit(1)
                            .font(.callout.weight(tab.path == workspace.selected ? .medium : .regular))
                            .foregroundStyle(tab.path == workspace.selected ? .primary : .secondary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(L10n.string("Open note %@", tab.title))
                    .accessibilityAddTraits(tab.path == workspace.selected ? .isSelected : [])
                    Button { workspace.closeTab(tab.path) } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .medium)).foregroundStyle(.secondary)
                    }
                .buttonStyle(.plain).accessibilityLabel(L10n.string("Close %@", tab.title))
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(tab.path == workspace.selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
                .help(tab.path)
                .id(tab.path)
            }
        }.padding(4)
    }
}

struct EditorView: View {
    @Environment(Workspace.self) private var workspace
    let insets: EdgeInsets

    var body: some View {
        let note = workspace.current
        let readable = note?.loading == false && note?.error == nil ? note : nil
        VStack(spacing: 0) {
            if let document = readable?.document { DocumentNotice(document: document) }
            ZStack {
                if let document = readable?.document, document.reading {
                    ReadingText(document: document, path: readable?.path ?? "", vault: workspace.root ?? "",
                                fontSize: workspace.fontSize, service: workspace.diagrams, core: workspace.diagramCore) { ordinal, nodes in
                        if let path = readable?.path { workspace.rememberDiagram(note: path, block: ordinal, collapsed: nodes) }
                    }
                } else {
                NoteText(text: readable?.text ?? "", path: readable?.path ?? "", vault: workspace.root ?? "", fontSize: workspace.fontSize,
                             service: workspace.diagrams, core: workspace.diagramCore, document: readable?.document) { ordinal, nodes in
                    if let path = readable?.path { workspace.rememberDiagram(note: path, block: ordinal, collapsed: nodes) }
                }
                .opacity(readable == nil ? 0 : 1)
                .allowsHitTesting(readable != nil)
                .accessibilityHidden(readable == nil)
                }
                if workspace.root == nil {
                    EmptyWorkspace()
                } else if let note {
                    if note.loading { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
                    else if let error = note.error {
                        ContentUnavailableView("Unable to Read Note", systemImage: "doc.badge.ellipsis", description: Text(error))
                    }
                } else {
                    ContentUnavailableView("Select a note", systemImage: "doc.text", description: Text("Open a Markdown or Mermaid note from the file tree"))
                }
            }
        }
        .background(ContentFrameProbe(name: "editor-column"))
        .padding(.leading, insets.leading).padding(.trailing, insets.trailing)
        .padding(.bottom, insets.bottom).padding(.top, insets.top)
    }
}

private struct DocumentNotice: View {
    let document: NoteDocument
    var body: some View {
        if document.issue != .none || document.readOnlyReason != nil {
            VStack(alignment: .leading, spacing: 6) {
                Text(L10n.documentMessage(issue: document.issue, readOnlyReason: document.readOnlyReason, raw: document.message))
                    .foregroundStyle(.secondary).textSelection(.enabled)
                HStack(spacing: 16) {
                    switch document.issue {
                    case .conflict:
                        Button("Use Disk Version") { Task { await document.useDisk() } }
                        Button("Keep Mine") { Task { await document.saveNow(.keepMine) } }
                        Button("Save Conflict Copy") { Task { await document.saveNow(.conflictCopy) } }
                    case .missing:
                        Button("Save to Original Path") { Task { await document.saveNow(.recreate) } }
                        CloseMissingNote()
                    case .recovery:
                        Button("Restore") { Task { await document.recover(true) } }.disabled(document.readOnlyReason != nil)
                        Button("Discard") { Task { await document.recover(false) } }
                    case .saveFailed:
                        Button("Retry Save") { Task { await document.saveNow() } }
                    case .none: EmptyView()
                    }
                }.buttonStyle(.link).disabled(document.marked)
            }
            .font(.callout).frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 30).padding(.vertical, 10)
        }
    }
}

private struct CloseMissingNote: View {
    @Environment(Workspace.self) private var workspace
    var body: some View { Button("Close Tab") { workspace.closeTab() } }
}

private struct EmptyWorkspace: View {
    @Environment(Workspace.self) private var workspace
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Hortus").font(.largeTitle.weight(.semibold))
                Text("Open a knowledge base to start reading.").foregroundStyle(.secondary)
            }
            Button("Open Knowledge Base…") { workspace.chooseFolder() }
                .buttonStyle(.borderedProminent).controlSize(.large)
                .disabled(workspace.opening)
            if !workspace.recent.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Recent").font(.callout.weight(.medium)).foregroundStyle(.secondary)
                    ForEach(workspace.recent) { recent in
                        Button { Task { await workspace.openFolder(recent.path) } } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(URL(fileURLWithPath: recent.path).lastPathComponent)
                                Text(recent.path).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                            }.contentShape(Rectangle())
                        }.buttonStyle(.plain)
                    }
                }
            }
        }
        .frame(maxWidth: 460, alignment: .leading)
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
