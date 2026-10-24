#!/usr/bin/env python3
"""真实窗口中的表格、图片、编辑投影与可见区域首绘检查。"""
import argparse
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = spec_from_file_location('reading', ROOT / 'tools/check-reading-view.py')
READING = module_from_spec(SPEC)
SPEC.loader.exec_module(READING)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/media-view')
    parser.add_argument('--cpu-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-media-') as temporary:
        base = Path(temporary)
        if args.cpu_only:
            empty = READING.HELPERS.idle_cpu(base / 'empty', [])
            print('空库 CPU 采样完成', flush=True)
            opened = READING.HELPERS.idle_cpu(base / 'opened',
                [f'--vault={ROOT / "fixtures/vault-sample"}', '--note=表格与图片.md'])
            result = {'empty': empty, 'reading_media': opened,
                      'difference_percentage_points': opened['average_cpu_percent'] - empty['average_cpu_percent']}
            (output / 'media-idle-cpu.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            assert abs(result['difference_percentage_points']) <= .1
            return
        vault = base / 'vault'
        vault.mkdir()
        shutil.copytree(ROOT / 'fixtures/vault-sample/media', vault / 'media')
        chinese = '| 项目 | 说明 | 数量 |\n| :--- | :---: | ---: |\n| 阅读 | **每天一页**，写下一点想法 | 12 |\n| 整理 | *轻松开始*，再慢慢完善 | 8 |\n| 记录 | `中文代码` 和转义 A\\|B | 24 |\n'
        wide = '| ' + ' | '.join(f'Column {i}' for i in range(1, 11)) + ' |\n'
        wide += '|' + '---|' * 10 + '\n'
        wide += '| ' + ' | '.join('A long description wraps within its column' for _ in range(10)) + ' |\n'
        sources = {
            'wide': '# Wide table\n\n' + wide,
            'chinese': '# 中文表格\n\n' + chinese,
            'image': '# 本地图片\n\n![山与日光](media/landscape.png)\n\n![[media/landscape.svg]]\n',
            'remote': '# 远程图片\n\n![示例链接](https://example.invalid/remote.png)\n',
            'editing': '# 编辑表格\n\n' + chinese + '\n光标离开表格时显示预览。\n',
        }
        for name, source in sources.items():
            (vault / f'{name}.md').write_text(source)
        for appearance in ['light', 'dark']:
            for name in sources:
                target = f'{name}-{appearance}'
                READING.run(base / target, output, target, vault, f'{name}.md', editing=name == 'editing',
                    extra=[f'--{appearance}', f'--snapshot={output / (target + ".png")}'])
            target = f'editing-source-{appearance}'
            READING.run(base / target, output, target, vault, 'editing.md', editing=True,
                extra=[f'--{appearance}', '--media-source', f'--snapshot={output / (target + ".png")}'])
        parts = ['# Images and tables\n\n']
        for i in range(20):
            shutil.copyfile(vault / 'media/landscape.png', vault / 'media' / f'image-{i}.png')
            parts.append(f'![Image {i}](media/image-{i}.png)\n\n')
            if i % 4 == 0:
                parts.append(chinese + '\n')
        (vault / 'performance.md').write_text(''.join(parts))
        perf = READING.run(base / 'performance', output, 'performance', vault, 'performance.md', mode='media')
        assert perf['media'] == 25 and perf['tables'] == 5, perf
        assert perf['visible_images'] > 0 and perf['main_thread_decodes'] == 0, perf
        assert 0 < perf['visible_painted_ms'] <= 150, perf
        preview = READING.run(base / 'quick-look', output, 'quick-look', vault, 'image.md', mode='media',
            extra=['--media-preview'])
        assert preview['quick_look_visible'], preview
        # 对进程和网络条件取交集，避免 lsof 默认 OR 将其他进程的连接算进来。
        network_base = base / 'network'
        config = network_base / 'config'
        config.mkdir(parents=True)
        (config / 'config.toml').write_text('font_size = 15.0\ndefault_view = "reading"\n')
        ready = output / 'network-ready.json'
        ready.unlink(missing_ok=True)
        # 工具栏自检提供一个短暂保持窗口的就绪信号。
        command = [str(READING.HELPERS.APP), f'--vault={vault}', '--note=remote.md',
            f'--toolbar-inspection={ready}', '--inspect-hold']
        with subprocess.Popen(command, env=READING.HELPERS.environment(network_base)) as process:
            try:
                READING.HELPERS.wait_for(ready, process)
                network = subprocess.run(['lsof', '-nP', '-a', '-p', str(process.pid), '-i'], capture_output=True, text=True)
                (output / 'network.txt').write_text(f'pid={process.pid}\nexit={network.returncode}\n' + network.stdout + network.stderr)
                assert network.returncode == 1 and not network.stdout.strip(), network.stdout
                assert process.wait(timeout=20) == 0
            finally:
                if process.poll() is None:
                    process.terminate(); process.wait(timeout=10)
        (output / 'summary.json').write_text(json.dumps({'performance': perf, 'network_connections': 0}, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    main()
