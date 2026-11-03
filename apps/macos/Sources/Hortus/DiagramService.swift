import Foundation
import CoreText
import HortusCore

struct NoteDiagram: Sendable {
    let ordinal: UInt32
    var range: NSRange
    var collapsed: Set<String>
    var tailStart = 0
    var prepared: PreparedScene?
    var documentIdentity = ""
    var revision: UInt64 = 0
    var sceneVersion: UInt32 = 0
    var previous: DiagramPrevious?
    var viewport: [DiagramViewportAnchor] = []
    mutating func inherit(_ old: NoteDiagram) {
        guard documentIdentity == old.documentIdentity else { return }
        collapsed = old.collapsed
        prepared = old.prepared
        previous = old.previous
        sceneVersion = old.sceneVersion
        viewport = old.viewport
    }
    var reason: String?
    var diagnostic: String?
    var renderMilliseconds = 0.0
    var transferMilliseconds = 0.0
    var layoutMilliseconds = 0.0
}
/// 应用存活期间复用单个引擎。Task 创建即在后台预热，不占用主线程。
actor DiagramService {
    private var lineCache: [DiagramLineKey: CTLine] = [:]
    private var cachedSource: String?
    private var activeDocument = ""
    private var previousBlocks: [UInt32: NoteDiagram] = [:]
    private let engine = Task.detached(priority: .userInitiated) { try DiagramEngine() }

    func blocks(snapshot: DocumentSnapshot, vault: String, note: String, core: WorkspaceCore?, documentIdentity: String? = nil) throws -> [NoteDiagram] {
        let text = snapshot.text as NSString
        return try snapshot.blocks.map { block in
            let collapsed = try core?.collapsedNodes(vault: vault, note: note, block: block.ordinal) ?? []
            let range = NSRange(location: Int(block.start), length: Int(block.length))
            let tail = text.length > 0 ? text.paragraphRange(for: NSRange(location: max(0, NSMaxRange(range) - 1), length: 0)).location : 0
            return NoteDiagram(ordinal: block.ordinal, range: range, collapsed: Set(collapsed), tailStart: tail,
                               documentIdentity: documentIdentity ?? (vault + "\u{0}" + note), revision: snapshot.revision)
        }
    }

    func blocks(source: String, mmd: Bool, vault: String, note: String, core: WorkspaceCore?, documentIdentity: String? = nil) throws -> [NoteDiagram] {
        let blocks = mmd ? [DiagramBlock(ordinal: 1, fenceStart: 0, fenceEnd: UInt64(source.utf8.count),
                                         sourceStart: 0, sourceEnd: UInt64(source.utf8.count), diagramType: "")]
            : diagramBlocks(note: source)
        return try blocks.map { block in
            guard let range = Self.sourceRange(start: block.fenceStart, end: block.fenceEnd, in: source) else {
                throw DiagramSceneFailure.sourceRange
            }
            let collapsed = try core?.collapsedNodes(vault: vault, note: note, block: block.ordinal) ?? []
            let text = source as NSString
            let tail = text.length > 0 ? text.paragraphRange(for: NSRange(location: max(0, NSMaxRange(range) - 1), length: 0)).location : 0
            return NoteDiagram(ordinal: block.ordinal, range: range, collapsed: Set(collapsed), tailStart: tail,
                               documentIdentity: documentIdentity ?? (vault + "\u{0}" + note))
        }
    }

    func render(source: String, mmd: Bool, blocks: [NoteDiagram], width: Double, dark: Bool, requestGeneration: UInt64 = 0, fontGeneration: UInt64 = 0) async -> [NoteDiagram] {
        // 空正文无需排版；保留最后一篇图文档的有界行缓存，重新打开时复用 CoreText 对象。
        guard !blocks.isEmpty else { return blocks }
        var result = blocks
        do {
            let engine = try await engine.value
            return autoreleasepool {
                if cachedSource != source { lineCache = [:]; cachedSource = source }
                let document = result[0].documentIdentity
                if activeDocument != document { previousBlocks = [:]; activeDocument = document }
                for index in result.indices {
                    if let old = previousBlocks[result[index].ordinal], old.revision <= result[index].revision {
                        let viewport = result[index].viewport
                        let collapsed = result[index].collapsed
                        result[index].inherit(old)
                        result[index].viewport = viewport
                        result[index].collapsed = collapsed
                    }
                    do {
                        let started = ProcessInfo.processInfo.systemUptime
                        guard !Task.isCancelled else { return blocks }
                        let identity = DiagramIdentity(document: result[index].documentIdentity, block: result[index].ordinal,
                            revision: result[index].revision, dark: dark, fontGeneration: fontGeneration, requestGeneration: requestGeneration)
                        let scene = try engine.renderRequest(source: source, mmd: mmd, width: width,
                            collapsed: Array(result[index].collapsed), identity: identity, previous: result[index].previous,
                            viewport: result[index].viewport)
                        guard !Task.isCancelled else { return blocks }
                        guard scene.identity == identity, abs(scene.width - width) < 0.001 else { throw DiagramSceneFailure.staleRequest }
                        var ranges: [String: NSRange] = [:]
                        for hit in scene.hitRegions {
                            guard let range = Self.sourceRange(start: hit.sourceStart, end: hit.sourceEnd, in: source) else { throw DiagramSceneFailure.sourceRange }
                            ranges[hit.nodeId] = range
                        }
                        var edgeRanges: [UInt64: NSRange] = [:]
                        var edgeLabels: [UInt64: String] = [:]
                        for primitive in scene.primitives {
                            if case let .graphPath(id, _, _, start, end, _, _, _, _, _, _, _) = primitive {
                                guard let range = Self.sourceRange(start: start, end: end, in: source) else { throw DiagramSceneFailure.sourceRange }
                                edgeRanges[id] = range
                                edgeLabels[id] = (source as NSString).substring(with: range)
                            }
                        }
                        let transferred = ProcessInfo.processInfo.systemUptime
                        result[index].prepared = PreparedScene(scene: try DiagramScene(scene), payloadBytes: Int(scene.wireBytes), sourceRanges: ranges, edgeRanges: edgeRanges, edgeLabels: edgeLabels) { span in
                            let key = DiagramLineKey(span)
                            if let line = lineCache[key] { return line }
                            let line = SceneRenderer.line(for: span)
                            lineCache[key] = line
                            return line
                        }
                        result[index].renderMilliseconds = (ProcessInfo.processInfo.systemUptime - started) * 1000
                        result[index].transferMilliseconds = (transferred - started) * 1000
                        result[index].layoutMilliseconds = scene.layoutMs
                        result[index].previous = scene.previous
                        result[index].sceneVersion = scene.schemaVersion
                        result[index].reason = nil
                        result[index].diagnostic = nil
                        previousBlocks[result[index].ordinal] = result[index]
                    } catch {
                        // 上一张有效图继续显示；过期的命中范围不会提交到当前原文。
                        result[index].prepared = result[index].prepared?.withoutSourceInteraction()
                        result[index].reason = Self.reason(error)
                        result[index].diagnostic = String(describing: error)
                    }
                }
                previousBlocks = previousBlocks.filter { key, _ in result.contains { $0.ordinal == key } }
                return result
            }
        } catch {
            for index in result.indices {
                result[index].reason = Self.reason(error)
                result[index].diagnostic = String(describing: error)
            }
        }
        return result
    }

    /// UTF-8 与 UTF-16 都必须落在原文字符边界，越界或截断时不构造命中范围。
    static func sourceRange(start: UInt64, end: UInt64, in source: String) -> NSRange? {
        guard start <= end, end <= source.utf8.count,
              let a = source.utf8.index(source.utf8.startIndex, offsetBy: Int(start)).samePosition(in: source),
              let b = source.utf8.index(source.utf8.startIndex, offsetBy: Int(end)).samePosition(in: source) else { return nil }
        return NSRange(a..<b, in: source)
    }
    static func byteRange(_ range: NSRange, in source: String) -> Range<Int>? {
        guard let value = Range(range, in: source),
              let a = value.lowerBound.samePosition(in: source.utf8), let b = value.upperBound.samePosition(in: source.utf8) else { return nil }
        return source.utf8.distance(from: source.utf8.startIndex, to: a)..<source.utf8.distance(from: source.utf8.startIndex, to: b)
    }

    static func reason(_ error: Error) -> String {
        if let error = error as? DiagramSceneFailure {
            return switch error {
            case let .schema(version): "Unsupported diagram scene schema: \(version)"
            case .sourceRange: "Invalid diagram source range"
            case .staleRequest: "Stale diagram request"
            }
        }
        guard let error = error as? DiagramFailure else {
            return L10n.string("Diagram could not be laid out; showing source")
        }
        return switch error {
        case .NotATree(multiParent: _):
            L10n.string("Not a tree; showing source")
        case let .Unsupported(diagramType: diagramType):
            L10n.string("Unsupported %@ diagram; showing source", diagramType)
        case let .Syntax(line: line, kind: _):
            L10n.string("Syntax error on line %d; showing source", line)
        case .Engine(kind: _):
            L10n.string("Diagram could not be laid out; showing source")
        }
    }
}
