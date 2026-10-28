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
