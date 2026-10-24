import Foundation
import CoreText
import HortusCore

struct NoteDiagram: Sendable {
    let ordinal: UInt32
    var range: NSRange
    var collapsed: Set<String>
    var tailStart = 0
    var prepared: PreparedScene?
    var reason: String?
    var renderMilliseconds = 0.0
    var transferMilliseconds = 0.0
    var layoutMilliseconds = 0.0
}

/// 应用存活期间复用单个引擎。Task 创建即在后台预热，不占用主线程。
actor DiagramService {
    private var lineCache: [DiagramLineKey: CTLine] = [:]
    private var cachedSource: String?
    private let engine = Task.detached(priority: .userInitiated) { try DiagramEngine() }

    func blocks(snapshot: DocumentSnapshot, vault: String, note: String, core: WorkspaceCore?) throws -> [NoteDiagram] {
        let text = snapshot.text as NSString
        return try snapshot.blocks.map { block in
            let collapsed = try core?.collapsedNodes(vault: vault, note: note, block: block.ordinal) ?? []
            let range = NSRange(location: Int(block.start), length: Int(block.length))
            let tail = text.length > 0 ? text.paragraphRange(for: NSRange(location: max(0, NSMaxRange(range) - 1), length: 0)).location : 0
            return NoteDiagram(ordinal: block.ordinal, range: range, collapsed: Set(collapsed), tailStart: tail)
        }
    }

    func blocks(source: String, mmd: Bool, vault: String, note: String, core: WorkspaceCore?) throws -> [NoteDiagram] {
        let blocks = mmd ? [DiagramBlock(ordinal: 1, fenceStart: 0, fenceEnd: UInt64(source.utf8.count),
                                         sourceStart: 0, sourceEnd: UInt64(source.utf8.count), diagramType: "")]
            : diagramBlocks(note: source)
        return try blocks.map { block in
            let start = String.Index(utf8Offset: Int(block.fenceStart), in: source)
            let end = String.Index(utf8Offset: Int(block.fenceEnd), in: source)
            let collapsed = try core?.collapsedNodes(vault: vault, note: note, block: block.ordinal) ?? []
            let range = NSRange(start..<end, in: source)
            let text = source as NSString
            let tail = text.length > 0 ? text.paragraphRange(for: NSRange(location: max(0, NSMaxRange(range) - 1), length: 0)).location : 0
            return NoteDiagram(ordinal: block.ordinal, range: range, collapsed: Set(collapsed), tailStart: tail)
        }
    }

    func render(source: String, mmd: Bool, blocks: [NoteDiagram], width: Double, dark: Bool) async -> [NoteDiagram] {
        // 空正文无需排版；保留最后一篇图文档的有界行缓存，重新打开时复用 CoreText 对象。
        guard !blocks.isEmpty else { return blocks }
        var result = blocks
        do {
            let engine = try await engine.value
            return autoreleasepool {
                if cachedSource != source { lineCache = [:]; cachedSource = source }
                for index in result.indices {
                    do {
                        let started = ProcessInfo.processInfo.systemUptime
                        let scene = try engine.render(source: source, mmd: mmd, block: result[index].ordinal,
                                                     width: width, dark: dark, collapsed: Array(result[index].collapsed))
                        let transferred = ProcessInfo.processInfo.systemUptime
                        result[index].prepared = PreparedScene(scene: DiagramScene(scene), payloadBytes: Int(scene.wireBytes)) { span in
                            let key = DiagramLineKey(span)
                            if let line = lineCache[key] { return line }
                            let line = SceneRenderer.line(for: span)
                            lineCache[key] = line
                            return line
                        }
                        result[index].renderMilliseconds = (ProcessInfo.processInfo.systemUptime - started) * 1000
                        result[index].transferMilliseconds = (transferred - started) * 1000
                        result[index].layoutMilliseconds = scene.layoutMs
                        result[index].reason = nil
                    } catch {
                        result[index].prepared = nil
                        result[index].reason = Self.reason(error)
                    }
                }
                return result
            }
        } catch {
            for index in result.indices { result[index].reason = Self.reason(error) }
        }
        return result
    }

    static func reason(_ error: Error) -> String {
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

private extension String.Index {
    init(utf8Offset: Int, in string: String) {
        self = string.utf8.index(string.utf8.startIndex, offsetBy: utf8Offset).samePosition(in: string)!
    }
}
