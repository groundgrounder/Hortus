import CoreGraphics
import Foundation
import HortusCore
import ImageIO
import UniformTypeIdentifiers

/// 图表导出格式；菜单标题、扩展名与内容类型集中在这里。
public enum DiagramExportFormat: String, CaseIterable, Sendable {
    case svg
    case png
    case pdf

    public var fileExtension: String { rawValue }

    public var contentType: UTType {
        switch self {
        case .svg: return .svg
        case .png: return .png
        case .pdf: return .pdf
        }
    }

    public var menuTitle: String { "\(rawValue.uppercased())…" }
}

public enum DiagramExportStrings {
    public static var exportDiagram: String { L10n.string("Export Diagram") }
    public static var export: String { L10n.string("Export") }
    public static var openLinkedNote: String { L10n.string("Open Linked Note") }
    public static var exportFailed: String { L10n.string("Export Failed") }
    public static var exportFailedMessage: String { L10n.string("The diagram could not be rendered or written.") }
}

public enum DiagramExportError: LocalizedError {
    case render
    case encode

    public var errorDescription: String? {
        switch self {
        case .render: return DiagramExportStrings.exportFailedMessage
        case .encode: return DiagramExportStrings.exportFailedMessage
        }
    }
}

/// 一次导出所需的不可变场景与按需生成的 SVG；宿主负责提供当前源码与折叠状态。
@MainActor
public struct DiagramExportRequest {
    public let prepared: PreparedScene
    public let note: String
    public let ordinal: UInt32
    public let svg: () async throws -> String

    public init(prepared: PreparedScene, note: String, ordinal: UInt32, svg: @escaping () async throws -> String) {
        self.prepared = prepared
        self.note = note
        self.ordinal = ordinal
        self.svg = svg
    }
}

/// 导出专用引擎：避免主线程承担字体预热与全文重排版。
public actor DiagramSVGEngine {
    private var engine: DiagramEngine?
    #if DEBUG
    public private(set) var inspectionInitializedOnMain: Bool?
    #endif

    public init() {}

    public func render(source: String, mmd: Bool, block: UInt32, width: Double, dark: Bool,
                       collapsed: [String]) throws -> String {
        if engine == nil {
            #if DEBUG
            inspectionInitializedOnMain = Thread.isMainThread
            #endif
            engine = try? PlatformGraphics.makeDiagramEngine()
        }
        guard let engine else { throw DiagramExportError.render }
        return try engine.renderSvg(source: source, mmd: mmd, block: block,
                                    width: width, dark: dark, collapsed: collapsed)
    }
}

@MainActor
public enum DiagramExport {
    private static let svgEngine = DiagramSVGEngine()

    /// 直接从屏幕已排好的 `PreparedScene` 导出 SVG，不重新解析或重排，保证三端及三格式完全一致。
    nonisolated public static func renderSVG(prepared: PreparedScene) throws -> String {
        try sceneSvg(scene: prepared.scene.sceneData)
    }

    /// 与屏幕同一套 Rust 布局；在导出引擎上运行，点击菜单后不阻塞主线程。
    nonisolated public static func renderSVG(source: String, mmd: Bool, block: UInt32,
                                             width: Double, dark: Bool, collapsed: [String]) async throws -> String {
        try await svgEngine.render(source: source, mmd: mmd, block: block,
                                   width: width, dark: dark, collapsed: collapsed)
    }

    /// 默认文件名取当前笔记名与图块序号，例如 `Learning-map-diagram-1.svg`。
    nonisolated public static func defaultName(note: String, ordinal: UInt32,
                                               format: DiagramExportFormat) -> String {
        let base = URL(fileURLWithPath: note).deletingPathExtension().lastPathComponent
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let name = base.isEmpty ? "Diagram" : base
        return "\(name)-diagram-\(ordinal).\(format.fileExtension)"
    }

    /// 2x Retina 位图；与屏幕绘制共用 `SceneRenderer`，含背景、字体、连线与阴影细节。
    nonisolated public static func renderPNG(prepared: PreparedScene, scale: CGFloat = 2) -> Data? {
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
    nonisolated public static func renderPDF(prepared: PreparedScene) -> Data? {
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

    nonisolated public static func writeSVG(_ svg: String, to url: URL) throws {
        guard let data = svg.data(using: .utf8) else { throw DiagramExportError.encode }
        try write(data, to: url)
    }

    nonisolated public static func write(_ data: Data, to url: URL) throws {
        try data.write(to: url, options: .atomic)
    }

    /// 无弹窗落盘入口：验收脚本、macOS 右键菜单与 iOS 分享导出共用同一编码路径。
    public static func export(_ format: DiagramExportFormat, request: DiagramExportRequest,
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
}
