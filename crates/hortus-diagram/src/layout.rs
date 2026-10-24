use crate::text::{RowStyle, Style, row, translate};
use crate::{Color, FontMetrics, HitRegion, Metrics, Point, Primitive, Rect, Scene, Theme, Tree};
use std::collections::HashSet;
use std::fmt;
use std::time::Instant;

const COLUMN_GAP: f64 = 32.0;
const PANEL_GAP: f64 = 24.0;
const SIDE: f64 = 24.0;
const HUES: &[f64] = &[255.0, 45.0, 160.0, 320.0, 85.0, 210.0, 10.0, 130.0];

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    /// 正文栏宽度，含图容器的左右各 24px 内边距。
    pub width: f64,
    pub theme: Theme,
    pub collapsed: HashSet<String>,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            width: 700.0,
            theme: Theme::Light,
            collapsed: HashSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError(pub String);
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LayoutError {}

/// 连续分组的动态规划；各高度已含末尾 24px，和 v2 样稿的目标函数相同。
pub fn partition(heights: &[f64], k: usize) -> Vec<std::ops::Range<usize>> {
    if heights.is_empty() {
        return Vec::new();
    }
    let k = k.clamp(1, heights.len());
    let n = heights.len();
    let mut prefix = vec![0.0; n + 1];
    for (i, h) in heights.iter().enumerate() {
        prefix[i + 1] = prefix[i] + h;
    }
    let mut best = vec![vec![f64::INFINITY; n + 1]; k + 1];
    let mut cut = vec![vec![0; n + 1]; k + 1];
    best[0][0] = 0.0;
    for c in 1..=k {
        for i in c..=n {
            for j in c - 1..i {
                let cost = best[c - 1][j].max(prefix[i] - prefix[j]);
                // 并列时取靠后的切点：前面的栏多放，空位落在右下角，和普通分栏排版一致；
                // 容差吸收前缀和相减的浮点误差，让等高分支稳定判为并列
                if cost <= best[c][i] + 1e-6 {
                    best[c][i] = best[c][i].min(cost);
                    cut[c][i] = j;
                }
            }
        }
    }
    let mut result = Vec::new();
    let mut i = n;
    for c in (1..=k).rev() {
        result.push(cut[c][i]..i);
        i = cut[c][i];
    }
    result.reverse();
    result
}

struct Palette {
    text: Color,
    secondary: Color,
    muted: Color,
    rule: Color,
    background: Color,
    dark: bool,
}
impl Palette {
    fn new(theme: Theme) -> Self {
        let dark = theme == Theme::Dark;
        Self {
            text: Color::oklch(
                if dark { 0.91 } else { 0.25 },
                if dark { 0.008 } else { 0.01 },
                260.0,
            ),
            secondary: Color::oklch(
                if dark { 0.80 } else { 0.38 },
                if dark { 0.008 } else { 0.01 },
                260.0,
            ),
            muted: Color::oklch(if dark { 0.66 } else { 0.50 }, 0.01, 260.0),
            rule: Color::oklch(
                if dark { 0.32 } else { 0.90 },
                if dark { 0.01 } else { 0.005 },
                260.0,
            ),
            background: Color::oklch(
                if dark { 0.215 } else { 0.982 },
                if dark { 0.01 } else { 0.004 },
                260.0,
            ),
            dark,
        }
    }
    fn title(&self, hue: f64) -> Color {
        Color::oklch(
            if self.dark { 0.78 } else { 0.48 },
            if self.dark { 0.085 } else { 0.1 },
            hue,
        )
    }
    fn guide(&self, hue: f64) -> Color {
        Color::oklch(
            if self.dark { 0.34 } else { 0.89 },
            if self.dark { 0.03 } else { 0.025 },
            hue,
        )
    }
}

struct Panel {
    primitives: Vec<Primitive>,
    hits: Vec<HitRegion>,
    height: f64,
}

struct PanelLayout<'a> {
    tree: &'a Tree,
    options: &'a LayoutOptions,
    palette: &'a Palette,
    fonts: &'a mut dyn FontMetrics,
    width: f64,
    hue: f64,
    panel: Panel,
}

struct Frame {
    index: usize,
    depth: usize,
    bottom: f64,
    top: f64,
    next_child: usize,
    guide: bool,
}

impl PanelLayout<'_> {
    /// 单层排版：递归会让极深单链耗尽线程栈，这里改为显式栈。
    fn enter(&mut self, index: usize, depth: usize, y: f64) -> Frame {
        let node = &self.tree.nodes[index];
        let head = depth == 0;
        // 极深树仍保留可读文字宽度，正常深度完全按样稿的 17.5px 缩进。
        let x = (depth as f64 * 17.5).min((self.width - 48.0).max(0.0));
        let has_children = !node.children.is_empty();
        let collapsed = has_children && self.options.collapsed.contains(&node.id);
        let color = if head {
            self.palette.title(self.hue)
        } else if has_children {
            self.palette.text
        } else {
            self.palette.secondary
        };
        let mut text = row(
            &node.label,
            node.prefix.as_deref(),
            collapsed.then_some(node.children.len()),
            RowStyle {
                main: Style {
                    size: if head { 16.5 } else { 15.0 },
                    weight: if head {
                        600
                    } else if has_children {
                        500
                    } else {
                        400
                    },
                    color,
                },
                muted: self.palette.muted,
                badge_color: self.palette.title(self.hue),
                line_height: if head { 24.0 } else { 23.0 },
                root: false,
            },
            self.width - x,
            self.fonts,
        );
        let height = text.height + 6.0;
        translate(&mut text.primitives, x, y + if head { 0.0 } else { 3.0 });
        self.panel.primitives.extend(text.primitives);
        self.panel.hits.push(HitRegion {
            node_id: node.id.clone(),
            label: node.label.clone(),
            prefix: node.prefix.clone(),
            rect: Rect {
                x,
                y,
                width: self.width - x,
                height,
            },
            can_collapse: has_children,
            collapsed,
            source_range: node.source_range.clone(),
        });
        let bottom = y + height;
        Frame {
            index,
            depth,
            bottom,
            top: bottom,
            next_child: 0,
            guide: has_children && !collapsed,
        }
    }

    fn node(&mut self, index: usize, depth: usize, y: f64) -> f64 {
        let mut stack = vec![self.enter(index, depth, y)];
        loop {
            let next = {
                let frame = stack.last_mut().unwrap();
                let children = &self.tree.nodes[frame.index].children;
                if frame.guide && frame.next_child < children.len() {
                    let child = children[frame.next_child];
                    frame.next_child += 1;
                    Some((child, frame.depth + 1, frame.bottom))
                } else {
                    None
                }
            };
            if let Some((child, child_depth, child_y)) = next {
                stack.push(self.enter(child, child_depth, child_y));
                continue;
            }
            let mut frame = stack.pop().unwrap();
            if frame.guide {
                let guide_x = ((frame.depth as f64 * 17.5) + 2.75).min(self.width - 44.0);
                self.panel.primitives.push(Primitive::Line {
                    from: Point {
                        x: guide_x,
                        y: frame.top,
                    },
                    to: Point {
                        x: guide_x,
                        y: frame.bottom,
                    },
                    width: 1.5,
                    color: self.palette.guide(self.hue),
                });
                frame.bottom += 2.0;
            }
            match stack.last_mut() {
                Some(parent) => parent.bottom = frame.bottom,
                None => return frame.bottom,
            }
        }
    }
}

pub fn layout(
    tree: &Tree,
    options: &LayoutOptions,
    fonts: &mut dyn FontMetrics,
) -> Result<Scene, LayoutError> {
    let started = Instant::now();
    if !options.width.is_finite() || options.width < 120.0 {
        return Err(LayoutError(
            "Width must be a finite value of at least 120px".into(),
        ));
    }
    if tree.nodes.is_empty() || tree.root >= tree.nodes.len() {
        return Err(LayoutError("Tree has no valid root".into()));
    }
    let palette = Palette::new(options.theme);
    let inner = options.width - SIDE * 2.0;
    let root = &tree.nodes[tree.root];
    let root_hue = tree
        .root_style()
        .and_then(|s| {
            s.stroke
                .as_deref()
                .and_then(Color::parse_css)
                .or_else(|| s.fill.as_deref().and_then(Color::parse_css))
        })
        .map(Color::hue);
    let hues = HUES
        .iter()
        .copied()
        .filter(|h| {
            root_hue.is_none_or(|r| {
                let d = (h - r).abs();
                d.min(360.0 - d) > 20.0
            })
        })
        .collect::<Vec<_>>();
    let root_color = root_hue.map_or(palette.text, |h| {
        Color::oklch(
            if palette.dark { 0.78 } else { 0.50 },
            if palette.dark { 0.10 } else { 0.13 },
            h,
        )
    });
    let root_collapsed = options.collapsed.contains(&root.id) && !root.children.is_empty();
    let mut title = row(
        &root.label,
        None,
        root_collapsed.then_some(root.children.len()),
        RowStyle {
            main: Style {
                size: 21.0,
                weight: 650,
                color: root_color,
            },
            muted: palette.muted,
            badge_color: root_color,
            line_height: 21.0 * 1.35,
            root: true,
        },
        inner,
        fonts,
    );
    translate(&mut title.primitives, SIDE, 22.0);
    let mut primitives = title.primitives;
    let mut hits = vec![HitRegion {
        node_id: root.id.clone(),
        label: root.label.clone(),
        prefix: root.prefix.clone(),
        rect: Rect {
            x: SIDE,
            y: 22.0,
            width: inner,
            height: title.height,
        },
        can_collapse: !root.children.is_empty(),
        collapsed: root_collapsed,
        source_range: root.source_range.clone(),
    }];
    let rule_width = if root_hue.is_some() { 2.0 } else { 1.0 };
    let rule_y = 22.0 + title.height + 14.0 + rule_width / 2.0;
    primitives.push(Primitive::Line {
        from: Point { x: SIDE, y: rule_y },
        to: Point {
            x: options.width - SIDE,
            y: rule_y,
        },
        width: rule_width,
        color: root_hue.map_or(palette.rule, |h| {
            Color::oklch(
                if palette.dark { 0.48 } else { 0.76 },
                if palette.dark { 0.08 } else { 0.09 },
                h,
            )
        }),
    });
    let start_y = rule_y + rule_width / 2.0 + 22.0;
    let branches = if root_collapsed {
        &[][..]
    } else {
        &root.children[..]
    };
    let title_minimum = branches
        .iter()
        .map(|&branch| {
            let node = &tree.nodes[branch];
            let title = node
                .label
                .replace("<br/>", " ")
                .replace("<br>", " ")
                .replace("<br />", " ");
            let prefix = node
                .prefix
                .as_ref()
                .map(|p| format!("{p}："))
                .unwrap_or_default();
            fonts.measure(&format!("{prefix}{title}"), 16.5, 600).width
        })
        .fold(160.0_f64, f64::max)
        .clamp(160.0, 300.0);
    // 短标题允许紧凑分栏；分区内的长行仍保留阅读宽度，避免大型深树因短标题突增栏数。
    let mut minimum = title_minimum;
    let mut pending: Vec<_> = branches.iter().map(|&node| (node, 0_usize)).collect();
    while let Some((index, depth)) = pending.pop() {
        let node = &tree.nodes[index];
        let width = fonts
            .measure(
                &node.label,
                if depth == 0 { 16.5 } else { 15.0 },
                if depth == 0 { 600 } else { 400 },
            )
            .width;
        minimum = minimum.max((width + depth as f64 * 17.5).clamp(160.0, 300.0));
        pending.extend(node.children.iter().map(|&child| (child, depth + 1)));
    }
    let columns = (((inner + COLUMN_GAP) / (minimum + COLUMN_GAP)).floor() as usize)
        .max(1)
        .min(branches.len().max(1));
    let column_width = (inner - COLUMN_GAP * (columns - 1) as f64) / columns as f64;
    let mut panels = Vec::new();
    for (i, &branch) in branches.iter().enumerate() {
        let mut builder = PanelLayout {
            tree,
            options,
            palette: &palette,
            fonts,
            width: column_width,
            hue: hues[i % hues.len()],
            panel: Panel {
                primitives: Vec::new(),
                hits: Vec::new(),
                height: 0.0,
            },
        };
        builder.panel.height = builder.node(branch, 0, 0.0);
        panels.push(builder.panel);
    }
    let heights = panels
        .iter()
        .map(|p| p.height + PANEL_GAP)
        .collect::<Vec<_>>();
    let groups = partition(&heights, columns);
    let mut bottom = start_y;
    for (column, group) in groups.into_iter().enumerate() {
        let x = SIDE + column as f64 * (column_width + COLUMN_GAP);
        let mut y = start_y;
        for index in group {
            let panel = &mut panels[index];
            translate(&mut panel.primitives, x, y);
            primitives.append(&mut panel.primitives);
            for hit in &mut panel.hits {
                hit.rect.x += x;
                hit.rect.y += y;
            }
            hits.append(&mut panel.hits);
            bottom = bottom.max(y + panel.height);
            y += panel.height + PANEL_GAP;
        }
    }
    let height = bottom + 26.0;
    primitives.insert(
        0,
        Primitive::RoundedRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: options.width,
                height,
            },
            radius: 16.0,
            color: palette.background,
        },
    );
    let min_font_px = primitives
        .iter()
        .filter_map(|p| {
            if let Primitive::Text(t) = p {
                Some(t.font_size)
            } else {
                None
            }
        })
        .fold(21.0, f64::min);
    let horizontal_overflow = primitives.iter().any(|p| match p {
        Primitive::Text(t) => {
            t.baseline.x < -0.001 || t.baseline.x + t.width > options.width + 0.001
        }
        Primitive::RoundedRect { rect, .. } => {
            rect.x < -0.001 || rect.x + rect.width > options.width + 0.001
        }
        Primitive::Line {
            from, to, width, ..
        } => {
            from.x.min(to.x) - width / 2.0 < -0.001
                || from.x.max(to.x) + width / 2.0 > options.width + 0.001
        }
    });
    // 命中区域按纵向起点扫描，只在仍可能相交的范围内比较，避免深链的 O(n²) 检查。
    let mut ordered = (0..hits.len()).collect::<Vec<_>>();
    ordered.sort_by(|&a, &b| hits[a].rect.y.total_cmp(&hits[b].rect.y));
    let mut active: Vec<usize> = Vec::new();
    let mut node_overlaps = 0;
    for index in ordered {
        let rect = hits[index].rect;
        active.retain(|&other| rect.y < hits[other].rect.y + hits[other].rect.height - 0.001);
        node_overlaps += active
            .iter()
            .filter(|&&other| {
                let other = hits[other].rect;
                rect.x < other.x + other.width - 0.001 && other.x < rect.x + rect.width - 0.001
            })
            .count();
        active.push(index);
    }
    let fonts_used = fonts.used_fonts().into_iter().filter(|f| primitives.iter().any(|p| matches!(p, Primitive::Text(t) if t.font_family == f.family && (t.font_weight == f.weight || f.family == "Deterministic")))).collect();
    Ok(Scene {
        width: options.width,
        height,
        primitives,
        hit_regions: hits,
        metrics: Metrics {
            columns,
            diagram_height: height,
            min_font_px,
            horizontal_overflow,
            node_overlaps,
            fonts: fonts_used,
            layout_ms: started.elapsed().as_secs_f64() * 1000.0,
            root_hue,
            branch_hues: (0..branches.len()).map(|i| hues[i % hues.len()]).collect(),
        },
    })
}
