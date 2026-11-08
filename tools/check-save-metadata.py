#!/usr/bin/env python3
"""P1-04b：真实应用自动保存的元数据、混合换行和磁盘映像卷根验收。

可选 `--provenance-source` 指向一篇只读私人笔记。脚本只在临时根内创建携带
`com.apple.provenance` 的副本，运行真实自动保存三次并逐项核对其它元数据；
来源文件本身不会被写入。未提供来源时明确跳过，不计作通过。示例：

    python3 -B tools/check-save-metadata.py \
      --provenance-source /path/to/private/note.md
"""
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
import uuid

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
        # provenance 由系统管理；证据里只保留存在性，不记录可复制的字节值。
        attributes[name] = ('<system-managed>' if name == 'com.apple.provenance'
                            else ''.join(command('xattr', '-px', name, str(path)).split()).lower())
    stat = path.stat()
    return {
        'birth': stat.st_birthtime, 'mode': stat.st_mode, 'uid': stat.st_uid, 'gid': stat.st_gid,
        'xattrs': attributes, 'acl': command('ls', '-le', str(path)).splitlines()[1:],
        'stat_output': command('stat', '-f', 'inode=%i birth=%B modified=%m mode=%Sp uid=%u gid=%g size=%z', str(path)),
        'ls_output': command('ls', '-le', str(path)).replace(str(path), '<vault>/note.md'),
    }


def prepare_attributes(path):
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


def prepare(path):
    path.write_text('# Metadata fixture\nOriginal content.\n')
    prepare_attributes(path)


def check_metadata(before, after):
    for field in ['birth', 'mode', 'uid', 'gid', 'xattrs', 'acl']:
        assert before[field] == after[field], (field, before[field], after[field])


def check_metadata_except_provenance(before, after):
    for field in ['birth', 'mode', 'uid', 'gid', 'acl']:
        assert before[field] == after[field], (field, before[field], after[field])
    ordinary = [{name: value for name, value in item['xattrs'].items() if name != 'com.apple.provenance'}
                for item in (before, after)]
    assert ordinary[0] == ordinary[1], ordinary
    assert 'com.apple.provenance' in after['xattrs'], '保存后 provenance 丢失'


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


def run_app_holding_open(state, output, name, vault):
    target = output / f'{name}.json'
    inspection = output / f'{name}-app-state.json'
    target.unlink(missing_ok=True)
    inspection.unlink(missing_ok=True)
    allowed = {p.name for p in vault.iterdir()}
    args = [str(APP), f'--vault={vault}', '--note=note.md', '--light',
            f'--edit-inspection={target}', '--edit-mode=idle', '--inspect-hold-open',
            f'--editor-command-inspection={inspection}']
    with subprocess.Popen(args, env=HELPERS.environment(state)) as process:
        try:
            deadline = time.monotonic() + 30
            while not target.exists():
                assert process.poll() is None and time.monotonic() < deadline, f'{name} 超时'
                assert {p.name for p in vault.iterdir()} == allowed, '保存期间笔记文件夹出现临时文件'
                time.sleep(.005)
            while True:
                assert process.poll() is None and time.monotonic() < deadline, f'{name} 状态超时'
                try:
                    record = json.loads(inspection.read_text())
                except (OSError, json.JSONDecodeError):
                    record = {}
                documents = [item for item in record.get('documents', []) if item.get('path') == 'note.md']
                if documents and documents[0].get('dirty') is False and documents[0].get('issue') == 'none':
                    break
                time.sleep(.02)
            assert {p.name for p in vault.iterdir()} == allowed, '保存期间笔记文件夹出现临时文件'
            recovery = state / 'cache' / 'recovery'
            assert not list(recovery.glob('*.json')), '保存成功后残留恢复稿'
            result = json.loads(target.read_text())
            result['app_state'] = documents[0]
            result['no_temporary_files_during_save'] = True
            result['recovery_cleared'] = True
            target.write_text(json.dumps(result, indent=2) + '\n')
            return result, documents[0]
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=6)


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


def provenance_value(path):
    return ''.join(command('xattr', '-px', 'com.apple.provenance', str(path)).split()).lower()


def restore_provenance(value, path):
    # 临时副本的 provenance 会被 cp 按调用进程改写；launchd 作业不带终端来源，
    # 可把来源字节原样写回，且只作用于临时根内的副本。
    label = f'hortus-p1-47-{os.getpid()}-{uuid.uuid4().hex[:8]}'
    subprocess.run(['launchctl', 'submit', '-l', label, '--', '/usr/bin/xattr',
                    '-wx', 'com.apple.provenance', value, str(path)], check=True)
    try:
        deadline = time.monotonic() + 5
        while provenance_value(path) != value:
            assert time.monotonic() < deadline, 'provenance 回写超时'
            time.sleep(.02)
    finally:
        subprocess.run(['launchctl', 'remove', label], check=False)


def check_provenance(base, output, source):
    source = source.resolve()
    if not source.is_file():
        print('跳过 provenance 场景：来源不存在', flush=True)
        return {'skipped': True, 'reason': 'provenance source missing'}
    if 'com.apple.provenance' not in command('xattr', str(source)).splitlines():
        print('跳过 provenance 场景：来源没有 com.apple.provenance', flush=True)
        return {'skipped': True, 'reason': 'provenance attribute missing'}
    source_value = provenance_value(source)
    source_before = snapshot(source)
    runs = []
    for index in range(1, 4):
        name = f'provenance-{index}'
        vault = base / name
        vault.mkdir()
        path = vault / 'note.md'
        # -p 复制扩展属性；来源只读，副本只落在临时根内。
        command('cp', '-p', str(source), str(path))
        prepare_attributes(path)
        # cp 会按调用进程生成新的 provenance；用 launchd 作业把来源字节写回副本，
        # 让保存路径面对真实的来源值而不改动只读来源。
        restore_provenance(source_value, path)
        assert provenance_value(path) == source_value
        original = path.read_bytes()
        before_provenance = provenance_value(path)
        before = snapshot(path)
        expected = '自动保存😀'.encode() + original
        saved, app_state = run_app_holding_open(base / f'{name}-state', output, name, vault)
        after_provenance = provenance_value(path)
        after = snapshot(path)
        (output / f'{name}-metadata.json').write_text(
            json.dumps({'before': before, 'after': after}, ensure_ascii=False, indent=2) + '\n')
        check_metadata_except_provenance(before, after)
        assert path.read_bytes() == expected
        assert app_state.get('dirty') is False, app_state
        assert app_state.get('issue') == 'none', app_state
        assert app_state.get('marked') is False, app_state
        expected_length = len(expected.decode().encode('utf-16-le')) // 2
        assert app_state.get('view', {}).get('length') == expected_length, app_state
        runs.append({
            'run': index, 'saved': bool(saved.get('saved')), 'automatic_save': saved,
            'content_updated': True, 'other_metadata_preserved': True,
            'provenance_restored_from_source': True,
            'provenance_present_after': after_provenance != '',
            'dirty_after_save': app_state.get('dirty'), 'issue_after_save': app_state.get('issue'),
            'view_length_matches_disk': True, 'recovery_cleared': True,
            'provenance_changed': before_provenance != after_provenance,
        })
        print(name, 'provenance save;', len(after['xattrs']), 'xattrs;', saved, flush=True)
    assert snapshot(source) == source_before, 'provenance 来源被修改'
    assert provenance_value(source) == source_value, 'provenance 来源被修改'
    assert any(run['provenance_changed'] for run in runs), 'provenance 未发生系统转换，场景没有覆盖目标路径'
    result = {'skipped': False, 'source_read_only': True, 'runs': runs,
              'passed': len(runs) == 3 and all(run['saved'] for run in runs)}
    (output / 'provenance.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/save-metadata')
    parser.add_argument('--provenance-source', type=Path,
                        help='只读私人笔记；附加三次真实 com.apple.provenance 自动保存检查')
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
        provenance = check_provenance(base, output, args.provenance_source) if args.provenance_source else None
        acceptance = {'metadata': True, 'newline_cases': len(cases),
            'volume_root_saved': True, 'volume_failure_cleaned': True, 'image_detached_and_deleted': True}
        if provenance is None:
            acceptance['provenance'] = 'skipped: --provenance-source 未提供'
        elif provenance['skipped']:
            acceptance['provenance'] = f"skipped: {provenance['reason']}"
        else:
            acceptance['provenance'] = provenance['passed']
            acceptance['provenance_runs'] = len(provenance['runs'])
        (output / 'acceptance.json').write_text(json.dumps(acceptance, indent=2) + '\n')


if __name__ == '__main__':
    main()
