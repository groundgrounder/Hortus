#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""面向生产二进制的 Linux 一键并排视觉巡检图集脚本（纯标准库实现，零外部 Python 依赖）。"""
from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]

TOUR_NOTES = [
    ("start-here", "Start here.md"),
    ("learning-map", "Learning/Learning map.md"),
    ("field-guide", "Markdown field guide.md"),
    ("study-loop", "Learning/Study loop.md"),
    ("project-workflow", "Project/Project workflow.md"),
]


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


def discover_binary(explicit: Path | None) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的二进制不存在: {explicit}")
        return explicit.resolve()
    candidates = [
        ROOT / "result/bin/hortus-gtk",
        ROOT / "build/linux/result-linux/bin/hortus-gtk",
        ROOT / "build/linux/bin/hortus-gtk",
    ]
    for cand in candidates:
        if cand.is_file():
            return cand.resolve()
    raise AssertionError("未找到生产 hortus-gtk 二进制，请先 nix build .#hortus-linux 或传入 --binary")


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

    def find_mapped_toplevel(self) -> tuple[int, int, int, int, int] | None:
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
            best: tuple[int, int, int, int, int] | None = None
            for i in range(nchildren.value):
                win = children[i]
                attrs = self.XWindowAttributes()
                if self.x11.XGetWindowAttributes(self.dpy, win, ctypes.byref(attrs)):
                    if attrs.map_state == 2 and attrs.width >= 300 and attrs.height >= 200:
                        if best is None or attrs.width * attrs.height > best[3] * best[4]:
                            best = (int(win), attrs.x, attrs.y, attrs.width, attrs.height)
            return best
        finally:
            if children:
                self.x11.XFree(children)

    def close(self) -> None:
        if self.dpy:
            self.x11.XCloseDisplay(self.dpy)
            self.dpy = None


def _pad4(n: int) -> int:
    return (4 - (n % 4)) % 4


class X11ResizeClient:
    def __init__(self, display: str) -> None:
        _, rest = display.split(":", 1)
        dnum = int(rest.split(".", 1)[0])
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        unix_path = f"/tmp/.X11-unix/X{dnum}"
        try:
            self.sock.connect(unix_path)
        except OSError:
            self.sock.connect(f"\x00/tmp/.X11-unix/X{dnum}")
        self.sock.settimeout(5.0)
        self.seq = 0
        self._handshake()

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def _recv_exact(self, n: int) -> bytes:
        buf = bytearray()
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise AssertionError("X11 连接提前关闭")
            buf.extend(chunk)
        return bytes(buf)

    def _handshake(self) -> None:
        self.sock.sendall(struct.pack("<BxHHHHxx", ord("l"), 11, 0, 0, 0))
        head = self._recv_exact(8)
        status, _, _, _, extra_words = struct.unpack("<BBHHH", head)
        self._recv_exact(extra_words * 4)
        check(status == 1, f"X11 握手失败 status={status}")

    def _send_req(self, data: bytes) -> int:
        self.seq = (self.seq + 1) & 0xFFFF
        self.sock.sendall(data)
        return self.seq

    def sync(self) -> None:
        seq = self._send_req(struct.pack("<BBH", 43, 0, 1))
        while True:
            head = self._recv_exact(32)
            if head[0] == 1:
                _, _, rseq, extra = struct.unpack_from("<BBHI", head, 0)
                if extra:
                    self._recv_exact(extra * 4)
                if rseq == seq:
                    return
            elif head[0] == 0:
                raise AssertionError("X11 请求错误")

    def configure_window(self, wid: int, x: int, y: int, width: int, height: int) -> None:
        # X(0x1) | Y(0x2) | Width(0x4) | Height(0x8) = 0x000F
        req = struct.pack("<BBHIHHIIII", 12, 0, 7, wid, 0x000F, 0, x, y, width, height)
        self._send_req(req)
        self.sync()


def encode_rgb_png(width: int, height: int, rgb_rows: list[bytes]) -> bytes:
    raw = bytearray()
    for row in rgb_rows:
        raw.append(0)
        raw.extend(row)

    def chunk(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    idat = zlib.compress(bytes(raw), 6)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def decode_rgb_png(png_path: Path) -> tuple[int, int, list[bytes]]:
    data = png_path.read_bytes()
    check(data[:8] == b"\x89PNG\r\n\x1a\n", f"非合法 PNG: {png_path}")
    pos = 8
    width = height = 0
    idat_chunks: list[bytes] = []
    while pos < len(data):
        length, ctype = struct.unpack_from(">I4s", data, pos)
        payload = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            width, height, bit_depth, color_type, _, _, _ = struct.unpack(">IIBBBBB", payload)
            check(bit_depth == 8 and color_type == 2, f"仅支持 8-bit RGB PNG: {png_path}")
        elif ctype == b"IDAT":
            idat_chunks.append(payload)
        elif ctype == b"IEND":
            break

    raw = zlib.decompress(b"".join(idat_chunks))
    stride = width * 3
    rows: list[bytes] = []
    prev = bytearray(stride)
    idx = 0
    for _ in range(height):
        ftype = raw[idx]
        idx += 1
        scan = bytearray(raw[idx : idx + stride])
        idx += stride
        if ftype == 1:  # Sub
            for i in range(3, stride):
                scan[i] = (scan[i] + scan[i - 3]) & 0xFF
        elif ftype == 2:  # Up
            for i in range(stride):
                scan[i] = (scan[i] + prev[i]) & 0xFF
        elif ftype == 3:  # Average
            for i in range(stride):
                left = scan[i - 3] if i >= 3 else 0
                scan[i] = (scan[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = scan[i - 3] if i >= 3 else 0
                b = prev[i]
                c = prev[i - 3] if i >= 3 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                scan[i] = (scan[i] + pr) & 0xFF
        rows.append(bytes(scan))
        prev = scan
    return width, height, rows


def stitch_side_by_side(left_png: Path, right_png: Path, out_png: Path, divider_px: int = 4) -> tuple[int, int]:
    lw, lh, lrows = decode_rgb_png(left_png)
    rw, rh, rrows = decode_rgb_png(right_png)
    out_w = lw + divider_px + rw
    out_h = max(lh, rh)
    div = bytes([128, 128, 128]) * divider_px
    lblank = bytes([240, 240, 240]) * lw
    rblank = bytes([240, 240, 240]) * rw
    merged_rows: list[bytes] = []
    for y in range(out_h):
        lr = lrows[y] if y < lh else lblank
        rr = rrows[y] if y < rh else rblank
        merged_rows.append(lr + div + rr)
    out_png.parent.mkdir(parents=True, exist_ok=True)
    out_png.write_bytes(encode_rgb_png(out_w, out_h, merged_rows))
    return out_w, out_h


def capture_xwd_png(xwd_path: Path, out_png: Path, crop: tuple[int, int, int, int]) -> tuple[int, int]:
    data = xwd_path.read_bytes()
    fields = struct.unpack(">25I", data[:100])
    header_size = fields[0]
    check(fields[1] == 7, f"不支持的 XWD 版本: {fields[1]}")
    byte_order = fields[7]
    pix_w, pix_h = fields[4], fields[5]
    bpp, bytes_per_line, ncolors = fields[11], fields[12], fields[19]
    check(bpp == 32, f"仅支持 32bpp XWD，实际 {bpp}")
    px_buf = data[header_size + ncolors * 12 :]

    cx, cy, cw, ch = crop
    cx = max(0, cx)
    cy = max(0, cy)
    cw = min(cw, pix_w - cx)
    ch = min(ch, pix_h - cy)

    rows: list[bytes] = []
    for y in range(cy, cy + ch):
        row_start = y * bytes_per_line + cx * 4
        row = px_buf[row_start : row_start + cw * 4]
        rgb = bytearray(cw * 3)
        if byte_order == 0:
            rgb[0::3] = row[2::4]
            rgb[1::3] = row[1::4]
            rgb[2::3] = row[0::4]
        else:
            rgb[0::3] = row[1::4]
            rgb[1::3] = row[2::4]
            rgb[2::3] = row[3::4]
        rows.append(bytes(rgb))

    out_png.parent.mkdir(parents=True, exist_ok=True)
    out_png.write_bytes(encode_rgb_png(cw, ch, rows))
    return cw, ch


def make_isolated_env(base_dir: Path, dbus_addr: str, display: str) -> dict[str, str]:
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
    env["LC_ALL"] = "en_US.UTF-8"
    env["LANG"] = "en_US.UTF-8"
    env["HORTUS_CONFIG_DIR"] = str(cfg)
    env["HORTUS_CACHE_DIR"] = str(cache)
    env["HORTUS_DATA_DIR"] = str(data)
    env["XDG_CONFIG_HOME"] = str(cfg)
    env["XDG_CACHE_HOME"] = str(cache)
    env["XDG_DATA_HOME"] = str(data)
    if (ROOT / "apps/macos/Resources/Fonts/KaTeX").is_dir():
        env.setdefault("HORTUS_KATEX_FONTS_DIR", str(ROOT / "apps/macos/Resources/Fonts/KaTeX"))
    return env


def wait_for_window(helper: X11DisplayHelper, proc: subprocess.Popen[str], timeout_s: float = 12.0) -> tuple[int, int, int, int, int]:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            err = proc.stderr.read() if proc.stderr else ""
            raise AssertionError(f"hortus-gtk 提前退出 ({proc.returncode}):\n{err}")
        box = helper.find_mapped_toplevel()
        if box is not None:
            return box
        time.sleep(0.03)
    raise AssertionError("hortus-gtk 在超时内未映射窗口")


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
        time.sleep(0.08)
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--vault", type=Path, default=ROOT / "fixtures/demo-vault")
    parser.add_argument("--out", type=Path, default=Path("/tmp/hortus-evidence/visual-tour"))
    args = parser.parse_args()

    gui_bin = discover_binary(args.binary)
    src_vault = args.vault.resolve()
    check(src_vault.is_dir(), f"Vault 不存在: {src_vault}")
    out_dir = args.out.resolve()
    raw_dir = out_dir / "raw"
    out_dir.mkdir(parents=True, exist_ok=True)
    raw_dir.mkdir(parents=True, exist_ok=True)

    xvfb_bin = shutil.which("Xvfb")
    check(xvfb_bin is not None, "缺少 Xvfb（请在 nix develop 中运行）")
    gdbus = find_gdbus()

    with tempfile.TemporaryDirectory(prefix="hortus-visual-tour-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "vault"
        shutil.copytree(src_vault, vault)

        fbdir = tmp_path / "fb"
        fbdir.mkdir(parents=True, exist_ok=True)
        disp_num = next(n for n in range(115, 155) if not Path(f"/tmp/.X11-unix/X{n}").exists())
        display = f":{disp_num}"
        xvfb = subprocess.Popen(
            [xvfb_bin, display, "-screen", "0", "1460x920x24", "-fbdir", str(fbdir), "-nolisten", "tcp", "-ac"],
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
            ["dbus-daemon", f"--config-file={dbus_cfg}", "--nofork", "--nopidfile", "--print-address=1"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        dbus_addr = dbus.stdout.readline().strip()
        check(bool(dbus_addr), "私有 dbus-daemon 启动失败")

        helper = X11DisplayHelper(display)
        x11_resize = X11ResizeClient(display)
        xwd_file = fbdir / "Xvfb_screen0"

        records = []
        try:
            for slug, relpath in TOUR_NOTES:
                scene_dir = tmp_path / f"tour-{slug}"
                env = make_isolated_env(scene_dir, dbus_addr, display)
                cache_dir = Path(env["HORTUS_CACHE_DIR"])
                (cache_dir / "session.toml").write_text(
                    f'[[windows]]\nid = "w1"\npath = {json.dumps(str(vault))}\ntabs = [{json.dumps(relpath)}]\nselected = {json.dumps(relpath)}\n',
                    encoding="utf-8",
                )
                proc = subprocess.Popen(
                    [str(gui_bin)],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                try:
                    wid, _, _, _, _ = wait_for_window(helper, proc)
                    wait_dbus_window(gdbus, env)
                    x11_resize.configure_window(wid, 0, 0, 1280, 820)
                    time.sleep(1.4)

                    def snap(name: str, w_exp: int, h_exp: int) -> Path:
                        geom = helper.find_mapped_toplevel()
                        crop = (geom[1], geom[2], geom[3], geom[4]) if geom else (0, 0, w_exp, h_exp)
                        out_p = raw_dir / f"{slug}-{name}.png"
                        capture_xwd_png(xwd_file, out_p, crop)
                        return out_p

                    # 1. Light Reading
                    activate_win_action(gdbus, env, "color-scheme", "light")
                    time.sleep(0.9)
                    p_lr = snap("light-reading", 1280, 820)

                    # 2. Light Editing
                    activate_win_action(gdbus, env, "toggle-edit-mode")
                    time.sleep(0.8)
                    p_le = snap("light-editing", 1280, 820)

                    # 3. Dark Editing
                    activate_win_action(gdbus, env, "color-scheme", "dark")
                    time.sleep(0.8)
                    p_de = snap("dark-editing", 1280, 820)

                    # 4. Dark Reading
                    activate_win_action(gdbus, env, "toggle-edit-mode")
                    time.sleep(1.0)
                    p_dr = snap("dark-reading", 1280, 820)

                    # 5. Wide No-Sidebars Editing (1440x860)
                    activate_win_action(gdbus, env, "color-scheme", "light")
                    activate_win_action(gdbus, env, "toggle-left-sidebar")
                    activate_win_action(gdbus, env, "toggle-right-sidebar")
                    activate_win_action(gdbus, env, "toggle-edit-mode")
                    x11_resize.configure_window(wid, 0, 0, 1440, 860)
                    time.sleep(1.1)
                    p_we = raw_dir.parent / f"wide-{slug}-edit.png"
                    geom_w = helper.find_mapped_toplevel()
                    crop_w = (geom_w[1], geom_w[2], geom_w[3], geom_w[4]) if geom_w else (0, 0, 1440, 860)
                    ww, wh = capture_xwd_png(xwd_file, p_we, crop_w)

                    pair_light = out_dir / f"pair-{slug}-light.png"
                    plw, plh = stitch_side_by_side(p_lr, p_le, pair_light)
                    pair_dark = out_dir / f"pair-{slug}-dark.png"
                    pdw, pdh = stitch_side_by_side(p_dr, p_de, pair_dark)

                    records.append({
                        "slug": slug,
                        "note": relpath,
                        "pair_light": {"file": pair_light.name, "w": plw, "h": plh, "sha256": hashlib.sha256(pair_light.read_bytes()).hexdigest()},
                        "pair_dark": {"file": pair_dark.name, "w": pdw, "h": pdh, "sha256": hashlib.sha256(pair_dark.read_bytes()).hexdigest()},
                        "wide_edit": {"file": p_we.name, "w": ww, "h": wh, "sha256": hashlib.sha256(p_we.read_bytes()).hexdigest()},
                    })
                finally:
                    proc.terminate()
                    _, err_s = proc.communicate(timeout=5)
                    check("panicked at" not in err_s, f"{relpath} 发生 panic:\n{err_s}")
        finally:
            x11_resize.close()
            helper.close()
            dbus.terminate()
            xvfb.terminate()

    lines = [
        "# Hortus Linux Production Visual Tour",
        "",
        f"- **Binary**: `{gui_bin}`",
        f"- **Source Vault**: `{src_vault}`",
        "",
        "## Side-by-Side Comparison Sheets (Left: Reading / Right: Editing)",
        "",
        "| Note | Light Pair | Dark Pair | Wide Edit (No Sidebars) |",
        "|---|---|---|---|",
    ]
    for rec in records:
        pl = rec["pair_light"]
        pd = rec["pair_dark"]
        we = rec["wide_edit"]
        lines.append(
            f"| `{rec['note']}` | [`{pl['file']}`]({pl['file']}) ({pl['w']}x{pl['h']}) | "
            f"[`{pd['file']}`]({pd['file']}) ({pd['w']}x{pd['h']}) | "
            f"[`{we['file']}`]({we['file']}) ({we['w']}x{we['h']}) |"
        )
    lines.append("")
    (out_dir / "index.md").write_text("\n".join(lines), encoding="utf-8")
    (out_dir / "tour.json").write_text(
        json.dumps({"binary": str(gui_bin), "records": records}, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(f"PASS: visual tour generated {len(records)} notes -> {out_dir / 'index.md'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as err:
        print(f"FAIL: {err}", file=sys.stderr)
        raise SystemExit(1)
