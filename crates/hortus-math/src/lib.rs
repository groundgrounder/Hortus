//! 公式排版适配层；公开场景和错误不暴露排版库的类型。
use ratex_layout::{
    FallbackGlyphMetrics, LayoutOptions, layout_with_fallback_metrics, to_display_list,
    validate_layout,
};
use ratex_types::{
    color::Color as RaColor, display_item::DisplayItem, math_style::MathStyle,
    path_command::PathCommand,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

mod katex_cmap;

pub const MAX_SOURCE_BYTES: usize = 16_384;
pub const MAX_ITEMS: usize = 32_768;
const MAX_COMMANDS: usize = 131_072;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_GEOMETRY_EM: f64 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Style {
    Display,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphMetrics {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub foreground: bool,
    pub rgba: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Command {
    MoveTo {
        x: f64,
        y: f64,
    },
    LineTo {
        x: f64,
        y: f64,
    },
    CubicTo {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    QuadTo {
        x1: f64,
        y1: f64,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Item {
    Glyph {
        x: f64,
        y: f64,
        scale: f64,
        font: String,
        codepoint: u32,
        fallback: bool,
        color: Color,
    },
    Line {
        x: f64,
        y: f64,
        width: f64,
        thickness: f64,
        dashed: bool,
        color: Color,
    },
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: Color,
    },
    Path {
        x: f64,
        y: f64,
        commands: Vec<Command>,
        fill: bool,
        color: Color,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub items: Vec<Item>,
    pub missing_metrics: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Parse(String),
    ResourceLimit(String),
    Panic(String),
    InvalidLayout(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(s) | Self::ResourceLimit(s) | Self::Panic(s) | Self::InvalidLayout(s) => {
                f.write_str(s)
            }
        }
    }
}
impl std::error::Error for Error {}

fn finite(values: &[f64]) -> Result<(), Error> {
    if values.iter().any(|value| !value.is_finite()) {
        Err(Error::InvalidLayout("Non-finite formula geometry".into()))
    } else if values.iter().any(|value| value.abs() > MAX_GEOMETRY_EM) {
        Err(Error::ResourceLimit(
            "Formula resource limit exceeded: geometry is above 4096 em".into(),
        ))
    } else {
        Ok(())
    }
}

fn layout_error(error: impl fmt::Display) -> Error {
    let message = error.to_string();
    if message.to_ascii_lowercase().contains("resource limit") {
        Error::ResourceLimit(message)
    } else {
        Error::InvalidLayout(message)
    }
}
fn color(item: &DisplayItem) -> RaColor {
    match item {
        DisplayItem::GlyphPath { color, .. }
        | DisplayItem::Line { color, .. }
        | DisplayItem::Rect { color, .. }
        | DisplayItem::Path { color, .. } => *color,
    }
}
fn convert_command(command: PathCommand) -> Result<Command, Error> {
    Ok(match command {
        PathCommand::MoveTo { x, y } => {
            finite(&[x, y])?;
            Command::MoveTo { x, y }
        }
        PathCommand::LineTo { x, y } => {
            finite(&[x, y])?;
            Command::LineTo { x, y }
        }
        PathCommand::CubicTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        } => {
            finite(&[x1, y1, x2, y2, x, y])?;
            Command::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            }
        }
        PathCommand::QuadTo { x1, y1, x, y } => {
            finite(&[x1, y1, x, y])?;
            Command::QuadTo { x1, y1, x, y }
        }
        PathCommand::Close => Command::Close,
    })
}

/// 每式独立捕获 unwind；输入深度另设上限，避免耗尽原生线程栈。
pub fn layout(
    source: &str,
    style: Style,
    metrics: &HashMap<char, GlyphMetrics>,
) -> Result<Scene, Error> {
    catch_unwind(AssertUnwindSafe(|| layout_inner(source, style, metrics))).unwrap_or_else(
        |panic| {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "Formula engine panicked".into());
            Err(Error::Panic(message))
        },
    )
}

fn layout_inner(
    source: &str,
    style: Style,
    metrics: &HashMap<char, GlyphMetrics>,
) -> Result<Scene, Error> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Error::ResourceLimit("Formula source exceeds 16 KiB".into()));
    }
    let mut depth = 0usize;
    let mut escaped = false;
    for byte in source.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'{' {
            depth += 1;
        }
        if depth > 128 {
            return Err(Error::ResourceLimit(
                "Formula nesting exceeds 128 levels".into(),
            ));
        }
        if byte == b'}' {
            depth = depth.saturating_sub(1);
        }
    }
    for metric in metrics.values() {
        finite(&[metric.width, metric.height, metric.depth])?;
        if metric.width < 0.0 || metric.height < 0.0 || metric.depth < 0.0 {
            return Err(Error::InvalidLayout("Negative fallback metrics".into()));
        }
    }
    let ast = ratex_parser::parse(source).map_err(|e| {
        if e.message == "Recursion limit exceeded"
            || e.message.starts_with("Too many expansions")
            || e.message
                .contains("Macro expansion resource limit exceeded")
        {
            Error::ResourceLimit(e.to_string())
        } else {
            Error::Parse(e.to_string())
        }
    })?;
    let fallback = Arc::new(
        metrics
            .iter()
            .map(|(ch, m)| {
                (
                    *ch,
                    FallbackGlyphMetrics {
                        width: m.width,
                        height: m.height,
                        depth: m.depth,
                    },
                )
            })
            .collect(),
    );
    let options = LayoutOptions::default().with_style(match style {
        Style::Display => MathStyle::Display,
        Style::Text => MathStyle::Text,
    });
    let tree = layout_with_fallback_metrics(&ast, &options, Arc::clone(&fallback));
    validate_layout(&tree).map_err(layout_error)?;
    let display = to_display_list(&tree);
    finite(&[display.width, display.height, display.depth])?;
    if display.width < 0.0 || display.height < 0.0 || display.depth < 0.0 {
        return Err(Error::InvalidLayout("Negative formula dimensions".into()));
    }
    if display.items.len() > MAX_ITEMS {
        return Err(Error::ResourceLimit(
            "Formula exceeds 32768 drawing items".into(),
        ));
    }
    // 默认色不同、显式颜色不变：不用猜颜色值，也不误改用户指定的 black。
    let alternate = layout_with_fallback_metrics(
        &ast,
        &options.with_color(RaColor::rgb(0.25, 0.5, 0.75)),
        fallback,
    );
    validate_layout(&alternate).map_err(layout_error)?;
    let alternate = to_display_list(&alternate);
    if display.items.len() != alternate.items.len() {
        return Err(Error::InvalidLayout(
            "Color changed formula geometry".into(),
        ));
    }
    let mut items = Vec::with_capacity(display.items.len());
    let mut missing = BTreeSet::new();
    let mut command_count = 0;
    for (item, other) in display.items.into_iter().zip(alternate.items) {
        let c = color(&item);
        let color = Color {
            foreground: c != color(&other),
            rgba: [c.r, c.g, c.b, c.a],
        };
        if !color
            .rgba
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            return Err(Error::InvalidLayout("Invalid formula color".into()));
        }
        items.push(match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font,
                char_code,
                ..
            } => {
                finite(&[x, y, scale])?;
                if scale <= 0.0 {
                    return Err(Error::InvalidLayout("Invalid glyph scale".into()));
                }
                if matches!(char_code, 0x200d | 0xfe0e | 0xfe0f) {
                    continue;
                }
                let id = ratex_font::FontId::parse(&font).filter(|id| {
                    !matches!(
                        id,
                        ratex_font::FontId::CjkRegular
                            | ratex_font::FontId::CjkFallback
                            | ratex_font::FontId::EmojiFallback
                    )
                });
                let ch = char::from_u32(char_code)
                    .ok_or_else(|| Error::InvalidLayout("Invalid glyph codepoint".into()))?;
                let katex_codepoint =
                    id.map(|id| ratex_font::katex_ttf_glyph_char(id, char_code) as u32);
                let has_katex_glyph = id
                    .zip(katex_codepoint)
                    .is_some_and(|(id, codepoint)| katex_cmap::contains(id.as_str(), codepoint));
                let fallback = !has_katex_glyph;
                let codepoint = if fallback {
                    char_code
                } else {
                    katex_codepoint.unwrap()
                };
                if fallback && !metrics.contains_key(&ch) {
                    missing.insert(char_code);
                }
                Item::Glyph {
                    x,
                    y,
                    scale,
                    font,
                    codepoint,
                    fallback,
                    color,
                }
            }
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                dashed,
                ..
            } => {
                finite(&[x, y, width, thickness])?;
                if width < 0.0 || thickness < 0.0 {
                    return Err(Error::InvalidLayout("Negative line dimensions".into()));
                }
                Item::Line {
                    x,
                    y,
                    width,
                    thickness,
                    dashed,
                    color,
                }
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                ..
            } => {
                finite(&[x, y, width, height])?;
                if width < 0.0 || height < 0.0 {
                    return Err(Error::InvalidLayout("Negative rectangle dimensions".into()));
                }
                Item::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                }
            }
            DisplayItem::Path {
                x,
                y,
                commands,
                fill,
                ..
            } => {
                finite(&[x, y])?;
                command_count += commands.len();
                if command_count > MAX_COMMANDS {
                    return Err(Error::ResourceLimit(
                        "Formula exceeds 131072 path commands".into(),
                    ));
                }
                Item::Path {
                    x,
                    y,
                    commands: commands
                        .into_iter()
                        .map(convert_command)
                        .collect::<Result<_, _>>()?,
                    fill,
                    color,
                }
            }
        });
    }
    Ok(Scene {
        width: display.width,
        height: display.height,
        depth: display.depth,
        items,
        missing_metrics: missing.into_iter().collect(),
    })
}

/// 宿主度量改变时使场景失效；有界缓存不随打开笔记次数增长。
#[derive(Default)]
pub struct Engine {
    metrics: HashMap<char, GlyphMetrics>,
    cache: VecDeque<(String, Style, Arc<Scene>, usize)>,
    cached_bytes: usize,
}
impl Engine {
    pub fn set_metrics(
        &mut self,
        values: impl IntoIterator<Item = (char, GlyphMetrics)>,
    ) -> Result<(), Error> {
        let values: Vec<_> = values.into_iter().collect();
        for (_, m) in &values {
            finite(&[m.width, m.height, m.depth])?;
            if m.width < 0.0 || m.height < 0.0 || m.depth < 0.0 {
                return Err(Error::InvalidLayout("Negative fallback metrics".into()));
            }
        }
        let mut changed = false;
        for (ch, m) in values {
            if self.metrics.get(&ch) != Some(&m) {
                self.metrics.insert(ch, m);
                changed = true;
            }
        }
        if changed {
            self.cache.clear();
            self.cached_bytes = 0;
        }
        Ok(())
    }
    pub fn render(&mut self, source: &str, style: Style) -> Result<Arc<Scene>, Error> {
        if let Some(index) = self
            .cache
            .iter()
            .position(|(s, t, _, _)| s == source && *t == style)
        {
            let entry = self.cache.remove(index).unwrap();
            let scene = Arc::clone(&entry.2);
            self.cache.push_back(entry);
            return Ok(scene);
        }
        let scene = Arc::new(layout(source, style, &self.metrics)?);
        let bytes = source.len()
            + scene.items.len() * std::mem::size_of::<Item>()
            + scene.missing_metrics.len() * 4
            + scene
                .items
                .iter()
                .map(|item| match item {
                    Item::Glyph { font, .. } => font.len(),
                    Item::Path { commands, .. } => commands.len() * std::mem::size_of::<Command>(),
                    _ => 0,
                })
                .sum::<usize>();
        if bytes > MAX_CACHE_BYTES {
            return Ok(scene);
        }
        while self.cache.len() >= 128 || self.cached_bytes + bytes > MAX_CACHE_BYTES {
            if let Some((_, _, _, old)) = self.cache.pop_front() {
                self.cached_bytes -= old;
            } else {
                break;
            }
        }
        self.cached_bytes += bytes;
        self.cache
            .push_back((source.into(), style, Arc::clone(&scene), bytes));
        Ok(scene)
    }
}
