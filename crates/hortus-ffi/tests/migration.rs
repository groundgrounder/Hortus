use hortus_ffi::{DocumentIssue, WorkspaceCore};
use std::fs;
use std::process::Command;

const LEGACY_CONFIG_ENV: &str = "MARKNOTE_CONFIG_DIR";
const LEGACY_CACHE_ENV: &str = "MARKNOTE_CACHE_DIR";

#[test]
fn directory_migration_preserves_sessions_folds_and_restart_recovery_prompt() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path();
    let vault = base.join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("note.md"), "Original\n").unwrap();
    let root = vault.to_string_lossy().into_owned();
    let old_config = base.join("old-config");
    let old_cache = base.join("old-cache");
    let core = WorkspaceCore::in_directories(
        old_config.to_string_lossy().into(),
        old_cache.to_string_lossy().into(),
    )
    .unwrap();
    core.remember(root.clone(), vec!["note.md".into()], Some("note.md".into()))
        .unwrap();
    core.remember_diagram(root.clone(), "note.md".into(), 1, vec!["A".into()])
        .unwrap();
    let document = core.open_document(root, "note.md".into()).unwrap();
    document.replace(0, 0, "Draft ".into()).unwrap();
    document.checkpoint().unwrap();
    fs::write(old_config.join("config.toml"), "font_size = 18.0\n").unwrap();
    // 整体移动应连同未知配置、嵌套目录一起保留。
    fs::create_dir(old_config.join("themes")).unwrap();
    fs::write(old_config.join("themes/custom.toml"), "custom = true\n").unwrap();
    drop(document);
    drop(core);
    for restart in [false, true] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "migration_child"])
            .env("HORTUS_TEST_MIGRATION", &vault)
            .env("HORTUS_TEST_RESTART", if restart { "1" } else { "0" })
            .env("HORTUS_CONFIG_DIR", base.join("config"))
            .env("HORTUS_CACHE_DIR", base.join("cache"))
            .env(LEGACY_CONFIG_ENV, &old_config)
            .env(LEGACY_CACHE_ENV, &old_cache)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!old_config.exists() && !old_cache.exists());
        assert_eq!(
            fs::read_to_string(base.join("config/themes/custom.toml")).unwrap(),
            "custom = true\n"
        );
        assert_eq!(
            fs::read_dir(base.join("cache/recovery")).unwrap().count(),
            1
        );
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).unwrap(),
            "Original\n"
        );
    }
}

#[test]
fn migration_child() {
    let Some(vault) = std::env::var_os("HORTUS_TEST_MIGRATION") else {
        return;
    };
    let root = vault.to_string_lossy().into_owned();
    let core = WorkspaceCore::new().unwrap();
    assert_eq!(core.font_size().unwrap(), 18.0);
    let session: serde_json::Value = serde_json::from_str(&core.session_json().unwrap()).unwrap();
    assert_eq!(session["last_vault"], root);
    assert_eq!(session["recent"][0]["tabs"], serde_json::json!(["note.md"]));
    assert_eq!(session["recent"][0]["selected"], "note.md");
    assert_eq!(
        core.collapsed_nodes(root.clone(), "note.md".into(), 1)
            .unwrap(),
        ["A"]
    );
    let document = core.open_document(root, "note.md".into()).unwrap();
    assert_eq!(document.initial().unwrap().issue, DocumentIssue::Recovery);
    if std::env::var("HORTUS_TEST_RESTART").unwrap() == "1" {
        document.recover(true).unwrap();
        assert_eq!(document.snapshot().unwrap().text, "Draft Original\n");
    }
}
