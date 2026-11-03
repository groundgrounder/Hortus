// SPDX-License-Identifier: MIT OR Apache-2.0
import AppKit
import ApplicationServices
import CoreGraphics

@main
struct StartupWindowProbe {
    @MainActor static func main() async throws {
        let pid = Int32(CommandLine.arguments[1])!
        guard AXIsProcessTrusted() else { throw NSError(domain: "Accessibility permission required", code: 1) }
        let application = AXUIElementCreateApplication(pid)
        var samples: [[String: Any]] = []
        let start = ProcessInfo.processInfo.systemUptime
        for _ in 0..<60 {
            var value: CFTypeRef?
            let status = AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &value)
            let windows = (value as? [AXUIElement] ?? []).map { window -> [String: String] in
                var row: [String: String] = [:]
                for name in [kAXTitleAttribute, kAXRoleAttribute, kAXSubroleAttribute] {
                    var attribute: CFTypeRef?
                    AXUIElementCopyAttributeValue(window, name as CFString, &attribute)
                    row[name] = attribute as? String ?? ""
                }
                return row
            }
            samples.append(["seconds": ProcessInfo.processInfo.systemUptime - start,
                            "ax_status": status.rawValue, "windows": windows])
            try await Task.sleep(for: .milliseconds(50))
        }
        let info = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
        let owned = info.filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        let data = try JSONSerialization.data(withJSONObject: ["pid": pid, "samples": samples, "cg_windows": owned], options: [.sortedKeys, .prettyPrinted])
        try data.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
        for (index, window) in owned.enumerated() {
            guard let number = window[kCGWindowNumber as String] as? Int else { continue }
            let task = Process()
            task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            task.arguments = ["-x", "-l", String(number), CommandLine.arguments[3] + "-\(index).png"]
            try task.run(); task.waitUntilExit()
            guard task.terminationStatus == 0 else { throw NSError(domain: "Window screenshot failed", code: 2) }
        }
        if CommandLine.arguments.contains("--hold-open") { return }
        guard NSRunningApplication(processIdentifier: pid)?.terminate() == true else {
            throw NSError(domain: "Quit request failed", code: 3)
        }
    }
}
