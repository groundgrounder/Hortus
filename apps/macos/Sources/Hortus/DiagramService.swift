import Foundation
import CoreText
import HortusCore

/// 按 Unicode scalar 边界一次建立映射；同一 revision 的所有节点和边共用。
struct DiagramSourceOffsets {
    let utf16: [Int]
    init(_ source: String) {
        var values = [Int](repeating: -1, count: source.utf8.count + 1)
        var byte = 0, unit = 0
        values[0] = 0
        for scalar in source.unicodeScalars {
            let value = scalar.value
            byte += value < 0x80 ? 1 : (value < 0x800 ? 2 : (value < 0x10000 ? 3 : 4))
            unit += value < 0x10000 ? 1 : 2
            values[byte] = unit
        }
        utf16 = values
    }
    func range(start: UInt64, end: UInt64) -> NSRange? {
        guard start <= end, end < utf16.count else { return nil }
        let a = utf16[Int(start)], b = utf16[Int(end)]
        guard a >= 0, b >= a else { return nil }
        return NSRange(location: a, length: b - a)
    }
}

struct NoteDiagram: Sendable {
    let ordinal: UInt32
    var sourceStart: UInt64 = 0
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
        guard documentIdentity == old.documentIdentity, sourceStart == old.sourceStart else { return }
        collapsed = old.collapsed
        prepared = old.prepared
        previous = old.previous
        sceneVersion = old.sceneVersion
        viewport = old.viewport
    }
    var reason: String?
    var diagnostic: String?
    var heavyWorkThreadChecks: [String: Bool] = [:]
    var renderMilliseconds = 0.0
    var transferMilliseconds = 0.0
    var layoutMilliseconds = 0.0
}
/// 应用存活期间复用单个引擎。Task 创建即在后台预热，不占用主线程。
actor DiagramService {
    private var lineCache: [DiagramLineKey: CTLine] = [:]
    private var cachedSource: String?
    private var sourceOffsets: DiagramSourceOffsets?
    private var activeDocument = ""
    private var previousBlocks: [UInt32: NoteDiagram] = [:]
    private let engine = Task.detached(priority: .userInitiated) { try DiagramEngine() }

    func blocks(snapshot: DocumentSnapshot, vault: String, note: String, core: WorkspaceCore?, documentIdentity: String? = nil) throws -> [NoteDiagram] {
        let text = snapshot.text as NSString
        return try snapshot.blocks.map { block in
            let collapsed = try core?.collapsedNodes(vault: vault, note: note, block: block.ordinal) ?? []
            let range = NSRange(location: Int(block.start), length: Int(block.length))
            let tail = text.length > 0 ? text.paragraphRange(for: NSRange(location: max(0, NSMaxRange(range) - 1), length: 0)).location : 0
            return NoteDiagram(ordinal: block.ordinal, sourceStart: block.start, range: range, collapsed: Set(collapsed), tailStart: tail,
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
            return NoteDiagram(ordinal: block.ordinal, sourceStart: block.fenceStart, range: range, collapsed: Set(collapsed), tailStart: tail,
                               documentIdentity: documentIdentity ?? (vault + "\u{0}" + note))
        }
    }

    /// 在实际编辑区间上搬移块身份；前插失败块不能占用原 ordinal 的旧图。
    private func remapPrevious(from oldSource: String, to source: String, mmd: Bool, blocks: [NoteDiagram]) {
        guard !mmd else { return }
        let old = Array(oldSource.utf8), new = Array(source.utf8)
        var prefix = 0
        while prefix < min(old.count, new.count), old[prefix] == new[prefix] { prefix += 1 }
        var suffix = 0
        while suffix < min(old.count, new.count) - prefix,
              old[old.count - 1 - suffix] == new[new.count - 1 - suffix] { suffix += 1 }
        let end = old.count - suffix, delta = new.count - old.count
        let oldBlocks = diagramBlocks(note: oldSource), newBlocks = diagramBlocks(note: source)
        var moved: [UInt32: NoteDiagram] = [:]
        var mapped: Set<UInt32> = []
        func content(_ bytes: [UInt8], _ block: DiagramBlock) -> ArraySlice<UInt8> {
            bytes[Int(block.fenceStart)..<Int(block.fenceEnd)]
        }
        func move(_ block: DiagramBlock, _ next: DiagramBlock) {
            guard var previous = previousBlocks[block.ordinal], moved[next.ordinal] == nil,
                  let target = blocks.first(where: { $0.ordinal == next.ordinal }) else { return }
            // snapshot 和独立块入口分别使用 UTF-16/UTF-8；不能用字节 delta 修改 sourceStart。
            previous.sourceStart = target.sourceStart
            moved[next.ordinal] = previous
            mapped.insert(block.ordinal)
        }
        // 先找完整且唯一的未变块，防止共同围栏头把前插误判成块内修改。
        for block in oldBlocks {
            let candidates = newBlocks.filter { content(old, block) == content(new, $0) }
            if candidates.count == 1 { move(block, candidates[0]) }
        }
        for block in oldBlocks where !mapped.contains(block.ordinal) {
            let start = Int(block.fenceStart), finish = Int(block.fenceEnd)
            let nextStart: Int
            if end <= start { nextStart = start + delta }
            else if finish <= prefix { nextStart = start }
            else {
                // 仅正文内编辑保留该块的旧图；围栏被替换时丢弃身份。
                guard prefix >= Int(block.sourceStart) else { continue }
                nextStart = start
            }
            guard nextStart >= 0,
                  let next = newBlocks.first(where: { $0.fenceStart == UInt64(nextStart) }),
                  (prefix >= Int(block.sourceStart) || Int(next.fenceEnd) == finish + (finish > prefix ? delta : 0)) else { continue }
            move(block, next)
        }
        previousBlocks = moved
    }

    func render(source: String, mmd: Bool, blocks: [NoteDiagram], width: Double, dark: Bool, requestGeneration: UInt64 = 0, fontGeneration: UInt64 = 0) async -> [NoteDiagram] {
        // 空正文无需排版；保留最后一篇图文档的有界行缓存，重新打开时复用 CoreText 对象。
        guard !blocks.isEmpty else { return blocks }
        var result = blocks
        do {
            let engine = try await engine.value
            return autoreleasepool {
                if let oldSource = cachedSource, oldSource != source {
                    remapPrevious(from: oldSource, to: source, mmd: mmd, blocks: blocks)
                }
                if cachedSource != source {
                    lineCache = [:]; cachedSource = source
                    sourceOffsets = DiagramSourceOffsets(source)
                }
                guard let sourceOffsets else { return blocks }
                let original = source as NSString
                let document = result[0].documentIdentity
                if activeDocument != document { previousBlocks = [:]; activeDocument = document }
                for index in result.indices {
                    result[index].prepared = nil; result[index].previous = nil; result[index].sceneVersion = 0
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
                        result[index].heavyWorkThreadChecks["rust_layout_and_ffi"] = Thread.isMainThread
                        let scene = try engine.renderRequest(source: source, mmd: mmd, width: width,
                            collapsed: Array(result[index].collapsed), identity: identity, previous: result[index].previous,
                            viewport: result[index].viewport)
                        guard !Task.isCancelled else { return blocks }
                        guard scene.identity == identity, abs(scene.width - width) < 0.001 else { throw DiagramSceneFailure.staleRequest }
                        var ranges: [String: NSRange] = [:]
                        for hit in scene.hitRegions {
                            guard let range = sourceOffsets.range(start: hit.sourceStart, end: hit.sourceEnd) else { throw DiagramSceneFailure.sourceRange }
                            ranges[hit.nodeId] = range
                        }
                        var edgeRanges: [UInt64: NSRange] = [:]
                        var edgeLabels: [UInt64: String] = [:]
                        for primitive in scene.primitives {
                            if case let .graphPath(id, _, _, start, end, _, _, _, _, _, _, _) = primitive {
                                guard let range = sourceOffsets.range(start: start, end: end) else { throw DiagramSceneFailure.sourceRange }
                                edgeRanges[id] = range
                                edgeLabels[id] = original.substring(with: range)
                            }
                        }
                        let transferred = ProcessInfo.processInfo.systemUptime
                        result[index].heavyWorkThreadChecks["scene_decode_and_coretext_prepare"] = Thread.isMainThread
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
                        result[index].reason = (result[index].prepared == nil ? "" : L10n.string("Diagram is stale; source error") + " — ") + Self.reason(error)
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
        case let .UnsupportedConstruct(line: line, start: _, end: _, kind: kind):
            L10n.string("Unsupported %@ on line %d; showing source", kind, line)
        case let .Syntax(line: line, kind: _):
            L10n.string("Syntax error on line %d; showing source", line)
        case .Engine(kind: _):
            L10n.string("Diagram could not be laid out; showing source")
        }
    }
}
