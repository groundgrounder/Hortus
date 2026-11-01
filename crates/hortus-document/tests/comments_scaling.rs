//! 注释两条路径的倍增检查；release 显式运行，原始三次数值可用于提交间对照。
use hortus_document::{Document, reading::Cache};
use std::hint::black_box;
use std::time::Instant;

fn paragraphs(count: usize, kind: &str) -> String {
    (0..count)
        .map(|i| {
            let tail = match kind {
                "html" => format!("<!-- note {i} --> comment."),
                "percent" => format!("%%note {i}%% comment."),
                "plain" => "[[link]] text.".to_owned(),
                _ => unreachable!(),
            };
            format!("Paragraph {i} has **bold** and a {tail}\n\n")
        })
        .collect()
}

fn lines(count: usize, kind: &str) -> String {
    match kind {
        "html" => "<!-- c -->\n".repeat(count),
        "percent" => "%% c %%\n".repeat(count),
        _ => unreachable!(),
    }
}

fn measure(text: &str, path: &str, shape: &str, kind: &str, count: usize) -> f64 {
    let mut samples = Vec::new();
    for _ in 0..3 {
        let milliseconds = match path {
            "reading" => {
                let mut cache = Cache::default();
                let start = Instant::now();
                let model = black_box(cache.model(black_box(text), false));
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                black_box(model);
                elapsed
            }
            "styles" => {
                let mut document = Document::new(black_box(text));
                let start = Instant::now();
                let runs = black_box(document.style_changes(u64::MAX, true).unwrap());
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                black_box(runs);
                elapsed
            }
            _ => unreachable!(),
        };
        samples.push(milliseconds);
    }
    let best = samples.iter().copied().fold(f64::INFINITY, f64::min);
    println!(
        "{{\"path\":\"{path}\",\"shape\":\"{shape}\",\"kind\":\"{kind}\",\"count\":{count},\"bytes\":{},\"samples_ms\":{samples:?},\"best_ms\":{best}}}",
        text.len()
    );
    best
}

fn check_scaling(shape: &str, counts: [usize; 2], kinds: &[&str]) {
    assert!(!black_box(cfg!(debug_assertions)), "请使用 --release");
    let mut failures = Vec::new();
    for kind in kinds {
        let sources = counts.map(|count| match shape {
            "paragraphs" => paragraphs(count, kind),
            "lines" => lines(count, kind),
            _ => unreachable!(),
        });
        for path in ["reading", "styles"] {
            let small = measure(&sources[0], path, shape, kind, counts[0]);
            let large = measure(&sources[1], path, shape, kind, counts[1]);
            let ratio = large / small;
            println!(
                "{{\"path\":\"{path}\",\"shape\":\"{shape}\",\"kind\":\"{kind}\",\"ratio\":{ratio},\"limit\":3}}"
            );
            if ratio > 3.0 {
                failures.push(format!("{path}/{shape}/{kind}: {ratio:.3}"));
            }
        }
    }
    assert!(failures.is_empty(), "倍增超过三倍：{failures:?}");
}

#[test]
#[ignore = "release 注释段落倍增计时；串行显式运行"]
fn comment_paragraphs_scale_linearly() {
    check_scaling("paragraphs", [4_000, 8_000], &["html", "percent", "plain"]);
}

#[test]
#[ignore = "release 单块纯注释行倍增计时；串行显式运行"]
fn pure_comment_lines_scale_linearly() {
    check_scaling("lines", [10_000, 20_000], &["html", "percent"]);
}

#[test]
#[ignore = "release 三版同机对照原始数值；串行显式运行"]
fn comment_reproduction_timings() {
    assert!(!black_box(cfg!(debug_assertions)), "请使用 --release");
    for (path, kind, count) in [
        ("reading", "html", 2_000),
        ("reading", "html", 5_000),
        ("reading", "percent", 5_000),
        ("reading", "plain", 5_000),
        ("styles", "html", 2_000),
        ("styles", "html", 5_000),
        ("styles", "percent", 5_000),
    ] {
        measure(&paragraphs(count, kind), path, "paragraphs", kind, count);
    }
    for kind in ["html", "percent"] {
        measure(&lines(20_000, kind), "reading", "lines", kind, 20_000);
    }
}
