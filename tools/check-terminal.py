#!/usr/bin/env python3
"""内置终端：真实窗口里经 PTY 输入命令，检查环境、文件树、阅读视图原地刷新、
切换知识库和退出应用后 shell 及其子进程都已结束，再测三种空闲 CPU。

shell 用 /bin/zsh，ZDOTDIR 指向空的临时目录，不读用户自己的 rc 文件，也不写用户的历史记录；
环境里去掉 LANG/LC_*，和从 Finder、程序坞启动时一样。
"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('helpers', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)
APP = HELPERS.APP
DEMO = ROOT / 'fixtures/demo-vault'
NEW_NOTE = '终端新建.md'
NEW_TITLE = '终端里写的笔记'


def environment(base):
    env = {key: value for key, value in HELPERS.environment(base).items()
           if key != 'LANG' and not key.startswith('LC_')}
    zdot = base / 'zdotdir'
    zdot.mkdir(parents=True, exist_ok=True)
    env.update(SHELL='/bin/zsh', ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'))
    (base / 'config').mkdir(parents=True, exist_ok=True)
    (base / 'config/config.toml').write_text('font_size = 15.0\ndefault_view = "reading"\n')
    return env


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def leftover_sleeps():
    found = subprocess.run(['pgrep', '-f', 'sleep 600\\.[0-9]{3}'], capture_output=True, text=True)
    return [int(pid) for pid in found.stdout.split()]


def lifecycle(base, output):
    first, second, scratch = base / 'first', base / 'second', base / 'scratch'
    for directory in (first, second, scratch):
        directory.mkdir(parents=True)
    paragraphs = ''.join(f'Paragraph {index:04d}: a line of plain text for the refresh check.\n\n' for index in range(1, 3001))
    (first / 'long.md').write_text('# Long note\n\n' + paragraphs)
    (second / 'other.md').write_text('# Other vault\n')
    second_before = sorted(path.relative_to(second).as_posix() for path in second.rglob('*'))
    result_path = output / 'lifecycle.json'
    result_path.unlink(missing_ok=True)
    command = [str(APP), f'--vault={first}', '--note=long.md', f'--terminal-inspection={result_path}',
               f'--terminal-scratch={scratch}', f'--terminal-second-vault={second}',
               f'--terminal-new-note={NEW_NOTE}', f'--terminal-new-title={NEW_TITLE}',
               f'--terminal-exit-snapshot={output / "terminal-exited.png"}']
    with subprocess.Popen(command, env=environment(base / 'state')) as process:
        HELPERS.wait_for(result_path, process, seconds=180)
        result = json.loads(result_path.read_text())
        quit_started = time.monotonic()
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            raise AssertionError('退出应用 30 s 内没有结束（终端停止流程可能卡住）')
        result['quit_s'] = time.monotonic() - quit_started
    pids = {name: result[name] for name in ['shell_pid', 'restarted_pid', 'job1_pid', 'shell2_pid', 'job2_pid', 'job3_pid']}
    deadline = time.monotonic() + 5
    while (any(alive(pid) for pid in pids.values()) or leftover_sleeps()) and time.monotonic() < deadline:
        time.sleep(0.1)
    result['alive_after_quit'] = {name: pid for name, pid in pids.items() if alive(pid)}
    result['leftover_sleeps'] = leftover_sleeps()
    result['scratch'] = {name: (scratch / name).read_text() for name in ['vault.txt', 'pwd.txt', 'lang.txt']}
    result['new_note'] = (first / NEW_NOTE).read_text()
    long_text = (first / 'long.md').read_text()
    result['second_unchanged'] = sorted(path.relative_to(second).as_posix() for path in second.rglob('*')) == second_before
    (output / 'lifecycle.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')

    real_first, real_second = os.path.realpath(first), os.path.realpath(second)
    assert result['shell_pid'] > 0 and os.path.realpath(result['shell_cwd']) == real_first, result
    assert os.path.realpath(result['scratch']['vault.txt']) == real_first, result
    assert os.path.realpath(result['scratch']['pwd.txt'].strip()) == real_first, result
    assert result['scratch']['lang.txt'].endswith('.UTF-8'), result
    assert result['new_note_tree_ms'] <= 1000, result
    assert result['new_note'] == f'# {NEW_TITLE}\n', result
    assert [item['case'] for item in result['refresh']] == ['sed_replace', 'append'], result
    for item in result['refresh']:
        assert item['same_anchor'] and item['anchor_shift_pt'] <= 1 and item['matches_disk'], item
    assert 'Paragraph 0400: rewritten from the terminal.' in long_text, 'sed 改写没有落盘'
    assert long_text.endswith('Paragraph 9999: appended from the terminal.\n'), '追加没有落盘'
    assert result['exit_stays_exited'], 'shell 退出后被界面刷新重新拉起'
    assert result['restarted_pid'] not in (0, result['shell_pid']), result
    assert os.path.realpath(result['shell2_cwd']) == real_second and result['shell2_pid'] != result['shell_pid'], result
    assert not result['alive_after_quit'] and not result['leftover_sleeps'], result
    assert result['second_unchanged'], result
    return result


def idle(base, output, mode=None, appearance='--light', snapshot=None):
    """mode 为 None 时测没有打开知识库的基线"""
    vault = base / 'vault'
    shutil.copytree(DEMO, vault)
    command = [str(APP), appearance]
    ready = output / f'idle-{mode or "empty"}.json'
    ready.unlink(missing_ok=True)
    if mode:
        command += [f'--vault={vault}', '--note=Start here.md', f'--terminal-idle={ready}', f'--terminal-idle-mode={mode}']
        if snapshot:
            command.append(f'--snapshot={snapshot}')
    with subprocess.Popen(command, env=environment(base / 'state')) as process:
        try:
            pids = [process.pid]
            if mode:
                HELPERS.wait_for(ready, process, seconds=60)
                evidence = json.loads(ready.read_text())
                pids.append(evidence['shell_pid'])
                if mode == 'unfocused':
                    subprocess.run(['open', '-a', 'Finder'], check=True)
            time.sleep(5)
            start = time.monotonic()
            before = sum(HELPERS.cpu_time(pid) for pid in pids)
            time.sleep(30)
            after = sum(HELPERS.cpu_time(pid) for pid in pids)
            elapsed = time.monotonic() - start
            return {'mode': mode or 'empty', 'wall_seconds': elapsed, 'cpu_delta_seconds': (after - before) / 1e9,
                    'average_cpu_percent': (after - before) / 1e9 / elapsed * 100, 'processes': len(pids)}
        finally:
            process.terminate()
            process.wait(timeout=30)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/terminal')
    parser.add_argument('--skip-cpu', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-terminal-') as temporary:
        base = Path(temporary)
        result = lifecycle(base / 'lifecycle', output)
        summary = {key: result[key] for key in ['new_note_tree_ms', 'switch_old_exit_ms', 'quit_s']}
        summary['refresh'] = result['refresh']
        summary['lang'] = result['scratch']['lang.txt']
        print(json.dumps(summary, ensure_ascii=False, indent=2), flush=True)
        if args.skip_cpu:
            return
        empty = idle(base / 'empty', output)
        cpu = {'empty': empty}
        for mode in ['hidden', 'unfocused', 'focused']:
            cpu[mode] = idle(base / mode, output, mode, snapshot=output / 'terminal-light.png' if mode == 'focused' else None)
            cpu[mode]['difference_percentage_points'] = cpu[mode]['average_cpu_percent'] - empty['average_cpu_percent']
        (output / 'idle-cpu.json').write_text(json.dumps(cpu, indent=2) + '\n')
        print(json.dumps(cpu, indent=2), flush=True)
        for mode in ['hidden', 'unfocused']:
            assert abs(cpu[mode]['difference_percentage_points']) <= .1, cpu


if __name__ == '__main__':
    main()
