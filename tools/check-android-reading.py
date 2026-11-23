#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""M1 Android 阅读视图验收：拼装验收库、驱动 Debug 入口、执行 A2–A8 场景。"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import shutil
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import time
import urllib.parse
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = 'io.github.groundgrounder.hortus'
ACTIVITY = f'{PACKAGE}/.MainActivity'
DEVICE_VAULT = '/sdcard/Documents/HortusM1Vault'
REMOTE_DIR = f'/sdcard/Android/data/{PACKAGE}/files'
PUBLIC_MERMAID_DIRS = ['mindmap', 'flowchart', 'sequence', 'state', 'class', 'er']


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


def find_ndk(sdk: Path) -> Path | None:
    for key in ('ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT'):
        value = os.environ.get(key)
        if value and Path(value).is_dir():
            return Path(value)
    candidates = sorted((sdk / 'ndk').glob('*')) if (sdk / 'ndk').is_dir() else []
    candidates += [Path('/opt/homebrew/share/android-ndk')]
    for candidate in candidates:
        if (candidate / 'source.properties').is_file():
            return candidate
    return None


def safe_name(name: str) -> str:
    return re.sub(r'[^A-Za-z0-9_.-]', '_', name)


class Device:
    def __init__(self, adb: Path, serial: str, output: Path):
        self.adb = adb
        self.serial = serial
        self.output = output

    def run(self, *args: str, check: bool = True, binary: bool = False):
        command = [str(self.adb), '-s', self.serial, *args]
        result = subprocess.run(command, capture_output=True, timeout=120)
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

    def grant(self) -> None:
        self.shell(f'appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow')

    def start(self, note: str | None = None, dump: str | None = None,
              export: str | None = None, wait_ms: int = 1200, cold: bool = False,
              extra: list[str] | None = None) -> None:
        if cold:
            self.shell(f'am force-stop {PACKAGE}')
        args = ['am', 'start', '-W', '-n', ACTIVITY]
        if note is not None:
            args += ['--es', 'debug_vault', urllib.parse.quote(DEVICE_VAULT, safe='')]
            args += ['--es', 'debug_note', urllib.parse.quote(note, safe='')]
        if dump:
            args += ['--es', 'debug_dump', dump]
            args += ['--el', 'debug_wait_ms', str(wait_ms)]
        if export:
            args += ['--es', 'debug_export_all', export]
            args += ['--el', 'debug_wait_ms', str(wait_ms)]
        if extra:
            args += extra
        self.run('shell', *args)

    def pull(self, remote: str, local: Path) -> bool:
        result = subprocess.run(
            [str(self.adb), '-s', self.serial, 'pull', remote, str(local)],
            capture_output=True, timeout=120,
        )
        return result.returncode == 0 and local.is_file()

    def dump(self, name: str, note: str | None = None, cold: bool = False,
             wait_ms: int = 1200, timeout: float = 40.0) -> dict:
        name = safe_name(name)
        remote = f'{REMOTE_DIR}/{name}'
        self.shell(f'rm -f {remote}', check=False)
        self.start(note=note, dump=name, wait_ms=wait_ms, cold=cold)
        local = self.output / name
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.shell(f'test -s {remote}', check=False).strip() == '':
                if self.pull(remote, local):
                    return json.loads(local.read_text())
            time.sleep(0.2)
        log(self.shell('logcat -d -t 400', check=False)[-8000:])
        raise RuntimeError(f'等待检查 JSON 超时：{name}')

    def export_all(self, note: str, name: str, wait_ms: int = 1800, timeout: float = 90.0) -> dict:
        name = safe_name(name)
        remote = f'{REMOTE_DIR}/{name}'
        self.shell(f'rm -f {remote}', check=False)
        self.start(note=note, export=name, wait_ms=wait_ms)
        local = self.output / name
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.pull(remote, local):
                return json.loads(local.read_text())
            time.sleep(0.3)
        raise RuntimeError(f'等待导出 JSON 超时：{name}')

    def screenshot(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = subprocess.run(
            [str(self.adb), '-s', self.serial, 'exec-out', 'screencap', '-p'],
            capture_output=True, timeout=60,
        ).stdout
        path.write_bytes(data)

    def tap(self, x: float, y: float) -> None:
        self.shell(f'input tap {int(x)} {int(y)}')
        time.sleep(0.8)

    def key(self, code: int = 4) -> None:
        self.shell(f'input keyevent {code}')
        time.sleep(0.8)

    def night(self, dark: bool) -> None:
        self.shell(f'cmd uimode night {"yes" if dark else "no"}')
        time.sleep(1.2)

    def locale(self, locale: str) -> None:
        self.shell(f'cmd locale set-app-locales {PACKAGE} --locales {locale}', check=False)
        time.sleep(0.6)

    def rotate(self, landscape: bool) -> None:
        self.shell('settings put system accelerometer_rotation 0')
        self.shell(f'settings put system user_rotation {1 if landscape else 0}')
        time.sleep(1.4)

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
    serial = None
    for line in subprocess.run([str(adb), 'devices'], capture_output=True, text=True).stdout.splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 2 and parts[1] == 'device' and parts[0].startswith('emulator-'):
            name = subprocess.run(
                [str(adb), '-s', parts[0], 'emu', 'avd', 'name'],
                capture_output=True, text=True,
            ).stdout.splitlines()
            if name and name[0].strip() == avd:
                serial = parts[0]
                break
    proc = None
    if serial is None:
        log(f'启动模拟器 {avd}…')
        with (output / 'emulator-boot.log').open('w') as log_file:
            proc = subprocess.Popen(
                [str(emulator), '-avd', avd, '-no-window', '-no-boot-anim', '-no-audio',
                 '-no-snapshot-save', '-gpu', 'auto'],
                stdout=log_file, stderr=subprocess.STDOUT,
            )
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        for line in subprocess.run([str(adb), 'devices'], capture_output=True, text=True).stdout.splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2 and parts[1] == 'device' and parts[0].startswith('emulator-'):
                booted = subprocess.run(
                    [str(adb), '-s', parts[0], 'shell', 'getprop', 'sys.boot_completed'],
                    capture_output=True, text=True,
                ).stdout.strip()
                if booted == '1':
                    return parts[0], proc
        if proc is not None and proc.poll() is not None:
            raise RuntimeError('模拟器提前退出，见 emulator-boot.log')
        time.sleep(2)
    raise RuntimeError('等待模拟器启动超时')


def quote_path(path: str) -> str:
    return urllib.parse.quote(path, safe='')


def write_note(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8')


def copy_tree(source: Path, destination: Path) -> None:
    import shutil
    for item in source.iterdir():
        target = destination / item.name
        if item.is_dir():
            shutil.copytree(item, target, dirs_exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(item, target)


def build_vault(stage: Path, output: Path) -> dict:
    demo = ROOT / 'fixtures/demo-vault'
    stage.mkdir(parents=True, exist_ok=True)
    copy_tree(demo, stage)
    math = ROOT / 'fixtures/math'
    (stage / 'Math').mkdir(parents=True, exist_ok=True)
    for item in math.glob('*.md'):
        (stage / 'Math' / item.name).write_bytes(item.read_bytes())

    mermaid = ROOT / 'fixtures/mermaid'
    mmd_files: list[dict] = []
    for directory in PUBLIC_MERMAID_DIRS:
        source_dir = mermaid / directory
        if not source_dir.is_dir():
            continue
        diagrams_dir = stage / 'Mermaid/Diagrams' / directory
        diagrams_dir.mkdir(parents=True, exist_ok=True)
        class_blocks = []
        for mmd in sorted(source_dir.glob('*.mmd')):
            (diagrams_dir / mmd.name).write_bytes(mmd.read_bytes())
            stem = mmd.stem
            note = f'Mermaid/Embed/{directory}__{stem}.md'
            write_note(stage, note, f'# {directory}/{stem}\n\n![[{mmd.name}]]\n')
            code_note = f'Mermaid/Code/{directory}__{stem}.md'
            write_note(stage, code_note, f'# {directory}/{stem} code\n\n```mermaid\n{mmd.read_text()}\n```\n')
            class_blocks.append(mmd.read_text().rstrip())
            mmd_files.append({
                'directory': directory,
                'name': mmd.name,
                'stem': stem,
                'embedNote': note,
                'codeNote': code_note,
                'source': str(mmd),
            })
        if class_blocks:
            write_note(
                stage,
                f'Mermaid/Class {directory}.md',
                f'# {directory}\n\n' + '\n\n'.join(
                    f'```mermaid\n{block}\n```' for block in class_blocks
                ) + '\n',
            )
    all_blocks = []
    for item in mmd_files:
        all_blocks.append((stage / 'Mermaid/Diagrams' / item['directory'] / item['name']).read_text())
    if all_blocks:
        write_note(
            stage,
            'Mermaid/All code blocks.md',
            '# Mermaid fixtures\n\n' + '\n\n'.join(
                f'```mermaid\n{block.rstrip()}\n```' for block in all_blocks
            ) + '\n',
        )

    write_note(stage, 'Synthetic/Refresh.md', '''# Refresh target

Original paragraph that stays visible while the file changes.

A second stable paragraph.

A third stable paragraph.
''')
    write_note(stage, 'Synthetic/Links/Normal link.md', '# Normal link\n\nGo to [[Start here]].\n')
    write_note(stage, 'Synthetic/Links/Alias link.md', '# Alias link\n\nGo to [[Start here|the notebook index]].\n')
    write_note(stage, 'Synthetic/Links/Heading link.md', '# Heading link\n\nGo to [[Start here#This week]].\n')
    write_note(stage, 'Synthetic/Links/Broken link.md', '# Broken link\n\nGo to [[No such note at all]].\n')
    write_note(stage, 'Synthetic/Links/Chain A.md', '# Chain A\n\n[[Chain B]]\n')
    write_note(stage, 'Synthetic/Links/Chain B.md', '# Chain B\n\n[[Chain C]]\n')
    write_note(stage, 'Synthetic/Links/Chain C.md', '# Chain C\n\nEnd of the chain.\n')
    write_note(stage, 'Synthetic/Embeds/Note embed.md', '# Note embed\n\n![[Study loop]]\n')
    write_note(stage, 'Synthetic/Embeds/Diagram embed.md', '# Diagram embed\n\n![[connectors-shapes.mmd]]\n')
    write_note(stage, 'Synthetic/Embeds/Cycle A.md', '# Cycle A\n\n![[Cycle B]]\n')
    write_note(stage, 'Synthetic/Embeds/Cycle B.md', '# Cycle B\n\n![[Cycle A]]\n')
    for index in range(10):
        next_note = f'![[Depth {index + 1}]]\n' if index < 9 else 'Leaf.\n'
        write_note(stage, f'Synthetic/Embeds/Depth {index}.md', f'# Depth {index}\n\n{next_note}')
    write_note(stage, 'Synthetic/Embeds/Heading missing.md', '# Heading missing\n\n![[Study loop#No such heading]]\n')
    write_note(stage, 'Synthetic/Ambiguous/Duplicate.md', '# Duplicate one\n\nfirst\n')
    write_note(stage, 'Synthetic/Ambiguous/Nested/Duplicate.md', '# Duplicate two\n\nsecond\n')
    write_note(stage, 'Synthetic/Embeds/Ambiguous embed.md', '# Ambiguous\n\n![[Duplicate]]\n')
    write_note(stage, 'Synthetic/Embeds/Broken embed.md', '# Broken\n\n![[No such note at all]]\n')
    write_note(stage, 'Synthetic/Embeds/Math embed.md', '# Math embed\n\n![[inline]]\n')
    write_note(stage, 'Synthetic/Diagrams/Interactive.md', '''# Interactive diagram

```mermaid
mindmap
  root((Interactive))
    Branch A
      Leaf 1
      Leaf 2
    [[Start here#This week|Linked node]]
```
''')
    write_note(stage, 'Synthetic/Tables/Math table.md', '''# Math table

| Value | Formula |
| --- | --- |
| square | $x^2$ |
| ratio | $\\frac{a}{b}$ |

| Left | Center | Right |
| :--- | :---: | ---: |
| a | b | c |
''')
    write_note(stage, 'Synthetic/Images/Missing image.md', '# Missing image\n\n![[no-such-image.png]]\n')

    large_lines = ['# Large reading note', '', 'Opening paragraph with $x^2$ and text.', '']
    for index in range(2000):
        if index % 50 == 0:
            large_lines += [
                '```mermaid',
                'flowchart LR',
                f'  Q{index}[Question {index}] --> E{index}[Example {index}]',
                '```',
                '',
            ]
        if index % 10 == 0:
            large_lines += [f'Paragraph {index} with $x_{{{index}}}$ and **bold** text.', '']
        else:
            large_lines += [f'Paragraph {index} plain text about reading.', '']
    write_note(stage, 'Synthetic/Large note.md', '\n'.join(large_lines))

    # 公开语料之外的私人 tree 样本不复制进验收库，避免进入公开证据。
    skipped_tree = (mermaid / 'tree').is_dir()
    manifest = {
        'mmd': mmd_files,
        'skippedPrivateTree': skipped_tree,
        'largeParagraphs': 2000,
    }
    (output / 'vault-manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n')
    return manifest


def build_cli(output: Path) -> Path:
    cargo = Path.home() / '.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo'
    env = dict(os.environ)
    if cargo.is_file():
        env['PATH'] = f'{cargo.parent}:{env.get("PATH", "")}'
    binary = ROOT / 'target/release/hortus'
    if not binary.is_file():
        log('构建宿主机 hortus CLI…')
        subprocess.run(
            ['cargo', 'build', '--locked', '--release', '-p', 'hortus-cli'],
            cwd=ROOT, env=env, check=True, timeout=1800,
        )
    if not binary.is_file():
        raise RuntimeError('未找到 target/release/hortus')
    return binary


def cli_check(binary: Path, file: Path) -> dict:
    result = subprocess.run(
        [str(binary), 'mermaid', 'check', str(file)],
        cwd=ROOT, capture_output=True, text=True, timeout=120,
    )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {'file': str(file), 'valid': False, 'blocks': [], 'error': result.stderr}


def sha_tree(directory: Path) -> dict[str, str]:
    return {
        path.relative_to(directory).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in directory.rglob('*') if path.is_file()
    }


class Results:
    def __init__(self, output: Path):
        self.output = output
        self.items: list[dict] = []

    def check(self, item: str, name: str, ok: bool, detail) -> bool:
        self.items.append({'item': item, 'name': name, 'passed': bool(ok), 'detail': detail})
        if not ok:
            log(f'{item} FAIL: {name}: {detail}')
        return ok

    def passed(self, item: str) -> bool:
        return all(entry['passed'] for entry in self.items if entry['item'] == item)

    def summary(self) -> dict:
        def clean(value):
            if isinstance(value, (bytes, bytearray)):
                return value.decode('utf-8', errors='replace')
            if isinstance(value, dict):
                return {str(key): clean(item) for key, item in value.items()}
            if isinstance(value, (list, tuple)):
                return [clean(item) for item in value]
            if isinstance(value, (str, int, float, bool)) or value is None:
                return value
            return str(value)

        return {
            'items': [{**entry, 'detail': clean(entry['detail'])} for entry in self.items],
            'byItem': {
                item: all(entry['passed'] for entry in self.items if entry['item'] == item)
                for item in sorted({entry['item'] for entry in self.items})
            },
            'passed': all(entry['passed'] for entry in self.items),
        }


def dump_screenshot(device: Device, media: Path, name: str) -> Path:
    path = media / name
    device.screenshot(path)
    return path


def first_interactive(dump: dict, kind: str, predicate) -> dict | None:
    for hit in dump.get('interactive', []):
        if hit.get('kind') == kind and predicate(hit):
            return hit
    return None


def center(hit: dict) -> tuple[float, float]:
    return hit['x'] + hit['width'] / 2, hit['y'] + hit['height'] / 2

FOCUS_ACTIVITY = f'{PACKAGE}/.reader.DiagramFocusActivity'
FOCUS_STATE = f'/sdcard/Android/data/{PACKAGE}/files/focus/focus-state.json'


def top_activity(device: Device) -> str:
    text = device.shell('dumpsys activity activities')
    match = re.search(r'topResumedActivity=ActivityRecord\{\S+ u0 ([\w./]+)', text)
    return match.group(1) if match else ''


def wait_top(device: Device, needle: str, timeout: float = 8.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in top_activity(device):
            return True
        time.sleep(0.25)
    return False


def pull_focus_state(device: Device, name: str = 'focus-state.json', timeout: float = 8.0) -> dict:
    local = device.output / safe_name(name)
    if local.exists():
        local.unlink()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if device.pull(FOCUS_STATE, local):
            try:
                return json.loads(local.read_text())
            except json.JSONDecodeError:
                pass
        time.sleep(0.25)
    raise RuntimeError('等待 focus-state.json 超时')


def ui_nodes(device: Device) -> list[dict]:
    device.shell('uiautomator dump /sdcard/hortus-ui.xml >/dev/null 2>&1')
    raw = device.shell('cat /sdcard/hortus-ui.xml', check=False)
    try:
        root = ET.fromstring(raw)
    except ET.ParseError:
        return []
    nodes = []
    for node in root.iter('node'):
        bounds = node.get('bounds') or ''
        match = re.match(r'\[(\d+),(\d+)\]\[(\d+),(\d+)\]', bounds)
        nodes.append({
            'text': node.get('text') or '',
            'desc': node.get('content-desc') or '',
            'class': node.get('class') or '',
            'bounds': tuple(int(value) for value in match.groups()) if match else None,
        })
    return nodes


def ui_click(device: Device, needles, timeout: float = 8.0, exact: bool = False) -> bool:
    if isinstance(needles, str):
        needles = [needles]
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for node in ui_nodes(device):
            label = node['text'] or node['desc']
            hit = (label == needles[0]) if exact else any(needle in label for needle in needles)
            if hit and node['bounds']:
                x1, y1, x2, y2 = node['bounds']
                device.tap((x1 + x2) / 2, (y1 + y2) / 2)
                return True
        time.sleep(0.4)
    return False


def inline_preview(value: dict, note: str) -> dict | None:
    for preview in value.get('inlinePreviews', []):
        if note in preview.get('pictureKey', preview.get('key', '')):
            return preview
    return None


def inline_rect(value: dict, ordinal: int) -> dict | None:
    suffix = f':{ordinal}'
    for rect in value.get('diagramRects', []):
        if rect.get('key', '').endswith(suffix):
            return rect
    return None


def focus_screen_point(hit: dict, state: dict) -> tuple[float, float]:
    return (
        hit['x'] * state['scale'] + state['offsetX'] + hit['width'] * state['scale'] / 2,
        hit['y'] * state['scale'] + state['offsetY'] + hit['height'] * state['scale'] / 2,
    )


def focus_menu(device: Device) -> bool:
    if ui_click(device, ['图表选项', 'Diagram options'], timeout=3):
        return True
    width, _, _ = device.screen()
    device.tap(width - 55, 105)
    return True


def open_focus_from_inline(device: Device, note: str, ordinal: int = 1,
                           dump_name: str = 'focus-open.json') -> dict:
    value = device.dump(dump_name, note=note, wait_ms=1600)
    rect = inline_rect(value, ordinal)
    if rect is None:
        raise RuntimeError(f'没有找到正文图位置：{note}')
    device.tap(rect['x'] + rect['width'] / 2, rect['y'] + rect['height'] / 2)
    if not wait_top(device, FOCUS_ACTIVITY):
        raise RuntimeError('点击正文图后没有进入全屏页')
    return pull_focus_state(device, f'{dump_name}.focus-state.json')


def assert_a2(device: Device, results: Results, media: Path) -> None:
    field_guide = 'Markdown field guide.md'
    fg = device.dump('a2-field-guide.json', note=field_guide, cold=True, wait_ms=1800)
    elements = fg.get('elements', {})
    item_types = fg.get('itemTypes', {})
    results.check('A2', 'field guide opened', fg.get('note') == field_guide and not fg.get('error'), fg.get('error'))
    results.check('A2', 'headings present', bool(elements.get('headings')), elements.get('headings'))
    for masked in ('strong', 'emphasis', 'strike', 'highlight', 'inlineCode', 'codeBlock', 'quote', 'task', 'rule'):
        results.check('A2', f'{masked} mask present', elements.get(masked, 0) > 0, elements.get(masked))
    results.check('A2', 'callout note+warning', {'note', 'warning'}.issubset({c['kind'] for c in elements.get('callouts', [])}),
                  elements.get('callouts'))
    results.check('A2', 'table present', item_types.get('table', 0) >= 1, item_types)
    table = (fg.get('tables') or [{}])[0]
    results.check('A2', 'three column alignments', sorted(table.get('alignments', [])) == [0, 1, 2], table.get('alignments'))
    results.check('A2', 'math block and inline', fg.get('mathStatus', {}).get('rendered', 0) >= 2, fg.get('mathStatus'))
    results.check('A2', 'image decoded', any(image.get('decoded') for image in fg.get('images', [])), fg.get('images'))
    results.check('A2', 'note embed present', item_types.get('embed', 0) >= 1, item_types)
    results.check('A2', 'mermaid diagram present', item_types.get('diagram', 0) >= 1, item_types)
    results.check('A2', 'no source markers in display text', all(value == 0 for value in fg.get('displayMarkers', {}).values()),
                  fg.get('displayMarkers'))
    results.check('A2', 'no source fallback', fg.get('sourceFallbackCount', 1) == 0, fg.get('sourceFallbackCount'))
    results.check('A2', 'link present', elements.get('link', 0) > 0, elements.get('link'))

    before_id = fg.get('firstVisibleBlockId')
    before_offset = fg.get('firstVisibleOffsetPx', 0)
    _, _, density = device.screen()
    device.rotate(True)
    land = device.dump('a2-field-guide-landscape.json', wait_ms=1500)
    device.rotate(False)
    portrait = device.dump('a2-field-guide-portrait.json', wait_ms=1500)
    for name, value in (('landscape', land), ('portrait', portrait)):
        results.check('A2', f'rotation {name} first block stable', value.get('firstVisibleBlockId') == before_id,
                      (before_id, value.get('firstVisibleBlockId')))
        results.check('A2', f'rotation {name} offset <= 24dp',
                      abs(value.get('firstVisibleOffsetPx', 0) - before_offset) <= 24 * density,
                      (before_offset, value.get('firstVisibleOffsetPx')))
    device.night(True)
    dark = device.dump('a2-field-guide-dark.json', wait_ms=1500)
    results.check('A2', 'theme first block stable', dark.get('firstVisibleBlockId') == before_id,
                  (before_id, dark.get('firstVisibleBlockId')))
    results.check('A2', 'theme offset <= 24dp', abs(dark.get('firstVisibleOffsetPx', 0) - before_offset) <= 24 * density,
                  (before_offset, dark.get('firstVisibleOffsetPx')))
    device.night(False)

    for locale, language in (('en-US', 'en'), ('zh-CN', 'zh')):
        device.locale(locale)
        for dark_mode, shade in ((False, 'light'), (True, 'dark')):
            device.night(dark_mode)
            device.dump(f'a2-shot-{language}-{shade}.json', note=field_guide, cold=True, wait_ms=1200)
            dump_screenshot(device, media, f'a2-field-guide-{language}-{shade}.png')
    device.locale('en-US')
    device.night(False)

    refresh_note = 'Synthetic/Refresh.md'
    refresh_before = device.dump('a2-refresh-before.json', note=refresh_note, cold=True, wait_ms=900)
    changed = '''# Refresh target

Externally changed paragraph that must appear after the sweep.

A second stable paragraph.

A third stable paragraph.
'''
    local_changed = results.output / 'refresh-changed.md'
    local_changed.write_text(changed)
    device.run('push', str(local_changed), f'{DEVICE_VAULT}/{refresh_note}')
    time.sleep(2.5)
    refresh_after = device.dump('a2-refresh-after.json', wait_ms=1200)
    results.check('A2', 'external change visible',
                  'Externally changed paragraph' in refresh_after.get('displayText', ''),
                  refresh_after.get('displayText', '')[:200])
    results.check('A2', 'external change keeps first block',
                  refresh_after.get('firstVisibleBlockId') == refresh_before.get('firstVisibleBlockId'),
                  (refresh_before.get('firstVisibleBlockId'), refresh_after.get('firstVisibleBlockId')))
    # 恢复 Refresh.md 的原始内容。
    (results.output / 'refresh-original.md').write_text('''# Refresh target

Original paragraph that stays visible while the file changes.

A second stable paragraph.

A third stable paragraph.
''')
    device.run('push', str(results.output / 'refresh-original.md'), f'{DEVICE_VAULT}/{refresh_note}')


def assert_a3(device: Device, results: Results, media: Path, manifest: dict, cli: Path) -> None:
    cli_results = []
    for entry in manifest['mmd']:
        report = cli_check(cli, Path(entry['source']))
        entry['valid'] = bool(report.get('valid'))
        entry['cli'] = report
        cli_results.append({'name': entry['name'], 'valid': entry['valid'], 'blocks': report.get('blocks')})
    (results.output / 'a3-host-cli.json').write_text(json.dumps(cli_results, ensure_ascii=False, indent=2) + '\n')

    accum = {}
    for entry in manifest['mmd']:
        for kind, note in (('embed', entry['embedNote']), ('code', entry['codeNote'])):
            name = f'a3-{kind}-{entry["directory"]}-{entry["stem"]}.json'
            value = device.dump(name, note=note, wait_ms=1500)
            expected_key = note if kind == 'code' else f'{entry["directory"]}/{entry["name"]}'
            records = [
                record for record in value.get('diagrams', [])
                if expected_key in record.get('key', '') or
                    (kind == 'embed' and entry['name'] in record.get('key', ''))
            ]
            failures = {
                item.get('key'): item.get('reason')
                for item in value.get('diagramFailures', [])
                if expected_key in item.get('key', '') or
                    (kind == 'embed' and entry['name'] in item.get('key', ''))
            }
            if entry['valid']:
                rendered = [
                    record for record in records
                    if record.get('width', 0) > 0 and record.get('height', 0) > 0
                ]
                results.check('A3', f'{entry["name"]} {kind} rendered', bool(rendered),
                              {'records': len(records), 'failures': failures, 'items': value.get('diagramItems')})
                results.check('A3', f'{entry["name"]} {kind} no failure', not failures, failures)
            else:
                results.check('A3', f'{entry["name"]} {kind} shows source and reason', bool(failures), failures)
            for record in records:
                accum[record['key']] = record
    # 全量打开后再取一次累计计数（同一进程内累加）。
    cumulative = device.dump('a3-cumulative.json', note='Mermaid/All code blocks.md', wait_ms=2500)
    counts = cumulative.get('diagramCounts', {})
    for key in ('RoundedRect', 'Line', 'GraphPath', 'GraphShape', 'LabelBackground', 'Text'):
        results.check('A3', f'primitive {key}', counts.get(key, 0) > 0, counts.get(key))
    for key in ('Arrow', 'Circle', 'Cross'):
        results.check('A3', f'marker {key}', counts.get(key, 0) > 0, counts.get(key))
    for key in ('Solid', 'Dotted', 'Thick'):
        results.check('A3', f'line style {key}', counts.get(key, 0) > 0, counts.get(key))
    results.check('A3', 'text width samples', counts.get('textSamples', 0) > 0, counts)
    results.check('A3', 'text width error <= 0.10 px', counts.get('maxTextWidthErrorPx', 9) <= 0.10, counts)
    preview_counts = cumulative.get('diagramPreview', {}).get('counts', {})
    results.check('A3', 'inline preview primitives drawn',
                  preview_counts.get('Text', 0) > 0 and preview_counts.get('RoundedRect', 0) > 0,
                  preview_counts)
    results.check('A3', 'inline preview text width error <= 0.10 px',
                  preview_counts.get('textSamples', 0) > 0 and
                  preview_counts.get('maxTextWidthErrorPx', 9) <= 0.10, preview_counts)
    results.check('A3', 'inline preview has no overlay controls',
                  cumulative.get('inlineOverlayControls') == 0, cumulative.get('inlineOverlayControls'))
    (results.output / 'a3-scene-counts.json').write_text(json.dumps({
        'diagramCounts': counts,
        'records': list(accum.values()),
        'cli': cli_results,
    }, ensure_ascii=False, indent=2) + '\n')

    for directory in PUBLIC_MERMAID_DIRS:
        note = f'Mermaid/Class {directory}.md'
        device.dump(f'a3-class-{directory}-light.json', note=note, wait_ms=1400)
        dump_screenshot(device, media, f'a3-{directory}-light.png')
        device.night(True)
        device.dump(f'a3-class-{directory}-dark.json', note=note, wait_ms=1400)
        dump_screenshot(device, media, f'a3-{directory}-dark.png')
        device.night(False)


def assert_a4(device: Device, results: Results, baseline: dict) -> None:
    note = 'Synthetic/Diagrams/Interactive.md'
    before = device.dump('a4-before.json', note=note, cold=True, wait_ms=1600)
    rect = inline_rect(before, 1)
    results.check('A4', 'inline preview visible', rect is not None and rect.get('width', 0) > 0,
                  before.get('diagramRects'))
    results.check('A4', 'inline preview has no overlay controls',
                  before.get('inlineOverlayControls') == 0, before.get('inlineOverlayControls'))
    if rect is None:
        return
    device.tap(rect['x'] + rect['width'] / 2, rect['y'] + rect['height'] / 2)
    entered = wait_top(device, FOCUS_ACTIVITY)
    results.check('A4', 'tap on inline preview opens focus', entered, top_activity(device))
    if not entered:
        return
    state = pull_focus_state(device, 'a4-focus-before.json')
    results.check('A4', 'focus shows the same note', state.get('notePath') == note, state.get('notePath'))
    collapsible = [hit for hit in state.get('hits', []) if hit.get('canCollapse') and hit.get('nodeId')]
    node = next((hit for hit in collapsible if 'Interactive' not in hit.get('label', '')), None)
    node = node or (collapsible[0] if collapsible else None)
    results.check('A4', 'collapsible node visible in focus', node is not None, state.get('hits'))
    if node is None:
        return
    before_visible = state.get('visibleNodes', -1)
    x, y = focus_screen_point(node, state)
    device.tap(x, y)
    time.sleep(1.6)
    after = pull_focus_state(device, 'a4-focus-after.json')
    results.check('A4', '+N label after collapse', after.get('plusLabels', 0) > 0, after.get('plusLabels'))
    results.check('A4', 'visible nodes decreased', 0 <= after.get('visibleNodes', 10 ** 9) < before_visible,
                  (before_visible, after.get('visibleNodes')))
    results.check('A4', 'collapse stored in record', node['nodeId'] in after.get('collapsed', []),
                  after.get('collapsed'))
    device.key(4)
    if not wait_top(device, PACKAGE):
        results.check('A4', 'back returns to reader', False, top_activity(device))
        return
    inline = device.dump('a4-inline-collapsed.json', wait_ms=1400)
    preview = inline_preview(inline, note)
    results.check('A4', 'inline preview reflects collapse',
                  preview is not None and node['nodeId'] in preview.get('collapsed', []) and
                  preview.get('hits') == after.get('visibleNodes'),
                  preview)
    reopened = device.dump('a4-reopened.json', note=note, cold=True, wait_ms=1800)
    reopened_preview = inline_preview(reopened, note)
    results.check('A4', 'collapse persists after reopen',
                  reopened_preview is not None and node['nodeId'] in reopened_preview.get('collapsed', []),
                  reopened_preview)
    diagrams = [record for record in reopened.get('diagrams', []) if 'Interactive' in record.get('key', '')]
    results.check('A4', 'persisted scene keeps collapse',
                  bool(diagrams) and node['nodeId'] in diagrams[-1].get('collapsed', []), diagrams)

    before_block = inline.get('firstVisibleBlockId')
    reopened_rect = inline_rect(reopened, 1)
    if reopened_rect is None:
        results.check('A4', 'reopened preview visible', False, reopened.get('diagramRects'))
        return
    device.tap(reopened_rect['x'] + reopened_rect['width'] / 2,
               reopened_rect['y'] + reopened_rect['height'] / 2)
    if not wait_top(device, FOCUS_ACTIVITY):
        results.check('A4', 'focus reopened for link', False, top_activity(device))
        return
    link_state = pull_focus_state(device, 'a4-focus-link.json')
    link = next((hit for hit in link_state.get('hits', []) if hit.get('target')), None)
    if link is None and node is not None:
        # 之前折叠的节点可能遮住链接节点；先展开再找。
        collapsed_node = next(
            (hit for hit in link_state.get('hits', []) if hit.get('nodeId') == node.get('nodeId')),
            node,
        )
        x, y = focus_screen_point(collapsed_node, link_state)
        device.tap(x, y)
        time.sleep(1.6)
        link_state = pull_focus_state(device, 'a4-focus-link-expanded.json')
        link = next((hit for hit in link_state.get('hits', []) if hit.get('target')), None)
    results.check('A4', 'linked node visible in focus', link is not None, link_state.get('hits'))
    if link is not None:
        x, y = focus_screen_point(link, link_state)
        device.tap(x, y)
        if not wait_top(device, PACKAGE):
            results.check('A4', 'node link opens target activity', False, top_activity(device))
            return
        opened = device.dump('a4-link-opened.json', wait_ms=1200)
        results.check('A4', 'node link opens target note', opened.get('note') == 'Start here.md',
                      opened.get('note'))
        device.key(4)
        results.check('A4', 'first back returns to focus', wait_top(device, FOCUS_ACTIVITY),
                      top_activity(device))
        device.key(4)
        results.check('A4', 'second back returns to source note', wait_top(device, PACKAGE),
                      top_activity(device))
        returned = device.dump('a4-link-returned.json', wait_ms=1400)
        results.check('A4', 'back returns to source note', returned.get('note') == note, returned.get('note'))
        results.check('A4', 'back restores first block', returned.get('firstVisibleBlockId') == before_block,
                      (before_block, returned.get('firstVisibleBlockId')))

    pulled = results.output / 'a4-pulled-vault'
    import shutil
    shutil.rmtree(pulled, ignore_errors=True)
    subprocess.run(
        [str(device.adb), '-s', device.serial, 'pull', DEVICE_VAULT, str(pulled)],
        capture_output=True, timeout=300,
    )
    after_hash = sha_tree(pulled)
    results.check('A4', 'vault unchanged', after_hash == baseline, {
        'added': sorted(set(after_hash) - set(baseline)),
        'removed': sorted(set(baseline) - set(after_hash)),
        'changed': sorted(key for key in baseline if key in after_hash and baseline[key] != after_hash[key]),
    })


def assert_a5(device: Device, results: Results, media: Path) -> None:
    math_files = sorted((ROOT / 'fixtures/math').glob('*.md'))
    ratio_samples = []
    baseline_diffs = []
    overlaps = []
    block_gaps = []
    block_heights = []
    table_diffs = []
    bad_reasons = []
    seen_layouts = 0
    seen_formulas = 0
    for file in math_files:
        note = f'Math/{file.name}'
        value = device.dump(f'a5-{file.stem}.json', note=note, cold=True, wait_ms=2200)
        status = value.get('mathStatus', {})
        total = status.get('rendered', 0) + status.get('failed', 0) + status.get('empty', 0)
        results.check('A5', f'{file.name} formulas seen', total >= 0, status)
        for formula in value.get('math', []):
            if formula.get('note') == note and formula.get('emRatio') is not None:
                ratio_samples.append(formula['emRatio'])
            reason = formula.get('reason') or ''
            if any(bad in reason for bad in ('io.github.', 'MathFailure', 'detail=')):
                bad_reasons.append(reason)
        if file.name == 'showcase.md':
            failed_formulas = [
                formula for formula in value.get('math', [])
                if formula.get('note') == note and not formula.get('rendered')
                and formula.get('reason') not in (None, '')
            ]
            results.check('A5', 'invalid formulas show source and reason',
                          status.get('failed', 0) > 0 and
                          len(failed_formulas) == status.get('failed', 0) and
                          all(formula.get('reason') for formula in failed_formulas),
                          {'status': status, 'failed': failed_formulas})
            for node in ui_nodes(device):
                label = node['text'] or node['desc']
                if 'detail=' in label and 'io.github.' in label:
                    bad_reasons.append(label)
        if file.name == 'missing-glyphs.md':
            results.check('A5', 'fallback metrics returned', value.get('mathMetricsCodepoints', 0) > 0,
                          value.get('mathMetricsCodepoints'))
        for layout in value.get('textLayouts', []):
            if layout.get('note') != note:
                continue
            seen_layouts += 1
            lines = {line['line']: line for line in layout.get('lines', [])}
            for box in layout.get('formulas', []):
                if not box.get('rendered'):
                    continue
                seen_formulas += 1
                line = lines.get(box.get('line'))
                if line is not None:
                    baseline_diffs.append(abs(box['baselineY'] - line['baselineY']))
                for index, other in lines.items():
                    if index == box.get('line'):
                        continue
                    if box['top'] < other['bottom'] - 0.01 and box['bottom'] > other['top'] + 0.01:
                        overlaps.append({'key': layout.get('key'), 'line': box.get('line'), 'other': index})
        for table in value.get('tableLayouts', []):
            if table.get('note') != note:
                continue
            for cell in table.get('cells', []):
                lines = {line['line']: line for line in cell.get('lines', [])}
                for box in cell.get('formulas', []):
                    if not box.get('rendered'):
                        continue
                    seen_formulas += 1
                    line = lines.get(box.get('line'))
                    if line is not None:
                        table_diffs.append(abs(box['baselineY'] - line['baselineY']))
        for block in value.get('blockMath', []):
            if block.get('note') != note:
                continue
            for formula in block.get('formulas', []):
                if formula.get('rendered'):
                    block_gaps.append(formula.get('gapEm', 0))
                    ascent = formula.get('ascent') or 0
                    descent = formula.get('descent') or 0
                    block_heights.append(abs((formula.get('boxHeight') or 0) - (ascent + descent)))

    field_guide = device.dump('a5-field-guide.json', note='Markdown field guide.md', wait_ms=2200)
    for layout in field_guide.get('textLayouts', []):
        if layout.get('note') != 'Markdown field guide.md':
            continue
        lines = {line['line']: line for line in layout.get('lines', [])}
        for box in layout.get('formulas', []):
            if not box.get('rendered'):
                continue
            seen_formulas += 1
            line = lines.get(box.get('line'))
            if line is not None:
                baseline_diffs.append(abs(box['baselineY'] - line['baselineY']))
    large = device.dump('a5-large.json', note='Synthetic/Large note.md', wait_ms=2200)
    for layout in large.get('textLayouts', []):
        if layout.get('note') != 'Synthetic/Large note.md':
            continue
        lines = {line['line']: line for line in layout.get('lines', [])}
        for box in layout.get('formulas', []):
            if not box.get('rendered'):
                continue
            seen_formulas += 1
            line = lines.get(box.get('line'))
            if line is not None:
                baseline_diffs.append(abs(box['baselineY'] - line['baselineY']))
    for formula in large.get('math', []):
        reason = formula.get('reason') or ''
        if any(bad in reason for bad in ('io.github.', 'MathFailure', 'detail=')):
            bad_reasons.append(reason)

    table = device.dump('a5-table.json', note='Synthetic/Tables/Math table.md', wait_ms=2000)
    for layout in table.get('tableLayouts', []):
        for cell in layout.get('cells', []):
            lines = {line['line']: line for line in cell.get('lines', [])}
            for box in cell.get('formulas', []):
                if not box.get('rendered'):
                    continue
                seen_formulas += 1
                line = lines.get(box.get('line'))
                if line is not None:
                    table_diffs.append(abs(box['baselineY'] - line['baselineY']))
    table_math = [cell for row in table.get('tables', []) for cell in row.get('cells', []) if cell.get('math')]
    results.check('A5', 'table cell formula present', bool(table_math), table.get('tables'))
    results.check('A5', 'table cell formula rendered', table.get('mathStatus', {}).get('rendered', 0) > 0,
                  table.get('mathStatus'))
    embed = device.dump('a5-embed.json', note='Synthetic/Embeds/Math embed.md', wait_ms=1800)
    results.check('A5', 'embedded formula rendered', embed.get('mathStatus', {}).get('rendered', 0) > 0,
                  embed.get('mathStatus'))

    max_baseline = max(baseline_diffs) if baseline_diffs else 0.0
    max_table = max(table_diffs) if table_diffs else 0.0
    results.check('A5', 'inline formula baseline <= 1 px', bool(baseline_diffs) and max_baseline <= 1.0,
                  {'samples': len(baseline_diffs), 'max': max_baseline})
    results.check('A5', 'inline formula boxes do not overlap other lines', not overlaps,
                  {'samples': seen_formulas, 'overlaps': overlaps[:5]})
    results.check('A5', 'block formula spacing >= 0.5 em',
                  bool(block_gaps) and min(block_gaps) >= 0.5 - 0.001,
                  {'samples': len(block_gaps), 'min': min(block_gaps) if block_gaps else None})
    results.check('A5', 'block formula height covers ascent + descent', not block_heights or max(block_heights) <= 0.5,
                  {'samples': len(block_heights), 'max': max(block_heights) if block_heights else None})
    results.check('A5', 'table formula baseline <= 1 px', bool(table_diffs) and max_table <= 1.0,
                  {'samples': len(table_diffs), 'max': max_table})
    results.check('A5', 'math errors expose no class names', not bad_reasons, bad_reasons[:5])
    results.check('A5', 'inline ratio 1.21', bool(ratio_samples) and
                  all(abs(value - 1.21) <= 0.01 for value in ratio_samples),
                  {'samples': len(ratio_samples), 'min': min(ratio_samples) if ratio_samples else None,
                   'max': max(ratio_samples) if ratio_samples else None})
    (results.output / 'a5-layout.json').write_text(json.dumps({
        'seenLayouts': seen_layouts,
        'seenFormulas': seen_formulas,
        'inlineBaselineSamples': len(baseline_diffs),
        'inlineBaselineMaxPx': max_baseline,
        'tableBaselineSamples': len(table_diffs),
        'tableBaselineMaxPx': max_table,
        'blockGapSamples': len(block_gaps),
        'blockGapMinEm': min(block_gaps) if block_gaps else None,
        'overlaps': overlaps,
        'badReasons': bad_reasons,
    }, ensure_ascii=False, indent=2) + '\n')
    for note, name in (
        ('Math/showcase.md', 'a5-block.png'),
        ('Math/inline.md', 'a5-inline.png'),
        ('Math/inline-table-context.md', 'a5-table.png'),
    ):
        device.dump(f'a5-shot-{name}.json', note=note, wait_ms=1400)
        dump_screenshot(device, media, name)


def assert_a6(device: Device, results: Results) -> None:
    cases = [
        ('Synthetic/Links/Normal link.md', 'Start here.md'),
        ('Synthetic/Links/Alias link.md', 'Start here.md'),
        ('Synthetic/Links/Heading link.md', 'Start here.md'),
    ]
    for note, target in cases:
        value = device.dump(f'a6-{Path(note).stem}.json', note=note, cold=True, wait_ms=1200)
        hit = first_interactive(value, 'link', lambda item: bool(item.get('target')))
        results.check('A6', f'{Path(note).stem} link visible', hit is not None, value.get('interactive'))
        if hit:
            x, y = center(hit)
            device.tap(x, y)
            opened = device.dump(f'a6-{Path(note).stem}-opened.json', wait_ms=1200)
            results.check('A6', f'{Path(note).stem} opens target', opened.get('note') == target, opened.get('note'))
            if 'Heading' in note:
                headings = [item for item in opened.get('outline', []) if item.get('title') == 'This week']
                first = opened.get('firstVisibleBlockId')
                source = None
                for item in opened.get('items', []):
                    if item.get('blockId') == first:
                        source = item.get('sourceStart')
                        break
                heading_start = int(headings[0].get('start')) if headings else 10**18
                results.check('A6', 'heading link lands on heading',
                              bool(headings) and source is not None and
                              heading_start - 4 <= source <= heading_start + 200,
                              {'headings': headings, 'firstBlock': first, 'source': source})
                _, height, _ = device.screen()
                results.check('A6', 'heading near top', opened.get('firstVisibleOffsetPx', 10**9) <= height * 0.25,
                              opened.get('firstVisibleOffsetPx'))

    broken = device.dump('a6-broken.json', note='Synthetic/Links/Broken link.md', cold=True, wait_ms=1200)
    hit = first_interactive(broken, 'link', lambda item: bool(item.get('target')))
    if hit:
        device.tap(*center(hit))
        after = device.dump('a6-broken-after.json', wait_ms=800)
        results.check('A6', 'broken link shows notice', after.get('lastAction', '').startswith('link_missing'),
                      after.get('lastAction'))
        results.check('A6', 'process survives broken link', device.pid() > 0, device.pid())

    embeds = [
        ('Synthetic/Embeds/Note embed.md', 'note'),
        ('Synthetic/Embeds/Diagram embed.md', 'diagram'),
        ('Synthetic/Embeds/Cycle A.md', 'cycle'),
        ('Synthetic/Embeds/Depth 0.md', 'depth_exceeded'),
        ('Synthetic/Embeds/Heading missing.md', 'heading_missing'),
        ('Synthetic/Embeds/Ambiguous embed.md', 'ambiguous'),
        ('Synthetic/Embeds/Broken embed.md', 'broken'),
    ]
    for note, kind in embeds:
        value = device.dump(f'a6-embed-{Path(note).stem}.json', note=note, cold=True, wait_ms=2000)
        kinds = {item.get('kind') for item in value.get('embeds', [])}
        results.check('A6', f'embed {kind}', kind in kinds, {'kinds': sorted(kinds), 'embeds': value.get('embeds')})

    device.dump('a6-chain-a.json', note='Synthetic/Links/Chain A.md', cold=True, wait_ms=1200)
    value = device.dump('a6-chain-a.json', wait_ms=900)
    hit = first_interactive(value, 'link', lambda item: bool(item.get('target')))
    if hit:
        device.tap(*center(hit))
    value = device.dump('a6-chain-b.json', wait_ms=900)
    results.check('A6', 'chain A opens B', value.get('note') == 'Synthetic/Links/Chain B.md', value.get('note'))
    hit = first_interactive(value, 'link', lambda item: bool(item.get('target')))
    if hit:
        device.tap(*center(hit))
    value = device.dump('a6-chain-c.json', wait_ms=900)
    results.check('A6', 'chain B opens C', value.get('note') == 'Synthetic/Links/Chain C.md', value.get('note'))
    device.key(4)
    value = device.dump('a6-chain-back-b.json', wait_ms=900)
    results.check('A6', 'first back returns B', value.get('note') == 'Synthetic/Links/Chain B.md', value.get('note'))
    device.key(4)
    value = device.dump('a6-chain-back-a.json', wait_ms=900)
    results.check('A6', 'second back returns A', value.get('note') == 'Synthetic/Links/Chain A.md', value.get('note'))


def pull_run_as(device: Device, remote: str, local: Path) -> bool:
    result = subprocess.run(
        [str(device.adb), '-s', device.serial, 'exec-out', 'run-as', PACKAGE, 'cat', remote],
        capture_output=True, timeout=120,
    )
    if result.returncode != 0 or not result.stdout:
        return False
    local.parent.mkdir(parents=True, exist_ok=True)
    local.write_bytes(result.stdout)
    return True


def assert_a7(device: Device, results: Results, media: Path) -> None:
    instrument = results.output / 'instrument-focus-400.json'
    if instrument.is_file():
        data = json.loads(instrument.read_text())
        results.check('A7', 'instrument scale is exactly 400%',
                      abs(data.get('scale', 0) - 4.0) <= 0.01, data)
        results.check('A7', 'instrument drawn font size = scene × 4',
                      data.get('sceneFontSize', 0) > 0 and
                      abs(data.get('drawnFontSize', 0) - data.get('sceneFontSize', 0) * 4) <= 0.05,
                      data)
    else:
        results.check('A7', 'instrument focus JSON present', False, str(instrument))
    restored = results.output / 'instrument-focus-restored.json'
    if restored.is_file():
        data = json.loads(restored.read_text())
        results.check('A7', 'restored scale is 400%', abs(data.get('scale', 0) - 4.0) <= 0.02, data)
    else:
        results.check('A7', 'restored focus JSON present', False, str(restored))
    cache_report = results.output / 'instrument-share-cache.json'
    if cache_report.is_file():
        data = json.loads(cache_report.read_text())
        results.check('A7', 'share cache bounded',
                      data.get('bytes', 10 ** 18) <= data.get('maxBytes', 0) and
                      data.get('files', 10 ** 9) <= data.get('maxFiles', 0), data)
        results.check('A7', 'share cache content URI matches bytes',
                      data.get('contentMatches') is True, data)
    else:
        results.check('A7', 'share cache JSON present', False, str(cache_report))

    note = 'Synthetic/Diagrams/Interactive.md'
    state = open_focus_from_inline(device, note, dump_name='a7-focus-open.json')
    results.check('A7', 'focus state has scene font', state.get('sceneFontSize', 0) > 0, state)
    share_mimes = {}
    for label, fmt, mime in (
        ('PNG', 'png', 'image/png'),
        ('SVG', 'svg', 'image/svg+xml'),
        ('PDF', 'pdf', 'application/pdf'),
    ):
        if not wait_top(device, FOCUS_ACTIVITY):
            results.check('A7', f'share {label} focus active', False, top_activity(device))
            break
        opened = focus_menu(device)
        results.check('A7', f'share {label} menu opens', opened, '')
        clicked = ui_click(device, [f'分享 {label}', f'Share {label}'], timeout=5)
        results.check('A7', f'share {label} item clicked', clicked, '')
        appeared = wait_top(device, 'ChooserActivity', timeout=10)
        results.check('A7', f'{label} share panel appears', appeared, top_activity(device))
        if appeared:
            dump_screenshot(device, media, f'a7-share-{fmt}.png')
            device.key(4)
            time.sleep(0.8)
        share_mimes[mime] = appeared

    if wait_top(device, FOCUS_ACTIVITY):
        device.key(4)
    wait_top(device, PACKAGE)
    shared = device.dump('a7-shares.json', wait_ms=1200)
    records = [record for record in shared.get('shares', []) if record.get('name')]
    results.check('A7', 'three share intents recorded', len(records) >= 3, records)
    results.check('A7', 'share actions are ACTION_SEND',
                  all(record.get('action') == 'android.intent.action.SEND' for record in records), records)
    results.check('A7', 'share mimes cover png svg pdf',
                  {record.get('mime') for record in records} >= {'image/png', 'image/svg+xml', 'application/pdf'},
                  [record.get('mime') for record in records])
    results.check('A7', 'share URIs are FileProvider content URIs',
                  all(str(record.get('uri', '')).startswith('content://') and 'fileprovider' in record.get('uri', '')
                      for record in records), [record.get('uri') for record in records])
    results.check('A7', 'content URI bytes equal encoded bytes',
                  all(record.get('contentMatches') is True for record in records), records)
    focus_counts = shared.get('focusDrawCounts', {})
    results.check('A7', 'focus scene drawn with text',
                  focus_counts.get('Text', 0) > 0 and focus_counts.get('textSamples', 0) > 0, focus_counts)
    results.check('A7', 'focus text width error <= 0.10 px',
                  focus_counts.get('maxTextWidthErrorPx', 9) <= 0.10, focus_counts)

    exports_report = device.export_all(note, 'a7-debug-exports.json', wait_ms=2000)
    exports = [entry for entry in exports_report.get('exports', []) if not entry.get('error') and entry.get('name')]
    for record in records:
        name = record.get('name', '')
        export = next((entry for entry in exports if entry.get('name') == name), None)
        share_local = results.output / f'share-{name}'
        if export is None or not pull_run_as(device, f'cache/diagram-exports/{name}', share_local):
            results.check('A7', f'compare share bytes {name}', False, {'export': export, 'share': str(share_local)})
            continue
        debug_local = media / name
        device.pull(export.get('path', ''), debug_local)
        same = debug_local.is_file() and debug_local.stat().st_size == share_local.stat().st_size and \
            debug_local.read_bytes() == share_local.read_bytes()
        results.check('A7', f'share bytes equal no-dialog export {name}', same, {
            'share': share_local.stat().st_size,
            'debug': debug_local.stat().st_size if debug_local.is_file() else None,
        })

    device.shell('rm -f /sdcard/Download/Interactive-diagram-1.pdf')
    state = open_focus_from_inline(device, note, dump_name='a7-save-open.json')
    focus_menu(device)
    clicked = ui_click(device, ['保存 PDF…', 'Save PDF…'], timeout=5)
    results.check('A7', 'save PDF menu item clicked', clicked, '')
    picker = wait_top(device, 'documentsui', timeout=10)
    results.check('A7', 'ACTION_CREATE_DOCUMENT picker appears', picker, top_activity(device))
    if picker:
        picker_nodes = ui_nodes(device)
        results.check('A7', 'default PDF name shown in picker',
                      any('Interactive-diagram-1.pdf' in (node['text'] or node['desc']) for node in picker_nodes),
                      [node['text'] for node in picker_nodes if node['text']][:12])
        ui_click(device, ['保存', 'Save'], exact=True, timeout=5)
        deadline = time.monotonic() + 15
        saved = results.output / 'Interactive-diagram-1.pdf'
        while time.monotonic() < deadline:
            if device.pull('/sdcard/Download/Interactive-diagram-1.pdf', saved) and saved.stat().st_size > 0:
                break
            time.sleep(0.4)
        if saved.is_file() and saved.stat().st_size > 0:
            data = saved.read_bytes()
            text = data.decode('latin-1', errors='replace')
            pages = len(re.findall(r'/Type\s*/Page\b', text))
            media_box = re.search(r'/MediaBox\s*\[\s*([-0-9.]+)\s+([-0-9.]+)\s+([-0-9.]+)\s+([-0-9.]+)\s*]', text)
            results.check('A7', 'saved PDF header', data.startswith(b'%PDF-'), data[:8])
            results.check('A7', 'saved PDF has one page', pages == 1, pages)
            results.check('A7', 'saved PDF MediaBox', media_box is not None, text[:200])
            if media_box is not None:
                results.check('A7', 'saved PDF page size equals scene',
                              abs(float(media_box.group(3)) - math.ceil(state.get('sceneWidth', 0))) <= 0.5 and
                              abs(float(media_box.group(4)) - math.ceil(state.get('sceneHeight', 0))) <= 0.5,
                              {'mediaBox': media_box.groups()[2:], 'scene': (state.get('sceneWidth'), state.get('sceneHeight'))})
        else:
            results.check('A7', 'saved PDF written', False, str(saved))
    if wait_top(device, FOCUS_ACTIVITY):
        device.key(4)

    for directory in PUBLIC_MERMAID_DIRS:
        note = f'Mermaid/Class {directory}.md'
        report = device.export_all(note, f'a7-export-{directory}.json')
        exports = [entry for entry in report.get('exports', [])
                   if not entry.get('error') and entry.get('name')]
        formats = {entry.get('format') for entry in exports}
        results.check('A7', f'{directory} three formats', len(exports) >= 3 and formats == {'svg', 'png', 'pdf'},
                      {'count': len(exports), 'formats': sorted(formats), 'all': len(report.get('exports', []))})
        for entry in exports:
            name = entry.get('name', '')
            extra = entry.get('extra', {})
            results.check('A7', f'{directory} default name {name}',
                          bool(re.match(r'^Class .+-diagram-\d+\.(svg|png|pdf)$', name)), name)
            local_export = media / (entry.get('name') or 'export')
            device.pull(entry.get('path', ''), local_export)
            if entry.get('format') == 'svg':
                text = local_export.read_text(errors='replace') if local_export.is_file() else ''
                results.check('A7', f'{directory} svg xml', extra.get('validXmlStart') is True, extra)
                try:
                    ET.fromstring(text)
                    results.check('A7', f'{directory} svg parses', True, '')
                except ET.ParseError as error:
                    results.check('A7', f'{directory} svg parses', False, str(error))
                results.check('A7', f'{directory} svg no script', extra.get('hasScript') is False, extra)
                results.check('A7', f'{directory} svg no foreignObject', extra.get('hasForeignObject') is False, extra)
                view_box = extra.get('viewBox')
                results.check('A7', f'{directory} svg viewBox', view_box is not None and
                              abs(view_box[2] - extra.get('sceneWidth', -1)) <= 0.5 and
                              abs(view_box[3] - extra.get('sceneHeight', -1)) <= 0.5, view_box)
            elif entry.get('format') == 'png':
                results.check('A7', f'{directory} png dimensions',
                              extra.get('pngWidth') == math.ceil(extra.get('sceneWidth', 0) * 2) and
                              extra.get('pngHeight') == math.ceil(extra.get('sceneHeight', 0) * 2),
                              extra)
                results.check('A7', f'{directory} png pulled', local_export.is_file(), str(local_export))
            elif entry.get('format') == 'pdf':
                results.check('A7', f'{directory} pdf header', extra.get('pdfHeader') == '%PDF-', extra)
                results.check('A7', f'{directory} pdf one page', extra.get('pdfPages') == 1, extra)
                media_box = extra.get('mediaBox')
                results.check('A7', f'{directory} pdf MediaBox',
                              media_box is not None and
                              abs(media_box[2] - math.ceil(extra.get('sceneWidth', 0))) <= 0.5 and
                              abs(media_box[3] - math.ceil(extra.get('sceneHeight', 0))) <= 0.5,
                              {'mediaBox': media_box, 'scene': (extra.get('sceneWidth'), extra.get('sceneHeight'))})

    download = results.output / 'a7-create-document.png'
    device.pull('/sdcard/Download/m1-document-export.png', download)
    results.check('A7', 'ACTION_CREATE_DOCUMENT file', download.is_file() and download.stat().st_size > 0,
                  str(download))
    if download.is_file():
        data = download.read_bytes()
        results.check('A7', 'CreateDocument PNG header', data.startswith(b'\x89PNG\r\n\x1a\n'), len(data))


def parse_gfxinfo(text: str) -> dict:
    janky = re.search(r'Janky frames:\s*(\d+)\s*\(([0-9.]+)%\)', text)
    frames = []
    header = None
    started = False
    for line in text.splitlines():
        if line.startswith('---PROFILEDATA---'):
            started = not started
            header = None
            continue
        if not started:
            continue
        parts = line.split(',')
        if line.startswith('Flags,') or 'IntendedVsync' in parts[:4]:
            header = [part.strip() for part in parts]
            continue
        if not parts or not parts[0].isdigit():
            continue
        try:
            if header and 'IntendedVsync' in header and 'FrameCompleted' in header:
                intended = int(parts[header.index('IntendedVsync')])
                completed = int(parts[header.index('FrameCompleted')])
            else:
                intended = int(parts[2])
                completed = int(parts[16])
            if completed > intended:
                frames.append((completed - intended) / 1_000_000)
        except (ValueError, IndexError):
            continue
    frames.sort()

    def percentile(value: float) -> float | None:
        if not frames:
            return None
        index = min(len(frames) - 1, max(0, int(round((len(frames) - 1) * value))))
        return frames[index]

    return {
        'jankyFrames': int(janky.group(1)) if janky else None,
        'jankyPercent': float(janky.group(2)) if janky else None,
        'p50': percentile(0.50),
        'p90': percentile(0.90),
        'p99': percentile(0.99),
        'samples': len(frames),
    }


def run_macrobenchmark(results: Results) -> tuple[bool, list[dict], str]:
    env = dict(os.environ)
    sdk = find_sdk_root()
    java_home = Path('/Applications/Android Studio.app/Contents/jbr/Contents/Home')
    if java_home.is_dir():
        env['JAVA_HOME'] = str(java_home)
    env['ANDROID_HOME'] = str(sdk)
    env['ANDROID_SDK_ROOT'] = str(sdk)
    version_code = subprocess.run(
        ['git', 'rev-list', '--count', 'HEAD'], cwd=ROOT, capture_output=True, text=True,
    ).stdout.strip() or '1'
    command = ['./gradlew', ':macrobenchmark:connectedBenchmarkReleaseAndroidTest', '--no-daemon',
               f'-PhortusVersionCode={version_code}']
    shutil.rmtree(ROOT / 'apps/android/macrobenchmark/build/outputs/connected_android_test_additional_output',
                  ignore_errors=True)
    completed = subprocess.run(
        command, cwd=ROOT / 'apps/android', env=env, capture_output=True, text=True, timeout=3600,
    )
    log_text = completed.stdout + completed.stderr
    (results.output / 'macrobenchmark.log').write_text(log_text)
    records = []
    build = ROOT / 'apps/android/macrobenchmark/build'
    for path in sorted(build.rglob('*.json')) if build.is_dir() else []:
        try:
            data = json.loads(path.read_text())
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        benchmarks = data.get('benchmarks')
        if not isinstance(benchmarks, list):
            continue
        for entry in benchmarks:
            if isinstance(entry, dict) and entry.get('metrics'):
                records.append(entry)
        target = results.output / 'benchmark' / path.name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(path.read_text())
    return completed.returncode == 0 and bool(records), records, log_text[-4000:]


def benchmark(name: str, records: list[dict]) -> dict | None:
    for record in records:
        if name.lower() in str(record.get('name', '')).lower():
            return record
    return None


def metric(record: dict | None, name: str) -> dict | None:
    if not record:
        return None
    for group in ('metrics', 'sampledMetrics'):
        value = record.get(group, {}).get(name)
        if isinstance(value, dict):
            return value
    return None


def metric_value(record: dict | None, name: str, key: str) -> float | None:
    value = metric(record, name)
    if not value:
        return None
    raw = value.get(key)
    try:
        return float(raw) if raw is not None else None
    except (TypeError, ValueError):
        return None


def restart_device(device: Device) -> None:
    device.run('reboot', check=False)
    subprocess.run([str(device.adb), '-s', device.serial, 'wait-for-device'],
                   capture_output=True, timeout=300)
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        if device.shell('getprop sys.boot_completed', check=False).strip() == '1':
            break
        time.sleep(2)
    time.sleep(5)
    device.shell('wm dismiss-keyguard', check=False)
    device.shell('input keyevent 82', check=False)
    device.grant()


def measure_settings_baseline(device: Device, results: Results, attempt: int) -> dict:
    device.shell('am start -a android.settings.SETTINGS', check=False)
    time.sleep(2.5)
    device.shell('dumpsys gfxinfo com.android.settings reset', check=False)
    width, height, _ = device.screen()
    for index in range(20):
        if index % 2 == 0:
            device.shell(f'input swipe {width // 2} {int(height * 0.82)} {width // 2} {int(height * 0.22)} 120', check=False)
        else:
            device.shell(f'input swipe {width // 2} {int(height * 0.22)} {width // 2} {int(height * 0.82)} 120', check=False)
    raw = device.shell('dumpsys gfxinfo com.android.settings framestats', check=False)
    (results.output / f'a8-settings-baseline-{attempt}.txt').write_text(raw)
    return {'attempt': attempt, **parse_gfxinfo(raw)}


def assert_a8(device: Device, results: Results, stage: Path) -> None:
    final = device.dump('a8-main-thread.json', wait_ms=800)
    for name, value in final.get('mainThread', {}).items():
        results.check('A8', f'main thread {name}', value == 0, value)

    large = []
    for index in range(3):
        device.shell(f'am force-stop {PACKAGE}')
        value = device.dump(f'a8-large-{index}.json', note='Synthetic/Large note.md', wait_ms=1000)
        large.append(value.get('openMs', 10 ** 9))
    results.check('A8', 'large note open median <= 1500ms', sorted(large)[1] <= 1500, large)

    device.dump('a8-large-open.json', note='Synthetic/Large note.md', cold=True, wait_ms=1400)
    time.sleep(2.0)
    scrolled = device.dump('a8-scrolled.json', wait_ms=600)
    results.check('A8', 'composed canvases <= visible + 4',
                  scrolled.get('maxComposedDiagramCanvases', 0) <=
                  scrolled.get('maxVisibleDiagramCanvases', 0) + 4,
                  {'maxComposed': scrolled.get('maxComposedDiagramCanvases'),
                   'maxVisible': scrolled.get('maxVisibleDiagramCanvases')})

    all_notes = sorted(
        path.relative_to(stage).as_posix()
        for path in stage.rglob('*')
        if path.is_file() and path.suffix.lower() in ('.md', '.mmd')
    )
    for note in all_notes:
        device.start(note=note, cold=False)
        time.sleep(0.22)
    meminfo = device.shell(f'dumpsys meminfo {PACKAGE}')
    (results.output / 'a8-meminfo.txt').write_text(meminfo)
    match = re.search(r'TOTAL PSS:\s+(\d+)', meminfo) or re.search(r'TOTAL\s+(\d+)', meminfo)
    pss_mb = int(match.group(1)) / 1024 if match else None
    results.check('A8', 'total PSS <= 250 MB', pss_mb is not None and pss_mb <= 250, pss_mb)
    results.check('A8', 'process alive', device.pid() > 0, device.pid())

    cold_probe = device.dump('a8-cold-probe.json', note='Markdown field guide.md', cold=True, wait_ms=2200)
    app_fully_drawn_ms = cold_probe.get('appFullyDrawnMs', -1)

    passed, records, tail = run_macrobenchmark(results)
    baseline = measure_settings_baseline(device, results, 1)
    if baseline.get('jankyPercent') is None or baseline['jankyPercent'] > 2.0:
        # 基线无效时按任务书重启模拟器，整轮重测（无效轮次保留）。
        invalid = results.output / 'benchmark-invalid-1'
        if (results.output / 'benchmark').is_dir():
            shutil.copytree(results.output / 'benchmark', invalid, dirs_exist_ok=True)
        restart_device(device)
        passed, records, tail = run_macrobenchmark(results)
        baseline = measure_settings_baseline(device, results, 2)
    results.check('A8', 'macrobenchmark produced JSON', passed, tail)
    if not records:
        return
    cold = benchmark('coldStartWithNoteBaselineProfile', records)
    cold_off = benchmark('coldStartWithNoteNoProfile', records)
    opened = benchmark('openFieldGuideInProcessBaselineProfile', records)
    opened_off = benchmark('openFieldGuideInProcessNoProfile', records)
    scroll = benchmark('fastScrollLargeNoteBaselineProfile', records)
    scroll_off = benchmark('fastScrollLargeNoteNoProfile', records)

    cold_median = metric_value(cold, 'timeToInitialDisplayMs', 'median')
    cold_full = metric_value(cold, 'timeToFullDisplayMs', 'median')
    results.check('A8', 'cold start with note <= 1000ms',
                  cold_median is not None and cold_median <= 1000,
                  {'median': cold_median, 'timeToFullDisplayMs': cold_full})
    results.check('A8', 'timeToFullDisplayMs reported',
                  cold_full is not None or app_fully_drawn_ms > 0,
                  {'macrobenchmark': cold_full, 'appFullyDrawnMs': app_fully_drawn_ms})

    open_p99 = metric_value(opened, 'frameDurationCpuMs', 'P99')
    results.check('A8', 'open note frameDurationCpuMs P99 <= 33ms',
                  open_p99 is not None and open_p99 <= 33, open_p99)

    overrun = metric(scroll, 'frameOverrunMs')
    overrun_p90 = metric_value(scroll, 'frameOverrunMs', 'P90')
    results.check('A8', 'scroll frameOverrunMs P90 <= 0',
                  overrun_p90 is not None and overrun_p90 <= 0,
                  {key: overrun.get(key) for key in ('P50', 'P90', 'P95', 'P99')} if overrun else None)

    results.check('A8', 'baseline profile off comparison reported',
                  cold_off is not None and scroll_off is not None,
                  {'coldOff': metric_value(cold_off, 'timeToInitialDisplayMs', 'median'),
                   'scrollOffP90': metric_value(scroll_off, 'frameOverrunMs', 'P90')})

    results.check('A8', 'settings baseline jank <= 2%',
                  baseline.get('jankyPercent') is not None and baseline['jankyPercent'] <= 2.0, baseline)

    summary = {
        'coldStart': {
            'baselineProfile': {'timeToInitialDisplayMs': cold_median,
                                'timeToFullDisplayMs': cold_full,
                                'appFullyDrawnMs': app_fully_drawn_ms},
            'none': {'timeToInitialDisplayMs': metric_value(cold_off, 'timeToInitialDisplayMs', 'median'),
                     'timeToFullDisplayMs': metric_value(cold_off, 'timeToFullDisplayMs', 'median')},
        },
        'openNote': {
            'baselineProfile': metric(opened, 'frameDurationCpuMs'),
            'none': metric(opened_off, 'frameDurationCpuMs'),
        },
        'scroll': {
            'baselineProfile': metric(scroll, 'frameOverrunMs'),
            'none': metric(scroll_off, 'frameOverrunMs'),
        },
        'settingsBaseline': baseline,
        'raw': [str(path) for path in sorted((results.output / 'benchmark').glob('*.json'))],
    }
    (results.output / 'a8-benchmark-summary.json').write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + '\n')


def assert_a10(device: Device, results: Results, apk: Path, sdk: Path) -> None:
    report_path = results.output / 'instrument-selection.json'
    if report_path.is_file():
        data = json.loads(report_path.read_text())
        results.check('A10', 'selection toolbar with copy appears', data.get('toolbarFound') is True, data)
        results.check('A10', 'clipboard contains selected display text',
                      bool(data.get('clipboard')) and data.get('clipboardLength', 0) > 0, data)
        results.check('A10', 'clipboard has no markdown markers', data.get('markers') is False, data)
        results.check('A10', 'link still opens after selection', data.get('linkOpened') is True, data)
        results.check('A10', 'scrolling survives selection', data.get('scrollSurvived') is True, data)
    else:
        results.check('A10', 'selection instrument JSON present', False, str(report_path))

    aapt2 = None
    candidates = sorted((sdk / 'build-tools').glob('*/aapt2')) if (sdk / 'build-tools').is_dir() else []
    if candidates:
        aapt2 = candidates[-1]
    found = False
    if aapt2 is not None and apk.is_file():
        completed = subprocess.run(
            [str(aapt2), 'dump', 'xmltree', '--file', 'AndroidManifest.xml', str(apk)],
            capture_output=True, text=True, timeout=120,
        )
        found = 'enableOnBackInvokedCallback' in completed.stdout
    results.check('A10', 'manifest enables OnBackInvokedCallback', found, str(aapt2))


def assert_a9(results: Results, apk_debug: Path, apk_release: Path, sdk: Path) -> None:
    smoke = ROOT / 'tools/check-android-smoke.py'
    for apk, release in ((apk_debug, False), (apk_release, True)):
        if not apk.is_file():
            results.check('A9', f'APK exists {apk.name}', False, str(apk))
            continue
        command = [sys.executable, '-B', str(smoke), '--apk', str(apk), '--static-only', '--output', str(results.output / f'a9-static-{"release" if release else "debug"}')]
        if release:
            command.append('--release')
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=600)
        results.check('A9', f'static audit {apk.name}', completed.returncode == 0,
                      (completed.stdout + completed.stderr)[-2000:])
    diff = subprocess.run(
        ['git', 'diff', '--name-status', '0b87ea4..HEAD'],
        cwd=ROOT, capture_output=True, text=True, timeout=60,
    )
    (results.output / 'a9-diff.txt').write_text(diff.stdout)
    allowed = re.compile(r'^(apps/android/|tools/check-android-reading\.py|tools/build-android\.sh|tools/check-android-smoke\.py|crates/hortus-ffi/uniffi\.toml|\.gitignore)')
    bad = [line.split('\t')[-1] for line in diff.stdout.splitlines()
           if line.strip() and not allowed.match(line.split('\t')[-1]) and 'agents-ui' not in line]
    results.check('A9', 'diff within allowed paths', not bad, bad)
    if (ROOT / 'tools/check-public-tree.py').is_file():
        public = subprocess.run([sys.executable, '-B', 'tools/check-public-tree.py'], cwd=ROOT,
                                capture_output=True, text=True, timeout=600)
        results.check('A9', 'public tree check', public.returncode == 0, public.stdout[-1000:] + public.stderr[-1000:])


def main() -> int:
    parser = argparse.ArgumentParser(description='M1 Android 阅读视图验收（A2–A8）')
    parser.add_argument('--apk', type=Path, default=ROOT / 'build/android/hortus-debug.apk')
    parser.add_argument('--release-apk', type=Path, default=ROOT / 'build/android/hortus-release-unsigned.apk')
    parser.add_argument('--avd', default='Medium_Phone_API_36.1')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/android-reading')
    parser.add_argument('--media', type=Path, default=None)
    parser.add_argument('--keep-emulator', action='store_true')
    parser.add_argument('--skip-tests', action='store_true')
    parser.add_argument('--items', default='A2,A3,A4,A5,A6,A7,A8,A10',
                        help='逗号分隔的检查项，例如 A4,A6')
    args = parser.parse_args()

    output = args.output.expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.media.expanduser().resolve() if args.media else output / 'media')
    media.mkdir(parents=True, exist_ok=True)
    results = Results(output)
    sdk = find_sdk_root()
    serial, emulator_proc = ensure_emulator(sdk, args.avd, output)
    adb = sdk / 'platform-tools/adb'
    device = Device(adb, serial, output)
    log(f'设备：{serial}')
    device.install(args.apk.expanduser().resolve())
    device.shell(f'pm clear {PACKAGE}', check=False)
    device.grant()

    stage = Path(tempfile.mkdtemp(prefix='hortus-m1-vault-'))
    try:
        manifest = build_vault(stage, output)
        baseline = sha_tree(stage)
        device.shell(f'rm -rf {DEVICE_VAULT}')
        device.shell(f'mkdir -p {DEVICE_VAULT}')
        for item in sorted(stage.iterdir()):
            device.run('push', str(item), f'{DEVICE_VAULT}/')
        (output / 'vault-sha.json').write_text(json.dumps(baseline, ensure_ascii=False, indent=2) + '\n')

        device.shell('rm -rf /sdcard/Download/hortus-m1-instrument')
        if not args.skip_tests:
            ndk = find_ndk(sdk)
            env = dict(os.environ)
            java_home = Path('/Applications/Android Studio.app/Contents/jbr/Contents/Home')
            if java_home.is_dir():
                env['JAVA_HOME'] = str(java_home)
            env['ANDROID_HOME'] = str(sdk)
            env['ANDROID_SDK_ROOT'] = str(sdk)
            version_code = subprocess.run(
                ['git', 'rev-list', '--count', 'HEAD'], cwd=ROOT, capture_output=True, text=True,
            ).stdout.strip() or '1'
            command = ['./gradlew', ':app:connectedDebugAndroidTest', '--no-daemon',
                       f'-PhortusVersionCode={version_code}']
            if ndk:
                command.append(f'-PhortusNdkPath={ndk}')
            proxy = os.environ.get('HTTPS_PROXY') or os.environ.get('https_proxy')
            if proxy:
                match = re.match(r'https?://([^:]+):(\d+)', proxy)
                if match:
                    command += [
                        f'-Dhttps.proxyHost={match.group(1)}',
                        f'-Dhttps.proxyPort={match.group(2)}',
                        f'-Dhttp.proxyHost={match.group(1)}',
                        f'-Dhttp.proxyPort={match.group(2)}',
                        '-Dhttps.protocols=TLSv1.2',
                    ]
            log('运行 connectedDebugAndroidTest…')
            tests = subprocess.run(command, cwd=ROOT / 'apps/android', env=env, capture_output=True,
                                   text=True, timeout=1800)
            (output / 'connected-tests.log').write_text(tests.stdout + tests.stderr)
            results.check('A1', 'connectedDebugAndroidTest', tests.returncode == 0,
                          (tests.stdout + tests.stderr)[-3000:])
            for name in ('focus-400.json', 'focus-400.png', 'focus-restored.json',
                         'share-cache.json', 'selection.json'):
                device.pull(f'/sdcard/Download/hortus-m1-instrument/{name}',
                            output / f'instrument-{name}')
            if (output / 'instrument-focus-400.png').is_file():
                shutil.copy2(output / 'instrument-focus-400.png', media / 'focus-400.png')
            device.install(args.apk.expanduser().resolve())
            device.shell(f'pm clear {PACKAGE}', check=False)
            device.grant()

        cli = build_cli(output)
        device.night(False)
        device.locale('en-US')

        wanted = {item.strip().upper() for item in args.items.split(',') if item.strip()}
        if 'A2' in wanted:
            assert_a2(device, results, media)
        if 'A3' in wanted:
            assert_a3(device, results, media, manifest, cli)
        if 'A4' in wanted:
            assert_a4(device, results, baseline)
        if 'A5' in wanted:
            assert_a5(device, results, media)
        if 'A6' in wanted:
            assert_a6(device, results)
        if 'A7' in wanted:
            assert_a7(device, results, media)
        if 'A8' in wanted:
            assert_a8(device, results, stage)
        if 'A10' in wanted:
            assert_a10(device, results, args.apk.expanduser().resolve(), sdk)
        if 'A9' in wanted:
            assert_a9(results, args.apk, args.release_apk, sdk)
    finally:
        summary = results.summary()
        (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        index = ['# M1 Android 阅读验收证据', '', f'- 设备：{serial}', f'- APK：{args.apk}',
                 f'- 媒体：{media}', '']
        for item in sorted(summary['byItem']):
            index.append(f'## {item}：{"通过" if summary["byItem"][item] else "未通过"}')
            for entry in summary['items']:
                if entry['item'] != item:
                    continue
                index.append(f'- [{"x" if entry["passed"] else " "}] {entry["name"]}: {entry["detail"]}')
            index.append('')
        (output / 'index.md').write_text('\n'.join(index) + '\n')
        if emulator_proc is not None and not args.keep_emulator:
            subprocess.run([str(adb), '-s', serial, 'emu', 'kill'], capture_output=True, timeout=30)
    for item, passed in results.summary()['byItem'].items():
        log(f'{item}: {"PASS" if passed else "FAIL"}')
    return 0 if results.summary()['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
