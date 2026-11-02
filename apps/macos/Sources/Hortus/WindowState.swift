import AppKit

struct WindowFrame: Codable, Equatable, Sendable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
    var screen: String?

    @MainActor init(window: NSWindow) {
        let frame = window.frame
        x = frame.minX; y = frame.minY; width = frame.width; height = frame.height
        screen = window.screen?.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")].map { String(describing: $0) }
    }

    /// 完整落在某块可用屏幕内才沿用；屏幕变化或离屏时级联回退。
    func restored(in screens: [NSRect], cascade: Int) -> NSRect {
        let frame = NSRect(x: x, y: y, width: width, height: height)
        if [x, y, width, height].allSatisfy(\.isFinite), width >= 900, height >= 560,
           screens.contains(where: { $0.contains(frame) }) { return frame }
        let screen = screens.first ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let width = min(1320, screen.width), height = min(840, screen.height)
        let offset = CGFloat(cascade % 5) * 24
        return NSRect(x: min(screen.maxX - width, screen.minX + offset),
                      y: max(screen.minY, screen.maxY - height - offset), width: width, height: height)
    }
}

struct WindowState: Codable, Equatable, Sendable {
    var id: String
    var path: String
    var tabs: [String]
    var selected: String?
    var views: [String: String]
    var frame: WindowFrame?
}

struct SessionState: Codable, Sendable {
    var last_vault: String?
    var recent: [VaultState]
    var windows: [WindowState]?
    var default_visible: [String: Bool]?
}
