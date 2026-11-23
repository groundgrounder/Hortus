import AppKit
import CoreGraphics
import HortusCore
import UniformTypeIdentifiers

/// 图表导出格式；菜单标题、扩展名与 `NSSavePanel` 内容类型集中在这里。
enum DiagramExportFormat: String, CaseIterable {
    case svg
    case png
    case pdf

    var fileExtension: String { rawValue }

    var contentType: UTType {
        switch self {
        case .svg: return .svg
        case .png: return .png
        case .pdf: return .pdf
        }
    }

    var menuTitle: String { "\(rawValue.uppercased())…" }
}

enum DiagramExportStrings {
    static var exportDiagram: String { L10n.string("Export Diagram") }
    static var export: String { L10n.string("Export") }
    static var openLinkedNote: String { L10n.string("Open Linked Note") }
    static var exportFailed: String { L10n.string("Export Failed") }
    static var exportFailedMessage: String { L10n.string("The diagram could not be rendered or written.") }
}

enum DiagramExportError: LocalizedError {
    case render
    case encode

    var errorDescription: String? {
        switch self {
        case .render: return DiagramExportStrings.exportFailedMessage
        case .encode: return DiagramExportStrings.exportFailedMessage
        }
    }
}

/// 一次导出所需的不可变场景与按需生成的 SVG；宿主负责提供当前源码与折叠状态。
@MainActor
struct DiagramExportRequest {
    let prepared: PreparedScene
    let note: String
    let ordinal: UInt32
    let svg: () async throws -> String
}

/// 导出专用引擎：避免主线程承担字体预热与全文重排版。
actor DiagramSVGEngine {
    private var engine: DiagramEngine?
    #if DEBUG
    private(set) var inspectionInitializedOnMain: Bool?
    #endif

    func render(source: String, mmd: Bool, block: UInt32, width: Double, dark: Bool,
                collapsed: [String]) throws -> String {
        if engine == nil {
            #if DEBUG
            inspectionInitializedOnMain = Thread.isMainThread
            #endif
            engine = try? DiagramEngine()
        }
        guard let engine else { throw DiagramExportError.render }
        return try engine.renderSvg(source: source, mmd: mmd, block: block,
                                    width: width, dark: dark, collapsed: collapsed)
    }
}

@MainActor
enum DiagramExport {
    private static let svgEngine = DiagramSVGEngine()

    /// 与屏幕同一套 Rust 布局；在导出引擎上运行，点击菜单后不阻塞主线程。
    nonisolated static func renderSVG(source: String, mmd: Bool, block: UInt32,
                                      width: Double, dark: Bool, collapsed: [String]) async throws -> String {
        try await svgEngine.render(source: source, mmd: mmd, block: block,
                                   width: width, dark: dark, collapsed: collapsed)
    }

    /// 默认文件名取当前笔记名与图块序号，例如 `Learning-map-diagram-1.svg`。
    nonisolated static func defaultName(note: String, ordinal: UInt32,
                                        format: DiagramExportFormat) -> String {
        let base = URL(fileURLWithPath: note).deletingPathExtension().lastPathComponent
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let name = base.isEmpty ? "Diagram" : base
        return "\(name)-diagram-\(ordinal).\(format.fileExtension)"
    }

    /// 2x Retina 位图；与屏幕绘制共用 `SceneRenderer`，含背景、字体、连线与阴影细节。
    nonisolated static func renderPNG(prepared: PreparedScene, scale: CGFloat = 2) -> Data? {
        let scene = prepared.scene
        let width = Int((scene.width * scale).rounded(.up))
        let height = Int((scene.height * scale).rounded(.up))
        guard width > 0, height > 0,
              let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(data: nil, width: width, height: height,
                                      bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        context.setShouldAntialias(true)
        context.interpolationQuality = .high
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: scale, y: -scale)
        context.clip(to: CGRect(x: 0, y: 0, width: scene.width, height: scene.height))
        SceneRenderer.draw(prepared, in: context, highlight: nil)
        guard let image = context.makeImage() else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(
            data, UTType.png.identifier as CFString, 1, nil
        ) else { return nil }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return data as Data
    }

    /// `CGPDFContext` 写出的真矢量页面，文字与路径都可无限放大。
    nonisolated static func renderPDF(prepared: PreparedScene) -> Data? {
        let scene = prepared.scene
        guard scene.width > 0, scene.height > 0 else { return nil }
        let data = NSMutableData()
        guard let consumer = CGDataConsumer(data: data as CFMutableData) else { return nil }
        var mediaBox = CGRect(x: 0, y: 0, width: scene.width, height: scene.height)
        guard let context = CGContext(consumer: consumer, mediaBox: &mediaBox, nil) else {
            return nil
        }
        context.beginPDFPage(nil)
        context.translateBy(x: 0, y: mediaBox.height)
        context.scaleBy(x: 1, y: -1)
        context.clip(to: CGRect(x: 0, y: 0, width: scene.width, height: scene.height))
        SceneRenderer.draw(prepared, in: context, highlight: nil)
        context.endPDFPage()
        context.closePDF()
        return data as Data
    }

    nonisolated static func writeSVG(_ svg: String, to url: URL) throws {
        guard let data = svg.data(using: .utf8) else { throw DiagramExportError.encode }
        try write(data, to: url)
    }

    nonisolated static func write(_ data: Data, to url: URL) throws {
        try data.write(to: url, options: .atomic)
    }

    /// 无弹窗落盘入口：验收脚本与右键菜单共用同一编码路径。
    static func export(_ format: DiagramExportFormat, request: DiagramExportRequest,
                       to url: URL) async throws {
        switch format {
        case .svg:
            try writeSVG(try await request.svg(), to: url)
        case .png:
            let prepared = request.prepared
            guard let data = await Task.detached(priority: .userInitiated, operation: {
                renderPNG(prepared: prepared)
            }).value else { throw DiagramExportError.render }
            try write(data, to: url)
        case .pdf:
            let prepared = request.prepared
            guard let data = await Task.detached(priority: .userInitiated, operation: {
                renderPDF(prepared: prepared)
            }).value else { throw DiagramExportError.render }
            try write(data, to: url)
        }
    }

    /// 原生保存面板；用户选定路径后才在后台生成并写入。
    static func presentSavePanel(format: DiagramExportFormat, request: DiagramExportRequest,
                                 window: NSWindow?, completion: ((URL?) -> Void)? = nil) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [format.contentType]
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = defaultName(note: request.note, ordinal: request.ordinal,
                                                 format: format)
        let finish: @MainActor (NSApplication.ModalResponse) -> Void = { response in
            guard response == .OK, let url = panel.url else {
                completion?(nil)
                return
            }
            Task { @MainActor in
                do {
                    try await export(format, request: request, to: url)
                    completion?(url)
                } catch {
                    presentFailure(error, window: window)
                    completion?(nil)
                }
            }
        }
        if let window {
            panel.beginSheetModal(for: window) { response in
                MainActor.assumeIsolated { finish(response) }
            }
        } else {
            panel.begin { response in
                MainActor.assumeIsolated { finish(response) }
            }
        }
    }

    private static func presentFailure(_ error: Error, window: NSWindow?) {
        let alert = NSAlert()
        alert.messageText = DiagramExportStrings.exportFailed
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        if let window {
            alert.beginSheetModal(for: window, completionHandler: nil)
        } else {
            alert.runModal()
        }
    }
}
