// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 嵌入笔记、标题切片与 `.mmd` 图表解析（全部在后台线程完成，卡片内部共享阅读渲染链）。

use crate::diagram_render::{
    DiagramCallbacks, DiagramExportKind, DiagramExportMeta, RenderedDiagram, build_diagram_card,
    format_diagram_failure, render_block_with_engine,
};
use crate::math_render::{MathRenderService, PreparedMathBlock};
use crate::media_render::{PreparedMedia, prepare_media_blocks_bg};
use crate::reading::{ReadingEnv, build_embedded_note_body};
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use hortus_ffi::{DiagramEngine, DiagramIdentity, EmbedContentInfo, VaultHandle, WorkspaceCore};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct EmbedRequest {
    pub source_path: String,
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub ancestors: Vec<String>,
    pub depth: u32,
    pub width: f64,
    pub font_size: f64,
    pub dark: bool,
    pub vault_root: String,
    pub handle: Arc<VaultHandle>,
    pub core: Arc<WorkspaceCore>,
    pub diagram_engine: Arc<DiagramEngine>,
    pub math_service: Arc<MathRenderService>,
}

pub struct PreparedEmbed {
    pub info: EmbedContentInfo,
    pub media: Vec<PreparedMedia>,
    pub math: Vec<PreparedMathBlock>,
    pub diagrams: Vec<RenderedDiagram>,
}

impl EmbedRequest {
    pub fn content_width(card_width: f64) -> f64 {
        (card_width - 36.0).max(140.0)
    }
}

/// 必须在后台工作线程调用：解析嵌入内容、递归准备嵌套媒体/公式/Mermaid 图表。
pub fn prepare_embed_bg(req: &EmbedRequest) -> Result<PreparedEmbed, String> {
    let body_width = EmbedRequest::content_width(req.width);
    let info = req
        .handle
        .resolve_embed(
            req.source_path.clone(),
            req.target.clone(),
            req.heading.clone(),
            req.alias.clone(),
            req.ancestors.clone(),
            3,
            body_width,
            req.font_size,
        )
        .map_err(|e| format!("嵌入解析失败：{e}"))?;

    let mut media = Vec::new();
    let mut math = Vec::new();
    let mut diagrams = Vec::new();

    if info.kind == "note" {
        let local_media = crate::adapt_media_blocks!(&info.media);
        media = prepare_media_blocks_bg(
            &local_media,
            &req.vault_root,
            req.font_size,
            &req.math_service,
        );
        if !info.math.is_empty() {
            math = req
                .math_service
                .prepare_blocks(&info.math, &info.content, req.font_size, false);
        }
        let doc_id = format!("embed:{}", info.resolved_path.as_deref().unwrap_or(""));
        let note_rel = info.resolved_path.as_deref().unwrap_or("");
        let mut ordinal = 0u32;
        for blk in &info.blocks {
            if blk.diagram {
                ordinal += 1;
                let collapsed = if !note_rel.is_empty() {
                    req.core
                        .collapsed_nodes(req.vault_root.clone(), note_rel.to_string(), ordinal)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                match render_block_with_engine(
                    &req.diagram_engine,
                    &info.content,
                    info.is_mmd,
                    body_width,
                    &collapsed,
                    DiagramIdentity {
                        document: doc_id.clone(),
                        block: ordinal,
                        revision: 1,
                        dark: req.dark,
                        font_generation: req.font_size.to_bits(),
                        request_generation: 1,
                    },
                    None,
                ) {
                    Ok(scene) => diagrams.push(RenderedDiagram {
                        ordinal,
                        source_start: blk.source_start_byte,
                        source_end: blk.source_start_byte + blk.source_length_byte,
                        previous: scene.previous.clone(),
                        scene: Some(scene),
                        failure: None,
                        collapsed,
                    }),
                    Err(err) => diagrams.push(RenderedDiagram {
                        ordinal,
                        source_start: blk.source_start_byte,
                        source_end: blk.source_start_byte + blk.source_length_byte,
                        previous: None,
                        scene: None,
                        failure: Some(format_diagram_failure(&err)),
                        collapsed,
                    }),
                }
            }
        }
    } else if info.kind == "diagram" {
        let collapsed = info
            .resolved_path
            .as_deref()
            .and_then(|p| {
                req.core
                    .collapsed_nodes(req.vault_root.clone(), p.to_string(), 1)
                    .ok()
            })
            .unwrap_or_default();
        let doc_id = format!("embed:{}", info.resolved_path.as_deref().unwrap_or(""));
        match render_block_with_engine(
            &req.diagram_engine,
            &info.content,
            true,
            body_width,
            &collapsed,
            DiagramIdentity {
                document: doc_id,
                block: 1,
                revision: 1,
                dark: req.dark,
                font_generation: req.font_size.to_bits(),
                request_generation: 1,
            },
            None,
        ) {
            Ok(scene) => diagrams.push(RenderedDiagram {
                ordinal: 1,
                source_start: 0,
                source_end: info.content.len() as u64,
                previous: scene.previous.clone(),
                scene: Some(scene),
                failure: None,
                collapsed,
            }),
            Err(err) => diagrams.push(RenderedDiagram {
                ordinal: 1,
                source_start: 0,
                source_end: info.content.len() as u64,
                previous: None,
                scene: None,
                failure: Some(format_diagram_failure(&err)),
                collapsed,
            }),
        }
    }

    Ok(PreparedEmbed {
        info,
        media,
        math,
        diagrams,
    })
}

#[derive(Clone)]
pub struct EmbedCallbacks {
    pub on_open_note: Rc<dyn Fn(String, Option<String>)>,
    pub on_open_external: Rc<dyn Fn(String)>,
    pub on_open_tag: Rc<dyn Fn(String)>,
    pub on_export_diagram: Rc<dyn Fn(DiagramExportKind, DiagramExportMeta)>,
    pub on_open_focus: Rc<dyn Fn(String, u32)>,
}

/// 构造嵌入卡片 GTK4 控件，支持同步安装已预准备的结果或异步后台加载。
pub fn build_embed_card(
    req: EmbedRequest,
    precomputed: Option<PreparedEmbed>,
    callbacks: EmbedCallbacks,
) -> gtk4::Widget {
    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    card.add_css_class("hortus-embed-card");
    card.set_halign(gtk4::Align::Start);
    // 卡片最小宽跟随阅读栏可用宽，而不是固定 220px 压扁内嵌正文。
    card.set_size_request((req.width - 16.0).max(240.0).round() as i32, -1);

    let header_btn = gtk4::Button::new();
    header_btn.add_css_class("flat");
    header_btn.add_css_class("hortus-embed-header");
    header_btn.set_halign(gtk4::Align::Fill);

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let icon = gtk4::Image::from_icon_name(if req.target.ends_with(".mmd") {
        "network-workgroup-symbolic"
    } else {
        "text-x-generic-symbolic"
    });
    let initial_title = match (&req.alias, &req.heading) {
        (Some(a), Some(h)) => format!("{a} › {h}"),
        (Some(a), None) => a.clone(),
        (None, Some(h)) => format!("{} › {h}", req.target),
        (None, None) => req.target.clone(),
    };
    let title_label = gtk4::Label::new(Some(&initial_title));
    title_label.set_ellipsize(pango::EllipsizeMode::End);
    title_label.set_halign(gtk4::Align::Start);
    title_label.set_hexpand(true);
    hbox.append(&icon);
    hbox.append(&title_label);
    header_btn.set_child(Some(&hbox));
    card.append(&header_btn);

    let body_slot = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    card.append(&body_slot);

    if let Some(prepared) = precomputed {
        populate_embed_card(
            &body_slot,
            &header_btn,
            &title_label,
            &icon,
            &req,
            prepared,
            &callbacks,
        );
    } else {
        let loading = gtk4::Label::new(Some("正在解析嵌入内容…"));
        loading.add_css_class("dim-label");
        loading.add_css_class("caption");
        loading.set_halign(gtk4::Align::Start);
        body_slot.append(&loading);

        let req_bg = req.clone();
        glib::MainContext::default().spawn_local(async move {
            let res = gio::spawn_blocking(move || prepare_embed_bg(&req_bg))
                .await
                .unwrap_or_else(|_| Err("嵌入解析线程异常".into()));
            while let Some(child) = body_slot.first_child() {
                body_slot.remove(&child);
            }
            match res {
                Ok(prepared) => {
                    populate_embed_card(
                        &body_slot,
                        &header_btn,
                        &title_label,
                        &icon,
                        &req,
                        prepared,
                        &callbacks,
                    );
                }
                Err(msg) => {
                    show_embed_message(&body_slot, &msg);
                }
            }
        });
    }

    card.upcast()
}

fn populate_embed_card(
    body_slot: &gtk4::Box,
    header_btn: &gtk4::Button,
    title_label: &gtk4::Label,
    icon: &gtk4::Image,
    req: &EmbedRequest,
    prepared: PreparedEmbed,
    callbacks: &EmbedCallbacks,
) {
    let info = &prepared.info;
    let mut title = if info.display_title.is_empty() {
        req.target.clone()
    } else {
        info.display_title.clone()
    };
    if let Some(ref h) = info.heading
        && !h.is_empty()
    {
        title.push_str(" › ");
        title.push_str(h);
    }
    title_label.set_text(&title);
    if let Some(ref resolved) = info.resolved_path {
        header_btn.set_tooltip_text(Some(resolved));
        header_btn.set_sensitive(true);
        let open_cb = callbacks.on_open_note.clone();
        let target_path = resolved.clone();
        let target_heading = info.heading.clone().or_else(|| req.heading.clone());
        header_btn.connect_clicked(move |_| {
            (open_cb)(target_path.clone(), target_heading.clone());
        });
    } else {
        header_btn.set_sensitive(false);
    }
    if info.is_mmd {
        icon.set_icon_name(Some("network-workgroup-symbolic"));
    }

    match info.kind.as_str() {
        "note" => {
            let mut next_ancestors = req.ancestors.clone();
            if let Some(ref p) = info.resolved_path {
                next_ancestors.push(p.clone());
            }
            let note_rel = info
                .resolved_path
                .clone()
                .unwrap_or_else(|| req.target.clone());
            let env = ReadingEnv {
                note_path: note_rel,
                vault_root: req.vault_root.clone(),
                core: Arc::clone(&req.core),
                handle: Arc::clone(&req.handle),
                diagram_engine: Arc::clone(&req.diagram_engine),
                math_service: Arc::clone(&req.math_service),
                font_size: req.font_size,
                content_width: EmbedRequest::content_width(req.width),
                ancestors: next_ancestors,
                depth: req.depth + 1,
                parent_window: None,
                callbacks: callbacks.clone(),
                on_open_source: Rc::new(|_, _| {}),
                on_open_tag: callbacks.on_open_tag.clone(),
            };
            let body_widget = build_embedded_note_body(
                &env,
                info,
                &prepared.media,
                &prepared.math,
                &prepared.diagrams,
            );
            body_slot.append(&body_widget);
        }
        "diagram" => {
            if let Some(diagram) = prepared.diagrams.into_iter().next() {
                let target_note = info
                    .resolved_path
                    .clone()
                    .unwrap_or_else(|| req.target.clone());
                let open_note = callbacks.on_open_note.clone();
                let handle = Arc::clone(&req.handle);
                let src_for_links = target_note.clone();
                let core = Arc::clone(&req.core);
                let vault = req.vault_root.clone();
                let note_for_collapse = target_note.clone();
                let export_cb = callbacks.on_export_diagram.clone();
                let focus_cb = callbacks.on_open_focus.clone();
                let note_for_focus = target_note.clone();

                let diag_callbacks = DiagramCallbacks {
                    on_collapse_changed: Rc::new(move |ordinal, collapsed, _| {
                        let _ = core.remember_diagram(
                            vault.clone(),
                            note_for_collapse.clone(),
                            ordinal,
                            collapsed,
                        );
                    }),
                    on_open_link: Rc::new(move |target, heading| {
                        let res = handle.resolve_link(src_for_links.clone(), target);
                        if let Some(path) = res.resolved_path {
                            (open_note)(path, heading);
                        }
                    }),
                    on_open_source: Rc::new(|_, _| {}),
                    on_focus: Rc::new(move |ordinal| (focus_cb)(note_for_focus.clone(), ordinal)),
                    on_export: export_cb,
                };
                let diag_widget = build_diagram_card(
                    target_note,
                    info.content.clone(),
                    true,
                    diagram,
                    Arc::clone(&req.diagram_engine),
                    diag_callbacks,
                    EmbedRequest::content_width(req.width),
                );
                body_slot.append(&diag_widget);
            } else {
                show_embed_message(body_slot, "嵌入图表无法渲染");
            }
        }
        "cycle" => show_embed_message(body_slot, &format!("循环嵌入：{}", info.display_title)),
        "depth_exceeded" => show_embed_message(
            body_slot,
            &format!("嵌入层级超过最大限制：{}", info.display_title),
        ),
        "heading_missing" => {
            let h = info
                .heading
                .as_deref()
                .or(req.heading.as_deref())
                .unwrap_or("");
            show_embed_message(body_slot, &format!("未找到目标标题：{h}"));
        }
        "ambiguous" => {
            let list: String = info
                .candidates
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            show_embed_message(body_slot, &format!("多个笔记匹配“{}”：{list}", req.target));
        }
        "broken" => show_embed_message(body_slot, &format!("未找到嵌入目标：“{}”", req.target)),
        other => show_embed_message(body_slot, &format!("暂无法嵌入内容（{other}）")),
    }
}

fn show_embed_message(slot: &gtk4::Box, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk4::Align::Start);
    label.set_wrap(true);
    slot.append(&label);
}
