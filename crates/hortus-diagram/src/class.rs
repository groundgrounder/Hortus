//! 类图的语义结构：类、成员、关系、命名空间与便签。

use crate::{Direction, EdgeLine, Style};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClassVisibility {
    #[default]
    Public,
    Private,
    Protected,
    Package,
}

impl ClassVisibility {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Public => "+",
            Self::Private => "-",
            Self::Protected => "#",
            Self::Package => "~",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClassMemberKind {
    #[default]
    Field,
    Method,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassMember {
    /// 去除首尾空白后的原始成员文本，包含可见性前缀与后缀修饰符。
    pub text: String,
    pub visibility: ClassVisibility,
    pub kind: ClassMemberKind,
    pub abstract_member: bool,
    pub static_member: bool,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassNode {
    pub id: String,
    pub label: String,
    pub stereotype: Option<String>,
    pub fields: Vec<ClassMember>,
    pub methods: Vec<ClassMember>,
    /// `classDef` / `class` / `A:::cls` 指定的样式类。
    pub class: Option<String>,
    pub namespace: Option<usize>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClassMarker {
    #[default]
    None,
    Arrow,
    Triangle,
    DiamondFilled,
    DiamondHollow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassRelation {
    pub from: usize,
    pub to: usize,
    pub start: ClassMarker,
    pub end: ClassMarker,
    pub line: EdgeLine,
    pub from_cardinality: Option<String>,
    pub to_cardinality: Option<String>,
    pub label: Option<String>,
    pub label_range: Option<Range<usize>>,
    pub connector_range: Range<usize>,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassNamespace {
    pub id: String,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassNote {
    pub target: Option<usize>,
    pub text: String,
    pub source_range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassDiagram {
    pub title: Option<String>,
    pub direction: Direction,
    pub classes: Vec<ClassNode>,
    pub relations: Vec<ClassRelation>,
    pub namespaces: Vec<ClassNamespace>,
    pub notes: Vec<ClassNote>,
    pub class_defs: HashMap<String, Style>,
    pub inline_styles: HashMap<String, Style>,
}

impl ClassDiagram {
    pub(crate) fn map_ranges(&mut self, map: impl Fn(Range<usize>) -> Range<usize> + Copy) {
        for class in &mut self.classes {
            class.source_range = map(class.source_range.clone());
            for member in class.fields.iter_mut().chain(class.methods.iter_mut()) {
                member.source_range = map(member.source_range.clone());
            }
        }
        for relation in &mut self.relations {
            relation.source_range = map(relation.source_range.clone());
            relation.connector_range = map(relation.connector_range.clone());
            relation.label_range = relation.label_range.take().map(map);
        }
        for namespace in &mut self.namespaces {
            namespace.source_range = map(namespace.source_range.clone());
        }
        for note in &mut self.notes {
            note.source_range = map(note.source_range.clone());
        }
    }
}
