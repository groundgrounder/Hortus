//! 类图的三段式卡片、命名空间分组与关系端点矢量。

use crate::class::*;
use crate::graph_router::route_uml;
use crate::text::TextLayout;
use crate::uml::{self, Layer, Palette};
use crate::{
    Color, Connector, Edge, EdgeLine, FontMetrics, Graph, HitRegion, LayoutError, LayoutOptions,
    Marker, Metrics, Point, Primitive, Rect, Scene, Shape,
};
use std::ops::Range;
use std::time::Instant;

const NAME_SIZE: f64 = 15.0;
const STEREOTYPE_SIZE: f64 = 12.6;
const MEMBER_SIZE: f64 = 13.2;
const MEMBER_LINE: f64 = 17.5;
const NOTE_SIZE: f64 = 13.0;
const NOTE_LINE: f64 = 17.0;
const NOTE_GAP: f64 = 15.0;
const MIN_CARD: f64 = 132.0;
const MAX_CARD: f64 = 280.0;
const NS_PAD: f64 = 10.0;
const NS_TOP: f64 = 28.0;
const BLOCK_GAP: f64 = 40.0;

struct Cell {
    layer: Layer,
    /// 卡片在 cell 内的矩形；类命中区域使用它。
    rect: Rect,
    notes: Vec<(Rect, Range<usize>, String)>,
}

impl Cell {
    fn size(&self) -> (f64, f64) {
        (self.layer.width, self.layer.height)
    }
}

struct Layout<'a> {
    diagram: &'a ClassDiagram,
    fonts: &'a mut dyn FontMetrics,
    palette: Palette,
    next_path: usize,
}

fn text_width(text: &TextLayout) -> f64 {
    uml::text_width(text)
}

impl Layout<'_> {
    fn text(
        &mut self,
        text: &str,
        max_width: f64,
        size: f64,
        weight: u16,
        color: Color,
        line_height: f64,
    ) -> TextLayout {
        uml::text_block(
            text,
            max_width,
            size,
            weight,
            color,
            line_height,
            self.fonts,
        )
    }

    fn style(&self, class: &ClassNode) -> Option<&crate::Style> {
        class
            .class
            .as_ref()
            .and_then(|name| self.diagram.class_defs.get(name))
    }

    fn card(&mut self, class: &ClassNode, limit: f64) -> (Layer, Rect) {
        let style = self.style(class);
        let (fill, stroke, color) = self.palette.card(style);
        let limit = limit.clamp(72.0, MAX_CARD);
        let text_limit = (limit - 2.0 * uml::CARD_PAD).max(48.0);
        let stereotype = class
            .stereotype
            .as_ref()
            .map(|stereotype| format!("«{stereotype}»"));
        let stereotype_text = self.text(
            stereotype.as_deref().unwrap_or(""),
            text_limit,
            STEREOTYPE_SIZE,
            500,
            self.palette.muted,
            15.0,
        );
        let name = self.text(
            &crate::wikilink::display(&class.label),
            text_limit,
            NAME_SIZE,
            600,
            color,
            19.0,
        );
        let fields = class
            .fields
            .iter()
            .map(|member| {
                self.text(
                    &member.text,
                    text_limit,
                    MEMBER_SIZE,
                    400,
                    color,
                    MEMBER_LINE,
                )
            })
            .collect::<Vec<_>>();
        let methods = class
            .methods
            .iter()
            .map(|member| {
                self.text(
                    &member.text,
                    text_limit,
                    MEMBER_SIZE,
                    400,
                    color,
                    MEMBER_LINE,
                )
            })
            .collect::<Vec<_>>();
        let mut natural = text_width(&name).max(text_width(&stereotype_text));
        for block in fields.iter().chain(methods.iter()) {
            natural = natural.max(text_width(block));
        }
        let width = (natural + 2.0 * uml::CARD_PAD).clamp(MIN_CARD.min(limit), limit);
        let mut y = 7.0;
        if stereotype_text.height > 0.0 {
            y += stereotype_text.height;
        }
        let name_top = y;
        y += name.height.max(20.0);
        let header = y + 8.0;
        let section_height = |blocks: &[TextLayout]| {
            if blocks.is_empty() {
                0.0
            } else {
                6.0 + blocks.iter().map(|block| block.height + 2.0).sum::<f64>() + 4.0
            }
        };
        let fields_height = section_height(&fields);
        let methods_height = section_height(&methods);
        let height = header + fields_height + methods_height + 1.0;
        let mut layer = Layer {
            width,
            height,
            ..Layer::default()
        };
        layer.shape(
            Rect {
                width,
                height,
                ..Rect::default()
            },
            Shape::Rounded,
            fill,
            stroke,
            1.0,
        );
        if stereotype_text.height > 0.0 {
            layer.text(stereotype_text, uml::CARD_PAD, 7.0);
        }
        layer.text(name, uml::CARD_PAD, name_top);
        let mut top = header;
        for blocks in [&fields, &methods] {
            if blocks.is_empty() {
                continue;
            }
            layer.rule(
                Point { x: 0.0, y: top },
                Point { x: width, y: top },
                self.palette.line,
            );
            top += 6.0;
            for block in blocks {
                layer.text(block.clone(), uml::CARD_PAD, top);
                top += block.height + 2.0;
            }
            top += 4.0;
        }
        (
            layer,
            Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
        )
    }

    fn cell(&mut self, class: &ClassNode, notes: &[(usize, &ClassNote)], limit: f64) -> Cell {
        let (card, rect) = self.card(class, limit);
        if notes.is_empty() {
            let mut layer = card;
            layer.width = rect.width;
            layer.height = rect.height;
            let link = crate::wikilink::extract(&class.label);
            layer.hit_with_link(
                format!("class-{}", class.id),
                crate::wikilink::display(&class.label).into_owned(),
                rect,
                class.source_range.clone(),
                link.as_ref(),
            );
            return Cell {
                layer,
                rect,
                notes: Vec::new(),
            };
        }
        let space = limit - rect.width - NOTE_GAP;
        let beside = space >= 96.0;
        let note_limit = if beside {
            space.min(168.0)
        } else {
            (limit - 2.0 * uml::CARD_PAD).clamp(80.0, 168.0)
        };
        let mut prepared = Vec::new();
        for &(index, note) in notes {
            let text = self.text(
                &note.text,
                note_limit - 2.0 * uml::CARD_PAD,
                NOTE_SIZE,
                400,
                self.palette.text,
                NOTE_LINE,
            );
            let width = (text_width(&text) + 2.0 * uml::CARD_PAD)
                .max(64.0)
                .min(note_limit);
            let height = text.height.max(NOTE_LINE) + 16.0;
            prepared.push((index, note, text, width, height));
        }
        let note_width = prepared
            .iter()
            .map(|(_, _, _, width, _)| *width)
            .fold(0.0, f64::max);
        let note_height = prepared
            .iter()
            .map(|(_, _, _, _, height)| *height)
            .sum::<f64>()
            + NOTE_GAP * prepared.len().saturating_sub(1) as f64;
        let (width, height, card_at) = if beside {
            let height = rect.height.max(note_height);
            (
                rect.width + NOTE_GAP + note_width,
                height,
                Point {
                    x: 0.0,
                    y: (height - rect.height) / 2.0,
                },
            )
        } else {
            (
                rect.width.max(note_width),
                rect.height + NOTE_GAP + note_height,
                Point {
                    x: ((rect.width.max(note_width)) - rect.width) / 2.0,
                    y: 0.0,
                },
            )
        };
        let mut layer = Layer {
            width,
            height,
            ..Layer::default()
        };
        layer.append(card, card_at.x, card_at.y);
        let card_rect = uml::offset_rect(rect, card_at);
        let link = crate::wikilink::extract(&class.label);
        layer.hit_with_link(
            format!("class-{}", class.id),
            crate::wikilink::display(&class.label).into_owned(),
            card_rect,
            class.source_range.clone(),
            link.as_ref(),
        );
        let mut note_rects = Vec::new();
        let mut y = if beside {
            (height - note_height) / 2.0
        } else {
            card_rect.y + card_rect.height + NOTE_GAP
        };
        for (index, note, text, note_width, note_height) in prepared {
            let x = if beside {
                card_rect.x + card_rect.width + NOTE_GAP
            } else {
                (width - note_width) / 2.0
            };
            let rect = Rect {
                x,
                y,
                width: note_width,
                height: note_height,
            };
            let card_anchor = if beside {
                Point {
                    x: card_rect.x + card_rect.width,
                    y: card_rect.y + card_rect.height * 0.62,
                }
            } else {
                Point {
                    x: card_rect.x + card_rect.width / 2.0,
                    y: card_rect.y + card_rect.height,
                }
            };
            let note_anchor = if beside {
                Point {
                    x: rect.x,
                    y: rect.y + rect.height / 2.0,
                }
            } else {
                Point {
                    x: rect.x + rect.width / 2.0,
                    y: rect.y,
                }
            };
            let points = if beside {
                let channel = (card_anchor.x + note_anchor.x) / 2.0;
                vec![
                    card_anchor,
                    Point {
                        x: channel,
                        y: card_anchor.y,
                    },
                    Point {
                        x: channel,
                        y: note_anchor.y,
                    },
                    note_anchor,
                ]
            } else {
                let channel = (card_anchor.y + note_anchor.y) / 2.0;
                vec![
                    card_anchor,
                    Point {
                        x: card_anchor.x,
                        y: channel,
                    },
                    Point {
                        x: note_anchor.x,
                        y: channel,
                    },
                    note_anchor,
                ]
            };
            self.dotted(&mut layer, points, note.source_range.clone());
            layer.shape(
                rect,
                Shape::Rounded,
                self.palette.tone(0.972, 0.268, 0.03, 84.0),
                self.palette.tone(0.84, 0.45, 0.04, 84.0),
                1.0,
            );
            layer.text(text, rect.x + uml::CARD_PAD, rect.y + 8.0);
            layer.hit(
                format!("note-{index}"),
                note.text.clone(),
                rect,
                note.source_range.clone(),
            );
            note_rects.push((rect, note.source_range.clone(), format!("note-{index}")));
            y += note_height + NOTE_GAP;
        }
        Cell {
            layer,
            rect: card_rect,
            notes: note_rects,
        }
    }

    fn dotted(
        &mut self,
        layer: &mut Layer,
        points: Vec<Point>,
        source_range: Range<usize>,
    ) -> usize {
        let edge_id = self.next_path;
        self.next_path += 1;
        layer.primitives.push(Primitive::GraphPath {
            edge_id,
            corner_radius: 6.0,
            points,
            source_range,
            label_range: None,
            line: EdgeLine::Dotted,
            start: Marker::None,
            end: Marker::None,
            color: self.palette.line,
            width: 1.0,
        });
        edge_id
    }

    fn block(&mut self, members: &[usize], namespace: Option<usize>, width: f64) -> Block {
        let margin = if namespace.is_some() { 10.0 } else { uml::SIDE };
        let limit = (width - 2.0 * margin).clamp(72.0, MAX_CARD);
        let mut notes = vec![Vec::new(); self.diagram.classes.len()];
        for (index, note) in self.diagram.notes.iter().enumerate() {
            if let Some(target) = note.target {
                notes[target].push((index, note));
            }
        }
        let mut cells = Vec::with_capacity(members.len());
        for &member in members {
            cells.push(self.cell(&self.diagram.classes[member], &notes[member], limit));
        }
        let mut sizes = vec![(0.0, 0.0); self.diagram.classes.len()];
        for (cell, member) in cells.iter().zip(members) {
            sizes[*member] = cell.size();
        }
        let placement = uml::place_block(
            members,
            self.diagram
                .relations
                .iter()
                .map(|relation| (relation.from, relation.to)),
            &sizes,
            self.diagram.direction,
            width,
            margin,
        );
        let mut positions = placement.positions;
        let mut width = placement.width;
        let mut height = placement.height;
        if namespace.is_some() {
            for position in &mut positions {
                position.x += NS_PAD;
                position.y += NS_TOP;
            }
            width += 2.0 * NS_PAD;
            height += NS_TOP + NS_PAD;
        }
        Block {
            members: members.to_vec(),
            cells,
            positions,
            width,
            height,
            namespace,
        }
    }

    fn marker(&mut self, layer: &mut Layer, port: Point, out: Point, marker: ClassMarker) {
        let perp = Point {
            x: -out.y,
            y: out.x,
        };
        let point = |along: f64, side: f64| Point {
            x: port.x + out.x * along + perp.x * side,
            y: port.y + out.y * along + perp.y * side,
        };
        let color = self.palette.edge;
        let hollow = Color { a: 0.0, ..color };
        match marker {
            ClassMarker::None => {}
            ClassMarker::Arrow => layer.polygon(
                vec![port, point(8.5, 3.8), point(8.5, -3.8)],
                color,
                color,
                1.0,
            ),
            ClassMarker::Triangle => layer.polygon(
                vec![port, point(12.0, 5.4), point(12.0, -5.4)],
                hollow,
                color,
                1.2,
            ),
            ClassMarker::DiamondFilled | ClassMarker::DiamondHollow => {
                let fill = if marker == ClassMarker::DiamondFilled {
                    color
                } else {
                    hollow
                };
                layer.polygon(
                    vec![port, point(7.0, 4.6), point(14.0, 0.0), point(7.0, -4.6)],
                    fill,
                    color,
                    1.2,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cardinality(
        &mut self,
        layer: &mut Layer,
        edge_id: usize,
        port: Point,
        out: Point,
        text: &str,
        avoid: Option<Rect>,
        card: Rect,
    ) {
        if text.is_empty() {
            return;
        }
        let layout = self.text(text, 120.0, 13.4, 500, self.palette.muted, 16.0);
        let width = text_width(&layout) + 6.0;
        let height = layout.height.max(16.0);
        let perp = Point {
            x: -out.y,
            y: out.x,
        };
        let place = |along: f64, side: f64| Rect {
            x: port.x + out.x * along + perp.x * side,
            y: port.y + out.y * along + perp.y * side - height / 2.0,
            width,
            height,
        };
        let candidates = [
            place(18.0, 7.0),
            place(18.0, -(7.0 + width)),
            place(18.0, 44.0),
            place(18.0, -(44.0 + width)),
            place(34.0, 7.0),
            place(34.0, -(7.0 + width)),
        ];
        let fallback = candidates[4];
        let rect = candidates
            .into_iter()
            .find(|rect| {
                !rect.intersects(card) && avoid.is_none_or(|avoid| !rect.intersects(avoid))
            })
            .unwrap_or(fallback);
        layer.primitives.push(Primitive::LabelBackground {
            edge_id,
            rect,
            color: self.palette.background,
        });
        layer.text(layout, rect.x + 3.0, rect.y + 1.0);
    }

    fn global_notes(&mut self, width: f64) -> Layer {
        let notes: Vec<_> = self
            .diagram
            .notes
            .iter()
            .filter(|note| note.target.is_none())
            .collect();
        let mut layer = Layer::default();
        if notes.is_empty() {
            return layer;
        }
        let width = width.min(420.0);
        let mut y = 0.0;
        for note in notes {
            let text = self.text(
                &note.text,
                width - 2.0 * uml::CARD_PAD,
                NOTE_SIZE,
                400,
                self.palette.text,
                NOTE_LINE,
            );
            let height = text.height.max(NOTE_LINE) + 16.0;
            let rect = Rect {
                x: 0.0,
                y,
                width,
                height,
            };
            layer.shape(
                rect,
                Shape::Rounded,
                self.palette.tone(0.972, 0.268, 0.03, 84.0),
                self.palette.tone(0.84, 0.45, 0.04, 84.0),
                1.0,
            );
            layer.text(text, uml::CARD_PAD, y + 8.0);
            y += height + NOTE_GAP;
        }
        layer.width = width;
        layer.height = y - NOTE_GAP;
        layer
    }
}

struct Block {
    members: Vec<usize>,
    cells: Vec<Cell>,
    positions: Vec<Point>,
    width: f64,
    height: f64,
    namespace: Option<usize>,
}

fn endpoint(points: &[Point], at_start: bool) -> Option<(Point, Point)> {
    if points.len() < 2 {
        return None;
    }
    let (port, next) = if at_start {
        (points[0], points[1])
    } else {
        (points[points.len() - 1], points[points.len() - 2])
    };
    let direction = uml::unit(port, next)?;
    Some((port, direction))
}

fn routing_graph(diagram: &ClassDiagram, nodes: &[(String, Range<usize>)]) -> Graph {
    let edges = diagram
        .relations
        .iter()
        .enumerate()
        .map(|(id, relation)| Edge {
            id,
            from: relation.from,
            to: relation.to,
            source_range: relation.source_range.clone(),
            connector_range: relation.connector_range.clone(),
            label: relation.label.clone(),
            label_range: relation.label_range.clone(),
            connector: Connector {
                line: relation.line,
                ..Connector::default()
            },
            visible: true,
            container: None,
        })
        .collect();
    uml::routing_graph(diagram.direction, nodes, edges)
}

pub fn layout_class(
    diagram: &ClassDiagram,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<Scene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if diagram.classes.is_empty() {
        return Err(LayoutError("Class diagram has no classes".into()));
    }
    let palette = Palette::new(options.theme);
    let mut layout = Layout {
        diagram,
        fonts,
        palette,
        next_path: 0,
    };
    let inner = (options.width - 2.0 * uml::SIDE).max(72.0);
    let (vertical, _) = uml::axes(diagram.direction);
    let mut groups: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
    if diagram.namespaces.is_empty() {
        groups.push((None, (0..diagram.classes.len()).collect()));
    } else {
        let root: Vec<usize> = diagram
            .classes
            .iter()
            .enumerate()
            .filter(|(_, class)| class.namespace.is_none())
            .map(|(index, _)| index)
            .collect();
        if !root.is_empty() {
            groups.push((None, root));
        }
        for (index, _) in diagram.namespaces.iter().enumerate() {
            let members = diagram
                .classes
                .iter()
                .enumerate()
                .filter(|(_, class)| class.namespace == Some(index))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if !members.is_empty() {
                groups.push((Some(index), members));
            }
        }
    }
    let count = groups.len().max(1);
    let block_width = if count > 1 && !vertical {
        ((inner - BLOCK_GAP * (count - 1) as f64) / count as f64)
            .max(168.0)
            .min(inner)
    } else {
        inner
    };
    let mut blocks = Vec::new();
    for (namespace, members) in groups {
        let available = if namespace.is_some() {
            (block_width - 2.0 * NS_PAD).max(120.0)
        } else {
            block_width
        };
        blocks.push(layout.block(&members, namespace, available));
    }
    let rows = |blocks: &[Block]| {
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut row = Vec::new();
        let mut width = 0.0;
        for (index, block) in blocks.iter().enumerate() {
            if width > 0.0 && width + BLOCK_GAP + block.width > inner {
                rows.push(std::mem::take(&mut row));
                width = 0.0;
            }
            width += if width > 0.0 {
                BLOCK_GAP + block.width
            } else {
                block.width
            };
            row.push(index);
        }
        if !row.is_empty() {
            rows.push(row);
        }
        rows
    };
    let rows = if vertical {
        blocks
            .iter()
            .enumerate()
            .map(|(index, _)| vec![index])
            .collect::<Vec<_>>()
    } else {
        rows(&blocks)
    };
    let content_width = rows
        .iter()
        .map(|row| {
            row.iter().map(|index| blocks[*index].width).sum::<f64>()
                + BLOCK_GAP * row.len().saturating_sub(1) as f64
        })
        .fold(0.0, f64::max);
    let content_height = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|index| blocks[*index].height)
                .fold(0.0, f64::max)
        })
        .sum::<f64>()
        + BLOCK_GAP * rows.len().saturating_sub(1) as f64;
    let mut offsets = vec![Point::default(); blocks.len()];
    let mut y = 0.0;
    for row in &rows {
        let width = row.iter().map(|index| blocks[*index].width).sum::<f64>()
            + BLOCK_GAP * row.len().saturating_sub(1) as f64;
        let height = row
            .iter()
            .map(|index| blocks[*index].height)
            .fold(0.0, f64::max);
        let mut x = (content_width - width) / 2.0;
        for index in row {
            offsets[*index] = Point { x, y };
            x += blocks[*index].width + BLOCK_GAP;
        }
        y += height + BLOCK_GAP;
    }
    let title = layout.text(
        diagram.title.as_deref().unwrap_or(""),
        options.width - 2.0 * uml::SIDE,
        16.5,
        600,
        palette.text,
        21.0,
    );
    let title_top = if title.height > 0.0 { 18.0 } else { 0.0 };
    let global_notes = layout.global_notes(inner);
    let content_top = title_top + title.height;
    let height = content_top
        + content_height
        + if global_notes.height > 0.0 {
            NOTE_GAP + 6.0 + global_notes.height
        } else {
            0.0
        };
    let mut scene = Layer {
        width: options.width,
        height,
        ..Layer::default()
    };
    scene.primitives.push(Primitive::RoundedRect {
        rect: Rect {
            width: options.width,
            height,
            ..Rect::default()
        },
        radius: 14.0,
        color: palette.background,
    });
    let title_x = (options.width - text_width(&title)).max(0.0) / 2.0;
    let title_width = text_width(&title);
    scene.text(title, title_x, title_top);
    let x_offset = (options.width - content_width) / 2.0;
    let mut namespace_hits = Vec::new();
    for (block, offset) in blocks.iter().zip(&offsets) {
        if let Some(namespace) = block.namespace {
            let rect = Rect {
                x: x_offset + offset.x,
                y: content_top + offset.y,
                width: block.width,
                height: block.height,
            };
            scene.primitives.push(Primitive::GraphShape {
                rect,
                shape: Shape::Rounded,
                outline: crate::shape::outline(Shape::Rounded, rect),
                details: Vec::new(),
                fill: palette.tone(0.975, 0.252, 0.014, 255.0),
                stroke: palette.tone(0.9, 0.38, 0.012, 255.0),
                stroke_width: 1.0,
            });
            let text = layout.text(
                &diagram.namespaces[namespace].id,
                (block.width - 2.0 * NS_PAD).max(40.0),
                13.0,
                600,
                palette.muted,
                16.0,
            );
            let mut title_layer = Layer::default();
            title_layer.text(
                text,
                x_offset + offset.x + NS_PAD,
                content_top + offset.y + 8.0,
            );
            scene.primitives.extend(title_layer.primitives);
            namespace_hits.push(HitRegion {
                label: diagram.namespaces[namespace].id.clone(),
                prefix: None,
                node_id: format!("namespace-{}", diagram.namespaces[namespace].id),
                rect,
                can_collapse: false,
                collapsed: false,
                source_range: diagram.namespaces[namespace].source_range.clone(),
                link_target: None,
                link_heading: None,
            });
        }
    }
    let mut obstacles = Vec::new();
    let mut nodes = Vec::new();
    let mut pending_notes = Vec::new();
    for (block, offset) in blocks.iter().zip(&offsets) {
        for ((cell, position), &member) in
            block.cells.iter().zip(&block.positions).zip(&block.members)
        {
            let at = Point {
                x: x_offset + offset.x + position.x,
                y: content_top + offset.y + position.y,
            };
            obstacles.push((uml::offset_rect(cell.rect, at), Shape::Rounded));
            nodes.push((
                format!("class-{}", diagram.classes[member].id),
                diagram.classes[member].source_range.clone(),
            ));
            for (rect, range, id) in &cell.notes {
                pending_notes.push((uml::offset_rect(*rect, at), range.clone(), id.clone()));
            }
        }
    }
    for (rect, range, id) in pending_notes {
        obstacles.push((rect, Shape::Rounded));
        nodes.push((id, range));
    }
    let graph = routing_graph(diagram, &nodes);
    let (_, _, accepted) = crate::graph_layout::assign_ranks(&graph);
    let labels: Vec<_> = diagram
        .relations
        .iter()
        .map(|relation| {
            layout.text(
                relation.label.as_deref().unwrap_or(""),
                uml::EDGE_LABEL_LIMIT,
                13.4,
                500,
                palette.text,
                16.5,
            )
        })
        .collect();
    let label_sizes: Vec<_> = labels
        .iter()
        .map(|text| (text.height > 0.0).then(|| (text_width(text) + 8.0, text.height + 4.0)))
        .collect();
    let routes = if graph.edges.is_empty() {
        None
    } else {
        Some(route_uml(
            &graph,
            &obstacles,
            &accepted,
            (scene.width, scene.height),
            &label_sizes,
        )?)
    };
    let mut edge_layer = Layer::default();
    if let Some(routes) = routes {
        for (index, relation) in diagram.relations.iter().enumerate() {
            let points = routes.paths[index].clone();
            if points.len() < 2 {
                continue;
            }
            let edge_id = layout.next_path;
            layout.next_path += 1;
            edge_layer.primitives.push(Primitive::GraphPath {
                edge_id,
                corner_radius: 6.0,
                points: points.clone(),
                source_range: relation.source_range.clone(),
                label_range: relation.label_range.clone(),
                line: relation.line,
                start: Marker::None,
                end: Marker::None,
                color: palette.edge,
                width: 1.25,
            });
            let label_rect = routes.labels[index];
            if let Some((port, out)) = endpoint(&points, true) {
                layout.marker(&mut edge_layer, port, out, relation.start);
                if let Some(text) = relation.from_cardinality.as_deref() {
                    layout.cardinality(
                        &mut edge_layer,
                        edge_id,
                        port,
                        out,
                        text,
                        label_rect,
                        obstacles[relation.from].0,
                    );
                }
            }
            if let Some((port, out)) = endpoint(&points, false) {
                layout.marker(&mut edge_layer, port, out, relation.end);
                if let Some(text) = relation.to_cardinality.as_deref() {
                    layout.cardinality(
                        &mut edge_layer,
                        edge_id,
                        port,
                        out,
                        text,
                        label_rect,
                        obstacles[relation.to].0,
                    );
                }
            }
            let hit_rect = label_rect.unwrap_or_else(|| uml::path_hit_rect(&points));
            if let Some(rect) = label_rect {
                edge_layer.primitives.push(Primitive::LabelBackground {
                    edge_id,
                    rect,
                    color: palette.background,
                });
                edge_layer.text(labels[index].clone(), rect.x + 4.0, rect.y + 2.0);
            }
            let label = relation
                .label
                .clone()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "{} → {}",
                        diagram.classes[relation.from].label, diagram.classes[relation.to].label
                    )
                });
            edge_layer.hit(
                format!("relation-{index}"),
                label,
                hit_rect,
                relation.source_range.clone(),
            );
        }
    }
    scene.append(edge_layer, 0.0, 0.0);
    for (block, offset) in blocks.iter_mut().zip(&offsets) {
        for (cell, position) in block.cells.iter_mut().zip(&block.positions) {
            scene.append(
                std::mem::take(&mut cell.layer),
                x_offset + offset.x + position.x,
                content_top + offset.y + position.y,
            );
        }
    }
    scene.hits.extend(namespace_hits);
    if global_notes.height > 0.0 {
        scene.append(
            global_notes,
            x_offset,
            content_top + content_height + NOTE_GAP + 6.0,
        );
    }
    let card_rects: Vec<_> = obstacles.iter().map(|(rect, _)| *rect).collect();
    let min_font_px = scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Text(span) => Some(span.font_size),
            _ => None,
        })
        .reduce(f64::min)
        .unwrap_or(MEMBER_SIZE);
    Ok(Scene {
        width: options.width,
        height,
        primitives: scene.primitives,
        hit_regions: scene.hits,
        metrics: Metrics {
            columns: 1,
            diagram_height: height,
            min_font_px,
            horizontal_overflow: content_width > options.width + 0.001
                || title_width > options.width,
            node_overlaps: uml::overlap_count(&card_rects),
            fonts: layout.fonts.used_fonts(),
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue: None,
            branch_hues: Vec::new(),
            diagram_kind: "class".into(),
        },
    })
}
