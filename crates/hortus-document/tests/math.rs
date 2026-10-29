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
fn display_math_preserves_existing_newlines_and_source_container_prefixes() {
    for (source, expected) in [
        ("$$a+b$$\r\n\r\nnext\r\n", "\u{fffc}\r\n\r\nnext\r\n"),
        ("$$a+b$$  \n\nnext\n", "\u{fffc}  \n\nnext\n"),
        ("- $$x$$\r\n- $$y$$ \n", "• \u{fffc}\r\n• \u{fffc} \n"),
        ("1) $$z$$\n", "1) \u{fffc}\n"),
        ("t[^1]\n\n[^1]: $$x$$\n", "t[1]\n\n[1] \u{fffc}\n"),
        ("**$$x$$**\n", "\u{fffc}\n"),
        ("$$x$$%%c%%\n", "\u{fffc}\n"),
        ("> $$x$$\r\n> t\r\n", "\u{fffc}\r\nt\r\n"),
    ] {
        assert_eq!(
            output(&Cache::default().model(source, false)),
            expected,
            "{source:?}"
        );
    }
}

#[test]
fn inserted_display_newlines_hide_adjacent_spaces_without_changing_formula_mapping() {
    for (source, expected) in [
        ("前 $$a+b$$ 后", "前\n\u{fffc}\n后"),
        ("$$a$$ $$b$$", "\u{fffc}\n\u{fffc}"),
        ("前 **$$x$$** %%hidden%% 后", "前\n\u{fffc}\n后"),
    ] {
        let model = Cache::default().model(source, false);
        assert_eq!(output(&model), expected, "{source:?}");
        let units: Vec<_> = source.encode_utf16().collect();
        for block in model.blocks {
            for formula in &block.content.math {
                let raw = String::from_utf16_lossy(
                    &units[block.source.start + formula.source.start
                        ..block.source.start + formula.source.end],
                );
                assert_eq!(raw, formula.display_source);
            }
        }
    }
}

#[test]
fn outline_preserves_formula_source_and_single_spaces() {
    let mut document = hortus_document::Document::new("# H $$x$$ t\n\n# a $x*y*z$ b\n");
    let headings = document.outline(false).unwrap();
    assert_eq!(
        headings
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        ["H $$x$$ t", "a $x*y*z$ b"]
    );
}

#[test]
fn gfm_table_unescapes_pipe_inside_latex() {
    let (_, table) = hortus_document::media::Table::parse("| $a\\|b$ |\n|---|\n").unwrap();
    assert_eq!(table.rows[0][0].math[0].latex, "a|b");
    assert_eq!(table.rows[0][0].math[0].display_source, "$a|b$");
}

#[test]
fn inline_ranges_masks_and_heading_source() {
    use hortus_document::Document;
    let source = "# 标题 $a_i b_i$😀\n\n中😀 **$x^2$** [公式 $y_i$](target) 后\n\n> [!note] 标题 $z$\n> 前 $$a+b$$ 后\n";
    let model = Cache::default().model(source, false);
    let units: Vec<_> = source.encode_utf16().collect();
    let formulas: Vec<_> = model
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .content
                .math
                .iter()
                .map(move |formula| (block, formula))
        })
        .collect();
    assert_eq!(formulas.len(), 5);
    for ((block, formula), expected) in
        formulas
            .iter()
            .zip(["$a_i b_i$", "$x^2$", "$y_i$", "$z$", "$$a+b$$"])
    {
        assert_eq!(
            String::from_utf16_lossy(
                &units[block.source.start + formula.source.start
                    ..block.source.start + formula.source.end]
            ),
            expected
        );
        assert_eq!(formula.display.len(), 1);
        assert_eq!(
            block.content.text.encode_utf16().nth(formula.display.start),
            Some(0xfffc)
        );
    }
    assert_eq!(formulas[0].1.mask >> 24, 1);
    assert_ne!(formulas[1].1.mask & 2, 0);
    assert_ne!(formulas[2].1.mask & 128, 0);
    assert_ne!(formulas[3].1.mask & 4096, 0);
    assert!(output(&model).contains("前\n\u{fffc}\n后"));
    assert_eq!(
        Document::new(source).outline(false).unwrap()[0].title,
        "标题 $a_i b_i$😀"
    );
}

#[test]
fn table_formula_widths_fit_three_reading_columns() {
    use hortus_diagram::DeterministicMetrics;
    use hortus_document::media::Table;
    let latex = r"\frac{a+b+c+d+e}{f+g+h}";
    let source = format!("| Formula | 中文 |\n|---|---|\n| before ${latex}$ after | $x^2$ |\n");
    let (_, table) = Table::parse(&source).unwrap();
    let cell = &table.rows[1][0];
    assert_eq!(cell.math.len(), 1);
    assert_eq!(cell.math[0].style, Style::Text);
    assert_eq!(cell.text, "before \u{fffc} after");
    let scene = hortus_math::Engine::default()
        .render(latex, hortus_math::Style::Text)
        .unwrap();
    for width in [380.0, 700.0, 1100.0] {
        let layout = table.layout(width, 15.0, &mut DeterministicMetrics);
        assert!(layout.width <= width + 0.001);
        assert!(!layout.scrolls);
        assert!(layout.widths[0] >= scene.width * 15.0 * 1.21 + 20.0);
    }
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
        &[(Style::Display, true)],
        &[(Style::Text, false), (Style::Text, false)],
        &[(Style::Text, false), (Style::Text, false)],
        &[],
        &[(Style::Display, true)],
        &[(Style::Text, false)],
    ];
    let latex: &[&[&str]] = &[
        &[],
        &[],
        &[],
        &["x"],
        &["x"],
        &[r"\frac{a}{b}"],
        &[],
        &[],
        &["x", "x^2"],
        &["x^2+y^2=z^2"],
        &["x"],
        &["5", "6"],
        &["x", "x"],
        &[],
        &[""],
        &["a\n+b"],
    ];
    for (index, (source, expected)) in cases(fixtures).iter().zip(expected).enumerate() {
        let formulas = math::scan(source);
        assert_eq!(
            formulas.iter().map(|f| f.latex.trim()).collect::<Vec<_>>(),
            latex[index],
            "C{:02}: {source}",
            index + 1
        );
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
            let hortus_document::media::Kind::Table(table) = &table.kind else {
                panic!("C09 must be a table")
            };
            assert_eq!(table.rows[1][0].text, "\u{fffc}");
            assert_eq!(table.rows[1][1].math[0].latex, "x^2");
            assert_eq!(table.rows[1][1].math[0].style, Style::Text);
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
        if index == 10 {
            assert_eq!(text, "正文\n\u{fffc}\n继续。");
        }
        for block in &model.blocks {
            for formula in &block.content.math {
                let units: Vec<_> = block.content.text.encode_utf16().collect();
                let display = String::from_utf16_lossy(&units[formula.display.clone()]);
                assert_eq!(
                    display,
                    if formula.latex.trim().is_empty() {
                        formula.display_source.as_str()
                    } else {
                        "\u{fffc}"
                    },
                    "C{:02}",
                    index + 1
                );
            }
        }
    }
    let isolated = cases(include_str!("../../../fixtures/math/markdown-isolated.md"));
    for ((source, expected), projection) in isolated
        .into_iter()
        .zip([
            vec![Style::Text],
            vec![Style::Text],
            vec![Style::Text],
            vec![Style::Display],
        ])
        .zip(["\u{fffc}2", "a\u{fffc}b", "\u{fffc}", "$$$$"])
    {
        assert_eq!(
            math::scan(source)
                .iter()
                .map(|f| f.style)
                .collect::<Vec<_>>(),
            expected,
            "{source}"
        );
        assert_eq!(
            output(&Cache::default().model(source, false)),
            projection,
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
            .all(|formula| formula.block)
    );
    assert!(output(&Cache::default().model(callout_title, false)).contains('\u{fffc}'));
    for source in [
        "text $$x$$ text",
        "$$x$$ $y$",
        "- text $$x$$",
        "> text $$x$$",
    ] {
        assert!(
            math::scan(source)
                .iter()
                .all(|f| f.block == (f.style == Style::Display)),
            "{source}"
        );
        assert!(output(&Cache::default().model(source, false)).contains('\u{fffc}'));
    }
}

#[test]
fn callout_title_formulas_do_not_change_body_math_classification() {
    for prefix in ["> ", "- > "] {
        let continuation = if prefix.starts_with('-') {
            "  > "
        } else {
            "> "
        };
        for title in ["Title", "$$a$$", "$a$"] {
            let source = format!("{prefix}[!note] {title}\n{continuation}$$x$$\n");
            let formulas = math::scan(&source);
            assert_eq!(
                formulas.len(),
                if title == "Title" { 1 } else { 2 },
                "{source}"
            );
            assert!(formulas.last().unwrap().block, "{source}: {formulas:?}");
        }
    }
}

#[test]
fn literal_math_hides_only_list_continuation_indentation() {
    for (marker, indent) in [("- ", "  "), ("1. ", "   "), ("100. ", "     ")] {
        for quote in ["", "> "] {
            let source = format!(
                "{marker}{quote}解方程\n{indent}{quote}$$\n{indent}{quote}x^2=1\n{indent}{quote}$$\n"
            );
            let text = output(&Cache::default().model(&source, false));
            assert!(
                text.contains(&format!(
                    "解方程\n{}\u{fffc}",
                    if quote.is_empty() { indent } else { "" }
                )),
                "{source:?}: {text:?}"
            );
        }
        let ordinary = format!("{marker}文字\n{indent}续行\n");
        assert!(
            output(&Cache::default().model(&ordinary, false)).contains(&format!("\n{indent}续行"))
        );
    }
}

#[test]
fn unicode_ranges_are_utf16_and_inline_math_is_opaque() {
    let source = "前😀\n\n$$\\text{中😀}$$\n\n后😀 $a_1 b_1 ==literal== [[target]]$。\n";
    let model = Cache::default().model(source, false);
    let text = output(&model);
    assert_eq!(text.matches('\u{fffc}').count(), 2, "{text}");
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
            assert_eq!(display, "\u{fffc}");
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

#[test]
fn list_continuations_preserve_indentation_outside_formula_source() {
    let cases = [
        ("- 解方程\n\n  $$x^2=1$$\n", "• 解方程\n\n  \u{fffc}\n"),
        ("- $$x$$\n  $$y$$\n", "• \u{fffc}\n  \u{fffc}\n"),
        ("100. 解\n\n     $$x$$\n", "100. 解\n\n     \u{fffc}\n"),
        ("- 文字\n  $a$ 续行\n", "• 文字\n  \u{fffc} 续行\n"),
    ];
    let actual: Vec<_> = cases
        .iter()
        .map(|(source, _)| output(&Cache::default().model(source, false)))
        .collect();
    assert_eq!(
        actual,
        cases
            .iter()
            .map(|(_, expected)| *expected)
            .collect::<Vec<_>>()
    );
}

#[test]
fn callout_headers_require_a_quote_and_the_first_paragraph_line() {
    for source in [
        "[!a] $$a$$\n$$x$$\n",
        "- [!a] $$a$$\n  $$x$$\n",
        "> intro\n> [!a] $$a$$\n> $$x$$\n",
        "> intro\n> [!a] title\n> $$x$$\n",
    ] {
        let formulas = math::scan(source);
        assert!(!formulas.is_empty());
        assert!(
            formulas.iter().all(|formula| formula.block),
            "{source}: {formulas:?}"
        );
        assert!(output(&Cache::default().model(source, false)).contains('\u{fffc}'));
    }
}
