#!/usr/bin/env python3
"""P1-04：真实 release 窗口中的保存时机、监听、退出恢复、按键性能及编辑后空闲 CPU。"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
SPEC = spec_from_file_location('toolbar_check', ROOT / 'tools/check-tabs-in-toolbar.py')
HELPERS = module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def run(state, output, name, vault, mode):
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    allowed = {str(p.relative_to(vault)) for p in vault.rglob('*')}
    arguments = [str(APP), f'--vault={vault}', '--note=note.md', '--light',
                 f'--edit-inspection={target}', f'--edit-mode={mode}']
    with subprocess.Popen(arguments, env=HELPERS.environment(state)) as process:
        try:
            deadline = time.monotonic() + 30
            while not target.exists():
                assert process.poll() is None and time.monotonic() < deadline, f'{name} 自检失败'
                assert {str(p.relative_to(vault)) for p in vault.rglob('*')} == allowed, '保存期间笔记文件夹出现临时文件'
                time.sleep(.005)
            ready = time.monotonic()
            assert process.wait(timeout=6) == 0
            result = json.loads(target.read_text())
            result['exit_seconds_after_result'] = time.monotonic() - ready
            result['no_temporary_files_during_save'] = True
            assert {str(p.relative_to(vault)) for p in vault.rglob('*')} == allowed
            target.write_text(json.dumps(result, indent=2) + '\n')
            print(name, json.dumps(result, indent=2), flush=True)
            return result
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=6)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/edit-and-save')
    parser.add_argument('--skip-cpu', action='store_true')
    parser.add_argument('--cpu-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-edit-check-') as temporary:
        base = Path(temporary)
        vault = base / 'vault'
        vault.mkdir()
        source = (ROOT / 'fixtures/vault-sample/欢迎.md').read_text()
        (vault / 'note.md').write_text(source)
        (vault / 'other.md').write_text('# Other\n')
        alternate = base / 'alternate'
        alternate.mkdir()
        (alternate / 'note.md').write_text('# Alternate\n')
        if not args.cpu_only:
            lifecycle = run(base / 'lifecycle', output, 'lifecycle', vault, 'lifecycle')
            assert .9 <= lifecycle['autosave_delay_seconds'] <= 1.5, lifecycle
            assert all(lifecycle[key] for key in ['command_save', 'focus_save', 'switch_save', 'library_save', 'close_save',
                                                  'selection_preserved', 'undo_cleared', 'dirty_before_exit']), lifecycle
            assert lifecycle['external_delay_seconds'] <= 1, lifecycle
            assert (vault / 'note.md').read_text().startswith('退出保存'), '退出时没有保存'
            failed = run(base / 'recovery', output, 'shutdown-failure', vault, 'shutdown-failure')
            assert 2.7 <= failed['exit_seconds_after_result'] <= 3.7, failed
            recovery = list((base / 'recovery/cache/recovery').glob('*.json'))
            assert len(recovery) == 1
            assert json.loads(recovery[0].read_text())['text'].startswith('退出恢复😀')
            (vault / 'note.md').chmod(0o644)
            restored = run(base / 'recovery', output, 'restart-recovery', vault, 'recovery')
            assert restored['recovery_prompt'] and restored['recovered'] and restored['saved'], restored
            assert not list((base / 'recovery/cache/recovery').glob('*.json'))
            block = '\n```mermaid\nmindmap\n  Root\n    One\n    Two\n    Three\n```\n'
            chunk = 'Ordinary Markdown source text for incremental editing.\n' * 3300
            large = '\n'.join(chunk + block for _ in range(6))
            (vault / 'note.md').write_text(large)
            performance = run(base / 'performance', output, 'performance', vault, 'performance')
            assert performance['utf8_bytes'] >= 1024 * 1024 and performance['blocks'] == 6, performance
            assert performance['key_p95_ms'] <= 8, performance
            assert performance['core_matches_native'] and performance['saved'], performance
        if not args.skip_cpu:
            empty = HELPERS.idle_cpu(base / 'cpu-empty', [])
            print('未打开库的 30 s CPU 采样完成', flush=True)
            (vault / 'note.md').write_text(source)
            ready = output / 'idle-ready.json'
            ready.unlink(missing_ok=True)
            opened = HELPERS.idle_cpu(base / 'cpu-opened', [f'--vault={vault}', '--note=note.md',
                f'--edit-inspection={ready}', '--edit-mode=idle', '--inspect-hold-open'])
            assert json.loads(ready.read_text())['saved']
            difference = opened['average_cpu_percent'] - empty['average_cpu_percent']
            result = {'empty': empty, 'edited_and_saved': opened, 'difference_percentage_points': difference}
            (output / 'idle-cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            assert abs(difference) <= .1, result


if __name__ == '__main__':
    main()
