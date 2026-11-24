// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 表格排版、本地图片解码及远程图片零联网占位组件。

use crate::core::{utf16_range_to_char_range, utf16_slice};
use crate::render::math::{MathRenderService, PreparedMathBlock, build_inline_math_widget};
use crate::render::svg::{checked_within_vault, sanitize_svg_bytes};
use gtk4::gdk;
use gtk4::gdk_pixbuf;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::MathBlock;
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
    pub image_height: f64,
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
                    image_height: 0.0,
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
                    image_height: 0.0,
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
                    image_height: 0.0,
                    reason,
                    cell_math: Vec::new(),
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                };
            };

            match decode_local_image(&safe_path) {
                Ok((texture, ext_refs, scripts)) => {
                    let w = f64::from(texture.width()).max(1.0);
                    let h = f64::from(texture.height()).max(1.0);
                    PreparedMedia {
                        block: block.clone(),
                        texture: Some(texture),
                        image_width: w,
                        image_height: h,
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
                    image_height: 0.0,
                    reason,
                    cell_math: Vec::new(),
                    sanitized_svg_external_refs: 0,
                    sanitized_svg_scripts: 0,
                },
            }
        })
        .collect()
}

/// 构造图片（或远程禁止联网占位 / 错误降级卡片）GTK4 控件。
pub fn build_image_widget(
    media: &PreparedMedia,
    max_width: f64,
    parent_window: Option<&gtk4::Window>,
) -> gtk4::Widget {
    let block = &media.block;
    if let Some(ref texture) = media.texture {
        let target_w = block
            .image_width
            .map(f64::from)
            .unwrap_or(media.image_width)
            .min(max_width.max(120.0))
            .max(16.0);
        let aspect = if media.image_width > 0.0 {
            media.image_height / media.image_width
        } else {
            1.0
        };
        let target_h = (target_w * aspect).ceil().max(16.0);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        container.add_css_class("hortus-image-box");

        let picture = gtk4::Picture::for_paintable(texture);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::Contain);
        picture.set_size_request(target_w.round() as i32, target_h.round() as i32);
        picture.set_halign(gtk4::Align::Start);
        let picture_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .child(&picture)
            .build();
        picture_scroll.add_css_class("hortus-image-scroll");
        picture_scroll.set_size_request(target_w.round() as i32, -1);
        let picture = picture_scroll;

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
        container.append(&picture);
        return container.upcast();
    }

    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    card.add_css_class("hortus-media-placeholder");
    card.set_halign(gtk4::Align::Fill);
    card.set_hexpand(true);
    card.set_size_request((max_width - 16.0).max(240.0).round() as i32, -1);

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
        let hint = gtk4::Label::new(Some("出于隐私与安全策略，阅读视图不自动请求远程图片"));
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
            "image.missing" => "本地图片文件不存在",
            "image.outside" => "图片路径位于当前知识库外部，已阻止访问",
            "image.svg_unsupported" => "SVG 文件格式不支持或无法解析",
            "image.decode_failed" => "图片解码失败",
            other => other,
        };
        let label = gtk4::Label::new(Some(reason_text));
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.set_halign(gtk4::Align::Start);
        card.append(&label);
    }

    card.upcast()
}

pub type LinkOpenCallback = Rc<dyn Fn(String, Option<String>)>;
pub type LinkOpenUriCallback = Rc<dyn Fn(String)>;

/// 使用内核计算列宽、对齐方式、行内样式及单元格内公式构造原生 `GtkGrid` 表格。
pub fn build_table_widget(
    media: &PreparedMedia,
    max_width: f64,
    on_open_link: Option<LinkOpenCallback>,
    on_open_uri: Option<LinkOpenUriCallback>,
) -> gtk4::Widget {
    let block = &media.block;
    let cols = block.columns.len().max(1);
    let rows = usize::try_from(block.rows).unwrap_or(0);

    let grid = gtk4::Grid::new();
    grid.add_css_class("hortus-table-grid");
    grid.set_column_spacing(0);
    grid.set_row_spacing(0);
    grid.set_hexpand(true);
    grid.set_halign(gtk4::Align::Fill);

    for r in 0..rows {
        let is_header = r == 0;
        for c in 0..cols {
            let cell_idx = r * cols + c;
            let Some(cell) = block.cells.get(cell_idx) else {
                continue;
            };
            let col_w = block.columns.get(c).copied().unwrap_or(100.0).max(40.0);
            let align = block.alignments.get(c).copied().unwrap_or(0);

            let table = crate::platform::style::create_tag_table();
            let buf = gtk4::TextBuffer::new(Some(&table));
            buf.set_text(&cell.text);

            if is_header {
                let s = buf.start_iter();
                let e = buf.end_iter();
                buf.apply_tag_by_name("strong", &s, &e);
            }
            for run in &cell.runs {
                if run.length == 0 || run.mask == 0 {
                    continue;
                }
                let (cs, ce) =
                    utf16_range_to_char_range(&cell.text, run.start as usize, run.length as usize);
                if cs < ce {
                    crate::platform::style::apply_mask_tags(
                        &buf,
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

            if let Some(cell_math_list) = media.cell_math.get(cell_idx) {
                for math_item in cell_math_list {
                    let (cs, ce) = utf16_range_to_char_range(
                        &cell.text,
                        math_item.block.display_start as usize,
                        math_item.block.display_length as usize,
                    );
                    if let Some(ch) = cell.text.chars().nth(cs)
                        && ch == '\u{fffc}'
                        && ce > cs
                    {
                        let mut s_it = buf.iter_at_offset(cs as i32);
                        let mut e_it = buf.iter_at_offset(ce as i32);
                        buf.delete(&mut s_it, &mut e_it);
                        let anchor = buf.create_child_anchor(&mut s_it);
                        let w: gtk4::Widget = if math_item.math.is_some() {
                            build_inline_math_widget(math_item)
                        } else {
                            let badge =
                                gtk4::Label::new(Some(math_item.block.display_source.trim()));
                            badge.add_css_class("hortus-math-inline-fallback");
                            badge.add_css_class("monospace");
                            badge.set_valign(gtk4::Align::Baseline);
                            badge.set_ellipsize(pango::EllipsizeMode::End);
                            badge.set_max_width_chars(14);
                            badge.set_tooltip_text(Some(
                                math_item.failure.as_deref().unwrap_or("公式语法错误"),
                            ));
                            badge.upcast()
                        };
                        tv.add_child_at_anchor(&w, &anchor);
                    }
                }
            }

            if !cell.source.is_empty() && (cell.source.contains("[[") || cell.source.contains("]("))
            {
                attach_simple_cell_links(
                    &tv,
                    &cell.text,
                    &cell.runs,
                    &cell.source,
                    on_open_link.clone(),
                    on_open_uri.clone(),
                );
            }

            let frame = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            frame.add_css_class(if is_header {
                "hortus-table-header-cell"
            } else {
                "hortus-table-body-cell"
            });
            frame.set_hexpand(true);
            tv.set_hexpand(true);
            frame.append(&tv);
            grid.attach(&frame, c as i32, r as i32, 1, 1);
        }
    }

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .hexpand(true)
        .child(&grid)
        .build();
    scroll.add_css_class("hortus-table-wrap");
    scroll.set_size_request(max_width.round().max(240.0) as i32, -1);
    scroll.upcast()
}

#[derive(Clone)]
enum CellLinkAction {
    Note(String),
    Uri(String),
}

fn attach_simple_cell_links(
    tv: &gtk4::TextView,
    text: &str,
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
        let (cs, ce) = utf16_range_to_char_range(text, run.start as usize, run.length as usize);
        if let Some(target) = utf16_slice(text, run.start as usize, run.length as usize) {
            link_ranges.push((cs as i32, ce as i32, CellLinkAction::Note(target)));
        }
    }
    let mut search_from = 0usize;
    for (label, url) in markdown_cell_links(source) {
        if let Some(pos) = text.get(search_from..).and_then(|tail| tail.find(&label)) {
            let byte_start = search_from + pos;
            let char_start = text[..byte_start].chars().count() as i32;
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
}
