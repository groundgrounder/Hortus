#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""真实应用中的行内公式和双主题截图。"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('reading', ROOT / 'tools/check-reading-view.py')
READING = module_from_spec(SPEC)
SPEC.loader.exec_module(READING)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/inline-math')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    crlf = (ROOT / 'fixtures/math/inline-crlf.md').read_bytes()
    assert b'\r\n' in crlf and b'\n' not in crlf.replace(b'\r\n', b''), 'CRLF fixture was normalized'
    with tempfile.TemporaryDirectory(prefix='hortus-inline-') as directory:
        base = Path(directory)
        for appearance in ['light', 'dark']:
            target = output / 'scale-1.21'
            target.mkdir(exist_ok=True)
            name = 'comparison-' + appearance
            data = READING.run(base / name, target, name, ROOT / 'fixtures/math', 'inline.md', 'math',
                extra=[f'--{appearance}', '--math-reveal=## Scale comparison', f'--snapshot={target / (name + ".png")}'])
            assert data['inline_formulas'] >= 15 and data['missing_glyphs'] == 0, data
            assert data['main_thread_preparations'] == 0 and data['visible_inline_views'] > 0, data
            assert data['math_scale'] == 1.21, data
            target = output / 'inline-view'
            target.mkdir(exist_ok=True)
            for section in ['Tall inline', 'Heading', 'Table', 'Containers', 'Mixed display', 'Errors', 'Wide inline']:
                name = section.lower().replace(' ', '-') + '-' + appearance
                extra = ['--inline-scroll-start'] if section == 'Wide inline' else [f'--math-reveal=## {section}']
                data = READING.run(base / name, target, name, ROOT / 'fixtures/math', 'inline.md', 'math',
                    extra=[f'--{appearance}', *extra, f'--snapshot={target / (name + ".png")}'])
                assert data['visible_inline_views'] > 0 and len(data['inline_errors']) == 1, data
                if section == 'Wide inline':
                    assert data['inline_scroll_offset'] == 0 and not data['page_horizontal_scroll'], data
            name = 'wide-end-' + appearance
            data = READING.run(base / name, target, name, ROOT / 'fixtures/math', 'inline.md', 'math',
                extra=[f'--{appearance}', '--inline-scroll-end', f'--snapshot={target / (name + ".png")}'])
            assert data['inline_scroll_offset'] > 0, data
            assert not data['page_horizontal_scroll'], data
            for section, note in [('CRLF', 'inline-crlf.md'), ('Parenthesis list', 'inline-fixes.md'),
                    ('Table fixes', 'inline-fixes.md'), ('Callout display', 'display-callout.md'),
                    ('Footnote display', 'display-footnote.md'), ('Comment display', 'display-comment.md')]:
                name = section.lower().replace(' ', '-') + '-' + appearance
                data = READING.run(base / name, target, name, ROOT / 'fixtures/math', note, 'math',
                    extra=[f'--{appearance}', f'--math-reveal=## {section}', f'--snapshot={target / (name + ".png")}'])
                assert data['missing_glyphs'] == 0, data
                if note.startswith('display-'):
                    assert data['math_blocks'] == (2 if section == 'Comment display' else 1), data
            for focus in ['body', 'sidebar']:
                name = 'selection-' + focus + '-' + appearance
                data = READING.run(base / name, target, name, ROOT / 'fixtures/math', 'inline-fixes.md', 'math',
                    extra=[f'--{appearance}', '--inline-select', *(['--inline-sidebar-focus'] if focus == 'sidebar' else []),
                           f'--snapshot={target / (name + ".png")}'])
                assert data['selected_inline_length'] > 1 and data['selection_window_key'], data
                assert data['selection_focus_body'] == (focus == 'body'), data
                assert data['selection_background_delta'] <= .03, data
                if focus == 'sidebar':
                    assert data['sidebar_focused'], data

if __name__ == '__main__':
    main()
