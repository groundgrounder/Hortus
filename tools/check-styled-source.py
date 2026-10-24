#!/usr/bin/env python3
"""P1-05：真实 release 窗口的三种外观、1 MiB 样式延迟及按键性能。"""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('edit_check', ROOT / 'tools/check-edit-and-save.py')
EDIT = module_from_spec(SPEC)
SPEC.loader.exec_module(EDIT)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/styled-source')
    parser.add_argument('--screenshots-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = ROOT / 'fixtures/vault-sample'
    before = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in source.rglob('*') if p.is_file()}
    with tempfile.TemporaryDirectory(prefix='hortus-styling-') as temporary:
        base = Path(temporary)
        for name, appearance, extra in [('light', '--light', []), ('dark', '--dark', []),
                ('contrast-simulated', '--simulate-high-contrast', []), ('light-end', '--light', ['--snapshot-end']),
                ('dark-end', '--dark', ['--snapshot-end'])]:
            subprocess.run([str(EDIT.APP), f'--vault={source}', '--note=样式化源码.md', appearance, *extra,
                f'--snapshot={output / (name + ".png")}'], env=EDIT.HELPERS.environment(base / name), check=True, timeout=30)
        assert all(hashlib.sha256(p.read_bytes()).hexdigest() == value for p, value in before.items())
        if args.screenshots_only:
            return
        vault = base / 'vault'
        vault.mkdir()
        block = '\n```mermaid\nmindmap\n  Root\n    One\n    Two\n    Three\n```\n'
        ordinary = 'Ordinary Markdown source text for incremental editing.\n' * 3300
        rich = '# Heading\nText **bold** _italic_ ==highlight== and `code`.\n- [ ] Task\n> Quote\n\n'
        samples = {'plain': '\n'.join(ordinary + block for _ in range(6)),
                   'rich': rich * (1024 * 1024 // len(rich) + 1) + block}
        results = []
        for name, text in samples.items():
            (vault / 'note.md').write_text(text)
            result = EDIT.run(base / name, output, f'performance-{name}', vault, 'styled-performance')
            results.append(result)
        for result in results:
            assert result['utf8_bytes'] >= 1024 * 1024
            assert result['core_matches_native'] and result['saved']
            assert result['key_p95_ms'] <= 8, result
            assert result['open_styles_ms'] <= 100, result
            assert result['style_update_max_ms'] <= 50, result
            assert max(result['style_patch_characters']) < 4096, result
        print(json.dumps({'public_vault_unchanged': True, 'screenshots': 5, 'system_contrast_verified': False}, indent=2))


if __name__ == '__main__':
    main()
