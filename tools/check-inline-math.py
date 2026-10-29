#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""真实应用中的行内公式、两种字号和双主题截图。"""
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
    with tempfile.TemporaryDirectory(prefix='hortus-inline-') as directory:
        base = Path(directory)
        for appearance in ['light', 'dark']:
            for scale in ['1.15', '1.21']:
                target = output / ('scale-' + scale)
                target.mkdir(exist_ok=True)
                name = 'comparison-' + appearance
                data = READING.run(base / (scale + appearance), target, name, ROOT / 'fixtures/math', 'inline.md', 'math',
                    extra=[f'--{appearance}', f'--math-scale={scale}', '--math-reveal=## Scale comparison', f'--snapshot={target / (name + ".png")}'])
                assert data['inline_formulas'] >= 15 and data['missing_glyphs'] == 0, data
                assert data['main_thread_preparations'] == 0, data
                assert data['visible_inline_views'] > 0, data
                assert data['math_scale'] == float(scale), data
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
            name = 'selection-' + appearance
            data = READING.run(base / name, target, name, ROOT / 'fixtures/math', 'inline.md', 'math',
                extra=[f'--{appearance}', '--inline-select', f'--snapshot={target / (name + ".png")}'])
            assert data['selected_inline_length'] == 1, data

if __name__ == '__main__':
    main()
