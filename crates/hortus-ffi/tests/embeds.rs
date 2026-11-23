use hortus_ffi::{VaultHandle, WorkspaceCore};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn fixture(notes: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf, Arc<VaultHandle>) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("vault");
    fs::create_dir(&root).unwrap();
    for (path, text) in notes {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let string = |path: &Path| path.to_str().unwrap().to_owned();
    let core = WorkspaceCore::in_directories(
        string(&temporary.path().join("config")),
        string(&temporary.path().join("cache")),
    )
    .unwrap();
    let handle = core.open_vault(string(&root)).unwrap();
    handle.update_search_index(
        notes.iter().map(|(path, _)| (*path).into()).collect(),
        vec![],
    );
    (temporary, root, handle)
}

const TARGET: &str = concat!(
    "---\n",
    "aliases: [Blueprint]\ntags: [system]\n",
    "---\n",
    "# Title\n\n",
    "Intro paragraph with [[root]] and #system.\n\n",
    "## Section Two\n\n",
    "Section body with `code` and $x$.\n\n",
    "```text\n# fake heading\n```\n\n",
    "| A | B |\n|---|---|\n| 1 | 2 |\n\n",
    "### Nested\n\n",
    "Nested body.\n\n",
    "## Section Three\n\n",
    "Later body.\n",
);

#[test]
fn full_note_embed_strips_frontmatter_and_prepares_blocks() {
    let image = b"\x89PNG\r\n\x1a\n".to_vec();
    let (_temporary, root, handle) = fixture(&[
        ("design.md", TARGET.as_bytes()),
        ("pics/diagram.png", &image),
        ("probe.md", b""),
    ]);
    let before = fs::read(root.join("design.md")).unwrap();
    let embed = handle
        .resolve_embed(
            "probe.md".into(),
            "design".into(),
            None,
            None,
            vec!["probe.md".into()],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(embed.kind, "note");
    assert_eq!(embed.resolved_path.as_deref(), Some("design.md"));
    assert_eq!(embed.display_title, "design");
    assert!(embed.heading_found && !embed.is_mmd);
    assert!(!embed.content.contains("aliases:"));
    assert!(embed.content.starts_with("# Title"));
    assert!(embed.content.contains("Section Three"));
    assert!(!embed.blocks.is_empty());
    assert!(embed.text.contains("Section body"));
    assert!(!embed.math.is_empty());
    assert!(embed.media.iter().any(|block| !block.columns.is_empty()));
    assert_eq!(
        embed
            .links
            .iter()
            .map(|link| &*link.target)
            .collect::<Vec<_>>(),
        vec!["root"]
    );
    assert_eq!(embed.tags.len(), 1);
    assert_eq!(embed.tags[0].tag, "system");
    assert_eq!(fs::read(root.join("design.md")).unwrap(), before);

    let nested = handle
        .resolve_embed(
            "probe.md".into(),
            "design".into(),
            None,
            None,
            vec!["probe.md".into()],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(nested.content, embed.content);
    handle.close();
}

#[test]
fn heading_embed_slices_one_section_and_reports_missing() {
    let (_temporary, _root, handle) =
        fixture(&[("design.md", TARGET.as_bytes()), ("probe.md", b"")]);
    let embed = handle
        .resolve_embed(
            "probe.md".into(),
            "design".into(),
            Some("  ## section TWO ".into()),
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(embed.kind, "note");
    assert!(embed.heading_found);
    assert!(embed.content.starts_with("## Section Two"));
    assert!(embed.content.contains("### Nested"));
    assert!(!embed.content.contains("## Section Three"));
    assert!(!embed.content.contains("fake heading\n## Section Three"));
    let missing = handle
        .resolve_embed(
            "probe.md".into(),
            "design".into(),
            Some("No Such Heading".into()),
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(missing.kind, "heading_missing");
    assert!(!missing.heading_found);
    assert!(missing.content.is_empty());
    handle.close();
}

#[test]
fn mmd_alias_cycle_depth_and_broken_states_are_distinct() {
    let (_temporary, _root, handle) = fixture(&[
        ("arch.mmd", b"flowchart LR\nA --> B\n"),
        ("design.md", TARGET.as_bytes()),
        ("a.md", b"![[b]]"),
        ("b.md", b"![[a]]"),
        ("c.md", b""),
        ("x/Same.md", b""),
        ("y/Same.md", b""),
    ]);
    let mmd = handle
        .resolve_embed(
            "c.md".into(),
            "arch.mmd".into(),
            None,
            Some("Architecture".into()),
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(mmd.kind, "diagram");
    assert!(mmd.is_mmd);
    assert_eq!(mmd.display_title, "Architecture");
    assert_eq!(mmd.content, "flowchart LR\nA --> B\n");
    assert_eq!(mmd.diagrams.len(), 1);
    let alias = handle
        .resolve_embed(
            "c.md".into(),
            "Blueprint".into(),
            None,
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(alias.kind, "note");
    assert_eq!(alias.resolved_path.as_deref(), Some("design.md"));
    let cycle = handle
        .resolve_embed(
            "a.md".into(),
            "b".into(),
            None,
            None,
            vec!["a.md".into(), "b.md".into()],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(cycle.kind, "cycle");
    let self_cycle = handle
        .resolve_embed(
            "a.md".into(),
            "a".into(),
            None,
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(self_cycle.kind, "cycle");
    let deep = handle
        .resolve_embed(
            "c.md".into(),
            "design".into(),
            None,
            None,
            vec!["a.md".into(), "b.md".into(), "c.md".into(), "d.md".into()],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(deep.kind, "depth_exceeded");
    let allowed = handle
        .resolve_embed(
            "c.md".into(),
            "design".into(),
            None,
            None,
            vec!["a.md".into(), "b.md".into(), "c.md".into()],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(allowed.kind, "note");
    let ambiguous = handle
        .resolve_embed(
            "c.md".into(),
            "Same".into(),
            None,
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(ambiguous.kind, "ambiguous");
    assert_eq!(ambiguous.candidates, vec!["x/Same.md", "y/Same.md"]);
    let broken = handle
        .resolve_embed(
            "c.md".into(),
            "missing".into(),
            None,
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(broken.kind, "broken");
    handle.close();
}

#[test]
fn folder_rename_plan_and_apply_update_inside_and_outside_notes() {
    let outside = "\u{feff}Links\r\n[[docs/intro]] [[docs/sub/api#Auth|API]]\r\n![[docs/arch.mmd]] [[intro]] [[docs-extra/keep]]\r\n";
    let (_temporary, root, handle) = fixture(&[
        ("docs/intro.md", b"[[docs/sub/api]]\n[[intro]]\n"),
        ("docs/sub/api.md", b"[[docs/intro]]\n"),
        ("docs/arch.mmd", b"flowchart LR\nA --> B\n"),
        ("docs-extra/keep.md", b"# Keep\n"),
        ("outside.md", outside.as_bytes()),
        ("dirty.md", b"[[docs/intro]]"),
        ("unrelated.md", b"[[docs-extra/keep]]\n"),
    ]);
    let plan = handle.plan_folder_rename_links("docs".into(), "papers".into());
    assert_eq!(plan.total_rewrites, 6);
    assert_eq!(plan.affected_notes, 4);
    let outside_edits = plan
        .files
        .iter()
        .find(|file| file.source_path == "outside.md")
        .unwrap();
    assert_eq!(
        outside_edits
            .edits
            .iter()
            .map(|edit| (&*edit.before, &*edit.after))
            .collect::<Vec<_>>(),
        vec![
            ("[[docs/intro]]", "[[papers/intro]]"),
            ("[[docs/sub/api#Auth|API]]", "[[papers/sub/api#Auth|API]]"),
            ("![[docs/arch.mmd]]", "![[papers/arch.mmd]]"),
        ]
    );
    assert!(
        !plan
            .files
            .iter()
            .any(|file| file.source_path == "docs-extra/keep.md"
                || file.source_path == "unrelated.md")
    );
    assert!(
        handle
            .apply_folder_rename_links("docs".into(), "papers".into(), vec![])
            .is_err()
    );
    handle.rename_entry("docs".into(), "papers".into()).unwrap();
    let result = handle
        .apply_folder_rename_links("docs".into(), "papers".into(), vec!["dirty.md".into()])
        .unwrap();
    assert_eq!(result.total_rewrites, 5);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].path, "dirty.md");
    assert!(result.failed.is_empty());
    assert!(result.updated.contains(&"outside.md".into()));
    assert!(result.updated.contains(&"papers/intro.md".into()));
    assert!(result.updated.contains(&"papers/sub/api.md".into()));
    assert_eq!(fs::read_dir(root.join("papers")).unwrap().count(), 3);
    let bytes = fs::read(root.join("outside.md")).unwrap();
    assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    assert_eq!(
        String::from_utf8(bytes[3..].to_vec()).unwrap(),
        outside
            .trim_start_matches('\u{feff}')
            .replace("docs/intro", "papers/intro")
            .replace("docs/sub/api", "papers/sub/api")
            .replace("docs/arch.mmd", "papers/arch.mmd")
    );
    assert_eq!(
        fs::read_to_string(root.join("papers/intro.md")).unwrap(),
        "[[papers/sub/api]]\n[[intro]]\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("papers/sub/api.md")).unwrap(),
        "[[papers/intro]]\n"
    );
    assert_eq!(fs::read(root.join("dirty.md")).unwrap(), b"[[docs/intro]]");
    assert_eq!(
        fs::read(root.join("docs-extra/keep.md")).unwrap(),
        b"# Keep\n"
    );
    assert_eq!(
        fs::read(root.join("unrelated.md")).unwrap(),
        b"[[docs-extra/keep]]\n"
    );
    assert_eq!(
        handle
            .resolve_link("outside.md".into(), "papers/intro".into())
            .resolved_path
            .as_deref(),
        Some("papers/intro.md")
    );
    assert_eq!(handle.backlinks("papers/intro.md".into()).len(), 5);
    assert!(
        handle
            .apply_folder_rename_links("docs".into(), "papers".into(), vec![])
            .is_err()
    );
    assert!(
        handle
            .apply_folder_rename_links("../outside".into(), "papers".into(), vec![])
            .is_err()
    );
    handle.close();
}

#[test]
fn embed_resolution_never_writes_to_the_vault() {
    let (_temporary, root, handle) = fixture(&[
        ("design.md", TARGET.as_bytes()),
        ("arch.mmd", b"flowchart LR\nA --> B\n"),
        (
            "probe.md",
            b"![[design]] ![[design#Section Two]] ![[arch.mmd]]",
        ),
    ]);
    let before = fs::read(root.join("design.md")).unwrap();
    let archive = fs::read(root.join("arch.mmd")).unwrap();
    for (heading, target) in [
        (None, "design"),
        (Some("Section Two"), "design"),
        (None, "arch.mmd"),
        (Some("Missing"), "design"),
        (None, "absent"),
    ] {
        handle
            .resolve_embed(
                "probe.md".into(),
                target.into(),
                heading.map(str::to_owned),
                None,
                vec!["probe.md".into()],
                3,
                640.0,
                15.0,
            )
            .unwrap();
    }
    assert_eq!(fs::read(root.join("design.md")).unwrap(), before);
    assert_eq!(fs::read(root.join("arch.mmd")).unwrap(), archive);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
    handle.close();
}

#[test]
fn embed_cycles_use_exact_normalized_paths() {
    let (_temporary, _root, handle) = fixture(&[("a.md", b"![[b]]"), ("b.md", b"![[a]]")]);
    let resolve = |source: &str, target: &str, ancestors| {
        handle
            .resolve_embed(
                source.into(),
                target.into(),
                None,
                None,
                ancestors,
                3,
                700.0,
                15.0,
            )
            .unwrap()
    };
    // Supply the source spelling directly to simulate a case-sensitive vault.
    let distinct = resolve("./A.md", "a.md", vec![]);
    assert_eq!(distinct.resolved_path.as_deref(), Some("a.md"));
    assert_eq!(distinct.kind, "note");
    assert_eq!(resolve("a.md", "b", vec![]).kind, "note");
    assert_eq!(resolve("b.md", "a", vec!["/./a.md".into()]).kind, "cycle");
    assert_eq!(resolve("./a.md", "A.md", vec![]).kind, "cycle");
    assert_eq!(resolve("./a.md", "", vec![]).kind, "cycle");
    handle.close();
}

#[test]
fn embed_block_and_run_utf16_offsets_match_nested_multiblock_content() {
    let raw = concat!(
        "# 图解🧭\r\n\r\n",
        "正文 **粗体😀** 和 [[base|引述🚀]]。\r\n\r\n",
        "> 引用🪶\r\n>\r\n> - 内层 **文字🎈**\r\n",
        ">   - 次层 \u{60}代码🔎\u{60}\r\n\r\n",
        "\u{60}\u{60}\u{60}mermaid\r\nflowchart TB\r\nA --> B\r\n\u{60}\u{60}\u{60}\r\n\r\n",
        "结尾 ![[base]]。\r\n",
    );
    let (_temporary, _root, handle) =
        fixture(&[("base.md", b"Base"), ("blocks.md", raw.as_bytes())]);
    let result = handle
        .resolve_embed(
            "base.md".into(),
            "blocks".into(),
            None,
            None,
            vec![],
            3,
            700.0,
            15.0,
        )
        .unwrap();
    assert_eq!(result.kind, "note");
    let normalized = raw.replace("\r\n", "\n");
    assert_eq!(result.content, normalized);
    let model = hortus_document::Document::new(&normalized)
        .reading(false)
        .unwrap();
    assert!(model.blocks.len() >= 5);
    assert_eq!(result.blocks.len(), model.blocks.len());
    let utf16 = |s: &str| s.encode_utf16().count() as u64;
    let mut display_start = 0;
    let mut run_count = 0;
    let mut found_bold = false;
    for (actual, block) in result.blocks.iter().zip(&model.blocks) {
        let start = actual.source_start_byte as usize;
        let end = start + actual.source_length_byte as usize;
        let body = &normalized[start..end];
        let body_utf16 = body.encode_utf16().collect::<Vec<_>>();
        assert_eq!(actual.source_start_utf16, utf16(&normalized[..start]));
        assert_eq!(actual.source_length_utf16, utf16(body));
        assert_eq!(actual.source_start_utf16, block.source.start as u64);
        assert_eq!(actual.source_length_utf16, block.source.len() as u64);
        assert_eq!(actual.display_start, display_start);
        assert_eq!(actual.display_length, utf16(&block.content.text));
        assert_eq!(actual.runs.len(), block.content.runs.len());
        for (actual, run) in actual.runs.iter().zip(&block.content.runs) {
            assert_eq!(actual.start, run.display.start as u64);
            assert_eq!(actual.length, run.display.len() as u64);
            assert_eq!(actual.source_start_utf16, run.source.start as u64);
            assert_eq!(actual.source_length_utf16, run.source.len() as u64);
            assert_eq!(actual.mask, run.mask);
            let start = actual.source_start_utf16 as usize;
            let end = start + actual.source_length_utf16 as usize;
            let mapped = String::from_utf16(&body_utf16[start..end]).unwrap();
            if mapped == "粗体😀" {
                assert_eq!(actual.source_start_utf16, utf16("正文 **"));
                assert_eq!(actual.source_length_utf16, 4);
                found_bold = true;
            }
            run_count += 1;
        }
        display_start += actual.display_length;
    }
    assert!(found_bold && run_count > model.blocks.len());
    assert_eq!(display_start, utf16(&result.text));
    handle.close();
}

#[test]
fn folder_rename_rewrites_attachment_links_without_moving_any_notes() {
    let (_temporary, root, handle) = fixture(&[(
        "base.md",
        b"![[./media/picture.png]] [[/media/nested/spec.pdf#page|Spec]]",
    )]);
    fs::create_dir_all(root.join("media/nested")).unwrap();
    fs::write(root.join("media/picture.png"), b"example image").unwrap();
    fs::write(root.join("media/nested/spec.pdf"), b"example document").unwrap();
    let plan = handle.plan_folder_rename_links("media".into(), "assets".into());
    assert_eq!(plan.total_rewrites, 2);
    fs::rename(root.join("media"), root.join("assets")).unwrap();
    let result = handle
        .apply_folder_rename_links("media".into(), "assets".into(), vec![])
        .unwrap();
    assert!(result.failed.is_empty());
    assert_eq!(result.total_rewrites, 2);
    assert_eq!(
        fs::read_to_string(root.join("base.md")).unwrap(),
        "![[assets/picture.png]] [[/assets/nested/spec.pdf#page|Spec]]",
    );
    handle.close();
}
