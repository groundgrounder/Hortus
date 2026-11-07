//! 长期存活的字体与单篇笔记解析缓存；调用方须在后台初始化及排版。
use hortus_diagram::{
    Diagram, DiagramError, FontMetrics, LayoutOptions, SystemFonts, layout, layout_graph,
    mermaid_blocks, parse_diagram_at,
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
    NotATree {
        multi_parent: Vec<String>,
    },
    Unsupported {
        diagram_type: String,
    },
    UnsupportedConstruct {
        line: u32,
        start: u64,
        end: u64,
        kind: String,
    },
    Syntax {
        line: u32,
        kind: String,
    },
    Engine {
        kind: String,
    },
}
impl fmt::Display for DiagramFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotATree { .. } => f.write_str("diagram.not_a_tree"),
            Self::Unsupported { .. } | Self::UnsupportedConstruct { .. } => {
                f.write_str("diagram.unsupported")
            }
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
    diagrams: Vec<Result<Diagram, DiagramError>>,
    document: String,
    revision: u64,
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
                diagrams: Vec::new(),
                document: String::new(),
                revision: 0,
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
        self.render_request(
            source,
            mmd,
            width,
            collapsed,
            crate::DiagramIdentity {
                block,
                dark,
                ..crate::DiagramIdentity::default()
            },
            None,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_request(
        &self,
        source: String,
        mmd: bool,
        width: f64,
        collapsed: Vec<String>,
        identity: crate::DiagramIdentity,
        previous: Option<crate::DiagramPrevious>,
        viewport: Vec<crate::DiagramViewportAnchor>,
    ) -> Result<crate::SceneData, DiagramFailure> {
        self.render_request_impl(
            source, mmd, width, collapsed, identity, previous, viewport, false,
        )
    }

    /// 仅验收使用：与 render_request 同一布局路径，额外返回完整 Scene JSON。
    #[allow(clippy::too_many_arguments)]
    pub fn render_request_inspection(
        &self,
        source: String,
        mmd: bool,
        width: f64,
        collapsed: Vec<String>,
        identity: crate::DiagramIdentity,
        previous: Option<crate::DiagramPrevious>,
        viewport: Vec<crate::DiagramViewportAnchor>,
    ) -> Result<crate::SceneData, DiagramFailure> {
        self.render_request_impl(
            source, mmd, width, collapsed, identity, previous, viewport, true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_request_impl(
        &self,
        source: String,
        mmd: bool,
        width: f64,
        collapsed: Vec<String>,
        identity: crate::DiagramIdentity,
        previous: Option<crate::DiagramPrevious>,
        viewport: Vec<crate::DiagramViewportAnchor>,
        include_semantic_json: bool,
    ) -> Result<crate::SceneData, DiagramFailure> {
        let mut state = self.state.lock().map_err(|_| DiagramFailure::Engine {
            kind: "poisoned".into(),
        })?;
        if state.source != source
            || state.mmd != mmd
            || state.document != identity.document
            || state.revision != identity.revision
            || state.diagrams.is_empty()
        {
            state.diagrams = if mmd {
                vec![parse_diagram_at(&source, 0)]
            } else {
                mermaid_blocks(&source)
                    .into_iter()
                    .map(|b| b.parse_diagram())
                    .collect()
            };
            state.source = source;
            state.mmd = mmd;
            state.document = identity.document.clone();
            state.revision = identity.revision;
        }
        let diagram = state
            .diagrams
            .get(identity.block.wrapping_sub(1) as usize)
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
                DiagramError::UnsupportedConstruct { range, kind } => {
                    DiagramFailure::UnsupportedConstruct {
                        line: state.source[..range.start]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count() as u32
                            + 1,
                        start: range.start as u64,
                        end: range.end as u64,
                        kind: kind.clone(),
                    }
                }
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
            theme: if identity.dark {
                hortus_diagram::Theme::Dark
            } else {
                hortus_diagram::Theme::Light
            },
            collapsed: collapsed.into_iter().collect(),
            previous: previous.map(Into::into),
            viewport: Some(hortus_diagram::GraphViewport {
                visible: viewport
                    .into_iter()
                    .map(|anchor| hortus_diagram::ViewportAnchor {
                        node_id: anchor.node_id,
                        screen_center: hortus_diagram::Point {
                            x: anchor.screen_center.x,
                            y: anchor.screen_center.y,
                        },
                    })
                    .collect(),
            }),
        };
        let (scene, previous, metrics) = match &diagram {
            Diagram::Tree(tree) => {
                layout(tree, &options, &mut state.fonts).map(|scene| (scene, None, None))
            }
            Diagram::Graph(graph) => {
                layout_graph(graph, &options, &mut state.fonts).map(|result| {
                    (
                        result.scene,
                        Some(result.previous.into()),
                        Some(result.metrics.into()),
                    )
                })
            }
        }
        .map_err(|error| DiagramFailure::Engine { kind: error.0 })?;
        let semantic_json = if include_semantic_json {
            Some(
                serde_json::to_string(&scene.semantic_scene()).map_err(|error| {
                    DiagramFailure::Engine {
                        kind: format!("semantic_json:{error}"),
                    }
                })?,
            )
        } else {
            None
        };
        Ok(crate::SceneData::from(scene)
            .with_request(identity, previous, metrics)
            .with_semantic_json(semantic_json))
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
    fn container_fences_are_exposed_and_rendered_by_ordinal() {
        let source = "> ```mermaid\n> flowchart TB\n> A --> B\n> ```\n\n```mermaid\nflowchart TB\nC --> D\n```\n";
        let blocks = diagram_blocks(source.into());
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].fence_start < blocks[0].source_start);
        assert!(blocks[0].source_end < blocks[0].fence_end);
        assert_eq!(blocks[0].diagram_type, "flowchart");
        let engine = DiagramEngine::new().unwrap();
        let first = engine
            .render(source.into(), false, 1, 700.0, false, vec![])
            .unwrap();
        let second = engine
            .render(source.into(), false, 2, 700.0, false, vec![])
            .unwrap();
        assert_eq!(first.hit_regions.len(), 2);
        assert_eq!(second.hit_regions.len(), 2);
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
        let graph = render("flowchart TB\nB & C --> D", vec![]).unwrap();
        assert_eq!(graph.hit_regions.len(), 3);
        assert_eq!(graph.schema_version, 2);
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

    #[test]
    fn top_level_containers_and_invisible_links_survive_the_ffi_scene() {
        let source = include_str!("../../../fixtures/mermaid/flowchart/subgraphs-top-level.mmd");
        let engine = DiagramEngine::new().unwrap();
        let scene = engine
            .render(source.into(), true, 1, 700.0, false, vec![])
            .unwrap();
        assert_eq!(scene.hit_regions.len(), 8);
        let metrics = scene.graph_metrics.as_ref().unwrap();
        assert_eq!(metrics.edges, 3);
        assert_eq!(metrics.invisible_edges, 4);
        assert_eq!(metrics.containers, 2);
        assert_eq!(
            scene
                .primitives
                .iter()
                .filter(|primitive| matches!(primitive, crate::ScenePrimitive::GraphPath { .. }))
                .count(),
            3
        );
        assert_eq!(
            scene
                .primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    crate::ScenePrimitive::GraphShape { shape, .. } if shape == "Container"
                ))
                .count(),
            2
        );
    }
}

#[cfg(test)]
mod graph_wire_tests {
    use super::*;

    #[test]
    fn graph_width_changes_keep_a_renderable_previous_scene() {
        let source = include_str!("../../../fixtures/mermaid/flowchart/graph-syntax.mmd");
        let engine = DiagramEngine::new().unwrap();
        let mut previous = None;
        for width in [1100.0, 700.0, 900.0, 1100.0] {
            let scene = engine
                .render_request(
                    source.into(),
                    true,
                    width,
                    vec![],
                    crate::DiagramIdentity {
                        document: "width-regression".into(),
                        block: 1,
                        ..Default::default()
                    },
                    previous,
                    vec![],
                )
                .unwrap();
            assert_eq!(scene.width, width);
            assert_eq!(scene.hit_regions.len(), 8);
            assert!(!scene.graph_metrics.as_ref().unwrap().horizontal_overflow);
            previous = scene.previous;
        }
    }

    #[test]
    fn every_graph_primitive_and_request_matches_actual_uniffi_bytes() {
        let engine = DiagramEngine::new().unwrap();
        for source in [
            "flowchart TB\nA[甲😀] -->|是| B{乙}\nC[(库)] --> B\nB --o A\nB --x C",
            "flowchart RL\nA((甲)) <--> B[[乙]]\nA --> B\nB ==> C[/丙/]",
        ] {
            let identity = crate::DiagramIdentity {
                document: "session-甲😀".into(),
                block: 1,
                revision: 12,
                dark: true,
                font_generation: 3,
                request_generation: 7,
            };
            let first = engine
                .render_request(
                    source.into(),
                    true,
                    700.0,
                    vec![],
                    identity.clone(),
                    None,
                    vec![],
                )
                .unwrap();
            for scene in [
                first.clone(),
                engine
                    .render_request(
                        source.into(),
                        true,
                        900.0,
                        vec![],
                        identity.clone(),
                        first.previous,
                        vec![],
                    )
                    .unwrap(),
            ] {
                let mut wire = Vec::new();
                <crate::SceneData as uniffi::Lower<crate::UniFfiTag>>::write(
                    scene.clone(),
                    &mut wire,
                );
                assert_eq!(scene.wire_bytes as usize, wire.len());
                assert_eq!(scene.identity, identity);
                assert_eq!(scene.schema_version, 2);
                assert!(scene.graph_metrics.is_some());
            }
        }
    }
}
