// Hortus - Linux GTK4 + libadwaita 原生界面原型
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::future::poll_fn;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

pub const APP_ID: &str = "io.github.groundgrounder.Hortus";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSchemeChoice {
    System,
    Light,
    Dark,
}

impl ColorSchemeChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            other => Err(format!(
                "不支持的主题模式：{other}（可选 light|dark|system）"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelRegion {
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderKind {
    Backlinks,
    Changes,
    Terminal,
}

impl PlaceholderKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Backlinks => "backlinks",
            Self::Changes => "changes",
            Self::Terminal => "terminal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "backlinks" => Some(Self::Backlinks),
            "changes" => Some(Self::Changes),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Backlinks => "反链",
            Self::Changes => "改动",
            Self::Terminal => "终端",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Backlinks => "insert-link-symbolic",
            Self::Changes => "document-revert-symbolic",
            Self::Terminal => "utilities-terminal-symbolic",
        }
    }

    pub fn milestone_note(self) -> &'static str {
        match self {
            Self::Backlinks => {
                "反链面板将在 L2 接入内核 KnowledgeIndex，展示引用当前笔记的双链、嵌入与图内链接。"
            }
            Self::Changes => {
                "改动审阅面板将在 L3 接入内核 HistoryStore，展示全库会话 diff 与按处/按文件/按会话回滚。"
            }
            Self::Terminal => {
                "内置终端与 Agent 会话将在 L3 基于 vte-gtk4 接入，启动前先拍全库快照并支持发送路径与选区。"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelPlacement {
    locations: BTreeMap<PlaceholderKind, PanelRegion>,
}

impl Default for PanelPlacement {
    fn default() -> Self {
        let mut locations = BTreeMap::new();
        locations.insert(PlaceholderKind::Backlinks, PanelRegion::Right);
        locations.insert(PlaceholderKind::Changes, PanelRegion::Right);
        locations.insert(PlaceholderKind::Terminal, PanelRegion::Bottom);
        Self { locations }
    }
}

impl PanelPlacement {
    pub fn region_of(&self, kind: PlaceholderKind) -> PanelRegion {
        *self.locations.get(&kind).unwrap_or(&PanelRegion::Right)
    }

    pub fn toggle_region(&mut self, kind: PlaceholderKind) -> PanelRegion {
        let next = match self.region_of(kind) {
            PanelRegion::Right => PanelRegion::Bottom,
            PanelRegion::Bottom => PanelRegion::Right,
        };
        self.locations.insert(kind, next);
        next
    }

    pub fn views_in(&self, region: PanelRegion) -> Vec<PlaceholderKind> {
        [
            PlaceholderKind::Backlinks,
            PlaceholderKind::Changes,
            PlaceholderKind::Terminal,
        ]
        .into_iter()
        .filter(|kind| self.region_of(*kind) == region)
        .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliOptions {
    pub vault: Option<PathBuf>,
    pub open_notes: Vec<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub color_scheme: Option<ColorSchemeChoice>,
    pub overview: bool,
    pub hide_top_bar: bool,
    pub edit_mode: bool,
    pub focus_diagram: Option<u32>,
    pub scroll_fraction: Option<f64>,
    pub smoke_json: Option<PathBuf>,
    pub reading_json: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub quit_after_ms: Option<u64>,
    pub show_help: bool,
}

impl CliOptions {
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
                "--overview" => {
                    options.overview = true;
                }
                "--hide-top-bar" => {
                    options.hide_top_bar = true;
                }
                "--edit" => {
                    options.edit_mode = true;
                }
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
                "--scroll-fraction" => {
                    let value = iter
                        .next()
                        .ok_or("--scroll-fraction 需要提供 0..1 浮点数")?;
                    let parsed = value
                        .parse::<f64>()
                        .map_err(|_| format!("--scroll-fraction 参数无效：{value}"))?;
                    options.scroll_fraction = Some(parsed.clamp(0.0, 1.0));
                }
                "--smoke-json" => {
                    let value = iter.next().ok_or("--smoke-json 需要提供输出路径")?;
                    options.smoke_json = Some(PathBuf::from(value));
                }
                "--reading-json" => {
                    let value = iter.next().ok_or("--reading-json 需要提供输出路径")?;
                    options.reading_json = Some(PathBuf::from(value));
                }
                "--screenshot" => {
                    let value = iter.next().ok_or("--screenshot 需要提供输出 PNG 路径")?;
                    options.screenshot = Some(PathBuf::from(value));
                }
                "--quit-after-ms" => {
                    let value = iter.next().ok_or("--quit-after-ms 需要提供毫秒整数")?;
                    let parsed = value
                        .parse::<u64>()
                        .map_err(|_| format!("--quit-after-ms 参数无效：{value}"))?;
                    options.quit_after_ms = Some(parsed);
                }
                other => return Err(format!("未知参数：{other}")),
            }
        }
        Ok(options)
    }

    pub fn usage() -> &'static str {
        "用法：hortus-gtk [选项]
  --vault <目录>             启动时直接打开指定知识库
  --open <相对路径>          初始打开指定笔记（可多次传入）
  --width <像素>             指定窗口初始宽度
  --height <像素>            指定窗口初始高度
  --color-scheme <模式>      主题：light | dark | system
  --overview                 初始打开 AdwTabOverview 标签全景网格
  --hide-top-bar             初始隐藏顶栏
  --edit                     打开首篇笔记后切入编辑模式
  --focus-diagram <序号>     打开首篇笔记后进入指定图表专注模式
  --scroll-fraction <比例>   截图或导出前滚动到指定纵向比例 (0..1)
  --smoke-json <路径>        运行 L0 行为自检并导出 JSON 后退出
  --reading-json <路径>      运行 L1 阅读态与场景完整自检并导出 JSON 后退出
  --screenshot <路径>        渲染完成后导出窗口 PNG 截图后退出
  --quit-after-ms <毫秒>     到达延迟后退出"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleRun {
    pub start_utf16: u64,
    pub length_utf16: u64,
    pub mask: u32,
}

/// 将 `hortus_ffi::document::SourceStyles::packed_runs` 解码为样式片段列表。
/// 每条记录固定 20 字节小端序：`start:u64`、`length:u64`、`mask:u32`。
pub fn decode_source_styles(packed: &[u8]) -> Vec<StyleRun> {
    packed
        .chunks_exact(20)
        .map(|chunk| StyleRun {
            start_utf16: u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
            length_utf16: u64::from_le_bytes(chunk[8..16].try_into().unwrap()),
            mask: u32::from_le_bytes(chunk[16..20].try_into().unwrap()),
        })
        .collect()
}

/// 将源码 UTF-16 偏移转换为 `GtkTextBuffer` 使用的 Unicode 字符偏移。
pub fn utf16_offset_to_char_offset(text: &str, target_utf16: usize) -> usize {
    let mut utf16 = 0usize;
    let mut chars = 0usize;
    for ch in text.chars() {
        if utf16 >= target_utf16 {
            return chars;
        }
        utf16 += ch.len_utf16();
        chars += 1;
    }
    chars
}

/// 将源码 UTF-16 区间 `(start, length)` 转换为 `(char_start, char_end)`。
pub fn utf16_range_to_char_range(
    text: &str,
    start_utf16: usize,
    length_utf16: usize,
) -> (usize, usize) {
    let end_utf16 = start_utf16.saturating_add(length_utf16);
    let mut utf16 = 0usize;
    let mut chars = 0usize;
    let mut char_start = None;
    for ch in text.chars() {
        if char_start.is_none() && utf16 >= start_utf16 {
            char_start = Some(chars);
        }
        if utf16 >= end_utf16 {
            return (char_start.unwrap_or(chars), chars);
        }
        utf16 += ch.len_utf16();
        chars += 1;
    }
    (char_start.unwrap_or(chars), chars)
}

/// 从源码中提取给定 UTF-16 起点与长度的切片。
pub fn utf16_slice(text: &str, start_utf16: usize, length_utf16: usize) -> Option<String> {
    let mapper = Utf16Mapper::new(text);
    let b0 = mapper.utf16_to_byte(start_utf16);
    let b1 = mapper.utf16_to_byte(start_utf16.saturating_add(length_utf16));
    text.get(b0..b1).map(str::to_string)
}

/// 线性预扫描一次字符串，提供 O(log N) 精确 UTF-16 ↔ Unicode char ↔ UTF-8 byte 索引转换。
pub struct Utf16Mapper {
    utf16_offsets: Vec<usize>,
    byte_offsets: Vec<usize>,
}

impl Utf16Mapper {
    pub fn new(text: &str) -> Self {
        let mut utf16_offsets = Vec::with_capacity(text.chars().count() + 1);
        let mut byte_offsets = Vec::with_capacity(utf16_offsets.capacity());
        let mut u16 = 0usize;
        for (b, ch) in text.char_indices() {
            utf16_offsets.push(u16);
            byte_offsets.push(b);
            u16 += ch.len_utf16();
        }
        utf16_offsets.push(u16);
        byte_offsets.push(text.len());
        Self {
            utf16_offsets,
            byte_offsets,
        }
    }

    pub fn utf16_to_char(&self, target_utf16: usize) -> usize {
        self.utf16_offsets
            .partition_point(|&u| u < target_utf16)
            .min(self.utf16_offsets.len().saturating_sub(1))
    }

    pub fn utf16_to_byte(&self, target_utf16: usize) -> usize {
        let idx = self.utf16_to_char(target_utf16);
        self.byte_offsets[idx]
    }

    pub fn byte_to_utf16(&self, target_byte: usize) -> usize {
        let idx = self
            .byte_offsets
            .partition_point(|&b| b < target_byte)
            .min(self.utf16_offsets.len().saturating_sub(1));
        self.utf16_offsets[idx]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntryItem {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub directory: bool,
}

impl From<&hortus_ffi::TreeEntry> for TreeEntryItem {
    fn from(entry: &hortus_ffi::TreeEntry) -> Self {
        Self {
            path: entry.path.clone(),
            parent: entry.parent.clone(),
            name: entry.name.clone(),
            directory: entry.directory,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingItem {
    pub level: u8,
    pub title: String,
    pub start: u64,
}

impl From<hortus_ffi::DocumentHeading> for HeadingItem {
    fn from(heading: hortus_ffi::DocumentHeading) -> Self {
        Self {
            level: heading.level,
            title: heading.title,
            start: heading.start,
        }
    }
}

/// 跨后台线程与 GLib 主循环异步传递批次的无轮询通道。
pub struct NotifyQueue<T> {
    inner: Mutex<NotifyQueueInner<T>>,
}

struct NotifyQueueInner<T> {
    items: VecDeque<T>,
    closed: bool,
    waker: Option<Waker>,
}

impl<T> NotifyQueue<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(NotifyQueueInner {
                items: VecDeque::new(),
                closed: false,
                waker: None,
            }),
        })
    }

    pub fn push(&self, item: T) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.closed {
            return;
        }
        inner.items.push_back(item);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.closed = true;
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    pub async fn next(&self) -> Option<T> {
        poll_fn(|cx| {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(item) = inner.items.pop_front() {
                Poll::Ready(Some(item))
            } else if inner.closed {
                Poll::Ready(None)
            } else {
                inner.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

/// 按内核规则排序目录树节点：目录优先，其次使用 `hortus_vault::compare_names`。
pub fn sort_tree_entries(entries: &mut [TreeEntryItem]) {
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| hortus_vault::compare_names(&a.name, &b.name))
    });
}

pub fn is_note_file_name(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("md")
                || ext.eq_ignore_ascii_case("markdown")
                || ext.eq_ignore_ascii_case("mmd")
        })
}

#[derive(Clone, Debug, Serialize)]
pub struct SmokeHeading {
    pub level: u8,
    pub title: String,
    pub start: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SmokeTab {
    pub path: String,
    pub title: String,
    pub char_count: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SmokeReport {
    pub vault_root: String,
    pub scan_complete: bool,
    pub scan_micros: u64,
    pub entry_count: usize,
    pub note_count: usize,
    pub note_paths: Vec<String>,
    pub open_tabs: Vec<SmokeTab>,
    pub active_note: Option<String>,
    pub outline_headings: Vec<SmokeHeading>,
    pub outline_scroll_moved: bool,
    pub outline_scroll_before: f64,
    pub outline_scroll_after: f64,
    pub tab_overview_opened: bool,
    pub tab_overview_listed_count: usize,
    pub wide_width: i32,
    pub wide_left_collapsed: bool,
    pub wide_right_collapsed: bool,
    pub narrow_width: i32,
    pub narrow_left_collapsed: bool,
    pub narrow_right_collapsed: bool,
    pub top_bar_hidden_ok: bool,
    pub top_bar_restored_ok: bool,
    pub edit_mode_toggled_on: bool,
    pub edit_mode_toggled_off: bool,
    pub edit_buffer_modified_in_memory: bool,
    pub panel_move_verified: bool,
    pub startup_ms: u64,
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
            "--overview",
            "--smoke-json",
            "/tmp/smoke.json",
            "--screenshot",
            "/tmp/out.png",
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
        assert!(parsed.overview);
        assert_eq!(parsed.smoke_json, Some(PathBuf::from("/tmp/smoke.json")));
        assert_eq!(parsed.screenshot, Some(PathBuf::from("/tmp/out.png")));

        assert!(CliOptions::parse(["hortus-gtk", "--unknown"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--width", "100"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--color-scheme", "sepia"]).is_err());
    }

    #[test]
    fn utf16_offsets_map_accurately_across_cjk_and_surrogate_pairs() {
        let text = "# 标题😀\n正文";
        // `# ` = 2 UTF-16, `标题` = 2 UTF-16, `😀` = 2 UTF-16 (1 char), `\n` = 1 UTF-16
        assert_eq!(utf16_offset_to_char_offset(text, 0), 0);
        assert_eq!(utf16_offset_to_char_offset(text, 4), 4);
        assert_eq!(utf16_offset_to_char_offset(text, 6), 5);
        assert_eq!(utf16_offset_to_char_offset(text, 7), 6);
        assert_eq!(utf16_range_to_char_range(text, 2, 4), (2, 5));
    }

    #[test]
    fn decode_source_styles_unpacks_little_endian_runs() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4u64.to_le_bytes());
        bytes.extend_from_slice(&9u64.to_le_bytes());
        bytes.extend_from_slice(&(2u32 << 24 | 1u32).to_le_bytes());
        let runs = decode_source_styles(&bytes);
        assert_eq!(
            runs,
            vec![StyleRun {
                start_utf16: 4,
                length_utf16: 9,
                mask: (2u32 << 24) | 1,
            }]
        );
    }

    #[test]
    fn sort_tree_entries_puts_directories_first_naturally() {
        let mut entries = vec![
            TreeEntryItem {
                path: "note10.md".into(),
                parent: String::new(),
                name: "note10.md".into(),
                directory: false,
            },
            TreeEntryItem {
                path: "note2.md".into(),
                parent: String::new(),
                name: "note2.md".into(),
                directory: false,
            },
            TreeEntryItem {
                path: "Assets".into(),
                parent: String::new(),
                name: "Assets".into(),
                directory: true,
            },
        ];
        sort_tree_entries(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["Assets", "note2.md", "note10.md"]
        );
    }

    #[test]
    fn panel_placement_moves_placeholders_between_right_and_bottom() {
        let mut placement = PanelPlacement::default();
        assert_eq!(
            placement.views_in(PanelRegion::Right),
            vec![PlaceholderKind::Backlinks, PlaceholderKind::Changes]
        );
        assert_eq!(
            placement.views_in(PanelRegion::Bottom),
            vec![PlaceholderKind::Terminal]
        );

        assert_eq!(
            placement.toggle_region(PlaceholderKind::Backlinks),
            PanelRegion::Bottom
        );
        assert_eq!(
            placement.views_in(PanelRegion::Right),
            vec![PlaceholderKind::Changes]
        );
        assert_eq!(
            placement.views_in(PanelRegion::Bottom),
            vec![PlaceholderKind::Backlinks, PlaceholderKind::Terminal]
        );
    }
}
