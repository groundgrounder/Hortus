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

fn substring(text: &str, start: u64, length: u64) -> String {
    let code: Vec<_> = text.encode_utf16().collect();
    String::from_utf16(&code[start as usize..(start + length) as usize]).unwrap()
}

#[test]
fn shared_queries_use_the_editor_projection_and_update_together() {
    let target = "---\naliases: [Blueprint]\ntags: [system]\n---\n## Section Two\n";
    let source = "中文🔬 ![[design#Section Two|Index]] #system/storage #system/storage\n\n```mermaid\nflowchart LR\nA[\"[[design]]\"] --> B\n```\n";
    let (_temporary, _root, handle) = fixture(&[
        ("design.md", target.as_bytes()),
        ("probe.md", source.as_bytes()),
        ("a/Duplicate.md", b""),
        ("b/Duplicate.md", b""),
        ("mods/worker.md", b""),
    ]);
    let backlinks = handle.backlinks("design.md".into());
    assert_eq!(backlinks.len(), 2);
    assert!(backlinks[0].embed && backlinks[1].in_diagram);
    assert_eq!(backlinks[0].start_utf16, 5);
    assert_eq!(
        substring(source, backlinks[0].start_utf16, backlinks[0].length_utf16),
        "![[design#Section Two|Index]]"
    );
    let links = handle.links("probe.md".into());
    assert_eq!(links[0].heading.as_deref(), Some("Section Two"));
    assert_eq!(links[0].resolved_path.as_deref(), Some("design.md"));
    assert_eq!(
        handle
            .knowledge_markup("probe.md".into(), source.into())
            .tags
            .len(),
        2
    );
    let tags = handle.tags();
    assert_eq!(
        tags.iter().map(|t| (&*t.tag, t.count)).collect::<Vec<_>>(),
        vec![("system", 1), ("system/storage", 1)]
    );
    assert_eq!(
        handle.notes_with_tag("#SYSTEM".into()),
        vec!["design.md", "probe.md"]
    );

    assert_eq!(
        handle
            .resolve_link("a/sender.md".into(), "Duplicate".into())
            .resolved_path
            .as_deref(),
        Some("a/Duplicate.md")
    );
    assert_eq!(
        handle
            .resolve_link("probe.md".into(), "Duplicate".into())
            .candidates
            .len(),
        2
    );
    assert_eq!(
        handle
            .resolve_link("probe.md".into(), "missing".into())
            .resolution_kind,
        "broken"
    );
    assert_eq!(
        handle
            .resolve_link("probe.md".into(), "chart.png".into())
            .resolution_kind,
        "attachment"
    );
    let alias = handle.link_completions("BLUE".into(), 5);
    assert_eq!(alias[0].insert_target, "design");
    assert_eq!(alias[0].matched_alias.as_deref(), Some("Blueprint"));
    let duplicates = handle.link_completions("Dup".into(), 4);
    assert_eq!(
        duplicates
            .iter()
            .map(|hit| &*hit.insert_target)
            .collect::<Vec<_>>(),
        vec!["a/Duplicate", "b/Duplicate"]
    );
    assert_eq!(
        handle.link_completions("MODS/WO".into(), 1)[0].path,
        "mods/worker.md"
    );
    assert!(
        handle
            .link_completions_at("`[[design]]`".into(), 1, "des".into(), 5)
            .is_empty()
    );
    assert_eq!(
        handle
            .link_completions_at("🔬 [[design]]".into(), 3, "des".into(), 5)
            .len(),
        1
    );

    handle.upsert_note_content("probe.md".into(), "#protocol fresh content".into());
    assert!(handle.backlinks("design.md".into()).is_empty());
    assert_eq!(handle.notes_with_tag("protocol".into()), vec!["probe.md"]);
    assert!(!handle.quick_open("probe".into(), 3).is_empty());
    handle.update_search_index(vec![], vec!["probe.md".into()]);
    assert!(handle.notes_with_tag("protocol".into()).is_empty());
    assert!(handle.quick_open("probe".into(), 3).is_empty());
    handle.close();
}

#[test]
fn container_fences_and_mermaid_comments_share_the_masking() {
    let source = concat!(
        "> ```rust\n",
        "> [[not-a-link]] #not-a-tag\n",
        "> ```\n\n",
        "> [!note]\n",
        "> ```mermaid\n",
        "> flowchart TB\n",
        "> %% ordinary comment\n",
        "> A[\"[[design]]\"] --> B\n",
        "> #mermaid-tag\n",
        "> ```\n\n",
        "```mermaid\n",
        "%% comment\n",
        "flowchart TB\n",
        "C[\"[[design]]\"] --> D\n",
        "```\n\n",
        "[[design]]\n",
    );
    let (_temporary, _root, handle) =
        fixture(&[("design.md", b""), ("probe.md", source.as_bytes())]);
    let links = handle.links("probe.md".into());
    assert_eq!(links.len(), 3);
    assert!(links[0].in_diagram && links[1].in_diagram);
    assert!(!links[2].in_diagram);
    assert_eq!(handle.tags().len(), 0);
    assert_eq!(handle.backlinks("design.md".into()).len(), 3);

    let code_at = source.find("[[not-a-link]]").unwrap() as u64;
    assert!(
        handle
            .link_completions_at(source.into(), code_at, "des".into(), 5)
            .is_empty()
    );
    let diagram_at = source.find("[[design]]").unwrap() as u64;
    assert!(
        handle
            .link_completions_at(source.into(), diagram_at, "des".into(), 5)
            .is_empty()
    );
    let outside_at = source.rfind("[[design]]").unwrap() as u64;
    assert_eq!(
        handle
            .link_completions_at(source.into(), outside_at, "des".into(), 5)
            .len(),
        1
    );
}

#[test]
fn malformed_inputs_do_not_poison_following_queries() {
    let (_temporary, _root, handle) = fixture(&[("a.md", b"[[b]] #tag"), ("b.md", b"")]);
    let _ = handle.knowledge_markup("missing.md".into(), "[[".into());
    let _ = handle.knowledge_markup("a.md".into(), "![[".into());
    let _ = handle.link_completions_at("[[".into(), u64::MAX, "x".into(), 0);
    let _ = handle.link_completions_at(String::new(), 0, "x".into(), 0);
    let _ = handle.resolve_link("a.md".into(), String::new());
    let _ = handle.resolve_link("a.md".into(), "../outside".into());
    let _ = handle.plan_rename_links("missing.md".into(), "x.md".into());
    assert_eq!(handle.backlinks("b.md".into()).len(), 1);
    assert_eq!(handle.tags().len(), 1);
    assert!(!handle.link_completions("b".into(), 5).is_empty());
}

#[test]
fn confirmed_rename_rewrites_exact_bytes_and_reports_skips() {
    let target = b"---\naliases: [Blueprint]\n---\n";
    let source = "\u{feff}中文🔬\r\n![[ design # Section Two | Index ]] and [[design.md]]\n[[Blueprint]]\r\n";
    let (temporary, root, handle) = fixture(&[
        ("design.md", target),
        ("probe.md", source.as_bytes()),
        ("dirty.md", b"[[design]]"),
        ("external.md", b"[[design]]"),
        ("link.md", b"[[design]]"),
        ("untouched.md", b"[[Blueprint]] [[external]]"),
    ]);
    let plan = handle.plan_rename_links("design.md".into(), "layout.markdown".into());
    assert_eq!(plan.total_rewrites, 5);
    assert_eq!(plan.affected_notes, 4);
    let edit = &plan
        .files
        .iter()
        .find(|file| file.source_path == "probe.md")
        .unwrap()
        .edits[0];
    assert_eq!(edit.start_utf16, 5);
    assert_eq!(edit.before, "![[ design # Section Two | Index ]]");
    assert_eq!(edit.after, "![[ layout # Section Two | Index ]]");
    assert!(
        handle
            .apply_rename_links("design.md".into(), "layout.markdown".into(), vec![])
            .is_err()
    );
    assert_eq!(fs::read(root.join("probe.md")).unwrap(), source.as_bytes());
    handle
        .rename_entry("design.md".into(), "layout.markdown".into())
        .unwrap();
    fs::write(root.join("external.md"), "[[design]]\nExternal edit").unwrap();
    let outside = temporary.path().join("outside.md");
    fs::write(&outside, "[[design]]").unwrap();
    #[cfg(unix)]
    {
        fs::remove_file(root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.md")).unwrap();
    }
    assert!(
        handle
            .apply_rename_links("../outside.md".into(), "layout.markdown".into(), vec![])
            .is_err()
    );
    assert!(
        handle
            .apply_rename_links("design.md".into(), "link.md".into(), vec![])
            .is_err()
    );
    let result = handle
        .apply_rename_links(
            "design.md".into(),
            "layout.markdown".into(),
            vec!["dirty.md".into()],
        )
        .unwrap();
    assert_eq!(result.updated, vec!["probe.md"]);
    assert_eq!(result.total_rewrites, 2);
    assert_eq!(result.skipped[0].path, "dirty.md");
    assert_eq!(result.failed[0].path, "external.md");
    assert_eq!(result.failed[1].path, "link.md");
    assert_eq!(
        fs::read(root.join("probe.md")).unwrap(),
        source
            .replace(" design #", " layout #")
            .replace("[[design.md]]", "[[layout.markdown]]")
            .as_bytes()
    );
    assert_eq!(fs::read(root.join("dirty.md")).unwrap(), b"[[design]]");
    assert_eq!(fs::read(outside).unwrap(), b"[[design]]");
    assert_eq!(
        fs::read(root.join("untouched.md")).unwrap(),
        b"[[Blueprint]] [[external]]"
    );
    assert_eq!(handle.backlinks("layout.markdown".into()).len(), 4);
    assert!(
        handle
            .apply_rename_links("design.md".into(), "layout.markdown".into(), vec![])
            .is_err()
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 6);
    handle.close();
}

#[test]
fn standalone_mermaid_markup_backlinks_and_rename_share_one_projection() {
    let graph =
        "stateDiagram-v2\r\nstate check [[choice]]\r\nstate \"[[Target|跳转]] #fake\" as T\r\n";
    let (_temporary, root, handle) = fixture(&[
        ("graph.mmd", graph.as_bytes()),
        ("Target.md", b"Target"),
        ("choice.md", b"Choice"),
    ]);
    let links = handle.links("graph.mmd".into());
    assert_eq!(links.len(), 1);
    assert!(links[0].in_diagram);
    let markup = handle.knowledge_markup("graph.mmd".into(), graph.into());
    assert_eq!(markup.links.len(), 1);
    assert!(markup.links[0].in_diagram && markup.tags.is_empty());
    assert!(handle.backlinks("choice.md".into()).is_empty());
    assert_eq!(handle.backlinks("Target.md".into())[0].source, "graph.mmd");
    let plan = handle.plan_rename_links("Target.md".into(), "Renamed.md".into());
    assert_eq!(plan.total_rewrites, 1);
    fs::rename(root.join("Target.md"), root.join("Renamed.md")).unwrap();
    let result = handle
        .apply_rename_links("Target.md".into(), "Renamed.md".into(), vec![])
        .unwrap();
    assert!(result.failed.is_empty());
    assert_eq!(result.total_rewrites, 1);
    assert_eq!(
        fs::read_to_string(root.join("graph.mmd")).unwrap(),
        graph.replace("[[Target|", "[[Renamed|"),
    );
    handle.close();
}
