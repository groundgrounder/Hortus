#![cfg(target_os = "macos")]

//! 会话改动审阅：全库扫描、重命名配对、逐行 diff、二进制/符号链接元数据
//! 与损坏会话拒绝。
use hortus_history::review::{ChangeKind, MAX_TEXT_BYTES, ReviewEntry, ReviewSummary};
use hortus_history::snapshot::ManifestEntry;
use hortus_history::{Cancellation, HistoryStore, SessionRecord, SessionState, SnapshotStats};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

fn vault_fixture() -> (tempfile::TempDir, PathBuf, HistoryStore) {
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir_all(vault.join("folder")).unwrap();
    fs::write(vault.join("folder/inside.md"), b"folder content\n").unwrap();
    fs::create_dir_all(vault.join("nested/.git")).unwrap();
    fs::create_dir_all(vault.join(".git/refs")).unwrap();
    fs::create_dir_all(vault.join("empty-dir")).unwrap();
    fs::create_dir_all(root.path().join("outside")).unwrap();
    fs::write(vault.join("keep.md"), b"# Keep\n\nNo change.\n").unwrap();
    fs::write(vault.join("edit.md"), b"alpha\nbeta\ngamma\n").unwrap();
    fs::write(
        vault.join("rename.md"),
        b"rename stable line\nsecond line\nthird line\n",
    )
    .unwrap();
    fs::write(
        vault.join("rename-edit.md"),
        b"one\ntwo\nthree\nfour\nfive\nsix\n",
    )
    .unwrap();
    fs::write(vault.join("delete.md"), b"delete this\n").unwrap();
    fs::write(vault.join("mode.md"), b"mode stays\n").unwrap();
    fs::write(vault.join("type.md"), b"type changes\n").unwrap();
    fs::write(vault.join(".hidden.md"), b"hidden\n").unwrap();
    fs::write(vault.join("empty.bin"), b"").unwrap();
    fs::write(vault.join("crlf.md"), b"line one\r\nline two\r\n").unwrap();
    fs::write(vault.join("no-final.md"), b"first\nsecond").unwrap();
    fs::write(vault.join(".git/config"), b"[core]\n").unwrap();
    fs::write(vault.join("nested/.git/HEAD"), b"ref\n").unwrap();
    let binary: Vec<u8> = (0..=255u8).collect();
    fs::write(vault.join("attachment.png"), &binary).unwrap();
    fs::write(root.path().join("outside/changed.txt"), b"outside\n").unwrap();
    symlink("edit.md", vault.join("link-edit")).unwrap();
    symlink("keep.md", vault.join("link-drop")).unwrap();
    symlink(root.path().join("outside"), vault.join("outside-dir")).unwrap();
    fs::write(root.path().join("outside/tracked.txt"), b"outside\n").unwrap();
    let store = HistoryStore::new(root.path().join("data"));
    (root, vault, store)
}

fn mutate(vault: &Path, root: &Path) {
    // 同秒、同字节长度的修改：不能依赖时间戳。
    let edit = vault.join("edit.md");
    let metadata = fs::metadata(&edit).unwrap();
    let times = fs::FileTimes::new()
        .set_accessed(UNIX_EPOCH + Duration::from_secs(metadata.atime() as u64))
        .set_modified(UNIX_EPOCH + Duration::from_secs(metadata.mtime() as u64));
    fs::write(&edit, b"alpha\nBETA\ngamma\n").unwrap();
    fs::File::open(&edit).unwrap().set_times(times).unwrap();

    fs::create_dir_all(vault.join("Added")).unwrap();
    fs::write(vault.join("Added/New note.md"), b"# New\n\nAdded text.\n").unwrap();
    fs::write(vault.join("added-zero.bin"), b"").unwrap();
    fs::write(vault.join(".hidden-added"), b"hidden add\n").unwrap();
    fs::create_dir_all(vault.join("empty-added-dir")).unwrap();

    fs::create_dir_all(vault.join("Renamed")).unwrap();
    fs::rename(vault.join("rename.md"), vault.join("Renamed/Renamed.md")).unwrap();
    fs::rename(
        vault.join("rename-edit.md"),
        vault.join("renamed-with-edit.md"),
    )
    .unwrap();
    fs::write(
        vault.join("renamed-with-edit.md"),
        b"one\ntwo\nthree\nfour\nFIVE\nsix\n",
    )
    .unwrap();
    fs::remove_file(vault.join("delete.md")).unwrap();
    fs::remove_dir_all(vault.join("folder")).unwrap();

    fs::set_permissions(vault.join("mode.md"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(vault.join("type.md")).unwrap();
    fs::create_dir_all(vault.join("type.md")).unwrap();
    fs::write(vault.join("type.md/inside.md"), b"inside\n").unwrap();

    let mut changed: Vec<u8> = (1..=255u8).chain([0u8]).collect();
    fs::write(vault.join("attachment.png"), &changed).unwrap();
    changed.rotate_left(7);

    symlink("keep.md", vault.join("link-new")).unwrap();
    fs::remove_file(vault.join("link-edit")).unwrap();
    symlink("keep.md", vault.join("link-edit")).unwrap();
    fs::remove_file(vault.join("link-drop")).unwrap();

    let _ = fs::read_link(vault.join("outside-dir")).unwrap();

    // .git 内外的变化都不能进入审阅。
    fs::write(vault.join(".git/config"), b"[core]\nchanged\n").unwrap();
    fs::write(vault.join(".git/new"), b"ignored\n").unwrap();
    fs::write(root.join("outside/tracked.txt"), b"outside changed\n").unwrap();
}

fn write_session(
    store: &HistoryStore,
    id: &str,
    state: SessionState,
    stats: Option<SnapshotStats>,
    manifest: Option<&[ManifestEntry]>,
) -> SessionRecord {
    let directory = store.session_directory(id).unwrap();
    fs::create_dir_all(directory.join("snapshot")).unwrap();
    if let Some(manifest) = manifest {
        fs::write(
            store.manifest_path(id).unwrap(),
            serde_json::to_vec(manifest).unwrap(),
        )
        .unwrap();
    }
    let record = SessionRecord {
        format: hortus_history::FORMAT_VERSION,
        id: id.into(),
        vault: store.data_root().to_string_lossy().into_owned(),
        created_at_ms: 1,
        started_at_ms: Some(1),
        ended_at_ms: None,
        state,
        snapshot: "snapshot".into(),
        stats,
        exit_code: None,
        error: None,
    };
    fs::write(
        directory.join("session.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    record
}

fn find<'a>(summary: &'a ReviewSummary, id: &str) -> &'a ReviewEntry {
    summary
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("missing entry {id}: {:#?}", summary.entries))
}

fn changed(summary: &ReviewSummary, path: &str) -> bool {
    summary.entries.iter().any(|entry| {
        entry.id == path
            || entry.old_path.as_deref() == Some(path)
            || entry.new_path.as_deref() == Some(path)
    })
}

#[test]
fn session_review_detects_all_change_classes_and_stays_read_only() {
    let (root, vault, store) = vault_fixture();
    let record = store.begin(&vault, None).unwrap();
    assert_eq!(record.state, SessionState::Ready);
    mutate(&vault, root.path());

    let before = describe(&vault);
    let summary = store.review(&record.id, None).unwrap();
    assert_eq!(summary.session_id, record.id);
    assert_eq!(summary.state, "ready");
    assert!(summary.elapsed_ms <= summary.elapsed_ms.max(1));
    assert_eq!(describe(&vault), before, "审阅不得向源库写文件或改元数据");

    let added = find(&summary, "Added/New note.md");
    assert_eq!(added.change, ChangeKind::Added);
    assert_eq!(added.after.as_ref().unwrap().kind, "file");
    assert_eq!(find(&summary, "empty-added-dir").change, ChangeKind::Added);
    assert_eq!(find(&summary, ".hidden-added").change, ChangeKind::Added);
    assert_eq!(find(&summary, "added-zero.bin").change, ChangeKind::Added);
    assert_eq!(
        find(&summary, "added-zero.bin")
            .after
            .as_ref()
            .unwrap()
            .size,
        0
    );

    let edited = find(&summary, "edit.md");
    assert_eq!(edited.change, ChangeKind::Modified);
    assert!(!edited.content_identical);
    assert_eq!(
        edited.before.as_ref().unwrap().size,
        edited.after.as_ref().unwrap().size
    );

    assert_eq!(find(&summary, "delete.md").change, ChangeKind::Deleted);

    let renamed = find(&summary, "Renamed/Renamed.md");
    assert_eq!(renamed.change, ChangeKind::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("rename.md"));
    assert!(renamed.content_identical);

    let shifted = find(&summary, "renamed-with-edit.md");
    assert_eq!(shifted.change, ChangeKind::Renamed);
    assert_eq!(shifted.old_path.as_deref(), Some("rename-edit.md"));
    assert!(!shifted.content_identical);

    let mode = find(&summary, "mode.md");
    assert_eq!(mode.change, ChangeKind::Modified);
    assert!(mode.content_identical);
    assert_ne!(
        mode.before.as_ref().unwrap().mode,
        mode.after.as_ref().unwrap().mode
    );

    let type_changed = find(&summary, "type.md");
    assert_eq!(type_changed.change, ChangeKind::TypeChanged);
    assert_eq!(type_changed.before.as_ref().unwrap().kind, "file");
    assert_eq!(type_changed.after.as_ref().unwrap().kind, "directory");

    assert_eq!(
        find(&summary, "attachment.png").change,
        ChangeKind::Modified
    );
    let link_renamed = find(&summary, "link-new");
    assert_eq!(link_renamed.change, ChangeKind::Renamed);
    assert_eq!(link_renamed.old_path.as_deref(), Some("link-drop"));
    assert!(link_renamed.content_identical);
    assert_eq!(find(&summary, "link-edit").change, ChangeKind::Modified);

    // 未变化的文件、目录与 .git 内容都不进入列表。
    assert!(!changed(&summary, "keep.md"));
    assert!(!changed(&summary, "empty-dir"));
    assert!(
        !summary
            .entries
            .iter()
            .any(|entry| entry.id.contains(".git"))
    );
    // 符号链接目标在库外，审阅不跟随目录链接。
    assert!(
        !summary
            .entries
            .iter()
            .any(|entry| entry.id.contains("outside/tracked"))
    );
    assert_eq!(summary.counts.added, 7);
    assert_eq!(summary.counts.deleted, 3);
    assert_eq!(summary.counts.renamed, 3);
    assert_eq!(summary.counts.modified, 4);
    assert_eq!(summary.counts.type_changed, 1);

    // 详情：逐行 diff。
    let detail = store
        .review_entry(&record.id, "edit.md", None, None)
        .unwrap();
    assert!(detail.text && !detail.binary && !detail.large);
    assert!(!detail.content_identical);
    assert_eq!(detail.hunks.len(), 1);
    let lines: Vec<&str> = detail.hunks[0]
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert!(lines.contains(&"beta"));
    assert!(lines.contains(&"BETA"));
    let removed = detail.hunks[0]
        .lines
        .iter()
        .find(|line| line.text == "beta")
        .unwrap();
    assert_eq!(removed.kind, "removed");
    assert_eq!(removed.old_line, Some(2));
    let added_line = detail.hunks[0]
        .lines
        .iter()
        .find(|line| line.text == "BETA")
        .unwrap();
    assert_eq!(added_line.kind, "added");
    assert_eq!(added_line.new_line, Some(2));

    let rename_detail = store
        .review_entry(&record.id, "Renamed/Renamed.md", Some("rename.md"), None)
        .unwrap();
    assert!(rename_detail.text);
    assert!(rename_detail.content_identical);
    assert!(rename_detail.hunks.is_empty());

    let binary_detail = store
        .review_entry(&record.id, "attachment.png", None, None)
        .unwrap();
    assert!(!binary_detail.text);
    assert!(binary_detail.binary);
    assert!(!binary_detail.content_identical);
    assert_eq!(
        binary_detail.entry.before.as_ref().unwrap().size,
        binary_detail.entry.after.as_ref().unwrap().size
    );
    assert!(binary_detail.hunks.is_empty());

    let link_detail = store
        .review_entry(&record.id, "link-edit", None, None)
        .unwrap();
    assert!(!link_detail.text && !link_detail.binary);
    assert_eq!(
        link_detail.entry.before.as_ref().unwrap().link.as_deref(),
        Some("edit.md")
    );
    assert_eq!(
        link_detail.entry.after.as_ref().unwrap().link.as_deref(),
        Some("keep.md")
    );

    let added_detail = store
        .review_entry(&record.id, "Added/New note.md", None, None)
        .unwrap();
    assert!(added_detail.text);
    assert!(
        added_detail.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == "added" && line.text == "# New")
    );

    let deleted_detail = store
        .review_entry(&record.id, "delete.md", None, None)
        .unwrap();
    assert!(deleted_detail.text);
    assert!(
        deleted_detail.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == "removed" && line.text == "delete this")
    );
    // 整个目录删除后，目录内文件仍可按快照详情审阅。
    let folder_detail = store
        .review_entry(&record.id, "folder/inside.md", None, None)
        .unwrap();
    assert_eq!(folder_detail.entry.change, ChangeKind::Deleted);
    assert!(folder_detail.text);
    assert!(
        folder_detail.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == "removed" && line.text == "folder content")
    );

    let type_detail = store
        .review_entry(&record.id, "type.md", None, None)
        .unwrap();
    assert_eq!(type_detail.entry.change, ChangeKind::TypeChanged);
    assert!(!type_detail.text && !type_detail.binary);
}

fn describe(root: &Path) -> BTreeMap<String, (String, u64, u32, i64, Vec<u8>)> {
    let mut result = BTreeMap::new();
    fn walk(
        root: &Path,
        relative: &Path,
        result: &mut BTreeMap<String, (String, u64, u32, i64, Vec<u8>)>,
    ) {
        for entry in fs::read_dir(root.join(relative)).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = relative.join(entry.file_name());
            let metadata = fs::symlink_metadata(&path).unwrap();
            let name = name.to_string_lossy().into_owned();
            if metadata.file_type().is_symlink() {
                result.insert(
                    name,
                    (
                        "symlink".into(),
                        metadata.size(),
                        metadata.mode() & 0o7777,
                        metadata.mtime(),
                        fs::read_link(&path)
                            .unwrap()
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                    ),
                );
            } else if metadata.is_dir() {
                result.insert(
                    name,
                    (
                        "directory".into(),
                        metadata.size(),
                        metadata.mode() & 0o7777,
                        metadata.mtime(),
                        Vec::new(),
                    ),
                );
                walk(root, &relative.join(entry.file_name()), result);
            } else {
                result.insert(
                    name,
                    (
                        "file".into(),
                        metadata.size(),
                        metadata.mode() & 0o7777,
                        metadata.mtime(),
                        fs::read(&path).unwrap(),
                    ),
                );
            }
        }
    }
    walk(root, Path::new(""), &mut result);
    result
}

#[test]
fn line_endings_and_missing_final_newline_are_reported() {
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("crlf.md"), b"line one\r\nline two\r\n").unwrap();
    fs::write(vault.join("final.md"), b"first\nsecond").unwrap();
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&vault, None).unwrap();
    fs::write(vault.join("crlf.md"), b"line one\nline two\n").unwrap();
    fs::write(vault.join("final.md"), b"first\nsecond\n").unwrap();

    let summary = store.review(&record.id, None).unwrap();
    assert_eq!(summary.entries.len(), 2);
    let crlf = store
        .review_entry(&record.id, "crlf.md", None, None)
        .unwrap();
    assert!(crlf.text && !crlf.content_identical);
    assert!(crlf.line_endings_changed);
    assert!(crlf.hunks.is_empty());
    let final_newline = store
        .review_entry(&record.id, "final.md", None, None)
        .unwrap();
    assert!(final_newline.text);
    assert!(final_newline.old_no_final_newline);
    assert!(!final_newline.new_no_final_newline);
    assert!(final_newline.hunks.is_empty());
}

#[test]
fn rename_pairing_is_deterministic_and_never_merges_unrelated_edits() {
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("alpha.md"), b"same alpha\n").unwrap();
    fs::write(vault.join("beta.md"), b"same beta\n").unwrap();
    fs::write(vault.join("empty-a"), b"").unwrap();
    fs::write(vault.join("empty-b"), b"").unwrap();
    fs::write(
        vault.join("unrelated.md"),
        b"alpha unique line\nbeta unique line\n",
    )
    .unwrap();
    fs::write(vault.join("pair-old.md"), b"one\ntwo\nthree\nfour\nfive\n").unwrap();
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&vault, None).unwrap();

    fs::rename(vault.join("alpha.md"), vault.join("alpha-copy.md")).unwrap();
    fs::rename(vault.join("beta.md"), vault.join("beta-copy.md")).unwrap();
    fs::rename(vault.join("empty-a"), vault.join("empty-x")).unwrap();
    fs::rename(vault.join("empty-b"), vault.join("empty-y")).unwrap();
    fs::remove_file(vault.join("unrelated.md")).unwrap();
    fs::write(
        vault.join("totally-new.md"),
        b"gamma unique line\ndelta unique line\n",
    )
    .unwrap();
    fs::rename(vault.join("pair-old.md"), vault.join("pair-new.md")).unwrap();
    fs::write(vault.join("pair-new.md"), b"one\ntwo\nthree\nfour\nFIVE\n").unwrap();

    let first = store.review(&record.id, None).unwrap();
    let second = store.review(&record.id, None).unwrap();
    assert_eq!(first.entries, second.entries);
    let renamed: Vec<(&str, &str)> = first
        .entries
        .iter()
        .filter(|entry| entry.change == ChangeKind::Renamed)
        .map(|entry| {
            (
                entry.old_path.as_deref().unwrap(),
                entry.new_path.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        renamed,
        vec![
            ("alpha.md", "alpha-copy.md"),
            ("beta.md", "beta-copy.md"),
            ("empty-a", "empty-x"),
            ("empty-b", "empty-y"),
            ("pair-old.md", "pair-new.md"),
        ]
    );
    assert_eq!(find(&first, "totally-new.md").change, ChangeKind::Added);
    assert_eq!(find(&first, "unrelated.md").change, ChangeKind::Deleted);
    assert_eq!(first.counts.added, 1);
    assert_eq!(first.counts.deleted, 1);
    assert_eq!(first.counts.renamed, 5);
    let pair = find(&first, "pair-new.md");
    assert_eq!(pair.change, ChangeKind::Renamed);
    assert!(!pair.content_identical);
}

#[test]
fn unusable_and_corrupt_sessions_are_rejected() {
    let (_root, vault, store) = vault_fixture();
    let record = store.begin(&vault, None).unwrap();
    for state in [
        SessionState::Preparing,
        SessionState::Cancelled,
        SessionState::Failed,
    ] {
        let id = match state {
            SessionState::Preparing => "11111111-1111-4111-8111-111111111111",
            SessionState::Cancelled => "22222222-2222-4222-8222-222222222222",
            _ => "33333333-3333-4333-8333-333333333333",
        };
        write_session(&store, id, state, None, None);
        let error = store.review(id, None).unwrap_err();
        assert_eq!(error.to_string(), "history.session_unusable", "{state:?}");
    }

    let missing_manifest = "44444444-4444-4444-8444-444444444444";
    write_session(
        &store,
        missing_manifest,
        SessionState::Ready,
        Some(SnapshotStats::default()),
        None,
    );
    assert_eq!(
        store
            .review(missing_manifest, None)
            .unwrap_err()
            .to_string(),
        "history.missing_manifest"
    );

    let missing_snapshot = "55555555-5555-4555-8555-555555555555";
    write_session(
        &store,
        missing_snapshot,
        SessionState::Ready,
        Some(SnapshotStats::default()),
        Some(&[]),
    );
    fs::remove_dir_all(
        store
            .session_directory(missing_snapshot)
            .unwrap()
            .join("snapshot"),
    )
    .unwrap();
    assert_eq!(
        store
            .review(missing_snapshot, None)
            .unwrap_err()
            .to_string(),
        "history.missing_snapshot"
    );

    let invalid_manifest = "66666666-6666-4666-8666-666666666666";
    write_session(
        &store,
        invalid_manifest,
        SessionState::Ready,
        Some(SnapshotStats::default()),
        None,
    );
    fs::write(store.manifest_path(invalid_manifest).unwrap(), b"not json").unwrap();
    assert_eq!(
        store
            .review(invalid_manifest, None)
            .unwrap_err()
            .to_string(),
        "history.invalid_manifest"
    );

    let corrupt = "77777777-7777-4777-8777-777777777777";
    let entry = ManifestEntry {
        path: "missing.md".into(),
        kind: "file".into(),
        size: 4,
        mode: 0o644,
        modified_ms: None,
        link: None,
    };
    write_session(
        &store,
        corrupt,
        SessionState::Ready,
        Some(SnapshotStats {
            entries: 1,
            ..SnapshotStats::default()
        }),
        Some(&[entry]),
    );
    assert_eq!(
        store.review(corrupt, None).unwrap_err().to_string(),
        "history.snapshot_corrupt"
    );

    assert_eq!(
        store.review("../escape", None).unwrap_err().to_string(),
        "history.invalid_session"
    );
    for path in ["", "../escape", "/etc/passwd", "a/../b", "a//b"] {
        let error = store
            .review_entry(&record.id, path, None, None)
            .unwrap_err();
        assert_eq!(error.to_string(), "history.invalid_path", "{path}");
    }
    assert_eq!(
        store
            .review_entry(&record.id, "absent.md", None, None)
            .unwrap_err()
            .to_string(),
        "history.invalid_entry"
    );
}

#[test]
fn cancellation_is_honored_before_and_during_review() {
    let (root, vault, store) = vault_fixture();
    let record = store.begin(&vault, None).unwrap();
    let cancellation = Cancellation::new();
    cancellation.cancel();
    let error = store
        .review(&record.id, Some(cancellation.flag()))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(error.to_string(), "history.cancelled");
    let error = store
        .review_entry(&record.id, "edit.md", None, Some(cancellation.flag()))
        .unwrap_err();
    assert_eq!(error.to_string(), "history.cancelled");
    let _ = root;
}

#[test]
fn large_library_review_reports_counts_and_cancels() {
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::create_dir(vault.join("notes")).unwrap();
    let payload = vec![b'x'; 32 * 1024];
    for index in 0..1000 {
        fs::write(vault.join(format!("notes/{:04}.md", index)), &payload).unwrap();
    }
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&vault, None).unwrap();
    for index in 0..100 {
        let path = vault.join(format!("notes/{:04}.md", index));
        let mut bytes = payload.clone();
        bytes[0] = b'y';
        fs::write(path, bytes).unwrap();
    }
    let cancellation = Cancellation::new();
    let summary = store.review(&record.id, Some(cancellation.flag())).unwrap();
    assert_eq!(summary.counts.modified, 100);
    assert_eq!(summary.entries.len(), 100);
    assert!(summary.elapsed_ms < 60_000, "{}ms", summary.elapsed_ms);

    cancellation.cancel();
    let error = store
        .review(&record.id, Some(cancellation.flag()))
        .unwrap_err();
    assert_eq!(error.to_string(), "history.cancelled");

    let large = root.path().join("vault/large.md");
    let mut bytes = vec![b'a'; MAX_TEXT_BYTES as usize + 32];
    bytes[0] = b'b';
    fs::write(&large, &bytes).unwrap();
    let summary = store.review(&record.id, None).unwrap();
    let detail = store
        .review_entry(&record.id, "large.md", None, None)
        .unwrap();
    assert!(
        summary
            .entries
            .iter()
            .any(|entry| entry.id == "large.md" && entry.change == ChangeKind::Added)
    );
    assert!(detail.large && !detail.text && detail.hunks.is_empty());
}
