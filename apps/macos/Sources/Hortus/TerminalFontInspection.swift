#if DEBUG
import AppKit
import CoreText
@testable import SwiftTerm

/// Uses SwiftTerm's real row builder, Core Text runs and placement math. It is
/// compiled only in Debug; the dependency is unmodified and built with testing enabled.
@MainActor
enum TerminalFontInspection {
    static let argumentPrefix = "--terminal-font-inspection="
    static var requested: Bool { ProcessInfo.processInfo.arguments.contains { $0.hasPrefix(argumentPrefix) } }

    private static func argument(_ name: String, _ arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func wait(_ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + 30
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else { throw CocoaError(.coderValueNotFound) }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private static func quoted(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private static func rect(_ value: CGRect) -> [CGFloat] { [value.minX, value.minY, value.width, value.height] }

    static func cells(in view: HortusTerminalView) -> [[String: Any]] {
        view.withTerminal { terminal in
            var result: [[String: Any]] = []
            for row in 0..<terminal.rows {
                guard let line = terminal.getLine(row: row) else { continue }
                let info = view.buildAttributedStringLocked(row: row, line: line, cols: terminal.cols)
                for powerline in info.powerlineGlyphs {
                    result.append(["row": row, "column": powerline.column, "width": powerline.columnWidth,
                                   "codepoint": String(format: "%X", powerline.codePoint),
                                   "renderer": "PowerlineRenderer"])
                }
                for segment in info.segments {
                    let text = segment.attributedString
                    let ctLine = CTLineCreateWithAttributedString(text)
                    for run in CTLineGetGlyphRuns(ctLine) as! [CTRun] {
                        let attributes = CTRunGetAttributes(run) as! [NSAttributedString.Key: Any]
                        guard let font = attributes[.font] else { continue }
                        let runFont = font as! CTFont
                        let count = CTRunGetGlyphCount(run)
                        var glyphs = [CGGlyph](repeating: 0, count: count)
                        var indices = [CFIndex](repeating: 0, count: count)
                        CTRunGetGlyphs(run, CFRange(), &glyphs)
                        CTRunGetStringIndices(run, CFRange(), &indices)
                        let policy = attributes[SwiftTermGlyphPolicyKey] as? TerminalGlyphPlacementPolicy
                        for index in 0..<count {
                            let source = (text.string as NSString).rangeOfComposedCharacterSequence(at: indices[index])
                            let character = (text.string as NSString).substring(with: source)
                            guard !character.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { continue }
                            let column = segment.column + segment.cellOrdinal(forUTF16: indices[index]) * segment.columnWidth
                            let fit: GlyphSlotFit
                            if let policy {
                                fit = view.glyphSlotFit(font: runFont, glyph: glyphs[index],
                                                       columnWidth: segment.columnWidth, policy: policy)
                            } else if segment.columnWidth >= 2 {
                                fit = view.glyphSlotFit(font: runFont, glyph: glyphs[index], columnWidth: segment.columnWidth)
                            } else if !view.isPrimaryFont(runFont) {
                                fit = view.fallbackGlyphFit(font: runFont, glyph: glyphs[index])
                            } else { fit = .identity }
                            var glyph = glyphs[index]
                            let ink = CTFontGetBoundingRectsForGlyphs(runFont, .horizontal, &glyph, nil, 1)
                            let fitted = CGRect(x: ink.minX * fit.scaleX + fit.dx,
                                                y: ink.minY * fit.scaleY + fit.dy + view.baselineOffset,
                                                width: ink.width * fit.scaleX, height: ink.height * fit.scaleY)
                            let cell = line[column]
                            result.append([
                                "row": row, "column": column, "width": segment.columnWidth,
                                "text": character, "codepoint": character.unicodeScalars.map { String(format: "%X", $0.value) }.joined(separator: "+"),
                                "font": CTFontCopyPostScriptName(runFont) as String, "glyph": Int(glyph),
                                "bold": cell.attribute.style.contains(.bold), "italic": cell.attribute.style.contains(.italic),
                                "provider": policy != nil, "ink": rect(ink), "fitted_ink": rect(fitted),
                                "scale": fit.scaleX, "renderer": "CoreText",
                            ])
                        }
                    }
                }
            }
            return result
        }
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard let fixture = argument("terminal-font-fixture", arguments),
              let marker = argument("terminal-font-marker", arguments),
              let shot = argument("terminal-font-snapshot", arguments) else { throw CocoaError(.coderValueNotFound) }
        layout.setTerminalVisible(true)
        layout.size[.bottom] = 520
        if workspace.terminals.currentSession == nil { workspace.terminals.newShellTab() }
        guard let session = workspace.terminals.currentSession else { throw CocoaError(.coderValueNotFound) }
        try await wait { session.state == .running && session.inspectionViewAttached }
        guard let view = session.inspectionView, let catalog = session.nerdFonts,
              let window = view.window else { throw CocoaError(.coderValueNotFound) }
        // The complete fixture needs 92 columns at the product's default 15 pt.
        // Only this Debug inspection owns the larger window; normal launch is unchanged.
        window.setContentSize(NSSize(width: 1710, height: 979))
        try await wait { view.terminalDimensions.cols >= 92 && view.terminalDimensions.rows >= 21 }
        let before = view.cellDimension!
        let provider = view.glyphFallbackProvider
        let size = workspace.fontSize
        workspace.fontSize = size + 2
        try await Task.sleep(for: .milliseconds(100))
        workspace.fontSize = size
        layout.moveToRegion(.terminal, .right)
        try await Task.sleep(for: .milliseconds(150))
        layout.moveToRegion(.terminal, .bottom)
        layout.size[.bottom] = 520
        try await Task.sleep(for: .milliseconds(150))
        try await wait { view.terminalDimensions.cols >= 92 && view.terminalDimensions.rows >= 21 }
        let after = view.cellDimension!
        let sameCatalog = session.nerdFonts === catalog && view.glyphFallbackProvider === provider
        let realShell = arguments.contains("--terminal-font-login-shell")
        if realShell {
            session.inspectionSend("unset HISTFILE; eza --icons; printf done > \(quoted(marker))\r")
        } else {
            session.inspectionSend("PS1=''; /bin/zsh -f \(quoted(fixture)); printf done > \(quoted(marker))\r")
        }
        try await wait { FileManager.default.fileExists(atPath: marker) }
        if !realShell {
            try await wait {
                view.visibleRowsText(0..<view.terminalDimensions.rows).contains { $0.contains("FONT FIXTURE END") }
            }
        }
        try await Task.sleep(for: .milliseconds(300))
        let actual = cells(in: view)
        try await AppDelegate.snapshot(to: shot)
        let normal = view.fontSet.normal
        let result: [String: Any] = [
            "families": catalog.families, "enumeration_on_main": catalog.enumerationOnMain,
            "enumeration_ms": catalog.enumerationMilliseconds,
            "cached_across_size_and_panel_move": sameCatalog,
            "cell_before": [before.width, before.height], "cell_after": [after.width, after.height],
            "base_font": normal.fontName, "cells": actual,
            "visible_text": view.visibleRowsText(0..<view.terminalDimensions.rows),
            "font_ready_before_shell": session.inspectionFontReadyBeforeShell,
            "custom_block_glyphs": view.customBlockGlyphs,
            "font_size": size, "columns": view.terminalDimensions.cols, "rows": view.terminalDimensions.rows,
            "window_content_size": [window.contentLayoutRect.width, window.contentLayoutRect.height],
        ]
        return result
    }
}
#endif
