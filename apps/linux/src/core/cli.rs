// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::model::ColorSchemeChoice;
use std::path::PathBuf;

/// L2 截图场景：由 `--edit-knowledge-scene` 在截图前装配真实界面状态。
#[cfg(feature = "automation")]
pub const L2_SCENES: [&str; 5] = [
    "search-backlinks",
    "tags",
    "quick-open",
    "edit-find-completion",
    "rename-links",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliOptions {
    pub vault: Option<PathBuf>,
    pub open_notes: Vec<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub color_scheme: Option<ColorSchemeChoice>,
    pub new_instance: bool,
    pub show_help: bool,
    #[cfg(feature = "automation")]
    pub overview: bool,
    #[cfg(feature = "automation")]
    pub hide_top_bar: bool,
    #[cfg(feature = "automation")]
    pub edit_mode: bool,
    #[cfg(feature = "automation")]
    pub focus_diagram: Option<u32>,
    #[cfg(feature = "automation")]
    pub scroll_fraction: Option<f64>,
    #[cfg(feature = "automation")]
    pub smoke_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub reading_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub screenshot: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub quit_after_ms: Option<u64>,
    #[cfg(feature = "automation")]
    pub edit_knowledge_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub edit_knowledge_scene: Option<String>,
    #[cfg(feature = "automation")]
    pub wikilink_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub layout_width_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub layout_hide_sidebars: bool,
    #[cfg(feature = "automation")]
    pub live_resize_json: Option<PathBuf>,
}

impl CliOptions {
    /// 是否需要隔离的单次测试/自动化进程（`NON_UNIQUE`）。
    pub fn wants_non_unique(&self) -> bool {
        if self.new_instance
            || self.vault.is_some()
            || !self.open_notes.is_empty()
            || self.width.is_some()
            || self.height.is_some()
            || self.color_scheme.is_some()
        {
            return true;
        }
        #[cfg(feature = "automation")]
        if self.has_automation_mode() {
            return true;
        }
        false
    }

    #[cfg(feature = "automation")]
    pub fn has_automation_mode(&self) -> bool {
        self.smoke_json.is_some()
            || self.reading_json.is_some()
            || self.edit_knowledge_json.is_some()
            || self.edit_knowledge_scene.is_some()
            || self.wikilink_json.is_some()
            || self.layout_width_json.is_some()
            || self.live_resize_json.is_some()
            || self.screenshot.is_some()
            || self.focus_diagram.is_some()
            || self.edit_mode
            || self.scroll_fraction.is_some()
            || self.overview
            || self.hide_top_bar
            || self.quit_after_ms.is_some()
    }

    #[cfg(feature = "automation")]
    pub fn locks_window_size(&self) -> bool {
        self.width.is_some()
            && self.height.is_some()
            && self.live_resize_json.is_none()
            && (self.screenshot.is_some()
                || self.smoke_json.is_some()
                || self.reading_json.is_some())
    }

    #[cfg(not(feature = "automation"))]
    pub fn locks_window_size(&self) -> bool {
        false
    }

    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut options = Self::default();
        let mut iter = args.into_iter().map(Into::into).skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    options.show_help = true;
                    return Ok(options);
                }
                "--vault" => {
                    let value = iter.next().ok_or("--vault 需要提供目录路径")?;
                    options.vault = Some(PathBuf::from(value));
                }
                "--open" => {
                    let value = iter.next().ok_or("--open 需要提供笔记相对路径")?;
                    options.open_notes.push(value);
                }
                "--width" => {
                    let value = iter.next().ok_or("--width 需要提供整数像素值")?;
                    let parsed = value
                        .parse::<i32>()
                        .map_err(|_| format!("--width 参数无效：{value}"))?;
                    if parsed < 320 {
                        return Err(format!("--width 过小：{parsed}"));
                    }
                    options.width = Some(parsed);
                }
                "--height" => {
                    let value = iter.next().ok_or("--height 需要提供整数像素值")?;
                    let parsed = value
                        .parse::<i32>()
                        .map_err(|_| format!("--height 参数无效：{value}"))?;
                    if parsed < 240 {
                        return Err(format!("--height 过小：{parsed}"));
                    }
                    options.height = Some(parsed);
                }
                "--color-scheme" => {
                    let value = iter
                        .next()
                        .ok_or("--color-scheme 需要提供 light|dark|system")?;
                    options.color_scheme = Some(ColorSchemeChoice::parse(&value)?);
                }
                "--new-instance" => {
                    options.new_instance = true;
                }
                #[cfg(feature = "automation")]
                "--overview" => options.overview = true,
                #[cfg(feature = "automation")]
                "--hide-top-bar" => options.hide_top_bar = true,
                #[cfg(feature = "automation")]
                "--edit" => options.edit_mode = true,
                #[cfg(feature = "automation")]
                "--focus-diagram" => {
                    let value = iter
                        .next()
                        .ok_or("--focus-diagram 需要提供图块序号（从 1 开始）")?;
                    let parsed = value
                        .parse::<u32>()
                        .map_err(|_| format!("--focus-diagram 参数无效：{value}"))?;
                    if parsed == 0 {
                        return Err("--focus-diagram 序号须 >= 1".into());
                    }
                    options.focus_diagram = Some(parsed);
                }
                #[cfg(feature = "automation")]
                "--scroll-fraction" => {
                    let value = iter
                        .next()
                        .ok_or("--scroll-fraction 需要提供 0..1 浮点数")?;
                    let parsed = value
                        .parse::<f64>()
                        .map_err(|_| format!("--scroll-fraction 参数无效：{value}"))?;
                    options.scroll_fraction = Some(parsed.clamp(0.0, 1.0));
                }
                #[cfg(feature = "automation")]
                "--smoke-json" => {
                    let value = iter.next().ok_or("--smoke-json 需要提供输出路径")?;
                    options.smoke_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--reading-json" => {
                    let value = iter.next().ok_or("--reading-json 需要提供输出路径")?;
                    options.reading_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--screenshot" => {
                    let value = iter.next().ok_or("--screenshot 需要提供输出 PNG 路径")?;
                    options.screenshot = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--quit-after-ms" => {
                    let value = iter.next().ok_or("--quit-after-ms 需要提供毫秒整数")?;
                    let parsed = value
                        .parse::<u64>()
                        .map_err(|_| format!("--quit-after-ms 参数无效：{value}"))?;
                    options.quit_after_ms = Some(parsed);
                }
                #[cfg(feature = "automation")]
                "--edit-knowledge-json" => {
                    let value = iter
                        .next()
                        .ok_or("--edit-knowledge-json 需要提供输出路径")?;
                    options.edit_knowledge_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--edit-knowledge-scene" => {
                    let value = iter
                        .next()
                        .ok_or("--edit-knowledge-scene 需要提供场景名称")?;
                    if !L2_SCENES.contains(&value.as_str()) {
                        return Err(format!("未知的 L2 截图场景：{value}"));
                    }
                    options.edit_knowledge_scene = Some(value);
                }
                #[cfg(feature = "automation")]
                "--wikilink-json" => {
                    let value = iter.next().ok_or("--wikilink-json 需要提供输出路径")?;
                    options.wikilink_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--layout-width-json" => {
                    let value = iter.next().ok_or("--layout-width-json 需要提供输出路径")?;
                    options.layout_width_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--layout-hide-sidebars" => {
                    options.layout_hide_sidebars = true;
                }
                #[cfg(feature = "automation")]
                "--live-resize-json" => {
                    let value = iter.next().ok_or("--live-resize-json 需要提供输出路径")?;
                    options.live_resize_json = Some(PathBuf::from(value));
                }
                other => return Err(format!("未知参数：{other}")),
            }
        }
        Ok(options)
    }

    pub fn usage() -> &'static str {
        #[cfg(feature = "automation")]
        {
            "用法：hortus-gtk [选项]
  --vault <目录>             启动时直接打开指定知识库
  --open <相对路径>          初始打开指定笔记（可多次传入）
  --width <像素>             指定窗口初始宽度
  --height <像素>            指定窗口初始高度
  --color-scheme <模式>      主题：light | dark | system
  --new-instance             启动独立新进程实例
  --overview                 初始打开 AdwTabOverview 标签全景网格
  --hide-top-bar             初始隐藏顶栏
  --edit                     打开首篇笔记后切入编辑模式
  --focus-diagram <序号>     打开首篇笔记后进入指定图表专注模式
  --scroll-fraction <比例>   截图或导出前滚动到指定纵向比例 (0..1)
  --smoke-json <路径>        运行 L0 行为自检并导出 JSON 后退出
  --reading-json <路径>      运行 L1 阅读态与场景完整自检并导出 JSON 后退出
  --edit-knowledge-json <路径>
                             运行 L2 编辑/保存/知识导航/文件管理自检并导出 JSON 后退出
  --wikilink-json <路径>      运行阅读视图双链真实点击回归并导出 JSON 后退出
  --layout-width-json <路径>  测量阅读视图图表/表格/图片实际宽与侧栏重排并导出 JSON 后退出
  --layout-hide-sidebars      宽度适配回归后在测量前收起左右侧栏
  --live-resize-json <路径>   运行真实窗口动态缩放自检并导出 JSON 后退出
  --edit-knowledge-scene <场景>
                             L2 界面截图场景：search-backlinks | tags | quick-open |
                             edit-find-completion | rename-links
  --screenshot <路径>        渲染完成后导出窗口 PNG 截图后退出
  --quit-after-ms <毫秒>     到达延迟后退出"
        }
        #[cfg(not(feature = "automation"))]
        {
            "用法：hortus-gtk [选项]
  --vault <目录>             启动时直接打开指定知识库
  --open <相对路径>          初始打开指定笔记（可多次传入）
  --width <像素>             指定窗口初始宽度
  --height <像素>            指定窗口初始高度
  --color-scheme <模式>      主题：light | dark | system
  --new-instance             启动独立新进程实例"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_options_parse_and_reject_invalid_values() {
        let parsed = CliOptions::parse([
            "hortus-gtk",
            "--vault",
            "/tmp/vault",
            "--open",
            "Start here.md",
            "--open",
            "Learning/Study loop.md",
            "--width",
            "1280",
            "--height",
            "820",
            "--color-scheme",
            "dark",
        ])
        .unwrap();
        assert_eq!(parsed.vault, Some(PathBuf::from("/tmp/vault")));
        assert_eq!(
            parsed.open_notes,
            vec!["Start here.md", "Learning/Study loop.md"]
        );
        assert_eq!(parsed.width, Some(1280));
        assert_eq!(parsed.height, Some(820));
        assert_eq!(parsed.color_scheme, Some(ColorSchemeChoice::Dark));

        assert!(CliOptions::parse(["hortus-gtk", "--unknown"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--width", "100"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--color-scheme", "sepia"]).is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn automation_cli_options_parse() {
        let parsed = CliOptions::parse([
            "hortus-gtk",
            "--overview",
            "--smoke-json",
            "/tmp/smoke.json",
            "--screenshot",
            "/tmp/out.png",
            "--live-resize-json",
            "/tmp/resize.json",
        ])
        .unwrap();
        assert!(parsed.overview);
        assert_eq!(parsed.smoke_json, Some(PathBuf::from("/tmp/smoke.json")));
        assert_eq!(parsed.screenshot, Some(PathBuf::from("/tmp/out.png")));
        assert_eq!(
            parsed.live_resize_json,
            Some(PathBuf::from("/tmp/resize.json"))
        );
    }
}
