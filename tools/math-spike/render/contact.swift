// SPDX-License-Identifier: MIT OR Apache-2.0
import Foundation
import CoreText
import ImageIO
import UniformTypeIdentifiers
let directory = URL(fileURLWithPath: CommandLine.arguments[1])
let rows = try JSONSerialization.jsonObject(with: Data(contentsOf: directory.appendingPathComponent("index.json"))) as! [[String:Any]]
for theme in ["light","dark"] {
    for page in 0..<2 {
        let width=1600, height=1400
        let c=CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        c.setFillColor(CGColor(gray:0.85,alpha:1));c.fill(CGRect(x:0,y:0,width:width,height:height))
        for (index,row) in rows[(page*10)..<(page*10+10)].enumerated() {
            let id=(row["id"] as! NSNumber).intValue, top=index*140
            let label=String(format:"%03d  Core Text (left) / ratex-render (right)",id)
            let attr=NSAttributedString(string:label,attributes:[NSAttributedString.Key(kCTFontAttributeName as String):CTFontCreateWithName("Helvetica" as CFString,13,nil)])
            c.textPosition=CGPoint(x:12,y:height-top-18);CTLineDraw(CTLineCreateWithAttributedString(attr),c)
            for (side,suffix) in ["","-ratex"].enumerated() {
                let url=directory.appendingPathComponent(String(format:"%03d-%@%@.png",id,theme,suffix))
                let im=CGImageSourceCreateImageAtIndex(CGImageSourceCreateWithURL(url as CFURL,nil)!,0,nil)!
                let scale=min(1,min(770.0/Double(im.width),110.0/Double(im.height)))
                let imageWidth = Double(im.width) * scale
                let imageHeight = Double(im.height) * scale
                let left = Double(12 + side * 800)
                let bottom = Double(height - top - 26) - imageHeight
                c.draw(im, in: CGRect(x:left,y:bottom,width:imageWidth,height:imageHeight))
            }
        }
        let url=directory.appendingPathComponent("contact-\(theme)-\(page+1).png")
        let dest=CGImageDestinationCreateWithURL(url as CFURL,UTType.png.identifier as CFString,1,nil)!
        CGImageDestinationAddImage(dest,c.makeImage()!,nil);CGImageDestinationFinalize(dest)
    }
}
