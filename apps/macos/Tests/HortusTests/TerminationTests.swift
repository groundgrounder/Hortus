import AppKit
import Testing
@testable import Hortus

@Suite @MainActor
struct TerminationTests {
    @Test func documentsBeyondShortTimeoutStillDelayExit() async throws {
        let gate = TerminationGate()
        gate.shortTimeout = 0.05
        gate.documentTimeout = 1
        var outcome: TerminationGate.Outcome?
        var cleaned = false
        gate.begin(documentTitles: ["note.md"], documents: { done in
            // 模拟文档持久化晚于 3 秒（测试中按比例缩短）。
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { done([]) }
        }, cleanup: { done in
            cleaned = true
            done()
        }) { outcome = $0 }

        try await Task.sleep(for: .milliseconds(100))
        #expect(outcome == nil)
        #expect(!cleaned)
        try await Task.sleep(for: .milliseconds(300))
        for _ in 0..<200 where outcome == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(outcome == .allowed)
        #expect(cleaned)
    }

    @Test func documentsBeyondLimitBlockExitWithOpenNotes() async throws {
        let gate = TerminationGate()
        gate.shortTimeout = 0.05
        gate.documentTimeout = 0.08
        var outcome: TerminationGate.Outcome?
        var cleaned = false
        gate.begin(documentTitles: ["a.md", "b.md"], documents: { _ in }, cleanup: { done in
            cleaned = true
            done()
        }) { outcome = $0 }

        try await Task.sleep(for: .milliseconds(300))
        for _ in 0..<200 where outcome == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(outcome == .blocked(unsaved: ["a.md", "b.md"]))
        #expect(!cleaned)
    }

    @Test func unsavedDocumentsBlockExitWithoutCleanup() async throws {
        let gate = TerminationGate()
        gate.shortTimeout = 1
        gate.documentTimeout = 1
        var outcome: TerminationGate.Outcome?
        var cleaned = false
        gate.begin(documentTitles: ["a.md", "b.md"], documents: { done in
            done(["b.md"])
        }, cleanup: { done in
            cleaned = true
            done()
        }) { outcome = $0 }

        try await Task.sleep(for: .milliseconds(50))
        for _ in 0..<200 where outcome == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(outcome == .blocked(unsaved: ["b.md"]))
        #expect(!cleaned)
    }
}
