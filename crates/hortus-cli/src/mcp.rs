//! `hortus mcp`: a read-only Model Context Protocol server over stdio.
//!
//! Every call loads the vault from disk again, so notes created or edited
//! outside Hortus are visible immediately. Nothing is written to the vault and
//! diagnostics go to stderr; stdout carries one JSON-RPC message per line.

use crate::mermaid_check;
use crate::query::{self, Vault};
use hortus_config::Directories;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;

const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const FALLBACK_PROTOCOL_VERSION: &str = "2025-03-26";
const GUIDE_URI: &str = "hortus://guide";

/// 当前生效的 Agent 指南；只读配置目录，不创建目录。
pub fn guide_text() -> Result<String, String> {
    let directories = Directories::resolve_from_env().map_err(|error| crate::describe(&error))?;
    hortus_config::effective_guide(&directories.config).map_err(|error| crate::describe(&error))
}

/// 启动服务直到 stdin 结束；库根在建立连接前锁定。
pub fn run(vault: Option<&Path>) -> Result<(), String> {
    let root = query::resolve_root(vault)?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = input
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("unable to read stdin: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let Some(response) = respond(&bytes, &root) else {
            continue;
        };
        serde_json::to_writer(&mut output, &response)
            .map_err(|error| format!("unable to write JSON: {error}"))?;
        output
            .write_all(b"\n")
            .and_then(|()| output.flush())
            .map_err(|error| format!("unable to write JSON: {error}"))?;
    }
}

/// 解析一行 JSON-RPC；通知返回 `None`，其余错误也用 JSON-RPC 错误包回答。
fn respond(line: &[u8], root: &Path) -> Option<Value> {
    let message: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_) => return Some(error_response(Value::Null, -32700, "Parse error")),
    };
    let Some(object) = message.as_object() else {
        return Some(error_response(Value::Null, -32600, "Invalid Request"));
    };
    let has_id = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return has_id.then(|| error_response(id, -32600, "Invalid Request"));
    };
    if method.starts_with("notifications/") {
        return None;
    }
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let response = match method {
        "initialize" => success_response(id, initialize_result(&params)),
        "ping" => success_response(id, json!({})),
        "tools/list" => success_response(id, json!({ "tools": tools() })),
        "tools/call" => call_tool(id, &params, root),
        "resources/list" => success_response(id, json!({ "resources": resources() })),
        "resources/read" => read_resource(id, &params),
        _ => error_response(id, -32601, "Method not found"),
    };
    has_id.then_some(response)
}

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let protocol_version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        FALLBACK_PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {}, "resources": {} },
        "serverInfo": { "name": "hortus", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

fn tool(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": annotations(),
    })
}

fn tools() -> Vec<Value> {
    let note = json!({ "type": "string", "description": "Vault-relative note path or note name" });
    vec![
        tool(
            "hortus_search",
            "Search the vault for a phrase and return matching notes with snippets.",
            json!({
                "query": { "type": "string", "description": "Text to search for" },
                "limit": { "type": "integer", "minimum": 0, "description": "Maximum results (default 50)" },
            }),
            vec!["query"],
        ),
        tool(
            "hortus_backlinks",
            "List every note that links to the given note.",
            json!({ "note": note }),
            vec!["note"],
        ),
        tool(
            "hortus_links",
            "List every outgoing link in the given note with its resolution.",
            json!({ "note": note }),
            vec!["note"],
        ),
        tool(
            "hortus_tags",
            "List the tags in the vault, or the notes carrying one tag.",
            json!({ "tag": { "type": "string", "description": "Optional tag to filter by" } }),
            vec![],
        ),
        tool(
            "hortus_outline",
            "Return the heading outline of the given note.",
            json!({ "note": note }),
            vec!["note"],
        ),
        tool(
            "hortus_orphans",
            "List notes that no other note links to.",
            json!({}),
            vec![],
        ),
        tool(
            "hortus_broken_links",
            "List links whose target note is missing or ambiguous.",
            json!({}),
            vec![],
        ),
        tool(
            "hortus_mermaid_check",
            "Validate the Mermaid diagrams in a note; optionally check only one block.",
            json!({
                "note": note,
                "block": { "type": "integer", "minimum": 1, "description": "Optional 1-based block number" },
            }),
            vec!["note"],
        ),
        tool(
            "hortus_guide",
            "Return the built-in or user-overridden Agent guide.",
            json!({}),
            vec![],
        ),
    ]
}

fn resources() -> Vec<Value> {
    vec![json!({
        "uri": GUIDE_URI,
        "name": "Hortus Agent Guide",
        "description": "The built-in or user-overridden guide for agents working in a Hortus vault.",
        "mimeType": "text/markdown",
    })]
}

enum CallError {
    /// Malformed tool arguments; reported as JSON-RPC -32602.
    Invalid(String),
    /// A valid request whose result is an error (missing note, invalid diagram).
    Tool(String),
}

fn call_tool(id: Value, params: &Value, root: &Path) -> Value {
    let Some(params) = params.as_object() else {
        return error_response(id, -32602, "Invalid params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "Invalid params: tool name is required");
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(arguments)) => arguments.clone(),
        Some(_) => {
            return error_response(id, -32602, "Invalid params: arguments must be an object");
        }
    };
    let arguments = Value::Object(arguments);
    if !tools().iter().any(|tool| tool["name"] == name) {
        return error_response(id, -32602, &format!("Unknown tool: {name}"));
    }
    let vault = match Vault::load(Some(root)) {
        Ok(vault) => vault,
        Err(error) => return tool_error(id, &error),
    };
    let outcome = match name {
        "hortus_search" => search(&vault, &arguments),
        "hortus_backlinks" => note_query(&vault, &arguments, query::backlinks_value),
        "hortus_links" => note_query(&vault, &arguments, query::links_value),
        "hortus_tags" => tags(&vault, &arguments),
        "hortus_outline" => note_query(&vault, &arguments, query::outline_value),
        "hortus_orphans" => Ok(query::orphans_value(&vault)),
        "hortus_broken_links" => Ok(query::broken_links_value(&vault)),
        "hortus_mermaid_check" => mermaid(&vault, &arguments),
        "hortus_guide" => guide(),
        _ => unreachable!("tool names are checked above"),
    };
    match outcome {
        Ok(result) => {
            // 指南以原文返回，结构化副本仍供程序读取。
            let text = if name == "hortus_guide" {
                result["guide"].as_str().map(str::to_owned)
            } else {
                None
            };
            success_response(id, tool_success(result, text.as_deref()))
        }
        Err(CallError::Invalid(message)) => error_response(id, -32602, &message),
        Err(CallError::Tool(message)) => tool_error(id, &message),
    }
}

fn tool_success(result: Value, text: Option<&str>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.map_or_else(|| result.to_string(), str::to_owned) }],
        "structuredContent": result,
        "isError": false,
    })
}

fn tool_error(id: Value, message: &str) -> Value {
    success_response(
        id,
        json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    )
}

fn invalid_params(message: impl Into<String>) -> CallError {
    CallError::Invalid(message.into())
}

fn required_string(arguments: &Value, key: &str) -> Result<String, CallError> {
    match arguments.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(invalid_params(format!("{key} is empty"))),
        Some(Value::Null) | None => Err(invalid_params(format!("{key} is required"))),
        Some(_) => Err(invalid_params(format!("{key} must be a string"))),
    }
}

fn optional_string(arguments: &Value, key: &str) -> Result<Option<String>, CallError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_params(format!("{key} must be a string"))),
    }
}

fn optional_integer(arguments: &Value, key: &str) -> Result<Option<i64>, CallError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| invalid_params(format!("{key} must be an integer"))),
        Some(_) => Err(invalid_params(format!("{key} must be an integer"))),
    }
}

fn is_path_error(message: &str) -> bool {
    message.contains("escapes the vault")
        || message.contains("outside the vault")
        || message.contains("symbolic links")
        || message.contains("invalid note path")
        || message.contains("note path is empty")
}

fn resolve_note(vault: &Vault, note: &str) -> Result<String, CallError> {
    vault.resolve_note(note).map_err(|message| {
        if is_path_error(&message) {
            invalid_params(message)
        } else {
            CallError::Tool(message)
        }
    })
}

fn search(vault: &Vault, arguments: &Value) -> Result<Value, CallError> {
    let query = required_string(arguments, "query")?;
    let limit = optional_integer(arguments, "limit")?.unwrap_or(50);
    let limit = usize::try_from(limit)
        .map_err(|_| invalid_params("limit must be a non-negative integer"))?;
    Ok(query::search_value(vault, &query, limit))
}

fn note_query(
    vault: &Vault,
    arguments: &Value,
    operation: fn(&Vault, &str) -> Result<Value, String>,
) -> Result<Value, CallError> {
    let note = required_string(arguments, "note")?;
    let note = resolve_note(vault, &note)?;
    operation(vault, &note).map_err(CallError::Tool)
}

fn tags(vault: &Vault, arguments: &Value) -> Result<Value, CallError> {
    let tag = optional_string(arguments, "tag")?;
    Ok(query::tags_value(vault, tag.as_deref()))
}

fn mermaid(vault: &Vault, arguments: &Value) -> Result<Value, CallError> {
    let note = required_string(arguments, "note")?;
    let note = resolve_note(vault, &note)?;
    let block = optional_integer(arguments, "block")?;
    let block = block
        .map(|block| {
            usize::try_from(block)
                .ok()
                .filter(|block| *block > 0)
                .ok_or_else(|| invalid_params("block must be a positive integer"))
        })
        .transpose()?;
    let file = vault
        .notes
        .get(&note)
        .ok_or_else(|| CallError::Tool(format!("note not found: {note}")))?;
    let (_, report) = mermaid_check::evaluate(&file.text, &note, block).map_err(CallError::Tool)?;
    Ok(report)
}

fn guide() -> Result<Value, CallError> {
    let text = guide_text().map_err(CallError::Tool)?;
    Ok(json!({ "guide": text }))
}

fn read_resource(id: Value, params: &Value) -> Value {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return error_response(id, -32602, "Invalid params: uri is required");
    };
    if uri != GUIDE_URI {
        return error_response(id, -32002, "Resource not found");
    }
    match guide_text() {
        Ok(text) => success_response(
            id,
            json!({
                "contents": [{ "uri": GUIDE_URI, "mimeType": "text/markdown", "text": text }]
            }),
        ),
        Err(error) => error_response(id, -32603, &error),
    }
}
