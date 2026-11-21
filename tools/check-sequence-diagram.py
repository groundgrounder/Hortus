#!/usr/bin/env python3
"""时序图 CLI/FFI 一致性、排版质量与真实窗口回归检查。"""
import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from statistics import median

from importlib.util import module_from_spec, spec_from_file_location

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
CLI = ROOT / 'target/release/hortus'
FIXTURES = ROOT / 'fixtures/mermaid/sequence'
RENDERED = ['agent-workflow.mmd', 'control-flow.mmd', 'narrow-wrap.mmd']
WIDTHS = [700, 1100]
THEMES = ['light', 'dark']
SPEC = spec_from_file_location('toolbar_check', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def note_for(fixture):
    """把 .mmd 夹具包成带围栏的临时笔记。"""
    return '# 时序图\n\n```mermaid\n' + fixture.read_text() + '```\n'


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False)


def render_cli(note, width, theme, output, tag):
    svg = output / f'{tag}.svg'
    metrics_path = output / f'{tag}.metrics.json'
    scene_path = output / f'{tag}.scene.json'
    completed = subprocess.run(
        [str(CLI), 'mermaid', 'render', str(note), '--width', str(width),
         '--theme', theme, '--out', str(svg), '--metrics', str(metrics_path),
         '--scene', str(scene_path)],
        capture_output=True, text=True, timeout=120)
    assert completed.returncode == 0, (tag, completed.stderr)
    assert 'Unsupported' not in completed.stderr and 'Syntax' not in completed.stderr, completed.stderr
    metrics = json.loads(metrics_path.read_text())
    scene = json.loads(scene_path.read_text())
    return svg, metrics, scene


def inspect(base, output, name, vault, note, extra=(), media=None):
    output.mkdir(parents=True, exist_ok=True)
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "editing"\n')
    arguments = [str(APP), f'--vault={vault}', f'--note={note}',
                 '--light' if '--dark' not in extra else '--dark',
                 f'--diagram-inspection={target}', *extra]
    if media is not None:
        arguments.append(f'--snapshot={media}')
    with subprocess.Popen(arguments, env=HELPERS.environment(base), stdout=subprocess.DEVNULL) as process:
        try:
            HELPERS.wait_for(target, process)
            assert process.wait(timeout=20) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
    assert target.exists(), f'{name} 未生成检查输出'
    return json.loads(target.read_text())


def cli_matrix(output):
    """3 个公开样例 × 700/1100 × light/dark 的 CLI 渲染与指标断言。"""
    rows = []
    with tempfile.TemporaryDirectory(prefix='hortus-sequence-') as directory:
        base = Path(directory)
        for fixture in RENDERED:
            note = base / (fixture.replace('.mmd', '.md'))
            note.write_text(note_for(FIXTURES / fixture))
            for width in WIDTHS:
                for theme in THEMES:
                    tag = f'{fixture[:-4]}-{width}-{theme}'
                    svg, metrics, scene = render_cli(note, width, theme, output, tag)
                    assert metrics['columns'] == 1, (tag, metrics)
                    assert metrics['node_overlaps'] == 0, (tag, metrics)
                    assert metrics['horizontal_overflow'] is False, (tag, metrics)
                    assert metrics['min_font_px'] >= 12.5, (tag, metrics)
                    assert metrics['diagram_kind'] == 'sequence', (tag, metrics)
                    assert scene['metrics']['diagram_kind'] == 'sequence', tag
                    assert scene['width'] == width, tag
                    text = svg.read_text()
                    assert '<svg' in text and 'aria-label="Mermaid sequence diagram"' in text, tag
                    assert 'stroke-dasharray' in text, tag
                    assert 'NaN' not in text and 'inf' not in text, tag
                    hits = scene['hit_regions']
                    assert hits, tag
                    assert all(not hit['can_collapse'] for hit in hits), tag
                    assert any(hit['node_id'].startswith('msg-') for hit in hits), tag
                    if fixture == 'control-flow.mmd':
                        assert any(hit['node_id'].startswith('note-') for hit in hits), tag
                    rows.append({
                        'fixture': fixture, 'width': width, 'theme': theme,
                        'columns': metrics['columns'], 'height': metrics['diagram_height'],
                        'min_font_px': metrics['min_font_px'],
                        'node_overlaps': metrics['node_overlaps'],
                        'horizontal_overflow': metrics['horizontal_overflow'],
                        'layout_ms_cold': metrics['layout_ms'],
                        'layout_ms_zeroed': scene['metrics']['layout_ms'] == 0.0,
                        'hit_regions': len(hits),
                        'scene_sha256': hashlib.sha256(canonical(scene).encode()).hexdigest(),
                        'svg_bytes': svg.stat().st_size,
                    })
    (output / 'cli-matrix.json').write_text(json.dumps(rows, indent=2) + '\n')
    cold = sorted(row['layout_ms_cold'] for row in rows)
    timing = {
        'cold_cli_samples_ms': cold,
        'cold_cli_median_ms': median(cold),
        'note': 'cold CLI runs include one-time system font loading; warm median is asserted via the app resize inspection',
    }
    (output / 'layout-timing-cold.json').write_text(json.dumps(timing, indent=2) + '\n')
    assert median(cold) < 200, timing
    return rows


def scene_equality(base, output):
    """CLI `--scene` 与 FFI DiagramEngine 的 semantic_scene 逐字节一致。"""
    vault = base / 'equality-vault'
    vault.mkdir()
    note = vault / 'agent-workflow.md'
    note.write_text(note_for(FIXTURES / 'agent-workflow.mmd'))
    cli_svg = output / 'scene-equality.svg'
    cli_scene_path = output / 'scene-equality.scene.json'
    completed = subprocess.run(
        [str(CLI), 'mermaid', 'render', str(note), '--width', '700', '--theme', 'light',
         '--out', str(cli_svg), '--scene', str(cli_scene_path)],
        capture_output=True, text=True, timeout=120)
    assert completed.returncode == 0, completed.stderr
    cli_scene = json.loads(cli_scene_path.read_text())
    data = inspect(base / 'equality', output, 'scene-equality', vault, note.name,
                   ['--diagram-width=700', '--diagram-reference'])
    assert data['diagrams'] == 1 and not data['reasons'], data
    assert data['scene_widths'] == [700], data
    assert data['initial_scene_widths'] == [700], data
    scenes = data['initial_semantic_scenes']
    assert len(scenes) == 1, data
    ffi_scene = json.loads(scenes[0])
    equality = {
        'strict_equal': ffi_scene == cli_scene,
        'cli_sha256': hashlib.sha256(canonical(cli_scene).encode()).hexdigest(),
        'ffi_sha256': hashlib.sha256(canonical(ffi_scene).encode()).hexdigest(),
        'hit_regions': len(cli_scene['hit_regions']),
        'width': cli_scene['width'],
        'theme': 'light',
    }
    (output / 'scene-equality.json').write_text(json.dumps(equality, indent=2) + '\n')
    assert equality['strict_equal'], equality
    return equality


def warm_timing(base, output):
    """真实窗口连续缩放下的 Rust 排版耗时（字体已预热）。"""
    vault = base / 'timing-vault'
    vault.mkdir()
    note = vault / 'narrow-wrap.md'
    note.write_text(note_for(FIXTURES / 'narrow-wrap.mmd'))
    data = inspect(base / 'timing', output, 'warm-timing', vault, note.name,
                   ['--diagram-width=700', '--diagram-resize'])
    samples = [step['rust_layout_ms'] for step in data['resize'] if step['rust_layout_ms'] > 0]
    assert len(samples) >= 20, len(samples)
    result = {
        'samples': len(samples),
        'median_ms': median(samples),
        'p95_ms': sorted(samples)[int((len(samples) - 1) * 0.95)],
        'max_ms': max(samples),
        'scene_widths': sorted({step['width'] for step in data['resize']}),
        'note': 'real-window resize re-renders after fonts and shaping are warm',
    }
    (output / 'layout-timing-warm.json').write_text(json.dumps(result, indent=2) + '\n')
    assert result['median_ms'] < 15, result
    return result


def error_paths(base, output):
    """语法错误与未支持类型都非零退出且不回退到 SVG。"""
    vault = base / 'error-vault'
    vault.mkdir()
    cases = [
        ('syntax', note_for(FIXTURES / 'syntax-errors.mmd'), 'Syntax'),
        ('unsupported', '```mermaid\nclassDiagram\nA <|-- B\n```\n', 'Unsupported'),
    ]
    rows = []
    for name, content, diagnostic in cases:
        note = vault / f'{name}.md'
        note.write_text(content)
        target = output / f'error-{name}.svg'
        target.unlink(missing_ok=True)
        completed = subprocess.run(
            [str(CLI), 'mermaid', 'render', str(note), '--out', str(target)],
            capture_output=True, text=True, timeout=120)
        assert completed.returncode != 0, (name, completed.stdout)
        assert diagnostic in completed.stderr, (name, completed.stderr)
        assert not target.exists(), name
        rows.append({'case': name, 'diagnostic': diagnostic,
                     'stderr': completed.stderr.strip()})
    # syntax-errors 夹具的错误行号必须指向 alt 语句。
    completed = subprocess.run(
        [str(CLI), 'mermaid', 'render', str(vault / 'syntax.md'), '--out',
         str(output / 'error-syntax.svg')],
        capture_output=True, text=True, timeout=120)
    assert 'unclosed_alt' in completed.stderr, completed.stderr
    (output / 'errors.json').write_text(json.dumps(rows, indent=2) + '\n')
    return rows


def screenshots(base, output, media):
    """真实 build/Hortus.app 阅读视图：700px 与常规窗口 × 浅深色截图。"""
    media.mkdir(parents=True, exist_ok=True)
    results = []
    for fixture in ['agent-workflow.mmd', 'control-flow.mmd']:
        for width in [700, None]:
            for theme in THEMES:
                tag = f'{fixture[:-4]}-{width if width else "regular"}-{theme}'
                extra = ['--diagram-reading', f'--{theme}']
                if width is not None:
                    extra.append(f'--diagram-width={width}')
                data = inspect(base / f'window-{tag}', output, f'window-{tag}',
                               FIXTURES, fixture, extra, media / f'{tag}.png')
                assert data['reading'], tag
                assert data['diagrams'] == 1 and not data['reasons'], (tag, data['reasons'])
                assert data['visible_nodes'] > 0, tag
                assert data['accessibility_nodes'] == data['visible_nodes'], tag
                assert (media / f'{tag}.png').stat().st_size > 10_000, tag
                results.append({
                    'fixture': fixture, 'width': width, 'theme': theme,
                    'reading': data['reading'], 'visible_nodes': data['visible_nodes'],
                    'scene_widths': data['scene_widths'],
                    'screenshot': str(media / f'{tag}.png'),
                })
    (output / 'windows.json').write_text(json.dumps(results, indent=2) + '\n')
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/sequence-diagram')
    parser.add_argument('--media-output', type=Path,
                        default=ROOT / 'target/reports/sequence-diagram/media')
    parser.add_argument('--skip-app', action='store_true')
    parser.add_argument('--skip-cli', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    media = args.media_output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    if not args.skip_cli:
        assert CLI.exists(), f'缺少 release CLI：{CLI}（先 cargo build --release -p hortus-cli）'
        cli_matrix(output)
        print('CLI 矩阵完成', flush=True)
        with tempfile.TemporaryDirectory(prefix='hortus-sequence-errors-') as directory:
            error_paths(Path(directory), output)
        print('错误路径完成', flush=True)
    if not args.skip_app:
        assert APP.exists(), f'缺少真实应用：{APP}（先 ./tools/build-macos.sh）'
        with tempfile.TemporaryDirectory(prefix='hortus-sequence-app-') as directory:
            base = Path(directory)
            scene_equality(base, output)
            print('CLI/FFI 场景一致性完成', flush=True)
            warm_timing(base, output)
            print('预热排版耗时完成', flush=True)
            screenshots(base, output, media)
            print('真实窗口截图完成', flush=True)
    summary = {'seconds': round(time.monotonic() - started, 1),
               'output': str(output), 'media': str(media)}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
