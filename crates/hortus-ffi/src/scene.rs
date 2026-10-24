//! Scene 的类型化线协议；不在边界重复解析 JSON。
use hortus_diagram::{Color, HitRegion, Point, Primitive, Rect, Scene, TextSpan};

#[derive(Clone, Debug, uniffi::Record)]
pub struct ScenePoint {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneText {
    pub text: String,
    pub baseline: ScenePoint,
    pub width: f64,
    pub font_size: f64,
    pub font_weight: u16,
    pub font_family: String,
    pub color: SceneColor,
}
#[derive(Clone, Debug, uniffi::Enum)]
pub enum ScenePrimitive {
    RoundedRect {
        rect: SceneRect,
        radius: f64,
        color: SceneColor,
    },
    Line {
        from: ScenePoint,
        to: ScenePoint,
        width: f64,
        color: SceneColor,
    },
    Text {
        span: SceneText,
    },
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneHit {
    pub node_id: String,
    pub label: String,
    pub prefix: Option<String>,
    pub rect: SceneRect,
    pub can_collapse: bool,
    pub collapsed: bool,
    pub source_start: u64,
    pub source_end: u64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneData {
    pub width: f64,
    pub height: f64,
    pub primitives: Vec<ScenePrimitive>,
    pub hit_regions: Vec<SceneHit>,
    pub wire_bytes: u64,
    pub layout_ms: f64,
}
impl From<Point> for ScenePoint {
    fn from(p: Point) -> Self {
        Self { x: p.x, y: p.y }
    }
}
impl From<Rect> for SceneRect {
    fn from(p: Rect) -> Self {
        Self {
            x: p.x,
            y: p.y,
            width: p.width,
            height: p.height,
        }
    }
}
impl From<Color> for SceneColor {
    fn from(c: Color) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}
impl From<TextSpan> for SceneText {
    fn from(s: TextSpan) -> Self {
        Self {
            text: s.text,
            baseline: s.baseline.into(),
            width: s.width,
            font_size: s.font_size,
            font_weight: s.font_weight,
            font_family: s.font_family,
            color: s.color.into(),
        }
    }
}
impl From<HitRegion> for SceneHit {
    fn from(h: HitRegion) -> Self {
        Self {
            node_id: h.node_id,
            label: h.label,
            prefix: h.prefix,
            rect: h.rect.into(),
            can_collapse: h.can_collapse,
            collapsed: h.collapsed,
            source_start: h.source_range.start as u64,
            source_end: h.source_range.end as u64,
        }
    }
}
impl From<Scene> for SceneData {
    fn from(scene: Scene) -> Self {
        let mut bytes = 40;
        let primitives = scene
            .primitives
            .into_iter()
            .map(|p| match p {
                Primitive::RoundedRect {
                    rect,
                    radius,
                    color,
                } => {
                    bytes += 76;
                    ScenePrimitive::RoundedRect {
                        rect: rect.into(),
                        radius,
                        color: color.into(),
                    }
                }
                Primitive::Line {
                    from,
                    to,
                    width,
                    color,
                } => {
                    bytes += 76;
                    ScenePrimitive::Line {
                        from: from.into(),
                        to: to.into(),
                        width,
                        color: color.into(),
                    }
                }
                Primitive::Text(span) => {
                    bytes += 78 + span.text.len() + span.font_family.len();
                    ScenePrimitive::Text { span: span.into() }
                }
            })
            .collect();
        let hit_regions = scene
            .hit_regions
            .into_iter()
            .map(|h| {
                bytes += 59
                    + h.node_id.len()
                    + h.label.len()
                    + h.prefix.as_ref().map_or(0, |s| 4 + s.len());
                h.into()
            })
            .collect();
        Self {
            width: scene.width,
            height: scene.height,
            primitives,
            hit_regions,
            wire_bytes: bytes as u64,
            layout_ms: scene.metrics.layout_ms,
        }
    }
}
