use hortus_document::{
    math::{self, Style},
    reading::{Cache, Model},
};
use std::sync::Arc;

fn output(model: &Model) -> String {
    model
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect()
}
fn cases(source: &str) -> Vec<&str> {
    source
        .split("\n## ")
        .skip(1)
        .map(|s| s.split_once('\n').unwrap().1.trim())
        .collect()
}

#[test]
fn twenty_pulldown_boundaries_match_the_documented_rules() {
    let fixtures = include_str!("../../../fixtures/math/markdown-boundaries.md");
    // C09 是表格附件，单元格中的公式不会进入公式显示模型。
    let expected: &[&[(Style, bool)]] = &[
        &[],
        &[],
        &[],
        &[(Style::Text, false)],
        &[(Style::Display, true)],
        &[(Style::Display, true)],
        &[],
        &[],
        &[(Style::Text, false), (Style::Text, false)],
        &[(Style::Display, true)],
        &[(Style::Display, false)],
        &[(Style::Text, false), (Style::Text, false)],
        &[(Style::Text, false), (Style::Text, false)],
        &[],
        &[(Style::Display, true)],
        &[(Style::Text, false)],
    ];
    for (index, (source, expected)) in cases(fixtures).iter().zip(expected).enumerate() {
        let formulas = math::scan(source);
        assert_eq!(
            formulas
                .iter()
                .map(|f| (f.style, f.block))
                .collect::<Vec<_>>(),
            *expected,
            "C{:02}: {source}",
            index + 1
        );
        let model = Cache::default().model(source, false);
        let text = output(&model);
        if index == 8 {
            assert_eq!(
                model
                    .blocks
                    .iter()
                    .map(|b| b.content.math.len())
                    .sum::<usize>(),
                0
            );
            let table = &model.blocks[0].content.media[0];
            assert!(format!("{table:?}").contains("$x$"));
        } else {
            assert_eq!(
                model
                    .blocks
                    .iter()
                    .map(|b| b.content.math.len())
                    .sum::<usize>(),
                formulas.len(),
                "C{:02}: {text}",
                index + 1
            );
        }
        if index == 14 {
            assert!(text.contains("$$$$"));
        }
        if index == 13 {
            assert!(text.contains("$x"));
        }
    }
    let isolated = cases(include_str!("../../../fixtures/math/markdown-isolated.md"));
    for (source, expected) in isolated.into_iter().zip([
        vec![Style::Text],
        vec![Style::Text],
        vec![Style::Text],
        vec![Style::Display],
    ]) {
        assert_eq!(
            math::scan(source)
                .iter()
                .map(|f| f.style)
                .collect::<Vec<_>>(),
            expected,
            "{source}"
        );
    }
}

#[test]
fn containers_multiple_formulas_and_mixed_paragraphs() {
    for source in [
        "$$x$$ $$y$$\n",
        "$$x$$\n$$y$$\n",
        "> $$x$$\n> $$y$$\n",
        "- $$x$$\n  $$y$$\n",
        "- [ ] $$x$$\n  $$y$$\n",
    ] {
        let formulas = math::scan(source);
        assert_eq!(formulas.len(), 2, "{source}");
        assert!(formulas.iter().all(|f| f.block), "{source}: {formulas:?}");
        let model = Cache::default().model(source, false);
        assert_eq!(output(&model).matches('\u{fffc}').count(), 2, "{source}");
    }
    let callout = "> [!note] Title\n> $$x$$\n> $$y$$\n";
    assert!(math::scan(callout).iter().all(|formula| formula.block));
    let callout_title = "> [!note] $$E=mc^2$$\n";
    assert!(
        math::scan(callout_title)
            .iter()
            .all(|formula| !formula.block)
    );
    assert!(!output(&Cache::default().model(callout_title, false)).contains('\u{fffc}'));
    for source in [
        "text $$x$$ text",
        "$$x$$ $y$",
        "- text $$x$$",
        "> text $$x$$",
    ] {
        assert!(math::scan(source).iter().all(|f| !f.block), "{source}");
        assert!(!output(&Cache::default().model(source, false)).contains('\u{fffc}'));
    }
}

#[test]
fn unicode_ranges_are_utf16_and_inline_math_is_opaque() {
    let source = "前😀\n\n$$\\text{中😀}$$\n\n后😀 $a_1 b_1 ==literal== [[target]]$。\n";
    let model = Cache::default().model(source, false);
    let text = output(&model);
    assert!(text.contains("$a_1 b_1 ==literal== [[target]]$"), "{text}");
    for block in &model.blocks {
        for formula in &block.content.math {
            let source16: Vec<_> = source.encode_utf16().collect();
            let raw = String::from_utf16(
                &source16[block.source.start + formula.source.start
                    ..block.source.start + formula.source.end],
            )
            .unwrap();
            assert!(raw.starts_with('$') && raw.ends_with('$'));
            let display16: Vec<_> = block.content.text.encode_utf16().collect();
            let display = String::from_utf16(&display16[formula.display.clone()]).unwrap();
            assert_eq!(display, if formula.block { "\u{fffc}" } else { &raw });
        }
    }
}

#[test]
fn code_frontmatter_comments_and_mmd_are_excluded_and_empty_is_safe() {
    let source = "---\nmath: $$x$$\n---\n\n`$x$`\n\n```text\n$$y$$\n```\n\n%% $$hidden$$ %%\n\n$$$$\n\n$$   $$\n\n$x\n\nfollowing\n";
    let model = Cache::default().model(source, false);
    let formulas: Vec<_> = model.blocks.iter().flat_map(|b| &b.content.math).collect();
    assert_eq!(formulas.len(), 2);
    assert!(formulas.iter().all(|f| f.latex.trim().is_empty()));
    assert!(output(&model).contains("following"));
    assert_eq!(
        Cache::default()
            .model(source, true)
            .blocks
            .iter()
            .map(|b| b.content.math.len())
            .sum::<usize>(),
        0
    );
}

#[test]
fn incremental_fast_and_full_builds_match_and_only_one_block_changes() {
    let source = "Before😀 **bold**\n\n$$x^2$$\n\n$a_1 b_1$\n\nAfter\n";
    let mut cache = Cache::default();
    let first = cache.model(source, false);
    for changed in [
        source.replace("x^2", "\\frac{中}{😀}"),
        source.replace("$$x^2$$", "text $$x$$"),
        source.replace("$a_1 b_1$", "$a_2 b_2$"),
    ] {
        let mut cache = Cache::default();
        let prior = cache.model(source, false);
        let next = cache.model(&changed, false);
        let full = Cache::default().model(&changed, false);
        assert_eq!(next.rebuilt, 1);
        assert_eq!(output(&next), output(&full));
        for (a, b) in next.blocks.iter().zip(&full.blocks) {
            assert_eq!(a.source, b.source);
            assert_eq!(format!("{:?}", a.content), format!("{:?}", b.content));
        }
        assert!(Arc::ptr_eq(
            &prior.blocks[0].content,
            &next.blocks[0].content
        ));
        assert!(Arc::ptr_eq(
            &prior.blocks.last().unwrap().content,
            &next.blocks.last().unwrap().content
        ));
    }
    assert_eq!(cache.model(source, false).rebuilt, 0);
    assert!(Arc::ptr_eq(
        &first.blocks[1].content,
        &cache.model(source, false).blocks[1].content
    ));
}
