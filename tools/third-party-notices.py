#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""生成应用包里的第三方许可证声明：链接进 Hortus.app 的 Rust crate、Rust 标准库和 Swift 包。

Rust 部分从 hortus-ffi 出发，只沿普通依赖走（不含 build/dev 依赖），平台为 aarch64-apple-darwin。
每个依赖都要找到许可证原文，找不到就报错退出，不生成残缺的声明。
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'aarch64-apple-darwin'
# 多选一时优先用的许可证，靠前的先选
PREFERENCE = ['MIT', 'Apache-2.0', 'BSD-3-Clause', 'BSD-2-Clause', 'ISC', 'Zlib', 'Unicode-3.0',
              'CC0-1.0', 'Unlicense', 'MPL-2.0', 'Apache-2.0 WITH LLVM-exception']
# 原文里能认出许可证的句子
SIGNATURES = {
    # 先认特征明显的；Unicode 许可证里也有 MIT 那句 "Permission is hereby granted"
    'Unicode-3.0': r'UNICODE LICENSE V3|Unicode, Inc\.',
    'MPL-2.0': r'Mozilla Public License,? [Vv]ersion 2\.0',
    'Apache-2.0': r'Apache License,?\s+Version 2\.0',
    'CC0-1.0': r'CC0 1\.0 Universal|Creative Commons Zero|CREATIVE COMMONS CORPORATION IS NOT A LAW FIRM',
    'Unlicense': r'This is free and unencumbered software released into the public domain',
    'Zlib': r'This software is provided .as-is.,? without any express or implied',
    'ISC': r'Permission to use, copy, modify, and(/or)? distribute this software for any',
    'MIT': r'Permission is hereby granted, free of charge',
    'BSD-3-Clause': r'Neither the name of',
    'BSD-2-Clause': r'Redistributions in binary form must reproduce',
}
SEPARATOR = '=' * 78
LICENSE_FILE = re.compile(r'(?i)^(licen[cs]e|copying|copyright|unlicense)')
NOTICE_FILE = re.compile(r'(?i)^notice')
BUNDLED = {'MPL-2.0': ROOT / 'tools/licenses/MPL-2.0.txt'}


def choose(expression):
    """把 SPDX 表达式拆成必须同时满足的几项，每项在可选的许可证里按偏好选一个"""
    expression = (expression or '').replace('/', ' OR ')
    chosen = []
    for part in re.split(r'\s+AND\s+', expression.strip()):
        options = [option.strip(' ()') for option in re.split(r'\s+OR\s+', part.strip(' ()'))]
        options = [option for option in options if option]
        ranked = sorted(options, key=lambda option: PREFERENCE.index(option) if option in PREFERENCE else len(PREFERENCE))
        if not ranked:
            raise SystemExit(f'无法解析许可证表达式：{expression!r}')
        chosen.append(ranked[0])
    return chosen


def classify(text):
    for spdx, pattern in SIGNATURES.items():
        if re.search(pattern, text):
            return spdx
    return None


def license_text(directory, spdx, name):
    """在包目录里找这个许可证的原文；文件名有提示时按文件名，否则按内容认"""
    base = spdx.split(' WITH ')[0]
    files = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE_FILE.match(path.name))
    hint = {'Apache-2.0': 'apache', 'Unicode-3.0': 'unicode', 'CC0-1.0': 'cc0', 'Unlicense': 'unlicense'}.get(base, base.lower())
    by_name = [path for path in files if hint in path.name.lower()]
    for path in by_name + files:
        text = path.read_text(errors='replace')
        # 有的包把几种许可证和版权说明写在同一个文件里（如 libm），整份保留
        if (path in by_name and len(files) > 1) or re.search(SIGNATURES.get(base, r'(?!)'), text):
            return text
    if len(files) == 1 and classify(files[0].read_text(errors='replace')) is None:
        return files[0].read_text(errors='replace')
    if base in BUNDLED:
        return BUNDLED[base].read_text()
    raise SystemExit(f'{name}：找不到 {spdx} 的许可证原文（{directory}）')


def notices(directory):
    """Apache-2.0 第 4(d) 条：包里带的 NOTICE 文件要一并保留"""
    return [path.read_text(errors='replace') for path in sorted(directory.iterdir())
            if path.is_file() and NOTICE_FILE.match(path.name)]


def rust_crates():
    metadata = json.loads(subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--offline', '--filter-platform', TARGET],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    packages = {package['id']: package for package in metadata['packages']}
    start = next(package['id'] for package in metadata['packages'] if package['name'] == 'hortus-ffi')
    seen, pending = set(), [start]
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        for dependency in nodes[current]['deps']:
            if any(kind['kind'] is None for kind in dependency['dep_kinds']):
                pending.append(dependency['pkg'])
    crates = []
    for package in sorted((packages[i] for i in seen if packages[i]['source']), key=lambda p: (p['name'], p['version'])):
        directory = Path(package['manifest_path']).parent
        for spdx in choose(package['license']):
            crates.append({'name': package['name'], 'version': package['version'], 'license': spdx,
                           'expression': package['license'], 'source': package.get('repository') or '',
                           'text': license_text(directory, spdx, package['name']), 'notices': notices(directory)})
    return crates


def rust_std():
    # 标准库代码会编进二进制；Rust 项目自己的说明是 MIT OR Apache-2.0，这里按 MIT 给出
    sysroot = Path(subprocess.run(['rustc', '--print', 'sysroot'], check=True, capture_output=True, text=True).stdout.strip())
    template = (sysroot / 'share/doc/rust/licenses/MIT.txt').read_text()
    version = subprocess.run(['rustc', '--version'], check=True, capture_output=True, text=True).stdout.split()[1]
    text = template.replace('<year> <copyright holders>', 'The Rust Project Developers')
    return {'name': 'Rust standard library', 'version': version, 'license': 'MIT', 'expression': 'MIT OR Apache-2.0',
            'source': 'https://github.com/rust-lang/rust', 'text': text, 'notices': []}


def swift_packages(resolved, checkouts):
    if not resolved.exists():
        raise SystemExit(f'没有 {resolved}：先运行 swift build 解析依赖')
    pins = json.loads(resolved.read_text()).get('pins', [])
    packages = []
    for pin in sorted(pins, key=lambda pin: pin['identity']):
        location = pin['location']
        directory = checkouts / Path(location.rstrip('/')).name.removesuffix('.git')
        if not directory.is_dir():
            raise SystemExit(f'{pin["identity"]}：找不到检出目录 {directory}')
        files = [path for path in sorted(directory.iterdir()) if path.is_file() and LICENSE_FILE.match(path.name)]
        if not files:
            raise SystemExit(f'{pin["identity"]}：检出目录里没有许可证文件')
        text = files[0].read_text(errors='replace')
        spdx = classify(text) or 'see text'
        state = pin.get('state', {})
        packages.append({'name': pin['identity'], 'version': state.get('version') or state.get('revision', '')[:12],
                         'license': spdx, 'expression': spdx, 'source': location, 'text': text,
                         'notices': notices(directory)})
    return packages


def render(groups):
    lines = ['Hortus third-party licenses', '=' * 27, '',
             'Hortus.app includes the following open source software. The Hortus application is licensed',
             'under GPL-3.0-only and its Rust core under MIT OR Apache-2.0; see the Hortus repository.',
             'Where a package offers a choice of licenses, the license used is the one shown below.', '']
    # 原文相同的放在一起，只印一次
    blocks = {}
    for item in groups:
        key = re.sub(r'\s+', ' ', item['text']).strip()
        blocks.setdefault(key, {'text': item['text'], 'items': []})['items'].append(item)
    for block in blocks.values():
        lines.append(SEPARATOR)
        for item in block['items']:
            choice = '' if item['expression'] == item['license'] else f' (chosen from {item["expression"]})'
            lines.append(f'{item["name"]} {item["version"]} — {item["license"]}{choice}')
            if item['source']:
                lines.append(f'  Source: {item["source"]}')
        if any(item['license'] == 'MPL-2.0' for item in block['items']):
            lines.append('  These files are used unmodified. Their source code is available at the address above.')
        lines += ['', block['text'].strip(), '']
        for notice in dict.fromkeys(n.strip() for item in block['items'] for n in item['notices']):
            lines += ['NOTICE:', notice, '']
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--package-resolved', type=Path, default=ROOT / 'apps/macos/Package.resolved')
    parser.add_argument('--checkouts', type=Path, default=ROOT / 'apps/macos/.build/checkouts')
    parser.add_argument('--rust-only', action='store_true', help='skip Swift packages (for checking the Rust part)')
    args = parser.parse_args()
    items = rust_crates() + [rust_std()]
    if not args.rust_only:
        items += swift_packages(args.package_resolved, args.checkouts)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render(items))
    print(f'{args.output}: {len(items)} components', file=sys.stderr)


if __name__ == '__main__':
    main()
