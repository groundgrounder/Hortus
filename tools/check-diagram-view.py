#!/usr/bin/env python3
"""P1-03：真实窗口、折叠重启、源码回退、库只读、重排/内存/退出及 CPU。"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from importlib.util import module_from_spec, spec_from_file_location

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
SAMPLE = ROOT / 'fixtures/vault-sample'
SPEC = spec_from_file_location('toolbar_check', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def inventory(root):
    return {str(path.relative_to(root)): (path.stat().st_mtime_ns, path.stat().st_size,
            hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else '')
            for path in [root, *root.rglob('*')]}


def inspect(base, output, name, vault, note, extra=(), screenshot=False):
    output.mkdir(parents=True, exist_ok=True)
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    appearance = [] if '--dark' in extra else ['--light']
    arguments = [str(APP), f'--vault={vault}', f'--note={note}', *appearance,
                 f'--diagram-inspection={target}', *extra]
    if screenshot:
        arguments += [f'--snapshot={output / (name + ".png")}']
    with subprocess.Popen(arguments, env=HELPERS.environment(base), stdout=subprocess.DEVNULL) as process:
        try:
            HELPERS.wait_for(target, process)
            ready = time.monotonic()
            assert process.wait(timeout=10) == 0
            exit_delay = time.monotonic() - ready
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
    data = json.loads(target.read_text())
    assert data['source_preserved'] and data['textkit2'], data
    assert data['accessibility_nodes'] == data['visible_nodes'], data
    assert data['accessibility_expanded_valid'], data
    assert data['custom_fragments'] >= data['diagrams'], data
    assert data['error_fragments'] == len(data['reasons']), data
    assert data['width_max_px'] <= 0.5, data
    return data, exit_delay


def scenarios(base, output):
    before = inventory(SAMPLE)
    welcome, _ = inspect(base / 'welcome', output, 'welcome-light', SAMPLE, '欢迎.md', screenshot=True)
    assert welcome['diagrams'] == 1 and welcome['visible_nodes'] == 4 and not welcome['dark'], welcome
    flow, _ = inspect(base / 'flow', output, 'flow', SAMPLE, '参考/流程.mmd', screenshot=True)
    assert flow['visible_nodes'] == 3, flow
    dark, _ = inspect(base / 'dark', output, 'dark', SAMPLE, '欢迎.md', ['--dark'], screenshot=True)
    assert dark['dark'], dark
    folded, _ = inspect(base / 'fold', output, 'collapsed', SAMPLE, '欢迎.md', ['--diagram-click'], True)
    assert folded['visible_nodes'] == 1 and folded['badge_count'] > 0, folded
    restored, _ = inspect(base / 'fold', output, 'restored', SAMPLE, '欢迎.md')
    assert restored['visible_nodes'] == 1 and restored['collapsed_nodes'] == 1, restored
    expanded, _ = inspect(base / 'fold', output, 'expanded', SAMPLE, '欢迎.md', ['--diagram-click'])
    assert expanded['visible_nodes'] == 4 and expanded['collapsed_nodes'] == 0, expanded
    assert inventory(SAMPLE) == before, '示例库被改写'
    vault = base / 'errors'
    vault.mkdir()
    cases = [('unsupported', 'sequenceDiagram\nA->>B: Hi', '暂不支持'),
             ('not-tree', 'flowchart TB\nA --> C\nB --> C', '不是树形结构'),
             ('syntax', 'flowchart TB\nA --> B\nB ??? C', '第 3 行语法错误')]
    for name, source, expected in cases:
        (vault / f'{name}.mmd').write_text(source)
        data, _ = inspect(base / name, output, name, vault, f'{name}.mmd', screenshot=True)
        assert data['diagrams'] == 0 and data['reasons'][0].startswith(expected), data
    (vault / 'mixed.md').write_text('前文😀\n\n```mermaid\nmindmap\n  Root\n    First\n```\n中间\n~~~mermaid\nflowchart TB\nA --> B\n~~~\n末尾\n')
    mixed, _ = inspect(base / 'mixed', output, 'mixed', vault, 'mixed.md')
    assert mixed['diagrams'] == 2 and mixed['visible_nodes'] == 4, mixed
    private = ROOT / 'fixtures/mermaid/tree'
    files = sorted(private.glob('04-*.md')) if private.exists() else []
    if files:
        private_before = inventory(private)
        sample = files[0]
        performance, _ = inspect(base / 'private', output / 'private', '04-default', private, sample.name,
                                 ['--diagram-resize'], True)
        memory_data, _ = inspect(base / 'private-memory', output / 'private', 'memory-only', private, sample.name,
                                ['--diagram-memory'])
        assert inventory(private) == private_before, '私人样本被改写'
    else:
        print('私人 04 样本缺失：跳过截图，使用最大公开夹具执行性能/内存验收', flush=True)
        sample = max((ROOT / 'fixtures/mermaid/mindmap').glob('*.mmd'), key=lambda path: path.stat().st_size)
        performance, _ = inspect(base / 'performance', output, 'public-performance', sample.parent,
                                 sample.name, ['--diagram-resize'])
        memory_data, _ = inspect(base / 'memory', output, 'public-memory', sample.parent,
                                sample.name, ['--diagram-memory'])
    resize = performance['resize']
    assert len(resize) == 60 and all(row['draw_height'] > 0 and row['draw_scale'] >= 1 for row in resize), '缩放用例未绘制可见图'
    totals = sorted(row['total_ms'] for row in resize)
    p95 = totals[int((len(totals) - 1) * .95)]
    memory = memory_data['memory']
    middle = sorted(row['opened_bytes'] for row in memory[5:10])[2]
    end = sorted(row['opened_bytes'] for row in memory[-5:])[2]
    result = {'private_sample_present': bool(files), 'first_main_work_max_ms': performance['first_main_work_max_ms'],
              'resize_samples': len(resize), 'resize_p95_ms': p95, 'resize': resize, 'memory': memory,
              'memory_middle_median_bytes': middle, 'memory_end_median_bytes': end}
    (output / 'performance.json').write_text(json.dumps(result, indent=2) + '\n')
    assert performance['first_main_work_max_ms'] <= 16, performance['first_main_work_max_ms']
    assert p95 <= 4, f'重排加绘制 P95={p95:.3f} ms，明细见 performance.json'
    assert end <= middle + 4 * 1024 * 1024, f'20 次开关后的中位内存增长 {(end - middle) / 1024**2:.2f} MiB'
    assert memory[-1]['peak_bytes'] <= memory[9]['peak_bytes'] + 4 * 1024 * 1024, '后半程内存峰值持续增长'
    assert all(row['retained_old_diagram_views'] == 0 and row['retained_old_text_views'] == 0 for row in memory), '旧视图未释放'
    _, exit_delay = inspect(base / 'slow-exit', output, 'slow-exit', SAMPLE, '欢迎.md', ['--inspect-slow-shutdown'])
    shutdown = {'injected_cleanup_seconds': 10, 'seconds_after_result_to_exit': exit_delay}
    (output / 'shutdown.json').write_text(json.dumps(shutdown, indent=2) + '\n')
    assert 2.7 <= exit_delay <= 3.7, shutdown
    print(json.dumps({key: value for key, value in result.items() if key not in ['resize', 'memory']}, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/diagram-view')
    parser.add_argument('--skip-cpu', action='store_true')
    parser.add_argument('--cpu-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-diagrams-') as directory:
        base = Path(directory)
        if not args.cpu_only:
            scenarios(base, output)
        if not args.skip_cpu:
            empty = HELPERS.idle_cpu(base / 'cpu-empty', [])
            print('未打开库的 30 s CPU 采样完成', flush=True)
            opened = HELPERS.idle_cpu(base / 'cpu-opened', [f'--vault={SAMPLE}', '--note=欢迎.md'])
            difference = opened['average_cpu_percent'] - empty['average_cpu_percent']
            result = {'empty': empty, 'opened': opened, 'difference_percentage_points': difference}
            (output / 'idle-cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            assert abs(difference) <= .1, result


if __name__ == '__main__':
    main()
