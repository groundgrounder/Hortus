#!/usr/bin/env python3
"""真实窗口中的大纲跳转、长文显示、双主题截图与空闲 CPU 对照。"""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('reading', ROOT / 'tools/check-reading-view.py')
READING = module_from_spec(SPEC)
SPEC.loader.exec_module(READING)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/outline')
    parser.add_argument('--skip-cpu', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    results = {}
    with tempfile.TemporaryDirectory(prefix='hortus-outline-') as temporary:
        base = Path(temporary)
        sample = ROOT / 'fixtures/demo-vault'
        note = 'Markdown field guide.md'
        before = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in sample.rglob('*') if p.is_file()}
        for name, appearance, editing in [('light', '--light', False), ('dark', '--dark', False),
                                           ('editing', '--light', True)]:
            result = READING.run(base / name, output, name, sample, note, 'outline', editing=editing,
                extra=[appearance, f'--snapshot={output / (name + ".png")}'])
            assert result['outline_count'] == result['outline_correct_targets'] == 9, result
            assert result['outline_revision_matches'] and set(result['outline_levels']) == {1, 2, 3}, result
            results[name] = result
        assert all(hashlib.sha256(p.read_bytes()).hexdigest() == value for p, value in before.items())
        vault = base / 'vault'
        vault.mkdir()
        (vault / 'empty.md').write_text('A note without headings.\n\nPlain text.\n')
        empty = READING.run(base / 'empty', output, 'empty', vault, 'empty.md', 'outline',
            extra=['--light', f'--snapshot={output / "empty.png"}'])
        assert empty['outline_count'] == 0 and empty['outline_revision_matches'], empty
        results['empty'] = empty
        # 与阅读回归相同的 1 MiB 样本，另补三个标题验证实际列表出现与远距离跳转。
        paragraphs = [f'Paragraph {i}: text **bold** and Unicode 中文😀.\n\n' for i in range(22000)]
        large = ''.join(paragraphs)
        (vault / 'large.md').write_text(large)
        (vault / 'headings.md').write_text('# Large outline\n\n' + ''.join(paragraphs[:11000])
            + '## Middle\n\n' + ''.join(paragraphs[11000:]) + '## End\n')
        for name, note in [('large', 'large.md'), ('large-headings', 'headings.md')]:
            result = READING.run(base / name, output, name, vault, note, 'outline')
            assert result['utf8_bytes'] >= 1024 * 1024 and result['outline_revision_matches'], result
            assert result['open_ms'] <= 100 and result['outline_ready_ms'] <= 100, result
            if name == 'large-headings':
                assert result['outline_count'] == result['outline_correct_targets'] == 3, result
                assert result['outline_displayed_ms'] <= 100, result
            results[name] = result
        if not args.skip_cpu:
            empty = READING.HELPERS.idle_cpu(base / 'cpu-empty', [])
            print('空窗口 CPU 采样完成', flush=True)
            opened = READING.HELPERS.idle_cpu(base / 'cpu-outline', [f'--vault={sample}', '--note=Markdown field guide.md'])
            difference = opened['average_cpu_percent'] - empty['average_cpu_percent']
            cpu = {'empty': empty, 'outline': opened, 'difference_percentage_points': difference}
            (output / 'idle-cpu.json').write_text(json.dumps(cpu, indent=2) + '\n')
            print('idle-cpu', json.dumps(cpu), flush=True)
            assert abs(difference) <= .1, cpu
            results['idle_cpu'] = cpu
        (output / 'summary.json').write_text(json.dumps(results, indent=2, ensure_ascii=False) + '\n')


if __name__ == '__main__':
    main()
