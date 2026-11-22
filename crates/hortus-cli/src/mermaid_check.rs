//! `hortus mermaid check`: validate every Mermaid fence in a note.

use hortus_diagram::{Diagram, DiagramError, mermaid_block, mermaid_blocks};
use hortus_vault::note::Format;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Prints the JSON report and returns whether every checked block is valid.
pub fn run(input: &Path, block: Option<usize>) -> Result<bool, String> {
    let bytes = std::fs::read(input)
        .map_err(|error| format!("unable to read {}: {error}", input.display()))?;
    let (_format, text, _valid) = Format::decode(&bytes);
    let mut entries = Vec::new();
    if let Some(requested) = block {
        let block = mermaid_block(&text, requested).map_err(|error| match error {
            DiagramError::MissingBlock { requested } => {
                format!("MissingBlock: requested={requested}")
            }
            other => other.to_string(),
        })?;
        entries.push(report(&text, requested, &block));
    } else {
        for (index, block) in mermaid_blocks(&text).iter().enumerate() {
            entries.push(report(&text, index + 1, block));
        }
    }
    let valid = entries
        .iter()
        .all(|entry| entry["valid"].as_bool().unwrap_or(false));
    emit(&json!({
        "file": input.display().to_string(),
        "valid": valid,
        "blocks": entries,
    }))?;
    Ok(valid)
}

fn report(text: &str, number: usize, block: &hortus_diagram::MermaidBlock<'_>) -> Value {
    match block.parse_diagram() {
        Ok(diagram) => {
            json!({
                "block": number,
                "diagram_type": diagram_type(&diagram),
                "valid": true,
                "error_kind": Value::Null,
                "line": line_of(text, block.fence_range.start),
                "message": Value::Null,
            })
        }
        Err(error) => {
            let line = match &error {
                DiagramError::Syntax { range, .. }
                | DiagramError::UnsupportedConstruct { range, .. } => range.start,
                _ => block.fence_range.start,
            };
            json!({
                "block": number,
                "diagram_type": infer_type(&block.source),
                "valid": false,
                "error_kind": error_kind(&error),
                "line": line_of(text, line),
                "message": error.to_string(),
            })
        }
    }
}

fn diagram_type(diagram: &Diagram) -> &'static str {
    match diagram {
        Diagram::Tree(_) => "tree",
        Diagram::Graph(_) => "flowchart",
        Diagram::Sequence(_) => "sequence",
        Diagram::State(_) => "state",
    }
}

fn infer_type(source: &str) -> String {
    match hortus_diagram::infer_diagram_header(source) {
        Some("stateDiagram" | "stateDiagram-v2") => "state".into(),
        Some("mindmap") => "tree".into(),
        Some("flowchart" | "graph") => "flowchart".into(),
        Some("sequenceDiagram") => "sequence".into(),
        Some(other) => other.to_owned(),
        None => "unknown".into(),
    }
}

fn error_kind(error: &DiagramError) -> &'static str {
    match error {
        DiagramError::Unsupported { .. } => "unsupported",
        DiagramError::UnsupportedConstruct { .. } => "unsupported_construct",
        DiagramError::NotATree { .. } => "not_a_tree",
        DiagramError::Syntax { .. } => "syntax",
        DiagramError::MissingBlock { .. } => "missing_block",
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn emit(value: &Value) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)
        .map_err(|error| format!("unable to write JSON: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("unable to write JSON: {error}"))
}
