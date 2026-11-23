import AppKit
import CoreGraphics
import Foundation
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct DiagramExportAndLinksTests {
    private let source = """
        flowchart TB
          A["[[design#Section Two|Design Spec]]"] --> B["[[architecture]]"]
          C["![[architecture]]"]
        """

    private func prepared(_ source: String, dark: Bool = false) throws -> PreparedScene {
        let engine = try DiagramEngine()
        let scene = try engine.render(source: source, mmd: true, block: 1, width: 700,
                                      dark: dark, collapsed: [])
        return PreparedScene(scene: try DiagramScene(scene))
    }

    @Test func linkedLabelsStripMarkersAndKeepTargets() throws {
        let prepared = try prepared(source)
        let linked = try #require(prepared.scene.hitRegions.first { $0.linkTarget == "design" })
        #expect(linked.label == "Design Spec")
        #expect(linked.linkHeading == "Section Two")
        let bytes = Array(source.utf8)
        let fragment = String(decoding: bytes[Int(linked.sourceStart)..<Int(linked.sourceEnd)], as: UTF8.self)
        #expect(fragment.contains("[[design#Section Two|Design Spec]]"))

        let plain = try #require(prepared.scene.hitRegions.first { $0.label.contains("![[architecture]]") })
        #expect(plain.linkTarget == nil)
        #expect(plain.linkHeading == nil)
    }

    @Test func engineSVGIsSelfContainedVectorText() throws {
        let engine = try DiagramEngine()
        let svg = try engine.renderSvg(source: source, mmd: true, block: 1, width: 700,
                                       dark: true, collapsed: [])
        #expect(svg.hasPrefix("<svg xmlns=\"http://www.w3.org/2000/svg\""))
        #expect(svg.contains("viewBox=\"0 0 700.000"))
        #expect(svg.contains("Design Spec"))
        #expect(svg.contains("architecture"))
        #expect(!svg.contains("<script"))
        #expect(!svg.contains("href="))
    }

    @Test func svgExportEngineInitializesOnItsActorAtFirstRender() async throws {
        let engine = DiagramSVGEngine()
        #expect(await engine.inspectionInitializedOnMain == nil)
        let svg = try await engine.render(source: source, mmd: true, block: 1, width: 700,
                                          dark: false, collapsed: [])
        #expect(svg.hasPrefix("<svg"))
        #expect(await engine.inspectionInitializedOnMain == false)
    }

    @Test func pngUsesTwoTimesRetinaPixels() throws {
        let prepared = try prepared(source)
        let png = try #require(DiagramExport.renderPNG(prepared: prepared))
        #expect(Array(png.prefix(8)) == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
        #expect(Int(png.uint32BE(16)) == Int((prepared.scene.width * 2).rounded(.up)))
        #expect(Int(png.uint32BE(20)) == Int((prepared.scene.height * 2).rounded(.up)))

        let rep = try #require(NSBitmapImageRep(data: png))
        var opaque = 0
        var colors = Set<UInt32>()
        let stepX = max(1, rep.pixelsWide / 32)
        let stepY = max(1, rep.pixelsHigh / 32)
        for y in stride(from: 0, to: rep.pixelsHigh, by: stepY) {
            for x in stride(from: 0, to: rep.pixelsWide, by: stepX) {
                guard let color = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                if color.alphaComponent > 0.01 { opaque += 1 }
                let r = UInt32((color.redComponent * 255).rounded())
                let g = UInt32((color.greenComponent * 255).rounded())
                let b = UInt32((color.blueComponent * 255).rounded())
                colors.insert(r << 16 | g << 8 | b)
            }
        }
        #expect(opaque > 0)
        #expect(colors.count > 1)
    }

    @Test func pdfIsOnePageAndKeepsVectorOperators() throws {
        let prepared = try prepared(source)
        let pdf = try #require(DiagramExport.renderPDF(prepared: prepared))
        #expect(String(decoding: pdf.prefix(5), as: UTF8.self) == "%PDF-")
        let document = try #require(CGDataProvider(data: pdf as CFData).flatMap { CGPDFDocument($0) })
        #expect(document.numberOfPages == 1)
        let box = try #require(document.page(at: 1)?.getBoxRect(.mediaBox))
        #expect(abs(box.width - prepared.scene.width) < 0.01)
        #expect(abs(box.height - prepared.scene.height) < 0.01)
        let text = String(data: pdf, encoding: .isoLatin1) ?? ""
        #expect(!text.contains("/Subtype /Image"))
        #expect(!text.contains("/Subtype/Image"))
    }

    @Test func exportStringsUseTheSharedLocalizationTable() {
        let strings = [
            ("Export Diagram", DiagramExportStrings.exportDiagram),
            ("Export", DiagramExportStrings.export),
            ("Open Linked Note", DiagramExportStrings.openLinkedNote),
            ("Export Failed", DiagramExportStrings.exportFailed),
            ("The diagram could not be rendered or written.", DiagramExportStrings.exportFailedMessage),
        ]
        for (key, value) in strings {
            #expect(!value.isEmpty)
            #expect(value == L10n.string(key))
        }
    }

    @Test func noPanelExportWritesRequestedFormat() async throws {
        let prepared = try prepared(source)
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("hortus-diagram-export-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><text>Design Spec</text></svg>"
        let request = DiagramExportRequest(prepared: prepared, note: "Learning-map.md", ordinal: 1) {
            svg
        }
        let url = directory.appendingPathComponent("diagram.svg")
        try await DiagramExport.export(.svg, request: request, to: url)
        #expect(try String(contentsOf: url, encoding: .utf8) == svg)
        #expect(DiagramExport.defaultName(note: "Learning-map.md", ordinal: 1, format: .svg)
            == "Learning-map-diagram-1.svg")
        #expect(DiagramExport.defaultName(note: "notes/Learning-map.md", ordinal: 2, format: .pdf)
            == "Learning-map-diagram-2.pdf")
    }
}

private extension Data {
    func uint32BE(_ offset: Int) -> UInt32 {
        self[offset..<offset + 4].reduce(0) { ($0 << 8) | UInt32($1) }
    }
}
