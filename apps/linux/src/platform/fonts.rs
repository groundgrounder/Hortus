// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! 进程内私有 KaTeX 字体注册与字体描述映射；注册动作仅在后台工作线程执行。

use crate::platform::pangocairo;
use gtk4::gio;
use gtk4::pango;
use gtk4::prelude::{FontExt, FontMapExt};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;

pub const KATEX_FONT_FILES: [&str; 20] = [
    "KaTeX_AMS-Regular.ttf",
    "KaTeX_Caligraphic-Bold.ttf",
    "KaTeX_Caligraphic-Regular.ttf",
    "KaTeX_Fraktur-Bold.ttf",
    "KaTeX_Fraktur-Regular.ttf",
    "KaTeX_Main-Bold.ttf",
    "KaTeX_Main-BoldItalic.ttf",
    "KaTeX_Main-Italic.ttf",
    "KaTeX_Main-Regular.ttf",
    "KaTeX_Math-BoldItalic.ttf",
    "KaTeX_Math-Italic.ttf",
    "KaTeX_SansSerif-Bold.ttf",
    "KaTeX_SansSerif-Italic.ttf",
    "KaTeX_SansSerif-Regular.ttf",
    "KaTeX_Script-Regular.ttf",
    "KaTeX_Size1-Regular.ttf",
    "KaTeX_Size2-Regular.ttf",
    "KaTeX_Size3-Regular.ttf",
    "KaTeX_Size4-Regular.ttf",
    "KaTeX_Typewriter-Regular.ttf",
];

#[derive(Clone, Debug, Default, Serialize)]
pub struct FontRegistrationStatus {
    pub registered: bool,
    pub registered_count: usize,
    pub registered_off_main_thread: bool,
    pub fonts_dir: Option<String>,
    pub error: Option<String>,
    pub resolved_families: BTreeMap<String, String>,
}

struct FontState {
    main_thread: Option<ThreadId>,
    status: FontRegistrationStatus,
}

static FONT_STATE: OnceLock<Mutex<FontState>> = OnceLock::new();
static FONT_EPOCH: AtomicU64 = AtomicU64::new(0);

fn state_lock() -> &'static Mutex<FontState> {
    FONT_STATE.get_or_init(|| {
        Mutex::new(FontState {
            main_thread: None,
            status: FontRegistrationStatus::default(),
        })
    })
}

/// 记录 GTK 主线程 ID，用于断言后续 Fontconfig 字体注册不在主线程发生。
pub fn mark_main_thread() {
    let mut guard = state_lock().lock().unwrap_or_else(|e| e.into_inner());
    if guard.main_thread.is_none() {
        guard.main_thread = Some(std::thread::current().id());
    }
}

pub fn current_epoch() -> u64 {
    FONT_EPOCH.load(Ordering::Acquire)
}

#[cfg(feature = "automation")]
pub fn registration_status() -> FontRegistrationStatus {
    state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status
        .clone()
}

fn dir_has_all_katex_fonts(dir: &Path) -> bool {
    dir.is_dir() && KATEX_FONT_FILES.iter().all(|name| dir.join(name).is_file())
}

/// 按环境变量、安装包共享目录、`XDG_DATA_DIRS` 与工作区祖先路径查找 KaTeX 字体目录。
pub fn locate_katex_fonts_dir() -> Option<PathBuf> {
    if let Some(env_dir) = std::env::var_os("HORTUS_KATEX_FONTS_DIR") {
        let path = PathBuf::from(env_dir);
        if dir_has_all_katex_fonts(&path) {
            return Some(path);
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(bin_dir) = exe.parent()
    {
        for rel in ["../share/hortus/fonts/KaTeX", "share/hortus/fonts/KaTeX"] {
            let candidate = bin_dir.join(rel);
            if dir_has_all_katex_fonts(&candidate) {
                return Some(candidate);
            }
        }
        for ancestor in exe.ancestors().take(6) {
            let candidate = ancestor.join("apps/macos/Resources/Fonts/KaTeX");
            if dir_has_all_katex_fonts(&candidate) {
                return Some(candidate);
            }
        }
    }

    if let Some(xdg_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for base in std::env::split_paths(&xdg_dirs) {
            let candidate = base.join("hortus/fonts/KaTeX");
            if dir_has_all_katex_fonts(&candidate) {
                return Some(candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors().take(5) {
            let candidate = ancestor.join("apps/macos/Resources/Fonts/KaTeX");
            if dir_has_all_katex_fonts(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

/// 在后台线程中通过 Fontconfig 私有注册全部 20 个 KaTeX TTF（幂等）。
pub fn ensure_katex_fonts_registered_sync() -> Result<FontRegistrationStatus, String> {
    let mut guard = state_lock().lock().unwrap_or_else(|e| e.into_inner());
    if guard.status.registered {
        return Ok(guard.status.clone());
    }
    if let Some(err) = &guard.status.error {
        return Err(err.clone());
    }

    let current_tid = std::thread::current().id();
    let off_main = guard.main_thread.is_some_and(|main| main != current_tid);

    let Some(dir) = locate_katex_fonts_dir() else {
        let msg = crate::i18n::tr(crate::i18n::Key::FontsKatexMissing).to_string();
        guard.status.error = Some(msg.clone());
        return Err(msg);
    };

    let mut count = 0usize;
    for filename in KATEX_FONT_FILES {
        let full = dir.join(filename);
        pangocairo::fc_add_app_font_file(&full)?;
        count += 1;
    }

    guard.status.registered = true;
    guard.status.registered_count = count;
    guard.status.registered_off_main_thread = off_main;
    guard.status.fonts_dir = Some(dir.to_string_lossy().into_owned());
    guard.status.error = None;
    FONT_EPOCH.fetch_add(1, Ordering::Release);
    Ok(guard.status.clone())
}

/// 异步预热 KaTeX 字体（切到 `gio::spawn_blocking` 池中执行，永不阻塞主线程），并在完成后刷新主线程默认 FontMap。
pub async fn ensure_katex_fonts_registered_async() -> Result<FontRegistrationStatus, String> {
    let _status = gio::spawn_blocking(ensure_katex_fonts_registered_sync)
        .await
        .map_err(|_| crate::i18n::tr(crate::i18n::Key::FontsTaskFailed).to_string())??;

    let resolved = probe_katex_families_on_current_thread();
    let fresh_map = pangocairo::new_font_map();
    pangocairo::set_default_font_map(&fresh_map);

    let mut guard = state_lock().lock().unwrap_or_else(|e| e.into_inner());
    guard.status.resolved_families = resolved;
    Ok(guard.status.clone())
}

/// 把 RaTeX 场景输出中的字体 key（例如 `"AMS-Regular"`、`"Math-Italic"`）映射为 Fontconfig 精确字体描述。
pub fn katex_font_description(ratex_font: &str, size_px: f64) -> Option<pango::FontDescription> {
    let (prefix, style) = ratex_font.split_once('-')?;
    let family = match prefix {
        "AMS" | "Caligraphic" | "Fraktur" | "Main" | "Math" | "SansSerif" | "Script" | "Size1"
        | "Size2" | "Size3" | "Size4" | "Typewriter" => format!("KaTeX_{prefix}"),
        _ => return None,
    };
    let (weight, slant) = match style {
        "Regular" => (pango::Weight::Normal, pango::Style::Normal),
        "Bold" => (pango::Weight::Bold, pango::Style::Normal),
        "Italic" => (pango::Weight::Normal, pango::Style::Italic),
        "BoldItalic" => (pango::Weight::Bold, pango::Style::Italic),
        _ => return None,
    };
    let mut desc = pango::FontDescription::new();
    desc.set_family(&family);
    desc.set_weight(weight);
    desc.set_style(slant);
    desc.set_absolute_size((size_px.max(1.0)) * f64::from(pango::SCALE));
    Some(desc)
}

/// 生成用于 CJK / Emoji / 非 KaTeX 字符的回退 Pango 字体描述。
pub fn fallback_font_description(size_px: f64) -> pango::FontDescription {
    let mut desc = pango::FontDescription::new();
    desc.set_family("sans-serif");
    desc.set_absolute_size((size_px.max(1.0)) * f64::from(pango::SCALE));
    desc
}

/// 验证当前 FontMap 能实际匹配全部 20 个 KaTeX 字体 key 对应的 `KaTeX_*` 家族。
pub fn probe_katex_families_on_current_thread() -> BTreeMap<String, String> {
    let font_map = pangocairo::new_font_map();
    let ctx = font_map.create_context();
    let mut resolved = BTreeMap::new();
    for file in KATEX_FONT_FILES {
        if let Some(stem) = file
            .strip_prefix("KaTeX_")
            .and_then(|s| s.strip_suffix(".ttf"))
            && let Some(desc) = katex_font_description(stem, 16.0)
            && let Some(font) = font_map.load_font(&ctx, &desc)
        {
            let actual = font
                .describe()
                .family()
                .map(|s| s.to_string())
                .unwrap_or_default();
            resolved.insert(stem.to_string(), actual);
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn katex_fonts_register_off_main_and_resolve_all_20_families() {
        let mut status = std::thread::spawn(ensure_katex_fonts_registered_sync)
            .join()
            .unwrap()
            .expect("KaTeX fonts should register on background thread");
        assert!(status.registered);
        assert_eq!(status.registered_count, 20);
        let families = probe_katex_families_on_current_thread();
        status.resolved_families = families;
        assert_eq!(status.resolved_families.len(), 20);
        for (key, resolved_family) in &status.resolved_families {
            let expected_prefix = format!(
                "KaTeX_{}",
                key.split_once('-').map(|(p, _)| p).unwrap_or("")
            );
            assert_eq!(
                resolved_family, &expected_prefix,
                "KaTeX font key {key} resolved unexpectedly to {resolved_family}"
            );
        }
    }
}
