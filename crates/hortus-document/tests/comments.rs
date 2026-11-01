use hortus_document::{
    markdown::{self, Kind},
    reading::{Cache, Model},
};

fn displayed(model: &Model) -> String {
    model
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect()
}

fn check(source: &str, expected: &str) {
    let full = Cache::default().model(source, false);
    assert_eq!(displayed(&full), expected, "{source:?}");
    for before in ["before\n\n> [!note] old\n", source] {
        let mut cache = Cache::default();
        cache.model(before, false);
        let actual = cache.model(source, false);
        assert_eq!(actual.blocks.len(), full.blocks.len());
        for (a, b) in actual.blocks.iter().zip(&full.blocks) {
            assert_eq!(a.source, b.source);
            assert_eq!(
                format!("{:?}", a.content),
                format!("{:?}", b.content),
                "{source:?}"
            );
        }
    }
}

#[test]
fn callout_comments_match_removed_comments() {
    for (source, clean) in [
        (">[!%%%%]", ">[!]"),
        ("> [!%%x%%] t", "> [!] t"),
        ("> [!a%%b%%] t", "> [!a] t"),
        (">[!%% %%]x", ">[!]x"),
        ("> [!a%%]%%] t", "> [!a] t"),
        ("> > [!a%%b%%] t", "> > [!a] t"),
        ("> [!a%%b%%] t\r\n> body\r\n", "> [!a] t\r\n> body\r\n"),
        ("> [!a%%b%%] t\n> body\n", "> [!a] t\n> body\n"),
        ("> [!note] t", "> [!note] t"),
        (">[!%%]", ">[!%%]"),
        ("> [!]", "> [!]"),
    ] {
        check(source, &displayed(&Cache::default().model(clean, false)));
    }
}

#[test]
fn only_first_quote_line_is_callout() {
    for (source, expected) in [
        ("> a\n> [!b] c", "a\n[!b] c"),
        ("> [!a] t\n> [!b] u", "a · t\n[!b] u"),
        ("> > [!a] t", "a · t"),
        ("> [!a] t\n>\n> [!b] u", "a · t\n\n[!b] u"),
        ("> [!a] t\n\n> [!b] u", "a · t\n\nb · u"),
    ] {
        check(source, expected);
    }
}

#[test]
fn html_comments_are_hidden_without_swallowing_text_or_code() {
    for (source, expected) in [
        ("a <!-- c --> b", "a  b"),
        ("<!-- c -->\n\npara", "para"),
        ("<!-- c -->", ""),
        ("<!-- c -->\r\rb\r", "b\r"),
        ("<!-- c\ncontinued -->\nbody", "body"),
        ("<!-- c\ncontinued --> tail", " tail"),
        ("a <!-- c --> b <!-- d --> c", "a  b  c"),
        ("`<!-- c -->`", "<!-- c -->"),
        ("```text\n<!-- c -->\n```\n", "<!-- c -->\n"),
        ("a <!-- open", "a <!-- open"),
        ("<div>\ntext\n</div>", "<div>\ntext\n</div>"),
        ("x <!-- c\n\ny --> z", "x <!-- c\n\ny --> z"),
        ("前文 <!--a\nb--> $$x$$ 后", "前文\n\u{fffc}\n后"),
        ("$$x$$ <!--a\nb--> tail", "\u{fffc}\ntail"),
    ] {
        check(source, expected);
    }
    let source = include_str!("../../../fixtures/math/display-comment.md");
    assert!(!displayed(&Cache::default().model(source, false)).contains("SPDX"));
}

#[test]
fn comment_styles_and_callout_markers_do_not_overlap() {
    let source = "> [!a%%]%%] t\n> [!b] u\n\na <!-- c --> b";
    let spans = markdown::analyze(source);
    let mask_at = |position| {
        spans
            .iter()
            .filter(|s| s.bytes.contains(&position))
            .fold(0, |m, s| m | s.kind.mask())
    };
    let comment = source.find("]%%").unwrap();
    assert_eq!(
        mask_at(comment) & (Kind::Comment.mask() | Kind::Callout.mask()),
        Kind::Comment.mask()
    );
    assert_ne!(
        mask_at(source.find("] t").unwrap()) & Kind::Marker.mask(),
        0
    );
    assert_eq!(
        mask_at(source.find("[!b]").unwrap()) & (Kind::Marker.mask() | Kind::Callout.mask()),
        0
    );
    assert_eq!(
        mask_at(source.find(" c ").unwrap() + 1) & Kind::Comment.mask(),
        Kind::Comment.mask()
    );
}

#[test]
fn comment_splits_keep_other_semantic_ranges_valid() {
    for source in [
        "[[target|alias]] %%comment%% tail",
        "[[ta%%comment%%rget|alias]] tail",
        "![[image.png]] %%comment%% tail",
        "![[im%%comment%%age.png]] tail",
        "- [x] task %%comment%% tail\n",
        "1) list %%comment%% tail\n",
        "- list %%comment%% tail\n",
        "ref[^a] %%comment%%\n\n[^a]: note %%comment%% tail\n",
        "ref[^a%%comment%%b]\n\n[^a%%comment%%b]: note\n",
        "[link %%comment%% text](target)",
        "😀 %%comment%% ![[中.png]]",
    ] {
        let html = source.replace("%%comment%%", "<!--comment-->");
        let expected = displayed(&Cache::default().model(source, false));
        // 脚注标签不允许 HTML：两种定界符都保留在原始标签中，原文字面值应不同。
        let html_expected = if source.starts_with("ref[^a%%") {
            expected.replace("%%comment%%", "<!--comment-->")
        } else {
            expected.clone()
        };
        for (text, expected) in [
            (source, expected.as_str()),
            (html.as_str(), html_expected.as_str()),
        ] {
            check(text, expected);
            let full = Cache::default().model(text, false);
            for block in full.blocks {
                for run in &block.content.runs {
                    assert!(run.source.end <= block.source.len());
                    assert!(run.display.end <= block.content.display_len());
                }
            }
        }
    }
}

macro_rules! list_comment_case {
    ($name:ident, $source:expr, $expected:expr) => {
        #[test]
        fn $name() {
            check($source, $expected);
            check(
                &$source.replace("<!--", "%%").replace("-->", "%%"),
                $expected,
            );
        }
    };
}

list_comment_case!(
    list_html_inline,
    "- item <!-- c -->\n- next",
    "• item \n• next"
);
list_comment_case!(list_html_asterisk, "* a <!-- c -->\n* b", "• a \n• b");
list_comment_case!(
    list_html_task,
    "- [ ] task <!-- c -->\n- [x] done",
    "☐  task \n☑  done"
);
list_comment_case!(
    list_html_between_items,
    "- a\n  <!-- c -->\n- b",
    "• a\n• b"
);
list_comment_case!(list_html_continuation, "- a\n  <!-- c -->\n  b", "• a\n  b");
list_comment_case!(
    list_html_loose,
    "- a\n\n  <!-- c -->\n\n- b",
    "• a\n\n\n• b"
);
list_comment_case!(list_html_wikilink, "- [[a|b]] <!--c--> x", "• b  x");
list_comment_case!(
    list_html_footnote,
    "- x[^1] <!--c-->\n\n[^1]: n",
    "• x[1] \n\n[1] n"
);

#[test]
fn html_comments_in_other_containers_match_percent_comments() {
    for (source, expected) in [
        ("1) a <!-- c -->\n2) b", "1) a \n2) b"),
        ("> a <!-- c -->\n> b", "a \nb"),
        ("> [!note] a <!-- c -->\n> b", "note · a \nb"),
        ("- ![[image.png]] <!--c--> x", "\n\u{fffc}\n  x"),
        ("- <!--c-->\n- b", "\n• b"),
        ("- <!--c--> x", " x"),
        ("- <!--c-->", ""),
    ] {
        check(source, expected);
        check(&source.replace("<!--", "%%").replace("-->", "%%"), expected);
    }
}

#[test]
fn short_html_comments_match_obsidian_inline_and_as_blocks() {
    for comment in ["<!-->", "<!--->"] {
        let inline = format!("before {comment} after");
        check(&inline, &inline);
        check(&format!("{comment}\n\nbody"), "body");
        check(comment, "");
        check(&format!("`{comment}`"), comment);
    }
}
