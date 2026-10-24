use crate::{Color, FontInfo};
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
}
