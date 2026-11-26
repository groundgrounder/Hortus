import Foundation
import HortusCore

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

public struct TagPanelRow: Identifiable, Sendable {
    public let id: String
    public let name: String
    public let count: Int
    public var depth: Int { id.split(separator: "/").count - 1 }

    public static func rows(_ summaries: [TagSummaryInfo]) -> [Self] {
        var names: [String: String] = [:]
        var notes: [String: Set<String>] = [:]
        for summary in summaries {
            var prefix = ""
            var raw = ""
            for (key, label) in zip(summary.tag.split(separator: "/"), summary.raw.split(separator: "/")) {
                if !prefix.isEmpty { prefix += "/"; raw += "/" }
                prefix += key
                raw += label
                if names[prefix] == nil { names[prefix] = raw }
                notes[prefix, default: []].formUnion(summary.notes)
            }
        }
        return notes.keys.sorted { $0.split(separator: "/").lexicographicallyPrecedes($1.split(separator: "/")) }
            .map { Self(id: $0, name: names[$0] ?? $0, count: notes[$0]?.count ?? 0) }
    }
}

public struct SourceStyleRun: Sendable {
    public var start, length: UInt64
    public var mask: UInt32
    public init(start: UInt64, length: UInt64, mask: UInt32) {
        self.start = start
        self.length = length
        self.mask = mask
    }
}

public struct StyleBatch: Sendable {
    public let revision: UInt64
    public let runs: [SourceStyleRun]
    public let coreMilliseconds, ffiMilliseconds: Double
    public let queuedMilliseconds, decodedAt: Double
    public init(revision: UInt64, runs: [SourceStyleRun], coreMilliseconds: Double,
                ffiMilliseconds: Double, queuedMilliseconds: Double, decodedAt: Double) {
        self.revision = revision
        self.runs = runs
        self.coreMilliseconds = coreMilliseconds
        self.ffiMilliseconds = ffiMilliseconds
        self.queuedMilliseconds = queuedMilliseconds
        self.decodedAt = decodedAt
    }

    public static func decodeRuns(_ packedRuns: Data) -> [SourceStyleRun] {
        packedRuns.withUnsafeBytes { buffer in
            var result: [SourceStyleRun] = []
            result.reserveCapacity(buffer.count / 20)
            for offset in stride(from: 0, to: buffer.count, by: 20) {
                result.append(SourceStyleRun(
                    start: UInt64(littleEndian: buffer.loadUnaligned(fromByteOffset: offset, as: UInt64.self)),
                    length: UInt64(littleEndian: buffer.loadUnaligned(fromByteOffset: offset + 8, as: UInt64.self)),
                    mask: UInt32(littleEndian: buffer.loadUnaligned(fromByteOffset: offset + 16, as: UInt32.self))
                ))
            }
            return result
        }
    }
}

public extension NSAttributedString.Key {
    static let sourceStyle = NSAttributedString.Key("HortusSourceStyle")
}

/// 跨平台 Markdown 语义掩码到原生属性字体的统一映射。
public enum SourceStylePalette {
    public static func attributes(for mask: UInt32, fontSize: Double, contrast: Bool) -> [NSAttributedString.Key: Any] {
        let heading = Int(mask >> 24)
        let code = mask & 48 != 0
        let size = fontSize * ([1: 1.6, 2: 1.35, 3: 1.15][heading] ?? 1)
        var font = code ? PlatformGraphics.monospacedFont(ofSize: size, weight: .regular)
            : PlatformGraphics.systemFont(ofSize: size, weight: heading > 0 || mask & 2 != 0 ? .bold : .regular)
        if mask & 4 != 0 { font = PlatformGraphics.italicFont(font) }
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 4
        paragraph.paragraphSpacing = heading > 0 ? 6 : 0
        paragraph.paragraphSpacingBefore = heading > 0 ? 10 : 0
        var color = PlatformGraphics.textColor
        if mask & (128 | 256 | 512 | 8192) != 0 { color = PlatformGraphics.accentColor }
        if mask & (64 | 2048 | 16384) != 0 { color = PlatformGraphics.secondaryLabelColor }
        if mask & 512 != 0 { color = PlatformGraphics.secondaryLabelColor }
        if mask & (1 << 18) != 0 { color = PlatformGraphics.linkColor }
        if mask & (1 << 19) != 0 { color = PlatformGraphics.secondaryLabelColor }
        if mask & 1 != 0 { color = contrast ? PlatformGraphics.secondaryLabelColor : PlatformGraphics.tertiaryLabelColor }
        var result: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color,
            .paragraphStyle: paragraph, .sourceStyle: mask]
        if mask & 4 != 0 && !PlatformGraphics.hasItalicTrait(font) { result[.obliqueness] = 0.12 }
        if mask & 8 != 0 { result[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
        if mask & 16 != 0 { result[.backgroundColor] = PlatformGraphics.quaternaryLabelColor.withAlphaComponent(0.10) }
        if mask & 512 != 0 { result[.backgroundColor] = PlatformGraphics.accentColor.withAlphaComponent(0.08) }
        if mask & 1024 != 0 { result[.backgroundColor] = PlatformGraphics.systemYellow.withAlphaComponent(0.19) }
        return result
    }
}

public enum FileNamingRules {
    /// 与内核 `vault.invalid_name` 同口径的本地校验；返回可向用户展示的错误文案。
    public static func localError(_ name: String) -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed == "." || trimmed == ".."
            || trimmed.hasPrefix(".") || trimmed == "node_modules"
            || trimmed.contains("/") || trimmed.contains("\\")
            || trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
            return L10n.string("The name is invalid")
        }
        return nil
    }

    public static func stemLength(_ name: String) -> Int {
        guard let dot = name.lastIndex(of: "."), dot != name.startIndex else { return name.count }
        return name.utf16.distance(from: name.startIndex, to: dot)
    }

    public static func suggestedName(base: String, ext: String, parent: String, entries: [String: TreeEntry]) -> String {
        let names = Set(entries.values
            .filter { $0.parent == parent }
            .map { $0.name.lowercased() })
        var candidate = ext.isEmpty ? base : "\(base).\(ext)"
        var index = 2
        while names.contains(candidate.lowercased()) {
            candidate = ext.isEmpty ? "\(base) \(index)" : "\(base) \(index).\(ext)"
            index += 1
        }
        return candidate
    }
}
