//! 公开语料的无缓存排版基准；两种样式都包含验证和场景转换。
use hortus_math::{Style, layout};
use std::{collections::HashMap, hint::black_box, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map_or(50, |s| s.parse().unwrap());
    assert!(rounds > 0);
    let samples: Vec<_> = include_str!("../../../fixtures/math/samples.txt")
        .lines()
        .filter(|s| !s.starts_with("# "))
        .collect();
    let metrics = HashMap::new();
    let mut times = Vec::new();
    let mut successes = 0;
    let mut items = 0;
    for _ in 0..rounds {
        for source in &samples {
            for style in [Style::Display, Style::Text] {
                let start = Instant::now();
                let result = black_box(layout(black_box(source), style, &metrics));
                times.push(start.elapsed().as_secs_f64() * 1e6);
                if let Ok(scene) = result {
                    successes += 1;
                    items += scene.items.len();
                }
            }
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "rounds": rounds, "calls": times.len(), "successes": successes, "items": items,
            "median_us": times[times.len() / 2], "p95_us": times[(times.len() - 1) * 95 / 100],
            "maximum_us": times.last(), "total_ms": times.iter().sum::<f64>() / 1000.0
        })
    );
}
