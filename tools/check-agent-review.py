#!/usr/bin/env python3
"""P1-34 Changes 面板真实窗口检查。

用公开假 Agent 在快照后真实写入：新增、修改、删除、纯重命名、重命名并编辑、
二进制附件、符号链接、权限与类型变化；再验证逐行 diff、准备中会话隔离、
应用重启后的历史会话、双库多窗口隔离与大库后台扫描/取消。
截图使用发布包的本地化资源 + Debug 可执行文件。
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
DEBUG_APP = ROOT / 'apps/macos/.build/debug/Hortus'
FAKE_AGENT = ROOT / 'tools/fixtures/review-agent.sh'


def wait_for(path, process, seconds=180):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists():
            return
        if process.poll() is not None:
            _, stderr = process.communicate()
            raise AssertionError(f'app exited before writing {path}: {stderr.decode(errors="replace")}')
        time.sleep(0.1)
    _, stderr = process.communicate(timeout=20)
    raise AssertionError(f'timed out waiting for {path}: {stderr.decode(errors="replace")}')


def debug_bundle(base, release, debug):
    bundle = base / 'Hortus.app'
    shutil.copytree(release, bundle, symlinks=True)
    shutil.copy2(debug, bundle / 'Contents/MacOS/Hortus')
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)],
                   check=True, capture_output=True)
    return bundle


def environment(base, extra=None):
    zdot = base / 'zdotdir'
    zdot.mkdir(parents=True, exist_ok=True)
    (zdot / '.zshrc').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    (zdot / '.zprofile').write_text("PS1='❯ '\nRPROMPT=''\n", encoding='utf-8')
    env = {key: value for key, value in os.environ.items()
           if key not in ('LANG', 'LC_ALL', 'LC_CTYPE') and not key.startswith('FAKE_AGENT')}
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'),
               HORTUS_DATA_DIR=str(base / 'data'),
               HORTUS_SNAPSHOT_GATE=str(base / 'gate'),
               FAKE_AGENT_SCRATCH=str(base / 'scratch'))
    if extra:
        env.update(extra)
    (base / 'cache').mkdir(parents=True, exist_ok=True)
    (base / 'data').mkdir(parents=True, exist_ok=True)
    (base / 'gate').mkdir(parents=True, exist_ok=True)
    (base / 'scratch').mkdir(parents=True, exist_ok=True)
    return env


def write_config(base):
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text(
        'font_size = 15.0\ndefault_view = "reading"\n\n[agent]\n'
        f'command = {json.dumps(str(FAKE_AGENT))}\n'
        'args = ["--guide", "{guide}"]\n',
        encoding='utf-8')
    (config / 'agent-guide.md').write_text('P1-34 review guide\n', encoding='utf-8')


def seed_vault(vault):
    vault.mkdir(parents=True, exist_ok=True)
    (vault / 'Assets').mkdir(exist_ok=True)
    (vault / 'EmptyDir').mkdir(exist_ok=True)
    (vault / '.git').mkdir(exist_ok=True)
    (vault / 'Edit.md').write_text('alpha\nbeta\ngamma\n', encoding='utf-8')
    (vault / 'Rename.md').write_text('stable rename\n', encoding='utf-8')
    (vault / 'Shift.md').write_text('one\ntwo\nthree\nfour\nfive\nsix\n', encoding='utf-8')
    (vault / 'Delete.md').write_text('delete this\n', encoding='utf-8')
    (vault / 'Mode.md').write_text('mode stays\n', encoding='utf-8')
    (vault / 'Type.md').write_text('type changes\n', encoding='utf-8')
    (vault / 'Keep.md').write_text('# Keep\n\nNo change.\n', encoding='utf-8')
    (vault / '.hidden.md').write_text('hidden\n', encoding='utf-8')
    (vault / 'Assets/picture.png').write_bytes(bytes(range(256)))
    (vault / '.git/config').write_text('[core]\n', encoding='utf-8')
    (vault / 'LinkEdit').symlink_to('Edit.md')
    (vault / 'LinkDrop').symlink_to('Keep.md')


def seed_second(vault):
    vault.mkdir(parents=True, exist_ok=True)
    (vault / 'SecondEdit.md').write_text('# Second edit\n', encoding='utf-8')
    (vault / 'Keep.md').write_text('# Keep second\n', encoding='utf-8')


def seed_large(vault):
    notes = vault / 'notes'
    notes.mkdir(parents=True, exist_ok=True)
    payload = b'x' * (32 * 1024)
    for index in range(1000):
        (notes / f'{index:04d}.md').write_bytes(payload)


def run(base, vault, mode, result_name, extra=(), env_extra=None, timeout=300,
        app=None, screenshot=None, hold_open=False):
    result = base / result_name
    result.unlink(missing_ok=True)
    executable = app or DEBUG_APP
    if executable.suffix == '.app':
        executable = executable / 'Contents/MacOS/Hortus'
    command = [str(executable), f'--vault={vault}', f'--agent-mode={mode}',
               f'--agent-launch-inspection={result}',
               f'--agent-scratch={base / "scratch"}', *extra]
    if screenshot:
        screenshot.parent.mkdir(parents=True, exist_ok=True)
        screenshot.unlink(missing_ok=True)
        command += [f'--snapshot={screenshot}']
    if hold_open or screenshot:
        command += ['--inspect-hold-open']
    process = subprocess.Popen(command, env=environment(base, env_extra),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait_for(result, process, timeout)
        data = json.loads(result.read_text())
        if screenshot:
            wait_for(screenshot, process, 60)
            process.terminate()
            process.wait(timeout=30)
            return data
        _, stderr = process.communicate(timeout=60)
        (base / (result_name + '.log')).write_bytes(stderr)
        assert process.returncode == 0, (mode, process.returncode, stderr.decode(errors='replace'))
        return data
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=30)


def idle_cpu(base, vault, session, output):
    result = base / 'idle.json'
    result.unlink(missing_ok=True)
    command = [str(DEBUG_APP), f'--vault={vault}', '--agent-mode=review-history',
               f'--agent-launch-inspection={result}', f'--review-session={session}',
               '--inspect-hold-open']
    process = subprocess.Popen(command, env=environment(base),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for(result, process, 120)
        time.sleep(2)
        raw = subprocess.check_output(
            ['top', '-l', '6', '-s', '1', '-pid', str(process.pid), '-stats', 'pid,cpu,threads'],
            text=True, timeout=20)
        (output / 'review-idle-top.txt').write_text(raw)
        rows = [line for line in raw.splitlines() if line.strip().startswith(str(process.pid))]
        samples = [float(line.split()[1]) for line in rows[1:]]
        return {'samples': samples, 'median_cpu_percent': statistics.median(samples) if samples else 0.0}
    finally:
        process.terminate()
        process.wait(timeout=30)


def save(output, name, data):
    (output / name).write_text(
        json.dumps(data, indent=2, ensure_ascii=False, sort_keys=True) + '\n',
        encoding='utf-8')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/agent-review')
    parser.add_argument('--media', type=Path)
    parser.add_argument('--bundle', type=Path, default=ROOT / 'build/Hortus.app')
    parser.add_argument('--debug-app', type=Path, default=DEBUG_APP)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.media or output / 'media').resolve()
    media.mkdir(parents=True, exist_ok=True)
    assert args.debug_app.is_file(), f'missing debug app {args.debug_app}'
    assert FAKE_AGENT.is_file(), f'missing fake agent {FAKE_AGENT}'

    with tempfile.TemporaryDirectory(prefix='hortus-review-') as temporary:
        base = Path(temporary)
        vault = base / 'vault'
        second = base / 'second'
        seed_vault(vault)
        seed_second(second)
        write_config(base)
        report = {}
        changes = run(base, vault, 'review-changes', 'review-changes.json')
        save(output, 'review-changes.json', changes)
        report['changes'] = changes
        assert changes['summary_main_thread'] is False, changes
        assert changes['counts']['type_changed'] == 1, changes
        assert changes['counts']['renamed'] >= 3, changes
        assert changes['counts']['modified'] >= 3, changes
        assert changes['counts']['deleted'] >= 1, changes
        assert changes['text_hunks'] >= 1 and changes['text_lines'] >= 2, changes
        assert changes['binary_sizes'] == [256, 256], changes
        assert changes['link_targets'] == ['Edit.md', 'Keep.md'], changes
        assert changes['open_note_tabs'] == 1, changes
        assert changes['preparing_isolated'] is True, changes
        assert changes['session_state'] == 'exited', changes
        assert changes['session_exit_code'] == 0, changes
        assert not any('.git' in path for path in changes['entries']), changes
        assert 'Keep.md' not in changes['entries'], changes
        session = changes['session']
        assert session

        history = run(base, vault, 'review-history', 'review-history.json',
                      [f'--review-session={session}'])
        save(output, 'review-history.json', history)
        report['history'] = history
        assert history['entries'] > 0, history
        assert history['selected'] == session, history
        assert history['state'] in ('exited', 'interrupted'), history

        windows = run(base, vault, 'review-two-windows', 'review-two-windows.json',
                      [f'--review-session={session}', f'--review-second-vault={second}'])
        save(output, 'review-two-windows.json', windows)
        report['two_windows'] = windows
        assert windows['isolated'] is True, windows
        assert windows['first_root'] != windows['second_root'], windows
        assert windows['first_entries'] > 0 and windows['second_entries'] > 0, windows

        idle = idle_cpu(base, vault, session, output)
        report['idle'] = idle
        assert idle['median_cpu_percent'] < 3.0, idle

        # 大库：1,000 个 Markdown，约 32 MiB；后台扫描与取消。
        large_base = Path(temporary) / 'large'
        large_vault = large_base / 'vault'
        seed_large(large_vault)
        write_config(large_base)
        large = run(large_base, large_vault, 'review-large', 'review-large.json',
                    env_extra={'REVIEW_AGENT_MODE': 'large'}, timeout=600)
        save(output, 'review-large.json', large)
        report['large'] = large
        assert large['entries'] >= 100, large
        assert large['summary_main_thread'] is False, large
        assert large['max_main_gap_ms'] < 500, large
        assert 'history.cancelled' in large['pre_cancelled'], large

        # 中英浅深色截图：发布包的本地化资源 + Debug 可执行文件。
        assert args.bundle.is_dir(), f'missing release bundle {args.bundle}'
        bundle = debug_bundle(base, args.bundle.resolve(), args.debug_app.resolve())
        screenshots = {}
        for language in ('en', 'zh-Hans'):
            for theme in ('light', 'dark'):
                for kind, entry in (('diff', 'Edit.md'), ('binary', 'Assets/picture.png')):
                    name = f'{language}-{theme}-{kind}.png'
                    target = media / name
                    data = run(base, vault, 'review-screenshot', f'screenshot-{name}.json', [
                        f'--review-session={session}', f'--review-entry={entry}',
                        '-AppleLanguages', f'({language})', f'--{theme}',
                    ], app=bundle, screenshot=target)
                    assert target.is_file() and target.stat().st_size > 10_000, target
                    screenshots[name] = {'session': data['session'], 'entries': data['entries']}
        report['screenshots'] = screenshots

        (output / 'summary.json').write_text(
            json.dumps(report, indent=2, ensure_ascii=False, sort_keys=True) + '\n',
            encoding='utf-8')
        print(json.dumps({
            'changes': {'counts': changes['counts'], 'entries': len(changes['entries'])},
            'history_entries': history['entries'],
            'two_windows': [windows['first_entries'], windows['second_entries']],
            'large': {'entries': large['entries'], 'max_main_gap_ms': large['max_main_gap_ms']},
            'idle_cpu_percent': idle['median_cpu_percent'],
            'screenshots': len(screenshots),
        }, indent=2, ensure_ascii=False))


if __name__ == '__main__':
    main()
