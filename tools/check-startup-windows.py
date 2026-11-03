#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check production LaunchServices windows, including every AX standard window.
Requires Accessibility and Screen Recording permission. --app accepts a copied
.app from a read-only release DMG; this tool does not use inspection switches.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--app', type=Path, default=ROOT / 'build/Hortus.app')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/startup-windows')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    media = (args.screenshots or output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    source = args.app.resolve()
    results = []
    with tempfile.TemporaryDirectory(prefix='hortus-startup-windows-') as directory:
        base = Path(directory)
        helper = base / 'probe'
        subprocess.run(['swiftc', '-parse-as-library', '-warnings-as-errors',
                        str(ROOT / 'tools/startup-window-probe.swift'), '-o', str(helper)], check=True)
        app = base / 'Hortus.app'
        shutil.copytree(source, app, symlinks=True)
        executable = app / 'Contents/MacOS/Hortus'
        vaults = [base / 'vault-a', base / 'vault-b']
        for vault, name in zip(vaults, ['a.md', 'b.md']):
            vault.mkdir()
            (vault / name).write_text('# Public startup fixture\n')
        state = base / 'state'

        def seed_two():
            cache = state / 'cache'
            cache.mkdir(parents=True, exist_ok=True)
            text = 'recent = []\n'
            for index, (vault, name) in enumerate(zip(vaults, ['a.md', 'b.md'])):
                text += f'\n[[windows]]\nid = "startup-{index}"\npath = {json.dumps(str(vault))}\n'
                text += f'tabs = ["{name}"]\nselected = "{name}"\n[windows.views]\n"{name}" = "reading"\n'
            (cache / 'session.toml').write_text(text)

        cases = [(f'explicit-{index}', [f'--vault={vaults[0]}', '--note=a.md'], ['a.md']) for index in range(1, 4)]
        cases += [('restore-two-1', [], ['a.md', 'b.md']), ('restore-two-2', [], ['a.md', 'b.md']),
                  ('empty', ['--empty'], ['Hortus'])]
        # 直接启动也必须创建首窗；不能依赖 LaunchServices 的激活事件。
        cases += [(f'direct-{index}', [f'--vault={vaults[0]}', '--note=a.md'], ['a.md']) for index in range(1, 4)]
        for name, arguments, titles in cases:
            if name in ('restore-two-1', 'empty'):
                seed_two()
            command = ['open', '-n', '-a', str(app), '--env', f'HORTUS_CONFIG_DIR={state / "config"}',
                       '--env', f'HORTUS_CACHE_DIR={state / "cache"}', '--args', *arguments, '--light']
            process = None
            if name.startswith('direct-'):
                command = [str(executable), *arguments, '--light']
                process = subprocess.Popen(command, env=dict(os.environ,
                    HORTUS_CONFIG_DIR=str(state / 'config'), HORTUS_CACHE_DIR=str(state / 'cache')))
            else:
                subprocess.run(command, check=True)
            pid = process.pid if process else None
            try:
                deadline = time.monotonic() + 15
                while pid is None and time.monotonic() < deadline:
                    found = subprocess.run(['pgrep', '-f', str(executable)], capture_output=True, text=True)
                    if found.returncode == 0:
                        pids = found.stdout.split()
                        assert len(pids) == 1, pids
                        pid = int(pids[0])
                        break
                    time.sleep(.05)
                assert pid is not None, 'LaunchServices process did not start'
                target = output / (name + '.json')
                subprocess.run([str(helper), str(pid), str(target), str(media / name)], check=True, timeout=30)
                data = json.loads(target.read_text())
                data.update(name=name, command=command, expected_titles=titles)
                standard = [[w for w in s['windows'] if w['AXSubrole'] == 'AXStandardWindow'] for s in data['samples']]
                data['standard_window_counts'] = [len(w) for w in standard]
                data['final_titles'] = sorted(w['AXTitle'] for w in standard[-1])
                target.write_text(json.dumps(data, indent=2) + '\n')
                results.append(data)
                assert max(data['standard_window_counts']) <= len(titles), data
                assert all(sorted(w['AXTitle'] for w in sample) == sorted(titles) for sample in standard[-10:]), data
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    exited = process.poll() is not None if process else subprocess.run(['kill', '-0', str(pid)], capture_output=True).returncode != 0
                    if exited:
                        pid = None
                        break
                    time.sleep(.1)
                assert pid is None, 'Process survived normal quit'
                print(name, 'PASS', data['final_titles'], flush=True)
            finally:
                if pid is not None:
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    if process:
                        process.wait(timeout=15)
        summary = {'app': str(source), 'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
                   'empty_behavior': '--empty opens one empty user window and ignores saved vault windows',
                   'cases': [{'name': d['name'], 'final_titles': d['final_titles'],
                              'max_standard_windows': max(d['standard_window_counts'])} for d in results]}
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
