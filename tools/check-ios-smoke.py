#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Static IPA audit and iOS Simulator smoke verification for Hortus."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import signal
import subprocess
import tempfile
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUNDLE_ID = "io.github.groundgrounder.Hortus"
FORBIDDEN_LINK_TOKENS = ("WebKit", "JavaScriptCore", "CoreServices", "SwiftTerm")
FORBIDDEN_TEXT_TOKENS = (b"WKWebView", b"JSContext", b"/Users/", b"/private/var/folders/", bytes(Path.home()))
DEFAULT_IPHONE_NAME = "iPhone 18 Pro"
DEFAULT_IPAD_NAME = "iPad Pro 13-inch (M5)"
DEFAULT_TEST_DEVICE_NAME = DEFAULT_IPHONE_NAME


def run(cmd: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, cwd=cwd, env=env, check=check, text=True, capture_output=True)


def audit_app_bundle(app_dir: Path) -> dict:
    info_plist = app_dir / "Info.plist"
    if not info_plist.is_file():
        raise AssertionError(f"Missing Info.plist in {app_dir}")
    with info_plist.open("rb") as fh:
        info = plistlib.load(fh)
    if info.get("CFBundleIdentifier") != BUNDLE_ID:
        raise AssertionError(f"Unexpected CFBundleIdentifier: {info.get('CFBundleIdentifier')}")
    exe_name = info.get("CFBundleExecutable")
    if exe_name != "Hortus":
        raise AssertionError(f"Unexpected CFBundleExecutable: {exe_name}")
    binary = app_dir / exe_name
    if not binary.is_file():
        raise AssertionError(f"Missing Mach-O binary: {binary}")

    archs = run(["lipo", "-archs", str(binary)]).stdout.strip().split()
    if archs != ["arm64"]:
        raise AssertionError(f"Expected single arm64 architecture, got {archs}")

    otool_out = run(["otool", "-L", str(binary)]).stdout
    for token in FORBIDDEN_LINK_TOKENS:
        if token in otool_out:
            raise AssertionError(f"Forbidden linked framework/symbol {token} in {binary}:\n{otool_out}")

    raw_bytes = binary.read_bytes()
    for token in FORBIDDEN_TEXT_TOKENS:
        if token in raw_bytes:
            raise AssertionError(f"Forbidden byte sequence {token!r} found in {binary}")

    katex_dir = app_dir / "Fonts" / "KaTeX"
    ttfs = sorted(p.name for p in katex_dir.glob("KaTeX_*.ttf")) if katex_dir.is_dir() else []
    if len(ttfs) != 20:
        raise AssertionError(f"Expected 20 KaTeX TTFs in {katex_dir}, found {len(ttfs)}: {ttfs}")
    if not (katex_dir / "OFL.txt").is_file():
        raise AssertionError(f"Missing Fonts/KaTeX/OFL.txt in {app_dir}")
    for required_doc in ("LICENSE.txt", "ThirdPartyNotices.txt"):
        doc = app_dir / required_doc
        if not doc.is_file() or doc.stat().st_size == 0:
            raise AssertionError(f"Missing or empty {required_doc} in {app_dir}")
    notices = (app_dir / "ThirdPartyNotices.txt").read_text(encoding="utf-8", errors="ignore")
    if "SwiftTerm" in notices:
        raise AssertionError("iOS ThirdPartyNotices.txt unexpectedly includes SwiftTerm")
    if not (app_dir / "Assets.car").is_file():
        raise AssertionError(f"Missing compiled Assets.car app icon in {app_dir}")

    web_files = [
        str(p.relative_to(app_dir))
        for p in app_dir.rglob("*")
        if p.is_file() and p.suffix.lower() in {".html", ".js"}
    ]
    if web_files:
        raise AssertionError(f"Unexpected HTML/JS files in app bundle: {web_files}")

    return {
        "bundle_id": info.get("CFBundleIdentifier"),
        "short_version": info.get("CFBundleShortVersionString"),
        "build_version": info.get("CFBundleVersion"),
        "minimum_os": info.get("MinimumOSVersion"),
        "archs": archs,
        "katex_fonts": len(ttfs),
        "binary_bytes": binary.stat().st_size,
        "otool_libs": [line.strip() for line in otool_out.splitlines()[1:] if line.strip()],
    }


def audit_ipa(ipa_path: Path) -> dict:
    if not ipa_path.is_file():
        raise AssertionError(f"Missing IPA: {ipa_path}")
    sha256 = hashlib.sha256(ipa_path.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix="hortus-ipa-audit-") as tmp:
        with zipfile.ZipFile(ipa_path, "r") as zf:
            names = zf.namelist()
            if not any(n.startswith("Payload/Hortus.app/") for n in names):
                raise AssertionError(f"IPA missing Payload/Hortus.app/: {ipa_path}")
            zf.extractall(tmp)
        app_dir = Path(tmp) / "Payload" / "Hortus.app"
        info = audit_app_bundle(app_dir)
        info["ipa_path"] = str(ipa_path)
        info["ipa_bytes"] = ipa_path.stat().st_size
        info["ipa_sha256"] = sha256
        return info


def snapshot_tree(root: Path) -> dict[str, str]:
    files: dict[str, str] = {}
    for p in sorted(root.rglob("*")):
        if p.is_file():
            files[str(p.relative_to(root))] = hashlib.sha256(p.read_bytes()).hexdigest()
    return files


def list_sim_devices() -> list[dict]:
    data = json.loads(run(["xcrun", "simctl", "list", "devices", "available", "--json"]).stdout)
    devices: list[dict] = []
    for runtime, entries in data.get("devices", {}).items():
        if "iOS" not in runtime:
            continue
        for entry in entries:
            if entry.get("isAvailable"):
                devices.append({**entry, "runtime": runtime})
    return devices


def pick_device(devices: list[dict], explicit_udid: str | None, preferred_name: str, kind: str) -> dict:
    if explicit_udid:
        for d in devices:
            if d["udid"] == explicit_udid:
                return d
        raise RuntimeError(f"Simulator UDID not found: {explicit_udid}")
    exact = [d for d in devices if d["name"] == preferred_name]
    if exact:
        exact.sort(key=lambda d: (d.get("state") == "Booted", d["runtime"]), reverse=True)
        return exact[0]
    fallback = [d for d in devices if kind in d["name"]]
    if not fallback:
        raise RuntimeError(f"No available {kind} simulator found")
    fallback.sort(key=lambda d: (d.get("state") == "Booted", d["runtime"]), reverse=True)
    return fallback[0]


def boot_simulator(udid: str) -> None:
    run(["xcrun", "simctl", "boot", udid], check=False)
    run(["xcrun", "simctl", "bootstatus", udid, "-b"])


def run_xctest(udid: str) -> dict:
    env = os.environ.copy()
    env["PATH"] = f"/opt/homebrew/opt/rustup/bin:{Path.home() / '.cargo' / 'bin'}:{env.get('PATH', '')}"
    result_bundle = Path("/tmp/hortus-ios-smoke.xcresult")
    if result_bundle.exists():
        shutil.rmtree(result_bundle, ignore_errors=True)
    cmd = [
        "xcodebuild",
        "test",
        "-project",
        "apps/ios/Hortus.xcodeproj",
        "-scheme",
        "Hortus",
        "-configuration",
        "Debug",
        "-sdk",
        "iphonesimulator",
        "-destination",
        f"id={udid}",
        "-derivedDataPath",
        "apps/ios/DerivedData",
        "-resultBundlePath",
        str(result_bundle),
        "CODE_SIGNING_ALLOWED=NO",
    ]
    print(f"[smoke] running XCTest on {udid}...", flush=True)
    proc = subprocess.Popen(
        cmd,
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    lines: list[str] = []
    passed_line = None
    assert proc.stdout is not None
    deadline = time.monotonic() + 240.0
    try:
        while time.monotonic() < deadline:
            line = proc.stdout.readline()
            if not line:
                if proc.poll() is not None:
                    break
                time.sleep(0.1)
                continue
            stripped = line.rstrip("\n")
            lines.append(stripped)
            if "Test Case" in stripped or "Test Suite" in stripped or "Executed " in stripped:
                print(f"[xctest] {stripped.strip()}", flush=True)
            if "Test Suite 'All tests' passed" in stripped:
                passed_line = stripped.strip()
            if "Test Suite 'All tests' failed" in stripped:
                time.sleep(0.5)
                tail = "\n".join(lines[-50:])
                raise AssertionError(f"xcodebuild test failed:\n{tail}")
            if "Executed " in stripped and "with 0 failures" in stripped and passed_line:
                time.sleep(0.5)
                break
        if proc.poll() is None:
            if passed_line:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            else:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                tail = "\n".join(lines[-50:])
                raise AssertionError(f"xcodebuild test timed out without passing:\n{tail}")
        elif proc.returncode != 0 and not passed_line:
            tail = "\n".join(lines[-60:])
            raise AssertionError(f"xcodebuild test failed ({proc.returncode}):\n{tail}")
    finally:
        if proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    executed = [l.strip() for l in lines if "Executed " in l]
    return {"device_udid": udid, "summary": executed[-1] if executed else passed_line or "passed"}


def launch_and_capture(udid: str, appearance: str, args: list[str], out_png: Path, wait_s: float = 2.0) -> dict:
    run(["xcrun", "simctl", "ui", udid, "appearance", appearance])
    run(["xcrun", "simctl", "terminate", udid, BUNDLE_ID], check=False)
    launch = run(["xcrun", "simctl", "launch", udid, BUNDLE_ID, *args])
    m = re.search(r":\s*(\d+)", launch.stdout)
    pid = m.group(1) if m else ""
    time.sleep(wait_s)
    ps_out = run(["xcrun", "simctl", "spawn", udid, "launchctl", "list"], check=False).stdout
    if BUNDLE_ID not in ps_out and not pid:
        raise AssertionError(f"App failed to stay running for {out_png.name}")
    out_png.parent.mkdir(parents=True, exist_ok=True)
    run(["xcrun", "simctl", "io", udid, "screenshot", str(out_png)])
    if not out_png.is_file() or out_png.stat().st_size < 10_000:
        raise AssertionError(f"Screenshot invalid or empty: {out_png}")
    print(f"[smoke] captured {out_png.name} ({out_png.stat().st_size} bytes)", flush=True)
    return {"file": str(out_png), "bytes": out_png.stat().st_size, "pid": pid, "appearance": appearance, "args": args}


def run_simulator_smoke(args: argparse.Namespace) -> dict:
    app_path = Path(args.app).resolve()
    if not app_path.is_dir():
        raise AssertionError(f"Missing simulator app: {app_path}")

    demo_dir = ROOT / "fixtures" / "demo-vault"
    before_vault = snapshot_tree(demo_dir)

    devices = list_sim_devices()
    iphone = pick_device(devices, args.iphone_udid, DEFAULT_IPHONE_NAME, "iPhone")
    ipad = pick_device(devices, args.ipad_udid, DEFAULT_IPAD_NAME, "iPad")
    test_device = pick_device(devices, None, DEFAULT_TEST_DEVICE_NAME, "iPhone")

    test_report = None
    if not args.skip_tests:
        boot_simulator(test_device["udid"])
        test_report = run_xctest(test_device["udid"])

    boot_simulator(iphone["udid"])
    boot_simulator(ipad["udid"])
    for d in (iphone, ipad):
        run(["xcrun", "simctl", "uninstall", d["udid"], BUNDLE_ID], check=False)
        run(["xcrun", "simctl", "install", d["udid"], str(app_path)])

    media_dir = Path(args.media).expanduser().resolve()
    media_dir.mkdir(parents=True, exist_ok=True)

    scenarios = [
        ("iphone-light-start.png", iphone["udid"], "light", ["--demo", "--reset-demo", "--note", "Start here.md", "--theme", "light"], 2.2),
        ("iphone-light-guide.png", iphone["udid"], "light", ["--demo", "--note", "Markdown field guide.md", "--reveal", "a^2 + b^2", "--theme", "light"], 2.4),
        ("iphone-dark-guide.png", iphone["udid"], "dark", ["--demo", "--note", "Markdown field guide.md", "--reveal", "Content: CC0", "--theme", "dark"], 2.4),
        ("iphone-light-focus.png", iphone["udid"], "light", ["--demo", "--note", "Markdown field guide.md", "--focus", "1", "--theme", "light"], 2.6),
        ("iphone-dark-search.png", iphone["udid"], "dark", ["--demo", "--note", "Markdown field guide.md", "--panel", "search", "--query", "Mermaid", "--theme", "dark"], 2.4),
        ("iphone-light-edit.png", iphone["udid"], "light", ["--demo", "--note", "Markdown field guide.md", "--mode", "editing", "--theme", "light"], 2.0),
        ("iphone-dark-settings.png", iphone["udid"], "dark", ["--demo", "--settings", "--theme", "dark"], 2.0),
        ("ipad-light-reading.png", ipad["udid"], "light", ["--demo", "--reset-demo", "--note", "Markdown field guide.md", "--theme", "light"], 2.5),
        ("ipad-dark-search.png", ipad["udid"], "dark", ["--demo", "--note", "Markdown field guide.md", "--panel", "search", "--query", "Mermaid", "--theme", "dark"], 2.4),
        ("ipad-light-focus.png", ipad["udid"], "light", ["--demo", "--note", "Markdown field guide.md", "--focus", "1", "--theme", "light"], 2.6),
    ]

    captures = []
    for filename, udid, appearance, launch_args, wait_s in scenarios:
        captures.append(launch_and_capture(udid, appearance, launch_args, media_dir / filename, wait_s))
    for d in (iphone, ipad):
        run(["xcrun", "simctl", "terminate", d["udid"], BUNDLE_ID], check=False)

    after_vault = snapshot_tree(demo_dir)
    if before_vault != after_vault:
        raise AssertionError("fixtures/demo-vault changed during simulator smoke run")

    return {
        "app": str(app_path),
        "iphone": {"name": iphone["name"], "udid": iphone["udid"], "runtime": iphone["runtime"]},
        "ipad": {"name": ipad["name"], "udid": ipad["udid"], "runtime": ipad["runtime"]},
        "xctest": test_report,
        "demo_vault_untouched": True,
        "screenshots": captures,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Verify Hortus iOS IPA and simulator build.")
    parser.add_argument("--ipa", help="Path to Hortus-unsigned.ipa for static IPA audit.")
    parser.add_argument("--simulator", action="store_true", help="Run simulator tests and screenshots.")
    parser.add_argument("--app", default=str(ROOT / "build" / "ios" / "Hortus-sim.app"), help="Simulator .app path.")
    parser.add_argument("--iphone-udid", help="Explicit iPhone simulator UDID.")
    parser.add_argument("--ipad-udid", help="Explicit iPad simulator UDID.")
    parser.add_argument("--media", default=str(ROOT / "build" / "ios" / "smoke-media"), help="Screenshot directory.")
    parser.add_argument("--output", default=str(ROOT / "build" / "ios" / "smoke-output"), help="JSON output directory.")
    parser.add_argument("--skip-tests", action="store_true", help="Skip xcodebuild test during simulator check.")
    args = parser.parse_args()

    if not args.ipa and not args.simulator:
        parser.error("Specify --ipa and/or --simulator")

    report: dict = {}
    default_ipa = ROOT / "build" / "ios" / "Hortus-unsigned.ipa"
    ipa_target = Path(args.ipa).resolve() if args.ipa else (default_ipa if args.simulator and default_ipa.is_file() else None)
    if ipa_target is not None:
        report["ipa"] = audit_ipa(ipa_target)
    if args.simulator:
        if ipa_target is None:
            report["app_audit"] = audit_app_bundle(Path(args.app).resolve())
        report["simulator"] = run_simulator_smoke(args)

    out_dir = Path(args.output).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    out_file = out_dir / "ios-smoke.json"
    out_file.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
