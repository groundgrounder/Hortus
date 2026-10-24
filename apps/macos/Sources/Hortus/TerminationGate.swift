import AppKit

/// terminateLater 的嵌套事件循环也必须能处理超时；完成与超时只答复一次。
/// 文档持久化与不可逆清理分开计时：文档工作可以等到上限，清理仍沿用短超时。
@MainActor
final class TerminationGate {
    enum Outcome: Equatable {
        case allowed
        /// 未持久化的笔记路径；超时时为所有打开的笔记。
        case blocked(unsaved: [String])
    }
    /// 终端停止、监听关闭、布局与会话保存的短超时。
    var shortTimeout: TimeInterval = 3
    /// 文档持久化的上限；测试可注入更短的值。
    var documentTimeout: TimeInterval = 10

    private var documentTimer: Timer?
    private var cleanupTimer: Timer?
    private var reply: ((Outcome) -> Void)?
    private var cleanup: ((@escaping @Sendable () -> Void) -> Void)?
    private var documentTitles: [String] = []
    /// 退出被拦截后可以再次尝试；上一轮迟到的回调按代次丢弃。
    private var generation = 0

    /// `documents` 回调尚未持久化的笔记路径（空数组表示全部安全）；只有全部安全才会执行 `cleanup`。
    func begin(documentTitles: [String],
               documents: (@escaping @Sendable ([String]) -> Void) -> Void,
               cleanup: @escaping (@escaping @Sendable () -> Void) -> Void,
               reply: @escaping (Outcome) -> Void) {
        guard self.reply == nil else { return }
        generation += 1
        let current = generation
        self.reply = reply
        self.cleanup = cleanup
        self.documentTitles = documentTitles
        let timer = Timer(timeInterval: documentTimeout, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.finish(.blocked(unsaved: self?.documentTitles ?? []), current) }
        }
        documentTimer = timer
        RunLoop.main.add(timer, forMode: .common)
        documents { [weak self] unsaved in
            RunLoop.main.perform(inModes: [.common]) {
                MainActor.assumeIsolated { self?.documentsFinished(unsaved, current) }
            }
        }
    }

    private func documentsFinished(_ unsaved: [String], _ current: Int) {
        guard current == generation, reply != nil, cleanupTimer == nil else { return }
        guard unsaved.isEmpty else {
            finish(.blocked(unsaved: unsaved), current)
            return
        }
        documentTimer?.invalidate()
        documentTimer = nil
        // 文档已安全落盘，才开始终端、监听与会话的不可逆清理。
        let timer = Timer(timeInterval: shortTimeout, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.finish(.allowed, current) }
        }
        cleanupTimer = timer
        RunLoop.main.add(timer, forMode: .common)
        cleanup? { [weak self] in
            RunLoop.main.perform(inModes: [.common]) {
                MainActor.assumeIsolated { self?.finish(.allowed, current) }
            }
        }
    }

    private func finish(_ outcome: Outcome, _ current: Int) {
        guard current == generation, let reply else { return }
        self.reply = nil
        cleanup = nil
        documentTimer?.invalidate(); documentTimer = nil
        cleanupTimer?.invalidate(); cleanupTimer = nil
        reply(outcome)
    }
}
