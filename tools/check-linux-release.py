#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Hortus Linux L4 生产发布态验收脚本（仅测无 automation 特性的真实发行包）。

覆盖 D5 全量验收项：
1. 生产 CLI `--help`（en / zh-Hans）与拒绝调试参数检查；
2. 真实 ELF（含 Nix wrapper 解包）、`ldd`（零 WebView、链接 libvte-gtk4）、零用户路径残留检查；
3. 安装前缀布局（.desktop / metainfo.xml / SVG / LICENSE / THIRD_PARTY_NOTICES.txt / 20 KaTeX TTF / 零 html/js）；
4. 源码（excl. i18n.rs / automation / tests）静态审计：零裸硬编码中文界面字面量、零禁用工程术语；
5. 调用 `tools/check-public-tree.py` 与 `tools/linux-third-party-notices.py --check`；
6. 私有 `Xvfb -fbdir` + 私有 `dbus-daemon` 下 `en_US.UTF-8` 与 `zh_CN.UTF-8` 双语真实运行、5 次冷启动中位数、RSS、15s 静息 CPU 增量（vs 空窗基线）、零网络套接字、closure/tarball 体积与 SHA-256；
7. 采集 4 张生产公开验收截图到 `--media`。
"""
from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
import zlib

ROOT = Path(__file__).resolve().parents[1]

AUTOMATION_FLAGS = (
    "--smoke-json",
    "--reading-json",
    "--edit-knowledge-json",
    "--wikilink-json",
    "--layout-width-json",
    "--layout-hide-sidebars",
    "--live-resize-json",
    "--terminal-agent-json",
    "--restore-session-json",
    "--matrix-audit-json",
    "--fuzz-json",
    "--fuzz-seed",
    "--fuzz-steps",
    "--fuzz-media",
    "--screenshot",
    "--quit-after-ms",
    "--overview",
    "--hide-top-bar",
    "--edit",
    "--focus-diagram",
    "--scroll-fraction",
)

FORBIDDEN_UI_TOKENS = (
    "知识库",
    "笔记库",
    "智能体",
    "逐行 diff",
    "单 hunk",
    "shell 拆分",
    "review_stale",
    "VTE4",
)


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def find_gdbus() -> str:
    explicit = shutil.which("gdbus")
    if explicit:
        return explicit
    for cand in sorted(Path("/nix/store").glob("*-glib-*-bin/bin/gdbus")):
        if cand.is_file():
            return str(cand)
    raise AssertionError("未找到 gdbus（需要 glib bin 以触发 DBus 窗口动作）")


def discover_binary(explicit: Path | None, candidates: list[Path], label: str) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的 {label} 二进制不存在: {explicit}")
        return explicit.resolve()
    for cand in candidates:
        if cand.is_file():
            return cand.resolve()
    raise AssertionError(f"未找到 {label} 生产二进制，请先执行 nix build 或传入显式路径")


def resolve_elf_target(binary: Path) -> Path:
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
    raise AssertionError(f"无法在 {binary} 解析真实 ELF 目标")


def audit_cli_production(gui_bin: Path) -> dict[str, object]:
    results = {}
    for locale, expected_word, expected_flag in (
        ("en_US.UTF-8", "Usage:", "--vault <dir>"),
        ("zh_CN.UTF-8", "用法：", "--vault <目录>"),
    ):
        env = dict(os.environ)
        env["LC_ALL"] = locale
        res = subprocess.run([str(gui_bin), "--help"], env=env, capture_output=True, text=True, check=True)
        check(expected_word in res.stdout, f"{locale} --help 缺少 {expected_word!r}:\n{res.stdout}")
        check(expected_flag in res.stdout, f"{locale} --help 缺少 {expected_flag!r}:\n{res.stdout}")
        for flag in AUTOMATION_FLAGS:
            check(flag not in res.stdout, f"生产 --help 泄露自动化参数 {flag}:\n{res.stdout}")
        results[locale] = {"lines": len(res.stdout.splitlines()), "len": len(res.stdout)}

    bad = subprocess.run(
        [str(gui_bin), "--smoke-json", "/tmp/should-fail.json"],
        capture_output=True,
        text=True,
    )
    check(bad.returncode != 0, "生产二进制不应接受 --smoke-json 参数")
    results["reject_automation_flag"] = True
    return results


def audit_elf_and_prefix(gui_bin: Path, cli_bin: Path) -> dict[str, object]:
    gui_elf = resolve_elf_target(gui_bin)
    cli_elf = resolve_elf_target(cli_bin)
    forbidden_bytes = {
        b"/home/",
        b"/Users/",
        bytes(str(Path.home()), "utf-8"),
        b"/build/source",
    }

    elf_info = {}
    for label, launcher, elf in (("gui", gui_bin, gui_elf), ("cli", cli_bin, cli_elf)):
        raw = elf.read_bytes()
        hits = [pat.decode("utf-8", errors="replace") for pat in forbidden_bytes if pat in raw]
        check(not hits, f"{label} ELF ({elf}) 包含本机用户路径字节: {hits}")
        ldd_out = subprocess.check_output(["ldd", str(elf)], text=True, stderr=subprocess.STDOUT)
        web_links = [
            line.strip()
            for line in ldd_out.splitlines()
            if re.search(r"webkit|javascriptcore", line, re.IGNORECASE)
        ]
        check(not web_links, f"{label} 存在禁止的 WebKit 链接: {web_links}")
        if label == "gui":
            check("libvte-2.91-gtk4.so.0" in ldd_out, f"hortus-gtk 未链接 libvte-2.91-gtk4.so.0:\n{ldd_out}")
        elf_info[label] = {
            "launcher": str(launcher),
            "elf": str(elf),
            "size_bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "ldd_libs": len(ldd_out.splitlines()),
        }

    # 检查 prefix 安装布局与禁含 .html / .js
    prefix_root = gui_bin.resolve().parents[1]
    nix_closure_bytes = None
    if str(prefix_root).startswith("/nix/store/"):
        required_files = [
            "share/applications/io.github.groundgrounder.Hortus.desktop",
            "share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml",
            "share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg",
            "share/licenses/hortus-linux/LICENSE",
            "share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt",
            "share/hortus/fonts/KaTeX/OFL.txt",
            "share/hortus/fonts/KaTeX/FONT_NOTICE.txt",
        ]
        for rel in required_files:
            check((prefix_root / rel).is_file(), f"Nix 安装输出缺少必需文件: {prefix_root / rel}")
        katex_ttfs = sorted((prefix_root / "share/hortus/fonts/KaTeX").glob("KaTeX_*.ttf"))
        check(len(katex_ttfs) == 20, f"Nix 安装输出 KaTeX TTF 数量应为 20，实际 {len(katex_ttfs)}")

        desktop_txt = (prefix_root / "share/applications/io.github.groundgrounder.Hortus.desktop").read_text(encoding="utf-8")
        check("GenericName[zh_CN]=" in desktop_txt and "Comment[zh_CN]=" in desktop_txt, ".desktop 缺少双语字段")
        for tok in ("知识库", "Vault", "智能体"):
            check(tok not in desktop_txt, f".desktop 含禁用文案 {tok!r}")

        meta_xml = (prefix_root / "share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml").read_text(encoding="utf-8")
        root_xml = ET.fromstring(meta_xml)
        summaries = root_xml.findall("summary")
        langs = {s.attrib.get("{http://www.w3.org/XML/1998/namespace}lang", "en") for s in summaries}
        check("en" in langs and "zh-Hans" in langs, f"metainfo.xml 缺少 en/zh-Hans summary: {langs}")
        rel_el = root_xml.find("./releases/release")
        check(rel_el is not None and rel_el.attrib.get("version") == "0.5.0", "metainfo.xml 缺少 0.5.0 release")

        web_in_prefix = [
            str(p) for p in prefix_root.rglob("*") if p.is_file() and p.suffix.casefold() in (".html", ".js")
        ]
        check(not web_in_prefix, f"Nix 安装输出发现禁止的 html/js 文件: {web_in_prefix}")

        try:
            pi = subprocess.check_output(["nix", "path-info", "-S", str(prefix_root)], text=True, stderr=subprocess.DEVNULL)
            parts = pi.strip().split()
            if parts and parts[-1].isdigit():
                nix_closure_bytes = int(parts[-1])
        except (OSError, subprocess.SubprocessError):
            pass

    web_in_src = [
        str(p)
        for p in (ROOT / "apps/linux").rglob("*")
        if "target" not in p.parts and p.is_file() and p.suffix.casefold() in (".html", ".js")
    ]
    check(not web_in_src, f"apps/linux 源码树出现禁止的 html/js 文件: {web_in_src}")

    return {
        "binaries": elf_info,
        "prefix_root": str(prefix_root),
        "nix_closure_bytes": nix_closure_bytes,
    }


def strip_comments_rust(source: str) -> list[tuple[int, str]]:
    """剥离行注释/块注释并在 #[cfg(test)] 处停止，返回 (1-based line_no, text) 列表。"""
    out: list[tuple[int, str]] = []
    in_block = False
    for line_no, line in enumerate(source.splitlines(), 1):
        trimmed = line.strip()
        if trimmed == "#[cfg(test)]":
            break
        res = []
        i = 0
        n = len(line)
        in_str = False
        while i < n:
            if in_block:
                if line.startswith("*/", i):
                    in_block = False
                    i += 2
                else:
                    i += 1
            elif in_str:
                res.append(line[i])
                if line[i] == "\\" and i + 1 < n:
                    res.append(line[i + 1])
                    i += 2
                elif line[i] == '"':
                    in_str = False
                    i += 1
                else:
                    i += 1
            else:
                if line.startswith("//", i):
                    break
                if line.startswith("/*", i):
                    in_block = True
                    i += 2
                else:
                    if line[i] == '"':
                        in_str = True
                    res.append(line[i])
                    i += 1
        out.append((line_no, "".join(res)))
    return out


def audit_i18n_and_copy() -> dict[str, object]:
    src_root = ROOT / "apps/linux/src"
    bare_chinese = []
    forbidden_hits = []
    for path in sorted(src_root.rglob("*.rs")):
        rel = path.relative_to(src_root).as_posix()
        if rel == "i18n.rs" or rel.startswith("automation/"):
            continue
        text = path.read_text(encoding="utf-8")
        for line_no, code in strip_comments_rust(text):
            for lit in re.findall(r'"(?:\\.|[^"\\])*"', code):
                if re.search(r"[\u4e00-\u9fff]", lit):
                    bare_chinese.append(f"{rel}:{line_no}: {lit}")
                for tok in FORBIDDEN_UI_TOKENS:
                    if tok == "review_stale" and lit == '"history.review_stale"':
                        continue
                    if tok in lit:
                        forbidden_hits.append(f"{rel}:{line_no}: {tok} in {lit}")
                if re.search(r"\bVault\b", lit) and "--vault" not in lit:
                    forbidden_hits.append(f"{rel}:{line_no}: Vault in {lit}")
    check(not bare_chinese, "i18n.rs 之外发现裸硬编码中文字符串:\n" + "\n".join(bare_chinese[:20]))
    check(not forbidden_hits, "源码字面量发现禁用术语:\n" + "\n".join(forbidden_hits[:20]))

    # 校验 i18n.rs 中不允许出现的术语（允许注释与 #[cfg(test)] 中的校验测试本身）
    i18n_text = (src_root / "i18n.rs").read_text(encoding="utf-8")
    i18n_forbidden = []
    for line_no, code in strip_comments_rust(i18n_text):
        for lit in re.findall(r'"(?:\\.|[^"\\])*"', code):
            for tok in FORBIDDEN_UI_TOKENS:
                if tok in lit:
                    i18n_forbidden.append(f"i18n.rs:{line_no}: {tok} in {lit}")
            if re.search(r"\bVault\b", lit) and "--vault" not in lit:
                i18n_forbidden.append(f"i18n.rs:{line_no}: Vault in {lit}")
    check(not i18n_forbidden, "i18n.rs 翻译词条含禁用文案:\n" + "\n".join(i18n_forbidden[:20]))

    prev_bytecode = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        spec = importlib.util.spec_from_file_location("check_public_tree", ROOT / "tools/check-public-tree.py")
        check_public_tree = importlib.util.module_from_spec(spec)
        assert spec and spec.loader
        spec.loader.exec_module(check_public_tree)
    finally:
        sys.dont_write_bytecode = prev_bytecode
    tracked = check_public_tree.tracked_public_files(ROOT)
    segments = check_public_tree.private_segments(ROOT)
    public_violations = []
    for name in tracked:
        if name.endswith(".pyc") or "__pycache__/" in name:
            continue
        text = (ROOT / name).read_bytes().decode("utf-8", errors="replace")
        public_violations.extend(check_public_tree.scan_text(name, text, segments))
    check(not public_violations, f"check-public-tree 公开仓库检查失败: {public_violations[:10]}")
    subprocess.run([sys.executable, str(ROOT / "tools/linux-third-party-notices.py"), "--check"], cwd=ROOT, check=True)

    window_lines = {}
    for p in sorted((src_root / "window").glob("*.rs")):
        cnt = len(p.read_text(encoding="utf-8").splitlines())
        check(cnt <= 800, f"window 模块单文件超过 800 行: {p.name} ({cnt})")
        window_lines[p.name] = cnt

    return {
        "bare_chinese_outside_i18n": 0,
        "forbidden_tokens": 0,
        "public_tree_ok": True,
        "notices_check_ok": True,
        "max_window_lines": max(window_lines.values()),
    }


def read_proc_cpu_ticks(pid: int) -> int:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8", errors="replace")
    rparen = stat.rfind(")")
    fields = stat[rparen + 2 :].split()
    return int(fields[11]) + int(fields[12])


def read_rss_kib(pid: int) -> int | None:
    status_file = Path(f"/proc/{pid}/status")
    if not status_file.is_file():
        return None
    for line in status_file.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return None


def process_network_sockets(pid: int) -> list[dict[str, str]]:
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
        pf = Path(f"/proc/{pid}/net/{proto}")
        if not pf.is_file():
            continue
        for line in pf.read_text(encoding="utf-8", errors="replace").splitlines()[1:]:
            parts = line.split()
            if len(parts) > 9 and parts[9] in inodes:
                found.append({"proto": proto, "local": parts[1], "remote": parts[2]})
    return found


class X11DisplayHelper:
    def __init__(self, display_str: str) -> None:
        lib_candidates = []
        found = ctypes.util.find_library("X11")
        if found:
            lib_candidates.append(found)
        lib_candidates.extend([
            "libX11.so.6",
            "/usr/lib/x86_64-linux-gnu/libX11.so.6",
            "/usr/lib64/libX11.so.6",
            "/usr/lib/libX11.so.6",
        ])
        lib_candidates.extend(str(p) for p in sorted(Path("/nix/store").glob("*-lib[xX]11-*/lib/libX11.so.6")))
        self.x11 = None
        last_err: OSError | None = None
        for cand in lib_candidates:
            try:
                self.x11 = ctypes.cdll.LoadLibrary(cand)
                break
            except OSError as exc:
                last_err = exc
        if self.x11 is None:
            raise OSError(f"无法加载 libX11.so.6: {last_err}")
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x11.XOpenDisplay.restype = ctypes.c_void_p
        self.x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.x11.XDefaultRootWindow.restype = ctypes.c_ulong
        self.x11.XQueryTree.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
            ctypes.POINTER(ctypes.c_uint),
        ]
        self.x11.XQueryTree.restype = ctypes.c_int
        self.x11.XFree.argtypes = [ctypes.c_void_p]

        class XWindowAttributes(ctypes.Structure):
            _fields_ = [
                ("x", ctypes.c_int),
                ("y", ctypes.c_int),
                ("width", ctypes.c_int),
                ("height", ctypes.c_int),
                ("border_width", ctypes.c_int),
                ("depth", ctypes.c_int),
                ("visual", ctypes.c_void_p),
                ("root", ctypes.c_ulong),
                ("c_class", ctypes.c_int),
                ("bit_gravity", ctypes.c_int),
                ("win_gravity", ctypes.c_int),
                ("backing_store", ctypes.c_int),
                ("backing_planes", ctypes.c_ulong),
                ("backing_pixel", ctypes.c_ulong),
                ("save_under", ctypes.c_int),
                ("colormap", ctypes.c_ulong),
                ("map_installed", ctypes.c_int),
                ("map_state", ctypes.c_int),
                ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long),
                ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int),
                ("screen", ctypes.c_void_p),
            ]

        self.XWindowAttributes = XWindowAttributes
        self.x11.XGetWindowAttributes.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(XWindowAttributes),
        ]
        self.x11.XGetWindowAttributes.restype = ctypes.c_int
        self.dpy = self.x11.XOpenDisplay(display_str.encode("utf-8"))
        check(bool(self.dpy), f"XOpenDisplay({display_str}) 失败")
        self.root = self.x11.XDefaultRootWindow(self.dpy)

    def find_mapped_toplevel(self) -> tuple[int, int, int, int] | None:
        root_ret = ctypes.c_ulong()
        parent_ret = ctypes.c_ulong()
        children = ctypes.POINTER(ctypes.c_ulong)()
        nchildren = ctypes.c_uint()
        if not self.x11.XQueryTree(
            self.dpy,
            self.root,
            ctypes.byref(root_ret),
            ctypes.byref(parent_ret),
            ctypes.byref(children),
            ctypes.byref(nchildren),
        ):
            return None
        try:
            best: tuple[int, int, int, int] | None = None
            for i in range(nchildren.value):
                win = children[i]
                attrs = self.XWindowAttributes()
                if self.x11.XGetWindowAttributes(self.dpy, win, ctypes.byref(attrs)):
                    # IsViewable == 2
                    if attrs.map_state == 2 and attrs.width >= 300 and attrs.height >= 200:
                        if best is None or attrs.width * attrs.height > best[2] * best[3]:
                            best = (attrs.x, attrs.y, attrs.width, attrs.height)
            return best
        finally:
            if children:
                self.x11.XFree(children)

    def close(self) -> None:
        if self.dpy:
            self.x11.XCloseDisplay(self.dpy)
            self.dpy = None


def capture_xwd_png(xwd_path: Path, out_png: Path, crop: tuple[int, int, int, int] | None) -> tuple[int, int]:
    data = xwd_path.read_bytes()
    fields = struct.unpack(">25I", data[:100])
    header_size = fields[0]
    file_version = fields[1]
    check(file_version == 7, f"不支持的 XWD 版本: {file_version}")
    byte_order = fields[7]
    pix_w = fields[4]
    pix_h = fields[5]
    bpp = fields[11]
    bytes_per_line = fields[12]
    ncolors = fields[19]
    check(bpp == 32, f"仅支持 32bpp XWD，实际 {bpp}")
    px_offset = header_size + ncolors * 12
    px_buf = data[px_offset:]

    cx, cy, cw, ch = crop if crop is not None else (0, 0, pix_w, pix_h)
    cx = max(0, cx)
    cy = max(0, cy)
    cw = min(cw, pix_w - cx)
    ch = min(ch, pix_h - cy)

    rows = bytearray()
    for y in range(cy, cy + ch):
        rows.append(0)
        row_start = y * bytes_per_line + cx * 4
        row = px_buf[row_start : row_start + cw * 4]
        if byte_order == 0:  # LSBFirst: B G R X -> R G B
            rgb = bytearray(cw * 3)
            rgb[0::3] = row[2::4]
            rgb[1::3] = row[1::4]
            rgb[2::3] = row[0::4]
            rows.extend(rgb)
        else:  # MSBFirst: X R G B -> R G B
            rgb = bytearray(cw * 3)
            rgb[0::3] = row[1::4]
            rgb[1::3] = row[2::4]
            rgb[2::3] = row[3::4]
            rows.extend(rgb)

    def png_chunk(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", cw, ch, 8, 2, 0, 0, 0)
    idat = zlib.compress(bytes(rows), 6)
    png = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr) + png_chunk(b"IDAT", idat) + png_chunk(b"IEND", b"")
    out_png.parent.mkdir(parents=True, exist_ok=True)
    out_png.write_bytes(png)
    return cw, ch


def wait_for_window_map(helper: X11DisplayHelper, proc: subprocess.Popen[str], timeout_s: float = 12.0) -> tuple[int, int, int, int]:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            err_txt = proc.stderr.read() if proc.stderr else ""
            raise AssertionError(f"hortus-gtk 提前退出 (code={proc.returncode}):\n{err_txt}")
        box = helper.find_mapped_toplevel()
        if box is not None:
            return box
        time.sleep(0.02)
    raise AssertionError("hortus-gtk 在超时内未映射顶层窗口")


def make_isolated_env(base_dir: Path, dbus_addr: str, display: str, locale: str) -> dict[str, str]:
    env = dict(os.environ)
    for k in ("WAYLAND_DISPLAY", "XAUTHORITY", "HORTUS_VAULT", "HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"):
        env.pop(k, None)
    cfg = base_dir / "config"
    cache = base_dir / "cache"
    data = base_dir / "data"
    for d in (cfg, cache, data):
        d.mkdir(parents=True, exist_ok=True)
    env["DISPLAY"] = display
    env["DBUS_SESSION_BUS_ADDRESS"] = dbus_addr
    env["GDK_BACKEND"] = "x11"
    env["GSK_RENDERER"] = "cairo"
    env["NO_AT_BRIDGE"] = "1"
    env["GTK_A11Y"] = "none"
    env["GTK_USE_PORTAL"] = "0"
    env["GIO_USE_VFS"] = "local"
    env["LC_ALL"] = locale
    env["LANG"] = locale
    env["HORTUS_CONFIG_DIR"] = str(cfg)
    env["HORTUS_CACHE_DIR"] = str(cache)
    env["HORTUS_DATA_DIR"] = str(data)
    env["XDG_CONFIG_HOME"] = str(cfg)
    env["XDG_CACHE_HOME"] = str(cache)
    env["XDG_DATA_HOME"] = str(data)
    if (ROOT / "apps/macos/Resources/Fonts/KaTeX").is_dir():
        env.setdefault("HORTUS_KATEX_FONTS_DIR", str(ROOT / "apps/macos/Resources/Fonts/KaTeX"))
    return env


def wait_dbus_window(gdbus: str, env: dict[str, str], timeout_s: float = 10.0) -> None:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        r = subprocess.run(
            [
                gdbus,
                "call",
                "--session",
                "--dest",
                "io.github.groundgrounder.Hortus",
                "--object-path",
                "/io/github/groundgrounder/Hortus/window/1",
                "--method",
                "org.gtk.Actions.List",
            ],
            env=env,
            capture_output=True,
            text=True,
        )
        if r.returncode == 0:
            return
        time.sleep(0.1)
    raise AssertionError("DBus /io/github/groundgrounder/Hortus/window/1 未在超时内就绪")


def activate_win_action(gdbus: str, env: dict[str, str], action: str, param: str | None = None) -> None:
    param_arg = f"[<{param!r}>]" if param is not None else "[]"
    subprocess.run(
        [
            gdbus,
            "call",
            "--session",
            "--dest",
            "io.github.groundgrounder.Hortus",
            "--object-path",
            "/io/github/groundgrounder/Hortus/window/1",
            "--method",
            "org.gtk.Actions.Activate",
            action,
            param_arg,
            "{}",
        ],
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )


def run_benchmark_and_screenshots(
    gui_bin: Path, media_dir: Path, output_dir: Path
) -> dict[str, object]:
    xvfb_bin = shutil.which("Xvfb")
    check(xvfb_bin is not None, "缺少 Xvfb（请在 nix develop 中运行）")
    gdbus = find_gdbus()
    media_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="hortus-l4-rel-") as tmp:
        tmp_path = Path(tmp)
        fbdir = tmp_path / "fb"
        fbdir.mkdir(parents=True, exist_ok=True)
        disp_num = next(
            n for n in range(110, 150) if not Path(f"/tmp/.X11-unix/X{n}").exists()
        )
        display_str = f":{disp_num}"
        xvfb = subprocess.Popen(
            [xvfb_bin, display_str, "-screen", "0", "1440x900x24", "-fbdir", str(fbdir), "-nolisten", "tcp", "-ac"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        x_sock = Path(f"/tmp/.X11-unix/X{disp_num}")
        for _ in range(100):
            if x_sock.exists():
                break
            time.sleep(0.05)
        check(x_sock.exists(), "私有 Xvfb 启动失败")

        dbus_cfg = tmp_path / "dbus.conf"
        dbus_cfg.write_text(
            '<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"\n'
            ' "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">\n'
            "<busconfig>\n"
            "  <type>session</type>\n"
            f"  <listen>unix:tmpdir={tmp_path}</listen>\n"
            '  <policy context="default">\n'
            '    <allow send_destination="*" eavesdrop="true"/>\n'
            '    <allow eavesdrop="true"/>\n'
            '    <allow own="*"/>\n'
            '    <allow user="*"/>\n'
            "  </policy>\n"
            "</busconfig>\n",
            encoding="utf-8",
        )
        dbus = subprocess.Popen(
            [
                "dbus-daemon",
                f"--config-file={dbus_cfg}",
                "--nofork",
                "--nopidfile",
                "--print-address=1",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        dbus_addr = dbus.stdout.readline().strip()
        check(bool(dbus_addr), "私有 dbus-daemon 启动失败")
        helper = X11DisplayHelper(display_str)
        xwd_file = fbdir / "Xvfb_screen0"

        try:
            vault = tmp_path / "demo-vault"
            shutil.copytree(ROOT / "fixtures/demo-vault", vault)

            # 1. 5 次冷启动时间中位数 + RSS + 空网络检查（英文生产运行）
            startup_ms = []
            for idx in range(5):
                env = make_isolated_env(tmp_path / f"boot-{idx}", dbus_addr, display_str, "en_US.UTF-8")
                t0 = time.monotonic()
                proc = subprocess.Popen(
                    [
                        str(gui_bin),
                        "--vault",
                        str(vault),
                        "--open",
                        "Markdown field guide.md",
                        "--width",
                        "1280",
                        "--height",
                        "820",
                    ],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                try:
                    wait_for_window_map(helper, proc)
                    startup_ms.append(round((time.monotonic() - t0) * 1000.0, 1))
                finally:
                    proc.terminate()
                    proc.wait(timeout=5)
                time.sleep(0.15)

            # 2. 15s 静息 CPU 对比（demo-vault vs 空窗口基线）
            def measure_idle(name: str, extra_args: list[str]) -> dict[str, object]:
                env = make_isolated_env(tmp_path / f"idle-{name}", dbus_addr, display_str, "en_US.UTF-8")
                proc = subprocess.Popen(
                    [str(gui_bin), *extra_args, "--width", "1280", "--height", "820"],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                try:
                    wait_for_window_map(helper, proc)
                    time.sleep(2.5)
                    rss_start = read_rss_kib(proc.pid) or 0
                    t0 = time.monotonic()
                    c0 = read_proc_cpu_ticks(proc.pid)
                    time.sleep(15.0)
                    t1 = time.monotonic()
                    c1 = read_proc_cpu_ticks(proc.pid)
                    rss_end = read_rss_kib(proc.pid) or 0
                    nets = process_network_sockets(proc.pid)
                    check(len(nets) == 0, f"{name} 发现非零网络套接字: {nets}")
                    cpu_sec = (c1 - c0) / float(os.sysconf("SC_CLK_TCK"))
                    cpu_pct = round((cpu_sec / (t1 - t0)) * 100.0, 3)
                    return {
                        "idle_cpu_seconds_15s": round(cpu_sec, 4),
                        "idle_cpu_pct": cpu_pct,
                        "rss_start_mib": round(rss_start / 1024.0, 2),
                        "rss_end_mib": round(rss_end / 1024.0, 2),
                        "net_sockets": len(nets),
                    }
                finally:
                    proc.terminate()
                    proc.wait(timeout=5)

            idle_vault = measure_idle(
                "vault",
                ["--vault", str(vault), "--open", "Markdown field guide.md"],
            )
            idle_baseline = measure_idle("baseline", ["--new-instance"])
            idle_delta_pct = round(idle_vault["idle_cpu_pct"] - idle_baseline["idle_cpu_pct"], 3)

            # 3. 采集 4 张真实生产公开截图（通过 session.toml + layout.toml + DBus org.gtk.Actions）
            screenshots = []

            def run_scene(
                filename: str,
                locale: str,
                width: int,
                height: int,
                tabs: list[str],
                selected: str,
                layout_toml: str | None,
                actions: list[tuple[str, str | None]],
            ) -> dict[str, object]:
                scene_dir = tmp_path / f"scene-{filename}"
                env = make_isolated_env(scene_dir, dbus_addr, display_str, locale)
                cache_dir = Path(env["HORTUS_CACHE_DIR"])
                cfg_dir = Path(env["HORTUS_CONFIG_DIR"])
                tabs_toml = ", ".join(json.dumps(t) for t in tabs)
                (cache_dir / "session.toml").write_text(
                    f'[[windows]]\nid = "w1"\npath = {json.dumps(str(vault))}\ntabs = [{tabs_toml}]\nselected = {json.dumps(selected)}\n',
                    encoding="utf-8",
                )
                if layout_toml is not None:
                    (cfg_dir / "layout.toml").write_text(layout_toml, encoding="utf-8")

                proc = subprocess.Popen(
                    [str(gui_bin)],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                try:
                    geom = wait_for_window_map(helper, proc)
                    wait_dbus_window(gdbus, env)
                    time.sleep(2.2)
                    for act, param in actions:
                        activate_win_action(gdbus, env, act, param)
                        time.sleep(1.3)
                    geom = helper.find_mapped_toplevel() or geom
                    # 无 WM 时 Xvfb 下默认窗口尺寸若需校验：AdwApplicationWindow 已加载保存尺寸或默认 1220x800
                    out_png = media_dir / filename
                    w, h = capture_xwd_png(xwd_file, out_png, geom)
                    return {
                        "filename": filename,
                        "path": str(out_png.resolve()),
                        "locale": locale,
                        "width": w,
                        "height": h,
                        "size_bytes": out_png.stat().st_size,
                        "sha256": hashlib.sha256(out_png.read_bytes()).hexdigest(),
                    }
                finally:
                    proc.terminate()
                    out_s, err_s = proc.communicate(timeout=5)
                    (output_dir / f"{filename}.stdout.log").write_text(out_s, encoding="utf-8")
                    (output_dir / f"{filename}.stderr.log").write_text(err_s, encoding="utf-8")
                    check("panicked at" not in out_s + err_s, f"{filename} 发生 panic:\n{err_s}")

            # 截图 1: 英文主窗 + Preferences
            screenshots.append(
                run_scene(
                    "l4-en-main-and-settings.png",
                    "en_US.UTF-8",
                    1280,
                    820,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    None,
                    [("open-settings", None)],
                )
            )

            # 截图 2: 中文主窗 + 极简关于对话
            screenshots.append(
                run_scene(
                    "l4-zh-main-and-about.png",
                    "zh_CN.UTF-8",
                    1280,
                    820,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    None,
                    [("about", None)],
                )
            )

            # 截图 3: 窄边栏 ToolDock 居中 + 动态纯图标折叠（左侧放入 5 工具且收窄到 190px）
            narrow_layout = """
[regions.left]
size = 190.0
[[regions.left.groups]]
id = "left-main"
tabs = ["files", "search", "tags", "outline", "changes"]
selected = "files"

[regions.right]
size = 240.0
[[regions.right.groups]]
id = "right-main"
tabs = ["backlinks"]
selected = "backlinks"

[regions.bottom]
size = 220.0
[[regions.bottom.groups]]
id = "bottom-main"
tabs = ["terminal"]
selected = "terminal"
"""
            screenshots.append(
                run_scene(
                    "l4-dock-centered-and-icon-only.png",
                    "en_US.UTF-8",
                    1180,
                    800,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    narrow_layout,
                    [],
                )
            )

            # 截图 4: 英文 Preferences + 底部终端
            screenshots.append(
                run_scene(
                    "l4-en-settings-and-terminal.png",
                    "en_US.UTF-8",
                    1280,
                    820,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    None,
                    [("dock-new-shell", "bottom"), ("open-settings", None)],
                )
            )

            # 截图 5 (L5a): 暗色模式英文主窗 + 阅读（公式/图表示例）
            screenshots.append(
                run_scene(
                    "l5a-dark-main-and-reading.png",
                    "en_US.UTF-8",
                    1280,
                    820,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    None,
                    [("color-scheme", "dark")],
                )
            )

            # 截图 6 (L5a): 暗色模式设置对话框 + 底部终端
            screenshots.append(
                run_scene(
                    "l5a-dark-settings-and-terminal.png",
                    "en_US.UTF-8",
                    1280,
                    820,
                    ["Markdown field guide.md", "Learning/Learning map.md"],
                    "Markdown field guide.md",
                    None,
                    [("color-scheme", "dark"), ("dock-new-shell", "bottom"), ("open-settings", None)],
                )
            )

            return {
                "startup_runs_ms": startup_ms,
                "startup_median_ms": round(statistics.median(startup_ms), 1),
                "idle_vault": idle_vault,
                "idle_baseline": idle_baseline,
                "idle_cpu_delta_pct": idle_delta_pct,
                "screenshots": screenshots,
            }
        finally:
            helper.close()
            dbus.terminate()
            xvfb.terminate()


def collect_dist_info() -> dict[str, object]:
    dist = ROOT / "build/linux/dist"
    items = []
    if dist.is_dir():
        for path in sorted(dist.iterdir()):
            if path.is_file() and not path.name.endswith(".sha256"):
                items.append(
                    {
                        "name": path.name,
                        "size_bytes": path.stat().st_size,
                        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                    }
                )
    return {"dist_artifacts": items}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--cli-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-release")
    parser.add_argument("--media", type=Path, default=None)
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()
    media_dir.mkdir(parents=True, exist_ok=True)

    gui_bin = discover_binary(
        args.gui_bin,
        [
            ROOT / "build/linux/result-linux/bin/hortus-gtk",
            ROOT / "result/bin/hortus-gtk",
            ROOT / "build/linux/bin/hortus-gtk",
        ],
        "hortus-gtk",
    )
    cli_bin = discover_binary(
        args.cli_bin,
        [
            ROOT / "build/linux/result-cli/bin/hortus",
            ROOT / "result-cli/bin/hortus",
            ROOT / "build/linux/bin/hortus",
        ],
        "hortus CLI",
    )

    cli_audit = audit_cli_production(gui_bin)
    prefix_audit = audit_elf_and_prefix(gui_bin, cli_bin)
    source_audit = audit_i18n_and_copy()
    runtime = run_benchmark_and_screenshots(gui_bin, media_dir, output_dir)
    dist = collect_dist_info()

    summary = {
        "passed": True,
        "gui_bin": str(gui_bin),
        "cli_bin": str(cli_bin),
        "cli_audit": cli_audit,
        "prefix_audit": prefix_audit,
        "source_audit": source_audit,
        "runtime": runtime,
        "dist": dist,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    stray_pngs = sorted(str(p.relative_to(output_dir)) for p in output_dir.rglob("*.png"))
    check(not stray_pngs, f"--output 目录内出现残留 PNG：{stray_pngs}")
    print(
        f"PASS: startup_median={runtime['startup_median_ms']}ms, "
        f"rss={runtime['idle_vault']['rss_end_mib']}MiB, "
        f"idle_cpu_delta={runtime['idle_cpu_delta_pct']}%, "
        f"shots={len(runtime['screenshots'])}, 0 bare_zh, 0 net"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as err:
        print(f"FAIL: {err}", file=sys.stderr)
        raise SystemExit(1)
