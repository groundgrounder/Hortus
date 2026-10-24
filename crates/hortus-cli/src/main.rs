use hortus_diagram::{LayoutOptions, SystemFonts, Theme, layout, mermaid_block, parse_at};
use std::collections::HashSet;
use std::path::PathBuf;

const USAGE: &str = "Usage: hortus mermaid render <note.md> [--block N] [--width 700] [--theme light|dark] [--collapse id,id] --out diagram.svg [--metrics metrics.json]";

struct Arguments {
    input: PathBuf,
    out: PathBuf,
    metrics: Option<PathBuf>,
    block: usize,
    options: LayoutOptions,
}

fn arguments() -> Result<Option<Arguments>, String> {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--help") || first.as_deref() == Some("-h") {
        return Ok(None);
    }
    if first.as_deref() != Some("mermaid") || args.next().as_deref() != Some("render") {
        return Err(USAGE.into());
    }
    let input = args.next().filter(|s| !s.starts_with('-')).ok_or(USAGE)?;
    let mut out = None;
    let mut metrics = None;
    let mut block = 1;
    let mut options = LayoutOptions::default();
    while let Some(option) = args.next() {
        if option == "--help" || option == "-h" {
            return Ok(None);
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_str() {
            "--out" => out = Some(PathBuf::from(value)),
            "--metrics" => metrics = Some(PathBuf::from(value)),
            "--block" => {
                block = value
                    .parse()
                    .map_err(|_| "--block must be a positive integer")?;
                if block == 0 {
                    return Err("--block is 1-based".into());
                }
            }
            "--width" => options.width = value.parse().map_err(|_| "--width must be a number")?,
            "--theme" => {
                options.theme = match value.as_str() {
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    _ => return Err("--theme must be light or dark".into()),
                }
            }
            "--collapse" => {
                options.collapsed = value
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect::<HashSet<_>>()
            }
            _ => return Err(format!("unknown option {option}")),
        }
    }
    Ok(Some(Arguments {
        input: input.into(),
        out: out.ok_or("--out is required")?,
        metrics,
        block,
        options,
    }))
}

fn run(args: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    // 源文件只读；禁止输出覆盖笔记或让两个输出相互覆盖。
    let destination = |path: &std::path::Path| -> std::io::Result<PathBuf> {
        if path.exists() {
            path.canonicalize()
        } else {
            Ok(path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."))
                .canonicalize()?
                .join(
                    path.file_name()
                        .ok_or_else(|| std::io::Error::other("Output path has no file name"))?,
                ))
        }
    };
    let input = args.input.canonicalize()?;
    let out = destination(&args.out)?;
    let metrics_path = args.metrics.as_deref().map(destination).transpose()?;
    if same_file(&input, &out)
        || metrics_path
            .as_ref()
            .is_some_and(|m| same_file(m, &input) || same_file(m, &out))
    {
        return Err("Input, SVG, and metrics must use different files".into());
    }
    let markdown = std::fs::read_to_string(&input)?;
    let block = mermaid_block(&markdown, args.block)?;
    let tree = parse_at(block.source, block.source_range.start)?;
    let mut fonts = SystemFonts::new()?;
    let scene = layout(&tree, &args.options, &mut fonts)?;
    std::fs::write(out, scene.to_svg())?;
    if let Some(path) = metrics_path {
        std::fs::write(path, serde_json::to_string_pretty(&scene.metrics)? + "\n")?;
    }
    println!(
        "{} nodes, {} columns, {:.2}px tall, laid out in {:.3}ms",
        scene.hit_regions.len(),
        scene.metrics.columns,
        scene.height,
        scene.metrics.layout_ms
    );
    Ok(())
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

fn main() -> std::process::ExitCode {
    let result = match arguments() {
        Ok(Some(args)) => run(args),
        Ok(None) => {
            println!("{USAGE}");
            Ok(())
        }
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
