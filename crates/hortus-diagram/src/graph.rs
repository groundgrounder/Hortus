use crate::parser::Tree;
use serde::Serialize;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Rectangle,
    Container,
    Rounded,
    Circle,
    DoubleCircle,
    Stadium,
    Subroutine,
    Cylinder,
    Diamond,
    Hexagon,
    Asymmetric,
    Parallelogram,
    ParallelogramReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeLine {
    #[default]
    Solid,
    Dotted,
    Thick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    #[default]
    None,
    Arrow,
    Circle,
    Cross,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Connector {
    pub line: EdgeLine,
    pub start: Marker,
    pub end: Marker,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub stroke: Option<String>,
    pub fill: Option<String>,
    pub color: Option<String>,
    pub stroke_width: Option<f64>,
    pub stroke_dasharray: Option<Vec<f64>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphNodeKind {
    #[default]
    Node,
    Container,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub id_range: Range<usize>,
    pub label_range: Range<usize>,
    pub source_range: Range<usize>,
    pub shape: Shape,
    pub class: Option<String>,
    pub kind: GraphNodeKind,
    pub owner: Option<usize>,
    pub preferred_size: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub id: usize,
    pub from: usize,
    pub to: usize,
    pub source_range: Range<usize>,
    pub connector_range: Range<usize>,
    pub label: Option<String>,
    pub label_range: Option<Range<usize>>,
    pub connector: Connector,
    pub visible: bool,
    pub container: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphContainer {
    pub node_index: usize,
    pub direction: Direction,
    pub members: Vec<usize>,
    pub id_range: Range<usize>,
    pub title_range: Range<usize>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub direction: Direction,
    pub warnings: Vec<(Range<usize>, String)>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<Edge>,
    pub class_defs: HashMap<String, Style>,
    pub inline_styles: HashMap<String, Style>,
    pub forest: bool,
    pub containers: Vec<GraphContainer>,
}

#[derive(Clone, Debug)]
pub enum Diagram {
    Tree(Tree),
    Graph(Graph),
}

impl Graph {
    pub fn container_at(&self, node_index: usize) -> Option<usize> {
        self.containers
            .iter()
            .position(|container| container.node_index == node_index)
    }

    pub fn ordinary_node_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.kind == GraphNodeKind::Node)
            .count()
    }

    pub fn visible_edge_count(&self) -> usize {
        self.edges.iter().filter(|edge| edge.visible).count()
    }

    pub fn invisible_edge_count(&self) -> usize {
        self.edges.iter().filter(|edge| !edge.visible).count()
    }

    pub(crate) fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        for (range, _) in &mut self.warnings {
            *range = map(range.clone());
        }
        for node in &mut self.nodes {
            node.id_range = map(node.id_range.clone());
            node.label_range = map(node.label_range.clone());
            node.source_range = map(node.source_range.clone());
        }
        for edge in &mut self.edges {
            edge.source_range = map(edge.source_range.clone());
            edge.connector_range = map(edge.connector_range.clone());
            edge.label_range = edge.label_range.take().map(map);
        }
        for container in &mut self.containers {
            container.id_range = map(container.id_range.clone());
            container.title_range = map(container.title_range.clone());
            container.source_range = map(container.source_range.clone());
        }
    }
}
