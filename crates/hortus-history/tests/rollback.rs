#![cfg(target_os = "macos")]

//! P1-35 安全回滚：按处、按文件、按会话、stale 拒绝与显式快照清理。
use hortus_history::review::{ChangeKind, ReviewEntry};
use hortus_history::rollback::{RollbackExpectation, SessionRollbackReport};
use hortus_history::{HistoryStore, SessionState};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    root: tempfile::TempDir,
    vault: PathBuf,
    store: HistoryStore,
    session: String,
    temporary: tempfile::TempDir,
}

impl Fixture {
    fn new(seed: impl FnOnce(&Path)) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let vault = root.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        seed(&vault);
        let store = HistoryStore::new(root.path().join("data"));
        let record = store.begin(&vault, None).unwrap();
        let temporary = tempfile::TempDir::new_in(root.path()).unwrap();
        Self {
            root,
            vault,
            store,
            session: record.id,
            temporary,
        }
    }

    fn review(&self) -> Vec<ReviewEntry> {
        self.store.review(&self.session, None).unwrap().entries
    }

    fn entry(&self, id: &str) -> ReviewEntry {
        self.review()
            .into_iter()
            .find(|entry| entry.id == id)
            .unwrap_or_else(|| panic!("missing review entry {id}"))
    }

    fn expectation(&self, id: &str) -> RollbackExpectation {
        expectation_of(&self.entry(id))
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.vault.join(path)).unwrap()
    }
}

fn expectation_of(entry: &ReviewEntry) -> RollbackExpectation {
    RollbackExpectation {
        path: entry.id.clone(),
        old_path: entry.old_path.clone(),
        new_path: entry.new_path.clone(),
        change: entry.change,
        expected_before: entry.before_digest.clone(),
        expected_after: entry.after_digest.clone(),
    }
}

#[test]
fn hunk_rollback_preserves_other_hunks_crlf_and_missing_final_newline() {
    let fixture = Fixture::new(|vault| {
        fs::write(
            vault.join("hunks.md"),
            b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nTEN",
        )
        .unwrap();
    });
    fs::write(
        fixture.vault.join("hunks.md"),
        b"one\r\nchanged2\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10",
    )
    .unwrap();
    let detail = fixture
        .store
        .review_entry(&fixture.session, "hunks.md", None, None)
        .unwrap();
    assert_eq!(detail.hunks.len(), 2, "expected two independent hunks");
    let expectation = expectation_of(&detail.entry);
    fixture
        .store
        .rollback_hunk(
            &fixture.session,
            "hunks.md",
            None,
            &expectation,
            0,
            &detail.hunks[0],
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(
        fixture.read("hunks.md"),
        b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10"
    );
    let after_first = fixture
        .store
        .review_entry(&fixture.session, "hunks.md", None, None)
        .unwrap();
    assert_eq!(after_first.hunks.len(), 1);
    fixture
        .store
        .rollback_hunk(
            &fixture.session,
            "hunks.md",
            None,
            &expectation_of(&after_first.entry),
            0,
            &after_first.hunks[0],
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(
        fixture.read("hunks.md"),
        b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nTEN"
    );
}

#[test]
fn hunk_rollback_rejects_binary_and_outdated_context() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("text.md"), b"alpha\nbeta\ngamma\n").unwrap();
        fs::write(vault.join("image.png"), (0..=255u8).collect::<Vec<_>>()).unwrap();
    });
    fs::write(fixture.vault.join("text.md"), b"alpha\nBETA\ngamma\n").unwrap();
    fs::write(
        fixture.vault.join("image.png"),
        (1..=255u8).chain([0u8]).collect::<Vec<_>>(),
    )
    .unwrap();
    let text = fixture
        .store
        .review_entry(&fixture.session, "text.md", None, None)
        .unwrap();
    let binary = fixture
        .store
        .review_entry(&fixture.session, "image.png", None, None)
        .unwrap();
    assert!(binary.binary);
    // 二进制条目没有 hunk；即使传入伪造上下文也必须拒绝。
    let failure = fixture.store.rollback_hunk(
        &fixture.session,
        "image.png",
        None,
        &expectation_of(&binary.entry),
        0,
        &text.hunks[0],
        fixture.temporary.path(),
        None,
    );
    assert_eq!(failure.unwrap_err().to_string(), "history.hunk_unsupported");

    // 审阅后外部把整段换掉：上下文验证拒绝写入。
    fs::write(fixture.vault.join("text.md"), b"alpha\nOTHER\ngamma\n").unwrap();
    let failure = fixture.store.rollback_hunk(
        &fixture.session,
        "text.md",
        None,
        &expectation_of(&text.entry),
        0,
        &text.hunks[0],
        fixture.temporary.path(),
        None,
    );
    assert_eq!(failure.unwrap_err().to_string(), "history.review_stale");
}

#[test]
fn stale_checks_cover_equal_length_mode_and_symlink_changes() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("edit.md"), b"alpha\nbeta\ngamma\n").unwrap();
        fs::write(vault.join("mode.md"), b"mode stays\n").unwrap();
        fs::write(vault.join("one.md"), b"one\n").unwrap();
        fs::write(vault.join("two.md"), b"two\n").unwrap();
        symlink("one.md", vault.join("link")).unwrap();
    });
    fs::write(fixture.vault.join("edit.md"), b"alpha\nBETA\ngamma\n").unwrap();
    fs::set_permissions(
        fixture.vault.join("mode.md"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let link_location = fixture.vault.join("link");
    fs::remove_file(&link_location).unwrap();
    symlink("two.md", &link_location).unwrap();

    let edit = fixture.expectation("edit.md");
    let mode = fixture.expectation("mode.md");
    let link = fixture.expectation("link");
    assert!(
        edit.expected_after
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(mode.expected_after.is_some());
    assert!(link.expected_before.is_some());

    // 同字节长度的外部改写。
    fs::write(fixture.vault.join("edit.md"), b"alpha\nZETA\ngamma\n").unwrap();
    assert_eq!(
        fixture
            .store
            .rollback_entry(&fixture.session, &edit, fixture.temporary.path(), None)
            .unwrap_err()
            .to_string(),
        "history.review_stale"
    );
    assert_eq!(fixture.read("edit.md"), b"alpha\nZETA\ngamma\n");

    // 审阅后用户又改回可写权限。
    fs::set_permissions(
        fixture.vault.join("mode.md"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .rollback_entry(&fixture.session, &mode, fixture.temporary.path(), None)
            .unwrap_err()
            .to_string(),
        "history.review_stale"
    );
    assert_eq!(
        fs::metadata(fixture.vault.join("mode.md")).unwrap().mode() & 0o777,
        0o644
    );

    // 链接目标再次变化。
    fs::remove_file(fixture.vault.join("link")).unwrap();
    symlink("edit.md", fixture.vault.join("link")).unwrap();
    assert_eq!(
        fixture
            .store
            .rollback_entry(&fixture.session, &link, fixture.temporary.path(), None)
            .unwrap_err()
            .to_string(),
        "history.review_stale"
    );
    assert_eq!(
        fs::read_link(fixture.vault.join("link"))
            .unwrap()
            .to_string_lossy(),
        "edit.md"
    );
}

#[test]
fn entry_rollback_restores_each_change_kind() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("edit.md"), b"alpha\nbeta\ngamma\n").unwrap();
        fs::write(vault.join("mode.md"), b"mode stays\n").unwrap();
        fs::write(vault.join("del.md"), b"deleted\n").unwrap();
        fs::write(vault.join("rename.md"), b"rename me\n").unwrap();
        fs::write(vault.join("type.md"), b"was a file\n").unwrap();
        fs::create_dir_all(vault.join("dir/deep")).unwrap();
        fs::write(vault.join("dir/deep/child.md"), b"child\n").unwrap();
        fs::write(vault.join("one.md"), b"one\n").unwrap();
        fs::write(vault.join("two.md"), b"two\n").unwrap();
        symlink("one.md", vault.join("link")).unwrap();
        fs::create_dir_all(vault.join("empty")).unwrap();
        fs::write(vault.join("big.png"), (0..=255u8).collect::<Vec<_>>()).unwrap();
    });
    fs::write(fixture.vault.join("edit.md"), b"alpha\nBETA\ngamma\n").unwrap();
    fs::set_permissions(
        fixture.vault.join("mode.md"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::remove_file(fixture.vault.join("del.md")).unwrap();
    fs::rename(
        fixture.vault.join("rename.md"),
        fixture.vault.join("renamed.md"),
    )
    .unwrap();
    fs::remove_file(fixture.vault.join("type.md")).unwrap();
    fs::create_dir_all(fixture.vault.join("type.md")).unwrap();
    fs::write(fixture.vault.join("type.md/inside.md"), b"inside\n").unwrap();
    fs::remove_dir_all(fixture.vault.join("dir")).unwrap();
    fs::create_dir_all(fixture.vault.join("Added")).unwrap();
    fs::write(fixture.vault.join("Added/new.md"), b"# New\n").unwrap();
    fs::write(fixture.vault.join("zero.bin"), b"").unwrap();
    fs::create_dir_all(fixture.vault.join("empty-added")).unwrap();
    fs::write(fixture.vault.join(".hidden-added"), b"hidden\n").unwrap();
    fs::remove_dir(fixture.vault.join("empty")).unwrap();
    fs::remove_file(fixture.vault.join("link")).unwrap();
    symlink("two.md", fixture.vault.join("link")).unwrap();
    fs::write(
        fixture.vault.join("big.png"),
        (1..=255u8).chain([0u8]).collect::<Vec<_>>(),
    )
    .unwrap();

    let expectations: Vec<RollbackExpectation> =
        fixture.review().iter().map(expectation_of).collect();
    let kinds: Vec<(String, ChangeKind)> = expectations
        .iter()
        .map(|entry| (entry.path.clone(), entry.change))
        .collect();
    assert!(
        kinds
            .iter()
            .any(|(path, kind)| path == "renamed.md" && *kind == ChangeKind::Renamed)
    );
    assert!(
        kinds
            .iter()
            .any(|(path, kind)| path == "type.md" && *kind == ChangeKind::TypeChanged)
    );

    // 按会话顺序覆盖嵌套目录、重命名、符号链接、权限与二进制。
    let report: SessionRollbackReport = fixture
        .store
        .rollback_session(
            &fixture.session,
            &expectations,
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert!(
        report.is_complete(),
        "stale={:?} failed={:?}",
        report.skipped_stale,
        report.failed
    );
    assert_eq!(fixture.review(), Vec::<ReviewEntry>::new());
    assert_eq!(fixture.read("edit.md"), b"alpha\nbeta\ngamma\n");
    assert_eq!(fixture.read("mode.md"), b"mode stays\n");
    assert_eq!(
        fs::metadata(fixture.vault.join("mode.md")).unwrap().mode() & 0o777,
        0o644
    );
    assert_eq!(fixture.read("del.md"), b"deleted\n");
    assert_eq!(fixture.read("rename.md"), b"rename me\n");
    assert!(!fixture.vault.join("renamed.md").exists());
    assert_eq!(fixture.read("type.md"), b"was a file\n");
    assert!(!fixture.vault.join("type.md").is_dir());
    assert_eq!(fixture.read("dir/deep/child.md"), b"child\n");
    assert!(!fixture.vault.join("Added").exists());
    assert!(!fixture.vault.join("zero.bin").exists());
    assert!(!fixture.vault.join("empty-added").exists());
    assert!(!fixture.vault.join(".hidden-added").exists());
    assert!(fixture.vault.join("empty").is_dir());
    assert_eq!(
        fs::read_link(fixture.vault.join("link"))
            .unwrap()
            .to_string_lossy(),
        "one.md"
    );
    assert_eq!(fixture.read("big.png"), (0..=255u8).collect::<Vec<_>>());
}

#[test]
fn entry_rollback_never_overwrites_concurrent_new_file_or_nonempty_directory() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("del.md"), b"deleted\n").unwrap();
    });
    fs::remove_file(fixture.vault.join("del.md")).unwrap();
    fs::create_dir_all(fixture.vault.join("added-dir")).unwrap();
    fs::write(fixture.vault.join("added-dir/inside.md"), b"inside\n").unwrap();
    let deleted = fixture.expectation("del.md");
    let added_dir = fixture.expectation("added-dir");

    fs::write(fixture.vault.join("del.md"), b"user recreated\n").unwrap();
    let failure = fixture
        .store
        .rollback_entry(&fixture.session, &deleted, fixture.temporary.path(), None)
        .unwrap_err()
        .to_string();
    assert_eq!(failure, "history.review_stale");
    assert_eq!(fixture.read("del.md"), b"user recreated\n");

    let failure = fixture
        .store
        .rollback_entry(&fixture.session, &added_dir, fixture.temporary.path(), None)
        .unwrap_err()
        .to_string();
    assert_eq!(failure, "history.directory_not_empty");
    assert_eq!(fixture.read("added-dir/inside.md"), b"inside\n");
}

#[test]
fn session_rollback_reports_stale_entries_and_keeps_them() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("one.md"), b"one\n").unwrap();
        fs::write(vault.join("two.md"), b"two\nthree\n").unwrap();
        fs::write(vault.join("mode.md"), b"mode\n").unwrap();
        fs::create_dir_all(vault.join("nested/deep")).unwrap();
        fs::write(vault.join("nested/deep/del.md"), b"del\n").unwrap();
        fs::write(vault.join("type.md"), b"file\n").unwrap();
    });
    fs::write(fixture.vault.join("one.md"), b"ONE\n").unwrap();
    fs::write(fixture.vault.join("two.md"), b"two\nTHREE\n").unwrap();
    fs::set_permissions(
        fixture.vault.join("mode.md"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::remove_dir_all(fixture.vault.join("nested")).unwrap();
    fs::remove_file(fixture.vault.join("type.md")).unwrap();
    fs::create_dir_all(fixture.vault.join("type.md")).unwrap();
    fs::write(fixture.vault.join("type.md/inside.md"), b"inside\n").unwrap();

    let expectations: Vec<RollbackExpectation> =
        fixture.review().iter().map(expectation_of).collect();
    // 审阅后外部再次修改 one.md：整批继续，但这一项必须保留并汇报。
    fs::write(fixture.vault.join("one.md"), b"EXTERNAL\n").unwrap();
    let report = fixture
        .store
        .rollback_session(
            &fixture.session,
            &expectations,
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(report.skipped_stale.len(), 1, "{:?}", report);
    assert_eq!(report.skipped_stale[0].path, "one.md");
    assert_eq!(report.skipped_stale[0].reason, "history.review_stale");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(fixture.read("one.md"), b"EXTERNAL\n");
    assert_eq!(fixture.read("two.md"), b"two\nthree\n");
    assert_eq!(
        fs::metadata(fixture.vault.join("mode.md")).unwrap().mode() & 0o777,
        0o644
    );
    assert_eq!(fixture.read("nested/deep/del.md"), b"del\n");
    assert_eq!(fixture.read("type.md"), b"file\n");
    assert!(!fixture.vault.join("type.md").is_dir());
    let remaining = fixture.review();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "one.md");
}

#[test]
fn delete_session_clears_readonly_and_immutable_snapshot_without_touching_vault() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("source.md"), b"user content\n").unwrap();
        fs::write(vault.join("locked.md"), b"locked\n").unwrap();
        fs::write(vault.join("readonly.md"), b"readonly\n").unwrap();
    });
    let snapshot = fixture.store.snapshot_directory(&fixture.session).unwrap();
    fs::set_permissions(
        snapshot.join("source.md"),
        fs::Permissions::from_mode(0o400),
    )
    .unwrap();
    fs::set_permissions(
        snapshot.join("readonly.md"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    let status = Command::new("chflags")
        .args(["uchg", snapshot.join("locked.md").to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let source_before = fs::read(fixture.vault.join("source.md")).unwrap();
    let source_meta = fs::metadata(fixture.vault.join("source.md")).unwrap();
    let session_directory = fixture.store.session_directory(&fixture.session).unwrap();
    assert!(session_directory.is_dir());
    fixture.store.delete_session(&fixture.session).unwrap();
    assert!(!session_directory.exists());
    assert_eq!(
        fs::read(fixture.vault.join("source.md")).unwrap(),
        source_before
    );
    assert_eq!(
        fs::metadata(fixture.vault.join("source.md"))
            .unwrap()
            .mode(),
        source_meta.mode()
    );
    assert_eq!(
        fs::metadata(fixture.vault.join("locked.md"))
            .unwrap()
            .mode()
            & 0o222,
        0o644 & 0o222
    );
    assert!(
        fixture
            .store
            .list(None)
            .unwrap()
            .iter()
            .all(|record| record.id != fixture.session)
    );
}

#[test]
fn delete_session_refuses_active_sessions() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("note.md"), b"note\n").unwrap();
    });
    fixture.store.mark_running(&fixture.session).unwrap();
    assert_eq!(
        fixture
            .store
            .delete_session(&fixture.session)
            .unwrap_err()
            .to_string(),
        "history.session_active"
    );
    assert!(
        fixture
            .store
            .session_directory(&fixture.session)
            .unwrap()
            .is_dir()
    );

    let preparing = "11111111-1111-4111-8111-111111111111";
    let directory = fixture.store.session_directory(preparing).unwrap();
    fs::create_dir_all(directory.join("snapshot")).unwrap();
    fs::write(
        directory.join("session.json"),
        format!(
            r#"{{"format":1,"id":"{preparing}","vault":"{}","created_at_ms":1,"state":"preparing","snapshot":"snapshot"}}"#,
            fixture.vault.to_string_lossy()
        ),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .delete_session(preparing)
            .unwrap_err()
            .to_string(),
        "history.session_active"
    );
    assert!(directory.is_dir());
    assert_eq!(
        fixture.store.record(preparing).unwrap().state,
        SessionState::Preparing
    );
}

#[test]
fn rollback_rejects_paths_outside_the_vault_and_missing_entries() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("note.md"), b"note\n").unwrap();
        fs::create_dir_all(vault.join("real")).unwrap();
        symlink("real", vault.join("linkdir")).unwrap();
    });
    let bad = RollbackExpectation {
        path: "../outside.md".into(),
        old_path: None,
        new_path: Some("../outside.md".into()),
        change: ChangeKind::Modified,
        expected_before: None,
        expected_after: None,
    };
    assert_eq!(
        fixture
            .store
            .rollback_entry(&fixture.session, &bad, fixture.temporary.path(), None)
            .unwrap_err()
            .to_string(),
        "history.invalid_path"
    );

    let linked = RollbackExpectation {
        path: "linkdir/note.md".into(),
        old_path: None,
        new_path: Some("linkdir/note.md".into()),
        change: ChangeKind::Added,
        expected_before: None,
        expected_after: None,
    };
    let error = fixture
        .store
        .rollback_entry(&fixture.session, &linked, fixture.temporary.path(), None)
        .unwrap_err()
        .to_string();
    assert!(matches!(
        error.as_str(),
        "history.invalid_path" | "history.invalid_entry"
    ));
    let _ = &fixture.root;
}

#[test]
fn read_only_files_restore_content_and_permissions() {
    let fixture = Fixture::new(|vault| {
        fs::write(vault.join("readonly.md"), b"original\n").unwrap();
        fs::set_permissions(vault.join("readonly.md"), fs::Permissions::from_mode(0o444)).unwrap();
    });
    let path = fixture.vault.join("readonly.md");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&path, b"changed\n").unwrap();
    let expectation = fixture.expectation("readonly.md");
    fixture
        .store
        .rollback_entry(
            &fixture.session,
            &expectation,
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"original\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o444);

    // 按处回滚同样能在只读目标上完成，并保持回滚前的权限。
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&path, b"one\ntwo\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let detail = fixture
        .store
        .review_entry(&fixture.session, "readonly.md", None, None)
        .unwrap();
    fixture
        .store
        .rollback_hunk(
            &fixture.session,
            "readonly.md",
            None,
            &expectation_of(&detail.entry),
            0,
            &detail.hunks[0],
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"original\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o444);
}

#[test]
fn deleted_nested_file_recreates_missing_parents() {
    let fixture = Fixture::new(|vault| {
        fs::create_dir_all(vault.join("nested/deep")).unwrap();
        fs::write(vault.join("nested/deep/note.md"), b"nested note\n").unwrap();
        fs::set_permissions(vault.join("nested"), fs::Permissions::from_mode(0o750)).unwrap();
    });
    fs::remove_dir_all(fixture.vault.join("nested")).unwrap();
    let expectation = fixture.expectation("nested/deep/note.md");
    fixture
        .store
        .rollback_entry(
            &fixture.session,
            &expectation,
            fixture.temporary.path(),
            None,
        )
        .unwrap();
    assert_eq!(
        fs::read(fixture.vault.join("nested/deep/note.md")).unwrap(),
        b"nested note\n"
    );
    assert_eq!(
        fs::metadata(fixture.vault.join("nested")).unwrap().mode() & 0o777,
        0o750
    );
}
