import SwiftUI

struct OutlinePanel: View {
    @Environment(Workspace.self) private var workspace

    var body: some View {
        if let document = workspace.current?.document {
            if document.headings.isEmpty {
                Text("No headings").font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(document.headings, id: \.start) { heading in
                            Button { workspace.revealHeading(heading) } label: {
                                Text(verbatim: heading.title)
                                    .font(.callout.weight(heading.level <= 2 ? .medium : .regular))
                                    .foregroundStyle(heading.level <= 2 ? .primary : .secondary)
                                    .lineLimit(1).truncationMode(.tail)
                                    .padding(.leading, CGFloat(heading.level - 1) * 12)
                                    .padding(.vertical, 5)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(L10n.string("Heading level %1$@, %2$@", String(heading.level), heading.title))
                        }
                    }.padding(.horizontal, 14).padding(.vertical, 8)
                        .onAppear { document.outlineDisplayedAt = ProcessInfo.processInfo.systemUptime }
                }
            }
        } else {
            Text("No note open").font(.callout).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}
