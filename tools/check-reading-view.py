#!/usr/bin/env python3
"""P1-06：真实 release 窗口中的阅读、切换、长文刷新和性能对照。"""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
from statistics import median
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('helpers', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def run(base, output, name, vault, note, mode='visual', editing=False, extra=()):
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "' + ('editing' if editing else 'reading') + '"\n')
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    subprocess.run([str(HELPERS.APP), f'--vault={vault}', f'--note={note}',
        f'--reading-inspection={target}', f'--reading-check={mode}', '--reading-wait-for-scan', *extra],
        env=HELPERS.environment(base), check=True, timeout=180)
    result = json.loads(target.read_text())
    print(name, json.dumps(result, ensure_ascii=False), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/reading-view')
    parser.add_argument('--cpu-only', action='store_true')
    parser.add_argument('--screenshots-only', action='store_true')
    parser.add_argument('--checks', choices=['all', 'switching', 'performance', 'external'], default='all')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-reading-') as temporary:
        base = Path(temporary)
        sample = ROOT / 'fixtures/vault-sample'
        if args.cpu_only:
            empty = HELPERS.idle_cpu(base / 'empty', [])
            print('空库 CPU 采样完成', flush=True)
            opened = HELPERS.idle_cpu(base / 'opened', [f'--vault={sample}', '--note=欢迎.md'])
            result = {'empty': empty, 'reading_diagram': opened,
                      'difference_percentage_points': opened['average_cpu_percent'] - empty['average_cpu_percent']}
            (output / 'idle-cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            assert abs(result['difference_percentage_points']) <= .1
            return
        before = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in sample.rglob('*') if p.is_file()}
        for name, appearance in ([('light', '--light'), ('dark', '--dark')] if args.checks == 'all' else []):
            run(base / name, output, name, sample, '样式化源码.md',
                extra=[appearance, f'--snapshot={output / (name + ".png")}'])
        vault = base / 'vault'
        vault.mkdir()
        quotes = '> Quote one\n> Quote two\n> Quote three\n\n> [!note] Title\n> Callout two\n> Callout three\n'
        (vault / 'quotes.md').write_text(quotes)
        for editing in ([False, True] if args.checks == 'all' else []):
            name = 'quotes-editing' if editing else 'quotes-reading'
            run(base / name, output, name, vault, 'quotes.md', editing=editing,
                extra=['--light', f'--snapshot={output / (name + ".png")}'])
        assert all(hashlib.sha256(p.read_bytes()).hexdigest() == value for p, value in before.items())
        if args.screenshots_only:
            return
        (vault / 'other.md').write_text('# Other\n')
        (vault / 'note.md').write_text(''.join(f'## Heading {i}😀\n\nText **bold** and [[Target|Alias]] {i}.\n\n' for i in range(220)))
        if args.checks in ['all', 'switching']:
            switched = run(base / 'switching', output, 'switching', vault, 'note.md', 'switching')
            assert switched['correct_blocks'] == switched['visible_blocks'] == 200, switched
            assert switched['retained_old_resources'] == 0, switched
        if args.checks == 'switching':
            return
        large = ''.join(f'Paragraph {i}: text **bold** and Unicode 中文😀.\n\n' for i in range(22000))
        assert len(large.encode()) >= 1024 * 1024
        (vault / 'note.md').write_text(large)
        for editing in ([False, True] if args.checks in ['all', 'performance'] else []):
            name = 'performance-editing' if editing else 'performance-reading'
            run(base / name, output, name, vault, 'note.md', 'performance', editing=editing)
        if args.checks in ['all', 'external']:
            # 固定测量五次，以中位数排除调度抖动；每次仍检查重建范围与阅读位置。
            refreshes = []
            for index in range(1, 6):
                (vault / 'note.md').write_text(large)
                name = f'external-{index}'
                refreshes.append(run(base / name, output, name, vault, 'note.md', 'external'))
            summary = {
                'sample_count': len(refreshes),
                'after_debounce_samples_ms': [refresh['after_debounce_ms'] for refresh in refreshes],
                'after_debounce_median_ms': median(refresh['after_debounce_ms'] for refresh in refreshes),
                'rebuilds': [refresh['rebuilds'] for refresh in refreshes],
                'same_anchor': all(refresh['same_anchor'] for refresh in refreshes),
                'max_anchor_shift_pt': max(refresh['anchor_shift_pt'] for refresh in refreshes),
            }
            (output / 'external.json').write_text(json.dumps(summary, indent=2) + '\n')
            print('external', json.dumps(summary), flush=True)
            assert all(refresh['rebuilds'] == 1 and refresh['same_anchor'] and refresh['anchor_shift_pt'] <= 1
                       for refresh in refreshes), summary
            assert summary['after_debounce_median_ms'] <= 50, summary
        if args.checks == 'external':
            return
        performance = json.loads((output / 'performance-reading.json').read_text())
        assert performance['open_ms'] <= 100, performance
        assert performance['cached_open_ms'] <= 30, performance
        assert performance['scroll_p95_ms'] <= 8, performance
        assert max(performance['switch_ms']) <= 100, performance


if __name__ == '__main__':
    main()
