#!/usr/bin/env python3
"""Exercise real vault windows with isolated application state."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/multi-window')
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.screenshots or output / 'screenshots').resolve()
    media.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-windows-') as directory:
        base = Path(directory)
        for name in ['a', 'b', 'c']:
            (base / name).mkdir()
            (base / name / 'same.md').write_text(f'# Vault {name}\n\nPublic window fixture.\n')
        env = dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'))
        result = output / 'windows.json'
        result.unlink(missing_ok=True)
        command = [str(args.app.resolve()), f'--vault={base / "a"}', '--note=same.md',
                   f'--multi-window-second={base / "b"}', f'--multi-window-inspection={result}', f'--multi-window-media={media}']
        completed = subprocess.run(command, env=env, capture_output=True, timeout=60)
        (output / 'application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        data = json.loads(result.read_text())
        assert data['visible_windows'] == 2 and data['original_preserved'], data
        for field in ['documents_isolated', 'existing_activated', 'canonical_deduplicated', 'groups_shared',
                      'size_existing_unchanged', 'visible_isolated', 'quick_open_isolated',
                      'key_window_command', 'background_terminal_preserved', 'external_isolated']:
            assert data[field], (field, data)
        assert data['new_size'] == 340, data
        assert data['external_update_ms'] < 1000 and data['external_rename_delete_ms'] < 1000, data
        assert len(set(data['terminal_pids'])) == 2 and min(data['terminal_pids']) > 0, data
        for context in data['contexts']:
            assert all(context[key] == 1 for key in ['handles', 'listeners', 'trees', 'indexes', 'windows']), context
        for pid in data['terminal_pids']:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(('shell survived application exit', pid))
        session = tomllib.loads((base / 'cache/session.toml').read_text())
        assert len(session['windows']) == 2, session
        (output / 'saved-session.json').write_text(json.dumps(session, indent=2) + '\n')
        restored = output / 'restored.json'
        restore_media = media / 'restored'
        restore_media.mkdir(exist_ok=True)
        completed = subprocess.run([str(args.app.resolve()), '--multi-window-restore',
                                    f'--multi-window-inspection={restored}', f'--multi-window-media={restore_media}'],
                                   env=env, capture_output=True, timeout=60)
        (output / 'restored-application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        restore = json.loads(restored.read_text())
        assert restore['visible_windows'] == 2 and restore['other_shell_survived'] and restore['closed_window_hidden'], restore
        assert [w['id'] for w in restore['windows']] == [w['id'] for w in data['windows']], restore
        assert [w['tabs'] for w in restore['windows']] == [w['tabs'] for w in data['windows']], restore
        assert [w['views'] for w in restore['windows']] == [w['views'] for w in data['windows']], restore
        assert [w['left_visible'] for w in restore['windows']] == [w['left_visible'] for w in data['windows']], restore
        for window in restore['windows']:
            try:
                os.kill(window['terminal_pid'], 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(('restored shell survived exit', window))
        print(json.dumps({'windows': data, 'restore': restore}, indent=2))


if __name__ == '__main__':
    main()
