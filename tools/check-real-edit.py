#!/usr/bin/env python3
"""P1-38：组合中输入法焦点离开编辑器后，保存和快捷键必须仍然可用。

检查只驱动真实点击与真实 ⌘E（应用内 CGEvent），组合由输入法入口建立；
断言全部落在用户可观察结果：是否结束组合、是否完成保存、磁盘是否更新、⌘E 是否生效。
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus.app/Contents/MacOS/Hortus'
NOTE_NAME = 'note.md'
NOTE_TEXT = '# 示例笔记\n\n公开的真实编辑回归夹具。\n'
SESSION = '''last_vault = "{vault}"

[[recent]]
path = "{vault}"
tabs = ["{note}"]
selected = "{note}"

[recent.views]
"{note}" = "editing"

[recent.visible]
bottom = true
left = true

[[windows]]
id = "legacy"
path = "{vault}"
tabs = ["{note}"]
selected = "{note}"

[windows.views]
"{note}" = "editing"

[windows.visible]
bottom = true
left = true

[default_visible]
bottom = true
left = true
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/real-edit')
    parser.add_argument('--app', type=Path, default=APP)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-real-edit-') as directory:
        base = Path(directory)
        vault = base / 'vault'
        vault.mkdir()
        (vault / NOTE_NAME).write_text(NOTE_TEXT, encoding='utf-8')
        config = base / 'config'
        config.mkdir()
        (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "editing"\n', encoding='utf-8')
        cache = base / 'cache'
        cache.mkdir()
        (cache / 'session.toml').write_text(SESSION.format(vault=vault, note=NOTE_NAME), encoding='utf-8')
        target = output / 'composition-focus-loss.json'
        target.unlink(missing_ok=True)
        environment = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(cache))
        with subprocess.Popen([str(args.app.resolve()), '--light',
                               f'--edit-inspection={target}', '--edit-mode=composition-focus-loss'],
                              env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
            try:
                deadline = time.monotonic() + 60
                while not target.exists():
                    assert process.poll() is None and time.monotonic() < deadline, '组合焦点回归未写出结果'
                    time.sleep(0.02)
                stdout, _ = process.communicate(timeout=20)
            finally:
                if process.poll() is None:
                    process.terminate()
                    stdout, _ = process.communicate(timeout=10)
        (output / 'application.log').write_bytes(stdout or b'')
        assert process.returncode == 0, (process.returncode, (output / 'application.log').read_text())
        result = json.loads(target.read_text())
        print(json.dumps(result, indent=2, ensure_ascii=False), flush=True)
        assert result.get('post_event_access'), '需要辅助功能权限才能发送真实点击和 ⌘E'
        assert result.get('terminal_found') and result.get('terminal_visible'), '底部终端不可见，无法让焦点离开编辑器'
        assert result.get('focused'), '真实点击没有把焦点交给编辑器'
        assert result.get('composing') and result.get('marked_during_composition'), '输入法入口没有建立组合状态'
        # 组合进行中仍然不允许保存和切换视图；修复不能靠放开这道保护通过。
        assert result.get('save_blocked_while_composing'), '组合进行中保存未被阻止'
        assert result.get('toggle_blocked_while_composing'), '组合进行中视图切换未被阻止'
        assert result.get('focus_left_editor'), '真实点击没有让焦点离开编辑器'
        # 用户可观察结果：组合结束、保存完成、磁盘更新、⌘E 生效。
        assert result.get('marked_cleared'), '焦点离开后组合状态没有结束'
        assert result.get('save_completed'), '焦点离开后自动保存没有完成'
        assert result.get('disk_updated'), '磁盘没有写入组合后的正文'
        assert result.get('disk_matches_editor'), '磁盘正文与编辑器不一致'
        assert result.get('toggle_worked'), '⌘E 没有切换到阅读视图'
        print('真实编辑回归：通过', flush=True)


if __name__ == '__main__':
    main()
