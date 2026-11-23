#!/usr/bin/env python3
"""Verify native note embeds and folder-rename link updates in the real app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / 'build/Hortus.app'
DEBUG = ROOT / 'apps/macos/.build/debug/Hortus'
CASES = (
    ('en', 'light', 'en'),
    ('zh-Hans', 'light', 'zh-light'),
    ('zh-Hans', 'dark', 'zh-dark'),
)
SHOTS = (
    'embeds-en-full.png',
    'folder-rename-preview-light-en.png',
    'embeds-zh-light-full.png',
    'embeds-zh-dark-mmd.png',
    'embeds-zh-dark-boundary.png',
    'folder-rename-zh-dark-after.png',
)
GALLERY = 'mindmap\n  root((Root))\n    [[Target#Section]]\n      Leaf\n'
FIXTURE = {
    'seams.md': '# Seams\n\n![[gallery/gallery.mmd]]\n\n![[rich]]\n',
    'gallery/gallery.mmd': GALLERY,
    'gallery/nested.md': '# Nested\n\n```mermaid\n' + GALLERY + '```\n\n'
                         '| Name | Value |\n| --- | --- |\n| A sample | Another sample |\n',
    'gallery/Target.md': '# Target\n\n## Section\n\nLinked body.\n',
    'other/Target.md': '# Other target\n',
    'host.md': (
        '# Host\n\n'
        '![[child]]\n\n'
        '![[child#Section Two|Section alias]]\n\n'
        '![[arch.mmd]]\n\n'
        '![[missing]]\n\n![[Same]]\n\n'
        '![[child#No Such Heading]]\n\n'
        '![[cycle-a]]\n\n'
        '![[deep-1]]\n\n![[rich]]\n'),
    'child.md': (
        '---\naliases: [Junior]\ntags: [embed]\n---\n'
        '# Child\n\n'
        'Intro visible body.\n\n'
        '## Section Two\n\n'
        'Section two body.\n\n'
        '## Section Three\n\n'
        'Later body.\n'),
    'arch.mmd': 'flowchart LR\n  A[Start] --> B[End]\n',
    'boundary.md': '# Boundary\n\n![[missing]]\n\n![[Same]]\n\n![[child#No Such Heading]]\n\n![[cycle-a]]\n\n![[deep-1]]\n',
    'mmd-first.md': '# Diagram\n\n![[arch.mmd]]\n',
    'cycle-a.md': '![[cycle-b]]\n',
    'cycle-b.md': '![[cycle-a]]\n',
    'deep-1.md': '![[deep-2]]\n',
    'deep-2.md': '![[deep-3]]\n',
    'deep-3.md': '![[deep-4]]\n',
    'deep-4.md': '# End\n',
    'docs/intro.md': '[[docs/sub/api]]\n',
    'docs/sub/api.md': '[[docs/intro]]\n',
    'docs/arch.mmd': 'flowchart LR\n  C[Old] --> D[Path]\n',
    'docs-extra/keep.md': '# Keep\n',
    'outside.md': (
        '[[docs/intro]] [[docs/sub/api#Auth|API]]\n'
        '![[docs/arch.mmd]] [[intro]] [[docs-extra/keep]]\n'),
    'dirty.md': '[[docs/intro]]\n',
    'unrelated.md': '[[docs-extra/keep]]\n',
    'x/Same.md': '# Same X\n',
    'y/Same.md': '# Same Y\n',
    'rich.md': '# Rich\n\n| Key | Value |\n|---|---|\n| One | Two |\n\nFormula $x^2$.\n\n```mermaid\nflowchart LR\n  R[Rich] --> S[Body]\n```\n\n![[gallery/nested]]\n',
}
EXPECTED_OUTSIDE = (
    '[[papers/intro]] [[papers/sub/api#Auth|API]]\n'
    '![[papers/arch.mmd]] [[intro]] [[docs-extra/keep]]\n')


def prepare_app(directory):
    if not DEBUG.is_file():
        subprocess.run(['./tools/test-macos.sh'], cwd=ROOT, check=True)
    if not (RELEASE / 'Contents/MacOS/Hortus').is_file():
        subprocess.run(['./tools/build-macos.sh'], cwd=ROOT, check=True)
    bundle = directory / 'Hortus.app'
    shutil.copytree(RELEASE, bundle)
    app = bundle / 'Contents/MacOS/Hortus'
    shutil.copy2(DEBUG, app)
    subprocess.run(['codesign', '--force', '--sign', '-', bundle], check=True, capture_output=True)
    return app


def hashes(vault):
    return {
        str(path.relative_to(vault)): 'directory' if path.is_dir()
        else hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(vault.rglob('*'))
    }


def write_json(path, result):
    path.write_text(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True) + '\n', encoding='utf-8')


def verify(result, before, mode):
    assert result['read_only'] and result['title_click'] and result['section_click'], result
    assert result['mmd_click'], result
    assert result['live_refresh'], result
    for depth, geometry in result['geometry'].items():
        assert geometry['diagrams'] and geometry['media'], (depth, geometry)
        assert all(0 < edge <= geometry['width'] + 0.01
                   for edge in geometry['diagrams'] + geometry['media']), (depth, geometry)
    for key, path in [('mmd_interaction', 'gallery/gallery.mmd'),
                      ('nested_interaction', 'gallery/nested.md')]:
        interaction = result[key]
        assert interaction['target'] == 'gallery/Target.md' and interaction['source'] == path, interaction
        assert interaction['collapsed'] and not interaction['autoenables_items'], interaction
        assert interaction['svg_bytes'] > 0, interaction
    cards = {card['title']: card for card in result['cards']}
    assert cards['child']['kind'] == 'note' and cards['child']['path'] == 'child.md', cards
    assert cards['Section alias › Section Two']['kind'] == 'note', cards
    assert cards['arch']['kind'] == 'diagram', cards
    assert any(card['kind'] == 'broken' for card in result['cards']), cards
    assert any(card['kind'] == 'heading_missing' for card in result['cards']), cards
    assert any(card['kind'] == 'cycle' for card in result['cards']), cards
    assert any(card['kind'] == 'ambiguous' for card in result['cards']), cards
    assert result['full_body_length'] > 0 and result['section_body_length'] > 0, result
    rename = result['folder_rename']
    assert rename['dirty_preserved'] and rename['resolved_after'] == 'papers/arch.mmd', rename
    assert 'outside.md' in rename['preview'] and '[[docs/' in rename['preview'], rename
    assert result['before_read_only'] == before, 'read-only phase touched the vault'


def verify_disk(vault, before):
    assert (vault / 'outside.md').read_text(encoding='utf-8') == EXPECTED_OUTSIDE
    assert (vault / 'docs-extra/keep.md').read_text(encoding='utf-8') == FIXTURE['docs-extra/keep.md']
    assert (vault / 'unrelated.md').read_text(encoding='utf-8') == FIXTURE['unrelated.md']
    assert (vault / 'dirty.md').read_text(encoding='utf-8') == FIXTURE['dirty.md']
    assert not (vault / 'docs').exists()
    assert (vault / 'papers/intro.md').read_text(encoding='utf-8') == '[[papers/sub/api]]\n'
    assert (vault / 'papers/sub/api.md').read_text(encoding='utf-8') == '[[papers/intro]]\n'
    assert (vault / 'papers/arch.mmd').read_text(encoding='utf-8') == FIXTURE['docs/arch.mmd']
    assert 'Fresh body marker.' in (vault / 'child.md').read_text(encoding='utf-8')
    assert 'Fresh' in (vault / 'arch.mmd').read_text(encoding='utf-8')
    after = hashes(vault)
    assert before['docs-extra/keep.md'] == after['docs-extra/keep.md']
    assert before['dirty.md'] == after['dirty.md']
    assert before['unrelated.md'] == after['unrelated.md']


def run_case(app, directory, output, media, language, appearance, mode):
    state = directory / mode
    vault = state / 'vault'
    vault.mkdir(parents=True)
    config = state / 'config'
    config.mkdir()
    for path, text in FIXTURE.items():
        target = vault / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding='utf-8')
    before = hashes(vault)
    report = state / 'result.json'
    env = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(state / 'cache'))
    log = output / f'embeds-{mode}.log'
    with log.open('wb') as stream:
        completed = subprocess.run(
            [app, '-AppleLanguages', f'({language})', f'--{appearance}', f'--vault={vault}',
             f'--embeds-inspection={report}', f'--embeds-media={media}', f'--embeds-shot={mode}'],
            env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=180,
        )
    assert completed.returncode == 0 and report.exists(), f'native run failed: {log}'
    result = json.loads(report.read_text(encoding='utf-8'))
    verify(result, before, mode)
    verify_disk(vault, before)
    result['after_process_exit'] = hashes(vault)
    write_json(output / f'embeds-{mode}.json', result)
    return {'language': language, 'appearance': appearance, 'files': len(before), 'read_only': True,
            'renamed': True, 'cards': len(result['cards'])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/embeds')
    parser.add_argument('--media', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    media = args.media.resolve() if args.media else output / 'screenshots'
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-embeds-') as temporary:
        directory = Path(temporary)
        app = prepare_app(directory)
        cases = [run_case(app, directory, output, media, *case) for case in CASES]
    assert all((media / name).read_bytes().startswith(b'\x89PNG\r\n\x1a\n') for name in SHOTS)
    result = {'cases': cases, 'screenshots': list(SHOTS), 'passed': True}
    write_json(output / 'embeds.json', result)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
