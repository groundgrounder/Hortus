#!/usr/bin/env python3
"""P1-35 回滚真实窗口检查。

用公开假 Agent 在快照后真实写入完整改动集，再通过 Changes 面板模型验证：
按处回滚（CRLF/无末尾换行、多 hunk）、stale 拒绝、干净/脏打开标签页保护、
按文件与按会话回滚（含嵌套类型变化与部分 stale）、显式快照清理与活动会话
拒绝、双库隔离、大库后台响应与空闲 CPU，以及中英浅深色截图。
"""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
DEBUG_APP = ROOT / 'apps/macos/.build/debug/Hortus'
FAKE_AGENT = ROOT / 'tools/fixtures/rollback-agent.sh'


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
    import shutil
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
           if key not in ('LANG', 'LC_ALL', 'LC_CTYPE') and not key.startswith('ROLLBACK_AGENT')}
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'),
               HORTUS_DATA_DIR=str(base / 'data'),
               FAKE_AGENT_SCRATCH=str(base / 'scratch'))
    if extra:
        env.update(extra)
    for name in ('cache', 'data', 'scratch'):
        (base / name).mkdir(parents=True, exist_ok=True)
    return env


def write_config(base):
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text(
        'font_size = 15.0\ndefault_view = "reading"\n\n[agent]\n'
        f'command = {json.dumps(str(FAKE_AGENT))}\n'
        'args = ["--guide", "{guide}"]\n',
        encoding='utf-8')
    (config / 'agent-guide.md').write_text('P1-35 rollback guide\n', encoding='utf-8')


def seed_vault(vault):
    vault.mkdir(parents=True, exist_ok=True)
    (vault / 'Assets').mkdir(exist_ok=True)
    (vault / 'EmptyDir').mkdir(exist_ok=True)
    (vault / '.git').mkdir(exist_ok=True)
    (vault / 'Edit.md').write_text('alpha\nbeta\ngamma\n', encoding='utf-8')
    (vault / 'Rename.md').write_text('stable rename\n', encoding='utf-8')
    (vault / 'Shift.md').write_text('one\ntwo\nthree\nfour\nfive\nsix\n', encoding='utf-8')
    (vault / 'Delete.md').write_text('delete this\n', encoding='utf-8')
    (vault / 'Mode.md').write_text('mode\n', encoding='utf-8')
    (vault / 'Type.md').write_text('type changes\n', encoding='utf-8')
    (vault / 'Keep.md').write_text('# Keep\n\nNo change.\n', encoding='utf-8')
    (vault / '.hidden.md').write_text('hidden\n', encoding='utf-8')
    (vault / 'Assets/picture.png').write_bytes(bytes(range(256)))
    (vault / '.git/config').write_text('[core]\n', encoding='utf-8')
    (vault / 'LinkEdit').symlink_to('Edit.md')
    (vault / 'LinkDrop').symlink_to('Keep.md')
    (vault / 'Hunks.md').write_bytes(
        b'one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nTEN')
    (vault / 'OpenClean.md').write_text('clean baseline\n', encoding='utf-8')
    (vault / 'OpenDirty.md').write_text('dirty baseline\n', encoding='utf-8')
    locked = vault / 'Locked.md'
    locked.write_text('locked\n', encoding='utf-8')
    os.chmod(locked, 0o400)
    subprocess.run(['chflags', 'uchg', str(locked)], check=True)
    readonly = vault / 'ReadOnly.md'
    readonly.write_text('readonly\n', encoding='utf-8')
    os.chmod(readonly, 0o444)


def seed_large(vault):
    notes = vault / 'notes'
    notes.mkdir(parents=True, exist_ok=True)
    payload = b'x' * (32 * 1024)
    for index in range(1000):
        (notes / f'{index:04d}.md').write_bytes(payload)


def run(base, vault, mode, result_name, extra=(), env_extra=None, timeout=300,
        app=None, screenshot=None):
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
    if screenshot:
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


def idle_cpu(base, vault, output):
    result = base / 'idle.json'
    result.unlink(missing_ok=True)
    command = [str(DEBUG_APP), f'--vault={vault}', '--agent-mode=rollback-idle',
               f'--agent-launch-inspection={result}', '--inspect-hold-open']
    process = subprocess.Popen(command, env=environment(base),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for(result, process, 180)
        time.sleep(2)
        raw = subprocess.check_output(
            ['top', '-l', '6', '-s', '1', '-pid', str(process.pid), '-stats', 'pid,cpu,threads'],
            text=True, timeout=20)
        (output / 'rollback-idle-top.txt').write_text(raw)
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
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/agent-rollback')
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

    vaults = []
    try:
        with tempfile.TemporaryDirectory(prefix='hortus-rollback-') as temporary:
            base = Path(temporary)
            report = {}

            def make(name):
                base_dir = base / name
                vault = base_dir / 'vault'
                seed_vault(vault)
                write_config(base_dir)
                vaults.append(vault)
                return base_dir, vault

            hunks_base, hunks_vault = make('hunks')
            hunks = run(hunks_base, hunks_vault, 'rollback-hunks', 'rollback-hunks.json')
            save(output, 'rollback-hunks.json', hunks)
            report['hunks'] = hunks
            assert hunks['initial_hunks'] == 2, hunks
            assert hunks['remaining_hunks'] == 1, hunks
            assert hunks['stale_rejected'] is True, hunks
            assert hunks['open_clean_text'] == 'clean baseline\n', hunks
            assert hunks['dirty_preserved'] is True, hunks

            files_base, files_vault = make('files')
            files = run(files_base, files_vault, 'rollback-files', 'rollback-files.json')
            save(output, 'rollback-files.json', files)
            report['files'] = files
            assert files['png_bytes'] == 256, files
            assert files['mode_after'] == '644', files
            assert set(files['remaining']) == {
                'Assets/added.png', 'Hunks.md', 'LinkNew',
                'OpenClean.md', 'OpenDirty.md',
            }, files
            assert (files_vault / 'Rename.md').is_file(), files
            assert not (files_vault / 'Renamed.md').exists(), files
            assert not (files_vault / 'Type.md').is_dir(), files

            session_base, session_vault = make('session')
            session = run(session_base, session_vault, 'rollback-session', 'rollback-session.json')
            save(output, 'rollback-session.json', session)
            report['session'] = session
            assert session['skipped_stale'] == 1, session
            assert session['failed'] == 0, session
            assert session['reverted'] == session['initial_entries'] - 1, session
            assert session['remaining'] == ['Shifted.md'], session
            assert 'disk changed' in session['notice'].lower(), session

            cleanup_base, cleanup_vault = make('cleanup')
            cleanup = run(cleanup_base, cleanup_vault, 'rollback-cleanup', 'rollback-cleanup.json')
            save(output, 'rollback-cleanup.json', cleanup)
            report['cleanup'] = cleanup
            assert cleanup['snapshot_deleted'] is True, cleanup
            assert cleanup['source_flags'] is True, cleanup
            assert 'history.session_active' in cleanup['running_error'], cleanup
            assert (cleanup_vault / 'Locked.md').is_file(), cleanup
            assert (cleanup_vault / 'Locked.md').read_text() == 'locked\n', cleanup

            two_base, two_vault = make('two')
            second_base = base / 'two' / 'second'
            second_vault = second_base / 'vault'
            seed_vault(second_vault)
            write_config(second_base)
            vaults.append(second_vault)
            two = run(two_base, two_vault, 'rollback-two-windows', 'rollback-two-windows.json',
                      [f'--rollback-second-vault={second_vault}'],
                      env_extra={'ROLLBACK_AGENT_MODE': 'second'})
            save(output, 'rollback-two-windows.json', two)
            report['two_windows'] = two
            assert two['isolated'] is True, two
            assert two['second_entries'] == 1, two

            idle_base, idle_vault = make('idle')
            idle = idle_cpu(idle_base, idle_vault, output)
            report['idle'] = idle
            assert idle['median_cpu_percent'] < 3.0, idle

            large_base = base / 'large'
            large_vault = large_base / 'vault'
            seed_large(large_vault)
            write_config(large_base)
            vaults.append(large_vault)
            large = run(large_base, large_vault, 'rollback-large', 'rollback-large.json',
                        env_extra={'ROLLBACK_AGENT_MODE': 'large'}, timeout=600)
            save(output, 'rollback-large.json', large)
            report['large'] = large
            assert large['entries'] >= 100, large
            assert large['reverted'] >= 100, large
            assert large['max_main_gap_ms'] < 500, large
            assert large['failed'] == 0, large

            assert args.bundle.is_dir(), f'missing release bundle {args.bundle}'
            bundle = debug_bundle(base, args.bundle.resolve(), args.debug_app.resolve())
            screenshots = {}
            for language in ('en', 'zh-Hans'):
                for theme in ('light', 'dark'):
                    for kind, mode in (('controls', 'rollback-screenshot'),
                                       ('result', 'rollback-screenshot-result')):
                        name = f'{language}-{theme}-{kind}.png'
                        shot_base = base / f'shot-{language}-{theme}-{kind}'
                        shot_vault = shot_base / 'vault'
                        seed_vault(shot_vault)
                        write_config(shot_base)
                        vaults.append(shot_vault)
                        data = run(shot_base, shot_vault, mode, f'screenshot-{name}.json', [
                            '-AppleLanguages', f'({language})', f'--{theme}',
                        ], app=bundle, screenshot=media / name)
                        assert (media / name).stat().st_size > 10_000, name
                        screenshots[name] = {
                            'entries': data['entries'],
                            'selected': data['selected'],
                            'notice': data['notice'],
                        }
            report['screenshots'] = screenshots

            (output / 'summary.json').write_text(
                json.dumps(report, indent=2, ensure_ascii=False, sort_keys=True) + '\n',
                encoding='utf-8')
            print(json.dumps({
                'hunks': {'initial': hunks['initial_hunks'], 'remaining': hunks['remaining_hunks'],
                          'stale_rejected': hunks['stale_rejected']},
                'files_remaining': len(files['remaining']),
                'session': {'reverted': session['reverted'], 'stale': session['skipped_stale']},
                'cleanup': cleanup['snapshot_deleted'],
                'two_windows': two['second_entries'],
                'idle_cpu_percent': idle['median_cpu_percent'],
                'large': {'reverted': large['reverted'], 'max_main_gap_ms': large['max_main_gap_ms']},
                'screenshots': len(screenshots),
            }, indent=2, ensure_ascii=False))
    finally:
        # 源库里的 uchg 是检查夹具的一部分；临时目录清理前解除，避免残留。
        for vault in vaults:
            locked = vault / 'Locked.md'
            if locked.exists():
                subprocess.run(['chflags', 'nouchg', str(locked)], capture_output=True)


if __name__ == '__main__':
    main()
