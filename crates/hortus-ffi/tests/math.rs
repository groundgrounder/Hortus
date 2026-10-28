use hortus_ffi::{MathEngine, MathFailure, MathGlyphMetrics, MathStyle, WorkspaceCore};

#[test]
fn scene_packet_has_version_length_metrics_and_isolated_errors() {
    let engine = MathEngine::new();
    assert!(matches!(
        engine.render("\\unknowncommand".into(), MathStyle::Display),
        Err(MathFailure::Parse { .. })
    ));
    assert!(matches!(
        engine.render("x".repeat(16385), MathStyle::Display),
        Err(MathFailure::ResourceLimit { .. })
    ));
    let bytes = engine
        .render("\\text{中😀}".into(), MathStyle::Text)
        .unwrap();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 1);
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
        bytes.len() - 8
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes[8..]).unwrap();
    assert_eq!(value["missing_metrics"].as_array().unwrap().len(), 2);
    engine
        .set_metrics(vec![
            MathGlyphMetrics {
                codepoint: '中' as u32,
                width: 1.,
                height: 0.9,
                depth: 0.1,
            },
            MathGlyphMetrics {
                codepoint: '😀' as u32,
                width: 1.1,
                height: 1.,
                depth: 0.2,
            },
        ])
        .unwrap();
    let bytes = engine
        .render("\\text{中😀}".into(), MathStyle::Text)
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes[8..]).unwrap();
    assert!(value["missing_metrics"].as_array().unwrap().is_empty());
    assert!(
        engine
            .set_metrics(vec![MathGlyphMetrics {
                codepoint: 0xd800,
                width: 1.,
                height: 1.,
                depth: 0.
            }])
            .is_err()
    );
    assert!(engine.render("x".into(), MathStyle::Display).is_ok());
}

#[test]
fn metadata_uses_reading_revision_unicode_ranges_and_file_type() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let core = WorkspaceCore::in_directories(
        temp.path().join("config").to_string_lossy().into(),
        temp.path().join("cache").to_string_lossy().into(),
    )
    .unwrap();
    for path in ["note.md", "note.mmd"] {
        let source = "前😀\n\n$$x^2$$\n\n后 $a_1 b_1$\n";
        std::fs::write(vault.join(path), source).unwrap();
        let session = core
            .open_document(vault.to_string_lossy().into(), path.into())
            .unwrap();
        let first = session.math_blocks().unwrap();
        if path.ends_with(".mmd") {
            assert!(first.is_empty());
            continue;
        }
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].source_start, 5);
        assert_eq!(first[0].source_length, 7);
        assert!(first[0].block);
        assert_eq!(first[0].display_length, 1);
        assert!(!first[1].block);
        session.replace(7, 1, "y".into()).unwrap();
        assert_eq!(session.math_blocks().unwrap()[0].latex, "y^2");
        assert_eq!(std::fs::read_to_string(vault.join(path)).unwrap(), source);
    }
}
