//! Read-only vault loading and JSON output for the knowledge queries.

use hortus_links::{KnowledgeIndex, LinkResolution, tag_key};
use hortus_search::SearchIndex;
use hortus_vault::note::Format;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct NoteFile {
    pub text: String,
    pub mmd: bool,
}

pub struct Vault {
    pub root: PathBuf,
    pub notes: BTreeMap<String, NoteFile>,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(unix)]
    identity: (u64, u64),
}

impl FileStamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        }
    }

    fn stable_at(&self, scan_started: SystemTime) -> bool {
        self.modified
            .and_then(|modified| scan_started.duration_since(modified).ok())
            .is_some_and(|age| age >= Duration::from_secs(2))
    }

    fn matches_file(&self, metadata: &Metadata) -> bool {
        #[cfg(unix)]
        if self.identity != (metadata.dev(), metadata.ino()) {
            return false;
        }
        metadata.is_file()
    }
}

/// 跨 MCP 请求复用已解码笔记与知识/搜索索引；元数据有变化或时间戳过近时重读。
pub struct CachedVault {
    vault: Vault,
    meta: BTreeMap<String, FileStamp>,
    knowledge: KnowledgeIndex,
    search: SearchIndex,
    #[cfg(unix)]
    root_identity: (u64, u64),
    #[cfg(unix)]
    _root_handle: fs::File,
    scan_started: Option<SystemTime>,
    load_count: u64,
    cache_hits: u64,
    reread_files: u64,
}

impl CachedVault {
    pub fn bind(explicit: Option<&Path>) -> Result<Self, String> {
        // 移除尾部斜杠和 `/.`，避免 symlink_metadata 在检查末级时跟随软链。
        let requested: PathBuf = requested_root(explicit)?.components().collect();
        let original = vault_directory_metadata(&requested)?;
        let root = requested
            .canonicalize()
            .map_err(|error| format!("unable to read vault root: {error}"))?;
        let bound = vault_directory_metadata(&root)?;
        #[cfg(unix)]
        let (root_identity, root_handle) = {
            let identity = (original.dev(), original.ino());
            // 保留打开的目录，避免删除后复用 inode 的新目录冒充原根。
            let handle = fs::File::open(&root)
                .map_err(|error| format!("unable to open vault root: {error}"))?;
            let opened = handle
                .metadata()
                .map_err(|error| format!("unable to read vault root: {error}"))?;
            if identity != (bound.dev(), bound.ino()) || identity != (opened.dev(), opened.ino()) {
                return Err("vault root changed while binding".into());
            }
            (identity, handle)
        };
        #[cfg(not(unix))]
        let _ = (original, bound);
        Ok(Self {
            vault: Vault {
                root,
                notes: BTreeMap::new(),
            },
            meta: BTreeMap::new(),
            knowledge: KnowledgeIndex::new(),
            search: SearchIndex::new(),
            #[cfg(unix)]
            root_identity,
            #[cfg(unix)]
            _root_handle: root_handle,
            scan_started: None,
            load_count: 0,
            cache_hits: 0,
            reread_files: 0,
        })
    }

    fn verify_root(&mut self) -> Result<(), String> {
        let checked = (|| {
            let metadata = vault_directory_metadata(&self.vault.root)?;
            #[cfg(unix)]
            if self.root_identity != (metadata.dev(), metadata.ino()) {
                return Err("vault root directory identity changed".into());
            }
            #[cfg(not(unix))]
            let _ = metadata;
            if self.vault.root.canonicalize().as_ref().ok() != Some(&self.vault.root) {
                return Err("vault root canonical path changed".into());
            }
            Ok(())
        })();
        if checked.is_err() {
            self.vault.notes.clear();
            self.meta.clear();
            self.knowledge = KnowledgeIndex::new();
            self.search = SearchIndex::new();
            self.scan_started = None;
        }
        checked
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        self.verify_root()?;
        let started = SystemTime::now();
        let mut current = BTreeMap::new();
        let scan = scan_metadata(&self.vault.root, &self.vault.root, true, &mut current);
        self.verify_root()?;
        scan?;
        // 用上次扫描的时间判断稳定性：上次扫描时仍可能同刻度改写的文件，
        // 即使两次请求相隔很久，也需再读一次才能进入缓存。
        let previous_scan = self.scan_started;
        let stable =
            |stamp: &FileStamp| previous_scan.is_some_and(|previous| stamp.stable_at(previous));
        if self.scan_started.is_some() && current == self.meta && current.values().all(stable) {
            self.cache_hits += 1;
            return Ok(());
        }
        let removed: Vec<String> = self
            .meta
            .keys()
            .filter(|path| !current.contains_key(*path))
            .cloned()
            .collect();
        for path in removed {
            self.vault.notes.remove(&path);
            self.knowledge.remove(&path);
            self.search.remove(&path);
        }
        let mut next_meta = BTreeMap::new();
        for (relative, stamp) in current {
            let unchanged = stable(&stamp) && self.meta.get(&relative) == Some(&stamp);
            if unchanged {
                next_meta.insert(relative, stamp);
                continue;
            }
            let full_path = self.vault.root.join(&relative);
            let Ok(bytes) = read_stamped_note(&full_path, &stamp) else {
                self.vault.notes.remove(&relative);
                self.knowledge.remove(&relative);
                self.search.remove(&relative);
                continue;
            };
            let (_format, text, _valid) = Format::decode(&bytes);
            let mmd = hortus_links::is_mmd_path(&relative);
            if self
                .vault
                .notes
                .get(&relative)
                .is_none_or(|note| note.text != text)
            {
                self.knowledge.upsert(relative.clone(), text.clone());
                self.search.upsert(relative.clone(), text.clone());
                self.vault
                    .notes
                    .insert(relative.clone(), NoteFile { text, mmd });
            }
            next_meta.insert(relative, stamp);
            self.reread_files += 1;
        }
        self.verify_root()?;
        self.meta = next_meta;
        self.scan_started = Some(started);
        self.load_count += 1;
        Ok(())
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn knowledge(&self) -> &KnowledgeIndex {
        &self.knowledge
    }

    pub fn search(&self) -> &SearchIndex {
        &self.search
    }

    #[cfg(test)]
    pub fn load_count(&self) -> u64 {
        self.load_count
    }

    #[cfg(test)]
    pub fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    #[cfg(test)]
    pub fn reread_files(&self) -> u64 {
        self.reread_files
    }
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
        let mut exact = forms.iter().filter(|form| self.notes.contains_key(*form));
        if let Some(path) = exact.next()
            && exact.next().is_none()
        {
            return Some(path.clone());
        }
        let mut folded = self
            .notes
            .keys()
            .filter(|path| forms.iter().any(|form| path.eq_ignore_ascii_case(form)));
        let path = folded.next()?;
        folded.next().is_none().then(|| path.clone())
    }
}

/// 锁定库根：显式参数优先，其次 HORTUS_VAULT，最后当前目录；始终返回规范路径。
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let root = requested_root(explicit)?
        .canonicalize()
        .map_err(|error| format!("unable to read vault: {error}"))?;
    if !root.is_dir() {
        return Err(format!("vault is not a directory: {}", root.display()));
    }
    Ok(root)
}

fn requested_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    Ok(match explicit {
        Some(path) => path.to_path_buf(),
        None => match std::env::var_os("HORTUS_VAULT").filter(|value| !value.is_empty()) {
            Some(value) => PathBuf::from(value),
            None => std::env::current_dir()
                .map_err(|error| format!("unable to read current directory: {error}"))?,
        },
    })
}

fn vault_directory_metadata(root: &Path) -> Result<Metadata, String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("unable to read vault root: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("vault root must not be a symbolic link".into());
    }
    if !metadata.is_dir() {
        return Err(format!("vault root is not a directory: {}", root.display()));
    }
    Ok(metadata)
}

fn read_stamped_note(path: &Path, stamp: &FileStamp) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    if !stamp.matches_file(&file.metadata()?) {
        return Err(std::io::Error::other(
            "note identity changed during vault scan",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
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
                mmd: hortus_links::is_mmd_path(name),
            },
        );
    }
    Ok(())
}

fn scan_metadata(
    root: &Path,
    directory: &Path,
    is_root: bool,
    meta: &mut BTreeMap<String, FileStamp>,
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
            scan_metadata(root, &entry.path(), false, meta)?;
            continue;
        }
        if !file_type.is_file() || !has_note_extension(name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        meta.insert(relative, FileStamp::from_metadata(&metadata));
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
    name.rsplit_once('.')
        .filter(|_| has_note_extension(name))
        .map_or(name, |(stem, _)| stem)
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
    search_value_with_index(vault, &index, query, limit)
}

pub fn search_value_with_index(
    vault: &Vault,
    index: &SearchIndex,
    query: &str,
    limit: usize,
) -> Value {
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
    let index = knowledge(vault);
    backlinks_value_with_index(vault, &index, note)
}

pub fn backlinks_value_with_index(
    vault: &Vault,
    index: &KnowledgeIndex,
    note: &str,
) -> Result<Value, String> {
    let note = vault.resolve_note(note)?;
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
    let index = knowledge(vault);
    links_value_with_index(vault, &index, note)
}

pub fn links_value_with_index(
    vault: &Vault,
    index: &KnowledgeIndex,
    note: &str,
) -> Result<Value, String> {
    let note = vault.resolve_note(note)?;
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
    tags_value_with_index(vault, &index, tag)
}

pub fn tags_value_with_index(vault: &Vault, index: &KnowledgeIndex, tag: Option<&str>) -> Value {
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
    orphans_value_with_index(vault, &index)
}

pub fn orphans_value_with_index(vault: &Vault, index: &KnowledgeIndex) -> Value {
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
    broken_links_value_with_index(vault, &index)
}

pub fn broken_links_value_with_index(vault: &Vault, index: &KnowledgeIndex) -> Value {
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
