//! 状态图的语义结构。每个并发区域拥有独立的方向、转换与起止伪状态。

use crate::{Direction, Style};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StateKind {
    #[default]
    Ordinary,
    Start,
    End,
    Choice,
    Fork,
    Join,
    Composite(Vec<StateRegion>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateNode {
    pub id: String,
    pub label: String,
    pub descriptions: Vec<String>,
    pub kind: StateKind,
    pub class: Option<String>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTransition {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub label_range: Option<Range<usize>>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateNoteSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateNote {
    pub target: usize,
    pub side: StateNoteSide,
    pub text: String,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateRegion {
    pub direction: Direction,
    pub nodes: Vec<StateNode>,
    pub transitions: Vec<StateTransition>,
    pub notes: Vec<StateNote>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StateDiagram {
    pub title: Option<String>,
    pub root: StateRegion,
    pub class_defs: HashMap<String, Style>,
}

impl StateDiagram {
    pub(crate) fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        self.root.map_ranges(map);
    }
}

impl StateRegion {
    fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        for node in &mut self.nodes {
            node.source_range = map(node.source_range.clone());
            if let StateKind::Composite(regions) = &mut node.kind {
                for region in regions {
                    region.map_ranges(map);
                }
            }
        }
        for edge in &mut self.transitions {
            edge.source_range = map(edge.source_range.clone());
            edge.label_range = edge.label_range.take().map(map);
        }
        for note in &mut self.notes {
            note.source_range = map(note.source_range.clone());
        }
    }
}
