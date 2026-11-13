#!/usr/bin/env python3
"""P1-53 工具标签文本二态：真实窗口窄宽度、反复改宽与上下文关闭检查。

用 Debug 可执行文件（含自检入口）替换发布包的二进制，保留发布包的中英文本地化。
覆盖静态/Shell/Agent、选中/未选中、浅色/深色、反复改宽、上下文菜单关闭与 + 菜单重加。
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
TITLES = {
    'en': {'static': 'Outline', 'shell': 'Shell', 'agent': 'Agent'},
    'zh-Hans': {'static': '大纲', 'shell': '终端', 'agent': 'Agent'},
}


def wait(path, process, seconds=180):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists():
            return
        if process.poll() is not None:
            raise AssertionError(f'app exited early with {process.returncode}')
        time.sleep(0.1)
    raise AssertionError(f'timed out waiting for {path}')


def debug_bundle(base, release, debug):
    bundle = base / 'Hortus.app'
    shutil.copytree(release, bundle, symlinks=True)
    shutil.copy2(debug, bundle / 'Contents/MacOS/Hortus')
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)],
                   check=True, capture_output=True)
    return bundle


def check_states(case, name, media, title):
    states = case['states']
    assert len(states) == 8, (name, len(states))
    seen = set()
    for state in states:
        assert state['title'] == title, (name, state['title'], title)
        seen.add((state['appearance'], state['width'], state['selected']))
        if state['title_visible']:
            assert state['title_is_full'], (name, state)
            assert state['title_width'] > 0, (name, state)
        if state['width'] == 'wide':
            assert state['title_visible'], (name, state)
        else:
            assert not state['title_visible'], (name, state)
        shot = Path(state['screenshot'])
        assert shot.is_file() and shot.stat().st_size > 10_000, (name, shot)
        assert media in shot.parents, (name, shot, media)
    assert seen == {(appearance, width, selected)
                    for appearance in ('light', 'dark')
                    for width in ('wide', 'narrow')
                    for selected in (True, False)}, (name, seen)
    for state in case['resize']:
        if state['title_visible']:
            assert state['title_is_full'], (name, state)
        if state['width'] == 'wide':
            assert state['title_visible'], (name, state)
        else:
            assert not state['title_visible'], (name, state)
    close = case['context_close']
    assert close['clicked'], (name, close)
    assert close['menu_visible'], (name, close)
    assert close['closed'], (name, close)
    menu = Path(close['menu_screenshot'])
    assert menu.is_file() and menu.stat().st_size > 10_000, (name, menu)


def check_accessibility(probe, language, prefix, bundle, base, output):
    """图标-only 状态下，Accessibility 仍暴露完整标题和关闭 action，且没有独立 X 按钮。"""
    titles = TITLES[language]
    root = base / f'{prefix}-hold'
    vault = root / 'vault'
    vault.mkdir(parents=True, exist_ok=True)
    (vault / 'note.md').write_text('# Tool labels\n\nFixture note.\n', encoding='utf-8')
    config = root / 'config'
    cache = root / 'cache'
    data = root / 'data'
    scratch = root / 'scratch'
    zdot = root / 'zdotdir'
    for path in (config, cache, data, scratch, zdot):
        path.mkdir(parents=True, exist_ok=True)
    (zdot / '.zshrc').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    (zdot / '.zprofile').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    (config / 'config.toml').write_text(
        'font_size = 15.0\ndefault_view = "reading"\n\n[agent]\n'
        f'command = {json.dumps(str(FAKE_AGENT))}\n'
        'args = ["--guide", "{guide}", "--sleep=600"]\n',
        encoding='utf-8')
    (config / 'agent-guide.md').write_text('P1-53 inspection guide\n', encoding='utf-8')
    env = {key: value for key, value in os.environ.items()
           if key not in ('LANG', 'LC_ALL', 'LC_CTYPE') and not key.startswith('FAKE_AGENT')}
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache),
               HORTUS_DATA_DIR=str(data), FAKE_AGENT_SCRATCH=str(scratch))
    result = output / f'{prefix}-hold.json'
    result.unlink(missing_ok=True)
    command = [
        str(bundle / 'Contents/MacOS/Hortus'),
        '-AppleLanguages', f'({language})',
        f'--vault={vault}',
        f'--tool-label-inspection={result}',
        f'--tool-label-media={output / (prefix + "-hold-media")}',
        '--tool-label-hold=1',
        '--inspect-hold-open',
    ]
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait(result, process)
        data_result = json.loads(result.read_text())
        time.sleep(0.6)
        trusted = json.loads(subprocess.run([str(probe), 'ax', str(process.pid)], check=True,
                                            capture_output=True).stdout)
        assert trusted.get('ax_trusted'), 'Accessibility 未授权；需在系统设置中允许终端使用辅助功能'
        tree = json.loads(subprocess.run([str(probe), 'tree', str(process.pid)], check=True,
                                         capture_output=True).stdout)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=30)
    assert data_result.get('hold'), data_result
    expected = [titles['static'], titles['shell'] + ' 2', titles['shell'] + ' 3']
    assert data_result['titles'] == expected, data_result
    assert data_result['titles_visible'] == [False, False, False], data_result
    elements = tree['elements']
    for title in expected:
        matches = [element for element in elements
                   if element.get('role') == 'AXButton' and element.get('description') == title]
        assert matches, (title, elements)
        tab = min(matches, key=lambda element: (element.get('size') or {}).get('width', 1e9))
        width = (tab.get('size') or {}).get('width', 0)
        assert width < 60, (title, width)
        actions = tab.get('actions') or []
        close_action = 'Name:Close ' if language == 'en' else 'Name:关闭 '
        assert any(action.startswith(close_action + title) for action in actions), (title, actions)
    close_prefix = 'Close ' if language == 'en' else '关闭 '
    stray = [element for element in elements
             if (element.get('description') or '').startswith(close_prefix)
             or (element.get('title') or '').startswith(close_prefix)]
    assert not stray, stray
    (output / f'{prefix}-ax.json').write_text(json.dumps(tree, indent=2, ensure_ascii=False) + '\n')
    return data_result


def run_language(language, prefix, bundle, base, media, output):
    vault = base / prefix / 'vault'
    vault.mkdir(parents=True, exist_ok=True)
    (vault / 'note.md').write_text('# Tool labels\n\nFixture note.\n', encoding='utf-8')
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
    (config / 'agent-guide.md').write_text('P1-53 inspection guide\n', encoding='utf-8')
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
        f'--vault={vault}',
        f'--tool-label-inspection={result}',
        f'--tool-label-media={target_media}',
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
    assert data['language'].startswith('en' if language == 'en' else 'zh'), data['language']
    assert data['failures'] == [], data['failures']
    cases = data['cases']
    titles = TITLES[language]
    for name in ('static', 'shell', 'agent'):
        assert name in cases, (name, cases)
        check_states(cases[name], name, target_media, titles[name])
    assert cases['static']['readded'], cases['static']
    for name in ('shell', 'agent'):
        case = cases[name]
        assert case['closed'], (name, case)
        assert case['target_dead'], (name, case)
        assert case['background_alive'], (name, case)
    assert cases['shell']['pid'] != cases['shell']['second_pid'], cases['shell']
    return data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/tool-labels')
    parser.add_argument('--bundle', type=Path, default=ROOT / 'build/Hortus.app')
    parser.add_argument('--debug-app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--media', type=Path)
    parser.add_argument('--probe-source', type=Path, default=ROOT / 'tools/editor-command-probe.swift')
    args = parser.parse_args()
    output = args.output.resolve()
    media = (args.media or output / 'media').resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    assert args.bundle.is_dir(), f'missing release bundle {args.bundle}; run ./tools/build-macos.sh'
    assert args.debug_app.is_file(), f'missing debug app {args.debug_app}; run ./tools/test-macos.sh'
    with tempfile.TemporaryDirectory(prefix='hortus-tool-labels-') as temporary:
        base = Path(temporary)
        probe = base / 'editor-command-probe'
        subprocess.run(['swiftc', '-parse-as-library', '-O', str(args.probe_source),
                        '-framework', 'Carbon', '-o', str(probe)], check=True)
        bundle = debug_bundle(base, args.bundle.resolve(), args.debug_app.resolve())
        summary = {}
        for language, prefix in (('en', 'en'), ('zh-Hans', 'zh-Hans')):
            summary[prefix] = run_language(language, prefix, bundle, base, media, output)
            summary[prefix]['accessibility'] = check_accessibility(probe, language, prefix, bundle, base, output)
        (output / 'summary.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps({prefix: {
            'titles': [value['cases'][name]['states'][0]['title'] for name in ('static', 'shell', 'agent')],
            'wide_text_visible': all(state['title_visible'] for state in value['cases']['static']['states']
                                     if state['width'] == 'wide'),
            'narrow_text_hidden': all(not state['title_visible'] for state in value['cases']['static']['states']
                                      if state['width'] == 'narrow'),
            'closed': [value['cases'][name]['context_close']['closed'] for name in ('static', 'shell', 'agent')],
        } for prefix, value in summary.items()}, indent=2, ensure_ascii=False))


if __name__ == '__main__':
    main()
