#!/usr/bin/env python3
"""Regression checks for hortus-links and the read-only hortus CLI.

Builds the CLI, generates a 500-note technical vault, checks the JSON contract
of every query, measures four end-to-end commands, and verifies that the vault
is byte- and mtime-identical afterwards.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
TOTAL_NOTES = 505
TIMING_LIMIT_MS = 1000.0


def check(condition, message):
    if not condition:
        raise SystemExit(f"CHECK FAILED: {message}")


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def generate(vault):
    vault.mkdir()
    write(
        vault / "guides/setup.md",
        "---\n"
        "tags: [topic, topic/setup]\n"
        "aliases: [Install]\n"
        "source: upstream/setup.md\n"
        "---\n"
        "# Setup\n\n"
        "needleword pipeline notes for [[index]] and [[missing-note]] and ![[diagram.png]].\n\n"
        "```mermaid\n"
        "flowchart TB\n"
        'A["[[index]]"] --> B\n'
        "A --> C\n"
        "B --> D\n"
        "C --> D\n"
        "```\n\n"
        "#topic/setup\n\n"
        "## Details\n\n"
        "Text with a `[[inline code]]` sample and $[[inline math]]$ sample.\n",
    )
    write(
        vault / "index.md",
        "---\ntags: [hub]\n---\n# Index\n\n"
        "[[guides/setup]] [[topic-000]] [[Shared]]\n",
    )
    for number in range(500):
        next_number = (number + 1) % 500
        text = (
            f"# Topic {number:03}\n\n"
            f"Engine pipelines and caches for topic {number:03}.\n"
            f"#topic #topic/sub-{number % 10}\n\n"
            f"Next [[topic-{next_number:03}]] and setup [[guides/setup]].\n"
        )
        if number % 50 == 0:
            text += f"Broken [[broken-{number:03}]].\n"
        if number % 100 == 0:
            text += (
                "```mermaid\nmindmap\n"
                f"  Topic {number:03}\n"
                "    Child\n"
                "```\n"
            )
        write(vault / "topics" / f"topic-{number:03}.md", text)
    write(
        vault / "a/duplicate.md",
        "---\naliases: [Shared]\n---\n# Duplicate A\n",
    )
    write(
        vault / "b/duplicate.md",
        "---\naliases: [Shared]\n---\n# Duplicate B\n",
    )
    write(vault / "isolated.md", "# Isolated\n\nNothing links here.\n")
    write(vault / ".hidden/secret.md", "[[index]] [[isolated]]\n")
    write(vault / "node_modules/pkg/note.md", "# Packaged\n\n[[index]]\n")
    os.symlink("guides/setup.md", vault / "link.md")
    os.symlink("guides", vault / "linked-dir")


def snapshot(root):
    records = {}
    for directory, dirnames, filenames in os.walk(root, followlinks=False):
        directory = Path(directory)
        for name in list(dirnames):
            path = directory / name
            if path.is_symlink():
                records[str(path.relative_to(root))] = ["link", os.readlink(path)]
                dirnames.remove(name)
        for name in filenames:
            path = directory / name
            relative = str(path.relative_to(root))
            if path.is_symlink():
                records[relative] = ["link", os.readlink(path)]
            else:
                stat = path.stat()
                records[relative] = [
                    hashlib.sha256(path.read_bytes()).hexdigest(),
                    stat.st_size,
                    stat.st_mtime_ns,
                ]
    return records


def invoke(binary, vault, args, cwd=None, env=None):
    return subprocess.run(
        [str(binary), *args],
        cwd=str(cwd or vault),
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )


def parse_json(result, label):
    check(result.returncode == 0, f"{label} exited {result.returncode}: {result.stderr}")
    check(result.stderr == "", f"{label} wrote to stderr: {result.stderr}")
    check(result.stdout.endswith("\n"), f"{label} did not end its JSON with a newline")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"CHECK FAILED: {label} is not valid JSON: {error}") from error


def sorted_paths(value):
    paths = [entry["path"] for entry in value["results"]]
    check(paths == sorted(paths), "search results are not sorted")
    return paths


def sorted_sources(value):
    sources = [(entry["source"], entry["line"]) for entry in value["backlinks"]]
    check(sources == sorted(sources), "backlinks are not sorted")
    return sources


def measure(binary, vault, args, repeats=3):
    best = None
    for _ in range(repeats):
        start = time.perf_counter()
        result = subprocess.run(
            [str(binary), *args], cwd=str(vault), capture_output=True, text=True, timeout=60
        )
        elapsed = (time.perf_counter() - start) * 1000.0
        parse_json(result, f"timing {args[0]}")
        best = elapsed if best is None else min(best, elapsed)
    return best


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "target/reports/knowledge-cli",
        help="directory for the JSON summary and per-command output",
    )
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "json").mkdir(exist_ok=True)

    if not args.skip_build:
        subprocess.run(["cargo", "build", "-p", "hortus-cli"], cwd=ROOT, check=True)
    binary = ROOT / "target/debug/hortus"
    check(binary.is_file(), f"missing CLI binary: {binary}")

    with tempfile.TemporaryDirectory(prefix="hortus-knowledge-") as temporary:
        vault = Path(temporary) / "vault"
        generate(vault)
        write(Path(temporary) / "invalid.md", "```mermaid\nflowchart TB\nA[unclosed\n```\n")
        before = snapshot(vault)

        search = parse_json(
            invoke(binary, vault, ["search", "needleword"]), "search"
        )
        check(search["total_notes"] == TOTAL_NOTES, "search total_notes is wrong")
        check(len(search["results"]) == 1, "search needleword should match one note")
        check(search["results"][0]["path"] == "guides/setup.md", "search path is wrong")
        (output / "json/search.json").write_text(
            json.dumps(search, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        limited = parse_json(
            invoke(binary, vault, ["search", "pipelines", "--limit", "5"]), "search limit"
        )
        check(len(limited["results"]) == 5, "--limit did not cap the result list")
        sorted_paths(limited)

        backlinks = parse_json(
            invoke(binary, vault, ["backlinks", "guides/setup.md"]), "backlinks"
        )
        check(backlinks["note"] == "guides/setup.md", "backlinks note is wrong")
        check(backlinks["count"] == 501, f"backlinks count is {backlinks['count']}")
        sources = sorted_sources(backlinks)
        check(sources[0][0] == "index.md", "backlinks should start with index.md")
        check(
            sources[-1][0] == "topics/topic-499.md",
            "backlinks should end with topic-499",
        )
        (output / "json/backlinks.json").write_text(
            json.dumps(backlinks, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        links = parse_json(invoke(binary, vault, ["links", "index.md"]), "links")
        check(links["note"] == "index.md", "links note is wrong")
        check(links["count"] == 3, "index should have three links")
        check(
            links["links"][0]["resolution"] == {"kind": "note", "path": "guides/setup.md"},
            "first link should resolve to guides/setup.md",
        )
        check(links["links"][1]["resolution"]["kind"] == "note", "second link should resolve")
        check(
            links["links"][2]["resolution"] == {
                "kind": "ambiguous",
                "candidates": ["a/duplicate.md", "b/duplicate.md"],
            },
            "alias link should be ambiguous",
        )
        (output / "json/links.json").write_text(
            json.dumps(links, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        tags = parse_json(invoke(binary, vault, ["tags"]), "tags")
        keys = [entry["tag"] for entry in tags["tags"]]
        check(keys == sorted(keys), "tags are not sorted")
        check(tags["count"] == len(keys), "tag count does not match the list")
        topic = next(entry for entry in tags["tags"] if entry["tag"] == "topic")
        check(topic["count"] == 501, f"topic tag count is {topic['count']}")
        (output / "json/tags.json").write_text(
            json.dumps(tags, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        prefixed = parse_json(
            invoke(binary, vault, ["tags", "topic/sub-0"]), "tags prefix"
        )
        check(prefixed["count"] == 50, "hierarchical tag prefix did not match 50 notes")
        check(prefixed["notes"] == sorted(prefixed["notes"]), "tag notes are not sorted")
        (output / "json/tags-topic.json").write_text(
            json.dumps(prefixed, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        outline = parse_json(
            invoke(binary, vault, ["outline", "guides/setup.md"]), "outline"
        )
        check(
            [heading["text"] for heading in outline["headings"]] == ["Setup", "Details"],
            "outline headings are wrong",
        )
        check(
            all(
                heading["start"] >= 0 and heading["length"] > 0
                for heading in outline["headings"]
            ),
            "outline spans are missing",
        )
        (output / "json/outline.json").write_text(
            json.dumps(outline, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        orphans = parse_json(invoke(binary, vault, ["orphans"]), "orphans")
        check(
            orphans["orphans"] == ["a/duplicate.md", "b/duplicate.md", "isolated.md"],
            f"unexpected orphans: {orphans['orphans']}",
        )
        (output / "json/orphans.json").write_text(
            json.dumps(orphans, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        broken = parse_json(invoke(binary, vault, ["broken-links"]), "broken links")
        check(broken["count"] == 12, f"broken link count is {broken['count']}")
        ambiguous = [entry for entry in broken["broken_links"] if entry["reason"] == "ambiguous"]
        check(len(ambiguous) == 1, "expected exactly one ambiguous link")
        check(
            ambiguous[0]["candidates"] == ["a/duplicate.md", "b/duplicate.md"],
            "ambiguous candidates are wrong",
        )
        (output / "json/broken-links.json").write_text(
            json.dumps(broken, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        mermaid = invoke(binary, vault, ["mermaid", "check", "guides/setup.md"])
        report = parse_json(mermaid, "mermaid check")
        check(report["valid"] is True, "setup note should contain a valid diagram")
        check(report["blocks"][0]["diagram_type"] == "flowchart", "diagram type is wrong")
        (output / "json/mermaid-check.json").write_text(
            json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        invalid = invoke(
            binary, vault, ["mermaid", "check", "invalid.md"], cwd=Path(temporary)
        )
        check(invalid.returncode != 0, "invalid Mermaid should exit non-zero")
        invalid_report = json.loads(invalid.stdout)
        check(invalid_report["valid"] is False, "invalid Mermaid should report valid: false")
        check(invalid_report["blocks"][0]["error_kind"] == "syntax", "error kind is wrong")

        environment = dict(os.environ)
        environment["HORTUS_VAULT"] = str(vault)
        from_environment = parse_json(
            invoke(binary, vault, ["tags"], cwd=ROOT, env=environment), "environment vault"
        )
        check(
            from_environment["vault"] == str(vault.resolve()),
            "HORTUS_VAULT was not used",
        )

        for arguments in [
            ["backlinks", "../outside.md"],
            ["links", "link.md"],
            ["links", "linked-dir/setup.md"],
            ["backlinks", "/etc/passwd"],
        ]:
            result = invoke(binary, vault, arguments)
            check(result.returncode != 0, f"{arguments} should fail")
            check(result.stderr.isascii(), f"{arguments} should report an English error")

        after = snapshot(vault)
        check(before == after, "the vault changed during read-only queries")

        timings = {
            "search": measure(binary, vault, ["search", "pipelines", "--limit", "20"]),
            "backlinks": measure(binary, vault, ["backlinks", "guides/setup.md"]),
            "tags": measure(binary, vault, ["tags"]),
            "broken-links": measure(binary, vault, ["broken-links"]),
        }
        for name, elapsed in timings.items():
            check(
                elapsed < TIMING_LIMIT_MS,
                f"{name} took {elapsed:.1f} ms (limit {TIMING_LIMIT_MS:.0f} ms)",
            )
        (output / "timings.json").write_text(
            json.dumps(timings, indent=2) + "\n", encoding="utf-8"
        )

        summary = {
            "vault_notes": TOTAL_NOTES,
            "timings_ms": {name: round(value, 2) for name, value in timings.items()},
            "timing_limit_ms": TIMING_LIMIT_MS,
            "read_only": before == after,
            "checks": {
                "search": True,
                "backlinks": True,
                "links": True,
                "tags": True,
                "outline": True,
                "orphans": True,
                "broken_links": True,
                "mermaid_check": True,
                "environment_vault": True,
                "unsafe_paths": True,
                "vault_untouched": True,
            },
        }
        (output / "summary.json").write_text(
            json.dumps(summary, indent=2) + "\n", encoding="utf-8"
        )

    print(
        "knowledge CLI checks passed: "
        + ", ".join(f"{name} {value:.1f} ms" for name, value in timings.items())
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
