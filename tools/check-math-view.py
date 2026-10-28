#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""真实窗口中的公式字形、几何、刷新、内存、截图与空闲检查。"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
from statistics import median
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('reading', ROOT / 'tools/check-reading-view.py')
READING = module_from_spec(SPEC)
SPEC.loader.exec_module(READING)


def idle_math_cpu(base, vault, ready):
    config = base / 'config'
    config.mkdir(parents=True)
    (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "reading"\n')
    ready.unlink(missing_ok=True)
    command = [str(READING.HELPERS.APP), f'--vault={vault}', '--note=performance-note.md',
        f'--reading-inspection={ready}', '--reading-check=math', '--reading-wait-for-scan', '--inspect-hold-open']
    with subprocess.Popen(command, env=READING.HELPERS.environment(base)) as process:
        try:
            READING.HELPERS.wait_for(ready, process)
            assert json.loads(ready.read_text())['math_blocks'] == 50
            time.sleep(5)
            start = time.monotonic()
            before = READING.HELPERS.cpu_time(process.pid)
            time.sleep(30)
            after = READING.HELPERS.cpu_time(process.pid)
            elapsed = time.monotonic() - start
            return {'wall_seconds': elapsed, 'cpu_start_ns': before, 'cpu_end_ns': after,
                'cpu_delta_seconds': (after - before) / 1e9,
                'average_cpu_percent': (after - before) / 1e9 / elapsed * 100}
        finally:
            process.terminate(); process.wait(timeout=10)


def check_memory(sample):
    peak = sample['peak_bytes']
    retained = sample['retained_old_views']
    retained_math = sample['retained_old_math_views']
    # 分配器可保留首轮工作集；后十轮不得持续逐轮增加。
    for key in ['memory_bytes', 'footprint_bytes']:
        memory = sample[key]
        assert len(memory) == 20, (key, memory)
        assert not all(b > a for a, b in zip(memory[10:], memory[11:])), (key, memory)
        assert max(memory[10:]) - min(memory[10:]) <= 16 * 1024 * 1024, (key, memory)
    # ru_maxrss 只记录历史峰值；泄漏趋势由当前 RSS 和物理占用共同检查。
    assert len(peak) == 20, peak
    assert len(retained) == 20 and max(retained) <= 1, retained
    assert all(count <= retained[10] for count in retained[10:]), retained
    assert len(retained_math) == 20 and all(count == 0 for count in retained_math), retained_math


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/math-view')
    parser.add_argument('--checks', choices=['all', 'visual', 'performance', 'cpu', 'memory', 'network'], default='all')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-math-') as temporary:
        base = Path(temporary)
        vault = base / 'vault'
        vault.mkdir()
        for name in ['showcase.md', 'performance-note.md', 'missing-glyphs.md', 'container-regressions.md']:
            shutil.copyfile(ROOT / 'fixtures/math' / name, vault / name)
        (vault / 'other.md').write_text('# Other\n')
        def run(name, note='showcase.md', mode='math', extra=()):
            return READING.run(base / name, output, name, vault, note, mode=mode, extra=extra)
        if args.checks in ['all', 'visual']:
            for appearance in ['light', 'dark']:
                for index in range(27):
                    name = f'sample-{index:02}-{appearance}'
                    data = run(name, extra=[f'--{appearance}', f'--math-index={index}',
                        f'--math-capture={output / (name + "-formula.png")}', f'--snapshot={output / (name + ".png")}'])
                    assert data['math_blocks'] == 27, data
                    assert len(data['math_errors']) == 1, data
                    assert data['glyphs'] > 100, data
                    assert data['missing_glyphs'] == 0 and data['unresolved_metrics'] == 0, data
                    assert data['maximum_metric_error_px'] <= .5, data
                    assert data['main_thread_preparations'] == 0 and data['font_registrations'] == 1, data
                data = run(f'missing-glyphs-{appearance}', 'missing-glyphs.md', extra=[f'--{appearance}',
                    f'--snapshot={output / ("missing-glyphs-" + appearance + ".png")}'])
                required = {0x041F, 0x03B1, 0x20BB7, 0x2764, 0x1F468}
                measured = set(data['measured_fallback_codepoints'])
                assert data['math_blocks'] == 6, data
                assert data['missing_glyphs'] == 0 and data['unresolved_metrics'] == 0, data
                assert required <= measured, (sorted(required - measured), data)
                for case, section in [('dashed-array', 'Dashed array'), ('list-source', 'List source'),
                                      ('quote-error', 'Quote error'), ('numbered-error', 'Numbered error')]:
                    name = f'{case}-{appearance}'
                    data = run(name, 'container-regressions.md', extra=[f'--{appearance}',
                        f'--math-reveal=## {section}', f'--snapshot={output / (name + ".png")}'])
                    assert data['math_blocks'] == 7 and len(data['math_errors']) == 2, data
                    assert data['missing_glyphs'] == 0 and data['unresolved_metrics'] == 0, data
                run(f'wide-end-{appearance}', extra=[f'--{appearance}', '--math-index=23', '--math-scroll-end',
                    f'--snapshot={output / ("wide-end-" + appearance + ".png")}'])
                run(f'empty-{appearance}', extra=[f'--{appearance}', '--math-reveal=## Empty',
                    f'--snapshot={output / ("empty-" + appearance + ".png")}'])
                name = f'field-guide-{appearance}'
                READING.run(base / name, output, name, ROOT / 'fixtures/demo-vault', 'Markdown field guide.md', mode='math',
                    extra=[f'--{appearance}', '--math-reveal=## Mathematics', f'--snapshot={output / (name + ".png")}'])
        if args.checks in ['all', 'performance']:
            perf = run('performance', 'performance-note.md', 'math-performance')
            assert perf['math_blocks'] == 50, perf
            assert 0 < perf['open_ms'] <= 150, perf
            assert perf['scroll_p95_ms'] <= 8, perf
            assert perf['scroll_max_ms'] <= 16, perf
            assert max(perf['main_thread_phases_ms'].values()) <= 16, perf
            assert perf['maximum_main_work_ms'] <= 16, perf
            assert perf['main_thread_preparations'] == 0, perf
            assert 0 < perf['maximum_formula_draw_ms'] <= 16, perf
            refreshes = []
            for index in range(5):
                shutil.copyfile(ROOT / 'fixtures/math/performance-note.md', vault / 'performance-note.md')
                refreshes.append(run(f'external-{index+1}', 'performance-note.md', 'math-external'))
            summary = {'median_ms': median(r['after_debounce_ms'] for r in refreshes),
                'samples_ms': [r['after_debounce_ms'] for r in refreshes],
                'rebuilt': [r['rebuilt'] for r in refreshes], 'reused': [r['reused_math_views'] for r in refreshes],
                'maximum_anchor_shift_pt': max(r['anchor_shift_pt'] for r in refreshes)}
            (output / 'external-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
            assert summary['median_ms'] <= 50, summary
            assert all(r['rebuilt'] == 1 and r['same_anchor'] and r['anchor_shift_pt'] <= 1 for r in refreshes), summary
        if args.checks in ['all', 'memory']:
            sample = run('memory', 'performance-note.md', 'math-memory')
            check_memory(sample)
        if args.checks in ['all', 'cpu']:
            empty = READING.HELPERS.idle_cpu(base / 'empty', [])
            opened = idle_math_cpu(base / 'opened', vault, output / 'cpu-ready.json')
            data = {'empty': empty, 'math': opened,
                'difference_percentage_points': opened['average_cpu_percent'] - empty['average_cpu_percent']}
            (output / 'idle-cpu.json').write_text(json.dumps(data, indent=2) + '\n')
            print(json.dumps(data), flush=True)
            assert abs(data['difference_percentage_points']) <= .1, data
        if args.checks in ['all', 'network']:
            ready = output / 'network-ready.json'
            ready.unlink(missing_ok=True)
            config = base / 'network/config'
            config.mkdir(parents=True)
            (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "reading"\n')
            command = [str(READING.HELPERS.APP), f'--vault={vault}', '--note=showcase.md',
                f'--reading-inspection={ready}', '--reading-check=math', '--reading-wait-for-scan', '--inspect-hold-open']
            with subprocess.Popen(command, env=READING.HELPERS.environment(base / 'network')) as process:
                try:
                    READING.HELPERS.wait_for(ready, process)
                    assert json.loads(ready.read_text())['math_blocks'] == 27
                    network = subprocess.run(['lsof', '-nP', '-a', '-p', str(process.pid), '-i'], capture_output=True, text=True)
                    (output / 'network.txt').write_text(f'pid={process.pid}\nexit={network.returncode}\n' + network.stdout + network.stderr)
                    assert network.returncode == 1 and not network.stdout.strip(), network.stdout
                finally:
                    if process.poll() is None:
                        process.terminate(); process.wait(timeout=10)


if __name__ == '__main__':
    main()
