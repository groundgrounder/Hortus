//! 强制完整复制路径同样不能绕过源文件可读性门禁。
use hortus_history::{HistoryStore, SessionState};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn unreadable_source_fails_the_snapshot_with_forced_full_copy() {
    // 该测试二进制只有一个用例，进程级开关不会影响其它测试。
    unsafe {
        std::env::set_var("HORTUS_HISTORY_COPY", "copy");
    }
    let root = tempfile::TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("readable.md"), "# readable\n").unwrap();
    let locked = vault.join("locked.md");
    fs::write(&locked, "secret\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let store = HistoryStore::new(root.path().join("data"));
    let error = store.begin(&vault, None).unwrap_err();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(error.to_string(), "history.source_unreadable");
    let sessions = store.list(None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, SessionState::Failed);
}
