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
}

#[derive(Clone, Debug)]
struct Folded {
    text: String,
    /// Each folded character maps back to the UTF-8 byte range of its source char.
    map: Vec<(usize, usize)>,
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
        self.notes.insert(path, Note { title, text });
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

        let mut results = self
            .notes
            .iter()
            .filter_map(|(path, note)| {
                let title = fuzzy_match(&note.title, query);
                let path_match = fuzzy_match(path, query);
                let file_name = Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(path);
                let file_name_match = fuzzy_match(file_name, query);
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
        if query.is_empty() {
            return Some(vec![]);
        }
        let mut results = Vec::new();
        for (path, note) in &self.notes {
            if cancelled.is_some_and(|token| token.load(AtomicOrdering::Acquire)) {
                return None;
            }
            let (count, matches) =
                substring_matches(&note.text, query, max_matches_per_note, cancelled)?;
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

fn folded(input: &str) -> Folded {
    let mut text = String::new();
    let mut map = Vec::new();
    for (start, character) in input.char_indices() {
        let end = start + character.len_utf8();
        for lowered in character.to_lowercase() {
            text.push(lowered);
            map.push((start, end));
        }
    }
    Folded { text, map }
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

fn substring_matches(
    text: &str,
    query: &str,
    limit: usize,
    cancelled: Option<&AtomicBool>,
) -> Option<(u32, Vec<TextMatch>)> {
    let pattern = folded(query).text.chars().collect::<Vec<_>>();
    if pattern.is_empty() {
        return Some((0, vec![]));
    }
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
    let mut source_ranges = std::collections::VecDeque::with_capacity(pattern.len());
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
    if count == 0 {
        return Some((0, vec![]));
    }
    let graphemes: Vec<usize> = UnicodeSegmentation::grapheme_indices(text, true)
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect();
    let matches = spans
        .into_iter()
        .enumerate()
        .map(|(index, span)| {
            if index % 16 == 0 && cancelled.is_some_and(|token| token.load(AtomicOrdering::Acquire))
            {
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
            Some(TextMatch {
                start: utf16_offset(text, span.start) as u32,
                length: (utf16_offset(text, span.end) - utf16_offset(text, span.start)) as u32,
                snippet: text[snippet_start..snippet_end].to_owned(),
                snippet_start: utf16_offset(text, snippet_start) as u32,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((count, matches))
}

fn fuzzy_match(text: &str, query: &str) -> Option<FuzzyMatch> {
    let folded_text = folded(text);
    let text_chars: Vec<char> = folded_text.text.chars().collect();
    let query_chars: Vec<char> = folded(query).text.chars().collect();
    if query_chars.is_empty() {
        return None;
    }
    let mut found = Vec::with_capacity(query_chars.len());
    let mut cursor = 0;
    for character in query_chars {
        let index = text_chars
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
}
