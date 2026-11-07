#!/usr/bin/env python3
"""P1-31：一般 flowchart 的真实窗口、源码命中、无障碍和性能验收。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from statistics import median

from importlib.util import module_from_spec, spec_from_file_location

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
CLI = ROOT / 'target/release/hortus'
SAMPLE = ROOT / 'fixtures/vault-sample'
MEDIA_ROOT = ROOT / 'target/reports/diagram-view/media'
PRIVATE_ROOT = Path(tempfile.gettempdir()) / 'hortus-diagram-private'
PRIVATE_NOTE = None
SCROLLBAR_ARGUMENTS = []
SCROLLBAR_SETTING = None
SCROLLBAR_STYLE = None
SPEC = spec_from_file_location('toolbar_check', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def inventory(root):
    return {str(path.relative_to(root)): (path.stat().st_mtime_ns, path.stat().st_size,
            hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else '')
            for path in [root, *root.rglob('*')]}


def global_scrollbar_default():
    completed = subprocess.run(['defaults', 'read', '-g', 'AppleShowScrollBars'],
                               capture_output=True, text=True)
    return {'present': completed.returncode == 0,
            'value': completed.stdout.strip() if completed.returncode == 0 else None}


def prepare_cpu_root(base):
    # 两种 CPU 路径使用同一已初始化配置，排除首次启动目录创建对空载基线的偏差。
    (base / 'config').mkdir(parents=True, exist_ok=True)
    (base / 'cache').mkdir(parents=True, exist_ok=True)
    (base / 'config' / 'config.toml').write_text('font_size = 15.0\n')


def inspect(base, output, name, vault, note, extra=(), screenshot=False):
    output.mkdir(parents=True, exist_ok=True)
    media = PRIVATE_ROOT if output.is_relative_to(PRIVATE_ROOT) else MEDIA_ROOT
    media.mkdir(parents=True, exist_ok=True)
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    appearance = [] if '--dark' in extra else ['--light']
    arguments = [str(APP), *SCROLLBAR_ARGUMENTS, f'--vault={vault}', f'--note={note}', *appearance,
                 f'--diagram-inspection={target}', *extra]
    if screenshot:
        arguments += [f'--snapshot={media / (name + ".png")}']
    # 图表源码投影的检查直接进入编辑视图，重开标签也保持同一模式。
    config = base / 'config'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "editing"\n')
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
    assert data['reading'] == ('--diagram-reading' in extra), data
    if SCROLLBAR_SETTING is not None:
        assert data['scrollbar_setting'] == SCROLLBAR_SETTING, data
        assert data['scroller_style'] == SCROLLBAR_STYLE, data
    assert data['accessibility_nodes'] == data['visible_nodes'], data
    items = data['accessibility_items']
    assert len(items) == data['accessibility_nodes'] + data['accessibility_edges'], data
    assert all(item['label_matches'] and item['frame_error_px'] <= .5 for item in items), items
    assert data['accessibility_edges'] == data['source_hits']['edges'], data
    assert all(not value for block in data['heavy_work_thread_checks'] for value in block.values()), data
    if data['diagrams']:
        assert data['heavy_work_thread_evidence'] and all(data['heavy_work_thread_evidence']), data
        assert all(any(line == 'main_thread=false' for lines in block.values() for line in lines)
                   for block in data['heavy_work_thread_evidence']), data
    if '--diagram-ax-press' in extra:
        assert data['accessibility_presses'] and all(x['performed'] and x['selection_matches'] for x in data['accessibility_presses']), data
    hits = data['source_hits']
    assert hits['node_matches'] == hits['nodes'] and hits['edge_matches'] == hits['edges'], hits
    assert data['accessibility_expanded_valid'], data
    assert data['custom_fragments'] >= data['diagrams'], data
    assert data['error_fragments'] == len(data['reasons']), data
    assert data['width_max_px'] <= 0.5, data
    width = next((float(value.split('=', 1)[1]) for value in extra if value.startswith('--diagram-width=')), None)
    if width is not None:
        assert abs(data['requested_content_width'] - width) <= .5, data
    if data['diagrams']:
        assert data['scene_widths'], data
        if width is not None:
            assert all(abs(value - width) <= .5 for value in data['scene_widths']), data
        if '--diagram-reference' in extra:
            assert data['initial_scene_widths'] and len(data['initial_scene_widths']) == data['diagrams'], data
            assert all(abs(value - width) <= .5 for value in data['initial_scene_widths']), data
            initial_heights = [json.loads(scene)['height'] for scene in data['initial_semantic_scenes']]
            assert len(initial_heights) == len(data['scene_heights']), data
            assert all(abs(after - before) <= .5 for before, after in
                       zip(sorted(initial_heights), sorted(data['scene_heights']))), data
            assert sorted(data['initial_row_counts']) == sorted(data['row_counts']), data
    else:
        assert data['scene_widths'] == [], data
    assert_geometry(name, data)
    return data, exit_delay


def assert_geometry(name, data):
    metrics = data['graph_metrics']
    geometry = data['graph_geometry']
    assert len(metrics) == len(geometry), data
    for summary, actual in zip(metrics, geometry):
        assert actual['paths'] == actual['expected_edges'] == summary['edges'], actual
        assert actual['node_overlaps'] == summary['node_overlaps'] == 0, actual
        assert actual['crossings'] == summary['edge_crossings'], actual
        assert bool(actual['horizontal_overflow']) == bool(summary['horizontal_overflow']), actual
        assert actual['edge_node'] == 0, actual
        assert actual['nonorthogonal'] == 0, actual
        assert actual['label_node'] == actual['label_edge'] == actual['label_label'] == 0, actual
        assert actual['foreign_shared_segments'] == 0, actual
        # 私人原文的任务书未给交叉上限；仍检查节点重叠、穿节点、标签归属、
        # 非正交和横向溢出。
        limit = (1 if name.startswith('dense-dag')
                 else None if name.startswith('dense-40') or name.endswith('-private') else 0)
        if limit is not None:
            assert actual['crossings'] <= limit, actual


def assert_research_geometry(data, source=None):
    if source is None:
        source = (ROOT / 'fixtures/mermaid/flowchart/research.mmd').read_bytes()
    loop = 'I[修正假设] -->|重新检验| D'.encode()
    label_text = '重新检验'.encode()
    assert source.count(loop) == 1 and source.count(label_text) == 1, source
    assert data['visible_nodes'] == 10, data
    assert data['accessibility_edges'] == 10, data
    assert data['graph_metrics'][0]['edges'] == 10, data['graph_metrics']
    assert data['graph_geometry'][0]['paths'] == 10, data['graph_geometry']

    scene = json.loads(data['initial_semantic_scenes'][0])
    nodes = {item['node_id']: item['rect'] for item in scene['hit_regions']}
    paths = [item for item in scene['primitives'] if item['kind'] == 'graph_path']
    texts = [item for item in scene['primitives'] if item['kind'] == 'text']
    backgrounds = {item['edge_id']: item['rect'] for item in scene['primitives']
                   if item['kind'] == 'label_background'}

    def touches(point, rect, side):
        in_y = rect['y'] <= point['y'] <= rect['y'] + rect['height']
        in_x = rect['x'] <= point['x'] <= rect['x'] + rect['width']
        return ((side == 'right' and abs(point['x'] - (rect['x'] + rect['width'])) < .5 and in_y)
                or (side == 'left' and abs(point['x'] - rect['x']) < .5 and in_y)
                or (side == 'bottom' and abs(point['y'] - (rect['y'] + rect['height'])) < .5 and in_x)
                or (side == 'top' and abs(point['y'] - rect['y']) < .5 and in_x))

    center_ports = []
    for rect in nodes.values():
        center_ports.extend([
            {'x': rect['x'] + rect['width'] / 2, 'y': rect['y']},
            {'x': rect['x'] + rect['width'] / 2, 'y': rect['y'] + rect['height']},
            {'x': rect['x'], 'y': rect['y'] + rect['height'] / 2},
            {'x': rect['x'] + rect['width'], 'y': rect['y'] + rect['height'] / 2},
        ])
    for path in paths:
        target = path['points'][-1]
        assert any(abs(target['x'] - port['x']) < .5 and abs(target['y'] - port['y']) < .5
                   for port in center_ports), (path['source_range'], target, center_ports)

    def edge(text):
        matches = [path for path in paths
                   if source[path['source_range']['start']:path['source_range']['end']] == text.encode()]
        assert len(matches) == 1, (text, matches)
        return matches[0]

    def points(text):
        return [(round(point['x'], 3), round(point['y'], 3)) for point in edge(text)['points']]

    def marker_rect(path):
        end, previous = path['points'][-1], path['points'][-2]
        dx, dy = end['x'] - previous['x'], end['y'] - previous['y']
        length = (dx * dx + dy * dy) ** .5
        assert length > 0
        back = {'x': end['x'] - 9 * dx / length, 'y': end['y'] - 9 * dy / length}
        side = {'x': -dy / length * 4, 'y': dx / length * 4}
        vertices = [end, {'x': back['x'] + side['x'], 'y': back['y'] + side['y']},
                    {'x': back['x'] - side['x'], 'y': back['y'] - side['y']}]
        xs, ys = [point['x'] for point in vertices], [point['y'] for point in vertices]
        return {'x': min(xs), 'y': min(ys), 'width': max(xs) - min(xs), 'height': max(ys) - min(ys)}

    def intersects(a, b):
        return (a['x'] < b['x'] + b['width'] - .001 and b['x'] < a['x'] + a['width'] - .001
                and a['y'] < b['y'] + b['height'] - .001 and b['y'] < a['y'] + a['height'] - .001)

    cx, cy = lambda node: nodes[node]['x'] + nodes[node]['width'] / 2, lambda node: nodes[node]['y'] + nodes[node]['height'] / 2
    expected_paths = {
        'C[形成假设] --> D[设计实验]': [
            (cx('C'), nodes['C']['y'] + nodes['C']['height']),
            (cx('D'), nodes['D']['y'])],
        'G -->|是| H[得出结论]': [
            (cx('G'), nodes['G']['y'] + nodes['G']['height']), (cx('H'), nodes['H']['y'])],
        'G -->|否| I[修正假设]': [
            (nodes['G']['x'] + nodes['G']['width'], cy('G')),
            (nodes['I']['x'], cy('I'))],
        'I[修正假设] -->|重新检验| D': [
            (cx('I'), nodes['I']['y']), (cx('I'), cy('D')),
            (nodes['D']['x'] + nodes['D']['width'], cy('D'))],
    }
    for text, expected in expected_paths.items():
        assert points(text) == expected, (text, points(text), expected, nodes)

    loops = [path for path in paths
             if path.get('label_range') is not None
             and touches(path['points'][0], nodes['I'], 'top')
             and touches(path['points'][-1], nodes['D'], 'right')]
    assert len(loops) == 1, paths
    loop_path = loops[0]
    source_range = loop_path['source_range']
    label_range = loop_path['label_range']
    assert source[source_range['start']:source_range['end']] == loop, source_range
    assert source[label_range['start']:label_range['end']] == label_text, label_range
    labels = {text: [item for item in texts if item['text'] == text]
              for text in ('是', '否', '重新检验')}
    assert all(len(items) == 1 for items in labels.values()), labels
    assert all(item['font_size'] == 13.6 and item['font_weight'] == 400
               for items in labels.values() for item in items), labels

    for text, path_text in [
        ('是', 'G -->|是| H[得出结论]'),
        ('重新检验', 'I[修正假设] -->|重新检验| D'),
    ]:
        path = edge(path_text)
        label = labels[text][0]
        background = backgrounds[path['edge_id']]
        assert not intersects(background, marker_rect(path)), (text, background, marker_rect(path))
        vertical = [(a, b) for a, b in zip(path['points'], path['points'][1:])
                    if abs(a['x'] - b['x']) < .001]
        line_x = max(vertical, key=lambda pair: abs(pair[0]['y'] - pair[1]['y']))[0]['x']
        assert background['x'] > line_x + 4, (text, background, line_x)
        assert label['baseline']['x'] > line_x + 4, (text, label, line_x)

    no_path = edge('G -->|否| I[修正假设]')
    no_label, no_background = labels['否'][0], backgrounds[no_path['edge_id']]
    assert not intersects(no_background, marker_rect(no_path)), (no_background, marker_rect(no_path))
    line_y = no_path['points'][0]['y']
    assert no_background['y'] < line_y < no_background['y'] + no_background['height'], no_background
    gap = (nodes['G']['x'] + nodes['G']['width'] + nodes['I']['x']) / 2
    assert abs(no_background['x'] + no_background['width'] / 2 - gap) <= 5, (no_background, gap)
    assert no_background['x'] <= no_label['baseline']['x'], (no_label, no_background)
    assert no_label['baseline']['x'] + no_label['width'] <= no_background['x'] + no_background['width'], (no_label, no_background)

    background = backgrounds[loop_path['edge_id']]
    label = labels['重新检验'][0]
    assert background['x'] <= label['baseline']['x'], (label, background)
    assert label['baseline']['x'] + label['width'] <= background['x'] + background['width'], (label, background)
    assert background['y'] <= label['baseline']['y'] - label['font_size'], (label, background)
    assert label['baseline']['y'] <= background['y'] + background['height'], (label, background)
    return {
        'nodes': data['visible_nodes'],
        'edges': data['accessibility_edges'],
        'edge_id': loop_path['edge_id'],
        'source_range': loop_path['source_range'],
        'label_range': loop_path['label_range'],
        'path': loop_path['points'],
        'label_background': background,
        'label_text': label['text'],
        'labels': {text: {'edge_id': edge(edge_text)['edge_id'],
                          'background': backgrounds[edge(edge_text)['edge_id']],
                          'text': labels[text][0]} for text, edge_text in [
                              ('是', 'G -->|是| H[得出结论]'),
                              ('否', 'G -->|否| I[修正假设]'),
                              ('重新检验', 'I[修正假设] -->|重新检验| D')]},
        'paths': {
            'C_to_D': edge('C[形成假设] --> D[设计实验]')['points'],
            'G_to_H': edge('G -->|是| H[得出结论]')['points'],
            'G_to_I': edge('G -->|否| I[修正假设]')['points'],
            'I_to_D': edge('I[修正假设] -->|重新检验| D')['points'],
        },
    }


def assert_research_loop_label(data, source=None):
    return assert_research_geometry(data, source)


def opening_main_thread_pass(samples, limit=33):
    values = list(samples)
    assert len(values) == 3, values
    return median(values) <= limit


def scenarios(base, output):
    before = inventory(SAMPLE)
    welcome, _ = inspect(base / 'welcome', output, 'welcome-light', SAMPLE, '欢迎.md', screenshot=True)
    assert welcome['diagrams'] == 1 and welcome['visible_nodes'] == 4 and not welcome['dark'], welcome
    flow, _ = inspect(base / 'flow', output, 'flow', SAMPLE, '参考/流程.mmd', screenshot=True)
    assert flow['visible_nodes'] == 3, flow
    dark, _ = inspect(base / 'dark', output, 'dark', SAMPLE, '欢迎.md', ['--dark'], screenshot=True)
    assert dark['dark'], dark
    fold_base = base / 'fold-session'
    folded, _ = inspect(fold_base, output, 'folded', SAMPLE, '欢迎.md', ['--diagram-click'], screenshot=True)
    assert folded['before_click_nodes'] == 4 and folded['visible_nodes'] == 1 and folded['badge_count'] == 1, folded
    restored, _ = inspect(fold_base, output, 'restored', SAMPLE, '欢迎.md', screenshot=True)
    assert restored['visible_nodes'] == 1 and restored['collapsed_nodes'] == 1 and restored['badge_count'] == 1, restored
    expanded, _ = inspect(fold_base, output, 'expanded', SAMPLE, '欢迎.md', ['--diagram-click'], screenshot=True)
    assert expanded['before_click_nodes'] == 1 and expanded['visible_nodes'] == 4 and expanded['collapsed_nodes'] == 0, expanded
    continuity, _ = inspect(base / 'continuity', output, 'accessibility-continuity', SAMPLE,
                            '欢迎.md', ['--diagram-continuity'])
    chain = continuity['accessibility_continuity']
    assert continuity['visible_nodes'] == 4 and continuity['collapsed_nodes'] == 0, continuity
    assert chain['collapsed_press'] and chain['expanded_press'], chain
    assert chain['collapsed_state'] and chain['expanded_state'], chain
    assert chain['collapsed_identity_preserved'] and chain['expanded_identity_preserved'], chain
    assert inventory(SAMPLE) == before, '示例库被改写'
    vault = base / 'errors'
    vault.mkdir()
    cases = [
        ('unsupported', 'sequenceDiagram\nA->>B: Hi', '暂不支持', 'Unsupported'),
        ('not-tree', 'flowchart TB\nA --> C\nB --> C', None, None),
        ('syntax', 'flowchart TB\nA --> B\nB ??? C', '第 3 行语法错误', 'Syntax'),
        ('unsupported-subgraph',
         'flowchart TB\nA --> B\nsubgraph S[C]\nsubgraph T[D]\nD --> E\nend\nend\n',
         '第 4 行的 subgraph 暂不支持', 'UnsupportedConstruct'),
        ('unsupported-linkstyle',
         'flowchart TB\nA --> B\nB --> C\nA --> C\nlinkStyle 0 stroke:#f00\n',
         '第 5 行的 linkStyle 暂不支持', 'UnsupportedConstruct'),
    ]
    for name, source, expected, diagnostic_type in cases:
        (vault / f'{name}.mmd').write_text(source)
        data, _ = inspect(base / name, output, name, vault, f'{name}.mmd', screenshot=True)
        if name == 'not-tree':
            assert data['diagrams'] == 1 and data['visible_nodes'] == 3 and not data['reasons'], data
            assert data['accessibility_edges'] == 2, data
        else:
            assert data['diagrams'] == 0 and data['reasons'][0].startswith(expected), data
            assert diagnostic_type in data['diagnostics'][0], data
    utf8_error = vault / 'utf8-unsupported.md'
    utf8_error.write_text('前言😀\n\n```mermaid\nflowchart TB\nA --> B\nB --> C\nA --> C\nlinkStyle 0 stroke:#f00\n```\n')
    utf8_data, _ = inspect(base / 'utf8-unsupported', output, 'utf8-unsupported', vault,
                           utf8_error.name, ['--diagram-width=700'], screenshot=True)
    assert utf8_data['diagrams'] == 0 and utf8_data['scene_widths'] == [], utf8_data
    assert utf8_data['reasons'][0].startswith('第 8 行的 linkStyle 暂不支持'), utf8_data
    assert 'UnsupportedConstruct' in utf8_data['diagnostics'][0], utf8_data
    (vault / 'mixed.md').write_text('前文😀\n\n```mermaid\nmindmap\n  Root\n    First\n```\n中间\n~~~mermaid\nflowchart TB\nA --> B\n~~~\n末尾\n')
    mixed, _ = inspect(base / 'mixed', output, 'mixed', vault, 'mixed.md')
    assert mixed['diagrams'] == 2 and mixed['visible_nodes'] == 4, mixed
    public_flowcharts = [
        'chinese-labels.mmd', 'class-styles.mmd', 'connectors-shapes.mmd',
        'directions-bt.mmd', 'fanout.mmd', 'forest.mmd', 'graph-syntax.mmd',
        'identity.mmd', 'merge-diamond.mmd', 'research.mmd',
        'dense-40.mmd', 'dense-dag.mmd', 'erikson.mmd',
        'subgraphs-top-level.mmd', 'subgraph-lr-dag.mmd', 'invisible-links.mmd',
    ]
    flowchart_output = output / 'flowchart'
    for name in public_flowcharts:
        sample = ROOT / 'fixtures/mermaid/flowchart' / name
        for width in [700, 1100]:
            for theme in ['light', 'dark']:
                for mode in ['editing', 'reading']:
                    tag = f'{sample.stem}-{width}-{theme}-{mode}'
                    extra = [f'--{theme}', f'--diagram-width={width}', '--diagram-reference']
                    if mode == 'reading': extra.append('--diagram-reading')
                    data, _ = inspect(base / tag, flowchart_output, tag, sample.parent, sample.name,
                                      extra, screenshot=True)
                    assert data['diagrams'] == 1 and data['visible_nodes'] > 0 and not data['reasons'], data
                    assert data['scene_widths'] and all(abs(w - width) < .5 for w in data['scene_widths']), data
                    assert data['accessibility_nodes'] == data['visible_nodes'], data
                    if name == 'research.mmd': assert_research_geometry(data)
                    if name == 'erikson.mmd': assert data['row_counts'] == [[4, 4]], data
                    if name == 'erikson.mmd': assert data['initial_row_counts'] == [[4, 4]], data
    deterministic = ROOT / 'fixtures/mermaid/flowchart/subgraphs-top-level.mmd'
    deterministic_hashes = []
    for index in range(10):
        data, _ = inspect(base / f'determinism-{index}', output, f'determinism-{index}',
                          deterministic.parent, deterministic.name,
                          ['--light', '--diagram-width=700'], index == 0)
        assert data['scene_signatures'], data
        deterministic_hashes.append(data['scene_signatures'][0])
    (output / 'determinism.json').write_text(json.dumps({
        'samples': 10, 'scene_hashes': deterministic_hashes,
        'unique_scene_hashes': len(set(deterministic_hashes)),
    }, indent=2) + '\n')
    assert len(set(deterministic_hashes)) == 1, deterministic_hashes
    scene_vault = base / 'scene-equality'
    scene_vault.mkdir()
    scene_note = scene_vault / 'same-scene.md'
    scene_note.write_text('# Scene equality\n\n```mermaid\nflowchart TB\n  A[开始] --> B[处理]\n  B --> C[结束]\n  A --> C\n```\n')
    scene_data, _ = inspect(base / 'scene-macos', output, 'scene-macos', scene_vault,
                            scene_note.name, ['--light', '--diagram-width=700', '--diagram-reference'])
    assert len(scene_data['initial_semantic_scenes']) == 1, scene_data
    cli_scene = output / 'scene-cli.json'
    cli_svg = MEDIA_ROOT / 'scene-cli.svg'
    completed = subprocess.run([str(CLI), 'mermaid', 'render', str(scene_note), '--block', '1',
                                '--width', '700', '--theme', 'light', '--out', str(cli_svg),
                                '--scene', str(cli_scene)], capture_output=True, timeout=30)
    assert completed.returncode == 0, completed.stderr.decode(errors='replace')
    macos_scene = json.loads(scene_data['initial_semantic_scenes'][0])
    command_scene = json.loads(cli_scene.read_text())
    same_scene = macos_scene == command_scene
    canonical = lambda value: json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False)
    equality = {'strict_equal': same_scene,
                'macos_sha256': hashlib.sha256(canonical(macos_scene).encode()).hexdigest(),
                'cli_sha256': hashlib.sha256(canonical(command_scene).encode()).hexdigest(),
                'blocks': 1, 'width': 700, 'theme': 'light'}
    (output / 'scene-equality.json').write_text(json.dumps(equality, indent=2) + '\n')
    assert same_scene, equality
    anchor_sample = ROOT / 'fixtures/mermaid/flowchart/research.mmd'
    anchor_data, _ = inspect(base / 'anchor-resize', output, 'anchor-resize', anchor_sample.parent,
                             anchor_sample.name, ['--light', '--diagram-width=700',
                                                  '--diagram-reveal', '--diagram-anchor'])
    assert len(anchor_data['anchor_resize']) == 2, anchor_data['anchor_resize']
    for step in anchor_data['anchor_resize']:
        assert step['common_anchors'] > 0, step
        assert step['median_displacement_px'] <= 8, step
        assert step['max_displacement_px'] <= 48, step
    (output / 'anchor-resize.json').write_text(json.dumps(anchor_data['anchor_resize'], indent=2) + '\n')
    ax_sample = ROOT / 'fixtures/mermaid/flowchart/research.mmd'
    inspect(base / 'ax-press', output, 'ax-press', ax_sample.parent, ax_sample.name, ['--diagram-ax-press'])
    if PRIVATE_NOTE is not None:
        original_hash = hashlib.sha256(PRIVATE_NOTE.read_bytes()).hexdigest()
        states = []
        for width in [700, 1100]:
            for theme in ['light', 'dark']:
                for mode in ['editing', 'reading']:
                    tag = f'original-{width}-{theme}-{mode}-private'
                    extra = [f'--diagram-width={width}', f'--{theme}', '--diagram-reveal']
                    if mode == 'reading': extra.append('--diagram-reading')
                    data, _ = inspect(base / tag, PRIVATE_ROOT, tag, PRIVATE_NOTE.parent, PRIVATE_NOTE.name,
                                      extra, True)
                    assert data['diagrams'] > 0 and data['scene_widths'] and all(abs(w-width)<.5 for w in data['scene_widths']), data
                    states.append({'width': width, 'theme': theme, 'mode': mode, 'diagrams': data['diagrams'], 'nodes': data['visible_nodes'],
                                   'source_hits': data['source_hits'], 'fills': data['graph_fill_count'], 'scene_widths': data['scene_widths'],
                                   'window_width': data['window_width'], 'width_max_px': data['width_max_px']})
        assert hashlib.sha256(PRIVATE_NOTE.read_bytes()).hexdigest() == original_hash
        (output / 'private-original-summary.json').write_text(json.dumps({'sha256_unchanged': True, 'states': states}, indent=2)+'\n')
    else:
        print('未指定私人原文：跳过原文四态覆盖', flush=True)
    private = ROOT / 'fixtures/mermaid/tree'
    files = sorted(private.glob('04-*.md')) if private.exists() else []
    if files:
        private_before = inventory(private)
        sample = files[0]
        performance_runs = [inspect(base / f'private-{index}', PRIVATE_ROOT, f'04-default-{index}-private',
                                    private, sample.name, ['--diagram-resize'], index == 0)[0]
                            for index in range(3)]
        performance = performance_runs[0]
        memory_data, _ = inspect(base / 'private-memory', PRIVATE_ROOT, 'memory-only-private', private, sample.name,
                                ['--diagram-memory'])
        assert inventory(private) == private_before, '私人样本被改写'
    else:
        print('私人 04 样本缺失：跳过该样本，使用公开 dense-40 Graph 执行性能/内存验收', flush=True)
        sample = ROOT / 'fixtures/mermaid/flowchart/dense-40.mmd'
        performance_runs = [inspect(base / f'performance-{index}', output, f'dense-40-performance-{index}',
                                    sample.parent, sample.name, ['--diagram-resize'], index == 0)[0]
                            for index in range(3)]
        performance = performance_runs[0]
        memory_data, _ = inspect(base / 'memory', output, 'dense-40-memory', sample.parent,
                                sample.name, ['--diagram-memory'])
    opening_values = [sample['first_main_work_max_ms'] for sample in performance_runs]
    opening_summary = {'samples_ms': opening_values, 'median_ms': median(opening_values), 'limit_ms': 33}
    (output / 'performance-opening.json').write_text(json.dumps(opening_summary, indent=2) + '\n')
    print(json.dumps({'diagram_opening_ms': opening_values,
                      'diagram_opening_median_ms': opening_summary['median_ms']}), flush=True)
    resize = performance['resize']
    assert len(resize) == 60 and all(row['draw_height'] > 0 and row['draw_scale'] >= 1 for row in resize), '缩放用例未绘制可见图'
    totals = sorted(row['total_ms'] for row in resize)
    p95 = totals[(len(totals) * 95 + 99) // 100 - 1]
    memory = memory_data['memory']
    middle = sorted(row['opened_bytes'] for row in memory[5:10])[2]
    end = sorted(row['opened_bytes'] for row in memory[-5:])[2]
    result = {'private_sample_present': bool(files), 'first_main_work_max_ms': performance['first_main_work_max_ms'],
              'resize_samples': len(resize), 'resize_p95_ms': p95, 'resize': resize, 'memory': memory,
              'memory_middle_median_bytes': middle, 'memory_end_median_bytes': end}
    (output / 'performance.json').write_text(json.dumps(result, indent=2) + '\n')
    assert opening_main_thread_pass(opening_values), opening_summary
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
    global MEDIA_ROOT, PRIVATE_ROOT, PRIVATE_NOTE, SCROLLBAR_ARGUMENTS, SCROLLBAR_SETTING, SCROLLBAR_STYLE
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/diagram-view')
    parser.add_argument('--media-output', type=Path, default=MEDIA_ROOT)
    parser.add_argument('--private-output', type=Path, default=PRIVATE_ROOT)
    parser.add_argument('--private-note', type=Path)
    parser.add_argument('--skip-cpu', action='store_true')
    parser.add_argument('--cpu-only', action='store_true')
    parser.add_argument('--scrollbar-style', choices=['always', 'when-scrolling'])
    args = parser.parse_args()
    if args.scrollbar_style:
        SCROLLBAR_SETTING = 'Always' if args.scrollbar_style == 'always' else 'WhenScrolling'
        SCROLLBAR_STYLE = 'legacy' if args.scrollbar_style == 'always' else 'overlay'
        SCROLLBAR_ARGUMENTS = ['-AppleShowScrollBars', SCROLLBAR_SETTING]
    PRIVATE_NOTE = args.private_note.resolve() if args.private_note else None
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    global_before = global_scrollbar_default()
    setting_evidence = {
        'requested_setting': SCROLLBAR_SETTING,
        'expected_scroller_style': SCROLLBAR_STYLE,
        'mechanism': 'process_argument_domain',
        'arguments': SCROLLBAR_ARGUMENTS,
        'global_default_modified': False,
        'global_before': global_before,
        'global_after': None,
        'load_before': os.getloadavg(),
        'load_after': None,
        'restore_action': 'not_required: process-local defaults argument',
    }
    (output / 'scrollbar-setting.json').write_text(json.dumps(setting_evidence, indent=2) + '\n')
    MEDIA_ROOT = args.media_output.resolve()
    PRIVATE_ROOT = args.private_output.resolve()
    PRIVATE_ROOT.mkdir(parents=True, exist_ok=True, mode=0o700)
    PRIVATE_ROOT.chmod(0o700)
    MEDIA_ROOT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-diagrams-') as directory:
        base = Path(directory)
        if not args.cpu_only:
            scenarios(base, output)
        if not args.skip_cpu:
            prepare_cpu_root(base / 'cpu-empty')
            prepare_cpu_root(base / 'cpu-opened')
            empty = HELPERS.idle_cpu(base / 'cpu-empty', SCROLLBAR_ARGUMENTS)
            print('未打开库的 30 s CPU 采样完成', flush=True)
            graph_vault = ROOT / 'fixtures/mermaid/flowchart'
            opened = HELPERS.idle_cpu(base / 'cpu-opened', [*SCROLLBAR_ARGUMENTS, f'--vault={graph_vault}', '--note=dense-40.mmd'])
            difference = opened['average_cpu_percent'] - empty['average_cpu_percent']
            result = {'empty': empty, 'opened': opened, 'difference_percentage_points': difference}
            (output / 'idle-cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            assert abs(difference) <= .1, result
        global_after = global_scrollbar_default()
        setting_evidence['global_after'] = global_after
        setting_evidence['load_after'] = os.getloadavg()
        (output / 'scrollbar-setting.json').write_text(json.dumps(setting_evidence, indent=2) + '\n')
        assert global_after == global_before, (global_before, global_after)


if __name__ == '__main__':
    main()
