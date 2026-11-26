import Foundation
import HortusApple
import HortusCore
import SwiftUI
import UIKit

struct IOSEditingTextView: UIViewRepresentable {
    let document: IOSNoteDocument
    let fontSize: Double

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = document.readOnlyReason == nil
        view.isSelectable = true
        view.backgroundColor = .clear
        view.alwaysBounceVertical = true
        view.keyboardDismissMode = .interactive
        view.textContainerInset = UIEdgeInsets(top: 16, left: 18, bottom: 28, right: 18)
        view.textContainer.lineFragmentPadding = 0
        view.delegate = context.coordinator
        view.font = PlatformGraphics.systemFont(ofSize: fontSize)
        view.textColor = .label
        view.accessibilityIdentifier = "ios-editing-text"
        context.coordinator.view = view
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        context.coordinator.update(self, view: view)
    }

    static func dismantleUIView(_ uiView: UITextView, coordinator: Coordinator) {
        coordinator.teardown(view: uiView)
    }

    @MainActor
    final class Coordinator: NSObject, UITextViewDelegate {
        weak var view: UITextView?
        private var input: IOSEditingTextView?
        private var applyingProgrammatic = false
        private var styleTask: Task<Void, Never>?
        private var lastStyledRevision: UInt64 = .max
        private var lastSize: Double = 0
        private var forceStyle = true
        private(set) var styleRequestCount = 0

        static func minimalEdit(from oldText: String, to newText: String) -> (range: NSRange, replacement: String)? {
            guard oldText != newText else { return nil }
            let oldNS = oldText as NSString
            let newNS = newText as NSString
            let oldLen = oldNS.length
            let newLen = newNS.length
            let minLen = min(oldLen, newLen)
            var prefix = 0
            while prefix < minLen && oldNS.character(at: prefix) == newNS.character(at: prefix) {
                prefix += 1
            }
            if prefix > 0 && (0xD800...0xDBFF).contains(oldNS.character(at: prefix - 1)) {
                prefix -= 1
            }
            var suffix = 0
            let remaining = min(oldLen - prefix, newLen - prefix)
            while suffix < remaining &&
                  oldNS.character(at: oldLen - 1 - suffix) == newNS.character(at: newLen - 1 - suffix) {
                suffix += 1
            }
            if suffix > 0 && (0xDC00...0xDFFF).contains(oldNS.character(at: oldLen - suffix)) {
                suffix -= 1
            }
            let range = NSRange(location: prefix, length: oldLen - prefix - suffix)
            let replacement = newNS.substring(with: NSRange(location: prefix, length: newLen - prefix - suffix))
            return (range, replacement)
        }

        func reconcileIfNeeded(textView: UITextView, doc: IOSNoteDocument) {
            let current = textView.text ?? ""
            guard let edit = Self.minimalEdit(from: doc.text, to: current) else { return }
            doc.edited(range: edit.range, replacement: edit.replacement)
        }

        func teardown(view: UITextView) {
            styleTask?.cancel()
            styleTask = nil
            if let doc = input?.document {
                if view.markedTextRange != nil {
                    view.unmarkText()
                }
                if view.markedTextRange == nil {
                    reconcileIfNeeded(textView: view, doc: doc)
                    if doc.marked {
                        doc.compositionChanged(false)
                    }
                }
                doc.onEndEditing = nil
            }
        }

        func update(_ next: IOSEditingTextView, view: UITextView) {
            let docChanged = input?.document !== next.document
            if docChanged {
                input?.document.onEndEditing = nil
            }
            self.input = next
            view.isEditable = next.document.readOnlyReason == nil

            next.document.onEndEditing = { [weak self, weak view] in
                guard let self, let view, let doc = self.input?.document else { return }
                view.endEditing(true)
                if view.markedTextRange != nil {
                    view.unmarkText()
                }
                self.reconcileIfNeeded(textView: view, doc: doc)
                doc.compositionChanged(false)
            }

            if docChanged {
                lastStyledRevision = .max
                forceStyle = true
                applyingProgrammatic = true
                view.text = next.document.text
                applyingProgrammatic = false
                next.document.onReload = { [weak self, weak view] reloaded in
                    guard let self, let view else { return }
                    if view.markedTextRange == nil && view.text != reloaded {
                        self.applyingProgrammatic = true
                        let selection = view.selectedRange
                        view.text = reloaded
                        let clamped = min(selection.location, (reloaded as NSString).length)
                        view.selectedRange = NSRange(location: clamped, length: 0)
                        self.applyingProgrammatic = false
                        self.forceStyle = true
                    }
                    self.scheduleStyles(immediate: true)
                }
            } else if view.markedTextRange == nil && view.text != next.document.text {
                applyingProgrammatic = true
                let selection = view.selectedRange
                view.text = next.document.text
                let clamped = min(selection.location, (next.document.text as NSString).length)
                view.selectedRange = NSRange(location: clamped, length: 0)
                applyingProgrammatic = false
                forceStyle = true
            }

            if lastSize != next.fontSize {
                lastSize = next.fontSize
                forceStyle = true
            }

            if docChanged || lastStyledRevision != next.document.revision || forceStyle {
                scheduleStyles(immediate: docChanged || forceStyle)
            }

            if let pending = next.document.pendingReveal {
                next.document.pendingReveal = nil
                let ns = view.text as NSString
                let loc = min(max(0, pending.source), ns.length)
                let len = min(max(0, pending.length), max(0, ns.length - loc))
                view.selectedRange = NSRange(location: loc, length: len)
                view.scrollRangeToVisible(view.selectedRange)
            }
        }

        func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
            guard !applyingProgrammatic, let doc = input?.document else { return true }
            if textView.markedTextRange != nil { return true }
            doc.edited(range: range, replacement: text)
            return true
        }

        func textViewDidChange(_ textView: UITextView) {
            guard !applyingProgrammatic, let doc = input?.document else { return }
            let marked = textView.markedTextRange != nil
            if marked {
                doc.compositionChanged(true)
            } else {
                reconcileIfNeeded(textView: textView, doc: doc)
                doc.compositionChanged(false)
                scheduleStyles()
            }
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            guard !applyingProgrammatic, let doc = input?.document else { return }
            if textView.markedTextRange == nil {
                reconcileIfNeeded(textView: textView, doc: doc)
                if doc.marked {
                    doc.compositionChanged(false)
                }
            }
        }

        private func scheduleStyles(immediate: Bool = false) {
            guard let input, let view else { return }
            let doc = input.document
            let size = input.fontSize
            let revision = doc.revision
            let io = doc.io
            let delay: Duration = (immediate || lastStyledRevision == .max || forceStyle) ? .zero : .milliseconds(95)
            styleTask?.cancel()
            styleTask = Task { [weak self, weak view] in
                do {
                    if delay != .zero {
                        try await Task.sleep(for: delay)
                    }
                    guard !Task.isCancelled, let self, let view,
                          self.input?.document === doc, doc.revision == revision,
                          view.markedTextRange == nil else { return }
                    let force = self.forceStyle || self.lastStyledRevision == .max
                    let since = force ? UInt64.max : self.lastStyledRevision
                    self.styleRequestCount += 1
                    let wire = try await io.perform { try $0.sourceStyles(since: since, force: force) }
                    guard !Task.isCancelled,
                          self.input?.document === doc, doc.revision == revision,
                          view.markedTextRange == nil else { return }
                    let runs = StyleBatch.decodeRuns(wire.packedRuns)
                    let storage = view.textStorage
                    let base = SourceStylePalette.attributes(for: 0, fontSize: size, contrast: false)
                    let selected = view.selectedRange
                    self.applyingProgrammatic = true
                    if force {
                        let fullRange = NSRange(location: 0, length: storage.length)
                        storage.beginEditing()
                        storage.setAttributes(base, range: fullRange)
                        for run in runs {
                            let range = NSRange(location: Int(run.start), length: Int(run.length))
                            if NSMaxRange(range) <= storage.length {
                                storage.setAttributes(SourceStylePalette.attributes(for: run.mask, fontSize: size, contrast: false), range: range)
                            }
                        }
                        storage.endEditing()
                    } else if let head = runs.first, let tail = runs.last {
                        let lower = Int(head.start)
                        let upper = min(storage.length, Int(tail.start + tail.length))
                        if upper >= lower {
                            let patchRange = NSRange(location: lower, length: upper - lower)
                            storage.beginEditing()
                            storage.setAttributes(base, range: patchRange)
                            for run in runs {
                                let range = NSRange(location: Int(run.start), length: Int(run.length))
                                if NSMaxRange(range) <= storage.length {
                                    storage.setAttributes(SourceStylePalette.attributes(for: run.mask, fontSize: size, contrast: false), range: range)
                                }
                            }
                            storage.endEditing()
                        }
                    }
                    view.selectedRange = selected
                    view.typingAttributes = base
                    self.applyingProgrammatic = false
                    self.lastStyledRevision = wire.revision
                    self.forceStyle = false
                } catch {}
            }
        }
    }
}
