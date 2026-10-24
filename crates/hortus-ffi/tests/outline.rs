use hortus_ffi::WorkspaceCore;
use std::fs;

#[test]
fn outline_uses_current_session_and_file_type() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::create_dir(root.join("vault")).unwrap();
    let core = WorkspaceCore::in_directories(
        root.join("config").to_string_lossy().into(),
        root.join("cache").to_string_lossy().into(),
    )
    .unwrap();
    for path in ["note.md", "note.MMD"] {
        fs::write(root.join("vault").join(path), "# Heading😀\n").unwrap();
        let session = core
            .open_document(root.join("vault").to_string_lossy().into(), path.into())
            .unwrap();
        let initial = session.outline().unwrap();
        assert_eq!(initial.revision, 0);
        assert_eq!(initial.headings.len(), usize::from(path.ends_with(".md")));
        session.replace(0, 0, "## Draft\n\n".into()).unwrap();
        let changed = session.outline().unwrap();
        assert_eq!(changed.revision, 1);
        if path.ends_with(".md") {
            assert_eq!(changed.headings.len(), 2);
            assert_eq!(changed.headings[1].start, 10);
        } else {
            assert!(changed.headings.is_empty());
        }
        assert_eq!(
            fs::read_to_string(root.join("vault").join(path)).unwrap(),
            "# Heading😀\n"
        );
    }
}
