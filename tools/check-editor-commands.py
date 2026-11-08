#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""P1-46 editor command routing regression.

Keyboard and mouse input goes through physical keycodes at the HID event tap.
Focus and menus are read through Accessibility from a separate process; note
contents come from disk reads in this script. The app-side inspection file is
auxiliary evidence only and never decides a verdict.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
PROBE_SOURCE = ROOT / 'tools/editor-command-probe.swift'
SCIM = 'com.apple.inputmethod.SCIM.ITABC'
ABC = 'com.apple.keylayout.ABC'

SESSION = '''last_vault = "{vault}"

[[recent]]
path = "{vault}"
tabs = {tabs}
selected = "{selected}"

[recent.views]
{views}

[recent.visible]
bottom = true
left = true

[[windows]]
id = "legacy"
path = "{vault}"
tabs = {tabs}
selected = "{selected}"

[windows.views]
{views}

[windows.visible]
bottom = true
left = true

[default_visible]
bottom = true
left = true
'''


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name('.' + path.name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + '\n',
                          encoding='utf-8')
    os.replace(temporary, path)


def read_json(path):
    try:
        return json.loads(path.read_text(encoding='utf-8'))
    except (OSError, json.JSONDecodeError):
        return {}


def disk(vault, name):
    path = vault / name
    raw = path.read_bytes()
    return {
        'path': name,
        'text': raw.decode('utf-8'),
        'bytes': len(raw),
        'sha256': digest(raw),
        'mtime_ns': path.stat().st_mtime_ns,
        'vault_entries': sorted(str(entry.relative_to(vault)) for entry in vault.rglob('*')),
    }


def wait_until(predicate, timeout=10, interval=0.05):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    return predicate()


class Probe:
    def __init__(self, path):
        self.path = path

    def run(self, *arguments, timeout=40):
        result = subprocess.run([str(self.path), *map(str, arguments)], capture_output=True, text=True,
                                timeout=timeout)
        if result.returncode != 0:
            raise RuntimeError(f'probe {arguments}: {result.stderr.strip()}')
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError:
            return {'stdout': result.stdout, 'stderr': result.stderr}

    def key(self, code, *modifiers):
        return self.run('key', code, *modifiers)

    def type(self, text):
        return self.run('type', text)

    def click(self, x, y):
        return self.run('click', x, y)

    def ax(self, pid):
        return self.run('ax', pid)

    def tree(self, pid):
        return self.run('tree', pid, timeout=60)

    def screenshot(self, pid, path):
        return self.run('screenshot', pid, path)

    def press_menu(self, pid, menu, item):
        return self.run('press-menu', pid, menu, item)

    def activate(self, pid):
        return self.run('activate', pid)

    def wait_window(self, pid, timeout=15):
        return self.run('wait-window', pid, timeout, timeout=timeout + 5)

    def input_source(self, identifier=None):
        if identifier is None:
            return self.run('input-source', 'get')['input_source']
        return self.run('input-source', identifier)


def menu_names(pid):
    script = ('tell application "System Events" to tell process "Hortus" to get name of every '
              'menu bar item of menu bar 1')
    result = subprocess.run(['osascript', '-e', script], capture_output=True, text=True, timeout=20)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip())
    return [name.strip() for name in result.stdout.strip().split(',')]


def menu_items(pid, title):
    script = (f'tell application "System Events" to tell process "Hortus" to get name of every '
              f'menu item of menu 1 of menu bar item "{title}" of menu bar 1')
    result = subprocess.run(['osascript', '-e', script], capture_output=True, text=True, timeout=20)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip())
    return [name.strip() for name in result.stdout.strip().split(',')]


def frontmost():
    script = 'tell application "System Events" to get name of first process whose frontmost is true'
    return subprocess.run(['osascript', '-e', script], capture_output=True, text=True, timeout=20).stdout.strip()


def note_area(snapshot):
    """The note editor exposes the text area with the smallest top coordinate."""
    areas = [area for area in snapshot.get('text_areas', []) if area.get('role') == 'AXTextArea']
    if not areas:
        return None
    return min(areas, key=lambda area: area.get('position', {}).get('y', 100000))


def terminal_area(snapshot):
    areas = [area for area in snapshot.get('text_areas', []) if area.get('role') == 'AXTextArea']
    if len(areas) < 2:
        return None
    return max(areas, key=lambda area: area.get('position', {}).get('y', 0))


def focused_role(snapshot):
    focused = snapshot.get('focused', {})
    return f"{focused.get('role', '')}/{focused.get('subrole', '')}"


def editor_focused(value):
    focused = value.get('focused')
    role = focused_role(value) if isinstance(focused, dict) else (focused or '')
    return role.startswith('AXTextArea')


def prepare(base, notes, view='editing', windows=1):
    if isinstance(notes, dict):
        notes = [notes]
    vaults = []
    for index in range(windows):
        vault = base / f'vault-{index + 1}'
        vault.mkdir(parents=True)
        for name, text in notes[index].items():
            (vault / name).write_text(text, encoding='utf-8')
        vaults.append(vault)
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text(f'font_size = 15.0\ndefault_view = "{view}"\n', encoding='utf-8')
    cache = base / 'cache'
    cache.mkdir(parents=True, exist_ok=True)
    blocks = []
    for index, vault in enumerate(vaults):
        tabs = ', '.join(f'"{name}"' for name in notes[index])
        selected = next(iter(notes[index]))
        views = '\n'.join(f'"{name}" = "{view}"' for name in notes[index])
        if len(vaults) == 1:
            cache.joinpath('session.toml').write_text(
                SESSION.format(vault=vault, tabs=f'[{tabs}]', selected=selected, views=views),
                encoding='utf-8')
            return vaults, config, cache
        blocks.append((vault, tabs, selected, views))
    text = ''
    for index, (vault, tabs, selected, views) in enumerate(blocks):
        entry = f'''[[windows]]
id = "window-{index + 1}"
path = "{vault}"
tabs = [{tabs}]
selected = "{selected}"

[windows.views]
{views}

'''
        text += entry
    text += '\n[default_visible]\nbottom = true\nleft = true\n'
    (cache / 'session.toml').write_text(text, encoding='utf-8')
    return vaults, config, cache


class Run:
    def __init__(self, name, base, probe, app, notes, view='editing', windows=1, inspection=True):
        self.name = name
        self.base = base
        self.probe = probe
        self.app = app
        self.notes = notes
        self.vaults, self.config, self.cache = prepare(base, notes, view=view, windows=windows)
        self.vault = self.vaults[0]
        if MEDIA and OUTPUT:
            self.media = MEDIA / base.relative_to(OUTPUT)
        else:
            self.media = base
        if MEDIA:
            self.media.mkdir(parents=True, exist_ok=True)
        self.inspection = base / 'inspection.json' if inspection else None
        self.log = base / 'application.log'
        self.process = None
        self.pid = None

    def start(self):
        environment = dict(os.environ, HORTUS_CONFIG_DIR=str(self.config), HORTUS_CACHE_DIR=str(self.cache))
        arguments = [str(self.app), '--light']
        if self.inspection:
            arguments.append(f'--editor-command-inspection={self.inspection}')
        self.log.parent.mkdir(parents=True, exist_ok=True)
        self.handle = self.log.open('wb')
        self.process = subprocess.Popen(arguments, env=environment, stdout=self.handle, stderr=subprocess.STDOUT)
        self.pid = self.process.pid
        window = self.probe.wait_window(self.pid, 20).get('window') or {}
        if window.get('width', 0) < 100:
            raise RuntimeError('window did not appear')
        self.window = window
        self.probe.activate(self.pid)
        for _ in range(40):
            if frontmost() == 'Hortus':
                break
            time.sleep(0.1)
        self.probe.activate(self.pid)
        return window

    def state(self):
        return read_json(self.inspection) if self.inspection else {}

    def stop(self, timeout=10):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if getattr(self, 'handle', None):
            self.handle.close()

    def snapshot(self):
        return self.probe.ax(self.pid)

    def disk(self, name):
        return disk(self.vault, name)

    def click_note(self, x=700, y=400):
        self.probe.click(x, y)

    def click_terminal(self):
        tree = self.probe.tree(self.pid)
        for element in tree.get('elements', []):
            if element.get('description') == '终端' and element.get('role') == 'AXGroup':
                position = element['position']
                size = element['size']
                self.probe.click(position['x'] + size['width'] / 2, position['y'] + size['height'] / 2)
                return True
        return False

    def current_input(self):
        return self.state().get('first_responder_input_source', '')

    def ensure_input(self, target):
        """Toggle the focused app input source until the requested source is active."""
        current = self.current_input()
        if current == '':
            return current
        for _ in range(5):
            if current == target:
                return current
            self.probe.key(49, 'ctrl')
            time.sleep(0.6)
            current = self.current_input()
        return current

    def focus_editor(self, attempts=20):
        for _ in range(attempts):
            area = note_area(self.snapshot())
            if area and area.get('position') and area.get('size'):
                position = area['position']
                self.probe.click(position['x'] + 60, position['y'] + 60)
                time.sleep(0.3)
                if editor_focused(self.snapshot()):
                    return True
            time.sleep(0.2)
        return False

    def note_text(self):
        area = note_area(self.snapshot())
        return (area or {}).get('value_prefix', '')

    def note_length(self):
        area = note_area(self.snapshot())
        return (area or {}).get('value_length', -1)

    def screenshot(self, path):
        target = self.media / Path(path).name
        try:
            self.probe.screenshot(self.pid, str(target))
            return str(target)
        except Exception as error:
            return f'failed: {error}' 


def observe(run, label, disk_name, extra=None):
    value = {
        'label': label,
        'focused': focused_role(run.snapshot()),
        'disk': run.disk(disk_name),
        'state': run.state(),
    }
    if extra:
        value.update(extra)
    return value


def scenario_a1(base, probe, app):
    """⌘E with terminal focus, immediate SCIM input, empty composition, save shortcuts."""
    notes = {'note.md': '# Note\n\nAlpha and Alpha here.\n\nTail.\n'}
    run = Run('a1', base, probe, app, {'note.md': notes['note.md']}, view='reading')
    result = {'scenario': 'A1', 'runs': []}
    try:
        run.start()
        run.click_terminal()
        time.sleep(0.5)
        before = observe(run, 'terminal_focused', 'note.md')
        run.probe.key(14, 'cmd')
        time.sleep(0.4)
        after_toggle = observe(run, 'after_command_e', 'note.md')
        run.ensure_input(SCIM)
        run.probe.type('nihao')
        time.sleep(0.35)
        composing = observe(run, 'composing', 'note.md')
        run.probe.key(49)
        time.sleep(2.2)
        committed = observe(run, 'committed', 'note.md')
        run.probe.key(1, 'cmd')
        time.sleep(0.7)
        saved = observe(run, 'after_command_s', 'note.md')
        run.probe.key(14, 'cmd')
        time.sleep(0.9)
        toggled = observe(run, 'after_command_e_again', 'note.md')
        run.screenshot(base / 'a1-final.png')

        def document(observation):
            return next((row for row in observation['state'].get('documents', [])
                         if row.get('path') == 'note.md'), {})

        state_after_toggle = document(after_toggle)
        state_composing = document(composing)
        state_committed = document(committed)

        def reading_mode(observation):
            value = document(observation).get('reading')
            if value is None:
                area = note_area(run.snapshot()) or {}
                value = '阅读' in (area.get('description') or '')
            return value

        errors = []
        if not editor_focused(after_toggle):
            errors.append(f"editor not focused after ⌘E: {after_toggle['focused']}")
        if reading_mode(after_toggle) is not False:
            errors.append('document did not enter editing mode')
        if not state_composing.get('marked'):
            errors.append('SCIM composition did not start')
        if '你好' not in committed['disk']['text']:
            errors.append('committed Chinese text missing from disk')
        if committed['disk']['text'].count('你好') != 1:
            errors.append('committed text written more than once')
        if state_committed.get('marked') or state_committed.get('dirty'):
            errors.append('composition did not settle for autosave')
        if '你好' not in saved['disk']['text']:
            errors.append('⌘S lost committed text')
        reading_again = reading_mode(toggled)
        if reading_again is not True:
            errors.append('second ⌘E did not return to reading')
        result['runs'].append({'observations': [before, after_toggle, composing, committed, saved, toggled],
                               'errors': errors, 'passed': not errors})
    finally:
        run.stop()
    result['passed'] = all(item['passed'] for item in result['runs'])
    return result


def scenario_a1_deterministic(base, probe, app):
    """Deterministic empty setMarkedText completion with save failure retry."""
    notes = {'note.md': '# Note\n\nBody.\n'}
    old = Run('a1-empty', base, probe, app, notes, view='editing', inspection=False)
    vault, config, cache = old.vaults[0], old.config, old.cache
    result_path = base / 'empty.json'
    environment = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache))
    log = base / 'empty.log'
    with log.open('wb') as handle:
        process = subprocess.Popen([str(app), '--light', f'--edit-inspection={result_path}',
                                    '--edit-mode=empty-composition'],
                                   env=environment, stdout=handle, stderr=subprocess.STDOUT)
        try:
            process.wait(timeout=90)
        except subprocess.TimeoutExpired:
            process.kill(); process.wait(timeout=5)
    value = read_json(result_path)
    keys = ['save_failed_first', 'marked_during_composition', 'empty_composition_cleared',
            'save_retry_succeeded', 'disk_matches']
    errors = [f'{key} is not true' for key in keys if value.get(key) is not True]
    return {'scenario': 'A1-deterministic', 'result': value, 'errors': errors, 'passed': not errors}


def scenario_a2(base, probe, app, terminal_visible):
    notes = {'note.md': '# Note\n\nAlpha and Alpha here.\n\nTail.\n'}
    run = Run('a2', base, probe, app, notes)
    result = {'scenario': f'A2-terminal-{"visible" if terminal_visible else "hidden"}', 'runs': []}
    try:
        run.start()
        if not terminal_visible:
            run.probe.key(38, 'cmd')  # ⌘J hides the bottom panel.
            time.sleep(0.7)
        if not run.focus_editor():
            raise RuntimeError('editor did not take focus')
        run.ensure_input(ABC)
        run.probe.key(3, 'cmd')
        time.sleep(0.7)
        run.probe.type('Alpha')
        time.sleep(0.3)
        run.probe.key(36)
        time.sleep(0.4)
        find_state = observe(run, 'find_open', 'note.md')
        match = note_area(run.snapshot()) or {}
        selected = match.get('selected_range') or [0, 0]
        text = run.disk('note.md')['text']
        if selected[1] != 5 or text[selected[0]:selected[0] + 5] != 'Alpha':
            find_state['errors'] = [f'find did not select the match: {selected}']
        run.probe.key(53)
        time.sleep(0.8)
        closed = observe(run, 'find_closed', 'note.md')
        terminal_before = terminal_area(run.snapshot())
        run.probe.type('Q')
        time.sleep(1.8)
        typed = observe(run, 'typed_after_close', 'note.md')
        terminal_after = terminal_area(run.snapshot())
        run.screenshot(base / f'a2-{"visible" if terminal_visible else "hidden"}.png')
        errors = list(find_state.pop('errors', []))
        if not editor_focused(closed):
            errors.append(f"focus did not return to editor: {closed['focused']}")
        if typed['disk']['text'].count('Q') != 1:
            errors.append('Q was not written to the note exactly once')
        if terminal_before and terminal_after and terminal_before.get('value_prefix') != terminal_after.get('value_prefix'):
            errors.append('terminal input buffer changed after find closed')
        result['runs'].append({'observations': [find_state, closed, typed], 'errors': errors,
                               'passed': not errors})
    finally:
        run.stop()
    result['passed'] = all(item['passed'] for item in result['runs'])
    return result


def replace_all(run, entry, screenshot):
    """Open replace through a real entry, replace Alpha with Gamma, save, undo, save."""
    errors = []
    if entry == 'shortcut':
        run.probe.key(3, 'cmd', 'opt')
    else:
        run.probe.press_menu(run.pid, '编辑', '查找并替换…')
    time.sleep(0.9)
    run.ensure_input(ABC)
    opened = observe(run, f'replace_open_{entry}', 'note.md')
    tree = run.probe.tree(run.pid)
    ax_fields = [element for element in tree.get('elements', [])
                 if element.get('role') == 'AXTextField' and element.get('position')]
    if len(ax_fields) < 2:
        errors.append(f'{entry} did not open the replace bar: {len(ax_fields)} fields')
    # The find bar owns two fields: search above, replacement below.
    tree = run.probe.tree(run.pid)
    fields = sorted((element for element in tree.get('elements', [])
                     if element.get('role') == 'AXTextField' and element.get('position')),
                    key=lambda element: element['position'].get('y', 0))
    if len(fields) < 2:
        errors.append('find bar fields not found')
        return errors, opened
    for field, text in ((fields[0], 'Alpha'), (fields[-1], 'Gamma')):
        position = field['position']
        size = field['size']
        run.probe.click(position['x'] + size['width'] / 2, position['y'] + size['height'] / 2)
        time.sleep(0.3)
        run.probe.key(0, 'cmd')
        time.sleep(0.2)
        run.probe.type(text)
        time.sleep(0.3)
    tree = run.probe.tree(run.pid)
    values = sorted((element.get('position', {}).get('y', 0), element.get('value_prefix'))
                    for element in tree.get('elements', []) if element.get('role') == 'AXTextField')
    if [value for _, value in values] != ['Alpha', 'Gamma']:
        errors.append(f'replace fields were not filled: {values}')
    all_button = next((element for element in tree.get('elements', [])
                       if element.get('role') == 'AXButton' and
                       (element.get('description') or '') == '全部'), None)
    if not all_button:
        errors.append('replace all button not found')
        return errors, opened
    position = all_button['position']
    size = all_button['size']
    run.probe.click(position['x'] + size['width'] / 2, position['y'] + size['height'] / 2)
    time.sleep(1.0)
    replaced = observe(run, f'replaced_{entry}', 'note.md')
    run.probe.key(1, 'cmd')
    time.sleep(0.7)
    saved = observe(run, f'replaced_saved_{entry}', 'note.md')
    run.probe.key(53)
    time.sleep(0.8)
    run.probe.key(6, 'cmd')
    time.sleep(0.6)
    run.probe.key(1, 'cmd')
    time.sleep(0.7)
    undone = observe(run, f'undone_{entry}', 'note.md')
    run.screenshot(screenshot)
    if replaced['disk']['text'].count('Gamma') != 2:
        errors.append(f'{entry} replace did not replace both matches')
    if 'Alpha' in replaced['disk']['text']:
        errors.append(f'{entry} replace left the source matches')
    if saved['disk']['text'].count('Gamma') != 2:
        errors.append(f'{entry} save lost replacements')
    if undone['disk']['text'].count('Alpha') != 2:
        errors.append(f'{entry} single undo did not restore all matches')
    if not editor_focused(observe(run, 'after_undo_focus', 'note.md')):
        errors.append(f'{entry} undo did not run on the note editor')
    return errors, [opened, replaced, saved, undone]


def scenario_a3(base, probe, app):
    notes = {'note.md': '# Note\n\nAlpha and Alpha here.\n\nTail.\n'}
    result = {'scenario': 'A3', 'runs': []}
    for entry in ('shortcut', 'menu'):
        run = Run(f'a3-{entry}', base / entry, probe, app, {'note.md': notes['note.md']})
        try:
            run.start()
            if not run.focus_editor():
                raise RuntimeError('editor did not take focus')
            errors, observations = replace_all(run, entry, base / f'a3-{entry}.png')
            result['runs'].append({'entry': entry, 'observations': observations,
                                   'errors': errors, 'passed': not errors})
        finally:
            run.stop()
    result['passed'] = all(item['passed'] for item in result['runs'])
    return result


def scenario_a4(base, probe, app):
    notes = {'note.md': '# Note\n\nAlpha and Alpha here.\n\n中文 测试\n'}
    result = {'scenario': 'A4', 'runs': []}
    for visible in (False, True):
        run = Run(f'a4-{visible}', base / str(visible), probe, app, {'note.md': notes['note.md']})
        try:
            run.start()
            wait_until(lambda: not (run.state().get('search') or {}).get('scanning', True), timeout=15)
            if visible:
                run.probe.key(3, 'shift', 'cmd')
                time.sleep(0.7)
            if not run.focus_editor():
                raise RuntimeError('editor did not take focus')
            run.probe.key(3, 'shift', 'cmd')
            time.sleep(0.9)
            opened = observe(run, 'search_opened', 'note.md')
            opened_focus = run.snapshot().get('focused') or {}
            run.ensure_input(SCIM)
            run.probe.type('zhong')
            time.sleep(0.5)
            composing = observe(run, 'search_composing', 'note.md')
            composing_focus = run.snapshot().get('focused') or {}
            run.probe.key(49)
            time.sleep(1.2)
            committed = observe(run, 'search_committed', 'note.md')
            committed_focus = run.snapshot().get('focused') or {}
            run.screenshot(base / f'a4-{"visible" if visible else "hidden"}.png')
            errors = []
            if opened_focus.get('role') != 'AXTextField':
                errors.append(f"⇧⌘F did not focus the search field: {opened_focus.get('role')}")
            if composing_focus.get('value_prefix') != 'zhong':
                errors.append(f"search field lost the preedit: {composing_focus.get('value_prefix')!r}")
            if committed_focus.get('value_prefix') != '中':
                errors.append(f"search field did not commit the candidate: {committed_focus.get('value_prefix')!r}")
            result_tree = run.probe.tree(run.pid)
            result_buttons = [element.get('description', '') for element in result_tree.get('elements', [])
                              if element.get('role') == 'AXButton' and '中文' in (element.get('description') or '')]
            if len(result_buttons) != 1:
                errors.append(f'search result count is not one: {len(result_buttons)}')
            # ASCII search must not regress: switch back to ABC, clear, query a word.
            run.ensure_input(ABC)
            run.probe.key(0, 'cmd')
            time.sleep(0.3)
            run.probe.type('Alpha')
            time.sleep(1.2)
            ascii_observation = observe(run, 'ascii_search', 'note.md')
            ascii_focus = run.snapshot().get('focused') or {}
            ascii_tree = run.probe.tree(run.pid)
            ascii_results = [element for element in ascii_tree.get('elements', [])
                             if element.get('role') == 'AXButton' and 'Alpha' in (element.get('description') or '')]
            if ascii_focus.get('value_prefix') != 'Alpha':
                errors.append(f'ASCII search query regressed: {ascii_focus.get("value_prefix")!r}')
            if not ascii_results:
                errors.append('ASCII search returned no synthetic match')
            result['runs'].append({'visible_before': visible,
                                   'observations': [opened, composing, committed, ascii_observation],
                                   'errors': errors, 'passed': not errors})
        finally:
            run.stop()
    result['passed'] = all(item['passed'] for item in result['runs'])
    return result


def scenario_a5(base, probe, app):
    notes_a = {'a.md': '# A\n\nAlpha.\n'}
    notes_b = {'b.md': '# B\n\nBeta.\n'}
    run = Run('a5', base, probe, app, (notes_a, notes_b), windows=2)
    result = {'scenario': 'A5', 'runs': []}
    try:
        run.start()
        time.sleep(1.0)
        names = menu_names(run.pid)
        edit = menu_items(run.pid, '编辑')
        errors = []
        if names.count('视图') != 1 or names.count('面板') != 1:
            errors.append(f'duplicate View/Panels menus: {names}')
        for title in ['对齐表格源码', '查找…', '查找下一个', '查找上一个', '查找并替换…', '快速打开', '搜索笔记']:
            if edit.count(title) != 1:
                errors.append(f'edit menu item {title!r} count {edit.count(title)}')
        snapshot = run.snapshot()
        windows = snapshot.get('windows', [])
        if len(windows) < 2:
            errors.append(f'two windows were not opened: {len(windows)}')
        focused_window = snapshot.get('focused_window') or {}
        key_title = focused_window.get('title', '')
        before_areas = [area.get('description', '') for area in focused_window.get('text_areas', [])
                        if area.get('role') == 'AXTextArea']
        run.probe.key(14, 'cmd')
        time.sleep(1.0)
        after_snapshot = run.snapshot()
        after_window = after_snapshot.get('focused_window') or {}
        after_areas = [area.get('description', '') for area in after_window.get('text_areas', [])
                       if area.get('role') == 'AXTextArea']
        if not key_title:
            errors.append('key window title unavailable')
        if before_areas == after_areas:
            errors.append(f'⌘E did not switch the key window view: {before_areas} -> {after_areas}')
        if len(after_snapshot.get('windows', [])) < 2:
            errors.append('window count changed after ⌘E')
        run.probe.key(14, 'cmd')
        time.sleep(0.8)
        result['runs'].append({'menus': names, 'edit_items': edit, 'windows': len(windows),
                               'key_title': key_title, 'before_areas': before_areas,
                               'after_areas': after_areas, 'errors': errors, 'passed': not errors})
    finally:
        run.stop()
    result['passed'] = all(item['passed'] for item in result['runs'])
    return result


SCENARIOS = {
    'A1': scenario_a1,
    'A1-empty': scenario_a1_deterministic,
    'A2': scenario_a2,
    'A3': scenario_a3,
    'A4': scenario_a4,
    'A5': scenario_a5,
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/editor-commands')
    parser.add_argument('--app', type=Path, default=APP)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument('--scenario', choices=['all', *SCENARIOS], default='all')
    parser.add_argument('--skip-empty', action='store_true')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    global MEDIA, OUTPUT
    OUTPUT = output
    MEDIA = args.screenshots.resolve() if args.screenshots else None
    if args.runs < 1:
        parser.error('--runs must be positive')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    helper_dir = Path(tempfile.mkdtemp(prefix='hortus-editor-commands-'))
    probe_path = helper_dir / 'editor-command-probe'
    subprocess.run(['swiftc', '-parse-as-library', '-O', str(PROBE_SOURCE), '-framework', 'Carbon',
                    '-o', str(probe_path)], check=True)
    probe = Probe(probe_path)
    app = args.app.resolve()
    summary = {'status': 'passed', 'app': str(app), 'runs': args.runs, 'scenarios': {}}
    selected = [name for name in SCENARIOS if args.scenario in ('all', name)]
    for name in selected:
        if args.skip_empty and name == 'A1-empty':
            continue
        runs = []
        count = 1 if name == 'A1-empty' else (args.runs * 2 if name == 'A2' else args.runs)
        for index in range(1, count + 1):
            base = output / name / f'run-{index}'
            if base.exists():
                shutil.rmtree(base)
            base.mkdir(parents=True)
            if name == 'A2':
                value = scenario_a2(base, probe, app, terminal_visible=index % 2 == 1)
            elif name == 'A1-empty':
                value = scenario_a1_deterministic(base, probe, app)
            else:
                value = SCENARIOS[name](base, probe, app)
            write_json(base / 'verdict.json', value)
            runs.append(value)
            print(f'{name} run {index}: {"PASS" if value["passed"] else "FAIL"}', flush=True)
            for run in value.get('runs', []):
                for error in run.get('errors', []):
                    print(f'  {error}', flush=True)
            for error in value.get('errors', []):
                print(f'  {error}', flush=True)
        passed = all(item['passed'] for item in runs)
        summary['scenarios'][name] = {
            'passed': passed,
            'runs_passed': sum(item['passed'] for item in runs),
            'runs_total': len(runs),
            'results': runs,
        }
        if not passed:
            summary['status'] = 'failed'
    write_json(output / 'summary.json', summary)
    shutil.rmtree(helper_dir, ignore_errors=True)
    print(json.dumps({'status': summary['status'],
                      'scenarios': {name: {'passed': value['passed'],
                                           'runs': f"{value['runs_passed']}/{value['runs_total']}"}
                                    for name, value in summary['scenarios'].items()}},
                     indent=2, ensure_ascii=False))
    if summary['status'] != 'passed':
        raise SystemExit(1)
    print('编辑器命令检查：全部通过')


if __name__ == '__main__':
    main()
