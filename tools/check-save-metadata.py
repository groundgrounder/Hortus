#!/usr/bin/env python3
"""P1-04b：真实应用自动保存的元数据、混合换行和磁盘映像卷根验收。"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
import json
import os
from pathlib import Path
import plistlib
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


def command(*args):
    return subprocess.check_output(args, text=True)


def snapshot(path):
    attributes = {}
    for name in command('xattr', str(path)).splitlines():
        attributes[name] = ''.join(command('xattr', '-px', name, str(path)).split()).lower()
    stat = path.stat()
    return {
        'birth': stat.st_birthtime, 'mode': stat.st_mode, 'uid': stat.st_uid, 'gid': stat.st_gid,
        'xattrs': attributes, 'acl': command('ls', '-le', str(path)).splitlines()[1:],
        'stat_output': command('stat', '-f', 'inode=%i birth=%B modified=%m mode=%Sp uid=%u gid=%g size=%z', str(path)),
        'xattr_output': command('xattr', '-lx', str(path)),
        'ls_output': command('ls', '-le', str(path)).replace(str(path), '<vault>/note.md'),
    }


def prepare(path):
    path.write_text('# Metadata fixture\nOriginal content.\n')
    attrs = {
        'com.apple.metadata:_kMDItemUserTags': plistlib.dumps(['Hortus P1-04b\n6'], fmt=plistlib.FMT_BINARY),
        'com.apple.metadata:kMDItemFinderComment': plistlib.dumps('P1-04b Finder comment', fmt=plistlib.FMT_BINARY),
        'com.apple.metadata:kMDItemWhereFroms': plistlib.dumps(['file:///hortus-fixture'], fmt=plistlib.FMT_BINARY),
        'user.hortus-p1-04b': bytes.fromhex('000102ffabcdef'),
        'com.apple.ResourceFork': b'Hortus resource fork',
        'com.apple.quarantine': b'0081;00000000;Hortus;',
    }
    for name, data in attrs.items():
        command('xattr', '-wx', name, data.hex(), str(path))
    command('SetFile', '-d', '01/02/2000 03:04:05', '-m', '01/02/2000 03:04:05', str(path))
    command('chmod', '640', str(path))
    command('chmod', '+a', 'everyone allow read,readattr,readextattr,readsecurity', str(path))


def check_metadata(before, after):
    for field in ['birth', 'mode', 'uid', 'gid', 'xattrs', 'acl']:
        assert before[field] == after[field], (field, before[field], after[field])


def run_app(state, output, name, vault, volume=False, failure=False):
    target = output / f'{name}.json'
    target.unlink(missing_ok=True)
    allowed = {p.name for p in vault.iterdir()}
    seen_system_files = set()
    system = vault / '.TemporaryItems' / f'folders.{os.geteuid()}' / 'TemporaryItems'

    def check_tree():
        if not volume:
            assert {p.name for p in vault.iterdir()} == allowed, '普通库出现暂存文件'
            return
        # 卷根例外经用户明确允许；其他系统目录由挂载服务管理，不计作应用暂存。
        assert {p.name for p in vault.iterdir()} <= allowed | {'.TemporaryItems', '.fseventsd', '.Spotlight-V100'}, '卷根出现额外文件'
        if system.exists():
            for item in system.iterdir():
                assert item.name.startswith('NSIRD_Hortus_'), item.name
                seen_system_files.update(str(p.relative_to(vault)) for p in item.rglob('*') if p.is_file())

    args = [str(APP), f'--vault={vault}', '--note=note.md', '--light',
            f'--edit-inspection={target}', f'--edit-mode={"metadata-failure" if failure else "idle"}']
    with subprocess.Popen(args, env=HELPERS.environment(state)) as process:
        try:
            deadline = time.monotonic() + 30
            while process.poll() is None:
                check_tree()
                assert time.monotonic() < deadline, f'{name} 超时'
                time.sleep(.005)
            assert process.returncode == 0 and target.exists(), f'{name} 自检失败'
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=6)
    check_tree()
    assert not system.exists() or not list(system.iterdir()), '退出后系统替换目录没有清理'
    result = json.loads(target.read_text())
    expected = result.get('save_failed') and result.get('dirty') if failure else result.get('saved')
    assert expected, result
    result.update({'sample_interval_ms': 5, 'temporary_files_only_in_system_directory': volume,
                   'no_remaining_temporary_files': True, 'observed_system_staged_files': len(seen_system_files)})
    target.write_text(json.dumps(result, indent=2) + '\n')
    return result


def check_one(base, output, vault, name, volume=False):
    path = vault / 'note.md'
    prepare(path)
    original = path.read_bytes()
    before = snapshot(path)
    saved = run_app(base / name, output, name, vault, volume)
    after = snapshot(path)
    check_metadata(before, after)
    assert path.read_bytes() == '自动保存😀'.encode() + original
    result = {'before': before, 'after': after, 'automatic_save': saved, 'content_updated': True}
    if volume:
        command('chmod', '+a', 'everyone deny delete', str(path))
        try:
            before_failure = snapshot(path)
            content = path.read_bytes()
            failed = run_app(base / f'{name}-failure', output, f'{name}-failure', vault, True, True)
            check_metadata(before_failure, snapshot(path))
            assert path.read_bytes() == content
            recovery = list((base / f'{name}-failure/cache/recovery').glob('*.json'))
            assert len(recovery) == 1
            assert json.loads(recovery[0].read_text())['text'].startswith('替换应失败')
            result['failed_replacement'] = failed
        finally:
            command('chmod', '-N', str(path))
    (output / f'{name}-evidence.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    print(name, 'metadata preserved;', len(before['xattrs']), 'xattrs;', saved, flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/save-metadata')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-metadata-') as temporary:
        base = Path(temporary)
        vault = base / 'vault'
        vault.mkdir()
        check_one(base, output, vault, 'metadata')
        cases = [('lf', 'a\nb\nc\n', False), ('crlf', 'a\r\nb\r\nc\r\n', True),
                 ('lf-majority', 'a\nb\r\nc\n', False), ('crlf-majority', 'a\r\nb\nc\r\n', True),
                 ('tie-lf-first', 'a\nb\r\n', False), ('tie-crlf-first', 'a\r\nb\n', True)]
        for name, source, crlf in cases:
            case = base / name
            case.mkdir()
            (case / 'note.md').write_bytes(source.encode())
            run_app(base / f'{name}-state', output, name, case)
            canonical = '自动保存😀' + source.replace('\r\n', '\n')
            expected = canonical.replace('\n', '\r\n') if crlf else canonical
            assert (case / 'note.md').read_bytes() == expected.encode(), name
        image = base / 'volume.dmg'
        mount = base / 'volume'
        command('hdiutil', 'create', '-size', '64m', '-fs', 'APFS', '-volname', 'HortusP104b', str(image))
        command('hdiutil', 'attach', str(image), '-nobrowse', '-owners', 'on', '-mountpoint', str(mount))
        try:
            assert mount.stat().st_dev != mount.parent.stat().st_dev
            check_one(base, output, mount, 'volume-root', volume=True)
        finally:
            command('hdiutil', 'detach', str(mount))
        image.unlink()
        (output / 'acceptance.json').write_text(json.dumps({'metadata': True, 'newline_cases': len(cases),
            'volume_root_saved': True, 'volume_failure_cleaned': True, 'image_detached_and_deleted': True}, indent=2) + '\n')


if __name__ == '__main__':
    main()
