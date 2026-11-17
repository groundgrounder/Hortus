#!/usr/bin/env python3
"""P1-58 settings panel: comment-preserving config, conflict guard, live propagation.

Build first with ./tools/build-macos.sh; the packaged app provides the compiled
String Catalog for en / zh-Hans screenshots. All application state is isolated.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
FAKE_AGENT = ROOT / "tools/fixtures/fake-agent.sh"

CONFIG = """# 顶部说明 top comment
font_size = 15.0 # 字号 inline
default_view = "reading"
unknown = { keep = [1, 2] }

[[vault]]
path = "/vault/a"
font_size = 18.0 # vault 覆盖行尾注释 keep

[[vault]]
path = "/vault/b"

[agent]
command = "/bin/sh"
args = ["-c", "printf '%s' {guide}"] # argv inline
note = "keep"
"""


def environment(base: Path) -> dict:
    return dict(os.environ,
                HOME=str(base),
                HORTUS_CONFIG_DIR=str(base / "config"),
                HORTUS_CACHE_DIR=str(base / "cache"),
                HORTUS_DATA_DIR=str(base / "data"),
                FAKE_AGENT_SCRATCH=str(base / "scratch"))


def write_fixture(base: Path) -> tuple[Path, Path]:
    for name in ("a", "b"):
        vault = base / name
        vault.mkdir(parents=True, exist_ok=True)
        (vault / "same.md").write_text(f"# Vault {name}\n", encoding="utf-8")
        (vault / "second.md").write_text(f"# Second {name}\n", encoding="utf-8")
    config = base / "config"
    config.mkdir(parents=True, exist_ok=True)
    (config / "config.toml").write_text(CONFIG, encoding="utf-8")
    (config / "agent-guide.md").write_text("seed guide\n", encoding="utf-8")
    (base / "cache").mkdir(exist_ok=True)
    return base / "a", base / "b"


def package_debug_app(binary: Path, destination: Path) -> Path:
    """A Debug .app with the compiled String Catalog for localized window evidence.

    The release app intentionally contains none of the self-check arguments, so
    behavior and screenshots use this temporary Debug bundle; the release binary
    is checked separately for absent markers.
    """
    app = destination / "Hortus Debug.app"
    if app.exists():
        shutil.rmtree(app)
    macos = app / "Contents/MacOS"
    resources = app / "Contents/Resources"
    macos.mkdir(parents=True)
    resources.mkdir(parents=True)
    shutil.copy2(binary, macos / "Hortus")
    shutil.copy2(ROOT / "apps/macos/Info.plist", app / "Contents/Info.plist")
    tool = subprocess.run(["xcrun", "--find", "xcstringstool"], check=True,
                          capture_output=True, text=True).stdout.strip()
    subprocess.run([tool, "compile", str(ROOT / "apps/macos/Localizable.xcstrings"),
                    "--output-directory", str(resources),
                    "--language", "en", "--language", "zh-Hans",
                    "--serialization-format", "text"], check=True,
                   capture_output=True, text=True)
    fonts = resources / "Fonts/KaTeX"
    shutil.copytree(ROOT / "apps/macos/Resources/Fonts/KaTeX", fonts)
    subprocess.run(["codesign", "--force", "--sign", "-", str(app)], check=True,
                   capture_output=True, text=True)
    return macos / "Hortus"


def wait_for(path: Path, process, seconds: float) -> None:
    deadline = time.monotonic() + seconds
    while not path.exists():
        if process.poll() is not None:
            _, stderr = process.communicate()
            raise AssertionError(f"app exited early: {stderr.decode(errors='replace')}")
        if time.monotonic() > deadline:
            process.kill()
            raise AssertionError(f"timed out waiting for {path}")
        time.sleep(0.1)


def inspect(app: Path, base: Path, label: str, language: str, theme: str,
            output: Path, media: Path | None) -> dict:
    vault_a, vault_b = write_fixture(base)
    agent = base / "fake-agent.sh"
    shutil.copy2(FAKE_AGENT, agent)
    agent.chmod(0o755)
    result = output / f"{label}.json"
    result.unlink(missing_ok=True)
    command = [str(app), "-AppleLanguages", f"({language})", f"--{theme}",
               f"--vault={vault_a}", f"--settings-second-vault={vault_b}",
               f"--settings-inspection={result}",
               f"--settings-agent={agent}"]
    if media is not None:
        command += [f"--settings-media={media}", f"--settings-label={label}"]
    process = subprocess.Popen(command, env=environment(base),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait_for(result, process, 180)
        _, stderr = process.communicate(timeout=60)
        assert process.returncode == 0, stderr.decode(errors="replace")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=15)
    return json.loads(result.read_text())


def validate(data: dict, base: Path, screenshots: list[Path]) -> dict:
    assert data["conflict_detected"], data
    assert data["external_edit_preserved"], data
    assert data["comments_preserved"], data
    assert data["unknown_preserved"], data
    assert data["vault_preserved"], data
    assert data["saved_font_size"] and data["saved_view"] and data["saved_font"], data
    assert data["settings_window_is_vault"] is False, data
    assert data["vault_windows_before"] == 2, data
    assert data["pids_stable"] and all(pid > 0 for pid in data["pids_before"]), data
    assert data["font_size_propagated"], data
    assert data["font_propagated"], data
    assert data["buffer_preserved"], data
    assert data["nerdfont_provider_preserved"], data
    glyph = data["nerdfont_glyph"]
    assert glyph["checked"], data
    assert glyph["provider"] and glyph["cells"] >= 1, glyph
    assert any("Nerd" in name or "NF-" in name or "NFM-" in name for name in glyph["fonts"]), glyph
    assert data["default_view_applied"] and data["default_view"] == "editing", data
    assert data["guide_overridden"], data
    assert data["guide_chars"] > 0, data
    assert data["probe_available"], data
    assert data["agent_guide_received"], data
    assert data["settings_closed"], data
    assert data["records_after_close"] == 2 and data["vault_windows_after_close"] == 2, data
    text = (base / "config/config.toml").read_text(encoding="utf-8")
    for kept in ("# 顶部说明 top comment", "# 字号 inline", "# vault 覆盖行尾注释 keep",
                 "unknown = { keep = [1, 2] }", 'note = "keep"'):
        assert kept in text, (kept, text)
    assert text.count("\n[[vault]]") == 2, text
    assert 'font_size = 20.0' in text and 'default_view = "editing"' in text, text
    assert f'terminal_font = "{data["terminal_font"]}"' in text, text
    assert "# external manual edit" in text, text
    if screenshots:
        assert len(screenshots) == 3, screenshots
        for shot in screenshots:
            assert shot.is_file() and shot.stat().st_size > 10_000, shot
    return {
        "terminal_font": data["terminal_font"],
        "font_families": data["font_families"],
        "pids": data["pids_before"],
        "glyph": glyph,
        "guide_chars": data["guide_chars"],
        "agent_args": data["agent_args_received"],
        "vaults": data["vault_windows_before"],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/settings")
    parser.add_argument("--media", type=Path)
    parser.add_argument("--app", type=Path,
                        default=ROOT / "build/Hortus.app/Contents/MacOS/Hortus")
    parser.add_argument("--debug-app", type=Path,
                        default=ROOT / "apps/macos/.build/debug/Hortus")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    release = args.app.resolve()
    debug = args.debug_app.resolve()
    assert release.is_file(), f"missing {release}; run ./tools/build-macos.sh first"
    assert debug.is_file(), f"missing {debug}; run swift build --package-path apps/macos first"
    summary = {}

    with tempfile.TemporaryDirectory(prefix="hortus-settings-") as temporary:
        base = Path(temporary)
        app = package_debug_app(debug, base / "bundle")
        data = inspect(app, base / "behavior", "en-light", "en", "light", output, None)
        summary["behavior"] = validate(data, base / "behavior", [])

        media = (args.media or output / "screenshots").resolve()
        media.mkdir(parents=True, exist_ok=True)
        hashes = {}
        for language, prefix in (("en", "en"), ("zh-Hans", "zh-Hans")):
            for theme in ("light", "dark"):
                label = f"{prefix}-{theme}"
                run_base = base / label
                shots = [media / f"settings-{label}-{name}.png"
                         for name in ("general", "terminal", "agent")]
                data = inspect(app, run_base, label, language, theme, output, media)
                summary[label] = validate(data, run_base, shots)
                hashes[label] = {shot.name: hashlib.sha256(shot.read_bytes()).hexdigest()
                                 for shot in shots}
        assert hashes["en-light"]["settings-en-light-general.png"] != \
               hashes["zh-Hans-light"]["settings-zh-Hans-light-general.png"], "zh-Hans screenshots did not change"
        (output / "screenshot-hashes.json").write_text(
            json.dumps(hashes, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    markers = [b"--settings-inspection=", b"Settings check guide"]
    binary = release.read_bytes()
    for marker in markers:
        assert marker not in binary, f"Debug self-check marker in release executable: {marker!r}"
    summary["release_markers_absent"] = [marker.decode() for marker in markers]
    (output / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"runs": len(summary) - 1,
                      "terminal_font": summary["behavior"]["terminal_font"],
                      "pids": summary["behavior"]["pids"],
                      "glyph_fonts": summary["behavior"]["glyph"]["fonts"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
