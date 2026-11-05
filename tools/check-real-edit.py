#!/usr/bin/env python3
"""P1-39 real-edit regression with an external disk observer.

The app sends only physical keycode events through the HID event tap. After
each named phase it waits for this script to read the public vault and write an
ack. Text entry always goes through SCIM ITABC; no direct text API is used.
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
import uuid


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "build/Hortus.app/Contents/MacOS/Hortus"
NOTE_NAME = "note.md"
NOTE_TEXT = "# 示例笔记\n\n公开的真实编辑回归夹具。\n"
SCIM_ID = "com.apple.inputmethod.SCIM.ITABC"
COMMITTED_INITIAL = "中" + NOTE_TEXT
FIRST_PREEDIT = "ni hao"
FIRST_COMMITTED = "中" + FIRST_PREEDIT + NOTE_TEXT
SECOND_PREEDIT = "hao"
SECOND_COMMITTED = SECOND_PREEDIT + FIRST_COMMITTED
ALPHA_NAME = "alpha.md"
BETA_NAME = "beta.md"
ALPHA_TEXT = "# 公开夹具 A\n\n标签路径的公开正文。\n"
BETA_TEXT = "# 公开夹具 B\n\n搜索结果专用词 searchb。\n"
ALPHA_PREEDIT = "ni hao" + ALPHA_TEXT
BETA_COMMITTED = "中" + BETA_TEXT
SWITCH_MODES = {"composition-tab-switch", "composition-search-switch"}
SESSION = '''last_vault = "{vault}"

[[recent]]
path = "{vault}"
tabs = ["{note}"]
selected = "{note}"

[recent.views]
"{note}" = "editing"

[recent.visible]
bottom = true
left = true

[[windows]]
id = "legacy"
path = "{vault}"
tabs = ["{note}"]
selected = "{note}"

[windows.views]
"{note}" = "editing"

[windows.visible]
bottom = true
left = true

[default_visible]
bottom = true
left = true
'''
SWITCH_SESSION = '''last_vault = "{vault}"

[[recent]]
path = "{vault}"
tabs = ["{alpha}", "{beta}"]
selected = "{alpha}"

[recent.views]
"{alpha}" = "editing"
"{beta}" = "editing"

[recent.visible]
bottom = false
left = true

[[windows]]
id = "p1-39"
path = "{vault}"
tabs = ["{alpha}", "{beta}"]
selected = "{alpha}"

[windows.views]
"{alpha}" = "editing"
"{beta}" = "editing"

[windows.visible]
bottom = false
left = true

[default_visible]
bottom = false
left = true
'''


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")


def composition_expectation(phase):
    values = {
        "before_composition": (COMMITTED_INITIAL, COMMITTED_INITIAL, 1),
        "composition_active": (FIRST_COMMITTED, COMMITTED_INITIAL, 7),
        "command_s_while_composing": (FIRST_COMMITTED, COMMITTED_INITIAL, 7),
        "command_e_while_composing": (FIRST_COMMITTED, COMMITTED_INITIAL, 7),
        "focus_left": (FIRST_COMMITTED, COMMITTED_INITIAL, 7),
        "autosave_after_focus_loss": (FIRST_COMMITTED, FIRST_COMMITTED, 7),
        "second_editor_focused": (FIRST_COMMITTED, FIRST_COMMITTED, 0),
        "second_composition_active": (SECOND_COMMITTED, FIRST_COMMITTED, 3),
        "second_focus_left": (SECOND_COMMITTED, FIRST_COMMITTED, 3),
        "command_s_after_focus_loss": (SECOND_COMMITTED, SECOND_COMMITTED, 3),
        "command_e_after_focus_loss": (SECOND_COMMITTED, SECOND_COMMITTED, 3),
    }
    value = values.get(phase)
    if value is None:
        return None
    view_text, disk_text, selection = value
    return {
        "view_text": view_text,
        "view_bytes": len(view_text.encode("utf-8")),
        "disk_text": disk_text,
        "disk_bytes": len(disk_text.encode("utf-8")),
        "selection_location": selection,
    }


def switch_expectation(mode, phase):
    alpha_disk = ALPHA_TEXT
    alpha_text = ALPHA_TEXT
    beta_disk = BETA_TEXT
    beta_text = BETA_TEXT
    current = ALPHA_NAME
    selection = 0
    if phase == "composition_active":
        alpha_text = ALPHA_PREEDIT
        selection = len("ni hao")
    elif phase == "after_switch":
        alpha_text = ALPHA_PREEDIT
        current = BETA_NAME
    elif phase == "after_autosave":
        alpha_text = ALPHA_PREEDIT
        alpha_disk = ALPHA_PREEDIT
        current = BETA_NAME
    elif phase == "after_beta_input":
        alpha_text = ALPHA_PREEDIT
        alpha_disk = ALPHA_PREEDIT
        beta_text = BETA_COMMITTED
        beta_disk = BETA_COMMITTED
        current = BETA_NAME
        selection = 1
    elif phase == "switched_back":
        alpha_text = ALPHA_PREEDIT
        alpha_disk = ALPHA_PREEDIT
        beta_text = BETA_COMMITTED
        beta_disk = BETA_COMMITTED
        selection = 0
    elif phase == "alpha_refocused_input":
        alpha_text = "中" + ALPHA_PREEDIT
        alpha_disk = ALPHA_PREEDIT
        beta_text = BETA_COMMITTED
        beta_disk = BETA_COMMITTED
        selection = 1
    elif phase in {"command_s_after_return", "command_e_after_return"}:
        alpha_text = "中" + ALPHA_PREEDIT
        alpha_disk = "中" + ALPHA_PREEDIT
        beta_text = BETA_COMMITTED
        beta_disk = BETA_COMMITTED
    return {
        "mode": mode,
        "phase": phase,
        "current": current,
        "selection_location": selection,
        "alpha": {"text": alpha_text, "disk": alpha_disk},
        "beta": {"text": beta_text, "disk": beta_disk},
        "after_switch_alpha_disk_choices": [ALPHA_TEXT, ALPHA_PREEDIT] if phase == "after_switch" else None,
    }


def initialise_run(base, switch=False):
    vault = base / "vault"
    vault.mkdir(parents=True)
    if switch:
        (vault / ALPHA_NAME).write_text(ALPHA_TEXT, encoding="utf-8")
        (vault / BETA_NAME).write_text(BETA_TEXT, encoding="utf-8")
        (base / "pristine-alpha.md").write_text(ALPHA_TEXT, encoding="utf-8")
        (base / "pristine-beta.md").write_text(BETA_TEXT, encoding="utf-8")
    else:
        (vault / NOTE_NAME).write_text(NOTE_TEXT, encoding="utf-8")
        (base / "pristine-note.md").write_text(NOTE_TEXT, encoding="utf-8")
    config = base / "config"
    config.mkdir()
    (config / "config.toml").write_text('font_size = 15.0\ndefault_view = "editing"\n', encoding="utf-8")
    cache = base / "cache"
    cache.mkdir()
    session = SWITCH_SESSION.format(vault=vault, alpha=ALPHA_NAME, beta=BETA_NAME) if switch else SESSION.format(vault=vault, note=NOTE_NAME)
    (cache / "session.toml").write_text(session, encoding="utf-8")
    return vault, config, cache


def disk_observation(vault):
    note = vault / NOTE_NAME
    raw = note.read_bytes()
    attributes = note.stat()
    temporary = sorted(
        str(path.relative_to(vault))
        for path in vault.rglob("*")
        if path != note
    )
    return {
        "path_name": NOTE_NAME,
        "bytes": len(raw),
        "mtime_ns": attributes.st_mtime_ns,
        "sha256": sha256(raw),
        "text": raw.decode("utf-8"),
        "temporary_entries": temporary,
    }


def switch_disk_observation(vault):
    declared = {ALPHA_NAME, BETA_NAME}
    notes = {}
    for name in sorted(declared):
        path = vault / name
        raw = path.read_bytes()
        attributes = path.stat()
        notes[name] = {
            "bytes": len(raw), "mtime_ns": attributes.st_mtime_ns,
            "sha256": sha256(raw), "text": raw.decode("utf-8"),
        }
    unexpected = sorted(str(path.relative_to(vault)) for path in vault.rglob("*") if str(path.relative_to(vault)) not in declared)
    return {"notes": notes, "unexpected_entries": unexpected}


def read_phase_lines(path):
    if not path.exists():
        return []
    values = []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            break
        if isinstance(value, dict):
            values.append(value)
    return values


def acknowledge_phase(run_dir, ack_dir, phase, observation, mode):
    sequence = phase["sequence"]
    ack = {
        "run_id": phase["run_id"],
        "sequence": sequence,
        "phase": phase["phase"],
        "observed": observation,
    }
    target = ack_dir / f"{sequence:03d}.json"
    temporary = ack_dir / f".{sequence:03d}.tmp"
    write_json(temporary, ack)
    os.replace(temporary, target)
    record = {
        "sequence": sequence,
        "phase": phase["phase"],
        "state": phase.get("state", {}),
        "disk": observation,
        "expectation": switch_expectation(mode, phase["phase"]) if mode in SWITCH_MODES else composition_expectation(phase["phase"]),
        "ack_wall_time": time.time(),
    }
    with (run_dir / "observations.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")
    return record


def run_app(app, mode, output, label):
    run_dir = output / label
    if run_dir.exists():
        shutil.rmtree(run_dir)
    run_dir.mkdir(parents=True)
    base = Path(tempfile.mkdtemp(prefix=f"hortus-{mode}-"))
    switch = mode in SWITCH_MODES
    vault, config, cache = initialise_run(base, switch=switch)
    result_path = run_dir / "result.json"
    phase_log = run_dir / "phases.jsonl"
    ack_dir = run_dir / "acks"
    ack_dir.mkdir()
    observe_disk = switch_disk_observation if switch else disk_observation
    initial = observe_disk(vault)
    write_json(run_dir / "initial-disk.json", initial)
    run_id = str(uuid.uuid4())
    environment = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache))
    command = [
        str(app.resolve()), "--light",
        f"--edit-inspection={result_path}",
        f"--edit-mode={mode}",
        f"--edit-run-id={run_id}",
        f"--edit-phase-log={phase_log}",
        f"--edit-ack-dir={ack_dir}",
    ]
    application_log = run_dir / "application.log"
    observations = []
    seen = 0
    timed_out = False
    with application_log.open("wb") as log_handle:
        process = subprocess.Popen(command, env=environment, stdout=log_handle, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 90
        while True:
            phases = read_phase_lines(phase_log)
            for phase in phases:
                if phase.get("kind") != "phase":
                    continue
                sequence = phase.get("sequence")
                if not isinstance(sequence, int) or sequence <= seen:
                    continue
                if sequence != seen + 1:
                    raise RuntimeError(f"phase sequence gap: expected {seen + 1}, got {sequence}")
                observations.append(acknowledge_phase(run_dir, ack_dir, phase, observe_disk(vault), mode))
                seen = sequence
            if process.poll() is not None:
                break
            if time.monotonic() >= deadline:
                timed_out = True
                process.terminate()
                break
            time.sleep(0.025)
        try:
            returncode = process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            returncode = process.wait(timeout=10)
    # Drain phases written immediately before process exit.
    for phase in read_phase_lines(phase_log):
        sequence = phase.get("sequence")
        if phase.get("kind") == "phase" and isinstance(sequence, int) and sequence > seen:
            observations.append(acknowledge_phase(run_dir, ack_dir, phase, observe_disk(vault), mode))
            seen = sequence
    final_disk = observe_disk(vault)
    write_json(run_dir / "final-disk.json", final_disk)
    result = None
    result_error = None
    if result_path.exists():
        try:
            result = json.loads(result_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            result_error = str(error)
    shutil.rmtree(base, ignore_errors=True)
    return {
        "label": label,
        "mode": mode,
        "run_id": run_id,
        "returncode": returncode,
        "timed_out": timed_out,
        "result": result,
        "result_error": result_error,
        "initial": initial,
        "final": final_disk,
        "observations": observations,
        "application_log": str(application_log),
    }


def observation_map(run):
    return {item["phase"]: item for item in run["observations"]}


def require(condition, message, errors):
    if not condition:
        errors.append(message)


def require_prerequisites(run, errors):
    result = run["result"] or {}
    require(run["returncode"] == 0, f"app exited {run['returncode']}", errors)
    require(not run["timed_out"], "app timed out", errors)
    require(run["result_error"] is None, f"result error: {run['result_error']}", errors)
    require(result.get("status") == "completed", f"status={result.get('status')}", errors)
    prerequisites = result.get("prerequisites", {})
    for key in ("event_access", "application_active", "activation_converged", "key_window"):
        require(prerequisites.get(key) is True, f"prerequisite {key}=false", errors)
    require(prerequisites.get("input_source_available") is True, "SCIM ITABC unavailable", errors)
    require(prerequisites.get("input_source_selected") is True, "SCIM ITABC was not selected", errors)
    require(result.get("input_source_after_select") == SCIM_ID, "input source identity mismatch", errors)
    require(result.get("input_source_restored") is True, "original input source was not restored", errors)
    require(result.get("input_source_after_run") == prerequisites.get("input_source_before"),
            "input source restore identity mismatch", errors)
    require(result.get("event_tap") == "cghidEventTap", "events were not sent through cghidEventTap", errors)
    require(result.get("focus_event_tap") == "cghidEventTap", "focus click was not sent through cghidEventTap", errors)
    if result.get("mode") == "undo-matrix":
        require(result.get("search_input_source_selected") is True,
                "ABC input source could not be selected for the native search field", errors)


def require_common_disk(run, errors):
    for item in run["observations"]:
        if "notes" in item["disk"]:
            require(item["disk"].get("unexpected_entries") == [],
                    f"unexpected vault entries after {item['phase']}: {item['disk'].get('unexpected_entries')}", errors)
            for name, note in item["disk"]["notes"].items():
                require(note["bytes"] == len(note["text"].encode("utf-8")),
                        f"{name} byte count mismatch after {item['phase']}", errors)
            continue
        require(item["disk"]["temporary_entries"] == [],
                f"temporary vault entries after {item['phase']}: {item['disk']['temporary_entries']}", errors)
        require(item["disk"]["bytes"] == len(item["disk"]["text"].encode("utf-8")),
                f"byte count mismatch after {item['phase']}", errors)


def event_is(state, keycode, flags):
    event = state.get("last_event", {})
    return event.get("keycode") == keycode and event.get("flags") == flags and event.get("tap") == "cghidEventTap"


def menu_matches(state, errors, label):
    expected_undo = bool(state.get("current_editable")) and not bool(state.get("current_marked_text")) and bool(state.get("can_undo"))
    expected_redo = bool(state.get("current_editable")) and not bool(state.get("current_marked_text")) and bool(state.get("can_redo"))
    require(state.get("expected_menu_undo") is expected_undo, f"{label}: app undo expectation disagrees", errors)
    require(state.get("expected_menu_redo") is expected_redo, f"{label}: app redo expectation disagrees", errors)
    require(state.get("menu_undo_enabled") is expected_undo,
            f"{label}: Undo enabled={state.get('menu_undo_enabled')} expected={expected_undo}", errors)
    require(state.get("menu_redo_enabled") is expected_redo,
            f"{label}: Redo enabled={state.get('menu_redo_enabled')} expected={expected_redo}", errors)


def composition_text_matches(record, errors, label):
    expectation = record.get("expectation")
    if expectation is None:
        errors.append(f"{label}: missing expected text model")
        return
    state = record["state"]
    view_text = state.get("view_text")
    disk_text = record["disk"]["text"]
    require(view_text == expectation["view_text"],
            f"{label}: visible body mismatch; expected {expectation['view_bytes']} B "
            f"{expectation['view_text']!r}, got {len(view_text.encode('utf-8')) if isinstance(view_text, str) else -1} B "
            f"{view_text!r}", errors)
    require(disk_text == expectation["disk_text"],
            f"{label}: disk body mismatch; expected {expectation['disk_bytes']} B "
            f"{expectation['disk_text']!r}, got {record['disk']['bytes']} B {disk_text!r}", errors)
    selection = state.get("selection") or {}
    require(selection.get("location") == expectation["selection_location"],
            f"{label}: selection={selection.get('location')} expected={expectation['selection_location']}", errors)


def revision_change(first, second, expected, label, errors):
    before = first["state"].get("revision")
    after = second["state"].get("revision")
    require(isinstance(before, int) and isinstance(after, int) and after - before == expected,
            f"{label}: revision delta expected {expected}, got {before}->{after}", errors)


def evaluate_composition(run):
    errors = []
    require_prerequisites(run, errors)
    require_common_disk(run, errors)
    phases = observation_map(run)
    names = ["before_composition", "composition_active", "command_s_while_composing",
             "command_e_while_composing", "focus_left", "autosave_after_focus_loss",
             "second_editor_focused", "second_composition_active", "second_focus_left",
             "command_s_after_focus_loss", "command_e_after_focus_loss"]
    require(sorted(phases) == sorted(names), f"phase set mismatch: {sorted(phases)}", errors)
    if not set(names).issubset(phases):
        return {"passed": False, "errors": errors}
    before = phases["before_composition"]
    active = phases["composition_active"]
    command_s = phases["command_s_while_composing"]
    command_e = phases["command_e_while_composing"]
    focus_left = phases["focus_left"]
    autosave = phases["autosave_after_focus_loss"]
    second_focus = phases["second_editor_focused"]
    second_active = phases["second_composition_active"]
    second_focus_left = phases["second_focus_left"]
    saved_s = phases["command_s_after_focus_loss"]
    saved_e = phases["command_e_after_focus_loss"]
    for record in (before, active, command_s, command_e, focus_left, autosave,
                   second_focus, second_active, second_focus_left, saved_s, saved_e):
        composition_text_matches(record, errors, record["phase"])
    require(before["state"].get("marked_text") is False and before["state"].get("dirty") is False,
            "real IME commit did not settle", errors)
    require(before["disk"]["sha256"] != run["initial"]["sha256"], "real IME commit was not saved externally", errors)
    require(active["disk"]["sha256"] == before["disk"]["sha256"], "active preedit reached disk", errors)
    require(active["state"].get("marked_text") is True and active["state"].get("marked") is True,
            "SCIM did not produce a marked composition", errors)
    require(active["state"].get("marked_range") == {"location": 1, "length": len(FIRST_PREEDIT)},
            f"first marked range={active['state'].get('marked_range')} expected=1..<7", errors)
    revision_change(before, active, len("nihao"), "first preedit", errors)
    for label, record, keycode in (("composition ⌘S", command_s, 1), ("composition ⌘E", command_e, 14)):
        require(record["disk"]["sha256"] == before["disk"]["sha256"], f"{label} changed disk", errors)
        require(record["state"].get("marked_text") is True, f"{label} committed the preedit", errors)
        require(record["state"].get("reading") is False, f"{label} switched views", errors)
        require(event_is(record["state"], keycode, ["command"]), f"{label} event is not the real command keycode", errors)
        require(record["state"].get("revision") == active["state"].get("revision"),
                f"{label} changed the document revision", errors)
    menu_matches(command_s["state"], errors, "composition ⌘S")
    menu_matches(command_e["state"], errors, "composition ⌘E")
    require(focus_left["state"].get("first_responder") != "DiagramTextView",
            "real terminal click did not leave the editor", errors)
    require(autosave["disk"]["sha256"] != before["disk"]["sha256"], "focus-loss autosave did not reach disk", errors)
    require(autosave["state"].get("marked_text") is False and autosave["state"].get("marked") is False,
            "focus-loss autosave left a marked composition", errors)
    require(autosave["state"].get("dirty") is False, "focus-loss autosave did not settle", errors)
    require(autosave["state"].get("input_context_id") != autosave["state"].get("current_input_context_id"),
            "focus-loss evidence did not leave the editor input context", errors)
    revision_change(active, focus_left, 2, "first preedit commit", errors)
    require(autosave["state"].get("revision") == focus_left["state"].get("revision"),
            "first autosave changed the document revision", errors)
    require(second_focus["state"].get("first_responder") == "DiagramTextView",
            "second composition did not refocus the editor", errors)
    require(second_focus["state"].get("input_context_current") is True,
            "second composition started without the editor input context", errors)
    require(second_focus["state"].get("marked_text") is False and second_focus["state"].get("marked") is False,
            "second composition started with stale marked text", errors)
    require(second_focus["state"].get("revision") == autosave["state"].get("revision"),
            "refocusing the editor inserted stale preedit text", errors)
    require(second_active["state"].get("first_responder") == "DiagramTextView",
            "second composition lost editor focus", errors)
    require(second_active["state"].get("input_context_current") is True,
            "second composition lost its input context", errors)
    require(second_active["state"].get("marked_text") is True and second_active["state"].get("marked") is True,
            "second SCIM composition did not start", errors)
    require(second_active["state"].get("marked_range") == {"location": 0, "length": len(SECOND_PREEDIT)},
            f"second marked range={second_active['state'].get('marked_range')} expected=0..<3", errors)
    revision_change(second_focus, second_active, len("hao"), "second preedit", errors)
    require(second_focus_left["state"].get("first_responder") != "DiagramTextView",
            "second composition did not leave the editor", errors)
    require(second_focus_left["state"].get("marked_text") is False
            and second_focus_left["state"].get("marked") is False,
            "second focus loss left a marked composition", errors)
    revision_change(second_active, second_focus_left, 2, "second preedit commit", errors)
    require(saved_s["disk"]["sha256"] != autosave["disk"]["sha256"], "post-focus real ⌘S did not save an edit", errors)
    require(saved_s["state"].get("dirty") is False, "post-focus ⌘S did not settle", errors)
    require(saved_s["state"].get("revision") == second_focus_left["state"].get("revision"),
            "post-focus ⌘S changed the document revision", errors)
    require(event_is(saved_s["state"], 1, ["command"]), "post-focus save event is not real ⌘S", errors)
    require(saved_e["state"].get("reading") is True, "post-focus real ⌘E did not switch to reading", errors)
    require(saved_e["disk"]["sha256"] == saved_s["disk"]["sha256"], "post-focus ⌘E changed disk", errors)
    require(event_is(saved_e["state"], 14, ["command"]), "post-focus toggle event is not real ⌘E", errors)
    require(saved_e["state"].get("revision") == saved_s["state"].get("revision"),
            "post-focus ⌘E changed the document revision", errors)
    return {"passed": not errors, "errors": errors}


def evaluate_undo(run):
    errors = []
    require_prerequisites(run, errors)
    require_common_disk(run, errors)
    phases = observation_map(run)
    names = ["commit_ready", "after_undo", "after_redo", "composition_undo_ready", "after_composition_undo"]
    require(sorted(phases) == sorted(names), f"phase set mismatch: {sorted(phases)}", errors)
    if not set(names).issubset(phases):
        return {"passed": False, "errors": errors}
    commit = phases["commit_ready"]
    undo = phases["after_undo"]
    redo = phases["after_redo"]
    composition = phases["composition_undo_ready"]
    composition_undo = phases["after_composition_undo"]
    require(commit["disk"]["sha256"] != run["initial"]["sha256"], "real IME commit did not reach disk", errors)
    require(commit["disk"]["text"] == commit["state"].get("view_text"), "commit disk differs from editor", errors)
    require(commit["state"].get("note_can_undo") is True, "canUndo prerequisite is false", errors)
    menu_matches(commit["state"], errors, "editor commit")
    require(event_is(undo["state"], 6, ["command"]), "undo event is not real ⌘Z", errors)
    require(undo["disk"]["sha256"] != commit["disk"]["sha256"], "real ⌘Z did not change disk", errors)
    require(undo["disk"]["text"] == undo["state"].get("view_text"), "undo disk differs from editor", errors)
    require(undo["state"].get("dirty") is False and undo["state"].get("note_can_redo") is True,
            "undo did not settle with redo available", errors)
    require(undo["state"].get("revision") != commit["state"].get("revision"), "revision did not follow undo", errors)
    menu_matches(undo["state"], errors, "after undo")
    require(event_is(redo["state"], 6, ["command", "shift"]), "redo event is not real ⇧⌘Z", errors)
    require(redo["disk"]["sha256"] == commit["disk"]["sha256"], "real ⇧⌘Z did not restore disk body", errors)
    require(redo["disk"]["text"] == redo["state"].get("view_text"), "redo disk differs from editor", errors)
    require(redo["state"].get("revision") != undo["state"].get("revision"), "revision did not follow redo", errors)
    require(redo["state"].get("dirty") is False, "redo did not settle", errors)
    menu_matches(redo["state"], errors, "after redo")
    require(composition["state"].get("marked_text") is True and composition["state"].get("note_can_undo") is True,
            "composition undo prerequisite lacks canUndo", errors)
    menu_matches(composition["state"], errors, "composition before undo")
    require(event_is(composition_undo["state"], 6, ["command"]), "composition undo is not real ⌘Z", errors)
    require(composition_undo["disk"]["sha256"] == redo["disk"]["sha256"], "composition ⌘Z changed disk", errors)
    require(composition_undo["state"].get("view_text") == composition["state"].get("view_text"),
            "composition ⌘Z changed visible text", errors)
    require(composition_undo["state"].get("revision") == composition["state"].get("revision"),
            "composition ⌘Z changed document revision", errors)
    require(composition_undo["state"].get("marked_text") is True, "composition ⌘Z ended the preedit", errors)
    menu_matches(composition_undo["state"], errors, "composition after undo")
    return {"passed": not errors, "errors": errors}


def evaluate_matrix(run):
    errors = []
    require_prerequisites(run, errors)
    require_common_disk(run, errors)
    phases = observation_map(run)
    names = ["editor", "reading", "after_reading_undo", "terminal", "after_terminal_undo",
             "search_typed", "after_search_undo", "after_search_redo"]
    require(sorted(phases) == sorted(names), f"phase set mismatch: {sorted(phases)}", errors)
    if not set(names).issubset(phases):
        return {"passed": False, "errors": errors}
    editor = phases["editor"]
    reading = phases["reading"]
    reading_undo = phases["after_reading_undo"]
    terminal = phases["terminal"]
    terminal_undo = phases["after_terminal_undo"]
    search = phases["search_typed"]
    search_undo = phases["after_search_undo"]
    search_redo = phases["after_search_redo"]
    require(editor["state"].get("first_responder") == "DiagramTextView", "matrix editor lacks responder", errors)
    require(editor["state"].get("note_can_undo") is True, "matrix editor canUndo=false", errors)
    menu_matches(editor["state"], errors, "matrix editor")
    require(reading["state"].get("first_responder") == "DiagramTextView" and reading["state"].get("reading") is True,
            "matrix did not enter reading view", errors)
    menu_matches(reading["state"], errors, "matrix reading")
    require(reading_undo["disk"]["sha256"] == reading["disk"]["sha256"], "reading ⌘Z changed disk", errors)
    require(reading_undo["state"].get("view_text") == reading["state"].get("view_text"),
            "reading ⌘Z changed body", errors)
    menu_matches(reading_undo["state"], errors, "matrix reading after ⌘Z")
    require("HortusTerminalView" in terminal["state"].get("first_responder", ""),
            "matrix terminal focus missing", errors)
    menu_matches(terminal["state"], errors, "matrix terminal")
    require(terminal_undo["disk"]["sha256"] == terminal["disk"]["sha256"], "terminal ⌘Z changed note disk", errors)
    require(terminal_undo["state"].get("view_text") == terminal["state"].get("view_text"),
            "terminal ⌘Z changed note body", errors)
    menu_matches(terminal_undo["state"], errors, "matrix terminal after ⌘Z")
    require(search["state"].get("search_editor_active") is True, "search field did not become the field editor", errors)
    require(search["state"].get("current_marked_text") is False, "search field input was not committed", errors)
    search_text = search["state"].get("search_query")
    require(bool(search_text), "real IME input did not reach search field", errors)
    require(search["state"].get("can_undo") is True, "search field undo stack is empty", errors)
    require(search["state"].get("undo_manager_id") != editor["state"].get("undo_manager_id"),
            "search field did not use its own undo manager", errors)
    menu_matches(search["state"], errors, "search field")
    require(search_undo["state"].get("search_query") != search_text, "search ⌘Z did not change its text", errors)
    require(search_undo["state"].get("can_redo") is True, "search undo did not retain redo", errors)
    menu_matches(search_undo["state"], errors, "search after undo")
    require(search_redo["state"].get("search_query") == search_text, "search ⇧⌘Z did not restore its text", errors)
    menu_matches(search_redo["state"], errors, "search after redo")
    for record in (reading, reading_undo, terminal, terminal_undo, search, search_undo, search_redo):
        require(record["disk"]["sha256"] == editor["disk"]["sha256"],
                f"{record['phase']} changed note disk", errors)
    return {"passed": not errors, "errors": errors}


def switch_notes(record):
    return record["disk"].get("notes", {})


def switch_document(record, name):
    return (record["state"].get("documents") or {}).get(name, {})


def evaluate_switch(run, mode):
    errors = []
    result = run["result"] or {}
    require(run["returncode"] == 0, f"app exited {run['returncode']}", errors)
    require(not run["timed_out"], "app timed out", errors)
    require(run["result_error"] is None, f"result error: {run['result_error']}", errors)
    require(result.get("status") == "completed", f"status={result.get('status')}", errors)
    prerequisites = result.get("prerequisites", {})
    for key in ("event_access", "application_active", "activation_converged", "key_window",
                "alpha_loaded", "beta_loaded", "alpha_selected", "editor_focused",
                "composition_started", "switch_preconditions"):
        require(prerequisites.get(key) is True, f"prerequisite {key}=false", errors)
    require(prerequisites.get("input_source_available") is True, "SCIM ITABC unavailable", errors)
    require(prerequisites.get("input_source_selected") is True, "SCIM ITABC was not selected", errors)
    require(result.get("input_source_after_select") == SCIM_ID, "input source identity mismatch", errors)
    require(result.get("input_source_restored") is True, "original input source was not restored", errors)
    require(result.get("input_source_after_run") == prerequisites.get("input_source_before"),
            "input source restore identity mismatch", errors)
    require(result.get("event_tap") == "cghidEventTap", "events were not sent through cghidEventTap", errors)
    require(result.get("focus_event_tap") == "cghidEventTap", "focus click was not sent through cghidEventTap", errors)
    if mode == "composition-tab-switch":
        require((result.get("tab_target") or {}).get("path") == BETA_NAME,
                "toolbar target was not the beta tab", errors)
    else:
        require(prerequisites.get("search_input_source_selected") is True,
                "ABC input source could not be selected for the real search entry", errors)
        require(prerequisites.get("search_panel") is True, "real search shortcut did not open the panel", errors)
        require(prerequisites.get("search_field_focused") is True, "real search field click did not focus it", errors)
        require(prerequisites.get("single_beta_result") is True, "real query did not resolve only to beta", errors)
        require(prerequisites.get("beta_result_visible") is True, "beta result row was not visible", errors)
        require(prerequisites.get("search_not_refocused") is True, "search field regained focus during composition", errors)
        require(result.get("search_results") == [BETA_NAME], "search result path evidence mismatch", errors)
        require((result.get("search_target") or {}).get("role") == "AXButton",
                "search click target was not an accessibility button", errors)
        require("searchb" in (result.get("search_target") or {}).get("label", "").lower(),
                "visible search result target did not identify the unique beta match", errors)

    require_common_disk(run, errors)
    phases = observation_map(run)
    names = ["before_composition", "composition_active", "after_switch", "after_autosave",
             "after_beta_input", "switched_back", "alpha_refocused_input",
             "command_s_after_return", "command_e_after_return"]
    require(sorted(phases) == sorted(names), f"phase set mismatch: {sorted(phases)}", errors)
    if not set(names).issubset(phases):
        return {"passed": False, "errors": errors}
    before = phases["before_composition"]
    active = phases["composition_active"]
    switched = phases["after_switch"]
    autosaved = phases["after_autosave"]
    beta_input = phases["after_beta_input"]
    returned = phases["switched_back"]
    alpha_input = phases["alpha_refocused_input"]
    saved = phases["command_s_after_return"]
    reading = phases["command_e_after_return"]

    for record in (before, active, switched, autosaved, beta_input, returned, alpha_input, saved, reading):
        phase = record["phase"]
        notes = switch_notes(record)
        require(set(notes) == {ALPHA_NAME, BETA_NAME}, f"{phase}: declared notes missing", errors)
        expectation = switch_expectation(mode, phase)
        allowed_alpha = expectation.get("after_switch_alpha_disk_choices")
        if allowed_alpha is not None:
            alpha_disk = notes.get(ALPHA_NAME, {}).get("text")
            require(alpha_disk in allowed_alpha, f"{phase}: unexpected alpha disk transition {alpha_disk!r}", errors)
        else:
            require(notes.get(ALPHA_NAME, {}).get("text") == expectation["alpha"]["disk"],
                    f"{phase}: alpha disk mismatch; expected {expectation['alpha']['disk']!r}, got {notes.get(ALPHA_NAME, {}).get('text')!r}", errors)
        require(notes.get(BETA_NAME, {}).get("text") == expectation["beta"]["disk"],
                f"{phase}: beta disk mismatch; expected {expectation['beta']['disk']!r}, got {notes.get(BETA_NAME, {}).get('text')!r}", errors)
        require(record["state"].get("selected_path") == expectation["current"],
                f"{phase}: selected={record['state'].get('selected_path')} expected={expectation['current']}", errors)

    before_alpha = switch_document(before, ALPHA_NAME)
    active_alpha = switch_document(active, ALPHA_NAME)
    require(before_alpha.get("marked") is False and before_alpha.get("dirty") is False,
            "before composition: alpha did not start clean", errors)
    require((before.get("state").get("documents") or {}).get(ALPHA_NAME, {}).get("view_present") is True,
            "before composition: alpha editor was not visible", errors)
    require(active.get("state").get("first_responder") == "DiagramTextView",
            "composition: editor was not first responder", errors)
    require(active_alpha.get("marked") is True and active_alpha.get("view_present") is True,
            "composition: alpha document/view was not marked", errors)
    active_view = active.get("state").get("tracked_view") or {}
    require(active_view.get("text") == ALPHA_PREEDIT and active_view.get("marked_text") is True,
            "composition: visible preedit did not remain in alpha", errors)
    require(active.get("state").get("marked_range") == {"location": 0, "length": len("ni hao")},
            f"composition: marked range={active.get('state').get('marked_range')}", errors)
    if mode == "composition-search-switch":
        require(active.get("state").get("search_editor_active") is False,
                "composition: search field became the active field", errors)

    switched_alpha = switch_document(switched, ALPHA_NAME)
    switched_beta = switch_document(switched, BETA_NAME)
    require(switched_alpha.get("marked") is False, "after switch: alpha remained marked", errors)
    require(switched.get("state").get("first_responder") == "DiagramTextView" and
            switched_beta.get("view_present") is True, "after switch: beta editor view missing", errors)
    require(switched.get("state").get("view_text") == BETA_TEXT, "after switch: beta view contains foreign text", errors)
    require((switched.get("state").get("tracked_view") or {}).get("path") == BETA_NAME,
            "after switch: editor was not rebound to beta", errors)

    autosave_alpha = switch_document(autosaved, ALPHA_NAME)
    require(autosave_alpha.get("marked") is False and autosave_alpha.get("dirty") is False,
            "autosave: alpha did not settle without new input", errors)
    seconds = result.get("alpha_autosave_seconds")
    require(isinstance(seconds, (int, float)) and seconds <= 2.0,
            f"autosave: alpha settled after {seconds}s, limit 2s", errors)
    require(switch_notes(autosaved)[ALPHA_NAME]["text"] == ALPHA_PREEDIT,
            "autosave: alpha expected body was not written", errors)

    beta_document = switch_document(beta_input, BETA_NAME)
    require(beta_input.get("state").get("view_text") == BETA_COMMITTED,
            "beta input: visible body mismatch", errors)
    require(beta_document.get("marked") is False and beta_document.get("dirty") is False,
            "beta input: edit did not settle", errors)
    require("ni hao" not in beta_input.get("state").get("view_text", ""),
            "beta input: alpha preedit leaked into beta view", errors)
    require("ni hao" not in switch_notes(beta_input)[BETA_NAME]["text"],
            "beta input: alpha preedit leaked into beta disk", errors)

    require(returned.get("state").get("view_text") == ALPHA_PREEDIT,
            "return to alpha: expected body missing", errors)
    require(switch_document(returned, ALPHA_NAME).get("marked") is False,
            "return to alpha: marked state remained", errors)
    require(result.get("alpha_return_input_settled") is True,
            "return to alpha: real IME input did not settle", errors)
    require(alpha_input.get("state").get("view_text") == "中" + ALPHA_PREEDIT,
            "return to alpha: real input body mismatch", errors)
    require(switch_notes(alpha_input)[ALPHA_NAME]["text"] == ALPHA_PREEDIT,
            "return to alpha: unsaved input reached disk before ⌘S", errors)
    require(event_is(saved["state"], 1, ["command"]), "returned alpha save was not real ⌘S", errors)
    require(saved["state"].get("dirty") is False, "real ⌘S did not settle alpha", errors)
    require(switch_notes(saved)[ALPHA_NAME]["text"] == "中" + ALPHA_PREEDIT,
            "real ⌘S did not write the returned alpha body", errors)
    require(event_is(reading["state"], 14, ["command"]), "returned alpha switch was not real ⌘E", errors)
    require(reading["state"].get("reading") is True, "real ⌘E did not enter reading view", errors)
    require(reading["state"].get("marked") is False, "reading switch retained marked state", errors)
    return {"passed": not errors, "errors": errors}


def evaluate_run(mode, run):
    if mode == "composition-focus-loss":
        return evaluate_composition(run)
    if mode == "undo-redo":
        return evaluate_undo(run)
    if mode in SWITCH_MODES:
        return evaluate_switch(run, mode)
    return evaluate_matrix(run)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/real-edit")
    parser.add_argument("--app", type=Path, default=APP)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--scenario", choices=["all", "composition-focus-loss", "undo-redo", "undo-matrix",
                                               "composition-tab-switch", "composition-search-switch"],
                        default="all")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    selected = ["composition-focus-loss", "undo-redo", "undo-matrix",
                "composition-tab-switch", "composition-search-switch"] if args.scenario == "all" else [args.scenario]
    summary = {"status": "passed", "scenarios": {}, "runs": args.runs, "app": str(args.app.resolve())}
    for mode in selected:
        runs = []
        count = args.runs if mode != "undo-matrix" else 1
        for index in range(1, count + 1):
            run = run_app(args.app, mode, output / mode, f"run-{index}")
            verdict = evaluate_run(mode, run)
            run["verdict"] = verdict
            runs.append(run)
            write_json(output / mode / f"run-{index}" / "verdict.json", {
                "passed": verdict["passed"],
                "errors": verdict["errors"],
                "result": run["result"],
            })
            print(f"{mode} run {index}: {'PASS' if verdict['passed'] else 'FAIL'}", flush=True)
            for error in verdict["errors"]:
                print(f"  {error}", flush=True)
        passed = all(run["verdict"]["passed"] for run in runs)
        summary["scenarios"][mode] = {
            "passed": passed,
            "runs_passed": sum(run["verdict"]["passed"] for run in runs),
            "runs_total": len(runs),
            "errors": [{"run": index + 1, "errors": run["verdict"]["errors"]}
                       for index, run in enumerate(runs) if not run["verdict"]["passed"]],
        }
        if not passed:
            summary["status"] = "failed"
    write_json(output / "summary.json", summary)
    print(json.dumps(summary, indent=2, ensure_ascii=False), flush=True)
    if summary["status"] != "passed":
        raise SystemExit(1)
    print("真实编辑回归：全部通过", flush=True)


if __name__ == "__main__":
    main()
