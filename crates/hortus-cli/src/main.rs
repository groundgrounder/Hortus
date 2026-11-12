use hortus_diagram::{
    Diagram, LayoutOptions, SystemFonts, Theme, layout, layout_graph, mermaid_block,
};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "Usage: hortus guide\n       hortus mermaid render <note.md> [--block N] [--width 700] [--theme light|dark] [--collapse id,id] --out diagram.svg [--metrics metrics.json] [--scene scene.json]";

enum Command {
    Usage,
    Guide,
    Render(Box<Arguments>),
}

struct Arguments {
    input: PathBuf,
    out: PathBuf,
    metrics: Option<PathBuf>,
    scene: Option<PathBuf>,
    block: usize,
    options: LayoutOptions,
}

fn arguments() -> Result<Command, String> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(Command::Usage);
    };
    match first.as_str() {
        "--help" | "-h" => return Ok(Command::Usage),
        "guide" => {
            if let Some(extra) = args.next() {
                return Err(format!("unknown argument {extra}"));
            }
            return Ok(Command::Guide);
        }
        "mermaid" => {}
        other => return Err(format!("unknown command {other}")),
    }
    if args.next().as_deref() != Some("render") {
        return Err(USAGE.into());
    }
    let input = args.next().filter(|s| !s.starts_with('-')).ok_or(USAGE)?;
    let mut out = None;
    let mut metrics = None;
    let mut scene = None;
    let mut block = 1;
    let mut options = LayoutOptions::default();
    while let Some(option) = args.next() {
        if option == "--help" || option == "-h" {
            return Ok(Command::Usage);
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_str() {
            "--out" => out = Some(PathBuf::from(value)),
            "--metrics" => metrics = Some(PathBuf::from(value)),
            "--scene" => scene = Some(PathBuf::from(value)),
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
    Ok(Command::Render(Box::new(Arguments {
        input: input.into(),
        out: out.ok_or("--out is required")?,
        metrics,
        scene,
        block,
        options,
    })))
}

/// 写出有效指南原文；只读取配置目录，不创建目录、不迁移旧数据。
fn run_guide() -> std::io::Result<()> {
    let directories = hortus_config::Directories::resolve_from_env()?;
    let guide = hortus_config::effective_guide(&directories.config)?;
    let mut output = std::io::stdout().lock();
    output.write_all(guide.as_bytes())?;
    output.flush()
}

fn describe(error: &std::io::Error) -> String {
    match error.to_string().as_str() {
        "guide.invalid_utf8" => "agent-guide.md is not valid UTF-8".into(),
        "guide.empty" => "agent-guide.md is empty".into(),
        "guide.read_failed" => "unable to read agent-guide.md".into(),
        "config.home_missing" => "HOME is not set".into(),
        "config.path_not_absolute" => "configuration paths must be absolute".into(),
        other if other.starts_with("guide.") || other.starts_with("config.") => other.into(),
        other => other.into(),
    }
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
    let scene_path = args.scene.as_deref().map(destination).transpose()?;
    if same_file(&input, &out)
        || metrics_path
            .as_ref()
            .is_some_and(|m| same_file(m, &input) || same_file(m, &out))
        || scene_path.as_ref().is_some_and(|s| {
            same_file(s, &input)
                || same_file(s, &out)
                || metrics_path.as_ref().is_some_and(|m| same_file(s, m))
        })
    {
        return Err("Input, SVG, and metrics must use different files".into());
    }
    let markdown = std::fs::read_to_string(&input)?;
    let block = mermaid_block(&markdown, args.block)?;
    let diagram = block.parse_diagram()?;
    let mut fonts = SystemFonts::new()?;
    let scene = match diagram {
        Diagram::Tree(tree) => layout(&tree, &args.options, &mut fonts)?,
        Diagram::Graph(graph) => layout_graph(&graph, &args.options, &mut fonts)?.scene,
    };
    std::fs::write(out, scene.to_svg())?;
    if let Some(path) = metrics_path {
        std::fs::write(path, serde_json::to_string_pretty(&scene.metrics)? + "\n")?;
    }
    if let Some(path) = scene_path {
        std::fs::write(path, serde_json::to_string(&scene.semantic_scene())? + "\n")?;
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
    match arguments() {
        Ok(Command::Usage) => {
            println!("{USAGE}");
            std::process::ExitCode::SUCCESS
        }
        Ok(Command::Guide) => match run_guide() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{}", describe(&error));
                std::process::ExitCode::FAILURE
            }
        },
        Ok(Command::Render(args)) => match run(*args) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
