#!/usr/bin/env python3
"""P1-09：真实窗口中的文件管理：新建、跨窗口重命名迁移、移到废纸篓与安全边界。

自检入口按任务书要求 #if DEBUG 编译、不进入 Release 二进制（A4），
因此本检查启动 DEBUG 构建的真实应用可执行文件（与 check-multi-window 同一模式），
在隔离临时知识库与隔离 HORTUS_CONFIG_DIR / HORTUS_CACHE_DIR 下逐项断言；
测试放入系统废纸篓的临时项立即按 resultingItemURL 清理。
"""
import argparse
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus-debug.app/Contents/MacOS/Hortus'
FINAL_DISK = {'Created Note.md', 'renamed.md', 'Case.md', 'a.md', 'b.md', 'link.md', 'linkdir'}
INVALID_NAMES = {'': 'vault.invalid_name', '.hidden': 'vault.invalid_name',
                 'node_modules': 'vault.invalid_name', 'a/b': 'vault.invalid_name',
                 '..': 'vault.invalid_name'}


def build_vault(base):
    vault = base / 'vault'
    vault.mkdir(parents=True)
    (vault / 'alpha.md').write_text('# Alpha\n')
    (vault / 'case.md').write_text('# Case\n')
    (vault / 'a.md').write_text('a\n')
    (vault / 'b.md').write_text('b\n')
    (vault / 'dup.md').write_text('# Dup\n')
    outside = base / 'outside'
    outside.mkdir()
    (outside / 'far.md').write_text('outside\n')
    (outside / 'target.md').write_text('target\n')
    os.symlink(outside / 'far.md', vault / 'link.md')
    os.symlink(outside, vault / 'linkdir')
    return vault


def environment(state):
    zdot = state / 'zdotdir'
    zdot.mkdir(parents=True, exist_ok=True)
    (zdot / '.zshrc').write_text("PS1='❯ '\nRPROMPT=''\n")
    (zdot / '.zprofile').write_text("PS1='❯ '\nRPROMPT=''\n")
    return dict(os.environ, ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'),
                HORTUS_CONFIG_DIR=str(state / 'config'), HORTUS_CACHE_DIR=str(state / 'cache'))


def assert_result(data):
    created = data['create_note']
    assert created['path'] == 'Created Note.md' and created['disk'] and created['saved'] and created['editing_view'], created
    assert created['tree_name'], created
    folder = data['create_folder']
    assert folder['path'] == 'Projects' and all(folder[key] for key in ['disk', 'selected', 'nested_disk']), folder
    selection = data['folder_selection']
    assert selection['folder'] == 'Projects' and selection['selected'] and selection['parent'] == 'Projects', selection
    assert selection['cleared_on_deselect'], selection
    assert selection['note_path'] == 'Projects/picked.md' and selection['disk'], selection
    subfolder = data['create_note_subfolder']
    assert subfolder['path'] == 'Projects/memo.md' and subfolder['disk'] and subfolder['editing_view'], subfolder
    assert data['second_window_attached'], data
    renamed = data['rename_file_cross_window']
    assert renamed['immediate'] and renamed['immediate_name'] and renamed['tree_name'], renamed
    assert renamed['selected'] and 'renamed.md' in renamed['title'], renamed
    assert renamed['dirty_preserved'] and renamed['no_missing_or_conflict'] and renamed['saved_to_new'], renamed
    assert renamed['recovery_before'] == 1 and renamed['recovery_after_rename'] == 1 and renamed['recovery_after_save'] == 0, renamed
    case_only = data['rename_case_only']
    assert case_only['path'] == 'Case.md' and case_only['disk'] and case_only['immediate'], case_only
    assert case_only['immediate_name'] and case_only['tree_name'], case_only
    conflict = data['rename_conflict']
    assert conflict['error_key'] == 'vault.already_exists' and conflict['disk_unchanged'], conflict
    folder_renamed = data['rename_folder_cross_window']
    assert folder_renamed['path'] == 'Papers' and folder_renamed['tab_second'] and folder_renamed['dirty_preserved'], folder_renamed
    assert folder_renamed['saved_to_new'] is True and folder_renamed['no_missing'], folder_renamed
    assert folder_renamed['quick_open_hit'] and folder_renamed['entries_subtree'], folder_renamed
    assert folder_renamed['children_names'] and folder_renamed['tree_name'], folder_renamed
    assert folder_renamed['first_window_tab'], folder_renamed
    assert data['invalid_names'] == INVALID_NAMES, data['invalid_names']
    symlink = data['symlink_rejects']
    assert symlink['create'] == 'vault.symlink_unsupported' and symlink['rename'] == 'vault.symlink_unsupported', symlink
    assert symlink['trash'] == 'vault.symlink_unsupported' and symlink['outside'] == 'vault.invalid_path', symlink
    cancel = data['trash_cancel_unsaved']
    assert cancel['prompts'] == 1 and cancel['tab_intact'] and cancel['file_intact'] and cancel['unsaved_intact'], cancel
    file_trash = data['trash_file_confirm']
    assert file_trash['disk_removed'] and file_trash['tab_closed'] and file_trash['entry_removed'], file_trash
    assert file_trash['quick_open_gone'] and file_trash['resulting_exists'] and file_trash['recovery_clean'], file_trash
    keyboard = data['keyboard_trash_folder']
    assert keyboard['performed'] and keyboard['disk_removed'], keyboard
    assert keyboard['first_window_tab_closed'] and keyboard['cross_window_tab_closed'], keyboard
    assert keyboard['entry_removed'] and keyboard['resulting_exists'], keyboard


def clean_trash(data):
    """立即清理本测试放入系统废纸篓的临时项，不污染用户真实废纸篓。"""
    trash_root = Path.home() / '.Trash'
    trashed = []
    for key in ['trash_file_confirm', 'keyboard_trash_folder']:
        section = data.get(key) or {}
        url = section.get('resulting') or ''
        if not url:
            continue
        path = Path(url)
        assert path.exists(), f'trashed item missing: {url}'
        assert path.parent == trash_root, f'trashed item outside the Trash: {url}'
        trashed.append(path)
    for path in trashed:
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink()
    for path in trashed:
        assert not path.exists(), f'trash cleanup failed: {path}'


def run_case(index, language, appearance, media, app, output):
    base = Path(tempfile.mkdtemp(prefix=f'hortus-file-ops-{index}-'))
    state = base / 'state'
    state.mkdir()
    vault = build_vault(base)
    target = state / 'file-ops.json'
    arguments = [str(app), '-AppleLanguages', f'({language})',
                 f'--{appearance}', f'--vault={vault}',
                 f'--file-ops-inspection={target}', f'--file-ops-media={media}']
    log = open(output / f'run-{language}-{appearance}.log', 'wb')
    process = subprocess.Popen(arguments, env=environment(state),
                               stdout=log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 180
    while process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.05)
    log.close()
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=6)
    assert process.returncode == 0, f'file-ops inspection failed: see {output / f"run-{language}-{appearance}.log"}'
    assert target.exists(), 'file-ops inspection produced no result'
    data = json.loads(target.read_text())
    (output / f'run-{language}-{appearance}.json').write_text(json.dumps(
        data, ensure_ascii=False, indent=2) + '\n')
    try:
        assert_result(data)
        # 库内零残留临时/隐藏文件：磁盘终态与预期集合一致，重命名与废纸篓按预期生效。
        # 不跟随符号链接目录：终态集合按库内实际条目比较。
        actual = set()
        for dirpath, dirnames, filenames in os.walk(vault, followlinks=False):
            for name in dirnames + filenames:
                actual.add(os.path.relpath(os.path.join(dirpath, name), vault))
        assert actual == FINAL_DISK, f'unexpected vault residue: {actual ^ FINAL_DISK}'
        # 证据归档：检查输出含全部断言字段与命令日志。
        for shot in ['screenshot_window', 'screenshot_naming']:
            assert data.get(shot) and Path(data[shot]).exists(), f'{shot} missing'
        print(f'{language}-{appearance}', json.dumps({key: data[key] for key in
              ['create_note', 'folder_selection', 'trash_file_confirm', 'keyboard_trash_folder']},
              ensure_ascii=False, indent=2), flush=True)
    finally:
        # 无论断言是否通过都清理本轮放入系统废纸篓的临时项。
        clean_trash(data)
        shutil.rmtree(base, ignore_errors=True)
    return data

def ensure_app(app):
    """组装包含 #if DEBUG 自检入口的调试 .app：资源与本地化取自 Release 包，
    可执行文件换成 DEBUG 构建，重签名后供检查真实启动。"""
    debug_binary = ROOT / 'apps/macos/.build/debug/Hortus'
    release_bundle = ROOT / 'build/Hortus.app'
    assert release_bundle.exists(), '先运行 ./tools/build-macos.sh 生成 Release 应用包'
    static_library = ROOT / 'target/release/libhortus_ffi.a'
    if not debug_binary.exists() or debug_binary.stat().st_mtime < static_library.stat().st_mtime:
        # Rust 内核变更后必须重链调试可执行文件，否则旧静态库会留在二进制里。
        command = ['swift', 'build', '--package-path', str(ROOT / 'apps/macos'), '-c', 'debug',
                   '-Xlinker', str(static_library),
                   '-Xlinker', '-framework', '-Xlinker', 'CoreFoundation',
                   '-Xlinker', '-framework', '-Xlinker', 'CoreServices',
                   '-Xlinker', '-framework', '-Xlinker', 'Security', '-Xlinker', '-liconv']
        completed = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')
    bundle = ROOT / 'build/Hortus-debug.app'
    executable = bundle / 'Contents/MacOS/Hortus'
    stale = not executable.exists() or executable.stat().st_mtime < debug_binary.stat().st_mtime \
        or (bundle / 'Contents/Info.plist').stat().st_mtime < (release_bundle / 'Contents/Info.plist').stat().st_mtime
    if stale:
        shutil.rmtree(bundle, ignore_errors=True)
        shutil.copytree(release_bundle, bundle)
        shutil.copyfile(debug_binary, executable)
        completed = subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)],
                                   check=False, capture_output=True)
        assert completed.returncode == 0, completed.stderr.decode(errors='replace')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/file-ops')
    parser.add_argument('--media', type=Path,
                        default=ROOT / 'target/reports/file-ops/screenshots')
    parser.add_argument('--app', type=Path, default=APP)
    args = parser.parse_args()
    app = args.app.resolve()
    ensure_app(app)
    output = args.output.resolve()
    media = args.media.resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    summary = {}
    for index, (language, appearance) in enumerate([
            ('en', 'light'), ('en', 'dark'), ('zh-Hans', 'light'), ('zh-Hans', 'dark')]):
        data = run_case(index, language, appearance, media, app, output)
        summary[f'{language}-{appearance}'] = data
        # 公开媒体截图按语言/外观命名，双语浅深色各一张主窗口与命名界面。
        for shot in ['screenshot_window', 'screenshot_naming']:
            source = Path(data[shot])
            target = media / f'file-ops-{language}-{appearance}-{source.name.split("-", 2)[2]}'
            shutil.copyfile(source, target)
    (output / 'file-ops.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'runs': list(summary), 'media': str(media)}, indent=2))


if __name__ == '__main__':
    main()
