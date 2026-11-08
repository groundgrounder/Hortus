// SPDX-License-Identifier: MIT OR Apache-2.0
// 编辑器命令检查的外部驱动：只发真实 HID 键盘/鼠标事件，并通过 Accessibility
// 读取焦点、文本区与菜单，不向被测应用注入调用。结果以 JSON 写到标准输出或文件。
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics

private let keycodes: [Character: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
    "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
    "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
    " ": 49, "`": 50,
]

private let shifted: [Character: Character] = [
    "A": "a", "S": "s", "D": "d", "F": "f", "H": "h", "G": "g", "Z": "z", "X": "x",
    "C": "c", "V": "v", "B": "b", "Q": "q", "W": "w", "E": "e", "R": "r", "Y": "y",
    "T": "t", "O": "o", "U": "u", "I": "i", "P": "p", "L": "l", "J": "j", "K": "k",
    "N": "n", "M": "m", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6",
    "&": "7", "*": "8", "(": "9", ")": "0", "_": "-", "+": "=", "{": "[", "}": "]",
    "|": "\\", ":": ";", "\"": "'", "<": ",", ">": ".", "?": "/", "~": "`",
]

private func flag(named value: String) -> CGEventFlags {
    switch value {
    case "cmd", "command": return .maskCommand
    case "opt", "option", "alt": return .maskAlternate
    case "shift": return .maskShift
    case "ctrl", "control": return .maskControl
    case "fn": return .maskSecondaryFn
    default: return []
    }
}

private func post(_ event: CGEvent?) {
    event?.post(tap: .cghidEventTap)
}

private func keyEvent(_ code: CGKeyCode, flags: CGEventFlags, down: Bool) {
    let event = CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState),
                        virtualKey: code, keyDown: down)
    event?.flags = flags
    post(event)
}

private func press(_ code: CGKeyCode, flags: CGEventFlags = []) {
    keyEvent(code, flags: flags, down: true)
    usleep(12_000)
    keyEvent(code, flags: flags, down: false)
    usleep(12_000)
}

private func type(_ text: String) {
    for character in text {
        if let base = shifted[character], let code = keycodes[base] {
            press(code, flags: .maskShift)
        } else if let code = keycodes[character] {
            press(code)
        } else {
            fputs("unsupported character: \(character)\n", stderr)
            exit(4)
        }
        usleep(28_000)
    }
}

private func axValue(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}

private func string(_ element: AXUIElement, _ name: String) -> String {
    (axValue(element, name) as? String) ?? ""
}

private func number(_ element: AXUIElement, _ name: String) -> Double? {
    (axValue(element, name) as? NSNumber)?.doubleValue
}

private func point(_ element: AXUIElement, _ name: String) -> CGPoint? {
    guard let raw = axValue(element, name), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let value = raw as! AXValue
    var point = CGPoint.zero
    guard AXValueGetValue(value, .cgPoint, &point) else { return nil }
    return point
}

private func size(_ element: AXUIElement, _ name: String) -> CGSize? {
    guard let raw = axValue(element, name), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let value = raw as! AXValue
    var size = CGSize.zero
    guard AXValueGetValue(value, .cgSize, &size) else { return nil }
    return size
}

private func children(_ element: AXUIElement) -> [AXUIElement] {
    (axValue(element, kAXChildrenAttribute as String) as? [AXUIElement]) ?? []
}

private func describe(_ element: AXUIElement, depth: Int = 0) -> [String: Any] {
    var row: [String: Any] = [
        "role": string(element, kAXRoleAttribute as String),
        "subrole": string(element, kAXSubroleAttribute as String),
        "title": string(element, kAXTitleAttribute as String),
        "description": string(element, kAXDescriptionAttribute as String),
        "focused": number(element, kAXFocusedAttribute as String) == 1,
        "identifier": string(element, "AXIdentifier"),
    ]
    if let value = axValue(element, kAXValueAttribute as String) {
        if let text = value as? String {
            row["value_length"] = text.utf16.count
            row["value_prefix"] = String(text.prefix(80))
        } else if let number = value as? NSNumber {
            row["value"] = number
        }
    }
    if let length = number(element, kAXNumberOfCharactersAttribute as String) {
        row["characters"] = length
    }
    if let raw = axValue(element, kAXSelectedTextRangeAttribute as String), CFGetTypeID(raw) == AXValueGetTypeID() {
        var range = CFRange()
        if AXValueGetValue(raw as! AXValue, .cfRange, &range) {
            row["selected_range"] = [range.location, range.length]
        }
    }
    if let position = point(element, kAXPositionAttribute as String) {
        row["position"] = ["x": position.x, "y": position.y]
    }
    if let frame = size(element, kAXSizeAttribute as String) {
        row["size"] = ["width": frame.width, "height": frame.height]
    }
    if depth < 2 {
        let nested = children(element)
        if !nested.isEmpty {
            row["children"] = nested.prefix(60).map { describe($0, depth: depth + 1) }
        }
    }
    return row
}

private func menuTree(_ element: AXUIElement, depth: Int = 0) -> [String: Any] {
    var row: [String: Any] = [
        "title": string(element, kAXTitleAttribute as String),
        "enabled": number(element, kAXEnabledAttribute as String) == 1,
    ]
    if let character = string(element, "AXMenuItemCmdChar") as String?, !character.isEmpty {
        row["command"] = character
    }
    let modifiers = number(element, "AXMenuItemCmdModifiers").map { Int($0) } ?? 0
    row["command_modifiers"] = modifiers
    if depth < 4 {
        let nested = children(element)
        if !nested.isEmpty { row["items"] = nested.map { menuTree($0, depth: depth + 1) } }
    }
    return row
}

private func application(_ pid: pid_t) -> AXUIElement {
    AXUIElementCreateApplication(pid)
}

private func windows(_ application: AXUIElement) -> [AXUIElement] {
    (axValue(application, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
}

private func focused(_ application: AXUIElement) -> AXUIElement? {
    guard let value = axValue(application, kAXFocusedUIElementAttribute as String) else { return nil }
    return (value as! AXUIElement)
}

private func textAreas(_ element: AXUIElement, into result: inout [[String: Any]]) {
    let role = string(element, kAXRoleAttribute as String)
    if role == kAXTextAreaRole as String || role == kAXTextFieldRole as String {
        result.append(describe(element))
    }
    for child in children(element) { textAreas(child, into: &result) }
}

private func snapshot(pid: pid_t) -> [String: Any] {
    guard AXIsProcessTrusted() else { return ["ax_trusted": false] }
    let app = application(pid)
    var result: [String: Any] = ["ax_trusted": true, "pid": Int(pid)]
    let windowList = windows(app)
    result["windows"] = windowList.map { window -> [String: Any] in
        var row = describe(window, depth: 0)
        row.removeValue(forKey: "children")
        return row
    }
    if let focus = focused(app) {
        result["focused"] = describe(focus, depth: 0)
    }
    var areas: [[String: Any]] = []
    for window in windowList { textAreas(window, into: &areas) }
    result["text_areas"] = areas
    if let raw = axValue(app, kAXFocusedWindowAttribute as String) { let window = raw as! AXUIElement
        var focusedAreas: [[String: Any]] = []
        textAreas(window, into: &focusedAreas)
        result["focused_window"] = [
            "title": string(window, kAXTitleAttribute as String),
            "text_areas": focusedAreas.map { area in
                ["role": area["role"] ?? "", "description": area["description"] ?? "",
                 "value_length": area["value_length"] ?? -1]
            },
        ]
    }
    var groups: [[String: Any]] = []
    for window in windowList {
        for child in children(window) where string(child, kAXRoleAttribute as String) == kAXGroupRole as String {
            if let description = Optional(string(child, kAXDescriptionAttribute as String)), !description.isEmpty {
                var row = describe(child, depth: 0)
                row.removeValue(forKey: "children")
                groups.append(row)
            }
        }
    }
    result["groups"] = groups
    if let menu = axValue(app, kAXMenuBarAttribute as String) {
        let bar = menu as! AXUIElement
        result["menu_bar"] = children(bar).map { item -> [String: Any] in
            var row = menuTree(item, depth: 1)
            row["title"] = string(item, kAXTitleAttribute as String)
            return row
        }
    }
    return result
}

private func inputSources(_ includeAll: Bool) -> [TISInputSource] {
    (TISCreateInputSourceList(nil, includeAll)?.takeRetainedValue() as? [TISInputSource]) ?? []
}

private func inputSourceID(_ source: TISInputSource) -> String? {
    guard let pointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return nil }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
}

private func inputSourceBool(_ source: TISInputSource, _ key: CFString) -> Bool {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return false }
    return (Unmanaged<CFBoolean>.fromOpaque(pointer).takeUnretainedValue() as NSNumber).boolValue
}

private func currentInputSource() -> String {
    inputSources(false).first { inputSourceBool($0, kTISPropertyInputSourceIsSelected) }
        .flatMap(inputSourceID) ?? ""
}

private func selectInputSource(_ identifier: String) -> (Bool, Bool, Int32) {
    guard let source = inputSources(false).first(where: { inputSourceID($0) == identifier }) else {
        return (false, false, -1)
    }
    let available = inputSourceBool(source, kTISPropertyInputSourceIsEnabled)
        && inputSourceBool(source, kTISPropertyInputSourceIsSelectCapable)
    let status = TISSelectInputSource(source)
    usleep(250_000)
    return (available, currentInputSource() == identifier, status)
}

private func click(_ point: CGPoint) {
    CGWarpMouseCursorPosition(point)
    let down = CGEvent(mouseEventSource: CGEventSource(stateID: .hidSystemState),
                       mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left)
    post(down)
    usleep(60_000)
    let up = CGEvent(mouseEventSource: CGEventSource(stateID: .hidSystemState),
                     mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
    post(up)
    usleep(80_000)
}

private func write(_ value: [String: Any], to path: String?) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .prettyPrinted])
    if let path {
        try data.write(to: URL(fileURLWithPath: path))
    } else {
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
    }
}

private func pid(from value: String) -> pid_t { pid_t(Int32(value) ?? 0) }

@main
struct EditorCommandProbe {
    static func main() {
        var arguments = Array(CommandLine.arguments.dropFirst())
        guard let command = arguments.first else {
            fputs("usage: editor-command-probe <command> ...\n", stderr); exit(2)
        }
        arguments.removeFirst()
        let output = arguments.firstIndex(of: "--json").flatMap { index in
            index + 1 < arguments.count ? arguments[index + 1] : nil
        }
        do {
            switch command {
            case "key":
                let code = CGKeyCode(UInt16(arguments[0]) ?? 0)
                let flags = arguments.dropFirst().filter { !$0.hasPrefix("--") && $0 != output }
                    .reduce(CGEventFlags()) { $0.union(flag(named: $1)) }
                press(code, flags: flags)
                try write(["command": "key", "keycode": Int(code), "flags": Int(flags.rawValue)], to: output)
            case "type":
                type(arguments[0])
                try write(["command": "type", "text": arguments[0]], to: output)
            case "click":
                guard arguments.count >= 2, let x = Double(arguments[0]), let y = Double(arguments[1]) else {
                    fputs("click requires x y\n", stderr); exit(2)
                }
                click(CGPoint(x: x, y: y))
                try write(["command": "click", "x": x, "y": y], to: output)
            case "ax":
                try write(snapshot(pid: pid(from: arguments[0])), to: output)
            case "tree":
                let app = application(pid(from: arguments[0]))
                let windowList = windows(app)
                var rows: [[String: Any]] = []
                func walk(_ element: AXUIElement, depth: Int) {
                    var row = describe(element, depth: 0)
                    row.removeValue(forKey: "children")
                    row["depth"] = depth
                    rows.append(row)
                    if depth < 8 {
                        for child in children(element) { walk(child, depth: depth + 1) }
                    }
                }
                for window in windowList { walk(window, depth: 0) }
                try write(["pid": Int(pid(from: arguments[0])), "elements": rows], to: output)
            case "input-source":
                if arguments[0] == "get" {
                    try write(["input_source": currentInputSource()], to: output)
                } else {
                    let result = selectInputSource(arguments[0])
                    try write(["found": result.0, "selected": result.1, "status": Int(result.2),
                               "current": currentInputSource()], to: output)
                }
            case "screenshot":
                let pidValue = pid(from: arguments[0])
                let app = application(pidValue)
                guard let window = windows(app).first, let position = point(window, kAXPositionAttribute as String),
                      let frame = size(window, kAXSizeAttribute as String),
                      let info = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]],
                      let number = info.first(where: {
                          ($0[kCGWindowOwnerPID as String] as? Int32) == pidValue &&
                          ($0[kCGWindowLayer as String] as? Int) == 0
                      })?[kCGWindowNumber as String] as? Int else {
                    throw NSError(domain: "screenshot", code: 1)
                }
                let task = Process()
                task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                task.arguments = ["-x", "-l", String(number), arguments[1]]
                try task.run(); task.waitUntilExit()
                try write(["command": "screenshot", "window": number,
                           "position": ["x": position.x, "y": position.y],
                           "size": ["width": frame.width, "height": frame.height],
                           "status": Int(task.terminationStatus)], to: output)
            case "press-menu":
                guard arguments.count >= 3 else { fputs("press-menu requires menu title and item title\n", stderr); exit(2) }
                let app = application(pid(from: arguments[0]))
                guard let barRaw = axValue(app, kAXMenuBarAttribute as String) else { throw NSError(domain: "menu", code: 1) }
                let bar = barRaw as! AXUIElement
                func findItem(_ element: AXUIElement, depth: Int) -> AXUIElement? {
                    if depth > 0 {
                        let title = string(element, kAXTitleAttribute as String)
                        if title == arguments[2] { return element }
                    }
                    for child in children(element) {
                        if let found = findItem(child, depth: depth + 1) { return found }
                    }
                    return nil
                }
                var target: AXUIElement?
                for item in children(bar) where string(item, kAXTitleAttribute as String) == arguments[1] {
                    for menu in children(item) {
                        target = findItem(menu, depth: 1)
                        if target != nil { break }
                    }
                    if target != nil { break }
                }
                guard let target else { throw NSError(domain: "menu item not found", code: 2) }
                let status = AXUIElementPerformAction(target, kAXPressAction as CFString)
                try write(["command": "press-menu", "status": Int(status.rawValue)], to: output)
            case "activate":
                let running = NSRunningApplication(processIdentifier: pid(from: arguments[0]))
                let activated = running?.activate(options: [.activateAllWindows]) ?? false
                try write(["activated": activated], to: output)
            case "wait-window":
                let appPID = pid(from: arguments[0])
                let timeout = Double(arguments.count > 1 ? arguments[1] : "10") ?? 10
                let deadline = Date().addingTimeInterval(timeout)
                var frame: [String: Any] = [:]
                while Date() < deadline {
                    let app = application(appPID)
                    if let window = windows(app).first, let position = point(window, kAXPositionAttribute as String),
                       let size = size(window, kAXSizeAttribute as String),
                       size.width > 100, size.height > 100 {
                        frame = ["x": position.x, "y": position.y, "width": size.width, "height": size.height,
                                 "title": string(window, kAXTitleAttribute as String)]
                        break
                    }
                    usleep(100_000)
                }
                try write(["window": frame], to: output)
            case "tap":
                let seconds = Double(arguments.first ?? "3") ?? 3
                fputs("TAPREADY\n", stderr)
                let mask = CGEventMask(1 << CGEventType.keyDown.rawValue)
                let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
                                            options: .listenOnly, eventsOfInterest: mask,
                                            callback: { _, _, event, _ in
                    let keycode = event.getIntegerValueField(.keyboardEventKeycode)
                    fputs("TAP keycode=\(keycode) flags=\(event.flags.rawValue)\n", stderr)
                    return Unmanaged.passUnretained(event)
                }, userInfo: nil)
                if let tap {
                    let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
                    CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
                    CGEvent.tapEnable(tap: tap, enable: true)
                } else {
                    fputs("TAP unavailable\n", stderr)
                }
                RunLoop.current.run(until: Date().addingTimeInterval(seconds))
                try write(["command": "tap", "seconds": seconds], to: output)
            case "press-until":
                // press-until <timeout> <predicate: textarea|findbar|none>
                exit(2)
            default:
                fputs("unknown command: \(command)\n", stderr); exit(2)
            }
        } catch {
            fputs("probe failed: \(error)\n", stderr); exit(1)
        }
    }
}
