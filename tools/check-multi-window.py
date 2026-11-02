#!/usr/bin/env python3
"""Exercise real vault windows with isolated application state."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/multi-window')
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-windows-') as directory:
        base = Path(directory)
        for name in ['a', 'b']:
            (base / name).mkdir()
            (base / name / 'same.md').write_text(f'# Vault {name}\n\nPublic window fixture.\n')
        env = dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'))
        result = output / 'windows.json'
        result.unlink(missing_ok=True)
        command = [str(args.app.resolve()), f'--vault={base / "a"}', '--note=same.md',
                   f'--multi-window-second={base / "b"}', f'--multi-window-inspection={result}']
        completed = subprocess.run(command, env=env, capture_output=True, timeout=60)
        (output / 'application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')
        data = json.loads(result.read_text())
        assert data['visible_windows'] == 2 and data['original_preserved'], data
        print(json.dumps(data, indent=2))


if __name__ == '__main__':
    main()
