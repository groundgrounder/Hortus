use crate::{Marker, Primitive, Scene};
use std::fmt::Write;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn marker_svg(marker: Marker, points: &[crate::Point], at_start: bool, color: &str) -> String {
    let (Some(point), Some(next)) = (if at_start {
        (points.first(), points.get(1))
    } else {
        (points.last(), points.get(points.len().saturating_sub(2)))
    }) else {
        return String::new();
    };
    if marker == Marker::None {
        return String::new();
    }
    let dx = point.x - next.x;
    let dy = point.y - next.y;
    let length = dx.hypot(dy).max(0.001);
    let ux = dx / length;
    let uy = dy / length;
    match marker {
        Marker::None => String::new(),
        Marker::Arrow => {
            let bx = point.x - ux * 9.0;
            let by = point.y - uy * 9.0;
            let px = -uy * 4.0;
            let py = ux * 4.0;
            format!(
                "<polygon points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" fill=\"{color}\"/>",
                point.x,
                point.y,
                bx + px,
                by + py,
                bx - px,
                by - py
            )
        }
        Marker::Circle => format!(
            "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"3.5\" fill=\"{color}\" stroke=\"{color}\" stroke-width=\"1.5\"/>",
            point.x - ux * 4.25,
            point.y - uy * 4.25
        ),
        Marker::Cross => {
            let cx = point.x - ux * 5.0;
            let cy = point.y - uy * 5.0;
            let mut result = String::new();
            for sign in [-1.0, 1.0] {
                write!(result, "<path d=\"M {:.3} {:.3} L {:.3} {:.3}\" stroke=\"{color}\" stroke-width=\"1.5\" stroke-linecap=\"round\"/>",
                    cx-ux*3.0-uy*3.0*sign, cy-uy*3.0+ux*3.0*sign,
                    cx+ux*3.0+uy*3.0*sign, cy+uy*3.0-ux*3.0*sign).unwrap();
            }
            result
        }
    }
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
                Primitive::GraphPath {
                    edge_id,
                    corner_radius,
                    points,
                    source_range,
                    line,
                    start,
                    end,
                    color,
                    width,
                    ..
                } => {
                    let dash = if *line == crate::EdgeLine::Dotted {
                        " stroke-dasharray=\"4 4\""
                    } else {
                        ""
                    };
                    let line_width = *width;
                    let mut d = String::new();
                    if let Some(first) = points.first() {
                        write!(d, "M {:.3} {:.3}", first.x, first.y).unwrap();
                        for triple in points.windows(3) {
                            let (a, b, c) = (triple[0], triple[1], triple[2]);
                            let incoming = (b.x - a.x).hypot(b.y - a.y);
                            let outgoing = (c.x - b.x).hypot(c.y - b.y);
                            let radius = corner_radius.min(incoming / 2.0).min(outgoing / 2.0);
                            if radius > 0.001 {
                                let before = crate::Point {
                                    x: b.x + (a.x - b.x) * radius / incoming,
                                    y: b.y + (a.y - b.y) * radius / incoming,
                                };
                                let after = crate::Point {
                                    x: b.x + (c.x - b.x) * radius / outgoing,
                                    y: b.y + (c.y - b.y) * radius / outgoing,
                                };
                                write!(
                                    d,
                                    " L {:.3} {:.3} Q {:.3} {:.3} {:.3} {:.3}",
                                    before.x, before.y, b.x, b.y, after.x, after.y
                                )
                                .unwrap();
                            } else {
                                write!(d, " L {:.3} {:.3}", b.x, b.y).unwrap();
                            }
                        }
                        if let Some(last) = points.last() {
                            write!(d, " L {:.3} {:.3}", last.x, last.y).unwrap();
                        }
                    }
                    writeln!(output, "<g data-edge-id=\"{edge_id}\" data-source-start=\"{}\" data-source-end=\"{}\"><path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{line_width:.3}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"{dash}/>{}{}</g>", source_range.start, source_range.end, color.svg(), marker_svg(*start, points, true, &color.svg()), marker_svg(*end, points, false, &color.svg())).unwrap();
                }
                Primitive::GraphShape {
                    outline,
                    details,
                    fill,
                    stroke,
                    stroke_width,
                    ..
                } => {
                    let path = |points: &[crate::Point]| {
                        points
                            .iter()
                            .enumerate()
                            .map(|(index, p)| {
                                format!(
                                    "{} {:.3} {:.3}",
                                    if index == 0 { "M" } else { "L" },
                                    p.x,
                                    p.y
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    };
                    writeln!(output, "<path d=\"{} Z\" fill=\"{}\" fill-opacity=\"{:.3}\" stroke=\"{}\" stroke-width=\"{stroke_width:.3}\"/>", path(outline), fill.svg(), fill.a, stroke.svg()).unwrap();
                    for points in details {
                        writeln!(output, "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{stroke_width:.3}\"/>", path(points), stroke.svg()).unwrap();
                    }
                }
                Primitive::LabelBackground { rect, color, .. } => {
                    writeln!(output, "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" rx=\"3\" fill=\"{}\" fill-opacity=\"{:.3}\"/>", rect.x, rect.y, rect.width, rect.height, color.svg(), color.a).unwrap();
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
