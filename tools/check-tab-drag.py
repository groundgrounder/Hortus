#!/usr/bin/env python3
"""Move a real note tab using CGEvent with isolated vault and session state."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/tab-drag')
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-tab-drag-') as directory:
        base = Path(directory)
        vault = base / 'vault'
        vault.mkdir()
        for name in ['move.md', 'stay.md']:
            (vault / name).write_text(f'# {name}\n\nPublic tab drag fixture.\n')
        env = dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'))
        target = output / 'drag.json'
        completed = subprocess.run([str(args.app.resolve()), f'--vault={vault}', '--note=move.md',
                                    '--multi-window-tab-drag', f'--multi-window-inspection={target}'],
                                   env=env, capture_output=True, timeout=90)
        (output / 'application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        result = json.loads(target.read_text())
        assert result['real_cgevent_drag'] and result['window_count'] == 2, result
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
