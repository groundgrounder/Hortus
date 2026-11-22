import SwiftUI
import HortusCore

struct TagPanelRow: Identifiable {
    let id: String
    let name: String
    let count: Int
    var depth: Int { id.split(separator: "/").count - 1 }

    static func rows(_ summaries: [TagSummaryInfo]) -> [Self] {
        var names: [String: String] = [:]
        var notes: [String: Set<String>] = [:]
        for summary in summaries {
            var prefix = ""
            var raw = ""
            for (key, label) in zip(summary.tag.split(separator: "/"), summary.raw.split(separator: "/")) {
                if !prefix.isEmpty { prefix += "/"; raw += "/" }
                prefix += key
                raw += label
                if names[prefix] == nil { names[prefix] = raw }
                notes[prefix, default: []].formUnion(summary.notes)
            }
        }
        return notes.keys.sorted { $0.split(separator: "/").lexicographicallyPrecedes($1.split(separator: "/")) }
            .map { Self(id: $0, name: names[$0] ?? $0, count: notes[$0]?.count ?? 0) }
    }
}

struct TagsPanel: View {
    @Environment(Workspace.self) private var workspace
    @State private var rows: [TagPanelRow] = []
    @State private var notes: [String] = []

    var body: some View {
        @Bindable var workspace = workspace
        let query = workspace.tagFilter.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        let visible = rows.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }
        let selected = workspace.selectedTag
        VStack(alignment: .leading, spacing: 8) {
            TextField(L10n.string("Filter tags"), text: $workspace.tagFilter)
                .textFieldStyle(.roundedBorder).controlSize(.small)
                .padding(.horizontal, 12)
            if visible.isEmpty {
                Text(L10n.string("No matching tags"))
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 1) {
                        ForEach(visible) { row in
                            Button { workspace.selectedTag = row.id } label: {
                                HStack(spacing: 6) {
                                    Text("#" + row.name).lineLimit(1).truncationMode(.middle)
                                    Spacer(minLength: 4)
                                    Text(row.count, format: .number)
                                        .monospacedDigit().foregroundStyle(.secondary)
                                }
                                .padding(.leading, CGFloat(min(row.depth, 5)) * 9)
                                .padding(.horizontal, 6).padding(.vertical, 4)
                                .background(selected == row.id ? Color.primary.opacity(0.08) : .clear,
                                            in: RoundedRectangle(cornerRadius: 5))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("tag-" + row.id)
                            .accessibilityAddTraits(selected == row.id ? .isSelected : [])
                            .help("#" + row.name)
                            #if DEBUG
                            .background {
                                if KnowledgeUIInspection.requested {
                                    ContentFrameProbe(name: "tag-" + row.id)
                                }
                            }
                            #endif
                        }
                    }.padding(.horizontal, 8)
                }
                .frame(minHeight: 45)
            }
            if let selected {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("#" + selected).lineLimit(1).truncationMode(.middle)
                            .font(.caption.weight(.semibold)).accessibilityAddTraits(.isHeader)
                        Spacer(minLength: 4)
                        Button { workspace.selectedTag = nil } label: {
                            Image(systemName: "xmark.circle.fill")
                        }
                        .buttonStyle(.borderless)
                        .help(L10n.string("Clear selected tag"))
                        .accessibilityLabel(L10n.string("Clear selected tag"))
                    }
                    ScrollView {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(notes, id: \.self) { path in
                                Button { workspace.openNote(path, savePrevious: false) } label: {
                                    Label(path, systemImage: "doc.text").lineLimit(1)
                                        .truncationMode(.middle).frame(maxWidth: .infinity, alignment: .leading)
                                        .padding(.vertical, 4).contentShape(Rectangle())
                                }
                                .buttonStyle(.plain).help(path)
                                .accessibilityIdentifier("tag-note-" + path)
                                #if DEBUG
                                .background {
                                    if KnowledgeUIInspection.requested {
                                        ContentFrameProbe(name: "tag-note-" + path)
                                    }
                                }
                                #endif
                            }
                        }
                    }.frame(minHeight: 30)
                }.padding(.horizontal, 12)
            }
        }
        .font(.callout).padding(.vertical, 8)
        .task(id: Query(tag: selected, version: workspace.context?.knowledgeVersion,
                        context: workspace.context.map(ObjectIdentifier.init))) {
            guard let handle = workspace.context?.handle else { rows = []; notes = []; return }
            let (updated, matching) = await Task.detached {
                (TagPanelRow.rows(handle.tags()), selected.map { handle.notesWithTag(tag: $0) } ?? [])
            }.value
            guard !Task.isCancelled else { return }
            rows = updated
            notes = matching
        }
        .onChange(of: selected) { notes = [] }
    }

    private struct Query: Equatable {
        let tag: String?
        let version: Int?
        let context: ObjectIdentifier?
    }
}
