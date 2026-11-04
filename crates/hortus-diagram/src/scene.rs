use crate::{Color, EdgeLine, FontInfo, Marker, Shape};
use serde::Serialize;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.x + self.width && p.y >= self.y && p.y <= self.y + self.height
    }
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width - 0.001
            && other.x < self.x + self.width - 0.001
            && self.y < other.y + other.height - 0.001
            && other.y < self.y + self.height - 0.001
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextSpan {
    pub text: String,
    pub baseline: Point,
    pub width: f64,
    pub font_size: f64,
    pub font_weight: u16,
    pub font_family: String,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    RoundedRect {
        rect: Rect,
        radius: f64,
        color: Color,
    },
    Line {
        from: Point,
        to: Point,
        width: f64,
        color: Color,
    },
    GraphPath {
        edge_id: usize,
        corner_radius: f64,
        points: Vec<Point>,
        source_range: Range<usize>,
        label_range: Option<Range<usize>>,
        line: EdgeLine,
        start: Marker,
        end: Marker,
        color: Color,
        width: f64,
    },
    GraphShape {
        rect: Rect,
        shape: Shape,
        outline: Vec<Point>,
        details: Vec<Vec<Point>>,
        fill: Color,
        stroke: Color,
        stroke_width: f64,
    },
    LabelBackground {
        edge_id: usize,
        rect: Rect,
        color: Color,
    },
    Text(TextSpan),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HitRegion {
    pub label: String,
    pub prefix: Option<String>,
    pub node_id: String,
    pub rect: Rect,
    pub can_collapse: bool,
    pub collapsed: bool,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Metrics {
    pub columns: usize,
    pub diagram_height: f64,
    pub min_font_px: f64,
    pub horizontal_overflow: bool,
    pub node_overlaps: usize,
    pub fonts: Vec<FontInfo>,
    pub layout_ms: f64,
    pub root_hue: Option<f64>,
    pub branch_hues: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub primitives: Vec<Primitive>,
    pub hit_regions: Vec<HitRegion>,
    pub metrics: Metrics,
}

impl Scene {
    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        self.hit_regions.iter().find(|hit| hit.rect.contains(point))
    }

    /// 去除仅用于性能统计的耗时后，返回可跨 CLI/FFI 严格比较的完整 Scene。
    pub fn semantic_scene(&self) -> Self {
        let mut scene = self.clone();
        scene.metrics.layout_ms = 0.0;
        // 字体缓存命中记录取决于进程预热历史；实际字体族和字形仍在每个文本图元中。
        scene.metrics.fonts.clear();
        scene
    }
}
