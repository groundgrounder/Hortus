//! 树形 Mermaid 的解析、字体度量、响应式排版和平台无关场景。
mod class;
mod class_layout;
mod class_parser;
mod color;
mod er;
mod er_layout;
mod er_parser;
mod font;
mod geometry;
mod graph;
mod graph_layout;
mod graph_parser;
mod graph_router;
mod graph_state;
mod layout;
mod parser;
mod scene;
mod sequence;
mod sequence_layout;
mod shape;
mod state;
mod state_layout;
mod state_parser;
mod svg;
mod syntax;
mod text;
mod uml;
mod uml_parser;
mod wikilink;

pub use class::{
    ClassDiagram, ClassMarker, ClassMember, ClassMemberKind, ClassNamespace, ClassNode, ClassNote,
    ClassRelation, ClassVisibility,
};
pub use class_layout::layout_class;
pub use color::Color;
pub use er::{ErAttribute, ErCardinality, ErDiagram, ErEntity, ErKey, ErRelation};
pub use er_layout::layout_er;
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
    ClassStyle, DiagramError, MermaidBlock, Node, Tree, infer_diagram_header, mermaid_block,
    mermaid_blocks, parse, parse_at, parse_diagram, parse_diagram_at,
};
pub use scene::{HitRegion, Metrics, Point, Primitive, Rect, Scene, TextSpan, Theme};
pub use sequence::{
    FragmentKind, NoteSide, SequenceDiagram, SequenceMessage, SequenceNote, SequenceParticipant,
    SequenceRow, SequenceText,
};
pub use sequence_layout::layout_sequence;
pub use shape::outline as shape_outline;
pub use state::{
    StateDiagram, StateKind, StateNode, StateNote, StateNoteSide, StateRegion, StateTransition,
};
pub use state_layout::layout_state;
pub use state_parser::is_state_diagram;
