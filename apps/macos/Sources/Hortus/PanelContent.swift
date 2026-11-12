import SwiftUI

struct PanelContent: View {
    @Environment(Workspace.self) private var workspace
    let tool: ToolID

    var body: some View {
        switch tool {
        case .tool(.files):
            files
        case .tool(.search):
            SearchPanel()
        case .tool(.outline):
            OutlinePanel()
        case .terminal(let id):
            if let tab = workspace.terminals.tab(id) {
                TerminalPanel(tab: tab, fontSize: workspace.fontSize)
            } else {
                terminalEmptyState
            }
        case .legacy:
            EmptyView()
        }
    }

    private var files: some View {
        Group {
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
        }
    }

    private var terminalEmptyState: some View {
        VStack(spacing: 10) {
            Image(systemName: "terminal").font(.title2).foregroundStyle(.tertiary)
            Text(L10n.string("Open a knowledge base to use the terminal"))
                .font(.callout).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(L10n.string("Open a knowledge base to use the terminal"))
    }
}
