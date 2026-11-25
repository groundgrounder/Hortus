// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 表格排版、本地图片解码及远程图片零联网占位组件。

use crate::core::Utf16Mapper;
use crate::render::math::{MathRenderService, PreparedMathBlock, build_inline_math_widget};
use crate::render::svg::{checked_within_vault, sanitize_svg_bytes};
use gtk4::gdk;
use gtk4::gdk_pixbuf;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::MathBlock;
use libadwaita as adw;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct LocalMediaRun {
    pub start: u64,
    pub length: u64,
    pub mask: u32,
}

#[derive(Clone, Debug)]
pub struct LocalTableCell {
    pub source: String,
    pub text: String,
    pub runs: Vec<LocalMediaRun>,
    pub math: Vec<MathBlock>,
}

#[derive(Clone, Debug)]
pub struct LocalMediaBlock {
    pub block_id: u64,
    pub source_start: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub columns: Vec<f64>,
    pub alignments: Vec<u8>,
    pub cells: Vec<LocalTableCell>,
    pub rows: u64,
    pub image_target: String,
    pub image_alt: String,
    pub image_path: String,
    pub image_reason: String,
    pub image_wiki: bool,
    pub image_width: Option<u32>,
}

/// 将 `hortus_ffi` 返回的未公开导出类型 `MediaBlock` 集合适配为本 crate 的 owned 结构体。
#[macro_export]
macro_rules! adapt_media_blocks {
    ($blocks:expr) => {
        $blocks
            .iter()
            .map(|b| $crate::render::media::LocalMediaBlock {
                block_id: b.block_id,
                source_start: b.source_start,
                display_start: b.display_start,
                display_length: b.display_length,
                columns: b.columns.clone(),
                alignments: b.alignments.clone(),
                cells: b
                    .cells
                    .iter()
                    .map(|c| $crate::render::media::LocalTableCell {
                        source: c.source.clone(),
                        text: c.text.clone(),
                        runs: c
                            .runs
                            .iter()
                            .map(|r| $crate::render::media::LocalMediaRun {
                                start: r.start,
                                length: r.length,
                                mask: r.mask,
                            })
                            .collect(),
                        math: c.math.clone(),
                    })
                    .collect(),
                rows: b.rows,
                image_target: b.image_target.clone(),
                image_alt: b.image_alt.clone(),
                image_path: b.image_path.clone(),
                image_reason: b.image_reason.clone(),
                image_wiki: b.image_wiki,
                image_width: b.image_width,
            })
            .collect::<Vec<$crate::render::media::LocalMediaBlock>>()
    };
}

#[derive(Clone)]
pub struct PreparedMedia {
    pub block: LocalMediaBlock,
    pub texture: Option<gdk::Texture>,
    pub image_width: f64,
    pub reason: String,
    pub cell_math: Vec<Vec<PreparedMathBlock>>,
    #[allow(dead_code)]
    pub sanitized_svg_external_refs: usize,
    #[allow(dead_code)]
    pub sanitized_svg_scripts: usize,
}

/// 在后台线程解码本地图片字节（raster 直接通过 GdkTexture/Pixbuf，SVG 先经安全清洗再用 librsvg 解码）。
fn decode_local_image(path: &Path) -> Result<(gdk::Texture, usize, usize), String> {
    let raw = std::fs::read(path).map_err(|_| "image.missing".to_string())?;
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));

    if is_svg {
        let sanitized =
            sanitize_svg_bytes(&raw).map_err(|_| "image.svg_unsupported".to_string())?;
        let loader = gdk_pixbuf::PixbufLoader::with_mime_type("image/svg+xml")
            .unwrap_or_else(|_| gdk_pixbuf::PixbufLoader::new());
        loader
            .write(&sanitized.bytes)
            .map_err(|_| "image.svg_unsupported".to_string())?;
        loader
            .close()
            .map_err(|_| "image.svg_unsupported".to_string())?;
        let pixbuf = loader
            .pixbuf()
            .ok_or_else(|| "image.svg_unsupported".to_string())?;
        #[allow(deprecated)]
        let texture = gdk::Texture::for_pixbuf(&pixbuf);
        return Ok((
            texture,
            sanitized.stripped_external_refs,
            sanitized.stripped_scripts,
        ));
    }

    if let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(raw.clone())) {
        return Ok((tex, 0, 0));
    }

    let loader = gdk_pixbuf::PixbufLoader::new();
    loader
        .write(&raw)
        .map_err(|_| "image.decode_failed".to_string())?;
    loader
        .close()
        .map_err(|_| "image.decode_failed".to_string())?;
    let pixbuf = loader
        .pixbuf()
        .ok_or_else(|| "image.decode_failed".to_string())?;
    #[allow(deprecated)]
    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    Ok((texture, 0, 0))
}

/// 必须在后台工作线程执行：准备表格单元格内公式、安全校验并解码本地图片。
pub fn prepare_media_blocks_bg(
    blocks: &[LocalMediaBlock],
    vault_root: &str,
    body_size: f64,
    math_service: &MathRenderService,
) -> Vec<PreparedMedia> {
    blocks
        .iter()
        .map(|block| {
            if !block.columns.is_empty() {
                let cell_math = block
                    .cells
                    .iter()
                    .map(|cell| math_service.prepare_blocks(&cell.math, body_size, true))
                    .collect();
                return PreparedMedia {
                    block: block.clone(),
                    texture: None,
                    image_width: 0.0,
                    reason: String::new(),
                    cell_math,
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                };
            }

            if !block.image_reason.is_empty() {
                return PreparedMedia {
                    block: block.clone(),
                    texture: None,
                    image_width: 0.0,
                    reason: block.image_reason.clone(),
                    cell_math: Vec::new(),
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                };
            }

            let Some(safe_path) = checked_within_vault(&block.image_path, vault_root) else {
                let reason = if block.image_path.is_empty() {
                    "image.missing".to_string()
                } else {
                    "image.outside".to_string()
                };
                return PreparedMedia {
                    block: block.clone(),
                    texture: None,
                    image_width: 0.0,
                    reason,
                    cell_math: Vec::new(),
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                };
            };

            match decode_local_image(&safe_path) {
                Ok((texture, ext_refs, scripts)) => {
                    let w = f64::from(texture.width()).max(1.0);
                    PreparedMedia {
                        block: block.clone(),
                        texture: Some(texture),
                        image_width: w,
                        reason: String::new(),
                        cell_math: Vec::new(),
                        sanitized_svg_external_refs: ext_refs,
                        sanitized_svg_scripts: scripts,
                    }
                }
                Err(reason) => PreparedMedia {
                    block: block.clone(),
                    texture: None,
                    image_width: 0.0,
                    reason,
                    cell_math: Vec::new(),
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                },
            }
        })
        .collect()
}

/// 原生块级图片控件句柄：外层容器与实际等比图片控件分离，支持零锁自适应与精确测宽。
#[derive(Clone)]
pub struct ImageBlock {
    root: gtk4::Widget,
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    measured: gtk4::Widget,
}

impl ImageBlock {
    pub fn widget(&self) -> &gtk4::Widget {
        &self.root
    }

    #[cfg(feature = "automation")]
    pub fn measured_widget(&self) -> &gtk4::Widget {
        &self.measured
    }
}

/// 构造图片（或远程禁止联网占位 / 错误降级卡片）GTK4 控件。
pub fn build_image_widget(
    media: &PreparedMedia,
    _max_width: f64,
    parent_window: Option<&gtk4::Window>,
) -> ImageBlock {
    let block = &media.block;
    if let Some(ref texture) = media.texture {
        let target_nat_w = block
            .image_width
            .map(f64::from)
            .unwrap_or(media.image_width)
            .round()
            .max(16.0) as i32;

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        container.add_css_class("hortus-image-box");
        container.set_hexpand(true);
        container.set_halign(gtk4::Align::Fill);

        let picture = gtk4::Picture::for_paintable(texture);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::Contain);
        picture.set_hexpand(true);
        picture.set_halign(gtk4::Align::Fill);

        if !block.image_path.is_empty() {
            let path = PathBuf::from(&block.image_path);
            let win_weak = parent_window.map(glib::object::ObjectExt::downgrade);
            let click = gtk4::GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_, n_press, _, _| {
                if n_press >= 2 {
                    let file = gio::File::for_path(&path);
                    let launcher = gtk4::FileLauncher::new(Some(&file));
                    let win = win_weak.as_ref().and_then(glib::WeakRef::upgrade);
                    launcher.launch(win.as_ref(), gio::Cancellable::NONE, |_| {});
                }
            });
            picture.add_controller(click);
        }

        if !block.image_alt.is_empty() {
            picture.set_tooltip_text(Some(&block.image_alt));
        }

        let clamp = adw::Clamp::builder()
            .maximum_size(target_nat_w)
            .tightening_threshold(target_nat_w)
            .hexpand(true)
            .halign(gtk4::Align::Fill)
            .child(&picture)
            .build();
        container.append(&clamp);
        return ImageBlock {
            root: container.upcast(),
            measured: picture.upcast(),
        };
    }

    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    card.add_css_class("hortus-media-placeholder");
    card.set_halign(gtk4::Align::Fill);
    card.set_hexpand(true);
    card.set_size_request(120, -1);

    let title_text = if block.image_alt.is_empty() {
        block.image_target.clone()
    } else {
        block.image_alt.clone()
    };
    let title = gtk4::Label::new(Some(&title_text));
    title.set_halign(gtk4::Align::Start);
    title.set_ellipsize(pango::EllipsizeMode::End);
    card.append(&title);

    if media.reason == "image.remote" {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let hint = gtk4::Label::new(Some(crate::i18n::tr(
            crate::i18n::Key::MediaRemoteNotLoaded,
        )));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        let open_btn = gtk4::Button::with_label(&block.image_target);
        open_btn.add_css_class("link");
        open_btn.add_css_class("flat");
        let uri = block.image_target.clone();
        let win_weak = parent_window.map(glib::object::ObjectExt::downgrade);
        open_btn.connect_clicked(move |_| {
            if uri.starts_with("http://") || uri.starts_with("https://") {
                let launcher = gtk4::UriLauncher::new(&uri);
                let win = win_weak.as_ref().and_then(glib::WeakRef::upgrade);
                launcher.launch(win.as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });
        row.append(&hint);
        card.append(&row);
        card.append(&open_btn);
    } else {
        let reason_text = match media.reason.as_str() {
            "image.missing" => crate::i18n::tr(crate::i18n::Key::MediaMissing),
            "image.outside" => crate::i18n::tr(crate::i18n::Key::MediaOutside),
            "image.svg_unsupported" => crate::i18n::tr(crate::i18n::Key::MediaSvgUnsupported),
            "image.decode_failed" => crate::i18n::tr(crate::i18n::Key::MediaDecodeFailed),
            other => other,
        };
        let label = gtk4::Label::new(Some(reason_text));
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.set_halign(gtk4::Align::Start);
        card.append(&label);
    }

    let root: gtk4::Widget = card.upcast();
    ImageBlock {
        measured: root.clone(),
        root,
    }
}

pub type LinkOpenCallback = Rc<dyn Fn(String, Option<String>)>;
pub type LinkOpenUriCallback = Rc<dyn Fn(String)>;

/// 原生块级表格控件句柄：外层非传播自然宽的横向滚动容器包装 `GtkGrid`。
#[derive(Clone)]
pub struct TableBlock {
    root: gtk4::Widget,
}

impl TableBlock {
    pub fn widget(&self) -> &gtk4::Widget {
        &self.root
    }
}

/// 判断表格单元格是否包含双链或 Markdown 链接。
fn cell_has_links(cell: &LocalTableCell) -> bool {
    !cell.source.is_empty() && (cell.source.contains("[[") || cell.source.contains("]("))
}

/// 纯文本单元格判断：无行内样式、无行内公式、无双链/Markdown 链接时可直接走轻量 `gtk4::Label`。
pub(crate) fn is_plain_table_cell(cell: &LocalTableCell, cell_math: &[PreparedMathBlock]) -> bool {
    cell.runs.is_empty() && cell_math.is_empty() && !cell_has_links(cell)
}

/// 使用内核计算列宽、对齐方式、行内样式及单元格内公式构造原生 `GtkGrid` 表格。
/// 纯文本单元格使用轻量 `gtk4::Label`，富文本/含链接/公式单元格共享一张表级 `TextTagTable`。
pub fn build_table_widget(
    media: &PreparedMedia,
    _max_width: f64,
    on_open_link: Option<LinkOpenCallback>,
    on_open_uri: Option<LinkOpenUriCallback>,
) -> TableBlock {
    let block = &media.block;
    let cols = block.columns.len().max(1);
    let rows = usize::try_from(block.rows).unwrap_or(0);

    let grid = gtk4::Grid::new();
    grid.add_css_class("hortus-table-grid");
    grid.set_column_spacing(0);
    grid.set_row_spacing(0);
    grid.set_hexpand(true);
    grid.set_halign(gtk4::Align::Fill);

    let mut shared_table_cache: Option<(gtk4::TextTagTable, crate::platform::style::MaskTagSet)> =
        None;

    for r in 0..rows {
        let is_header = r == 0;
        for c in 0..cols {
            let cell_idx = r * cols + c;
            let Some(cell) = block.cells.get(cell_idx) else {
                continue;
            };
            let col_w = block.columns.get(c).copied().unwrap_or(100.0).max(40.0);
            let align = block.alignments.get(c).copied().unwrap_or(0);
            let cell_math_list = media
                .cell_math
                .get(cell_idx)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let is_plain = is_plain_table_cell(cell, cell_math_list);

            let frame = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            frame.add_css_class(if is_header {
                "hortus-table-header-cell"
            } else {
                "hortus-table-body-cell"
            });
            frame.set_hexpand(true);

            if is_plain {
                let lbl = gtk4::Label::new(Some(&cell.text));
                lbl.set_wrap(true);
                lbl.set_wrap_mode(pango::WrapMode::WordChar);
                lbl.set_selectable(true);
                lbl.set_hexpand(true);
                lbl.set_xalign(match align {
                    1 => 0.5,
                    2 => 1.0,
                    _ => 0.0,
                });
                lbl.set_justify(match align {
                    1 => gtk4::Justification::Center,
                    2 => gtk4::Justification::Right,
                    _ => gtk4::Justification::Left,
                });
                lbl.set_margin_start(6);
                lbl.set_margin_end(6);
                lbl.set_margin_top(7);
                lbl.set_margin_bottom(7);
                lbl.set_size_request(col_w.round() as i32, -1);
                lbl.add_css_class("hortus-table-cell-text");
                if is_header {
                    lbl.add_css_class("hortus-table-header");
                }
                frame.append(&lbl);
                grid.attach(&frame, c as i32, r as i32, 1, 1);
                continue;
            }

            let (table, tag_set) = shared_table_cache.get_or_insert_with(|| {
                let tbl = crate::platform::style::create_tag_table();
                let set = crate::platform::style::MaskTagSet::new(&tbl);
                (tbl, set)
            });
            let buf = gtk4::TextBuffer::new(Some(table));
            buf.set_text(&cell.text);
            let cell_mapper = Utf16Mapper::new(&cell.text);

            if is_header {
                let s = buf.start_iter();
                let e = buf.end_iter();
                buf.apply_tag_by_name("strong", &s, &e);
            }
            for run in &cell.runs {
                if run.length == 0 || run.mask == 0 {
                    continue;
                }
                let cs = cell_mapper.utf16_to_char(run.start as usize);
                let ce = cell_mapper.utf16_to_char((run.start + run.length) as usize);
                if cs < ce {
                    crate::platform::style::apply_mask_tags_cached(
                        &buf,
                        tag_set,
                        &buf.iter_at_offset(cs as i32),
                        &buf.iter_at_offset(ce as i32),
                        run.mask,
                    );
                }
            }

            let tv = gtk4::TextView::builder()
                .buffer(&buf)
                .editable(false)
                .cursor_visible(false)
                .wrap_mode(gtk4::WrapMode::WordChar)
                .justification(match align {
                    1 => gtk4::Justification::Center,
                    2 => gtk4::Justification::Right,
                    _ => gtk4::Justification::Left,
                })
                .left_margin(6)
                .right_margin(6)
                .top_margin(7)
                .bottom_margin(7)
                .build();
            tv.add_css_class("hortus-table-cell-text");
            if is_header {
                tv.add_css_class("hortus-table-header");
            }
            tv.set_size_request(col_w.round() as i32, -1);

            for math_item in cell_math_list {
                let cs = cell_mapper.utf16_to_char(math_item.block.display_start as usize);
                let ce = cell_mapper.utf16_to_char(
                    (math_item.block.display_start + math_item.block.display_length) as usize,
                );
                let b0 = cell_mapper.char_to_byte(cs);
                if cell
                    .text
                    .get(b0..)
                    .is_some_and(|s| s.starts_with('\u{fffc}'))
                    && ce > cs
                {
                    let mut s_it = buf.iter_at_offset(cs as i32);
                    let mut e_it = buf.iter_at_offset(ce as i32);
                    buf.delete(&mut s_it, &mut e_it);
                    let anchor = buf.create_child_anchor(&mut s_it);
                    let w: gtk4::Widget = if math_item.math.is_some() {
                        build_inline_math_widget(math_item)
                    } else {
                        let badge = gtk4::Label::new(Some(math_item.block.display_source.trim()));
                        badge.add_css_class("hortus-math-inline-fallback");
                        badge.add_css_class("monospace");
                        badge.set_valign(gtk4::Align::Baseline);
                        badge.set_ellipsize(pango::EllipsizeMode::End);
                        badge.set_max_width_chars(14);
                        badge.set_tooltip_text(Some(
                            math_item
                                .failure
                                .as_deref()
                                .unwrap_or(crate::i18n::tr(crate::i18n::Key::MathSyntaxInvalid)),
                        ));
                        badge.upcast()
                    };
                    tv.add_child_at_anchor(&w, &anchor);
                }
            }

            if cell_has_links(cell) {
                attach_simple_cell_links(
                    &tv,
                    &cell.text,
                    &cell_mapper,
                    &cell.runs,
                    &cell.source,
                    on_open_link.clone(),
                    on_open_uri.clone(),
                );
            }

            tv.set_hexpand(true);
            frame.append(&tv);
            grid.attach(&frame, c as i32, r as i32, 1, 1);
        }
    }

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .min_content_width(160)
        .hexpand(true)
        .halign(gtk4::Align::Fill)
        .child(&grid)
        .build();
    scroll.add_css_class("hortus-table-wrap");
    TableBlock {
        root: scroll.upcast(),
    }
}

#[derive(Clone)]
enum CellLinkAction {
    Note(String),
    Uri(String),
}

fn attach_simple_cell_links(
    tv: &gtk4::TextView,
    text: &str,
    mapper: &Utf16Mapper,
    runs: &[LocalMediaRun],
    source: &str,
    on_open_link: Option<LinkOpenCallback>,
    on_open_uri: Option<LinkOpenUriCallback>,
) {
    let mut link_ranges: Vec<(i32, i32, CellLinkAction)> = Vec::new();
    for run in runs {
        if run.mask & (1 << 18) == 0 {
            continue;
        }
        let cs = mapper.utf16_to_char(run.start as usize);
        let ce = mapper.utf16_to_char((run.start + run.length) as usize);
        let b0 = mapper.char_to_byte(cs);
        let b1 = mapper.char_to_byte(ce);
        if let Some(target) = text.get(b0..b1) {
            link_ranges.push((
                cs as i32,
                ce as i32,
                CellLinkAction::Note(target.to_string()),
            ));
        }
    }
    let mut search_from = 0usize;
    for (label, url) in markdown_cell_links(source) {
        if let Some(pos) = text.get(search_from..).and_then(|tail| tail.find(&label)) {
            let byte_start = search_from + pos;
            let char_start = mapper.byte_to_char(byte_start) as i32;
            let char_end = char_start + label.chars().count() as i32;
            search_from = byte_start + label.len();
            if char_end > char_start
                && !link_ranges
                    .iter()
                    .any(|(s, e, _)| char_start >= *s && char_start < *e)
            {
                link_ranges.push((char_start, char_end, CellLinkAction::Uri(url)));
            }
        }
    }
    if link_ranges.is_empty() {
        return;
    }
    let map = Rc::new(link_ranges);
    let click = gtk4::GestureClick::new();
    click.set_button(1);
    let tv_weak = tv.downgrade();
    click.connect_pressed(move |_, _, x, y| {
        let Some(tv) = tv_weak.upgrade() else { return };
        let (bx, by) = tv.window_to_buffer_coords(gtk4::TextWindowType::Widget, x as i32, y as i32);
        let Some(iter) = tv.iter_at_location(bx, by) else {
            return;
        };
        let off = iter.offset();
        let Some((_, _, action)) = map.iter().find(|(s, e, _)| off >= *s && off < *e) else {
            return;
        };
        match action {
            CellLinkAction::Note(target) => {
                if let Some(ref cb) = on_open_link {
                    cb(target.clone(), None);
                }
            }
            CellLinkAction::Uri(url) => {
                if let Some(ref cb) = on_open_uri {
                    cb(url.clone());
                } else if url.starts_with("http://") || url.starts_with("https://") {
                    let launcher = gtk4::UriLauncher::new(url);
                    launcher.launch(None::<&gtk4::Window>, gio::Cancellable::NONE, |_| {});
                }
            }
        }
    });
    tv.add_controller(click);
}

/// 从单元格 Markdown 源中提取 `[显示文字](url)` 形式的外链。
pub fn markdown_cell_links(source: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(close) = source[index + 1..].find(']') else {
            break;
        };
        let label_end = index + 1 + close;
        if !source[label_end + 1..].starts_with('(') {
            index = label_end + 1;
            continue;
        }
        let url_start = label_end + 2;
        let Some(url_end_rel) = source[url_start..].find(')') else {
            break;
        };
        let url = &source[url_start..url_start + url_end_rel];
        let label = &source[index + 1..label_end];
        if !label.is_empty()
            && !label.contains('[')
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            links.push((label.to_string(), url.to_string()));
        }
        index = url_start + url_end_rel + 1;
    }
    links
}

#[cfg(feature = "automation")]
pub fn summarize_media_stats(items: &[PreparedMedia]) -> (usize, usize, usize, usize, usize) {
    let mut tables = 0;
    let mut local_images = 0;
    let mut remote_images = 0;
    let mut sanitized_ext_refs = 0;
    let mut sanitized_scripts = 0;
    for item in items {
        sanitized_ext_refs += item.sanitized_svg_external_refs;
        sanitized_scripts += item.sanitized_svg_scripts;
        if !item.block.columns.is_empty() {
            tables += 1;
        } else if item.reason == "image.remote" {
            remote_images += 1;
        } else if item.texture.is_some() {
            local_images += 1;
        }
    }
    (
        tables,
        local_images,
        remote_images,
        sanitized_ext_refs,
        sanitized_scripts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_cell_links_extracts_external_targets() {
        let links =
            markdown_cell_links("见 [规范指南](https://example.org) 与 [文档](http://a.example/x)");
        assert_eq!(
            links,
            vec![
                ("规范指南".to_string(), "https://example.org".to_string()),
                ("文档".to_string(), "http://a.example/x".to_string()),
            ]
        );
        assert!(markdown_cell_links("[[内部双链]] 不是外链").is_empty());
        assert!(markdown_cell_links("[相对](Assets/a.png)").is_empty());
    }

    #[test]
    fn table_cell_classification_routes_100_cells_correctly() {
        let mut plain_count = 0usize;
        let mut rich_count = 0usize;
        for idx in 0..100 {
            let cell = if idx % 10 == 0 {
                LocalTableCell {
                    source: "**粗体** [[页面]]".into(),
                    text: "粗体 页面".into(),
                    runs: vec![LocalMediaRun {
                        start: 0,
                        length: 2,
                        mask: 1,
                    }],
                    math: Vec::new(),
                }
            } else {
                LocalTableCell {
                    source: format!("cell-{idx}"),
                    text: format!("cell-{idx}"),
                    runs: Vec::new(),
                    math: Vec::new(),
                }
            };
            if is_plain_table_cell(&cell, &[]) {
                plain_count += 1;
            } else {
                rich_count += 1;
            }
        }
        assert_eq!(plain_count, 90);
        assert_eq!(rich_count, 10);
    }
}
