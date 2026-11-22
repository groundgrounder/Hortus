//! 状态、复合状态及附属便签的度量和绘制。

use super::*;

const PAD: f64 = 12.0;
const NOTE_GAP: f64 = 16.0;
const REGION_GAP: f64 = 20.0;

pub(super) struct Cell {
    pub layer: Layer,
    pub rect: Rect,
    pub shape: Shape,
    pub notes: Vec<PlacedNote>,
}

pub(super) struct PlacedNote {
    pub rect: Rect,
    pub source_range: Range<usize>,
}

impl Layout<'_> {
    pub(super) fn cell(
        &mut self,
        node: &StateNode,
        notes: &[(usize, &StateNote)],
        key: String,
        limit: f64,
        vertical: bool,
    ) -> Result<Cell, LayoutError> {
        let link = matches!(node.kind, StateKind::Ordinary | StateKind::Composite(_))
            .then(|| crate::wikilink::extract(&node.label))
            .flatten();
        let label = match node.kind {
            StateKind::Start => "[*] — start",
            StateKind::End => "[*] — end",
            _ => link
                .as_ref()
                .map_or(node.label.as_str(), |link| link.display.as_str()),
        };
        let mut cell = match node.kind {
            StateKind::Ordinary | StateKind::Composite(_) => {
                self.named_cell(node, &key, limit, vertical)?
            }
            _ => self.pseudo_cell(node, vertical),
        };
        cell.layer.hit_with_link(
            key.clone(),
            label.to_owned(),
            cell.rect,
            node.source_range.clone(),
            link.as_ref(),
        );
        self.place_notes(&mut cell, notes, &key, limit);
        Ok(cell)
    }

    fn named_cell(
        &mut self,
        node: &StateNode,
        key: &str,
        limit: f64,
        vertical: bool,
    ) -> Result<Cell, LayoutError> {
        let style = node
            .class
            .as_ref()
            .and_then(|id| self.diagram.class_defs.get(id));
        let hue = |value: Option<&String>, fallback| {
            value
                .and_then(|value| Color::parse_css(value))
                .map_or(fallback, Color::hue)
        };
        let fill_hue = hue(style.and_then(|s| s.fill.as_ref()), 255.0);
        let stroke_hue = hue(style.and_then(|s| s.stroke.as_ref()), fill_hue);
        let text_hue = hue(style.and_then(|s| s.color.as_ref()), 255.0);
        let palette = self.palette;
        let composite = matches!(node.kind, StateKind::Composite(_));
        let fill = if composite {
            palette.tone(0.972, 0.248, 0.016, fill_hue)
        } else if style.and_then(|s| s.fill.as_ref()).is_some() {
            palette.tone(0.954, 0.30, 0.033, fill_hue)
        } else {
            palette.tone(0.998, 0.275, 0.008, fill_hue)
        };
        let stroke = if style.and_then(|s| s.stroke.as_ref()).is_some() {
            palette.tone(0.73, 0.51, 0.07, stroke_hue)
        } else {
            palette.tone(0.83, 0.42, 0.016, stroke_hue)
        };
        let color = palette.tone(
            0.25,
            0.91,
            if style.is_some() { 0.044 } else { 0.012 },
            text_hue,
        );

        let text_limit =
            (limit.min(if composite { f64::INFINITY } else { 224.0 }) - 2.0 * PAD).max(40.0);
        let title = self.text(
            &crate::wikilink::display(&node.label),
            text_limit,
            15.0,
            550,
            color,
        );
        let descriptions = self.text(
            &node.descriptions.join("\n"),
            text_limit,
            TEXT_SIZE,
            400,
            palette.muted,
        );
        let mut width = (text_width(&title).max(text_width(&descriptions)) + 2.0 * PAD).max(64.0);
        let title_height = title.height.max(20.0);
        let has_descriptions = descriptions.height > 0.0;
        let description_top = 12.0 + title_height + 8.0;
        let header = if has_descriptions {
            description_top + descriptions.height + 8.0
        } else {
            title_height + 20.0
        };
        let children = if let StateKind::Composite(regions) = &node.kind {
            self.children(regions, key, (limit - 2.0 * PAD).max(72.0), fill, vertical)?
        } else {
            Layer::default()
        };
        let height = header + children.height + if composite { PAD } else { 0.0 };
        if composite {
            width = width.max(children.width + 2.0 * PAD);
        }
        let rect = Rect {
            width,
            height,
            ..Rect::default()
        };
        let mut layer = Layer {
            width,
            height,
            ..Layer::default()
        };
        let shape = if composite {
            Shape::Container
        } else {
            Shape::Rounded
        };
        layer.shape(rect, shape, fill, stroke, 1.0);
        layer.text(title, PAD, 9.0);
        if has_descriptions {
            layer.rule(
                Point {
                    x: PAD,
                    y: description_top - 5.0,
                },
                Point {
                    x: width - PAD,
                    y: description_top - 5.0,
                },
                palette.line,
            );
            layer.text(descriptions, PAD, description_top);
        }
        if composite {
            layer.rule(
                Point { x: 1.0, y: header },
                Point {
                    x: width - 1.0,
                    y: header,
                },
                palette.line,
            );
            let x = (width - children.width) / 2.0;
            layer.append(children, x, header + 3.0);
        }
        Ok(Cell {
            layer,
            rect,
            shape,
            notes: Vec::new(),
        })
    }

    fn pseudo_cell(&self, node: &StateNode, vertical: bool) -> Cell {
        let (width, height, shape) = match node.kind {
            StateKind::Start => (15.0, 15.0, Shape::Circle),
            StateKind::End => (20.0, 20.0, Shape::Circle),
            StateKind::Choice => (30.0, 30.0, Shape::Diamond),
            _ if vertical => (62.0, 6.0, Shape::Stadium),
            _ => (6.0, 62.0, Shape::Stadium),
        };
        let rect = Rect {
            width,
            height,
            ..Rect::default()
        };
        let ink = self.palette.tone(0.36, 0.75, 0.026, 255.0);
        let fill = if matches!(node.kind, StateKind::End | StateKind::Choice) {
            self.palette.background
        } else {
            ink
        };
        let mut layer = Layer {
            width,
            height,
            ..Layer::default()
        };
        layer.shape(rect, shape, fill, ink, 1.2);
        if node.kind == StateKind::End {
            // 内圆是实心图元，SVG 与原生场景都保留双圈的填充语义。
            layer.shape(
                Rect {
                    x: 4.5,
                    y: 4.5,
                    width: 11.0,
                    height: 11.0,
                },
                Shape::Circle,
                ink,
                ink,
                0.0,
            );
        }
        Cell {
            layer,
            rect,
            shape,
            notes: Vec::new(),
        }
    }

    fn children(
        &mut self,
        regions: &[StateRegion],
        key: &str,
        limit: f64,
        background: Color,
        vertical: bool,
    ) -> Result<Layer, LayoutError> {
        let across = vertical && (limit + REGION_GAP) / regions.len().max(1) as f64 >= 180.0;
        let child_limit = if across {
            (limit + REGION_GAP) / regions.len().max(1) as f64 - REGION_GAP
        } else {
            limit
        };
        let parts: Vec<_> = regions
            .iter()
            .enumerate()
            .map(|(index, region)| {
                self.prepare_region(
                    region,
                    &format!("{key}/region-{index}/"),
                    child_limit,
                    background,
                )
            })
            .collect::<Result<_, _>>()?;
        let mut layer = Layer::default();
        if across {
            layer.height = parts.iter().map(|part| part.height).fold(0.0, f64::max);
            for (index, part) in parts.into_iter().enumerate() {
                if index > 0 {
                    let at = layer.width + REGION_GAP / 2.0;
                    dashed_rule(&mut layer, at, true, self.palette.line);
                    layer.width += REGION_GAP;
                }
                let (x, width) = (layer.width, part.width.max(42.0));
                layer.append(part, x, 0.0);
                layer.width += width;
            }
        } else {
            layer.width = parts.iter().map(|part| part.width).fold(0.0, f64::max);
            for (index, part) in parts.into_iter().enumerate() {
                if index > 0 {
                    let at = layer.height + REGION_GAP / 2.0;
                    dashed_rule(&mut layer, at, false, self.palette.line);
                    layer.height += REGION_GAP;
                }
                let (y, height) = (layer.height, part.height.max(24.0));
                let x = (layer.width - part.width) / 2.0;
                layer.append(part, x, y);
                layer.height += height;
            }
        }
        Ok(layer)
    }

    fn place_notes(
        &mut self,
        cell: &mut Cell,
        notes: &[(usize, &StateNote)],
        key: &str,
        limit: f64,
    ) {
        if notes.is_empty() {
            return;
        }
        let left = notes
            .iter()
            .any(|(_, note)| note.side == StateNoteSide::Left);
        let right = notes
            .iter()
            .any(|(_, note)| note.side == StateNoteSide::Right);
        let sides = usize::from(left) + usize::from(right);
        let side_width = (limit - cell.rect.width) / sides as f64 - NOTE_GAP;
        let beside = side_width >= 70.0;
        let note_limit = if beside { side_width } else { limit - NOTE_GAP }.clamp(64.0, 184.0);
        let mut groups = [Vec::new(), Vec::new()];
        for &(index, note) in notes {
            let text = self.text(
                &note.text,
                note_limit - 2.0 * PAD,
                TEXT_SIZE,
                400,
                self.palette.muted,
            );
            let w = (text_width(&text) + 2.0 * PAD).max(54.0);
            groups[usize::from(note.side == StateNoteSide::Right)].push((index, note, text, w));
        }
        let widths = groups
            .each_ref()
            .map(|group| group.iter().map(|(_, _, _, w)| *w).fold(0.0, f64::max));
        let heights = groups.each_ref().map(|group| {
            group
                .iter()
                .map(|(_, _, text, _)| text.height.max(TEXT_LINE) + 18.0)
                .sum::<f64>()
                + NOTE_GAP * group.len().saturating_sub(1) as f64
        });
        let mut layer = Layer::default();
        let state = if beside {
            let x = if left { widths[0] + NOTE_GAP } else { 0.0 };
            layer.width = x + cell.rect.width + if right { widths[1] + NOTE_GAP } else { 0.0 };
            layer.height = cell.rect.height.max(heights[0]).max(heights[1]);
            Point {
                x,
                y: (layer.height - cell.rect.height) / 2.0,
            }
        } else {
            layer.width = cell.rect.width.max(widths[0]).max(widths[1]) + NOTE_GAP;
            layer.height = cell.rect.height + heights[0] + heights[1] + NOTE_GAP * sides as f64;
            Point {
                x: (layer.width - cell.rect.width) / 2.0,
                y: 0.0,
            }
        };
        cell.rect = offset_rect(cell.rect, state);
        let mut below = cell.rect.height;
        for (side, group) in groups.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let left_side = side == 0;
            let start_x = if left_side {
                cell.rect.x
            } else {
                cell.rect.x + cell.rect.width
            };
            let start_y = cell.rect.y + cell.rect.height * 0.68;
            let mut y = if beside {
                (layer.height - heights[side]) / 2.0
            } else {
                below + NOTE_GAP
            };
            for (index, note, text, width) in group {
                let inset = if beside { 0.0 } else { NOTE_GAP / 2.0 };
                let x = if left_side {
                    inset
                } else {
                    layer.width - width - inset
                };
                let rect = Rect {
                    x,
                    y,
                    width,
                    height: text.height.max(TEXT_LINE) + 18.0,
                };
                let end_x = if left_side == beside {
                    rect.x + rect.width
                } else {
                    rect.x
                };
                let end_y = rect.y + rect.height / 2.0;
                let points = if beside {
                    let channel = (start_x + end_x) / 2.0;
                    vec![
                        Point {
                            x: start_x,
                            y: start_y,
                        },
                        Point {
                            x: channel,
                            y: start_y,
                        },
                        Point {
                            x: channel,
                            y: end_y,
                        },
                        Point { x: end_x, y: end_y },
                    ]
                } else {
                    let channel = if left_side { 4.0 } else { layer.width - 4.0 };
                    vec![
                        Point {
                            x: start_x,
                            y: start_y,
                        },
                        Point {
                            x: channel,
                            y: start_y,
                        },
                        Point {
                            x: channel,
                            y: end_y,
                        },
                        Point { x: end_x, y: end_y },
                    ]
                };
                self.path(
                    &mut layer,
                    points,
                    PathMark::Dotted(note.source_range.clone()),
                );
                layer.shape(
                    rect,
                    Shape::Rounded,
                    self.palette.tone(0.968, 0.274, 0.034, 84.0),
                    self.palette.tone(0.84, 0.45, 0.045, 84.0),
                    1.0,
                );
                layer.text(text, x + PAD, y + 9.0);
                layer.hit(
                    format!("{key}/note-{index}"),
                    note.text.clone(),
                    rect,
                    note.source_range.clone(),
                );
                cell.notes.push(PlacedNote {
                    rect,
                    source_range: note.source_range.clone(),
                });
                y += rect.height + NOTE_GAP;
            }
            below = y - NOTE_GAP;
        }
        layer.append(std::mem::take(&mut cell.layer), state.x, state.y);
        cell.layer = layer;
    }
}

fn dashed_rule(layer: &mut Layer, at: f64, vertical: bool, color: Color) {
    let length = if vertical { layer.height } else { layer.width };
    let point = |offset| {
        if vertical {
            Point { x: at, y: offset }
        } else {
            Point { x: offset, y: at }
        }
    };
    let mut offset = 6.0;
    while offset + 4.0 < length - 6.0 {
        layer.rule(point(offset), point(offset + 4.0), color);
        offset += 9.0;
    }
}
