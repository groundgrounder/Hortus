// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! SVG 安全清洗与 knowledge-vault 路径边界校验。

use std::fmt::Write as _;
use std::path::PathBuf;

/// 校验路径是否经过符号链接还原并严格处于目标知识库根目录之内。
pub fn checked_within_vault(path: &str, vault_root: &str) -> Option<PathBuf> {
    if path.is_empty() || vault_root.is_empty() {
        return None;
    }
    let root = std::fs::canonicalize(vault_root).ok()?;
    let target = std::fs::canonicalize(path).ok()?;
    target.starts_with(&root).then_some(target)
}

#[derive(Debug)]
pub struct SvgSanitizeOutcome {
    pub bytes: Vec<u8>,
    pub stripped_external_refs: usize,
    pub stripped_scripts: usize,
}

fn is_dangerous_uri(value: &str) -> bool {
    let trimmed = value.trim().to_ascii_lowercase();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ftp://")
        || trimmed.starts_with("file://")
        || trimmed.starts_with("javascript:")
        || trimmed.starts_with("vbscript:")
        || trimmed.starts_with("//")
}

fn style_has_external_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("@import")
        || (lower.contains("url(")
            && (lower.contains("http:")
                || lower.contains("https:")
                || lower.contains("ftp:")
                || lower.contains("file:")
                || lower.contains("//")))
}

fn sanitize_css_urls(value: &str) -> (String, usize) {
    if !style_has_external_url(value) {
        return (value.to_string(), 0);
    }
    let mut stripped = 0usize;
    let mut out = String::with_capacity(value.len());
    for part in value.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if style_has_external_url(trimmed) {
            stripped += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(trimmed);
    }
    (out, stripped)
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_xml_attr(s: &str) -> String {
    escape_xml_text(s)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 在内存中解析并清洗本地 SVG：删除脚本、事件处理器与外部 URL，确保零联网加载。
pub fn sanitize_svg_bytes(raw: &[u8]) -> Result<SvgSanitizeOutcome, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "SVG 文件非有效 UTF-8 编码".to_string())?;
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("SVG XML 解析失败：{e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("SVG 根节点不是 <svg>".into());
    }

    let mut stripped_external_refs = 0usize;
    let mut stripped_scripts = 0usize;
    let mut out = String::with_capacity(text.len());

    fn emit_node(
        node: roxmltree::Node<'_, '_>,
        out: &mut String,
        stripped_ext: &mut usize,
        stripped_scripts: &mut usize,
    ) {
        match node.node_type() {
            roxmltree::NodeType::Element => {
                let tag = node.tag_name().name();
                if matches!(
                    tag.to_ascii_lowercase().as_str(),
                    "script" | "foreignobject" | "iframe" | "embed" | "object" | "audio" | "video"
                ) {
                    *stripped_scripts += 1;
                    return;
                }
                let _ = write!(out, "<{tag}");
                if let Some(ns) = node.tag_name().namespace()
                    && node.is_root()
                {
                    let _ = write!(out, " xmlns=\"{}\"", escape_xml_attr(ns));
                } else if tag == "svg" && !node.attributes().any(|a| a.name() == "xmlns") {
                    out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
                }

                for attr in node.attributes() {
                    let name = attr.name();
                    let lower = name.to_ascii_lowercase();
                    if lower.starts_with("on") {
                        *stripped_scripts += 1;
                        continue;
                    }
                    let val = attr.value();
                    if matches!(
                        lower.as_str(),
                        "href" | "xlink:href" | "src" | "action" | "formaction"
                    ) && is_dangerous_uri(val)
                    {
                        *stripped_ext += 1;
                        continue;
                    }
                    if lower == "style" {
                        let (cleaned, n) = sanitize_css_urls(val);
                        *stripped_ext += n;
                        if !cleaned.is_empty() {
                            let _ = write!(out, " style=\"{}\"", escape_xml_attr(&cleaned));
                        }
                        continue;
                    }
                    let _ = write!(out, " {name}=\"{}\"", escape_xml_attr(val));
                }
                if tag == "svg" && text_uses_xlink(node) && !out.contains("xmlns:xlink=") {
                    out.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
                }
                out.push('>');
                for child in node.children() {
                    if tag.eq_ignore_ascii_case("style") && child.is_text() {
                        let raw_css = child.text().unwrap_or("");
                        if style_has_external_url(raw_css) {
                            *stripped_ext += 1;
                            let cleaned: String = raw_css
                                .lines()
                                .filter(|line| !style_has_external_url(line))
                                .collect::<Vec<_>>()
                                .join("\n");
                            out.push_str(&escape_xml_text(&cleaned));
                        } else {
                            out.push_str(&escape_xml_text(raw_css));
                        }
                    } else {
                        emit_node(child, out, stripped_ext, stripped_scripts);
                    }
                }
                let _ = write!(out, "</{tag}>");
            }
            roxmltree::NodeType::Text => {
                if let Some(txt) = node.text() {
                    out.push_str(&escape_xml_text(txt));
                }
            }
            _ => {}
        }
    }

    emit_node(
        root,
        &mut out,
        &mut stripped_external_refs,
        &mut stripped_scripts,
    );
    Ok(SvgSanitizeOutcome {
        bytes: out.into_bytes(),
        stripped_external_refs,
        stripped_scripts,
    })
}

fn text_uses_xlink(node: roxmltree::Node<'_, '_>) -> bool {
    node.descendants()
        .any(|d| d.attributes().any(|a| a.name().starts_with("xlink:")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_svg_strips_scripts_event_handlers_and_external_urls() {
        let raw = br##"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" width="64" height="64">
  <script>fetch("https://evil.example/")</script>
  <style>@import url("https://evil.example/a.css"); rect { fill: #3b82f6; }</style>
  <a href="https://evil.example/click"><rect x="4" y="4" width="56" height="56" onclick="evil()"/></a>
  <image href="https://evil.example/tracker.png" width="10" height="10"/>
</svg>"##;
        let out = sanitize_svg_bytes(raw).expect("valid SVG parses cleanly");
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("<script"));
        assert!(!text.contains("onload="));
        assert!(!text.contains("onclick="));
        assert!(!text.contains("https://evil.example"));
        assert!(out.stripped_scripts >= 3);
        assert!(out.stripped_external_refs >= 3);
        assert!(text.contains("<rect"));
    }

    #[test]
    fn checked_within_vault_blocks_escape() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("ok.png");
        std::fs::write(&inside, b"png").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(
            checked_within_vault(inside.to_str().unwrap(), dir.path().to_str().unwrap()).is_some()
        );
        assert!(
            checked_within_vault(
                outside.path().to_str().unwrap(),
                dir.path().to_str().unwrap()
            )
            .is_none()
        );
    }
}
