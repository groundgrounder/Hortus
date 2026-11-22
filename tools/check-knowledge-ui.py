#!/usr/bin/env python3
"""Drive the native knowledge panels against an isolated engineering vault."""
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
    'backlinks-en-light.png',
    'backlinks-and-wikilink-light-zh.png',
    'tags-panel-dark-zh.png',
    'link-completion-light.png',
)
LEGACY = """
[regions.left]
size = 260
[[regions.left.groups]]
id = "688DCCB1-6A7C-483D-8856-9F60E30B164D"
tabs = ["files", "search"]
selected = "files"

[regions.right]
size = 250
[[regions.right.groups]]
id = "A06F1A72-472A-40D7-B91D-A0459B479889"
tabs = ["outline", "changes"]
selected = "outline"

[regions.bottom]
size = 230
[[regions.bottom.groups]]
id = "A46774F9-72A2-48E0-B855-7D5592A7364D"
tabs = ["terminal"]
selected = "terminal"
"""
FIXTURE = {
    'architecture.md': (
        '---\naliases: [Blueprint]\ntags: [system]\n---\n'
        '# Storage Architecture\n\n'
        'Read [[target#  section two  |Recovery protocol]] and [[missing|Unindexed topic]].\n\n'
        'Labels: #system/storage #reliability.\n\n'
        'Code stays literal: `[[target]] #ignored`.\n'),
    'router.md': (
        '# Routing Table\n\nSee [[architecture#Data Flow|Storage Architecture]].\n'
        'Embed: ![[architecture#Data Flow|Diagram]].\n'
        'Alternate reference: [[Blueprint]].\n\n#system/network\n'),
    'pipeline.md': (
        '# Queue Pipeline\n\n```mermaid\nflowchart LR\n'
        '  A["[[architecture]]"] --> B[Queue]\n```\n\n#system/storage\n'),
    'target.md': (
        '# Replication Notes\n\n' +
        '\n\n'.join(f'Batch {i:02d} records the ordering of journal checkpoints.' for i in range(28)) +
        '\n\n## Section Two\n\nRecover the journal before resuming replication.\n'),
    'archive.md': '# Archive Policy\n',
    'dirty.md': '# Local Scratch\n\n[[architecture]]\n',
    'unrelated.md': '# Transport Checklist\n\n[[archive]]\n',
}


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
    assert result['left'] == ['files', 'search', 'tags'], result
    assert result['right'] == ['outline', 'backlinks', 'changes'], result
    assert not result['bottom_visible'], result
    assert (result['backlinks_label'], result['tags_label']) == (
        ('Backlinks', 'Tags') if mode == 'en' else ('反链', '标签')), result
    assert (result['backlink_count'], result['diagram_count'], result['embed_count']) == (5, 1, 1), result
    assert result['backlink_reveal'] and result['heading_start'] > 1000, result
    assert result['broken_stayed'] and result['command_click'] and result['tag_note_opened'], result
    assert result['tag_counts'] == {'reliability': 1, 'system': 3, 'system/network': 1, 'system/storage': 2}, result
    assert result['completion'] == {
        'matches': ['architecture', 'archive'], 'keyboard_selection': True, 'single_undo': True,
    }, result
    assert result['before_read_only'] == result['after_read_only'] == before, result


def run_case(app, directory, output, media, language, appearance, mode):
    state = directory / mode
    vault = state / 'vault'
    vault.mkdir(parents=True)
    config = state / 'config'
    config.mkdir()
    if mode == 'zh-dark':
        (config / 'layout.toml').write_text(LEGACY, encoding='utf-8')
    for path, text in FIXTURE.items():
        (vault / path).write_text(text, encoding='utf-8')
    before = hashes(vault)
    report = state / 'result.json'
    zdot = state / 'shell'
    zdot.mkdir()
    (zdot / '.zshrc').write_text("PS1='> '\nRPROMPT=''\n", encoding='utf-8')
    env = dict(os.environ, HORTUS_CONFIG_DIR=str(config), HORTUS_CACHE_DIR=str(state / 'cache'),
               ZDOTDIR=str(zdot), HISTFILE=str(zdot / 'history'))
    log = output / f'knowledge-{mode}.log'
    with log.open('wb') as stream:
        completed = subprocess.run(
            [app, '-AppleLanguages', f'({language})', f'--{appearance}', f'--vault={vault}',
             f'--knowledge-ui-inspection={report}', f'--knowledge-media={media}', f'--knowledge-shot={mode}'],
            env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=90,
        )
    assert completed.returncode == 0 and report.exists(), f'native run failed: {log}'
    result = json.loads(report.read_text(encoding='utf-8'))
    verify(result, before, mode)
    after = hashes(vault)
    if mode == 'en':
        rename = result['rename']
        assert rename['dirty_preserved'] and rename['updated_open_note'], rename
        assert rename['new_path'] == 'topology.md' and rename['backlink_count'] == 4, rename
        assert sorted(rename['targets']) == ['Blueprint', 'topology', 'topology', 'topology'], rename
        assert set(after) == (set(before) - {'architecture.md'}) | {'topology.md'}, after
        assert after['topology.md'] == before['architecture.md'], after
        for path in ['router.md', 'pipeline.md']:
            expected = FIXTURE[path].replace('[[architecture', '[[topology')
            assert (vault / path).read_text(encoding='utf-8') == expected, path
        for path in ['dirty.md', 'target.md', 'archive.md', 'unrelated.md']:
            assert before[path] == after[path], path
    else:
        assert before == after, after
    result['after_process_exit'] = after
    result['legacy_layout'] = mode == 'zh-dark'
    write_json(output / f'knowledge-{mode}.json', result)
    return {'language': language, 'appearance': appearance, 'files': len(before), 'read_only': True,
            'renamed': mode == 'en', 'legacy_layout': mode == 'zh-dark'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/knowledge-ui')
    parser.add_argument('--media', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    media = args.media.resolve() if args.media else output / 'screenshots'
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hortus-knowledge-') as temporary:
        directory = Path(temporary)
        app = prepare_app(directory)
        cases = [run_case(app, directory, output, media, *case) for case in CASES]
    assert all((media / name).read_bytes().startswith(b'\x89PNG\r\n\x1a\n') for name in SHOTS)
    result = {'cases': cases, 'screenshots': list(SHOTS), 'passed': True}
    write_json(output / 'knowledge-ui.json', result)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
