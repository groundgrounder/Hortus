//! Minimal frontmatter reader for the three fields the knowledge graph uses.
//!
//! The parser is deliberately forgiving: only `tags`, `aliases` and `source`
//! are read, unknown or complex YAML is ignored, and no input can panic.

pub(crate) struct Frontmatter {
    /// Raw tag text without the leading `#`, with its 1-based source line.
    pub tags: Vec<(String, usize)>,
    pub aliases: Vec<String>,
    pub source: Option<String>,
    /// Byte offset where the body starts (after the closing delimiter).
    pub end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Tags,
    Aliases,
    Source,
    Other,
}

pub(crate) fn parse(text: &str) -> Option<Frontmatter> {
    let start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let first_end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index);
    if text.get(start..first_end)?.trim_end() != "---" {
        return None;
    }

    let mut offset = if first_end < text.len() {
        first_end + 1
    } else {
        text.len()
    };
    let mut line_number = 2;
    let mut header = Vec::new();
    let end = loop {
        if offset > text.len() {
            return None;
        }
        let line_end = text[offset..]
            .find('\n')
            .map_or(text.len(), |index| offset + index);
        let line = text[offset..line_end].trim_end_matches('\r');
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            break if line_end < text.len() {
                line_end + 1
            } else {
                text.len()
            };
        }
        header.push((line_number, line));
        line_number += 1;
        if line_end == text.len() {
            return None;
        }
        offset = line_end + 1;
    };

    let mut frontmatter = Frontmatter {
        tags: Vec::new(),
        aliases: Vec::new(),
        source: None,
        end,
    };
    let mut current = Key::Other;
    for (number, line) in header {
        let trimmed = line.trim_start();
        if let Some(item) = trimmed.strip_prefix('-')
            && (item.is_empty() || item.starts_with(char::is_whitespace))
        {
            let item = item.trim();
            if !item.is_empty() {
                push_item(&mut frontmatter, current, item, number);
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            // Nested YAML we do not model (or a continuation line).
            continue;
        }
        let Some(colon) = line.find(':') else {
            current = Key::Other;
            continue;
        };
        let key = &line[..colon];
        if key.is_empty()
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            current = Key::Other;
            continue;
        }
        current = match key {
            "tags" => Key::Tags,
            "aliases" => Key::Aliases,
            "source" => Key::Source,
            _ => Key::Other,
        };
        let value = strip_comment(&line[colon + 1..]).trim();
        if value.is_empty() {
            continue;
        }
        match current {
            Key::Tags | Key::Aliases => {
                let mut items = flow_list(value);
                if items.is_empty() {
                    items = scalar_items(current, value);
                }
                for item in items {
                    push_item(&mut frontmatter, current, &item, number);
                }
            }
            Key::Source => {
                let value = unquote(value);
                if !value.is_empty() {
                    frontmatter.source = Some(value);
                }
            }
            Key::Other => {}
        }
    }
    Some(frontmatter)
}

fn push_item(frontmatter: &mut Frontmatter, key: Key, item: &str, line: usize) {
    match key {
        Key::Tags => {
            let value = item.trim().trim_start_matches('#').trim();
            if !value.is_empty() {
                frontmatter.tags.push((value.to_owned(), line));
            }
        }
        Key::Aliases => {
            let value = unquote(item.trim());
            if !value.is_empty() {
                frontmatter.aliases.push(value);
            }
        }
        Key::Source => {
            let value = unquote(item.trim());
            if !value.is_empty() && frontmatter.source.is_none() {
                frontmatter.source = Some(value);
            }
        }
        Key::Other => {}
    }
}

fn scalar_items(key: Key, value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let quoted = trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')));
    let value = unquote(trimmed);
    match key {
        Key::Tags => value
            .split(|character: char| character == ',' || character.is_whitespace())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect(),
        Key::Aliases if quoted => vec![value],
        Key::Aliases => value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn flow_list(value: &str) -> Vec<String> {
    let value = value.trim();
    let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Vec::new();
    };
    split_commas(inner)
        .into_iter()
        .map(|item| unquote(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

fn split_commas(value: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote = None;
    for (index, character) in value.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character == ',' => {
                items.push(&value[start..index]);
                start = index + 1;
            }
            None => {}
        }
    }
    items.push(&value[start..]);
    items
}

fn strip_comment(value: &str) -> &str {
    let mut quote = None;
    let mut depth = 0usize;
    let mut previous = ' ';
    for (index, character) in value.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character == '[' => depth += 1,
            None if character == ']' => depth = depth.saturating_sub(1),
            None if character == '#' && depth == 0 && previous.is_whitespace() => {
                return &value[..index];
            }
            None => {}
        }
        previous = character;
    }
    value
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flow_and_block_lists() {
        let text = "---\ntags: [rust, #nested/tag]\naliases:\n  - First\n  - 'Second alias'\nsource: https://example.com/spec\n---\n# Body\n";
        let frontmatter = parse(text).unwrap();
        assert_eq!(
            frontmatter.tags,
            vec![("rust".to_owned(), 2), ("nested/tag".to_owned(), 2)]
        );
        assert_eq!(frontmatter.aliases, vec!["First", "Second alias"]);
        assert_eq!(
            frontmatter.source.as_deref(),
            Some("https://example.com/spec")
        );
        assert_eq!(&text[frontmatter.end..], "# Body\n");
    }

    #[test]
    fn reads_scalar_tags_and_ignores_unknown_yaml() {
        let text = "---\ntags: \"alpha, beta\"\naliases: Single Name\nunknown:\n  nested: {a: b}\n  - odd\nother: [1, 2\n---\nbody";
        let frontmatter = parse(text).unwrap();
        assert_eq!(
            frontmatter.tags,
            vec![("alpha".to_owned(), 2), ("beta".to_owned(), 2)]
        );
        assert_eq!(frontmatter.aliases, vec!["Single Name"]);
        assert_eq!(frontmatter.source, None);
    }

    #[test]
    fn requires_a_closing_delimiter_on_the_first_line() {
        assert!(parse("no frontmatter\n---\n").is_none());
        assert!(parse("---\ntags: [unclosed\n").is_none());
        assert!(parse("---\n---\nbody").is_some());
        assert!(parse("\u{feff}---\ntags: [a]\n...\nbody").is_some());
    }

    #[test]
    fn comments_and_quotes_do_not_confuse_values() {
        let text =
            "---\ntags: [a, \"b # not comment\"] # trailing\naliases: \"name, with comma\"\n---\n";
        let frontmatter = parse(text).unwrap();
        assert_eq!(
            frontmatter.tags,
            vec![("a".to_owned(), 2), ("b # not comment".to_owned(), 2)]
        );
        assert_eq!(frontmatter.aliases, vec!["name, with comma"]);
    }
}
