use crate::{Primitive, Scene};
use std::fmt::Write;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

impl Scene {
    /// 一段一段放置已测量的文字，浏览器只绘制，不参与换行或节点定位。
    pub fn to_svg(&self) -> String {
        let mut output = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.3}\" height=\"{:.3}\" viewBox=\"0 0 {:.3} {:.3}\" role=\"img\" aria-label=\"Mermaid tree\">\n",
            self.width, self.height, self.width, self.height
        );
        for primitive in &self.primitives {
            match primitive {
                Primitive::RoundedRect {
                    rect,
                    radius,
                    color,
                } => {
                    writeln!(output, "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" rx=\"{radius:.3}\" fill=\"{}\" fill-opacity=\"{:.3}\"/>", rect.x, rect.y, rect.width, rect.height, color.svg(), color.a).unwrap();
                }
                Primitive::Line {
                    from,
                    to,
                    width,
                    color,
                } => {
                    writeln!(output, "<line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" stroke=\"{}\" stroke-width=\"{width:.3}\"/>", from.x, from.y, to.x, to.y, color.svg()).unwrap();
                }
                Primitive::Text(span) => {
                    if span.width > 0.0 {
                        // SF 的内部家族名不能直接通过浏览器 CSS 访问，系统别名指向同一字体。
                        let family = if span.font_family == "System Font"
                            || span.font_family == ".AppleSystemUIFont"
                        {
                            "-apple-system, BlinkMacSystemFont"
                        } else {
                            &span.font_family
                        };
                        writeln!(output, "<text x=\"{:.3}\" y=\"{:.3}\" font-family=\"{}\" font-size=\"{:.3}\" font-weight=\"{}\" fill=\"{}\" xml:space=\"preserve\" textLength=\"{:.3}\" lengthAdjust=\"spacingAndGlyphs\">{}</text>", span.baseline.x, span.baseline.y, escape(family), span.font_size, span.font_weight, span.color.svg(), span.width, escape(&span.text)).unwrap();
                    }
                }
            }
        }
        output.push_str("</svg>\n");
        output
    }
}
