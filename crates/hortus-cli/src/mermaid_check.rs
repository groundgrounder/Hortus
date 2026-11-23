//! Validate Mermaid fences in Markdown or a standalone Mermaid diagram.

use hortus_diagram::{
    Diagram, DiagramError, MermaidBlock, mermaid_block, mermaid_blocks, parse_diagram_at,
};
use hortus_vault::note::Format;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Prints the JSON report and returns whether every checked block is valid.
pub fn run(input: &Path, block: Option<usize>) -> Result<bool, String> {
    let bytes = std::fs::read(input)
        .map_err(|error| format!("unable to read {}: {error}", input.display()))?;
    let (_format, text, _valid) = Format::decode(&bytes);
    let file = input.to_string_lossy();
    let (valid, report) = evaluate(&text, &file, hortus_links::is_mmd_path(&file), block)?;
    emit(&report)?;
    Ok(valid)
}

pub enum DiagramBlock<'a> {
    Standalone(&'a str),
    Fenced(MermaidBlock<'a>),
}

impl DiagramBlock<'_> {
    pub fn parse(&self) -> Result<Diagram, DiagramError> {
        match self {
            Self::Standalone(text) => parse_diagram_at(text, 0),
            Self::Fenced(block) => block.parse_diagram(),
        }
    }

    fn source(&self) -> &str {
        match self {
            Self::Standalone(text) => text,
            Self::Fenced(block) => &block.source,
        }
    }

    fn start(&self) -> usize {
        match self {
            Self::Standalone(_) => 0,
            Self::Fenced(block) => block.fence_range.start,
        }
    }
}

pub fn diagram_block(
    text: &str,
    mmd: bool,
    requested: usize,
) -> Result<DiagramBlock<'_>, DiagramError> {
    match (mmd, requested) {
        (true, 1) => Ok(DiagramBlock::Standalone(text)),
        (true, _) => Err(DiagramError::MissingBlock { requested }),
        (false, _) => mermaid_block(text, requested).map(DiagramBlock::Fenced),
    }
}

/// 对已解码的正文执行同一套校验；MCP 与命令行共用，避免两套解析。
pub fn evaluate(
    text: &str,
    file: &str,
    mmd: bool,
    block: Option<usize>,
) -> Result<(bool, Value), String> {
    let entries = if let Some(requested) = block.or(mmd.then_some(1)) {
        let block = diagram_block(text, mmd, requested).map_err(|error| error.to_string())?;
        vec![report(text, requested, &block)]
    } else {
        mermaid_blocks(text)
            .into_iter()
            .enumerate()
            .map(|(index, block)| report(text, index + 1, &DiagramBlock::Fenced(block)))
            .collect()
    };
    let valid = entries
        .iter()
        .all(|entry| entry["valid"].as_bool().unwrap_or(false));
    Ok((
        valid,
        json!({
            "file": file,
            "valid": valid,
            "blocks": entries,
        }),
    ))
}

fn report(text: &str, number: usize, block: &DiagramBlock<'_>) -> Value {
    match block.parse() {
        Ok(diagram) => {
            json!({
                "block": number,
                "diagram_type": diagram_type(&diagram),
                "valid": true,
                "error_kind": Value::Null,
                "line": line_of(text, block.start()),
                "message": Value::Null,
            })
        }
        Err(error) => {
            let line = match &error {
                DiagramError::Syntax { range, .. }
                | DiagramError::UnsupportedConstruct { range, .. } => range.start,
                _ => block.start(),
            };
            json!({
                "block": number,
                "diagram_type": infer_type(block.source()),
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
        Diagram::Class(_) => "class",
        Diagram::Er(_) => "er",
    }
}

fn infer_type(source: &str) -> String {
    match hortus_diagram::infer_diagram_header(source) {
        Some("stateDiagram" | "stateDiagram-v2") => "state".into(),
        Some("classDiagram" | "classDiagram-v2") => "class".into(),
        Some("erDiagram") => "er".into(),
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
