use crate::{Color, FontMetrics, Point, Primitive, Rect, TextSpan};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Style {
    pub size: f64,
    pub weight: u16,
    pub color: Color,
}

#[derive(Clone)]
struct Piece {
    text: String,
    style: Style,
    badge: bool,
}

pub(crate) struct Label {
    pub main: String,
    pub rest: String,
    pub sub: String,
}

pub(crate) fn label_parts(label: &str) -> Label {
    let mut parts = Vec::new();
    let mut start = 0;
    let lower = label.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(next) = lower[cursor..].find('<') {
        let at = cursor + next;
        let Some(end) = lower[at..].find('>') else {
            break;
        };
        let end = at + end;
        let tag = lower[at + 1..end].replace([' ', '\t'], "");
        if tag == "br" || tag == "br/" {
            parts.push(&label[start..at]);
            start = end + 1;
        }
        cursor = end + 1;
    }
    parts.push(&label[start..]);
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = clean(parts[0]);
    let mut main = text.clone();
    let mut rest = String::new();
    if let Some((head, tail)) = text.split_once(' ')
        && head.chars().last().is_some_and(|c| !c.is_ascii())
        && tail.chars().next().is_some_and(|c| !c.is_ascii())
    {
        main = head.into();
        rest = format!(" {tail}");
    }
    Label {
        main,
        rest,
        sub: clean(&parts[1..].join(" ")),
    }
}

fn forbidden_start(ch: char) -> bool {
    "，。！？、：；）》」』】〕〉’”％…,.!?;:%)]}".contains(ch)
}
fn forbidden_end(ch: char) -> bool {
    "（《「『【〔〈‘“([{".contains(ch)
}
fn punctuation(ch: char) -> bool {
    "，。！？、：；,.;:!?-—/）》」』】〕〉".contains(ch)
}

fn tokens(pieces: &[Piece]) -> Vec<Vec<Piece>> {
    let mut result = Vec::new();
    let mut token: Vec<Piece> = Vec::new();
    let mut brackets = 0usize;
    let mut pending_break = false;
    for piece in pieces {
        if piece.badge {
            if !token.is_empty() {
                result.push(std::mem::take(&mut token));
            }
            result.push(vec![piece.clone()]);
            continue;
        }
        for grapheme in piece.text.graphemes(true) {
            let ch = grapheme.chars().next().unwrap();
            if ((pending_break && !forbidden_start(ch)) || forbidden_end(ch))
                && brackets == 0
                && !token.is_empty()
            {
                result.push(std::mem::take(&mut token));
            }
            if let Some(last) = token.last_mut().filter(|last| last.style == piece.style) {
                last.text.push_str(grapheme);
            } else {
                token.push(Piece {
                    text: grapheme.into(),
                    style: piece.style,
                    badge: false,
                });
            }
            if forbidden_end(ch) {
                brackets += 1;
            } else if "）)］]｝}》」』】〕〉’”".contains(ch) {
                brackets = brackets.saturating_sub(1);
            }
            pending_break = brackets == 0 && (ch.is_whitespace() || punctuation(ch));
        }
    }
    if !token.is_empty() {
        result.push(token);
    }
    result
}

fn width(pieces: &[Piece], fonts: &mut dyn FontMetrics) -> f64 {
    pieces
        .iter()
        .map(|p| {
            fonts.measure(&p.text, p.style.size, p.style.weight).width
                + if p.badge { 18.0 } else { 0.0 }
        })
        .sum()
}

fn trim_start(pieces: &mut Vec<Piece>) {
    while let Some(first) = pieces.first_mut() {
        first.text = first.text.trim_start().into();
        if !first.text.is_empty() {
            break;
        }
        pieces.remove(0);
    }
}

fn trim_end(pieces: &mut Vec<Piece>) {
    while let Some(last) = pieces.last_mut() {
        last.text = last.text.trim_end().into();
        if !last.text.is_empty() {
            break;
        }
        pieces.pop();
    }
}

// 只有一个词组在整行也放不下时才按字素强制断开，禁则标点与前一字素绑定。
fn emergency_units(pieces: Vec<Piece>) -> Vec<Vec<Piece>> {
    let mut units: Vec<Vec<Piece>> = Vec::new();
    let mut opening = false;
    for piece in pieces {
        if piece.badge {
            units.push(vec![piece]);
            continue;
        }
        for g in piece.text.graphemes(true) {
            let ch = g.chars().next().unwrap();
            let next = Piece {
                text: g.into(),
                style: piece.style,
                badge: false,
            };
            if (opening || forbidden_start(ch)) && !units.is_empty() {
                units.last_mut().unwrap().push(next);
            } else {
                units.push(vec![next]);
            }
            opening = forbidden_end(ch);
        }
    }
    units
}

pub(crate) struct TextLayout {
    pub primitives: Vec<Primitive>,
    pub height: f64,
}

pub(crate) struct RowStyle {
    pub main: Style,
    pub muted: Color,
    pub badge_color: Color,
    pub line_height: f64,
    pub root: bool,
}

pub(crate) fn row(
    label: &str,
    prefix: Option<&str>,
    badge: Option<usize>,
    style: RowStyle,
    available: f64,
    fonts: &mut dyn FontMetrics,
) -> TextLayout {
    let parts = label_parts(label);
    let rest_style = Style {
        weight: 400,
        color: style.muted,
        ..style.main
    };
    let sub_style = Style {
        size: if style.root {
            14.5
        } else {
            (style.main.size * 0.86).max(13.6)
        },
        weight: 400,
        color: style.muted,
    };
    let mut pieces = Vec::new();
    if let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty()) {
        pieces.push(Piece {
            text: format!("{prefix}："),
            style: Style {
                size: style.main.size,
                weight: 400,
                color: style.muted,
            },
            badge: false,
        });
    }
    pieces.push(Piece {
        text: parts.main,
        style: style.main,
        badge: false,
    });
    if !parts.rest.is_empty() {
        pieces.push(Piece {
            text: parts.rest,
            style: rest_style,
            badge: false,
        });
    }
    if !style.root && !parts.sub.is_empty() {
        pieces.push(Piece {
            text: format!(" {}", parts.sub),
            style: sub_style,
            badge: false,
        });
    }
    if let Some(count) = badge {
        pieces.push(Piece {
            text: format!("+{count}"),
            style: Style {
                size: 13.6,
                weight: 500,
                color: style.badge_color,
            },
            badge: true,
        });
    }
    let mut result = wrap(pieces, available, style.line_height, fonts);
    if style.root && !parts.sub.is_empty() {
        let mut sub = wrap(
            vec![Piece {
                text: parts.sub,
                style: sub_style,
                badge: false,
            }],
            available,
            14.5 * 1.7,
            fonts,
        );
        translate(&mut sub.primitives, 0.0, result.height + 2.0);
        result.primitives.extend(sub.primitives);
        result.height += sub.height + 2.0;
    }
    result
}

fn wrap(
    pieces: Vec<Piece>,
    available: f64,
    line_height: f64,
    fonts: &mut dyn FontMetrics,
) -> TextLayout {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut line_width = 0.0;
    for mut token in tokens(&pieces) {
        if line.is_empty() {
            trim_start(&mut token);
        }
        let token_width = width(&token, fonts);
        if !line.is_empty() && line_width + token_width > available + 0.001 {
            trim_end(&mut line);
            lines.push(std::mem::take(&mut line));
            line_width = 0.0;
            trim_start(&mut token);
        }
        if width(&token, fonts) > available + 0.001 {
            for unit in emergency_units(token) {
                let unit_width = width(&unit, fonts);
                if !line.is_empty() && line_width + unit_width > available + 0.001 {
                    trim_end(&mut line);
                    lines.push(std::mem::take(&mut line));
                    line_width = 0.0;
                }
                line_width += unit_width;
                line.extend(unit);
            }
        } else {
            line_width += width(&token, fonts);
            line.extend(token);
        }
    }
    trim_end(&mut line);
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    let height = lines.len() as f64 * line_height;
    let mut primitives = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        let mut merged: Vec<Piece> = Vec::new();
        for p in line {
            if let Some(last) = merged
                .last_mut()
                .filter(|last| last.style == p.style && !last.badge && !p.badge)
            {
                last.text.push_str(&p.text);
            } else {
                merged.push(p);
            }
        }
        let measurements = merged
            .iter()
            .map(|p| fonts.measure(&p.text, p.style.size, p.style.weight))
            .collect::<Vec<_>>();
        let ascent = measurements.iter().map(|m| m.ascent).fold(0.0, f64::max);
        let descent = measurements.iter().map(|m| m.descent).fold(0.0, f64::max);
        let baseline = index as f64 * line_height + (line_height + ascent - descent) / 2.0;
        let mut x = 0.0;
        for (piece, measured) in merged.into_iter().zip(measurements) {
            if piece.badge {
                x += 4.0;
                primitives.push(Primitive::RoundedRect {
                    rect: Rect {
                        x,
                        y: baseline - 14.0,
                        width: measured.width + 14.0,
                        height: 18.0,
                    },
                    radius: 9.0,
                    color: Color {
                        a: 0.14,
                        ..piece.style.color
                    },
                });
                x += 7.0;
            }
            for run in measured.runs {
                primitives.push(Primitive::Text(TextSpan {
                    text: run.text,
                    baseline: Point { x, y: baseline },
                    width: run.width,
                    font_size: piece.style.size,
                    font_weight: piece.style.weight,
                    font_family: run.font_family,
                    color: piece.style.color,
                }));
                x += run.width;
            }
            if piece.badge {
                x += 7.0;
            }
        }
    }
    TextLayout { primitives, height }
}

pub(crate) fn translate(primitives: &mut [Primitive], x: f64, y: f64) {
    for p in primitives {
        match p {
            Primitive::RoundedRect { rect, .. } => {
                rect.x += x;
                rect.y += y;
            }
            Primitive::Line { from, to, .. } => {
                from.x += x;
                from.y += y;
                to.x += x;
                to.y += y;
            }
            Primitive::Text(span) => {
                span.baseline.x += x;
                span.baseline.y += y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeterministicMetrics;

    #[test]
    fn changing_badge_color_preserves_all_row_geometry() {
        let main = Color::oklch(0.25, 0.01, 260.0);
        let muted = Color::oklch(0.5, 0.01, 260.0);
        let old_badge = Color::oklch(0.38, 0.008, 260.0);
        let branch_title = Color::oklch(0.48, 0.1, 255.0);
        let render = |badge_color| {
            row(
                "Nested branch",
                None,
                Some(3),
                RowStyle {
                    main: Style {
                        size: 15.0,
                        weight: 400,
                        color: main,
                    },
                    muted,
                    badge_color,
                    line_height: 23.0,
                    root: false,
                },
                240.0,
                &mut DeterministicMetrics,
            )
        };
        let before = render(old_badge);
        let after = render(branch_title);
        let neutral = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        let geometry = |layout: &TextLayout| {
            layout
                .primitives
                .iter()
                .map(|primitive| {
                    let mut normalized = primitive.clone();
                    match &mut normalized {
                        Primitive::RoundedRect { color, .. } => *color = neutral,
                        Primitive::Text(text) => text.color = neutral,
                        Primitive::Line { color, .. } => *color = neutral,
                    }
                    normalized
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(before.height, after.height);
        assert_eq!(geometry(&before), geometry(&after));
        for primitive in after.primitives {
            match primitive {
                Primitive::RoundedRect { color, .. } if color.a == 0.14 => {
                    assert_eq!(color.r, branch_title.r);
                    assert_eq!(color.g, branch_title.g);
                    assert_eq!(color.b, branch_title.b);
                }
                Primitive::Text(text) if text.text == "+3" => {
                    assert_eq!(text.color, branch_title);
                }
                _ => {}
            }
        }
    }
}
