//! 时序图的自适应布局：列间距按消息与便签需求压缩，长文字按目标列宽换行。
//!
//! 布局分四步：先量参与者胶囊定基础列距，再按消息、便签、自调用回环的
//! 单行宽度求每条间隙的最小需求；超出 `options.width` 时只压缩消息撑出的
//! 增量并按最终列宽换行，连基础间距都放不下才换行参与者标签并如实标记
//! `horizontal_overflow`。宽度充裕时间隙最多放宽 60%，避免铺得过散。
use crate::LayoutError;
use crate::sequence::{FragmentKind, NoteSide, SequenceDiagram, SequenceRow};
use crate::text::{Style, plain_wrap, translate};
use crate::{
    Color, EdgeLine, FontInfo, FontMetrics, HitRegion, LayoutOptions, Marker, Metrics, Point,
    Primitive, Rect, Scene, Shape, TextSpan, Theme,
};
use std::ops::Range;
use std::time::Instant;

const SIDE: f64 = 24.0;
const PARTICIPANT_FONT: f64 = 15.0;
const PARTICIPANT_WEIGHT: u16 = 600;
const PARTICIPANT_LINE: f64 = 20.0;
const PILL_PAD_X: f64 = 14.0;
const PILL_H: f64 = 34.0;
/// `actor` 人形图标在胶囊内占用的左侧宽度。
const ACTOR_ICON: f64 = 20.0;
const TEXT_FONT: f64 = 13.6;
const TEXT_LINE: f64 = 18.0;
const TEXT_WEIGHT: u16 = 400;
const TITLE_FONT: f64 = 16.5;
const TITLE_LINE: f64 = 24.0;
/// 相邻生命线的基础间隙。
const GAP_MIN: f64 = 24.0;
/// 宽度充裕时最多把间隙放宽到需求总和的 160%。
const GAP_SLACK_MAX: f64 = 0.6;
const SELF_LOOP_W: f64 = 30.0;
const SELF_LOOP_H: f64 = 24.0;
const NOTE_PAD_X: f64 = 10.0;
const NOTE_PAD_Y: f64 = 6.0;
const NOTE_MAX_W: f64 = 240.0;
const FRAGMENT_PAD_X: f64 = 12.0;
const FRAGMENT_INSET: f64 = 8.0;
/// 图主体高于该值时在底部镜像参与者栏，方便长图阅读。
const MIRROR_MIN_HEIGHT: f64 = 520.0;
const STROKE: f64 = 1.25;
const LIFELINE_WIDTH: f64 = 1.5;
const EDGE_WIDTH: f64 = 1.5;

struct Palette {
    background: Color,
    text: Color,
    secondary: Color,
    muted: Color,
    edge: Color,
    edge_reply: Color,
    pill_fill: Color,
    pill_stroke: Color,
    actor_fill: Color,
    actor_stroke: Color,
    lifeline: Color,
    activation: Color,
    note_fill: Color,
    note_stroke: Color,
    fragment_fill: Color,
    fragment_stroke: Color,
    fragment_label: Color,
}

impl Palette {
    fn new(theme: Theme) -> Self {
        let dark = theme == Theme::Dark;
        Self {
            background: Color::oklch(if dark { 0.215 } else { 0.982 }, 0.004, 260.0),
            text: Color::oklch(if dark { 0.91 } else { 0.25 }, 0.01, 260.0),
            secondary: Color::oklch(if dark { 0.78 } else { 0.40 }, 0.01, 260.0),
            muted: Color::oklch(if dark { 0.64 } else { 0.52 }, 0.01, 260.0),
            edge: Color::oklch(if dark { 0.68 } else { 0.46 }, 0.025, 250.0),
            edge_reply: Color::oklch(if dark { 0.58 } else { 0.55 }, 0.012, 250.0),
            pill_fill: Color::oklch(if dark { 0.27 } else { 0.995 }, 0.008, 250.0),
            pill_stroke: Color::oklch(if dark { 0.42 } else { 0.86 }, 0.012, 260.0),
            actor_fill: Color::oklch(if dark { 0.30 } else { 0.955 }, 0.025, 255.0),
            actor_stroke: Color::oklch(if dark { 0.56 } else { 0.72 }, 0.08, 255.0),
            lifeline: Color::oklch(if dark { 0.33 } else { 0.885 }, 0.006, 260.0),
            activation: Color {
                a: if dark { 0.32 } else { 0.25 },
                ..Color::oklch(if dark { 0.66 } else { 0.70 }, 0.09, 255.0)
            },
            note_fill: Color::oklch(if dark { 0.27 } else { 0.972 }, 0.02, 85.0),
            note_stroke: Color::oklch(if dark { 0.42 } else { 0.86 }, 0.035, 85.0),
            fragment_fill: Color::oklch(if dark { 0.245 } else { 0.968 }, 0.01, 255.0),
            fragment_stroke: Color::oklch(if dark { 0.36 } else { 0.87 }, 0.012, 260.0),
            fragment_label: Color::oklch(if dark { 0.72 } else { 0.45 }, 0.03, 255.0),
        }
    }
}

/// 已折行的文字块：原点在 (0, 0)，逐行宽度用于水平居中。
#[derive(Clone)]
struct TextBlock {
    primitives: Vec<Primitive>,
    line_widths: Vec<f64>,
    height: f64,
}

impl TextBlock {
    fn width(&self) -> f64 {
        self.line_widths.iter().copied().fold(0.0, f64::max)
    }
    fn lines(&self) -> usize {
        self.line_widths.len().max(1)
    }
    fn empty(&self) -> bool {
        self.primitives.is_empty()
    }
}

/// `<br/>` 拆成的硬换行；空段丢弃。
fn hard_lines(label: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
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
            parts.push(label[start..at].trim().into());
            start = end + 1;
        }
        cursor = end + 1;
    }
    parts.push(label[start..].trim().into());
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

fn block_line_widths(primitives: &[Primitive], line_height: f64) -> Vec<f64> {
    let mut widths: Vec<f64> = Vec::new();
    for primitive in primitives {
        if let Primitive::Text(span) = primitive {
            let index = (span.baseline.y / line_height).floor() as usize;
            let right = span.baseline.x + span.width;
            while widths.len() <= index {
                widths.push(0.0);
            }
            widths[index] = widths[index].max(right);
        }
    }
    widths
}

/// 按 `available` 折行一段文字；`prefix` 只并入首个硬行（自动编号、块类型角标）。
fn wrap_text(
    text: &str,
    prefix: Option<(&str, Style)>,
    main: Style,
    available: f64,
    line_height: f64,
    fonts: &mut dyn FontMetrics,
) -> TextBlock {
    let available = available.max(30.0);
    let mut block = TextBlock {
        primitives: Vec::new(),
        line_widths: Vec::new(),
        height: 0.0,
    };
    let lines = {
        let split = hard_lines(text);
        if split.is_empty() {
            vec![String::new()]
        } else {
            split
        }
    };
    for (index, hard) in lines.iter().enumerate() {
        let mut segments = Vec::new();
        if index == 0
            && let Some((prefix, style)) = prefix
        {
            segments.push((prefix, style));
        }
        segments.push((hard.as_str(), main));
        let mut layout = plain_wrap(&segments, available, line_height, fonts);
        let widths = block_line_widths(&layout.primitives, line_height);
        translate(&mut layout.primitives, 0.0, block.height);
        block.primitives.append(&mut layout.primitives);
        block.line_widths.extend(widths);
        block.height += layout.height;
    }
    block
}

enum Anchor {
    Left(f64),
    Center(f64),
}

fn emit_block(
    target: &mut Vec<Primitive>,
    block: &TextBlock,
    anchor: Anchor,
    top: f64,
    line_height: f64,
) {
    for primitive in &block.primitives {
        let Primitive::Text(span) = primitive else {
            continue;
        };
        let index = (span.baseline.y / line_height).floor() as usize;
        let x = match anchor {
            Anchor::Left(x) => x,
            Anchor::Center(x) => x - block.line_widths.get(index).copied().unwrap_or(0.0) / 2.0,
        };
        target.push(Primitive::Text(TextSpan {
            text: span.text.clone(),
            baseline: Point {
                x: span.baseline.x + x,
                y: span.baseline.y + top,
            },
            width: span.width,
            font_size: span.font_size,
            font_weight: span.font_weight,
            font_family: span.font_family.clone(),
            color: span.color,
        }));
    }
}

#[derive(Clone)]
struct Pill {
    label: TextBlock,
    width: f64,
    height: f64,
}

fn plan_pills(
    diagram: &SequenceDiagram,
    palette: &Palette,
    target: Option<f64>,
    fonts: &mut dyn FontMetrics,
) -> Vec<Pill> {
    let style = Style {
        size: PARTICIPANT_FONT,
        weight: PARTICIPANT_WEIGHT,
        color: palette.text,
    };
    let mut pills = Vec::new();
    for participant in &diagram.participants {
        let pad = PILL_PAD_X * 2.0 + if participant.actor { ACTOR_ICON } else { 0.0 };
        let mut label = wrap_text(
            &participant.display,
            None,
            style,
            f64::INFINITY,
            PARTICIPANT_LINE,
            fonts,
        );
        if let Some(limit) = target {
            let limit = (limit - pad).max(52.0);
            if label.width() + pad > limit {
                label = wrap_text(
                    &participant.display,
                    None,
                    style,
                    limit,
                    PARTICIPANT_LINE,
                    fonts,
                );
            }
        }
        let height = if label.lines() <= 1 {
            PILL_H
        } else {
            label.lines() as f64 * PARTICIPANT_LINE + 10.0
        };
        pills.push(Pill {
            width: (label.width() + pad).max(56.0),
            label,
            height,
        });
    }
    pills
}

fn band_height(pills: &[Pill]) -> f64 {
    pills.iter().map(|pill| pill.height).fold(0.0, f64::max) + 10.0
}

/// 单行文字宽度：不换行时对间隙的最小需求。
fn single_width(text: &str, fonts: &mut dyn FontMetrics) -> f64 {
    hard_lines(text)
        .into_iter()
        .map(|line| fonts.measure(&line, TEXT_FONT, TEXT_WEIGHT).width)
        .fold(0.0, f64::max)
}

fn row_cols(row: &SequenceRow) -> Option<Vec<usize>> {
    match row {
        SequenceRow::Message(message) => Some(vec![message.from, message.to]),
        SequenceRow::Note(note) => match &note.side {
            NoteSide::LeftOf(c) | NoteSide::RightOf(c) => Some(vec![*c]),
            NoteSide::Over(cols) => Some(cols.clone()),
        },
        SequenceRow::Activate { participant, .. } | SequenceRow::Deactivate { participant, .. } => {
            Some(vec![*participant])
        }
        _ => None,
    }
}

/// 组合块：水平预走定列范围与标签折行，主走再填纵向坐标。
#[derive(Clone)]
struct FrameMeta {
    fill: Option<Color>,
    x0: f64,
    x1: f64,
    label: TextBlock,
    dividers: Vec<TextBlock>,
    source_range: Range<usize>,
    top: f64,
    bottom: f64,
    label_top: f64,
    /// 每条分支分隔的 (标签顶, 虚线 y)。
    divider_slots: Vec<(f64, f64)>,
}

fn divider_word(kind: FragmentKind) -> &'static str {
    match kind {
        FragmentKind::Alt => "else",
        FragmentKind::Par => "and",
        FragmentKind::Critical => "option",
        _ => "else",
    }
}

/// 水平预走：每个组合块按受影响参与者列定 x 范围（嵌套内缩），
/// 折行块标签与分支分隔标签；返回按起始顺序排列的帧。
fn plan_frames(
    diagram: &SequenceDiagram,
    palette: &Palette,
    xs: &[f64],
    pills: &[Pill],
    width: f64,
    count: usize,
    fonts: &mut dyn FontMetrics,
) -> Vec<FrameMeta> {
    struct Open {
        refs: Vec<usize>,
        dividers: Vec<String>,
        kind: FragmentKind,
        fill: Option<Color>,
        label: String,
        source_range: Range<usize>,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut finished: Vec<FrameMeta> = Vec::new();
    // 第 k 个 FragmentStart 对应完成帧的索引。
    let mut by_start: Vec<Option<usize>> = Vec::new();
    let mut start_stack: Vec<usize> = Vec::new();
    for row in &diagram.rows {
        if let Some(cols) = row_cols(row) {
            for frame in &mut open {
                frame.refs.extend(cols.iter().copied());
            }
        }
        match row {
            SequenceRow::FragmentStart {
                kind,
                label,
                fill,
                source_range,
            } => {
                start_stack.push(by_start.len());
                by_start.push(None);
                open.push(Open {
                    refs: Vec::new(),
                    dividers: Vec::new(),
                    kind: *kind,
                    fill: *fill,
                    label: label.text.clone(),
                    source_range: source_range.clone(),
                });
            }
            SequenceRow::FragmentDivider { label, .. } => {
                if let Some(frame) = open.last_mut() {
                    frame.dividers.push(label.text.clone());
                }
            }
            SequenceRow::FragmentEnd { .. } => {
                let (frame, start) = match (open.pop(), start_stack.pop()) {
                    (Some(frame), Some(start)) => (frame, start),
                    _ => continue,
                };
                let depth = start_stack.len();
                let (min_col, max_col) = if frame.refs.is_empty() {
                    (0, count - 1)
                } else {
                    (
                        *frame.refs.iter().min().unwrap(),
                        *frame.refs.iter().max().unwrap(),
                    )
                };
                let base_x0 = xs[min_col] - pills[min_col].width / 2.0 - FRAGMENT_PAD_X;
                let base_x1 = xs[max_col] + pills[max_col].width / 2.0 + FRAGMENT_PAD_X;
                let inset = FRAGMENT_INSET * depth as f64;
                let mut x0 = base_x0 + inset;
                let mut x1 = base_x1 - inset;
                if x1 - x0 < 60.0 {
                    x0 = base_x0;
                    x1 = base_x1;
                }
                x0 = x0.clamp(10.0, width - 70.0);
                x1 = x1.clamp(x0 + 60.0, width - 10.0);
                let label_style = Style {
                    size: TEXT_FONT,
                    weight: 400,
                    color: palette.fragment_label,
                };
                let muted = Style {
                    size: TEXT_FONT,
                    weight: 500,
                    color: palette.muted,
                };
                let prefix = format!("{} ", frame.kind.word());
                let label = wrap_text(
                    &frame.label,
                    Some((prefix.as_str(), muted)),
                    label_style,
                    (x1 - x0) - 20.0,
                    TEXT_LINE,
                    fonts,
                );
                let divider_prefix = format!("{} ", divider_word(frame.kind));
                let dividers = frame
                    .dividers
                    .iter()
                    .map(|text| {
                        wrap_text(
                            text,
                            Some((divider_prefix.as_str(), muted)),
                            label_style,
                            (x1 - x0) - 20.0,
                            TEXT_LINE,
                            fonts,
                        )
                    })
                    .collect::<Vec<_>>();
                by_start[start] = Some(finished.len());
                finished.push(FrameMeta {
                    fill: frame.fill,
                    x0,
                    x1,
                    label,
                    dividers,
                    source_range: frame.source_range,
                    top: 0.0,
                    bottom: 0.0,
                    label_top: 0.0,
                    divider_slots: vec![(0.0, 0.0); frame.dividers.len()],
                });
            }
            _ => {}
        }
    }
    // 按起始顺序返回：外层先画，内层覆盖其上。
    by_start
        .into_iter()
        .filter_map(|slot| slot.map(|index| finished[index].clone()))
        .collect()
}

/// `actor` 人形图标：头部圆 + 肩部弧线，作为 `details` 细节线绘制。
fn person_details(cx: f64, cy: f64) -> Vec<Vec<Point>> {
    let mut head = Vec::new();
    for step in 0..=10 {
        let angle = step as f64 * std::f64::consts::TAU / 10.0;
        head.push(Point {
            x: cx + 3.4 * angle.cos(),
            y: cy - 4.2 + 3.4 * angle.sin(),
        });
    }
    let shoulders = vec![
        Point {
            x: cx - 5.6,
            y: cy + 6.0,
        },
        Point {
            x: cx - 3.0,
            y: cy + 1.0,
        },
        Point {
            x: cx + 3.0,
            y: cy + 1.0,
        },
        Point {
            x: cx + 5.6,
            y: cy + 6.0,
        },
    ];
    vec![head, shoulders]
}

struct Layout<'a> {
    diagram: &'a SequenceDiagram,
    palette: Palette,
    width: f64,
    fonts: &'a mut dyn FontMetrics,
    pills: Vec<Pill>,
    xs: Vec<f64>,
    frames: Vec<FrameMeta>,
    pills_drawn: Vec<Primitive>,
    fragment_shapes: Vec<Primitive>,
    fragment_lines: Vec<Primitive>,
    lifelines: Vec<Primitive>,
    bars: Vec<Primitive>,
    notes: Vec<Primitive>,
    arrows: Vec<Primitive>,
    label_bgs: Vec<Primitive>,
    texts: Vec<Primitive>,
    hits: Vec<HitRegion>,
    /// 每个参与者未闭合的激活条起点，栈底在外。
    activation: Vec<Vec<f64>>,
    edge_id: usize,
    message_ordinal: usize,
    note_ordinal: usize,
}

impl Layout<'_> {
    fn msg_style(&self) -> Style {
        Style {
            size: TEXT_FONT,
            weight: TEXT_WEIGHT,
            color: self.palette.secondary,
        }
    }
    fn muted_style(&self) -> Style {
        Style {
            size: TEXT_FONT,
            weight: TEXT_WEIGHT,
            color: self.palette.muted,
        }
    }

    /// 参与者胶囊；`top` 为表头带顶部（镜像栏复用）。
    fn emit_pills(&mut self, top: f64) {
        let diagram = self.diagram;
        let band = band_height(&self.pills);
        for (index, participant) in diagram.participants.iter().enumerate() {
            let pill = &self.pills[index];
            let rect = Rect {
                x: self.xs[index] - pill.width / 2.0,
                y: top + (band - pill.height) / 2.0,
                width: pill.width,
                height: pill.height,
            };
            let (shape, fill, stroke, details) = if participant.actor {
                (
                    Shape::Rounded,
                    self.palette.actor_fill,
                    self.palette.actor_stroke,
                    person_details(rect.x + 12.0, rect.y + rect.height / 2.0),
                )
            } else {
                (
                    Shape::Stadium,
                    self.palette.pill_fill,
                    self.palette.pill_stroke,
                    Vec::new(),
                )
            };
            self.pills_drawn.push(Primitive::GraphShape {
                rect,
                shape,
                outline: crate::shape::outline(shape, rect),
                details,
                fill,
                stroke,
                stroke_width: STROKE,
            });
            let icon = if participant.actor { ACTOR_ICON } else { 0.0 };
            let center = rect.x + icon + (rect.width - icon) / 2.0;
            let label_top = rect.y + (rect.height - pill.label.height) / 2.0;
            emit_block(
                &mut self.texts,
                &pill.label,
                Anchor::Center(center),
                label_top,
                PARTICIPANT_LINE,
            );
            self.hits.push(HitRegion {
                node_id: participant.id.clone(),
                label: participant.display.clone(),
                prefix: None,
                rect,
                can_collapse: false,
                collapsed: false,
                source_range: participant.source_range.clone(),
            });
        }
    }

    fn emit_lifelines(&mut self, top: f64, bottom: f64) {
        let diagram = self.diagram;
        for (index, participant) in diagram.participants.iter().enumerate() {
            self.lifelines.push(Primitive::GraphPath {
                edge_id: self.edge_id,
                corner_radius: 0.0,
                points: vec![
                    Point {
                        x: self.xs[index],
                        y: top,
                    },
                    Point {
                        x: self.xs[index],
                        y: bottom,
                    },
                ],
                source_range: participant.source_range.clone(),
                label_range: None,
                line: EdgeLine::Dotted,
                start: Marker::None,
                end: Marker::None,
                color: self.palette.lifeline,
                width: LIFELINE_WIDTH,
            });
            self.edge_id += 1;
        }
    }

    fn bar_half(depth: usize) -> f64 {
        if depth == 0 {
            0.0
        } else {
            5.0 + 3.0 * (depth - 1) as f64
        }
    }

    fn push_bar(&mut self, column: usize, depth: usize, top: f64, bottom: f64) {
        if depth == 0 {
            return;
        }
        self.bars.push(Primitive::RoundedRect {
            rect: Rect {
                x: self.xs[column] - 5.0 + 3.0 * (depth - 1) as f64,
                y: top,
                width: 10.0,
                height: (bottom - top).max(12.0),
            },
            radius: 3.0,
            color: self.palette.activation,
        });
    }

    fn display_head(&self, index: usize) -> String {
        let display = &self.diagram.participants[index].display;
        hard_lines(display)
            .first()
            .cloned()
            .unwrap_or_else(|| display.clone())
    }

    /// 普通消息文字的目标宽度：两端生命线之间减去激活条与留白。
    fn span_limit(&self, from: usize, to: usize) -> f64 {
        let (i, j) = if from <= to { (from, to) } else { (to, from) };
        (self.xs[j] - self.xs[i]) - 34.0
    }

    /// 自调用回环右侧文字的目标宽度。
    fn self_label_limit(&self, column: usize) -> f64 {
        let count = self.diagram.participants.len();
        let bound = if column + 1 < count {
            self.xs[column + 1]
        } else {
            self.width - 10.0
        };
        bound - self.xs[column] - SELF_LOOP_W - 8.0
    }

    /// 纵向主走：逐行放置消息、便签、激活与组合块边界，返回内容底部。
    fn walk(&mut self, top: f64) -> f64 {
        let diagram = self.diagram;
        let count = diagram.participants.len();
        let mut y = top;
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut frame_cursor = 0usize;
        let mut ordinal = 0usize;
        for row in &diagram.rows {
            match row {
                SequenceRow::Message(message) => {
                    ordinal += 1;
                    let number = diagram
                        .autonumber
                        .then(|| format!("{ordinal}. "))
                        .map(|prefix| (prefix, self.muted_style()));
                    let label = wrap_text(
                        &message.text.text,
                        number
                            .as_ref()
                            .map(|(prefix, style)| (prefix.as_str(), *style)),
                        self.msg_style(),
                        if message.from == message.to {
                            self.self_label_limit(message.from)
                        } else {
                            self.span_limit(message.from, message.to)
                        },
                        TEXT_LINE,
                        self.fonts,
                    );
                    let self_loop = message.from == message.to;
                    let from_depth = self.activation[message.from].len();
                    let to_depth =
                        self.activation[message.to].len() + usize::from(message.activate_to);
                    let content_h = if self_loop {
                        label.height.max(SELF_LOOP_H + 8.0)
                    } else {
                        label.height
                    };
                    let arrow_y = if self_loop {
                        y + (content_h - SELF_LOOP_H) / 2.0 + 6.0
                    } else {
                        y + label.height + 10.0
                    };
                    let color = if message.line == EdgeLine::Dotted {
                        self.palette.edge_reply
                    } else {
                        self.palette.edge
                    };
                    let points = if self_loop {
                        let start = self.xs[message.from] + Self::bar_half(from_depth) + 2.0;
                        vec![
                            Point {
                                x: start,
                                y: arrow_y,
                            },
                            Point {
                                x: start + SELF_LOOP_W,
                                y: arrow_y,
                            },
                            Point {
                                x: start + SELF_LOOP_W,
                                y: arrow_y + SELF_LOOP_H,
                            },
                            Point {
                                x: self.xs[message.from] + 2.0,
                                y: arrow_y + SELF_LOOP_H,
                            },
                        ]
                    } else {
                        let rightward = message.to > message.from;
                        let offset_from = Self::bar_half(from_depth) + 1.0;
                        let offset_to = Self::bar_half(to_depth) + 1.0;
                        let (from_x, to_x) = if rightward {
                            (
                                self.xs[message.from] + offset_from,
                                self.xs[message.to] - offset_to,
                            )
                        } else {
                            (
                                self.xs[message.from] - offset_from,
                                self.xs[message.to] + offset_to,
                            )
                        };
                        vec![
                            Point {
                                x: from_x,
                                y: arrow_y,
                            },
                            Point {
                                x: to_x,
                                y: arrow_y,
                            },
                        ]
                    };
                    self.arrows.push(Primitive::GraphPath {
                        edge_id: self.edge_id,
                        corner_radius: if self_loop { 8.0 } else { 0.0 },
                        points: points.clone(),
                        source_range: message.source_range.clone(),
                        label_range: None,
                        line: message.line,
                        start: Marker::None,
                        end: message.marker,
                        color,
                        width: EDGE_WIDTH,
                    });
                    let hit_rect = if self_loop {
                        Rect {
                            x: points[0].x,
                            y: arrow_y - 4.0,
                            width: SELF_LOOP_W + 12.0 + label.width(),
                            height: content_h + 8.0,
                        }
                    } else if label.empty() {
                        let lo = points[0].x.min(points[1].x);
                        Rect {
                            x: lo,
                            y: arrow_y - 11.0,
                            width: (points[1].x - points[0].x).abs(),
                            height: 22.0,
                        }
                    } else {
                        Rect {
                            x: (points[0].x + points[1].x) / 2.0 - label.width() / 2.0 - 6.0,
                            y: arrow_y - 8.0 - label.height,
                            width: label.width() + 12.0,
                            height: label.height + 4.0,
                        }
                    };
                    // 消息文字：自调用在回环右侧居中，其余在箭线上方居中。
                    if self_loop {
                        let label_top = arrow_y + SELF_LOOP_H / 2.0 - label.height / 2.0;
                        emit_block(
                            &mut self.texts,
                            &label,
                            Anchor::Left(points[1].x + 10.0),
                            label_top,
                            TEXT_LINE,
                        );
                    } else if !label.empty() {
                        let center = (points[0].x + points[1].x) / 2.0;
                        let label_top = arrow_y - 6.0 - label.height;
                        // 文字遮挡穿过的中间生命线。
                        let lo = points[0].x.min(points[1].x);
                        let hi = points[0].x.max(points[1].x);
                        let crossing = (0..count).any(|m| {
                            m != message.from
                                && m != message.to
                                && self.xs[m] > lo + 1.0
                                && self.xs[m] < hi - 1.0
                        });
                        if crossing {
                            self.label_bgs.push(Primitive::LabelBackground {
                                edge_id: self.edge_id,
                                rect: Rect {
                                    x: center - label.width() / 2.0 - 6.0,
                                    y: label_top - 2.0,
                                    width: label.width() + 12.0,
                                    height: label.height + 4.0,
                                },
                                color: self.palette.background,
                            });
                        }
                        emit_block(
                            &mut self.texts,
                            &label,
                            Anchor::Center(center),
                            label_top,
                            TEXT_LINE,
                        );
                    }
                    let from_display = self.display_head(message.from);
                    let to_display = self.display_head(message.to);
                    self.message_ordinal += 1;
                    self.hits.push(HitRegion {
                        node_id: format!("msg-{}", self.message_ordinal),
                        label: message.text.text.clone(),
                        prefix: Some(format!("{from_display} → {to_display}")),
                        rect: hit_rect,
                        can_collapse: false,
                        collapsed: false,
                        source_range: message.source_range.clone(),
                    });
                    self.edge_id += 1;
                    // 激活简写：`-` 关发送方，`+` 开接收方。
                    if message.deactivate_from
                        && let Some(bar_top) = self.activation[message.from].pop()
                    {
                        self.push_bar(message.from, from_depth, bar_top, arrow_y + 6.0);
                    }
                    if message.activate_to {
                        self.activation[message.to].push(arrow_y - 8.0);
                    }
                    y += if self_loop {
                        content_h + 24.0
                    } else {
                        label.height + 32.0
                    };
                }
                SequenceRow::Note(note) => {
                    let (rect, box_w) = match &note.side {
                        NoteSide::LeftOf(c) => {
                            let cap = self.xs[*c] - 10.0 - 10.0;
                            let w = (single_width(&note.text.text, self.fonts) + NOTE_PAD_X * 2.0)
                                .min(NOTE_MAX_W.max(cap.max(80.0)));
                            (
                                Rect {
                                    x: self.xs[*c] - 10.0 - w,
                                    y: y + 2.0,
                                    width: w,
                                    height: 0.0,
                                },
                                w,
                            )
                        }
                        NoteSide::RightOf(c) => {
                            let cap = self.width - 10.0 - (self.xs[*c] + 10.0);
                            let w = (single_width(&note.text.text, self.fonts) + NOTE_PAD_X * 2.0)
                                .min(NOTE_MAX_W.max(cap.max(80.0)));
                            (
                                Rect {
                                    x: self.xs[*c] + 10.0,
                                    y: y + 2.0,
                                    width: w,
                                    height: 0.0,
                                },
                                w,
                            )
                        }
                        NoteSide::Over(cols) => {
                            let mut sorted = cols.clone();
                            sorted.sort_unstable();
                            let (a, b) = (sorted[0], sorted[sorted.len() - 1]);
                            if a == b {
                                let left = if a > 0 {
                                    self.xs[a] - self.xs[a - 1]
                                } else {
                                    self.xs[a] - 10.0
                                };
                                let right = if a + 1 < count {
                                    self.xs[a + 1] - self.xs[a]
                                } else {
                                    self.width - 10.0 - self.xs[a]
                                };
                                let room = left.min(right) - 10.0;
                                let w = (single_width(&note.text.text, self.fonts)
                                    + NOTE_PAD_X * 2.0)
                                    .min((room * 2.0).clamp(96.0, 300.0));
                                (
                                    Rect {
                                        x: self.xs[a] - w / 2.0,
                                        y: y + 2.0,
                                        width: w,
                                        height: 0.0,
                                    },
                                    w,
                                )
                            } else {
                                let w = self.xs[b] - self.xs[a] + FRAGMENT_PAD_X * 2.0;
                                (
                                    Rect {
                                        x: self.xs[a] - FRAGMENT_PAD_X,
                                        y: y + 2.0,
                                        width: w,
                                        height: 0.0,
                                    },
                                    w,
                                )
                            }
                        }
                    };
                    let label = wrap_text(
                        &note.text.text,
                        None,
                        self.msg_style(),
                        box_w - NOTE_PAD_X * 2.0,
                        TEXT_LINE,
                        self.fonts,
                    );
                    let height = label.height + NOTE_PAD_Y * 2.0;
                    let rect = Rect { height, ..rect };
                    self.notes.push(Primitive::GraphShape {
                        rect,
                        shape: Shape::Rounded,
                        outline: crate::shape::outline(Shape::Rounded, rect),
                        details: Vec::new(),
                        fill: self.palette.note_fill,
                        stroke: self.palette.note_stroke,
                        stroke_width: 1.1,
                    });
                    emit_block(
                        &mut self.texts,
                        &label,
                        Anchor::Left(rect.x + NOTE_PAD_X),
                        rect.y + NOTE_PAD_Y,
                        TEXT_LINE,
                    );
                    self.note_ordinal += 1;
                    self.hits.push(HitRegion {
                        node_id: format!("note-{}", self.note_ordinal),
                        label: note.text.text.clone(),
                        prefix: None,
                        rect,
                        can_collapse: false,
                        collapsed: false,
                        source_range: note.source_range.clone(),
                    });
                    y += height + 18.0;
                }
                SequenceRow::Activate { participant, .. } => {
                    let center = y + 12.0;
                    self.activation[*participant].push(center - 6.0);
                    y += 24.0;
                }
                SequenceRow::Deactivate { participant, .. } => {
                    let center = y + 12.0;
                    let depth = self.activation[*participant].len();
                    if let Some(bar_top) = self.activation[*participant].pop() {
                        self.push_bar(*participant, depth, bar_top, center + 6.0);
                    }
                    y += 24.0;
                }
                SequenceRow::FragmentStart { .. } => {
                    let index = frame_cursor;
                    frame_cursor += 1;
                    let meta = &mut self.frames[index];
                    meta.top = y;
                    meta.label_top = y + 8.0;
                    let label_h = meta.label.height.max(TEXT_LINE);
                    y += label_h + 16.0;
                    open.push((index, 0));
                }
                SequenceRow::FragmentDivider { .. } => {
                    if let Some((frame, at)) = open.last_mut() {
                        let frame = *frame;
                        let divider = *at;
                        *at += 1;
                        let label_h = self.frames[frame].dividers[divider].height.max(TEXT_LINE);
                        let label_top = y + 2.0;
                        let line_y = y + 2.0 + label_h + 6.0;
                        self.frames[frame].divider_slots[divider] = (label_top, line_y);
                        y += label_h + 18.0;
                    }
                }
                SequenceRow::FragmentEnd { .. } => {
                    if let Some((frame, _)) = open.pop() {
                        self.frames[frame].bottom = y + 4.0;
                    }
                    y += 14.0;
                }
            }
        }
        // 解析器允许图尾留激活：未闭合的激活条画到内容底部。
        let bottom = y + 6.0;
        let stacks = std::mem::take(&mut self.activation);
        for (column, stack) in stacks.into_iter().enumerate() {
            for (level, bar_top) in stack.into_iter().enumerate() {
                self.push_bar(column, level + 1, bar_top, bottom);
            }
        }
        y
    }
}

pub fn layout_sequence(
    diagram: &SequenceDiagram,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<Scene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if diagram.participants.is_empty() {
        return Err(LayoutError("Sequence has no participants".into()));
    }
    let palette = Palette::new(options.theme);
    let width = options.width;
    let count = diagram.participants.len();
    let mut pills = plan_pills(diagram, &palette, None, fonts);

    // 1. 间隙需求：胶囊基础间距 + 消息与自调用回环的单行宽度。
    let base: Vec<f64> = (0..count - 1)
        .map(|i| pills[i].width / 2.0 + pills[i + 1].width / 2.0 + GAP_MIN)
        .collect();
    let mut need = base.clone();
    let mut extra_left = 0.0f64;
    let mut extra_right = 0.0f64;
    let mut ordinal = 0usize;
    for row in &diagram.rows {
        let SequenceRow::Message(message) = row else {
            continue;
        };
        let mut w = single_width(&message.text.text, fonts) + 28.0;
        if diagram.autonumber {
            ordinal += 1;
            w += fonts
                .measure(&format!("{ordinal}. "), TEXT_FONT, TEXT_WEIGHT)
                .width;
        }
        let (i, j) = if message.from <= message.to {
            (message.from, message.to)
        } else {
            (message.to, message.from)
        };
        if i == j {
            let demand = w + SELF_LOOP_W + 24.0;
            if i < count - 1 {
                need[i] = need[i].max(demand);
            } else {
                extra_right = extra_right.max(w + SELF_LOOP_W + 10.0);
            }
        } else {
            let span: f64 = need[i..j].iter().sum();
            if span < w {
                let add = (w - span) / (j - i) as f64;
                for gap in &mut need[i..j] {
                    *gap += add;
                }
            }
        }
    }
    // 便签需求：左右伸出与跨列都要放得下。
    for row in &diagram.rows {
        let SequenceRow::Note(note) = row else {
            continue;
        };
        let w = single_width(&note.text.text, fonts) + NOTE_PAD_X * 2.0;
        match &note.side {
            NoteSide::LeftOf(c) => {
                if *c > 0 {
                    need[*c - 1] = need[*c - 1].max(w + 30.0);
                } else {
                    extra_left = extra_left.max(w + 16.0);
                }
            }
            NoteSide::RightOf(c) => {
                if *c + 1 < count {
                    need[*c] = need[*c].max(w + 30.0);
                } else {
                    extra_right = extra_right.max(w + 16.0);
                }
            }
            NoteSide::Over(cols) => {
                let mut sorted = cols.clone();
                sorted.sort_unstable();
                let (a, b) = (sorted[0], sorted[sorted.len() - 1]);
                if a == b {
                    let half = w / 2.0 + NOTE_PAD_X;
                    if a > 0 {
                        need[a - 1] = need[a - 1].max(half + 20.0);
                    } else {
                        extra_left = extra_left.max(half + 8.0);
                    }
                    if a + 1 < count {
                        need[a] = need[a].max(half + 20.0);
                    } else {
                        extra_right = extra_right.max(half + 8.0);
                    }
                } else {
                    let span: f64 = need[a..b].iter().sum();
                    let want = w + FRAGMENT_PAD_X * 2.0;
                    if span < want {
                        let add = (want - span) / (b - a) as f64;
                        for gap in &mut need[a..b] {
                            *gap += add;
                        }
                    }
                }
            }
        }
    }

    // 2. 宽度结算：胶囊基础间距超宽时先换行参与者标签；仍超宽只压缩消息撑出的增量。
    let total_of = |pills: &[Pill], gaps: &[f64]| {
        SIDE + extra_left
            + pills[0].width / 2.0
            + gaps.iter().sum::<f64>()
            + pills[count - 1].width / 2.0
            + SIDE
            + extra_right
    };
    // 胶囊基础间距超宽时先换行参与者标签，再用新胶囊重算基础列距。
    let mut base = base;
    if total_of(&pills, &base) > width {
        let target =
            ((width - SIDE * 2.0 - extra_left - extra_right - GAP_MIN * (count - 1) as f64)
                / count as f64)
                .max(56.0);
        pills = plan_pills(diagram, &palette, Some(target), fonts);
        base = (0..count - 1)
            .map(|i| pills[i].width / 2.0 + pills[i + 1].width / 2.0 + GAP_MIN)
            .collect();
        for (gap, pill_gap) in need.iter_mut().zip(&base) {
            *gap = (*gap).max(*pill_gap);
        }
    }
    let mut overflow = false;
    let mut total = total_of(&pills, &need);
    if total > width {
        // 只压缩消息/便签撑出的增量，长文字随后按最终列宽换行。
        let excess: Vec<f64> = need
            .iter()
            .zip(&base)
            .map(|(need, base)| (*need - base).max(0.0))
            .collect();
        let excess_total: f64 = excess.iter().sum();
        let base_total = total - excess_total;
        if excess_total > 0.0 && base_total < width - 0.01 {
            let scale = ((width - base_total) / excess_total).min(1.0);
            for (gap, excess) in need.iter_mut().zip(excess) {
                *gap -= excess * (1.0 - scale);
            }
            total = total_of(&pills, &need);
        }
    }
    // 压缩后的总和按定义恰好等于宽度；吸收浮点误差，只有真实放不下才标记溢出。
    if total > width + 0.01 {
        overflow = true;
    } else if !need.is_empty() {
        let slack = (width - total)
            .max(0.0)
            .min(need.iter().sum::<f64>() * GAP_SLACK_MAX);
        let add = slack / need.len() as f64;
        for gap in &mut need {
            *gap += add;
        }
    }

    // 3. 生命线坐标。
    let mut xs = Vec::with_capacity(count);
    let mut x = SIDE + extra_left + pills[0].width / 2.0;
    xs.push(x);
    for gap in &need {
        x += *gap;
        xs.push(x);
    }

    // 4. 组合块水平范围与标签折行。
    let frames = plan_frames(diagram, &palette, &xs, &pills, width, count, fonts);

    let mut layout = Layout {
        diagram,
        palette,
        width,
        fonts,
        pills,
        xs,
        frames,
        pills_drawn: Vec::new(),
        fragment_shapes: Vec::new(),
        fragment_lines: Vec::new(),
        lifelines: Vec::new(),
        bars: Vec::new(),
        notes: Vec::new(),
        arrows: Vec::new(),
        label_bgs: Vec::new(),
        texts: Vec::new(),
        hits: Vec::new(),
        activation: vec![Vec::new(); count],
        edge_id: 0,
        message_ordinal: 0,
        note_ordinal: 0,
    };

    // 5. 标题、表头与主体。
    let title_style = Style {
        size: TITLE_FONT,
        weight: 650,
        color: layout.palette.text,
    };
    let title = diagram
        .title
        .as_ref()
        .filter(|title| !title.text.is_empty())
        .map(|title| {
            wrap_text(
                &title.text,
                None,
                title_style,
                width - SIDE * 2.0,
                TITLE_LINE,
                layout.fonts,
            )
        });
    let title_height = title.as_ref().map_or(0.0, |block| block.height + 8.0);
    let header_top = 18.0 + title_height;
    let band = band_height(&layout.pills);
    if let Some(title) = &title {
        emit_block(
            &mut layout.texts,
            title,
            Anchor::Center(width / 2.0),
            18.0,
            TITLE_LINE,
        );
    }
    layout.emit_pills(header_top);
    let lifeline_top = header_top + band + 2.0;
    let rows_top = header_top + band + 18.0;
    let rows_bottom = layout.walk(rows_top).max(lifeline_top + 44.0);

    // 6. 底部镜像与生命线。
    let mirror = rows_bottom - header_top > MIRROR_MIN_HEIGHT;
    let height = if mirror {
        let mirror_top = rows_bottom + 14.0;
        layout.emit_lifelines(lifeline_top, mirror_top - 4.0);
        layout.emit_pills(mirror_top);
        mirror_top + band + 16.0
    } else {
        layout.emit_lifelines(lifeline_top, rows_bottom + 4.0);
        rows_bottom + 18.0
    };

    // 7. 组合块绘制：块底在形状之上、生命线之下。
    let frames = &layout.frames;
    for meta in frames {
        let rect = Rect {
            x: meta.x0,
            y: meta.top,
            width: meta.x1 - meta.x0,
            height: (meta.bottom - meta.top).max(24.0),
        };
        let fill = meta
            .fill
            .map(|fill| Color { a: 0.32, ..fill })
            .unwrap_or(layout.palette.fragment_fill);
        layout.fragment_shapes.push(Primitive::GraphShape {
            rect,
            shape: Shape::Rounded,
            outline: crate::shape::outline(Shape::Rounded, rect),
            details: Vec::new(),
            fill,
            stroke: layout.palette.fragment_stroke,
            stroke_width: 1.1,
        });
        emit_block(
            &mut layout.texts,
            &meta.label,
            Anchor::Left(meta.x0 + 10.0),
            meta.label_top,
            TEXT_LINE,
        );
        for (index, (label_top, line_y)) in meta.divider_slots.iter().enumerate() {
            layout.fragment_lines.push(Primitive::GraphPath {
                edge_id: layout.edge_id,
                corner_radius: 0.0,
                points: vec![
                    Point {
                        x: meta.x0 + 8.0,
                        y: *line_y,
                    },
                    Point {
                        x: meta.x1 - 8.0,
                        y: *line_y,
                    },
                ],
                source_range: meta.source_range.clone(),
                label_range: None,
                line: EdgeLine::Dotted,
                start: Marker::None,
                end: Marker::None,
                color: layout.palette.fragment_stroke,
                width: 1.1,
            });
            layout.edge_id += 1;
            emit_block(
                &mut layout.texts,
                &meta.dividers[index],
                Anchor::Left(meta.x0 + 10.0),
                *label_top,
                TEXT_LINE,
            );
        }
    }

    // 8. 组装：背景 → 组合块 → 胶囊 → 生命线 → 激活条 → 便签 → 消息 → 遮底 → 文字。
    let mut primitives = Vec::with_capacity(layout.texts.len() + 64);
    primitives.push(Primitive::RoundedRect {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        radius: 16.0,
        color: layout.palette.background,
    });
    primitives.append(&mut layout.fragment_shapes);
    primitives.append(&mut layout.pills_drawn);
    primitives.append(&mut layout.lifelines);
    primitives.append(&mut layout.bars);
    primitives.append(&mut layout.notes);
    primitives.append(&mut layout.arrows);
    primitives.append(&mut layout.fragment_lines);
    primitives.append(&mut layout.label_bgs);
    primitives.append(&mut layout.texts);

    let horizontal_overflow = overflow
        || primitives.iter().any(|primitive| match primitive {
            Primitive::Text(span) => {
                span.baseline.x < -0.001 || span.baseline.x + span.width > width + 0.001
            }
            Primitive::RoundedRect { rect, .. } => {
                rect.x < -0.001 || rect.x + rect.width > width + 0.001
            }
            Primitive::GraphShape { rect, .. } | Primitive::LabelBackground { rect, .. } => {
                rect.x < -0.001 || rect.x + rect.width > width + 0.001
            }
            Primitive::GraphPath {
                points,
                width: line_width,
                ..
            } => points.iter().any(|point| {
                point.x - line_width / 2.0 < -0.001 || point.x + line_width / 2.0 > width + 0.001
            }),
            Primitive::Line {
                from,
                to,
                width: line_width,
                ..
            } => {
                from.x.min(to.x) - line_width / 2.0 < -0.001
                    || from.x.max(to.x) + line_width / 2.0 > width + 0.001
            }
        });
    let min_font_px = primitives
        .iter()
        .filter_map(|primitive| {
            if let Primitive::Text(text) = primitive {
                Some(text.font_size)
            } else {
                None
            }
        })
        .fold(21.0, f64::min);
    // 命中区域按纵向起点扫描，避免 O(n²) 的重叠检查。
    let hits = &layout.hits;
    let mut ordered = (0..hits.len()).collect::<Vec<_>>();
    ordered.sort_by(|&a, &b| hits[a].rect.y.total_cmp(&hits[b].rect.y));
    let mut active: Vec<usize> = Vec::new();
    let mut node_overlaps = 0;
    for index in ordered {
        let rect = hits[index].rect;
        active.retain(|&other| rect.y < hits[other].rect.y + hits[other].rect.height - 0.001);
        node_overlaps += active
            .iter()
            .filter(|&&other| {
                let other = hits[other].rect;
                rect.x < other.x + other.width - 0.001 && other.x < rect.x + rect.width - 0.001
            })
            .count();
        active.push(index);
    }
    let fonts_used: Vec<FontInfo> = layout.fonts.used_fonts();
    Ok(Scene {
        width,
        height,
        primitives,
        hit_regions: layout.hits,
        metrics: Metrics {
            columns: 1,
            diagram_height: height,
            min_font_px,
            horizontal_overflow,
            node_overlaps,
            fonts: fonts_used,
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue: None,
            branch_hues: Vec::new(),
            diagram_kind: "sequence".into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::sequence::parse_sequence;
    use crate::{DeterministicMetrics, LayoutOptions, Theme};

    fn lifelines(scene: &crate::Scene) -> Vec<f64> {
        let mut xs = scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::GraphPath {
                    line: crate::EdgeLine::Dotted,
                    points,
                    ..
                } if points.len() == 2 && points[0].x == points[1].x => Some(points[0].x),
                _ => None,
            })
            .collect::<Vec<_>>();
        xs.sort_by(f64::total_cmp);
        xs
    }

    fn scene(source: &str, width: f64, theme: Theme) -> crate::Scene {
        let diagram = parse_sequence(source).unwrap();
        crate::layout_sequence(
            &diagram,
            &LayoutOptions {
                width,
                theme,
                ..LayoutOptions::default()
            },
            &mut DeterministicMetrics,
        )
        .unwrap()
    }

    fn text_spans(scene: &crate::Scene) -> Vec<&crate::TextSpan> {
        scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::Text(span) => Some(span),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn agent_workflow_lays_out_readably_at_both_widths() {
        let source = concat!(
            "sequenceDiagram\n",
            "  participant U as 你\n",
            "  participant H as Hortus\n",
            "  participant T as 终端 agent\n",
            "  participant C as hortus CLI\n",
            "  U->>H: 发送当前笔记\n",
            "  H->>T: 填入提示词和笔记路径\n",
            "  T->>C: search 缓存策略\n",
            "  C-->>T: 12 条结果\n",
            "  T->>C: backlinks 04-架构笔记\n",
            "  C-->>T: 5 条反链\n",
            "  T->>T: 整理成 mindmap\n",
            "  T->>C: mermaid check\n",
            "  C-->>T: 语法通过\n",
            "  T->>H: 写入 总结.md\n",
            "  H-->>U: 刷新阅读视图，列出本次改动\n",
        );
        for width in [700.0, 1100.0] {
            let scene = scene(source, width, Theme::Light);
            assert_eq!(scene.width, width);
            assert!(!scene.metrics.horizontal_overflow, "{width}");
            assert_eq!(scene.metrics.node_overlaps, 0);
            assert_eq!(scene.metrics.columns, 1);
            assert!(scene.metrics.min_font_px >= 13.6);
            assert_eq!(lifelines(&scene).len(), 4);
            // 4 顶部胶囊 + 11 条消息 + 高图底部镜像 4 胶囊。
            assert_eq!(scene.hit_regions.len(), 19);
            for hit in &scene.hit_regions {
                assert!(!hit.can_collapse);
                assert!(hit.rect.x >= -0.001 && hit.rect.x + hit.rect.width <= width + 0.001);
            }
            for span in text_spans(&scene) {
                assert!(span.baseline.x >= -0.001, "{:?}", span.baseline);
                assert!(span.baseline.x + span.width <= width + 0.001, "{span:?}");
            }
        }
    }

    #[test]
    fn long_labels_wrap_within_target_spans_without_overflow() {
        let source = concat!(
            "sequenceDiagram\n",
            "participant R as 阅读视图\n",
            "participant I as 索引与检索\n",
            "participant M as 排序模型\n",
            "R->>I: 检索分布式任务队列两阶段提交与幂等重试机制在离线场景下的状态流转\n",
            "I->>M: 对十二条候选结果按主题相关性与最近编辑时间重新排序\n",
            "M-->>R: 返回前五条并附上每条的匹配度说明文字供阅读视图展示\n",
        );
        let scene = scene(source, 700.0, Theme::Light);
        assert!(!scene.metrics.horizontal_overflow);
        assert_eq!(scene.metrics.node_overlaps, 0);
        let xs = lifelines(&scene);
        let rows: Vec<f64> = scene
            .hit_regions
            .iter()
            .filter(|hit| hit.node_id.starts_with("msg-"))
            .map(|hit| hit.rect.height)
            .collect();
        assert_eq!(rows.len(), 3);
        // 前两条跨相邻列的长消息必须折行；跨全图的第三条单行放得下。
        assert!(
            rows.iter().filter(|height| **height > 22.0).count() >= 2,
            "{rows:?}"
        );
        for hit in &scene.hit_regions {
            if hit.node_id.starts_with("msg-") {
                assert!(hit.rect.x + hit.rect.width <= xs[2] + 200.0);
            }
        }
    }

    #[test]
    fn self_call_uses_orthogonal_loop_with_side_label() {
        let source = "sequenceDiagram\nparticipant A\nA->>A: 整理成 mindmap\n";
        let scene = scene(source, 700.0, Theme::Light);
        let path = scene
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                crate::Primitive::GraphPath { points, .. } if points.len() == 4 => {
                    Some(points.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(path[1].x - path[0].x, 30.0);
        assert_eq!(path[2].y - path[1].y, 24.0);
        assert!(path[3].x < path[2].x);
        let label = text_spans(&scene)
            .into_iter()
            .find(|span| span.text.contains("mindmap"))
            .unwrap();
        assert!(label.baseline.x > path[1].x + 8.0);
    }

    #[test]
    fn activations_render_nested_bars_attached_to_edges() {
        let source = concat!(
            "sequenceDiagram\n",
            "A->>+B: 请求\n",
            "B->>+B: 自处理\n",
            "B-->>-A: 响应\n",
        );
        let scene = scene(source, 700.0, Theme::Dark);
        let bars: Vec<crate::Rect> = scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::RoundedRect { rect, radius, .. } if *radius == 3.0 => Some(*rect),
                _ => None,
            })
            .collect();
        // B 两层激活未闭合到图尾：至少两条嵌套条，右移 3px。
        assert!(bars.len() >= 2, "{bars:?}");
        let sorted = {
            let mut values = bars.iter().map(|bar| bar.x).collect::<Vec<_>>();
            values.sort_by(f64::total_cmp);
            values
        };
        assert!(sorted.windows(2).all(|pair| pair[1] >= pair[0]));
        let xs = lifelines(&scene);
        for bar in &bars {
            assert!(bar.x + bar.width / 2.0 <= xs[1] + 12.0);
            assert!(bar.x + bar.width / 2.0 >= xs[1] - 6.0);
        }
    }

    #[test]
    fn notes_take_left_right_and_over_positions() {
        let source = concat!(
            "sequenceDiagram\n",
            "participant A\n",
            "participant B\n",
            "Note left of A: 左侧\n",
            "Note right of B: 右侧\n",
            "Note over A: 单列<br/>第二行\n",
            "Note over A,B: 跨列\n",
        );
        let scene = scene(source, 700.0, Theme::Light);
        let xs = lifelines(&scene);
        let notes: Vec<crate::Rect> = scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::GraphShape {
                    shape: crate::Shape::Rounded,
                    rect,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(notes.len(), 4);
        assert!(notes[0].x + notes[0].width <= xs[0] - 8.0);
        assert!(notes[1].x >= xs[1] + 8.0);
        let over_single = notes[2];
        assert!(over_single.x <= xs[0] && over_single.x + over_single.width >= xs[0]);
        assert_eq!(notes[3].x, xs[0] - 12.0);
        assert!(notes[2].height > notes[3].height);
        assert_eq!(
            scene
                .hit_regions
                .iter()
                .filter(|hit| hit.node_id.starts_with("note-"))
                .count(),
            4
        );
    }

    #[test]
    fn fragments_span_referenced_columns_with_dividers() {
        let source = concat!(
            "sequenceDiagram\n",
            "participant A\n",
            "participant B\n",
            "participant C\n",
            "loop 每次空闲\n",
            "  A->>C: 合并远端变更\n",
            "end\n",
            "alt 可用\n",
            "  A->>B: 主线\n",
            "else 不可用\n",
            "  B-->>A: 降级\n",
            "end\n",
        );
        let scene = scene(source, 700.0, Theme::Light);
        let xs = lifelines(&scene);
        let frames: Vec<crate::Rect> = scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::GraphShape {
                    shape: crate::Shape::Rounded,
                    rect,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        // 组合块按 GraphShape 收集时也包含参与者胶囊以外的便签形状；
        // 本样例只有两个组合块，帧高至少 40 且宽覆盖受影响列。
        assert!(frames.len() >= 2, "{frames:?}");
        let outer = frames[0];
        assert!(outer.x <= xs[0] + 1.0);
        assert!(outer.x + outer.width >= xs[2] - 1.0);
        let inner = frames[1];
        assert!(inner.x <= xs[0] + 1.0);
        assert!(inner.x + inner.width >= xs[1] - 1.0);
        let dividers: Vec<f64> = scene
            .primitives
            .iter()
            .filter_map(|primitive| match primitive {
                crate::Primitive::GraphPath {
                    points,
                    line,
                    width,
                    ..
                } if points.len() == 2
                    && points[0].y == points[1].y
                    && points[0].y > 100.0
                    && *line == crate::EdgeLine::Dotted
                    && *width == 1.1 =>
                {
                    Some(points[0].y)
                }
                _ => None,
            })
            .collect();
        assert_eq!(dividers.len(), 1);
        let label = text_spans(&scene)
            .into_iter()
            .find(|span| span.text.contains("else"))
            .unwrap();
        assert!(label.baseline.y < dividers[0]);
    }

    #[test]
    fn title_renders_centered_and_autonumber_prefixes_messages() {
        let source = concat!(
            "sequenceDiagram\n",
            "autonumber\n",
            "title 协作流程\n",
            "A->>B: 请求\n",
            "B-->>A: 响应\n",
        );
        let scene = scene(source, 700.0, Theme::Light);
        let title = text_spans(&scene)
            .into_iter()
            .find(|span| span.text.contains("协作流程"))
            .unwrap();
        assert!(title.baseline.x > 200.0, "{title:?}");
        let first = text_spans(&scene)
            .into_iter()
            .find(|span| span.text.starts_with("1. "))
            .unwrap();
        assert!(first.baseline.y > title.baseline.y);
    }

    #[test]
    fn theme_changes_colors_but_not_geometry() {
        let source = "sequenceDiagram\nA->>B: 消息\nB-->>A: 回复\n";
        let light = scene(source, 700.0, Theme::Light);
        let dark = scene(source, 700.0, Theme::Dark);
        assert_eq!(light.height, dark.height);
        assert_eq!(light.hit_regions, dark.hit_regions);
        let edge = |scene: &crate::Scene| {
            scene
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    crate::Primitive::GraphPath { color, .. } => Some(*color),
                    _ => None,
                })
                .unwrap()
        };
        assert_ne!(edge(&light), edge(&dark));
    }

    #[test]
    fn layouts_are_deterministic_across_runs() {
        let source = "sequenceDiagram\nparticipant A as 应用\nB->>A: 请求\nNote over A,B: 说明\nloop 循环\nA->>B: 步骤\nend\n";
        let first = scene(source, 700.0, Theme::Light);
        let second = scene(source, 700.0, Theme::Light);
        assert_eq!(first.semantic_scene(), second.semantic_scene());
    }

    #[test]
    fn very_long_single_word_emergency_wraps_without_overflow() {
        let source = "sequenceDiagram\nA->>B: 超级无敌长的一整段没有任何空格与标点的中文文本需要按字素强制折行才能放下不溢出画布宽度限制\n";
        let scene = scene(source, 320.0, Theme::Light);
        assert!(!scene.metrics.horizontal_overflow);
        for span in text_spans(&scene) {
            assert!(span.baseline.x + span.width <= 320.001, "{span:?}");
        }
    }

    #[test]
    fn svg_export_reflects_the_sequence_kind() {
        let source = "sequenceDiagram\nA->>B: 消息\n";
        let scene = scene(source, 700.0, Theme::Light);
        let svg = scene.to_svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("aria-label=\"Mermaid sequence diagram\""));
        assert!(svg.contains("stroke-dasharray"));
    }

    #[test]
    fn actors_use_person_details_and_participants_use_stadiums() {
        let source = "sequenceDiagram\nactor U as 用户\nparticipant S as 服务\nU->>S: 请求\n";
        let scene = scene(source, 700.0, Theme::Light);
        let mut stadium_details = 0;
        let mut actor_details = 0;
        for primitive in &scene.primitives {
            if let crate::Primitive::GraphShape { shape, details, .. } = primitive {
                if *shape == crate::Shape::Stadium {
                    stadium_details += details.len();
                } else if *shape == crate::Shape::Rounded && !details.is_empty() {
                    actor_details += details.len();
                }
            }
        }
        assert_eq!(stadium_details, 0);
        assert_eq!(actor_details, 2);
    }
}
