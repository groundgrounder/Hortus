//! 树形 Mermaid 的解析、字体度量、响应式排版和平台无关场景。
mod color;
mod font;
mod graph;
mod graph_layout;
mod graph_parser;
mod graph_router;
mod graph_state;
mod layout;
mod parser;
mod scene;
mod shape;
mod svg;
mod text;

pub use color::Color;
pub use font::{
    DeterministicMetrics, FontInfo, FontMetrics, MeasuredRun, Measurement, SystemFonts,
};
pub use graph::{
    Connector, Diagram, Direction, Edge, EdgeLine, Graph, GraphContainer, GraphNode, GraphNodeKind,
    Marker, Shape, Style,
};
pub use graph_layout::{GraphMetrics, GraphScene, layout_graph};
pub use graph_state::{EdgeAnchor, GraphAnchor, GraphViewport, PreviousGraph, ViewportAnchor};
pub use layout::{LayoutError, LayoutOptions, layout, partition};
pub use parser::{
    ClassStyle, DiagramError, MermaidBlock, Node, Tree, mermaid_block, mermaid_blocks, parse,
    parse_at, parse_diagram, parse_diagram_at,
};
pub use scene::{HitRegion, Metrics, Point, Primitive, Rect, Scene, TextSpan, Theme};
