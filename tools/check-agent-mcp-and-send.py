#!/usr/bin/env python3
"""End-to-end check for hortus mcp stdio server and macOS Send-to-Terminal.

Verifies JSON-RPC 2.0 read-only MCP tools/resources over target/release/hortus
and real-window PTY payload delivery, menu state, and screenshots in Hortus.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / 'target/release/hortus'
DEBUG_APP = ROOT / 'apps/macos/.build/debug/Hortus'
NOTE = 'Daily Notes/Plan for 今天.md'
MARKER = Path(f'/tmp/hortus-p3-05-{os.getpid()}-never-run')
SELECTION = ('第一段中文，包含表情 😀 和多字节字符。\n'
             f'第二段：touch {MARKER} 只作为文本保留。\n'
             '第三段结束。')
DIAGRAMS = (
    '```mermaid\nmindmap\n  Root\n    Child\n```\n\n'
    '```mermaid\nflowchart TB\nsubgraph G\nA --> B\nend\n```\n\n'
    '```mermaid\nsequenceDiagram\nA->>B: hello\n```\n\n'
    '```mermaid\nstateDiagram-v2\n[*] --> A\n```\n\n'
    '```mermaid\nclassDiagram\nOrder "1" *-- "1..*" LineItem : contains\n```\n\n'
    '```mermaid\nerDiagram\nUSER ||--o{ ORDER : places\n```\n'
)


def check(condition, message):
    if not condition:
        raise SystemExit(f'CHECK FAILED: {message}')


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8')


def snapshot(root):
    result = {}
    for path in sorted(root.rglob('*')):
        relative = str(path.relative_to(root))
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode):
            result[relative] = ['symlink', os.readlink(path)]
        elif path.is_dir():
            result[relative] = ['directory']
        else:
            result[relative] = [hashlib.sha256(path.read_bytes()).hexdigest(), info.st_mtime_ns]
    return result


def wait_for_file(path, process, seconds, label):
    deadline = time.monotonic() + seconds
    while not path.exists():
        if process.poll() is not None:
            stderr = process.stderr.read() if process.stderr else ''
            raise SystemExit(f'CHECK FAILED: {label}: app exited {process.returncode}: {stderr}')
        if time.monotonic() >= deadline:
            raise SystemExit(f'CHECK FAILED: {label}: timed out waiting for {path.name}')
        time.sleep(0.05)


# ------------------------------------------------------------------ MCP stdio

class Server:
    def __init__(self, vault, base):
        self.next_id = 1
        self.lines = []
        self.config = base / 'mcp-config'
        env = {key: value for key, value in os.environ.items() if not key.startswith('HORTUS_')}
        env.update(HOME=str(base), HORTUS_CONFIG_DIR=str(self.config),
                   HORTUS_CACHE_DIR=str(base / 'mcp-cache'),
                   HORTUS_DATA_DIR=str(base / 'mcp-data'))
        self.process = subprocess.Popen(
            [str(RELEASE), 'mcp', '--vault', str(vault)], cwd=base, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding='utf-8')

    def send_raw(self, line):
        self.process.stdin.write(line + '\n')
        self.process.stdin.flush()

    def send(self, message):
        self.send_raw(json.dumps(message))

    def read(self):
        line = self.process.stdout.readline()
        check(line, 'MCP server closed stdout')
        self.lines.append(line.rstrip('\n'))
        return json.loads(line)

    def request(self, method, params=None):
        identifier = self.next_id
        self.next_id += 1
        message = {'jsonrpc': '2.0', 'id': identifier, 'method': method}
        if params is not None:
            message['params'] = params
        self.send(message)
        response = self.read()
        check(response.get('jsonrpc') == '2.0', response)
        check(response.get('id') == identifier, response)
        return response

    def notify(self, method, params=None):
        message = {'jsonrpc': '2.0', 'method': method}
        if params is not None:
            message['params'] = params
        self.send(message)

    def call(self, name, arguments=None):
        return self.request('tools/call', {'name': name, 'arguments': arguments or {}})

    def finish(self):
        self.process.stdin.close()
        rest = self.process.stdout.read()
        self.lines.extend(rest.splitlines())
        stderr = self.process.stderr.read()
        code = self.process.wait(timeout=30)
        return code, stderr


def structured(response):
    check(response['result']['isError'] is False, response)
    return response['result']['structuredContent']


def tool_text(response):
    return response['result']['content'][0]['text']


def run_mcp(base, output):
    vault = base / 'mcp-vault'
    write(vault / 'index.md',
          '---\ntags: [topic, topic/sub]\n---\n# Index\n\n'
          'pipeline [[guides/intro]] [[missing]] ![[image.png]]\n\n'
          '```mermaid\nflowchart TB\nA[\"[[guides/intro]]\"] --> B\n```\n')
    write(vault / 'guides/intro.md', '# Intro\n## Setup\n\nBack [[index]].\n')
    write(vault / 'lonely.md', '# Lonely\n\nNo links.\n')
    write(vault / 'second.md', '# Second\n\n#topic #topic/other\n')
    write(vault / 'diagrams.md', DIAGRAMS)
    write(vault / 'invalid.md', '```mermaid\nflowchart TB\nA[unclosed\n```\n')
    write(base / 'outside.md', '# Outside\n')
    (vault / 'link.md').symlink_to(base / 'outside.md')

    before = snapshot(vault)
    server = Server(vault, base)
    result = {}

    initialize = server.request('initialize', {'protocolVersion': '2024-11-05'})
    check(initialize['result']['protocolVersion'] == '2024-11-05', initialize)
    check(initialize['result']['capabilities'] == {'tools': {}, 'resources': {}}, initialize)
    check(initialize['result']['serverInfo']['name'] == 'hortus', initialize)
    check(initialize['result']['serverInfo']['version'] == '0.4.0', initialize)
    server.notify('notifications/initialized', {})
    check(server.request('ping')['result'] == {}, 'ping')
    fallback = server.request('initialize', {'protocolVersion': '2099-01-01'})
    check(fallback['result']['protocolVersion'] == '2025-03-26', fallback)
    result['initialize'] = initialize['result']

    tools = server.request('tools/list')['result']['tools']
    names = [tool['name'] for tool in tools]
    check(names == ['hortus_search', 'hortus_backlinks', 'hortus_links', 'hortus_tags',
                    'hortus_outline', 'hortus_orphans', 'hortus_broken_links',
                    'hortus_mermaid_check', 'hortus_guide'], names)
    for tool in tools:
        annotations = tool['annotations']
        check(annotations['readOnlyHint'] is True, tool)
        check(annotations['destructiveHint'] is False
              and annotations['idempotentHint'] is True
              and annotations['openWorldHint'] is False, tool)
        check(tool['inputSchema']['type'] == 'object', tool)
    result['tools'] = names

    search = structured(server.call('hortus_search', {'query': 'pipeline'}))
    check(search['results'][0]['path'] == 'index.md', search)
    backlinks = structured(server.call('hortus_backlinks', {'note': 'guides/intro.md'}))
    check(backlinks['count'] == 2, backlinks)
    links = structured(server.call('hortus_links', {'note': 'index'}))
    check(links['count'] == 4 and links['links'][1]['resolution']['kind'] == 'broken', links)
    check(structured(server.call('hortus_tags'))['count'] == 3, 'tags')
    check(structured(server.call('hortus_tags', {'tag': 'topic'}))['count'] == 2, 'tag filter')
    outline = structured(server.call('hortus_outline', {'note': 'guides/intro.md'}))
    check([heading['text'] for heading in outline['headings']] == ['Intro', 'Setup'], outline)
    check(structured(server.call('hortus_orphans'))['count'] == 4, 'orphans')
    check(structured(server.call('hortus_broken_links'))['count'] == 1, 'broken links')
    diagrams = structured(server.call('hortus_mermaid_check', {'note': 'diagrams.md'}))
    check(diagrams['valid'], diagrams)
    kinds = [block['diagram_type'] for block in diagrams['blocks']]
    check(kinds == ['tree', 'flowchart', 'sequence', 'state', 'class', 'er'], kinds)
    single = structured(server.call('hortus_mermaid_check',
                                    {'note': 'diagrams.md', 'block': 5}))
    check(single['blocks'][0]['diagram_type'] == 'class', single)
    invalid = server.call('hortus_mermaid_check', {'note': 'invalid.md'})
    check(invalid['result']['isError'] is False, invalid)
    check(structured(invalid)['valid'] is False, invalid)
    check(structured(invalid)['blocks'][0]['error_kind'] == 'syntax', invalid)
    guide = server.call('hortus_guide')
    check(tool_text(guide).startswith('# Working in a Hortus knowledge base'), tool_text(guide)[:80])
    result['diagram_types'] = kinds

    # 外部新建的笔记在下一次调用立即可见。
    write(vault / 'fresh.md', '# Fresh\n\nhot-reload-token\n')
    fresh = structured(server.call('hortus_search', {'query': 'hot-reload-token'}))
    check([item['path'] for item in fresh['results']] == ['fresh.md'], fresh)
    (vault / 'fresh.md').unlink()

    resources = server.request('resources/list')['result']['resources']
    check([resource['uri'] for resource in resources] == ['hortus://guide'], resources)
    check(resources[0]['mimeType'] == 'text/markdown', resources)
    read = server.request('resources/read', {'uri': 'hortus://guide'})['result']['contents'][0]
    check(read['text'] == tool_text(guide), 'resource text differs from guide tool')

    server.send_raw('{oops')
    check(server.read()['error']['code'] == -32700, 'parse error')
    server.send_raw('[1, 2]')
    check(server.read()['error']['code'] == -32600, 'invalid request')
    check(server.request('unknown/method')['error']['code'] == -32601, 'unknown method')
    check(server.request('tools/call', {'arguments': {}})['error']['code'] == -32602, 'missing tool')
    check(server.call('hortus_write')['error']['code'] == -32602, 'unknown tool')
    check(server.call('hortus_outline')['error']['code'] == -32602, 'missing note')
    check(server.call('hortus_search', {'query': 'x', 'limit': 'many'})['error']['code'] == -32602,
          'bad limit')
    check(server.call('hortus_mermaid_check',
                      {'note': 'diagrams.md', 'block': 0})['error']['code'] == -32602, 'bad block')
    check(server.call('hortus_outline', {'note': '../outside.md'})['error']['code'] == -32602,
          'path traversal')
    check(server.call('hortus_outline',
                      {'note': str(base / 'outside.md')})['error']['code'] == -32602,
          'outside absolute path')
    check(server.call('hortus_outline', {'note': 'link.md'})['error']['code'] == -32602,
          'symlink note')
    missing = server.call('hortus_outline', {'note': 'absent.md'})
    check(missing['result']['isError'] is True, missing)
    check(server.request('resources/read',
                         {'uri': 'hortus://missing'})['error']['code'] == -32002, 'unknown resource')

    code, stderr = server.finish()
    check(code == 0, f'MCP EOF exit code {code}: {stderr}')
    check(stderr == '', f'MCP diagnostics polluted stderr: {stderr}')
    for line in server.lines:
        message = json.loads(line)
        check(message.get('jsonrpc') == '2.0' and ('result' in message or 'error' in message), line)
    after = snapshot(vault)
    check(before == after, 'MCP server changed the vault')
    check(not server.config.exists(), 'MCP server created the configuration directory')
    result.update(exit_code=code, stdout_lines=len(server.lines), vault_unchanged=True)
    write(output / 'mcp.json', json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    return result


# ------------------------------------------------------------- macOS real app

def environment(base, shell='/bin/zsh', capture=None):
    zdot = base / 'zdotdir'
    zdot.mkdir(parents=True, exist_ok=True)
    for name in ('.zshrc', '.zprofile'):
        write(zdot / name, "PROMPT='❯ '\nRPROMPT=''\nunsetopt share_history 2>/dev/null\n")
    env = {key: value for key, value in os.environ.items()
           if not key.startswith('HORTUS_') and not key.startswith('HORTUS_SEND')
           and key not in ('LANG', 'LC_ALL', 'LC_CTYPE')}
    env.update(HOME=str(base), SHELL=shell, ZDOTDIR=str(zdot), LANG='en_US.UTF-8',
               HISTFILE=str(zdot / 'history'),
               HORTUS_CONFIG_DIR=str(base / 'config'),
               HORTUS_CACHE_DIR=str(base / 'cache'),
               HORTUS_DATA_DIR=str(base / 'data'))
    if capture is not None:
        env['HORTUS_SEND_CAPTURE'] = str(capture)
    return env


def shell_quote(path):
    safe = all(character.isascii() and (character.isalnum() or character in '-._/')
               for character in path)
    if path and safe:
        return path
    return "'" + path.replace("'", "'\\''") + "'"


def run_inspection(base, vault, output, media, theme, capture=None, screenshot=False):
    result_path = output / f'send-{theme}.json'
    result_path.unlink(missing_ok=True)
    arguments = [str(DEBUG_APP), f'--{theme}', f'--vault={vault}',
                 f'--send-to-terminal-inspection={result_path}',
                 f'--send-to-terminal-note={NOTE}',
                 f'--send-to-terminal-selection={SELECTION}']
    if capture is not None:
        arguments.append(f'--send-to-terminal-capture={capture}')
    if screenshot:
        arguments.append(f'--send-to-terminal-snapshot={media / f"send-{theme}.png"}')
    process = subprocess.Popen(arguments, env=environment(base), cwd=base,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait_for_file(result_path, process, 180, f'send-{theme}')
        data = json.loads(result_path.read_text())
        code = process.wait(timeout=60)
        stderr = process.stderr.read().decode(errors='replace')
        check(code == 0, f'send-{theme}: exit {code}: {stderr}')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=20)
    return data


def validate_send(data, theme, expected_selection):
    check(data['path']['matches_expected'], data['path'])
    check(data['path']['text'] == shell_quote(NOTE) + ' ', data['path'])
    check(not data['path_has_trailing_newline'], data['path'])
    check(data['path']['tab_kind'] == 'shell', data['path'])
    check(data['terminal_opened'], 'terminal did not open')
    check(data['terminal_focused'], 'terminal did not take focus')
    check(data['path_screen'], 'terminal screen lacks the path')
    check(data['selection']['matches_expected'], data['selection'])
    check(data['selection']['text'] == expected_selection, data['selection'])
    check(data['selection_bracketed'], 'multi-line selection must use bracketed paste')
    check(data['selection_contains_han'], 'selection lost Chinese text')
    check(data['selection_contains_emoji'], 'selection lost emoji')
    check(not data['selection_has_trailing_newline'], 'selection added a newline')
    check(data['selection_screen'], 'terminal screen lacks the selection')
    check(data['reading_selection_bracketed'], 'reading selection must use bracketed paste')
    check(data['reading_selection_has_han'], 'reading selection lost Chinese text')
    check(data['empty_selection_sent'] is False, 'empty selection was sent')
    check(data['empty_selection_no_send'] is True, 'empty selection reached the PTY')
    check(bool(data['empty_selection_message']), 'empty selection gave no hint')
    menu = data['menu_with_selection']
    check(menu['path']['present'] and menu['path']['enabled'], menu)
    check(menu['selection']['present'] and menu['selection']['enabled'], menu)
    empty = data['menu_without_selection']
    check(empty['path']['enabled'] and not empty['selection']['enabled'], empty)
    check(menu['path']['key'] == 'l' and menu['selection']['key'] == 'l', menu)
    check(menu['path']['title'] == data['menu_path_title'], menu)
    check(menu['selection']['title'] == data['menu_selection_title'], menu)
    check(data['menu_path_title'] in ('Send Note Path to Terminal', '发送笔记路径到终端'),
          data['menu_path_title'])
    check(data['menu_selection_title'] in ('Send Selection to Terminal', '发送选区到终端'),
          data['menu_selection_title'])
    check(data['vault_unchanged'], f'{theme}: vault changed')
    check(data['editing_line_range'] == [3, 5], data['editing_line_range'])


def run_app_checks(base, output, media):
    vault = base / 'vault-ui'
    write(vault / NOTE,
          '# 发送到终端\n\n'
          '第一段中文，包含表情 😀 和多字节字符。\n'
          f'第二段：touch {MARKER} 只作为文本保留。\n'
          '第三段结束。\n\n'
          '另一段用于阅读视图。\n')
    write(vault / 'index.md', '# Index\n\n[[Daily Notes/Plan for 今天]]\n')
    MARKER.unlink(missing_ok=True)
    before = snapshot(vault)
    media.mkdir(parents=True, exist_ok=True)

    results = {}
    for theme in ('light', 'dark'):
        data = run_inspection(base, vault, output, media, theme, screenshot=True)
        expected_lines = data['editing_line_range']
        header = f'{NOTE}（第 {expected_lines[0]}-{expected_lines[1]} 行）：\n'
        validate_send(data, theme, header + SELECTION)
        screenshot = media / f'send-{theme}.png'
        check(screenshot.exists() and screenshot.stat().st_size > 20_000, f'{theme} screenshot')
        results[theme] = data

    recorder = base / 'pty-recorder.zsh'
    recorder.write_text(
        '#!/bin/zsh\n'
        'stty -icanon min 1 time 0 2>/dev/null\n'
        ': > "$HORTUS_SEND_READY"\n'
        'exec cat >> "$HORTUS_SEND_CAPTURE"\n')
    recorder.chmod(0o700)
    capture = base / 'pty-capture.bin'
    capture.unlink(missing_ok=True)

    def run_capture():
        result_path = output / 'send-capture.json'
        result_path.unlink(missing_ok=True)
        capture.unlink(missing_ok=True)
        ready = base / 'pty-ready'
        ready.unlink(missing_ok=True)
        arguments = [str(DEBUG_APP), '--light', f'--vault={vault}',
                     f'--send-to-terminal-inspection={result_path}',
                     f'--send-to-terminal-note={NOTE}',
                     f'--send-to-terminal-selection={SELECTION}',
                     f'--send-to-terminal-capture={capture}',
                     f'--send-to-terminal-ready={ready}']
        env = environment(base, shell=str(recorder), capture=capture)
        env['HORTUS_SEND_READY'] = str(ready)
        process = subprocess.Popen(arguments, env=env, cwd=base,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            wait_for_file(result_path, process, 180, 'send-capture')
            value = json.loads(result_path.read_text())
            code = process.wait(timeout=60)
            stderr = process.stderr.read().decode(errors='replace')
            check(code == 0, f'send-capture: exit {code}: {stderr}')
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=20)
        return value

    capture_data = run_capture()
    expected = shell_quote(NOTE) + ' '
    path_bytes = expected.encode()
    selection_bytes = (b'\x1b[200~' + capture_data['selection']['text'].encode()
                       + b'\x1b[201~')
    reading_bytes = (b'\x1b[200~' + capture_data['reading_selection']['text'].encode()
                     + b'\x1b[201~')
    actual = capture.read_bytes()
    check(actual == path_bytes + selection_bytes + reading_bytes,
          f'PTY bytes differ:\n{actual!r}\n{path_bytes + selection_bytes + reading_bytes!r}')
    check(not actual.endswith(b'\r') and not actual.endswith(b'\n'), 'PTY payload ended with Enter')
    check(capture_data['selection']['bracketed_paste'] and capture_data['reading_selection']['bracketed_paste'],
          'capture run did not use bracketed paste')
    # 选区里的命令只是文本，必须没有执行。
    check(not MARKER.exists(), 'selection executed a command')
    writing = {
        'theme': 'capture', 'path_bytes': len(path_bytes),
        'selection_bytes': len(selection_bytes), 'reading_bytes': len(reading_bytes),
        'total_bytes': len(actual), 'payload': actual.decode(errors='replace'),
    }
    write(output / 'send-capture-bytes.json', json.dumps(writing, ensure_ascii=False, indent=2) + '\n')

    after = snapshot(vault)
    check(before == after, 'real-window sends changed the vault')

    strings = json.loads((ROOT / 'apps/macos/Localizable.xcstrings').read_text())
    localization = {}
    for key, translation in [('Send Note Path to Terminal', '发送笔记路径到终端'),
                             ('Send Selection to Terminal', '发送选区到终端'),
                             ('Select text to send to the terminal', '请先选择要发送到终端的文本')]:
        value = strings['strings'][key]['localizations']['zh-Hans']['stringUnit']['value']
        check(value == translation, f'{key}: {value}')
        localization[key] = value
    results['capture'] = capture_data
    results['localization'] = localization
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/P3-05')
    parser.add_argument('--media', type=Path, default=ROOT / 'target/reports/P3-05/media')
    arguments = parser.parse_args()
    output = arguments.output.expanduser().resolve()
    media = arguments.media.expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    media.mkdir(parents=True, exist_ok=True)

    if not RELEASE.exists():
        subprocess.run(['cargo', 'build', '--release', '-p', 'hortus-cli'], cwd=ROOT, check=True)
    check(RELEASE.exists(), 'missing target/release/hortus')
    check(DEBUG_APP.exists(), 'missing apps/macos/.build/debug/Hortus; run ./tools/test-macos.sh first')

    base = Path(tempfile.mkdtemp(prefix='hortus-agent-mcp-send-'))
    try:
        print('MCP: starting release server…')
        mcp = run_mcp(base, output)
        print(f"MCP: {len(mcp['tools'])} tools, {mcp['stdout_lines']} JSON-RPC lines, exit {mcp['exit_code']}")
        print('macOS: sending path and selection in real windows…')
        sends = run_app_checks(base, output, media)
        summary = {
            'mcp': {key: mcp[key] for key in ('tools', 'diagram_types', 'stdout_lines',
                                              'exit_code', 'vault_unchanged')},
            'light': {key: sends['light'][key] for key in
                      ('path', 'selection', 'reading_selection', 'menu_with_selection',
                       'menu_without_selection', 'terminal_opened', 'terminal_focused',
                       'vault_unchanged')},
            'dark': {key: sends['dark'][key] for key in
                     ('path', 'selection', 'reading_selection', 'menu_with_selection',
                      'menu_without_selection', 'terminal_opened', 'terminal_focused',
                      'vault_unchanged')},
            'capture': {'theme': 'capture'},
            'localization': sends['localization'],
            'screenshots': [str(media / 'send-light.png'), str(media / 'send-dark.png')],
        }
        write(output / 'summary.json', json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        print('CHECK PASSED')
        print(json.dumps(summary, ensure_ascii=False, indent=2))
    finally:
        shutil.rmtree(base, ignore_errors=True)


if __name__ == '__main__':
    main()
