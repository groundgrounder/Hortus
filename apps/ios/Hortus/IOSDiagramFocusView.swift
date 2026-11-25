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
    @State private var viewportSize: CGSize = .zero
    @State private var loadedWidth: Double = 0
    @State private var renderGeneration: UInt64 = 0
    @State private var widthDebounceTask: Task<Void, Never>?
    @State private var shareURL: URL?
    @State private var exportError: String?

    static func renderWidth(for viewportSize: CGSize) -> Double {
        max(240.0, Double(viewportSize.width) - 32.0)
    }

    static func fitMetrics(sceneSize: CGSize, viewportSize: CGSize) -> (scale: Double, offset: CGSize) {
        let availW = max(240.0, Double(viewportSize.width) - 32.0)
        let availH = max(240.0, Double(viewportSize.height) - 48.0)
        let fitted = min(1.5, max(0.2, min(availW / max(1.0, Double(sceneSize.width)),
                                           availH / max(1.0, Double(sceneSize.height)))))
        let nextOffset = CGSize(
            width: (Double(viewportSize.width) - Double(sceneSize.width) * fitted) / 2.0,
            height: (Double(viewportSize.height) - Double(sceneSize.height) * fitted) / 2.0
        )
        return (fitted, nextOffset)
    }

    var body: some View {
        NavigationStack {
            GeometryReader { proxy in
                let size = proxy.size
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
                            },
                            onGestureEnded: {
                                persistViewport()
                            }
                        )
                    } else {
                        ProgressView()
                    }
                }
                .onAppear {
                    handleViewportSizeChange(size)
                }
                .onChange(of: size) { _, newSize in
                    handleViewportSizeChange(newSize)
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
                        fitToViewport(in: viewportSize)
                    }
                    .accessibilityIdentifier("focus-fit")
                    Button("100%") {
                        scale = 1.0
                        if let prepared, viewportSize.width > 1 {
                            offset = CGSize(
                                width: (Double(viewportSize.width) - prepared.scene.width) / 2.0,
                                height: (Double(viewportSize.height) - prepared.scene.height) / 2.0
                            )
                        } else {
                            offset = .zero
                        }
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
            .task(id: "\(colorScheme == .dark)-\(session.document.revision)-\(viewportSize.width > 1)") {
                guard viewportSize.width > 1, viewportSize.height > 1 else { return }
                widthDebounceTask?.cancel()
                await loadScene(preserveTransform: prepared != nil, viewportSize: viewportSize)
            }
            .onDisappear {
                widthDebounceTask?.cancel()
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

    private func handleViewportSizeChange(_ newSize: CGSize) {
        guard newSize.width > 1, newSize.height > 1 else { return }
        let previousSize = viewportSize
        viewportSize = newSize
        guard prepared != nil else { return }
        let nextWidth = Self.renderWidth(for: newSize)
        guard abs(nextWidth - loadedWidth) > 1.0 else { return }
        let centerScene = previousSize.width > 1
            ? CGPoint(
                x: (Double(previousSize.width) / 2.0 - offset.width) / max(0.01, scale),
                y: (Double(previousSize.height) / 2.0 - offset.height) / max(0.01, scale)
            )
            : nil
        widthDebounceTask?.cancel()
        widthDebounceTask = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            await loadScene(preserveTransform: true, viewportSize: newSize, preserveCenter: centerScene)
        }
    }

    private func loadScene(preserveTransform: Bool, viewportSize: CGSize, preserveCenter: CGPoint? = nil) async {
        guard viewportSize.width > 1 else { return }
        let dark = colorScheme == .dark
        let source = session.document.text
        let mmd = session.note.lowercased().hasSuffix(".mmd")
        let initialCollapsed = (try? appModel.core?.collapsedNodes(vault: session.vault, note: session.note, block: session.ordinal)) ?? []
        if !preserveTransform {
            collapsed = Set(initialCollapsed)
        }
        let targetWidth = Self.renderWidth(for: viewportSize)
        renderGeneration &+= 1
        let requestGen = renderGeneration
        let block = NoteDiagram(
            ordinal: session.ordinal,
            sourceStart: 0,
            range: NSRange(location: 0, length: (source as NSString).length),
            collapsed: collapsed,
            documentIdentity: "focus:" + session.document.diagramIdentity,
            revision: session.document.revision
        )
        let rendered = await appModel.diagramService.render(
            source: source, mmd: mmd, blocks: [block], width: targetWidth, dark: dark,
            requestGeneration: requestGen, fontGeneration: appModel.settings.bodyFontSize.bitPattern
        )
        guard !Task.isCancelled, requestGen == renderGeneration else { return }
        loadedWidth = targetWidth
        prepared = rendered.first?.prepared
        if !preserveTransform {
            if let initial = session.initial {
                scale = max(0.15, min(6.0, initial.scale))
                offset = CGSize(width: initial.offsetX, height: initial.offsetY)
            } else {
                fitToViewport(in: viewportSize)
            }
        } else if let preserveCenter {
            offset = CGSize(
                width: Double(viewportSize.width) / 2.0 - preserveCenter.x * scale,
                height: Double(viewportSize.height) / 2.0 - preserveCenter.y * scale
            )
        }
    }

    private func fitToViewport(in size: CGSize) {
        guard let prepared, size.width > 1, size.height > 1 else { return }
        let metrics = Self.fitMetrics(
            sceneSize: CGSize(width: prepared.scene.width, height: prepared.scene.height),
            viewportSize: size
        )
        scale = metrics.scale
        offset = metrics.offset
        persistViewport()
    }

    private func toggleNode(_ nodeId: String) {
        if collapsed.contains(nodeId) { collapsed.remove(nodeId) } else { collapsed.insert(nodeId) }
        appModel.rememberDiagramCollapsed(ordinal: session.ordinal, collapsed: collapsed.sorted())
        Task { await loadScene(preserveTransform: true, viewportSize: viewportSize) }
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
    let onGestureEnded: () -> Void

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
            case .ended, .cancelled:
                parent.onGestureEnded()
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
            case .ended, .cancelled:
                parent.onGestureEnded()
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
