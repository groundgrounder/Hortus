use hortus_document::{Document, reading};

#[test]
fn headings_share_reading_text_and_exclude_code_frontmatter_and_comments() {
    let source = "---\n# metadata\nname: value\n---\n\n# **粗体** `code` [链接](target) [[Note|别名]] ==高亮==\n\n标题😀\n===\n\n第二级\n---\n\n```markdown\n# fenced\n```\n\n    # indented\n\n%%\n# comment\n%%\n\n###### End\n";
    let mut document = Document::new(source);
    let headings = document.outline(false).unwrap().to_vec();
    assert_eq!(
        headings.iter().map(|h| h.level).collect::<Vec<_>>(),
        [1, 1, 2, 6]
    );
    assert_eq!(
        headings
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        ["粗体 code 链接 别名 高亮", "标题😀", "第二级", "End"]
    );
    let model = reading::Cache::default().model(source, false);
    for heading in &headings {
        let block = model
            .blocks
            .iter()
            .find(|b| b.source.contains(&heading.start))
            .unwrap();
        assert_eq!(block.content.text.trim(), heading.title);
    }
}

#[test]
fn crlf_and_unicode_offsets_follow_source_utf16() {
    let source =
        "前言😀\r\n\r\n# 中文😀\r\n\r\n正文🪴\r\n\r\nSetext **标题**\r\n---\r\n\r\n## 尾声\r\n";
    let mut document = Document::new(source);
    let headings = document.outline(false).unwrap();
    for (heading, marker) in headings.iter().zip(["# 中文", "Setext", "## 尾声"]) {
        let byte = source.find(marker).unwrap();
        assert_eq!(heading.start, source[..byte].encode_utf16().count());
    }
    assert_eq!(headings[1].title, "Setext 标题");
}

#[test]
fn inline_comments_do_not_split_headings_or_change_reading_titles() {
    let source = "---\r\nname: outline\r\n...\r\n\r\n# Before %%hidden%% **after**\r\n\r\nSetext %%hidden%% title\r\n---\r\n\r\n# [Reference][target]\r\n\r\n[target]: note.md\r\n";
    let mut document = Document::new(source);
    let headings = document.outline(false).unwrap();
    assert_eq!(headings.len(), 3);
    assert_eq!(headings[0].title, "Before  after");
    assert_eq!(headings[1].title, "Setext  title");
    assert_eq!(headings[2].title, "Reference");
    let model = reading::Cache::default().model(source, false);
    for heading in headings {
        let block = model
            .blocks
            .iter()
            .find(|b| b.source.contains(&heading.start))
            .unwrap();
        assert_eq!(
            block.content.text.lines().next().unwrap().trim(),
            heading.title
        );
    }
}

#[test]
fn heading_footnotes_keep_document_wide_reading_numbers() {
    let source = "Intro[^first]\n\n# Title[^second]\n\n[^first]: One\n\n[^second]: Two\n";
    let mut document = Document::new(source);
    let headings = document.outline(false).unwrap();
    assert_eq!(headings[0].title, "Title[2]");
    let model = reading::Cache::default().model(source, false);
    let block = model
        .blocks
        .iter()
        .find(|b| b.source.contains(&headings[0].start))
        .unwrap();
    assert_eq!(
        block.content.text.lines().next().unwrap().trim(),
        headings[0].title
    );
}

#[test]
fn nested_and_multiline_headings_preserve_reading_titles() {
    let source = "> # Quoted **heading**\n\n- ## Listed `heading`\n\n> Setext title\n> ===\n\nMultiline\ntitle\n---\n";
    let mut document = Document::new(source);
    let headings = document.outline(false).unwrap();
    assert_eq!(
        headings
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        [
            "Quoted heading",
            "Listed heading",
            "Setext title",
            "Multiline title"
        ]
    );
    for (heading, marker) in headings
        .iter()
        .zip(["# Quoted", "## Listed", "Setext", "Multiline"])
    {
        let byte = source.find(marker).unwrap();
        assert_eq!(heading.start, source[..byte].encode_utf16().count());
    }
}

#[test]
fn revision_cache_updates_unsaved_edits_resets_and_mmd_mode() {
    let mut document = Document::new("# Original\n");
    let pointer = document.outline(false).unwrap().as_ptr();
    assert_eq!(pointer, document.outline(false).unwrap().as_ptr());
    document.replace(2, 8, "修改😀").unwrap();
    assert_eq!(document.revision(), 1);
    assert_eq!(document.outline(false).unwrap()[0].title, "修改😀");
    assert!(document.outline(true).unwrap().is_empty());
    assert_eq!(document.outline(false).unwrap().len(), 1);
    document.reset("No headings\n");
    assert!(document.outline(false).unwrap().is_empty());
    document.reset("Setext\n===\n");
    assert_eq!(document.outline(false).unwrap()[0].title, "Setext");
}
