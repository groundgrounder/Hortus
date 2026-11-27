mod mcp;
mod mermaid_check;
mod query;

use hortus_diagram::{
    Diagram, LayoutOptions, SystemFonts, Theme, layout, layout_class, layout_er, layout_graph,
    layout_sequence, layout_state,
};
use hortus_vault::note::Format;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "Usage: hortus --version
       hortus guide
       hortus mcp [--vault DIR]
       hortus mermaid render <note.md|diagram.mmd> [--block N] [--width 700] [--theme light|dark] [--collapse id,id] --out diagram.svg [--metrics metrics.json] [--scene scene.json]
       hortus mermaid check <note.md|diagram.mmd> [--block N]
       hortus search <query> [--vault DIR] [--limit N]
       hortus backlinks <note> [--vault DIR]
       hortus links <note> [--vault DIR]
       hortus tags [tag] [--vault DIR]
       hortus outline <note> [--vault DIR]
       hortus orphans [--vault DIR]
       hortus broken-links [--vault DIR]";

enum Command {
    Usage,
    Version,
    Guide,
    Mcp {
        vault: Option<PathBuf>,
    },
    Render(Box<Arguments>),
    Search {
        query: String,
        vault: Option<PathBuf>,
        limit: usize,
    },
    Backlinks {
        note: String,
        vault: Option<PathBuf>,
    },
    Links {
        note: String,
        vault: Option<PathBuf>,
    },
    Tags {
        tag: Option<String>,
        vault: Option<PathBuf>,
    },
    Outline {
        note: String,
        vault: Option<PathBuf>,
    },
    Orphans {
        vault: Option<PathBuf>,
    },
    BrokenLinks {
        vault: Option<PathBuf>,
    },
    MermaidCheck {
        input: PathBuf,
        block: Option<usize>,
    },
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
        "--version" | "-V" => {
            if let Some(extra) = args.next() {
                return Err(format!("unknown argument {extra}"));
            }
            return Ok(Command::Version);
        }
        "guide" => {
            if let Some(extra) = args.next() {
                return Err(format!("unknown argument {extra}"));
            }
            return Ok(Command::Guide);
        }
        "mcp" => {
            let mut vault = None;
            while let Some(option) = args.next() {
                match option.as_str() {
                    "--help" | "-h" => return Ok(Command::Usage),
                    "--vault" => {
                        let value = args.next().ok_or("--vault requires a value")?;
                        vault = Some(PathBuf::from(value));
                    }
                    other => return Err(format!("unknown option {other}")),
                }
            }
            return Ok(Command::Mcp { vault });
        }
        "mermaid" => {
            let Some(subcommand) = args.next() else {
                return Err(USAGE.into());
            };
            match subcommand.as_str() {
                "render" => {}
                "check" => return check_command(args.collect()),
                other => return Err(format!("unknown command mermaid {other}")),
            }
        }
        "search" | "backlinks" | "links" | "tags" | "outline" | "orphans" | "broken-links" => {
            return query_command(first.as_str(), args.collect());
        }
        other => return Err(format!("unknown command {other}")),
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

fn query_command(name: &str, args: Vec<String>) -> Result<Command, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(Command::Usage);
    }
    let mut positionals = Vec::new();
    let mut vault = None;
    let mut limit = None;
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--vault" => {
                let value = iter.next().ok_or("--vault requires a value")?;
                vault = Some(PathBuf::from(value));
            }
            "--limit" if name == "search" => {
                let value = iter.next().ok_or("--limit requires a value")?;
                limit = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "--limit must be a non-negative integer")?,
                );
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other}"));
            }
            other => positionals.push(other.to_owned()),
        }
    }
    match name {
        "search" => {
            if positionals.len() != 1 {
                return Err("usage: hortus search <query> [--vault DIR] [--limit N]".into());
            }
            Ok(Command::Search {
                query: positionals.remove(0),
                vault,
                limit: limit.unwrap_or(50),
            })
        }
        "backlinks" => {
            if positionals.len() != 1 {
                return Err("usage: hortus backlinks <note> [--vault DIR]".into());
            }
            Ok(Command::Backlinks {
                note: positionals.remove(0),
                vault,
            })
        }
        "links" => {
            if positionals.len() != 1 {
                return Err("usage: hortus links <note> [--vault DIR]".into());
            }
            Ok(Command::Links {
                note: positionals.remove(0),
                vault,
            })
        }
        "tags" => {
            if positionals.len() > 1 {
                return Err("usage: hortus tags [tag] [--vault DIR]".into());
            }
            Ok(Command::Tags {
                tag: positionals.pop(),
                vault,
            })
        }
        "outline" => {
            if positionals.len() != 1 {
                return Err("usage: hortus outline <note> [--vault DIR]".into());
            }
            Ok(Command::Outline {
                note: positionals.remove(0),
                vault,
            })
        }
        "orphans" => {
            if !positionals.is_empty() {
                return Err("usage: hortus orphans [--vault DIR]".into());
            }
            Ok(Command::Orphans { vault })
        }
        "broken-links" => {
            if !positionals.is_empty() {
                return Err("usage: hortus broken-links [--vault DIR]".into());
            }
            Ok(Command::BrokenLinks { vault })
        }
        _ => Err(USAGE.into()),
    }
}

fn check_command(args: Vec<String>) -> Result<Command, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(Command::Usage);
    }
    let mut input = None;
    let mut block = None;
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--block" => {
                let value = iter.next().ok_or("--block requires a value")?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "--block must be a positive integer")?;
                if parsed == 0 {
                    return Err("--block is 1-based".into());
                }
                block = Some(parsed);
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other}"));
            }
            other => {
                if input.is_some() {
                    return Err(USAGE.into());
                }
                input = Some(PathBuf::from(other));
            }
        }
    }
    Ok(Command::MermaidCheck {
        input: input.ok_or(USAGE)?,
        block,
    })
}

/// 写出有效指南原文；只读取配置目录，不创建目录、不迁移旧数据。
fn run_guide() -> std::io::Result<()> {
    let guide = mcp::guide_text().map_err(std::io::Error::other)?;
    let mut output = std::io::stdout().lock();
    output.write_all(guide.as_bytes())?;
    output.flush()
}

pub(crate) fn describe(error: &std::io::Error) -> String {
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
    let mmd = hortus_links::is_mmd_path(&args.input.to_string_lossy());
    let text = if mmd {
        let bytes = std::fs::read(&input)?;
        let (_, text, valid) = Format::decode(&bytes);
        if !valid {
            return Err(
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8").into(),
            );
        }
        text
    } else {
        std::fs::read_to_string(&input)?
    };
    let diagram = mermaid_check::diagram_block(&text, mmd, args.block)?.parse()?;
    let mut fonts = SystemFonts::new()?;
    let scene = match diagram {
        Diagram::Tree(tree) => layout(&tree, &args.options, &mut fonts)?,
        Diagram::Graph(graph) => layout_graph(&graph, &args.options, &mut fonts)?.scene,
        Diagram::Sequence(sequence) => layout_sequence(&sequence, &args.options, &mut fonts)?,
        Diagram::State(state) => layout_state(&state, &args.options, &mut fonts)?,
        Diagram::Class(class) => layout_class(&class, &args.options, &mut fonts)?,
        Diagram::Er(er) => layout_er(&er, &args.options, &mut fonts)?,
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

fn finish(result: Result<(), String>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn main() -> std::process::ExitCode {
    match arguments() {
        Ok(Command::Usage) => {
            println!("{USAGE}");
            std::process::ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("hortus {}", env!("CARGO_PKG_VERSION"));
            std::process::ExitCode::SUCCESS
        }
        Ok(Command::Guide) => match run_guide() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{}", describe(&error));
                std::process::ExitCode::FAILURE
            }
        },
        Ok(Command::Mcp { vault }) => match mcp::run(vault.as_deref()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
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
        Ok(Command::Search {
            query,
            vault,
            limit,
        }) => finish(crate::query::search(vault.as_deref(), &query, limit)),
        Ok(Command::Backlinks { note, vault }) => {
            finish(crate::query::backlinks(vault.as_deref(), &note))
        }
        Ok(Command::Links { note, vault }) => finish(crate::query::links(vault.as_deref(), &note)),
        Ok(Command::Tags { tag, vault }) => {
            finish(crate::query::tags(vault.as_deref(), tag.as_deref()))
        }
        Ok(Command::Outline { note, vault }) => {
            finish(crate::query::outline(vault.as_deref(), &note))
        }
        Ok(Command::Orphans { vault }) => finish(crate::query::orphans(vault.as_deref())),
        Ok(Command::BrokenLinks { vault }) => finish(crate::query::broken_links(vault.as_deref())),
        Ok(Command::MermaidCheck { input, block }) => match mermaid_check::run(&input, block) {
            Ok(true) => std::process::ExitCode::SUCCESS,
            Ok(false) => std::process::ExitCode::FAILURE,
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
