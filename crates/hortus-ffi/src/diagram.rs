//! 长期存活的字体与单篇笔记解析缓存；调用方须在后台初始化及排版。
use hortus_diagram::{
    DiagramError, FontMetrics, LayoutOptions, SystemFonts, Tree, layout, mermaid_blocks, parse_at,
};
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, uniffi::Record)]
pub struct DiagramBlock {
    pub ordinal: u32,
    pub fence_start: u64,
    pub fence_end: u64,
    pub source_start: u64,
    pub source_end: u64,
    pub diagram_type: String,
}

#[derive(Debug, uniffi::Error)]
pub enum DiagramFailure {
    NotATree { multi_parent: Vec<String> },
    Unsupported { diagram_type: String },
    Syntax { line: u32, kind: String },
    Engine { kind: String },
}
impl fmt::Display for DiagramFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotATree { .. } => f.write_str("diagram.not_a_tree"),
            Self::Unsupported { .. } => f.write_str("diagram.unsupported"),
            Self::Syntax { .. } => f.write_str("diagram.syntax"),
            Self::Engine { .. } => f.write_str("diagram.engine"),
        }
    }
}
impl std::error::Error for DiagramFailure {}

#[uniffi::export]
pub fn diagram_blocks(note: String) -> Vec<DiagramBlock> {
    mermaid_blocks(&note)
        .iter()
        .enumerate()
        .map(|(i, b)| DiagramBlock {
            ordinal: i as u32 + 1,
            fence_start: b.fence_range.start as u64,
            fence_end: b.fence_range.end as u64,
            source_start: b.source_range.start as u64,
            source_end: b.source_range.end as u64,
            diagram_type: b
                .source
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty() && !s.starts_with("%%"))
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("")
                .into(),
        })
        .collect()
}

struct EngineState {
    fonts: SystemFonts,
    source: String,
    mmd: bool,
    trees: Vec<Result<Tree, DiagramError>>,
}

#[derive(uniffi::Object)]
pub struct DiagramEngine {
    state: Mutex<EngineState>,
}

#[uniffi::export]
impl DiagramEngine {
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, DiagramFailure> {
        let mut fonts = SystemFonts::new().map_err(|_| DiagramFailure::Engine {
            kind: "font".into(),
        })?;
        // 覆盖首张图要用的字体和字重，文件读取与整形均在后台预热。
        for weight in [400, 500, 600, 650] {
            fonts.measure("Hortus 笔记阅读 +0123456789", 15.0, weight);
        }
        Ok(Arc::new(Self {
            state: Mutex::new(EngineState {
                fonts,
                source: String::new(),
                mmd: false,
                trees: Vec::new(),
            }),
        }))
    }

    pub fn render(
        &self,
        source: String,
        mmd: bool,
        block: u32,
        width: f64,
        dark: bool,
        collapsed: Vec<String>,
    ) -> Result<crate::SceneData, DiagramFailure> {
        let mut state = self.state.lock().unwrap();
        if state.source != source || state.mmd != mmd || state.trees.is_empty() {
            state.trees = if mmd {
                vec![parse_at(&source, 0)]
            } else {
                mermaid_blocks(&source)
                    .into_iter()
                    .map(|b| parse_at(b.source, b.source_range.start))
                    .collect()
            };
            state.source = source;
            state.mmd = mmd;
        }
        let tree = state
            .trees
            .get(block.wrapping_sub(1) as usize)
            .ok_or_else(|| DiagramFailure::Engine {
                kind: "missing_block".into(),
            })?
            .as_ref()
            .map_err(|error| match error {
                DiagramError::NotATree { multi_parent } => DiagramFailure::NotATree {
                    multi_parent: multi_parent.clone(),
                },
                DiagramError::Unsupported { diagram_type } => DiagramFailure::Unsupported {
                    diagram_type: diagram_type.clone(),
                },
                DiagramError::Syntax { range, kind } => DiagramFailure::Syntax {
                    line: state.source[..range.start]
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count() as u32
                        + 1,
                    kind: kind.clone(),
                },
                DiagramError::MissingBlock { .. } => DiagramFailure::Engine {
                    kind: "missing_block".into(),
                },
            })?
            .clone();
        let options = LayoutOptions {
            width,
            theme: if dark {
                hortus_diagram::Theme::Dark
            } else {
                hortus_diagram::Theme::Light
            },
            collapsed: collapsed.into_iter().collect(),
        };
        let scene =
            layout(&tree, &options, &mut state.fonts).map_err(|_| DiagramFailure::Engine {
                kind: "layout".into(),
            })?;
        Ok(scene.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_ranges_cover_fences_and_exclude_other_languages() {
        let source = "中文😀\r\n```rust\n```mermaid\n```\n~~~mermaid\r\nmindmap\n  Root\n~~~\n```mermaid\nflowchart TB\nA --> B";
        let blocks = diagram_blocks(source.into());
        assert_eq!(blocks.len(), 2);
        for (i, block) in blocks.iter().enumerate() {
            assert_eq!(block.ordinal as usize, i + 1);
            assert!(source.is_char_boundary(block.fence_start as usize));
            assert!(source.is_char_boundary(block.fence_end as usize));
            assert!(block.fence_start < block.source_start);
            assert!(block.source_end <= block.fence_end);
        }
        assert_eq!(blocks[1].fence_end as usize, source.len());
    }

    #[test]
    fn engine_reuses_fonts_and_distinguishes_failures_and_stale_ids() {
        let engine = DiagramEngine::new().unwrap();
        let render = |source: &str, collapsed| {
            engine.render(source.into(), true, 1, 700.0, false, collapsed)
        };
        assert!(matches!(
            render("sequenceDiagram", vec![]),
            Err(DiagramFailure::Unsupported { .. })
        ));
        assert!(matches!(
            render("flowchart TB\nB & C --> D", vec![]),
            Err(DiagramFailure::NotATree { .. })
        ));
        assert!(matches!(
            render("flowchart TB\nA --> B\nB ??? C", vec![]),
            Err(DiagramFailure::Syntax { line: 3, .. })
        ));
        let scene = render("flowchart TB\nA --> B", vec!["missing".into()]).unwrap();
        assert_eq!(scene.hit_regions.len(), 2);
        let mut wire = Vec::new();
        <crate::SceneData as uniffi::Lower<crate::UniFfiTag>>::write(scene.clone(), &mut wire);
        assert_eq!(scene.wire_bytes as usize, wire.len());
        let scene = render("flowchart TB\nA --> B", vec!["A".into()]).unwrap();
        assert_eq!(scene.hit_regions.len(), 1);
    }
}
