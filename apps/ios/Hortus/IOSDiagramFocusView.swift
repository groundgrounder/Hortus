import CoreGraphics
import Foundation
import HortusApple
import HortusCore
import SwiftUI
import UIKit

struct IOSDiagramFocusView: View {
    let session: IOSDiagramFocusSession
    let appModel: IOSAppModel
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dismiss) private var dismiss

    @State private var prepared: PreparedScene?
    @State private var collapsed: Set<String> = []
    @State private var scale: Double = 1.0
    @State private var offset: CGSize = .zero
    @State private var shareURL: URL?
    @State private var exportError: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color(uiColor: .systemBackground).ignoresSafeArea()
                if let prepared {
                    FocusCanvasRepresentable(
                        prepared: prepared,
                        scale: $scale,
                        offset: $offset,
                        onToggleNode: toggleNode,
                        onOpenLink: { target, heading in
                            dismiss()
                            appModel.openLink(target: target, heading: heading)
                        }
                    )
                    .ignoresSafeArea(edges: .bottom)
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("\(Int((scale * 100).rounded()))%")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L10n.string("Close")) {
                        persistViewport()
                        dismiss()
                    }
                    .accessibilityIdentifier("focus-close")
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button(L10n.string("Fit")) {
                        fitToViewport()
                    }
                    .accessibilityIdentifier("focus-fit")
                    Button("100%") {
                        scale = 1.0
                        offset = .zero
                        persistViewport()
                    }
                    .accessibilityIdentifier("focus-100")
                    Menu {
                        Button(L10n.string("Export SVG")) { exportDiagram(.svg) }
                        Button(L10n.string("Export PNG")) { exportDiagram(.png) }
                        Button(L10n.string("Export PDF")) { exportDiagram(.pdf) }
                    } label: {
                        Image(systemName: "square.and.arrow.up")
                            .accessibilityLabel(L10n.string("Export Diagram"))
                    }
                    .accessibilityIdentifier("focus-export")
                }
            }
            .task(id: "\(colorScheme == .dark)-\(session.document.revision)") {
                await loadScene(preserveTransform: prepared != nil)
            }
            .sheet(item: Binding(
                get: { shareURL.map { ShareFileItem(url: $0) } },
                set: { if $0 == nil { shareURL = nil } }
            )) { item in
                ShareSheet(url: item.url)
            }
            .alert(L10n.string("Export Failed"), isPresented: Binding(
                get: { exportError != nil },
                set: { if !$0 { exportError = nil } }
            )) {
                Button(L10n.string("OK"), role: .cancel) { exportError = nil }
            } message: {
                Text(exportError ?? "")
            }
        }
    }

    private func loadScene(preserveTransform: Bool) async {
        let dark = colorScheme == .dark
        let source = session.document.text
        let mmd = session.note.lowercased().hasSuffix(".mmd")
        let initialCollapsed = (try? appModel.core?.collapsedNodes(vault: session.vault, note: session.note, block: session.ordinal)) ?? []
        if !preserveTransform {
            collapsed = Set(initialCollapsed)
        }
        let block = NoteDiagram(
            ordinal: session.ordinal,
            sourceStart: 0,
            range: NSRange(location: 0, length: (source as NSString).length),
            collapsed: collapsed,
            documentIdentity: "focus:" + session.document.diagramIdentity,
            revision: session.document.revision
        )
        let rendered = await appModel.diagramService.render(
            source: source, mmd: mmd, blocks: [block], width: 1200, dark: dark,
            requestGeneration: 1, fontGeneration: appModel.settings.bodyFontSize.bitPattern
        )
        prepared = rendered.first?.prepared
        if !preserveTransform {
            if let initial = session.initial {
                scale = max(0.15, min(6.0, initial.scale))
                offset = CGSize(width: initial.offsetX, height: initial.offsetY)
            } else {
                fitToViewport()
            }
        }
    }

    private func fitToViewport() {
        guard let prepared else { return }
        let bounds = UIScreen.main.bounds
        let availW = max(240.0, Double(bounds.width) - 32.0)
        let availH = max(240.0, Double(bounds.height) - 140.0)
        let fitted = min(1.5, max(0.2, min(availW / max(1, prepared.scene.width), availH / max(1, prepared.scene.height))))
        scale = fitted
        offset = CGSize(
            width: (availW - prepared.scene.width * fitted) / 2 + 16,
            height: max(24, (availH - prepared.scene.height * fitted) / 2)
        )
        persistViewport()
    }

    private func toggleNode(_ nodeId: String) {
        if collapsed.contains(nodeId) { collapsed.remove(nodeId) } else { collapsed.insert(nodeId) }
        appModel.rememberDiagramCollapsed(ordinal: session.ordinal, collapsed: collapsed.sorted())
        Task { await loadScene(preserveTransform: true) }
    }

    private func persistViewport() {
        appModel.rememberDiagramFocus(ordinal: session.ordinal, scale: scale, offsetX: offset.width, offsetY: offset.height)
    }

    private func exportDiagram(_ format: DiagramExportFormat) {
        guard let prepared else { return }
        let source = session.document.text
        let mmd = session.note.lowercased().hasSuffix(".mmd")
        let dark = colorScheme == .dark
        let ordinal = session.ordinal
        let collapsedList = collapsed.sorted()
        let request = DiagramExportRequest(prepared: prepared, note: session.note, ordinal: ordinal) {
            try await DiagramExport.renderSVG(source: source, mmd: mmd, block: ordinal,
                                              width: prepared.scene.width, dark: dark, collapsed: collapsedList)
        }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-exports", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(DiagramExport.defaultName(note: session.note, ordinal: ordinal, format: format))
        Task {
            do {
                try await DiagramExport.export(format, request: request, to: url)
                shareURL = url
            } catch {
                exportError = error.localizedDescription
            }
        }
    }
}

private struct ShareFileItem: Identifiable {
    let url: URL
    var id: String { url.path }
}

private struct ShareSheet: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }
    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}

private struct FocusCanvasRepresentable: UIViewRepresentable {
    let prepared: PreparedScene
    @Binding var scale: Double
    @Binding var offset: CGSize
    let onToggleNode: (String) -> Void
    let onOpenLink: (String, String?) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    func makeUIView(context: Context) -> FocusCanvasUIView {
        let view = FocusCanvasUIView()
        view.backgroundColor = .clear
        view.contentMode = .redraw
        view.accessibilityIdentifier = "focus-canvas"

        let pinch = UIPinchGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handlePinch(_:)))
        let pan = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handlePan(_:)))
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handleTap(_:)))
        view.addGestureRecognizer(pinch)
        view.addGestureRecognizer(pan)
        view.addGestureRecognizer(tap)
        return view
    }

    func updateUIView(_ uiView: FocusCanvasUIView, context: Context) {
        context.coordinator.parent = self
        uiView.prepared = prepared
        uiView.scale = scale
        uiView.offset = offset
        uiView.setNeedsDisplay()
    }

    @MainActor
    final class Coordinator: NSObject {
        var parent: FocusCanvasRepresentable
        private var startScale: Double = 1.0
        private var startOffset: CGSize = .zero

        init(parent: FocusCanvasRepresentable) {
            self.parent = parent
        }

        @objc func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
            switch recognizer.state {
            case .began:
                startScale = parent.scale
                startOffset = parent.offset
            case .changed:
                let nextScale = min(6.0, max(0.15, startScale * Double(recognizer.scale)))
                let anchor = recognizer.location(in: recognizer.view)
                let ratio = nextScale / max(0.01, startScale)
                parent.scale = nextScale
                parent.offset = CGSize(
                    width: Double(anchor.x) - (Double(anchor.x) - startOffset.width) * ratio,
                    height: Double(anchor.y) - (Double(anchor.y) - startOffset.height) * ratio
                )
            default: break
            }
        }

        @objc func handlePan(_ recognizer: UIPanGestureRecognizer) {
            switch recognizer.state {
            case .began:
                startOffset = parent.offset
            case .changed:
                let delta = recognizer.translation(in: recognizer.view)
                parent.offset = CGSize(width: startOffset.width + Double(delta.x), height: startOffset.height + Double(delta.y))
            default: break
            }
        }

        @objc func handleTap(_ recognizer: UITapGestureRecognizer) {
            let point = recognizer.location(in: recognizer.view)
            let scenePoint = CGPoint(
                x: (Double(point.x) - parent.offset.width) / max(0.01, parent.scale),
                y: (Double(point.y) - parent.offset.height) / max(0.01, parent.scale)
            )
            if let hit = parent.prepared.scene.hitRegions.last(where: { $0.rect.cgRect.contains(scenePoint) }) {
                if let target = hit.linkTarget, !target.isEmpty {
                    parent.onOpenLink(target, hit.linkHeading)
                    return
                }
                if hit.canCollapse {
                    parent.onToggleNode(hit.nodeId)
                }
            }
        }
    }
}

private final class FocusCanvasUIView: UIView {
    var prepared: PreparedScene?
    var scale: Double = 1.0
    var offset: CGSize = .zero

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext(), let prepared else { return }
        context.saveGState()
        context.translateBy(x: offset.width, y: offset.height)
        context.scaleBy(x: scale, y: scale)
        SceneRenderer.draw(prepared, in: context)
        context.restoreGState()
    }
}
