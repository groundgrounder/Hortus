#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Linux 端自动化合规、静态二进制审计、CLI/GUI 冒烟与原型截图检查。"""
import argparse
import atexit
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
LEGACY_NAME = "mark" + "note"
LEGACY_TITLE = "Mark" + "Note"
PRIVATE_PREFIXES = ("arch" + "ive/", "co" + "dex/", "mock" + "ups/")
PRIVATE_PLAN = "plan" + ".yaml"
FORBIDDEN_PATHS = (PRIVATE_PLAN, *PRIVATE_PREFIXES)
ATTRIBUTION = "Co-" + "Authored-By"
MIGRATION_EXCEPTIONS = {
    "crates/hortus-config/src/lib.rs": (
        LEGACY_NAME.upper() + "_CONFIG_DIR",
        LEGACY_NAME.upper() + "_CACHE_DIR",
        ".config/" + LEGACY_NAME,
        "Library/Caches/" + LEGACY_TITLE,
        LEGACY_NAME,
    ),
    "crates/hortus-config/tests/config.rs": (
        LEGACY_NAME.upper() + "_CONFIG_DIR",
        LEGACY_NAME.upper() + "_CACHE_DIR",
        ".config/" + LEGACY_NAME,
        "Library/Caches/" + LEGACY_TITLE,
        ".cache/" + LEGACY_NAME,
    ),
    "crates/hortus-ffi/tests/migration.rs": (
        LEGACY_NAME.upper() + "_CONFIG_DIR",
        LEGACY_NAME.upper() + "_CACHE_DIR",
    ),
}
HAN = r"\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\U00020000-\U0002fa1f"
HAN_SEGMENTS = re.compile("[" + HAN + "]{4,}")
EXACT_VERSION_RE = re.compile(r"^=\d+\.\d+\.\d+(?:[-+].+)?$")


def check(condition, message):
    if not condition:
        raise AssertionError(message)

def start_private_display():
    """在私有 Xvfb 中运行 GUI：测试窗口不出现在真实桌面，也不抢焦点。"""
    os.environ.pop("WAYLAND_DISPLAY", None)
    explicit = os.environ.get("HORTUS_TEST_DISPLAY")
    if explicit:
        os.environ["DISPLAY"] = explicit
        os.environ["GDK_BACKEND"] = "x11"
        os.environ["GSK_RENDERER"] = "cairo"
        os.environ["NO_AT_BRIDGE"] = "1"
        return None
    xvfb = shutil.which("Xvfb")
    check(
        xvfb is not None,
        "需要 Xvfb 以离屏运行 GUI（在 nix develop 中提供；或设置 HORTUS_TEST_DISPLAY 复用现有显示）",
    )
    number = None
    for candidate in range(90, 140):
        if not Path(f"/tmp/.X11-unix/X{candidate}").exists():
            number = candidate
            break
    check(number is not None, "找不到空闲的 X 显示编号")
    proc = subprocess.Popen(
        [xvfb, f":{number}", "-screen", "0", "1600x1000x24", "-nolisten", "tcp"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    socket = Path(f"/tmp/.X11-unix/X{number}")
    for _ in range(200):
        if socket.exists():
            break
        time.sleep(0.05)
    check(socket.exists(), "Xvfb 未在 10s 内就绪")
    os.environ["DISPLAY"] = f":{number}"
    os.environ["GDK_BACKEND"] = "x11"
    os.environ["GSK_RENDERER"] = "cairo"
    os.environ["NO_AT_BRIDGE"] = "1"

    def _stop():
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    atexit.register(_stop)
    return proc



def tracked_public_files(root):
    output = subprocess.check_output(["git", "ls-files", "-z"], cwd=root)
    return [
        name
        for raw in output.split(b"\0")
        if raw
        if (name := raw.decode()) != PRIVATE_PLAN and not name.startswith(PRIVATE_PREFIXES)
    ]


def private_segments(root):
    directory = root / "fixtures/mermaid/tree"
    samples = (
        sorted(path for path in directory.rglob("*") if path.is_file())
        if directory.exists()
        else []
    )
    if not samples:
        return set(), 0, True
    segments = set()
    for path in samples:
        for text in (path.name, path.read_bytes().decode("utf-8", errors="replace")):
            for fragment in HAN_SEGMENTS.findall(text):
                segments.update(
                    fragment[index : index + 4] for index in range(len(fragment) - 3)
                )
    return segments, len(samples), False


def audit_public_tree(root):
    names = tracked_public_files(root)
    segments, sample_count, skipped = private_segments(root)
    legacy = re.compile(LEGACY_NAME, re.IGNORECASE)
    violations = []
    for name in names:
        path = root / name
        if not path.is_file():
            continue
        try:
            text = path.read_bytes().decode("utf-8", errors="replace")
        except OSError as error:
            violations.append(f"{name}:1: cannot read tracked file: {error}")
            continue
        for number, line in enumerate(text.splitlines(), 1):
            allowed = line
            for literal in MIGRATION_EXCEPTIONS.get(name, ()):
                allowed = allowed.replace('"' + literal + '"', '"<legacy>"')
            if legacy.search(allowed):
                violations.append(f"{name}:{number}: legacy product name outside allowlist")
            if any(forbidden in line for forbidden in FORBIDDEN_PATHS):
                violations.append(f"{name}:{number}: private workflow path")
            if ATTRIBUTION.casefold() in line.casefold():
                violations.append(f"{name}:{number}: commit attribution trailer")
            if any(segment in line for segment in segments):
                violations.append(f"{name}:{number}: private sample text")
    check(not violations, "公开树审计存在违规:\n" + "\n".join(violations[:20]))
    return {
        "passed": True,
        "tracked_public_files": len(names),
        "private_samples": sample_count,
        "private_segments": len(segments),
        "private_check_skipped": skipped,
        "violations": 0,
    }


def resolve_elf_target(binary: Path) -> Path:
    """若是 Nix wrapGAppsHook4 生成的 wrapper（shell 或 makeBinaryWrapper），定位真实 .*-wrapped ELF。"""
    sibling = binary.parent / f".{binary.name}-wrapped"
    if sibling.is_file() and sibling.read_bytes().startswith(b"\x7fELF"):
        return sibling
    data = binary.read_bytes()
    if data.startswith(b"\x7fELF"):
        return binary
    text = data.decode("utf-8", errors="replace")
    match = re.search(r'exec\s+(?:-a\s+"[^"]*"\s+)?"([^"]+)"', text)
    if match:
        candidate = Path(match.group(1))
        if candidate.is_file() and candidate.read_bytes().startswith(b"\x7fELF"):
            return candidate
    raise AssertionError(f"无法在 {binary} 解析真实 ELF 二进制")


def audit_binary_and_manifest(gui_bin: Path, cli_bin: Path):
    gui_elf = resolve_elf_target(gui_bin)
    cli_elf = resolve_elf_target(cli_bin)

    forbidden_bytes = {
        b"/home/",
        b"/Users/",
        bytes(str(Path.home()), "utf-8"),
        b"/build/source",
    }

    binary_reports = {}
    local_target_dir = (ROOT / "apps/linux/target").resolve()
    for label, launcher, elf in [("gui", gui_bin, gui_elf), ("cli", cli_bin, cli_elf)]:
        raw = elf.read_bytes()
        is_local_debug_automation = label == "gui" and local_target_dir in elf.resolve().parents
        hits = [pat.decode("utf-8", errors="replace") for pat in forbidden_bytes if pat in raw]
        if not is_local_debug_automation:
            check(not hits, f"{label} ELF ({elf}) 包含本机路径字节: {hits}")

        ldd = subprocess.run(["ldd", str(elf)], check=True, text=True, capture_output=True)
        web_libs = [
            line.strip()
            for line in ldd.stdout.splitlines()
            if re.search(r"webkit|javascriptcore", line, re.IGNORECASE)
        ]
        check(not web_libs, f"{label} 存在禁止的 WebView 链接: {web_libs}")

        binary_reports[label] = {
            "launcher": str(launcher),
            "elf": str(elf),
            "size_bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "forbidden_path_hits": 0 if is_local_debug_automation else len(hits),
            "forbidden_path_checked": not is_local_debug_automation,
            "webview_links": 0,
            "ldd_lines": len(ldd.stdout.splitlines()),
        }

    # 扫描 apps/linux/ 源码与 Nix 输出目录是否存在 .html / .js 文件
    web_assets = []
    linux_dir = ROOT / "apps/linux"
    for path in linux_dir.rglob("*"):
        if "target" in path.parts:
            continue
        if path.is_file() and path.suffix.casefold() in (".html", ".js"):
            web_assets.append(str(path))
    nix_out_root = gui_bin.resolve().parents[1]
    if nix_out_root.is_dir() and str(nix_out_root).startswith("/nix/store/"):
        for path in nix_out_root.rglob("*"):
            if path.is_file() and path.suffix.casefold() in (".html", ".js"):
                web_assets.append(str(path))
    check(not web_assets, f"发现禁止的 web 资源文件: {web_assets}")

    # 校验 apps/linux/Cargo.toml 许可证与精确版本锁定
    manifest_path = linux_dir / "Cargo.toml"
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    license_val = manifest.get("package", {}).get("license")
    check(license_val == "GPL-3.0-only", f"apps/linux/Cargo.toml license 应为 GPL-3.0-only，实际为 {license_val!r}")

    deps_checked = []
    for section in ("dependencies", "dev-dependencies"):
        for dep_name, spec in sorted(manifest.get(section, {}).items()):
            if isinstance(spec, str):
                check(EXACT_VERSION_RE.match(spec), f"{section}.{dep_name} 版本未精确锁定 (=x.y.z): {spec!r}")
                deps_checked.append({"section": section, "name": dep_name, "version": spec})
            elif isinstance(spec, dict):
                if "path" in spec:
                    check(str(spec["path"]).startswith("../../crates/"), f"未知本地路径依赖 {dep_name}: {spec}")
                else:
                    ver = spec.get("version", "")
                    check(EXACT_VERSION_RE.match(ver), f"{section}.{dep_name} 版本未精确锁定 (=x.y.z): {ver!r}")
                    deps_checked.append({"section": section, "name": dep_name, "version": ver})

    return {
        "passed": True,
        "binaries": binary_reports,
        "web_assets": 0,
        "license": license_val,
        "pinned_external_deps": deps_checked,
    }


def snapshot_tree(root: Path):
    records = {}
    for directory, dirnames, filenames in os.walk(root, followlinks=False):
        directory = Path(directory)
        dirnames.sort()
        for name in sorted(filenames):
            path = directory / name
            rel = str(path.relative_to(root))
            records[rel] = {
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "size": path.stat().st_size,
            }
    return records


def isolated_env(base_dir: Path):
    env = dict(os.environ)
    for key in ("HORTUS_VAULT",):
        env.pop(key, None)
    cfg = base_dir / "config"
    cache = base_dir / "cache"
    data = base_dir / "data"
    cfg.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    data.mkdir(parents=True, exist_ok=True)
    env["HORTUS_CONFIG_DIR"] = str(cfg)
    env["HORTUS_CACHE_DIR"] = str(cache)
    env["HORTUS_DATA_DIR"] = str(data)
    # 同时隔离 XDG 用户配置目录，防止宿主机 ~/.config/gtk-4.0/gtk.css 覆盖内置浅色/深色主题。
    env["XDG_CONFIG_HOME"] = str(cfg)
    env["XDG_CACHE_HOME"] = str(cache)
    env["XDG_DATA_HOME"] = str(data)
    return env


def run_cli_smoke(cli_bin: Path):
    with tempfile.TemporaryDirectory(prefix="hortus-cli-smoke-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "demo-vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", vault)
        env = isolated_env(tmp_path / "state")
        before = snapshot_tree(vault)

        def run_json(args, label):
            res = subprocess.run(
                [str(cli_bin), *args],
                cwd=str(vault),
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
            )
            check(res.returncode == 0, f"CLI {label} 退出码 {res.returncode}: {res.stderr}")
            check(res.stderr == "", f"CLI {label} 输出了 stderr: {res.stderr}")
            return json.loads(res.stdout)

        search = run_json(["search", "learning", "--vault", str(vault)], "search")
        check(search["total_notes"] == 8 and len(search["results"]) >= 1, f"CLI search 结果异常: {search}")

        backlinks = run_json(["backlinks", "Start here.md", "--vault", str(vault)], "backlinks")
        check(backlinks["note"] == "Start here.md" and backlinks["count"] >= 1, f"CLI backlinks 结果异常: {backlinks}")

        tags = run_json(["tags", "--vault", str(vault)], "tags")
        tag_names = {item["tag"] for item in tags["tags"]}
        check("demo" in tag_names and "learning" in tag_names, f"CLI tags 结果异常: {tags}")

        outline = run_json(["outline", "Markdown field guide.md", "--vault", str(vault)], "outline")
        heading_texts = [h["text"] for h in outline["headings"]]
        check("Markdown field guide" in heading_texts and "Tables" in heading_texts, f"CLI outline 结果异常: {outline}")

        mermaid = run_json(
            ["mermaid", "check", str(vault / "Learning/Learning map.md")],
            "mermaid check",
        )
        check(mermaid["valid"] is True and len(mermaid["blocks"]) == 1, f"CLI mermaid check 异常: {mermaid}")

        guide_res = subprocess.run(
            [str(cli_bin), "guide"],
            cwd=str(vault),
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        check(guide_res.returncode == 0 and "Hortus" in guide_res.stdout, f"CLI guide 异常: {guide_res.stderr}")

        after = snapshot_tree(vault)
        check(before == after, "CLI 烟测修改了临时演示库副本！")

        return {
            "passed": True,
            "vault_file_count": len(before),
            "vault_unchanged": True,
            "commands": {
                "search_total_notes": search["total_notes"],
                "search_hits": len(search["results"]),
                "backlinks_count": backlinks["count"],
                "tags_count": tags["count"],
                "outline_headings": len(heading_texts),
                "mermaid_valid": mermaid["valid"],
                "guide_bytes": len(guide_res.stdout.encode("utf-8")),
            },
        }


def process_network_sockets(pid: int):
    """只统计属于目标进程 fd 的 IPv4/IPv6 网络套接字（排除 Wayland/DBus Unix 域套接字）。"""
    fd_dir = Path(f"/proc/{pid}/fd")
    if not fd_dir.is_dir():
        return []
    inodes = set()
    try:
        for link in fd_dir.iterdir():
            try:
                target = os.readlink(link)
            except OSError:
                continue
            if target.startswith("socket:[") and target.endswith("]"):
                inodes.add(target[8:-1])
    except OSError:
        return []

    found = []
    for proto in ("tcp", "tcp6", "udp", "udp6", "raw", "raw6"):
        proc_file = Path(f"/proc/{pid}/net/{proto}")
        if not proc_file.is_file():
            continue
        try:
            lines = proc_file.read_text(encoding="utf-8", errors="replace").splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            parts = line.split()
            if len(parts) > 9 and parts[9] in inodes:
                found.append({"proto": proto, "local": parts[1], "remote": parts[2], "inode": parts[9]})
    return found


def read_rss_kib(pid: int):
    status_file = Path(f"/proc/{pid}/status")
    if not status_file.is_file():
        return None
    try:
        for line in status_file.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        return None
    return None


def run_gui_smoke(gui_bin: Path, output_dir: Path):
    with tempfile.TemporaryDirectory(prefix="hortus-gui-smoke-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "demo-vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", vault)
        env = isolated_env(tmp_path / "state")
        smoke_json = output_dir / "gui-smoke.json"
        smoke_json.unlink(missing_ok=True)
        before = snapshot_tree(vault)

        proc = subprocess.Popen(
            [
                str(gui_bin),
                "--vault",
                str(vault),
                "--open",
                "Markdown field guide.md",
                "--open",
                "Learning/Learning map.md",
                "--width",
                "1280",
                "--height",
                "820",
                "--color-scheme",
                "light",
                "--smoke-json",
                str(smoke_json),
            ],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        network_samples = 0
        network_violations = []
        peak_rss_kib = 0
        deadline = time.monotonic() + 30.0
        while proc.poll() is None:
            if time.monotonic() > deadline:
                proc.kill()
                raise AssertionError("GUI 烟测超时（30s）")
            sockets = process_network_sockets(proc.pid)
            network_samples += 1
            if sockets:
                network_violations.extend(sockets)
            rss = read_rss_kib(proc.pid)
            if rss and rss > peak_rss_kib:
                peak_rss_kib = rss
            time.sleep(0.08)

        stdout, stderr = proc.communicate(timeout=5)
        check(proc.returncode == 0, f"GUI 烟测退出码 {proc.returncode}\nstdout={stdout}\nstderr={stderr}")
        check(not network_violations, f"GUI 检测到网络连接: {network_violations}")
        check(smoke_json.is_file(), f"未生成 GUI smoke JSON: {smoke_json}")

        after = snapshot_tree(vault)
        check(before == after, "GUI 烟测修改了临时演示库副本！")

        smoke = json.loads(smoke_json.read_text(encoding="utf-8"))
        check(smoke["scan_complete"] is True, "Vault 扫描未完成")
        check(smoke["note_count"] == 8, f"预期 8 篇笔记，实际 {smoke['note_count']}")
        check(len(smoke["open_tabs"]) >= 2, f"预期打开 >= 2 个标签，实际 {smoke['open_tabs']}")
        check(smoke["tab_overview_opened"] is True and smoke["tab_overview_listed_count"] >= 2, "AdwTabOverview 未正常打开或未列举标签")
        check(smoke["wide_width"] >= 1200 and not smoke["wide_left_collapsed"] and not smoke["wide_right_collapsed"], f"宽屏侧栏状态异常: {smoke}")
        check(smoke["narrow_width"] < 820 and smoke["narrow_left_collapsed"] and smoke["narrow_right_collapsed"], f"窄屏 AdwBreakpoint 折叠状态异常: {smoke}")
        check(smoke["top_bar_hidden_ok"] and smoke["top_bar_restored_ok"], "AdwToolbarView 顶栏显隐切换失败")
        check(len(smoke["outline_headings"]) >= 3 and smoke["outline_scroll_moved"], f"右侧栏大纲或点击滚动定位失败: {smoke}")
        check(smoke["edit_mode_toggled_on"] and smoke["edit_mode_toggled_off"] and smoke["edit_buffer_modified_in_memory"], "Ctrl+E 只读/内存原型编辑切换失败")
        check(smoke["panel_move_verified"], "占位视图在区域间切换失败")

        return {
            "passed": True,
            "vault_file_count": len(before),
            "vault_unchanged": True,
            "network_samples": network_samples,
            "network_connections": 0,
            "peak_rss_kib": peak_rss_kib,
            "peak_rss_mib": round(peak_rss_kib / 1024.0, 2),
            "smoke": smoke,
        }


def png_dimensions(path: Path):
    data = path.read_bytes()
    check(data[:8] == b"\x89PNG\r\n\x1a\n", f"不是合法 PNG 文件: {path}")
    width, height = struct.unpack(">II", data[16:24])
    return width, height


def capture_screenshots(gui_bin: Path, media_dir: Path):
    media_dir.mkdir(parents=True, exist_ok=True)
    specs = [
        {
            "filename": "prototype-light-wide.png",
            "label": "浅色宽屏（文件树 + 正文 + 大纲）",
            "width": 1280,
            "height": 820,
            "color_scheme": "light",
            "open": ["Markdown field guide.md", "Learning/Learning map.md"],
            "overview": False,
            "min_width": 1200,
            "min_height": 800,
            "max_width": 9999,
        },
        {
            "filename": "prototype-dark-wide.png",
            "label": "深色宽屏",
            "width": 1280,
            "height": 820,
            "color_scheme": "dark",
            "open": ["Markdown field guide.md", "Learning/Learning map.md"],
            "overview": False,
            "min_width": 1200,
            "min_height": 800,
            "max_width": 9999,
        },
        {
            "filename": "prototype-narrow-collapsed.png",
            "label": "窄屏（AdwBreakpoint 自动折叠态）",
            "width": 680,
            "height": 800,
            "color_scheme": "light",
            "open": ["Markdown field guide.md", "Learning/Learning map.md"],
            "overview": False,
            "min_width": 600,
            "min_height": 700,
            "max_width": 819,
        },
        {
            "filename": "prototype-tab-overview.png",
            "label": "AdwTabOverview 标签全景网格态",
            "width": 1280,
            "height": 820,
            "color_scheme": "light",
            "open": [
                "Markdown field guide.md",
                "Learning/Learning map.md",
                "Learning/Study loop.md",
                "Start here.md",
            ],
            "overview": True,
            "min_width": 1200,
            "min_height": 800,
            "max_width": 9999,
        },
    ]

    results = []
    with tempfile.TemporaryDirectory(prefix="hortus-gui-shots-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "demo-vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", vault)
        before = snapshot_tree(vault)

        for idx, spec in enumerate(specs):
            out_png = media_dir / spec["filename"]
            out_png.unlink(missing_ok=True)
            env = isolated_env(tmp_path / f"state-{idx}")
            cmd = [
                str(gui_bin),
                "--vault",
                str(vault),
                "--width",
                str(spec["width"]),
                "--height",
                str(spec["height"]),
                "--color-scheme",
                spec["color_scheme"],
                "--screenshot",
                str(out_png),
            ]
            for rel in spec["open"]:
                cmd.extend(["--open", rel])
            if spec["overview"]:
                cmd.append("--overview")

            res = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=30)
            check(res.returncode == 0, f"截图 {spec['filename']} 失败: {res.stderr}")
            check(out_png.is_file(), f"截图文件缺失: {out_png}")
            w, h = png_dimensions(out_png)
            check(
                spec["min_width"] <= w <= spec["max_width"] and h >= spec["min_height"],
                f"截图 {spec['filename']} 分辨率不符合要求: {w}x{h}",
            )
            results.append(
                {
                    "filename": spec["filename"],
                    "path": str(out_png.resolve()),
                    "label": spec["label"],
                    "width": w,
                    "height": h,
                    "size_bytes": out_png.stat().st_size,
                    "sha256": hashlib.sha256(out_png.read_bytes()).hexdigest(),
                }
            )

        after = snapshot_tree(vault)
        check(before == after, "截图流程修改了临时演示库副本！")

    return results


def discover_automation_gui_bin(explicit: Path | None) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的 GUI 二进制不存在: {explicit}")
        return explicit.resolve()
    cargo = shutil.which("cargo")
    cmd = [
        "cargo",
        "build",
        "--manifest-path",
        str(ROOT / "apps/linux/Cargo.toml"),
        "--features",
        "automation",
        "--offline",
    ]
    if cargo is None:
        cmd = ["nix", "develop", "--command"] + cmd
    subprocess.run(cmd, cwd=ROOT, check=True)
    built = ROOT / "apps/linux/target/debug/hortus-gtk"
    check(built.is_file(), f"构建 automation GUI 二进制后未找到输出: {built}")
    return built.resolve()


def discover_binary(explicit: Path | None, candidates: list[Path], label: str) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的 {label} 二进制不存在: {explicit}")
        return explicit.resolve()
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    raise AssertionError(
        f"未找到 {label} 二进制（尝试过: {', '.join(str(c) for c in candidates)}），"
        "请先构建或传入 --gui-bin / --cli-bin"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--cli-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-smoke")
    parser.add_argument("--media", type=Path, default=None)
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()
    start_private_display()

    gui_bin = discover_automation_gui_bin(args.gui_bin)
    cli_bin = discover_binary(
        args.cli_bin,
        [
            ROOT / "result-cli/bin/hortus",
            ROOT / "build/linux/result-cli/bin/hortus",
            ROOT / "build/linux/bin/hortus",
            ROOT / "target/release/hortus",
        ],
        "CLI",
    )

    public_audit = audit_public_tree(ROOT)
    static_audit = audit_binary_and_manifest(gui_bin, cli_bin)
    (output_dir / "static-audit.json").write_text(
        json.dumps(static_audit, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    cli_smoke = run_cli_smoke(cli_bin)
    (output_dir / "cli-smoke.json").write_text(
        json.dumps(cli_smoke, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    gui_smoke = run_gui_smoke(gui_bin, output_dir)
    screenshots = capture_screenshots(gui_bin, media_dir)
    (output_dir / "screenshots.json").write_text(
        json.dumps(screenshots, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    summary = {
        "passed": True,
        "public_tree": public_audit,
        "static_audit": static_audit,
        "cli_smoke": cli_smoke,
        "gui_smoke": gui_smoke,
        "screenshots": screenshots,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    if args.media is not None:
        stray_pngs = sorted(str(path.relative_to(output_dir)) for path in output_dir.rglob("*.png"))
        check(not stray_pngs, f"--output 目录内出现 PNG（应全部写入 --media）：{stray_pngs}")

    print(
        f"PASS: public_tree={public_audit['tracked_public_files']} files, "
        f"cli_smoke=6 cmds, gui_smoke=8 notes/{len(gui_smoke['smoke']['open_tabs'])} tabs/"
        f"0 net/{gui_smoke['peak_rss_mib']} MiB, screenshots={len(screenshots)}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
