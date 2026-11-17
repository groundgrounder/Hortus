//! 回滚与快照清理 FFI：指纹、stale 拒绝、会话部分失败与活动会话保护。
use hortus_ffi::{ReviewCancellation, ReviewEntryInfo, RollbackExpectationInfo, WorkspaceCore};
use std::fs;
use std::path::Path;
use std::sync::Arc;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_environment() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn expectation(entry: &ReviewEntryInfo) -> RollbackExpectationInfo {
    RollbackExpectationInfo {
        path: entry.id.clone(),
        old_path: entry.old_path.clone(),
        new_path: entry.new_path.clone(),
        change: entry.change.clone(),
        expected_before: entry.before_digest.clone(),
        expected_after: entry.after_digest.clone(),
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<WorkspaceCore>,
    String,
    std::path::PathBuf,
) {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(
        vault.join("hunks.md"),
        b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nTEN",
    )
    .unwrap();
    fs::write(vault.join("edit.md"), b"alpha\nbeta\ngamma\n").unwrap();
    fs::write(vault.join("rename.md"), b"rename\n").unwrap();
    fs::write(vault.join("delete.md"), b"delete\n").unwrap();
    fs::write(vault.join("mode.md"), b"mode\n").unwrap();
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("HORTUS_CONFIG_DIR", &config);
        std::env::set_var("HORTUS_CACHE_DIR", &cache);
        std::env::set_var("HORTUS_DATA_DIR", &data);
    }
    let core = WorkspaceCore::new().unwrap();
    let session = core
        .begin_agent_session(vault.to_string_lossy().into_owned())
        .unwrap();
    (root, core, session.id, vault)
}

fn mutate(vault: &Path) {
    fs::write(
        vault.join("hunks.md"),
        b"one\r\nchanged2\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10",
    )
    .unwrap();
    fs::write(vault.join("edit.md"), b"alpha\nBETA\ngamma\n").unwrap();
    fs::rename(vault.join("rename.md"), vault.join("renamed.md")).unwrap();
    fs::remove_file(vault.join("delete.md")).unwrap();
    fs::write(vault.join("added.md"), b"added\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(vault.join("mode.md"), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn review(core: &WorkspaceCore, id: &str) -> Vec<ReviewEntryInfo> {
    core.review_agent_session(id.to_owned(), ReviewCancellation::new())
        .unwrap()
        .entries
}

fn entry<'a>(entries: &'a [ReviewEntryInfo], id: &str) -> &'a ReviewEntryInfo {
    entries.iter().find(|entry| entry.id == id).unwrap()
}

#[test]
fn ffi_hunk_entry_and_session_rollback_round_trip() {
    let _environment = lock_environment();
    let (root, core, session, vault) = fixture();
    mutate(&vault);
    let cancellation = ReviewCancellation::new();
    let detail = core
        .review_agent_session_entry(
            session.clone(),
            "hunks.md".into(),
            None,
            ReviewCancellation::new(),
        )
        .unwrap();
    assert_eq!(detail.hunks.len(), 2, "expected two hunks");
    assert!(detail.entry.after_digest.is_some());
    let temporary = root.path().join("replacement");
    fs::create_dir_all(&temporary).unwrap();
    core.rollback_agent_session_hunk(
        session.clone(),
        "hunks.md".into(),
        None,
        expectation(&detail.entry),
        0,
        detail.hunks[0].clone(),
        Some(temporary.to_string_lossy().into_owned()),
        cancellation,
    )
    .unwrap();
    assert_eq!(
        fs::read(vault.join("hunks.md")).unwrap(),
        b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10"
    );

    let remaining = review(&core, &session);
    // 审阅后外部改写 edit.md：按文件回滚必须拒绝，磁盘保持外部内容。
    let edit = expectation(entry(&remaining, "edit.md"));
    fs::write(vault.join("edit.md"), b"alpha\nZETA\ngamma\n").unwrap();
    let failure = core
        .rollback_agent_session_entry(session.clone(), edit, None, ReviewCancellation::new())
        .unwrap_err();
    assert!(format!("{failure}").contains("history.review_stale"));
    assert_eq!(
        fs::read(vault.join("edit.md")).unwrap(),
        b"alpha\nZETA\ngamma\n"
    );

    // 按会话回滚其余条目；stale 的 edit.md 保留并清晰汇报。
    let expectations: Vec<RollbackExpectationInfo> = remaining.iter().map(expectation).collect();
    let report = core
        .rollback_agent_session(
            session.clone(),
            expectations,
            Some(temporary.to_string_lossy().into_owned()),
            ReviewCancellation::new(),
        )
        .unwrap();
    assert_eq!(report.skipped_stale.len(), 1, "{report:?}");
    assert_eq!(report.skipped_stale[0].path, "edit.md");
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(
        fs::read(vault.join("edit.md")).unwrap(),
        b"alpha\nZETA\ngamma\n"
    );
    assert_eq!(fs::read(vault.join("rename.md")).unwrap(), b"rename\n");
    assert!(!vault.join("renamed.md").exists());
    assert_eq!(fs::read(vault.join("delete.md")).unwrap(), b"delete\n");
    assert!(!vault.join("added.md").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(vault.join("mode.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
    assert_eq!(
        fs::read(vault.join("hunks.md")).unwrap(),
        b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nTEN"
    );

    // 显式删除快照，用户库不受影响。
    core.delete_agent_session(session.clone()).unwrap();
    let data = std::path::Path::new(&core.data_directory()).join("agent-sessions");
    assert!(!data.join(&session).exists());
    assert!(vault.join("edit.md").exists());
}

#[test]
fn ffi_delete_refuses_running_session() {
    let _environment = lock_environment();
    let (_root, core, session, _vault) = fixture();
    core.mark_agent_session(session.clone(), "running".into(), None)
        .unwrap();
    let failure = core.delete_agent_session(session.clone()).unwrap_err();
    assert!(format!("{failure}").contains("history.session_active"));
    assert!(
        std::path::Path::new(&core.data_directory())
            .join("agent-sessions")
            .join(&session)
            .is_dir()
    );
}
