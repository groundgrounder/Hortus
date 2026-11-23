#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""M1 Android 阅读视图验收：拼装验收库、驱动 Debug 入口、执行 A2–A8 场景。"""
from __future__ import annotations

import argparse
import hashlib
import json
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
        return {
            'items': self.items,
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
    before = device.dump('a4-before.json', note=note, cold=True, wait_ms=1500)
    node = first_interactive(before, 'node', lambda hit: hit.get('canCollapse') and hit.get('nodeId'))
    results.check('A4', 'collapsible node visible', node is not None, before.get('interactive'))
    if node:
        before_record = next((record for record in before.get('diagrams', []) if record.get('dark') is False), None)
        x, y = center(node)
        device.tap(x, y)
        after = device.dump('a4-after-collapse.json', wait_ms=1000)
        after_record = next((record for record in after.get('diagrams', []) if record.get('dark') is False), None)
        results.check('A4', '+N label after collapse', after_record and after_record.get('plusLabels', 0) > 0,
                      after_record)
        results.check('A4', 'visible nodes decreased', after_record and before_record and
                      after_record.get('hits', 10**9) < before_record.get('hits', -1),
                      (before_record.get('hits') if before_record else None,
                       after_record.get('hits') if after_record else None))
        results.check('A4', 'collapse stored in record', after_record and node['nodeId'] in after_record.get('collapsed', []),
                      after_record.get('collapsed') if after_record else None)
        reopened = device.dump('a4-reopened.json', note=note, cold=True, wait_ms=1500)
        reopened_record = next((record for record in reopened.get('diagrams', []) if record.get('dark') is False), None)
        results.check('A4', 'collapse persists after reopen',
                      reopened_record and node['nodeId'] in reopened_record.get('collapsed', []),
                      reopened_record.get('collapsed') if reopened_record else None)
        if reopened_record and reopened_record.get('collapsed'):
            expanded = device.dump('a4-reopened-expanded.json', wait_ms=1000)
            root = first_interactive(expanded, 'node', lambda hit: bool(hit.get('canCollapse')))
            if root:
                device.tap(*center(root))
                device.dump('a4-reopened-expanded.json', wait_ms=1000)

    link_before = device.dump('a4-reopened-expanded.json', note=note, wait_ms=1200) if False else device.dump('a4-link-before.json', note=note, wait_ms=1200)
    if not link_before.get('interactive'):
        expanded_path = results.output / 'a4-reopened-expanded.json'
        if expanded_path.exists():
            link_before = json.loads(expanded_path.read_text())
            link_before['key'] = 'a4-reopened-expanded.json'
    link_node = first_interactive(link_before, 'node', lambda hit: bool(hit.get('target')))
    results.check('A4', 'linked node visible', link_node is not None, link_before.get('interactive'))
    before_block = link_before.get('firstVisibleBlockId')
    if link_node:
        x, y = center(link_node)
        device.tap(x, y)
        opened = device.dump('a4-link-opened.json', wait_ms=1200)
        results.check('A4', 'node link opens target', opened.get('note') == 'Start here.md', opened.get('note'))
        device.key(4)
        returned = device.dump('a4-link-returned.json', wait_ms=1200)
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
    for file in math_files:
        note = f'Math/{file.name}'
        value = device.dump(f'a5-{file.stem}.json', note=note, cold=True, wait_ms=2000)
        status = value.get('mathStatus', {})
        total = status.get('rendered', 0) + status.get('failed', 0) + status.get('empty', 0)
        results.check('A5', f'{file.name} formulas seen', total >= 0, status)
        for formula in value.get('math', []):
            if formula.get('note') == note and formula.get('emRatio') is not None:
                ratio_samples.append(formula['emRatio'])
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
        if file.name == 'missing-glyphs.md':
            results.check('A5', 'fallback metrics returned', value.get('mathMetricsCodepoints', 0) > 0,
                          value.get('mathMetricsCodepoints'))
    results.check('A5', 'inline ratio 1.21', bool(ratio_samples) and all(abs(value - 1.21) <= 0.01 for value in ratio_samples),
                  {'samples': len(ratio_samples), 'min': min(ratio_samples) if ratio_samples else None,
                   'max': max(ratio_samples) if ratio_samples else None})
    table = device.dump('a5-table.json', note='Synthetic/Tables/Math table.md', wait_ms=1800)
    table_math = [cell for table_row in table.get('tables', []) for cell in table_row.get('cells', []) if cell.get('math')]
    results.check('A5', 'table cell formula present', bool(table_math), table.get('tables'))
    results.check('A5', 'table cell formula rendered', table.get('mathStatus', {}).get('rendered', 0) > 0,
                  table.get('mathStatus'))
    embed = device.dump('a5-embed.json', note='Synthetic/Embeds/Math embed.md', wait_ms=1800)
    results.check('A5', 'embedded formula rendered', embed.get('mathStatus', {}).get('rendered', 0) > 0,
                  embed.get('mathStatus'))
    for note, name in (
        ('Math/showcase.md', 'a5-block.png'),
        ('Math/inline.md', 'a5-inline.png'),
        ('Math/inline-table-context.md', 'a5-table.png'),
    ):
        device.dump(f'a5-shot-{name}.json', note=note, wait_ms=1200)
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


def assert_a7(device: Device, results: Results, media: Path) -> None:
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
                import math
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
                              abs(media_box[2] - extra.get('sceneWidth', -1)) <= 1.0 and
                              abs(media_box[3] - extra.get('sceneHeight', -1)) <= 1.0, media_box)

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


def assert_a8(device: Device, results: Results, stage: Path) -> None:
    final = device.dump('a8-main-thread.json', wait_ms=800)
    for name, value in final.get('mainThread', {}).items():
        results.check('A8', f'main thread {name}', value == 0, value)

    cold = []
    warm = []
    for index in range(3):
        device.shell(f'am force-stop {PACKAGE}')
        value = device.dump(f'a8-cold-{index}.json', note='Markdown field guide.md', wait_ms=700)
        cold.append((value.get('openMs', 10**9), value.get('firstFrameMs', 10**9)))
        results.check('A8', f'cold open no error {index}', not value.get('error'), value.get('error'))
    for index in range(3):
        device.key(3)
        value = device.dump(f'a8-warm-{index}.json', note='Markdown field guide.md', wait_ms=700)
        warm.append((value.get('openMs', 10**9), value.get('firstFrameMs', 10**9)))
    for label, values in (('cold', cold), ('warm', warm)):
        opens = sorted(value[0] for value in values)
        frames = sorted(value[1] for value in values)
        results.check('A8', f'{label} open median <= 800ms', opens[len(opens)//2] <= 800, opens)
        results.check('A8', f'{label} first frame median <= 800ms', frames[len(frames)//2] <= 800, frames)
        max_frames = [
            json.loads((results.output / f'a8-{label}-{index}.json').read_text()).get('openMaxFrameMs', 10**9)
            for index in range(3)
        ]
        results.check('A8', f'{label} max main-thread frame <= 33ms',
                      sorted(max_frames)[1] <= 33, max_frames)


    large = []
    for index in range(3):
        device.shell(f'am force-stop {PACKAGE}')
        value = device.dump(f'a8-large-{index}.json', note='Synthetic/Large note.md', wait_ms=900)
        large.append(value.get('openMs', 10**9))
    results.check('A8', 'large note open median <= 1500ms', sorted(large)[1] <= 1500, large)

    device.dump('a8-large-open.json', note='Synthetic/Large note.md', cold=True, wait_ms=1200)
    time.sleep(3.0)
    device.shell(f'dumpsys gfxinfo {PACKAGE} reset')
    for _ in range(8):
        device.shell('input swipe 540 1900 540 500 500')
        time.sleep(0.35)
    gfx = device.shell(f'dumpsys gfxinfo {PACKAGE} framestats')
    stats = parse_gfxinfo(gfx)
    (results.output / 'a8-gfxinfo.txt').write_text(gfx)
    results.check('A8', 'janky frames <= 10%', stats['jankyPercent'] is not None and stats['jankyPercent'] <= 10,
                  stats)
    results.check('A8', 'frame percentiles collected', stats['samples'] > 0, stats)
    scrolled = device.dump('a8-scrolled.json', wait_ms=600)
    results.check('A8', 'composed canvases <= visible + 4',
                  scrolled.get('maxComposedDiagramCanvases', 0) <=
                  scrolled.get('maxVisibleDiagramCanvases', 0) + 4,
                  {'maxComposed': scrolled.get('maxComposedDiagramCanvases'),
                   'maxVisible': scrolled.get('maxVisibleDiagramCanvases')})

    notes = sorted(
        path.relative_to(ROOT / 'fixtures/demo-vault').as_posix()
        for path in (ROOT / 'fixtures/demo-vault').rglob('*')
        if path.is_file() and path.suffix.lower() in ('.md', '.mmd')
    )
    notes += sorted(
        path.relative_to(ROOT / 'fixtures/math').as_posix()
        for path in (ROOT / 'fixtures/math').glob('*.md')
    )
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
    parser.add_argument('--items', default='A2,A3,A4,A5,A6,A7,A8',
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
