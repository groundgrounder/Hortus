import AppKit
import Testing
@testable import Hortus

@Suite @MainActor
struct TerminationTests {
    @Test func nestedLoopTimeoutRetainsAllWindowTitlesAndRepeatedQuitRepliesOnce() {
        let gate = TerminationGate()
        gate.documentTimeout = 0.05
        var replies: [TerminationGate.Outcome] = []
        var cleaned = false
        let titles = ["vault-a/same.md", "vault-b/same.md"]
        gate.begin(documentTitles: titles, documents: { _ in }, cleanup: { done in
            cleaned = true; done()
        }) { replies.append($0) }
        gate.begin(documentTitles: ["duplicate request"], documents: { $0([]) }, cleanup: { $0() }) { replies.append($0) }
        let deadline = Date(timeIntervalSinceNow: 1)
        while replies.isEmpty && Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.01))
        }
        #expect(replies == [.blocked(unsaved: titles)])
        #expect(!cleaned)
    }

    @Test func documentsBeyondShortTimeoutStillDelayExit() async throws {
        let gate = TerminationGate()
        gate.shortTimeout = 0.05
        gate.documentTimeout = 1
        var outcome: TerminationGate.Outcome?
        var cleaned = false
        var finishDocuments: (@Sendable ([String]) -> Void)?
        gate.begin(documentTitles: ["note.md"], documents: { done in
            finishDocuments = done
        }, cleanup: { done in
            cleaned = true
            done()
        }) { outcome = $0 }

        let completeDocuments = try #require(finishDocuments)
        // done 由测试持有，等待短超时不会与文档完成形成先后竞态。
        try await Task.sleep(for: .seconds(gate.shortTimeout * 2))
        #expect(outcome == nil)
        #expect(!cleaned)
        completeDocuments([])
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

    @Test func retryAfterBlockedExitCanStillQuit() async throws {
        let gate = TerminationGate()
        gate.shortTimeout = 1
        gate.documentTimeout = 0.05
        var first: TerminationGate.Outcome?
        var late: (@Sendable ([String]) -> Void)?
        gate.begin(documentTitles: ["a.md"], documents: { done in late = done }, cleanup: { done in done() }) {
            first = $0
        }
        for _ in 0..<200 where first == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(first == .blocked(unsaved: ["a.md"]))

        // 用户选择重试：新一轮必须能放行，上一轮迟到的完成回调不能串到新一轮。
        gate.documentTimeout = 1
        var second: TerminationGate.Outcome?
        var cleaned = false
        gate.begin(documentTitles: ["a.md"], documents: { done in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { done([]) }
        }, cleanup: { done in
            cleaned = true
            done()
        }) { second = $0 }
        late?(["a.md"])
        for _ in 0..<200 where second == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(second == .allowed)
        #expect(cleaned)
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
