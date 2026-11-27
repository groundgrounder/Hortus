#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""M2b Android 设置页、字号原子刷新、缓存清理、公式源码复制与私有存储边界验收（A2/A3）。"""
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
DEVICE_VAULT = '/sdcard/Documents/HortusM2bVault'
REMOTE_DIR = f'/sdcard/Android/data/{PACKAGE}/files'

M2B_NOTE = '''# M2b Check Note

中文与 English 混排段落，行内公式 $a^2 + b^2 = c^2$ 与 $E = mc^2$ 紧随其后。

$$\\frac{1}{2} + \\sqrt{x^2 + 1}$$

```mermaid
flowchart LR
  A[Start 主节点] --> B[End 次节点]
```
'''


WIDE_TABLE_MATH_NOTE = '''# Wide Table And Long Math

| Col1 | Col2 | Col3 | Col4 | Col5 | Col6 |
| --- | --- | --- | --- | --- | --- |
| AlphaLongCellValue | BetaLongCellValue | GammaLongCellValue | DeltaLongCellValue | EpsilonLongCellValue | ZetaLongCellValue |

$$\\sum_{i=1}^{24} \\frac{a_i x_i + b_i y_i + c_i z_i}{\\sqrt{d_i^2 + e_i^2 + f_i^2}} + \\prod_{j=1}^{12} \\left(1 + \\frac{u_j}{v_j + w_j}\\right)$$
'''

EMBED_TARGET_NOTE = '# Embed Target\n\nTarget reached.\n'

EMBED_SYNC_B_NOTE = '''# Embed Sync Child

```mermaid
mindmap
  root((RootNode))
    BranchOne
      LeafAlpha
      LeafBeta
    [[EmbedTarget]]
```
'''

EMBED_SYNC_A_NOTE = '# Embed Sync Host\n\n![[EmbedSyncB]]\n'

LONG_SCROLL_NOTE = '# Long Scroll Note\n\n' + '\n'.join(
    f'## Section {i:02d}\n\nParagraph {i:02d} with enough body text for vertical scrolling checks.\n'
    for i in range(1, 31)
)

BROKEN_ZH_NOTE = '''# 中文渲染失败验证

```mermaid
flowchart LR
  A -->
```

![[missing-zh-pic.png]]

$$\\unknowncommand{1}$$
'''


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


def copy_tree(source: Path, destination: Path) -> None:
    for item in source.iterdir():
        target = destination / item.name
        if item.is_dir():
            shutil.copytree(item, target, dirs_exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(item, target)


def tree_hashes(root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for path in sorted(root.rglob('*')):
        if path.is_file():
            result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


class Device:
    def __init__(self, adb: Path, serial: str, output: Path):
        self.adb = adb
        self.serial = serial
        self.output = output

    def run(self, *args: str, check: bool = True, binary: bool = False):
        cmd = [str(self.adb), '-s', self.serial, *args]
        res = subprocess.run(cmd, capture_output=True, timeout=180)
        if check and res.returncode != 0:
            raise RuntimeError(f'{" ".join(args)} failed: {res.stderr.decode(errors="replace")}')
        return res.stdout if binary else res.stdout.decode(errors='replace')

    def shell(self, command: str, check: bool = True) -> str:
        return self.run('shell', command, check=check)

    def install(self, apk: Path) -> None:
        res = self.run('install', '-r', '-t', str(apk))
        if 'Success' not in res and 'VERSION_DOWNGRADE' in res:
            self.shell(f'pm uninstall {PACKAGE}', check=False)
            res = self.run('install', '-r', '-t', str(apk))
        if 'Success' not in res:
            raise RuntimeError(f'APK 安装失败: {res}')
        self.shell(f'appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow', check=False)

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

    def pull(self, remote: str, local: Path) -> bool:
        res = subprocess.run(
            [str(self.adb), '-s', self.serial, 'pull', remote, str(local)],
            capture_output=True, timeout=180,
        )
        return res.returncode == 0 and local.exists()

    def dump(self, name: str, note: str | None = None, cold: bool = False,
             wait_ms: int = 1200, action: str | None = None,
             extra: list[str] | None = None, timeout: float = 40.0) -> dict:
        remote = f'{REMOTE_DIR}/{name}'
        self.shell(f'rm -f {remote}', check=False)
        self.start(note=note, dump=name, wait_ms=wait_ms, cold=cold, action=action, extra=extra)
        local = self.output / name
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.shell(f'test -s {remote}', check=False).strip() == '':
                if self.pull(remote, local):
                    return json.loads(local.read_text())
            time.sleep(0.25)
        raise RuntimeError(f'等待检查 JSON 超时: {name}')

    def ui_dump(self, name: str) -> list[dict]:
        remote = f'/sdcard/{name}'
        self.shell(f'uiautomator dump {remote}')
        local = self.output / name
        self.pull(remote, local)
        root = ET.fromstring(local.read_text())
        parents = {child: parent for parent in root.iter() for child in parent}
        nodes = []
        for elem in root.iter('node'):
            bounds = elem.attrib.get('bounds', '')
            m = re.match(r'\[(\d+),(\d+)\]\[(\d+),(\d+)\]', bounds)
            if not m:
                continue
            x1, y1, x2, y2 = (int(v) for v in m.groups())
            cur = elem
            effective_enabled = True
            while cur is not None:
                if cur.attrib.get('enabled') == 'false':
                    effective_enabled = False
                    break
                cur = parents.get(cur)
            nodes.append({
                'text': elem.attrib.get('text', ''),
                'desc': elem.attrib.get('content-desc', ''),
                'class': elem.attrib.get('class', ''),
                'clickable': elem.attrib.get('clickable') == 'true',
                'enabled': effective_enabled,
                'cx': (x1 + x2) // 2,
                'cy': (y1 + y2) // 2,
                'bounds': (x1, y1, x2, y2),
            })
        return nodes

    def tap_node(self, nodes: list[dict], *, text: str | None = None,
                 desc: str | None = None, contains: str | None = None) -> dict:
        for node in nodes:
            if text is not None and node['text'] == text:
                self.tap(node['cx'], node['cy'])
                return node
            if desc is not None and node['desc'] == desc:
                self.tap(node['cx'], node['cy'])
                return node
            if contains is not None and (contains in node['text'] or contains in node['desc']):
                self.tap(node['cx'], node['cy'])
                return node
        raise RuntimeError(f'未找到 UI 节点: text={text}, desc={desc}, contains={contains}')

    def tap(self, x: float, y: float, wait: float = 0.8) -> None:
        self.shell(f'input tap {int(x)} {int(y)}')
        time.sleep(wait)

    def key(self, code: int = 4, wait: float = 0.8) -> None:
        self.shell(f'input keyevent {code}')
        time.sleep(wait)

    def night(self, dark: bool) -> None:
        self.shell(f'cmd uimode night {"yes" if dark else "no"}')
        time.sleep(1.0)

    def screenshot(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = self.run('exec-out', 'screencap', '-p', binary=True)
        path.write_bytes(data)


def ensure_emulator(sdk: Path, avd: str, output: Path) -> tuple[str, subprocess.Popen | None]:
    adb = sdk / 'platform-tools/adb'
    emulator = sdk / 'emulator/emulator'
    for line in subprocess.run([str(adb), 'devices'], capture_output=True, text=True).stdout.splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 2 and parts[1] == 'device' and parts[0].startswith('emulator-'):
            name = subprocess.run([str(adb), '-s', parts[0], 'emu', 'avd', 'name'],
                                  capture_output=True, text=True).stdout.splitlines()
            if name and name[0].strip() == avd:
                return parts[0], None
    log(f'启动模拟器 {avd}...')
    with (output / 'emulator-boot.log').open('w') as log_file:
        proc = subprocess.Popen(
            [str(emulator), '-avd', avd, '-no-window', '-no-boot-anim', '-no-audio', '-no-snapshot-save', '-gpu', 'auto'],
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
        if proc.poll() is not None:
            raise RuntimeError('模拟器提前退出，见 emulator-boot.log')
        time.sleep(2)
    raise RuntimeError('等待模拟器启动超时')


class Results:
    def __init__(self, output: Path):
        self.output = output
        self.entries: list[dict] = []

    def check(self, item: str, name: str, passed: bool, detail=None) -> None:
        self.entries.append({'item': item, 'name': name, 'passed': bool(passed), 'detail': detail})
        log(f'[{"PASS" if passed else "FAIL"}] {item}: {name}')

    def summary(self) -> dict:
        by_item: dict[str, bool] = {}
        for e in self.entries:
            by_item[e['item']] = by_item.get(e['item'], True) and e['passed']
        return {
            'passed': all(e['passed'] for e in self.entries),
            'total': len(self.entries),
            'passedCount': sum(1 for e in self.entries if e['passed']),
            'byItem': by_item,
            'items': self.entries,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description='M2b Android 设置页、字号刷新与缓存清理验收')
    parser.add_argument('--apk', type=Path, default=ROOT / 'build/android/hortus-debug.apk')
    parser.add_argument('--avd', default='Medium_Phone_API_36.1')
    parser.add_argument('--serial', default=None)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/android-m2b')
    parser.add_argument('--media', type=Path, default=None)
    parser.add_argument('--keep-emulator', action='store_true')
    args = parser.parse_args()

    output = args.output.expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.media.expanduser().resolve() if args.media else output / 'media')
    media.mkdir(parents=True, exist_ok=True)
    sdk = find_sdk_root()
    adb = sdk / 'platform-tools/adb'
    if args.serial:
        serial, emu_proc = args.serial, None
    else:
        serial, emu_proc = ensure_emulator(sdk, args.avd, output)

    device = Device(adb, serial, output)
    results = Results(output)
    stage = Path(tempfile.mkdtemp(prefix='hortus-m2b-vault-'))
    try:
        copy_tree(ROOT / 'fixtures/demo-vault', stage)
        (stage / 'M2b check.md').write_text(M2B_NOTE, encoding='utf-8')
        (stage / 'WideTableMath.md').write_text(WIDE_TABLE_MATH_NOTE, encoding='utf-8')
        (stage / 'EmbedTarget.md').write_text(EMBED_TARGET_NOTE, encoding='utf-8')
        (stage / 'EmbedSyncB.md').write_text(EMBED_SYNC_B_NOTE, encoding='utf-8')
        (stage / 'EmbedSyncA.md').write_text(EMBED_SYNC_A_NOTE, encoding='utf-8')
        (stage / 'LongScroll.md').write_text(LONG_SCROLL_NOTE, encoding='utf-8')
        (stage / 'BrokenZh.md').write_text(BROKEN_ZH_NOTE, encoding='utf-8')
        baseline = tree_hashes(stage)
        (output / 'vault-baseline.json').write_text(json.dumps(baseline, indent=2) + '\n')

        device.shell(f'rm -rf {DEVICE_VAULT}')
        device.shell(f'mkdir -p {DEVICE_VAULT}')
        for item in sorted(stage.iterdir()):
            device.run('push', str(item), f'{DEVICE_VAULT}/')

        device.install(args.apk.expanduser().resolve())
        device.shell(f'pm clear {PACKAGE}', check=False)
        device.grant()
        device.night(False)
        device.shell(f'cmd locale set-app-locales {PACKAGE} --locales en-US', check=False)

        # 1. 首页进入设置并检查 About 字段（仅名称/版本/AGPL-3.0-only/仓库链接），以及存储根目录拦截（M4-17）
        device.start(cold=True)
        time.sleep(1.5)
        home_nodes = device.ui_dump('home-ui.xml')
        device.tap_node(home_nodes, text='Open folder')
        picker_nodes = device.ui_dump('home-picker-root.xml')
        use_btn = next((n for n in picker_nodes if n['text'] == 'Use this folder'), None)
        still_open = False
        if use_btn is not None:
            device.tap(use_btn['cx'], use_btn['cy'], wait=0.4)
            after_tap_nodes = device.ui_dump('home-picker-after-tap.xml')
            still_open = any(n['text'] == 'Use this folder' for n in after_tap_nodes)
        results.check(
            'A2',
            'home folder picker blocks storage root selection',
            use_btn is not None and use_btn.get('enabled') is False and still_open,
            {'button': use_btn, 'stillOpenAfterTap': still_open},
        )
        device.screenshot(media / 'home-root-blocked.png')
        device.key(4)
        time.sleep(0.5)
        home_nodes = device.ui_dump('home-ui-after-picker.xml')
        device.tap_node(home_nodes, desc='Settings')
        settings_light_nodes = device.ui_dump('settings-light-ui.xml')
        texts = [n['text'] for n in settings_light_nodes if n['text']]
        has_about = (
            'Hortus' in texts
            and any('1.0.0 (' in t for t in texts)
            and 'AGPL-3.0-only' in texts
            and 'https://github.com/groundgrounder/Hortus' in texts
        )
        results.check('A2', 'home settings entry and minimal about metadata', has_about, texts)
        device.screenshot(media / 'settings-light.png')
        device.key(4)

        # 2. 阅读页 -> 更多 -> 设置，切换深色主题并验证阅读页/全屏页同步及冷重启保留
        base_17 = device.dump('m2b-base-17.json', note='M2b check.md', cold=True, wait_ms=1600)
        device.screenshot(media / 'reader-font-17.png')

        reader_nodes = device.ui_dump('reader-ui.xml')
        device.tap_node(reader_nodes, desc='More options')
        menu_nodes = device.ui_dump('reader-more-ui.xml')
        device.tap_node(menu_nodes, text='Settings')

        s_nodes = device.ui_dump('settings-before-dark.xml')
        device.tap_node(s_nodes, text='Dark')
        time.sleep(0.8)
        device.screenshot(media / 'settings-dark.png')
        device.key(4)
        time.sleep(0.8)

        dark_dump = device.dump('m2b-theme-dark.json', wait_ms=800)
        dark_settings = dark_dump.get('settings', {})
        results.check(
            'A2',
            'dark theme applies immediately in reader',
            dark_settings.get('themeMode') == 'dark' and dark_settings.get('resolvedDark') is True,
            dark_settings,
        )

        # 冷重启保留深色主题
        restarted_dark = device.dump('m2b-theme-dark-restarted.json', note='M2b check.md', cold=True, wait_ms=1600)
        rs = restarted_dark.get('settings', {})
        results.check(
            'A2',
            'dark theme persists across cold restart',
            rs.get('themeMode') == 'dark' and rs.get('resolvedDark') is True and rs.get('settingsFileExists') is True,
            rs,
        )

        # 验证 DiagramFocusActivity 响应主题
        rect = (restarted_dark.get('diagramRects') or [{}])[0]
        if rect:
            device.shell(f'rm -f {REMOTE_DIR}/focus/focus-state.json', check=False)
            cx = rect['x'] + rect['width'] / 2
            cy = rect['y'] + rect['height'] / 2
            device.tap(cx, cy, wait=1.5)
            focus_local = output / 'focus-dark.json'
            pulled_focus = device.pull(f'{REMOTE_DIR}/focus/focus-state.json', focus_local)
            focus_data = json.loads(focus_local.read_text()) if pulled_focus else {}
            results.check(
                'A2',
                'diagram focus follows persisted dark theme',
                focus_data.get('themeMode') == 'dark' and focus_data.get('resolvedDark') is True,
                focus_data,
            )
            device.key(4)
            time.sleep(0.6)
        else:
            results.check('A2', 'diagram focus follows persisted dark theme', False, 'missing diagramRects')

        # 浅色 & 跟随系统
        device.dump('m2b-set-light.json', action='set_theme', extra=['--es', 'debug_theme', 'light'], wait_ms=600)
        device.night(True)
        light_in_night = device.dump('m2b-light-in-night.json', wait_ms=800).get('settings', {})
        results.check(
            'A2',
            'light theme overrides system night mode',
            light_in_night.get('themeMode') == 'light' and light_in_night.get('resolvedDark') is False,
            light_in_night,
        )
        device.dump('m2b-set-system.json', action='set_theme', extra=['--es', 'debug_theme', 'system'], wait_ms=600)
        sys_night = device.dump('m2b-system-night.json', wait_ms=800).get('settings', {})
        device.night(False)
        sys_day = device.dump('m2b-system-day.json', wait_ms=800).get('settings', {})
        results.check(
            'A2',
            'system theme follows system night toggle',
            sys_night.get('resolvedDark') is True and sys_day.get('resolvedDark') is False,
            {'night': sys_night, 'day': sys_day},
        )

        # 3. 通过设置 UI 调节字号 17 sp -> 21 sp，检查原子重排、图内字号比例与冷重启保留
        reader_nodes = device.ui_dump('reader-font-ui.xml')
        device.tap_node(reader_nodes, desc='More options')
        menu_nodes = device.ui_dump('reader-font-more.xml')
        device.tap_node(menu_nodes, text='Settings')
        for _ in range(4):
            cur_nodes = device.ui_dump('settings-font-step.xml')
            device.tap_node(cur_nodes, desc='Increase font size')
        after_step_nodes = device.ui_dump('settings-font-21.xml')
        has_21 = any(n['text'] == '21 sp' for n in after_step_nodes)
        device.key(4)
        time.sleep(1.2)

        font_21 = device.dump('m2b-font-21.json', wait_ms=1600)
        device.screenshot(media / 'reader-font-21.png')

        px_17 = base_17.get('bodySizePx', 0.0)
        px_21 = font_21.get('bodySizePx', 0.0)
        previews_17 = base_17.get('inlinePreviews', [])
        previews_21 = font_21.get('inlinePreviews', [])
        diag_17 = previews_17[0].get('drawnMainFontPx', 0.0) if previews_17 else 0.0
        diag_21 = previews_21[0].get('drawnMainFontPx', 0.0) if previews_21 else 0.0
        diag_ratio_21 = (diag_21 / px_21) if px_21 > 0 else 0.0
        math_17 = [m.get('em', 0.0) for m in base_17.get('math', []) if m.get('rendered')]
        math_21 = [m.get('em', 0.0) for m in font_21.get('math', []) if m.get('rendered')]

        results.check(
            'A2',
            'font size 17->21 sp updates text, math, and inline diagram in ratio 0.90..1.10',
            has_21
            and font_21.get('settings', {}).get('fontSizeSp') == 21.0
            and px_21 > px_17 * 1.15
            and diag_21 > diag_17
            and 0.90 <= diag_ratio_21 <= 1.10
            and bool(math_17) and bool(math_21)
            and max(math_21) > max(math_17),
            {
                'has21Label': has_21,
                'px17': px_17,
                'px21': px_21,
                'diag17': diag_17,
                'diag21': diag_21,
                'diagRatio21': diag_ratio_21,
                'mathMax17': max(math_17) if math_17 else None,
                'mathMax21': max(math_21) if math_21 else None,
            },
        )

        # 冷重启验证 21 sp 保留
        restarted_21 = device.dump('m2b-font-21-restarted.json', note='M2b check.md', cold=True, wait_ms=1600)
        r21_previews = restarted_21.get('inlinePreviews', [])
        r21_px = restarted_21.get('bodySizePx', 0.0)
        r21_diag = r21_previews[0].get('drawnMainFontPx', 0.0) if r21_previews else 0.0
        r21_ratio = (r21_diag / r21_px) if r21_px > 0 else 0.0
        results.check(
            'A2',
            'font size 21 sp persists across cold restart',
            restarted_21.get('settings', {}).get('fontSizeSp') == 21.0
            and abs(r21_px - px_21) < 0.5
            and 0.90 <= r21_ratio <= 1.10,
            {'fontSizeSp': restarted_21.get('settings', {}).get('fontSizeSp'), 'px': r21_px, 'ratio': r21_ratio},
        )

        # 4. 清理缓存：先确认存在预览缓存，点击“Clear cache”并检查确认提示、缓存归零与配置保留
        preview_cached_before = restarted_21.get('diagramPreview', {}).get('cached', 0)
        r_nodes = device.ui_dump('reader-cache-ui.xml')
        device.tap_node(r_nodes, desc='More options')
        m_nodes = device.ui_dump('reader-cache-more.xml')
        device.tap_node(m_nodes, text='Settings')
        c_nodes = device.ui_dump('settings-cache-ui.xml')
        device.tap_node(c_nodes, text='Clear cache')
        time.sleep(0.8)
        cleared_nodes = device.ui_dump('settings-cache-cleared-ui.xml')
        has_cleared_toast = any('Cache cleared' in n['text'] for n in cleared_nodes)
        device.key(4)
        after_clear = device.dump('m2b-after-clear.json', wait_ms=600)
        ac_settings = after_clear.get('settings', {})
        results.check(
            'A2',
            'clear cache clears caches without deleting settings',
            preview_cached_before > 0
            and has_cleared_toast
            and ac_settings.get('cacheDiagramExportsCount') == 0
            and ac_settings.get('cacheExternalPreviewCount') == 0
            and ac_settings.get('settingsFileExists') is True
            and ac_settings.get('fontSizeSp') == 21.0,
            {
                'previewCachedBefore': preview_cached_before,
                'hasClearedToast': has_cleared_toast,
                'after': ac_settings,
            },
        )

        # 4b. 宽表与超宽公式横滑（M4-15）
        wide_dump = device.dump('m2b-wide-table-math.json', note='WideTableMath.md', cold=True, wait_ms=1600)
        tables = wide_dump.get('tableLayouts', [])
        bmath = wide_dump.get('blockMath', [])
        wide_table_ok = bool(tables) and sum(tables[0].get('columns', [])) > wide_dump.get('contentWidth', 0)
        wide_math_ok = bool(bmath) and any(
            f.get('width', 0) > wide_dump.get('contentWidth', 0)
            for item in bmath for f in item.get('formulas', [])
        )
        device.shell('input swipe 900 500 200 500 220')
        time.sleep(0.4)
        device.screenshot(media / 'wide-table-math-scroll.png')
        results.check(
            'A2',
            'wide table and wide block math overflow content width with horizontal scroll',
            wide_table_ok and wide_math_ok,
            {'wideTableOk': wide_table_ok, 'wideMathOk': wide_math_ok},
        )

        # 4c. 嵌入图 documentKey 与全屏折叠同步（M4-12）
        embed_dump = device.dump('m2b-embed-sync.json', note='EmbedSyncA.md', cold=True, wait_ms=1800)
        embed_rect = (embed_dump.get('diagramRects') or [{}])[0]
        embed_sync_ok = False
        embed_detail: dict = {'rect': embed_rect}
        if embed_rect:
            device.shell(f'rm -f {REMOTE_DIR}/focus/focus-state.json', check=False)
            device.tap(embed_rect['x'] + embed_rect['width'] / 2, embed_rect['y'] + embed_rect['height'] / 2, wait=1.5)
            f_local = output / 'embed-focus-state.json'
            pulled_f = device.pull(f'{REMOTE_DIR}/focus/focus-state.json', f_local)
            f_data = json.loads(f_local.read_text()) if pulled_f else {}
            note_key_ok = f_data.get('notePath') == 'EmbedSyncB.md'
            collapse_hit = next((h for h in f_data.get('hits', []) if h.get('canCollapse')), None)
            collapsed_after: list[str] = []
            if collapse_hit:
                scale = float(f_data.get('scale', 1.0))
                unit = float(f_data.get('unitScale', 1.0))
                ox = float(f_data.get('offsetX', 0.0))
                oy = float(f_data.get('offsetY', 0.0))
                tx = ox + (collapse_hit['x'] + collapse_hit['width'] / 2.0) * scale * unit
                ty = oy + (collapse_hit['y'] + collapse_hit['height'] / 2.0) * scale * unit
                device.tap(tx, ty, wait=1.2)
                f_local2 = output / 'embed-focus-collapsed.json'
                if device.pull(f'{REMOTE_DIR}/focus/focus-state.json', f_local2):
                    collapsed_after = json.loads(f_local2.read_text()).get('collapsed', [])
            device.key(4)
            time.sleep(0.8)
            device.screenshot(media / 'embed-diagram-sync.png')
            after_embed_dump = device.dump('m2b-embed-sync-after.json', wait_ms=800)
            inline_collapsed = [
                p.get('collapsed', [])
                for p in after_embed_dump.get('inlinePreviews', [])
                if 'EmbedSyncB.md' in p.get('key', '')
            ]
            embed_sync_ok = note_key_ok and bool(collapsed_after) and any(c == collapsed_after for c in inline_collapsed)
            embed_detail = {
                'notePath': f_data.get('notePath'),
                'collapsedAfter': collapsed_after,
                'inlineCollapsed': inline_collapsed,
            }
        results.check('A2', 'embedded diagram uses real notePath and syncs collapse state', embed_sync_ok, embed_detail)

        # 4d. 640ms 滚动不劫持（M4-19）：从 M2b check 跳转至 LongScroll#Section 25（热切换触发 heading 定位）
        device.dump('m2b-scroll-primer.json', note='M2b check.md', cold=False, wait_ms=600)
        anchor_dump = device.dump('m2b-scroll-at-target.json', note='LongScroll.md', cold=False,
                                  extra=['--es', 'debug_heading', 'Section%2025'], wait_ms=900)
        anchor_block = anchor_dump.get('firstVisibleBlockId')
        device.dump('m2b-scroll-reset.json', note='M2b check.md', cold=False, wait_ms=600)
        device.start(note='LongScroll.md', cold=False, extra=['--es', 'debug_heading', 'Section%2025'])
        time.sleep(0.25)
        device.shell('input swipe 540 500 540 1600 120')
        time.sleep(0.85)
        after_swipe = device.dump('m2b-scroll-after-swipe.json', wait_ms=200)
        swiped_block = after_swipe.get('firstVisibleBlockId')
        results.check(
            'A2',
            'anchor scroll does not hijack manual swipe within 640ms',
            anchor_block is not None and swiped_block is not None and anchor_block > 10 and swiped_block < anchor_block,
            {'anchorBlock': anchor_block, 'swipedBlock': swiped_block},
        )

        # 4e. 中文渲染失败提示本地化（M4-19）
        device.shell(f'cmd locale set-app-locales {PACKAGE} --locales zh-CN', check=False)
        device.dump('m2b-zh-errors.json', note='BrokenZh.md', cold=True, wait_ms=1800)
        zh_nodes = device.ui_dump('zh-errors-ui.xml')
        zh_texts = [n['text'] for n in zh_nodes if n['text']]
        device.screenshot(media / 'zh-render-errors.png')
        has_zh_diagram = any('无法解析图表语法' in t or '图表未能' in t or '无法显示图表' in t for t in zh_texts)
        has_zh_image = any('找不到图片文件' in t for t in zh_texts)
        has_zh_math = any('无法渲染公式' in t for t in zh_texts)
        no_raw_codes = not any(
            code in t for t in zh_texts for code in ('diagram.', 'image.', 'math.')
        )
        results.check(
            'A2',
            'Chinese UI localizes diagram, image, and math failure messages without raw codes',
            has_zh_diagram and has_zh_image and has_zh_math and no_raw_codes,
            {'zhTexts': zh_texts[:30]},
        )
        device.shell(f'cmd locale set-app-locales {PACKAGE} --locales en-US', check=False)

        # 5. 检查笔记文件夹零写入
        pulled_vault = output / 'pulled-vault'
        if pulled_vault.exists():
            shutil.rmtree(pulled_vault)
        device.pull(DEVICE_VAULT, pulled_vault)
        after_hashes = tree_hashes(pulled_vault)
        results.check('A2', 'note folder untouched (zero writes)', after_hashes == baseline, {
            'beforeCount': len(baseline),
            'afterCount': len(after_hashes),
        })

        # 恢复默认字号与主题
        device.dump('m2b-reset-font.json', action='set_font_size', extra=['--ef', 'debug_font_size', '17.0'], wait_ms=400)
        device.dump('m2b-reset-theme.json', action='set_theme', extra=['--es', 'debug_theme', 'system'], wait_ms=400)

        summary = results.summary()
        (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        index = ['# M2b Android 设置与字号验收证据', '', f'- 设备：{serial}', f'- APK：{args.apk}', f'- 媒体：{media}', '']
        for item, passed in summary['byItem'].items():
            index.append(f'## {item}: {"通过" if passed else "未通过"}')
            for e in summary['items']:
                if e['item'] == item:
                    index.append(f'- [{"x" if e["passed"] else " "}] {e["name"]}: {e["detail"]}')
            index.append('')
        (output / 'index.md').write_text('\n'.join(index) + '\n')
        return 0 if summary['passed'] else 1
    finally:
        shutil.rmtree(stage, ignore_errors=True)
        if emu_proc is not None and not args.keep_emulator:
            subprocess.run([str(adb), '-s', serial, 'emu', 'kill'], capture_output=True, timeout=30)


if __name__ == '__main__':
    raise SystemExit(main())
