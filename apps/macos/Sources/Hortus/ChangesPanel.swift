import SwiftUI
import Foundation
import HortusCore

/// 本次会话改动面板：以启动前快照为基线，展示当前磁盘上的增删改与重命名。
/// 文案只说“本次会话改动”，不把磁盘变化全部归因于 Agent。
struct ChangesPanel: View {
    @Environment(Workspace.self) private var workspace
    @Environment(\.colorSchemeContrast) private var contrast

    private var model: ChangesModel { workspace.changes }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if let error = model.error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 6)
            }
            content
        }
        .task {
            await model.activate(preferred: workspace.currentAgentSessionID)
        }
        .task(id: model.runningSessionIDs) {
            guard !model.runningSessionIDs.isEmpty else { return }
            // 会话运行期间只触发一次后台刷新；防抖期间取消则不扫描。
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, !model.runningSessionIDs.isEmpty else { return }
            await model.refreshSessions(preferred: model.selectedSessionID)
            await model.refreshSummary()
        }
        .onChange(of: workspace.root) { _, _ in
            Task { await model.activate(preferred: workspace.currentAgentSessionID) }
        }
        .onChange(of: workspace.currentAgentSessionID) { _, id in
            guard let id else { return }
            Task { await model.activate(preferred: id) }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text(L10n.string("Session Changes"))
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                Spacer(minLength: 4)
                if model.loadingSessions || model.loadingSummary {
                    ProgressView().controlSize(.mini)
                }
                Button {
                    Task {
                        await model.refreshSessions(preferred: model.selectedSessionID)
                        await model.refreshSummary()
                    }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.callout)
                        .frame(width: 22, height: 22)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .disabled(model.loadingSessions || model.loadingSummary)
                .help(L10n.string("Refresh Changes"))
                .accessibilityLabel(L10n.string("Refresh Changes"))
            }
            if model.sessions.isEmpty {
                Text(L10n.string("No reviewable sessions"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                sessionSelector
                Text(L10n.string("Changes on disk since the session snapshot"))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 8)
        .padding(.bottom, 6)
    }

    private var sessionSelector: some View {
        Menu {
            ForEach(model.sessions, id: \.id) { session in
                Button {
                    Task { await model.select(sessionID: session.id) }
                } label: {
                    Text(sessionLabel(session))
                }
            }
        } label: {
            HStack(spacing: 5) {
                Circle()
                    .fill(stateColor(model.selectedSession?.state))
                    .frame(width: 6, height: 6)
                if let session = model.selectedSession {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(sessionStateLine(session))
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text(sessionDetailLine(session))
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                    }
                } else {
                    Text(L10n.string("Select a session"))
                        .font(.caption)
                        .lineLimit(1)
                }
                Image(systemName: "chevron.down")
                    .font(.caption2)
            }
            .foregroundStyle(.secondary)
            .contentShape(.rect)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .accessibilityLabel(L10n.string("Review session"))
        .accessibilityValue(model.selectedSession.map(sessionLabel) ?? "")
    }

    @ViewBuilder private var content: some View {
        if workspace.root == nil {
            empty(L10n.string("Open a knowledge base to review session changes"))
        } else if model.sessions.isEmpty {
            empty(L10n.string("No reviewable sessions"))
        } else if let session = model.selectedSession, !session.usable {
            VStack(spacing: 6) {
                Text(stateLabel(session.state))
                    .font(.callout.weight(.medium))
                Text(L10n.string("This session has no complete snapshot and cannot be reviewed"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                Text(L10n.string("Snapshot %@", formattedDate(session.snapshotAtMs)))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(.horizontal, 18)
        } else if let summary = model.summary {
            VStack(alignment: .leading, spacing: 0) {
                stats(summary)
                entryList(summary)
                Divider().opacity(0.35)
                detailView
            }
        } else if model.loadingSummary {
            ProgressView()
                .controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            empty(L10n.string("No changes in this session"))
        }
    }

    private func empty(_ text: String) -> some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(.horizontal, 18)
    }

    private func stats(_ summary: SessionReviewSummary) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 3) {
            GridRow {
                count(summary.counts.added, L10n.string("Added"), color: .green)
                count(summary.counts.modified, L10n.string("Modified"), color: .blue)
                count(summary.counts.deleted, L10n.string("Deleted"), color: .red)
            }
            GridRow {
                count(summary.counts.renamed, L10n.string("Renamed"), color: .orange)
                count(summary.counts.typeChanged, L10n.string("Type changed"), color: .purple)
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            L10n.string(
                "%d added, %d modified, %d deleted, %d renamed, %d type changed",
                Int(summary.counts.added), Int(summary.counts.modified), Int(summary.counts.deleted),
                Int(summary.counts.renamed), Int(summary.counts.typeChanged)
            )
        )
    }

    private func count(_ value: UInt64, _ title: String, color: Color) -> some View {
        HStack(spacing: 3) {
            Text("\(value)")
                .font(.caption.weight(.semibold))
                .foregroundStyle(value == 0 ? AnyShapeStyle(.tertiary) : AnyShapeStyle(color))
            Text(title)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .fixedSize(horizontal: true, vertical: false)
        .opacity(value == 0 ? 0.55 : 1)
    }

    private func entryList(_ summary: SessionReviewSummary) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                if summary.entries.isEmpty {
                    Text(L10n.string("No changes in this session"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                }
                ForEach(summary.entries, id: \.id) { entry in
                    entryRow(entry)
                }
            }
            .padding(.horizontal, 6)
            .padding(.bottom, 6)
        }
        .frame(minHeight: 90, maxHeight: 220)
    }

    private func entryRow(_ entry: ReviewEntryInfo) -> some View {
        let selected = entry.id == model.selectedEntryID
        return Button {
            Task { await model.selectEntry(entry.id) }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: changeSymbol(entry.change))
                    .font(.caption)
                    .foregroundStyle(changeColor(entry.change))
                    .frame(width: 14)
                VStack(alignment: .leading, spacing: 1) {
                    if entry.change == "renamed", let old = entry.oldPath {
                        Text("\(old) → \(entry.id)")
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    } else {
                        Text(entry.id)
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    if let after = entry.after {
                        Text(metadataSummary(after))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? AnyShapeStyle(Color.accentColor.opacity(0.14)) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 6))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(entryAccessibilityLabel(entry))
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }

    @ViewBuilder private var detailView: some View {
        if let entry = model.selectedEntry {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(changeTitle(entry.change))
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(changeColor(entry.change))
                        Text(entry.id)
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer(minLength: 0)
                        if model.loadingDetail {
                            ProgressView().controlSize(.mini)
                        }
                        if model.canOpenSelectedNote {
                            Button(L10n.string("Open Note")) { model.openSelectedNote() }
                                .buttonStyle(.link)
                                .font(.caption)
                        }
                    }
                    if entry.change == "renamed", let old = entry.oldPath {
                        Text(L10n.string("Renamed from %@", old))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    if let detail = model.detail, detail.entry.id == entry.id {
                        detailBody(detail, entry: entry)
                    } else if !model.loadingDetail {
                        Text(L10n.string("Select an entry to view details"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.top, 8)
                .padding(.bottom, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: .infinity)
        } else {
            Text(L10n.string("Select an entry to view details"))
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder private func detailBody(_ detail: ReviewEntryDetail, entry: ReviewEntryInfo) -> some View {
        if detail.large {
            note(L10n.string("Large file (not diffed)"))
        } else if detail.binary {
            note(L10n.string("Binary attachment"))
        } else if detail.text {
            if detail.contentIdentical {
                note(L10n.string("Content unchanged"))
            } else if detail.lineEndingsChanged, detail.hunks.isEmpty {
                note(L10n.string("Line endings changed"))
            } else if detail.hunks.isEmpty {
                if entry.change == "added" && entry.after?.size == 0 {
                    note(L10n.string("Empty file"))
                } else if entry.change == "deleted" && entry.before?.size == 0 {
                    note(L10n.string("Empty file"))
                } else {
                    note(L10n.string("No difference"))
                }
            }
            ForEach(Array(detail.hunks.enumerated()), id: \.offset) { _, hunk in
                hunkView(hunk)
            }
            if detail.oldNoFinalNewline || detail.newNoFinalNewline {
                Text(L10n.string("No newline at end of file"))
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .padding(.leading, 72)
            }
        } else if entry.before?.kind == "symlink" || entry.after?.kind == "symlink" {
            note(L10n.string("Symbolic link"))
        } else if entry.after?.kind == "directory" {
            note(L10n.string("Folder"))
        }

        if entry.before != nil || entry.after != nil {
            metadataTable(entry)
        }
    }

    private func hunkView(_ hunk: ReviewDiffHunk) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("@@ -\(hunk.oldStart),\(hunk.oldCount) +\(hunk.newStart),\(hunk.newCount) @@")
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 6)
                .padding(.top, 4)
                .padding(.bottom, 2)
            ForEach(Array(hunk.lines.enumerated()), id: \.offset) { _, line in
                diffRow(line)
            }
        }
        .background(Color.primary.opacity(0.03), in: .rect(cornerRadius: 5))
    }

    private func diffRow(_ line: ReviewDiffLine) -> some View {
        let removed = line.kind == "removed"
        let added = line.kind == "added"
        let tint = removed ? Color.red : added ? Color.green : Color.clear
        let opacity = contrast == .increased ? 0.20 : 0.09
        return HStack(alignment: .top, spacing: 0) {
            Text(line.oldLine.map { String($0) } ?? "")
                .frame(width: 30, alignment: .trailing)
            Text(line.newLine.map { String($0) } ?? "")
                .frame(width: 30, alignment: .trailing)
                .padding(.leading, 4)
            Text(removed ? "−" : added ? "+" : " ")
                .foregroundStyle(removed ? Color.red : added ? Color.green : Color.secondary)
                .frame(width: 14)
            Text(line.text.isEmpty ? " " : line.text)
                .textSelection(.enabled)
                .foregroundStyle(.primary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.system(size: 10.5, design: .monospaced))
        .padding(.horizontal, 4)
        .padding(.vertical, 1)
        .background(tint.opacity(removed || added ? opacity : 0))
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.secondary)
    }

    private func metadataTable(_ entry: ReviewEntryInfo) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let before = entry.before { metadataGroup(L10n.string("Before"), before) }
            if let after = entry.after { metadataGroup(L10n.string("After"), after) }
        }
        .padding(.top, 2)
    }

    private func metadataGroup(_ title: String, _ metadata: ReviewMetadata) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
            metadataRow(L10n.string("Kind"), kindLabel(metadata.kind))
            metadataRow(L10n.string("Size"), byteCount(metadata.size))
            if let modified = metadata.modifiedMs {
                metadataRow(L10n.string("Modified time"), formattedDate(modified))
            }
            metadataRow(L10n.string("Permissions"), String(format: "%04o", metadata.mode))
            if let link = metadata.link {
                metadataRow(L10n.string("Link target"), link)
            }
        }
    }

    private func metadataRow(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .frame(width: 82, alignment: .leading)
            Text(value)
                .font(.caption2.monospaced())
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
        }
    }

    private func sessionLabel(_ session: AgentSessionInfo) -> String {
        var label = sessionStateLine(session)
        if let code = session.exitCode {
            label += " · " + L10n.string("Exit code %@", String(code))
        }
        return label
    }

    private func sessionStateLine(_ session: AgentSessionInfo) -> String {
        "\(stateLabel(session.state)) · \(shortDate(session.createdAtMs))"
    }

    private func sessionDetailLine(_ session: AgentSessionInfo) -> String {
        var parts = [L10n.string("Snapshot %@", timeOnly(session.snapshotAtMs))]
        if let code = session.exitCode {
            parts.append(L10n.string("Exit code %@", String(code)))
        }
        return parts.joined(separator: " · ")
    }

    private func entryAccessibilityLabel(_ entry: ReviewEntryInfo) -> String {
        var parts = [changeTitle(entry.change), entry.id]
        if let old = entry.oldPath, old != entry.id { parts.append(L10n.string("renamed from %@", old)) }
        if let after = entry.after { parts.append(metadataSummary(after)) }
        return parts.joined(separator: ", ")
    }

    private func metadataSummary(_ metadata: ReviewMetadata) -> String {
        "\(kindLabel(metadata.kind)) · \(byteCount(metadata.size))"
    }

    private func kindLabel(_ kind: String) -> String {
        switch kind {
        case "file": L10n.string("File")
        case "directory": L10n.string("Folder")
        case "symlink": L10n.string("Symbolic link")
        default: kind
        }
    }

    private func stateLabel(_ state: String?) -> String {
        switch state {
        case "preparing": L10n.string("Preparing")
        case "ready": L10n.string("Ready")
        case "running": L10n.string("Running")
        case "exited": L10n.string("Exited")
        case "interrupted": L10n.string("Interrupted")
        case "cancelled": L10n.string("Cancelled")
        case "failed": L10n.string("Failed")
        default: state ?? L10n.string("Unknown")
        }
    }

    private func changeTitle(_ change: String) -> String {
        switch change {
        case "added": L10n.string("Added")
        case "modified": L10n.string("Modified")
        case "deleted": L10n.string("Deleted")
        case "renamed": L10n.string("Renamed")
        case "type_changed": L10n.string("Type changed")
        default: change
        }
    }

    private func changeSymbol(_ change: String) -> String {
        switch change {
        case "added": "plus.circle"
        case "modified": "pencil.circle"
        case "deleted": "minus.circle"
        case "renamed": "arrow.right.circle"
        case "type_changed": "arrow.triangle.2.circlepath.circle"
        default: "circle"
        }
    }

    private func changeColor(_ change: String) -> Color {
        switch change {
        case "added": .green
        case "modified": .blue
        case "deleted": .red
        case "renamed": .orange
        case "type_changed": .purple
        default: .secondary
        }
    }

    private func stateColor(_ state: String?) -> Color {
        switch state {
        case "running", "ready": .green
        case "interrupted", "preparing": .orange
        case "cancelled", "failed": .red
        default: .secondary
        }
    }

    private func byteCount(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    private func formattedDate(_ milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private func shortDate(_ milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        return date.formatted(.dateTime.month().day().hour().minute())
    }

    private func timeOnly(_ milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        return date.formatted(date: .omitted, time: .shortened)
    }
}
