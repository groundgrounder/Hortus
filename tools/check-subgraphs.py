#!/usr/bin/env python3
"""P1-44：顶层 LR subgraph 方向、汇点、折回与真实文档全 block 检查。"""
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
        'nodes': len(scene['hit_regions']) - len(containers),
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


def point_in_rect(point, rect, epsilon=0.51):
    return (rect['x'] - epsilon <= point['x'] <= rect['x'] + rect['width'] + epsilon
            and rect['y'] - epsilon <= point['y'] <= rect['y'] + rect['height'] + epsilon)


def path_direction_metrics(scene):
    containers = [primitive['rect'] for primitive in scene['primitives']
                  if primitive['kind'] == 'graph_shape' and primitive['shape'] == 'container']
    container_ids = {hit['node_id'] for hit in scene['hit_regions']
                     if hit['rect'] in containers}
    nodes = [hit for hit in scene['hit_regions'] if hit['node_id'] not in container_ids]

    def owner(hit):
        center = (hit['rect']['x'] + hit['rect']['width'] / 2,
                  hit['rect']['y'] + hit['rect']['height'] / 2)
        return next((index for index, rect in enumerate(containers)
                     if point_in_rect({'x': center[0], 'y': center[1]}, rect, 0.0)), None)

    def endpoint(point):
        matches = [hit for hit in nodes if point_in_rect(point, hit['rect'])]
        return min(matches, key=lambda hit: hit['rect']['width'] * hit['rect']['height'],
                   default=None)

    counts = {'internal_paths': 0, 'same_band_right_entries': 0,
              'cross_band_down_entries': 0, 'reverse_edges': 0,
              'invalid_entries': 0, 'container_overflow': 0}
    sinks = {}
    wrapped = set()
    for hit in nodes:
        container = owner(hit)
        if container is not None:
            rect, outer = hit['rect'], containers[container]
            if not (outer['x'] <= rect['x'] and outer['y'] <= rect['y']
                    and rect['x'] + rect['width'] <= outer['x'] + outer['width']
                    and rect['y'] + rect['height'] <= outer['y'] + outer['height']):
                counts['container_overflow'] += 1
    for path in (primitive for primitive in scene['primitives']
                 if primitive['kind'] == 'graph_path'):
        points = path['points']
        if len(points) < 2:
            counts['invalid_entries'] += 1
            continue
        source = endpoint(points[0])
        target = endpoint(points[-1])
        if source is None or target is None:
            continue
        container = owner(source)
        if container is None or container != owner(target):
            continue
        counts['internal_paths'] += 1
        if not all(point_in_rect(point, containers[container]) for point in points):
            counts['container_overflow'] += 1
        source_center = (source['rect']['x'] + source['rect']['width'] / 2,
                         source['rect']['y'] + source['rect']['height'] / 2)
        target_center = (target['rect']['x'] + target['rect']['width'] / 2,
                         target['rect']['y'] + target['rect']['height'] / 2)
        before, after = points[-2], points[-1]
        horizontal = abs(before['y'] - after['y']) < 0.51
        vertical = abs(before['x'] - after['x']) < 0.51
        target_left = abs(after['x'] - target['rect']['x']) < 0.51
        target_top = abs(after['y'] - target['rect']['y']) < 0.51
        source_right = abs(points[0]['x'] - source['rect']['x']
                           - source['rect']['width']) < 0.51
        source_bottom = abs(points[0]['y'] - source['rect']['y']
                            - source['rect']['height']) < 0.51
        if (horizontal and target_left and source_right
                and target_center[0] > source_center[0] + 0.01):
            counts['same_band_right_entries'] += 1
        elif (vertical and target_top and source_bottom
              and target_center[1] > source_center[1] + 0.01):
            counts['cross_band_down_entries'] += 1
            wrapped.add(container)
        elif target_center[0] <= source_center[0] + 0.01:
            counts['reverse_edges'] += 1
        else:
            counts['invalid_entries'] += 1
        sinks.setdefault((container, target['node_id']), []).append(points[-1])

    gaps = []
    for ports in sinks.values():
        if len(ports) < 2:
            continue
        for index, first in enumerate(ports):
            for second in ports[index + 1:]:
                gaps.append(((first['x'] - second['x']) ** 2
                             + (first['y'] - second['y']) ** 2) ** 0.5)
    return {**counts,
            'wrapped_containers': len(wrapped),
            'sink_port_groups': sum(len(ports) >= 2 for ports in sinks.values()),
            'min_sink_port_gap_px': round(min(gaps), 3) if gaps else None}


def public_lr_geometry(scene):
    hits = {hit['node_id']: hit['rect'] for hit in scene['hit_regions']}
    chain_y = [hits[f'C{index}']['y'] for index in range(1, 6)]
    wrap_y = sorted({round(hits[f'W{index}']['y'], 3) for index in range(1, 9)})
    return {'chain_rank_y_px': [round(value, 3) for value in chain_y],
            'chain_max_y_spread_px': round(max(chain_y) - min(chain_y), 3),
            'wrap_band_rows': len(wrap_y),
            'wrap_rank_y_px': wrap_y}


def public_structure(output, media):
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    cases = []
    with tempfile.TemporaryDirectory(prefix='p142-public-') as directory:
        directory = Path(directory)
        for name in ('subgraphs-top-level', 'subgraph-lr-dag', 'invisible-links'):
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
                    geometry = path_direction_metrics(scene)
                    assert counts['overflow'] is False and counts['node_overlaps'] == 0, counts
                    assert counts['invisible_paths'] == 0, counts
                    assert geometry['container_overflow'] == 0, geometry
                    row['renders'].append({'width': width, 'theme': theme,
                                           'svg_sha256': sha256(svg), 'scene_sha256': sha256(scene_path),
                                           **counts, **geometry})
            if name == 'subgraphs-top-level':
                assert all(row['hits'] == 8 and row['nodes'] == 6 and row['containers'] == 2
                           and row['paths'] == 3 and row['invisible_relations'] == 4
                           for row in row['renders']), row
                assert all(row['styled_strokes'] >= 2 and row['dashed_segments'] > 0
                           for row in row['renders']), row
            elif name == 'subgraph-lr-dag':
                assert all(row['hits'] == 26 and row['nodes'] == 22 and row['containers'] == 4
                           and row['paths'] == 19 and row['invisible_paths'] == 0
                           for row in row['renders']), row
                assert all(row['internal_paths'] == 19 and row['reverse_edges'] == 0
                           and row['invalid_entries'] == 0
                           and row['internal_paths'] == row['same_band_right_entries']
                           + row['cross_band_down_entries']
                           and row['min_sink_port_gap_px'] >= 6 for row in row['renders']), row
                assert all(public_lr_geometry(scene)['chain_max_y_spread_px'] <= 0.5
                           for scene in [json.loads((output / f'subgraph-lr-dag-{width}-{theme}.json').read_text())
                                         for width in (700, 1100) for theme in ('light', 'dark')]), row
                for render in row['renders']:
                    scene = json.loads((output / f"subgraph-lr-dag-{render['width']}-{render['theme']}.json").read_text())
                    lr = public_lr_geometry(scene)
                    assert lr['wrap_band_rows'] == (2 if render['width'] == 700 else 1), lr
                    if render['width'] == 700:
                        assert render['cross_band_down_entries'] > 0, render
                    else:
                        assert render['same_band_right_entries'] == 19, render
                        assert render['cross_band_down_entries'] == 0, render
            else:
                assert all(row['hits'] == 3 and row['nodes'] == 3 and row['containers'] == 0
                           and row['paths'] == 1 and row['invisible_relations'] == 3
                           for row in row['renders']), row
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
    blocks = []
    for index, source in enumerate(sources, 1):
        invisible_pairs = []
        for line in source.splitlines():
            if '~~~' not in line:
                continue
            match = re.match(r'\s*([A-Za-z0-9_-]+)\s*~~~\s*([A-Za-z0-9_-]+)', line)
            if match:
                invisible_pairs.append(match.groups())
        renders = []
        for width in (700, 1100):
            scene_path = output / f'block-{index}-{width}-light.json'
            svg = media / f'block-{index}-{width}-light.svg'
            completed = render_cli(private_note, index, width, 'light', svg, scene_path)
            assert completed.returncode == 0, completed.stderr.decode(errors='replace')
            scene = json.loads(scene_path.read_text())
            counts = scene_counts(scene, source, document)
            geometry = path_direction_metrics(scene)
            containers = container_map(document, scene)
            document_bytes = document.encode()
            container_ranges_exact = all(
                document_bytes[hit['source_range']['start']:hit['source_range']['end']]
                .decode().startswith('subgraph ') and
                document_bytes[hit['source_range']['start']:hit['source_range']['end']]
                .decode().rstrip().endswith('end')
                for hit in containers.values()
            )
            external_td = all(containers[a]['rect']['y'] <= containers[b]['rect']['y']
                              for a, b in invisible_pairs if a in containers and b in containers)
            renders.append({'width': width, 'theme': 'light',
                            'container_ranges_exact': container_ranges_exact,
                            **counts, 'external_td': external_td, **geometry,
                            'svg_sha256': sha256(svg), 'scene_sha256': sha256(scene_path)})
            (output / f'block-{index}-{width}-light-scene-semantic.json').write_text(
                json.dumps(scene, ensure_ascii=True, separators=(',', ':')))
        blocks.append({'block': index,
                       'declared_subgraphs': sum(bool(re.match(r'^\s*subgraph\s', line))
                                                 for line in source.splitlines()),
                       'container_direction_lr': sum(bool(re.match(r'^\s*direction\s+LR\s*$', line))
                                                     for line in source.splitlines()),
                       'renders': renders})
    after = sha256(private_note)
    assert before == after
    expected = [
        {'hits': 36, 'nodes': 29, 'containers': 7, 'paths': 24, 'invisible_relations': 6},
        {'hits': 7, 'nodes': 7, 'containers': 0, 'paths': 4, 'invisible_relations': 0},
    ]
    for block, values in zip(blocks, expected):
        for render in block['renders']:
            for key, value in values.items():
                assert render[key] == value, (render, key, value)
            assert render['external_td'] and render['invisible_paths'] == 0, render
            assert render['container_ranges_exact'] and render['container_overflow'] == 0, render
    first = blocks[0]
    assert first['container_direction_lr'] == first['declared_subgraphs'] == 7
    declared = [m.group(1) for m in re.finditer(
        r'(?m)^\s*subgraph\s+([A-Za-z0-9_-]+)', sources[0])]
    assert list(container_map(document, json.loads(
        (output / 'block-1-1100-light.json').read_text()))) == declared, declared
    for render in first['renders']:
        assert render['reverse_edges'] == 0 and render['invalid_entries'] == 0, render
        assert render['internal_paths'] == render['same_band_right_entries'] \
            + render['cross_band_down_entries'], render
        assert render['min_sink_port_gap_px'] >= 6, render
        if render['width'] == 1100:
            assert render['same_band_right_entries'] == 24, render
            assert render['cross_band_down_entries'] == 0, render
            assert render['wrapped_containers'] == 0, render
        else:
            assert render['cross_band_down_entries'] > 0, render
    return {'sha256_before': before, 'sha256_after': after, 'sha256_unchanged': before == after,
            'blocks': blocks}


def private_ui(private_note, output, media):
    CHECK.MEDIA_ROOT = media
    CHECK.PRIVATE_ROOT = media
    states = []
    scene_checks = {}
    with tempfile.TemporaryDirectory(prefix='p142-real-ui-') as directory, \
            tempfile.TemporaryDirectory(prefix='p142-real-scene-') as scene_directory:
        directory = Path(directory)
        scene_directory = Path(scene_directory)
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
                    key = (width, theme)
                    if key not in scene_checks:
                        equal = []
                        for block_index, semantic in enumerate(data['initial_semantic_scenes'], 1):
                            scene_path = scene_directory / f'{width}-{theme}-block-{block_index}.json'
                            svg = scene_directory / f'{width}-{theme}-block-{block_index}.svg'
                            completed = render_cli(private_note, block_index, width, theme, svg, scene_path)
                            assert completed.returncode == 0, completed.stderr.decode(errors='replace')
                            equal.append(canonical(json.loads(semantic))
                                         == canonical(json.loads(scene_path.read_text())))
                        scene_checks[key] = {'blocks': len(equal), 'strict_equal': all(equal)}
                    assert scene_checks[key]['strict_equal'], scene_checks[key]
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
                                   'scene_cli_equal': scene_checks[key]['strict_equal'],
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
                      'real_observed': [[(render['nodes'], render['containers'], render['paths'],
                                          render['invisible_relations'],
                                          render['same_band_right_entries'],
                                          render['cross_band_down_entries'])
                                         for render in block['renders']]
                                        for block in real['blocks']]}, indent=2))


if __name__ == '__main__':
    main()
