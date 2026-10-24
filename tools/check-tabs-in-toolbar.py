#!/usr/bin/env python3
"""P1-02c：真实工具栏结构、对齐、全屏屏幕截图和 30 s CPU 时间增量。"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
SAMPLE = ROOT / 'fixtures/vault-sample'


def environment(base):
    return dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'),
                HORTUS_CACHE_DIR=str(base / 'cache'))


def wait_for(path, process, seconds=20):
    deadline = time.monotonic() + seconds
    while not path.exists():
        if process.poll() is not None or time.monotonic() >= deadline:
            raise AssertionError(f'自检未生成 {path.name}')
        time.sleep(0.02)


def check_window(data, aligned=True):
    assert data['found'] and data['in_toolbar'] and not data['in_content'], data
    if aligned:
        assert abs(data['tabs_x'] - data['column_x']) <= 4, data
    else:
        # 间距来自系统：固定 spacer 在同一工具栏玻璃组中按默认规则处理。
        assert 0 <= data['tabs_x'] - data['sidebar_button_right'] <= 16, data
    assert data['panel_buttons_visible'], data
    end = data['tabs_x'] + data['tabs_width']
    assert end <= min(data['column_right'], data['panel_buttons_x']) + 1, data


def inspect(base, output, name, vault, notes, extra=(), screen=False):
    result_path = output / f'{name}.json'
    if result_path.exists():
        result_path.unlink()
    arguments = [str(APP), f'--vault={vault}', '--light',
                 *[f'--note={note}' for note in notes], *extra,
                 f'--toolbar-inspection={result_path}']
    if screen:
        arguments.append('--inspect-hold')
    else:
        arguments.append(f'--snapshot={output / (name + ".png")}')
    with subprocess.Popen(arguments, env=environment(base / name)) as process:
        try:
            wait_for(result_path, process)
            data = json.loads(result_path.read_text())
            if screen:
                assert data['fullscreen'], data
                # 独立窗口截图会拼回已滑出屏幕的标题栏，全屏必须截真实显示器。
                subprocess.run(['screencapture', '-x', '-D', '1', str(output / f'{name}.png')],
                               check=True, timeout=5)
            assert process.wait(timeout=20) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
    return data


def cpu_time(pid):
    library = ctypes.CDLL('/usr/lib/libproc.dylib')
    data = ctypes.create_string_buffer(96)
    count = library.proc_pidinfo(pid, 4, 0, data, len(data))
    assert count == len(data), count
    # Apple Silicon 的 proc_taskinfo 使用 Mach ticks，按本机 timebase 转成 ns。
    user, system = struct.unpack_from('QQ', data.raw, 16)
    timebase = (ctypes.c_uint32 * 2)()
    system_library = ctypes.CDLL('/usr/lib/libSystem.B.dylib')
    assert system_library.mach_timebase_info(ctypes.byref(timebase)) == 0
    return (user + system) * timebase[0] // timebase[1]


def idle_cpu(base, arguments):
    with subprocess.Popen([str(APP), *arguments], env=environment(base)) as process:
        try:
            ready = base / ('cache/session.toml' if arguments else 'config/config.toml')
            wait_for(ready, process)
            time.sleep(5)
            start = time.monotonic()
            before = cpu_time(process.pid)
            time.sleep(30)
            after = cpu_time(process.pid)
            elapsed = time.monotonic() - start
            return {'wall_seconds': elapsed, 'cpu_start_ns': before, 'cpu_end_ns': after,
                    'cpu_delta_seconds': (after - before) / 1e9,
                    'average_cpu_percent': (after - before) / 1e9 / elapsed * 100}
        finally:
            process.terminate()
            process.wait(timeout=10)


def inspect_scenarios(base, output):
    one = inspect(base, output, 'one-tab', SAMPLE, ['欢迎.md'])
    check_window(one)
    hidden = inspect(base, output, 'sidebar-hidden', SAMPLE, ['欢迎.md'], ['--inspect-hide-left'])
    check_window(hidden, aligned=False)
    vault = base / 'vault-sample'
    shutil.copytree(SAMPLE, vault)
    notes = [f'Note-{index:02d}-layout-check.md' for index in range(12)]
    for note in notes:
        shutil.copyfile(SAMPLE / '欢迎.md', vault / note)
    overflow = inspect(base, output, 'overflow-tabs', vault, notes)
    check_window(overflow)
    resized = inspect(base, output, 'resized', vault, notes, ['--inspect-resize'])
    for sample in resized['resize_samples']:
        check_window(sample)
    fullscreen = inspect(base, output, 'fullscreen-hidden', SAMPLE, ['欢迎.md'],
                         ['--inspect-fullscreen'], screen=True)
    assert fullscreen['content_safe_top'] == 0, fullscreen
    assert fullscreen['in_toolbar'] and not fullscreen['in_content'], fullscreen
    assert not fullscreen['toolbar_intersects_content'], fullscreen
    hover = inspect(base, output, 'fullscreen-top', SAMPLE, ['欢迎.md'],
                    ['--inspect-fullscreen-top'], screen=True)
    if hover['toolbar_intersects_content']:
        assert hover['column_top'] == fullscreen['column_top'], (hover, fullscreen)
        assert hover['panel_top'] == fullscreen['panel_top'], (hover, fullscreen)
    else:
        # 未触发展开时不把隐藏态截图冒充悬停验收证据。
        (output / 'fullscreen-top.png').unlink()
    print(json.dumps({'one_tab_x': one['tabs_x'], 'column_x': one['column_x'],
                      'overflow_width': overflow['tabs_width'],
                      'resize_samples': len(resized['resize_samples']),
                      'fullscreen_hover_triggered': hover['toolbar_intersects_content']}, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/tabs-in-toolbar')
    parser.add_argument('--skip-cpu', action='store_true')
    parser.add_argument('--cpu-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-toolbar-') as temporary:
        base = Path(temporary)
        if not args.cpu_only:
            inspect_scenarios(base, output)
        if not args.skip_cpu:
            empty = idle_cpu(base / 'idle-empty', [])
            print('未打开库 CPU 采样完成', flush=True)
            opened = idle_cpu(base / 'idle-opened', [f'--vault={SAMPLE}', '--note=欢迎.md'])
            difference = opened['average_cpu_percent'] - empty['average_cpu_percent']
            result = {'empty': empty, 'opened': opened, 'difference_percentage_points': difference,
                      'target_pass': abs(difference) <= 0.1,
                      'method': 'release .app，隔离配置/缓存，预热 5 s；proc_pidinfo 累计 user+system Mach ticks 经 mach_timebase_info 转为 ns，测量 30 s 增量；打开库同时显示一个标签。'}
            (output / 'idle-cpu.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
            print(json.dumps(result, ensure_ascii=False, indent=2), flush=True)
            assert result['target_pass'], result


if __name__ == '__main__':
    main()
