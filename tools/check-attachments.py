#!/usr/bin/env python3
"""库内附件后备查找：真实窗口首绘、截图、30 秒 CPU 和只读库检查。"""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('reading', ROOT / 'tools/check-reading-view.py')
READING = module_from_spec(SPEC)
SPEC.loader.exec_module(READING)
SEARCH_SPEC = spec_from_file_location('search', ROOT / 'tools/check-search.py')
SEARCH = module_from_spec(SEARCH_SPEC)
SEARCH_SPEC.loader.exec_module(SEARCH)


def fingerprint(vault):
    return {path.relative_to(vault).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in vault.rglob('*') if path.is_file()}


def idle(base, output, vault=None):
    base.mkdir(parents=True, exist_ok=True)
    config = base / 'config'
    config.mkdir()
    (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "reading"\n')
    command = [str(READING.HELPERS.APP)]
    ready = output / 'idle-ready.json'
    if vault:
        ready.unlink(missing_ok=True)
        command += [f'--vault={vault}', '--note=attachments.md', '--reading-wait-for-scan',
                    f'--reading-inspection={ready}', '--reading-check=media', '--inspect-hold-open']
    with subprocess.Popen(command, env=READING.HELPERS.environment(base)) as process:
        try:
            if vault:
                READING.HELPERS.wait_for(ready, process, seconds=120)
                evidence = json.loads(ready.read_text())
                assert evidence['attachment_index_builds'] == 1, evidence
                assert evidence['visible_images'] > 0 and all(not x for x in evidence['media_reasons']), evidence
            time.sleep(5)
            start = time.monotonic()
            before = READING.HELPERS.cpu_time(process.pid)
            time.sleep(30)
            after = READING.HELPERS.cpu_time(process.pid)
            elapsed = time.monotonic() - start
            return {'wall_seconds': elapsed, 'cpu_delta_seconds': (after - before) / 1e9,
                    'average_cpu_percent': (after - before) / 1e9 / elapsed * 100}
        finally:
            process.terminate()
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/attachments')
    parser.add_argument('--skip-cpu', action='store_true')
    parser.add_argument('--cpu-only', action='store_true')
    parser.add_argument('--screenshots-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-attachments-') as temporary:
        base = Path(temporary)
        if not args.cpu_only:
            sample = ROOT / 'fixtures/vault-sample'
            before = fingerprint(sample)
            for appearance in ['light', 'dark']:
                name = f'attachments-{appearance}'
                result = READING.run(base / name, output, name, sample, 'Obsidian 附件.md', mode='media',
                    extra=[f'--{appearance}', '-AppleLanguages', '(zh-Hans)',
                           f'--snapshot={output / (name + ".png")}'])
                assert result['media_reasons'] == ['', '', 'embed.note', 'embed.file'], result
            assert fingerprint(sample) == before, '公开示例库发生了写入'
            if args.screenshots_only:
                return
        vault = base / 'vault'
        vault.mkdir()
        SEARCH.make_large_vault(vault)
        for folder in range(200):
            target = vault / f'folder{folder:03d}' / 'assets'
            target.mkdir()
            for image in range(10):
                shutil.copyfile(ROOT / 'fixtures/vault-sample/media/landscape.png',
                                target / f'Attachment {folder:03d}-{image:02d}.png')
        parts = ['# Attachments\n\n']
        for index in range(20):
            name = f'Attachment {index:03d}-00.png'
            parts.append(f'![[{name}]]\n\n' if index % 2 == 0 else f'![]({name.replace(" ", "%20")})\n\n')
        (vault / 'attachments.md').write_text(''.join(parts))
        before = fingerprint(vault)
        if not args.cpu_only:
            result = READING.run(base / 'performance', output, 'performance', vault, 'attachments.md', mode='media')
            assert result['media'] == 20 and result['visible_images'] > 0, result
            assert all(not reason for reason in result['media_reasons']), result
            assert result['main_thread_decodes'] == 0, result
            assert result['attachment_index_builds'] == 1, result
            assert result['attachment_index_files'] == 12001, result
            assert 0 < result['visible_painted_ms'] <= 150, result
        if not args.skip_cpu:
            empty = idle(base / 'idle-empty', output)
            print('空库 CPU 采样完成', flush=True)
            opened = idle(base / 'idle-opened', output, vault)
            cpu = {'empty': empty, 'opened': opened,
                   'difference_percentage_points': opened['average_cpu_percent'] - empty['average_cpu_percent']}
            (output / 'idle-cpu.json').write_text(json.dumps(cpu, indent=2) + '\n')
            print(json.dumps(cpu, indent=2), flush=True)
            assert abs(cpu['difference_percentage_points']) <= .1, cpu
        assert fingerprint(vault) == before, '测试库发生了写入'
        (output / 'vault-unchanged.json').write_text(json.dumps({'files': len(before), 'unchanged': True}, indent=2) + '\n')


if __name__ == '__main__':
    main()
