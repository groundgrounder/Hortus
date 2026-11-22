//! Read-only vault loading and JSON output for the knowledge queries.

use hortus_links::{KnowledgeIndex, LinkResolution, tag_key};
use hortus_search::SearchIndex;
use hortus_vault::note::Format;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub struct NoteFile {
    pub text: String,
    pub mmd: bool,
}

pub struct Vault {
    pub root: PathBuf,
    pub notes: BTreeMap<String, NoteFile>,
}

impl Vault {
    /// Resolves the vault root, scans notes and decodes them exactly like the
    /// editor. Nothing is ever written to the vault.
    pub fn load(explicit: Option<&Path>) -> Result<Self, String> {
        let root = resolve_root(explicit)?;
        let mut notes = BTreeMap::new();
        scan(&root, &root, true, &mut notes)?;
        Ok(Self { root, notes })
    }

    pub fn resolve_note(&self, query: &str) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("note path is empty".into());
        }
        let path = Path::new(query);
        if path.is_absolute() {
            let canonical =
                std::fs::canonicalize(path).map_err(|_| format!("note not found: {query}"))?;
            if path != canonical.as_path() {
                return Err(format!("symbolic links are not supported: {query}"));
            }
            let relative = canonical
                .strip_prefix(&self.root)
                .map_err(|_| format!("note is outside the vault: {query}"))?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            return self
                .match_exact(&relative)
                .ok_or_else(|| format!("note not found: {query}"));
        }
        if path
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(format!("note path escapes the vault: {query}"));
        }
        let Some(normalized) = hortus_links::normalize_note_path(query) else {
            return Err(format!("invalid note path: {query}"));
        };
        if has_symlink_component(&self.root, Path::new(&normalized)) {
            return Err(format!("symbolic links are not supported: {query}"));
        }
        if let Some(note) = self.match_exact(&normalized) {
            return Ok(note);
        }
        let query_name = normalized.rsplit('/').next().unwrap_or(&normalized);
        let query_has_extension = has_note_extension(query_name);
        let query_stem = stem(query_name);
        let mut matches = self
            .notes
            .keys()
            .filter(|path| {
                let name = path.rsplit('/').next().unwrap_or(path);
                if query_has_extension {
                    name.eq_ignore_ascii_case(query_name)
                } else {
                    stem(name).eq_ignore_ascii_case(query_stem)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Err(format!("note not found: {query}")),
            1 => Ok(matches.remove(0)),
            _ => Err(format!("ambiguous note: {query}")),
        }
    }

    fn match_exact(&self, query: &str) -> Option<String> {
        let mut forms = vec![query.to_owned()];
        if !has_note_extension(query) {
            forms.push(format!("{query}.md"));
            forms.push(format!("{query}.markdown"));
            forms.push(format!("{query}.mmd"));
        }
        let mut matches = self
            .notes
            .keys()
            .filter(|path| forms.iter().any(|form| path.eq_ignore_ascii_case(form)))
            .cloned()
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }
}

/// 锁定库根：显式参数优先，其次 HORTUS_VAULT，最后当前目录；始终返回规范路径。
pub fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let root = match explicit {
        Some(path) => path.to_path_buf(),
        None => match std::env::var_os("HORTUS_VAULT").filter(|value| !value.is_empty()) {
            Some(value) => PathBuf::from(value),
            None => std::env::current_dir()
                .map_err(|error| format!("unable to read current directory: {error}"))?,
        },
    };
    let root = root
        .canonicalize()
        .map_err(|error| format!("unable to read vault: {error}"))?;
    if !root.is_dir() {
        return Err(format!("vault is not a directory: {}", root.display()));
    }
    Ok(root)
}

fn scan(
    root: &Path,
    directory: &Path,
    is_root: bool,
    notes: &mut BTreeMap<String, NoteFile>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if is_root => return Err(format!("unable to read vault: {error}")),
        Err(_) => return Ok(()),
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if name == "node_modules" {
                continue;
            }
            scan(root, &entry.path(), false, notes)?;
            continue;
        }
        if !file_type.is_file() || !has_note_extension(name) {
            continue;
        }
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let (_format, text, _valid) = Format::decode(&bytes);
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        notes.insert(
            relative,
            NoteFile {
                text,
                mmd: name.to_ascii_lowercase().ends_with(".mmd"),
            },
        );
    }
    Ok(())
}

fn has_note_extension(name: &str) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "mmd"
    )
}

fn stem(name: &str) -> &str {
    for extension in [".md", ".markdown", ".mmd"] {
        if name.len() >= extension.len()
            && name[name.len() - extension.len()..].eq_ignore_ascii_case(extension)
        {
            return &name[..name.len() - extension.len()];
        }
    }
    name
}

fn has_symlink_component(root: &Path, relative: &Path) -> bool {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return true;
        }
    }
    false
}

fn knowledge(vault: &Vault) -> KnowledgeIndex {
    let mut index = KnowledgeIndex::new();
    for (path, note) in &vault.notes {
        index.upsert(path.clone(), note.text.clone());
    }
    index
}

fn vault_path(vault: &Vault) -> String {
    vault.root.display().to_string()
}

fn emit(value: &Value) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)
        .map_err(|error| format!("unable to write JSON: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("unable to write JSON: {error}"))
}

fn resolution_json(resolution: &LinkResolution) -> Value {
    match resolution {
        LinkResolution::Note(path) => json!({"kind": "note", "path": path}),
        LinkResolution::Attachment => json!({"kind": "attachment"}),
        LinkResolution::Ambiguous(candidates) => {
            json!({"kind": "ambiguous", "candidates": candidates})
        }
        LinkResolution::Broken => json!({"kind": "broken"}),
    }
}

pub fn search(explicit: Option<&Path>, query: &str, limit: usize) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&search_value(&vault, query, limit))
}

pub fn search_value(vault: &Vault, query: &str, limit: usize) -> Value {
    let mut index = SearchIndex::new();
    for (path, note) in &vault.notes {
        index.upsert(path.clone(), note.text.clone());
    }
    let total_notes = index.len();
    let results = index
        .search(query, 50)
        .into_iter()
        .take(limit)
        .map(|result| {
            json!({
                "path": result.path,
                "title": result.title,
                "count": result.count,
                "matches": result.matches.into_iter().map(|item| json!({
                    "start": item.start,
                    "length": item.length,
                    "snippet": item.snippet,
                    "snippet_start": item.snippet_start,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "vault": vault_path(vault),
        "query": query,
        "total_notes": total_notes,
        "results": results,
    })
}

pub fn backlinks(explicit: Option<&Path>, note: &str) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&backlinks_value(&vault, note)?)
}

pub fn backlinks_value(vault: &Vault, note: &str) -> Result<Value, String> {
    let note = vault.resolve_note(note)?;
    let index = knowledge(vault);
    let backlinks = index
        .backlinks(&note)
        .ok_or_else(|| format!("note not found: {note}"))?;
    let items = backlinks
        .into_iter()
        .map(|backlink| {
            json!({
                "source": backlink.source,
                "line": backlink.line,
                "target": backlink.target,
                "heading": backlink.heading,
                "alias": backlink.alias,
                "embed": backlink.embed,
                "in_diagram": backlink.in_diagram,
                "context": backlink.context,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "vault": vault_path(vault),
        "note": note,
        "count": items.len(),
        "backlinks": items,
    }))
}

pub fn links(explicit: Option<&Path>, note: &str) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&links_value(&vault, note)?)
}

pub fn links_value(vault: &Vault, note: &str) -> Result<Value, String> {
    let note = vault.resolve_note(note)?;
    let index = knowledge(vault);
    let links = index
        .links(&note)
        .ok_or_else(|| format!("note not found: {note}"))?;
    let items = links
        .into_iter()
        .map(|link| {
            json!({
                "target": link.target,
                "heading": link.heading,
                "alias": link.alias,
                "embed": link.embed,
                "in_diagram": link.in_diagram,
                "attachment": link.attachment,
                "line": link.line,
                "context": link.context,
                "resolution": resolution_json(&link.resolution),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "vault": vault_path(vault),
        "note": note,
        "count": items.len(),
        "links": items,
    }))
}

pub fn tags(explicit: Option<&Path>, tag: Option<&str>) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&tags_value(&vault, tag))
}

pub fn tags_value(vault: &Vault, tag: Option<&str>) -> Value {
    let index = knowledge(vault);
    if let Some(tag) = tag {
        let notes = index.notes_with_tag(tag);
        return json!({
            "vault": vault_path(vault),
            "tag": tag_key(tag),
            "count": notes.len(),
            "notes": notes,
        });
    }
    let tags = index.tags();
    let count = tags.len();
    let items = tags
        .into_iter()
        .map(|tag| {
            json!({
                "tag": tag.tag,
                "count": tag.count,
                "notes": tag.notes,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "vault": vault_path(vault),
        "count": count,
        "tags": items,
    })
}

pub fn outline(explicit: Option<&Path>, note: &str) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&outline_value(&vault, note)?)
}

pub fn outline_value(vault: &Vault, note: &str) -> Result<Value, String> {
    let note = vault.resolve_note(note)?;
    let Some(file) = vault.notes.get(&note) else {
        return Err(format!("note not found: {note}"));
    };
    let mut document = hortus_document::Document::new(&file.text);
    let headings = document
        .outline(file.mmd)
        .map_err(|error| format!("unable to read outline: {error}"))?;
    let headings = headings
        .iter()
        .map(|heading| {
            json!({
                "level": heading.level,
                "text": heading.title,
                "start": heading.start,
                "length": heading_length(&file.text, heading.start),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "vault": vault_path(vault),
        "note": note,
        "headings": headings,
    }))
}

fn heading_length(text: &str, start: usize) -> usize {
    let Some(byte) = utf16_to_byte(text, start) else {
        return 0;
    };
    let end = text[byte..]
        .find('\n')
        .map_or(text.len(), |offset| byte + offset);
    text[byte..end].encode_utf16().count()
}

fn utf16_to_byte(text: &str, units: usize) -> Option<usize> {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen >= units {
            return Some(index);
        }
        seen += character.len_utf16();
    }
    (seen >= units).then_some(text.len())
}

pub fn orphans(explicit: Option<&Path>) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&orphans_value(&vault))
}

pub fn orphans_value(vault: &Vault) -> Value {
    let index = knowledge(vault);
    let orphans = index.orphans();
    json!({
        "vault": vault_path(vault),
        "count": orphans.len(),
        "orphans": orphans,
    })
}

pub fn broken_links(explicit: Option<&Path>) -> Result<(), String> {
    let vault = Vault::load(explicit)?;
    emit(&broken_links_value(&vault))
}

pub fn broken_links_value(vault: &Vault) -> Value {
    let index = knowledge(vault);
    let broken = index.broken_links();
    let items = broken
        .into_iter()
        .map(|link| {
            json!({
                "source": link.source,
                "line": link.line,
                "target": link.target,
                "reason": match link.reason {
                    hortus_links::BrokenReason::Broken => "broken",
                    hortus_links::BrokenReason::Ambiguous => "ambiguous",
                },
                "candidates": link.candidates,
                "context": link.context,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "vault": vault_path(vault),
        "count": items.len(),
        "broken_links": items,
    })
}
