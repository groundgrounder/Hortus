//! ER 图的语义结构：实体、属性、鸦脚基数与关系。

use crate::{Direction, Style};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ErCardinality {
    #[default]
    ExactlyOne,
    ZeroOrOne,
    OneOrMore,
    ZeroOrMore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErKey {
    Primary,
    Foreign,
    Unique,
}

impl ErKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Primary => "PK",
            Self::Foreign => "FK",
            Self::Unique => "UK",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErAttribute {
    /// 类型文本，例如 `varchar(64)`、`string[]`。
    pub kind: String,
    pub name: String,
    pub keys: Vec<ErKey>,
    pub comment: Option<String>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErEntity {
    pub id: String,
    pub label: String,
    pub attributes: Vec<ErAttribute>,
    pub class: Option<String>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErRelation {
    pub from: usize,
    pub to: usize,
    pub from_cardinality: ErCardinality,
    pub to_cardinality: ErCardinality,
    /// 实线识别关系；虚线非识别关系。
    pub identifying: bool,
    pub label: Option<String>,
    pub label_range: Option<Range<usize>>,
    pub connector_range: Range<usize>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErDiagram {
    pub title: Option<String>,
    pub direction: Direction,
    pub entities: Vec<ErEntity>,
    pub relations: Vec<ErRelation>,
    pub class_defs: HashMap<String, Style>,
    pub inline_styles: HashMap<String, Style>,
}

impl ErDiagram {
    pub(crate) fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        for entity in &mut self.entities {
            entity.source_range = map(entity.source_range.clone());
            for attribute in &mut entity.attributes {
                attribute.source_range = map(attribute.source_range.clone());
            }
        }
        for relation in &mut self.relations {
            relation.source_range = map(relation.source_range.clone());
            relation.connector_range = map(relation.connector_range.clone());
            relation.label_range = relation.label_range.take().map(map);
        }
    }
}
