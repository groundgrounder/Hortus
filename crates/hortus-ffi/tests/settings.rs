//! 设置界面 FFI 边界：保注释保存、冲突保护、Agent 探测与指南管理。
use hortus_ffi::{EditableConfigInfo, EditableConfigInputInfo, WorkspaceCore};
use std::fs;

fn input(font_size: f64, default_view: &str) -> EditableConfigInputInfo {
    EditableConfigInputInfo {
        font_size,
        default_view: default_view.into(),
        terminal_font: None,
        agent_command: None,
        agent_args: vec![],
    }
}

fn error_kind<T>(result: Result<T, hortus_ffi::CoreError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => format!("{error}"),
    }
}

#[test]
fn editable_config_round_trip_and_guide_management() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    fs::create_dir_all(&config).unwrap();
    unsafe {
        std::env::set_var("HORTUS_DATA_DIR", &data);
    }
    let core = WorkspaceCore::in_directories(
        config.to_string_lossy().into_owned(),
        cache.to_string_lossy().into_owned(),
    )
    .unwrap();
    let path = config.join("config.toml");
    fs::write(
        &path,
        "# 顶部注释 top\nfont_size = 15.0 # inline 字号\ndefault_view = \"reading\"\nunknown = 1\n\n[[vault]]\npath = \"/vault/a\"\n\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"printf '%s' {guide}\"]\n",
    )
    .unwrap();
    let before: EditableConfigInfo = core.editable_config().unwrap();
    assert_eq!(before.config_path, path.to_string_lossy());
    assert_eq!(before.digest.len(), 64);
    assert_eq!(before.font_size, 15.0);
    assert_eq!(before.default_view, "reading");
    assert_eq!(before.terminal_font, None);
    assert!(before.agent_configured);
    assert_eq!(before.agent_command.as_deref(), Some("/bin/sh"));
    assert_eq!(
        before.agent_args,
        vec!["-c".to_string(), "printf '%s' {guide}".to_string()]
    );
    assert!(before.diagnostics.is_empty(), "{:?}", before.diagnostics);

    let saved = core
        .save_editable_config(
            EditableConfigInputInfo {
                font_size: 19.0,
                default_view: "editing".into(),
                terminal_font: Some("Menlo".into()),
                agent_command: Some("/bin/sh".into()),
                agent_args: vec!["{guide}".into()],
            },
            Some(before.digest.clone()),
        )
        .unwrap();
    assert_eq!(saved.font_size, 19.0);
    assert_eq!(saved.default_view, "editing");
    assert_eq!(saved.terminal_font.as_deref(), Some("Menlo"));
    assert_eq!(core.terminal_font().unwrap().as_deref(), Some("Menlo"));
    let text = fs::read_to_string(&path).unwrap();
    for kept in [
        "# 顶部注释 top",
        "# inline 字号",
        "unknown = 1",
        "[[vault]]",
        "path = \"/vault/a\"",
    ] {
        assert!(text.contains(kept), "missing {kept:?} in:\n{text}");
    }
    assert!(text.contains("font_size = 19.0"), "{text}");

    // 打开后磁盘又被手改：旧指纹拒绝覆盖，原手改保留。
    fs::write(&path, format!("# external\n{text}")).unwrap();
    assert_eq!(
        error_kind(core.save_editable_config(input(20.0, "reading"), Some(saved.digest))),
        "config.externally_modified"
    );
    assert!(fs::read_to_string(&path).unwrap().starts_with("# external"));
    let reloaded = core.editable_config().unwrap();
    let cleared = core
        .save_editable_config(input(20.0, "reading"), Some(reloaded.digest))
        .unwrap();
    assert_eq!(cleared.terminal_font, None);
    assert!(!cleared.agent_configured);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("terminal_font"), "{text}");
    assert!(!text.contains("[agent]"), "{text}");

    for (font_size, view, expected) in [
        (8.9, "reading", "config.font_size_invalid"),
        (40.1, "reading", "config.font_size_invalid"),
        (15.0, "sideways", "config.default_view_invalid"),
    ] {
        let current = core.editable_config().unwrap();
        assert_eq!(
            error_kind(core.save_editable_config(input(font_size, view), Some(current.digest))),
            expected
        );
    }

    let probe = core.probe_agent_launch("/bin/sh".into(), vec!["{guide}".into()]);
    assert!(probe.available);
    assert!(probe.executable.unwrap().ends_with("/sh"));
    assert_eq!(probe.error, None);
    let missing = core.probe_agent_launch("/definitely/missing".into(), vec!["{guide}".into()]);
    assert!(!missing.available);
    assert_eq!(missing.error.as_deref(), Some("agent.not_found"));
    let relative = core.probe_agent_launch("relative/tool".into(), vec!["{guide}".into()]);
    assert_eq!(
        relative.error.as_deref(),
        Some("agent.command_not_absolute")
    );

    let guide = core.agent_guide_state().unwrap();
    assert!(!guide.overridden);
    assert_eq!(guide.text, guide.default_text);
    let custom = "custom guide\nsecond\n";
    let saved_guide = core.save_agent_guide(custom.into()).unwrap();
    assert!(saved_guide.overridden);
    assert_eq!(saved_guide.text, custom);
    assert_eq!(
        core.save_agent_guide(" \n".into()).unwrap_err().to_string(),
        "guide.empty"
    );
    let reset = core.reset_agent_guide().unwrap();
    assert!(!reset.overridden);
    assert_eq!(reset.text, reset.default_text);
    assert!(!config.join("agent-guide.md").exists());
    assert!(!core.reset_agent_guide().unwrap().overridden);
}
