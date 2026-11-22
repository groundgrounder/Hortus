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
        ("gantt\ntitle 计划", "Unsupported"),
        (
            "flowchart TB\nsubgraph G\nsubgraph H\nA --> B\nend\nend",
            "Unsupported",
        ),
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
    assert!(stdout.starts_with("Usage: hortus"), "{stdout}");
    assert!(stdout.contains("hortus guide") && stdout.contains("hortus mcp [--vault DIR]"));
    assert!(stdout.contains("hortus mermaid render"));
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
    for arguments in [vec!["mcp", "--bogus"], vec!["mcp", "--vault"]] {
        let error = sandbox.invoke(&arguments);
        assert!(!error.status.success(), "{arguments:?}");
        assert!(String::from_utf8_lossy(&error.stderr).is_ascii());
    }
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

#[test]
fn sequence_diagrams_render_with_svg_metrics_and_scene() {
    let sandbox = Sandbox::new();
    let source = concat!(
        "# 时序图\n\n```mermaid\n",
        "sequenceDiagram\n",
        "  participant U as 你\n",
        "  participant M as Hortus\n",
        "  U->>M: 发送当前笔记\n",
        "  M-->>U: 刷新阅读视图，列出本次改动\n",
        "  Note over U,M: 本地渲染\n",
        "```\n",
    );
    std::fs::write(sandbox.0.join("note.md"), source).unwrap();
    let result = sandbox.invoke(&[
        "mermaid",
        "render",
        "note.md",
        "--width",
        "700",
        "--theme",
        "dark",
        "--out",
        "diagram.svg",
        "--metrics",
        "metrics.json",
        "--scene",
        "scene.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = std::fs::read_to_string(sandbox.0.join("diagram.svg")).unwrap();
    assert!(svg.contains("aria-label=\"Mermaid sequence diagram\""));
    assert!(svg.contains("stroke-dasharray") && svg.contains("你"));
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sandbox.0.join("metrics.json")).unwrap())
            .unwrap();
    assert_eq!(metrics["columns"], 1);
    assert_eq!(metrics["horizontal_overflow"], false);
    assert_eq!(metrics["node_overlaps"], 0);
    assert!(metrics["min_font_px"].as_f64().unwrap() >= 13.6);
    assert_eq!(metrics["diagram_kind"], "sequence");
    // 两次渲染的语义场景一致；hit 覆盖参与者、消息与便签。
    let scene: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sandbox.0.join("scene.json")).unwrap())
            .unwrap();
    let hits = scene["hit_regions"].as_array().unwrap();
    let ids: Vec<&str> = hits
        .iter()
        .map(|hit| hit["node_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["U", "M", "msg-1", "msg-2", "note-1"]);
    assert_eq!(scene["metrics"]["diagram_kind"], "sequence");
    let second = sandbox.invoke(&[
        "mermaid",
        "render",
        "note.md",
        "--width",
        "700",
        "--theme",
        "dark",
        "--out",
        "second.svg",
        "--scene",
        "second-scene.json",
    ]);
    assert!(second.status.success());
    assert_eq!(
        std::fs::read_to_string(sandbox.0.join("scene.json")).unwrap(),
        std::fs::read_to_string(sandbox.0.join("second-scene.json")).unwrap()
    );
}

#[test]
fn state_diagrams_render_with_svg_metrics_and_scene() {
    let sandbox = Sandbox::new();
    let fixture = include_str!("../../../fixtures/mermaid/state/lifecycle.mmd");
    std::fs::write(
        sandbox.0.join("note.md"),
        format!("\x60\x60\x60mermaid\n{fixture}\x60\x60\x60\n"),
    )
    .unwrap();
    let result = sandbox.invoke(&[
        "mermaid",
        "render",
        "note.md",
        "--width",
        "700",
        "--theme",
        "dark",
        "--out",
        "diagram.svg",
        "--metrics",
        "metrics.json",
        "--scene",
        "scene.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = std::fs::read_to_string(sandbox.0.join("diagram.svg")).unwrap();
    assert!(svg.contains("aria-label=\"Mermaid state diagram\"") && svg.contains("套接字"));
    let json_file = |name| {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(sandbox.0.join(name)).unwrap())
            .unwrap()
    };
    let metrics = json_file("metrics.json");
    assert_eq!(metrics["diagram_kind"], "state");
    assert_eq!(metrics["horizontal_overflow"], false);
    assert_eq!(metrics["node_overlaps"], 0);
    let scene = json_file("scene.json");
    assert_eq!(scene["metrics"]["layout_ms"], 0.0);
    assert_eq!(scene["hit_regions"].as_array().unwrap().len(), 11);
}

#[test]
fn class_diagrams_render_with_svg_metrics_and_scene() {
    let sandbox = Sandbox::new();
    let fixture = include_str!("../../../fixtures/mermaid/class/domain-model.mmd");
    std::fs::write(
        sandbox.0.join("note.md"),
        format!("\x60\x60\x60mermaid\n{fixture}\x60\x60\x60\n"),
    )
    .unwrap();
    let result = sandbox.invoke(&[
        "mermaid",
        "render",
        "note.md",
        "--width",
        "700",
        "--theme",
        "light",
        "--out",
        "diagram.svg",
        "--metrics",
        "metrics.json",
        "--scene",
        "scene.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = std::fs::read_to_string(sandbox.0.join("diagram.svg")).unwrap();
    assert!(
        svg.contains("aria-label=\"Mermaid class diagram\"") && svg.contains("OrderRepository")
    );
    let json_file = |name| {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(sandbox.0.join(name)).unwrap())
            .unwrap()
    };
    let metrics = json_file("metrics.json");
    assert_eq!(metrics["diagram_kind"], "class");
    assert_eq!(metrics["horizontal_overflow"], false);
    assert_eq!(metrics["node_overlaps"], 0);
    assert!(metrics["min_font_px"].as_f64().unwrap() >= 12.5);
    let scene = json_file("scene.json");
    assert_eq!(scene["metrics"]["layout_ms"], 0.0);
    let hits = scene["hit_regions"].as_array().unwrap();
    let ids = hits
        .iter()
        .map(|hit| hit["node_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), hits.len());
    assert!(hits.iter().any(|hit| hit["node_id"] == "class-Order"));
    assert!(hits.iter().any(|hit| hit["node_id"] == "relation-0"));
}

#[test]
fn er_diagrams_render_with_svg_metrics_and_scene() {
    let sandbox = Sandbox::new();
    let fixture = include_str!("../../../fixtures/mermaid/er/order-schema.mmd");
    std::fs::write(
        sandbox.0.join("note.md"),
        format!("\x60\x60\x60mermaid\n{fixture}\x60\x60\x60\n"),
    )
    .unwrap();
    let result = sandbox.invoke(&[
        "mermaid",
        "render",
        "note.md",
        "--width",
        "1100",
        "--theme",
        "dark",
        "--out",
        "diagram.svg",
        "--metrics",
        "metrics.json",
        "--scene",
        "scene.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = std::fs::read_to_string(sandbox.0.join("diagram.svg")).unwrap();
    assert!(svg.contains("aria-label=\"Mermaid entity-relationship diagram\""));
    let json_file = |name| {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(sandbox.0.join(name)).unwrap())
            .unwrap()
    };
    let metrics = json_file("metrics.json");
    assert_eq!(metrics["diagram_kind"], "er");
    assert_eq!(metrics["horizontal_overflow"], false);
    assert_eq!(metrics["node_overlaps"], 0);
    assert!(metrics["min_font_px"].as_f64().unwrap() >= 12.5);
    let scene = json_file("scene.json");
    assert_eq!(scene["metrics"]["layout_ms"], 0.0);
    let hits = scene["hit_regions"].as_array().unwrap();
    let ids = hits
        .iter()
        .map(|hit| hit["node_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), hits.len());
    assert!(hits.iter().any(|hit| hit["node_id"] == "entity-ORDER-ITEM"));
}
