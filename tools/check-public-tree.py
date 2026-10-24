#!/usr/bin/env python3
"""检查受版本控制的公开文件、私人文本泄漏和原生应用包。"""
import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LEGACY_NAME = 'mark' + 'note'
LEGACY_TITLE = 'Mark' + 'Note'
PRIVATE_PREFIXES = ('arch' + 'ive/', 'co' + 'dex/', 'mock' + 'ups/')
PRIVATE_PLAN = 'plan' + '.yaml'
FORBIDDEN_PATHS = (PRIVATE_PLAN, *PRIVATE_PREFIXES)
ATTRIBUTION = 'Co-' + 'Authored-By'
# 只允许迁移实现和隔离迁移测试中的旧目录、旧环境变量字面量。
MIGRATION_EXCEPTIONS = {
    'crates/hortus-config/src/lib.rs': (
        LEGACY_NAME.upper() + '_CONFIG_DIR', LEGACY_NAME.upper() + '_CACHE_DIR',
        '.config/' + LEGACY_NAME, 'Library/Caches/' + LEGACY_TITLE, LEGACY_NAME,
    ),
    'crates/hortus-config/tests/config.rs': (
        LEGACY_NAME.upper() + '_CONFIG_DIR', LEGACY_NAME.upper() + '_CACHE_DIR',
        '.config/' + LEGACY_NAME, 'Library/Caches/' + LEGACY_TITLE, '.cache/' + LEGACY_NAME,
    ),
    'crates/hortus-ffi/tests/migration.rs': (
        LEGACY_NAME.upper() + '_CONFIG_DIR', LEGACY_NAME.upper() + '_CACHE_DIR',
    ),
}
HAN = r'\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\U00020000-\U0002fa1f'
HAN_SEGMENTS = re.compile('[' + HAN + ']{4,}')


def tracked_public_files(root):
    output = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root)
    return [name for raw in output.split(b'\0') if raw
            if (name := raw.decode()) != PRIVATE_PLAN and not name.startswith(PRIVATE_PREFIXES)]


def private_segments(root):
    directory = root / 'fixtures/mermaid/tree'
    samples = sorted(path for path in directory.rglob('*') if path.is_file()) if directory.exists() else []
    if not samples:
        print('SKIP: private sample text check: local fixtures/mermaid/tree files are absent')
        return set()
    segments = set()
    for path in samples:
        # 也检查文件名；只在运行时提取，日志不打印任何私人文字。
        for text in (path.name, path.read_bytes().decode('utf-8', errors='replace')):
            for fragment in HAN_SEGMENTS.findall(text):
                # 四字窗口可检出任意至少四字的重叠片段，避免漏掉长段中的部分复制。
                segments.update(fragment[index:index + 4] for index in range(len(fragment) - 3))
    print(f'Private sample text check: {len(samples)} local files, {len(segments)} distinct segments')
    return segments


def scan_text(name, text, segments):
    violations = []
    legacy = re.compile(LEGACY_NAME, re.IGNORECASE)
    for number, line in enumerate(text.splitlines(), 1):
        allowed = line
        for literal in MIGRATION_EXCEPTIONS.get(name, ()):
            # 仅放行完整字符串字面量，旁边出现的旧名称仍报错。
            allowed = allowed.replace('"' + literal + '"', '"<legacy>"')
        if legacy.search(allowed):
            violations.append((name, number, 'legacy product name outside migration allowlist'))
        if any(path in line for path in FORBIDDEN_PATHS):
            violations.append((name, number, 'private workflow path'))
        if ATTRIBUTION.casefold() in line.casefold():
            violations.append((name, number, 'commit attribution trailer'))
        if any(segment in line for segment in segments):
            violations.append((name, number, 'private sample text'))
    return violations


def check_bundle(bundle):
    violations = []
    binary = bundle / 'Contents/MacOS/Hortus'
    if not binary.is_file():
        return [(str(binary), 1, 'application binary missing; run tools/build-macos.sh first')]
    # 直接检查字节，panic 位置和只读数据里的路径也不能随包公开。
    try:
        data = binary.read_bytes()
    except OSError as error:
        return [(str(binary), 1, f'cannot read application binary: {error}')]
    prefixes = {b'/Users/', b'/private/var/folders/', bytes(Path.home())}
    if any(prefix in data for prefix in prefixes):
        violations.append((str(binary), 1, 'local user or temporary path in application binary'))
    try:
        linked = subprocess.run(['otool', '-L', str(binary)], check=True, text=True,
                                capture_output=True).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        return [(str(binary), 1, f'cannot inspect linked libraries: {error}')]
    for number, line in enumerate(linked.splitlines()[1:], 2):
        if re.search(r'WebKit|JavaScriptCore', line, re.IGNORECASE):
            violations.append((str(binary), number, 'forbidden web framework dependency'))
    for path in bundle.rglob('*'):
        if path.is_file() and path.suffix.casefold() in ('.html', '.js'):
            violations.append((str(path), 1, 'web asset in application bundle'))
    # 分发的应用必须带自身许可证和第三方许可证声明
    resources = bundle / 'Contents/Resources'
    if not (resources / 'LICENSE.txt').is_file():
        violations.append((str(resources / 'LICENSE.txt'), 1, 'application license missing from bundle'))
    notices = resources / 'ThirdPartyNotices.txt'
    if not notices.is_file():
        violations.append((str(notices), 1, 'third-party notices missing from bundle'))
    else:
        text = notices.read_text(errors='replace')
        for component in ('swiftterm', 'uniffi ', 'pulldown-cmark ', 'Rust standard library'):
            if component not in text:
                violations.append((str(notices), 1, f'third-party notices do not mention {component.strip()}'))
    return violations


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--app', type=Path, default=ROOT / 'build/Hortus.app')
    args = parser.parse_args()
    names = tracked_public_files(ROOT)
    segments = private_segments(ROOT)
    violations = []
    for name in names:
        path = ROOT / name
        try:
            text = path.read_bytes().decode('utf-8', errors='replace')
        except OSError as error:
            violations.append((name, 1, f'cannot read tracked file: {error}'))
            continue
        violations.extend(scan_text(name, text, segments))
    violations.extend(check_bundle(args.app))
    for name, number, reason in violations:
        print(f'{name}:{number}: {reason}')
    if violations:
        print(f'FAIL: {len(violations)} violations in public tree/application bundle')
        return 1
    print(f'PASS: {len(names)} tracked public files; legacy names, private paths, attribution and private text checked')
    print('PASS: Hortus.app has no WebKit/JavaScriptCore dependencies and no HTML/JavaScript assets')
    print('PASS: Hortus executable has no local user or temporary paths')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
