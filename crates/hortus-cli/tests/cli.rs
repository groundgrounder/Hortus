use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);

fn contains_han(value: &str) -> bool {
    value.chars().any(
        |character| matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff),
    )
}

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hortus-cli-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn invoke(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hortus"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn render_selects_block_theme_width_and_collapsed_nodes() {
    let sandbox = Sandbox::new();
    let markdown = "# Integration fixture\n```mermaid\nsequenceDiagram\nA->>B: first block\n```\n~~~mermaid\nflowchart TB\nR[Root] --> A[Branch]\nA --> B[Hidden one]\nA --> C[Hidden two]\nR --> D[Other section]\n~~~\n";
    std::fs::create_dir(sandbox.0.join("notes")).unwrap();
    std::fs::write(sandbox.0.join("notes/note.md"), markdown).unwrap();
    let result = sandbox.invoke(&[
        "mermaid",
        "render",
        "notes/note.md",
        "--block",
        "2",
        "--width",
        "1100",
        "--theme",
        "dark",
        "--collapse",
        "A",
        "--out",
        "diagram.svg",
        "--metrics",
        "metrics.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = std::fs::read_to_string(sandbox.0.join("diagram.svg")).unwrap();
    assert!(svg.contains("<svg") && svg.contains("+2") && svg.contains("textLength="));
    assert!(!svg.contains("Hidden"));
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sandbox.0.join("metrics.json")).unwrap())
            .unwrap();
    assert_eq!(metrics["columns"], 2);
    assert_eq!(metrics["horizontal_overflow"], false);
    assert_eq!(metrics["node_overlaps"], 0);
    assert!(metrics["min_font_px"].as_f64().unwrap() >= 13.6);
    assert!(!metrics["fonts"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(sandbox.0.join("notes/note.md")).unwrap(),
        markdown
    );
    assert_eq!(
        std::fs::read_dir(sandbox.0.join("notes")).unwrap().count(),
        1
    );
}

#[test]
fn errors_have_nonzero_exit_codes_and_do_not_write_output() {
    let sandbox = Sandbox::new();
    for (source, diagnostic) in [
        ("sequenceDiagram\nA->>B: message", "Unsupported"),
        ("flowchart TB\nA --> B --> D\nA --> C --> D", "NotATree"),
        ("flowchart TB\nA[unclosed", "Syntax"),
    ] {
        std::fs::write(
            sandbox.0.join("note.md"),
            format!("```mermaid\n{source}\n```\n"),
        )
        .unwrap();
        let result = sandbox.invoke(&["mermaid", "render", "note.md", "--out", "diagram.svg"]);
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(diagnostic));
        assert!(
            !contains_han(&stderr),
            "CLI error output must be English: {stderr}"
        );
        assert!(!sandbox.0.join("diagram.svg").exists());
    }
    std::fs::write(
        sandbox.0.join("note.md"),
        "```mermaid\nflowchart TB\nA --> B\n```\n",
    )
    .unwrap();
    for options in [
        vec!["--block", "0"],
        vec!["--block", "2"],
        vec!["--width", "NaN"],
        vec!["--width", "100"],
        vec!["--theme", "invalid"],
        vec!["--unknown", "value"],
    ] {
        let mut args = vec!["mermaid", "render", "note.md", "--out", "diagram.svg"];
        args.extend(options);
        assert!(!sandbox.invoke(&args).status.success());
    }
    let help = sandbox.invoke(&["--help"]);
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        stdout.starts_with("Usage: hortus mermaid render"),
        "{stdout}"
    );
    assert!(
        !contains_han(&stdout),
        "CLI help output must be English: {stdout}"
    );
    assert!(
        !sandbox
            .invoke(&["mermaid", "render", "note.md"])
            .status
            .success()
    );
}

#[test]
fn never_overwrites_source_or_conflicting_outputs() {
    let sandbox = Sandbox::new();
    let source = "```mermaid\nflowchart TB\nA --> B\n```\n";
    std::fs::write(sandbox.0.join("note.md"), source).unwrap();
    assert!(
        !sandbox
            .invoke(&["mermaid", "render", "note.md", "--out", "note.md"])
            .status
            .success()
    );
    assert!(
        !sandbox
            .invoke(&[
                "mermaid",
                "render",
                "note.md",
                "--out",
                "diagram.svg",
                "--metrics",
                "diagram.svg"
            ])
            .status
            .success()
    );
    assert!(
        !sandbox
            .invoke(&[
                "mermaid",
                "render",
                "note.md",
                "--out",
                "diagram.svg",
                "--metrics",
                "note.md"
            ])
            .status
            .success()
    );
    #[cfg(unix)]
    {
        std::fs::hard_link(sandbox.0.join("note.md"), sandbox.0.join("hard.md")).unwrap();
        assert!(
            !sandbox
                .invoke(&["mermaid", "render", "note.md", "--out", "hard.md"])
                .status
                .success()
        );
        std::os::unix::fs::symlink(sandbox.0.join("note.md"), sandbox.0.join("alias.md")).unwrap();
        assert!(
            !sandbox
                .invoke(&["mermaid", "render", "note.md", "--out", "alias.md"])
                .status
                .success()
        );
    }
    assert_eq!(
        std::fs::read_to_string(sandbox.0.join("note.md")).unwrap(),
        source
    );
}
