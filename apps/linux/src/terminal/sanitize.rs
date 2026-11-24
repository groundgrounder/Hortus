// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 终端输入安全清洗：路径 Shell 安全引用与选区控制序列剥离（对齐 FIX-05A）。

/// 将知识库相对路径格式化为可安全喂入 Shell 命令行的引用文本（末尾带空格，不附加回车）。
pub fn shell_quote_relative_path(path: &str) -> String {
    let trimmed = path.trim_matches('\0');
    let plain_safe = !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'/'));
    if plain_safe {
        return format!("{trimmed} ");
    }

    let has_control = trimmed
        .chars()
        .any(|ch| ch.is_control() || ('\u{007f}'..='\u{009f}').contains(&ch));
    if !has_control {
        let escaped = trimmed.replace('\'', "'\\''");
        return format!("'{escaped}' ");
    }

    // 含控制字符或换行时使用 ANSI-C $'...' 八进制/转义形式，确保绝对不会断行执行。
    let mut out = String::from("$'");
    for byte in trimmed.as_bytes() {
        match *byte {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            0x00..=0x1f | 0x7f..=0xff if !byte.is_ascii_graphic() && *byte != b' ' => {
                // 对多字节 UTF-8 字节或控制字节均使用 3 位八进制转义，避免残留控制符。
                if *byte < 0x20 || *byte >= 0x7f {
                    out.push_str(&format!("\\{:03o}", byte));
                } else {
                    out.push(*byte as char);
                }
            }
            other => out.push(other as char),
        }
    }
    out.push_str("' ");
    out
}

/// 剥离选区中的危险控制字符与伪造 bracketed-paste 终止符（如 `\x1b[201~`），严禁自动回车执行。
pub fn sanitize_selection_for_terminal(raw: &str) -> String {
    // 1. 先移除常见伪造 bracketed-paste / CSI 终止序列的完整 ESC 序列或孤立 ESC
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut chars = normalized.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // 若紧跟 '[' (CSI) 或 ']' (OSC)，吞掉控制序列直至终结字节，防止残留 "[201~" 等噪音或注入。
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        // 剥离 C0 控制符（保留 \n 和 \t）、DEL (0x7f) 以及 C1 控制符 (U+0080..=U+009F)。
        if ch == '\n' || ch == '\t' {
            out.push(ch);
            continue;
        }
        if ch.is_control() || ('\u{007f}'..='\u{009f}').contains(&ch) {
            continue;
        }
        // 剥离 Unicode 双向控制字符（防 Trojan Source）
        if matches!(
            ch,
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'
        ) {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_handles_spaces_cjk_quotes_and_control_bytes() {
        assert_eq!(shell_quote_relative_path("notes/a.md"), "notes/a.md ");
        assert_eq!(
            shell_quote_relative_path("Daily Notes/Plan for 今天.md"),
            "'Daily Notes/Plan for 今天.md' "
        );
        assert_eq!(
            shell_quote_relative_path("it's a $(touch PWNED).md"),
            "'it'\\''s a $(touch PWNED).md' "
        );
        let quoted_ctrl = shell_quote_relative_path("bad\r\n\x1bname.md");
        assert!(quoted_ctrl.starts_with("$'"));
        assert!(!quoted_ctrl.contains('\n'));
        assert!(!quoted_ctrl.contains('\r'));
        assert!(!quoted_ctrl.contains('\x1b'));
    }

    #[test]
    fn sanitize_selection_strips_bracketed_paste_spoof_and_controls() {
        let malicious =
            "hello\x1b[201~\ntouch /tmp/pwned\r\n\x1b[31mred\x1b[0m\x07\u{009b}201~world\t!";
        let clean = sanitize_selection_for_terminal(malicious);
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\r'));
        assert!(!clean.contains('\x07'));
        assert!(!clean.contains('\u{009b}'));
        assert!(!clean.contains("[201~"));
        assert_eq!(clean, "hello\ntouch /tmp/pwned\nred201~world\t!");
    }
}
