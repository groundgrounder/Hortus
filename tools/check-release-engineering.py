#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""检查发布脚本参数和公开演示库，不接触签名证书或私人笔记。"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/release-engineering')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    package = ROOT / 'tools/package-dmg.sh'
    cases = [([], 0),
             (['--developer-id', 'Developer ID Application: Example (TEAMID)',
               '--notary-profile', 'hortus-release'], 0),
             (['--developer-id'], 2),
             (['--developer-id', 'Example'], 2),
             (['--developer-id', 'Developer ID Application: Example (TEAMID)'], 2),
             (['--notary-profile', 'hortus-release'], 2),
             (['--output', 'bad.zip'], 2),
             (['--app', 'bad'], 2),
             (['--unknown'], 2)]
    checks = []
    for arguments, expected in cases:
        result = subprocess.run([str(package), *arguments, '--validate-only'],
                                text=True, capture_output=True, cwd=ROOT, check=False)
        assert result.returncode == expected, (arguments, result.stdout, result.stderr)
        checks.append({'arguments': arguments, 'exit_code': result.returncode})
    vault = ROOT / 'fixtures/demo-vault'
    notes = sorted(vault.rglob('*.md'))
    names = {note.stem for note in notes}
    sources = {note: note.read_bytes() for note in vault.rglob('*') if note.is_file()}
    diagrams = []
    for note in notes:
        text = note.read_text()
        assert 'CC0' in text, note
        for target in re.findall(r'!?\[\[([^]|#]+)(?:[^]]*)\]\]', text):
            assert target in names, (note, target)
        for block, _ in enumerate(re.findall(r'^```mermaid\n(.*?)^```', text, re.M | re.S), 1):
            for width in (700, 1100):
                stem = f'{note.stem}-{block}-{width}'
                svg, metrics = output / f'{stem}.svg', output / f'{stem}.json'
                result = subprocess.run([str(ROOT / 'target/release/hortus'), 'mermaid', 'render',
                                         str(note), '--block', str(block), '--width', str(width),
                                         '--out', str(svg), '--metrics', str(metrics)],
                                        check=True, text=True, capture_output=True)
                data = json.loads(metrics.read_text())
                assert not data['horizontal_overflow'] and data['node_overlaps'] == 0, data
                if note.stem == 'Learning map':
                    assert result.stdout.startswith('81 '), result.stdout
                diagrams.append({'note': note.relative_to(vault).as_posix(), 'block': block,
                                 'width': width, 'summary': result.stdout.strip(), **data})
    assert len(notes) >= 7 and len(diagrams) >= 8
    assert all(path.read_bytes() == before for path, before in sources.items())
    result = {'package_arguments': checks, 'notes': len(notes), 'diagrams': diagrams,
              'demo_sha256': {path.relative_to(vault).as_posix(): hashlib.sha256(data).hexdigest()
                              for path, data in sources.items()}}
    (output / 'release-engineering.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f'PASS: {len(checks)} package argument cases; {len(notes)} CC0 notes; '
          f'{len(diagrams)} diagram renders without overflow or overlap')


if __name__ == '__main__':
    main()
