#!/usr/bin/env python3
"""P1-50 统一工具标签：真实窗口证据与行为检查。

用 Debug 可执行文件（含自检入口）替换发布包的二进制，保留发布包的中英文本地化。
覆盖单行静态/Shell/Agent 标签、左/右/底部 + 菜单、终端跨区、两个窗口不同动态标签。
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
FAKE_AGENT = ROOT / 'tools/fixtures/fake-agent.sh'


def wait(path, process, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists():
            return
        if process.poll() is not None:
            _, stderr = process.communicate()
            raise AssertionError(f'app exited before writing {path}: {stderr.decode(errors="replace")}')
        time.sleep(0.1)
    raise AssertionError(f'timed out waiting for {path}')


def debug_bundle(base, release, debug):
    bundle = base / 'Hortus.app'
    shutil.copytree(release, bundle, symlinks=True)
    shutil.copy2(debug, bundle / 'Contents/MacOS/Hortus')
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)],
                   check=True, capture_output=True)
    return bundle


def run_language(language, prefix, bundle, base, media, output):
    vault = base / prefix / 'vault'
    second = base / prefix / 'second'
    for path in (vault, second):
        path.mkdir(parents=True, exist_ok=True)
        (path / 'note.md').write_text('# Unified tools\n\nFixture note.\n', encoding='utf-8')
    config = base / prefix / 'config'
    cache = base / prefix / 'cache'
    data = base / prefix / 'data'
    scratch = base / prefix / 'scratch'
    zdot = base / prefix / 'zdotdir'
    for path in (config, cache, data, scratch, zdot):
        path.mkdir(parents=True, exist_ok=True)
    (zdot / '.zshrc').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    (zdot / '.zprofile').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    (config / 'config.toml').write_text(
        'font_size = 15.0\ndefault_view = "reading"\n\n[agent]\n'
        f'command = {json.dumps(str(FAKE_AGENT))}\n'
        'args = ["--guide", "{guide}", "--sleep=600"]\n',
        encoding='utf-8')
    (config / 'agent-guide.md').write_text('P1-50 inspection guide\n', encoding='utf-8')
    env = {key: value for key, value in os.environ.items()
           if key not in ('LANG', 'LC_ALL', 'LC_CTYPE') and not key.startswith('FAKE_AGENT')}
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache),
               HORTUS_DATA_DIR=str(data), FAKE_AGENT_SCRATCH=str(scratch))
    target_media = media / prefix
    target_media.mkdir(parents=True, exist_ok=True)
    result = output / f'{prefix}.json'
    result.unlink(missing_ok=True)
    command = [
        str(bundle / 'Contents/MacOS/Hortus'),
        '-AppleLanguages', f'({language})',
        '--light',
        f'--vault={vault}',
        f'--tool-tabs-second={second}',
        f'--tool-tabs-inspection={result}',
        f'--tool-tabs-media={target_media}',
    ]
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait(result, process)
        data = json.loads(result.read_text())
        _, stderr = process.communicate(timeout=60)
        (output / f'{prefix}.log').write_bytes(stderr)
        if process.returncode != 0:
            raise AssertionError(f'{prefix}: app exit {process.returncode}: {stderr.decode(errors="replace")}')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=30)
    expected = ['single-row-tabs.png', 'plus-left.png', 'plus-right.png', 'plus-bottom.png',
                'terminal-right.png', 'two-windows.png']
    for name in expected:
        path = target_media / name
        assert path.is_file() and path.stat().st_size > 10_000, (name, path)
    assert data['single_row'] == {'shells': 2, 'agents': 1}, data
    assert data['terminal_moved_right'] and data['terminal_pid_preserved'], data
    assert data['static_drag'] == {'right': True, 'bottom': True}, data
    assert data['terminal_buffer_preserved'], data
    assert data['first_window_terminals'] >= 3, data
    assert data['second_window_terminals'] == 2, data
    assert data['second_window_terminals'] != data['first_window_terminals'], data
    assert len(data['plus_buttons']) == 3, data
    return data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/tool-tabs')
    parser.add_argument('--bundle', type=Path, default=ROOT / 'build/Hortus.app')
    parser.add_argument('--debug-app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--media', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    media = (args.media or output / 'media').resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    assert args.bundle.is_dir(), f'missing release bundle {args.bundle}; run ./tools/build-macos.sh'
    assert args.debug_app.is_file(), f'missing debug app {args.debug_app}; run ./tools/test-macos.sh'
    with tempfile.TemporaryDirectory(prefix='hortus-tool-tabs-') as temporary:
        base = Path(temporary)
        bundle = debug_bundle(base, args.bundle.resolve(), args.debug_app.resolve())
        summary = {}
        for language, prefix in (('en', 'en'), ('zh-Hans', 'zh-Hans')):
            summary[prefix] = run_language(language, prefix, bundle, base, media, output)
        (output / 'summary.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps({key: {'pids': [value['first_pid'], value['second_pid']],
                                'windows': [value['first_window_terminals'], value['second_window_terminals']]}
                          for key, value in summary.items()}, indent=2))


if __name__ == '__main__':
    main()
