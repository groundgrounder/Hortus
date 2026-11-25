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

    @MainActor
    final class Coordinator: NSObject, UITextViewDelegate {
        weak var view: UITextView?
        private var input: IOSEditingTextView?
        private var applyingProgrammatic = false
        private var styleTask: Task<Void, Never>?
        private var lastStyledRevision: UInt64 = .max
        private var lastSize: Double = 0

        func update(_ next: IOSEditingTextView, view: UITextView) {
            let docChanged = input?.document !== next.document
            self.input = next
            view.isEditable = next.document.readOnlyReason == nil

            if docChanged {
                lastStyledRevision = .max
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
                    }
                    self.scheduleStyles()
                }
            } else if view.markedTextRange == nil && view.text != next.document.text {
                applyingProgrammatic = true
                let selection = view.selectedRange
                view.text = next.document.text
                let clamped = min(selection.location, (next.document.text as NSString).length)
                view.selectedRange = NSRange(location: clamped, length: 0)
                applyingProgrammatic = false
            }

            if docChanged || lastStyledRevision != next.document.revision || lastSize != next.fontSize {
                lastSize = next.fontSize
                scheduleStyles()
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
            doc.compositionChanged(marked)
            if !marked { scheduleStyles() }
        }

        private func scheduleStyles() {
            guard let input, let view else { return }
            let doc = input.document
            let size = input.fontSize
            let revision = doc.revision
            let io = doc.io
            styleTask?.cancel()
            styleTask = Task { [weak self, weak view] in
                do {
                    let wire = try await io.perform { try $0.sourceStyles(since: UInt64.max, force: true) }
                    guard !Task.isCancelled, let self, let view,
                          self.input?.document === doc, doc.revision == revision,
                          view.markedTextRange == nil else { return }
                    let runs = StyleBatch.decodeRuns(wire.packedRuns)
                    let storage = view.textStorage
                    let base = SourceStylePalette.attributes(for: 0, fontSize: size, contrast: false)
                    let fullRange = NSRange(location: 0, length: storage.length)
                    let selected = view.selectedRange
                    self.applyingProgrammatic = true
                    storage.beginEditing()
                    storage.setAttributes(base, range: fullRange)
                    for run in runs {
                        let range = NSRange(location: Int(run.start), length: Int(run.length))
                        if NSMaxRange(range) <= storage.length {
                            storage.setAttributes(SourceStylePalette.attributes(for: run.mask, fontSize: size, contrast: false), range: range)
                        }
                    }
                    storage.endEditing()
                    view.selectedRange = selected
                    view.typingAttributes = base
                    self.applyingProgrammatic = false
                    self.lastStyledRevision = revision
                } catch {}
            }
        }
    }
}
