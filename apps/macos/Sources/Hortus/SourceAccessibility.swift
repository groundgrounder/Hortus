import AppKit

extension DiagramTextView: @preconcurrency NSAccessibilityCustomRotorItemSearchDelegate {
    override func accessibilityCustomRotors() -> [NSAccessibilityCustomRotor] {
        [NSAccessibilityCustomRotor(rotorType: .heading, itemSearchDelegate: self)]
    }
    func rotor(_ rotor: NSAccessibilityCustomRotor, resultFor parameters: NSAccessibilityCustomRotor.SearchParameters) -> NSAccessibilityCustomRotor.ItemResult? {
        guard let storage = textStorage else { return nil }
        var headings: [(NSRange, Int)] = []
        sourceStyling?.overlapping(NSRange(location: 0, length: storage.length)) { run, range in
            let mask = run.mask
            guard mask >> 24 > 0, mask & 1 == 0 else { return }
            let line = storage.mutableString.lineRange(for: range)
            if headings.last?.0 != line { headings.append((line, Int(mask >> 24))) }
        }
        let forward = parameters.searchDirection == .next
        let current = parameters.currentItem?.targetRange
        let candidates = forward ? headings : headings.reversed()
        for (range, level) in candidates {
            if let current, current.location != NSNotFound,
               forward ? range.location <= current.location : range.location >= current.location { continue }
            let label = storage.mutableString.substring(with: range).trimmingCharacters(in: .whitespacesAndNewlines)
            if !parameters.filterString.isEmpty && !label.localizedCaseInsensitiveContains(parameters.filterString) { continue }
            let result = NSAccessibilityCustomRotor.ItemResult(targetElement: self)
            result.targetRange = range
            result.customLabel = L10n.string("Heading level %1$@, %2$@", String(level), label)
            return result
        }
        return nil
    }
}
