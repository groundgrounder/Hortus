use crate::{Point, Rect};
use serde::Serialize;

/// 上一场景只由当前调用方传入；不保存在解析缓存中。
#[derive(Clone, Debug, Default, Serialize)]
pub struct PreviousGraph {
    pub width: f64,
    pub nodes: Vec<GraphAnchor>,
    pub edges: Vec<EdgeAnchor>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphAnchor {
    pub node_id: String,
    pub rect: Rect,
    pub rank: usize,
    pub group: usize,
}

#[derive(Clone, Debug, Default)]
pub struct GraphViewport {
    pub visible: Vec<ViewportAnchor>,
}

#[derive(Clone, Debug)]
pub struct ViewportAnchor {
    pub node_id: String,
    pub screen_center: Point,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdgeAnchor {
    pub edge_id: usize,
    pub points: Vec<Point>,
}
