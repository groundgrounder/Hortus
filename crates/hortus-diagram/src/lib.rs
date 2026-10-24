//! 树形 Mermaid 的解析、字体度量、响应式排版和平台无关场景。
mod color;
mod font;
mod layout;
mod parser;
mod scene;
mod svg;
mod text;

pub use color::Color;
pub use font::{
    DeterministicMetrics, FontInfo, FontMetrics, MeasuredRun, Measurement, SystemFonts,
};
pub use layout::{LayoutError, LayoutOptions, layout, partition};
pub use parser::{
    ClassStyle, DiagramError, MermaidBlock, Node, Tree, mermaid_block, mermaid_blocks, parse,
    parse_at,
};
pub use scene::{HitRegion, Metrics, Point, Primitive, Rect, Scene, TextSpan, Theme};
