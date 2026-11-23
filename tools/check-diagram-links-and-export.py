#!/usr/bin/env python3
"""P4-01：图内双链跳转与 SVG/PNG/PDF 原生导出的真实窗口检查。"""
import argparse
import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / 'build/Hortus-debug.app/Contents/MacOS/Hortus'
OUTPUT = ROOT / 'target/reports/diagram-links-and-export'

NOTE = 'diagram-links.md'
NOTE_TEXT = '''# Diagram links

```mermaid
flowchart TB
  Link["[[design#Section Two|Design Spec]]"] --> Note["[[architecture]]"]
  Embed["![[architecture]]"]
```

```mermaid
mindmap
  root(([[architecture]]))
    [[design#Section Two|Design Spec]]
      Child
```

```mermaid
stateDiagram-v2
  state "[[architecture]]" as Linked
  [*] --> Linked
```
'''
DESIGN = '# Design\n\n## Section Two\n\nLinked body.\n'
ARCHITECTURE = '# Architecture\n\nTarget body.\n'


def environment(base):
    return dict(os.environ, HORTUS_CONFIG_DIR=str(base / 'config'),
                HORTUS_CACHE_DIR=str(base / 'cache'))


def ensure_app():
    """#if DEBUG 自检入口只存在调试可执行文件；包资源与本地化沿用 Release 包。"""
    debug_binary = ROOT / 'apps/macos/.build/debug/Hortus'
    release_bundle = ROOT / 'build/Hortus.app'
    assert release_bundle.exists(), '先运行 ./tools/build-macos.sh 生成 Release 应用包'
    static_library = ROOT / 'target/release/libhortus_ffi.a'
    command = ['swift', 'build', '--package-path', str(ROOT / 'apps/macos'), '-c', 'debug',
               '-Xswiftc', '-warnings-as-errors',
               '-Xlinker', str(static_library),
               '-Xlinker', '-framework', '-Xlinker', 'CoreFoundation',
               '-Xlinker', '-framework', '-Xlinker', 'CoreServices',
               '-Xlinker', '-framework', '-Xlinker', 'Security',
               '-Xlinker', '-liconv']
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


def prepare_state(base):
    (base / 'config').mkdir(parents=True, exist_ok=True)
    (base / 'cache').mkdir(parents=True, exist_ok=True)
    (base / 'config' / 'config.toml').write_text('font_size = 15.0\n')


def prepare_vault(base):
    vault = base / 'vault'
    if vault.exists():
        shutil.rmtree(vault)
    vault.mkdir(parents=True)
    (vault / NOTE).write_text(NOTE_TEXT)
    (vault / 'design.md').write_text(DESIGN)
    (vault / 'architecture.md').write_text(ARCHITECTURE)
    (vault / 'reading-export.mmd').write_text('flowchart LR\nA[Origin] --> B[Destination]\n')
    (vault / 'plain.md').write_text('# Plain note\n\nNo diagrams here.\n')
    return vault


def inventory(root):
    return {str(path.relative_to(root)): (path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest())
            for path in sorted(root.rglob('*')) if path.is_file()}


def wait_for(path, process, seconds=180):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists():
            try:
                return json.loads(path.read_text())
            except json.JSONDecodeError:
                pass
        if process.poll() is not None and not path.exists():
            raise AssertionError(f'自检进程提前退出：{process.returncode}')
        time.sleep(0.02)
    raise AssertionError(f'自检未生成 {path.name}')


def launch(base, vault, note, inspection, extra=(), snapshot=None, dark=False, language=None,
           timeout=240):
    inspection.unlink(missing_ok=True)
    arguments = [str(APP), f'--vault={vault}']
    if note:
        arguments.append(f'--note={note}')
    arguments.append('--dark' if dark else '--light')
    arguments.append(f'--diagram-focus-inspection={inspection}')
    if snapshot:
        arguments.append(f'--snapshot={snapshot}')
    if language:
        arguments += ['-AppleLanguages', f'({language})']
    arguments += list(extra)
    process = subprocess.Popen(arguments, env=environment(base),
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=20)
        raise
    data = wait_for(inspection, process, timeout)
    if process.returncode not in (0, None):
        raise AssertionError(f'自检退出码 {process.returncode}：{process.stderr.read()[-800:]}')
    return data


def check_svg(path, labels=('Design Spec', 'architecture')):
    svg = path.read_text(encoding='utf-8')
    root = ET.fromstring(svg)
    assert root.tag.endswith('svg'), root.tag
    assert 'viewBox' in root.attrib, root.attrib
    assert all(label in svg for label in labels)
    assert '<script' not in svg
    return {'bytes': len(svg.encode()), 'viewBox': root.attrib['viewBox']}


def check_png(path, expected_width, expected_height):
    data = path.read_bytes()
    assert data[:8] == b'\x89PNG\r\n\x1a\n', data[:8]
    width = int.from_bytes(data[16:20], 'big')
    height = int.from_bytes(data[20:24], 'big')
    assert width == expected_width, (width, expected_width)
    assert height == expected_height, (height, expected_height)
    return {'width': width, 'height': height, 'bytes': len(data)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=OUTPUT)
    parser.add_argument('--media', type=Path, default=None)
    args = parser.parse_args()
    output = args.output.expanduser().resolve()
    media = (args.media or output / 'media').expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    export_dir = media / 'export'
    export_dir.mkdir(parents=True, exist_ok=True)
    for stale in export_dir.iterdir():
        if stale.is_file():
            stale.unlink()
    screenshots = output / 'screenshots'
    screenshots.mkdir(parents=True, exist_ok=True)

    strings = json.loads((ROOT / 'apps/macos/Localizable.xcstrings').read_text())['strings']
    for key in ('Export Diagram', 'Export', 'Open Linked Note', 'Export Failed',
                'The diagram could not be rendered or written.'):
        localizations = strings[key]['localizations']
        assert localizations['en']['stringUnit']['value'] == key
        assert localizations['zh-Hans']['stringUnit']['value'].strip()

    base = Path(tempfile.mkdtemp(prefix='hortus-p4-01-'))
    prepare_state(base)
    vault = prepare_vault(base)
    before = inventory(vault)
    ensure_app()

    print('== 图内双链与三格式导出 ==', flush=True)
    data = launch(base, vault, NOTE, output / 'links-export.json',
                  [f'--diagram-links-export={export_dir}'], language='en')
    scene = data['link_scene']
    labels = scene['labels']
    assert 'Design Spec' in labels, labels
    assert 'architecture' in labels, labels
    assert '![[architecture]]' in labels, labels
    assert 'design' in scene['targets'] and 'architecture' in scene['targets'], scene
    assert 'Section Two' in scene['headings'], scene
    assert scene['design_label'] == 'Design Spec'
    assert data['reading_click']['heading_matched'], data['reading_click']
    assert data['reading_click']['target'].endswith('design.md'), data['reading_click']
    assert data['collapse_click']['collapsed'], data['collapse_click']
    assert data['collapse_click']['menu_target'].endswith('architecture.md'), data['collapse_click']
    assert 'Open Linked Note' in data['collapse_click']['menu_titles'], data['collapse_click']
    assert data['command_click']['matches'], data['command_click']
    assert data['command_click']['left_reading'], data['command_click']

    export = data['export']
    assert export['export_enabled'], export
    assert export['export_menu'] == ['SVG…', 'PNG…', 'PDF…'], export
    svg = check_svg(export_dir / 'diagram-links-1.svg')
    reading_exports = data['reading_exports']
    assert set(reading_exports) == {NOTE, 'reading-export.mmd'}, reading_exports
    for note, label in [(NOTE, 'Design Spec'), ('reading-export.mmd', 'Destination')]:
        path = export_dir / f'{note}.svg'
        check_svg(path, labels=(label,))
        assert path.stat().st_size == reading_exports[note], path
    png = check_png(export_dir / 'diagram-links-1.png',
                    math.ceil(scene['width'] * 2), math.ceil(scene['height'] * 2))
    assert export['png_pixels'] == [png['width'], png['height']], (export, png)
    assert export['png_opaque_samples'] > 0, export
    assert export['png_distinct_colors'] > 1, export
    pdf = (export_dir / 'diagram-links-1.pdf').read_bytes()
    assert pdf.startswith(b'%PDF-'), pdf[:8]
    assert export['pdf_pages'] == 1, export
    assert not export['pdf_has_image'], export
    assert abs(export['pdf_box'][0] - scene['width']) < 0.01, export
    assert abs(export['pdf_box'][1] - scene['height']) < 0.01, export
    assert len(pdf) > 1000, len(pdf)

    focus = data['focus']
    assert focus['export_present'] and focus['export_enabled'], focus
    assert focus['export_titles'] == ['SVG…', 'PNG…', 'PDF…'], focus
    assert 'Open Linked Note' in focus['menu_titles'], focus
    assert focus['exited'], focus
    assert focus['target'].endswith('design.md'), focus
    assert focus['anchor_matched'], focus

    after = inventory(vault)
    assert before == after, '笔记库目录被改动'
    shutil.copyfile(export_dir / 'diagram-links-1.png', media / 'diagram-links-1.png')

    print('== 公开浅深色截图 ==', flush=True)
    shots = {}
    for name, dark, language in [
        ('links-reading-light-en', False, 'en'),
        ('links-reading-dark-zh', True, 'zh-Hans'),
        ('focus-export-light-en', False, 'en'),
        ('focus-export-dark-zh', True, 'zh-Hans'),
    ]:
        target = media / f'{name}.png'
        target.unlink(missing_ok=True)
        shot = 'focus' if name.startswith('focus') else 'reading'
        result = launch(base, vault, NOTE, screenshots / f'{name}.json',
                        [f'--diagram-links-shot={shot}'], snapshot=target,
                        dark=dark, language=language)
        assert target.exists() and target.stat().st_size > 10_000, target
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        shots[name] = {'path': str(target), 'sha256': digest, 'bytes': target.stat().st_size,
                       'result': result}
        print(f'截图 {name}: {target} sha256={digest[:16]}', flush=True)

    summary = {
        'link_scene': scene,
        'reading_click': data['reading_click'],
        'collapse_click': data['collapse_click'],
        'command_click': data['command_click'],
        'export': {**export, 'svg_check': svg, 'png_check': png, 'pdf_bytes': len(pdf)},
        'focus': focus,
        'reading_exports': reading_exports,
        'vault_unchanged': before == after,
        'media': {name: {'path': item['path'], 'sha256': item['sha256'], 'bytes': item['bytes']}
                  for name, item in shots.items()},
    }
    (output / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'vault_unchanged': summary['vault_unchanged'],
                      'export': export['export_menu'],
                      'shots': list(shots)}, ensure_ascii=False, indent=2))
    print(f'完成：{output}')


if __name__ == '__main__':
    main()
