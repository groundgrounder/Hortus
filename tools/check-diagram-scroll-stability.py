#!/usr/bin/env python3
"""验证阅读与编辑在 legacy/overlay 滚动条下保持稳定内容宽度。"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
SETTINGS = ('always', 'when-scrolling')
MODES = ('reading', 'editing')
STYLES = {'always': 'legacy', 'when-scrolling': 'overlay'}


def rect_width(value):
    match = re.fullmatch(r'\{\{[^,]+, [^}]+\}, \{([0-9.]+), [0-9.]+\}\}', value)
    assert match, value
    return float(match.group(1))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scrollbars', choices=SETTINGS, required=True)
    parser.add_argument('--mode', choices=MODES, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    events_path = output / 'events.jsonl'
    events_path.unlink(missing_ok=True)
    log_path = output / 'app.log'
    with tempfile.TemporaryDirectory(prefix='hortus-scroll-stability-') as temporary:
        base = Path(temporary)
        vault = base / 'vault'
        vault.mkdir()
        block = '\n```mermaid\nmindmap\n  Root\n    One\n    Two\n    Three\n```\n'
        chunk = 'Ordinary Markdown source text for incremental editing.\n' * 3300
        (vault / 'note.md').write_text('\n'.join(chunk + block for _ in range(6)))
        config = base / 'config'
        cache = base / 'cache'
        config.mkdir()
        (config / 'config.toml').write_text(f'font_size = 15.0\ndefault_view = "{args.mode}"\n')
        environment = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache))
        command = [str(APP), f'--vault={vault}', '--note=note.md', '--light',
                   f'--diagram-scroll-stability={events_path}', '--diagram-stability-resize',
                   '-AppleShowScrollBars', 'Always' if args.scrollbars == 'always' else 'WhenScrolling']
        if args.mode == 'reading':
            command.append('--diagram-reading')
        with log_path.open('w') as log:
            completed = subprocess.run(command, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                       timeout=45)
        assert completed.returncode == 0, log_path
        rows = [json.loads(line) for line in events_path.read_text().splitlines() if line.strip()]
        started = next(row for row in rows if row['event'] == 'observation_started')
        finished = next(row for row in rows if row['event'] == 'finished')
        samples = [row for row in rows if row['event'] == 'sample']
        assert samples and samples[-1]['observation_seconds'] >= 8, samples[-1] if samples else None
        assert all(row['view_kind'] == args.mode for row in samples)
        assert all(row['scrollbar_setting'] == ('Always' if args.scrollbars == 'always' else 'WhenScrolling')
                   for row in samples)
        assert all(row['scroller_style'] == STYLES[args.scrollbars] for row in samples)
        actual_styles = {row['scroller_style'] for row in samples}
        assert len(actual_styles) == 1
        widths = {round(row['content_width'], 2) for row in samples}
        assert len(widths) == 1, sorted(widths)
        assert all(row['scene_widths'] and all(abs(width - row['content_width']) <= .5 for width in row['scene_widths'])
                   for row in samples)
        assert all(abs(rect_width(row['document_bounds']) - row['stable_content_width']) <= .5
                   for row in samples)
        assert all(row['pending'] is False for row in samples)
        assert started['refresh_count'] == finished['refresh_count'], (started, finished)
        assert started['render_count'] == finished['render_count'], (started, finished)
        assert started['cancellation_count'] == finished['cancellation_count'], (started, finished)
        summary = {
            'scrollbar_setting': args.scrollbars,
            'mode': args.mode,
            'actual_scroller_style': next(iter(actual_styles)),
            'content_width': next(iter(widths)),
            'scene_widths': samples[-1]['scene_widths'],
            'sample_count': len(samples),
            'observed_seconds': samples[-1]['observation_seconds'],
            'refresh_delta': finished['refresh_count'] - started['refresh_count'],
            'render_delta': finished['render_count'] - started['render_count'],
            'cancellation_delta': finished['cancellation_count'] - started['cancellation_count'],
            'pending_final': finished['pending'],
            'events': str(events_path),
            'log': str(log_path),
        }
        (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + '\n')
        print(json.dumps(summary, ensure_ascii=False, indent=2), flush=True)


if __name__ == '__main__':
    main()
