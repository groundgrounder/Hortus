#!/usr/bin/env python3
"""P1-42：顶层 subgraph、不可见连线及真实文档全 block 检查。"""
import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / 'target/release/hortus'
FLOWCHART = ROOT / 'fixtures/mermaid/flowchart'
CHECK_SPEC = importlib.util.spec_from_file_location('diagram_check', ROOT / 'tools/check-diagram-view.py')
CHECK = importlib.util.module_from_spec(CHECK_SPEC)
CHECK_SPEC.loader.exec_module(CHECK)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_markdown(path, source):
    path.write_text('# Public fixture\n\n```mermaid\n' + source + '\n```\n')


def render_cli(markdown, block, width, theme, svg, scene):
    completed = subprocess.run(
        [str(CLI), 'mermaid', 'render', str(markdown), '--block', str(block),
         '--width', str(width), '--theme', theme, '--out', str(svg), '--scene', str(scene)],
        capture_output=True,
    )
    return completed


def scene_counts(scene, source, document):
    primitives = scene['primitives']
    containers = [primitive for primitive in primitives
                  if primitive['kind'] == 'graph_shape' and primitive['shape'] == 'container']
    paths = [primitive for primitive in primitives if primitive['kind'] == 'graph_path']
    dashed_segments = [primitive for primitive in primitives if primitive['kind'] == 'line']
    document_bytes = document.encode()
    invisible_sources = [line for line in source.splitlines() if '~~~' in line]
    return {
        'hits': len(scene['hit_regions']),
        'containers': len(containers),
        'paths': len(paths),
        'invisible_relations': sum(line.count('~~~') for line in invisible_sources),
        'invisible_paths': sum(b'~~~' in document_bytes[path['source_range']['start']:path['source_range']['end']]
                               for path in paths),
        'styled_strokes': sum(abs(shape['stroke_width'] - 1.4) > .001
                              for shape in primitives if shape['kind'] == 'graph_shape'),
        'dashed_segments': len(dashed_segments),
        'overflow': scene['metrics']['horizontal_overflow'],
        'node_overlaps': scene['metrics']['node_overlaps'],
    }


def public_structure(output, media):
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    cases = []
    with tempfile.TemporaryDirectory(prefix='p142-public-') as directory:
        directory = Path(directory)
        for name in ('subgraphs-top-level', 'invisible-links'):
            source = (FLOWCHART / f'{name}.mmd').read_text()
            markdown = directory / f'{name}.md'
            write_markdown(markdown, source)
            row = {'name': name, 'renders': []}
            for width in (700, 1100):
                for theme in ('light', 'dark'):
                    tag = f'{name}-{width}-{theme}'
                    svg = media / f'{tag}.svg'
                    scene_path = output / f'{tag}.json'
                    completed = render_cli(markdown, 1, width, theme, svg, scene_path)
                    assert completed.returncode == 0, completed.stderr.decode(errors='replace')
                    scene = json.loads(scene_path.read_text())
                    counts = scene_counts(scene, source, markdown.read_text())
                    assert counts['overflow'] is False and counts['node_overlaps'] == 0, counts
                    assert counts['invisible_paths'] == 0, counts
                    row['renders'].append({'width': width, 'theme': theme,
                                           'svg_sha256': sha256(svg), 'scene_sha256': sha256(scene_path),
                                           **counts})
            if name == 'subgraphs-top-level':
                assert all(row['hits'] == 8 and row['containers'] == 2 and row['paths'] == 3
                           and row['invisible_relations'] == 4 for row in row['renders']), row
                assert all(row['styled_strokes'] >= 2 and row['dashed_segments'] > 0
                           for row in row['renders']), row
            else:
                assert all(row['hits'] == 3 and row['containers'] == 0 and row['paths'] == 1
                           and row['invisible_relations'] == 3 for row in row['renders']), row
            cases.append(row)
    return cases


def public_ui(output, media):
    CHECK.MEDIA_ROOT = media
    CHECK.PRIVATE_ROOT = output / 'unused-private'
    states = []
    with tempfile.TemporaryDirectory(prefix='p142-ui-') as directory:
        directory = Path(directory)
        for width in (700, 1100):
            for theme in ('light', 'dark'):
                for mode in ('editing', 'reading'):
                    name = f'public-{width}-{theme}-{mode}'
                    extra = [f'--{theme}', f'--diagram-width={width}', '--diagram-reference']
                    if mode == 'editing':
                        extra.append('--diagram-ax-press')
                    else:
                        extra.append('--diagram-reading')
                    data, _ = CHECK.inspect(directory / name, output / 'ui', name,
                                            FLOWCHART, 'subgraphs-top-level.mmd', extra, True)
                    assert data['diagrams'] == 1 and data['visible_nodes'] == 8, data
                    assert data['accessibility_nodes'] == 8 and data['accessibility_edges'] == 3, data
                    assert data['graph_metrics'][0]['edges'] == 3, data
                    assert data['graph_metrics'][0]['invisible_edges'] == 4, data
                    assert data['graph_metrics'][0]['containers'] == 2, data
                    if mode == 'editing':
                        assert data['source_hits'] == {'nodes': 8, 'node_matches': 8, 'edges': 3, 'edge_matches': 3}, data
                        assert len(data['accessibility_presses']) == 11, data
                        assert all(press['performed'] and press['selection_matches']
                                   for press in data['accessibility_presses']), data
                    states.append({'width': width, 'theme': theme, 'mode': mode,
                                   'scene_signatures': data['scene_signatures'],
                                   'cli_signatures': data['initial_scene_signatures'],
                                   'source_hits': data['source_hits'],
                                   'geometry': data['graph_geometry']})
    anchor, _ = CHECK.inspect(
        directory / 'anchor', output / 'ui', 'public-anchor', FLOWCHART,
        'subgraphs-top-level.mmd',
        ['--light', '--diagram-width=700', '--diagram-reference', '--diagram-reveal', '--diagram-anchor'],
    )
    assert len(anchor['anchor_resize']) == 2, anchor
    for step in anchor['anchor_resize']:
        assert step['common_anchors'] == 8, step
        assert step['median_displacement_px'] <= 8 and step['max_displacement_px'] <= 48, step
    return states, anchor['anchor_resize']


def public_determinism(output):
    CHECK.MEDIA_ROOT = output / 'unused-media'
    CHECK.PRIVATE_ROOT = output / 'unused-private'
    signatures = []
    semantic = None
    with tempfile.TemporaryDirectory(prefix='p142-determinism-') as directory:
        directory = Path(directory)
        vault = directory / 'vault'
        vault.mkdir()
        note = vault / 'subgraphs.md'
        write_markdown(note, (FLOWCHART / 'subgraphs-top-level.mmd').read_text())
        for index in range(10):
            data, _ = CHECK.inspect(directory / f'run-{index}', output / 'determinism', f'run-{index}',
                                    vault, note.name,
                                    ['--light', '--diagram-width=700', '--diagram-reference'])
            signatures.append(data['scene_signatures'][0])
            semantic = data['initial_semantic_scenes'][0]
        scene_path = output / 'deterministic-cli-scene.json'
        svg = output / 'deterministic-cli-scene.svg'
        completed = render_cli(note, 1, 700, 'light', svg, scene_path)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')
    assert len(set(signatures)) == 1, signatures
    cli_scene = json.loads(scene_path.read_text())
    same = canonical(cli_scene) == canonical(json.loads(semantic))
    assert same
    return {'samples': 10, 'unique_scene_signatures': len(set(signatures)),
            'scene_signatures': signatures, 'cli_strict_equal': same,
            'cli_scene_sha256': hashlib.sha256(canonical(cli_scene).encode()).hexdigest()}


def mermaid_sources(markdown):
    pattern = re.compile(r'(?ms)^[ \t]*([`~]{3,})mermaid[^\n]*\n(.*?)^[ \t]*\1[ \t]*$')
    return [match.group(2) for match in pattern.finditer(markdown)]


def container_map(document, scene):
    result = {}
    document_bytes = document.encode()
    for hit in scene['hit_regions']:
        text = document_bytes[hit['source_range']['start']:hit['source_range']['end']].decode()
        match = re.match(r'subgraph\s+([A-Za-z0-9_-]+)', text)
        if match:
            result[match.group(1)] = hit
    return result


def real_structure(private_note, output, media):
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    before = sha256(private_note)
    document = private_note.read_text()
    sources = mermaid_sources(document)
    rows = []
    for index, source in enumerate(sources, 1):
        scene_path = output / f'block-{index}-700-light.json'
        svg = media / f'block-{index}-700-light.svg'
        completed = render_cli(private_note, index, 700, 'light', svg, scene_path)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')
        scene = json.loads(scene_path.read_text())
        counts = scene_counts(scene, source, document)
        containers = container_map(document, scene)
        document_bytes = document.encode()
        container_ranges_exact = all(
            document_bytes[hit['source_range']['start']:hit['source_range']['end']]
            .decode().startswith('subgraph ') and
            document_bytes[hit['source_range']['start']:hit['source_range']['end']]
            .decode().rstrip().endswith('end')
            for hit in containers.values()
        )
        invisible_pairs = []
        for line in source.splitlines():
            if '~~~' not in line:
                continue
            match = re.match(r'\s*([A-Za-z0-9_-]+)\s*~~~\s*([A-Za-z0-9_-]+)', line)
            if match:
                invisible_pairs.append(match.groups())
        external_td = all(containers[a]['rect']['y'] <= containers[b]['rect']['y']
                          for a, b in invisible_pairs if a in containers and b in containers)
        paths = [primitive for primitive in scene['primitives'] if primitive['kind'] == 'graph_path']
        internal_paths = []
        for path in paths:
            start, end = path['points'][0], path['points'][-1]
            for rect in (primitive['rect'] for primitive in scene['primitives']
                         if primitive['kind'] == 'graph_shape' and primitive['shape'] == 'container'):
                inside = lambda point: (rect['x'] <= point['x'] <= rect['x'] + rect['width']
                                        and rect['y'] <= point['y'] <= rect['y'] + rect['height'])
                if inside(start) and inside(end):
                    internal_paths.append(start['x'] <= end['x'])
                    break
        row = {'block': index, 'declared_subgraphs': sum(bool(re.match(r'^\s*subgraph\s', line))
                                                         for line in source.splitlines()),
               'container_direction_lr': sum(bool(re.match(r'^\s*direction\s+LR\s*$', line))
                                             for line in source.splitlines()),
               'container_ranges_exact': container_ranges_exact,
               **counts, 'external_td': external_td,
               'internal_paths': len(internal_paths),
               'internal_ltr_paths': sum(internal_paths), 'source_sha256': hashlib.sha256(source.encode()).hexdigest(),
               'svg_sha256': sha256(svg), 'scene_sha256': sha256(scene_path)}
        rows.append(row)
        source_path = output / f'block-{index}-700-light-scene-semantic.json'
        source_path.write_text(json.dumps(scene, ensure_ascii=True, separators=(',', ':')))
    after = sha256(private_note)
    assert before == after
    expected = [
        {'hits': 36, 'containers': 7, 'paths': 24, 'invisible_relations': 6},
        {'hits': 7, 'containers': 0, 'paths': 4, 'invisible_relations': 0},
    ]
    for row, values in zip(rows, expected):
        for key, value in values.items():
            assert row[key] == value, (row, key, value)
    assert rows[0]['external_td'] and rows[0]['invisible_paths'] == 0
    assert rows[0]['container_ranges_exact']
    assert rows[0]['container_direction_lr'] == rows[0]['declared_subgraphs'] == 7
    assert rows[1]['external_td'] and rows[1]['invisible_paths'] == 0
    return {'sha256_before': before, 'sha256_after': after, 'sha256_unchanged': before == after,
            'blocks': rows,
            'task_expected': {'block2_hits': 36, 'block2_paths': 35},
            'task_expected_match': rows[1]['hits'] == 36 and rows[1]['paths'] == 35}


def private_ui(private_note, output, media):
    CHECK.MEDIA_ROOT = media
    CHECK.PRIVATE_ROOT = media
    states = []
    with tempfile.TemporaryDirectory(prefix='p142-real-ui-') as directory:
        directory = Path(directory)
        for width in (700, 1100):
            for theme in ('light', 'dark'):
                for mode in ('editing', 'reading'):
                    name = f'real-{width}-{theme}-{mode}-private'
                    extra = [f'--{theme}', f'--diagram-width={width}', '--diagram-reference',
                             '--diagram-reveal']
                    if mode == 'editing':
                        extra.append('--diagram-ax-press')
                    else:
                        extra.append('--diagram-reading')
                    data, _ = CHECK.inspect(directory / name, output / 'ui', name,
                                            private_note.parent, private_note.name, extra, True)
                    assert data['diagrams'] == 2 and data['blocks'] == 2, data
                    assert data['visible_nodes'] == 43 and data['accessibility_nodes'] == 43, data
                    assert data['accessibility_edges'] == 28, data
                    assert not data['reasons'] and not data['diagnostics'], data
                    metrics = sorted(data['graph_metrics'], key=lambda item: item['containers'])
                    assert metrics[0]['edges'] == 4 and metrics[0]['invisible_edges'] == 0, data
                    assert metrics[1]['edges'] == 24 and metrics[1]['invisible_edges'] == 6, data
                    assert metrics[1]['containers'] == 7, data
                    assert all(item['node_overlaps'] == 0 and item['horizontal_overflow'] == 0
                               for item in metrics), data
                    if mode == 'editing':
                        assert data['source_hits'] == {'nodes': 43, 'node_matches': 43, 'edges': 28, 'edge_matches': 28}, data
                        assert data['accessibility_presses'], data
                        assert all(press['performed'] and press['selection_matches']
                                   for press in data['accessibility_presses']), data
                    states.append({'width': width, 'theme': theme, 'mode': mode,
                                   'scene_signatures': data['scene_signatures'],
                                   'cli_signatures': data['initial_scene_signatures'],
                                   'source_hits': data['source_hits'],
                                   'graph_metrics': metrics,
                                   'presses': len(data['accessibility_presses'])})
    return states


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--media-output', type=Path, required=True)
    parser.add_argument('--private-output', type=Path, required=True)
    parser.add_argument('--private-note', type=Path, required=True)
    parser.add_argument('--structure-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    media = args.media_output.resolve()
    private = args.private_output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    private.mkdir(parents=True, exist_ok=True, mode=0o700)
    private.chmod(0o700)
    public = public_structure(output / 'cli', media / 'synthetic')
    real = real_structure(args.private_note.resolve(), private / 'structure', private / 'structure-media')
    summary = {'public': public, 'real': real}
    if not args.structure_only:
        summary['public_ui'], summary['public_anchor'] = public_ui(
            output / 'public-ui', media / 'synthetic-ui')
        summary['determinism'] = public_determinism(output)
        summary['private_ui'] = private_ui(args.private_note.resolve(), private / 'ui', private / 'media')
    (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'public_renders': len(public),
                      'real_blocks': len(real['blocks']),
                      'real_sha256_unchanged': real['sha256_unchanged'],
                      'real_observed': [(row['hits'], row['containers'], row['paths'],
                                         row['invisible_relations']) for row in real['blocks']],
                      'task_expected_match': real['task_expected_match']}, indent=2))


if __name__ == '__main__':
    main()
