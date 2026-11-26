import HortusApple
import SwiftUI

/// 自定义命令启动 Agent 轻量弹窗；支持自由 Shell 命令与最近历史一键选用。
struct CustomAgentSheet: View {
    @Environment(Workspace.self) private var workspace
    @State private var command = ""
    @FocusState private var fieldFocused: Bool

    private var canLaunch: Bool {
        !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(L10n.string("Custom Agent Command"))
                .font(.headline)

            TextField(L10n.string("Command"), text: $command, prompt: Text("codex --model o3"))
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .focused($fieldFocused)
                .onSubmit { launch() }
                .accessibilityLabel(L10n.string("Custom Agent Command"))

            if !workspace.customAgentHistory.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L10n.string("Recent Commands"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    ScrollView {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(workspace.customAgentHistory, id: \.self) { item in
                                Button {
                                    command = item
                                } label: {
                                    Text(item)
                                        .font(.system(.callout, design: .monospaced))
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .padding(.horizontal, 8)
                                        .padding(.vertical, 5)
                                        .contentShape(.rect)
                                }
                                .buttonStyle(.plain)
                                .background(
                                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                                        .fill(command == item
                                              ? Color.accentColor.opacity(0.14)
                                              : Color.primary.opacity(0.04))
                                )
                            }
                        }
                    }
                    .frame(maxHeight: 160)
                }
            }

            HStack {
                Spacer()
                Button(L10n.string("Cancel"), role: .cancel) {
                    workspace.customAgentSheetPresented = false
                }
                .keyboardShortcut(.cancelAction)
                Button(L10n.string("Launch")) {
                    launch()
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!canLaunch)
            }
        }
        .padding(20)
        .frame(width: 460)
        .onAppear {
            if command.isEmpty, let first = workspace.customAgentHistory.first {
                command = first
            }
            fieldFocused = true
        }
    }

    private func launch() {
        let trimmed = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        workspace.launchCustomAgent(trimmed)
    }
}
