#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""M2a Android 知识导航、搜索、平板双栏、轻编辑与系统入口验收（A2–A7）。

在宿主机临时目录拼装验收库（公开 demo-vault + 合成笔记），分别在手机与平板模拟器上
执行场景并输出结构化 JSON、截图和汇总。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = 'io.github.groundgrounder.hortus'
ACTIVITY = f'{PACKAGE}/.MainActivity'
DEVICE_VAULT = '/sdcard/Documents/HortusM2aVault'
DEVICE_OUTSIDE = '/sdcard/Documents/HortusM2aOutside'
REMOTE_DIR = f'/sdcard/Android/data/{PACKAGE}/files'

NEW_NOTE_TEXT = '''# 移动端新笔记

中文与 English 混排。新的标签 #mobile-tag，链接到 [[Start here]]。
唯一检索词 移动端检索词。

```mermaid
flowchart LR
  A[开始] --> B[结束]
```
'''

CONFLICT_INITIAL = '# Conflict target\n\nOriginal bytes.\n'


def log(message: str) -> None:
    print(message, flush=True)


def find_sdk_root() -> Path:
    for key in ('ANDROID_HOME', 'ANDROID_SDK_ROOT'):
        value = os.environ.get(key)
        if value and Path(value).is_dir():
            return Path(value)
    default = Path.home() / 'Library/Android/sdk'
    if default.is_dir():
        return default
    raise SystemExit('未找到 Android SDK（请设置 ANDROID_HOME 或 ANDROID_SDK_ROOT）')


def safe_name(name: str) -> str:
    return re.sub(r'[^A-Za-z0-9_.-]', '_', name)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_note(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8')


def copy_tree(source: Path, destination: Path) -> None:
    for item in source.iterdir():
        target = destination / item.name
        if item.is_dir():
            shutil.copytree(item, target, dirs_exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(item, target)


class Device:
    def __init__(self, adb: Path, serial: str, output: Path, label: str):
        self.adb = adb
        self.serial = serial
        self.output = output
        self.label = label
        self.device_dir = output / label
        self.device_dir.mkdir(parents=True, exist_ok=True)

    def run(self, *args: str, check: bool = True, binary: bool = False):
        command = [str(self.adb), '-s', self.serial, *args]
        result = subprocess.run(command, capture_output=True, timeout=180)
        if check and result.returncode != 0:
            raise RuntimeError(
                f'{" ".join(args)} failed: {result.stderr.decode(errors="replace")}',
            )
        return result.stdout if binary else result.stdout.decode(errors='replace')

    def shell(self, command: str, check: bool = True) -> str:
        return self.run('shell', command, check=check)

    def install(self, apk: Path) -> None:
        result = self.run('install', '-r', '-t', str(apk))
        if 'Success' not in result and 'VERSION_DOWNGRADE' in result:
            self.shell(f'pm uninstall {PACKAGE}', check=False)
            result = self.run('install', '-r', '-t', str(apk))
        if 'Success' not in result:
            raise RuntimeError(f'APK 安装失败：{result}')
        self.shell(f'appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow', check=False)

    def ensure_installed(self, apk: Path) -> None:
        if 'package:' not in self.shell(f'pm path {PACKAGE}', check=False):
            self.install(apk)

    def grant(self) -> None:
        self.shell(f'appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow', check=False)

    def start(self, note: str | None = None, dump: str | None = None,
              wait_ms: int = 900, cold: bool = False, action: str | None = None,
              extra: list[str] | None = None) -> None:
        if cold:
            self.shell(f'am force-stop {PACKAGE}')
        args = ['am', 'start', '-W', '-n', ACTIVITY]
        if note is not None:
            args += ['--es', 'debug_vault', urllib.parse.quote(DEVICE_VAULT, safe='')]
            args += ['--es', 'debug_note', urllib.parse.quote(note, safe='')]
        if action is not None:
            args += ['--es', 'debug_action', action]
        if dump:
            args += ['--es', 'debug_dump', dump]
            args += ['--el', 'debug_wait_ms', str(wait_ms)]
        if extra:
            args += extra
        self.run('shell', *args)

    def view(self, uri: str, mime: str, dump: str, wait_ms: int = 1500) -> None:
        args = ['am', 'start', '-W', '-n', ACTIVITY,
                '-a', 'android.intent.action.VIEW',
                '-d', uri, '-t', mime, '--es', 'debug_dump', dump,
                '--el', 'debug_wait_ms', str(wait_ms)]
        self.run('shell', *args)

    def pull(self, remote: str, local: Path) -> bool:
        result = subprocess.run(
            [str(self.adb), '-s', self.serial, 'pull', remote, str(local)],
            capture_output=True, timeout=180,
        )
        return result.returncode == 0 and local.is_file()

    def read_remote(self, remote: str) -> bytes:
        return self.run('exec-out', 'cat', remote, binary=True)

    def write_remote(self, remote: str, data: bytes) -> None:
        local = self.device_dir / f'upload-{safe_name(Path(remote).name)}'
        local.write_bytes(data)
        self.shell(f'mkdir -p "{str(Path(remote).parent)}"')
        self.run('push', str(local), remote)

    def dump(self, name: str, note: str | None = None, cold: bool = False,
             wait_ms: int = 900, timeout: float = 60.0, action: str | None = None,
             extra: list[str] | None = None) -> dict:
        name = safe_name(name)
        remote = f'{REMOTE_DIR}/{name}'
        self.shell(f'rm -f {remote}', check=False)
        self.start(note=note, dump=name, wait_ms=wait_ms, cold=cold, action=action, extra=extra)
        local = self.device_dir / name
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.pull(remote, local):
                return json.loads(local.read_text())
            time.sleep(0.25)
        log(self.shell('logcat -d -t 400', check=False)[-8000:])
        raise RuntimeError(f'等待检查 JSON 超时：{name}')

    def wait_dump(self, name: str, timeout: float = 60.0) -> dict:
        name = safe_name(name)
        remote = f'{REMOTE_DIR}/{name}'
        local = self.device_dir / name
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.pull(remote, local):
                return json.loads(local.read_text())
            time.sleep(0.25)
        raise RuntimeError(f'等待检查 JSON 超时：{name}')

    def action(self, action: str, extra: list[str] | None = None) -> None:
        self.start(action=action, extra=extra)

    def screenshot(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = subprocess.run(
            [str(self.adb), '-s', self.serial, 'exec-out', 'screencap', '-p'],
            capture_output=True, timeout=90,
        ).stdout
        path.write_bytes(data)

    def tap(self, x: float, y: float) -> None:
        self.shell(f'input tap {int(x)} {int(y)}')
        time.sleep(0.9)

    def key(self, code: int = 4) -> None:
        self.shell(f'input keyevent {code}')
        time.sleep(0.9)

    def night(self, dark: bool) -> None:
        self.shell(f'cmd uimode night {"yes" if dark else "no"}')
        time.sleep(1.4)

    def rotate(self, landscape: bool) -> None:
        self.shell('settings put system accelerometer_rotation 0')
        self.shell(f'settings put system user_rotation {1 if landscape else 0}')
        time.sleep(1.5)

    def screen(self) -> tuple[int, int, float]:
        size = self.shell('wm size')
        match = re.search(r'(\d+)x(\d+)', size)
        if not match:
            raise RuntimeError(f'无法解析屏幕尺寸：{size}')
        density = re.search(r'(\d+)', self.shell('wm density')).group(1)
        return int(match.group(1)), int(match.group(2)), int(density) / 160.0

    def pid(self) -> int:
        value = self.shell(f'pidof {PACKAGE}', check=False).strip()
        return int(value.split()[0]) if value.split() else 0


def ensure_emulator(sdk: Path, avd: str, output: Path) -> tuple[str, subprocess.Popen | None]:
    adb = sdk / 'platform-tools/adb'
    emulator = sdk / 'emulator/emulator'

    def matching() -> str | None:
        for line in subprocess.run([str(adb), 'devices'], capture_output=True, text=True).stdout.splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2 and parts[1] == 'device' and parts[0].startswith('emulator-'):
                name = subprocess.run(
                    [str(adb), '-s', parts[0], 'emu', 'avd', 'name'],
                    capture_output=True, text=True,
                ).stdout.splitlines()
                if name and name[0].strip() == avd:
                    booted = subprocess.run(
                        [str(adb), '-s', parts[0], 'shell', 'getprop', 'sys.boot_completed'],
                        capture_output=True, text=True,
                    ).stdout.strip()
                    if booted == '1':
                        return parts[0]
        return None

    serial = matching()
    proc = None
    if serial is None:
        log(f'启动模拟器 {avd}…')
        output.mkdir(parents=True, exist_ok=True)
        with (output / f'emulator-{safe_name(avd)}.log').open('w') as log_file:
            proc = subprocess.Popen(
                [str(emulator), '-avd', avd, '-no-window', '-no-boot-anim', '-no-audio',
                 '-no-snapshot-save', '-gpu', 'auto'],
                stdout=log_file, stderr=subprocess.STDOUT,
            )
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        serial = matching()
        if serial is not None:
            return serial, proc
        if proc is not None and proc.poll() is not None:
            raise RuntimeError('模拟器提前退出，见 emulator 日志')
        time.sleep(2)
    raise RuntimeError('等待模拟器启动超时')


class Results:
    def __init__(self, output: Path):
        self.output = output
        self.checks: list[dict] = []

    def check(self, item: str, name: str, ok: bool, detail=None) -> bool:
        self.checks.append({
            'item': item,
            'name': name,
            'ok': bool(ok),
            'detail': detail,
        })
        log(f'{"PASS" if ok else "FAIL"} {item} {name}')
        if not ok:
            log(f'     {detail}')
        return bool(ok)

    def passed(self, item: str) -> bool:
        values = [c['ok'] for c in self.checks if c['item'] == item]
        return bool(values) and all(values)

    def summary(self) -> dict:
        items: dict[str, dict] = {}
        for check in self.checks:
            entry = items.setdefault(check['item'], {'ok': True, 'checks': []})
            entry['ok'] = entry['ok'] and check['ok']
            entry['checks'].append(check)
        return {'items': items, 'checks': self.checks}


def center(hit: dict) -> tuple[float, float]:
    return hit['x'] + hit['width'] / 2, hit['y'] + hit['height'] / 2


def tap_hit(device: Device, dump: dict, kind: str, label_contains: str) -> dict | None:
    for hit in dump.get('panelHits', []):
        if hit.get('kind') == kind and (
            label_contains in (hit.get('label') or '') or
            label_contains in (hit.get('target') or '')
        ):
            device.tap(*center(hit))
            return hit
    for hit in dump.get('interactive', []):
        if hit.get('kind') == kind and label_contains in (hit.get('label') or ''):
            device.tap(*center(hit))
            return hit
    return None


def ui_click(device: Device, needles: list[str], timeout: float = 8.0) -> bool:
    deadline = time.monotonic() + timeout
    remote = '/sdcard/window-m2a.xml'
    while time.monotonic() < deadline:
        device.shell(f'uiautomator dump {remote}', check=False)
        local = device.device_dir / 'window-m2a.xml'
        if not device.pull(remote, local):
            time.sleep(0.4)
            continue
        try:
            tree = ET.parse(local)
        except ET.ParseError:
            time.sleep(0.4)
            continue
        for node in tree.iter('node'):
            text = (node.get('text') or '') + (node.get('content-desc') or '')
            if all(needle in text for needle in needles):
                bounds = re.findall(r'\d+', node.get('bounds') or '')
                if len(bounds) == 4:
                    x1, y1, x2, y2 = map(int, bounds)
                    device.tap((x1 + x2) / 2, (y1 + y2) / 2)
                    return True
        time.sleep(0.4)
    return False


def build_vault(stage: Path) -> dict:
    copy_tree(ROOT / 'fixtures/demo-vault', stage)
    long_lines = ['# Long outline', '']
    for heading, words in [('Alpha', 'first'), ('Beta', 'second'), ('Gamma', 'third')]:
        long_lines += [f'## {heading}', '']
        for index in range(24):
            long_lines += [f'{words} section paragraph {index} with enough text to fill a phone screen.', '']
    write_note(stage, 'M2a/Long outline.md', '\n'.join(long_lines))
    write_note(stage, 'M2a/Backlinks body.md',
               '# Backlinks body\n\nLink to [[Long outline]].\n')
    write_note(stage, 'M2a/Backlinks diagram.md', '''# Backlinks diagram

```mermaid
mindmap
  root((References))
    [[Long outline]]
```
''')
    write_note(stage, 'M2a/Tags.md',
               '# Tags note\n\nBody tag #alpha and another #mobile-tag.\n\nLink to [[Long outline]].\n')
    write_note(stage, 'M2a/Search 搜索.md', '''# 山川与河流

A quiet paragraph about 山水 and rivers, with searchable English words.

## Section two

More text about mobile knowledge editing.
''')
    write_note(stage, 'M2a/Conflict target.md', CONFLICT_INITIAL)
    write_note(stage, 'M2a/Outside preview source.md',
               '# Outside preview source\n\nOpened from outside the known vault.\n')
    write_note(stage, 'M2a/编辑基准.md', '# 编辑基准\n\n初始内容。\n')
    extra = stage / 'M2a' / 'New mobile note.md'
    if extra.exists():
        extra.unlink()
    return {'longOutline': 'M2a/Long outline.md'}


def copy_vault(device: Device, stage: Path) -> None:
    device.shell(f'rm -rf {DEVICE_VAULT}')
    device.shell(f'mkdir -p {DEVICE_VAULT}')
    for item in sorted(stage.iterdir()):
        device.run('push', str(item), f'{DEVICE_VAULT}/')


def leftovers(device: Device) -> dict[str, list[str]]:
    vault = device.shell(
        f'find {DEVICE_VAULT} -type f \\( -name ".*" -o -name "*tmp*" -o -name "*hortus-save*" \\) -print',
        check=False,
    ).splitlines()
    parent = device.shell(
        f'find {DEVICE_OUTSIDE.rsplit("/", 1)[0]} -maxdepth 1 -name ".hortus-save-*" -print',
        check=False,
    ).splitlines()
    cache = device.shell(
        f'find /sdcard/Android/data/{PACKAGE}/cache -type f \\( -name ".*" -o -name "*tmp*" \\) -print',
        check=False,
    ).splitlines()
    return {'vault': [v for v in vault if v], 'parent': [p for p in parent if p], 'cache': [c for c in cache if c]}


def main_thread_zero(dump: dict, names: list[str]) -> tuple[bool, dict]:
    counters = dump.get('mainThread', {})
    values = {name: counters.get(name, -1) for name in names}
    return all(value == 0 for value in values.values()), values


# ---------------------------------------------------------------------------
# A2 统一搜索
# ---------------------------------------------------------------------------

def assert_a2(device: Device, results: Results, media: Path) -> None:
    note = 'M2a/Search 搜索.md'
    dump = device.dump('a2-quick.json', note=note,
                       action='search', extra=['--es', 'debug_tab', 'quick',
                                               '--es', 'debug_query', urllib.parse.quote('搜索')],
                       wait_ms=1200)
    quick_paths = dump.get('search', {}).get('quickPaths', [])
    results.check('A2', 'search bar open', dump.get('search', {}).get('open') is True, dump.get('search'))
    results.check('A2', 'quick open finds Chinese title', note in quick_paths, quick_paths)
    counts_ok, counts = main_thread_zero(dump, ['search', 'quickOpen'])
    results.check('A2', 'quick open stays off the main thread', counts_ok, counts)

    full = device.dump('a2-full.json', note=note,
                       action='search', extra=['--es', 'debug_tab', 'full',
                                               '--es', 'debug_query', urllib.parse.quote('山水')],
                       wait_ms=1400)
    notes = full.get('search', {}).get('fullNotes', [])
    target = next((item for item in notes if item.get('path') == note), None)
    results.check('A2', 'full text finds mixed-language query', target is not None and target.get('count', 0) > 0, notes)
    results.check('A2', 'full text returns snippets', bool(target and target.get('snippets', 0) > 0), target)
    counts_ok, counts = main_thread_zero(full, ['search', 'quickOpen'])
    results.check('A2', 'full text search stays off the main thread', counts_ok, counts)

    # 快速改写查询：第一次立即被第二次取代，最终结果必须对应第二次查询。
    device.action('search', ['--es', 'debug_tab', 'full',
                             '--es', 'debug_query', urllib.parse.quote('mobile')])
    replaced = device.dump('a2-replace.json', note=note,
                           action='search', extra=['--es', 'debug_tab', 'full',
                                                   '--es', 'debug_query', urllib.parse.quote('山水')],
                           wait_ms=1600)
    search = replaced.get('search', {})
    paths = [item.get('path') for item in search.get('fullNotes', [])]
    results.check('A2', 'stale results never replace the latest query',
                  search.get('query') == '山水' and note in paths,
                  {'query': search.get('query'), 'paths': paths,
                   'cancelled': search.get('cancelled'),
                   'staleDiscarded': search.get('staleDiscarded')})
    device.screenshot(media / 'm2a-search.png')
    results.check('A2', 'search screenshot taken', (media / 'm2a-search.png').is_file(), str(media / 'm2a-search.png'))

    hit = tap_hit(device, replaced, 'search-snippet', '山川与河流')
    results.check('A2', 'search result can be tapped', hit is not None,
                  replaced.get('panelHits', [])[:4])
    opened = device.dump('a2-opened.json', wait_ms=1400)
    results.check('A2', 'search result opens the target note',
                  opened.get('note') == note and opened.get('search', {}).get('open') is False,
                  {'note': opened.get('note'), 'searchOpen': opened.get('search', {}).get('open'),
                   'error': opened.get('error')})
    bounds = {item.get('blockId'): item for item in opened.get('itemBounds', [])}
    first = opened.get('firstVisibleBlockId')
    results.check('A2', 'matched content is visible after opening',
                  first in bounds and bounds[first].get('y', 1e9) < opened['screen']['height'] * 0.8,
                  {'first': first, 'bound': bounds.get(first)})


# ---------------------------------------------------------------------------
# A3 手机知识导航
# ---------------------------------------------------------------------------

def assert_a3(device: Device, results: Results, media: Path) -> None:
    note = 'M2a/Long outline.md'
    dump = device.dump('a3-outline.json', note=note, action='knowledge',
                       extra=['--es', 'debug_tab', 'outline'], wait_ms=1400)
    knowledge = dump.get('knowledge', {})
    kernel = dump.get('outline', [])
    results.check('A3', 'outline sheet opens', knowledge.get('open') is True, knowledge)
    results.check('A3', 'outline count matches the kernel',
                  knowledge.get('outlineCount') == len(kernel) and len(kernel) > 0,
                  {'sheet': knowledge.get('outlineCount'), 'kernel': len(kernel)})
    sheet_outline = knowledge.get('outline', [])
    results.check('A3', 'outline levels and titles match the kernel',
                  [(row['level'], row['title'], int(row['start'])) for row in sheet_outline] ==
                  [(row['level'], row['title'], int(row['start'])) for row in kernel],
                  sheet_outline[:4])
    device.screenshot(media / 'm2a-knowledge-sheet.png')
    results.check('A3', 'knowledge sheet screenshot taken',
                  (media / 'm2a-knowledge-sheet.png').is_file(),
                  str(media / 'm2a-knowledge-sheet.png'))

    target = next((row for row in sheet_outline if row['title'] == 'Beta'), None)
    hit = tap_hit(device, dump, 'outline', 'Beta')
    results.check('A3', 'secondary heading is tappable', hit is not None, sheet_outline[:4])
    scrolled = device.dump('a3-scrolled.json', wait_ms=1400)
    item_index = (scrolled.get('lastOpenTarget') or {}).get('itemIndex')
    items = scrolled.get('items', [])
    expected_block = items[item_index]['blockId'] if item_index is not None and 0 <= item_index < len(items) else None
    bounds = {item.get('blockId'): item for item in scrolled.get('itemBounds', [])}
    y = bounds.get(scrolled.get('firstVisibleBlockId'), {}).get('y')
    results.check('A3', 'secondary heading scrolls into the top 30%',
                  target is not None and expected_block == scrolled.get('firstVisibleBlockId') and
                  y is not None and y < scrolled['screen']['height'] * 0.3,
                  {'target': target, 'expectedBlock': expected_block,
                   'firstBlock': scrolled.get('firstVisibleBlockId'), 'y': y})
    count_ok, counts = main_thread_zero(scrolled, ['outline', 'backlinks', 'tags'])
    results.check('A3', 'knowledge queries stay off the main thread', count_ok, counts)

    back = device.dump('a3-backlinks.json', note=note, action='knowledge',
                       extra=['--es', 'debug_tab', 'backlinks'], wait_ms=1500)
    rows = back.get('knowledge', {}).get('backlinks', [])
    sources = {row.get('source') for row in rows}
    results.check('A3', 'backlinks include body and diagram sources',
                  'M2a/Backlinks body.md' in sources and 'M2a/Backlinks diagram.md' in sources and
                  any(row.get('inDiagram') for row in rows),
                  rows)
    first_source = 'M2a/Backlinks body.md'
    hit = tap_hit(device, back, 'backlink', first_source)
    results.check('A3', 'backlink row is tappable', hit is not None, rows)
    jumped = device.dump('a3-backlink-open.json', wait_ms=1400)
    results.check('A3', 'backlink opens the source note', jumped.get('note') == first_source,
                  {'note': jumped.get('note')})
    device.key(4)
    returned = device.dump('a3-backlink-back.json', wait_ms=1200)
    results.check('A3', 'back returns to the original note', returned.get('note') == note,
                  {'note': returned.get('note')})

    tags = device.dump('a3-tags.json', note=note, action='knowledge',
                       extra=['--es', 'debug_tab', 'tags'], wait_ms=1500)
    tag_rows = tags.get('knowledge', {}).get('tags', [])
    alpha = next((row for row in tag_rows if row.get('tag') == 'alpha'), None)
    results.check('A3', 'tags match the kernel index', alpha is not None, tag_rows)
    hit = tap_hit(device, tags, 'tag', 'alpha')
    results.check('A3', 'tag row is tappable', hit is not None, tag_rows)
    selected = device.dump('a3-tag-notes.json', wait_ms=1300)
    results.check('A3', 'tag selection lists its notes',
                  selected.get('knowledge', {}).get('selectedTag') == 'alpha' and
                  selected.get('knowledge', {}).get('tagNoteCount', 0) > 0,
                  selected.get('knowledge'))
    hit = tap_hit(device, selected, 'tag-note', 'Tags')
    if hit is None:
        hit = tap_hit(device, selected, 'tag-note', 'Tags note')
    results.check('A3', 'tag note row is tappable', hit is not None, selected.get('knowledge'))
    tagged_note = device.dump('a3-tag-open.json', wait_ms=1300)
    results.check('A3', 'tag note opens', tagged_note.get('note') == 'M2a/Tags.md',
                  {'note': tagged_note.get('note')})

    body = device.dump('a3-body-tag.json', note='M2a/Tags.md', wait_ms=1200)
    tag_hit = next((hit for hit in body.get('interactive', []) if hit.get('kind') == 'tag'), None)
    results.check('A3', 'inline tag is clickable', tag_hit is not None, body.get('interactive', [])[:8])
    if tag_hit:
        device.tap(*center(tag_hit))
    inline = device.dump('a3-inline-tag.json', wait_ms=1400)
    results.check('A3', 'inline tag opens its tag panel',
                  inline.get('knowledge', {}).get('open') is True and
                  inline.get('knowledge', {}).get('selectedTag') == 'alpha',
                  inline.get('knowledge'))


# ---------------------------------------------------------------------------
# A4 平板双栏
# ---------------------------------------------------------------------------

def assert_a4(device: Device, results: Results, media: Path) -> None:
    note = 'Learning/Learning map.md'
    light = device.dump('a4-light.json', note=note, cold=True, wait_ms=1500)
    width = light.get('screen', {}).get('width', 0)
    side = light.get('sidePaneWidthPx', 0)
    detail = light.get('detailPaneWidthPx', 0)
    results.check('A4', 'tablet layout mode', light.get('layoutMode') == 'tablet', light.get('layoutMode'))
    results.check('A4', 'both panes are measured', side > 0 and detail > 0, {'side': side, 'detail': detail})
    results.check('A4', 'panes do not overlap', side + detail <= width, {'side': side, 'detail': detail, 'width': width})
    device.screenshot(media / 'm2a-tablet-light.png')
    results.check('A4', 'tablet light screenshot taken', (media / 'm2a-tablet-light.png').is_file(),
                  str(media / 'm2a-tablet-light.png'))

    files = device.dump('a4-files.json', note=note, action='side',
                        extra=['--es', 'debug_tab', 'files'], wait_ms=1500)
    results.check('A4', 'side pane file tree is the default',
                  files.get('sideMode') == 'files', files.get('sideMode'))
    hit = tap_hit(device, files, 'file', 'Markdown field guide.md')
    results.check('A4', 'file tree row is tappable', hit is not None, files.get('panelHits', [])[:4])
    file_open = device.dump('a4-file-open.json', wait_ms=1400)
    results.check('A4', 'file tree opens a note in the detail pane',
                  file_open.get('note') == 'Markdown field guide.md',
                  {'note': file_open.get('note')})

    outline = device.dump('a4-outline.json', note=note, action='side',
                          extra=['--es', 'debug_tab', 'outline'], wait_ms=1600)
    results.check('A4', 'side pane outline loaded',
                  outline.get('sideMode') == 'outline' and
                  outline.get('knowledge', {}).get('outlineCount', 0) > 0,
                  outline.get('knowledge'))
    outline_rows = outline.get('knowledge', {}).get('outline', [])
    hit = next((row for row in outline.get('panelHits', []) if row.get('kind') == 'outline'), None)
    results.check('A4', 'outline rows have tap targets', hit is not None, outline_rows[:4])
    if hit:
        device.tap(*center(hit))
    outline_open = device.dump('a4-outline-open.json', wait_ms=1500)
    results.check('A4', 'outline selection updates the detail pane',
                  outline_open.get('lastOpenTarget') is not None and
                  outline_open.get('note') == note,
                  {'lastOpenTarget': outline_open.get('lastOpenTarget'),
                   'note': outline_open.get('note')})

    backlinks = device.dump('a4-backlinks.json', note=note, action='side',
                            extra=['--es', 'debug_tab', 'backlinks'], wait_ms=1600)
    back_rows = backlinks.get('knowledge', {}).get('backlinks', [])
    results.check('A4', 'side pane backlinks loaded',
                  backlinks.get('sideMode') == 'backlinks' and len(back_rows) > 0,
                  back_rows[:4])
    hit = next((row for row in backlinks.get('panelHits', []) if row.get('kind') == 'backlink'), None)
    results.check('A4', 'backlink rows have tap targets', hit is not None, back_rows[:4])
    if hit:
        device.tap(*center(hit))
    back_open = device.dump('a4-backlink-open.json', wait_ms=1500)
    results.check('A4', 'backlink selection opens the source note',
                  back_open.get('note') in {row.get('source') for row in back_rows},
                  {'note': back_open.get('note'), 'sources': [row.get('source') for row in back_rows]})

    tags = device.dump('a4-tags.json', note=note, action='side',
                       extra=['--es', 'debug_tab', 'tags'], wait_ms=1600)
    tag_rows = tags.get('knowledge', {}).get('tags', [])
    results.check('A4', 'side pane tags loaded',
                  tags.get('sideMode') == 'tags' and len(tag_rows) > 0, tag_rows[:4])
    tag_hit = next((row for row in tags.get('panelHits', []) if row.get('kind') == 'tag'), None)
    results.check('A4', 'tag rows have tap targets', tag_hit is not None, tag_rows[:4])
    if tag_hit:
        device.tap(*center(tag_hit))
    tag_notes = device.dump('a4-tag-notes.json', wait_ms=1500)
    results.check('A4', 'tag selection lists notes',
                  tag_notes.get('knowledge', {}).get('selectedTag') is not None and
                  tag_notes.get('knowledge', {}).get('tagNoteCount', 0) > 0,
                  tag_notes.get('knowledge'))
    note_hit = next((row for row in tag_notes.get('panelHits', []) if row.get('kind') == 'tag-note'), None)
    if note_hit:
        device.tap(*center(note_hit))
    tag_open = device.dump('a4-tag-open.json', wait_ms=1500)
    results.check('A4', 'tag note opens in the detail pane',
                  tag_open.get('knowledge', {}).get('tagNoteCount', 0) > 0 and
                  tag_open.get('note') is not None,
                  {'note': tag_open.get('note')})

    collapsed = device.dump('a4-collapsed.json', action='pane', wait_ms=1000)
    results.check('A4', 'side pane collapses',
                  collapsed.get('sidePaneOpen') is False and collapsed.get('sidePaneWidthPx') == 0 and
                  collapsed.get('detailPaneWidthPx', 0) > 0,
                  {'open': collapsed.get('sidePaneOpen'),
                   'side': collapsed.get('sidePaneWidthPx'),
                   'detail': collapsed.get('detailPaneWidthPx')})
    expanded = device.dump('a4-expanded.json', action='pane', wait_ms=1000)
    results.check('A4', 'side pane expands again',
                  expanded.get('sidePaneOpen') is True and expanded.get('sidePaneWidthPx', 0) > 0,
                  {'open': expanded.get('sidePaneOpen'), 'side': expanded.get('sidePaneWidthPx')})

    device.night(True)
    dark = device.dump('a4-dark.json', note=note, wait_ms=1200)
    device.screenshot(media / 'm2a-tablet-dark.png')
    results.check('A4', 'tablet dark screenshot taken', (media / 'm2a-tablet-dark.png').is_file(),
                  str(media / 'm2a-tablet-dark.png'))
    results.check('A4', 'dark mode keeps both panes',
                  dark.get('sidePaneWidthPx', 0) > 0 and dark.get('detailPaneWidthPx', 0) > 0,
                  {'side': dark.get('sidePaneWidthPx'), 'detail': dark.get('detailPaneWidthPx')})
    device.night(False)

    # 跨过 600 dp 断点：笔记与滚动位置不丢失。
    device.rotate(False)
    outline = device.dump('a4-breakpoint-outline.json', note='M2a/Long outline.md',
                          action='side', extra=['--es', 'debug_tab', 'outline'], wait_ms=1600)
    hit = tap_hit(device, outline, 'outline', 'Beta')
    results.check('A4', 'outline scroll prepared before the breakpoint', hit is not None,
                  outline.get('panelHits', [])[:4])
    before = device.dump('a4-breakpoint-before.json', wait_ms=1500)
    device.shell('wm size 1000x1600')
    time.sleep(1.5)
    narrow = device.dump('a4-breakpoint-phone.json', wait_ms=1500)
    results.check('A4', 'narrow window falls back to phone layout',
                  narrow.get('layoutMode') == 'phone', narrow.get('layoutMode'))
    results.check('A4', 'note survives the breakpoint',
                  narrow.get('note') == before.get('note'), {'before': before.get('note'), 'after': narrow.get('note')})
    results.check('A4', 'scroll position survives the breakpoint',
                  narrow.get('firstVisibleBlockId') == before.get('firstVisibleBlockId') and
                  abs(narrow.get('firstVisibleOffsetPx', 0) - before.get('firstVisibleOffsetPx', 0)) <=
                  24 * narrow['screen']['density'],
                  {'beforeBlock': before.get('firstVisibleBlockId'),
                   'afterBlock': narrow.get('firstVisibleBlockId'),
                   'beforeOffset': before.get('firstVisibleOffsetPx'),
                   'afterOffset': narrow.get('firstVisibleOffsetPx')})
    device.shell('wm size reset')
    time.sleep(1.2)
    device.rotate(True)


# ---------------------------------------------------------------------------
# A5 轻编辑、新建笔记与实时索引
# ---------------------------------------------------------------------------

def write_edit_file(device: Device, name: str, text: str) -> None:
    directory = device.device_dir / 'edit'
    directory.mkdir(parents=True, exist_ok=True)
    local = directory / name
    local.write_text(text, encoding='utf-8')
    device.run('push', str(local), f'{REMOTE_DIR}/{name}')


def assert_a5(device: Device, results: Results, media: Path) -> None:
    note = 'M2a/编辑基准.md'
    created = 'M2a/New mobile note.md'
    device.dump('a5-start.json', note=note, wait_ms=1200)
    device.shell(f'rm -f "{DEVICE_VAULT}/{created}"')

    created_dump = device.dump('a5-create.json', note=note, action='create',
                               extra=['--es', 'debug_name', urllib.parse.quote('New mobile note'),
                                      '--es', 'debug_parent', urllib.parse.quote('M2a')],
                               wait_ms=2000)
    results.check('A5', 'new note created', created_dump.get('newNote', {}).get('lastCreated') == created,
                  created_dump.get('newNote'))
    results.check('A5', 'new note opens in editor',
                  created_dump.get('edit', {}).get('active') is True, created_dump.get('edit'))
    counts_ok, counts = main_thread_zero(created_dump, ['createNote', 'save'])
    results.check('A5', 'note creation stays off the main thread', counts_ok, counts)

    duplicate = device.dump('a5-duplicate.json', note=note, action='create',
                            extra=['--es', 'debug_name', urllib.parse.quote('New mobile note'),
                                   '--es', 'debug_parent', urllib.parse.quote('M2a')],
                            wait_ms=1600)
    duplicate_state = duplicate.get('newNote', {})
    results.check('A5', 'duplicate name is rejected locally',
                  duplicate_state.get('open') is True and
                  duplicate_state.get('error') == 'vault.already_exists',
                  duplicate_state)

    write_edit_file(device, 'm2a-edit.txt', NEW_NOTE_TEXT)
    edited = device.dump('a5-edit.json', note=created, action='edit',
                         extra=['--es', 'debug_edit_file', 'm2a-edit.txt'],
                         wait_ms=1800)
    results.check('A5', 'editor shows unsaved text',
                  edited.get('edit', {}).get('active') is True and
                  edited.get('edit', {}).get('dirty') is True,
                  edited.get('edit'))
    device.screenshot(media / 'm2a-editor-light.png')
    results.check('A5', 'editor screenshot taken', (media / 'm2a-editor-light.png').is_file(),
                  str(media / 'm2a-editor-light.png'))

    saved = device.dump('a5-saved.json', action='save', extra=['--es', 'debug_save', 'normal'],
                        wait_ms=1600)
    results.check('A5', 'save clears the dirty flag',
                  saved.get('edit', {}).get('dirty') is False and
                  saved.get('edit', {}).get('lastSaveOk') is True,
                  saved.get('edit'))
    counts_ok, counts = main_thread_zero(saved, ['save', 'createNote'])
    results.check('A5', 'saving stays off the main thread', counts_ok, counts)

    expected = NEW_NOTE_TEXT.encode('utf-8')
    disk = device.read_remote(f'{DEVICE_VAULT}/{created}')
    results.check('A5', 'saved bytes match the edited text', sha256(disk) == sha256(expected),
                  {'disk': sha256(disk), 'expected': sha256(expected), 'size': len(disk)})
    left = leftovers(device)
    results.check('A5', 'no temporary files remain', not any(left.values()), left)

    finished = device.dump('a5-finished.json', note=created, wait_ms=2000)
    diagrams = finished.get('diagrams', [])
    rendered = [item for item in diagrams if item.get('width', 0) > 0 and item.get('height', 0) > 0]
    results.check('A5', 'new Mermaid block renders after saving', bool(rendered),
                  {'diagrams': len(diagrams), 'rendered': len(rendered)})
    results.check('A5', 'reading view returns after saving',
                  finished.get('edit', {}).get('active') is False,
                  finished.get('edit'))
    left = leftovers(device)
    results.check('A5', 'no temporary files remain after rendering', not any(left.values()), left)

    search = device.dump('a5-search.json', note=created, action='search',
                         extra=['--es', 'debug_tab', 'full',
                                '--es', 'debug_query', urllib.parse.quote('移动端检索词')],
                         wait_ms=1600)
    hits = [item.get('path') for item in search.get('search', {}).get('fullNotes', [])]
    results.check('A5', 'new text is searchable immediately', created in hits, hits)

    tags = device.dump('a5-tags.json', note=created, action='knowledge',
                       extra=['--es', 'debug_tab', 'tags'], wait_ms=1600)
    tag_rows = tags.get('knowledge', {}).get('tags', [])
    mobile = next((row for row in tag_rows if row.get('tag') == 'mobile-tag'), None)
    results.check('A5', 'new tag reaches the tag index',
                  mobile is not None and mobile.get('count', 0) >= 1, tag_rows)

    back = device.dump('a5-backlinks.json', note='Start here.md', action='knowledge',
                       extra=['--es', 'debug_tab', 'backlinks'], wait_ms=1800)
    backlinks = back.get('knowledge', {}).get('backlinks', [])
    results.check('A5', 'new backlink reaches Start here',
                  any(row.get('source') == created for row in backlinks), backlinks)



# ---------------------------------------------------------------------------
# A6 冲突安全保存
# ---------------------------------------------------------------------------

def edit_local(device: Device, text: str, name: str) -> dict:
    write_edit_file(device, name, text)
    return device.dump(f'a6-{name}.json', note='M2a/Conflict target.md', action='edit',
                       extra=['--es', 'debug_edit_file', name], wait_ms=1600)


def external_write(device: Device, remote: str, text: str) -> None:
    local = device.device_dir / 'external.md'
    local.write_text(text, encoding='utf-8')
    device.run('push', str(local), remote)


def assert_a6(device: Device, results: Results) -> None:
    note = 'M2a/Conflict target.md'
    remote = f'{DEVICE_VAULT}/{note}'

    keep_text = '# Conflict target\n\nLOCAL KEEP 保留\n'
    local = edit_local(device, keep_text, 'a6-keep.txt')
    results.check('A6', 'edit marks the session dirty', local.get('edit', {}).get('dirty') is True,
                  local.get('edit'))
    external_write(device, remote, '# Conflict target\n\nEXTERNAL 外部\n')
    conflict = device.dump('a6-keep-conflict.json', action='save',
                           extra=['--es', 'debug_save', 'normal'], wait_ms=1600)
    results.check('A6', 'external rewrite raises a conflict',
                  conflict.get('edit', {}).get('issue') == 'conflict', conflict.get('edit'))
    kept = device.dump('a6-keep-saved.json', action='save',
                       extra=['--es', 'debug_save', 'keep'], wait_ms=1800)
    disk = device.read_remote(remote)
    results.check('A6', 'KeepMine overwrites the disk and clears dirty',
                  kept.get('edit', {}).get('dirty') is False and
                  kept.get('edit', {}).get('issue') is None and
                  disk == keep_text.encode('utf-8'),
                  {'edit': kept.get('edit'), 'disk': disk.decode('utf-8', 'replace')})

    disk_text = '# Conflict target\n\nEXTERNAL FOR DISK 磁盘\n'
    edit_local(device, '# Conflict target\n\nLOCAL TO DISCARD\n', 'a6-disk.txt')
    external_write(device, remote, disk_text)
    device.dump('a6-disk-conflict.json', action='save', extra=['--es', 'debug_save', 'normal'],
                wait_ms=1600)
    disk_loaded = device.dump('a6-disk.json', action='save', extra=['--es', 'debug_save', 'disk'],
                              wait_ms=2000)
    results.check('A6', 'use_disk discards the local edit',
                  disk_loaded.get('edit', {}).get('active') is False and
                  'EXTERNAL FOR DISK' in disk_loaded.get('displayText', ''),
                  {'edit': disk_loaded.get('edit'),
                   'displayText': disk_loaded.get('displayText', '')[:160]})

    copy_local = '# Conflict target\n\nLOCAL COPY 副本\n'
    copy_external = '# Conflict target\n\nEXTERNAL COPY 原件\n'
    edit_local(device, copy_local, 'a6-copy.txt')
    external_write(device, remote, copy_external)
    device.dump('a6-copy-conflict.json', action='save', extra=['--es', 'debug_save', 'normal'],
                wait_ms=1600)
    copy_dump = device.dump('a6-copy.json', action='save', extra=['--es', 'debug_save', 'copy'],
                            wait_ms=2000)
    copy_path = copy_dump.get('edit', {}).get('copyPath')
    results.check('A6', 'ConflictCopy reports the copy path',
                  bool(copy_path and copy_path.endswith('(conflict copy).md')), copy_path)
    original = device.read_remote(remote)
    copied = device.read_remote(f'{DEVICE_VAULT}/{copy_path}') if copy_path else b''
    results.check('A6', 'conflict copy and original bytes are both intact',
                  original == copy_external.encode('utf-8') and
                  copied == copy_local.encode('utf-8'),
                  {'original': sha256(original), 'copy': sha256(copied)})
    left = leftovers(device)
    results.check('A6', 'conflict copy leaves no temporary files', not any(left.values()), left)

    recreate_text = '# Conflict target\n\nRECREATED 重建\n'
    edit_local(device, recreate_text, 'a6-recreate.txt')
    device.shell(f'rm -f "{remote}"')
    missing = device.dump('a6-missing.json', action='save',
                          extra=['--es', 'debug_save', 'normal'], wait_ms=1600)
    results.check('A6', 'external delete raises Missing',
                  missing.get('edit', {}).get('issue') == 'missing', missing.get('edit'))
    recreated = device.dump('a6-recreated.json', action='save',
                            extra=['--es', 'debug_save', 'keep'], wait_ms=2000)
    disk = device.read_remote(remote)
    results.check('A6', 'Recreate rebuilds the deleted file',
                  recreated.get('edit', {}).get('dirty') is False and
                  disk == recreate_text.encode('utf-8'),
                  {'edit': recreated.get('edit'), 'sha': sha256(disk)})
    left = leftovers(device)
    results.check('A6', 'recreate leaves no temporary files', not any(left.values()), left)


# ---------------------------------------------------------------------------
# A7 Android 原生入口
# ---------------------------------------------------------------------------

def assert_a7(device: Device, results: Results) -> None:
    note = 'Start here.md'
    device.dump('a7-warmup.json', note=note, wait_ms=1200)
    uri = 'file://' + urllib.parse.quote(f'{DEVICE_VAULT}/{note}')
    device.view(uri, 'text/markdown', 'a7-file.json')
    file_dump = device.wait_dump('a7-file.json')
    results.check('A7', 'ACTION_VIEW opens a file inside the known vault',
                  file_dump.get('externalOpenMode') == 'vault' and file_dump.get('note') == note and
                  not file_dump.get('error'),
                  {'mode': file_dump.get('externalOpenMode'), 'note': file_dump.get('note'),
                   'error': file_dump.get('error')})
    results.check('A7', 'vault file renders a reading model',
                  file_dump.get('itemTypes', {}).get('text', 0) > 0,
                  file_dump.get('itemTypes'))

    device.shell(f'rm -rf {DEVICE_OUTSIDE}')
    device.shell(f'mkdir -p {DEVICE_OUTSIDE}')
    outside_text = '# Outside note\n\n[[Start here]] from outside.\n'
    device.write_remote(f'{DEVICE_OUTSIDE}/Outside.md', outside_text.encode('utf-8'))
    outside_uri = 'file://' + urllib.parse.quote(f'{DEVICE_OUTSIDE}/Outside.md')
    device.view(outside_uri, 'text/markdown', 'a7-outside.json')
    outside = device.wait_dump('a7-outside.json')
    results.check('A7', 'unknown path opens with its parent as vault',
                  outside.get('externalOpenMode') == 'parent_vault' and
                  outside.get('note') == 'Outside.md' and outside.get('vault') == DEVICE_OUTSIDE,
                  {'mode': outside.get('externalOpenMode'), 'note': outside.get('note'),
                   'vault': outside.get('vault')})

    preview_text = '# 外部预览\n\n只读内容 unique-preview-token。\n'
    device.write_remote(f'{REMOTE_DIR}/m2a-preview.md', preview_text.encode('utf-8'))
    content_uri = f'content://{PACKAGE}.debugpreview/m2a-preview.md'
    device.view(content_uri, 'text/markdown', 'a7-content.json')
    preview = device.wait_dump('a7-content.json')
    results.check('A7', 'content URI opens a read-only preview',
                  preview.get('externalOpenMode') == 'readonly_stream' and
                  preview.get('readOnlyReason') == 'preview.external' and
                  preview.get('canEdit') is False,
                  {'mode': preview.get('externalOpenMode'),
                   'readOnly': preview.get('readOnlyReason'),
                   'canEdit': preview.get('canEdit')})
    results.check('A7', 'content preview renders its text',
                  'unique-preview-token' in preview.get('displayText', ''),
                  preview.get('displayText', '')[:200])
    results.check('A7', 'content preview keeps the process alive', device.pid() > 0, device.pid())

    recent = ['Start here.md', 'Learning/Learning map.md', 'Learning/Study loop.md',
              'Project/Project workflow.md', 'Markdown field guide.md']
    for index, path in enumerate(recent):
        device.dump(f'a7-recent-{index}.json', note=path, wait_ms=1400)
        time.sleep(0.4)
    shortcuts = device.shell('dumpsys shortcut')
    section_lines: list[str] = []
    in_package = False
    for line in shortcuts.splitlines():
        if 'Package: ' in line and 'UID:' in line:
            in_package = PACKAGE in line
            continue
        if in_package:
            section_lines.append(line)
    block = '\n'.join(section_lines)
    labels = re.findall(r'shortLabel=([^,\n]+)', block)
    count = block.count('ShortcutInfo {')
    results.check('A7', 'dynamic shortcuts are registered', 1 <= count <= 4,
                  {'count': count, 'labels': labels[:6]})
    results.check('A7', 'shortcuts carry recent note labels',
                  any('Learning map' in label or 'Start here' in label or 'Study loop' in label
                      for label in labels), labels[:6])

    before = device.dump('a7-shortcuts.json', wait_ms=600)
    stored = before.get('shortcuts', {}).get('stored', [])
    launched = device.dump('a7-shortcut.json', action='shortcut',
                           extra=['--el', 'debug_index', '0'], wait_ms=2200)
    launch = launched.get('shortcuts', {})
    expected_note = launch.get('lastLaunchNote')
    expected_vault = launch.get('lastLaunchVault')
    stored_paths = {item.get('path') for item in stored}
    results.check('A7', 'shortcut intent opens the stored note',
                  expected_note is not None and expected_note in stored_paths and
                  launched.get('note') == expected_note and
                  launched.get('vault') == expected_vault,
                  {'launch': launch, 'stored': stored[:4], 'note': launched.get('note'),
                   'vault': launched.get('vault')})


def main() -> int:
    parser = argparse.ArgumentParser(description='M2a Android 知识导航与编辑验收（A2–A7）')
    parser.add_argument('--apk', type=Path, default=ROOT / 'build/android/hortus-debug.apk')
    parser.add_argument('--phone-avd', default='Medium_Phone_API_36.1')
    parser.add_argument('--tablet-avd', default='Pixel_Tablet')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/android-m2a')
    parser.add_argument('--media', type=Path, default=None)
    parser.add_argument('--items', default='A2,A3,A4,A5,A6,A7')
    parser.add_argument('--keep-emulator', action='store_true')
    args = parser.parse_args()

    output = args.output.expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.media.expanduser().resolve() if args.media else output / 'media')
    media.mkdir(parents=True, exist_ok=True)
    results = Results(output)
    wanted = {item.strip().upper() for item in args.items.split(',') if item.strip()}
    sdk = find_sdk_root()
    apk = args.apk.expanduser().resolve()

    phone_serial, phone_proc = ensure_emulator(sdk, args.phone_avd, output / 'emulator-phone')
    phone = Device(sdk / 'platform-tools/adb', phone_serial, output, 'phone')
    log(f'手机设备：{phone_serial}')
    phone.install(apk)
    phone.shell(f'pm clear {PACKAGE}', check=False)
    phone.grant()

    stage = Path(tempfile.mkdtemp(prefix='hortus-m2a-vault-'))
    try:
        build_vault(stage)
        initial = {str(path.relative_to(stage)): sha256(path.read_bytes())
                   for path in stage.rglob('*') if path.is_file()}
        copy_vault(phone, stage)
        (output / 'vault-initial.json').write_text(
            json.dumps(initial, ensure_ascii=False, indent=2) + '\n')
        phone.night(False)
        phone.rotate(False)
        if 'A2' in wanted:
            assert_a2(phone, results, media)
        if 'A3' in wanted:
            assert_a3(phone, results, media)
        if 'A5' in wanted:
            assert_a5(phone, results, media)
        if 'A6' in wanted:
            assert_a6(phone, results)
        if 'A7' in wanted:
            assert_a7(phone, results)
    finally:
        shutil.rmtree(stage, ignore_errors=True)

    if 'A4' in wanted:
        tablet_serial, tablet_proc = ensure_emulator(sdk, args.tablet_avd, output / 'emulator-tablet')
        tablet = Device(sdk / 'platform-tools/adb', tablet_serial, output, 'tablet')
        log(f'平板设备：{tablet_serial}')
        tablet.install(apk)
        tablet.shell(f'pm clear {PACKAGE}', check=False)
        tablet.grant()
        tablet.night(False)
        tablet.rotate(True)
        stage = Path(tempfile.mkdtemp(prefix='hortus-m2a-tablet-'))
        try:
            build_vault(stage)
            copy_vault(tablet, stage)
            assert_a4(tablet, results, media)
        finally:
            shutil.rmtree(stage, ignore_errors=True)

    summary = results.summary()
    (output / 'results.json').write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    log('--- 汇总 ---')
    for item in sorted(summary['items']):
        entry = summary['items'][item]
        log(f'{item}: {"PASS" if entry["ok"] else "FAIL"} ({len(entry["checks"])} 项)')
    return 0 if all(entry['ok'] for entry in summary['items'].values()) else 1


if __name__ == '__main__':
    sys.exit(main())
