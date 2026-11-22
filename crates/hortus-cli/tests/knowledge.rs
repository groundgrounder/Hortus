use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::UNIX_EPOCH;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hortus")
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        fs::create_dir_all(vault.join("guides")).unwrap();
        fs::create_dir_all(vault.join(".hidden")).unwrap();
        fs::create_dir_all(vault.join("node_modules/pkg")).unwrap();
        write(
            &vault.join("index.md"),
            concat!(
                "---\n",
                "tags: [topic, topic/sub]\n",
                "aliases: [Home]\n",
                "source: upstream/spec.md\n",
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
        write(&vault.join(".hidden/secret.md"), "[[index]]\n");
        write(
            &vault.join("node_modules/pkg/note.md"),
            "# Packaged\n\n[[index]]\n",
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("guides/intro.md", vault.join("link.md")).unwrap();
            std::os::unix::fs::symlink("guides", vault.join("linked-dir")).unwrap();
        }
        let other = dir.path().join("other");
        fs::create_dir_all(&other).unwrap();
        write(&other.join("other.md"), "# Other\n\n#other\n");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn vault(&self) -> PathBuf {
        self.dir.path().join("vault")
    }

    fn other(&self) -> PathBuf {
        self.dir.path().join("other")
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn run(cwd: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(binary());
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path).unwrap().display().to_string()
}

#[test]
fn all_queries_return_expected_stable_json() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let vault_arg = vault.to_str().unwrap();

    let search = json(&run(
        fixture.path(),
        &["search", "pipeline", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(search["vault"], canonical(&vault));
    assert_eq!(search["query"], "pipeline");
    assert_eq!(search["total_notes"], 4);
    assert_eq!(search["results"].as_array().unwrap().len(), 1);
    assert_eq!(search["results"][0]["path"], "index.md");
    assert!(
        !search["results"][0]["matches"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let backlinks = json(&run(
        fixture.path(),
        &["backlinks", "guides/intro.md", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(backlinks["note"], "guides/intro.md");
    assert_eq!(backlinks["count"], 2);
    let entries = backlinks["backlinks"].as_array().unwrap();
    assert_eq!(entries[0]["source"], "index.md");
    assert_eq!(entries[0]["in_diagram"], false);
    assert_eq!(entries[1]["in_diagram"], true);

    let links = json(&run(
        fixture.path(),
        &["links", "index", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(links["note"], "index.md");
    assert_eq!(links["count"], 4);
    let entries = links["links"].as_array().unwrap();
    assert_eq!(entries[0]["target"], "guides/intro");
    assert_eq!(entries[0]["resolution"]["kind"], "note");
    assert_eq!(entries[0]["resolution"]["path"], "guides/intro.md");
    assert_eq!(entries[1]["resolution"]["kind"], "broken");
    assert_eq!(entries[2]["resolution"]["kind"], "attachment");
    assert_eq!(entries[2]["attachment"], true);
    assert_eq!(entries[3]["resolution"]["kind"], "note");
    assert_eq!(entries[3]["in_diagram"], true);

    let tags = json(&run(fixture.path(), &["tags", "--vault", vault_arg], &[]));
    assert_eq!(tags["count"], 3);
    let entries = tags["tags"].as_array().unwrap();
    assert_eq!(entries[0]["tag"], "topic");
    assert_eq!(entries[0]["count"], 2);
    assert_eq!(entries[1]["tag"], "topic/other");
    assert_eq!(entries[2]["tag"], "topic/sub");
    let again = run(fixture.path(), &["tags", "--vault", vault_arg], &[]);
    assert_eq!(tags, json(&again), "tag output must be byte-stable");

    let prefixed = json(&run(
        fixture.path(),
        &["tags", "topic", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(prefixed["tag"], "topic");
    assert_eq!(prefixed["count"], 2);
    assert_eq!(prefixed["notes"][0], "index.md");
    assert_eq!(prefixed["notes"][1], "second.md");

    let outline = json(&run(
        fixture.path(),
        &["outline", "guides/intro.md", "--vault", vault_arg],
        &[],
    ));
    let headings = outline["headings"].as_array().unwrap();
    assert_eq!(headings.len(), 2);
    assert_eq!(headings[0]["level"], 1);
    assert_eq!(headings[0]["text"], "Intro");
    assert_eq!(headings[0]["start"], 0);
    assert_eq!(headings[0]["length"], 7);
    assert_eq!(headings[1]["level"], 2);
    assert_eq!(headings[1]["text"], "Setup");
    assert_eq!(headings[1]["start"], 8);
    assert_eq!(headings[1]["length"], 8);

    let orphans = json(&run(
        fixture.path(),
        &["orphans", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(orphans["count"], 2);
    assert_eq!(orphans["orphans"][0], "lonely.md");
    assert_eq!(orphans["orphans"][1], "second.md");

    let broken = json(&run(
        fixture.path(),
        &["broken-links", "--vault", vault_arg],
        &[],
    ));
    assert_eq!(broken["count"], 1);
    assert_eq!(broken["broken_links"][0]["source"], "index.md");
    assert_eq!(broken["broken_links"][0]["target"], "missing");
    assert_eq!(broken["broken_links"][0]["reason"], "broken");
    assert_eq!(broken["broken_links"][0]["line"], 8);
}

#[test]
fn vault_selection_follows_flag_then_environment_then_cwd() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let other = fixture.other();

    let cwd = json(&run(&vault, &["tags"], &[]));
    assert_eq!(cwd["vault"], canonical(&vault));

    let environment = json(&run(
        fixture.path(),
        &["tags"],
        &[("HORTUS_VAULT", other.as_path())],
    ));
    assert_eq!(environment["vault"], canonical(&other));
    assert_eq!(environment["count"], 1);
    assert_eq!(environment["tags"][0]["tag"], "other");

    let flag = json(&run(
        &other,
        &["tags", "--vault", vault.to_str().unwrap()],
        &[("HORTUS_VAULT", other.as_path())],
    ));
    assert_eq!(flag["vault"], canonical(&vault));
    assert_eq!(flag["count"], 3);
}

#[test]
fn read_only_queries_leave_the_vault_untouched() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let vault_arg = vault.to_str().unwrap();
    let before = snapshot(&vault);
    let commands = [
        vec!["search", "pipeline", "--vault", vault_arg],
        vec!["backlinks", "guides/intro.md", "--vault", vault_arg],
        vec!["links", "index.md", "--vault", vault_arg],
        vec!["tags", "--vault", vault_arg],
        vec!["tags", "topic", "--vault", vault_arg],
        vec!["outline", "guides/intro.md", "--vault", vault_arg],
        vec!["orphans", "--vault", vault_arg],
        vec!["broken-links", "--vault", vault_arg],
        vec!["mermaid", "check", "index.md"],
    ];
    for arguments in commands {
        let output = run(&vault, &arguments, &[]);
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(before, snapshot(&vault));
}

#[test]
fn mermaid_check_reports_types_errors_and_block_selection() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    write(
        &vault.join("diagrams.md"),
        concat!(
            "```mermaid\nmindmap\n  Root\n    Child\n```\n\n",
            "```mermaid\nflowchart TB\nsubgraph G\nA --> B\nend\n```\n\n",
            "```mermaid\nsequenceDiagram\nA->>B: hello\n```\n",
        ),
    );
    let output = run(&vault, &["mermaid", "check", "diagrams.md"], &[]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["file"], "diagrams.md");
    assert_eq!(report["valid"], true);
    let blocks = report["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["diagram_type"], "tree");
    assert_eq!(blocks[1]["diagram_type"], "flowchart");
    assert_eq!(blocks[2]["diagram_type"], "sequence");
    assert!(blocks.iter().all(|block| block["valid"] == true));
    assert!(
        blocks
            .iter()
            .all(|block| block["line"].as_u64().unwrap() > 0)
    );

    let selected = run(
        &vault,
        &["mermaid", "check", "diagrams.md", "--block", "2"],
        &[],
    );
    assert!(selected.status.success());
    let report: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(report["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(report["blocks"][0]["block"], 2);

    let missing = run(
        &vault,
        &["mermaid", "check", "diagrams.md", "--block", "9"],
        &[],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("MissingBlock"));

    write(
        &vault.join("invalid.md"),
        "```mermaid\nflowchart TB\nA[unclosed\n```\n",
    );
    let invalid = run(&vault, &["mermaid", "check", "invalid.md"], &[]);
    assert!(!invalid.status.success());
    let report: Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert_eq!(report["blocks"][0]["error_kind"], "syntax");

    write(
        &vault.join("unsupported.md"),
        "```mermaid\nclassDiagram\nA <|-- B\n```\n",
    );
    let unsupported = run(&vault, &["mermaid", "check", "unsupported.md"], &[]);
    assert!(!unsupported.status.success());
    let report: Value = serde_json::from_slice(&unsupported.stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert_eq!(report["blocks"][0]["error_kind"], "unsupported");

    write(
        &vault.join("unclosed.md"),
        "```mermaid\nflowchart TB\nA --> B\n",
    );
    let unclosed = run(&vault, &["mermaid", "check", "unclosed.md"], &[]);
    assert!(!unclosed.status.success());
    let report: Value = serde_json::from_slice(&unclosed.stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert!(
        report["blocks"][0]["message"]
            .as_str()
            .unwrap()
            .contains("fence_unclosed")
    );

    write(&vault.join("plain.md"), "# Plain note\n\nNo diagrams.\n");
    let plain = run(&vault, &["mermaid", "check", "plain.md"], &[]);
    assert!(plain.status.success());
    let report: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(report["valid"], true);
    assert_eq!(report["blocks"].as_array().unwrap().len(), 0);
}

#[test]
fn unsafe_paths_and_symlinks_are_rejected() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let vault_arg = vault.to_str().unwrap();
    for arguments in [
        vec!["backlinks", "../outside.md", "--vault", vault_arg],
        vec!["backlinks", "/etc/passwd", "--vault", vault_arg],
        vec!["links", "link.md", "--vault", vault_arg],
        vec!["links", "linked-dir/intro.md", "--vault", vault_arg],
        vec!["outline", "missing.md", "--vault", vault_arg],
        vec!["tags", "--vault", "/does/not/exist"],
        vec!["mermaid", "check", "missing.md"],
    ] {
        let output = run(fixture.path(), &arguments, &[]);
        assert!(!output.status.success(), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.is_ascii(), "errors must stay English: {stderr}");
    }
}

#[test]
fn help_lists_every_query_and_stays_english() {
    let fixture = Fixture::new();
    let help = run(fixture.path(), &["--help"], &[]);
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.starts_with("Usage: hortus"));
    for command in [
        "hortus guide",
        "hortus mermaid render",
        "hortus mermaid check",
        "hortus search",
        "hortus backlinks",
        "hortus links",
        "hortus tags",
        "hortus outline",
        "hortus orphans",
        "hortus broken-links",
    ] {
        assert!(stdout.contains(command), "{command} missing from help");
    }
    assert!(stdout.is_ascii(), "help must stay English: {stdout}");
}

fn snapshot(root: &Path) -> BTreeMap<String, (u64, u128)> {
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, (u64, u128)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            let hash = fnv(target.to_string_lossy().as_bytes());
            files.insert(relative, (0, u128::from(hash)));
            continue;
        }
        if metadata.is_dir() {
            visit(root, &path, files);
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        files.insert(relative, (fnv(&bytes), modified ^ bytes.len() as u128));
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn absolute_note_paths_inside_the_vault_resolve() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    let note = fs::canonicalize(vault.join("guides/intro.md")).unwrap();
    let output = run(
        fixture.path(),
        &[
            "links",
            note.to_str().unwrap(),
            "--vault",
            vault.to_str().unwrap(),
        ],
        &[],
    );
    let value = json(&output);
    assert_eq!(value["note"], "guides/intro.md");
}

#[test]
fn mermaid_check_classifies_valid_and_invalid_state_fences() {
    let fixture = Fixture::new();
    let vault = fixture.vault();
    write(
        &vault.join("state.md"),
        "序😀\r\n\r\n> ~~~mermaid\r\n> %%{init: {} }%%\r\n> stateDiagram-v2\r\n> [*] --> 队列\r\n> ~~~\r\n\r\n~~~mermaid\n%% comment\nstateDiagram\n状态 --> --> B\n~~~\n",
    );
    let result = run(&vault, &["mermaid", "check", "state.md"], &[]);
    assert!(!result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["blocks"][0]["diagram_type"], "state");
    assert_eq!(report["blocks"][0]["valid"], true);
    assert_eq!(report["blocks"][1]["diagram_type"], "state");
    assert_eq!(report["blocks"][1]["valid"], false);
    assert_eq!(report["blocks"][1]["error_kind"], "syntax");
    assert_eq!(report["blocks"][1]["line"], 12);
}
