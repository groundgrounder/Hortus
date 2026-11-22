//! In-memory knowledge graph for a folder of Markdown notes.
//!
//! The crate only looks at text. Scanners and platform layers feed note
//! contents in, and every query returns a deterministically ordered result so
//! callers can serialize it or assert on it directly.
mod frontmatter;
mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedNote {
    pub tags: Vec<Tag>,
    pub aliases: Vec<String>,
    pub source: Option<String>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// Tag text as written, without the leading `#`.
    pub raw: String,
    /// ASCII-lowercase form used for lookup and grouping.
    pub key: String,
    /// UTF-8 byte range of `#tag` in the source note.
    pub start: usize,
    pub end: usize,
    /// 1-based source line.
    pub line: usize,
    pub frontmatter: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Target as written, without `#heading`, `|alias` or the `!` prefix.
    pub raw_target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub in_diagram: bool,
    /// Byte range of the whole `[[...]]` (including a leading `!`).
    pub start: usize,
    pub end: usize,
    /// Byte range of `raw_target` inside the note, for exact renames.
    pub target_range: Range<usize>,
    /// 1-based source line.
    pub line: usize,
    pub context: String,
}

impl Link {
    /// A link whose target names a non-note file (image, PDF, diagram, ...).
    pub fn is_attachment(&self) -> bool {
        is_attachment_target(&self.raw_target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkResolution {
    Note(String),
    Attachment,
    Ambiguous(Vec<String>),
    Broken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub in_diagram: bool,
    pub attachment: bool,
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub context: String,
    pub resolution: LinkResolution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    pub source: String,
    pub line: usize,
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub in_diagram: bool,
    pub context: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSummary {
    pub tag: String,
    pub raw: String,
    pub count: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokenLink {
    pub source: String,
    pub line: usize,
    pub target: String,
    pub reason: BrokenReason,
    pub candidates: Vec<String>,
    pub context: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokenReason {
    Broken,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCompletion {
    pub path: String,
    pub title: String,
    pub insert_target: String,
    pub matched_alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRewrite {
    pub source: String,
    pub line: usize,
    /// Byte range of the whole link in the source note.
    pub start: usize,
    pub end: usize,
    pub before: String,
    pub after: String,
}

/// Parses one note's frontmatter fields, inline tags and wiki links.
pub fn parse_note(text: &str) -> ParsedNote {
    parse::parse_note(text)
}

/// Normalizes a vault-relative note path. Returns `None` for empty paths,
/// paths that try to leave the vault (`..`), or paths with control characters.
pub fn normalize_note_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.trim().split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => {
                if other.chars().any(char::is_control) {
                    return None;
                }
                parts.push(other);
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Normalizes a tag for lookup: strips leading `#`, trims, folds ASCII case.
pub fn tag_key(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('#')
        .trim()
        .to_ascii_lowercase()
}

/// True when `tag` matches `candidate` exactly or as a hierarchical prefix.
pub fn tag_matches(candidate: &str, tag: &str) -> bool {
    candidate == tag
        || candidate
            .strip_prefix(tag)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// True when the target names a file with a non-note extension.
pub fn is_attachment_target(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    if dot == 0 || dot + 1 >= name.len() {
        return false;
    }
    !matches!(
        name[dot + 1..].to_ascii_lowercase().as_str(),
        "md" | "markdown" | "mmd"
    )
}

fn note_extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let dot = name.rfind('.')?;
    if dot + 1 >= name.len() {
        return None;
    }
    let extension = &name[dot + 1..];
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "mmd"
    )
    .then_some(extension)
}

fn stem_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match note_extension(path) {
        Some(extension) => &name[..name.len() - extension.len() - 1],
        None => name,
    }
}

fn without_note_extension(path: &str) -> &str {
    match note_extension(path) {
        Some(extension) => &path[..path.len() - extension.len() - 1],
        None => path,
    }
}

fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

struct NoteEntry {
    text: String,
    parsed: ParsedNote,
}

#[derive(Default)]
pub struct KnowledgeIndex {
    notes: BTreeMap<String, NoteEntry>,
    by_lower_path: BTreeMap<String, BTreeSet<String>>,
    by_stem: BTreeMap<String, BTreeSet<String>>,
    by_alias: BTreeMap<String, BTreeSet<String>>,
}

impl KnowledgeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn note(&self, path: &str) -> Option<&ParsedNote> {
        self.notes.get(path).map(|entry| &entry.parsed)
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.notes.get(path).map(|entry| entry.text.as_str())
    }

    pub fn upsert(&mut self, path: impl Into<String>, text: impl Into<String>) {
        let path = path.into();
        let path = normalize_note_path(&path).unwrap_or(path);
        self.remove(&path);
        let text = text.into();
        let parsed = parse_note(&text);
        self.by_lower_path
            .entry(path.to_lowercase())
            .or_default()
            .insert(path.clone());
        let stem = stem_of(&path).to_lowercase();
        self.by_stem.entry(stem).or_default().insert(path.clone());
        for alias in &parsed.aliases {
            let key = alias.trim().to_lowercase();
            if !key.is_empty() {
                self.by_alias.entry(key).or_default().insert(path.clone());
            }
        }
        self.notes.insert(path, NoteEntry { text, parsed });
    }

    pub fn remove(&mut self, path: &str) {
        let Some(entry) = self.notes.remove(path) else {
            return;
        };
        if let Some(paths) = self.by_lower_path.get_mut(&path.to_lowercase()) {
            paths.remove(path);
            if paths.is_empty() {
                self.by_lower_path.remove(&path.to_lowercase());
            }
        }
        let stem = stem_of(path).to_lowercase();
        if let Some(paths) = self.by_stem.get_mut(&stem) {
            paths.remove(path);
            if paths.is_empty() {
                self.by_stem.remove(&stem);
            }
        }
        for alias in &entry.parsed.aliases {
            let key = alias.trim().to_lowercase();
            if let Some(paths) = self.by_alias.get_mut(&key) {
                paths.remove(path);
                if paths.is_empty() {
                    self.by_alias.remove(&key);
                }
            }
        }
    }

    pub fn markup(&self, path: &str, text: &str) -> (Vec<ResolvedLink>, Vec<Tag>) {
        let parsed = parse::parse_occurrences(text);
        (
            parsed
                .links
                .iter()
                .map(|link| self.resolve_link(path, link))
                .collect(),
            parsed
                .tags
                .into_iter()
                .filter(|tag| !tag.frontmatter)
                .collect(),
        )
    }

    pub fn completions(&self, query: &str, limit: usize) -> Vec<LinkCompletion> {
        let query = query.trim().to_lowercase();
        let mut matches = Vec::new();
        for (path, note) in &self.notes {
            let title = stem_of(path);
            let folded_title = title.to_lowercase();
            let folded_path = path.to_lowercase();
            let name_rank = if folded_title == query {
                Some(0)
            } else if folded_title.starts_with(&query) {
                Some(2)
            } else if folded_path.starts_with(&query) {
                Some(4)
            } else if folded_title.contains(&query) || folded_path.contains(&query) {
                Some(5)
            } else {
                None
            };
            let alias = if query.is_empty() {
                None
            } else {
                note.parsed
                    .aliases
                    .iter()
                    .filter_map(|alias| {
                        let folded = alias.to_lowercase();
                        let rank = if folded == query {
                            1
                        } else if folded.starts_with(&query) {
                            3
                        } else if folded.contains(&query) {
                            6
                        } else {
                            return None;
                        };
                        Some((rank, alias))
                    })
                    .min()
            };
            let Some(rank) = name_rank
                .into_iter()
                .chain(alias.map(|(rank, _)| rank))
                .min()
            else {
                continue;
            };
            let short = self
                .by_stem
                .get(&folded_title)
                .is_some_and(|paths| paths.len() == 1)
                && !is_attachment_target(title)
                && matches!(self.resolve("", title), LinkResolution::Note(found) if found == *path);
            let full = without_note_extension(path);
            let insert_target = if short {
                title
            } else if !is_attachment_target(full)
                && matches!(self.resolve("", full), LinkResolution::Note(found) if found == *path)
            {
                full
            } else {
                path
            };
            matches.push((
                rank,
                folded_title,
                LinkCompletion {
                    path: path.clone(),
                    title: title.into(),
                    insert_target: insert_target.into(),
                    matched_alias: alias.map(|(_, alias)| alias.clone()),
                },
            ));
        }
        matches.sort_by(|a, b| (&a.0, &a.1, &a.2.path).cmp(&(&b.0, &b.1, &b.2.path)));
        matches
            .into_iter()
            .take(limit)
            .map(|(_, _, hit)| hit)
            .collect()
    }

    pub fn links(&self, path: &str) -> Option<Vec<ResolvedLink>> {
        let entry = self.notes.get(path)?;
        Some(
            entry
                .parsed
                .links
                .iter()
                .map(|link| self.resolve_link(path, link))
                .collect(),
        )
    }

    pub fn backlinks(&self, path: &str) -> Option<Vec<Backlink>> {
        self.notes.get(path)?;
        let mut backlinks = Vec::new();
        for (source, entry) in &self.notes {
            for link in &entry.parsed.links {
                if link.is_attachment() {
                    continue;
                }
                if matches!(self.resolve(source, &link.raw_target), LinkResolution::Note(target) if target == path)
                {
                    backlinks.push(Backlink {
                        source: source.clone(),
                        line: link.line,
                        target: link.raw_target.clone(),
                        heading: link.heading.clone(),
                        alias: link.alias.clone(),
                        embed: link.embed,
                        in_diagram: link.in_diagram,
                        context: link.context.clone(),
                        start: link.start,
                        end: link.end,
                    });
                }
            }
        }
        Some(backlinks)
    }

    pub fn tags(&self) -> Vec<TagSummary> {
        let mut summary: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
        for (path, entry) in &self.notes {
            for tag in &entry.parsed.tags {
                let value = summary
                    .entry(tag.key.clone())
                    .or_insert_with(|| (tag.raw.clone(), BTreeSet::new()));
                value.1.insert(path.clone());
            }
        }
        summary
            .into_iter()
            .map(|(tag, (raw, notes))| TagSummary {
                tag,
                raw,
                count: notes.len(),
                notes: notes.into_iter().collect(),
            })
            .collect()
    }

    pub fn notes_with_tag(&self, tag: &str) -> Vec<String> {
        let key = tag_key(tag);
        if key.is_empty() {
            return Vec::new();
        }
        self.notes
            .iter()
            .filter(|(_, entry)| {
                entry
                    .parsed
                    .tags
                    .iter()
                    .any(|tag| tag_matches(&tag.key, &key))
            })
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn orphans(&self) -> Vec<String> {
        let mut linked = BTreeSet::new();
        for (source, entry) in &self.notes {
            for link in &entry.parsed.links {
                if let LinkResolution::Note(target) = self.resolve(source, &link.raw_target) {
                    linked.insert(source.clone());
                    linked.insert(target);
                }
            }
        }
        self.notes
            .keys()
            .filter(|path| !linked.contains(*path))
            .cloned()
            .collect()
    }

    pub fn broken_links(&self) -> Vec<BrokenLink> {
        let mut broken = Vec::new();
        for (source, entry) in &self.notes {
            for link in &entry.parsed.links {
                let (reason, candidates) = match self.resolve(source, &link.raw_target) {
                    LinkResolution::Broken => (BrokenReason::Broken, Vec::new()),
                    LinkResolution::Ambiguous(candidates) => (BrokenReason::Ambiguous, candidates),
                    _ => continue,
                };
                broken.push(BrokenLink {
                    source: source.clone(),
                    line: link.line,
                    target: link.raw_target.clone(),
                    reason,
                    candidates,
                    context: link.context.clone(),
                    start: link.start,
                    end: link.end,
                });
            }
        }
        broken
    }

    /// Plans link rewrites for moving `old_path` to `new_path`.
    ///
    /// Links that resolve through an alias without naming the note keep
    /// working and are left alone. Heading anchors, aliases and the `![[`
    /// prefix are preserved, and `before` is the exact source text.
    pub fn plan_rename_links(&self, old_path: &str, new_path: &str) -> Vec<LinkRewrite> {
        let Some(old) = normalize_note_path(old_path) else {
            return Vec::new();
        };
        let Some(new) = normalize_note_path(new_path) else {
            return Vec::new();
        };
        if old == new {
            return Vec::new();
        }
        let Some(old_entry) = self.notes.get(&old) else {
            return Vec::new();
        };
        let mut old_names = BTreeSet::new();
        old_names.insert(old.to_lowercase());
        old_names.insert(without_note_extension(&old).to_lowercase());
        old_names.insert(stem_of(&old).to_lowercase());
        let old_aliases = old_entry
            .parsed
            .aliases
            .iter()
            .map(|alias| alias.trim().to_lowercase())
            .collect::<BTreeSet<_>>();

        let new_without_extension = without_note_extension(&new);
        let new_stem = stem_of(&new);
        let mut rewrites = Vec::new();
        for (source, entry) in &self.notes {
            for link in &entry.parsed.links {
                if link.is_attachment() {
                    continue;
                }
                if !matches!(self.resolve(source, &link.raw_target), LinkResolution::Note(target) if target == old)
                {
                    continue;
                }
                let Some(normalized_target) = normalize_note_path(&link.raw_target) else {
                    continue;
                };
                let key = normalized_target.to_lowercase();
                if !old_names.contains(&key) && old_aliases.contains(&key) {
                    continue;
                }
                let mut target = if note_extension(&link.raw_target).is_some() {
                    new.clone()
                } else if link.raw_target.contains('/') {
                    new_without_extension.to_owned()
                } else {
                    let collisions = self
                        .by_stem
                        .get(&new_stem.to_lowercase())
                        .is_some_and(|paths| paths.iter().any(|path| path != &old));
                    if collisions {
                        new_without_extension.to_owned()
                    } else {
                        new_stem.to_owned()
                    }
                };
                if is_attachment_target(&target)
                    || self.notes.keys().any(|path| {
                        path != &old
                            && without_note_extension(path)
                                .eq_ignore_ascii_case(new_without_extension)
                    })
                {
                    target = new.clone();
                }
                if link.raw_target.starts_with('/') {
                    target.insert(0, '/');
                }
                if target == link.raw_target {
                    continue;
                }
                let before = &entry.text[link.start..link.end];
                let mut after = before.to_owned();
                after.replace_range(
                    link.target_range.start - link.start..link.target_range.end - link.start,
                    &target,
                );
                rewrites.push(LinkRewrite {
                    source: source.clone(),
                    line: link.line,
                    start: link.start,
                    end: link.end,
                    before: before.into(),
                    after,
                });
            }
        }
        rewrites
    }

    pub fn resolve(&self, source: &str, target: &str) -> LinkResolution {
        if is_attachment_target(target) {
            return LinkResolution::Attachment;
        }
        let target = target.trim();
        if target.is_empty() {
            return LinkResolution::Note(source.to_owned());
        }
        let Some(normalized) = normalize_note_path(target) else {
            return LinkResolution::Broken;
        };
        let mut forms = vec![normalized.clone()];
        if note_extension(&normalized).is_none() {
            forms.push(format!("{normalized}.md"));
            forms.push(format!("{normalized}.markdown"));
            forms.push(format!("{normalized}.mmd"));
        }
        let mut exact = BTreeSet::new();
        for form in &forms {
            if let Some(paths) = self.by_lower_path.get(&form.to_lowercase()) {
                exact.extend(paths.iter().cloned());
            }
        }
        match exact.len() {
            1 => {
                return LinkResolution::Note(exact.into_iter().next().unwrap_or_default());
            }
            0 => {}
            _ => return LinkResolution::Ambiguous(exact.into_iter().collect()),
        }

        let stem = stem_of(&normalized).to_lowercase();
        if let Some(paths) = self.by_stem.get(&stem) {
            if paths.len() == 1 {
                return LinkResolution::Note(paths.iter().next().cloned().unwrap_or_default());
            }
            let source_directory = dirname(source).to_lowercase();
            let same_directory = paths
                .iter()
                .filter(|path| dirname(path).to_lowercase() == source_directory)
                .cloned()
                .collect::<Vec<_>>();
            if same_directory.len() == 1 {
                return LinkResolution::Note(same_directory.into_iter().next().unwrap_or_default());
            }
            return LinkResolution::Ambiguous(paths.iter().cloned().collect());
        }

        if let Some(paths) = self.by_alias.get(&normalized.to_lowercase()) {
            if paths.len() == 1 {
                return LinkResolution::Note(paths.iter().next().cloned().unwrap_or_default());
            }
            return LinkResolution::Ambiguous(paths.iter().cloned().collect());
        }

        LinkResolution::Broken
    }

    fn resolve_link(&self, source: &str, link: &Link) -> ResolvedLink {
        ResolvedLink {
            target: link.raw_target.clone(),
            heading: link.heading.clone(),
            alias: link.alias.clone(),
            embed: link.embed,
            in_diagram: link.in_diagram,
            attachment: link.is_attachment(),
            line: link.line,
            start: link.start,
            end: link.end,
            context: link.context.clone(),
            resolution: self.resolve(source, &link.raw_target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(entries: &[(&str, &str)]) -> KnowledgeIndex {
        let mut index = KnowledgeIndex::new();
        for (path, text) in entries {
            index.upsert(*path, *text);
        }
        index
    }

    #[test]
    fn resolves_exact_paths_stems_aliases_and_attachments() {
        let index = index(&[
            ("guides/intro.md", ""),
            ("notes/Plan.md", "---\naliases: [Roadmap]\n---\n"),
            ("a/Duplicate.md", ""),
            ("b/Duplicate.md", ""),
            (
                "a/Source.md",
                "[[Duplicate]] [[Roadmap]] [[intro]] [[Plan]] [[pic.png]] [[Missing]]",
            ),
        ]);
        let links = index.links("a/Source.md").unwrap();
        assert_eq!(
            links
                .iter()
                .map(|link| link.resolution.clone())
                .collect::<Vec<_>>(),
            vec![
                LinkResolution::Note("a/Duplicate.md".into()),
                LinkResolution::Note("notes/Plan.md".into()),
                LinkResolution::Note("guides/intro.md".into()),
                LinkResolution::Note("notes/Plan.md".into()),
                LinkResolution::Attachment,
                LinkResolution::Broken,
            ]
        );
    }

    #[test]
    fn ambiguous_links_list_sorted_candidates() {
        let index = index(&[
            ("a/Same.md", ""),
            ("b/Same.md", ""),
            ("c/Source.md", "[[Same]] [[Unknown]]"),
        ]);
        let links = index.links("c/Source.md").unwrap();
        assert_eq!(
            links[0].resolution,
            LinkResolution::Ambiguous(vec!["a/Same.md".into(), "b/Same.md".into()])
        );
        assert_eq!(links[1].resolution, LinkResolution::Broken);
        let broken = index.broken_links();
        assert_eq!(broken.len(), 2);
        assert_eq!(broken[0].reason, BrokenReason::Ambiguous);
        assert_eq!(broken[0].candidates.len(), 2);
        assert_eq!(broken[1].reason, BrokenReason::Broken);
    }

    #[test]
    fn backlinks_include_embeds_diagrams_and_self_references() {
        let index = index(&[
            (
                "target.md",
                "---\ntags: [base, base/child]\n---\n[[target]]\n",
            ),
            (
                "source.md",
                "[[target|Alias]]\n```mermaid\nflowchart TB\nA[\"[[target]]\"] --> B\n```\n",
            ),
        ]);
        let backlinks = index.backlinks("target.md").unwrap();
        assert_eq!(backlinks.len(), 3);
        assert_eq!(backlinks[0].source, "source.md");
        assert_eq!(backlinks[0].alias.as_deref(), Some("Alias"));
        assert!(!backlinks[0].in_diagram);
        assert!(backlinks[1].in_diagram);
        assert_eq!(backlinks[2].source, "target.md");
        assert!(index.backlinks("missing.md").is_none());
    }

    #[test]
    fn tags_aggregate_and_match_hierarchies() {
        let index = index(&[
            (
                "one.md",
                "---\ntags: [#base, base/child]\n---\n#inline #base/child\n",
            ),
            ("two.md", "#base #base/other\n"),
        ]);
        let tags = index.tags();
        assert_eq!(tags[0].tag, "base");
        assert_eq!(tags[0].count, 2);
        assert_eq!(tags[0].notes, vec!["one.md", "two.md"]);
        assert_eq!(
            index.notes_with_tag("base"),
            vec!["one.md".to_owned(), "two.md".to_owned()]
        );
        assert_eq!(index.notes_with_tag("#base/child"), vec!["one.md"]);
        assert!(index.notes_with_tag("").is_empty());
    }

    #[test]
    fn orphans_need_a_resolved_link_in_either_direction() {
        let index = index(&[
            ("a.md", "[[b]] [[missing]]"),
            ("b.md", "text"),
            ("c.md", "[[missing]]"),
            ("d.md", "text"),
        ]);
        assert_eq!(index.orphans(), vec!["c.md", "d.md"]);
    }

    #[test]
    fn rename_plan_preserves_link_shape_and_skips_aliases() {
        let index = index(&[
            ("notes/old.md", "---\naliases: [Legacy]\n---\n# Old\n"),
            (
                "source.md",
                "[[old]]\n![[old#Part|Alias]]\n[[notes/old]]\n[[Legacy]]\n[[pic.png]]\n",
            ),
        ]);
        let plan = index.plan_rename_links("notes/old.md", "notes/new.md");
        assert_eq!(plan.len(), 3);
        assert_eq!(&plan[0].before, "[[old]]");
        assert_eq!(&plan[0].after, "[[new]]");
        assert_eq!(&plan[1].before, "![[old#Part|Alias]]");
        assert_eq!(&plan[1].after, "![[new#Part|Alias]]");
        assert_eq!(&plan[2].before, "[[notes/old]]");
        assert_eq!(&plan[2].after, "[[notes/new]]");
    }

    #[test]
    fn rename_plan_keeps_extensions_and_utf8_targets() {
        let index = index(&[
            ("笔记/主题.md", ""),
            (
                "source.md",
                "[[主题]] [[主题#章节|别名]] [[笔记/主题.md]]\n",
            ),
        ]);
        let plan = index.plan_rename_links("笔记/主题.md", "笔记/新主题.md");
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].after, "[[新主题]]");
        assert_eq!(plan[1].after, "[[新主题#章节|别名]]");
        assert_eq!(plan[2].after, "[[笔记/新主题.md]]");
    }

    #[test]
    fn removing_a_note_clears_all_lookup_tables() {
        let mut index = index(&[
            ("dir/One.md", "---\naliases: [Shared]\n---\n"),
            ("Other.md", "[[Shared]] [[One]]"),
        ]);
        index.remove("dir/One.md");
        assert_eq!(index.len(), 1);
        let links = index.links("Other.md").unwrap();
        assert!(
            links
                .iter()
                .all(|link| link.resolution == LinkResolution::Broken)
        );
        assert_eq!(index.broken_links().len(), 2);
    }
}
