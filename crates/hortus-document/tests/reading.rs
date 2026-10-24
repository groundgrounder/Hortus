use hortus_document::Document;
use hortus_document::reading::Cache;
use std::sync::Arc;

fn assert_same_rendering(
    actual: &hortus_document::reading::Model,
    expected: &hortus_document::reading::Model,
) {
    assert_eq!(actual.blocks.len(), expected.blocks.len());
    for (actual, expected) in actual.blocks.iter().zip(&expected.blocks) {
        assert_eq!(actual.source, expected.source);
        assert_eq!(actual.content.text, expected.content.text);
        assert_eq!(
            actual.content.display_len(),
            actual.content.text.encode_utf16().count()
        );
        assert_eq!(actual.content.diagram, expected.content.diagram);
        assert_eq!(actual.content.runs.len(), expected.content.runs.len());
        for (actual, expected) in actual.content.runs.iter().zip(&expected.content.runs) {
            assert_eq!(actual.display, expected.display);
            assert_eq!(actual.source, expected.source);
            assert_eq!(actual.mask, expected.mask);
        }
    }
}

#[test]
fn rendered_text_and_source_mapping_cover_unicode_and_syntax() {
    let mut cache = Cache::default();
    let source = "# 标题😀\n\n正文 **加粗** *斜体* ~~删除~~ ==高亮== `代码` [[目标|别名]] #标签 %%隐藏%%。\n\n- [ ] 任务\n- 普通\n\n> [!note] 标题\n> 连续内容\n";
    let model = cache.model(source, false);
    let text: String = model
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect();
    println!("{text}");
    for marker in ["#", "**", "`", "[[", "%%", ">", "- [ ]", "目标|"] {
        assert!(!text.contains(marker), "{marker}: {text}");
    }
    for expected in ["标题😀", "加粗", "别名", "☐", "•", "note"] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert_eq!(
        model.blocks.last().unwrap().source.end,
        source.encode_utf16().count()
    );
    for b in &model.blocks {
        assert_eq!(
            b.content.display_len(),
            b.content.text.encode_utf16().count()
        );
        for r in &b.content.runs {
            assert!(r.source.end <= b.source.len());
            assert!(r.display.end <= b.content.text.encode_utf16().count());
        }
    }
}

#[test]
fn middle_edit_reuses_every_unchanged_block_and_second_open_hits_cache() {
    let mut cache = Cache::default();
    let source = (0..22000)
        .map(|i| format!("Paragraph {i}: text **bold** and Unicode 中文😀.\n\n"))
        .collect::<String>();
    assert!(source.len() > 1024 * 1024);
    let first = cache.model(&source, false);
    assert_eq!(cache.model(&source, false).rebuilt, 0);
    let changed = source.replacen("Paragraph 8000:", "Modified paragraph 8000:", 1);
    let next = cache.model(&changed, false);
    assert_eq!(next.rebuilt, 1);
    let expected = Cache::default().model(&changed, false);
    assert_same_rendering(&next, &expected);
    for (i, (a, b)) in first.blocks.iter().zip(&next.blocks).enumerate() {
        if i != 8000 {
            assert!(Arc::ptr_eq(&a.content, &b.content));
        }
    }
}

#[test]
fn document_reload_keeps_reading_cache_for_middle_edit() {
    let source = (0..22000)
        .map(|i| format!("Paragraph {i}: text **bold** and Unicode 中文😀.\n\n"))
        .collect::<String>();
    let changed = source.replacen("Paragraph 8000:", "Modified paragraph 8000:", 1);
    let mut document = Document::new(&source);
    assert_eq!(document.reading(false).unwrap().rebuilt, 22000);
    document.reset(&changed);
    assert_eq!(document.reading(false).unwrap().rebuilt, 1);
}

#[test]
fn cross_block_comments_fall_back_to_full_semantic_analysis() {
    let before = "Visible %%hidden\n\ncontinued hidden%% visible\n\nStable **text**.\n";
    let after = "Visible %%hidden\n\ncontinued hidden\n\nStable **text**.\n";
    let mut cache = Cache::default();
    cache.model(before, false);
    let actual = cache.model(after, false);
    let expected = Cache::default().model(after, false);
    assert_same_rendering(&actual, &expected);
    let text: String = actual
        .blocks
        .iter()
        .map(|block| block.content.text.as_str())
        .collect();
    assert!(text.contains("continued hidden"));
}

#[test]
fn incremental_edit_keeps_utf8_character_boundaries_and_cached_neighbors() {
    let before = (0..40)
        .map(|index| format!("Paragraph {index}: 😀token😀 and **text**.\n\n"))
        .collect::<String>();
    let mut cache = Cache::default();
    let first = cache.model(&before, false);
    let after = before.replacen("😀token😀", "😃token😄", 1);
    let actual = cache.model(&after, false);
    let expected = Cache::default().model(&after, false);
    assert_same_rendering(&actual, &expected);
    assert_eq!(actual.rebuilt, 1);
    assert!(Arc::ptr_eq(
        &first.blocks[1].content,
        &actual.blocks[1].content
    ));
}

#[test]
fn adding_a_block_boundary_falls_back_to_full_analysis() {
    let before = "First paragraph.\n\nSecond paragraph.\n";
    let after = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n";
    let mut cache = Cache::default();
    cache.model(before, false);
    let actual = cache.model(after, false);
    let expected = Cache::default().model(after, false);
    assert_same_rendering(&actual, &expected);
    assert_eq!(actual.blocks.len(), 3);
}

#[test]
fn opaque_code_images_tables_and_comments() {
    let mut cache = Cache::default();
    let model = cache.model("```text\n**literal** [[literal]] #literal\n```\n\n![alt](image.png)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n%%hidden\nline%%\n", false);
    let text: String = model
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect();
    assert!(text.contains("**literal** [[literal]] #literal"), "{text}");
    assert_eq!(
        model
            .blocks
            .iter()
            .map(|b| b.content.media.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(text.matches('\u{fffc}').count(), 2);
    for block in &model.blocks {
        assert_eq!(
            block.content.display_len(),
            block.content.text.encode_utf16().count()
        );
    }
    assert!(!text.contains("hidden"));
    assert!(!text.contains("```"));
}

#[test]
fn footnote_references_and_definitions_share_first_use_numbering() {
    let mut cache = Cache::default();
    let model = cache.model(
        "First[^alpha], second[^beta], repeat[^alpha].\n\n[^beta]: beta note\n\n[^alpha]: alpha note\n",
        false,
    );
    let text: String = model
        .blocks
        .iter()
        .map(|block| block.content.text.as_str())
        .collect();
    assert!(text.contains("First[1], second[2], repeat[1]."), "{text}");
    assert!(text.contains("[2] beta note"), "{text}");
    assert!(text.contains("[1] alpha note"), "{text}");
}

#[test]
fn blocks_keep_original_utf16_ranges_across_blank_lines_and_unicode() {
    let mut cache = Cache::default();
    let source = "前置😀\n\n# 标题\n\n正文";
    let model = cache.model(source, false);
    assert!(model.blocks.len() >= 3);
    let title = source.find("# 标题").unwrap();
    let title_start16 = source[..title].encode_utf16().count();
    let title_block = model
        .blocks
        .iter()
        .find(|b| b.source.start == title_start16)
        .unwrap();
    assert_eq!(title_block.source.start, title_start16);
    assert!(title_block.source.end <= source.encode_utf16().count());
    let body = source.find("正文").unwrap();
    let body_start16 = source[..body].encode_utf16().count();
    let body_block = model
        .blocks
        .iter()
        .find(|b| b.source.start <= body_start16 && b.source.end > body_start16)
        .unwrap();
    assert!(body_block.source.start <= body_start16);
    assert!(body_block.source.end >= source.encode_utf16().count());
    assert!(title_block.source.start == model.blocks[0].source.end);
    assert_eq!(title_block.content.text.trim(), "标题");
}
