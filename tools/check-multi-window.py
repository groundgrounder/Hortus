#!/usr/bin/env python3
"""Exercise real vault windows with isolated application state."""
import argparse
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
import time

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


def edges(app, base, output, env, saved):
    session_path = base / 'cache/session.toml'
    layout_path = base / 'config/layout.toml'
    def run(name):
        target = output / (name + '.json')
        completed = subprocess.run([str(app.resolve()), '--multi-window-edge',
                                    f'--multi-window-inspection={target}'], env=env, capture_output=True, timeout=30)
        (output / (name + '.log')).write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')
        return json.loads(target.read_text())
    session_path.write_text(re.sub(r'(?m)^([xy]) = .*$', r'\1 = 999999.0', saved))
    offscreen = run('offscreen')
    assert offscreen['frames_visible'] and offscreen['contexts'] == 2, offscreen
    session_path.write_text(saved.replace(str(base / 'b'), str(base / 'missing')))
    missing = run('missing-vault')
    assert missing['contexts'] == 1 and len(missing['errors']) == 1, missing
    root = json.dumps(str((base / 'a').resolve()))
    legacy = f'last_vault = {root}\n[[recent]]\npath = {root}\ntabs = ["same.md"]\nselected = "same.md"\n[recent.views]\n"same.md" = "editing"\n[[diagrams]]\nvault = {root}\nnote = "same.md"\nblock = 7\ncollapsed = ["public-node"]\n'
    old_layout = layout_path.read_text().replace('[regions.left]', '[regions.left]\nvisible = false')
    old_layout = old_layout.replace('[regions.right]', '[regions.right]\nvisible = true')
    old_layout = old_layout.replace('[regions.bottom]', '[regions.bottom]\nvisible = false')
    layout_path.write_text(old_layout)
    session_path.write_text(legacy)
    migrated = run('legacy-migration')
    assert migrated['contexts'] == 1 and len(migrated['windows']) == 1, migrated
    assert migrated['windows'][0]['views']['same.md'] == 'editing', migrated
    assert migrated['windows'][0]['left_visible'] is False, migrated
    session = tomllib.loads(session_path.read_text())
    assert session['diagrams'][0]['collapsed'] == ['public-node'], session
    assert session['recent'][0]['visible']['left'] is False, session
    assert 'visible' not in layout_path.read_text(), layout_path.read_text()
    (output / 'migrated-session.json').write_text(json.dumps(session, indent=2) + '\n')
    # 后续打开新库继承迁移默认值，旧 layout.visible 不再作为真相。
    assert session['default_visible']['left'] is False, session
    print(json.dumps({'offscreen': offscreen, 'missing': missing, 'migration': migrated}, indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/multi-window')
    parser.add_argument('--app', type=Path, default=ROOT / 'apps/macos/.build/debug/Hortus')
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--language')
    args = parser.parse_args()
    ensure_event_posting_host()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    media = (args.screenshots or output / 'screenshots').resolve()
    media.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-windows-') as directory:
        base = Path(directory)
        for name in ['a', 'b', 'c']:
            (base / name).mkdir()
            (base / name / 'same.md').write_text(f'# Vault {name}\n\nPublic window fixture.\n')
        zdot = base / 'zdotdir'
        zdot.mkdir(parents=True, exist_ok=True)
        (zdot / '.zshrc').write_text("PS1='❯ '\nRPROMPT=''\n")
        (zdot / '.zprofile').write_text("PS1='❯ '\nRPROMPT=''\n")
        env = dict(os.environ, ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
                   HORTUS_CONFIG_DIR=str(base / 'config'), HORTUS_CACHE_DIR=str(base / 'cache'))
        result = output / 'windows.json'
        result.unlink(missing_ok=True)
        command = [str(args.app.resolve())]
        if args.language:
            command += ['-AppleLanguages', f'({args.language})']
        command += [f'--vault={base / "a"}', '--note=same.md',
                    f'--multi-window-second={base / "b"}', f'--multi-window-inspection={result}', f'--multi-window-media={media}']
        completed = subprocess.run(command, env=env, capture_output=True, timeout=60)
        (output / 'application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        data = json.loads(result.read_text())
        assert data['visible_windows'] == 2 and data['actual_vault_windows'] == 2 and data['original_preserved'], data
        for field in ['documents_isolated', 'existing_activated', 'canonical_deduplicated', 'groups_isolated',
                      'size_existing_unchanged', 'visible_isolated', 'quick_open_isolated',
                      'key_window_command', 'background_terminal_preserved', 'external_isolated',
                      'close_cancel', 'close_retry', 'close_discard', 'quit_cancel_all_windows', 'latest_size_preserved',
                      'real_drag_isolated', 'search_ui_isolated', 'dynamic_tabs_isolated']:
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
        saved = (base / 'cache/session.toml').read_text()
        session = tomllib.loads(saved)
        assert len(session['windows']) == 2, session
        (output / 'saved-session.json').write_text(json.dumps(session, indent=2) + '\n')
        restored = output / 'restored.json'
        restore_media = media / 'restored'
        restore_media.mkdir(exist_ok=True)
        restore_command = [str(args.app.resolve())]
        if args.language:
            restore_command += ['-AppleLanguages', f'({args.language})']
        restore_command += ['--multi-window-restore',
                            f'--multi-window-inspection={restored}', f'--multi-window-media={restore_media}']
        completed = subprocess.run(restore_command, env=env, capture_output=True, timeout=60)
        (output / 'restored-application.log').write_bytes(completed.stdout + completed.stderr)
        assert completed.returncode == 0, (completed.returncode, completed.stderr.decode(errors='replace'))
        restore = json.loads(restored.read_text())
        assert restore['visible_windows'] == 2 and restore['other_shell_survived'] and restore['closed_window_hidden'], restore
        assert [w['id'] for w in restore['windows']] == [w['id'] for w in data['windows']], restore
        assert [w['tabs'] for w in restore['windows']] == [w['tabs'] for w in data['windows']], restore
        assert [w['views'] for w in restore['windows']] == [w['views'] for w in data['windows']], restore
        assert [w['left_visible'] for w in restore['windows']] == [w['left_visible'] for w in data['windows']], restore
        assert [w['terminal_count'] for w in restore['windows']] == [1, 1], restore
        assert all(w['agent_count'] == 0 for w in restore['windows']), restore
        assert 'terminal:' not in saved.lower(), saved
        assert [w['frame'] for w in restore['session']['windows']] == [w['frame'] for w in data['session']['windows']], restore
        for window in restore['windows']:
            try:
                os.kill(window['terminal_pid'], 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(('restored shell survived exit', window))
        print(json.dumps({'windows': data, 'restore': restore}, indent=2))
        assert restore['windows'][0]['left_size'] == 340, restore
        edges(args.app, base, output, env, saved)


if __name__ == '__main__':
    main()
