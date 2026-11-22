//! ER 图的实体卡片、属性三列与鸦脚基数端点。

use crate::er::*;
use crate::graph_router::route_uml;
use crate::text::TextLayout;
use crate::uml::{self, Layer, Palette};
use crate::{
    Color, Connector, Edge, EdgeLine, FontMetrics, LayoutError, LayoutOptions, Marker, Metrics,
    Point, Primitive, Rect, Scene, Shape,
};
use std::time::Instant;

const ENTITY_SIZE: f64 = 15.0;
const ATTRIBUTE_SIZE: f64 = 13.2;
const TYPE_SIZE: f64 = 13.0;
const ROW_LINE: f64 = 17.5;
const KEY_SIZE: f64 = 12.6;
const MIN_CARD: f64 = 160.0;
const MAX_CARD: f64 = 320.0;
const COLUMN_GAP: f64 = 9.0;

struct Cell {
    layer: Layer,
    rect: Rect,
}

struct Layout<'a> {
    diagram: &'a ErDiagram,
    fonts: &'a mut dyn FontMetrics,
    palette: Palette,
    next_path: usize,
}

fn text_width(text: &TextLayout) -> f64 {
    uml::text_width(text)
}

fn attribute_text(attribute: &ErAttribute) -> String {
    match &attribute.comment {
        Some(comment) if !comment.is_empty() => format!("{} · {}", attribute.kind, comment),
        _ => attribute.kind.clone(),
    }
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

    fn card(&mut self, entity: &ErEntity, limit: f64) -> Cell {
        let style = entity
            .class
            .as_ref()
            .and_then(|name| self.diagram.class_defs.get(name));
        let (fill, stroke, color) = self.palette.card(style);
        let limit = limit.clamp(72.0, MAX_CARD);
        let text_limit = (limit - 2.0 * uml::CARD_PAD).max(60.0);
        let title = self.text(&entity.label, text_limit, ENTITY_SIZE, 600, color, 19.0);
        let keys: Vec<String> = entity
            .attributes
            .iter()
            .map(|attribute| {
                attribute
                    .keys
                    .iter()
                    .map(|key| key.label())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        let key_layouts = keys
            .iter()
            .map(|key| self.text(key, text_limit, KEY_SIZE, 700, self.palette.accent, 14.0))
            .collect::<Vec<_>>();
        let key_width = key_layouts.iter().map(text_width).fold(0.0, f64::max).max(
            if entity.attributes.is_empty() {
                0.0
            } else {
                26.0
            },
        );
        let name_natural = entity
            .attributes
            .iter()
            .map(|attribute| {
                self.text(
                    &attribute.name,
                    10_000.0,
                    ATTRIBUTE_SIZE,
                    500,
                    color,
                    ROW_LINE,
                )
            })
            .map(|text| text_width(&text))
            .fold(0.0, f64::max);
        let type_natural = entity
            .attributes
            .iter()
            .map(|attribute| {
                self.text(
                    &attribute_text(attribute),
                    10_000.0,
                    TYPE_SIZE,
                    400,
                    self.palette.muted,
                    ROW_LINE,
                )
            })
            .map(|text| text_width(&text))
            .fold(0.0, f64::max);
        let available_name =
            (limit - 2.0 * uml::CARD_PAD - key_width - 2.0 * COLUMN_GAP - 56.0).max(40.0);
        let name_width = name_natural.min(available_name);
        let natural =
            text_width(&title).max(key_width + COLUMN_GAP + name_width + COLUMN_GAP + type_natural);
        let width = (natural + 2.0 * uml::CARD_PAD).clamp(MIN_CARD.min(limit), limit);
        let type_limit =
            (width - 2.0 * uml::CARD_PAD - key_width - 2.0 * COLUMN_GAP - name_width).max(56.0);
        let name_layouts = entity
            .attributes
            .iter()
            .map(|attribute| {
                self.text(
                    &attribute.name,
                    name_width,
                    ATTRIBUTE_SIZE,
                    500,
                    color,
                    ROW_LINE,
                )
            })
            .collect::<Vec<_>>();
        let type_layouts = entity
            .attributes
            .iter()
            .map(|attribute| {
                self.text(
                    &attribute_text(attribute),
                    type_limit,
                    TYPE_SIZE,
                    400,
                    self.palette.muted,
                    ROW_LINE,
                )
            })
            .collect::<Vec<_>>();
        let row_heights: Vec<f64> = name_layouts
            .iter()
            .zip(&type_layouts)
            .map(|(name, kind)| name.height.max(kind.height).max(ROW_LINE))
            .collect();
        let mut y = 8.0 + title.height.max(20.0) + 8.0;
        let header = y;
        if !entity.attributes.is_empty() {
            y += 5.0;
            for height in &row_heights {
                y += height + 2.0;
            }
            y += 4.0;
        }
        let height = y + 1.0;
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
        layer.text(title, uml::CARD_PAD, 8.0);
        if !entity.attributes.is_empty() {
            layer.rule(
                Point { x: 0.0, y: header },
                Point {
                    x: width,
                    y: header,
                },
                self.palette.line,
            );
            let mut top = header + 5.0;
            let name_x = uml::CARD_PAD + key_width + COLUMN_GAP;
            let type_x = name_x + name_width + COLUMN_GAP;
            for (index, ((key, key_text), (name, type_layout))) in key_layouts
                .iter()
                .zip(&keys)
                .zip(name_layouts.iter().zip(&type_layouts))
                .enumerate()
            {
                let height = row_heights[index];
                if !key_text.is_empty() {
                    let badge_width = text_width(key) + 8.0;
                    layer.primitives.push(Primitive::RoundedRect {
                        rect: Rect {
                            x: uml::CARD_PAD,
                            y: top + (height - 15.0) / 2.0,
                            width: badge_width,
                            height: 15.0,
                        },
                        radius: 7.0,
                        color: Color {
                            a: 0.14,
                            ..self.palette.accent
                        },
                    });
                    let mut badge = key.clone();
                    crate::text::translate(
                        &mut badge.primitives,
                        uml::CARD_PAD + 4.0,
                        top + (height - 15.0) / 2.0 + 0.5,
                    );
                    layer.text(badge, 0.0, 0.0);
                }
                let mut name = name.clone();
                crate::text::translate(
                    &mut name.primitives,
                    name_x,
                    top + (height - name.height) / 2.0,
                );
                layer.text(name, 0.0, 0.0);
                let mut third = type_layout.clone();
                crate::text::translate(
                    &mut third.primitives,
                    type_x,
                    top + (height - type_layout.height) / 2.0,
                );
                layer.text(third, 0.0, 0.0);
                top += height + 2.0;
            }
        }
        Cell {
            layer,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
        }
    }

    fn marker(&mut self, layer: &mut Layer, port: Point, out: Point, cardinality: ErCardinality) {
        let perp = Point {
            x: -out.y,
            y: out.x,
        };
        let point = |along: f64, side: f64| Point {
            x: port.x + out.x * along + perp.x * side,
            y: port.y + out.y * along + perp.y * side,
        };
        let color = self.palette.edge;
        let bar = |layer: &mut Layer, at: f64| {
            layer.segment(point(at, 4.6), point(at, -4.6), color, 1.3);
        };
        let circle = |layer: &mut Layer, at: f64| {
            let center = point(at, 0.0);
            let radius = 3.6;
            let rect = Rect {
                x: center.x - radius,
                y: center.y - radius,
                width: radius * 2.0,
                height: radius * 2.0,
            };
            layer.shape(rect, Shape::Circle, self.palette.background, color, 1.2);
        };
        let foot = |layer: &mut Layer, near: f64, far: f64, spread: f64| {
            let tip = point(far, 0.0);
            for side in [0.0, spread, -spread] {
                layer.segment(tip, point(near, side), color, 1.2);
            }
        };
        match cardinality {
            ErCardinality::ExactlyOne => {
                bar(layer, 8.0);
                bar(layer, 15.5);
            }
            ErCardinality::ZeroOrOne => {
                bar(layer, 8.0);
                circle(layer, 16.5);
            }
            ErCardinality::OneOrMore => {
                foot(layer, 1.5, 13.0, 6.0);
                bar(layer, 16.5);
            }
            ErCardinality::ZeroOrMore => {
                foot(layer, 1.5, 13.0, 6.0);
                circle(layer, 18.0);
            }
        }
    }
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

pub fn layout_er(
    diagram: &ErDiagram,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<Scene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if diagram.entities.is_empty() {
        return Err(LayoutError("ER diagram has no entities".into()));
    }
    let palette = Palette::new(options.theme);
    let mut layout = Layout {
        diagram,
        fonts,
        palette,
        next_path: 0,
    };
    let inner = (options.width - 2.0 * uml::SIDE).max(72.0);
    let limit = (inner - 2.0 * uml::SIDE).clamp(72.0, MAX_CARD);
    let entities: Vec<usize> = (0..diagram.entities.len()).collect();
    let cells: Vec<_> = diagram
        .entities
        .iter()
        .map(|entity| layout.card(entity, limit))
        .collect();
    let sizes: Vec<_> = cells
        .iter()
        .map(|cell| (cell.layer.width, cell.layer.height))
        .collect();
    let placement = uml::place_block(
        &entities,
        diagram
            .relations
            .iter()
            .map(|relation| (relation.from, relation.to)),
        &sizes,
        diagram.direction,
        inner,
        uml::SIDE,
    );
    let title = layout.text(
        diagram.title.as_deref().unwrap_or(""),
        options.width - 2.0 * uml::SIDE,
        16.5,
        600,
        palette.text,
        21.0,
    );
    let title_top = if title.height > 0.0 { 18.0 } else { 0.0 };
    let content_top = title_top + title.height;
    let height = content_top + placement.height;
    let x_offset = (options.width - placement.width) / 2.0;
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
    let mut obstacles = Vec::new();
    let mut nodes = Vec::new();
    for ((cell, position), entity) in cells
        .iter()
        .zip(&placement.positions)
        .zip(&diagram.entities)
    {
        let at = Point {
            x: x_offset + position.x,
            y: content_top + position.y,
        };
        obstacles.push((uml::offset_rect(cell.rect, at), Shape::Rounded));
        nodes.push((format!("entity-{}", entity.id), entity.source_range.clone()));
    }
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
                line: if relation.identifying {
                    EdgeLine::Solid
                } else {
                    EdgeLine::Dotted
                },
                ..Connector::default()
            },
            visible: true,
            container: None,
        })
        .collect();
    let graph = uml::routing_graph(diagram.direction, &nodes, edges);
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
                line: if relation.identifying {
                    EdgeLine::Solid
                } else {
                    EdgeLine::Dotted
                },
                start: Marker::None,
                end: Marker::None,
                color: palette.edge,
                width: 1.25,
            });
            if let Some((port, out)) = endpoint(&points, true) {
                layout.marker(&mut edge_layer, port, out, relation.from_cardinality);
            }
            if let Some((port, out)) = endpoint(&points, false) {
                layout.marker(&mut edge_layer, port, out, relation.to_cardinality);
            }
            let hit_rect = routes.labels[index].unwrap_or_else(|| uml::path_hit_rect(&points));
            if let Some(rect) = routes.labels[index] {
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
                        diagram.entities[relation.from].label, diagram.entities[relation.to].label
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
    for ((cell, position), entity) in cells
        .into_iter()
        .zip(&placement.positions)
        .zip(&diagram.entities)
    {
        let at = Point {
            x: x_offset + position.x,
            y: content_top + position.y,
        };
        let Cell { mut layer, rect } = cell;
        layer.hit(
            format!("entity-{}", entity.id),
            entity.label.clone(),
            rect,
            entity.source_range.clone(),
        );
        scene.append(layer, at.x, at.y);
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
        .unwrap_or(TYPE_SIZE);
    Ok(Scene {
        width: options.width,
        height,
        primitives: scene.primitives,
        hit_regions: scene.hits,
        metrics: Metrics {
            columns: 1,
            diagram_height: height,
            min_font_px,
            horizontal_overflow: placement.width > options.width + 0.001
                || title_width > options.width,
            node_overlaps: uml::overlap_count(&card_rects),
            fonts: layout.fonts.used_fonts(),
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue: None,
            branch_hues: Vec::new(),
            diagram_kind: "er".into(),
        },
    })
}
