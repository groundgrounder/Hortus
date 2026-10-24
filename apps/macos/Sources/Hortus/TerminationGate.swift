import AppKit

/// terminateLater 的嵌套事件循环也必须能处理超时；完成与超时只答复一次。
@MainActor
final class TerminationGate {
    private var timer: Timer?
    private var reply: (() -> Void)?
    func begin(work: (@escaping @Sendable () -> Void) -> Void, reply: @escaping () -> Void) {
        guard self.reply == nil else { return }
        self.reply = reply
        let timer = Timer(timeInterval: 3, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.finish() }
        }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
        work { [weak self] in
            RunLoop.main.perform(inModes: [.common]) {
                MainActor.assumeIsolated { self?.finish() }
            }
        }
    }
    private func finish() {
        guard let reply else { return }
        self.reply = nil
        timer?.invalidate()
        timer = nil
        reply()
    }
}
