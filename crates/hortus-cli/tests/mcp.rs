use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus};
use std::time::UNIX_EPOCH;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hortus")
}

struct Fixture {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        fs::create_dir_all(vault.join("guides")).unwrap();
        fs::create_dir_all(vault.join("其他")).unwrap();
        write(
            &vault.join("index.md"),
            concat!(
                "---\n",
                "tags: [topic, topic/sub]\n",
                "aliases: [Home]\n",
                "---\n",
                "# Index\n\n",
                "pipeline reference for [[guides/intro]] and [[missing]] and ![[image.png]].\n\n",
                "```mermaid\nflowchart TB\nA[\"[[guides/intro]]\"] --> B\n```\n",
            ),
        );
        write(
            &vault.join("guides/intro.md"),
            "# Intro\n## Setup\n\nBack [[index]].\n",
        );
        write(&vault.join("lonely.md"), "# Lonely\n\nNo links here.\n");
        write(
            &vault.join("second.md"),
            "# Second\n\n#topic #topic/other\n",
        );
        write(&vault.join("其他/带 空格.md"), "# 空格路径\n\n中文正文。\n");
        write(
            &vault.join("diagrams.md"),
            concat!(
                "```mermaid\nmindmap\n  Root\n    Child\n```\n\n",
                "```mermaid\nflowchart TB\nsubgraph G\nA --> B\nend\n```\n\n",
                "```mermaid\nsequenceDiagram\nA->>B: hello\n```\n\n",
                "```mermaid\nstateDiagram-v2\n[*] --> A\n```\n\n",
                "```mermaid\nclassDiagram\nOrder \"1\" *-- \"1..*\" LineItem : contains\n```\n\n",
                "```mermaid\nerDiagram\nUSER ||--o{ ORDER : places\n```\n",
            ),
        );
        write(
            &vault.join("invalid.md"),
            "```mermaid\nflowchart TB\nA[unclosed\n```\n",
        );
        let other = dir.path().join("outside.md");
        write(&other, "# Outside\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, vault.join("link.md")).unwrap();
        let config = dir.path().join("config");
        Self { dir, config }
    }

    fn vault(&self) -> PathBuf {
        self.dir.path().join("vault")
    }

    fn start(&self) -> Server {
        let mut command = self.command();
        command.args(["mcp", "--vault", self.vault().to_str().unwrap()]);
        Server::spawn(command)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .env("HORTUS_CONFIG_DIR", &self.config)
            .env("HORTUS_CACHE_DIR", self.dir.path().join("cache"))
            .env("HORTUS_DATA_DIR", self.dir.path().join("data"))
            .current_dir(self.dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    lines: Vec<String>,
    next_id: u64,
}

impl Server {
    fn spawn(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            lines: Vec::new(),
            next_id: 1,
        }
    }

    fn write(&mut self, value: &Value) {
        let line = serde_json::to_string(value).unwrap();
        self.write_raw(&line);
    }

    fn write_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "server closed stdout unexpectedly");
        self.lines.push(line.clone());
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("invalid JSON line: {line:?}: {error}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.write(&message);
        let response = self.read();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id);
        response
    }

    fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.write(&message);
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    fn finish(mut self) -> (ExitStatus, String) {
        self.stdin.take();
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).unwrap();
        self.lines.extend(rest.lines().map(str::to_owned));
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        let status = self.child.wait().unwrap();
        (status, stderr)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn structured(response: &Value) -> &Value {
    assert_eq!(response["result"]["isError"], false, "{response}");
    &response["result"]["structuredContent"]
}

fn tool_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn lifecycle_lists_and_calls_every_read_only_tool() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let before = snapshot(&vault);
    let mut server = fixture.start();

    let initialize = server.request("initialize", json!({"protocolVersion": "2024-11-05"}));
    assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(initialize["result"]["capabilities"]["tools"], json!({}));
    assert_eq!(initialize["result"]["capabilities"]["resources"], json!({}));
    assert_eq!(initialize["result"]["serverInfo"]["name"], "hortus");
    assert_eq!(
        initialize["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );

    server.notify("notifications/initialized", json!({}));
    let ping = server.request("ping", Value::Null);
    assert_eq!(ping["result"], json!({}));

    let latest = server.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(latest["result"]["protocolVersion"], "2025-06-18");
    let unknown = server.request("initialize", json!({"protocolVersion": "2099-01-01"}));
    assert_eq!(unknown["result"]["protocolVersion"], "2025-03-26");

    let listed = server.request("tools/list", Value::Null);
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "hortus_search",
            "hortus_backlinks",
            "hortus_links",
            "hortus_tags",
            "hortus_outline",
            "hortus_orphans",
            "hortus_broken_links",
            "hortus_mermaid_check",
            "hortus_guide",
        ]
    );
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
        assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");
        assert_eq!(tool["annotations"]["idempotentHint"], true, "{tool}");
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{tool}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }

    let search = server.call("hortus_search", json!({"query": "pipeline"}));
    let search = structured(&search);
    assert_eq!(search["total_notes"], 7);
    assert_eq!(search["results"][0]["path"], "index.md");
    assert_eq!(search["results"][0]["title"], "Index");

    let backlinks = server.call("hortus_backlinks", json!({"note": "guides/intro.md"}));
    let backlinks = structured(&backlinks);
    assert_eq!(backlinks["note"], "guides/intro.md");
    assert_eq!(backlinks["count"], 2);

    let links = server.call("hortus_links", json!({"note": "index"}));
    let links = structured(&links);
    assert_eq!(links["note"], "index.md");
    assert_eq!(links["count"], 4);
    assert_eq!(links["links"][1]["resolution"]["kind"], "broken");

    let tags = server.call("hortus_tags", json!({}));
    let tags = structured(&tags);
    assert_eq!(tags["count"], 3);
    let filtered = server.call("hortus_tags", json!({"tag": "topic"}));
    assert_eq!(structured(&filtered)["count"], 2);

    let outline = server.call("hortus_outline", json!({"note": "guides/intro.md"}));
    let outline = structured(&outline);
    assert_eq!(outline["headings"].as_array().unwrap().len(), 2);
    assert_eq!(outline["headings"][1]["text"], "Setup");

    let orphans = server.call("hortus_orphans", json!({}));
    assert_eq!(structured(&orphans)["count"], 5);

    let broken = server.call("hortus_broken_links", json!({}));
    assert_eq!(structured(&broken)["count"], 1);

    let diagrams = server.call("hortus_mermaid_check", json!({"note": "diagrams.md"}));
    let diagrams = structured(&diagrams);
    assert_eq!(diagrams["valid"], true);
    let kinds = diagrams["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["diagram_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        ["tree", "flowchart", "sequence", "state", "class", "er"]
    );
    let single = server.call(
        "hortus_mermaid_check",
        json!({"note": "diagrams.md", "block": 4}),
    );
    let single = structured(&single);
    assert_eq!(single["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(single["blocks"][0]["diagram_type"], "state");
    let invalid = server.call("hortus_mermaid_check", json!({"note": "invalid.md"}));
    assert_eq!(invalid["result"]["isError"], false);
    let invalid = structured(&invalid);
    assert_eq!(invalid["valid"], false);
    assert_eq!(invalid["blocks"][0]["error_kind"], "syntax");

    let guide = server.call("hortus_guide", json!({}));
    assert_eq!(tool_text(&guide), hortus_config::AGENT_GUIDE);
    assert_eq!(structured(&guide)["guide"], hortus_config::AGENT_GUIDE);

    let spaced = server.call("hortus_outline", json!({"note": "其他/带 空格.md"}));
    assert_eq!(structured(&spaced)["note"], "其他/带 空格.md");

    let resources = server.request("resources/list", Value::Null);
    let resources = resources["result"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], "hortus://guide");
    assert_eq!(resources[0]["mimeType"], "text/markdown");
    let read = server.request("resources/read", json!({"uri": "hortus://guide"}));
    assert_eq!(read["result"]["contents"][0]["uri"], "hortus://guide");
    assert_eq!(
        read["result"]["contents"][0]["text"],
        hortus_config::AGENT_GUIDE
    );

    let (status, stderr) = server.finish();
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(before, snapshot(&vault), "MCP server wrote to the vault");
    assert!(
        !fixture.config.exists(),
        "MCP server created the config directory"
    );
}

#[test]
fn vault_falls_back_to_the_environment_and_current_directory() {
    let fixture = Fixture::new();
    let vault = fs::canonicalize(fixture.vault()).unwrap();
    let mut command = fixture.command();
    command.args(["mcp"]).env("HORTUS_VAULT", &vault);
    let mut server = Server::spawn(command);
    let tags = server.call("hortus_tags", json!({}));
    assert_eq!(structured(&tags)["vault"], vault.display().to_string());
    let (status, stderr) = server.finish();
    assert!(status.success(), "stderr: {stderr}");

    let mut command = fixture.command();
    command
        .args(["mcp"])
        .current_dir(&vault)
        .env_remove("HORTUS_VAULT");
    let mut server = Server::spawn(command);
    let tags = server.call("hortus_tags", json!({}));
    assert_eq!(structured(&tags)["vault"], vault.display().to_string());
    let (status, stderr) = server.finish();
    assert!(status.success(), "stderr: {stderr}");
}

#[test]
fn user_guide_override_is_shared_by_tool_and_resource() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.config).unwrap();
    let guide = "# Custom guide\n\nUse [[links]] carefully.\n";
    fs::write(fixture.config.join("agent-guide.md"), guide).unwrap();
    let mut server = fixture.start();
    server.notify("notifications/initialized", json!({}));
    let tool = server.call("hortus_guide", json!({}));
    assert_eq!(tool_text(&tool), guide);
    let read = server.request("resources/read", json!({"uri": "hortus://guide"}));
    assert_eq!(read["result"]["contents"][0]["text"], guide);
}

#[test]
fn invalid_input_fails_with_json_rpc_codes_and_eof_exits_cleanly() {
    let fixture = Fixture::new();
    let mut server = fixture.start();

    server.write_raw("{oops");
    let parse = server.read();
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(parse["id"], Value::Null);

    server.write_raw("[1, 2]");
    let invalid = server.read();
    assert_eq!(invalid["error"]["code"], -32600);

    let unknown = server.request("unknown/method", Value::Null);
    assert_eq!(unknown["error"]["code"], -32601);

    let missing_name = server.request("tools/call", json!({"arguments": {}}));
    assert_eq!(missing_name["error"]["code"], -32602);

    let unknown_tool = server.call("hortus_write", json!({}));
    assert_eq!(unknown_tool["error"]["code"], -32602);

    let missing_note = server.call("hortus_outline", json!({}));
    assert_eq!(missing_note["error"]["code"], -32602);

    let bad_limit = server.call("hortus_search", json!({"query": "x", "limit": "many"}));
    assert_eq!(bad_limit["error"]["code"], -32602);

    let bad_block = server.call(
        "hortus_mermaid_check",
        json!({"note": "diagrams.md", "block": 0}),
    );
    assert_eq!(bad_block["error"]["code"], -32602);

    let traversal = server.call("hortus_outline", json!({"note": "../outside.md"}));
    assert_eq!(traversal["error"]["code"], -32602);
    let absolute = server.call(
        "hortus_outline",
        json!({"note": fixture.dir.path().join("outside.md").to_str().unwrap()}),
    );
    assert_eq!(absolute["error"]["code"], -32602);
    #[cfg(unix)]
    {
        let symlink = server.call("hortus_outline", json!({"note": "link.md"}));
        assert_eq!(symlink["error"]["code"], -32602);
    }

    let missing = server.call("hortus_outline", json!({"note": "absent.md"}));
    assert_eq!(missing["result"]["isError"], true);

    let resource = server.request("resources/read", json!({"uri": "hortus://missing"}));
    assert_eq!(resource["error"]["code"], -32002);
    let missing_uri = server.request("resources/read", json!({}));
    assert_eq!(missing_uri["error"]["code"], -32602);

    let (status, stderr) = server.finish();
    assert!(status.success(), "stderr: {stderr}");
    assert!(
        stderr.is_empty(),
        "diagnostics must stay quiet on success: {stderr}"
    );
}

fn snapshot(root: &Path) -> BTreeMap<String, (u64, u128)> {
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, (u64, u128)>) {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().display().to_string();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.file_type().is_symlink() {
            files.insert(relative, (u64::MAX, 0));
        } else if metadata.is_dir() {
            files.insert(relative, (u64::MAX - 1, 0));
            visit(root, &path, files);
        } else {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            files.insert(relative, (metadata.len(), modified));
        }
    }
}

#[test]
fn standalone_mermaid_checks_report_one_real_block() {
    let fixture = Fixture::new();
    write(
        &fixture.vault().join("valid.mmd"),
        "stateDiagram-v2\n[*] --> A\n",
    );
    write(
        &fixture.vault().join("invalid.mmd"),
        "stateDiagram-v2\nstate A {\n",
    );
    let mut server = fixture.start();
    for (note, valid) in [("valid.mmd", true), ("invalid.mmd", false)] {
        for block in [json!({ "note": note }), json!({ "note": note, "block": 1 })] {
            let response = server.call("hortus_mermaid_check", block);
            let report = structured(&response);
            assert_eq!(report["valid"], valid);
            let blocks = report["blocks"].as_array().unwrap();
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0]["block"], 1);
            assert_eq!(blocks[0]["diagram_type"], "state");
            assert_eq!(blocks[0]["valid"], valid);
            if !valid {
                assert_eq!(blocks[0]["error_kind"], "syntax");
                assert!(!blocks[0]["message"].as_str().unwrap().is_empty());
            }
        }
    }
    let missing = server.call(
        "hortus_mermaid_check",
        json!({"note": "valid.mmd", "block": 2}),
    );
    assert_eq!(missing["result"]["isError"], true);
    assert!(tool_text(&missing).contains("MissingBlock: requested=2"));
    assert!(server.finish().0.success());
}

#[test]
fn notification_methods_only_respond_when_the_request_has_an_id() {
    let fixture = Fixture::new();
    let mut server = fixture.start();
    server.notify("notifications/initialized", Value::Null);
    server.write(&json!({"jsonrpc": "2.0", "id": 42, "method": "notifications/initialized"}));
    // Send a ping before reading so a dropped request fails immediately.
    server.write(&json!({"jsonrpc": "2.0", "id": 43, "method": "ping"}));
    let response = server.read();
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 42);
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(server.read()["id"], 43);
    assert!(server.finish().0.success());
}

#[test]
fn oversized_line_is_drained_and_the_next_request_is_served() {
    let fixture = Fixture::new();
    let mut server = fixture.start();
    // Leave the first line exactly at the limit and keep the last line short.
    let ping = r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#;
    let limit = 8 * 1024 * 1024;
    server.write_raw(&format!("{}{}", " ".repeat(limit - ping.len()), ping));
    assert_eq!(server.read()["id"], 42);
    server.write_raw(&" ".repeat(limit + 4096));
    server.write(&json!({"jsonrpc": "2.0", "id": 43, "method": "ping"}));
    let error = server.read();
    assert_eq!(error["id"], Value::Null);
    assert_eq!(error["error"]["code"], -32600);
    assert_eq!(server.read()["id"], 43);
    assert!(server.finish().0.success());
}

#[test]
fn external_add_edit_delete_are_immediately_visible_and_guide_skips_vault() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let mut server = fixture.start();

    // Guide works before any vault query and even if vault is later emptied.
    let guide_before = server.call("hortus_guide", json!({}));
    assert_eq!(tool_text(&guide_before), hortus_config::AGENT_GUIDE);

    // Initial search warms the cache.
    let initial = server.call("hortus_search", json!({"query": "FreshExternalToken"}));
    assert_eq!(structured(&initial)["results"].as_array().unwrap().len(), 0);

    // External add
    let dynamic_note = vault.join("dynamic.md");
    write(&dynamic_note, "# Dynamic\n\nFreshExternalToken #fresh\n");
    let after_add = server.call("hortus_search", json!({"query": "FreshExternalToken"}));
    assert_eq!(structured(&after_add)["results"][0]["path"], "dynamic.md");
    let tags_after_add = server.call("hortus_tags", json!({"tag": "fresh"}));
    assert_eq!(structured(&tags_after_add)["count"], 1);

    // External edit
    write(&dynamic_note, "# Dynamic\n\nEditedExternalToken #fresh\n");
    let after_edit_old = server.call("hortus_search", json!({"query": "FreshExternalToken"}));
    assert_eq!(
        structured(&after_edit_old)["results"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let after_edit_new = server.call("hortus_search", json!({"query": "EditedExternalToken"}));
    assert_eq!(
        structured(&after_edit_new)["results"][0]["path"],
        "dynamic.md"
    );

    // External delete
    fs::remove_file(&dynamic_note).unwrap();
    let after_delete = server.call("hortus_search", json!({"query": "EditedExternalToken"}));
    assert_eq!(
        structured(&after_delete)["results"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Snapshot after external cleanup must remain untouched by further MCP calls.
    let before = snapshot(&vault);
    let _ = server.call("hortus_orphans", json!({}));
    let _ = server.call("hortus_guide", json!({}));
    let (status, stderr) = server.finish();
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(before, snapshot(&vault), "MCP server wrote to the vault");
}
