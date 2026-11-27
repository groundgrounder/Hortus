// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: AGPL-3.0-only

//! Fontconfig 私有字体注册与 `libpangocairo-1.0` 原生绑定。

use gtk4::cairo;
use gtk4::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use gtk4::pango;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::Path;

unsafe extern "C" {
    fn FcInit() -> c_int;
    fn FcConfigGetCurrent() -> *mut c_void;
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const c_char) -> c_int;

    fn pango_cairo_font_map_new() -> *mut pango::ffi::PangoFontMap;
    fn pango_cairo_font_map_set_default(fontmap: *mut pango::ffi::PangoFontMap);
    fn pango_cairo_create_context(cr: *mut cairo::ffi::cairo_t) -> *mut pango::ffi::PangoContext;
    fn pango_cairo_context_set_font_options(
        context: *mut pango::ffi::PangoContext,
        options: *const cairo::ffi::cairo_font_options_t,
    );
    fn pango_cairo_update_context(
        cr: *mut cairo::ffi::cairo_t,
        context: *mut pango::ffi::PangoContext,
    );
    fn pango_cairo_update_layout(
        cr: *mut cairo::ffi::cairo_t,
        layout: *mut pango::ffi::PangoLayout,
    );
    fn pango_cairo_show_layout(cr: *mut cairo::ffi::cairo_t, layout: *mut pango::ffi::PangoLayout);
    fn pango_cairo_show_layout_line(
        cr: *mut cairo::ffi::cairo_t,
        line: *mut pango::ffi::PangoLayoutLine,
    );
}

/// 初始化进程内 Fontconfig 并注册单个私有字体文件，不写入系统或用户字体配置。
pub fn fc_add_app_font_file(path: &Path) -> Result<(), String> {
    let raw = path
        .to_str()
        .ok_or_else(|| crate::i18n::fmt1(crate::i18n::Key::FontsNonUtf8Path, path.display()))?;
    let cstr = CString::new(raw)
        .map_err(|_| crate::i18n::tr(crate::i18n::Key::FontsNulPath).to_string())?;
    // SAFETY: FcInit / FcConfigGetCurrent / FcConfigAppFontAddFile 接收有效 C 字符串指针并在失败时返回 0。
    unsafe {
        let mut config = FcConfigGetCurrent();
        if config.is_null() {
            if FcInit() == 0 {
                return Err(crate::i18n::tr(crate::i18n::Key::FontsFcInitFailed).into());
            }
            config = FcConfigGetCurrent();
        }
        if config.is_null() {
            return Err(crate::i18n::tr(crate::i18n::Key::FontsFcConfigNull).into());
        }
        if FcConfigAppFontAddFile(config, cstr.as_ptr()) == 0 {
            return Err(crate::i18n::fmt1(crate::i18n::Key::FontsFcAddFailed, raw));
        }
    }
    Ok(())
}

/// 创建反映当前 Fontconfig AppFont 配置的新 `PangoCairoFontMap`。
pub fn new_font_map() -> pango::FontMap {
    // SAFETY: pango_cairo_font_map_new 返回新建的 GObject 引用，由 from_glib_full 接管。
    unsafe { pango::FontMap::from_glib_full(pango_cairo_font_map_new()) }
}

/// 将指定 `FontMap` 设为当前线程 `PangoCairo` 默认字体映射（仅主线程完成后调用）。
pub fn set_default_font_map(font_map: &pango::FontMap) {
    let ptr: *mut pango::ffi::PangoFontMap = font_map.to_glib_none().0;
    // SAFETY: pango_cairo_font_map_set_default 对有效 PangoCairoFontMap 增加内部引用或替换默认映射。
    unsafe {
        pango_cairo_font_map_set_default(ptr);
    }
}

/// 为 Cairo 上下文创建并同步 `PangoContext`，绑定指定 `FontMap` 并禁用提示微调以保证矢量保真。
pub fn create_pango_context(
    cr: &cairo::Context,
    font_map: Option<&pango::FontMap>,
) -> pango::Context {
    let cr_ptr = cr.to_raw_none();
    // SAFETY: pango_cairo_create_context 返回拥有所有权的 PangoContext，由 from_glib_full 接管。
    let ctx = unsafe { pango::Context::from_glib_full(pango_cairo_create_context(cr_ptr)) };
    if let Some(map) = font_map {
        ctx.set_font_map(Some(map));
    }
    if let Ok(mut options) = cairo::FontOptions::new() {
        options.set_hint_metrics(cairo::HintMetrics::Off);
        options.set_hint_style(cairo::HintStyle::None);
        options.set_antialias(cairo::Antialias::Gray);
        let ctx_ptr: *mut pango::ffi::PangoContext = ctx.to_glib_none().0;
        let opt_ptr = options.to_raw_none();
        // SAFETY: ctx 与 options 均在本函数内有效。
        unsafe {
            pango_cairo_context_set_font_options(ctx_ptr, opt_ptr);
        }
    }
    update_context(cr, &ctx);
    ctx
}

pub fn update_context(cr: &cairo::Context, context: &pango::Context) {
    let cr_ptr = cr.to_raw_none();
    let ctx_ptr: *mut pango::ffi::PangoContext = context.to_glib_none().0;
    // SAFETY: 指针由有效 Cairo/Pango 对象借出。
    unsafe {
        pango_cairo_update_context(cr_ptr, ctx_ptr);
    }
}

pub fn update_layout(cr: &cairo::Context, layout: &pango::Layout) {
    let cr_ptr = cr.to_raw_none();
    let layout_ptr: *mut pango::ffi::PangoLayout = layout.to_glib_none().0;
    // SAFETY: 指针由有效 Cairo/Pango 对象借出。
    unsafe {
        pango_cairo_update_layout(cr_ptr, layout_ptr);
    }
}

pub fn show_layout(cr: &cairo::Context, layout: &pango::Layout) {
    let cr_ptr = cr.to_raw_none();
    let layout_ptr: *mut pango::ffi::PangoLayout = layout.to_glib_none().0;
    // SAFETY: 指针由有效 Cairo/Pango 对象借出。
    unsafe {
        pango_cairo_show_layout(cr_ptr, layout_ptr);
    }
}

/// 按当前 Cairo 画笔位置 `(x, y)` 作为文本基线原点绘制单行 `PangoLayoutLine`。
pub fn show_layout_line(cr: &cairo::Context, line: &pango::LayoutLine) {
    let cr_ptr = cr.to_raw_none();
    let line_ptr: *mut pango::ffi::PangoLayoutLine = line.to_glib_none().0;
    // SAFETY: 指针由有效 Cairo/PangoLayoutLine 对象借出。
    unsafe {
        pango_cairo_show_layout_line(cr_ptr, line_ptr);
    }
}
