//! 树形图与一般流程图共用的词法、语句切分和节点标签原语。
//!
//! 两套解析器仍保留各自的错误契约与能力边界：这里只收纳逐字节等价的
//! 扫描逻辑，并通过 [`StatementMode`] 区分树形图与流程图对 `|`、中缀标签
//! 和注释的切分规则。

/// 语句切分选项。树形图与流程图的边界不同，不能共用同一套开关。
#[derive(Clone, Copy)]
pub(crate) struct StatementMode {
    /// 方括号外成对的 `|` 保护标签内的换行与分号。
    pub pipes: bool,
    /// `-- 标签 -->` 中的中缀标签保护其中的 `%%`、换行与分号。
    pub infix_labels: bool,
}

pub(crate) const TREE_STATEMENTS: StatementMode = StatementMode {
    pipes: false,
    infix_labels: false,
};

pub(crate) const GRAPH_STATEMENTS: StatementMode = StatementMode {
    pipes: true,
    infix_labels: true,
};

pub(crate) fn skip_space(text: &str, cursor: &mut usize) {
    *cursor += text[*cursor..].len() - text[*cursor..].trim_start().len();
}

pub(crate) fn is_id_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

pub(crate) fn connector_start(text: &str) -> bool {
    text.starts_with("--")
        || text.starts_with("-.")
        || text.starts_with("==")
        || text.starts_with("<--")
        || text.starts_with("~~~")
}

/// 指令词边界：整词，或后续紧跟空白/冒号。
pub(crate) fn starts_word(text: &str, word: &str) -> bool {
    text == word
        || text
            .strip_prefix(word)
            .is_some_and(|tail| tail.starts_with(char::is_whitespace) || tail.starts_with(':'))
}

/// 指令词后的参数；只接受空白分隔，冒号仍属于词本身。
pub(crate) fn word_tail<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    text.strip_prefix(word)
        .filter(|tail| tail.starts_with(char::is_whitespace))
        .map(str::trim_start)
}

/// 在形状起始符号之后寻找未嵌套、未引用的结束符号位置。
pub(crate) fn scan_shape_body(text: &str, body: usize, close: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    let mut nested = Vec::new();
    for (relative, ch) in text[body..].char_indices() {
        let index = body + relative;
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            continue;
        }
        if nested.is_empty() && text[index..].starts_with(close) {
            return Some(index);
        }
        match ch {
            '[' | '(' | '{' => nested.push(ch),
            ']' | ')' | '}' => {
                nested.pop();
            }
            _ => {}
        }
    }
    None
}

/// 标签反转义；调用方传入 trimmed 内容时与旧实现完全等价。
pub(crate) fn unquote(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(text);
    text.replace("\\\"", "\"")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// 样式属性按逗号切分，CSS 函数括号内的逗号不切开。切片保持原始空白，
/// 以便诊断范围仍指向源码中的实际字节。
pub(crate) fn split_style_items(properties: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth: usize = 0;
    for (i, ch) in properties.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&properties[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&properties[start..]);
    result
}

/// 切分语句，保留每条的绝对字节位置。注释中的分号不影响解析。
pub(crate) fn statements(source: &str, mode: StatementMode) -> Vec<(usize, &str)> {
    scan_statements(source, mode, false)
}

/// 只取第一条非空语句；用于在完整解析前识别图表类型。
pub(crate) fn first_statement(source: &str, mode: StatementMode) -> Option<(usize, &str)> {
    scan_statements(source, mode, true).into_iter().next()
}

fn scan_statements(source: &str, mode: StatementMode, first_only: bool) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut pipe = false;
    let mut infix = false;
    let mut comment = false;
    for (index, ch) in source.char_indices() {
        if comment {
            if ch == '\n' {
                comment = false;
                start = index + 1;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            continue;
        }
        if mode.pipes && depth == 0 && ch == '|' {
            pipe = !pipe;
            continue;
        }
        if pipe {
            continue;
        }
        if mode.infix_labels && depth == 0 {
            if source[index..].starts_with("-->") {
                infix = false;
            }
            if (index == 0 || source.as_bytes()[index - 1] != b'-')
                && source[index..].starts_with("--")
                && source[index + 2..].starts_with(char::is_whitespace)
            {
                infix = true;
            }
            if !infix && source[index..].starts_with("%%") {
                push_statement(&mut result, source, start, index);
                if first_only && !result.is_empty() {
                    return result;
                }
                comment = true;
                continue;
            }
            if infix {
                continue;
            }
        } else if depth == 0 && source[index..].starts_with("%%") {
            push_statement(&mut result, source, start, index);
            if first_only && !result.is_empty() {
                return result;
            }
            comment = true;
            continue;
        }
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            '\n' | ';' if depth == 0 => {
                push_statement(&mut result, source, start, index);
                if first_only && !result.is_empty() {
                    return result;
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    if !comment {
        push_statement(&mut result, source, start, source.len());
    }
    result
}

fn push_statement<'a>(
    result: &mut Vec<(usize, &'a str)>,
    source: &'a str,
    start: usize,
    end: usize,
) {
    let raw = &source[start..end];
    let text = raw.trim();
    if !text.is_empty() {
        result.push((start + raw.len() - raw.trim_start().len(), text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str, mode: StatementMode) -> Vec<&str> {
        statements(source, mode)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    #[test]
    fn first_statement_skips_leading_comments_and_blank_statements() {
        let source = "%% note; still comment\n\n;mindmap\n  root\n";
        assert_eq!(
            first_statement(source, TREE_STATEMENTS),
            Some((25, "mindmap"))
        );
        assert_eq!(first_statement("%% only comment", TREE_STATEMENTS), None);
    }

    #[test]
    fn quoted_comment_markers_do_not_end_statements() {
        let source = "flowchart TB\nA[\"a %% b ; c\"] --> B\nB --> C";
        assert_eq!(
            texts(source, TREE_STATEMENTS),
            ["flowchart TB", "A[\"a %% b ; c\"] --> B", "B --> C"]
        );
        // 流程图额外用 | 与中缀标签保护换行和分号，树形图保持旧边界。
        let labeled = "flowchart TB\nA -->|one\ntwo; %%| B\nA -- x; %% y --> C";
        assert_eq!(
            texts(labeled, GRAPH_STATEMENTS),
            [
                "flowchart TB",
                "A -->|one\ntwo; %%| B",
                "A -- x; %% y --> C"
            ]
        );
        assert_eq!(
            texts(labeled, TREE_STATEMENTS),
            ["flowchart TB", "A -->|one", "two", "A -- x"]
        );
    }

    #[test]
    fn style_split_keeps_function_commas_and_raw_offsets() {
        let text = "fill:rgba(1,2,3,.5), stroke-width:2px,font-size:18px";
        let items = split_style_items(text);
        assert_eq!(
            items,
            ["fill:rgba(1,2,3,.5)", " stroke-width:2px", "font-size:18px"]
        );
        assert_eq!(items[1].as_ptr() as usize - text.as_ptr() as usize, 20);
    }
}
