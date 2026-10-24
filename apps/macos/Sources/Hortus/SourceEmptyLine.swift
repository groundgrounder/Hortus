import AppKit

/// 保留原生字符映射，仅固定 TextKit 合成的文末空行度量。
final class SourceEmptyLine: NSTextLineFragment {
    let source: NSTextLineFragment
    let height: CGFloat
    init(source: NSTextLineFragment, height: CGFloat) {
        self.source = source; self.height = height
        super.init(attributedString: source.attributedString, range: source.characterRange)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var typographicBounds: CGRect {
        let rect = source.typographicBounds
        return CGRect(x: rect.minX, y: rect.maxY - height, width: rect.width, height: height)
    }
    override var glyphOrigin: CGPoint { source.glyphOrigin }
    override func locationForCharacter(at index: Int) -> CGPoint { source.locationForCharacter(at: index) }
    override func characterIndex(for point: CGPoint) -> Int { source.characterIndex(for: point) }
    override func fractionOfDistanceThroughGlyph(for point: CGPoint) -> CGFloat { source.fractionOfDistanceThroughGlyph(for: point) }
}
