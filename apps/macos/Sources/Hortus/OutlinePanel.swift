import SwiftUI

struct OutlinePanel: View {
    @Environment(Workspace.self) private var workspace
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        if let document = workspace.current?.document {
            if document.headings.isEmpty {
                Text("No headings").font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 2) {
                            ForEach(document.headings, id: \.start) { heading in
                                let active = document.activeHeadingStart == Int(heading.start)
                                Button {
                                    document.activeHeadingStart = Int(heading.start)
                                    workspace.revealHeading(heading, path: workspace.current?.path)
                                } label: {
                                    Text(verbatim: heading.title)
                                        .font(.callout.weight(active ? .semibold : (heading.level <= 2 ? .medium : .regular)))
                                        .foregroundStyle(active ? .primary : (heading.level <= 2 ? .primary : .secondary))
                                        .lineLimit(1).truncationMode(.tail)
                                        .padding(.leading, CGFloat(heading.level - 1) * 12 + 6)
                                        .padding(.trailing, 6)
                                        .padding(.vertical, 5)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .background(
                                            active
                                                ? AnyShapeStyle(Color.accentColor.opacity(contrast == .increased ? 0.24 : 0.14))
                                                : AnyShapeStyle(.clear),
                                            in: .rect(cornerRadius: 6)
                                        )
                                        .contentShape(.rect)
                                }
                                .buttonStyle(.plain)
                                .id(Int(heading.start))
                                .accessibilityLabel(L10n.string("Heading level %1$@, %2$@", String(heading.level), heading.title))
                                .accessibilityAddTraits(active ? .isSelected : [])
                            }
                        }.padding(.horizontal, 8).padding(.vertical, 8)
                            .onAppear { document.outlineDisplayedAt = ProcessInfo.processInfo.systemUptime }
                    }
                    .onChange(of: document.activeHeadingStart) { _, start in
                        guard let start else { return }
                        withAnimation(.easeInOut(duration: 0.18)) {
                            proxy.scrollTo(start, anchor: .center)
                        }
                    }
                }
            }
        } else {
            Text("No note open").font(.callout).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}
