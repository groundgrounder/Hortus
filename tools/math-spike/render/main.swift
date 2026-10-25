// SPDX-License-Identifier: MIT OR Apache-2.0
import AppKit
import CoreText
import ImageIO
import UniformTypeIdentifiers

typealias Object = [String: Any]
func number(_ object: Object, _ key: String, _ fallback: Double = 0) -> Double {
    (object[key] as? NSNumber)?.doubleValue ?? fallback
}
func load(_ path: String) throws -> Any { try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: path))) }
func writeJSON(_ value: Any, _ path: String) throws {
    try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: path))
}
var faces: [String: String] = [:]
var audit: [Object] = []
func register(_ directory: String) throws {
    for file in try FileManager.default.contentsOfDirectory(atPath: directory).sorted() where file.hasSuffix(".ttf") {
        let url = URL(fileURLWithPath: directory).appendingPathComponent(file)
        var error: Unmanaged<CFError>?
        guard CTFontManagerRegisterFontsForURL(url as CFURL, .process, &error) else {
            throw error!.takeRetainedValue()
        }
        let descriptors = CTFontManagerCreateFontDescriptorsFromURL(url as CFURL) as? [CTFontDescriptor] ?? []
        let descriptor = descriptors.first!
        let name = CTFontDescriptorCopyAttribute(descriptor, kCTFontNameAttribute) as! String
        let key = String(file.dropFirst("KaTeX_".count).dropLast(4))
        faces[key] = name
    }
}
func font(_ family: String, _ size: Double, _ scalar: Unicode.Scalar) -> CTFont {
    if let name = faces[family] { return CTFontCreateWithName(name as CFString, size, nil) }
    let system = CTFontCreateUIFontForLanguage(.system, size, nil)!
    let text = String(scalar)
    return CTFontCreateForString(system, text as CFString, CFRange(location: 0, length: text.utf16.count))
}
func color(_ item: Object, _ dark: Bool) -> CGColor {
    let c = item["color"] as? Object ?? [:]
    var values = [number(c,"r"),number(c,"g"),number(c,"b"),number(c,"a",1)]
    if dark && values[0] == 0 && values[1] == 0 && values[2] == 0 { values = [0.90,0.91,0.93,values[3]] }
    return CGColor(colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!, components: values.map { CGFloat($0) })!
}
func draw(_ row: Object, _ context: CGContext, _ origin: CGPoint, _ em: Double, _ dark: Bool) {
    let display = row["display"] as! Object
    let mapping = row["ttf_char_codes"] as? Object ?? [:]
    context.saveGState()
    context.translateBy(x: origin.x, y: origin.y)
    context.scaleBy(x: em, y: em)
    for item in display["items"] as! [Object] {
        let x = number(item,"x"), y = number(item,"y")
        context.setFillColor(color(item,dark)); context.setStrokeColor(color(item,dark))
        switch item["type"] as! String {
        case "GlyphPath":
            let family = item["font"] as! String
            let raw = (item["char_code"] as! NSNumber).uint32Value
            let cp = (mapping["\(family):\(raw)"] as? NSNumber)?.uint32Value ?? raw
            guard let scalar = Unicode.Scalar(cp) else { continue }
            // Font selection uses registered PostScript names, never guessed family names.
            let face = font(family, number(item,"scale",1), scalar)
            var chars = Array(String(scalar).utf16)
            var glyphs = Array(repeating: CGGlyph(0), count: chars.count)
            let found = CTFontGetGlyphsForCharacters(face, &chars, &glyphs, chars.count)
            let glyph = glyphs.first(where: { $0 != 0 }) ?? 0
            audit.append(["family":family,"codepoint":raw,"ttf_codepoint":cp,"actual_font":CTFontCopyPostScriptName(face) as String,"glyph":glyph,"found":found])
            context.saveGState(); context.translateBy(x:x,y:y); context.scaleBy(x:1,y:-1)
            context.textMatrix = .identity
            context.textPosition = .zero
            var g = glyph; var point = CGPoint.zero
            CTFontDrawGlyphs(face,&g,&point,1,context)
            context.restoreGState()
        case "Line":
            // Match ratex-render's minimum one-device-pixel rule at 3×.
            let thickness = max(number(item,"thickness"),1/(3*em))
            let width = number(item,"width")
            if item["dashed"] as? Bool == true {
                let dash = 4*thickness
                var offset = 0.0
                while offset < width {
                    let length = max(min(dash,width-offset),2/(3*em))
                    context.fill(CGRect(x:x+offset,y:y-thickness/2,width:length,height:thickness))
                    offset += 2*dash
                }
            } else {
                context.fill(CGRect(x:x,y:y-thickness/2,width:width,height:thickness))
            }
        case "Rect":
            context.fill(CGRect(x:x,y:y,width:number(item,"width"),height:number(item,"height")))
        case "Path":
            let path = CGMutablePath()
            for command in item["commands"] as! [Object] {
                let p = CGPoint(x:x+number(command,"x"),y:y+number(command,"y"))
                switch command["type"] as! String {
                case "MoveTo": path.move(to:p)
                case "LineTo": path.addLine(to:p)
                case "CubicTo": path.addCurve(to:p,control1:CGPoint(x:x+number(command,"x1"),y:y+number(command,"y1")),control2:CGPoint(x:x+number(command,"x2"),y:y+number(command,"y2")))
                case "QuadTo": path.addQuadCurve(to:p,control:CGPoint(x:x+number(command,"x1"),y:y+number(command,"y1")))
                case "Close": path.closeSubpath()
                default: fatalError("unknown path command")
                }
            }
            context.addPath(path)
            if item["fill"] as? Bool == true { context.fillPath() }
            else { context.setLineWidth(1.5/em); context.strokePath() }
        default: fatalError("unknown display item")
        }
    }
    context.restoreGState()
}
func canvas(_ width: Double, _ height: Double, _ dark: Bool, _ paint: (CGContext) -> Void, _ output: String) throws {
    let scale = 3.0
    let context = CGContext(data:nil,width:Int(ceil(width*scale)),height:Int(ceil(height*scale)),bitsPerComponent:8,bytesPerRow:0,space:CGColorSpace(name: CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x:scale,y:scale)
    context.translateBy(x:0,y:height); context.scaleBy(x:1,y:-1)
    let background = dark ? 0.09 : 1.0
    context.setFillColor(CGColor(srgbRed:background,green:background,blue:background,alpha:1)); context.fill(CGRect(x:0,y:0,width:width,height:height))
    paint(context)
    let image = context.makeImage()!
    let destination = CGImageDestinationCreateWithURL(URL(fileURLWithPath:output) as CFURL,UTType.png.identifier as CFString,1,nil)!
    CGImageDestinationAddImage(destination,image,nil)
    guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
}
@discardableResult func text(_ string: String, _ context: CGContext, _ x: Double, _ baseline: Double, _ size: Double, _ dark: Bool) -> Double {
    let face = CTFontCreateUIFontForLanguage(.system,size,nil)!
    let value = NSAttributedString(string:string,attributes:[NSAttributedString.Key(kCTFontAttributeName as String):face, .foregroundColor:dark ? NSColor.white : NSColor.black])
    let line = CTLineCreateWithAttributedString(value)
    context.saveGState(); context.translateBy(x:x,y:baseline); context.scaleBy(x:1,y:-1)
    context.textPosition = .zero; CTLineDraw(line,context); context.restoreGState()
    return CTLineGetTypographicBounds(line,nil,nil,nil)
}
let args = CommandLine.arguments
try register(args[2])
switch args[1] {
case "formula":
    let row = try load(args[3]) as! Object, display = row["display"] as! Object
    let em = Double(args[5]) ?? 15, dark = args[6] == "dark"
    try canvas(number(display,"width")*em+32,(number(display,"height")+number(display,"depth"))*em+32,dark,{ c in draw(row,c,CGPoint(x:16,y:16),em,dark) },args[4])
    try writeJSON(audit,args[4]+".json")
case "metrics":
    var metrics: Object = [:]
    for scalar in args[3].unicodeScalars {
        let face = font("CJK-Fallback",15,scalar)
        var chars = Array(String(scalar).utf16), glyphs = Array(repeating:CGGlyph(0),count:String(scalar).utf16.count)
        CTFontGetGlyphsForCharacters(face,&chars,&glyphs,chars.count)
        var glyph = glyphs.first(where: { $0 != 0 }) ?? 0
        var advance = CGSize.zero, box = CGRect.zero
        CTFontGetAdvancesForGlyphs(face,.horizontal,&glyph,&advance,1)
        CTFontGetBoundingRectsForGlyphs(face,.horizontal,&glyph,&box,1)
        metrics[String(scalar)] = ["width":advance.width/15,"height":max(0,box.maxY)/15,"depth":max(0,-box.minY)/15,"font":CTFontCopyPostScriptName(face) as String]
    }
    try writeJSON(metrics,args[4])
case "inline":
    let rows = try load(args[3]) as! [Object]
    var measures: [Object] = []
    try canvas(1050,360,false,{ context in
        for (index, configuration) in [("text",1.0),("text",1.15),("text",1.21),("display",1.21)].enumerated() {
            let (mode,ratio) = configuration, baseline = 65.0 + Double(index)*80
            text("\(mode) × \(ratio)",context,12,baseline-25,11,false)
            var x = text("正文 15 pt，变量 ",context,12,baseline,15,false)+12
            let selected = rows.filter { $0["style"] as? String == mode }
            var ascent = CTFontGetAscent(CTFontCreateUIFontForLanguage(.system,15,nil)!), descent = CTFontGetDescent(CTFontCreateUIFontForLanguage(.system,15,nil)!)
            for row in selected {
                let dl = row["display"] as! Object, em = 15*ratio
                let top = baseline-number(dl,"height")*em
                draw(row,context,CGPoint(x:x,y:top),em,false)
                ascent = max(ascent,number(dl,"height")*em); descent = max(descent,number(dl,"depth")*em)
                x += number(dl,"width")*em
                x += text(" 与正文 ",context,x,baseline,15,false)
            }
            context.setStrokeColor(CGColor(red:0.7,green:0.2,blue:0.2,alpha:0.5)); context.setLineWidth(0.4)
            context.move(to:CGPoint(x:12,y:baseline));context.addLine(to:CGPoint(x:x,y:baseline));context.strokePath()
            measures.append(["style":mode,"ratio":ratio,"ascent_pt":ascent,"descent_pt":descent,"minimum_line_height_pt":ascent+descent])
        }
    },args[4])
    try writeJSON(measures,args[4]+".json")
default: fatalError("formula fonts input.json output.png em light|dark; metrics fonts chars output.json; inline fonts rows.json output.png")
}
