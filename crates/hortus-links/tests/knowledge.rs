use hortus_links::{BrokenReason, KnowledgeIndex, LinkResolution, parse_note};

fn index(entries: &[(&str, &str)]) -> KnowledgeIndex {
    let mut index = KnowledgeIndex::new();
    for (path, text) in entries {
        index.upsert(*path, *text);
    }
    index
}

#[test]
fn parses_frontmatter_tags_aliases_source_and_link_forms() {
    let text = concat!(
        "---\r\n",
        "tags: [alpha, #alpha/beta]\r\n",
        "aliases:\r\n",
        "  - Complete note\r\n",
        "source: upstream/spec.md\r\n",
        "---\r\n",
        "# Title\r\n\r\n",
        "Body #inline #alpha/beta #dev_2026 #中文/索引\r\n",
        "[[Plain]] [[Target#Heading]] [[Target|Alias]] [[Target#Heading|Alias]]\r\n",
        "![[Target]] ![[picture.png]] [[dir/Other]] [[#Local heading]]\r\n",
        "`[[inline code]]` $[[inline math]]$ <!-- [[comment]] --> %% [[comment]] %%\r\n",
    );
    let note = parse_note(text);
    assert_eq!(
        note.tags
            .iter()
            .map(|tag| (tag.key.as_str(), tag.frontmatter))
            .collect::<Vec<_>>(),
        vec![
            ("alpha", true),
            ("alpha/beta", true),
            ("inline", false),
            ("dev_2026", false),
            ("中文/索引", false),
        ]
    );
    assert_eq!(note.aliases, vec!["Complete note"]);
    assert_eq!(note.source.as_deref(), Some("upstream/spec.md"));
    let targets = note
        .links
        .iter()
        .map(|link| (link.raw_target.as_str(), link.embed, link.is_attachment()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec![
            ("Plain", false, false),
            ("Target", false, false),
            ("Target", false, false),
            ("Target", false, false),
            ("Target", true, false),
            ("picture.png", true, true),
            ("dir/Other", false, false),
            ("", false, false),
        ]
    );
    assert_eq!(note.links[1].heading.as_deref(), Some("Heading"));
    assert_eq!(note.links[2].alias.as_deref(), Some("Alias"));
    assert_eq!(note.links[3].heading.as_deref(), Some("Heading"));
    assert_eq!(note.links[3].alias.as_deref(), Some("Alias"));
    assert!(note.links[4].embed);
    assert_eq!(note.links[5].alias, None);
    assert_eq!(note.links[7].heading.as_deref(), Some("Local heading"));
}

#[test]
fn resolution_follows_exact_path_stem_alias_order() {
    let index = index(&[
        ("guides/intro.md", "---\naliases: [Start]\n---\n"),
        ("notes/Intro.markdown", ""),
        ("deep/Stem.mmd", ""),
        (
            "a/Source.md",
            "[[guides/intro]] [[notes/Intro]] [[Stem]] [[Start]] [[intro]]",
        ),
    ]);
    let links = index.links("a/Source.md").unwrap();
    assert_eq!(
        links
            .iter()
            .map(|link| link.resolution.clone())
            .collect::<Vec<_>>(),
        vec![
            LinkResolution::Note("guides/intro.md".into()),
            LinkResolution::Note("notes/Intro.markdown".into()),
            LinkResolution::Note("deep/Stem.mmd".into()),
            LinkResolution::Note("guides/intro.md".into()),
            LinkResolution::Ambiguous(vec![
                "guides/intro.md".into(),
                "notes/Intro.markdown".into(),
            ]),
        ]
    );
}

#[test]
fn case_insensitive_matching_and_same_directory_disambiguation() {
    let index = index(&[
        ("Notes/Plan.MD", ""),
        ("Other/Plan.md", ""),
        ("Notes/Source.md", "[[Plan]] [[notes/plan.md]]"),
        ("Other/Source.md", "[[Plan]]"),
    ]);
    let same = index.links("Notes/Source.md").unwrap();
    assert_eq!(
        same[0].resolution,
        LinkResolution::Note("Notes/Plan.MD".into())
    );
    assert_eq!(
        same[1].resolution,
        LinkResolution::Note("Notes/Plan.MD".into())
    );
    let other = index.links("Other/Source.md").unwrap();
    assert_eq!(
        other[0].resolution,
        LinkResolution::Note("Other/Plan.md".into())
    );
    assert_eq!(
        index.broken_links().len(),
        0,
        "same-directory ties must resolve before ambiguity"
    );
}

#[test]
fn graph_queries_are_sorted_and_ignore_attachments() {
    let index = index(&[
        (
            "target.md",
            "---\ntags: [topic, topic/sub]\n---\n# Target\n",
        ),
        ("source.md", "[[target|Alias]] ![[image.png]] [[missing]]"),
        ("amb.md", "---\naliases: [Shared]\n---\n"),
        ("a/Other.md", "---\naliases: [Shared]\n---\n"),
        ("use.md", "[[Shared]]"),
        ("lonely.md", "# Lonely\n"),
    ]);
    let backlinks = index.backlinks("target.md").unwrap();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].source, "source.md");
    assert_eq!(backlinks[0].alias.as_deref(), Some("Alias"));
    assert_eq!(index.backlinks("image.png"), None);
    let broken = index.broken_links();
    assert_eq!(broken.len(), 2);
    assert_eq!(broken[0].source, "source.md");
    assert_eq!(broken[0].reason, BrokenReason::Broken);
    assert_eq!(broken[1].reason, BrokenReason::Ambiguous);
    assert_eq!(
        broken[1].candidates,
        vec!["a/Other.md".to_owned(), "amb.md".to_owned()]
    );
    assert_eq!(index.notes_with_tag("topic"), vec!["target.md".to_owned()]);
    assert_eq!(index.notes_with_tag("topic/sub"), vec!["target.md"]);
    assert_eq!(
        index.orphans(),
        vec![
            "a/Other.md".to_owned(),
            "amb.md".to_owned(),
            "lonely.md".to_owned(),
            "use.md".to_owned(),
        ]
    );
    let tags = index.tags();
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].tag, "topic");
    assert_eq!(tags[1].tag, "topic/sub");
}

#[test]
fn rename_plans_skip_alias_only_links_and_keep_every_part() {
    let index = index(&[
        ("notes/Old.md", "---\naliases: [Legacy, Old name]\n---\n"),
        (
            "source.md",
            "[[Old]] [[notes/Old#Part|Alias]] ![[Old#^block]] [[Legacy]] [[Old name]] [[Old.md]] [[image.png]]",
        ),
    ]);
    let plan = index.plan_rename_links("notes/Old.md", "archive/New.md");
    assert_eq!(plan.len(), 4);
    assert_eq!(plan[0].before, "[[Old]]");
    assert_eq!(plan[0].after, "[[New]]");
    assert_eq!(plan[1].before, "[[notes/Old#Part|Alias]]");
    assert_eq!(plan[1].after, "[[archive/New#Part|Alias]]");
    assert_eq!(plan[2].before, "![[Old#^block]]");
    assert_eq!(plan[2].after, "![[New#^block]]");
    assert_eq!(plan[3].before, "[[Old.md]]");
    assert_eq!(plan[3].after, "[[archive/New.md]]");
    assert!(index.plan_rename_links("missing.md", "x.md").is_empty());
    assert!(index.plan_rename_links("../Old.md", "x.md").is_empty());
    assert!(
        index
            .plan_rename_links("notes/Old.md", "notes/Old.md")
            .is_empty()
    );
}

#[test]
fn upsert_replaces_and_remove_clears_derived_indexes() {
    let mut index = index(&[
        ("a.md", "---\naliases: [Alpha]\n---\n[[b]]"),
        ("b.md", "text"),
    ]);
    assert_eq!(
        index.links("a.md").unwrap()[0].resolution,
        LinkResolution::Note("b.md".into())
    );
    index.upsert("a.md", "[[missing]]");
    assert_eq!(
        index.links("a.md").unwrap()[0].resolution,
        LinkResolution::Broken
    );
    assert!(
        index
            .broken_links()
            .iter()
            .any(|link| link.target == "missing")
    );
    index.remove("b.md");
    assert_eq!(index.len(), 1);
    assert!(index.backlinks("b.md").is_none());
    index.remove("never.md");
    assert_eq!(index.len(), 1);
}

#[test]
fn utf8_and_emoji_boundaries_are_safe_for_every_prefix() {
    let text = concat!(
        "---\n",
        "tags: [中文/索引, emoji😀]\n",
        "aliases: [中文别名]\n",
        "---\n",
        "# 标题 😀\n\n",
        "链接 [[笔记/主题#章节|别名😀]] 和 ![[图片😀.png]]\n",
        "```mermaid\nflowchart TB\nA[\"[[图笔记😀]]\"] --> B\n```\n",
        "行内 #标签/子级😀 和 `代码😀` $公式😀$\n",
        "%% 注释😀 [[隐藏]] %% <!-- 隐藏😀 [[隐藏]] -->\n",
        "孤立的中文😀文本\n",
    );
    for boundary in 0..=text.len() {
        if !text.is_char_boundary(boundary) {
            continue;
        }
        let prefix = &text[..boundary];
        let suffix = &text[boundary..];
        let note = parse_note(prefix);
        assert!(note.links.len() <= 3);
        let _ = parse_note(suffix);
        let mut index = KnowledgeIndex::new();
        index.upsert("笔记/主题.md", prefix);
        index.upsert("笔记/主题.md", text);
        let _ = index.tags();
        let _ = index.orphans();
        let _ = index.broken_links();
        let _ = index.plan_rename_links("笔记/主题.md", "笔记/新主题.md");
    }
    let note = parse_note(text);
    assert!(note.tags.iter().any(|tag| tag.key == "中文/索引"));
    assert!(note.links.iter().any(|link| link.in_diagram));
    assert!(note.links.iter().any(|link| link.is_attachment()));
}
