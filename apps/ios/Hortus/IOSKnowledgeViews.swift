import Foundation
import HortusApple
import HortusCore
import SwiftUI

struct IOSKnowledgeBrowser: View {
    @Bindable var appModel: IOSAppModel
    var showTabPicker: Bool = true

    var body: some View {
        VStack(spacing: 0) {
            if showTabPicker {
                Picker("", selection: $appModel.selectedTab) {
                    ForEach(KnowledgeTab.allCases) { tab in
                        Image(systemName: tab.systemImage)
                            .accessibilityLabel(tab.title)
                            .tag(tab)
                    }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .accessibilityIdentifier("knowledge-tab-picker")
            }

            switch appModel.selectedTab {
            case .files:
                IOSFilesList(appModel: appModel)
            case .outline:
                IOSOutlineList(appModel: appModel)
            case .backlinks:
                IOSBacklinksList(appModel: appModel)
            case .tags:
                IOSTagsList(appModel: appModel)
            case .search:
                IOSSearchList(appModel: appModel)
            }
        }
    }
}

struct IOSFilesList: View {
    @Bindable var appModel: IOSAppModel

    var body: some View {
        List {
            if appModel.vaultHandle == nil {
                Section {
                    Button {
                        appModel.fileImporterPresented = true
                    } label: {
                        Label(L10n.string("Open Folder…"), systemImage: "folder")
                    }
                    if appModel.hasBundledSampleFolder() {
                        Button {
                            appModel.importSampleFolder()
                        } label: {
                            Label(L10n.string("Import Sample Folder"), systemImage: "doc.on.doc")
                        }
                    }
                }
            } else if appModel.entries.isEmpty {
                ContentUnavailableView(L10n.string("No Markdown notes"), systemImage: "doc.text")
            } else {
                ForEach(appModel.sortedTreeEntries.filter { !$0.directory }, id: \.path) { entry in
                    Button {
                        appModel.openNote(entry.path)
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: entry.path.lowercased().hasSuffix(".mmd") ? "point.3.connected.trianglepath.dotted" : "doc.text")
                                .foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(entry.name)
                                    .lineLimit(1)
                                if !entry.parent.isEmpty {
                                    Text(entry.parent)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            Spacer()
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(appModel.currentPath == entry.path ? Color.accentColor.opacity(0.12) : nil)
                    .accessibilityIdentifier("file-\(entry.path)")
                }
            }
        }
        .listStyle(.insetGrouped)
    }
}

struct IOSOutlineList: View {
    @Bindable var appModel: IOSAppModel

    var body: some View {
        if let doc = appModel.currentDocument {
            if doc.headings.isEmpty {
                ContentUnavailableView(L10n.string("No headings"), systemImage: "list.bullet.indent")
            } else {
                List(doc.headings, id: \.start) { heading in
                    Button {
                        appModel.revealHeading(heading)
                    } label: {
                        Text(heading.title)
                            .font(heading.level <= 2 ? .body.weight(.medium) : .callout)
                            .foregroundStyle(heading.level <= 2 ? .primary : .secondary)
                            .padding(.leading, CGFloat(max(0, Int(heading.level) - 1)) * 14)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("outline-\(heading.start)")
                }
                .listStyle(.plain)
            }
        } else {
            ContentUnavailableView(L10n.string("No note open"), systemImage: "doc.text")
        }
    }
}

struct IOSBacklinksList: View {
    @Bindable var appModel: IOSAppModel
    @State private var entries: [BacklinkInfo] = []

    var body: some View {
        let path = appModel.currentPath
        let version = appModel.knowledgeVersion
        Group {
            if path == nil {
                ContentUnavailableView(L10n.string("No note open"), systemImage: "arrow.uturn.backward")
            } else if entries.isEmpty {
                ContentUnavailableView(L10n.string("No backlinks"), systemImage: "arrow.uturn.backward")
            } else {
                let groups = Dictionary(grouping: entries, by: \.source)
                List {
                    ForEach(groups.keys.sorted(), id: \.self) { source in
                        Section(source) {
                            ForEach(groups[source] ?? [], id: \.startUtf16) { entry in
                                Button {
                                    appModel.openBacklink(entry)
                                } label: {
                                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                                        Text("\(entry.line)")
                                            .font(.caption.monospacedDigit())
                                            .foregroundStyle(.secondary)
                                        if entry.inDiagram {
                                            Image(systemName: "point.3.connected.trianglepath.dotted")
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }
                                        if entry.embed {
                                            Image(systemName: "arrow.down.right.square")
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }
                                        Text(entry.context)
                                            .font(.callout)
                                            .lineLimit(2)
                                    }
                                }
                                .buttonStyle(.plain)
                                .accessibilityIdentifier("backlink-\(entry.source)-\(entry.line)")
                            }
                        }
                    }
                }
                .listStyle(.insetGrouped)
            }
        }
        .task(id: "\(path ?? "")-\(version)") {
            guard let path, let handle = appModel.vaultHandle else {
                entries = []
                return
            }
            let found = await Task.detached { handle.backlinks(path: path) }.value
            if !Task.isCancelled { entries = found }
        }
    }
}

struct IOSTagsList: View {
    @Bindable var appModel: IOSAppModel
    @State private var rows: [TagPanelRow] = []
    @State private var notes: [String] = []

    var body: some View {
        let query = appModel.tagFilter.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        let visible = rows.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }
        List {
            Section {
                TextField(L10n.string("Filter tags"), text: $appModel.tagFilter)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityIdentifier("tags-filter-field")
            }
            if visible.isEmpty {
                Section {
                    Text(L10n.string("No matching tags"))
                        .foregroundStyle(.secondary)
                }
            } else {
                Section {
                    ForEach(visible) { row in
                        Button {
                            appModel.selectedTag = (appModel.selectedTag == row.id) ? nil : row.id
                        } label: {
                            HStack {
                                Text("#" + row.name)
                                    .padding(.leading, CGFloat(min(row.depth, 5)) * 10)
                                Spacer()
                                Text("\(row.count)")
                                    .monospacedDigit()
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(appModel.selectedTag == row.id ? Color.accentColor.opacity(0.12) : nil)
                        .accessibilityIdentifier("tag-\(row.id)")
                    }
                }
            }
            if let selected = appModel.selectedTag {
                Section("#" + selected) {
                    ForEach(notes, id: \.self) { path in
                        Button {
                            appModel.openNote(path)
                        } label: {
                            Label(path, systemImage: "doc.text")
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("tag-note-\(path)")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .task(id: "\(appModel.selectedTag ?? "")-\(appModel.knowledgeVersion)-\(appModel.vaultRoot ?? "")") {
            guard let handle = appModel.vaultHandle else {
                rows = []
                notes = []
                return
            }
            let selected = appModel.selectedTag
            let (updated, matching) = await Task.detached {
                (TagPanelRow.rows(handle.tags()), selected.map { handle.notesWithTag(tag: $0) } ?? [])
            }.value
            if !Task.isCancelled {
                rows = updated
                notes = matching
            }
        }
    }
}

struct IOSSearchList: View {
    @Bindable var appModel: IOSAppModel

    var body: some View {
        List {
            Section {
                TextField(L10n.string("Search Notes"), text: Binding(
                    get: { appModel.searchQuery },
                    set: { appModel.runSearch(query: $0) }
                ))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .accessibilityIdentifier("search-query-field")
            }

            if !appModel.quickOpenHits.isEmpty {
                Section(L10n.string("Quick Open")) {
                    ForEach(appModel.quickOpenHits, id: \.path) { hit in
                        Button {
                            appModel.openNote(hit.path)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(hit.title)
                                    .font(.body.weight(.medium))
                                Text(hit.path)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("quick-open-\(hit.path)")
                    }
                }
            }

            if !appModel.searchResults.isEmpty {
                Section(L10n.string("Search")) {
                    ForEach(appModel.searchResults, id: \.path) { result in
                        ForEach(result.matches.indices, id: \.self) { index in
                            let match = result.matches[index]
                            Button {
                                appModel.openNote(result.path, sourceOffset: Int(match.start), selectionLength: Int(match.length))
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(result.title)
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(.secondary)
                                    Text(match.snippet.replacingOccurrences(of: "\n", with: " "))
                                        .font(.callout)
                                        .lineLimit(2)
                                }
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("search-hit-\(result.path)-\(index)")
                        }
                    }
                }
            } else if !appModel.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && appModel.quickOpenHits.isEmpty {
                Section {
                    Text(L10n.string("No matching notes"))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .listStyle(.insetGrouped)
    }
}
