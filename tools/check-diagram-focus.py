#!/usr/bin/env python3
"""P1-08：源码与图双向定位、专注模式、缓存与性能的真实窗口检查。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus-debug.app/Contents/MacOS/Hortus'
RELEASE_APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
OUTPUT = ROOT / 'target/reports/diagram-focus'
MEDIA = OUTPUT / 'media'
SOURCES = {
    'deep.mmd': ROOT / 'fixtures/mermaid/mindmap/deep.mmd',
    'tree.mmd': ROOT / 'fixtures/mermaid/mindmap/shapes-classes.mmd',
    'flow.mmd': ROOT / 'fixtures/mermaid/flowchart/subgraphs-top-level.mmd',
    'seq.mmd': ROOT / 'fixtures/mermaid/sequence/control-flow.mmd',
}


def environment(base):
    return dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'),
                HORTUS_CACHE_DIR=str(base / 'cache'))


def ensure_app():
    """#if DEBUG 自检入口只存在调试可执行文件；包资源与本地化沿用 Release 包。"""
    debug_binary = ROOT / 'apps/macos/.build/debug/Hortus'
    release_bundle = ROOT / 'build/Hortus.app'
    assert release_bundle.exists(), '先运行 ./tools/build-macos.sh 生成 Release 应用包'
    static_library = ROOT / 'target/release/libhortus_ffi.a'
    # 增量构建很快；源码或静态库变化都必须重新链接调试可执行文件。
    command = ['swift', 'build', '--package-path', str(ROOT / 'apps/macos'), '-c', 'debug',
               '-Xlinker', str(static_library),
               '-Xlinker', '-framework', '-Xlinker', 'CoreFoundation',
               '-Xlinker', '-framework', '-Xlinker', 'CoreServices',
               '-Xlinker', '-framework', '-Xlinker', 'Security',
               '-Xlinker', '-liconv']
    completed = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    assert completed.returncode == 0, completed.stderr.decode(errors='replace')
    bundle = ROOT / 'build/Hortus-debug.app'
    executable = bundle / 'Contents/MacOS/Hortus'
    stale = not executable.exists() or executable.stat().st_mtime < debug_binary.stat().st_mtime \
        or (bundle / 'Contents/Info.plist').stat().st_mtime < (release_bundle / 'Contents/Info.plist').stat().st_mtime
    if stale:
        shutil.rmtree(bundle, ignore_errors=True)
        shutil.copytree(release_bundle, bundle)
        shutil.copyfile(debug_binary, executable)
        completed = subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)],
                                   check=False, capture_output=True)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')


def prepare_state(base):
    (base / 'config').mkdir(parents=True, exist_ok=True)
    (base / 'cache').mkdir(parents=True, exist_ok=True)
    (base / 'config' / 'config.toml').write_text('font_size = 15.0\n')


def prepare_vault(base):
    vault = base / 'vault'
    if vault.exists():
        shutil.rmtree(vault)
    vault.mkdir(parents=True)
    for name, source in SOURCES.items():
        shutil.copyfile(source, vault / name)
    (vault / 'plain.md').write_text('# Plain note\n\nNo diagrams here.\n')
    empty = base / 'empty-vault'
    empty.mkdir(exist_ok=True)
    return vault


def inventory(root):
    return {str(path.relative_to(root)): (path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest())
            for path in sorted(root.rglob('*')) if path.is_file()}


def wait_for(path, process, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists():
            try:
                return json.loads(path.read_text())
            except json.JSONDecodeError:
                pass
        if process.poll() is not None and not path.exists():
            raise AssertionError(f'自检进程提前退出：{process.returncode}')
        time.sleep(0.02)
    raise AssertionError(f'自检未生成 {path.name}')


def launch(base, vault, note, inspection, extra=(), snapshot=None, hold=False,
           dark=False, language=None, timeout=180):
    inspection.unlink(missing_ok=True)
    arguments = [str(APP), f'--vault={vault}']
    if note:
        arguments.append(f'--note={note}')
    arguments.append('--dark' if dark else '--light')
    arguments.append(f'--diagram-focus-inspection={inspection}')
    if snapshot:
        arguments.append(f'--snapshot={snapshot}')
    if hold:
        arguments.append('--inspect-hold-open')
    if language:
        arguments += ['-AppleLanguages', f'({language})']
    arguments += list(extra)
    process = subprocess.Popen(arguments, env=environment(base),
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if hold:
        return process, wait_for(inspection, process, timeout)
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=20)
        raise
    data = wait_for(inspection, process, timeout)
    if process.returncode not in (0, None):
        raise AssertionError(f'自检退出码 {process.returncode}：{process.stderr.read()[-400:]}')
    return data


def spawn(base, vault, note='', extra=()):
    arguments = [str(APP), f'--vault={vault}']
    if note:
        arguments.append(f'--note={note}')
    arguments += ['--light', *extra]
    return subprocess.Popen(arguments, env=environment(base),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def p95(values):
    values = sorted(values)
    return values[int((len(values) - 1) * 0.95)] if values else 0.0


def idle_samples(pid, samples=30):
    output = subprocess.check_output(
        ['top', '-l', str(samples + 1), '-s', '1', '-pid', str(pid), '-stats', 'pid,cpu,threads'],
        text=True)
    values = []
    for line in output.splitlines():
        match = re.match(r'^\s*(\d+)\s+([\d.]+)\s+(\d+)', line)
        if match and int(match.group(1)) == pid:
            values.append(float(match.group(2)))
    return values[1:]


def assert_close(actual, expected, tolerance=0.01):
    assert abs(actual - expected) <= tolerance, (actual, expected)


def screenshot(base, media, name, note, scene, extra=(), dark=False, language=None, query=None):
    target = media / f'{name}.png'
    target.unlink(missing_ok=True)
    if query:
        extra = list(extra) + [f'--diagram-focus-search={query}']
    data = launch(base, base / 'vault', note, OUTPUT / 'screenshots' / f'{name}.json',
                  ['--diagram-focus-only-scene', f'--diagram-focus-scene={scene}', *extra], snapshot=target,
                  dark=dark, language=language)
    assert target.exists() and target.stat().st_size > 10_000, target
    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    print(f'截图 {name}: {target} sha256={digest[:16]}')
    return {'path': str(target), 'sha256': digest, 'bytes': target.stat().st_size, 'scene': data.get('scene')}


def main():
    global OUTPUT
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=OUTPUT)
    parser.add_argument('--media', type=Path, default=None)
    parser.add_argument('--skip-idle', action='store_true')
    parser.add_argument('--keep', action='store_true')
    args = parser.parse_args()
    output = args.output
    OUTPUT = output
    media = args.media or output / 'media'
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    (output / 'screenshots').mkdir(parents=True, exist_ok=True)
    ensure_app()
    assert APP.is_file(), f'缺少应用：{APP}'
    assert b'--diagram-focus-inspection=' not in RELEASE_APP.read_bytes(), 'Release 包不得包含自检开关'
    assert b'--diagram-focus-inspection=' in APP.read_bytes(), '调试包缺少自检开关'
    base = Path(tempfile.mkdtemp(prefix='hortus-focus-'))
    prepare_state(base)
    vault = prepare_vault(base)
    before = inventory(vault)
    summary = {}
    try:
        print('== 双向定位、专注模式与缓存 ==', flush=True)
        started = time.monotonic()
        data = launch(base, vault, 'tree.mmd', output / 'inspection.json')
        elapsed = time.monotonic() - started
        (output / 'inspection.json').write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps({k: data.get(k) for k in ('focus', 'highlight', 'resize', 'cache_restore')},
                         ensure_ascii=False, indent=2)[:4000], flush=True)

        cmd = data['reading_cmd_click']
        assert cmd['left_reading'] and cmd['matches'], cmd
        menu = data['menu']
        assert menu['has_jump'] and menu['has_focus'] and menu['matches'], menu
        accessibility = data['accessibility']
        assert accessibility['count'] >= 1 and accessibility['matches'], accessibility
        highlight = data['highlight']
        assert highlight['all_visible_matched'], highlight
        assert highlight['cleared'], highlight
        assert highlight['rust_layout_calls'] == 0, highlight
        assert highlight['ancestor'] and highlight['ancestor']['matches'], highlight['ancestor']
        focus = data['focus']
        entry = focus['entry']
        assert entry['hover_button'], entry
        assert entry['menu']['present'] and entry['menu']['key'] == 'd', entry
        assert entry['menu']['enabled'] is True, entry
        assert entry['menu_from_reading'] and entry['menu_key_handled'] and entry['menu_action'], entry
        assert data['exits']['escape'] and data['exits']['shift_command_d'], data['exits']
        assert data['exits']['menu_enabled'] and data['exits']['menu_shift_command_d'], data['exits']
        print('== 纯文本文档菜单禁用 ==', flush=True)
        plain = launch(base, vault, 'plain.md', output / 'plain-menu.json', ['--diagram-focus-only-menu'])
        assert plain['reading'] and not plain['prepared_diagrams'], plain
        assert plain['menu']['present'] and plain['menu']['enabled'] is False, plain['menu']
        summary['plain_menu'] = plain
        assert focus['first_frame_ms'] <= 100, focus
        assert focus['inside_editor'] and focus['covers_editor'] and focus['toolbar_present'], focus
        assert focus['sidebar_untouched'], focus
        zoom = focus['zoom']
        assert_close(zoom['low'], 0.25)
        assert_close(zoom['high'], 4.0)
        assert_close(zoom['actual'], 1.0)
        assert 0.25 <= zoom['fit'] <= 4.0, zoom
        assert zoom['rust_layout_calls'] == 0, zoom
        zoom_keys = focus['zoom_keys']
        assert zoom_keys['zoom_in'] > 1.0, zoom_keys
        assert_close(zoom_keys['zoom_out'], 1.0), zoom_keys
        assert 0.25 <= zoom_keys['fit'] <= 4.0 and zoom_keys['actual'] == 1.0, zoom_keys
        minimap = focus['minimap']
        assert minimap['visible_when_zoomed'] and minimap['hidden_at_fit'] and minimap['jump_matches'], minimap
        shortcuts = focus['shortcuts']
        assert shortcuts['search_field_visible'] and not shortcuts['editor_find_bar'], shortcuts
        cycle = shortcuts['cycle']
        assert cycle['count'] >= 2 and cycle['after'] != cycle['before'] and cycle['back'] == cycle['before'], cycle
        search = focus['search']
        assert search['matches']['visible'] >= 1 and search['current_in_viewport'], search
        hidden = focus['hidden_search']
        assert hidden and hidden['hidden_before'] >= 1, hidden
        assert hidden['hidden_after'] == 0 and hidden['visible_after'] >= 1, hidden
        frames = focus['frames']
        assert frames['count'] == 300 and frames['p95_ms'] <= 8 and frames['rust_layout_calls'] == 0, frames
        command = focus['command_click']
        assert command['exited'] and command['matches'] and command['reading'] is False, command
        restored = data['restore_editing']
        assert restored['reading'] is False and restored['selection_restored'], restored
        assert_close(restored['scroll'][1], restored['expected_scroll'][1], 1.5), restored
        reading = data['restore_reading']
        assert reading['reading'] and reading['scroll_restored'], reading
        cache = data['cache_restore']
        assert cache['scale_matches'] and cache['offset_matches'], cache
        resize = data['resize']
        assert resize['canvas_width_after'] > resize['canvas_width_before'], resize
        assert resize['scene_width_after'] > resize['scene_width_before'], resize
        assert resize['rows_changed'], resize
        assert data['reading_at_end'] is True, data['reading_at_end']
        summary['inspection'] = {'elapsed_seconds': elapsed, 'first_frame_ms': focus['first_frame_ms'],
                                 'frames_p95_ms': frames['p95_ms'],
                                 'highlight_layout_calls': highlight['rust_layout_calls'],
                                 'zoom_layout_calls': zoom['rust_layout_calls'],
                                 'resize_rows_before': resize['rows_before'],
                                 'resize_rows_after': resize['rows_after']}

        print('== 缓存跨进程恢复 ==', flush=True)
        first = launch(base, vault, 'tree.mmd', output / 'cache-first.json',
                       ['--diagram-focus-only-scene', '--diagram-focus-scene=focus-zoom-300'])
        second = launch(base, vault, 'tree.mmd', output / 'cache-restart.json',
                        ['--diagram-focus-only-scene', '--diagram-focus-scene=cache-check'])
        saved, restored_state = first['state'], second['state']
        assert_close(saved['scale'], 3.0, 0.001)
        assert_close(restored_state['scale'], saved['scale'], 0.01)
        assert_close(restored_state['offset'][0], saved['offset'][0], 2)
        assert_close(restored_state['offset'][1], saved['offset'][1], 2)
        summary['cache_restart'] = {'saved_scale': saved['scale'], 'restored_scale': restored_state['scale'],
                                    'saved_offset': saved['offset'], 'restored_offset': restored_state['offset']}

        print('== 公开双语浅深色截图 ==', flush=True)
        shots = {}
        shots['source-highlight-light'] = screenshot(base, media, 'source-highlight-light', 'tree.mmd',
                                                     'source-highlight', language='en')
        shots['focus-overview-en-light'] = screenshot(base, media, 'focus-overview-en-light', 'tree.mmd',
                                                      'focus-overview', language='en')
        shots['focus-overview-zh-Hans-light'] = screenshot(base, media, 'focus-overview-zh-Hans-light', 'flow.mmd',
                                                           'focus-overview', language='zh-Hans')
        shots['focus-overview-zh-Hans-dark'] = screenshot(base, media, 'focus-overview-zh-Hans-dark', 'flow.mmd',
                                                          'focus-overview', language='zh-Hans', dark=True)
        shots['focus-zoom-300'] = screenshot(base, media, 'focus-zoom-300', 'tree.mmd', 'focus-zoom-300',
                                             language='en')
        shots['focus-search-hit'] = screenshot(base, media, 'focus-search-hit', 'deep.mmd', 'focus-search',
                                               query='level', language='en')
        (output / 'screenshots.json').write_text(json.dumps(shots, ensure_ascii=False, indent=2) + '\n')

        print('== 专注模式空闲 CPU ==', flush=True)
        idle = {}
        if args.skip_idle:
            print('跳过（--skip-idle）')
        else:
            empty_base = base / 'idle-empty'
            prepare_state(empty_base)
            empty_empty = empty_base / 'empty-vault'
            empty_empty.mkdir(exist_ok=True)
            process = spawn(empty_base, empty_empty)
            time.sleep(2)
            empty_samples = idle_samples(process.pid)
            process.terminate(); process.wait(timeout=20)
            process, _ = launch(base, vault, 'deep.mmd', empty_base / 'focus.json',
                                ['--diagram-focus-scene=focus-overview', '--inspect-hold-open'], hold=True)
            time.sleep(2)
            focus_samples = idle_samples(process.pid)
            process.terminate(); process.wait(timeout=20)
            empty_median = statistics.median(empty_samples) if empty_samples else 0.0
            focus_median = statistics.median(focus_samples) if focus_samples else 0.0
            difference = focus_median - empty_median
            idle = {'empty_cpu_percent': empty_median, 'focus_cpu_percent': focus_median,
                    'difference_percentage_points': difference,
                    'empty_samples': empty_samples, 'focus_samples': focus_samples}
            assert len(empty_samples) >= 20 and len(focus_samples) >= 20, idle
            assert difference <= 0.1, idle
            summary['idle'] = idle
        (output / 'idle-cpu.json').write_text(json.dumps(idle, ensure_ascii=False, indent=2) + '\n')

        after = inventory(vault)
        writes = {'before': len(before), 'after': len(after), 'unchanged': before == after,
                  'added': sorted(set(after) - set(before)), 'removed': sorted(set(before) - set(after))}
        assert writes['unchanged'], writes
        summary['vault_writes'] = writes
        (output / 'vault-writes.json').write_text(json.dumps(writes, ensure_ascii=False, indent=2) + '\n')
        summary['media'] = {name: {'path': item['path'], 'sha256': item['sha256']} for name, item in shots.items()}
        summary['pass'] = True
        (output / 'checks.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps(summary, ensure_ascii=False, indent=2))
        print('P1-08 检查通过')
    finally:
        if not args.keep:
            shutil.rmtree(base, ignore_errors=True)


if __name__ == '__main__':
    main()
