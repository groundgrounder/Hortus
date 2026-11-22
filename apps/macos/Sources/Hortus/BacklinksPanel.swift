import SwiftUI
import HortusCore

struct BacklinksPanel: View {
    @Environment(Workspace.self) private var workspace
    @State private var entries: [BacklinkInfo] = []

    var body: some View {
        let path = workspace.current?.path
        let version = workspace.context?.knowledgeVersion
        VStack(alignment: .leading, spacing: 8) {
            Text(L10n.string("Backlinks (%lld)", Int64(entries.count)))
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .accessibilityAddTraits(.isHeader)
            if entries.isEmpty {
                Text(L10n.string(path == nil ? "Open a note to see its backlinks" : "No backlinks"))
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        let groups = Dictionary(grouping: entries, by: \.source)
                        ForEach(groups.keys.sorted(), id: \.self) { source in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(source).font(.caption.weight(.semibold))
                                    .lineLimit(1).truncationMode(.middle).help(source)
                                    .accessibilityAddTraits(.isHeader)
                                ForEach(groups[source] ?? [], id: \.startUtf16) { entry in
                                    Button {
                                        workspace.openBacklink(entry)
                                    } label: {
                                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                                            Text(entry.line, format: .number)
                                                .monospacedDigit().foregroundStyle(.secondary)
                                            if entry.inDiagram {
                                                Image(systemName: "point.3.connected.trianglepath.dotted")
                                                    .help(L10n.string("Mermaid diagram"))
                                                    .accessibilityLabel(L10n.string("Mermaid diagram"))
                                            }
                                            if entry.embed {
                                                Image(systemName: "arrow.down.right.square")
                                                    .help(L10n.string("Embedded link"))
                                                    .accessibilityLabel(L10n.string("Embedded link"))
                                            }
                                            Text(entry.context).lineLimit(2)
                                            Spacer(minLength: 0)
                                        }
                                        .font(.caption).padding(.vertical, 4)
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .accessibilityIdentifier("backlink-\(entry.source)-\(entry.line)")
                                    .help(L10n.string("Line %lld", Int64(entry.line)) + " · " + entry.context)
                                    #if DEBUG
                                    .background {
                                        if KnowledgeUIInspection.requested {
                                            ContentFrameProbe(name: "backlink-\(entry.source)-\(entry.line)")
                                        }
                                    }
                                    #endif
                                }
                            }
                        }
                    }
                    .padding(.horizontal, 12)
                }
            }
        }
        .padding(.vertical, 8)
        .task(id: Query(path: path, version: version, context: workspace.context.map(ObjectIdentifier.init))) {
            guard let path, let handle = workspace.context?.handle else { entries = []; return }
            let found = await Task.detached { handle.backlinks(path: path) }.value
            guard !Task.isCancelled else { return }
            entries = found
        }
        .onChange(of: path) { entries = [] }
    }

    private struct Query: Equatable {
        let path: String?
        let version: Int?
        let context: ObjectIdentifier?
    }
}
