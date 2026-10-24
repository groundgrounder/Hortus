use hortus_document::{
    Document,
    markdown::{self, Kind},
};

fn contains(text: &str, kind: Kind, needle: &str) {
    let start = text.find(needle).unwrap();
    let span = markdown::analyze(text)
        .into_iter()
        .find(|s| s.kind == kind && s.bytes == (start..start + needle.len()));
    let span =
        span.unwrap_or_else(|| panic!("缺少 {kind:?}: {needle:?}; {:?}", markdown::analyze(text)));
    assert_eq!(span.utf16.start, text[..start].encode_utf16().count());
    assert_eq!(span.utf16.len(), needle.encode_utf16().count());
}

#[test]
fn every_syntax_has_exact_source_and_utf16_ranges() {
    for level in 1..=6 {
        let line = format!("{} 标题😀\n", "#".repeat(level));
        contains(&line, Kind::Heading(level as u8), line.trim_end());
        contains(&line, Kind::Marker, &format!("{} ", "#".repeat(level)));
    }
    for (source, kind, content, marker) in [
        ("**加粗😀**", Kind::Strong, "加粗😀", "**"),
        ("_斜体_", Kind::Emphasis, "斜体", "_"),
        ("~~删除~~", Kind::Strike, "删除", "~~"),
        ("`代码`", Kind::InlineCode, "代码", "`"),
        ("[[双链]]", Kind::WikiLink, "双链", "[["),
        ("![[嵌入]]", Kind::Embed, "嵌入", "![["),
        ("#标签/子项", Kind::Tag, "标签/子项", "#"),
        ("==高亮==", Kind::Highlight, "高亮", "=="),
        ("%%注释%%", Kind::Comment, "注释", "%%"),
        ("[链接](https://example.org)", Kind::Link, "链接", "["),
        ("![图片](image.png)", Kind::Image, "图片", "!["),
        ("脚注[^one]\n\n[^one]: 说明\n", Kind::Footnote, "one", "[^"),
        ("> 引用\n", Kind::Quote, "引用", "> "),
        (
            "> [!note] 提示\n",
            Kind::Callout,
            "[!note] 提示\n",
            "[!note]",
        ),
        ("1. 有序\n", Kind::OrderedList, "有序\n", "1. "),
        ("- 无序\n", Kind::UnorderedList, "无序\n", "- "),
        ("- [ ] 任务\n", Kind::Task, "[ ]", "[ ]"),
        ("---\n", Kind::Rule, "---\n", "---\n"),
        (
            "---\ntitle: 示例\n---\n正文",
            Kind::Frontmatter,
            "title: 示例\n",
            "---\n",
        ),
        (
            "```rust\nlet x = 1;\n```\n",
            Kind::CodeBlock,
            "```rust\nlet x = 1;\n```\n",
            "```rust\n",
        ),
    ] {
        contains(source, kind, content);
        contains(source, Kind::Marker, marker);
    }
}

#[test]
fn code_is_opaque_and_unclosed_inline_markers_do_not_consume_body() {
    let text = "`**伪** [[伪]] #伪`\n\n```text\n**伪** [[伪]] #伪\n```\n\n正常 **粗体** #标签\n";
    let spans = markdown::analyze(text);
    let normal = text.find("正常").unwrap();
    assert!(
        !spans.iter().any(|s| s.bytes.start < normal
            && matches!(s.kind, Kind::Strong | Kind::WikiLink | Kind::Tag))
    );
    contains(text, Kind::Strong, "粗体");
    contains(text, Kind::Tag, "标签");
    for prefix in [
        "**未闭合",
        "_未闭合",
        "~~未闭合",
        "`未闭合",
        "[[未闭合",
        "![[未闭合",
        "==未闭合",
        "%%未闭合",
        "[未闭合",
        "---",
    ] {
        let source = format!("{prefix}\n\n后文 **有效** #标签\n");
        contains(&source, Kind::Strong, "有效");
        contains(&source, Kind::Tag, "标签");
    }
    contains(
        "%%第一行\n**第二行**\n%%\n正文",
        Kind::Comment,
        "第一行\n**第二行**\n",
    );
    contains(
        "---\r\ntitle: 中文\r\n---\r\n正文",
        Kind::Frontmatter,
        "title: 中文\r\n",
    );
    let escaped = r"\#标签 \[[双链]] \==高亮== \%%注释%%";
    assert!(!markdown::analyze(escaped).iter().any(|s| matches!(
        s.kind,
        Kind::Tag | Kind::WikiLink | Kind::Highlight | Kind::Comment
    )));
}

#[test]
fn heading_closing_hashes_do_not_mute_content_tags() {
    let source = "# 标题 #标签 和内容\n";
    assert!(
        !markdown::analyze(source)
            .iter()
            .any(|s| s.kind == Kind::Marker && s.bytes.end == source.len() - 1)
    );
    contains("# 标题 ### \n", Kind::Marker, " ### ");
    contains("# ###\n", Kind::Marker, " ###");
    contains("# 标题 #标签\n", Kind::Tag, "标签");
}

#[test]
fn closing_code_fence_requires_enough_markers_and_allows_indentation() {
    contains("```text\n代码\n  ```\n", Kind::Marker, "  ```\n");
    let unclosed = "````text\n代码\n```\n";
    assert!(
        !markdown::analyze(unclosed)
            .iter()
            .any(|s| s.kind == Kind::Marker && s.bytes.start > 0)
    );
}

#[test]
fn nested_traits_and_differential_updates_survive_editing_and_dropped_results() {
    let runs = markdown::runs("***中😀***");
    assert!(runs.iter().any(|r| r.mask & 6 == 6 && r.range == (3..6)));
    let source = format!("# 标题\n正文\n{}", "普通行\n".repeat(10000));
    let mut doc = Document::new(&source);
    assert!(!doc.style_changes(u64::MAX, true).unwrap().is_empty());
    assert!(doc.style_changes(0, false).unwrap().is_empty());
    doc.replace(7, 0, "字").unwrap();
    let patch = doc.style_changes(0, false).unwrap();
    assert!(patch.iter().map(|r| r.range.len()).sum::<usize>() < 30);
    doc.replace(7, 1, "**粗**").unwrap();
    // 上次结果没安装时必须返回完整恢复，不能遗漏上次变化。
    let patch = doc.style_changes(0, false).unwrap();
    assert_eq!(patch.first().unwrap().range.start, 0);
    assert_eq!(
        patch.last().unwrap().range.end,
        doc.text().unwrap().encode_utf16().count()
    );
    assert!(doc.style_changes(2, false).unwrap().is_empty());
}

#[test]
fn public_fixture_has_all_semantic_kinds_and_valid_unicode_boundaries() {
    let text = include_str!("../../../fixtures/vault-sample/样式化源码.md");
    let spans = markdown::analyze(text);
    for kind in [
        Kind::Strong,
        Kind::Emphasis,
        Kind::Strike,
        Kind::InlineCode,
        Kind::CodeBlock,
        Kind::Quote,
        Kind::OrderedList,
        Kind::UnorderedList,
        Kind::Task,
        Kind::Link,
        Kind::Image,
        Kind::WikiLink,
        Kind::Embed,
        Kind::Tag,
        Kind::Highlight,
        Kind::Comment,
        Kind::Callout,
        Kind::Footnote,
        Kind::Frontmatter,
        Kind::Rule,
    ] {
        assert!(spans.iter().any(|s| s.kind == kind), "缺少 {kind:?}");
    }
    for span in spans {
        assert!(text.is_char_boundary(span.bytes.start));
        assert!(text.is_char_boundary(span.bytes.end));
    }
}

#[test]
fn both_run_merging_paths_preserve_overlapping_semantics() {
    let nested = format!("{}中😀 **强调**\n", "> ".repeat(32));
    let fixture = include_str!("../../../fixtures/vault-sample/样式化源码.md");
    let comments =
        "# 标题 %%**隐藏**%% 末尾\n*前 %%注释😀%% 后*\n%%跨行\n[[隐藏]] #隐藏\n%%\n`%%代码%%`\n";
    for (index, text) in [fixture, &nested, comments].into_iter().enumerate() {
        let spans = markdown::analyze(text);
        let coverage: usize = spans
            .iter()
            .filter(|span| span.kind.mask() != 0)
            .map(|span| span.bytes.len())
            .sum();
        assert_eq!(coverage > text.len() * 8, index == 1);
        let mut expected = vec![0; text.encode_utf16().count()];
        for span in spans {
            for value in &mut expected[span.utf16] {
                *value |= span.kind.mask();
            }
        }
        let mut actual = vec![0; expected.len()];
        for run in markdown::runs(text) {
            actual[run.range].fill(run.mask);
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn style_patches_reconstruct_full_analysis_after_400_seeded_edits() {
    let mut document = Document::new("# Heading\n**bold** [[wiki]]\n\n- [ ] Task\n");
    let mut screen = vec![0; document.text().unwrap().encode_utf16().count()];
    let mut revision = u64::MAX;
    let mut seed = 0x1055u64;
    for step in 0..400 {
        let patches = document.style_changes(revision, false).unwrap();
        for patch in patches {
            screen[patch.range].fill(patch.mask);
        }
        revision = document.revision();
        let text = document.text().unwrap();
        let mut expected = vec![0; text.encode_utf16().count()];
        for run in markdown::runs(&text) {
            expected[run.range].fill(run.mask);
        }
        assert_eq!(screen, expected, "step={step}");
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let boundaries: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| text[..i].encode_utf16().count())
            .chain(std::iter::once(screen.len()))
            .collect();
        let a = (seed >> 32) as usize % boundaries.len();
        let b = (a + (seed as usize % 4)).min(boundaries.len() - 1);
        let replacements = ["中😀", "\n", "**", "[[", "# ", "%%", "", "=="];
        let replacement = replacements[(seed >> 16) as usize % replacements.len()];
        document
            .replace(boundaries[a], boundaries[b] - boundaries[a], replacement)
            .unwrap();
        screen.splice(
            boundaries[a]..boundaries[b],
            std::iter::repeat_n(0, replacement.encode_utf16().count()),
        );
    }
}
