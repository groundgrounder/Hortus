import CoreGraphics
import Foundation
import HortusCore
import ImageIO
#if canImport(UIKit)
import UIKit
#endif

public struct PreparedMedia: Sendable {
    public let block: MediaBlock
    public let imageSize: CGSize
    public let reason: String
    public let version: String
    public var math: [[PreparedMathBlock]] = []

    public init(block: MediaBlock, imageSize: CGSize, reason: String, version: String, math: [[PreparedMathBlock]] = []) {
        self.block = block
        self.imageSize = imageSize
        self.reason = reason
        self.version = version
        self.math = math
    }
}

/// 只在后台读文件、探测尺寸和解码。相同文件版本共享像素，不缓存到笔记目录。
public actor LocalImageCache {
    public static let shared = LocalImageCache()
    private struct Entry {
        let stamp: String
        let size: CGSize
        var image: CGImage?
        let reason: String
    }
    private var entries: [String: Entry] = [:]
    private var order: [String] = []
    private var pixelBytes = 0
    public private(set) var decodes = 0
    public private(set) var mainThreadDecodes = 0
    public var cachedPixelBytes: Int { pixelBytes }
    #if canImport(UIKit)
    private final class MemoryWarningToken: @unchecked Sendable {
        private let token: any NSObjectProtocol
        init(handler: @escaping @Sendable () -> Void) {
            token = NotificationCenter.default.addObserver(
                forName: UIApplication.didReceiveMemoryWarningNotification,
                object: nil,
                queue: nil
            ) { _ in handler() }
        }
        deinit {
            NotificationCenter.default.removeObserver(token)
        }
    }
    private var memoryWarningToken: MemoryWarningToken?
    #endif

    public init() {
        #if canImport(UIKit)
        Task { await installMemoryWarningObserver() }
        #endif
    }

    #if canImport(UIKit)
    private func installMemoryWarningObserver() {
        guard memoryWarningToken == nil else { return }
        memoryWarningToken = MemoryWarningToken { [weak self] in
            Task { await self?.evictPixels() }
        }
    }
    #endif

    public func evictPixels() {
        for key in entries.keys {
            entries[key]?.image = nil
        }
        pixelBytes = 0
    }

    public func clear() {
        entries.removeAll(keepingCapacity: false)
        order.removeAll(keepingCapacity: false)
        pixelBytes = 0
    }

    private func checkedURL(_ path: String, vault: String) -> URL? {
        let rootURL = URL(fileURLWithPath: vault).resolvingSymlinksInPath().standardizedFileURL
        let root = rootURL.path
        let prefix = root == "/" ? root : root + "/"
        func inside(_ candidate: URL) -> URL? {
            let resolved = candidate.resolvingSymlinksInPath().standardizedFileURL
            return resolved.path.hasPrefix(prefix) ? resolved : nil
        }
        func insideOrRoot(_ candidate: URL) -> URL? {
            let resolved = candidate.resolvingSymlinksInPath().standardizedFileURL
            return (resolved.path == root || resolved.path.hasPrefix(prefix)) ? resolved : nil
        }
        let direct = URL(fileURLWithPath: path)
        if let resolved = inside(direct), FileManager.default.fileExists(atPath: resolved.path) {
            return resolved
        }
        for variant in [path.precomposedStringWithCanonicalMapping, path.decomposedStringWithCanonicalMapping]
            where !variant.utf8.elementsEqual(path.utf8) {
            if let resolved = inside(URL(fileURLWithPath: variant)), FileManager.default.fileExists(atPath: resolved.path) {
                return resolved
            }
        }
        // 同目录内按 NFC 规范化文件名回退，兼容 APFS/HFS+ 与跨平台同步导致的 NFC/NFD 差异。
        let targetName = direct.lastPathComponent.precomposedStringWithCanonicalMapping
        let parentPath = direct.deletingLastPathComponent().path
        var parentVariants = [parentPath]
        for candidate in [parentPath.precomposedStringWithCanonicalMapping, parentPath.decomposedStringWithCanonicalMapping]
            where !parentVariants.contains(where: { $0.utf8.elementsEqual(candidate.utf8) }) {
            parentVariants.append(candidate)
        }
        for parentVariant in parentVariants {
            guard let parentURL = insideOrRoot(URL(fileURLWithPath: parentVariant)),
                  let items = try? FileManager.default.contentsOfDirectory(atPath: parentURL.path) else { continue }
            if let match = items.first(where: { $0.precomposedStringWithCanonicalMapping == targetName }),
               let resolved = inside(parentURL.appendingPathComponent(match)),
               FileManager.default.fileExists(atPath: resolved.path) {
                return resolved
            }
        }
        // 多级混合 NFC/NFD 目录与文件名：在已校验的库根内逐级按规范等价匹配组件，每级严防符号链接逃逸。
        let relativeComponents: [String]? = {
            let resolvedCandidate = direct.resolvingSymlinksInPath().standardizedFileURL.path
            if resolvedCandidate.hasPrefix(prefix) {
                return String(resolvedCandidate.dropFirst(prefix.count)).split(separator: "/").map(String.init)
            }
            let rawRoot = URL(fileURLWithPath: vault).standardizedFileURL.path
            let rawPrefix = rawRoot == "/" ? rawRoot : rawRoot + "/"
            let stdCandidate = direct.standardizedFileURL.path
            if stdCandidate.hasPrefix(rawPrefix) {
                return String(stdCandidate.dropFirst(rawPrefix.count)).split(separator: "/").map(String.init)
            }
            return nil
        }()
        if let components = relativeComponents, !components.isEmpty {
            var current = rootURL
            var matchedAll = true
            for component in components {
                if component == "." || component == ".." {
                    matchedAll = false
                    break
                }
                let directChild = current.appendingPathComponent(component)
                if FileManager.default.fileExists(atPath: directChild.path),
                   let safeChild = inside(directChild) {
                    current = safeChild
                    continue
                }
                guard let items = try? FileManager.default.contentsOfDirectory(atPath: current.path) else {
                    matchedAll = false
                    break
                }
                let targetNFC = component.precomposedStringWithCanonicalMapping
                if let match = items.first(where: { $0.precomposedStringWithCanonicalMapping == targetNFC }),
                   let safeChild = inside(current.appendingPathComponent(match)) {
                    current = safeChild
                } else {
                    matchedAll = false
                    break
                }
            }
            if matchedAll, let resolved = inside(current), FileManager.default.fileExists(atPath: resolved.path) {
                return resolved
            }
        }
        return inside(direct)
    }

    public func invalidateFailed() {
        for (key, entry) in entries where !entry.reason.isEmpty {
            entries.removeValue(forKey: key)
            order.removeAll { $0 == key }
        }
    }
    private func stamp(_ url: URL) -> String? {
        guard let values = try? url.resourceValues(forKeys: [.contentModificationDateKey, .fileSizeKey, .fileResourceIdentifierKey]),
              let date = values.contentModificationDate, let size = values.fileSize else { return nil }
        return "\(date.timeIntervalSince1970):\(size):\(String(describing: values.fileResourceIdentifier))"
    }
    private func inspect(_ path: String, vault: String) -> Entry {
        guard let url = checkedURL(path, vault: vault) else { return Entry(stamp: "", size: .zero, reason: "image.outside") }
        guard let stamp = stamp(url) else { return Entry(stamp: "", size: .zero, reason: "image.missing") }
        if let cached = entries[path], cached.stamp == stamp { return cached }
        if let old = entries.removeValue(forKey: path)?.image { pixelBytes -= old.bytesPerRow * old.height }
        let entry: Entry
        if let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
           let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
           let width = properties[kCGImagePropertyPixelWidth] as? Double,
           let height = properties[kCGImagePropertyPixelHeight] as? Double, width > 0, height > 0 {
            let orientation = properties[kCGImagePropertyOrientation] as? Int ?? 1
            entry = Entry(stamp: stamp, size: orientation >= 5 ? CGSize(width: height, height: width) : CGSize(width: width, height: height), reason: "")
        } else if url.pathExtension.lowercased() == "svg", let size = PlatformGraphics.svgImageSize(url: url) {
            entry = Entry(stamp: stamp, size: size, reason: "")
        } else {
            entry = Entry(stamp: stamp, size: .zero, reason: url.pathExtension.lowercased() == "svg" ? "image.svg_unsupported" : "image.decode_failed")
        }
        entries[path] = entry
        order.removeAll { $0 == path }; order.append(path)
        return entry
    }
    public func prepare(_ blocks: [MediaBlock], vault: String, size: Double = 15, mathService: MathService? = nil,
                        previousMath: [PreparedMathBlock] = []) async -> [PreparedMedia] {
        var result: [PreparedMedia] = []
        for block in blocks {
            guard block.columns.isEmpty, block.imageReason.isEmpty else {
                var item = PreparedMedia(block: block, imageSize: .zero, reason: block.imageReason, version: "")
                if let mathService {
                    for (index, cell) in block.cells.enumerated() {
                        item.math.append(await mathService.prepare(cell.math, source: cell.source, size: size,
                            width: block.columns[index % block.columns.count] - 20, displayText: cell.text,
                            previous: previousMath, cell: true))
                    }
                }
                result.append(item)
                continue
            }
            let entry = inspect(block.imagePath, vault: vault)
            result.append(PreparedMedia(block: block, imageSize: entry.size, reason: entry.reason, version: entry.stamp))
        }
        return result
    }
    public func version(_ path: String, vault: String) -> String {
        checkedURL(path, vault: vault).flatMap { stamp($0) } ?? ""
    }
    public func decode(_ path: String, vault: String) -> CGImage? {
        var entry = inspect(path, vault: vault)
        if let image = entry.image { return image }
        guard entry.reason.isEmpty, let url = checkedURL(path, vault: vault) else { return nil }
        if Thread.isMainThread { mainThreadDecodes += 1 }
        var image: CGImage?
        if let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary) {
            image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: 4096,
            ] as CFDictionary)
        }
        if image == nil, url.pathExtension.lowercased() == "svg" {
            image = PlatformGraphics.svgCGImage(url: url)
        }
        guard let image else { return nil }
        decodes += 1
        entry.image = image; entries[path] = entry
        pixelBytes += image.bytesPerRow * image.height
        while pixelBytes > 128 * 1024 * 1024, order.count > 1 {
            let oldest = order.removeFirst()
            if oldest == path { order.append(oldest); continue }
            if let removed = entries.removeValue(forKey: oldest)?.image { pixelBytes -= removed.bytesPerRow * removed.height }
        }
        return image
    }
}
