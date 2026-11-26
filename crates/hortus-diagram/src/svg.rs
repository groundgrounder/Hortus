use crate::{Color, Marker, Primitive, Scene};
use std::fmt::Write;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn paint(attribute: &str, color: Color) -> String {
    let mut paint = format!("{attribute}=\"{}\"", color.svg());
    if color.a < 1.0 {
        write!(paint, " {attribute}-opacity=\"{:.3}\"", color.a).unwrap();
    }
    paint
}

fn marker_svg(marker: Marker, points: &[crate::Point], at_start: bool, color: Color) -> String {
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
                "<polygon points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" {}/>",
                point.x,
                point.y,
                bx + px,
                by + py,
                bx - px,
                by - py,
                paint("fill", color),
            )
        }
        Marker::Circle => format!(
            "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"3.5\" {} {} stroke-width=\"1.5\"/>",
            point.x - ux * 4.25,
            point.y - uy * 4.25,
            paint("fill", color),
            paint("stroke", color),
        ),
        Marker::Cross => {
            let cx = point.x - ux * 5.0;
            let cy = point.y - uy * 5.0;
            let mut result = String::new();
            let stroke = paint("stroke", color);
            for sign in [-1.0, 1.0] {
                write!(result, "<path d=\"M {:.3} {:.3} L {:.3} {:.3}\" {stroke} stroke-width=\"1.5\" stroke-linecap=\"round\"/>",
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
        let label = match self.metrics.diagram_kind.as_str() {
            "tree" => "Mermaid tree",
            "flowchart" => "Mermaid flowchart",
            "sequence" => "Mermaid sequence diagram",
            "state" => "Mermaid state diagram",
            "class" => "Mermaid class diagram",
            "er" => "Mermaid entity-relationship diagram",
            _ => "Mermaid diagram",
        };
        let mut output = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.3}\" height=\"{:.3}\" viewBox=\"0 0 {:.3} {:.3}\" role=\"img\" aria-label=\"{label}\">\n",
            self.width, self.height, self.width, self.height
        );
        for primitive in &self.primitives {
            match primitive {
                Primitive::RoundedRect {
                    rect,
                    radius,
                    color,
                } => {
                    writeln!(output, "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" rx=\"{radius:.3}\" {}/>", rect.x, rect.y, rect.width, rect.height, paint("fill", *color)).unwrap();
                }
                Primitive::Line {
                    from,
                    to,
                    width,
                    color,
                } => {
                    writeln!(output, "<line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" {} stroke-width=\"{width:.3}\"/>", from.x, from.y, to.x, to.y, paint("stroke", *color)).unwrap();
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
                    writeln!(output, "<g data-edge-id=\"{edge_id}\" data-source-start=\"{}\" data-source-end=\"{}\"><path d=\"{d}\" fill=\"none\" {} stroke-width=\"{line_width:.3}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"{dash}/>{}{}</g>", source_range.start, source_range.end, paint("stroke", *color), marker_svg(*start, points, true, *color), marker_svg(*end, points, false, *color)).unwrap();
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
                    writeln!(
                        output,
                        "<path d=\"{} Z\" {} {} stroke-width=\"{stroke_width:.3}\"/>",
                        path(outline),
                        paint("fill", *fill),
                        paint("stroke", *stroke)
                    )
                    .unwrap();
                    for points in details {
                        writeln!(
                            output,
                            "<path d=\"{}\" fill=\"none\" {} stroke-width=\"{stroke_width:.3}\"/>",
                            path(points),
                            paint("stroke", *stroke)
                        )
                        .unwrap();
                    }
                }
                Primitive::LabelBackground { rect, color, .. } => {
                    writeln!(output, "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" rx=\"3\" {}/>", rect.x, rect.y, rect.width, rect.height, paint("fill", *color)).unwrap();
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
                        writeln!(output, "<text x=\"{:.3}\" y=\"{:.3}\" font-family=\"{}\" font-size=\"{:.3}\" font-weight=\"{}\" {} xml:space=\"preserve\" textLength=\"{:.3}\" lengthAdjust=\"spacingAndGlyphs\">{}</text>", span.baseline.x, span.baseline.y, escape(family), span.font_size, span.font_weight, paint("fill", span.color), span.width, escape(&span.text)).unwrap();
                    }
                }
            }
        }
        output.push_str("</svg>\n");
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeterministicMetrics, EdgeLine, LayoutOptions, Point, Rect, Shape, TextSpan};

    #[test]
    fn svg_preserves_alpha_for_each_fill_stroke_and_marker() {
        let mut scene = crate::layout(
            &crate::parse("mindmap\n  Root").unwrap(),
            &LayoutOptions::default(),
            &mut DeterministicMetrics,
        )
        .unwrap();
        let color = |a| crate::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a,
        };
        let from = Point { x: 1.0, y: 1.0 };
        let to = Point { x: 10.0, y: 10.0 };
        let points = vec![from, to];
        let rect = Rect {
            x: 1.0,
            y: 1.0,
            width: 10.0,
            height: 10.0,
        };
        let edge = |start, end, alpha| Primitive::GraphPath {
            edge_id: 1,
            corner_radius: 0.0,
            points: points.clone(),
            source_range: 0..1,
            label_range: None,
            line: EdgeLine::Solid,
            start,
            end,
            color: color(alpha),
            width: 1.0,
        };
        scene.primitives = vec![
            Primitive::RoundedRect {
                rect,
                radius: 2.0,
                color: color(0.25),
            },
            Primitive::LabelBackground {
                rect,
                edge_id: 1,
                color: color(0.5),
            },
            Primitive::Line {
                from,
                to,
                width: 1.0,
                color: color(0.375),
            },
            Primitive::GraphShape {
                rect,
                shape: Shape::Rectangle,
                outline: points.clone(),
                details: vec![points.clone()],
                fill: color(0.75),
                stroke: color(0.125),
                stroke_width: 1.0,
            },
            edge(Marker::Circle, Marker::Arrow, 0.625),
            edge(Marker::Cross, Marker::None, 0.875),
            Primitive::Text(TextSpan {
                text: "Alpha".into(),
                baseline: from,
                width: 5.0,
                font_size: 12.0,
                font_weight: 400,
                font_family: "Example".into(),
                color: color(0.0),
            }),
            Primitive::Line {
                from,
                to,
                width: 1.0,
                color: color(1.0),
            },
        ];
        let svg = scene.to_svg();
        let elements = |tag: &str| {
            svg.match_indices(&format!("<{tag} "))
                .map(|(index, _)| svg[index..].split_once('>').unwrap().0)
                .collect::<Vec<_>>()
        };
        for (tag, property, count) in [
            ("rect", "fill-opacity=\"0.250\"", 1),
            ("rect", "fill-opacity=\"0.500\"", 1),
            ("line", "stroke-opacity=\"0.375\"", 1),
            ("line", "stroke-opacity=", 1),
            ("path", "fill-opacity=\"0.750\"", 1),
            ("path", "stroke-opacity=\"0.125\"", 2),
            ("path", "stroke-opacity=\"0.625\"", 1),
            ("circle", "fill-opacity=\"0.625\"", 1),
            ("circle", "stroke-opacity=\"0.625\"", 1),
            ("polygon", "fill-opacity=\"0.625\"", 1),
            ("path", "stroke-opacity=\"0.875\"", 3),
            ("text", "fill-opacity=\"0.000\"", 1),
        ] {
            assert_eq!(
                elements(tag)
                    .iter()
                    .filter(|el| el.contains(property))
                    .count(),
                count,
                "expected {count} <{tag}> with {property} in {svg}"
            );
        }
    }
}
