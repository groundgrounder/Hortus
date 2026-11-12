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


def toggle_spacing(snapshot):
    diagrams = sorted(snapshot['diagrams'], key=lambda row: row['ordinal'])
    assert [row['ordinal'] for row in diagrams] == [1, 2], snapshot
    assert diagrams[1]['frame_min_y'] >= diagrams[0]['frame_max_y'], snapshot
    return diagrams[1]['frame_min_y'] - diagrams[0]['frame_max_y'], diagrams


def apparent_spacing(snapshot, reference):
    """第二张图实际起点减去第一张图按同宽参考场景应占的底部。"""
    diagrams = sorted(snapshot['diagrams'], key=lambda row: row['ordinal'])
    return diagrams[1]['frame_min_y'] - (diagrams[0]['frame_min_y'] + reference[0]['height'])


def check_toggle_spacing(data):
    assert data['mode'] == 'diagram-toggle-spacing', data
    assert data['real_keyboard'] and data['keyboard_access'] and data['activated'], data
    assert data['event_tap'] == 'cghidEventTap' and len(data['key_events']) == 2, data
    assert data['first_view'] != data['final_view'], data
    assert data['source_preserved'] and data['disk_matches_source'], data
    initial, diagrams = toggle_spacing(data['initial_second'])
    reference = sorted(data['reference'], key=lambda row: row['ordinal'])
    assert len(reference) == 2, data['reference']
    for current, canonical in zip(diagrams, reference):
        assert abs(current['scene_width'] - canonical['width']) <= .5, (current, canonical)
        assert abs(current['scene_height'] - canonical['height']) <= .5, (current, canonical)
        assert current['scene_signature'] == canonical['signature'], (current, canonical)
    initial_apparent = apparent_spacing(data['initial_second'], reference)
    assert abs(initial_apparent - initial) <= 1, (initial_apparent, initial)
    assert data['initial_top']['diagrams'][0]['visible'] is True, data['initial_top']
    assert data['initial_top']['diagrams'][1]['visible'] is False, data['initial_top']
    assert data['initial_second']['diagrams'][1]['visible'] is True, data['initial_second']
    assert data['final_top_visible']['diagrams'][0]['visible'] is True, data['final_top_visible']
    assert data['final_second']['diagrams'][1]['visible'] is True, data['final_second']
    for phase in ['initial_return', 'final_second', 'final_top_visible', 'scroll_return',
                  'scroll_return_again', 'restored']:
        snapshot = data[phase]
        spacing, current = toggle_spacing(snapshot)
        assert abs(spacing - initial) <= 1, (phase, spacing, initial, snapshot)
        assert abs(apparent_spacing(snapshot, reference) - initial_apparent) <= 1, (phase, snapshot)
        assert abs(snapshot['document_height'] - data['initial_second']['document_height']) <= 1, (phase, snapshot)
        assert current[1]['frame_max_y'] <= snapshot['document_height'] + .5, (phase, current[1], snapshot)
        assert all(abs(row['scene_width'] - snapshot['content_width']) <= .5 for row in current), (phase, snapshot)
        for before, after in zip(diagrams, current):
            assert before['scene_signature'] == after['scene_signature'], (phase, before, after)
            assert abs(before['scene_width'] - after['scene_width']) <= .5, (phase, before, after)
            assert abs(before['scene_height'] - after['scene_height']) <= .5, (phase, before, after)
            assert after['expanded'] is False, (phase, after)
    resized = data['resized']
    assert resized['content_width'] > data['final_second']['content_width'] + 100, resized
    _, resized_diagrams = toggle_spacing(resized)
    assert resized_diagrams[1]['frame_max_y'] <= resized['document_height'] + .5, resized
    return {'initial_spacing': initial, 'final_spacing': toggle_spacing(data['final_second'])[0],
            'initial_apparent_spacing': initial_apparent,
            'final_apparent_spacing': apparent_spacing(data['final_second'], reference),
            'restored_spacing': toggle_spacing(data['restored'])[0],
            'document_height': data['initial_second']['document_height'],
            'scene_heights': [row['scene_height'] for row in diagrams]}


def check_narrow(data, target):
    assert data['mode'] == 'diagram-narrow', data
    assert data['target_width'] == target, data
    assert data['real_keyboard'] and data['keyboard_access'] and data['activated'], data
    assert data['event_tap'] == 'cghidEventTap' and len(data['key_events']) == 2, data
    assert data['source_preserved'] and data['disk_matches_source'], data
    assert data['view_rebuilt'], data
    phases = {phase['phase']: phase for phase in data['phases']}
    assert set(phases) == {'first_open', 'toggle_editing', 'toggle_return', 'scroll_reveal', 'widened', 'restored'}, phases
    first = phases['first_open']
    assert abs(first['content_width'] - target) <= .5, (first, target)
    for name in ['first_open', 'toggle_editing', 'toggle_return', 'scroll_reveal', 'restored']:
        phase = phases[name]
        assert not phase['pending'], (name, phase)
        assert phase['text_utf16'] > 0, (name, phase)
        assert phase['blocks'] == 2 and phase['diagrams'] == 2, (name, phase)
        assert len(phase['rows']) == 2, (name, phase)
        for row in phase['rows']:
            assert row['prepared'], (name, row)
            assert row['scene_width'] > 0 and row['scene_height'] > 0, (name, row)
            assert row['nodes'] > 0 and row['primitives'] > 0, (name, row)
            assert abs(row['scene_width'] - phase['content_width']) <= .5, (name, row, phase)
        for row in phase['rows'][:1]:
            assert abs(row['scene_width'] - target) <= .5, (name, row, target)
        if name in ['first_open', 'toggle_return', 'scroll_reveal', 'restored']:
            assert abs(phase['content_width'] - target) <= .5, (name, phase)
            assert phase['reading'] is True, (name, phase)
    assert phases['toggle_editing']['reading'] is False, phases['toggle_editing']
    assert phases['scroll_reveal']['rows'][1]['visible'] is True, phases['scroll_reveal']
    widened = phases['widened']
    assert widened['content_width'] > target + 100, widened
    assert widened['blocks'] == 2 and widened['diagrams'] == 2 and not widened['pending'], widened
    assert all(abs(row['scene_width'] - widened['content_width']) <= .5 for row in widened['rows']), widened
    return {'target': target, 'content_width': first['content_width'],
            'scroll_width': first['scroll_width'], 'diagrams': first['diagrams'],
            'scene_widths': [row['scene_width'] for row in first['rows']],
            'nodes': [row['nodes'] for row in first['rows']],
            'toggle_return_content_width': phases['toggle_return']['content_width'],
            'widened_content_width': widened['content_width'],
            'document_height': first['document_height']}


def check_rapid_toggle(data):
    assert data['mode'] == 'rapid-toggle-anchor', data
    assert data['real_keyboard'] and data['keyboard_access'] and data['activated'], data
    assert data['event_tap'] == 'cghidEventTap' and data['rapid_sleep_ms'] == 0, data
    assert data['source_preserved'] and data['disk_matches_source'], data
    assert [round_['phase'] for round_ in data['rounds']] == ['top', 'middle', 'bottom', 'tail', 'widened'], data
    events = data['events']
    summaries = []
    for round_ in data['rounds']:
        phase = round_['phase']
        keys = round_['keys']
        assert len(keys) == 4, (phase, keys)
        assert [key['sequence'] for key in keys] == [1, 1, 2, 2], (phase, keys)
        assert [key['down'] for key in keys] == [True, False, True, False], (phase, keys)
        assert round_['delivered_toggles'] >= 2, (phase, round_)
        before, immediate, final = round_['before'], round_['immediate'], round_['final']
        assert before['view_reading'] and final['view_reading'] and final['document_reading'], (phase, before, final)
        assert final['doc_utf16'] == before['doc_utf16'] and final['revision'] == before['revision'], (phase, before, final)
        assert immediate.get('view_reading') is True, (phase, immediate)
        shift = abs(final['visible'][1] - before['visible'][1])
        immediate_shift = abs(immediate['visible'][1] - before['visible'][1])
        assert shift <= 1, (phase, before['visible'], final['visible'])
        assert immediate_shift <= 1, (phase, before['visible'], immediate['visible'])
        phase_events = [event for event in events if event.get('phase') == phase]
        assert sum(event['event'] == 'toggle_exit' for event in phase_events) >= 2, (phase, phase_events)
        eof_reveals = [event for event in phase_events
                       if event['event'] in ('reveal', 'initial_reveal') and event.get('eof_target')]
        if phase in ('top', 'middle', 'widened'):
            assert not eof_reveals, (phase, eof_reveals)
            assert final['anchor'] < before['doc_utf16'], (phase, final)
        summaries.append({'phase': phase, 'before_y': before['visible'][1], 'immediate_y': immediate['visible'][1],
                          'final_y': final['visible'][1], 'shift_pt': shift,
                          'before_height': before['document_height'], 'final_height': final['document_height'],
                          'before_anchor': before['anchor'], 'final_anchor': final['anchor'],
                          'eof_reveals': len(eof_reveals)})
    repeat = data['repeat']
    assert len(repeat['keys']) == 4 and repeat['delivered_toggles'] >= 2, repeat
    assert repeat['final']['view_reading'] and repeat['final']['document_reading'], repeat
    assert abs(repeat['final']['visible'][1] - repeat['before']['visible'][1]) <= 1, repeat
    assert repeat['immediate'].get('view_reading') is True, repeat
    repeat_events = [event for event in events if event.get('phase') == 'repeat']
    assert not [event for event in repeat_events
                if event['event'] in ('reveal', 'initial_reveal') and event.get('eof_target')], repeat_events
    return {'rounds': summaries, 'repeat_shift_pt': abs(repeat['final']['visible'][1] - repeat['before']['visible'][1]),
            'repeat_final_anchor': repeat['final']['anchor'], 'events': len(events)}


def check_inline_table(data):
    cells = data['table_cells']
    assert [cell['text'] for cell in cells] == [
        '# h', '# \ufffc', '- a', 'b', 'normal',
        '## h', '\ufffc', '> quote', 'em', 'code',
        '# h', '# \ufffc', '- a', 'b', 'normal'], data
    for cell in cells:
        assert all(mask & (0xff000000 | 32 | 64 | 4096 | 32768 | 65536) == 0 for mask in cell['masks']), cell
        assert all(not formula['block'] for formula in cell['formulas']), cell
    assert cells[1]['formulas'] == [{'latex': 'x', 'block': False}], data
    assert cells[6]['formulas'] == [{'latex': 'a|b', 'block': False}], data
    drawn = data['table_drawn_cells']
    assert len(drawn) == len(cells), data
    assert all(size == 15 for cell in drawn for size in cell['font_sizes']), data
    assert all(all(cell['bold']) for cell in drawn[:5]), data
    assert not any(drawn[10]['bold']) and all(drawn[13]['bold']), data
    assert '• ordinary list' in data['display_text'], data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/reading-view')
    parser.add_argument('--cpu-only', action='store_true')
    parser.add_argument('--screenshots-only', action='store_true')
    parser.add_argument('--checks', choices=['all', 'switching', 'toggle-spacing', 'rapid-toggle-anchor',
                                             'narrow', 'performance', 'external'], default='all')
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
            name = 'inline-table-' + name
            data = run(base / name, output, name, ROOT / 'fixtures/math', 'inline-table-context.md',
                extra=[appearance, '--inspect-reading-text', f'--snapshot={output / (name + ".png")}'])
            check_inline_table(data)
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
        if args.checks in ['all', 'narrow']:
            rounds = []
            for target in [120, 150]:
                name = f'narrow-{target}'
                data = run(base / name, output, name, sample, '参考/双图间距.md', 'diagram-narrow',
                           extra=[f'--narrow-width={target}'])
                rounds.append({'target': target, 'data': data})
            report = {'targets': [round_['target'] for round_ in rounds],
                      'rounds': [round_['data'] for round_ in rounds]}
            (output / 'narrow.json').write_text(json.dumps(report, indent=2) + '\n')
            report['rounds'] = [check_narrow(round_['data'], round_['target']) for round_ in rounds]
            (output / 'narrow-summary.json').write_text(json.dumps(report, indent=2) + '\n')
            print('narrow', json.dumps(report), flush=True)
        if args.checks == 'narrow':
            return
        if args.checks in ['all', 'rapid-toggle-anchor']:
            rounds = []
            for index in range(3):
                name = f'rapid-toggle-anchor-{index + 1}'
                events_path = output / f'{name}-events.jsonl'
                events_path.unlink(missing_ok=True)
                data = run(base / name, output, name, sample, '参考/双图间距.md', 'rapid-toggle-anchor',
                           extra=[f'--anchor-inspection={events_path}',
                                  f'--snapshot={output / (name + ".png")}'])
                summary = check_rapid_toggle(data)
                summary['name'] = name
                rounds.append(summary)
            report = {'launches': len(rounds), 'rounds': rounds}
            (output / 'rapid-toggle-anchor.json').write_text(json.dumps(report, indent=2) + '\n')
            print('rapid-toggle-anchor', json.dumps(report), flush=True)
            assert len(rounds) == 3
        if args.checks == 'rapid-toggle-anchor':
            return
        if args.checks in ['all', 'toggle-spacing']:
            rounds = []
            for index in range(3):
                name = f'toggle-spacing-{index + 1}'
                data = run(base / name, output, name, sample, '参考/双图间距.md', 'diagram-toggle-spacing')
                summary = check_toggle_spacing(data)
                summary['name'] = name
                rounds.append(summary)
            report = {'launches': len(rounds), 'rounds': rounds}
            (output / 'toggle-spacing.json').write_text(json.dumps(report, indent=2) + '\n')
            print('toggle-spacing', json.dumps(report), flush=True)
            assert len(rounds) == 3
        if args.checks == 'toggle-spacing':
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
