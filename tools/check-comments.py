#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""公开注释夹具的逐字检查及双主题截图。"""
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
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/comments')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    screenshots = args.screenshots.resolve() if args.screenshots else output
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-comments-') as directory:
        for appearance in ['light', 'dark']:
            for note in ['comments.md', 'callouts.md', 'lists.md']:
                name = note.removesuffix('.md') + '-' + appearance
                data = READING.run(Path(directory) / name, output, name, ROOT / 'fixtures/comments', note,
                    extra=[f'--{appearance}', '--inspect-reading-text', f'--snapshot={screenshots / (name + ".png")}'])
                text = data['display_text']
                assert 'SPDX' not in text and '%%' not in text, data
                if note == 'comments.md':
                    assert 'hidden' not in text and 'continued' not in text, data
                    assert 'Inline: before  after.' in text and 'Across lines: before  after.' in text, data
                    assert '<!-- literal -->' in text and '<!-- open' in text, data
                elif note == 'callouts.md':
                    assert 'a · Title with a comment' in text and 'note · Nested first line' in text, data
                    assert '[!b]' in text and '[!tip]' in text and '[!warning]' in text, data
                else:
                    expected = ('List comments\n\nInline\n• item \n• next\n\n'
                        'Asterisk\n• a \n• b\n\nTasks\n☐  task \n☑  done\n\n'
                        'Between items\n• a\n• b\n\nContinuation\n• a\n  b\n\n'
                        'Loose\n• a\n\n\n• b\n')
                    assert text == expected, (text, expected)


if __name__ == '__main__':
    main()
