//! 审阅 FFI：摘要、条目详情、取消与路径边界。
use hortus_ffi::{ReviewCancellation, WorkspaceCore};
use std::fs;

#[test]
fn review_summary_and_details_round_trip() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("edit.md"), b"alpha\nbeta\n").unwrap();
    fs::write(vault.join("rename.md"), b"stable rename\n").unwrap();
    fs::write(vault.join("delete.md"), b"delete\n").unwrap();
    fs::write(vault.join("binary.png"), [0u8, 1, 2, 3, 4]).unwrap();
    fs::write(vault.join("untouched.md"), b"keep\n").unwrap();
    // 该测试二进制只包含这一个用例；环境变量在构造 Core 前设置。
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
    assert!(session.usable);
    assert!(session.snapshot_at_ms >= session.created_at_ms);

    fs::write(vault.join("edit.md"), b"alpha\nBETA\n").unwrap();
    fs::rename(vault.join("rename.md"), vault.join("renamed.md")).unwrap();
    fs::remove_file(vault.join("delete.md")).unwrap();
    fs::write(vault.join("added.md"), b"added\n").unwrap();
    fs::write(vault.join("binary.png"), [0u8, 1, 2, 3, 4, 5]).unwrap();

    let cancellation = ReviewCancellation::new();
    let summary = core
        .review_agent_session(session.id.clone(), cancellation)
        .unwrap();
    assert_eq!(summary.session_id, session.id);
    assert_eq!(summary.counts.added, 1);
    assert_eq!(summary.counts.modified, 2);
    assert_eq!(summary.counts.deleted, 1);
    assert_eq!(summary.counts.renamed, 1);
    assert!(
        !summary
            .entries
            .iter()
            .any(|entry| entry.id == "untouched.md")
    );
    let renamed = summary
        .entries
        .iter()
        .find(|entry| entry.id == "renamed.md")
        .unwrap();
    assert_eq!(renamed.change, "renamed");
    assert_eq!(renamed.old_path.as_deref(), Some("rename.md"));
    assert!(renamed.content_identical);
    let binary = summary
        .entries
        .iter()
        .find(|entry| entry.id == "binary.png")
        .unwrap();
    assert_eq!(binary.change, "modified");
    assert_eq!(binary.before.as_ref().unwrap().size, 5);

    let text = core
        .review_agent_session_entry(
            session.id.clone(),
            "edit.md".into(),
            None,
            ReviewCancellation::new(),
        )
        .unwrap();
    assert!(text.text && !text.binary);
    let removed = text.hunks[0]
        .lines
        .iter()
        .find(|line| line.text == "beta")
        .unwrap();
    assert_eq!(removed.kind, "removed");
    assert_eq!(removed.old_line, Some(2));

    let binary = core
        .review_agent_session_entry(
            session.id.clone(),
            "binary.png".into(),
            None,
            ReviewCancellation::new(),
        )
        .unwrap();
    assert!(!binary.text && binary.binary);
    assert!(binary.hunks.is_empty());

    let renamed = core
        .review_agent_session_entry(
            session.id.clone(),
            "renamed.md".into(),
            Some("rename.md".into()),
            ReviewCancellation::new(),
        )
        .unwrap();
    assert!(renamed.content_identical);
    assert_eq!(renamed.entry.change, "renamed");

    // 路径穿越与绝对路径都拒绝，错误键稳定。
    for path in ["../outside.md", "/etc/passwd", "a/../b"] {
        let error = core
            .review_agent_session_entry(
                session.id.clone(),
                path.into(),
                None,
                ReviewCancellation::new(),
            )
            .unwrap_err();
        assert!(
            format!("{error}").contains("history.invalid_path"),
            "{path}: {error}"
        );
    }

    // 取消令牌在开始前生效时返回稳定错误。
    let cancelled = ReviewCancellation::new();
    cancelled.cancel();
    let error = core
        .review_agent_session(session.id.clone(), cancelled)
        .unwrap_err();
    assert!(format!("{error}").contains("history.cancelled"));

    let sessions = core
        .list_agent_sessions(Some(vault.to_string_lossy().into_owned()))
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].usable);
}

#[test]
fn preparing_sessions_are_listed_but_not_reviewable() {
    let root = tempfile::TempDir::new().unwrap();
    let data = root.path().join("data");
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::create_dir_all(data.join("agent-sessions/11111111-1111-4111-8111-111111111111/snapshot"))
        .unwrap();
    fs::write(
        data.join("agent-sessions/11111111-1111-4111-8111-111111111111/session.json"),
        r#"{"format":1,"id":"11111111-1111-4111-8111-111111111111","vault":"/tmp/vault","created_at_ms":1,"state":"preparing","snapshot":"snapshot"}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("HORTUS_CONFIG_DIR", root.path().join("config"));
        std::env::set_var("HORTUS_CACHE_DIR", root.path().join("cache"));
        std::env::set_var("HORTUS_DATA_DIR", &data);
    }
    let core = WorkspaceCore::new().unwrap();
    let sessions = core.list_agent_sessions(None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].usable);
    assert_eq!(sessions[0].state, "preparing");
    let error = core
        .review_agent_session(sessions[0].id.clone(), ReviewCancellation::new())
        .unwrap_err();
    assert!(format!("{error}").contains("history.session_unusable"));
}
