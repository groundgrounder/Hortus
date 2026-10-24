use hortus_diagram::{FontMetrics, LayoutOptions, SystemFonts, layout, mermaid_block};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("用法：cargo run --release -p hortus-diagram --example benchmark -- <笔记.md> [宽度] [次数]")?;
    let width = args.next().unwrap_or_else(|| "700".into()).parse()?;
    let count: usize = args.next().unwrap_or_else(|| "30".into()).parse()?;
    if count == 0 {
        return Err("次数必须大于零".into());
    }
    let markdown = std::fs::read_to_string(path)?;
    let tree = mermaid_block(&markdown, 1)?.parse()?;
    let mut fonts = SystemFonts::new()?;
    let options = LayoutOptions {
        width,
        ..LayoutOptions::default()
    };
    let mut times = Vec::with_capacity(count);
    // 同一字体度量对象参与全部排版，首轮未命中整形缓存也计入样本。
    for _ in 0..count {
        let start = Instant::now();
        std::hint::black_box(layout(std::hint::black_box(&tree), &options, &mut fonts)?);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let cold_ms = times[0];
    times.sort_by(f64::total_cmp);
    let median_ms = (times[(count - 1) / 2] + times[count / 2]) / 2.0;
    println!(
        "nodes={} width={} iterations={} median_ms={:.6} first_ms={:.6} min_ms={:.6} max_ms={:.6} reference_fonts={}",
        tree.nodes.len(),
        width,
        count,
        median_ms,
        cold_ms,
        times[0],
        times[count - 1],
        fonts.reference_fonts_available()
    );
    Ok(())
}
