#!/usr/bin/env python3
"""检查类图与 ER 图的解析、几何、跨入口场景、耗时和真实阅读窗口。"""

import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
from statistics import median
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
CLASS = ROOT / 'fixtures/mermaid/class'
ER = ROOT / 'fixtures/mermaid/er'
SPEC = spec_from_file_location('sequence_check', ROOT / 'tools/check-sequence-diagram.py')
SEQUENCE = module_from_spec(SPEC)
SPEC.loader.exec_module(SEQUENCE)


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def cli(*args, ok=True):
    result = subprocess.run([str(SEQUENCE.CLI), 'mermaid', *map(str, args)],
                            capture_output=True, text=True, timeout=120)
    assert (result.returncode == 0) == ok, (args, result.stderr)
    return result


def note_for(fixture):
    return f'# {fixture.stem}\n\n~~~mermaid\n{fixture.read_text()}~~~\n'


def intersects(a, b, epsilon=0.01):
    return all(a[pos] + a[size] > b[pos] + epsilon and b[pos] + b[size] > a[pos] + epsilon
               for pos, size in [('x', 'width'), ('y', 'height')])


def crosses(a, b, rect, epsilon=0.01):
    axis, fixed, extent, thickness = ('y', 'x', 'height', 'width') if abs(a['x'] - b['x']) < 0.001 else (
        'x', 'y', 'width', 'height')
    assert abs(a[fixed] - b[fixed]) < 0.001, (a, b)
    return (rect[fixed] + epsilon < a[fixed] < rect[fixed] + rect[thickness] - epsilon
            and min(a[axis], b[axis]) < rect[axis] + rect[extent] - epsilon
            and max(a[axis], b[axis]) > rect[axis] + epsilon)


def card_rects(scene, prefix):
    return [hit['rect'] for hit in scene['hit_regions']
            if hit['node_id'].startswith(prefix)]


def check_geometry(tag, scene, note):
    def contains(rect, point, epsilon=0.01):
        return all(rect[axis] - epsilon <= point[axis] <= rect[axis] + rect[size] + epsilon
                   for axis, size in [('x', 'width'), ('y', 'height')])

    shapes = [p for p in scene['primitives'] if p['kind'] == 'graph_shape']
    labels = [p['rect'] for p in scene['primitives'] if p['kind'] == 'label_background']
    hits = scene['hit_regions']
    ids = {hit['node_id'] for hit in hits}
    assert len(ids) == len(hits) and hits, tag
    assert all(not hit['can_collapse'] for hit in hits), tag
    raw = note.read_bytes()
    for hit in hits:
        span = hit['source_range']
        assert raw[span['start']:span['end']].decode(), (tag, hit)
    cards = card_rects(scene, 'class-') + card_rects(scene, 'entity-')
    notes = card_rects(scene, 'note-')
    for index, label in enumerate(labels):
        assert not any(intersects(label, other) for other in labels[:index]), (tag, label)
        assert not any(intersects(label, rect) for rect in cards + notes), (tag, label, 'card')
    paths = [p for p in scene['primitives'] if p['kind'] == 'graph_path']
    assert paths, tag
    for path in paths:
        points = path['points']
        assert len(points) >= 2, (tag, path)
        assert path['start'] == 'none' and path['end'] == 'none', (tag, path)
        for rect in cards:
            if contains(rect, points[0]) or contains(rect, points[-1]):
                continue
            assert not any(crosses(a, b, rect) for a, b in zip(points, points[1:])), (tag, path, rect)
    markers = [p for p in shapes if len(p['outline']) in (2, 3, 4, 97)]
    assert markers, tag
    return {'paths': len(paths), 'labels': len(labels), 'hit_regions': len(hits),
            'cards': len(cards), 'markers': len(markers)}


def render(fixture, note, width, theme, output, media):
    tag = f'{fixture.stem}-{width}-{theme}'
    svg = media / f'{tag}.svg'
    metrics_path = output / f'{tag}.metrics.json'
    scene_path = output / f'{tag}.scene.json'
    cli('render', note, '--width', width, '--theme', theme, '--out', svg,
        '--metrics', metrics_path, '--scene', scene_path)
    metrics = json.loads(metrics_path.read_text())
    scene = json.loads(scene_path.read_text())
    kind = 'class' if fixture.parent.name == 'class' else 'er'
    assert metrics['columns'] == 1 and metrics['diagram_kind'] == kind, (tag, metrics)
    assert not metrics['horizontal_overflow'] and metrics['node_overlaps'] == 0, (tag, metrics)
    assert metrics['min_font_px'] >= 12.5 and scene['width'] == width, (tag, metrics)
    aria = 'Mermaid class diagram' if kind == 'class' else 'Mermaid entity-relationship diagram'
    assert f'aria-label="{aria}"' in svg.read_text(), tag
    assert scene['metrics']['layout_ms'] == 0, tag
    geometry = check_geometry(tag, scene, note)
    return scene, {
        'fixture': fixture.name, 'width': width, 'theme': theme, **geometry,
        'height': metrics['diagram_height'], 'node_overlaps': metrics['node_overlaps'],
        'horizontal_overflow': metrics['horizontal_overflow'],
        'min_font_px': metrics['min_font_px'], 'cold_layout_ms': metrics['layout_ms'],
        'scene_sha256': hashlib.sha256(SEQUENCE.canonical(scene).encode()).hexdigest(),
    }


def error_paths(base):
    rows = []
    for source, line, diagnostic, kind in [
        ('classDiagram', 4, 'class_empty', 'syntax'),
        ('classDiagram\nclass Ring {', 5, 'class_unclosed_block', 'syntax'),
        ('classDiagram\nclass Ring\n}', 6, 'class_unexpected_close', 'syntax'),
        ('classDiagram\nA -->', 5, 'class_invalid_relation', 'syntax'),
        ('classDiagram\ndirection XX', 5, 'class_invalid_direction', 'syntax'),
        ('erDiagram', 4, 'er_empty', 'syntax'),
        ('erDiagram\nUSER {', 5, 'er_unclosed_block', 'syntax'),
        ('erDiagram\nUSER {\nbigint\n}', 6, 'er_invalid_attribute', 'syntax'),
        ('erDiagram\nA xx--yy B : x', 5, 'er_invalid_relation', 'syntax'),
        ('gantt\ntitle 计划', 2, 'Unsupported', 'unsupported'),
    ]:
        note = base / 'errors.md'
        note.write_text(f'序😀\n> ~~~mermaid\n> %% 注释\n'
                        + ''.join(f'> {s}\n' for s in source.splitlines()) + '> ~~~\n')
        report = json.loads(cli('check', note, ok=False).stdout)['blocks'][0]
        assert not report['valid'] and report['error_kind'] == kind and report['line'] == line, report
        if kind == 'syntax':
            assert report['diagram_type'] in ('class', 'er') and diagnostic in report['message'], report
        svg = base / 'error.svg'
        result = cli('render', note, '--out', svg, ok=False)
        assert diagnostic in result.stderr and not svg.exists(), (report, result.stderr)
        rows.append(report)
    return rows


def check_windows(base, output, media, rendered):
    rows = []
    for tag, note, scene, matrix in rendered:
        width, theme = matrix['width'], matrix['theme']
        png = media / f'{note.stem}-reading-{theme}.png' if width == 700 else None
        data = SEQUENCE.inspect(base / tag, output, tag, note.parent, note.name,
                                ['--diagram-reading', '--diagram-reference',
                                 f'--diagram-width={width}', f'--{theme}'], png)
        assert data['reading'] and data['diagrams'] == 1 and not data['reasons'], (tag, data)
        assert data['visible_nodes'] == matrix['hit_regions'], (tag, data)
        assert data['visible_nodes'] == data['accessibility_nodes'], (tag, data)
        assert data['dark'] == (theme == 'dark') and data['source_preserved'], (tag, data)
        assert data['initial_scene_widths'] == [width], (tag, data)
        ffi_scene, = map(json.loads, data['initial_semantic_scenes'])
        assert ffi_scene == scene, f'{tag} CLI 与 FFI 场景不同'
        hit_ids = [hit['node_id'] for hit in ffi_scene['hit_regions']]
        assert len(hit_ids) == len(set(hit_ids)), tag
        if png:
            assert png.stat().st_size > 10_000, tag
        rows.append({'fixture': note.stem, 'width': width, 'theme': theme,
                     'reading': True, 'strict_equal': True,
                     'scene_sha256': matrix['scene_sha256'],
                     'visible_nodes': data['visible_nodes'],
                     'screenshot': str(png) if png else None})
    return rows


def check_timing(base, output, rendered):
    rows = []
    for tag, note, _, matrix in rendered:
        width, theme = matrix['width'], matrix['theme']
        result = SEQUENCE.inspect(base / f'timing-{tag}', output, f'timing-{tag}',
                                  note.parent, note.name,
                                  [f'--diagram-width={width}', '--diagram-resize', f'--{theme}'])
        samples = [s['rust_layout_ms'] for s in result['resize'] if s['rust_layout_ms'] > 0]
        assert len(samples) >= 20, (tag, samples)
        typical = median(samples)
        assert typical < 15, (tag, typical)
        rows.append({'fixture': note.stem, 'width': width, 'theme': theme,
                     'samples_ms': samples, 'median_ms': typical, 'max_ms': max(samples)})
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/class-er-diagrams')
    parser.add_argument('--media', type=Path,
                        default=ROOT / 'target/reports/class-er-diagrams/media')
    args = parser.parse_args()
    output, media = args.output.resolve(), args.media.resolve()
    for path in [output, media, output / 'windows', output / 'timing']:
        path.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='hortus-uml-') as temp:
        base = Path(temp)
        vault = base / 'vault'
        vault.mkdir()
        rendered = []
        checked = []
        for folder in [CLASS, ER]:
            for fixture in sorted(folder.glob('*.mmd')):
                note = vault / f'{fixture.stem}.md'
                note.write_text(note_for(fixture))
                for width in [700, 1100]:
                    for theme in ['light', 'dark']:
                        tag = f'{fixture.stem}-{width}-{theme}'
                        report = json.loads(cli('check', note).stdout)
                        block, = report['blocks']
                        expected = 'class' if folder == CLASS else 'er'
                        assert report['valid'] and block['diagram_type'] == expected and block['valid'], report
                        checked.append({'tag': tag, **block})
                        scene, matrix = render(fixture, note, width, theme, output, media)
                        rendered.append((tag, note, scene, matrix))
        assert rendered, '缺少类图或 ER 图夹具'
        write_json(output / 'cli-matrix.json', [entry[3] for entry in rendered])
        write_json(output / 'cli-check.json', checked + error_paths(base))
        print(f'CLI checks and geometry: {len(rendered)} scenes', flush=True)
        write_json(output / 'scene-equality.json', check_windows(base, output / 'windows', media, rendered))
        print('CLI/FFI semantic scenes and reading window screenshots match', flush=True)
        write_json(output / 'layout-timing-warm.json', check_timing(base, output / 'timing', rendered))
    print(f'PASS: class and ER diagram checks in {time.monotonic() - started:.1f} s')


if __name__ == '__main__':
    main()
