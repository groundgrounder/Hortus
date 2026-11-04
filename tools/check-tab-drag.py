#!/usr/bin/env python3
"""Move a real note tab using CGEvent with isolated vault and session state."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TRUSTED_HOST_ENV = 'HORTUS_CHECK_ACCESSIBILITY_HOST'
RELOCATED_TIMEOUT = 300


def event_posting_trusted():
    """Whether this host may post CGEvents (macOS Accessibility permission)."""
    if sys.platform != 'darwin':
        return True
    try:
        import ctypes
        library = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
        library.CGPreflightPostEventAccess.restype = ctypes.c_bool
        return bool(library.CGPreflightPostEventAccess())
    except OSError:
        return False


def ensure_event_posting_host():
    """Re-run the identical check under Terminal when this host lacks Accessibility.

    The inspection app drives its own controls with CGEvents at the HID event
    tap. macOS attributes that permission to the responsible application, so a
    check started from an untrusted host silently loses every synthetic click
    and drag. Re-executing the same command under Terminal keeps the real
    CGEvents and every assertion; only the responsible host changes.
    """
    if os.environ.get(TRUSTED_HOST_ENV) == '1' or event_posting_trusted():
        return
    if sys.platform != 'darwin':
        raise SystemExit('Accessibility event posting is unavailable')
    base = Path(tempfile.mkdtemp(prefix='hortus-trusted-host-'))
    out, err, status, command = base / 'stdout', base / 'stderr', base / 'status', base / 'run.command'
    invocation = ' '.join(shlex.quote(part) for part in
                          [sys.executable, '-B', str(Path(__file__).resolve()), *sys.argv[1:]])
    command.write_text(f'''#!/bin/zsh
cd {shlex.quote(str(ROOT))}
export {TRUSTED_HOST_ENV}=1
{invocation} > {shlex.quote(str(out))} 2> {shlex.quote(str(err))}
print -r -- $? > {shlex.quote(str(status))}
''')
    command.chmod(0o700)
    print('Accessibility event posting is not granted to this host; '
          're-running the same check under Terminal.', file=sys.stderr)
    try:
        try:
            subprocess.run(['open', '-a', 'Terminal', str(command)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except (OSError, subprocess.CalledProcessError) as error:
            raise SystemExit(f'Cannot start Terminal for the Accessibility check: {error}') from error
        deadline = time.monotonic() + RELOCATED_TIMEOUT
        while time.monotonic() < deadline and not status.exists():
            time.sleep(0.5)
        if not status.exists():
            raise SystemExit('Timed out waiting for the Terminal-hosted check')
        sys.stdout.write(out.read_text() if out.exists() else '')
        sys.stderr.write(err.read_text() if err.exists() else '')
        sys.stdout.flush()
        sys.stderr.flush()
        raise SystemExit(int(status.read_text().strip() or '1'))
    finally:
        shutil.rmtree(base, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/tab-drag')
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    ensure_event_posting_host()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-tab-drag-') as directory:
        base = Path(directory)
        vault = base / 'vault'
        vault.mkdir()
        for name in ['move.md', 'stay.md', 'cycle.md', 'copy.md']:
            diagram = ''
            if name == 'move.md':
                diagram = '\n```mermaid\nflowchart TB\n  A[Moved] --> B[Diagram]\n```\n'
            (vault / name).write_text(f'# {name}\n\nPublic tab drag fixture.\n{diagram}')
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
        before_diagram = result['diagram_before_drag']
        assert len(before_diagram) == 1, before_diagram
        before_diagram = before_diagram[0]
        assert before_diagram['nodes'] == 2, before_diagram
        assert before_diagram['reason'] == '' and before_diagram['signature'], before_diagram
        moved = next(window for window in result['windows'] if 'move.md' in window['tabs'])
        assert len(moved['diagrams']) == 1, moved
        moved_diagram = moved['diagrams'][0]
        assert moved_diagram['nodes'] == 2 and moved_diagram['reason'] == '', moved_diagram
        assert moved_diagram['signature'], moved_diagram
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
        restored_move = next(window for window in restored['windows'] if 'move.md' in window['tabs'])
        assert len(restored_move['diagrams']) == 1, restored_move
        restored_diagram = restored_move['diagrams'][0]
        assert restored_diagram['nodes'] == 2 and restored_diagram['reason'] == '', restored_diagram
        assert restored_diagram['signature'], restored_diagram
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
