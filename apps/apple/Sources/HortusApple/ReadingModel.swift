import Foundation
import HortusCore

public struct ReadingRun: Sendable {
    public let start, length, sourceStart, sourceLength: UInt64
    public let mask: UInt32
    public init(start: UInt64, length: UInt64, sourceStart: UInt64, sourceLength: UInt64, mask: UInt32) {
        self.start = start
        self.length = length
        self.sourceStart = sourceStart
        self.sourceLength = sourceLength
        self.mask = mask
    }
}

public struct ReadingBlock: Sendable {
    public let id, sourceStart, sourceLength, displayStart, displayLength: UInt64
    public let runRange: Range<Int>
    public let diagram: Bool
    public init(id: UInt64, sourceStart: UInt64, sourceLength: UInt64, displayStart: UInt64,
                displayLength: UInt64, runRange: Range<Int>, diagram: Bool) {
        self.id = id
        self.sourceStart = sourceStart
        self.sourceLength = sourceLength
        self.displayStart = displayStart
        self.displayLength = displayLength
        self.runRange = runRange
        self.diagram = diagram
    }
}

public struct ReadingViewModel: Sendable {
    public let revision, rebuilt: UInt64
    public let text: String
    public let blocks: [ReadingBlock]
    private let packedRuns: Data
    private let runRecordSize: Int

    private init(revision: UInt64, rebuilt: UInt64, text: String, blocks: [ReadingBlock],
                 packedRuns: Data, runRecordSize: Int) {
        self.revision = revision; self.rebuilt = rebuilt; self.text = text; self.blocks = blocks
        self.packedRuns = packedRuns; self.runRecordSize = runRecordSize
    }

    public init(_ wire: HortusCore.ReadingModel) throws {
        func layout(_ data: Data, legacySize: Int, compactSize: Int) throws -> (Int, Int, Int) {
            guard data.count >= 8 else { throw CocoaError(.fileReadCorruptFile) }
            let (version, count) = data.withUnsafeBytes { raw in
                (UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self)),
                 UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 4, as: UInt32.self)))
            }
            let recordSize: Int
            switch version {
            case 1: recordSize = legacySize
            case 2: recordSize = compactSize
            default: throw CocoaError(.fileReadCorruptFile)
            }
            let (size, overflow) = Int(count).multipliedReportingOverflow(by: recordSize)
            guard !overflow, data.count == 8 + size else { throw CocoaError(.fileReadCorruptFile) }
            return (Int(version), Int(count), recordSize)
        }
        let (blockVersion, blockCount, _) = try layout(wire.packedBlocks, legacySize: 64, compactSize: 36)
        let (runVersion, runCount, runRecordSize) = try layout(wire.packedRuns, legacySize: 40, compactSize: 20)
        guard blockVersion == runVersion else { throw CocoaError(.fileReadCorruptFile) }
        let blocks = wire.packedBlocks.withUnsafeBytes { raw in
            (0..<blockCount).map { index -> ReadingBlock in
                let base = 8 + index * (blockVersion == 2 ? 36 : 64)
                func value64(_ offset: Int) -> UInt64 {
                    UInt64(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt64.self))
                }
                func value32(_ offset: Int) -> UInt64 {
                    UInt64(UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt32.self)))
                }
                if blockVersion == 2 {
                    let first = Int(value32(24)), length = Int(value32(28))
                    return ReadingBlock(id: value64(0), sourceStart: value32(8), sourceLength: value32(12),
                        displayStart: value32(16), displayLength: value32(20), runRange: first..<(first + length),
                        diagram: value32(32) != 0)
                }
                let first = Int(value64(40)), length = Int(value64(48))
                return ReadingBlock(id: value64(0), sourceStart: value64(8), sourceLength: value64(16),
                    displayStart: value64(24), displayLength: value64(32), runRange: first..<(first + length),
                    diagram: value64(56) != 0)
            }
        }
        guard blocks.allSatisfy({ $0.runRange.upperBound <= runCount }) else { throw CocoaError(.fileReadCorruptFile) }
        self.init(revision: wire.revision, rebuilt: wire.rebuilt, text: wire.text, blocks: blocks,
                  packedRuns: wire.packedRuns, runRecordSize: runRecordSize)
    }

    public func applying(_ wire: HortusCore.ReadingModel) throws -> ReadingViewModel {
        if wire.deltaIndex == UInt64.max - 1 { return self }
        guard wire.deltaIndex < UInt64(blocks.count), !wire.packedRuns.isEmpty else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let index = Int(wire.deltaIndex)
        guard let layout = Self.runLayout(wire.packedRuns), layout.version == (runRecordSize == 20 ? 2 : 1),
              wire.packedRuns.count == 8 + layout.count * layout.recordSize else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let oldBlock = blocks[index]
        let newRunCount = layout.count
        let oldRunCount = oldBlock.runRange.count
        let runDelta = newRunCount - oldRunCount
        let sourceDelta = Int64(wire.sourceLength) - Int64(oldBlock.sourceLength)
        let displayDelta = Int64(wire.displayLength) - Int64(oldBlock.displayLength)
        let replaced = (text as NSString).replacingCharacters(
            in: NSRange(location: Int(oldBlock.displayStart), length: Int(oldBlock.displayLength)),
            with: wire.text)
        var nextBlocks = blocks
        for blockIndex in nextBlocks.indices {
            if blockIndex == index {
                nextBlocks[blockIndex] = ReadingBlock(id: wire.blockId,
                    sourceStart: wire.sourceStart, sourceLength: wire.sourceLength,
                    displayStart: wire.displayStart, displayLength: wire.displayLength,
                    runRange: oldBlock.runRange.lowerBound..<(oldBlock.runRange.lowerBound + newRunCount),
                    diagram: wire.diagram)
            } else if blockIndex > index {
                let block = nextBlocks[blockIndex]
                nextBlocks[blockIndex] = ReadingBlock(id: block.id,
                    sourceStart: UInt64(Int64(block.sourceStart) + sourceDelta),
                    sourceLength: block.sourceLength,
                    displayStart: UInt64(Int64(block.displayStart) + displayDelta),
                    displayLength: block.displayLength,
                    runRange: (block.runRange.lowerBound + runDelta)..<(block.runRange.upperBound + runDelta),
                    diagram: block.diagram)
            }
        }
        var packed = Data()
        let oldBodyStart = 8 + oldBlock.runRange.lowerBound * runRecordSize
        let oldBodyEnd = 8 + oldBlock.runRange.upperBound * runRecordSize
        packed.append(packedRuns.prefix(oldBodyStart))
        packed.append(wire.packedRuns.dropFirst(8))
        packed.append(packedRuns.suffix(from: oldBodyEnd))
        var count = UInt32(blocks.reduce(0) { $0 + $1.runRange.count } + runDelta)
        withUnsafeBytes(of: &count) { packed.replaceSubrange(4..<8, with: $0) }
        return ReadingViewModel(revision: wire.revision, rebuilt: wire.rebuilt, text: replaced,
            blocks: nextBlocks, packedRuns: packed, runRecordSize: runRecordSize)
    }

    private static func runLayout(_ data: Data) -> (version: Int, count: Int, recordSize: Int)? {
        guard data.count >= 8 else { return nil }
        let (version, count) = data.withUnsafeBytes { raw in
            (UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self)),
             UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 4, as: UInt32.self)))
        }
        let recordSize: Int
        switch version {
        case 1: recordSize = 40
        case 2: recordSize = 20
        default: return nil
        }
        return (Int(version), Int(count), recordSize)
    }

    public func run(at index: Int) -> ReadingRun {
        let base = 8 + index * runRecordSize
        return packedRuns.withUnsafeBytes { raw in
            if runRecordSize == 20 {
                func value(_ offset: Int) -> UInt64 {
                    UInt64(UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt32.self)))
                }
                return ReadingRun(start: value(0), length: value(4), sourceStart: value(8),
                    sourceLength: value(12), mask: UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + 16, as: UInt32.self)))
            } else {
                func value(_ field: Int) -> UInt64 {
                    UInt64(littleEndian: raw.loadUnaligned(fromByteOffset: base + field * 8, as: UInt64.self))
                }
                return ReadingRun(start: value(0), length: value(1), sourceStart: value(2),
                    sourceLength: value(3), mask: UInt32(truncatingIfNeeded: value(4)))
            }
        }
    }

    public func firstRun(in range: Range<Int>, matching predicate: (ReadingRun) -> Bool) -> ReadingRun? {
        for index in range {
            let run = run(at: index)
            if predicate(run) { return run }
        }
        return nil
    }
}
