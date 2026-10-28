use hortus_math::{Color, Engine, Error, GlyphMetrics, Item, MAX_SOURCE_BYTES, Style, layout};
use std::{collections::HashMap, sync::Arc};

#[test]
fn public_corpus_matches_recorded_engine_results_in_both_styles() {
    let samples: Vec<_> = include_str!("../../../fixtures/math/samples.txt")
        .lines()
        .filter(|s| !s.starts_with("# "))
        .collect();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/math/expected.json")).unwrap();
    assert_eq!(samples.len(), 200);
    let mut successes = 0;
    for case in expected.as_array().unwrap() {
        let index = case["id"].as_u64().unwrap() as usize - 1;
        let style = if case["style"] == "display" {
            Style::Display
        } else {
            Style::Text
        };
        let actual = layout(samples[index], style, &HashMap::new());
        assert!(
            !matches!(actual, Err(Error::Panic(_))),
            "{case}: {actual:?}"
        );
        assert_eq!(
            actual.is_ok(),
            case["success"].as_bool().unwrap(),
            "{case}: {actual:?}"
        );
        match actual {
            Ok(scene) => {
                successes += 1;
                assert!(scene.width.is_finite());
                let wire = serde_json::to_vec(&scene).unwrap();
                let decoded: hortus_math::Scene = serde_json::from_slice(&wire).unwrap();
                assert_eq!(scene.items.len(), decoded.items.len());
                assert!((scene.width - decoded.width).abs() < 1e-10);
            }
            Err(error) => assert_eq!(error.to_string(), case["error"].as_str().unwrap(), "{case}"),
        }
    }
    assert_eq!(successes, 374);
}

#[test]
fn explicit_colors_are_distinct_from_theme_foreground_including_black() {
    let scene = layout(
        r"x+{\color{black}y}+{\color{red}z}",
        Style::Display,
        &HashMap::new(),
    )
    .unwrap();
    let glyphs: Vec<_> = scene
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Glyph {
                codepoint, color, ..
            } => Some((*codepoint, color)),
            _ => None,
        })
        .collect();
    let tint = |ch: char| {
        glyphs
            .iter()
            .find(|(code, _)| *code == ch as u32)
            .unwrap()
            .1
    };
    assert!(tint('x').foreground);
    assert_eq!(
        *tint('y'),
        Color {
            foreground: false,
            rgba: [0., 0., 0., 1.]
        }
    );
    assert_eq!(
        *tint('z'),
        Color {
            foreground: false,
            rgba: [1., 0., 0., 1.]
        }
    );
}

#[test]
fn fallback_metrics_use_original_scalars_and_invalidate_cached_layout() {
    let mut engine = Engine::default();
    let first = engine.render(r"\text{中文😀}", Style::Display).unwrap();
    assert_eq!(first.missing_metrics.len(), 3);
    assert!(Arc::ptr_eq(
        &first,
        &engine.render(r"\text{中文😀}", Style::Display).unwrap()
    ));
    let metrics = first
        .missing_metrics
        .iter()
        .map(|c| {
            (
                char::from_u32(*c).unwrap(),
                GlyphMetrics {
                    width: 1.4,
                    height: 1.,
                    depth: 0.2,
                },
            )
        })
        .collect::<Vec<_>>();
    engine.set_metrics(metrics.clone()).unwrap();
    let second = engine.render(r"\text{中文😀}", Style::Display).unwrap();
    assert!(second.missing_metrics.is_empty());
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.width > first.width);
    engine.set_metrics(metrics).unwrap();
    assert!(Arc::ptr_eq(
        &second,
        &engine.render(r"\text{中文😀}", Style::Display).unwrap()
    ));
    assert!(second.items.iter().any(|item| matches!(
        item,
        Item::Glyph {
            codepoint: 0x1f600,
            fallback: true,
            ..
        }
    )));
}

#[test]
fn missing_katex_glyphs_request_system_fallback_metrics() {
    for (source, codepoints) in [
        (
            r"\text{Привет}",
            vec![
                'П' as u32,
                'р' as u32,
                'и' as u32,
                'в' as u32,
                'е' as u32,
                'т' as u32,
            ],
        ),
        (r"\text{α}", vec!['α' as u32]),
        (r"\text{𠮷}", vec!['𠮷' as u32]),
        (r"\text{❤️}", vec!['❤' as u32]),
        (
            r"\text{👨‍👩‍👧‍👦}",
            vec!['👨' as u32, '👩' as u32, '👧' as u32, '👦' as u32],
        ),
        (r"\text{Αθήνα Жук café naïve}", vec!['Α' as u32, 'Ж' as u32]),
    ] {
        let scene = layout(source, Style::Display, &HashMap::new()).unwrap();
        let missing: std::collections::BTreeSet<_> =
            scene.missing_metrics.iter().copied().collect();
        for codepoint in codepoints {
            assert!(
                missing.contains(&codepoint),
                "U+{codepoint:04X} in {source}: {missing:?}"
            );
            assert!(
                scene.items.iter().any(|item| matches!(item,
                    Item::Glyph { codepoint: found, fallback: true, .. } if *found == codepoint
                )),
                "U+{codepoint:04X} has no fallback glyph in {source}"
            );
        }
    }
    let emoji = layout(r"\text{❤️‍👨‍👩‍👧‍👦}", Style::Display, &HashMap::new()).unwrap();
    assert!(
        !emoji
            .missing_metrics
            .iter()
            .any(|cp| matches!(*cp, 0x200d | 0xfe0e | 0xfe0f))
    );
    assert!(!emoji.items.iter().any(|item| matches!(
        item,
        Item::Glyph {
            codepoint: 0x200d | 0xfe0e | 0xfe0f,
            ..
        }
    )));
}

#[test]
fn reserved_noncharacters_are_not_mapped_by_bundled_katex_fonts() {
    use ratex_font::FontId;

    let fonts: [(FontId, &[u8]); 20] = [
        (
            FontId::AmsRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_AMS-Regular.ttf"),
        ),
        (
            FontId::CaligraphicRegular,
            include_bytes!(
                "../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Caligraphic-Regular.ttf"
            ),
        ),
        (
            FontId::FrakturRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Fraktur-Regular.ttf"),
        ),
        (
            FontId::FrakturBold,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Fraktur-Bold.ttf"),
        ),
        (
            FontId::MainBold,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Main-Bold.ttf"),
        ),
        (
            FontId::MainBoldItalic,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Main-BoldItalic.ttf"),
        ),
        (
            FontId::MainItalic,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Main-Italic.ttf"),
        ),
        (
            FontId::MainRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Main-Regular.ttf"),
        ),
        (
            FontId::MathBoldItalic,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Math-BoldItalic.ttf"),
        ),
        (
            FontId::MathItalic,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Math-Italic.ttf"),
        ),
        (
            FontId::SansSerifBold,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_SansSerif-Bold.ttf"),
        ),
        (
            FontId::SansSerifItalic,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_SansSerif-Italic.ttf"),
        ),
        (
            FontId::SansSerifRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_SansSerif-Regular.ttf"),
        ),
        (
            FontId::ScriptRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Script-Regular.ttf"),
        ),
        (
            FontId::Size1Regular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Size1-Regular.ttf"),
        ),
        (
            FontId::Size2Regular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Size2-Regular.ttf"),
        ),
        (
            FontId::Size3Regular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Size3-Regular.ttf"),
        ),
        (
            FontId::Size4Regular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Size4-Regular.ttf"),
        ),
        (
            FontId::TypewriterRegular,
            include_bytes!(
                "../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Typewriter-Regular.ttf"
            ),
        ),
        (
            FontId::CaligraphicRegular,
            include_bytes!("../../../apps/macos/Resources/Fonts/KaTeX/KaTeX_Caligraphic-Bold.ttf"),
        ),
    ];
    for (font, data) in fonts {
        let face = ttf_parser::Face::parse(data, 0).unwrap();
        for codepoint in [0xffff] {
            let Some(character) = char::from_u32(codepoint) else {
                continue;
            };
            assert_eq!(
                ratex_font::get_char_metrics(font, codepoint).is_some(),
                face.glyph_index(character)
                    .is_some_and(|glyph| glyph.0 != 0),
                "{font:?} U+{codepoint:04X}"
            );
        }
    }
}

#[test]
fn malformed_and_excessive_inputs_are_isolated() {
    let mut engine = Engine::default();
    assert!(matches!(
        engine.render(&"x".repeat(MAX_SOURCE_BYTES + 1), Style::Display),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        engine.render(
            &format!("{}x{}", "{".repeat(129), "}".repeat(129)),
            Style::Display
        ),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        engine.render(r"\unknowncommand", Style::Display),
        Err(Error::Parse(_))
    ));
    assert!(matches!(
        engine.render(r"\def\loop{\loop}\loop", Style::Display),
        Err(Error::ResourceLimit(_))
    ));
    assert!(
        engine
            .set_metrics([(
                '中',
                GlyphMetrics {
                    width: f64::NAN,
                    height: 1.,
                    depth: 0.
                }
            )])
            .is_err()
    );
    assert!(
        engine
            .set_metrics([(
                '中',
                GlyphMetrics {
                    width: -1.,
                    height: 1.,
                    depth: 0.
                }
            )])
            .is_err()
    );
    assert!(engine.render("x^2", Style::Display).is_ok());
}

#[test]
fn macro_layout_and_geometry_limits_are_reported_as_resource_limits() {
    let macro_expansion = concat!(
        r"\def\zz{zzzzzzzzzz}",
        r"\edef\aa{\zz\zz\zz\zz\zz\zz\zz\zz\zz\zz}",
        r"\edef\bb{\aa\aa\aa\aa\aa\aa\aa\aa\aa\aa}",
        r"\edef\cc{\bb\bb\bb\bb\bb\bb\bb\bb\bb\bb}",
        r"\edef\dd{\cc\cc\cc\cc\cc\cc\cc\cc\cc\cc}",
        r"\edef\ee{\dd\dd\dd\dd\dd\dd\dd\dd\dd\dd}",
        r"\edef\ff{\ee\ee\ee\ee\ee\ee\ee\ee\ee\ee}x"
    );
    let error = layout(macro_expansion, Style::Display, &HashMap::new()).unwrap_err();
    assert!(matches!(error, Error::ResourceLimit(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("Macro expansion resource limit exceeded")
    );

    let error = layout(
        r"\begin{array}{c}\hdashline\rule{100000em}{1em}\end{array}",
        Style::Display,
        &HashMap::new(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::ResourceLimit(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("resource limit")
    );
}

#[test]
fn public_corpus_covers_all_primitives_and_path_commands() {
    let mut kinds = std::collections::BTreeSet::new();
    let mut commands = std::collections::BTreeSet::new();
    for source in include_str!("../../../fixtures/math/samples.txt")
        .lines()
        .filter(|s| !s.starts_with("# "))
    {
        if let Ok(scene) = layout(source, Style::Display, &HashMap::new()) {
            for item in scene.items {
                let value = serde_json::to_value(&item).unwrap();
                kinds.insert(value["kind"].as_str().unwrap().to_owned());
                if let Item::Path { commands: path, .. } = item {
                    for command in path {
                        commands.insert(
                            serde_json::to_value(command).unwrap()["kind"]
                                .as_str()
                                .unwrap()
                                .to_owned(),
                        );
                    }
                }
            }
        }
    }
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(
        commands.contains("MoveTo") && commands.contains("LineTo"),
        "{commands:?}"
    );
}
