#!/usr/bin/env python3
"""Agent 安全启动的真实应用检查。

覆盖：每窗口多标签隔离、结构化 argv 与指南、全库快照范围与保真（clone 与
强制完整复制）、快照失败闭锁、迟到回调、会话持久化与重启恢复。

应用使用 Debug 可执行文件，因为启动闸门的检查入口只在 Debug 构建里存在。
先运行 ./tools/test-macos.sh 生成 apps/macos/.build/debug/Hortus。
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
import importlib.util
SPEC = importlib.util.spec_from_file_location('helpers', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)
DEBUG_APP = ROOT / 'apps/macos/.build/debug/Hortus'
FAKE_AGENT = ROOT / 'tools/fixtures/fake-agent.sh'
DEMO = ROOT / 'fixtures/demo-vault'
TRICKY_GUIDE = ('Guide line one with spaces, "quotes", ; semicolons and $(touch PWNED).\n'
                '第二行：中文、制表符\t和新的一行\n'
                'Last line without trailing newline')


def environment(base, extra_path=None, extra=None):
    env = {key: value for key, value in os.environ.items()
           if key not in ('LANG', 'LC_ALL', 'LC_CTYPE') and not key.startswith('FAKE_AGENT')}
    zdot = base / 'zdotdir'
    zdot.mkdir(parents=True, exist_ok=True)
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(base / 'config'),
               HORTUS_CACHE_DIR=str(base / 'cache'),
               HORTUS_DATA_DIR=str(base / 'data'),
               FAKE_AGENT_SCRATCH=str(base / 'scratch'))
    if extra_path:
        env['PATH'] = f'{extra_path}:{env["PATH"]}'
    if extra:
        env.update(extra)
    return env


def toml_string(value):
    return json.dumps(value, ensure_ascii=False)


def write_config(base, command=None, args=None, comment='# user comment stays'):
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    text = f'font_size = 15.0\ndefault_view = "reading"\n{comment}\n'
    if command is not None:
        text += '\n[agent]\n' + f'command = {toml_string(command)}\n'
        if isinstance(args, list):
            text += 'args = [' + ', '.join(toml_string(a) for a in args) + ']\n'
        else:
            text += f'args = {args}\n'
    (config / 'config.toml').write_text(text)
    return config


def write_guide(config, text):
    config.mkdir(parents=True, exist_ok=True)
    (config / 'agent-guide.md').write_text(text)


def default_agent_args(scratch, sleep=30, extra=None):
    args = ['--guide', '{guide}', '--literal', 'a b; $(touch PWNED)', '--quote', "q'\"d",
            '--multi', 'line one\nline two', '--sleep=%d' % sleep]
    if extra:
        args.extend(extra)
    return args


def run_app(base, vault, mode, extra=(), env_extra=None, output=None, timeout=120, screenshot=None):
    result_path = (output or base) / f'agent-{mode}.json'
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.unlink(missing_ok=True)
    if screenshot:
        screenshot.parent.mkdir(parents=True, exist_ok=True)
        screenshot.unlink(missing_ok=True)
    command = [str(DEBUG_APP), '--light', f'--vault={vault}', f'--agent-mode={mode}',
               f'--agent-launch-inspection={result_path}',
               f'--agent-scratch={base / "scratch"}', *extra]
    if screenshot:
        command += [f'--snapshot={screenshot}', '--inspect-hold-open']
    process = subprocess.Popen(command, env=environment(base, extra=env_extra),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        HELPERS.wait_for(result_path, process, seconds=timeout)
        data = json.loads(result_path.read_text())
        # 截图运行保持窗口；其余由应用在检查结束后自行退出。
        if screenshot:
            HELPERS.wait_for(screenshot, process, seconds=60)
            process.terminate()
            process.wait(timeout=30)
            return data, None
        _, stderr = process.communicate(timeout=60)
        if process.returncode != 0:
            raise AssertionError(f'{mode}: 应用退出码 {process.returncode}: {stderr.decode(errors="replace")}')
        return data, None
    finally:
        if process.poll() is None and not screenshot:
            process.terminate()
            process.wait(timeout=30)


def schedule_writer(path, seconds=6):
    """在快照期间持续修改源文件；用于证明起止清单检测。"""
    script = (
        'end=$((SECONDS + %d)); while [ $SECONDS -lt $end ]; do '
        'printf "append\\n" >> %s; sleep 0.001; done' % (seconds, str(path).replace(' ', '\\ '))
    )
    return subprocess.Popen(['/bin/sh', '-c', script])


def describe(root, skip_git=True):
    result = {}

    def walk(path, relative):
        with os.scandir(path) as entries:
            for entry in sorted(entries, key=lambda item: item.name):
                if skip_git and entry.name == '.git':
                    continue
                name = os.path.join(relative, entry.name) if relative else entry.name
                if entry.is_symlink():
                    result[name] = ('symlink', os.readlink(entry.path))
                elif entry.is_dir(follow_symlinks=False):
                    result[name] = ('directory', None)
                    walk(entry.path, name)
                else:
                    with open(entry.path, 'rb') as handle:
                        digest = hashlib.sha256(handle.read()).hexdigest()
                    result[name] = ('file', digest)
    walk(root, '')
    return result


def compare_snapshot(vault, snapshot):
    source = describe(vault)
    copied = describe(snapshot)
    assert copied == source, (
        'snapshot differs',
        sorted(set(source) - set(copied))[:5],
        sorted(set(copied) - set(source))[:5],
        [key for key in source if source.get(key) != copied.get(key)][:5],
    )
    for name in source:
        if source[name][0] == 'symlink':
            continue
        src = os.stat(os.path.join(vault, name), follow_symlinks=False)
        dst = os.stat(os.path.join(snapshot, name), follow_symlinks=False)
        assert stat.S_IMODE(src.st_mode) == stat.S_IMODE(dst.st_mode), name
        assert src.st_mtime_ns == dst.st_mtime_ns, name
    return source


def build_fixture(vault, outside):
    (vault / 'nested' / 'deeper').mkdir(parents=True)
    (vault / 'empty folder').mkdir()
    (vault / '.obsidian').mkdir()
    (vault / 'nested' / '.git').mkdir()
    (vault / '.git' / 'refs').mkdir(parents=True)
    (vault / 'notes.md').write_text('# Notes\n\nPlain text.\n')
    (vault / '中文 笔记.md').write_text('# 中文\n')
    (vault / 'nested' / 'deeper' / 'space name.md').write_text('line\n')
    (vault / 'empty.txt').write_text('')
    (vault / '.obsidian' / 'workspace.json').write_text('{}')
    (vault / '.hidden').write_text('hidden\n')
    (vault / '.git' / 'config').write_text('[core]\n')
    (vault / 'nested' / '.git' / 'HEAD').write_text('ref\n')
    (vault / 'attachment.bin').write_bytes(bytes(range(256)))
    script = vault / 'run.sh'
    script.write_text('#!/bin/sh\necho hi\n')
    script.chmod(0o755)
    outside.write_text('outside\n')
    os.symlink(outside, vault / 'outside-link')
    os.symlink('notes.md', vault / 'relative-link')


def manifest_for(session):
    return json.loads((Path(session['snapshot_path']).parent / 'manifest.json').read_text())


def metadata_for(session):
    """会话元数据原文；同时证明没有持久化 argv 或完整环境。"""
    return json.loads((Path(session['snapshot_path']).parent / 'session.json').read_text())


def assert_no_launch(data, scratch, label):
    assert data['sessions'] == [], (label, data['sessions'])
    assert not any(scratch.rglob('*')), (label, list(scratch.rglob('*')))
    assert data['shell_alive'], label


def main():
    global DEBUG_APP
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/agent-launch')
    parser.add_argument('--media', type=Path)
    parser.add_argument('--app', type=Path, default=DEBUG_APP)
    parser.add_argument('--skip-copy-path', action='store_true')
    parser.add_argument('--rounds', type=int, default=3)
    args = parser.parse_args()
    DEBUG_APP = args.app.resolve()
    assert DEBUG_APP.exists(), f'missing debug app {DEBUG_APP}; run ./tools/test-macos.sh first'
    output = args.output.resolve()
    media = (args.media or output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    summary = {}
    details = {}

    with tempfile.TemporaryDirectory(prefix='hortus-agent-') as temporary:
        base = Path(temporary)

        # A2: 结构化 argv、指南覆盖、PATH 解析与错误闭锁。
        case = base / 'argv'
        vault = case / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        scratch = case / 'scratch'
        config = write_config(case, str(FAKE_AGENT), default_agent_args(scratch))
        write_guide(config, TRICKY_GUIDE)
        data, held = run_app(case, vault, 'launch')
        assert held is None
        values = data['scratch']
        assert values['argc'] == "9", values
        assert values['arg-1'] == '--guide' and values['arg-2'] == TRICKY_GUIDE, values
        assert values['arg-4'] == 'a b; $(touch PWNED)'
        assert values['arg-6'] == "q'\"d"
        assert values['arg-8'] == 'line one\nline two'
        assert os.path.realpath(values['cwd'].strip()) == os.path.realpath(vault)
        assert os.path.realpath(values['vault']) == os.path.realpath(vault)
        session = data['sessions'][0]
        assert values['session'] == session['id']
        assert values['term'].startswith('xterm')
        assert values['lang'].endswith('.UTF-8')
        assert not list(case.rglob('PWNED')) and not list(vault.rglob('PWNED'))
        assert session['state'] in ('running', 'exited'), session
        assert session['method'] == 'clonefile', session
        assert data['max_main_gap_ms'] < 100, data['max_main_gap_ms']
        summary['argv'] = {'argc': values['argc'], 'guide_chars': len(TRICKY_GUIDE),
                           'session': session['id'], 'method': session['method'],
                           'max_main_gap_ms': data['max_main_gap_ms']}
        details['argv'] = {'scratch': values, 'session': session, 'tabs': data['tabs']}

        # 裸命令名按 PATH 解析。
        case = base / 'path-resolution'
        vault = case / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        config = write_config(case, 'fake-agent.sh', ['--guide', '{guide}', '--sleep=5'])
        write_guide(config, 'path guide')
        data, _ = run_app(case, vault, 'launch', env_extra={'PATH': f'{FAKE_AGENT.parent}:{os.environ["PATH"]}'})
        assert data['tabs']
        assert data['scratch']['arg-2'] == 'path guide'
        summary['path_resolution'] = data['sessions'][0]['id']

        # 错误模板：全部在快照与 exec 之前失败，普通 shell 仍可用。
        failures = {
            'not-configured': (None, None),
            'command-missing': ('', ['{guide}']),
            'guide-missing': (str(FAKE_AGENT), ['--sleep=0']),
            'relative-command': ('relative/fake-agent', ['{guide}']),
            'missing-command': (str(base / 'missing-agent'), ['{guide}']),
            'not-executable': (str(base / 'not-executable'), ['{guide}']),
            'invalid-args': (str(FAKE_AGENT), 7),
        }
        records = {}
        details['config_failures'] = {}
        for name, (command, agent_args) in failures.items():
            for round_index in range(args.rounds):
                case = base / f'fail-{name}-{round_index}'
                vault = case / 'vault'
                vault.mkdir(parents=True)
                (vault / 'note.md').write_text('# Note\n')
                config = write_config(case, command, agent_args)
                if name == 'not-executable':
                    (base / 'not-executable').write_text('#!/bin/sh\nexit 0\n')
                    (base / 'not-executable').chmod(0o644)
                if name == 'missing-command':
                    (config / 'agent-guide.md').write_text('guide')
                if name == 'relative-command':
                    (config / 'agent-guide.md').write_text('guide')
                data, _ = run_app(case, vault, 'fail')
                assert data['sessions'] == [], (name, data['sessions'])
                assert not any((case / 'scratch').rglob('*')), (name, list((case / 'scratch').rglob('*')))
                assert data['shell_alive'], name
                assert data['tabs'] and data['tabs'][0]['session_state'].endswith('running'), data['tabs']
                if round_index == 0:
                    records[name] = data.get('failure', '')
                details['config_failures'].setdefault(name, []).append({
                    'failure': data.get('failure', ''),
                    'sessions': data['sessions'],
                    'scratch_empty': not any((case / 'scratch').rglob('*')),
                    'shell_alive': data['shell_alive'],
                })
            assert all(records.values()), records
        summary['config_failures'] = records

        # A1: 同窗口两个 shell + 一个 Agent 标签；真实路径连续 3 轮。
        tab_runs = []
        for round_index in range(args.rounds):
            case = base / f'tabs-{round_index}'
            vault = case / 'vault'
            vault.mkdir(parents=True)
            (vault / 'note.md').write_text('# Note\n')
            write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
            screenshot = media / 'terminal-tabs.png' if round_index == 0 else None
            data, _ = run_app(case, vault, 'tabs', screenshot=screenshot,
                              env_extra={'HORTUS_AGENT_SESSION': 'leak-marker'})
            pids = data['shell_pids']
            assert pids[0] and pids[1] and pids[0] != pids[1], pids
            assert data['buffers_distinct']
            assert data['switch_pids'][:2] == pids, data['switch_pids']
            assert data['dock_pids'][:2] == pids
            assert data['switch_markers'] and data['dock_markers']
            assert data['focus_isolated'] and data['clear_isolated']
            assert data['closed_dead'] and data['other_shell_alive'] and data['agent_alive']
            agent_pid = [tab for tab in data['tabs'] if tab['kind'] == 'agent'][0]['pid']
            assert agent_pid > 0 and data['buffers_after_close']
            assert os.path.realpath(data['shell_cwds'][0]) == os.path.realpath(vault)
            assert os.path.realpath(data['shell_cwds'][1]) == os.path.realpath(vault)
            for pid in pids + [agent_pid, data['closed_tab_pid']]:
                deadline = time.monotonic() + 5
                while _alive(pid) and time.monotonic() < deadline:
                    time.sleep(0.05)
                assert not _alive(pid), pid
            tab_runs.append({'shell_pids': pids, 'agent_pid': agent_pid,
                             'closed': data['closed_tab_pid'],
                             'max_main_gap_ms': data['max_main_gap_ms'],
                             'all_processes_gone': True})
            details.setdefault('tabs', []).append(data)
        summary['tabs'] = {'rounds': len(tab_runs), 'runs': tab_runs}

        # A1: 两个不同库窗口各两个标签仍隔离；真实路径连续 3 轮。
        window_runs = []
        for round_index in range(args.rounds):
            two = base / f'two-windows-{round_index}'
            vault = two / 'a'
            vault.mkdir(parents=True)
            (vault / 'a.md').write_text('# A\n')
            other = two / 'b'
            other.mkdir()
            (other / 'b.md').write_text('# B\n')
            write_config(two, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
            two_data, _ = run_app(two, vault, 'two-windows', extra=[f'--agent-second-vault={other}'])
            assert two_data['distinct_pids'], two_data
            assert len(set(two_data['cwds'])) == 2, two_data
            assert os.path.realpath(two_data['cwds'][0]) == os.path.realpath(vault)
            assert os.path.realpath(two_data['cwds'][2].strip()) == os.path.realpath(other)
            assert two_data['closed_dead'] and all(two_data['remaining_alive']), two_data
            window_runs.append({'pids': two_data['pids'], 'closed': two_data['closed_pid'],
                                'remaining_alive': two_data['remaining_alive']})
            details.setdefault('two_windows', []).append(two_data)
        summary['two_windows'] = {'rounds': len(window_runs), 'runs': window_runs}

        # Agent 准备状态与失败状态的可见证据。
        preparing_case = base / 'preparing'
        vault = preparing_case / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        write_config(preparing_case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
        preparing, _ = run_app(preparing_case, vault, 'preparing',
                               extra=['--agent-gate-delay=8'],
                               screenshot=media / 'agent-preparing.png')
        assert preparing['snapshot_ready_before_exec'], preparing
        assert preparing['sessions'] and preparing['sessions'][0]['state'] == 'ready', preparing
        assert not any((preparing_case / 'scratch').rglob('*')), 'exec happened before the snapshot gate'
        summary['preparing'] = {'state': preparing['sessions'][0]['state'],
                                'exec_after_ready': not any((preparing_case / 'scratch').rglob('*'))}

        failed_case = base / 'visible-failure'
        vault = failed_case / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        write_config(failed_case)
        failed, _ = run_app(failed_case, vault, 'fail', screenshot=media / 'agent-failed.png')
        assert failed.get('failure'), failed
        assert failed['shell_alive'], failed
        summary['visible_failure'] = failed['failure']

        # A3: 全库快照范围与保真（APFS clone）。
        case = base / 'snapshot'
        vault = case / 'vault'
        vault.mkdir(parents=True)
        build_fixture(vault, case / 'outside.txt')
        before = describe(vault)
        write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
        data, _ = run_app(case, vault, 'launch')
        session = data['sessions'][0]
        snapshot = Path(session['snapshot_path'])
        assert session['method'] == 'clonefile', session
        assert describe(vault) == before, 'source changed during snapshot'
        compare_snapshot(vault, snapshot)
        entries = manifest_for(session)
        by_path = {entry['path']: entry for entry in entries}
        assert by_path['empty folder']['kind'] == 'directory'
        assert by_path['outside-link']['kind'] == 'symlink'
        assert Path(by_path['outside-link']['link']).is_absolute()
        assert by_path['attachment.bin']['size'] == 256
        assert by_path['run.sh']['mode'] & 0o777 == 0o755
        assert all('.git' not in entry['path'].split('/') for entry in entries)
        assert not (snapshot / 'outside.txt').exists()
        assert not list(vault.rglob('agent-sessions')) and not list(vault.rglob('manifest.json'))
        assert not str(snapshot).startswith(str(vault))
        summary['snapshot_clone'] = {'entries': session['entries'], 'bytes': session['bytes'],
                                     'method': session['method'], 'files': len(entries)}
        metadata = metadata_for(session)
        assert 'argv' not in json.dumps(metadata) and 'PATH' not in json.dumps(metadata)
        details['snapshot_clone'] = {'session': session, 'metadata': metadata,
                                     'manifest': entries, 'source_unchanged': True}

        # 第二次启动写新会话，旧快照字节不变。
        first_snapshot = Path(session['snapshot_path'])
        first_bytes = describe(first_snapshot)
        data, _ = run_app(case, vault, 'restart')
        sessions = data['sessions']
        ids = [item['id'] for item in sessions]
        # A3 的第一次启动、restart 的第一次启动与重新启动各有一个独立会话。
        assert len(ids) == 3 and len(set(ids)) == 3, sessions
        assert session['id'] in ids
        snapshot_paths = {item['snapshot_path'] for item in sessions}
        assert len(snapshot_paths) == 3, sessions
        assert describe(first_snapshot) == first_bytes
        summary['snapshot_restart'] = ids
        details['snapshot_restart'] = sessions

        # A3 强制完整复制：不经过 clonefile 的同一范围复制路径。
        case = base / 'copy-path'
        vault = case / 'vault'
        vault.mkdir(parents=True)
        build_fixture(vault, case / 'outside.txt')
        write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
        data, _ = run_app(case, vault, 'launch', env_extra={'HORTUS_HISTORY_COPY': 'copy'})
        session = data['sessions'][0]
        assert session['method'] == 'copy', session
        compare_snapshot(vault, Path(session['snapshot_path']))
        summary['snapshot_copy'] = {'method': session['method'], 'entries': session['entries']}
        details['snapshot_copy'] = {'session': session, 'metadata': metadata_for(session),
                                    'manifest': manifest_for(session)}

        # A4: 快照失败闭锁。
        lockout = {}
        # 目标目录不可写。
        for round_index in range(args.rounds):
            case = base / f'lock-readonly-{round_index}'
            vault = case / 'vault'
            vault.mkdir(parents=True)
            (vault / 'note.md').write_text('# Note\n')
            data_dir = case / 'data'
            data_dir.mkdir()
            data_dir.chmod(0o500)
            write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
            try:
                data, _ = run_app(case, vault, 'fail')
            finally:
                data_dir.chmod(0o700)
            assert data.get('failure'), data
            assert not any((case / 'scratch').rglob('*'))
            assert data['sessions'] == []
            assert data['shell_alive']
            details.setdefault('lockout', {}).setdefault('unwritable', []).append({
                'failure': data.get('failure', ''), 'sessions': data['sessions'],
                'scratch_empty': not any((case / 'scratch').rglob('*')),
                'shell_alive': data['shell_alive'],
            })
        lockout['unwritable'] = '3/3'

        # 源文件不可读：默认 clonefile 与强制完整复制都必须整体失败。
        for path_name, env_extra in [('clonefile', None), ('copy', {'HORTUS_HISTORY_COPY': 'copy'})]:
            for round_index in range(args.rounds):
                case = base / f'lock-unreadable-{path_name}-{round_index}'
                vault = case / 'vault'
                vault.mkdir(parents=True)
                (vault / 'note.md').write_text('# Note\n')
                locked = vault / 'locked.md'
                locked.write_text('secret\n')
                locked.chmod(0)
                write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
                try:
                    data, _ = run_app(case, vault, 'fail', env_extra=env_extra)
                finally:
                    locked.chmod(0o600)
                assert data.get('failure'), (path_name, data)
                assert 'cannot be read' in data['failure'], (path_name, data)
                assert not any((case / 'scratch').rglob('*')), (path_name, list((case / 'scratch').rglob('*')))
                assert data['shell_alive'], path_name
                assert data['sessions'] and data['sessions'][0]['state'] == 'failed', (path_name, data)
                details.setdefault('lockout', {}).setdefault(f'unreadable-{path_name}', []).append({
                    'failure': data.get('failure', ''),
                    'sessions': data['sessions'],
                    'scratch_empty': not any((case / 'scratch').rglob('*')),
                    'shell_alive': data['shell_alive'],
                })
            lockout[f'unreadable-{path_name}'] = '3/3'

        # 特殊文件、复制期间源变化。
        for name, prepare, writer in [
            ('special', lambda vault: subprocess.run(['mkfifo', str(vault / 'pipe')], check=True), None),
            ('source-change', lambda vault: (vault / 'growing.md').write_text('start\n'),
             lambda vault: schedule_writer(vault / 'growing.md')),
        ]:
            for round_index in range(args.rounds):
                case = base / f'lock-{name}-{round_index}'
                vault = case / 'vault'
                vault.mkdir(parents=True)
                (vault / 'note.md').write_text('# Note\n')
                if name == 'source-change':
                    (vault / 'many.md').write_text('x' * 2000000)
                    for index in range(40):
                        (vault / f'bulk-{index}.bin').write_bytes(bytes([index % 256]) * 524288)
                prepare(vault)
                write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
                background = writer(vault) if writer else None
                extra = {'HORTUS_HISTORY_COPY': 'copy'} if name == 'source-change' else None
                try:
                    data, _ = run_app(case, vault, 'fail', env_extra=extra)
                finally:
                    if background:
                        background.kill()
                        background.wait()
                assert data.get('failure'), (name, data)
                assert not any((case / 'scratch').rglob('*')), (name, list((case / 'scratch').rglob('*')))
                assert data['shell_alive'], name
                details.setdefault('lockout', {}).setdefault(name, []).append({
                    'failure': data.get('failure', ''),
                    'sessions': data['sessions'],
                    'scratch_empty': not any((case / 'scratch').rglob('*')),
                    'shell_alive': data['shell_alive'],
                })
            lockout[name] = '3/3'
        summary['lockout'] = lockout

        # 迟到回调：关闭标签、换库、关窗、退出。
        late = {}
        for name in ['close-tab', 'vault', 'window', 'quit']:
            for round_index in range(args.rounds):
                case = base / f'late-{name}-{round_index}'
                vault = case / 'vault'
                vault.mkdir(parents=True)
                (vault / 'note.md').write_text('# Note\n')
                second = case / 'second'
                second.mkdir()
                (second / 'other.md').write_text('# Other\n')
                write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=30'])
                extra = [f'--agent-second-vault={second}']
                data, _ = run_app(case, vault, f'late-{name}', extra=extra)
                assert data.get('sessions'), (name, data)
                session = data['sessions'][0]
                if name == 'quit':
                    # 退出在快照完成后的等待窗口开始；应用退出时同步标记作废。
                    assert session['state'] in ('ready', 'cancelled'), (name, session)
                    history, _ = run_app(case, vault, 'history')
                    assert history['sessions'][0]['state'] == 'cancelled', (name, history['sessions'])
                else:
                    assert session['state'] in ('cancelled', 'exited', 'interrupted'), (name, session)
                assert not any((case / 'scratch').rglob('*')), (name, list((case / 'scratch').rglob('*')))
                details.setdefault('late_callbacks', {}).setdefault(name, []).append({
                    'session': session,
                    'scratch_empty': not any((case / 'scratch').rglob('*')),
                })
            late[name] = '3/3'
        summary['late_callbacks'] = late

        # A5: 正常/异常退出、崩溃恢复、历史不自动执行。
        exit_case = base / 'exit'
        vault = exit_case / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        write_config(exit_case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=0'])
        data, _ = run_app(exit_case, vault, 'exit')
        assert data['exit_code'] == 0, data
        assert data['sessions'][0]['state'] == 'exited'
        normal_snapshot = Path(data['sessions'][0]['snapshot_path'])
        normal_bytes = describe(normal_snapshot)

        abnormal = base / 'abnormal'
        vault = abnormal / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        write_config(abnormal, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=0'])
        data, _ = run_app(abnormal, vault, 'exit', env_extra={'FAKE_AGENT_EXIT_CODE': '7'})
        assert data['exit_code'] == 7, data
        assert data['sessions'][0]['state'] == 'exited'

        # 运行中崩溃：SIGKILL 应用后重启，记录转为中断且不自动执行。
        crash = base / 'crash'
        vault = crash / 'vault'
        vault.mkdir(parents=True)
        (vault / 'note.md').write_text('# Note\n')
        write_config(crash, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=60'])
        result_path = crash / 'running.json'
        command = [str(DEBUG_APP), '--light', f'--vault={vault}', '--agent-mode=launch',
                   f'--agent-launch-inspection={result_path}',
                   f'--agent-scratch={crash / "scratch"}']
        process = subprocess.Popen(command, env=environment(crash),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        HELPERS.wait_for(result_path, process, seconds=120)
        first = json.loads(result_path.read_text())
        os.kill(process.pid, signal.SIGKILL)
        process.wait(timeout=30)
        agent_pid = int((crash / 'scratch' / 'pid').read_text())
        try:
            os.kill(agent_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        shutil.rmtree(crash / 'scratch')
        history, _ = run_app(crash, vault, 'history')
        assert history['app_start_recovered'] >= 1, history
        recovered = [item for item in history['sessions'] if item['state'] == 'interrupted']
        assert recovered, history['sessions']
        assert not any((crash / 'scratch').rglob('*')), 'history inspection launched an agent'
        summary['recovery'] = {'recovered': history['app_start_recovered'],
                               'state': recovered[0]['state'],
                               'no_auto_launch': True}
        details['recovery'] = history

        # 跨库会话共用数据根时互不覆盖。
        multi = base / 'multi-vault'
        shared_data = multi / 'data'
        for name, builder in [('a', lambda v: (v / 'a.md').write_text('# A\n')),
                              ('b', lambda v: (v / 'b.md').write_text('# B\n'))]:
            case = multi / name
            vault = case / 'vault'
            vault.mkdir(parents=True)
            builder(vault)
            write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=0'])
            data, _ = run_app(case, vault, 'exit', env_extra={'HORTUS_DATA_DIR': str(shared_data)})
            assert data['exit_code'] == 0, data
        case = multi / 'history'
        case.mkdir()
        vault = multi / 'a' / 'vault'
        write_config(case, str(FAKE_AGENT), ['--guide', '{guide}', '--sleep=0'])
        history, _ = run_app(case, vault, 'history', env_extra={'HORTUS_DATA_DIR': str(shared_data)})
        sessions = history['sessions']
        assert len(sessions) == 2, sessions
        assert len({item['vault'] for item in sessions}) == 2, sessions
        assert len({item['snapshot_path'] for item in sessions}) == 2, sessions
        assert all(Path(item['snapshot_path']).is_dir() for item in sessions), sessions
        summary['cross_vault_sessions'] = [item['id'] for item in sessions]
        details['cross_vault'] = sessions

        # 恢复后快照字节不变。
        assert describe(normal_snapshot) == normal_bytes

        # A6: Release 可执行文件不含新增检查参数。
        release = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
        if release.exists():
            strings = subprocess.run(['strings', str(release)], capture_output=True, text=True).stdout
            for marker in ['--agent-launch-inspection', '--agent-mode', '--agent-scratch',
                           '--agent-gate-delay', '--agent-second-vault']:
                assert marker not in strings, marker
        summary['release_strings_clean'] = True

    details['release_strings_clean'] = summary['release_strings_clean']
    (output / 'details.json').write_text(json.dumps(details, ensure_ascii=False, indent=2) + '\n')
    (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(summary, ensure_ascii=False, indent=2), flush=True)


def _alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


if __name__ == '__main__':
    main()
