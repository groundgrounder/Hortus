import SwiftUI

struct PanelContent: View {
    @Environment(Workspace.self) private var workspace
    let kind: ViewKind

    var body: some View {
        if kind == .files {
            if workspace.root != nil {
                VStack(spacing: 0) {
                    HStack {
                        Text(URL(fileURLWithPath: workspace.root!).lastPathComponent)
                            .font(.caption.weight(.medium)).foregroundStyle(.secondary).lineLimit(1)
                        Spacer()
                        if workspace.scanning { ProgressView().controlSize(.mini).accessibilityLabel(L10n.string("Scanning folder")) }
                    }.padding(.horizontal, 14).padding(.vertical, 6)
                    FileTree()
                        .overlay {
                            if workspace.entries.isEmpty && !workspace.scanning {
                                Text("No Markdown notes").font(.callout).foregroundStyle(.secondary)
                            }
                        }
                }
            } else {
                VStack(spacing: 12) {
                    Image(systemName: "folder").font(.title).foregroundStyle(.tertiary)
                    Button("Open Knowledge Base…") { workspace.chooseFolder() }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        } else if kind == .search {
            SearchPanel()
        } else if kind == .outline {
            OutlinePanel()
        } else if kind == .terminal {
            TerminalPanel(session: workspace.terminal, fontSize: workspace.fontSize)
        } else {
            VStack(spacing: 8) {
                Image(systemName: kind.symbol).font(.title2).foregroundStyle(.tertiary)
                Text("Not available yet").font(.callout).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityLabel(L10n.string("%@, not available yet", kind.title))
        }
    }
}
