//! 快照范围与保真、会话元数据与重启恢复。
use hortus_history::{HistoryStore, SessionState};
use std::fs;
use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn fixture() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir_all(vault.join("nested/deeper")).unwrap();
    fs::create_dir_all(vault.join("empty folder")).unwrap();
    fs::create_dir_all(vault.join(".obsidian")).unwrap();
    fs::create_dir_all(vault.join("nested/.git")).unwrap();
    fs::create_dir_all(vault.join(".git/refs")).unwrap();
    fs::write(vault.join("notes.md"), b"# Notes\n\nText\n").unwrap();
    fs::write(vault.join("中文 笔记.md"), "# 中文\n").unwrap();
    fs::write(vault.join("nested/deeper/space name.md"), "line\n").unwrap();
    fs::write(vault.join("empty.txt"), b"").unwrap();
    fs::write(vault.join(".obsidian/workspace.json"), b"{}").unwrap();
    fs::write(vault.join(".hidden"), b"secret\n").unwrap();
    fs::write(vault.join(".git/config"), b"[core]\n").unwrap();
    fs::write(vault.join(".git/refs/head"), b"ref\n").unwrap();
    fs::write(vault.join("nested/.git/HEAD"), b"ref\n").unwrap();
    let binary: Vec<u8> = (0..=255u8).collect();
    fs::write(vault.join("attachment.bin"), &binary).unwrap();
    let script = vault.join("run.sh");
    fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let outside = root.path().join("outside.txt");
    fs::write(&outside, b"outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, vault.join("outside-link")).unwrap();
    std::os::unix::fs::symlink("notes.md", vault.join("relative-link")).unwrap();
    // 固定修改时间，验证复制后逐字一致（含纳秒）。
    let stamp = UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_789);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(vault.join("notes.md"))
        .unwrap();
    file.set_times(fs::FileTimes::new().set_accessed(stamp).set_modified(stamp))
        .unwrap();
    root
}

fn relative_files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    fn walk(root: &Path, directory: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                found.push(relative + "/");
                walk(root, &path, found);
            } else {
                found.push(relative);
            }
        }
    }
    walk(root, root, &mut found);
    found.sort();
    found
}

fn manifest(store: &HistoryStore, id: &str) -> Vec<hortus_history::snapshot::ManifestEntry> {
    let data = fs::read_to_string(store.manifest_path(id).unwrap()).unwrap();
    serde_json::from_str(&data).unwrap()
}

#[test]
fn snapshot_matches_source_scope_and_fidelity() {
    let root = fixture();
    let vault = root.path().join("vault");
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&vault, None).unwrap();
    assert_eq!(record.state, SessionState::Ready);
    let snapshot = store.snapshot_directory(&record.id).unwrap();

    let source = relative_files(&vault);
    let copy = relative_files(&snapshot);
    let filtered: Vec<String> = source
        .into_iter()
        .filter(|path| !path.split('/').any(|part| part == ".git"))
        .collect();
    assert_eq!(copy, filtered, "snapshot scope differs from source");

    for path in filtered.iter().filter(|path| !path.ends_with('/')) {
        let from = vault.join(path);
        let to = snapshot.join(path);
        let from_meta = fs::symlink_metadata(&from).unwrap();
        let to_meta = fs::symlink_metadata(&to).unwrap();
        assert_eq!(from_meta.file_type(), to_meta.file_type(), "{path}");
        if from_meta.file_type().is_symlink() {
            assert_eq!(fs::read_link(&from).unwrap(), fs::read_link(&to).unwrap());
            continue;
        }
        assert_eq!(fs::read(&from).unwrap(), fs::read(&to).unwrap(), "{path}");
        assert_eq!(
            from_meta.permissions().mode() & 0o7777,
            to_meta.permissions().mode() & 0o7777,
            "{path}"
        );
        assert_eq!(from_meta.mtime(), to_meta.mtime(), "{path}");
        assert_eq!(from_meta.mtime_nsec(), to_meta.mtime_nsec(), "{path}");
    }
    assert_eq!(
        fs::metadata(&vault).unwrap().permissions().mode() & 0o7777,
        fs::metadata(&snapshot).unwrap().permissions().mode() & 0o7777
    );
    assert!(snapshot.join("empty folder").is_dir());
    assert!(!snapshot.join(".git").exists());
    assert!(!snapshot.join("nested/.git").exists());
    // 库外符号链接目标没有被复制。
    assert!(!snapshot.join("outside.txt").exists());

    let entries = manifest(&store, &record.id);
    let by_path = |path: &str| entries.iter().find(|entry| entry.path == path).unwrap();
    assert_eq!(by_path("empty folder").kind, "directory");
    assert_eq!(by_path("outside-link").kind, "symlink");
    assert_eq!(
        by_path("outside-link").link.as_deref(),
        Some(root.path().join("outside.txt").to_str().unwrap())
    );
    assert_eq!(by_path("attachment.bin").size, 256);
    assert_eq!(by_path("run.sh").mode & 0o777, 0o755);
    assert!(entries.iter().all(|entry| !entry.path.contains(".git")));
    assert_eq!(record.stats.as_ref().unwrap().symlinks, 2);
    assert_eq!(record.stats.as_ref().unwrap().files, 8);
}

#[test]
fn sessions_persist_independently_and_recover_running_ones() {
    let root = fixture();
    let vault = root.path().join("vault");
    let store = HistoryStore::new(root.path().join("data"));
    let first = store.begin(&vault, None).unwrap();
    let second = store.begin(&vault, None).unwrap();
    assert_ne!(first.id, second.id);
    let first_snapshot = store.snapshot_directory(&first.id).unwrap();
    let before = fs::read(first_snapshot.join("notes.md")).unwrap();

    store.mark_running(&first.id).unwrap();
    store
        .finish(&first.id, SessionState::Exited, Some(0))
        .unwrap();
    // 模拟应用崩溃后重启：另一份仍是 running。
    store.mark_running(&second.id).unwrap();
    assert_eq!(store.recover().unwrap(), 1);

    let canonical = fs::canonicalize(&vault).unwrap();
    let sessions = store.list(Some(&canonical.to_string_lossy())).unwrap();
    assert_eq!(sessions.len(), 2);
    let reloaded_first = store.record(&first.id).unwrap();
    let reloaded_second = store.record(&second.id).unwrap();
    assert_eq!(reloaded_first.state, SessionState::Exited);
    assert_eq!(reloaded_first.exit_code, Some(0));
    assert_eq!(reloaded_second.state, SessionState::Interrupted);
    assert!(reloaded_second.ended_at_ms.is_some());
    assert!(reloaded_second.started_at_ms.is_some());
    assert_eq!(fs::read(first_snapshot.join("notes.md")).unwrap(), before);
    // 第二次启动不覆盖第一次。
    assert_ne!(
        fs::read(
            store
                .snapshot_directory(&second.id)
                .unwrap()
                .join("notes.md")
        )
        .unwrap(),
        Vec::<u8>::new()
    );
    // 历史会话不会被自动执行或删除。
    assert!(fs::read_dir(store.sessions_directory()).unwrap().count() >= 2);
}

#[test]
fn session_and_metadata_use_private_permissions() {
    let root = fixture();
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&root.path().join("vault"), None).unwrap();
    let directory = store.session_directory(&record.id).unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&directory.join("session.json")), 0o600);
    assert_eq!(mode(&store.manifest_path(&record.id).unwrap()), 0o600);
    let text = fs::read_to_string(directory.join("session.json")).unwrap();
    assert!(text.contains("\"format\": 1"));
    assert!(text.contains("\"state\": \"ready\""));
    assert!(text.contains("\"snapshot\": \"snapshot\""));
    assert!(!text.contains("HORTUS_") && !text.contains("argv"));
}

#[test]
fn unsupported_special_entries_fail_the_whole_snapshot() {
    let root = fixture();
    let vault = root.path().join("vault");
    let fifo = vault.join("pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let store = HistoryStore::new(root.path().join("data"));
    let error = store.begin(&vault, None).unwrap_err();
    assert_eq!(error.to_string(), "history.unsupported_entry");
    let sessions = store.list(None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, SessionState::Failed);
    assert!(!sessions[0].state.usable());
}

#[test]
fn unwritable_data_directory_fails_without_a_usable_session() {
    let root = fixture();
    let data = root.path().join("data");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o500)).unwrap();
    let store = HistoryStore::new(&data);
    let result = store.begin(&root.path().join("vault"), None);
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.unwrap_err();
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Other
        ),
        "{error:?}"
    );
    assert!(store.list(None).unwrap().is_empty());
}

#[test]
fn cancellation_leaves_a_cancelled_record_and_no_usable_snapshot() {
    let root = fixture();
    let store = HistoryStore::new(root.path().join("data"));
    let cancel = hortus_history::Cancellation::new();
    cancel.cancel();
    let error = store
        .begin(&root.path().join("vault"), Some(cancel.flag()))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    let sessions = store.list(None).unwrap();
    assert_eq!(sessions[0].state, SessionState::Cancelled);
    assert!(!sessions[0].state.usable());
}

#[test]
fn custom_metadata_survives_the_copy() {
    let root = fixture();
    let vault = root.path().join("vault");
    let note = vault.join("notes.md");
    let status = Command::new("xattr")
        .args(["-w", "user.hortus-snapshot", "tree", note.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let store = HistoryStore::new(root.path().join("data"));
    let record = store.begin(&vault, None).unwrap();
    let copied = store
        .snapshot_directory(&record.id)
        .unwrap()
        .join("notes.md");
    let value = Command::new("xattr")
        .args(["-p", "user.hortus-snapshot", copied.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(value.status.success());
    assert_eq!(String::from_utf8_lossy(&value.stdout).trim(), "tree");
    // 创建时间也保留（允许系统不支持时为零）。
    let source = fs::symlink_metadata(&note).unwrap();
    let destination = fs::symlink_metadata(&copied).unwrap();
    assert_eq!(source.uid(), destination.uid());
    let source_birth = source.st_birthtime();
    assert_eq!(source_birth, destination.st_birthtime());
    assert!(SystemTime::now() > UNIX_EPOCH + Duration::from_secs(source.mtime() as u64));
}
