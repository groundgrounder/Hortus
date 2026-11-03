#!/usr/bin/env python3
"""Move a real note tab using CGEvent with isolated vault and session state."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

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
        for name in ['move.md', 'stay.md', 'cycle.md', 'copy.md']:
            (vault / name).write_text(f'# {name}\n\nPublic tab drag fixture.\n')
        env = dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'))
        media = (args.screenshots or output / 'screenshots').resolve()
        media.mkdir(parents=True, exist_ok=True)
        target = output / 'drag.json'
        completed = subprocess.run([str(args.app.resolve()), f'--vault={vault}', '--note=move.md',
                                    '--multi-window-tab-drag', f'--multi-window-inspection={target}', f'--tab-drag-media={media}'],
                                   env=env, capture_output=True, timeout=90)
        (output / 'application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        result = json.loads(target.read_text())
        assert result['real_cgevent_drag'] and result['stage'] == 'complete', result
        assert result['checks'] and all(result['checks'].values()), result
        assert len(result['cycles']) == 20, result
        for key in ['document', 'io', 'session', 'revision', 'reading']:
            assert result['before'][key] == result['after'][key], (key, result)
        session_path = base / 'cache/session.toml'
        saved_text = session_path.read_text()
        saved = tomllib.loads(saved_text)
        (output / 'saved-session.json').write_text(json.dumps(saved, indent=2) + '\n')
        assert len(saved['windows']) == 2, saved
        assert saved['recent'][0]['visible']['left'] is False, saved
        assert [w['visible']['left'] for w in saved['windows']] == [False, True], saved

        def restore(name, close=False, note=None):
            target = output / (name + '.json')
            command = [str(args.app.resolve()), '--multi-window-tab-drag', '--tab-drag-restore',
                       f'--multi-window-inspection={target}', f'--tab-drag-media={media / name}']
            if close:
                command.append('--tab-drag-close-restored')
            if note:
                command.append('--note=' + note)
            completed = subprocess.run(command, env=env, capture_output=True, timeout=45)
            (output / (name + '.log')).write_bytes(completed.stdout + completed.stderr)
            assert completed.returncode == 0, (name, completed.returncode, completed.stderr.decode(errors='replace'))
            data = json.loads(target.read_text())
            assert data['stage'] == 'restored', data
            return data

        restored = restore('restored', close=True, note='move.md')
        assert restored['active_id'] == restored['windows'][1]['id'], restored
        for key in ['id', 'tabs', 'selected', 'views', 'visible', 'frame']:
            assert [w.get(key) for w in result['session']['windows']] == [w.get(key) for w in restored['session']['windows']], (key, restored)
        assert restored['contexts'] == [{'windows': 2, 'listeners': 1}], restored
        assert len(tomllib.loads(session_path.read_text())['windows']) == 1, restored
        after_close = restore('after-close')
        assert len(after_close['windows']) == 1, after_close
        # 同库后续窗口包含重复 move.md/stay.md；首个有效窗口保留，后续诊断并跳过。
        assert saved_text.count('tabs = ["move.md"]') >= 1, saved_text
        session_path.write_text(saved_text.replace('tabs = ["move.md"]', 'tabs = ["move.md", "stay.md"]'))
        damaged = restore('duplicate')
        assert [w['tabs'] for w in damaged['windows']] == [['stay.md'], ['move.md']], damaged
        assert len(damaged['diagnostics']) == 1 and 'stay.md' in damaged['diagnostics'][0], damaged
        assert (vault / 'stay.md').read_text() == '# stay.md\n\nPublic tab drag fixture.\n'
        for pid in result['terminal_pids'] + [c['pid'] for c in result['cycles']]:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(('shell survived exit', pid))
        print(json.dumps({'drag': result, 'restore': restored, 'duplicate': damaged}, indent=2))


if __name__ == '__main__':
    main()
