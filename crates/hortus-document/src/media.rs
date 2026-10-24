//! 表格的语义、统一列宽和显式源码对齐；图片只解析库内本地路径。
use crate::{markdown, reading};
use hortus_diagram::FontMetrics;
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::{ops::Range, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub runs: Vec<reading::Run>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub alignments: Vec<Align>,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Clone, Debug)]
pub struct TableLayout {
    pub widths: Vec<f64>,
    /// 每列的下限：四个汉字宽和该列最长的词取大者
    pub floors: Vec<f64>,
    pub minimum: f64,
    pub width: f64,
    pub scrolls: bool,
}

impl Table {
    pub fn parse(text: &str) -> Option<(Range<usize>, Self)> {
        let mut table = None;
        let mut range = 0..0;
        let mut row = Vec::new();
        for (event, span) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
            match event {
                Event::Start(Tag::Table(alignments)) => {
                    range = span;
                    table = Some(Self {
                        alignments: alignments
                            .iter()
                            .map(|alignment| match alignment {
                                Alignment::Center => Align::Center,
                                Alignment::Right => Align::Right,
                                _ => Align::Left,
                            })
                            .collect(),
                        rows: Vec::new(),
                    });
                }
                Event::Start(Tag::TableHead | Tag::TableRow) if table.is_some() => row.clear(),
                Event::Start(Tag::TableCell) if table.is_some() => {
                    // 单元格两侧的空格只是源码排版，带进显示会让居中、右对齐偏一格
                    let source = text[span].trim_matches([' ', '\t']);
                    let content = reading::inline(source);
                    row.push(Cell {
                        text: content.text,
                        runs: content.runs,
                    });
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow) if table.is_some() => {
                    table.as_mut().unwrap().rows.push(std::mem::take(&mut row));
                }
                Event::End(TagEnd::Table) => return table.map(|table| (range, table)),
                _ => {}
            }
        }
        None
    }

    pub fn layout(&self, available: f64, size: f64, fonts: &mut dyn FontMetrics) -> TableLayout {
        let available = if available.is_finite() {
            available.max(1.0)
        } else {
            700.0
        };
        let minimum = fonts.measure("汉字宽度", size, 400).width + 20.0;
        // 一列最窄要放得下其中最长的词，否则英文会在词中间断开；超长的词（网址等）按 12 个汉字封顶
        let longest = fonts.measure(&"汉".repeat(12), size, 400).width + 20.0;
        let mut widths = vec![minimum; self.alignments.len()];
        let mut floors = vec![minimum; self.alignments.len()];
        for (row_index, row) in self.rows.iter().enumerate() {
            for (column, cell) in row.iter().enumerate().take(widths.len()) {
                // 与正文共用类别，测量时保留单元格内的字重变化。
                let weight = if row_index == 0 { 700 } else { 400 };
                let units: Vec<u16> = cell.text.encode_utf16().collect();
                let mut measured = 0.0;
                for run in &cell.runs {
                    let value = String::from_utf16_lossy(&units[run.display.clone()]);
                    let weight = if run.mask & 2 != 0 { 700 } else { weight };
                    measured += fonts.measure(&value, size, weight).width;
                    for word in words(&value) {
                        let word = fonts.measure(word, size, weight).width + 20.0;
                        floors[column] = floors[column].max(word.min(longest));
                    }
                }
                widths[column] = widths[column].max(measured + 20.0);
            }
        }
        let floor_total = floors.iter().sum::<f64>();
        let scrolls = floor_total > available;
        if scrolls {
            // 每列压到下限仍放不下：整张表在表格内部横向滚动
            widths.clone_from(&floors);
        } else if widths.iter().sum::<f64>() > available {
            // 水位法只压缩最宽的列，较短的列保持自然宽度；每列不低于自己的下限。
            let fit = |cap: f64| -> f64 {
                widths
                    .iter()
                    .zip(&floors)
                    .map(|(width, floor)| width.min(cap).max(*floor))
                    .sum()
            };
            let mut low = minimum;
            let mut high = widths.iter().copied().fold(minimum, f64::max);
            for _ in 0..60 {
                let cap = (low + high) / 2.0;
                if fit(cap) > available {
                    high = cap;
                } else {
                    low = cap;
                }
            }
            for (width, floor) in widths.iter_mut().zip(&floors) {
                *width = width.min(low).max(*floor);
            }
        }
        TableLayout {
            width: widths.iter().sum(),
            widths,
            floors,
            minimum,
            scrolls,
        }
    }
}

/// 不能在中间断行的片段：按空白切开；中日韩字符之间本来就能断，每个字单独算
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().flat_map(|chunk| {
        let mut pieces = Vec::new();
        let mut start = 0;
        for (index, character) in chunk.char_indices() {
            if is_cjk(character) {
                if start < index {
                    pieces.push(&chunk[start..index]);
                }
                pieces.push(&chunk[index..index + character.len_utf8()]);
                start = index + character.len_utf8();
            }
        }
        if start < chunk.len() {
            pieces.push(&chunk[start..]);
        }
        pieces
    })
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3000..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0xff00..=0xffef)
}

#[derive(Clone, Debug)]
pub struct Image {
    pub target: String,
    pub alt: String,
    pub wiki: bool,
    pub width: Option<u32>,
}

impl Image {
    pub(crate) fn wiki(inner: &str) -> Self {
        let (target, alt) = inner.split_once('|').unwrap_or((inner, inner));
        let width = inner.split_once('|').and_then(|(_, value)| {
            let (width, height) = value.split_once('x').unwrap_or((value, "1"));
            let number = |value: &str| {
                (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| value.parse::<u32>().ok().filter(|value| *value > 0))
                    .flatten()
            };
            number(height)?;
            number(width)
        });
        Self {
            target: target.into(),
            alt: if width.is_some() { target } else { alt }.into(),
            wiki: true,
            width,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Table(Table),
    Image(Image),
}

#[derive(Clone, Debug)]
pub struct Media {
    pub source: Range<usize>,
    pub display: Range<usize>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImagePath {
    Local(std::path::PathBuf),
    Rejected(&'static str),
}

pub use crate::attachments::{AttachmentStats, ImageResolver};

pub fn resolve_image(root: &Path, note: &Path, target: &str, wiki: bool) -> ImagePath {
    ImageResolver::default().resolve(root, note, target, wiki)
}

pub(crate) fn resolve_direct(root: &Path, note: &Path, target: &str, wiki: bool) -> ImagePath {
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("https:") || lower.starts_with("http:") || target.starts_with("//") {
        return ImagePath::Rejected("image.remote");
    }
    if target.starts_with('/')
        || target.contains(':')
        || target.contains('\0')
        || target.contains('\\')
        || note.is_absolute()
    {
        return ImagePath::Rejected("image.outside");
    }
    let Some(decoded) = percent_decode(target) else {
        return ImagePath::Rejected("image.invalid_path");
    };
    let target = Path::new(&decoded);
    if target.is_absolute() {
        return ImagePath::Rejected("image.outside");
    }
    let Ok(root) = root.canonicalize() else {
        return ImagePath::Rejected("image.missing");
    };
    let parent = note.parent().unwrap_or(Path::new(""));
    let mut candidates = vec![root.join(parent).join(target)];
    if wiki {
        candidates.insert(0, root.join(target));
    }
    let mut missing = true;
    for candidate in candidates {
        // 词法越界也必须拒绝，不能靠不存在的目标掩盖 .. 穿越。
        let mut depth = 0usize;
        let relative = candidate.strip_prefix(&root).unwrap();
        for part in relative.components() {
            match part {
                std::path::Component::ParentDir if depth == 0 => {
                    return ImagePath::Rejected("image.outside");
                }
                std::path::Component::ParentDir => depth -= 1,
                std::path::Component::Normal(_) => depth += 1,
                _ => {}
            }
        }
        let path = match candidate.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                // 不存在的文件仍检查其最近存在的祖先，阻止库外符号链接。
                let mut ancestor = candidate.parent();
                while let Some(parent) = ancestor {
                    if let Ok(parent) = parent.canonicalize() {
                        if !parent.starts_with(&root) {
                            return ImagePath::Rejected("image.outside");
                        }
                        break;
                    }
                    ancestor = parent.parent();
                }
                continue;
            }
        };
        if !path.starts_with(&root) {
            return ImagePath::Rejected("image.outside");
        }
        missing = false;
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !image_extension(&extension) {
            return ImagePath::Rejected("image.unsupported");
        }
        return ImagePath::Local(path);
    }
    ImagePath::Rejected(if missing {
        "image.missing"
    } else {
        "image.not_file"
    })
}

pub(crate) fn image_extension(extension: &str) -> bool {
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg"
    )
}

pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let high = (input.next()? as char).to_digit(16)?;
            let low = (input.next()? as char).to_digit(16)?;
            (high * 16 + low) as u8
        } else {
            byte
        });
    }
    String::from_utf8(bytes)
        .ok()
        .filter(|s| !s.contains(['\0', ':', '\\']) && !s.starts_with('/'))
}

#[derive(Clone, Debug)]
pub struct Patch {
    pub source: Range<usize>,
    pub replacement: String,
}

fn source_columns(line: &str) -> Vec<&str> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = if line.ends_with('|')
        && line[..line.len() - 1]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 0
    {
        &line[..line.len() - 1]
    } else {
        line
    };
    let mut cells = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if character == '|' && !escaped {
            cells.push(line[start..index].trim());
            start = index + 1;
        }
        escaped = character == '\\' && !escaped;
    }
    cells.push(line[start..].trim());
    cells
}

pub fn source_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            if c.is_ascii() {
                1
            } else if matches!(c, '\u{300}'..='\u{36f}' | '\u{fe00}'..='\u{fe0f}') {
                0
            } else {
                2
            }
        })
        .sum()
}

pub fn align_table(text: &str, caret: usize) -> Option<Patch> {
    let mut cache = reading::Cache::default();
    let model = cache.model(text, false);
    let units: Vec<u16> = text.encode_utf16().collect();
    for block in model.blocks {
        for media in &block.content.media {
            let Kind::Table(table) = &media.kind else {
                continue;
            };
            let mut source =
                block.source.start + media.source.start..block.source.start + media.source.end;
            let line_start = units[..source.start]
                .iter()
                .rposition(|unit| *unit == 10)
                .map_or(0, |index| index + 1);
            let prefix = String::from_utf16_lossy(&units[line_start..source.start]);
            // 解析器的范围从首个单元格开始；补回每行的缩进或引用前缀。
            if !prefix.chars().all(|c| matches!(c, ' ' | '\t' | '>')) {
                continue;
            }
            source.start = line_start;
            if !source.contains(&caret) {
                continue;
            }
            let original = String::from_utf16_lossy(&units[source.clone()]);
            let lines = original
                .lines()
                .map(|line| line.strip_prefix(&prefix).map(source_columns))
                .collect::<Option<Vec<_>>>()?;
            let mut widths = vec![3usize; table.alignments.len()];
            for (row, cells) in lines.iter().enumerate() {
                if row == 1 {
                    continue;
                }
                for (column, cell) in cells.iter().enumerate().take(widths.len()) {
                    widths[column] = widths[column].max(source_width(cell));
                }
            }
            let ending = if original.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            let mut replacement = String::new();
            for (row, cells) in lines.iter().enumerate() {
                if row > 0 {
                    replacement.push_str(ending);
                }
                replacement.push_str(&prefix);
                replacement.push('|');
                for (column, width) in widths.iter().enumerate() {
                    let cell = cells.get(column).copied().unwrap_or("");
                    replacement.push(' ');
                    if row == 1 {
                        let align = table.alignments[column];
                        replacement.push(if align == Align::Center { ':' } else { '-' });
                        replacement.push_str(&"-".repeat(width - 2));
                        replacement.push(if align != Align::Left { ':' } else { '-' });
                    } else {
                        replacement.push_str(cell);
                        replacement.push_str(&" ".repeat(width.saturating_sub(source_width(cell))));
                    }
                    replacement.push_str(" |");
                }
            }
            if original.ends_with('\n') {
                replacement.push_str(ending);
            }
            return (replacement != original).then_some(Patch {
                source,
                replacement,
            });
        }
    }
    None
}

pub(crate) fn inline_spans(text: &str) -> Vec<(markdown::Kind, Range<usize>)> {
    markdown::analyze_bytes(text)
}
