//! Agent 启动模板与持久会话的 FFI 边界。
use hortus_ffi::WorkspaceCore;
use std::fs;

#[test]
fn agent_spec_sessions_and_recovery_round_trip() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("note.md"), "# Note\n").unwrap();
    fs::write(vault.join(".hidden"), "x").unwrap();
    // 该测试二进制只包含这一个用例；环境变量在构造 Core 前设置。
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("HORTUS_CONFIG_DIR", &config);
        std::env::set_var("HORTUS_CACHE_DIR", &cache);
        std::env::set_var("HORTUS_DATA_DIR", &data);
    }
    let core = WorkspaceCore::new().unwrap();

    let missing = core.agent_spec().unwrap_err();
    assert!(format!("{missing}").contains("agent.not_configured"));

    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("config.toml"),
        "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"printf '%s' {guide}\"]\n",
    )
    .unwrap();
    let spec = core.agent_spec().unwrap();
    assert_eq!(
        spec.executable,
        fs::canonicalize("/bin/sh")
            .unwrap()
            .to_string_lossy()
            .into_owned()
    );
    assert_eq!(spec.args.len(), 2);
    assert!(spec.args[1].starts_with("printf '%s' # Working in a Hortus"));

    assert_eq!(core.data_directory(), data.to_string_lossy());
    let first = core
        .begin_agent_session(vault.to_string_lossy().into_owned())
        .unwrap();
    assert_eq!(first.state, "ready");
    assert_eq!(first.entries, 2);
    assert!(
        std::path::Path::new(&first.snapshot_path)
            .join("note.md")
            .exists()
    );
    assert!(data.join("agent-sessions").exists());

    core.mark_agent_session(first.id.clone(), "running".into(), None)
        .unwrap();
    core.mark_agent_session(first.id.clone(), "exited".into(), Some(0))
        .unwrap();
    let sessions = core.list_agent_sessions(None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, "exited");
    assert!(sessions[0].started_at_ms.is_some() && sessions[0].ended_at_ms.is_some());

    let second = core
        .begin_agent_session(vault.to_string_lossy().into_owned())
        .unwrap();
    core.mark_agent_session(second.id.clone(), "running".into(), None)
        .unwrap();
    assert_eq!(core.recover_agent_sessions().unwrap(), 1);
    let recovered = core
        .list_agent_sessions(Some(vault.to_string_lossy().into_owned()))
        .unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].id, second.id);
    assert_eq!(recovered[0].state, "interrupted");
    assert_eq!(recovered[1].state, "exited");

    assert!(
        core.mark_agent_session(first.id.clone(), "bogus".into(), None)
            .is_err()
    );
    // 两次启动的目录和快照互不覆盖。
    assert_ne!(first.snapshot_path, second.snapshot_path);
    assert_eq!(
        fs::read(std::path::Path::new(&first.snapshot_path).join("note.md")).unwrap(),
        b"# Note\n"
    );
}
