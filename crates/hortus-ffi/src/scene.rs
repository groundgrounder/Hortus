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
    GraphPath {
        edge_id: u64,
        corner_radius: f64,
        points: Vec<ScenePoint>,
        source_start: u64,
        source_end: u64,
        label_start: Option<u64>,
        label_end: Option<u64>,
        line: String,
        start: String,
        end: String,
        color: SceneColor,
        width: f64,
    },
    GraphShape {
        rect: SceneRect,
        shape: String,
        outline: Vec<ScenePoint>,
        details: Vec<Vec<ScenePoint>>,
        fill: SceneColor,
        stroke: SceneColor,
        stroke_width: f64,
    },
    LabelBackground {
        edge_id: u64,
        rect: SceneRect,
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
/// 调用方拥有请求身份与上一场景；解析缓存不保存它们。
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct DiagramIdentity {
    pub document: String,
    pub block: u32,
    pub revision: u64,
    pub dark: bool,
    pub font_generation: u64,
    pub request_generation: u64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramAnchor {
    pub node_id: String,
    pub rect: SceneRect,
    pub rank: u32,
    pub group: u32,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramEdgeAnchor {
    pub edge_id: u64,
    pub points: Vec<ScenePoint>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramPrevious {
    pub direction: u32,
    pub width: f64,
    pub nodes: Vec<DiagramAnchor>,
    pub edges: Vec<DiagramEdgeAnchor>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramViewportAnchor {
    pub node_id: String,
    pub screen_center: ScenePoint,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramGraphMetrics {
    pub ranks: u32,
    pub roots: u32,
    pub edges: u32,
    pub node_overlaps: u32,
    pub edge_crossings: u32,
    pub horizontal_overflow: bool,
    pub retained_nodes: u32,
    pub stability_reason: String,
}
impl From<hortus_diagram::PreviousGraph> for DiagramPrevious {
    fn from(value: hortus_diagram::PreviousGraph) -> Self {
        Self {
            direction: value.direction as u32,
            width: value.width,
            nodes: value
                .nodes
                .into_iter()
                .map(|node| DiagramAnchor {
                    node_id: node.node_id,
                    rect: node.rect.into(),
                    rank: node.rank as u32,
                    group: node.group as u32,
                })
                .collect(),
            edges: value
                .edges
                .into_iter()
                .map(|edge| DiagramEdgeAnchor {
                    edge_id: edge.edge_id as u64,
                    points: edge.points.into_iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}
impl From<DiagramPrevious> for hortus_diagram::PreviousGraph {
    fn from(value: DiagramPrevious) -> Self {
        Self {
            direction: match value.direction {
                1 => hortus_diagram::Direction::BottomToTop,
                2 => hortus_diagram::Direction::LeftToRight,
                3 => hortus_diagram::Direction::RightToLeft,
                _ => hortus_diagram::Direction::TopToBottom,
            },
            width: value.width,
            nodes: value
                .nodes
                .into_iter()
                .map(|node| hortus_diagram::GraphAnchor {
                    node_id: node.node_id,
                    rect: hortus_diagram::Rect {
                        x: node.rect.x,
                        y: node.rect.y,
                        width: node.rect.width,
                        height: node.rect.height,
                    },
                    rank: node.rank as usize,
                    group: node.group as usize,
                })
                .collect(),
            edges: value
                .edges
                .into_iter()
                .map(|edge| hortus_diagram::EdgeAnchor {
                    edge_id: edge.edge_id as usize,
                    points: edge
                        .points
                        .into_iter()
                        .map(|p| Point { x: p.x, y: p.y })
                        .collect(),
                })
                .collect(),
        }
    }
}
impl From<hortus_diagram::GraphMetrics> for DiagramGraphMetrics {
    fn from(value: hortus_diagram::GraphMetrics) -> Self {
        Self {
            ranks: value.ranks as u32,
            roots: value.roots as u32,
            edges: value.edge_count as u32,
            node_overlaps: value.node_overlaps as u32,
            edge_crossings: value.edge_crossings as u32,
            horizontal_overflow: value.horizontal_overflow,
            retained_nodes: value.retained_nodes as u32,
            stability_reason: value.stability_reason,
        }
    }
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SceneData {
    pub schema_version: u32,
    pub identity: DiagramIdentity,
    pub previous: Option<DiagramPrevious>,
    pub graph_metrics: Option<DiagramGraphMetrics>,
    pub width: f64,
    pub height: f64,
    pub primitives: Vec<ScenePrimitive>,
    pub hit_regions: Vec<SceneHit>,
    pub wire_bytes: u64,
    pub layout_ms: f64,
    pub semantic_json: Option<String>,
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
        // 末尾 semantic_json 选项标签也计入 wire 长度。
        let mut bytes = 44 + 33 + 2 + 1;
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
                Primitive::GraphPath {
                    edge_id,
                    corner_radius,
                    points,
                    source_range,
                    label_range,
                    line,
                    start,
                    end,
                    color,
                    width,
                } => {
                    let line = format!("{line:?}");
                    let start = format!("{start:?}");
                    let end = format!("{end:?}");
                    bytes += 94
                        + points.len() * 16
                        + line.len()
                        + start.len()
                        + end.len()
                        + usize::from(label_range.is_some()) * 16;
                    ScenePrimitive::GraphPath {
                        edge_id: edge_id as u64,
                        corner_radius,
                        points: points.into_iter().map(Into::into).collect(),
                        source_start: source_range.start as u64,
                        source_end: source_range.end as u64,
                        label_start: label_range.as_ref().map(|range| range.start as u64),
                        label_end: label_range.as_ref().map(|range| range.end as u64),
                        line,
                        start,
                        end,
                        color: color.into(),
                        width,
                    }
                }
                Primitive::GraphShape {
                    rect,
                    shape,
                    outline,
                    details,
                    fill,
                    stroke,
                    stroke_width,
                } => {
                    // 矩形/圆角/胶囊的轮廓可由平台按同一半径重建；跨 FFI 只传形状名，
                    // 避免 40 节点图每次改宽都搬运数千个重复轮廓点。
                    let standard_outline = matches!(
                        shape,
                        hortus_diagram::Shape::Rectangle
                            | hortus_diagram::Shape::Rounded
                            | hortus_diagram::Shape::Stadium
                    );
                    let shape = format!("{shape:?}");
                    bytes += 120
                        + shape.len()
                        + if standard_outline {
                            0
                        } else {
                            outline.len() * 16
                        }
                        + details.iter().map(|p| 4 + p.len() * 16).sum::<usize>();
                    ScenePrimitive::GraphShape {
                        rect: rect.into(),
                        shape,
                        outline: if standard_outline {
                            Vec::new()
                        } else {
                            outline.into_iter().map(Into::into).collect()
                        },
                        details: details
                            .into_iter()
                            .map(|p| p.into_iter().map(Into::into).collect())
                            .collect(),
                        fill: fill.into(),
                        stroke: stroke.into(),
                        stroke_width,
                    }
                }
                Primitive::LabelBackground {
                    edge_id,
                    rect,
                    color,
                } => {
                    bytes += 76;
                    ScenePrimitive::LabelBackground {
                        edge_id: edge_id as u64,
                        rect: rect.into(),
                        color: color.into(),
                    }
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
            schema_version: 2,
            identity: DiagramIdentity::default(),
            previous: None,
            graph_metrics: None,
            width: scene.width,
            height: scene.height,
            primitives,
            hit_regions,
            wire_bytes: bytes as u64,
            layout_ms: scene.metrics.layout_ms,
            semantic_json: None,
        }
    }
}

impl SceneData {
    pub(crate) fn with_request(
        mut self,
        identity: DiagramIdentity,
        previous: Option<DiagramPrevious>,
        metrics: Option<DiagramGraphMetrics>,
    ) -> Self {
        self.wire_bytes += identity.document.len() as u64;
        if let Some(value) = &previous {
            self.wire_bytes += 20
                + value
                    .nodes
                    .iter()
                    .map(|node| 44 + node.node_id.len() as u64)
                    .sum::<u64>()
                + value
                    .edges
                    .iter()
                    .map(|edge| 12 + 16 * edge.points.len() as u64)
                    .sum::<u64>();
        }
        if let Some(value) = &metrics {
            self.wire_bytes += 29 + value.stability_reason.len() as u64;
        }
        self.identity = identity;
        self.previous = previous;
        self.graph_metrics = metrics;
        self
    }

    pub(crate) fn with_semantic_json(mut self, semantic_json: Option<String>) -> Self {
        if let Some(value) = &semantic_json {
            self.wire_bytes += 4 + value.len() as u64;
        }
        self.semantic_json = semantic_json;
        self
    }
}
