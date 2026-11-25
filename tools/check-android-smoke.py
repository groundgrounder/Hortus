#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Android APK 静态安全合规审计与 arm64 模拟器端到端烟测脚本。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_NAME = 'io.github.groundgrounder.hortus'
SMOKE_ACTIVITY = f'{PACKAGE_NAME}/.DebugSmokeActivity'
DEVICE_VAULT = '/sdcard/Documents/HortusDemoVault'
SMOKE_FILENAME = 'm0-smoke-result.json'
EXPECTED_DIAGRAM_TYPES = ['mindmap', 'flowchart', 'sequence', 'state', 'class', 'er']
KATEX_FONT_FILES = {
    'KaTeX_AMS-Regular.ttf', 'KaTeX_Caligraphic-Bold.ttf', 'KaTeX_Caligraphic-Regular.ttf',
    'KaTeX_Fraktur-Bold.ttf', 'KaTeX_Fraktur-Regular.ttf', 'KaTeX_Main-Bold.ttf',
    'KaTeX_Main-BoldItalic.ttf', 'KaTeX_Main-Italic.ttf', 'KaTeX_Main-Regular.ttf',
    'KaTeX_Math-BoldItalic.ttf', 'KaTeX_Math-Italic.ttf', 'KaTeX_SansSerif-Bold.ttf',
    'KaTeX_SansSerif-Italic.ttf', 'KaTeX_SansSerif-Regular.ttf', 'KaTeX_Script-Regular.ttf',
    'KaTeX_Size1-Regular.ttf', 'KaTeX_Size2-Regular.ttf', 'KaTeX_Size3-Regular.ttf',
    'KaTeX_Size4-Regular.ttf', 'KaTeX_Typewriter-Regular.ttf',
}


def find_sdk_root() -> Path:
    for env_key in ('ANDROID_HOME', 'ANDROID_SDK_ROOT'):
        value = os.environ.get(env_key)
        if value and Path(value).is_dir():
            return Path(value)
    default = Path.home() / 'Library/Android/sdk'
    if default.is_dir():
        return default
    raise SystemExit('未找到 Android SDK（请设置 ANDROID_HOME 或 ANDROID_SDK_ROOT）')


def find_aapt2(sdk_root: Path) -> Path:
    build_tools = sdk_root / 'build-tools'
    if not build_tools.is_dir():
        raise SystemExit(f'未找到 build-tools 目录：{build_tools}')
    candidates = sorted(
        (p / 'aapt2' for p in build_tools.iterdir() if (p / 'aapt2').is_file()),
        reverse=True,
    )
    if not candidates:
        raise SystemExit(f'未在 {build_tools} 中找到 aapt2')
    return candidates[0]


def find_apksigner(sdk_root: Path) -> Path:
    build_tools = sdk_root / 'build-tools'
    if not build_tools.is_dir():
        raise SystemExit(f'未找到 build-tools 目录：{build_tools}')
    candidates = sorted(
        (p / 'apksigner' for p in build_tools.iterdir() if (p / 'apksigner').is_file()),
        reverse=True,
    )
    if not candidates:
        raise SystemExit(f'未在 {build_tools} 中找到 apksigner')
    return candidates[0]


def normalize_sha256_fingerprint(raw: str) -> str:
    hex_only = re.sub(r'[^0-9a-fA-F]', '', raw).upper()
    if len(hex_only) != 64:
        return raw.strip().upper()
    return ':'.join(hex_only[i:i + 2] for i in range(0, 64, 2))


def verify_apk_signature(apk_path: Path, sdk_root: Path, expected_sha256: str | None) -> tuple[dict, list[str]]:
    apksigner = find_apksigner(sdk_root)
    completed = subprocess.run(
        [str(apksigner), 'verify', '--verbose', '--print-certs', str(apk_path)],
        capture_output=True,
        text=True,
    )
    output = (completed.stdout or '') + (completed.stderr or '')
    match = re.search(r'Signer #1 certificate SHA-256 digest:\s*([0-9a-fA-F:]+)', output)
    actual_sha256 = normalize_sha256_fingerprint(match.group(1)) if match else None
    violations: list[str] = []
    if completed.returncode != 0:
        violations.append(f'apksigner verify 失败 (exit={completed.returncode}): {output.strip()}')
    if not actual_sha256:
        violations.append('未能从 apksigner 输出解析 Signer #1 SHA-256 指纹')
    elif expected_sha256:
        expected_norm = normalize_sha256_fingerprint(expected_sha256)
        if actual_sha256 != expected_norm:
            violations.append(f'签名证书 SHA-256 不匹配: actual={actual_sha256}, expected={expected_norm}')
    return {
        'verified': completed.returncode == 0,
        'signerSha256': actual_sha256,
        'output': output.strip(),
    }, violations


def static_audit_apk(
    apk_path: Path,
    sdk_root: Path,
    release: bool,
    expect_signer_sha256: str | None = None,
) -> dict:
    if not apk_path.is_file():
        raise SystemExit(f'APK 不存在：{apk_path}')

    violations: list[str] = []
    details: dict = {
        'apkPath': str(apk_path),
        'apkBytes': apk_path.stat().st_size,
        'buildType': 'release' if release else 'debug',
    }

    with zipfile.ZipFile(apk_path, 'r') as zf:
        names = zf.namelist()
        native_abis = sorted({n.split('/')[1] for n in names if n.startswith('lib/') and n.endswith('.so')})
        details['nativeAbis'] = native_abis
        if native_abis != ['arm64-v8a']:
            violations.append(f'APK 的原生 ABI 不符合预期: {native_abis}')
        so_entry = 'lib/arm64-v8a/libhortus_ffi.so'
        if so_entry not in names:
            violations.append(f'缺少 {so_entry}')
        else:
            so_bytes = zf.read(so_entry)
            details['soBytes'] = len(so_bytes)
            if (not so_bytes.startswith(b'\x7fELF\x02\x01') or
                    len(so_bytes) < 20 or struct.unpack_from('<H', so_bytes, 18)[0] != 183):
                violations.append(f'{so_entry} 不是 AArch64 ELF64')
            forbidden_prefixes = [b'/Users/', b'/var/folders/', b'/private/tmp/', bytes(Path.home())]
            leaked = [p.decode('utf-8', errors='replace') for p in forbidden_prefixes if p in so_bytes]
            details['soLeakedPrefixes'] = leaked
            if leaked:
                violations.append(f'{so_entry} 包含本机或临时路径前缀: {leaked}')

        web_assets = [n for n in names if Path(n).suffix.casefold() in ('.html', '.htm', '.js', '.mjs')]
        details['webAssets'] = web_assets
        if web_assets:
            violations.append(f'APK 包含禁止的 Web 资产: {web_assets}')

        dex_entries = [n for n in names if re.fullmatch(r'classes\d*\.dex', n)]
        dex_webview_hits = []
        dex_debug_hits = []
        dex_path_leaks = []
        forbidden_prefixes = [b'/Users/', b'/var/folders/', b'/private/tmp/', bytes(Path.home())]
        debug_dex_markers = (
            b'Lio/github/groundgrounder/hortus/DebugSmokeActivity;',
            b'Lio/github/groundgrounder/hortus/DebugExportActivity;',
            b'Lio/github/groundgrounder/hortus/DebugPreviewProvider;',
        )
        for dex in dex_entries:
            dex_bytes = zf.read(dex)
            webkit_matches = re.findall(
                rb'(?:Landroid/webkit/|android/webkit/|android\.webkit\.)[A-Za-z0-9_$.]+',
                dex_bytes,
            )
            forbidden_webkit = [
                m.decode('utf-8', errors='replace')
                for m in webkit_matches
                if m not in (b'Landroid/webkit/MimeTypeMap', b'android/webkit/MimeTypeMap', b'android.webkit.MimeTypeMap')
            ]
            if forbidden_webkit:
                dex_webview_hits.append(f'{dex}: {sorted(set(forbidden_webkit))}')
            if any(marker in dex_bytes for marker in debug_dex_markers):
                dex_debug_hits.append(dex)
            leaked = [p.decode('utf-8', errors='replace') for p in forbidden_prefixes if p in dex_bytes]
            if leaked:
                dex_path_leaks.append(f'{dex}: {leaked}')
        details['dexCount'] = len(dex_entries)
        details['dexWebViewHits'] = dex_webview_hits
        if dex_webview_hits:
            violations.append(f'DEX 包含 android/webkit/ 引用: {dex_webview_hits}')
        details['dexDebugComponentHits'] = dex_debug_hits
        if not dex_entries or bool(dex_debug_hits) == release:
            violations.append('DEX 中的 Debug 组件与构建类型不符')
        details['dexLeakedPrefixes'] = dex_path_leaks
        if dex_path_leaks:
            violations.append(f'DEX 包含本机或临时路径前缀: {dex_path_leaks}')

        asset_path_leaks = []
        for entry_name in names:
            if not entry_name.startswith('assets/'):
                continue
            entry_bytes = zf.read(entry_name)
            leaked = [p.decode('utf-8', errors='replace') for p in forbidden_prefixes if p in entry_bytes]
            if leaked:
                asset_path_leaks.append(f'{entry_name}: {leaked}')
        details['assetLeakedPrefixes'] = asset_path_leaks
        if asset_path_leaks:
            violations.append(f'assets 包含本机或临时路径前缀: {asset_path_leaks}')

        katex_prefix = 'assets/Fonts/KaTeX/'
        found_katex_fonts = {
            name[len(katex_prefix):]
            for name in names
            if name.startswith(katex_prefix) and name.endswith('.ttf')
        }
        details['katexFontCount'] = len(found_katex_fonts)
        if found_katex_fonts != KATEX_FONT_FILES:
            missing = sorted(KATEX_FONT_FILES - found_katex_fonts)
            extra = sorted(found_katex_fonts - KATEX_FONT_FILES)
            violations.append(f'KaTeX 字体不完整: missing={missing}, extra={extra}')
        katex_source = ROOT / 'apps/macos/Resources/Fonts/KaTeX'
        for name in sorted(found_katex_fonts & KATEX_FONT_FILES):
            if zf.read(katex_prefix + name) != (katex_source / name).read_bytes():
                violations.append(f'APK 字体与唯一源文件不一致: {name}')
        for required_asset in (
            f'{katex_prefix}OFL.txt',
            'assets/LICENSE.txt',
            'assets/ThirdPartyNotices.txt',
        ):
            if required_asset not in names:
                violations.append(f'缺少必要资产: {required_asset}')

        if 'assets/ThirdPartyNotices.txt' in names:
            notices_text = zf.read('assets/ThirdPartyNotices.txt').decode('utf-8', errors='replace')
            for component in (
                'uniffi ', 'pulldown-cmark ', 'Rust standard library',
                'ratex-layout ', 'KaTeX fonts', 'SIL OPEN FONT LICENSE',
                'org.jetbrains.kotlin:kotlin-stdlib', 'androidx.compose.ui:ui',
                'androidx.core:core-ktx', 'com.caverock:androidsvg-aar',
                'net.java.dev.jna:jna', 'Apache License, Version 2.0',
            ):
                if component not in notices_text:
                    violations.append(f'ThirdPartyNotices.txt 缺少声明: {component.strip()}')

    aapt2 = find_aapt2(sdk_root)
    badging = subprocess.run(
        [str(aapt2), 'dump', 'badging', str(apk_path)],
        check=True, capture_output=True, text=True,
    ).stdout
    xmltree = subprocess.run(
        [str(aapt2), 'dump', 'xmltree', '--file', 'AndroidManifest.xml', str(apk_path)],
        check=True, capture_output=True, text=True,
    ).stdout

    permissions = re.findall(r"uses-permission: name='([^']+)'", badging)
    details['permissions'] = permissions
    details['debuggable'] = 'application-debuggable' in badging
    debug_manifest_markers = ('DebugSmokeActivity', 'DebugExportActivity', 'DebugPreviewProvider')
    has_debug_manifest = any(marker in xmltree for marker in debug_manifest_markers)
    if details['debuggable'] == release or has_debug_manifest == release:
        violations.append('Manifest 的调试属性或 Debug 组件与构建类型不符')
    if not release and 'android.permission.DUMP' not in xmltree:
        violations.append('DebugSmokeActivity 缺少系统调试权限保护')
    if 'android.permission.INTERNET' in permissions or 'android.permission.INTERNET' in xmltree:
        violations.append('AndroidManifest.xml 声明了禁止的 android.permission.INTERNET 权限')
    if 'android.permission.MANAGE_EXTERNAL_STORAGE' not in permissions:
        violations.append('AndroidManifest.xml 缺少 MANAGE_EXTERNAL_STORAGE 权限')
    if 'android.webkit.' in xmltree:
        violations.append('AndroidManifest.xml 包含 android.webkit.')
    if not badging.startswith(f"package: name='{PACKAGE_NAME}'"):
        violations.append(f'APK 包名与预期 {PACKAGE_NAME} 不一致')
    if not re.search(r"(?m)^(?:minSdkVersion|sdkVersion):'29'$", badging) or "targetSdkVersion:'36'" not in badging:
        violations.append('APK 的 minSdk 或 targetSdk 与预期不符')

    if expect_signer_sha256 or (release and 'unsigned' not in apk_path.name.lower()):
        sig_info, sig_violations = verify_apk_signature(apk_path, sdk_root, expect_signer_sha256)
        details['signature'] = sig_info
        violations.extend(sig_violations)

    details['violations'] = violations
    details['passed'] = len(violations) == 0
    return details


def run_adb(adb: Path, serial: str | None, *args: str, check: bool = True, text: bool = True):
    cmd = [str(adb)]
    if serial:
        cmd += ['-s', serial]
    cmd += list(args)
    return subprocess.run(cmd, check=check, capture_output=True, text=text, timeout=60)


def list_online_devices(adb: Path) -> list[str]:
    out = subprocess.run([str(adb), 'devices'], check=True, capture_output=True, text=True).stdout
    devices = []
    for line in out.splitlines()[1:]:
        parts = line.strip().split()
        if len(parts) >= 2 and parts[1] == 'device':
            devices.append(parts[0])
    return devices


def find_avd(adb: Path, avd_name: str) -> str | None:
    for serial in list_online_devices(adb):
        # 固定演示库路径只用于指定的测试 AVD。
        if not serial.startswith('emulator-'):
            continue
        name = run_adb(adb, serial, 'emu', 'avd', 'name', check=False).stdout.splitlines()
        if name and name[0].strip() == avd_name:
            return serial
    return None


def ensure_emulator(
    sdk_root: Path, avd_name: str, output_dir: Path,
) -> tuple[str, subprocess.Popen | None]:
    adb = sdk_root / 'platform-tools/adb'
    emulator = sdk_root / 'emulator/emulator'
    serial = find_avd(adb, avd_name)
    proc = None
    if not serial:
        print(f'启动 arm64 模拟器 {avd_name}...', flush=True)
        with (output_dir / 'emulator-boot.log').open('w') as log:
            proc = subprocess.Popen(
                [str(emulator), '-avd', avd_name, '-no-window', '-no-boot-anim',
                 '-no-audio', '-no-snapshot-save', '-gpu', 'auto'],
                stdout=log,
                stderr=subprocess.STDOUT,
            )
    try:
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            serial = find_avd(adb, avd_name)
            if serial:
                booted = run_adb(adb, serial, 'shell', 'getprop', 'sys.boot_completed', check=False)
                if booted.stdout.strip() == '1':
                    abi = run_adb(adb, serial, 'shell', 'getprop', 'ro.product.cpu.abi').stdout.strip()
                    if abi != 'arm64-v8a':
                        raise SystemExit(f'模拟器 {avd_name} 的 ABI 为 {abi}')
                    pm = run_adb(adb, serial, 'shell', 'pm', 'path', 'android', check=False)
                    if pm.returncode == 0 and 'package:' in pm.stdout:
                        run_adb(adb, serial, 'shell', 'wm', 'dismiss-keyguard', check=False)
                        return serial, proc
            if proc is not None and proc.poll() is not None:
                raise SystemExit(f'模拟器进程提前退出，返回码 {proc.returncode}，见 emulator-boot.log')
            time.sleep(2)
        raise SystemExit(f'等待模拟器 {avd_name} 启动超时')
    except BaseException:
        if proc is not None and proc.poll() is None:
            proc.terminate()
        raise


def expected_demo_vault_notes() -> set[str]:
    demo_dir = ROOT / 'fixtures/demo-vault'
    return {
        path.relative_to(demo_dir).as_posix()
        for path in demo_dir.rglob('*')
        if path.is_file() and path.suffix.lower() in ('.md', '.mmd')
    }


def parse_meminfo(meminfo_text: str) -> dict:
    result: dict = {}
    for line in meminfo_text.splitlines():
        stripped = line.strip()
        if stripped.startswith('TOTAL PSS:'):
            m = re.search(r'TOTAL PSS:\s+(\d+)\s+TOTAL RSS:\s+(\d+)', stripped)
            if m:
                result['totalPssKb'] = int(m.group(1))
                result['totalRssKb'] = int(m.group(2))
        elif stripped.startswith('TOTAL '):
            parts = stripped.split()
            if len(parts) >= 2 and parts[1].isdigit() and 'totalPssKb' not in result:
                result['totalPssKb'] = int(parts[1])
        elif stripped.startswith('Native Heap'):
            parts = stripped.split()
            if len(parts) >= 3 and parts[2].isdigit():
                result['nativeHeapPssKb'] = int(parts[2])
        elif stripped.startswith('Dalvik Heap'):
            parts = stripped.split()
            if len(parts) >= 3 and parts[2].isdigit():
                result['dalvikHeapPssKb'] = int(parts[2])
    return result


def vault_snapshot(directory: Path) -> dict[str, str | None]:
    return {
        path.relative_to(directory).as_posix():
        hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None
        for path in directory.rglob('*')
    }


def run_emulator_smoke(
    apk_path: Path,
    sdk_root: Path,
    avd_name: str,
    output_dir: Path,
    media_dir: Path,
    keep_emulator: bool,
) -> dict:
    adb = sdk_root / 'platform-tools/adb'
    serial, started_proc = ensure_emulator(sdk_root, avd_name, output_dir)
    night_mode = run_adb(adb, serial, 'shell', 'cmd', 'uimode', 'night').stdout.strip()
    previous_night = night_mode.removeprefix('Night mode: ').strip()
    try:
        device_model = run_adb(adb, serial, 'shell', 'getprop', 'ro.product.model').stdout.strip()
        android_release = run_adb(adb, serial, 'shell', 'getprop', 'ro.build.version.release').stdout.strip()
        sdk_int = run_adb(adb, serial, 'shell', 'getprop', 'ro.build.version.sdk').stdout.strip()
        abi = run_adb(adb, serial, 'shell', 'getprop', 'ro.product.cpu.abi').stdout.strip()

        print(f'设备就绪: {serial} ({device_model}, Android {android_release} API {sdk_int}, {abi})')

        print('安装 Debug APK...')
        install = run_adb(adb, serial, 'install', '-r', '-t', str(apk_path))
        if 'Success' not in install.stdout:
            raise SystemExit(f'APK 安装失败: {install.stdout} {install.stderr}')

        print('授予 MANAGE_EXTERNAL_STORAGE 权限...')
        run_adb(adb, serial, 'shell', 'appops', 'set', PACKAGE_NAME, 'MANAGE_EXTERNAL_STORAGE', 'allow')

        print('推送 fixtures/demo-vault 到共享存储...')
        run_adb(adb, serial, 'shell', 'rm', '-rf', DEVICE_VAULT)
        run_adb(adb, serial, 'shell', 'mkdir', '-p', DEVICE_VAULT)
        demo_dir = ROOT / 'fixtures/demo-vault'
        for item in sorted(demo_dir.iterdir()):
            run_adb(adb, serial, 'push', str(item), f'{DEVICE_VAULT}/')

        remote_json = f'/sdcard/Android/data/{PACKAGE_NAME}/files/{SMOKE_FILENAME}'
        run_adb(adb, serial, 'shell', 'rm', '-f', remote_json)
        run_adb(adb, serial, 'shell', 'cmd', 'uimode', 'night', 'no')
        time.sleep(1)

        print('启动应用执行全量内核烟测...')
        launch = run_adb(
            adb, serial, 'shell', 'am', 'start', '-W', '-S',
            '-n', SMOKE_ACTIVITY,
            '--es', 'vault_path', DEVICE_VAULT,
            '--ez', 'run_smoke', 'true',
            '--es', 'smoke_output', SMOKE_FILENAME,
            check=False,
        )
        (output_dir / 'activity-start.txt').write_text(launch.stdout + launch.stderr)
        if launch.returncode != 0 or 'Status: ok' not in launch.stdout:
            raise SystemExit('启动 DebugSmokeActivity 失败，见 activity-start.txt')

        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            probe = run_adb(adb, serial, 'shell', 'test', '-s', remote_json, check=False)
            if probe.returncode == 0:
                break
            time.sleep(1)
        else:
            logcat = run_adb(adb, serial, 'logcat', '-d', '-t', '300', check=False).stdout
            (output_dir / 'logcat-timeout.txt').write_text(logcat)
            raise SystemExit(f'等待烟测结果 JSON 超时: {remote_json}（日志已保存至 {output_dir / "logcat-timeout.txt"}）')

        # 给 UI 一小段时间完成最终状态渲染再截图与采样内存
        time.sleep(2)
        local_smoke_json = output_dir / 'smoke-result.json'
        run_adb(adb, serial, 'pull', remote_json, str(local_smoke_json))
        smoke_data = json.loads(local_smoke_json.read_text())

        with tempfile.TemporaryDirectory(prefix='hortus-android-') as directory:
            pulled_vault = Path(directory) / 'vault'
            run_adb(adb, serial, 'pull', DEVICE_VAULT, str(pulled_vault))
            local_files = vault_snapshot(demo_dir)
            device_files = vault_snapshot(pulled_vault)
        vault_unchanged = device_files == local_files

        meminfo_raw = run_adb(adb, serial, 'shell', 'dumpsys', 'meminfo', PACKAGE_NAME).stdout
        (output_dir / 'meminfo.txt').write_text(meminfo_raw)
        meminfo = parse_meminfo(meminfo_raw)

        pids_out = run_adb(adb, serial, 'shell', 'pidof', PACKAGE_NAME, check=False).stdout.strip()
        process_count = len([p for p in pids_out.split() if p.isdigit()])

        media_dir.mkdir(parents=True, exist_ok=True)
        light_png = media_dir / 'emulator-smoke-light.png'
        dark_png = media_dir / 'emulator-smoke-dark.png'

        light_bytes = run_adb(adb, serial, 'exec-out', 'screencap', '-p', text=False).stdout
        light_png.write_bytes(light_bytes)

        run_adb(adb, serial, 'shell', 'cmd', 'uimode', 'night', 'yes')
        time.sleep(2)
        dark_bytes = run_adb(adb, serial, 'exec-out', 'screencap', '-p', text=False).stdout
        dark_png.write_bytes(dark_bytes)

        # 逐项断言烟测结果
        assertions: list[str] = []
        expected_notes = expected_demo_vault_notes()
        vault_scan = smoke_data.get('vaultScan', {})
        if not vault_scan.get('passed'):
            assertions.append(f"vaultScan 失败: {vault_scan.get('error')}")
        if (vault_scan.get('noteCount') != len(expected_notes) or
                set(vault_scan.get('notePaths', [])) != expected_notes):
            assertions.append(
                f"vaultScan 笔记路径或数量与 fixtures/demo-vault 的 {len(expected_notes)} 篇笔记不一致"
            )

        diagrams = smoke_data.get('diagrams', {})
        if not diagrams.get('passed'):
            assertions.append(f"diagrams 失败: {diagrams.get('error')}")
        diagram_items = {item['type']: item for item in diagrams.get('items', [])}
        for dtype in EXPECTED_DIAGRAM_TYPES:
            item = diagram_items.get(dtype)
            if not item or not item.get('passed'):
                assertions.append(f"Mermaid {dtype} 渲染失败: {item}")
            elif item.get('width', 0) <= 0 or item.get('height', 0) <= 0 or item.get('primitiveCount', 0) <= 0:
                assertions.append(f"Mermaid {dtype} 场景尺寸或图元数无效: {item}")

        math_res = smoke_data.get('math', {})
        if not math_res.get('passed'):
            assertions.append(f"math 失败: {math_res.get('error')}")
        if math_res.get('displayBytes', 0) <= 8 or math_res.get('textBytes', 0) <= 8:
            assertions.append(f"math 场景载荷过小: {math_res}")

        fonts_res = smoke_data.get('fonts', {})
        if not fonts_res.get('passed') or fonts_res.get('discoveredCount', 0) <= 0:
            assertions.append(f"fonts 桥接失败: {fonts_res.get('error')}")
        if (len(fonts_res.get('widthSamples', [])) < 4 or
                not isinstance(fonts_res.get('maxAbsDiffPx'), (float, int))):
            assertions.append('fonts 缺少完整的中英文宽度样本或偏差')

        atomic_save = smoke_data.get('atomicSave', {})
        if not atomic_save.get('passed'):
            assertions.append(f"atomicSave 失败: {atomic_save.get('error')}")
        if any(atomic_save.get(key) != [] for key in (
            'leftoverTempFiles', 'externalCacheLeftovers', 'sharedStagingLeftovers',
        )):
            assertions.append('atomicSave 有残留文件或缺少清理证据')
        if not all(atomic_save.get(key) for key in (
            'sameDevice', 'contentMatches', 'sourceUnchanged', 'vaultEntriesUnchanged',
        )):
            assertions.append('atomicSave 同卷、文件内容或目录纯净性校验失败')
        if not all(isinstance(atomic_save.get(key), int) for key in ('chmodErrno', 'hardLinkErrno')):
            assertions.append('atomicSave 缺少 chmod 和 hard_link 的实测 errno')
        if not vault_unchanged:
            assertions.append('adb pull 回读的库目录或文件字节与公开演示库不一致')
        if process_count < 1 or meminfo.get('totalPssKb', 0) <= 0:
            assertions.append('缺少应用进程或 PSS 采样')

        return {
            'device': {
                'serial': serial,
                'avd': avd_name,
                'model': device_model,
                'androidRelease': android_release,
                'sdkInt': int(sdk_int) if sdk_int.isdigit() else sdk_int,
                'abi': abi,
            },
            'expectedDemoNoteCount': len(expected_notes),
            'vaultMatchesFixtureAfterSave': vault_unchanged,
            'fixtureFileCount': sum(value is not None for value in local_files.values()),
            'meminfo': meminfo,
            'processCount': process_count,
            'screenshots': [str(light_png), str(dark_png)],
            'smokeResultPath': str(local_smoke_json),
            'smoke': smoke_data,
            'assertions': assertions,
            'passed': len(assertions) == 0 and bool(smoke_data.get('allPassed')),
        }
    finally:
        if previous_night in {'auto', 'no', 'yes', 'custom_schedule', 'custom_bedtime'}:
            run_adb(adb, serial, 'shell', 'cmd', 'uimode', 'night', previous_night, check=False)
        if started_proc is not None and not keep_emulator:
            run_adb(adb, serial, 'emu', 'kill', check=False)
            try:
                started_proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                started_proc.terminate()


def main() -> int:
    parser = argparse.ArgumentParser(description='Android APK 静态审计与 arm64 模拟器烟测')
    parser.add_argument('--apk', type=Path, default=ROOT / 'build/android/hortus-debug.apk')
    parser.add_argument('--avd', default='Medium_Phone_API_36.1')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/android-smoke')
    parser.add_argument('--media', type=Path, default=None)
    parser.add_argument('--static-only', action='store_true')
    parser.add_argument('--release', action='store_true', help='审计 Release APK')
    parser.add_argument('--expect-signer-sha256', default=None, help='校验 APK 签名证书 SHA-256 指纹')
    parser.add_argument('--keep-emulator', action='store_true')
    args = parser.parse_args()
    if args.release and not args.static_only:
        parser.error('--release 需要 --static-only')

    output_dir = args.output.expanduser().resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media.expanduser().resolve() if args.media else output_dir / 'media')

    sdk_root = find_sdk_root()
    static_report = static_audit_apk(
        args.apk.expanduser().resolve(),
        sdk_root,
        args.release,
        args.expect_signer_sha256,
    )
    (output_dir / 'static-audit.json').write_text(json.dumps(static_report, ensure_ascii=False, indent=2) + '\n')

    if not static_report['passed']:
        for v in static_report['violations']:
            print(f'STATIC FAIL: {v}', file=sys.stderr)
        return 1
    print(
        f"STATIC PASS: apk={static_report['apkBytes']}B, so={static_report['soBytes']}B, "
        f"katex={static_report['katexFontCount']}, permissions={static_report['permissions']}"
    )

    summary: dict = {'staticAudit': static_report}
    if not args.static_only:
        try:
            emu_report = run_emulator_smoke(
                args.apk.expanduser().resolve(), sdk_root, args.avd,
                output_dir, media_dir, args.keep_emulator,
            )
        except subprocess.CalledProcessError as error:
            print(f'EMULATOR FAIL: {error}\n{error.stderr}', file=sys.stderr)
            return 1
        summary['emulator'] = emu_report
        (output_dir / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        if not emu_report['passed']:
            for err in emu_report['assertions']:
                print(f'EMULATOR FAIL: {err}', file=sys.stderr)
            return 1
        print(
            f"EMULATOR PASS: notes={emu_report['smoke']['vaultScan']['noteCount']}, "
            f"diagrams=6/6, math=ok, "
            f"maxFontDiffPx={emu_report['smoke']['fonts']['maxAbsDiffPx']:.3f}, "
            f"atomicSave=ok, memPssKb={emu_report['meminfo'].get('totalPssKb')}"
        )
    else:
        (output_dir / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
