use hortus_ffi::WorkspaceCore;
use std::fs;

#[test]
fn focus_state_round_trips_across_restarts_and_keeps_collapsed_nodes() {
    let temporary = tempfile::tempdir().unwrap();
    let vault = temporary.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("note.md"), "# Note\n").unwrap();
    let root = vault.to_string_lossy().into_owned();
    let config = temporary.path().join("config");
    let cache = temporary.path().join("cache");

    let core = WorkspaceCore::in_directories(
        config.to_string_lossy().into(),
        cache.to_string_lossy().into(),
    )
    .unwrap();
    assert!(
        core.diagram_focus(root.clone(), "note.md".into(), 1)
            .unwrap()
            .is_none()
    );

    core.remember_diagram(root.clone(), "note.md".into(), 1, vec!["A".into()])
        .unwrap();
    core.remember_diagram_focus(root.clone(), "note.md".into(), 1, 2.5, -30.0, 40.0)
        .unwrap();
    let focus = core
        .diagram_focus(root.clone(), "note.md".into(), 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        (focus.scale, focus.offset_x, focus.offset_y),
        (2.5, -30.0, 40.0)
    );
    assert_eq!(
        core.collapsed_nodes(root.clone(), "note.md".into(), 1)
            .unwrap(),
        vec!["A"]
    );

    // 折叠更新不能抹掉同一图块的专注视图位置。
    core.remember_diagram(
        root.clone(),
        "note.md".into(),
        1,
        vec!["A".into(), "B".into()],
    )
    .unwrap();
    let focus = core
        .diagram_focus(root.clone(), "note.md".into(), 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        (focus.scale, focus.offset_x, focus.offset_y),
        (2.5, -30.0, 40.0)
    );
    drop(core);

    let restarted = WorkspaceCore::in_directories(
        config.to_string_lossy().into(),
        cache.to_string_lossy().into(),
    )
    .unwrap();
    let focus = restarted
        .diagram_focus(root.clone(), "note.md".into(), 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        (focus.scale, focus.offset_x, focus.offset_y),
        (2.5, -30.0, 40.0)
    );
    assert_eq!(
        restarted
            .collapsed_nodes(root, "note.md".into(), 1)
            .unwrap(),
        vec!["A", "B"]
    );
    assert_eq!(
        fs::read_dir(&temporary.path().join("vault"))
            .unwrap()
            .count(),
        1,
        "focus state must never be written into the vault"
    );
}
