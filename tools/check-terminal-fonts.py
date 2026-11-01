#!/usr/bin/env python3
"""Installed Nerd Font fallback: real PTY fixture, renderer font/ink evidence.
Run tools/test-macos.sh first to build the Debug executable; --app can select
an independently packaged Debug .app when localized window evidence is needed.
"""
import argparse
from collections import Counter
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('terminal_helpers', ROOT / 'tools/check-terminal.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)
CODES = (ROOT / 'fixtures/terminal/nerd-font-codepoints.txt').read_text().split()


def inspect(app, output, media, mode, theme):
    name = f'{mode}-{theme}'
    result_path = output / f'{name}.json'
    result_path.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-terminal-font-') as directory:
        base = Path(directory)
        vault = base / 'vault'
        vault.mkdir()
        (vault / 'Terminal.md').write_text('# Terminal font fixture\n')
        env = HELPERS.environment(base / 'state')
        command = [str(app), f'--{theme}', f'--vault={vault}', '--note=Terminal.md',
                   f'--terminal-font-inspection={result_path}', f'--terminal-nerd-fonts={mode}',
                   f'--terminal-font-fixture={ROOT / "fixtures/terminal/nerd-fonts.zsh"}',
                   f'--terminal-font-marker={base / "done"}',
                   f'--terminal-font-snapshot={media / (name + ".png")}']
        with subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            try:
                HELPERS.HELPERS.wait_for(result_path, process, seconds=90)
                _, stderr = process.communicate(timeout=30)
                assert process.returncode == 0, stderr.decode(errors='replace')
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=15)
        return json.loads(result_path.read_text())


def validate(data, mode):
    assert not data['enumeration_on_main'], data
    assert data['cached_across_size_and_panel_move'], 'font catalog/provider changed during panel move'
    assert data['font_ready_before_shell'], data
    assert data['cell_before'] == data['cell_after'], data
    assert 'AppleSystemUIFontMonospaced' in data['base_font'], data['base_font']
    cells = data['cells']
    icons = [cell for cell in cells if cell['codepoint'] in CODES]
    counts = Counter((cell['codepoint'], cell['bold'], cell['italic']) for cell in icons)
    assert len(icons) == 552 and len(counts) == 552, (len(icons), len(counts))
    assert set(counts.values()) == {1}, counts
    width, height = data['cell_after']
    if mode != 'none':
        for cell in icons:
            assert cell['glyph'] > 0 and cell['font'] != 'LastResort' and cell['provider'], cell
            assert any(name in cell['font'] for name in ['NerdFont', 'NF-', 'NFM-']), cell
            x, y, w, h = cell['fitted_ink']
            # Half a physical pixel at 2x plus floating point rounding.
            assert x >= -.251 and y >= -.251 and x + w <= width + .251 and y + h <= height + .251, cell
    else:
        assert data['families'] == [], data
        assert all(not cell['provider'] for cell in icons)
    apple = [cell for cell in cells if cell['codepoint'] == 'F8FF']
    assert apple and all('AppleSystemUIFontMonospaced' in cell['font'] and not cell['provider'] for cell in apple), apple
    powerline = [cell for cell in cells if cell['renderer'] == 'PowerlineRenderer']
    assert sorted(cell['codepoint'] for cell in powerline) == ['E0B0', 'E0B2', 'E0B4', 'E0B6'], powerline
    if mode == 'meslo-first':
        newer = [cell for cell in icons if cell['codepoint'] in ['E6B2', 'E8EB']]
        assert len(newer) == 8 and all('JetBrainsMono' in cell['font'] for cell in newer), newer
    return {'icon_cells': len(icons), 'styles': 4, 'fonts': dict(Counter(cell['font'] for cell in icons)),
            'cell_pt': [width, height], 'enumeration_ms': data['enumeration_ms'], 'families': data['families']}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/terminal-fonts')
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--mode', choices=['all', 'installed', 'none', 'meslo-first'], default='all')
    args = parser.parse_args()
    output = args.output.resolve()
    media = (args.screenshots or output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    summary = {}
    control_cells = {}
    for mode in ['installed', 'none', 'meslo-first'] if args.mode == 'all' else [args.mode]:
        for theme in ['light', 'dark']:
            data = inspect(args.app.resolve(), output, media, mode, theme)
            summary[f'{mode}-{theme}'] = validate(data, mode)
            control_cells[(mode, theme)] = [cell for cell in data['cells'] if cell['codepoint'] not in CODES]
            print(mode, theme, summary[f'{mode}-{theme}'], flush=True)
    if args.mode == 'all':
        for theme in ['light', 'dark']:
            assert control_cells[('installed', theme)] == control_cells[('none', theme)], 'ASCII/CJK/emoji/tab or Powerline changed'
            summary[f'controls-unchanged-{theme}'] = len(control_cells[('installed', theme)])
    (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    main()
