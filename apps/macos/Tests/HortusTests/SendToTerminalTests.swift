import AppKit
import Testing
@testable import Hortus

@MainActor
struct SendToTerminalTests {
    @Test func notePathQuotesOnlyUnsafePathsAndNeverAddsANewline() {
        let clean = TerminalPayload.notePath(relativePath: "notes/Plan.md")
        #expect(clean.text == "notes/Plan.md ")
        #expect(!clean.bracketedPaste)
        #expect(!clean.text.hasSuffix("\n") && !clean.text.hasSuffix("\r"))

        let spaced = TerminalPayload.notePath(relativePath: "Daily Notes/Plan for 今天.md")
        #expect(spaced.text == "'Daily Notes/Plan for 今天.md' ")

        let quoted = TerminalPayload.notePath(relativePath: "a'b.md")
        #expect(quoted.text == "'a'\\''b.md' ")

        let punctuation = TerminalPayload.notePath(relativePath: "notes/a;b&c.md")
        #expect(punctuation.text == "'notes/a;b&c.md' ")
        #expect(!punctuation.text.hasSuffix("\n") && !punctuation.text.hasSuffix("\r"))
    }

    @Test func selectionKeepsMultiByteTextAndMarksBracketedPaste() {
        let payload = TerminalPayload.selection(relativePath: "guides/计划.md",
                                                text: "第一行中文 😀\n第二行：$(touch PWNED)",
                                                lineRange: 3...4)
        #expect(payload.text == "guides/计划.md（第 3-4 行）：\n第一行中文 😀\n第二行：$(touch PWNED)")
        #expect(payload.bracketedPaste)
        #expect(!payload.text.hasSuffix("\n") && !payload.text.hasSuffix("\r"))
        #expect(payload.text.contains("😀"))

        let singleLine = TerminalPayload.selection(relativePath: "note.md", text: "选区", lineRange: 7...7)
        #expect(singleLine.text == "note.md（第 7 行）：\n选区")
        #expect(singleLine.bracketedPaste)

        let oneCharacter = TerminalPayload.selection(relativePath: "", text: "字", lineRange: nil)
        #expect(oneCharacter.text == "字")
        #expect(!oneCharacter.bracketedPaste)
    }

    @Test func lineRangeUsesSourceOffsets() {
        let text = "alpha\nbeta\ngamma" as NSString
        #expect(SendToTerminalSelection.lineRange(in: text, start: 0, end: 1) == 1...1)
        #expect(SendToTerminalSelection.lineRange(in: text, start: 6, end: 15) == 2...3)
        #expect(SendToTerminalSelection.lineRange(in: text, start: 5, end: 6) == 1...1)
    }

    @Test func startedSessionQueuesAndFlushesExactlyOnce() throws {
        let sandbox = try TemporaryDirectory()
        let session = TerminalSession()
        var received: [String] = []
        session.inspectionSink = { received.append($0) }
        #expect(!session.sendText(""))
        session.setInitialRoot(sandbox.path)
        #expect(session.state == .starting)
        #expect(session.sendText("first ", bracketedPaste: false))
        #expect(session.sendText("second", bracketedPaste: true))
        #expect(session.inspectionPendingSendCount == 2)
        #expect(received.isEmpty)

        session.inspectionMarkRunning()
        #expect(received == ["first ", "\u{1b}[200~second\u{1b}[201~"])
        #expect(session.inspectionPendingSendCount == 0)
        #expect(session.inspectionSends.count == 2)
        #expect(session.inspectionSends.allSatisfy { $0.queued })
        #expect(session.inspectionSends.allSatisfy { $0.tabKind == "shell" })
        #expect(session.inspectionSends.allSatisfy { !$0.text.hasSuffix("\n") })
    }

    @Test func controllerTargetsTheCurrentSessionAndFallsBackToTheLastOne() throws {
        let sandbox = try TemporaryDirectory()
        let controller = TerminalController()
        #expect(controller.targetSessionForSend() == nil)
        controller.setInitialRoot(sandbox.path)
        let first = try #require(controller.createShell())
        #expect(controller.targetSessionForSend() === first)
        let second = try #require(controller.createShell())
        #expect(controller.targetSessionForSend() === second)
    }

    @Test func payloadContainsNoExecutedCommand() {
        let payload = TerminalPayload.selection(relativePath: "note.md",
                                                text: "echo touched > /tmp/hortus-send-marker",
                                                lineRange: 1...1)
        #expect(payload.text.hasPrefix("note.md（第 1 行）：\n"))
        #expect(!payload.text.hasSuffix("\r"))
        #expect(payload.bracketedPaste)
    }
}

private final class TemporaryDirectory {
    let path: String
    init() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        path = url.path
    }
    deinit { try? FileManager.default.removeItem(atPath: path) }
}
