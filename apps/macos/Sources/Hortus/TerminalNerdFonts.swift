import Foundation
import CoreText
import SwiftTerm

/// Only installed icon faces participate; the terminal's four text faces stay SF Mono.
final class TerminalNerdFonts: Sendable {
    // A detached, process-wide task enumerates once. Await it before starting the
    // first PTY so its first output already has the provider, without a tofu flash.
    static let installed = Task.detached(priority: .userInitiated) { discover() }

    private static func discover() -> TerminalNerdFonts {
        let started = ProcessInfo.processInfo.systemUptime
        let onMain = Thread.isMainThread
        var families = CTFontManagerCopyAvailableFontFamilyNames() as! [String]
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--terminal-nerd-fonts=none") { families = [] }
        #endif
        return TerminalNerdFonts(families: families, enumerationOnMain: onMain,
                                 enumerationMilliseconds: (ProcessInfo.processInfo.systemUptime - started) * 1000)
    }

    let families: [String]
    let provider: NerdGlyphProvider?
    let enumerationOnMain: Bool
    let enumerationMilliseconds: Double

    init(families: [String], enumerationOnMain: Bool = false, enumerationMilliseconds: Double = 0) {
        self.families = Self.orderedFamilies(families)
        var fallbackFamilies = self.families
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--terminal-nerd-fonts=meslo-first") {
            fallbackFamilies = ["MesloLGS NF", "JetBrainsMono Nerd Font Mono"]
        }
        #endif
        self.provider = fallbackFamilies.isEmpty ? nil : NerdGlyphProvider(families: fallbackFamilies)
        self.enumerationOnMain = enumerationOnMain
        self.enumerationMilliseconds = enumerationMilliseconds
    }

    /// Stable order: Nerd Font Mono, other Nerd Font, legacy NF suffix, then Propo.
    /// Within each group use Unicode lexical order, independent of system enumeration
    /// and locale. Mono takes priority because these icons must fit in one column.
    static func orderedFamilies(_ names: [String]) -> [String] {
        func rank(_ name: String) -> Int {
            if name.contains("Nerd Font Mono") { return 0 }
            if name.contains("Nerd Font Propo") { return 3 }
            if name.contains("Nerd Font") { return 1 }
            return 2
        }
        return Set(names.filter { $0.contains("Nerd Font") || $0.hasSuffix(" NF") }).sorted {
            let left = rank($0), right = rank($1)
            return left == right ? $0 < $1 : left < right
        }
    }
}

/// Immutable coverage plus a locked, per-size font cache; SwiftTerm may call it
/// on its render thread. Core Text walks the cascade per glyph, including when
/// the first installed face (for example Meslo) lacks a newer Nerd Fonts icon.
final class NerdGlyphProvider: TerminalGlyphFallbackProvider, @unchecked Sendable {
    private let descriptors: [CTFontDescriptor]
    private let covered: CharacterSet
    private let lock = NSLock()
    private var fonts: [CGFloat: CTFont] = [:]
    let generation: UInt64 = 1

    init(families: [String]) {
        descriptors = families.map { family in
            CTFontDescriptorCreateWithAttributes([kCTFontFamilyNameAttribute: family] as CFDictionary)
        }
        covered = descriptors.reduce(into: CharacterSet()) { result, descriptor in
            result.formUnion(CTFontCopyCharacterSet(CTFontCreateWithFontDescriptor(descriptor, 15, nil)) as CharacterSet)
        }
    }

    func placementPolicy(for scalar: Unicode.Scalar) -> TerminalGlyphPlacementPolicy? {
        let value = scalar.value
        let privateUse = (0xE000...0xF8FF).contains(value)
            || (0xF0000...0xFFFFD).contains(value) || (0x100000...0x10FFFD).contains(value)
        // Claim the exact installed cmap coverage in the private-use planes,
        // never CJK/emoji or a whole PUA block. SwiftTerm checks each SF Mono
        // style's own cmap first (including U+F8FF). Keep its Powerline shapes.
        guard privateUse, covered.contains(scalar),
              ![0xE0B0, 0xE0B2, 0xE0B4, 0xE0B6].contains(value) else { return nil }
        return TerminalGlyphPlacementPolicy(placement: .icon, maximumCellWidth: 1)
    }

    func fallbackFont(forPointSize size: CGFloat) -> CTFont? {
        lock.lock()
        defer { lock.unlock() }
        if let font = fonts[size] { return font }
        guard let first = descriptors.first else { return nil }
        let descriptor = CTFontDescriptorCreateCopyWithAttributes(first, [
            kCTFontCascadeListAttribute: Array(descriptors.dropFirst()),
        ] as CFDictionary)
        let font = CTFontCreateWithFontDescriptor(descriptor, size, nil)
        fonts[size] = font
        return font
    }
}
