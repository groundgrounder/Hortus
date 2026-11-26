//! In-memory quick-open and full-text search.
//!
//! The index deliberately owns no files and is never persisted. Callers feed it
//! note contents as the vault scanner discovers or changes them.
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchRange {
    pub start: u32,
    pub length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickOpenResult {
    pub path: String,
    pub title: String,
    pub title_matches: Vec<MatchRange>,
    pub path_matches: Vec<MatchRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMatch {
    pub start: u32,
    pub length: u32,
    pub snippet: String,
    pub snippet_start: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullTextResult {
    pub path: String,
    pub title: String,
    pub count: u32,
    pub matches: Vec<TextMatch>,
}

#[derive(Clone, Debug)]
struct Note {
    title: String,
    text: String,
    /// `true` when no non-ASCII char in `text` folds to an ASCII char under `char::to_lowercase`.
    ascii_fold_safe: bool,
    title_folded: PreFolded,
    path_folded: PreFolded,
    file_name_folded: PreFolded,
    file_name_offset: usize,
}

#[derive(Clone, Debug)]
struct PreFolded {
    chars: Vec<char>,
    /// Each folded character maps back to the UTF-8 byte range of its source char.
    map: Vec<(usize, usize)>,
}

enum CompiledPattern {
    Ascii {
        bytes: Vec<u8>,
        first_lower: u8,
        first_upper: u8,
        prefix: Vec<usize>,
    },
    Unicode {
        chars: Vec<char>,
        prefix: Vec<usize>,
    },
}

impl CompiledPattern {
    fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        if query.is_ascii() {
            let bytes: Vec<u8> = query.bytes().map(|b| b.to_ascii_lowercase()).collect();
            let first_lower = bytes[0];
            let first_upper = first_lower.to_ascii_uppercase();
            let prefix = kmp_prefix(&bytes);
            Some(Self::Ascii {
                bytes,
                first_lower,
                first_upper,
                prefix,
            })
        } else {
            let chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
            if chars.is_empty() {
                return None;
            }
            let prefix = kmp_prefix(&chars);
            Some(Self::Unicode { chars, prefix })
        }
    }
}

fn kmp_prefix<T: Eq>(pattern: &[T]) -> Vec<usize> {
    let mut prefix = vec![0; pattern.len()];
    for index in 1..pattern.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    prefix
}

#[derive(Clone, Debug)]
struct FuzzyMatch {
    ranges: Vec<MatchRange>,
    contiguous: usize,
    word_start: bool,
    span: usize,
    start: usize,
}

#[derive(Clone, Debug)]
struct QuickCandidate {
    source: u8,
    matching: FuzzyMatch,
}

pub struct SearchIndex {
    notes: BTreeMap<String, Note>,
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchIndex {
    pub fn new() -> Self {
        Self {
            notes: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn upsert(&mut self, path: impl Into<String>, text: impl Into<String>) {
        let path = path.into();
        let text = text.into();
        let title = first_heading(&text).unwrap_or_else(|| file_name(&path));
        let ascii_fold_safe = text.is_ascii()
            || !text
                .chars()
                .any(|c| !c.is_ascii() && c.to_lowercase().any(|l| l.is_ascii()));
        let file_name_slice = Path::new(&path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&path);
        let file_name_offset = path.len() - file_name_slice.len();
        let title_folded = pre_fold(&title);
        let path_folded = pre_fold(&path);
        let file_name_folded = pre_fold(file_name_slice);
        self.notes.insert(
            path,
            Note {
                title,
                text,
                ascii_fold_safe,
                title_folded,
                path_folded,
                file_name_folded,
                file_name_offset,
            },
        );
    }

    pub fn remove(&mut self, path: &str) {
        self.notes.remove(path);
    }

    pub fn clear(&mut self) {
        self.notes.clear();
    }

    pub fn quick_open(&self, query: &str, limit: usize) -> Vec<QuickOpenResult> {
        let limit = limit.min(50);
        if query.trim().is_empty() {
            return self
                .notes
                .iter()
                .take(limit)
                .map(|(path, note)| QuickOpenResult {
                    path: path.clone(),
                    title: note.title.clone(),
                    title_matches: vec![],
                    path_matches: vec![],
                })
                .collect();
        }

        let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        if query_chars.is_empty() {
            return vec![];
        }
        let mut found_scratch = Vec::with_capacity(query_chars.len());

        let mut results = self
            .notes
            .iter()
            .filter_map(|(path, note)| {
                let title = fuzzy_match_folded(
                    &note.title,
                    &note.title_folded,
                    &query_chars,
                    &mut found_scratch,
                );
                let path_match =
                    fuzzy_match_folded(path, &note.path_folded, &query_chars, &mut found_scratch);
                let file_name = &path[note.file_name_offset..];
                let file_name_match = fuzzy_match_folded(
                    file_name,
                    &note.file_name_folded,
                    &query_chars,
                    &mut found_scratch,
                );
                let candidate = [
                    file_name_match.as_ref().map(|value| QuickCandidate {
                        source: 0,
                        matching: value.clone(),
                    }),
                    title.as_ref().map(|value| QuickCandidate {
                        source: 1,
                        matching: value.clone(),
                    }),
                    path_match.as_ref().map(|value| QuickCandidate {
                        source: 2,
                        matching: value.clone(),
                    }),
                ]
                .into_iter()
                .flatten()
                .min_by(quick_candidate_order)?;
                Some((
                    candidate,
                    QuickOpenResult {
                        path: path.clone(),
                        title: note.title.clone(),
                        title_matches: title.map(|value| value.ranges).unwrap_or_default(),
                        path_matches: path_match.map(|value| value.ranges).unwrap_or_default(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        results.sort_by(|a, b| {
            quick_candidate_order(&a.0, &b.0).then_with(|| a.1.path.cmp(&b.1.path))
        });
        results.truncate(limit);
        results.into_iter().map(|(_, result)| result).collect()
    }

    pub fn search(&self, query: &str, max_matches_per_note: usize) -> Vec<FullTextResult> {
        self.search_cancelled(query, max_matches_per_note, None)
            .unwrap_or_default()
    }

    /// Searches in path order and returns `None` when `cancelled` is set.
    /// Checking between notes and matches keeps typing responsive without a
    /// background worker or a persistent index.
    pub fn search_cancelled(
        &self,
        query: &str,
        max_matches_per_note: usize,
        cancelled: Option<&AtomicBool>,
    ) -> Option<Vec<FullTextResult>> {
        let Some(pattern) = CompiledPattern::new(query) else {
            return Some(vec![]);
        };
        struct UnicodeScratch {
            chars: Vec<char>,
            prefix: Vec<usize>,
            queue: std::collections::VecDeque<(usize, usize)>,
        }
        let mut results = Vec::new();
        let mut unicode_scratch: Option<UnicodeScratch> = None;
        for (path, note) in &self.notes {
            if cancelled.is_some_and(|token| token.load(AtomicOrdering::Acquire)) {
                return None;
            }
            let (count, matches) = match &pattern {
                CompiledPattern::Ascii {
                    bytes,
                    first_lower,
                    first_upper,
                    prefix,
                } if note.ascii_fold_safe => substring_matches_ascii(
                    &note.text,
                    bytes,
                    *first_lower,
                    *first_upper,
                    prefix,
                    max_matches_per_note,
                    cancelled,
                )?,
                CompiledPattern::Ascii { bytes, .. } => {
                    let scratch = unicode_scratch.get_or_insert_with(|| {
                        let chars: Vec<char> = bytes.iter().map(|&b| b as char).collect();
                        let prefix = kmp_prefix(&chars);
                        let queue = std::collections::VecDeque::with_capacity(chars.len());
                        UnicodeScratch {
                            chars,
                            prefix,
                            queue,
                        }
                    });
                    substring_matches_unicode(
                        &note.text,
                        &scratch.chars,
                        &scratch.prefix,
                        &mut scratch.queue,
                        max_matches_per_note,
                        cancelled,
                    )?
                }
                CompiledPattern::Unicode { chars, prefix } => {
                    let scratch = unicode_scratch.get_or_insert_with(|| UnicodeScratch {
                        chars: Vec::new(),
                        prefix: Vec::new(),
                        queue: std::collections::VecDeque::with_capacity(chars.len()),
                    });
                    substring_matches_unicode(
                        &note.text,
                        chars,
                        prefix,
                        &mut scratch.queue,
                        max_matches_per_note,
                        cancelled,
                    )?
                }
            };
            if count > 0 {
                results.push(FullTextResult {
                    path: path.clone(),
                    title: note.title.clone(),
                    count,
                    matches,
                });
            }
        }
        Some(results)
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_owned()
}

fn first_heading(text: &str) -> Option<String> {
    let mut fence: Option<(u8, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indentation = line.len() - trimmed.len();
        let marker = trimmed.as_bytes().first().copied();
        let marker_length = marker.map_or(0, |value| {
            trimmed.bytes().take_while(|byte| *byte == value).count()
        });
        if indentation <= 3
            && marker.is_some_and(|value| matches!(value, b'`' | b'~'))
            && marker_length >= 3
        {
            if let Some((open, length)) = fence {
                if marker == Some(open)
                    && marker_length >= length
                    && trimmed[marker_length..].trim().is_empty()
                {
                    fence = None;
                }
            } else {
                fence = Some((marker.unwrap(), marker_length));
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let hash_length = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if hash_length != 1 || !trimmed[hash_length..].starts_with([' ', '\t']) {
            continue;
        }
        let value = trimmed[hash_length..].trim().trim_end_matches('#').trim();
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

fn pre_fold(input: &str) -> PreFolded {
    let mut chars = Vec::with_capacity(input.len());
    let mut map = Vec::with_capacity(input.len());
    for (start, character) in input.char_indices() {
        let end = start + character.len_utf8();
        for lowered in character.to_lowercase() {
            chars.push(lowered);
            map.push((start, end));
        }
    }
    PreFolded { chars, map }
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn range(text: &str, start: usize, end: usize) -> MatchRange {
    let start = utf16_offset(text, start);
    MatchRange {
        start: start as u32,
        length: (utf16_offset(text, end) - start) as u32,
    }
}

const CANCEL_CHECK_BYTES: usize = 64 * 1024;

fn is_cancelled(cancelled: Option<&AtomicBool>) -> bool {
    cancelled.is_some_and(|token| token.load(AtomicOrdering::Relaxed))
}

/// Pure ASCII case-insensitive fast-path using tight byte scanning + KMP fallback.
/// Produces byte spans identical to `substring_matches_unicode` whenever `note.ascii_fold_safe`.
fn substring_matches_ascii(
    text: &str,
    pattern: &[u8],
    first_lower: u8,
    first_upper: u8,
    prefix: &[usize],
    limit: usize,
    cancelled: Option<&AtomicBool>,
) -> Option<(u32, Vec<TextMatch>)> {
    let bytes = text.as_bytes();
    let pat_len = pattern.len();
    if pat_len == 0 || bytes.len() < pat_len {
        return (!is_cancelled(cancelled)).then(|| (0, Vec::new()));
    }
    let mut matched = 0usize;
    let mut count = 0u32;
    let mut spans = Vec::new();
    let mut index = 0usize;
    let mut chunk_end = 0;
    while index < bytes.len() {
        if index == chunk_end {
            if is_cancelled(cancelled) {
                return None;
            }
            chunk_end = index.saturating_add(CANCEL_CHECK_BYTES).min(bytes.len());
        }
        if matched == 0 {
            // Bound each skip so long runs without the pattern's first byte still check cancellation.
            let found = bytes[index..chunk_end]
                .iter()
                .position(|&byte| byte == first_lower || byte == first_upper);
            let Some(offset) = found else {
                index = chunk_end;
                continue;
            };
            index += offset;
            if pat_len == 1 {
                count = count.saturating_add(1);
                if spans.len() < limit {
                    spans.push(index..index + 1);
                }
                index += 1;
                continue;
            }
            matched = 1;
            index += 1;
            continue;
        }
        let lowered = bytes[index].to_ascii_lowercase();
        while matched > 0 && lowered != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if lowered == pattern[matched] {
            matched += 1;
        }
        if matched == pat_len {
            count = count.saturating_add(1);
            if spans.len() < limit {
                spans.push((index + 1 - pat_len)..(index + 1));
            }
            matched = prefix[matched - 1];
        }
        index += 1;
    }
    build_text_matches(text, count, spans, cancelled)
}

fn substring_matches_unicode(
    text: &str,
    pattern: &[char],
    prefix: &[usize],
    source_ranges: &mut std::collections::VecDeque<(usize, usize)>,
    limit: usize,
    cancelled: Option<&AtomicBool>,
) -> Option<(u32, Vec<TextMatch>)> {
    if pattern.is_empty() {
        return Some((0, vec![]));
    }
    source_ranges.clear();
    let mut matched = 0;
    let mut count = 0u32;
    let mut spans = Vec::with_capacity(limit.min(8));
    for (character_index, (start, character)) in text.char_indices().enumerate() {
        if character_index % 1024 == 0
            && cancelled.is_some_and(|token| token.load(AtomicOrdering::Acquire))
        {
            return None;
        }
        let end = start + character.len_utf8();
        for lowered in character.to_lowercase() {
            source_ranges.push_back((start, end));
            if source_ranges.len() > pattern.len() {
                source_ranges.pop_front();
            }
            while matched > 0 && lowered != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if lowered == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                count = count.saturating_add(1);
                if spans.len() < limit {
                    spans.push(source_ranges.front().unwrap().0..end);
                }
                matched = prefix[matched - 1];
            }
        }
    }
    build_text_matches(text, count, spans, cancelled)
}

fn build_text_matches(
    text: &str,
    count: u32,
    spans: Vec<std::ops::Range<usize>>,
    cancelled: Option<&AtomicBool>,
) -> Option<(u32, Vec<TextMatch>)> {
    if is_cancelled(cancelled) {
        return None;
    }
    if spans.is_empty() {
        return Some((count, vec![]));
    }
    let mut graphemes = Vec::new();
    let mut next_cancel_check = CANCEL_CHECK_BYTES;
    for (offset, _) in UnicodeSegmentation::grapheme_indices(text, true) {
        if offset >= next_cancel_check {
            if is_cancelled(cancelled) {
                return None;
            }
            next_cancel_check = offset.saturating_add(CANCEL_CHECK_BYTES);
        }
        graphemes.push(offset);
    }
    graphemes.push(text.len());
    let mut last_byte = 0usize;
    let mut last_utf16 = 0usize;
    let mut utf16_at = |byte: usize| -> usize {
        if byte >= last_byte {
            last_utf16 += text[last_byte..byte].encode_utf16().count();
            last_byte = byte;
            last_utf16
        } else {
            utf16_offset(text, byte)
        }
    };
    let matches = spans
        .into_iter()
        .enumerate()
        .map(|(index, span)| {
            if index % 16 == 0 && is_cancelled(cancelled) {
                return None;
            }
            let start_index = graphemes
                .partition_point(|offset| *offset <= span.start)
                .saturating_sub(1)
                .saturating_sub(30);
            let end_index = (graphemes.partition_point(|offset| *offset < span.end) + 30)
                .min(graphemes.len() - 1);
            let snippet_start = graphemes[start_index];
            let snippet_end = graphemes[end_index];
            let snippet_start_utf16 = utf16_at(snippet_start) as u32;
            let start_utf16 = utf16_at(span.start) as u32;
            let end_utf16 = utf16_at(span.end) as u32;
            Some(TextMatch {
                start: start_utf16,
                length: end_utf16 - start_utf16,
                snippet: text[snippet_start..snippet_end].to_owned(),
                snippet_start: snippet_start_utf16,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (!is_cancelled(cancelled)).then_some((count, matches))
}

fn fuzzy_match_folded(
    text: &str,
    folded_text: &PreFolded,
    query_chars: &[char],
    found: &mut Vec<usize>,
) -> Option<FuzzyMatch> {
    if query_chars.is_empty() {
        return None;
    }
    found.clear();
    let mut cursor = 0;
    for &character in query_chars {
        let index = folded_text
            .chars
            .get(cursor..)?
            .iter()
            .position(|value| *value == character)?
            + cursor;
        found.push(index);
        cursor = index + 1;
    }
    let ranges = found
        .iter()
        .map(|index| range(text, folded_text.map[*index].0, folded_text.map[*index].1))
        .collect();
    let contiguous = found
        .windows(2)
        .filter(|pair| pair[1] == pair[0] + 1)
        .count();
    let first_byte = folded_text.map[found[0]].0;
    let word_start = first_byte == 0
        || text[..first_byte]
            .chars()
            .last()
            .is_some_and(|value| !value.is_alphanumeric());
    let last_byte = folded_text.map[*found.last()?].1;
    Some(FuzzyMatch {
        ranges,
        contiguous,
        word_start,
        span: last_byte - first_byte,
        start: first_byte,
    })
}

fn quick_candidate_order(a: &QuickCandidate, b: &QuickCandidate) -> Ordering {
    b.matching
        .contiguous
        .cmp(&a.matching.contiguous)
        .then_with(|| b.matching.word_start.cmp(&a.matching.word_start))
        .then_with(|| a.source.cmp(&b.source))
        .then_with(|| a.matching.span.cmp(&b.matching.span))
        .then_with(|| a.matching.start.cmp(&b.matching.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_ascii_skip_returns_none_without_a_match() {
        let text = "x".repeat(4 * 1024 * 1024);
        let cancelled = AtomicBool::new(true);
        assert!(
            substring_matches_ascii(&text, b"z", b'z', b'Z', &[0], 1, Some(&cancelled)).is_none()
        );
    }

    #[test]
    fn ascii_skip_keeps_matches_across_cancel_check_boundaries() {
        let text = format!("{}Aba", "x".repeat(CANCEL_CHECK_BYTES - 1));
        let cancelled = AtomicBool::new(false);
        let (count, matches) =
            substring_matches_ascii(&text, b"aba", b'a', b'A', &[0, 0, 1], 1, Some(&cancelled))
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(matches[0].start as usize, CANCEL_CHECK_BYTES - 1);
        assert_eq!(matches[0].length, 3);
    }

    #[test]
    fn fuzzy_sort_prefers_contiguous_word_start_and_file_name() {
        let mut index = SearchIndex::new();
        index.upsert("folder/one.md", "# Title containsnote");
        index.upsert("folder/two.md", "# Note at title start");
        let results = index.quick_open("note", 50);
        assert_eq!(results[0].path, "folder/two.md");
        assert_eq!(results[1].path, "folder/one.md");

        let mut index = SearchIndex::new();
        index.upsert("folder/keep-note.md", "Unrelated title");
        index.upsert("note-folder/other.md", "Unrelated title");
        let results = index.quick_open("note", 50);
        assert_eq!(results[0].path, "folder/keep-note.md");
        assert_eq!(results[1].path, "note-folder/other.md");
    }

    #[test]
    fn substring_search_is_case_insensitive_and_reports_utf16() {
        let mut index = SearchIndex::new();
        index.upsert("中.md", "前😀中文 MIXed 中文后");
        let result = &index.search("mixed", 5)[0];
        assert_eq!(result.count, 1);
        let hit = &result.matches[0];
        assert_eq!(hit.start, "前😀中文 ".encode_utf16().count() as u32);
        assert_eq!(hit.length, 5);
    }

    #[test]
    fn snippets_respect_grapheme_boundaries() {
        let mut index = SearchIndex::new();
        let text = format!("{}👩‍🔬 目标 {}", "字".repeat(40), "字".repeat(40));
        index.upsert("emoji.md", text);
        let hit = &index.search("目标", 5)[0].matches[0];
        assert!(hit.snippet.starts_with("字"));
        assert!(hit.snippet.contains("👩‍🔬 目标"));
        assert!(
            UnicodeSegmentation::graphemes(hit.snippet.as_str(), true).any(|value| value == "👩‍🔬")
        );
        assert!(
            !UnicodeSegmentation::graphemes(hit.snippet.as_str(), true).any(|value| value == "👩‍")
        );
    }

    #[test]
    fn updates_replace_and_remove_notes() {
        let mut index = SearchIndex::new();
        index.upsert("note.md", "old");
        assert_eq!(index.search("old", 5).len(), 1);
        index.upsert("note.md", "new");
        assert!(index.search("old", 5).is_empty());
        assert_eq!(index.search("new", 5).len(), 1);
        index.remove("note.md");
        assert!(index.is_empty());
    }

    #[test]
    fn cancellation_stops_full_text_search() {
        let mut index = SearchIndex::new();
        for number in 0..100 {
            index.upsert(format!("{number}.md"), "needle");
        }
        let cancelled = AtomicBool::new(true);
        assert!(
            index
                .search_cancelled("needle", 5, Some(&cancelled))
                .is_none()
        );
    }

    #[test]
    fn ascii_fast_path_matches_unicode_edges_and_overlaps() {
        let mut index = SearchIndex::new();
        // Overlapping ASCII matches
        index.upsert("overlap.md", "AbAbA ababa");
        let overlap = &index.search("aba", 10)[0];
        assert_eq!(overlap.count, 4);
        assert_eq!(
            overlap
                .matches
                .iter()
                .map(|m| (m.start, m.length))
                .collect::<Vec<_>>(),
            vec![(0, 3), (2, 3), (6, 3), (8, 3)]
        );

        // Non-ASCII character that lowers to ASCII ('İ' -> 'i' + U+0307, 'K' -> 'k')
        index.upsert("turkish.md", "İstanbul and Kelvin");
        let i_hits = index.search("i", 10);
        assert_eq!(i_hits.len(), 1);
        assert_eq!(i_hits[0].path, "turkish.md");
        assert_eq!(i_hits[0].count, 2); // 'İ' and 'i' in Kelvin
        let k_hits = index.search("kelvin", 10);
        assert_eq!(k_hits.len(), 1);
        assert_eq!(k_hits[0].path, "turkish.md");
        assert_eq!(k_hits[0].count, 1);

        // Mixed CJK + ASCII note uses ASCII fast-path safely
        index.upsert("cjk.md", "中文前段 HortusCore 中间 hortuscore 尾部");
        let cjk_hits = index.search("hortuscore", 10);
        assert_eq!(cjk_hits.len(), 1);
        assert_eq!(cjk_hits[0].count, 2);
        assert_eq!(
            cjk_hits[0].matches[0].start,
            "中文前段 ".encode_utf16().count() as u32
        );
    }

    #[test]
    fn large_vault_3000_notes_concurrent_reads_and_fast_search() {
        use std::sync::{Arc, RwLock};
        let mut index = SearchIndex::new();
        for i in 0..3000 {
            let body = if i % 100 == 0 {
                format!(
                    "# Note {i}\n\n这是一个包含中英文混合内容的长笔记，用于测试大库检索性能。TargetKeyword_{} 出现在这里。\n{}",
                    i / 100,
                    "普通段落文本与架构设计记录，确保每篇笔记具备真实正文长度。".repeat(8)
                )
            } else {
                format!(
                    "# Note {i}\n\n{}",
                    "日常知识笔记内容，包含双链、流程图说明与复盘记录。".repeat(10)
                )
            };
            index.upsert(format!("folder-{}/note-{i:04}.md", i % 30), body);
        }
        let shared = Arc::new(RwLock::new(index));
        let start = std::time::Instant::now();
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let guard = shared.read().unwrap();
                    let full = guard.search("targetkeyword", 5);
                    assert_eq!(full.len(), 30);
                    let quick = guard.quick_open(&format!("note-{:04}", worker * 100), 20);
                    assert!(!quick.is_empty());
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let elapsed = start.elapsed();
        // Sanity bound: 4 concurrent 3,000-note searches should finish well under 5s even in debug mode.
        assert!(
            elapsed.as_secs() < 5,
            "concurrent 3000-note search too slow: {elapsed:?}"
        );
    }
}
