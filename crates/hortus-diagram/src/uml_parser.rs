//! 类图与 ER 图共用的逐行词法：注释、指令、引号、标识符和样式。
use crate::DiagramError;
use crate::parser::syntax;
use crate::syntax::is_id_char;
use std::ops::Range;

#[derive(Clone, Copy)]
pub(crate) struct Line<'a> {
    pub at: usize,
    pub text: &'a str,
}

impl<'a> Line<'a> {
    pub fn new(at: usize, raw: &'a str) -> Self {
        Self {
            at: at + raw.len() - raw.trim_start().len(),
            text: raw.trim(),
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.at..self.at + self.text.len()
    }

    pub fn error(&self, reason: &str) -> DiagramError {
        syntax(self.range(), reason)
    }
}

pub(crate) fn lines(source: &str) -> Vec<Line<'_>> {
    let mut at = 0;
    source
        .split_inclusive('\n')
        .map(|raw| {
            let line = Line::new(at, raw);
            at += raw.len();
            line
        })
        .collect()
}

pub(crate) fn outside_quotes(text: &str, token: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if !quoted && text[index..].starts_with(token) {
            return Some(index);
        }
    }
    None
}

/// 去掉同一行内的注释和 `%%{...}%%` 指令，返回可解析的语句。
pub(crate) fn statement<'a>(mut line: Line<'a>, directive: &mut bool) -> Line<'a> {
    loop {
        if *directive {
            let Some(end) = line.text.find("}%%") else {
                return Line::new(line.at, "");
            };
            *directive = false;
            line = Line::new(line.at + end + 3, &line.text[end + 3..]);
        } else if let Some(rest) = line.text.strip_prefix("%%{") {
            *directive = true;
            line = Line::new(line.at + 3, rest);
        } else {
            let end = outside_quotes(line.text, "%%").unwrap_or(line.text.len());
            let text =
                line.text[..end].trim_start_matches(|ch: char| ch == ';' || ch.is_whitespace());
            return Line::new(line.at + end - text.len(), text.trim_end_matches(';'));
        }
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(is_id_char)
}

/// 读取标识符；`-` 只在不是关系线起点时属于名字。
pub(crate) fn id<'a>(text: &'a str, cursor: &mut usize) -> Option<&'a str> {
    let start = *cursor;
    let mut end = start;
    while end < text.len() {
        let rest = &text[end..];
        let ch = rest.chars().next()?;
        if (ch == '-' && (rest.starts_with("--") || rest.starts_with("-.")))
            || (ch == '.' && rest.starts_with(".."))
            || !is_id_char(ch)
        {
            break;
        }
        end += ch.len_utf8();
    }
    if end == start {
        return None;
    }
    *cursor = end;
    Some(&text[start..end])
}

pub(crate) fn skip_space(text: &str, cursor: &mut usize) {
    while let Some(ch) = text[*cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        *cursor += ch.len_utf8();
    }
}

/// 读取引号字符串，返回内容范围（不含引号）和引号后的偏移。
pub(crate) fn quoted(text: &str, cursor: &mut usize) -> Option<(Range<usize>, usize)> {
    if !text[*cursor..].starts_with('"') {
        return None;
    }
    let start = *cursor + 1;
    let mut escaped = false;
    let mut end = None;
    for (relative, ch) in text[start..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            end = Some(start + relative);
            break;
        }
    }
    let end = end?;
    *cursor = end + 1;
    Some((start..end, *cursor))
}

/// 读取 `[显示名]` 或 `["显示名"]`，返回显示文本和结束偏移。
pub(crate) fn bracket_label(text: &str, cursor: &mut usize) -> Option<(String, usize)> {
    if !text[*cursor..].starts_with('[') {
        return None;
    }
    let close = text[*cursor..].find(']')? + *cursor;
    let inner = text[*cursor + 1..close].trim();
    let label = inner
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(inner)
        .replace("\\\"", "\"");
    *cursor = close + 1;
    Some((label, *cursor))
}

/// 读取 `:::class` 并返回类名。
pub(crate) fn class_suffix<'a>(text: &'a str, cursor: &mut usize) -> Option<&'a str> {
    if !text[*cursor..].starts_with(":::") {
        return None;
    }
    *cursor += 3;
    id(text, cursor)
}

/// 第一处非 `:::` 的冒号把语句切成声明头和任意文本。
pub(crate) fn property(text: &str) -> (&str, Option<&str>) {
    let mut cursor = 0;
    while let Some(index) = outside_quotes(&text[cursor..], ":") {
        let at = cursor + index;
        if text[at..].starts_with(":::") {
            cursor = at + 3;
        } else {
            return (text[..at].trim_end(), Some(text[at + 1..].trim()));
        }
    }
    (text, None)
}

pub(crate) fn subslice_offset(source: &str, child: &str) -> usize {
    child.as_ptr() as usize - source.as_ptr() as usize
}

/// 拆分 `classDef` 之后的类名和属性文本。
pub(crate) fn class_definition(value: &str) -> Option<(&str, &str)> {
    value
        .split_once(char::is_whitespace)
        .filter(|(name, properties)| valid_id(name) && !properties.trim().is_empty())
}

/// `class A,B cls` 或 `cssClass "A,B" cls` 的类赋值。
pub(crate) fn assignments(value: &str) -> Option<(Vec<String>, String)> {
    let value = value.trim();
    let (ids, class) = if let Some(rest) = value.strip_prefix('"') {
        let end = rest.rfind('"')?;
        (&rest[..end], rest[end + 1..].trim())
    } else {
        value.rsplit_once(char::is_whitespace)?
    };
    if !valid_id(class) {
        return None;
    }
    let ids = ids
        .split(',')
        .map(str::trim)
        .map(|id| valid_id(id).then(|| id.to_owned()))
        .collect::<Option<Vec<_>>>()?;
    Some((ids, class.to_owned()))
}
