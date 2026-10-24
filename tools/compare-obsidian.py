#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""External release measurements for Hortus and Obsidian.

The two applications are driven from outside the application.  Each run gets
its own temporary vault and isolated Hortus configuration/cache directories.
The script intentionally keeps all generated notes, screenshots and raw JSON
under the requested output directory or a temporary directory that is removed
before exit.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager, redirect_stdout
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Iterable
from unittest.mock import patch
from urllib.parse import quote, urlencode


ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
DEFAULT_APP = ROOT / "build/Hortus.app"
DEFAULT_OBSIDIAN = Path("/Applications/Obsidian.app")
DEFAULT_VAULT = ROOT / "fixtures/demo-vault"
DEFAULT_OUTPUT = ROOT / "target/reports/obsidian-comparison"
OWNER_NAMES = {"hortus": "Hortus", "obsidian": "Obsidian"}
OWNED_PROCESSES: list[subprocess.Popen[str]] = []


WINDOW_HELPER = r'''
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import Vision

func encode(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}
func mouseClick(_ point: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)
        event?.setIntegerValueField(.mouseEventClickState, value: 1)
        event?.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
}
func screenWords(_ pid: Int32) throws -> [[String: Any]] {
    let candidates = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    guard let info = candidates.first(where: {
        ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && ($0[kCGWindowLayer as String] as? Int) == 0
    }), let bounds = info[kCGWindowBounds as String] as? [String: CGFloat],
       let windowID = info[kCGWindowNumber as String] as? Int else { return [] }
    let path = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".png")
    defer { try? FileManager.default.removeItem(at: path) }
    let capture = Process()
    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    capture.arguments = ["-x", "-o", "-l", String(windowID), path.path]
    try capture.run(); capture.waitUntilExit()
    guard capture.terminationStatus == 0 else { return [] }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.recognitionLanguages = ["zh-Hans", "en-US"]
    request.automaticallyDetectsLanguage = true
    request.minimumTextHeight = 0.005
    try VNImageRequestHandler(url: path).perform([request])
    return (request.results ?? []).compactMap { observation in
        guard let label = observation.topCandidates(1).first?.string else { return nil }
        let box = observation.boundingBox
        return ["label": label, "x": bounds["X"]! + box.midX * bounds["Width"]!,
                "y": bounds["Y"]! + (1 - box.midY) * bounds["Height"]!,
                "relative_x": box.midX]
    }
}
if CommandLine.arguments.count > 3, CommandLine.arguments[1] == "--click",
   let x = Double(CommandLine.arguments[2]), let y = Double(CommandLine.arguments[3]) {
    mouseClick(CGPoint(x: x, y: y)); exit(0)
}
if CommandLine.arguments.count > 3, CommandLine.arguments[1] == "--locate",
   let pid = Int32(CommandLine.arguments[2]) {
    NSRunningApplication(processIdentifier: pid)?.activate()
    let needle = CommandLine.arguments[3]
    let words = try screenWords(pid)
    let match = words.first {
        let label = $0["label"] as? String ?? ""
        return [needle, needle + ".md"].contains(label) && ($0["relative_x"] as? CGFloat ?? 1) < 0.35
    }
    encode(["found": match != nil, "target": match ?? [:], "labels": words.map { $0["label"]! }]); exit(0)
}
if CommandLine.arguments.count > 2, CommandLine.arguments[1] == "--prepare",
   let pid = Int32(CommandLine.arguments[2]) {
    guard AXIsProcessTrusted() else { fputs("Accessibility access unavailable\n", stderr); exit(1) }
    NSRunningApplication(processIdentifier: pid)?.activate()
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 1)
    var value: CFTypeRef?
    AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value)
    guard let windows = value as? [AXUIElement], let window = windows.first else { exit(2) }
    var size = CGSize(width: 1100, height: 800)
    var point = CGPoint(x: 200, y: 100)
    AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!)
    AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &point)!)
    Thread.sleep(forTimeInterval: 0.5)
    var clicked = false
    if CommandLine.arguments.contains("--allow-mermaid") {
        let words = try screenWords(pid)
        if let button = words.first(where: { ["Allow", "允许", "允許"].contains($0["label"] as? String ?? "") }),
           let x = button["x"] as? CGFloat, let y = button["y"] as? CGFloat {
            mouseClick(CGPoint(x: x, y: y)); clicked = true
            Thread.sleep(forTimeInterval: 2)
        }
    }
    let words = try screenWords(pid)
    let labels = words.compactMap { $0["label"] as? String }
    let hasCode = labels.contains { $0.contains("flowchart") || $0.contains("root[") || $0.contains("root [") }
    let hasPrompt = labels.contains { $0.contains("信任") || $0.contains("在此仓库") || $0.contains("trust this vault") }
    let graphLabels = labels.filter { text in
        ["Frame the question", "Name the unknown", "Choose a useful scope", "Ask better"].contains { text.contains($0) }
    }
    encode(["mermaid_allow_clicked": clicked, "diagram_ready": !hasCode && !hasPrompt && graphLabels.count >= 2,
            "visible_labels": labels]); exit(0)
}

let ownerNeedle = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
let titleNeedles = CommandLine.arguments.count > 2
    ? CommandLine.arguments[2].split(separator: "|", omittingEmptySubsequences: true).map(String.init)
    : []
let ownerOnly = CommandLine.arguments.contains("--owner-only")
let pidNeedle = CommandLine.arguments.count > 3 ? Int(CommandLine.arguments[3]) : nil

func lower(_ value: String?) -> String { (value ?? "").localizedLowercase }
let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]]) ?? []
for window in windows {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    let title = window[kCGWindowName as String] as? String ?? ""
    let layer = (window[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 999
    guard layer == 0, lower(owner).contains(lower(ownerNeedle)) else { continue }
    let pid = (window[kCGWindowOwnerPID as String] as? NSNumber)?.intValue ?? 0
    if let pidNeedle, pid != pidNeedle { continue }
    let titleMatch = titleNeedles.isEmpty || titleNeedles.contains {
        lower(title) == lower($0) || lower(title).hasPrefix(lower($0) + " - ")
    }
    if !titleMatch && !ownerOnly { continue }
    let result: [String: Any] = [
        "owner": owner,
        "title": title,
        "title_match": titleMatch,
        "window_id": (window[kCGWindowNumber as String] as? NSNumber)?.intValue ?? 0,
        "pid": (window[kCGWindowOwnerPID as String] as? NSNumber)?.intValue ?? 0,
        "layer": layer,
        "bounds": window[kCGWindowBounds as String] ?? [:]
    ]
    if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]),
       let line = String(data: data, encoding: .utf8) {
        print(line)
    }
}
'''


def command(args: Iterable[str | Path], *, timeout: float = 30, env: dict[str, str] | None = None,
            check: bool = False) -> subprocess.CompletedProcess[str]:
    argv = [str(arg) for arg in args]
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                              env=env, check=check)
    except (OSError, subprocess.TimeoutExpired) as error:
        # The managed test environment can deny process inspection or GUI
        # access.  Preserve the failure as data so a measurement report still
        # explains which primitive was unavailable instead of crashing halfway
        # through and losing the other measurements.
        if isinstance(error, subprocess.TimeoutExpired):
            stderr = f"command timed out after {timeout}s: {' '.join(argv)}"
            return subprocess.CompletedProcess(argv, 124, "", stderr)
        return subprocess.CompletedProcess(argv, 126, "", f"{type(error).__name__}: {error}")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def median(values: list[float | int]) -> float | None:
    return statistics.median(values) if values else None


def app_info(app: Path) -> dict[str, Any]:
    plist_path = app / "Contents/Info.plist"
    try:
        data = plistlib.loads(plist_path.read_bytes())
    except (OSError, plistlib.InvalidFileException) as error:
        return {"path": str(app), "error": str(error)}
    return {
        "path": str(app),
        "version": data.get("CFBundleShortVersionString"),
        "build": data.get("CFBundleVersion"),
        "bundle_identifier": data.get("CFBundleIdentifier"),
        "executable": data.get("CFBundleExecutable"),
    }


def disk_usage(path: Path) -> dict[str, Any]:
    du = command(["du", "-sk", str(path)], check=False)
    human = command(["du", "-sh", str(path)], check=False)
    allocated_kib = None
    if du.returncode == 0:
        try:
            allocated_kib = int(du.stdout.split()[0])
        except (IndexError, ValueError):
            pass
    logical_bytes = sum(item.stat().st_size for item in path.rglob("*") if item.is_file())
    return {
        "du_kib": allocated_kib,
        "du_bytes": allocated_kib * 1024 if allocated_kib is not None else None,
        "logical_bytes": logical_bytes,
        "du_command": du.args,
        "du_stderr": du.stderr.strip(),
        "du_sh": human.stdout.strip(),
    }


def copy_vault(source: Path, destination: Path) -> None:
    shutil.copytree(source, destination)


def make_large_note(vault: Path, name: str = "1 MiB.md") -> dict[str, Any]:
    # Keep the source compact while making a deterministic UTF-8 document just
    # over one MiB.  The content is original and carries the same CC0 notice as
    # the public demo vault.
    chunks = ["# Measurement note\n\nSPDX-License-Identifier: CC0-1.0\n\n"]
    size = len(chunks[0].encode())
    target = 1024 * 1024
    index = 0
    while size < target:
        chunk = f"Paragraph {index}: **public text** with a UTF-8 measurement sentence 中文😀.\n\n"
        chunks.append(chunk)
        size += len(chunk.encode("utf-8"))
        index += 1
    path = vault / name
    path.write_text("".join(chunks))
    return {"path": name, "utf8_bytes": path.stat().st_size, "paragraphs": index}


def compile_window_helper(directory: Path) -> Path | None:
    source = directory / "window_probe.swift"
    binary = directory / "window_probe"
    source.write_text(WINDOW_HELPER)
    result = command(["swiftc", str(source), "-framework", "CoreGraphics", "-o", str(binary)], timeout=60)
    if result.returncode != 0:
        (directory / "window_probe.stderr").write_text(result.stderr)
        return None
    return binary


def windows(helper: Path | None, owner: str, titles: list[str], owner_only: bool = False,
            pid: int | None = None) -> list[dict[str, Any]]:
    if helper is None:
        return []
    needles = "|".join(title.replace("|", "") for title in titles)
    args = [str(helper), owner, needles, str(pid or 0)]
    if owner_only:
        args.append("--owner-only")
    result = command(args, timeout=5)
    if result.returncode != 0:
        return []
    found: list[dict[str, Any]] = []
    for line in result.stdout.splitlines():
        try:
            found.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return found


def process_table() -> tuple[dict[int, dict[str, Any]], str | None]:
    result = command(["ps", "-axo", "pid=,ppid=,time=,rss=,comm="], timeout=10)
    if result.returncode != 0:
        return {}, result.stderr.strip() or f"ps exited {result.returncode}"
    table: dict[int, dict[str, Any]] = {}
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) < 5:
            continue
        try:
            pid, ppid, cpu, rss = int(parts[0]), int(parts[1]), parts[2], int(parts[3])
        except ValueError:
            continue
        table[pid] = {"pid": pid, "ppid": ppid, "cpu": cpu, "rss_kib": rss,
                      "comm": " ".join(parts[4:])}
    return table, None


def descendants(table: dict[int, dict[str, Any]], roots: Iterable[int]) -> list[int]:
    result = set(int(pid) for pid in roots if int(pid) in table)
    changed = True
    while changed:
        changed = False
        for pid, item in table.items():
            if item["ppid"] in result and pid not in result:
                result.add(pid)
                changed = True
    return sorted(result)


def parse_cpu_time(value: str) -> float:
    # ps on macOS uses [[dd-]hh:]mm:ss[.cc].
    value = value.strip()
    days = 0
    if "-" in value:
        day_text, value = value.split("-", 1)
        days = int(day_text)
    fields = value.split(":")
    if len(fields) == 3:
        hours, minutes, seconds = fields
    elif len(fields) == 2:
        hours, minutes, seconds = "0", fields[0], fields[1]
    else:
        return 0.0
    return days * 86400 + int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def cpu_seconds(table: dict[int, dict[str, Any]], pids: Iterable[int]) -> float:
    return sum(parse_cpu_time(table[pid]["cpu"]) for pid in pids if pid in table)


def rss_bytes(table: dict[int, dict[str, Any]], pids: Iterable[int]) -> int:
    return sum(table[pid]["rss_kib"] * 1024 for pid in pids if pid in table)


def footprint_bytes(pids: Iterable[int], output: Path) -> tuple[int | None, str, list[str]]:
    values: list[int] = []
    errors: list[str] = []
    for pid in pids:
        result = command(["footprint", "-p", str(pid), "-f", "bytes", "--noCategories"], timeout=20)
        output.mkdir(parents=True, exist_ok=True)
        (output / f"{pid}.stdout.txt").write_text(result.stdout)
        (output / f"{pid}.stderr.txt").write_text(result.stderr)
        if result.returncode != 0:
            errors.append(result.stderr.strip() or f"footprint exited {result.returncode} for {pid}")
            continue
        measured = parse_footprint(result.stdout)
        if measured is not None:
            values.append(measured)
        else:
            errors.append(f"unparsed footprint output for {pid}: {result.stdout.strip()[:1000]}")
    if values and not errors:
        return sum(values), "footprint", errors
    return None, "unavailable", errors


def parse_footprint(text: str) -> int | None:
    # The auxiliary current footprint is distinct from phys_footprint_peak.
    for pattern in (r"^\s*phys_footprint:\s*([0-9,]+)\s+B\s*$",
                    r"^\s*Physical footprint:\s*([0-9,]+)\s*(?:bytes|B)\s*$"):
        matches = re.findall(pattern, text, flags=re.MULTILINE | re.IGNORECASE)
        if len(matches) == 1:
            return int(matches[0].replace(",", ""))
    return None


def lsof_connections(pids: Iterable[int]) -> dict[str, Any]:
    rows: list[str] = []
    errors: list[str] = []
    pids = list(pids)
    if not pids:
        return {"connections": [], "errors": ["no target processes found"], "available": False}
    for selection in [",".join(map(str, pids))]:
        result = command(["lsof", "-nP", "-a", "-p", selection, "-i"], timeout=10)
        if result.returncode not in (0, 1):
            errors.append(result.stderr.strip() or f"lsof exited {result.returncode} for {selection}")
        elif result.stderr.strip():
            errors.append(result.stderr.strip())
        rows.extend(line for line in result.stdout.splitlines()[1:] if line.strip())
    return {"connections": sorted(set(rows)), "errors": errors,
            "available": not errors or bool(rows)}


class NetworkMonitor:
    """Sample only the launched process tree from launch through shutdown."""

    def __init__(self, process: subprocess.Popen[str]):
        self.process = process
        self.pid = process.pid
        self.samples: list[dict[str, Any]] = []
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self) -> None:
        started = time.monotonic()
        next_sample = started
        while not self.stopped.is_set():
            if self.process.poll() is not None:
                break
            table, error = process_table()
            pids = descendants(table, [self.pid])
            # Inspection helpers exit on their own.  Their monitor can outlive
            # them until the enclosing group finishes; shutdown is not a
            # failed network sample, and must not become a zero-connection one.
            if not pids and self.process.poll() is not None:
                break
            sample = lsof_connections(pids)
            if error:
                sample["errors"].append(error)
                sample["available"] = False
            sample.update({"elapsed_seconds": time.monotonic() - started, "pids": pids})
            self.samples.append(sample)
            next_sample += 1
            self.stopped.wait(max(0, next_sample - time.monotonic()))

    def stop(self) -> None:
        self.stopped.set()
        self.thread.join(timeout=12)


def purge_caches() -> dict[str, Any]:
    result = command(["purge"], timeout=20)
    return {"requested": True, "performed": result.returncode == 0,
            "returncode": result.returncode, "stdout": result.stdout.strip(),
            "stderr": result.stderr.strip()}


def wait_for_window(helper: Path | None, owner: str, titles: list[str], timeout: float = 30,
                    process: subprocess.Popen[str] | None = None) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = windows(helper, owner, titles, pid=process.pid if process else None)
        if found:
            return {"found": True, "title_match": True, "window": found[0], "poll_seconds": timeout - (deadline - time.monotonic())}
        if process is not None and process.poll() is not None:
            return {"found": False, "title_match": False, "window": None,
                    "poll_seconds": timeout - (deadline - time.monotonic()),
                    "process_returncode": process.returncode}
        time.sleep(0.05)
    # A dialog or vault chooser must never count as the requested note.
    found = windows(helper, owner, titles, owner_only=True, pid=process.pid if process else None)
    if found:
        return {"found": False, "title_match": False, "window": found[0], "poll_seconds": timeout,
                "error": "target note title not found; diagnostic window only"}
    return {"found": False, "title_match": False, "window": None, "poll_seconds": timeout}


def quit_app(kind: str, process: subprocess.Popen[str] | None = None,
             window: dict[str, Any] | None = None) -> dict[str, Any]:
    # Only stop a process launched by this script, never a user's open app.
    if process is not None and hasattr(process, "network_monitor"):
        process.network_monitor.stop()
    table, _ = process_table() if process is not None and process.poll() is None else ({}, None)
    owned = descendants(table, [process.pid]) if process else []
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    deadline = time.monotonic() + 5
    remaining = []
    while owned:
        table, _ = process_table()
        remaining = [pid for pid in owned if pid in table and table[pid]["comm"] != "<defunct>"]
        if not remaining or time.monotonic() >= deadline:
            break
        time.sleep(.05)
    if remaining:
        raise RuntimeError(f"owned processes did not exit: {remaining}")
    return {"method": "terminate-owned-process", "returncode": process.returncode if process else None,
            "remaining_pids": remaining}


def cleanup_processes() -> None:
    for process in reversed(OWNED_PROCESSES):
        quit_app("owned", process)
    OWNED_PROCESSES.clear()


def prepare_obsidian(state: Path, vault: Path) -> str:
    """Register only the temporary vault in a disposable Electron profile."""
    profile = state / "obsidian-profile"
    profile.mkdir(parents=True, exist_ok=True)
    vault_id = hashlib.sha256(str(vault).encode()).hexdigest()[:16]
    registry = profile / "obsidian.json"
    if not registry.exists():
        write_json(registry, {"vaults": {vault_id: {"path": str(vault),
                                                "ts": int(time.time() * 1000), "open": True}}})
    return vault_id


def isolated_env(state: Path) -> dict[str, str]:
    state.joinpath("config").mkdir(parents=True, exist_ok=True)
    state.joinpath("cache").mkdir(parents=True, exist_ok=True)
    return dict(os.environ, HORTUS_CONFIG_DIR=str(state / "config"),
                HORTUS_CACHE_DIR=str(state / "cache"))


def launch(kind: str, app: Path, vault: Path, note: str, state: Path,
           helper: Path | None, *, timeout: float = 30) -> tuple[subprocess.Popen[str] | None, dict[str, Any]]:
    owner = OWNER_NAMES[kind]
    titles = [Path(note).name, Path(note).stem]
    state.mkdir(parents=True, exist_ok=True)
    vault_id = prepare_obsidian(state, vault) if kind == "obsidian" else None
    env = isolated_env(state) if kind == "hortus" else None
    log = state / "launch.stderr.txt"
    started = time.perf_counter()
    try:
        if kind == "obsidian":
            url = "obsidian://open?" + urlencode({"vault": vault_id, "file": note}, quote_via=quote)
            # Calling the bundle executable directly avoids LaunchServices
            # returning -10827 (kLSNoExecutableErr) when `open -a` is given a
            # bundle path in a managed test session.  Obsidian accepts the
            # obsidian:// URI as its first command-line argument and brings
            # the requested vault/note to the foreground.
            executable = app / "Contents/MacOS/Obsidian"
            arguments = [str(executable), f"--user-data-dir={state / 'obsidian-profile'}", url]
        else:
            executable = app / "Contents/MacOS/Hortus"
            arguments = [str(executable), f"--vault={vault}", f"--note={note}", "--light"]
        with log.open("w") as stderr:
            process = subprocess.Popen(arguments, env=env, stdout=subprocess.DEVNULL,
                                       stderr=stderr, text=True)
    except OSError as error:
        return None, {"kind": kind, "note": note, "found": False, "title_match": False,
                      "window": None, "elapsed_ms": (time.perf_counter() - started) * 1000,
                      "start_event": "process/open command", "launch_error": f"{type(error).__name__}: {error}"}
    OWNED_PROCESSES.append(process)
    process.network_monitor = NetworkMonitor(process)
    result = wait_for_window(helper, owner, titles, timeout, process)
    result.update({"kind": kind, "note": note, "start_event": "process/open command",
                   "elapsed_ms": (time.perf_counter() - started) * 1000,
                   "network_samples": process.network_monitor.samples,
                   "launch_stderr": log.read_text()[:4096] if process.poll() is not None else ""})
    return process, result


def capture_window(window: dict[str, Any] | None, path: Path) -> dict[str, Any]:
    if not window or not window.get("window_id"):
        return {"path": str(path), "captured": False, "error": "window id unavailable"}
    path.parent.mkdir(parents=True, exist_ok=True)
    result = command(["screencapture", "-x", "-l", str(window["window_id"]), str(path)], timeout=20)
    return {"path": str(path), "captured": result.returncode == 0,
            "returncode": result.returncode, "stderr": result.stderr.strip(),
            "bytes": path.stat().st_size if path.exists() else 0}


def toggle_obsidian_preview(pid: int) -> dict[str, Any]:
    result = command(["osascript", "-e",
                      f'tell application "System Events" to tell (first process whose unix id is {pid})\n'
                      'set frontmost to true\nkeystroke "e" using {command down}\nend tell'],
                     timeout=10)
    return {"attempted": True, "succeeded": result.returncode == 0,
            "stderr": result.stderr.strip()}


def prepare_diagram(helper: Path, kind: str, window: dict[str, Any], *, preview: bool) -> dict[str, Any]:
    result: dict[str, Any] = {}
    if kind == "obsidian" and preview:
        result["preview_toggle"] = toggle_obsidian_preview(window["pid"])
    time.sleep(2)
    prepared = command([helper, "--prepare", str(window["pid"]),
                        *(["--allow-mermaid"] if kind == "obsidian" else [])], timeout=60)
    result.update({"returncode": prepared.returncode, "stdout": prepared.stdout,
                   "stderr": prepared.stderr})
    try:
        result.update(json.loads(prepared.stdout))
    except json.JSONDecodeError:
        result["diagram_ready"] = False
    time.sleep(2)
    return result


def launch_series(kind: str, app: Path, vault: Path, note: str, state_root: Path,
                  helper: Path | None, repetitions: int, *, cold: bool,
                  launch_timeout: float = 30, output: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for index in range(1, repetitions + 1):
        purge = purge_caches() if cold else {"requested": False, "performed": False}
        state = state_root / kind
        process, row = launch(kind, app, vault, note, state, helper, timeout=launch_timeout)
        row.update({"iteration": index, "mode": "cold" if cold else "hot", "purge": purge})
        rows.append(row)
        quit_app(kind, process=process, window=row.get("window"))
        write_json(output / f"{kind}-{Path(note).stem}-{'cold' if cold else 'hot'}-{index}.json", row)
        print(f"{kind} {note} {'cold' if cold else 'hot'} {index}/{repetitions}: "
              f"{row['elapsed_ms']:.1f} ms, title_match={row['title_match']}", flush=True)
    return rows


def switch_series(kind: str, app: Path, vault: Path, state_root: Path, helper: Path,
                  repetitions: int, launch_timeout: float, output: Path) -> list[dict[str, Any]]:
    rows = []
    for index in range(1, repetitions + 1):
        process, initial = launch(kind, app, vault, "Start here.md", state_root / kind,
                                  helper, timeout=launch_timeout)
        row: dict[str, Any] = {"iteration": index, "initial_launch": initial,
                               "network_samples": initial.get("network_samples", []), "title_match": False}
        if initial["found"]:
            time.sleep(1)
            located = command([helper, "--locate", str(process.pid), "1 MiB"], timeout=60)
            try:
                target = json.loads(located.stdout)
            except json.JSONDecodeError:
                target = {"found": False, "stderr": located.stderr}
            row["location"] = target
            if target["found"]:
                point = target["target"]
                started = time.perf_counter()
                clicked = command([helper, "--click", str(point["x"]), str(point["y"])])
                row.update(wait_for_window(helper, OWNER_NAMES[kind], ["1 MiB", "1 MiB.md"],
                                           launch_timeout, process))
                row.update({"elapsed_ms": (time.perf_counter() - started) * 1000,
                            "click_returncode": clicked.returncode,
                            "start_event": "external click helper invocation after locating the sidebar note"})
        quit_app(kind, process)
        rows.append(row)
        write_json(output / f"{kind}-large-note-switch-{index}.json", row)
        print(f"{kind} 1 MiB switch {index}/{repetitions}: {row.get('elapsed_ms')} ms, "
              f"title_match={row['title_match']}", flush=True)
    return rows


def runtime_series(kind: str, app: Path, vault: Path, note: str, state_root: Path,
                   helper: Path | None, output: Path, repetitions: int, seconds: int,
                   launch_timeout: float = 30) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for index in range(1, repetitions + 1):
        process, launched = launch(kind, app, vault, note, state_root / kind, helper,
                                   timeout=launch_timeout)
        if not launched["found"]:
            rows.append({"iteration": index, "launch": launched, "error": "window not found"})
            quit_app(kind, process=process, window=launched.get("window"))
            continue
        prepared = prepare_diagram(helper, kind, launched["window"], preview=kind == "obsidian" and index == 1)
        if not prepared.get("diagram_ready"):
            capture_window(launched["window"], output / f"{kind}-invalid-diagram-{index}.png")
            quit_app(kind, process)
            write_json(output / f"{kind}-runtime-{index}.json", {"launch": launched, "preparation": prepared})
            raise RuntimeError(f"{kind} diagram was not visibly rendered; see runtime evidence")
        evidence = capture_window(launched["window"], output / f"{kind}-diagram-{index}.png")
        table_before, ps_error_before = process_table()
        pids_before = descendants(table_before, [process.pid])
        cpu_before = cpu_seconds(table_before, pids_before)
        started = time.monotonic()
        cpu_samples = [{"elapsed_seconds": 0, "table": {pid: table_before[pid] for pid in pids_before}}]
        while time.monotonic() - started < seconds:
            remaining = seconds - (time.monotonic() - started)
            if remaining > 0:
                time.sleep(min(1, remaining))
            table_now, error = process_table()
            pids_now = descendants(table_now, [process.pid])
            cpu_samples.append({"elapsed_seconds": time.monotonic() - started,
                                "table": {pid: table_now[pid] for pid in pids_now}, "error": error})
        wall_seconds = time.monotonic() - started
        table_after, ps_error_after = process_table()
        pids_after = descendants(table_after, [process.pid])
        cpu_after = cpu_seconds(table_after, pids_after)
        footprint, memory_method, footprint_errors = footprint_bytes(pids_after, output / f"{kind}-footprint-{index}")
        memory = footprint
        observed_pids = set(pid for sample in cpu_samples for pid in sample["table"])
        lost_pids = observed_pids - set(pids_after)
        ps_errors = [error for error in (ps_error_before, ps_error_after) if error]
        ps_errors += [sample["error"] for sample in cpu_samples if sample.get("error")]
        valid_cpu = bool(pids_before and pids_after) and not lost_pids and not ps_errors
        row = {
            "iteration": index,
            "launch": launched,
            "diagram_preparation": prepared,
            "diagram_evidence": evidence,
            "measurement_seconds": seconds,
            "wall_seconds": wall_seconds,
            "pids_before": pids_before,
            "pids_after": pids_after,
            "process_count": len(pids_after) if pids_after and not ps_error_after else None,
            "memory_bytes": memory,
            "memory_method": memory_method,
            "rss_bytes_diagnostic": rss_bytes(table_after, pids_after),
            "footprint_errors": footprint_errors,
            "cpu_start_seconds": cpu_before,
            "cpu_end_seconds": cpu_after,
            "cpu_delta_seconds": cpu_after - cpu_before if valid_cpu else None,
            "cpu_percentage": (cpu_after - cpu_before) / wall_seconds * 100 if valid_cpu else None,
            "cpu_samples": cpu_samples,
            "lost_pids": sorted(lost_pids),
            "ps_errors": ps_errors,
            "network_samples": launched["network_samples"],
        }
        rows.append(row)
        quit_app(kind, process=process, window=launched.get("window"))
        write_json(output / f"{kind}-runtime-{index}.json", row)
        print(f"{kind} runtime {index}/{repetitions}: footprint={memory}, processes={len(pids_after)}, "
              f"CPU={row['cpu_delta_seconds']} s", flush=True)
    return rows


def load_check(name: str) -> Any:
    spec = spec_from_file_location(name.replace("-", "_"), ROOT / f"tools/{name}.py")
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def percentile95(values: list[float]) -> float:
    return sorted(values)[int((len(values) - 1) * .95)]


@contextmanager
def monitor_hortus_processes():
    original = subprocess.Popen
    monitored = []

    def start(arguments, *args, **kwargs):
        process = original(arguments, *args, **kwargs)
        if isinstance(arguments, (tuple, list)) and str(arguments[0]) == str(DEFAULT_APP / "Contents/MacOS/Hortus"):
            OWNED_PROCESSES.append(process)
            process.network_monitor = NetworkMonitor(process)
            monitored.append(process)
        return process

    try:
        with patch.object(subprocess, "Popen", start):
            yield monitored
    finally:
        for process in monitored:
            quit_app("hortus", process)


def hortus_metrics(output: Path, vault: Path, state: Path, repetitions: int, skip: bool) -> dict[str, Any]:
    if skip:
        return {"skipped": True, "reason": "--skip-hortus-metrics"}
    metrics_root = output / "hortus-metrics"
    metrics_root.mkdir(parents=True, exist_ok=True)
    diagram = load_check("check-diagram-view")
    reading = load_check("check-reading-view")
    editing = load_check("check-edit-and-save")
    rows = []
    samples = []
    original = (vault / "1 MiB.md").read_bytes()
    for index in range(1, repetitions + 1):
        run_state = state / f"metrics-{index}"
        with monitor_hortus_processes() as monitored, (metrics_root / f"run-{index}.stdout.txt").open("w") as log, redirect_stdout(log):
            measured, _ = diagram.inspect(run_state / "diagram", metrics_root, f"diagram-{index}",
                                          vault, "Learning/Learning map.md", ["--diagram-resize"])
            (vault / "1 MiB.md").write_bytes(original)
            refreshed = reading.run(run_state / "reading", metrics_root, f"external-{index}",
                                    vault, "1 MiB.md", "external")
            (vault / "note.md").write_bytes(original)
            keyed = editing.run(run_state / "editing", metrics_root, f"keys-{index}", vault, "styled-performance")
        samples.extend(sample for process in monitored for sample in process.network_monitor.samples)
        assert len(measured["resize"]) == 60 and measured["visible_nodes"] == 81, measured
        assert refreshed["rebuilds"] == 1 and refreshed["same_anchor"] and refreshed["anchor_shift_pt"] <= 1, refreshed
        assert keyed["saved"] and keyed["core_matches_native"] and keyed["utf8_bytes"] >= 1024**2, keyed
        rows.append({"width_max_px": measured["width_max_px"], "text_spans": measured["spans"],
                     "resize_p95_ms": percentile95([row["total_ms"] for row in measured["resize"]]),
                     "external_refresh_ms": refreshed["watch_to_display_ms"],
                     "external_after_debounce_ms": refreshed["after_debounce_ms"],
                     "one_mib_key_p95_ms": keyed["key_p95_ms"]})
        print(f"Hortus own metrics {index}/{repetitions} complete", flush=True)
    result = {"runs": rows, "medians": {key: summarize(rows, key) for key in rows[0]}, "network_samples": samples}
    result["width_error_max_px"] = max(row["width_max_px"] for row in rows)
    write_json(metrics_root / "summary.json", result)
    return result


def cli_diagram_metrics(output: Path, vault: Path, repetitions: int) -> dict[str, Any]:
    note = vault / "Learning/Learning map.md"
    result: dict[str, Any] = {"note": str(note), "widths": {}}
    for width in (700, 1100):
        rows = []
        for index in range(1, repetitions + 1):
            svg = output / f"learning-map-{width}-{index}.svg"
            metrics = output / f"learning-map-{width}-{index}.json"
            metrics.unlink(missing_ok=True)
            run = command([str(ROOT / "target/release/hortus"), "mermaid", "render", str(note),
                           "--width", str(width), "--out", str(svg), "--metrics", str(metrics)], timeout=120)
            item: dict[str, Any] = {"returncode": run.returncode, "stdout": run.stdout.strip(),
                                    "stderr": run.stderr.strip()}
            if run.returncode == 0 and metrics.exists():
                item.update(json.loads(metrics.read_text()))
            rows.append(item)
        result["widths"][str(width)] = rows
    return result


def summarize(rows: list[dict[str, Any]], field: str) -> dict[str, Any]:
    values = [row[field] for row in rows if isinstance(row.get(field), (int, float))
              and (field != "elapsed_ms" or row.get("title_match") is True)]
    return {"samples": values, "median": median(values), "count": len(values), "attempts": len(rows)}


def validation_errors(summary: dict[str, Any], repetitions: int, *, skip_runtime: bool,
                      skip_hortus_metrics: bool) -> list[str]:
    errors = []
    for name, measured in summary["medians"].items():
        if measured["count"] != repetitions:
            errors.append(f"{name}: {measured['count']}/{repetitions} valid measurements")
    for kind in OWNER_NAMES:
        if not summary["screenshots"][kind]["captured"]:
            errors.append(f"{kind}: no verified diagram screenshot")
        if not summary["network"][kind]["all_available"]:
            errors.append(f"{kind}: network sampling incomplete")
        if not skip_runtime and len(summary["runtime"].get(kind, [])) != repetitions:
            errors.append(f"{kind}: runtime measurements incomplete")
    if summary["network"]["hortus"]["connections"]:
        errors.append("Hortus had observed network connections")
    if not skip_hortus_metrics and len(summary["hortus_metrics"].get("runs", [])) != repetitions:
        errors.append("Hortus own measurements incomplete")
    for width, rows in summary["mermaid"]["widths"].items():
        if len(rows) != repetitions or any(row["returncode"] != 0 for row in rows):
            errors.append(f"CLI diagram width {width}: incomplete measurements")
    if not summary["source_unchanged"]:
        errors.append("source vault changed")
    if not summary["temporary_directory_removed_on_exit"]:
        errors.append("temporary directory remains")
    return errors


def inventory(root: Path) -> dict[str, str]:
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.rglob("*") if path.is_file()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=DEFAULT_APP)
    parser.add_argument("--obsidian", type=Path, default=DEFAULT_OBSIDIAN)
    parser.add_argument("--vault", type=Path, default=DEFAULT_VAULT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--promo-output", type=Path, default=ROOT / "target/promo/P1-15")
    parser.add_argument("--frozen-commit", help="Known frozen release commit; omit for a preflight run")
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--cpu-seconds", type=int, default=30)
    parser.add_argument("--launch-timeout", type=float, default=30)
    parser.add_argument("--skip-hortus-metrics", action="store_true")
    parser.add_argument("--skip-runtime", action="store_true")
    args = parser.parse_args()
    if args.app.resolve() != DEFAULT_APP.resolve() and not args.skip_hortus_metrics:
        parser.error("existing Hortus regression helpers use build/Hortus.app; use --skip-hortus-metrics for another bundle")
    revision = command(["git", "rev-parse", "HEAD"]).stdout.strip()
    if args.frozen_commit:
        frozen = command(["git", "rev-parse", args.frozen_commit]).stdout.strip()
        if frozen != revision:
            parser.error("--frozen-commit must identify the checked-out release source")
    if args.repetitions < 1 or args.cpu_seconds < 1 or args.launch_timeout <= 0:
        parser.error("--repetitions, --cpu-seconds and --launch-timeout must be positive")
    for path in (args.app, args.obsidian, args.vault):
        if not path.exists():
            parser.error(f"missing path: {path}")

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source_hashes = inventory(args.vault)
    with ExitStack() as stack:
        temporary = stack.enter_context(tempfile.TemporaryDirectory(prefix="hortus-obsidian-measure-"))
        stack.callback(cleanup_processes)
        temporary_root = Path(temporary)
        helper = compile_window_helper(temporary_root)
        if helper is None:
            shutil.copyfile(temporary_root / "window_probe.stderr", output / "window_probe.stderr")
            raise RuntimeError(f"window helper could not compile; see {output / 'window_probe.stderr'}")
        _, error = process_table()
        if error:
            raise RuntimeError(f"process inspection unavailable: {error}")
        hortus_vault = temporary_root / "hortus-vault"
        obsidian_vault = temporary_root / "obsidian-vault"
        copy_vault(args.vault, hortus_vault)
        copy_vault(args.vault, obsidian_vault)
        large_hortus = make_large_note(hortus_vault)
        large_obsidian = make_large_note(obsidian_vault)
        generated = {"hortus": large_hortus, "obsidian": large_obsidian}
        state_root = temporary_root / "state"
        state_root.mkdir()

        metadata = {
            "stage": "frozen-release" if args.frozen_commit else "preflight",
            "source_revision": revision,
            "timestamp_local": time.strftime("%Y-%m-%d %H:%M:%S %z"),
            "machine": command(["system_profiler", "SPHardwareDataType"], timeout=30).stdout,
            "system": command(["sw_vers"], timeout=30).stdout,
            "architecture": command(["uname", "-m"], timeout=10).stdout.strip(),
            "hortus": app_info(args.app),
            "hortus_executable_sha256": hashlib.sha256((args.app / "Contents/MacOS/Hortus").read_bytes()).hexdigest(),
            "obsidian": app_info(args.obsidian),
            "install_size": {kind: [disk_usage(app) for _ in range(args.repetitions)]
                             for kind, app in (("hortus", args.app), ("obsidian", args.obsidian))},
            "vault_source": str(args.vault),
            "generated_notes": generated,
            "measurement": {
                "repetitions": args.repetitions,
                "cpu_seconds": args.cpu_seconds,
                "launch_start": "immediately before the target app executable command",
                "launch_end": "first on-screen layer-0 window owned by the launched PID with the exact note title or Obsidian's title suffix; dialogs never count",
                "cold": "no target process remains; `purge` is attempted without sudo and its result is recorded",
                "hot": "no target process remains, but OS file and framework caches are left intact",
                "large_note": "in a running app on Start here, locate the sidebar note before timing; time from external click helper invocation to the exact 1 MiB note window title",
                "memory": f"sum of physical footprint for all processes in the launched process tree after {args.cpu_seconds} s idle; no RSS substitution",
                "cpu": "sum of ps user+system CPU time for the same process tree over the measurement interval",
                "polling": "50 ms sleep between CGWindowList helper invocations; polling and helper startup overhead is included for both apps",
                "obsidian_settings": "disposable --user-data-dir profile and copied vault, default settings, no third-party plugins; reading view and Mermaid permission selected with real keyboard/mouse events",
            },
        }
        write_json(output / "metadata.json", metadata)
        launch_results: dict[str, Any] = {}
        launch_results["hortus_cold"] = launch_series("hortus", args.app, hortus_vault, "Start here.md",
                                                        state_root, helper, args.repetitions, cold=True,
                                                        launch_timeout=args.launch_timeout, output=output)
        launch_results["hortus_hot"] = launch_series("hortus", args.app, hortus_vault, "Start here.md",
                                                       state_root, helper, args.repetitions, cold=False,
                                                       launch_timeout=args.launch_timeout, output=output)
        launch_results["obsidian_cold"] = launch_series("obsidian", args.obsidian, obsidian_vault, "Start here.md",
                                                         state_root, helper, args.repetitions, cold=True,
                                                         launch_timeout=args.launch_timeout, output=output)
        launch_results["obsidian_hot"] = launch_series("obsidian", args.obsidian, obsidian_vault, "Start here.md",
                                                        state_root, helper, args.repetitions, cold=False,
                                                        launch_timeout=args.launch_timeout, output=output)
        large_results: dict[str, Any] = {}
        for kind, app, vault in (("hortus", args.app, hortus_vault), ("obsidian", args.obsidian, obsidian_vault)):
            large_results[kind] = switch_series(kind, app, vault, state_root, helper,
                                                args.repetitions, args.launch_timeout, output)

        runtime_results: dict[str, Any] = {}
        screenshots: dict[str, Any] = {}
        for kind, app, vault in (("hortus", args.app, hortus_vault), ("obsidian", args.obsidian, obsidian_vault)):
            if not args.skip_runtime:
                runtime_results[kind] = runtime_series(kind, app, vault, "Learning/Learning map.md",
                                                       state_root, helper, output, args.repetitions, args.cpu_seconds,
                                                       launch_timeout=args.launch_timeout)
            # One screenshot per application is retained as non-committed promo evidence.
            process, launch_row = launch(kind, app, vault, "Learning/Learning map.md",
                                         state_root / kind, helper,
                                         timeout=args.launch_timeout)
            if launch_row["found"]:
                launch_row["diagram_preparation"] = prepare_diagram(helper, kind, launch_row["window"],
                                                                    preview=kind == "obsidian" and args.skip_runtime)
            ready = launch_row.get("diagram_preparation", {}).get("diagram_ready")
            screenshots[kind] = capture_window(launch_row.get("window") if ready else None,
                                               args.promo_output / f"{kind}-large-diagram.png")
            screenshots[kind]["launch"] = launch_row
            quit_app(kind, process=process, window=launch_row.get("window"))

        summary: dict[str, Any] = {
            "metadata": metadata,
            "launch": launch_results,
            "large_note": large_results,
            "runtime": runtime_results,
            "screenshots": screenshots,
            "mermaid": cli_diagram_metrics(output, args.vault, args.repetitions),
            "hortus_metrics": hortus_metrics(output, hortus_vault, state_root, args.repetitions, args.skip_hortus_metrics),
            "source_unchanged": inventory(args.vault) == source_hashes,
            "source_files": len(source_hashes),
            "temporary_directory_removed_on_exit": True,
        }
        for category, groups in (("launch", launch_results), ("large_note", large_results)):
            for name, rows in groups.items():
                summary.setdefault("medians", {})[f"{category}_{name}"] = summarize(rows, "elapsed_ms")
        for kind, rows in metadata["install_size"].items():
            summary.setdefault("medians", {})[f"{kind}_install_bytes"] = summarize(rows, "du_bytes")
        for kind, rows in runtime_results.items():
            summary.setdefault("medians", {})[f"{kind}_memory_bytes"] = summarize(rows, "memory_bytes")
            summary.setdefault("medians", {})[f"{kind}_process_count"] = summarize(rows, "process_count")
            summary.setdefault("medians", {})[f"{kind}_cpu_delta_seconds"] = summarize(rows, "cpu_delta_seconds")
            summary.setdefault("medians", {})[f"{kind}_cpu_percentage"] = summarize(rows, "cpu_percentage")
        for kind in OWNER_NAMES:
            samples = [sample for groups in (launch_results, large_results) for name, rows in groups.items()
                       if name.startswith(kind) for row in rows for sample in row.get("network_samples", [])]
            samples += [sample for row in runtime_results.get(kind, []) for sample in row.get("network_samples", [])]
            samples += screenshots[kind]["launch"].get("network_samples", [])
            if kind == "hortus":
                samples += summary["hortus_metrics"].get("network_samples", [])
            summary.setdefault("network", {})[kind] = {
                "sample_count": len(samples),
                "connections": sorted({line for sample in samples for line in sample["connections"]}),
                "errors": sorted({error for sample in samples for error in sample["errors"]}),
                "all_available": bool(samples) and all(sample["available"] for sample in samples),
            }
        write_json(output / "comparison.json", summary)
    summary["temporary_directory_removed_on_exit"] = not temporary_root.exists()
    summary["validation_errors"] = validation_errors(summary, args.repetitions,
                                                      skip_runtime=args.skip_runtime,
                                                      skip_hortus_metrics=args.skip_hortus_metrics)
    write_json(output / "comparison.json", summary)
    print(json.dumps({"output": str(output / "comparison.json"),
                      "source_unchanged": summary["source_unchanged"],
                      "repetitions": args.repetitions,
                      "runtime_skipped": args.skip_runtime,
                      "validation_errors": summary["validation_errors"]}, ensure_ascii=False, indent=2))
    return 1 if summary["validation_errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
