#!/usr/bin/env python3
"""Capture the P1-11 bilingual UI states using isolated app state."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP_BUNDLE = ROOT / "build/Hortus.app"


def launch(language, state, arguments, snapshot):
    state.mkdir(parents=True, exist_ok=True)
    config = state / "config"
    cache = state / "cache"
    config.mkdir(exist_ok=True)
    cache.mkdir(exist_ok=True)
    (config / "config.toml").write_text(
        'font_size = 15.0\ndefault_view = "' + ("editing" if "editing" in snapshot.name else "reading") + '"\n',
        encoding="utf-8",
    )
    env = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache))
    command = ["-AppleLanguages", f"({language})", "--light", *arguments]
    if snapshot.name.endswith("about.png"):
        command.append(f"--about-snapshot={snapshot}")
    else:
        command.append(f"--snapshot={snapshot}")
    snapshot.unlink(missing_ok=True)
    subprocess.run(["open", "-n", "-a", str(APP_BUNDLE), "--args", *command],
                   env=env, check=True, timeout=30)
    deadline = time.monotonic() + 45
    while not snapshot.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert snapshot.is_file() and snapshot.stat().st_size > 10_000, snapshot


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/bilingual-screenshots")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="hortus-bilingual-ui-") as temporary:
        base = Path(temporary)
        vault = base / "vault"
        vault.mkdir()
        (vault / "reading.md").write_text(
            "---\ntitle: Reading sample\ntags: [notes, sample]\n---\n\n"
            "# A clear reading view\n\n"
            "Short paragraphs stay easy to scan. **Important text** keeps its emphasis.\n\n"
            "## A second section\n\n"
            "Markdown content is rendered with native controls.\n",
            encoding="utf-8",
        )
        (vault / "fallback.md").write_text(
            "# Diagram fallback\n\n"
            "```mermaid\nflowchart TB\nA ??? B\n```\n",
            encoding="utf-8",
        )
        states = ("empty", "reading", "editing", "diagram-fallback", "about")
        for language, prefix in (("en", "en"), ("zh-Hans", "zh-Hans")):
            for state_name in states:
                snapshot = output / f"{prefix}-{state_name}.png"
                arguments = ["--empty"] if state_name in ("empty", "about") else [
                    f"--vault={vault}",
                    f"--note={'fallback.md' if state_name == 'diagram-fallback' else 'reading.md'}",
                    "--reading-wait-for-scan",
                ]
                launch(language, base / prefix / state_name, arguments, snapshot)
                print(f"{snapshot.relative_to(ROOT)}: {snapshot.stat().st_size} bytes")


if __name__ == "__main__":
    main()
