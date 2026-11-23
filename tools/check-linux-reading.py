#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""L1 Linux Reading View、Mermaid 原生渲染、RaTeX 公式、嵌入/表格/图片、专注模式与零写盘零联网综合自检。"""

import argparse
import atexit
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]

KATEX_FILES = [
    "KaTeX_AMS-Regular.ttf",
    "KaTeX_Caligraphic-Bold.ttf",
    "KaTeX_Caligraphic-Regular.ttf",
    "KaTeX_Fraktur-Bold.ttf",
    "KaTeX_Fraktur-Regular.ttf",
    "KaTeX_Main-Bold.ttf",
    "KaTeX_Main-BoldItalic.ttf",
    "KaTeX_Main-Italic.ttf",
    "KaTeX_Main-Regular.ttf",
    "KaTeX_Math-BoldItalic.ttf",
    "KaTeX_Math-Italic.ttf",
    "KaTeX_SansSerif-Bold.ttf",
    "KaTeX_SansSerif-Italic.ttf",
    "KaTeX_SansSerif-Regular.ttf",
    "KaTeX_Script-Regular.ttf",
    "KaTeX_Size1-Regular.ttf",
    "KaTeX_Size2-Regular.ttf",
    "KaTeX_Size3-Regular.ttf",
    "KaTeX_Size4-Regular.ttf",
    "KaTeX_Typewriter-Regular.ttf",
]


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



def discover_gui_bin(explicit: Path | None) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的 GUI 二进制不存在: {explicit}")
        return explicit.resolve()
    for candidate in (
        ROOT / "result/bin/hortus-gtk",
        ROOT / "build/linux/result-linux/bin/hortus-gtk",
        ROOT / "build/linux/bin/hortus-gtk",
        ROOT / "apps/linux/target/release/hortus-gtk",
    ):
        if candidate.is_file():
            return candidate.resolve()
    raise AssertionError("未找到 hortus-gtk 二进制，请先构建或传入 --gui-bin")


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
    env.pop("HORTUS_VAULT", None)
    cfg = base_dir / "config"
    cache = base_dir / "cache"
    data = base_dir / "data"
    cfg.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    data.mkdir(parents=True, exist_ok=True)
    env["HORTUS_CONFIG_DIR"] = str(cfg)
    env["HORTUS_CACHE_DIR"] = str(cache)
    env["HORTUS_DATA_DIR"] = str(data)
    env["XDG_CONFIG_HOME"] = str(cfg)
    env["XDG_CACHE_HOME"] = str(cache)
    env["XDG_DATA_HOME"] = str(data)
    return env


def process_network_sockets(pid: int):
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
                found.append({"proto": proto, "local": parts[1], "remote": parts[2]})
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


def make_tiny_png(width=64, height=40):
    """生成纯像素测试用最小合法 PNG（无需额外 Python 图像依赖）。"""
    import zlib

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows.extend((59, 130, 246, 255) if (x + y) % 8 < 4 else (226, 232, 240, 255))
    idat = zlib.compress(bytes(rows))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def populate_synthetic_l1_vault(vault: Path):
    """在临时隔离库副本中生成全场景测试文档与资源（不修改仓库 fixtures）。"""
    assets = vault / "Assets"
    assets.mkdir(exist_ok=True)
    (assets / "l1-sample.png").write_bytes(make_tiny_png(120, 72))
    (assets / "l1-unsafe.svg").write_text(
        """<svg xmlns="http://www.w3.org/2000/svg" width="160" height="80" viewBox="0 0 160 80" onload="alert(1)">
  <script>fetch("https://invalid.example.invalid/ping")</script>
  <style>@import url("https://invalid.example.invalid/style.css");</style>
  <rect x="6" y="6" width="148" height="68" rx="10" fill="#dbeafe" stroke="#2563eb" stroke-width="2" onclick="x()"/>
  <image href="https://invalid.example.invalid/pixel.png" x="10" y="10" width="8" height="8"/>
  <text x="80" y="45" text-anchor="middle" font-size="14" fill="#1e3a8a">SVG Safe Card</text>
</svg>
""",
        encoding="utf-8",
    )

    (vault / "L1-standalone.mmd").write_text(
        """flowchart LR
    Src["[[Start here|起点文档]]"] --> Engine["Rust 核心排版"]
    Engine --> Cairo["GTK4 / Cairo 渲染"]
    subgraph ExportGroup["多格式导出"]
      SVG["SVG 矢量"]
      PNG["PNG 位图"]
      PDF["PDF 文档"]
    end
    Cairo --> ExportGroup
""",
        encoding="utf-8",
    )

    (vault / "L1-embedded.md").write_text(
        """# L1 Embedded Reference

这是被引用的完整笔记片段，用于验证全文嵌入与标题切片能力。

## Target Section

此处包含公式 $\\alpha^2 + \\beta^2 = \\gamma^2$ 与引用链接 [[Learning/Learning map|学习导图]]。

> [!info] 嵌入式提示块
> 支持在嵌入卡片内保留 Callout 与样式语义。
""",
        encoding="utf-8",
    )

    (vault / "L1-cycle-a.md").write_text(
        "# Cycle A\n\n![[L1-cycle-b]]\n", encoding="utf-8"
    )
    (vault / "L1-cycle-b.md").write_text(
        "# Cycle B\n\n![[L1-cycle-a]]\n", encoding="utf-8"
    )

    (vault / "L1-math-mermaid-showcase.md").write_text(
        """---
title: L1 阅读态与场景综合验证
tags: [reading, mermaid, math]
---

# 阅读态、公式与六类 Mermaid 综合样本

%% 源码注释应在默认 Reading View 中隐藏 %%
<!-- HTML 注释同样不应出现在阅读正文 -->

## 一、RaTeX 原生公式与缺字度量回测

行内公式测试：欧拉恒等式 $e^{i\\pi} + 1 = 0$，以及分式对齐测试 $\\frac{a + b}{c^2 + d^2}$。

中文与混合文本度量回测：$\\text{动能公式 } E_k = \\frac{1}{2} m v^2$，保持上下文排版不塌陷。

块级居中公式与极限矩阵：

$$
f(x) = \\int_{-\\infty}^{\\infty} \\hat{f}(\\xi) e^{2\\pi i \\xi x} \\, d\\xi = \\sum_{n=0}^{\\infty} \\frac{f^{(n)}(0)}{n!} x^n
$$

异常公式局部降级测试：坏行内 $\\unknowninlinecmd{x}$ 与上下文同在；坏块级公式如下：

$$
\\unknownblockcmd{\\frac{1}{2}}
$$

---

## 二、六类 Mermaid 图表与异常降级

### 1. Mindmap（可折叠节点）

```mermaid
mindmap
  root((Hortus))
    Kernel
      [[Learning/Learning map|Knowledge]]
      Math
    Linux
      GTK4
      Cairo
```

### 2. Flowchart（含子图与双链节点）

```mermaid
flowchart TB
    Start["[[Start here|起始页]]"] --> Process["ReadingModel 解析"]
    subgraph Renderers["原生绘制层"]
      Pango["PangoCairo 文字"]
      Math["RaTeX 引擎"]
    end
    Process --> Renderers
```

### 3. Sequence 图

```mermaid
sequenceDiagram
    participant UI as GTK4 主线程
    participant BG as 后台 Worker
    UI->>BG: 请求 prepare_reading_note
    BG-->>UI: SceneData + PreparedMath
```

### 4. State 图

```mermaid
stateDiagram-v2
    [*] --> Reading
    Reading --> Editing : Ctrl+E
    Editing --> Reading : Ctrl+E
```

### 5. Class 图

```mermaid
classDiagram
    class ReadingViewModel {
      +text: String
      +blocks: Vec
    }
    class DiagramEngine {
      +render_request() SceneData
    }
    ReadingViewModel --> DiagramEngine
```

### 6. ER 图

```mermaid
erDiagram
    NOTE ||--o{ DIAGRAM : contains
    NOTE ||--o{ FORMULA : embeds
```

### 7. 不支持图类型与语法错误局部降级

```mermaid
gantt
    title 不支持图类应局部退回源码卡片
    section Stage
    Task A :a1, 2026-01-01, 5d
```

```mermaid
flowchart TB
    BrokenNode --> ??? BrokenSyntax
```

## 三、表格、本地图片、SVG 净化与远程拦截

| 特性项目 | 公式与样式列 | 对齐方式 |
| :--- | :---: | ---: |
| 行内公式单元格 | **核心** $x_{1,2} = \\frac{-b \\pm \\sqrt{\\Delta}}{2a}$ | `100%` |
| 链接与标记 | [规范指南](https://example.org) | 验证通过 |

![[l1-sample.png|180]]

![受净化本地 SVG](Assets/l1-unsafe.svg)

![远程图片不自动联网](https://invalid.example.invalid/remote-banner.png)

## 四、三类嵌入卡片

![[L1-embedded]]

![[L1-embedded#Target Section]]

![[L1-standalone.mmd]]

![[L1-math-mermaid-showcase]]

![[L1-embedded#MissingHeading]]

![[L1-missing-note]]

![[L1-cycle-a]]
""",
        encoding="utf-8",
    )

    (vault / "L1-embeds-dark-scene.md").write_text(
        """# 嵌入与媒体安全暗色检验页

![[L1-embedded#Target Section]]

![[L1-standalone.mmd]]

![[L1-cycle-a]]

![远程图像零请求占位](https://invalid.example.invalid/dark-remote.png)
""",
        encoding="utf-8",
    )

    (vault / "L1-mermaid-gallery.md").write_text(
        """# 六类 Mermaid 原生渲染总览

```mermaid
flowchart LR
    A["[[Start here|起始页]]"] --> B["Reading 解析"]
    subgraph Native["Cairo + Pango 绘制"]
      C["节点形状"]
      D["正交圆角边"]
    end
    B --> Native
```

```mermaid
sequenceDiagram
    participant UI as GTK4 界面
    participant BG as Rust 后台
    UI->>BG: render_request
    BG-->>UI: SceneData
```

```mermaid
stateDiagram-v2
    [*] --> Reading
    Reading --> Editing : Ctrl+E
    Editing --> Reading : Ctrl+E
```

```mermaid
classDiagram
    class ReadingViewModel {
      +text: String
      +blocks: Vec
    }
    class DiagramEngine {
      +render_request() SceneData
    }
    ReadingViewModel --> DiagramEngine
```

```mermaid
erDiagram
    NOTE ||--o{ DIAGRAM : contains
    NOTE ||--o{ FORMULA : embeds
```
""",
        encoding="utf-8",
    )

    (vault / "L1-table-media-view.md").write_text(
        """## 三、表格、本地图片、SVG 净化与远程拦截

| 特性项目 | 公式与样式列 | 对齐方式 |
| :--- | :---: | ---: |
| 行内公式单元格 | **核心** $x_{1,2} = \\frac{-b \\pm \\sqrt{\\Delta}}{2a}$ | `100%` |
| 链接与标记 | [规范指南](https://example.org) | 验证通过 |

![[l1-sample.png|180]]

![受净化本地 SVG](Assets/l1-unsafe.svg)

![远程图片不自动联网](https://invalid.example.invalid/remote-banner.png)
""",
        encoding="utf-8",
    )


def verify_nix_fonts_installed(gui_bin: Path):
    nix_out = gui_bin.resolve().parents[1]
    fonts_dir = nix_out / "share/hortus/fonts/KaTeX"
    if not fonts_dir.is_dir():
        # 非 nix build 路径时退回到源码共享资源检测并注明
        alt = ROOT / "apps/macos/Resources/Fonts/KaTeX"
        check(alt.is_dir(), f"未找到 KaTeX 字体目录：{fonts_dir}")
        fonts_dir = alt
    missing = [f for f in KATEX_FILES if not (fonts_dir / f).is_file()]
    check(not missing, f"缺失 KaTeX 字体文件: {missing}")
    check((fonts_dir / "OFL.txt").is_file(), f"缺失 OFL.txt: {fonts_dir}")
    # 确认 apps/linux 目录内无重复提交的 .ttf 文件
    linux_ttf = [
        p
        for p in (ROOT / "apps/linux").rglob("*.ttf")
        if "target" not in p.parts
    ]
    check(not linux_ttf, f"严禁复制 KaTeX TTF 至 apps/linux/ 目录下：{linux_ttf}")
    return str(fonts_dir)


def run_reading_automation(gui_bin: Path, vault: Path, env: dict, args_list: list[str], json_out: Path, label: str):
    before = snapshot_tree(vault)
    json_out.unlink(missing_ok=True)
    proc = subprocess.Popen(
        [str(gui_bin), *args_list, "--reading-json", str(json_out)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    sockets_seen = []
    samples = 0
    peak_rss_kib = 0
    deadline = time.monotonic() + 35.0
    while proc.poll() is None:
        if time.monotonic() > deadline:
            proc.kill()
            raise AssertionError(f"{label} 运行超时 (35s)")
        s = process_network_sockets(proc.pid)
        samples += 1
        if s:
            sockets_seen.extend(s)
        rss = read_rss_kib(proc.pid)
        if rss and rss > peak_rss_kib:
            peak_rss_kib = rss
        time.sleep(0.06)

    stdout, stderr = proc.communicate(timeout=5)
    check(proc.returncode == 0, f"{label} 退出码 {proc.returncode}\nstdout={stdout}\nstderr={stderr}")
    check(not sockets_seen, f"{label} 检测到非零网络连接：{sockets_seen}")
    check(json_out.is_file(), f"{label} 未生成输出 JSON：{json_out}")
    after = snapshot_tree(vault)
    check(before == after, f"{label} 修改了只读演示库内容！")

    data = json.loads(json_out.read_text(encoding="utf-8"))
    return {
        "report": data,
        "network_samples": samples,
        "network_connections": 0,
        "peak_rss_kib": peak_rss_kib,
        "peak_rss_mib": round(peak_rss_kib / 1024.0, 2),
        "vault_files": len(before),
    }


def png_dimensions(path: Path):
    data = path.read_bytes()
    check(data[:8] == b"\x89PNG\r\n\x1a\n", f"非合法 PNG: {path}")
    return struct.unpack(">II", data[16:24])


def capture_l1_screenshots(gui_bin: Path, vault: Path, media_dir: Path):
    media_dir.mkdir(parents=True, exist_ok=True)
    specs = [
        {
            "filename": "l1-reading-markdown-guide-light.png",
            "label": "L1 阅读视图：Markdown 字段总览（浅色，隐藏源码标记）",
            "width": 1280,
            "height": 840,
            "color_scheme": "light",
            "open": ["Markdown field guide.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-markdown-guide-dark.png",
            "label": "L1 阅读视图：Markdown 字段总览（深色，隐藏源码标记）",
            "width": 1280,
            "height": 840,
            "color_scheme": "dark",
            "open": ["Markdown field guide.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-learning-map-light.png",
            "label": "L1 阅读视图：Learning map 与原生 Mermaid Mindmap 导图（浅色）",
            "width": 1280,
            "height": 840,
            "color_scheme": "light",
            "open": ["Learning/Learning map.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-mermaid-flow-seq-state-light.png",
            "label": "L1 阅读视图：Mermaid Flowchart（含 subgraph）、Sequence 与 State（浅色）",
            "width": 1280,
            "height": 900,
            "color_scheme": "light",
            "open": ["L1-mermaid-gallery.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-mermaid-class-er-light.png",
            "label": "L1 阅读视图：Mermaid Class 与 ER 图渲染（浅色）",
            "width": 1280,
            "height": 860,
            "color_scheme": "light",
            "open": ["L1-mermaid-gallery.md"],
            "extra": ["--scroll-fraction", "1.0"],
        },
        {
            "filename": "l1-reading-math-and-mermaid-showcase-light.png",
            "label": "L1 阅读视图：RaTeX 行内/CJK/块级公式与错误回退（浅色）",
            "width": 1280,
            "height": 860,
            "color_scheme": "light",
            "open": ["L1-math-mermaid-showcase.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-diagrams-table-images-light.png",
            "label": "L1 阅读视图：带公式表格、本地 PNG、净化 SVG 与远程占位（浅色）",
            "width": 1280,
            "height": 860,
            "color_scheme": "light",
            "open": ["L1-table-media-view.md"],
            "extra": [],
        },
        {
            "filename": "l1-reading-embeds-and-svg-dark.png",
            "label": "L1 阅读视图：标题切片、独立 .mmd 嵌入、循环报错与远程占位（深色）",
            "width": 1280,
            "height": 860,
            "color_scheme": "dark",
            "open": ["L1-embeds-dark-scene.md"],
            "extra": [],
        },
        {
            "filename": "l1-focus-mode-light.png",
            "label": "L1 Mermaid 专注模式：矢量缩放/适应/100%/导出工具栏（浅色）",
            "width": 1280,
            "height": 840,
            "color_scheme": "light",
            "open": ["Learning/Learning map.md"],
            "extra": ["--focus-diagram", "1"],
        },
        {
            "filename": "l1-edit-mode-dark.png",
            "label": "L1 Ctrl+E 源码编辑态与状态徽章（深色）",
            "width": 1280,
            "height": 840,
            "color_scheme": "dark",
            "open": ["Markdown field guide.md"],
            "extra": ["--edit"],
        },
    ]

    results = []
    before = snapshot_tree(vault)
    with tempfile.TemporaryDirectory(prefix="hortus-l1-shots-") as tmp:
        tmp_path = Path(tmp)
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
                *spec["extra"],
            ]
            for rel in spec["open"]:
                cmd.extend(["--open", rel])

            proc = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            deadline = time.monotonic() + 30.0
            sockets = []
            while proc.poll() is None:
                if time.monotonic() > deadline:
                    proc.kill()
                    raise AssertionError(f"截图 {spec['filename']} 超时")
                sockets.extend(process_network_sockets(proc.pid))
                time.sleep(0.06)
            stdout, stderr = proc.communicate(timeout=5)
            check(proc.returncode == 0, f"截图 {spec['filename']} 失败:\n{stderr}\n{stdout}")
            check(not sockets, f"截图 {spec['filename']} 发生意外网络套接字: {sockets}")
            check(out_png.is_file(), f"截图缺失: {out_png}")
            w, h = png_dimensions(out_png)
            check(w >= 1200 and h >= 800, f"截图尺寸不合期望: {w}x{h}")
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
    check(before == after, "截图过程改动了隔离知识库！")
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-reading")
    parser.add_argument("--media", type=Path, default=None)
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()

    gui_bin = discover_gui_bin(args.gui_bin)
    katex_dir = verify_nix_fonts_installed(gui_bin)
    start_private_display()

    with tempfile.TemporaryDirectory(prefix="hortus-l1-check-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "demo-vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", vault)
        populate_synthetic_l1_vault(vault)

        # 测试组 1：综合样本页（6 类 Mermaid + 异常降级 + RaTeX 块/行内/CJK/降级 + SVG清洗 + 表格 + 嵌入 + Focus/Export + Ctrl+E/Ctrl+J）
        env1 = isolated_env(tmp_path / "run1")
        run1 = run_reading_automation(
            gui_bin,
            vault,
            env1,
            [
                "--vault",
                str(vault),
                "--open",
                "L1-math-mermaid-showcase.md",
                "--open",
                "L1-standalone.mmd",
                "--open",
                "Markdown field guide.md",
                "--width",
                "1280",
                "--height",
                "860",
                "--color-scheme",
                "light",
            ],
            output_dir / "reading-showcase.json",
            "reading-showcase",
        )
        rep = run1["report"]

        # Fontconfig 非主线程注册与 20 KaTeX 字体核验
        fonts = rep["fonts"]
        check(fonts["registered"] is True, f"KaTeX 字体未成功注册: {fonts}")
        check(fonts["registered_count"] == 20, f"KaTeX 注册数量应为 20，实际 {fonts['registered_count']}")
        check(fonts["registered_off_main_thread"] is True, f"KaTeX 字体必须在后台线程注册: {fonts}")
        check(len(fonts["resolved_families"]) == 20, f"KaTeX FontMap 解析家族数不足 20: {fonts}")
        for key, fam in fonts["resolved_families"].items():
            check(fam.startswith("KaTeX_"), f"KaTeX 字体 {key} 未正确解析为 KaTeX_* 而是 {fam}")

        sc_stats = rep["tabs"]["L1-math-mermaid-showcase.md"]
        check(sc_stats["prepared_off_main_thread"] is True, "笔记 Reading 准备未在后台线程完成")
        check(sc_stats["diagram_rendered"] >= 6, f"应成功渲染全部 6 类 Mermaid 图表，实际 {sc_stats['diagram_rendered']}")
        check(sc_stats["diagram_fallback"] >= 2, f"应保留不支持/语法错误图表局部降级，实际 {sc_stats['diagram_fallback']}")
        check(sc_stats["math_rendered"] >= 3, f"RaTeX 渲染公式数偏少: {sc_stats}")
        check(sc_stats["math_fallback_error"] >= 2, f"错误公式应局部降级保留原文: {sc_stats}")
        check(len(sc_stats["math_cjk_fallback_codepoints"]) >= 2, f"应触发 CJK/Unicode 缺失度量 Pango 回测: {sc_stats}")
        check(sc_stats["math_feedback_rounds"] >= 1, "未记录缺字度量回填重排轮次")
        check(sc_stats["table_count"] >= 1, "未检测到表格")
        check(sc_stats["local_image_count"] >= 2, f"应解码本地 PNG 和 SVG 图片: {sc_stats}")
        check(sc_stats["remote_image_count"] >= 1, f"未检测到远程图片安全占位: {sc_stats}")
        check(sc_stats["sanitized_svg_external_refs"] >= 2, f"SVG 外部引用未清洗: {sc_stats}")
        check(sc_stats["sanitized_svg_scripts"] >= 3, f"SVG 脚本/事件属性未清洗: {sc_stats}")
        check(sc_stats["embed_count"] >= 6, f"嵌入+异常嵌入数量不足: {sc_stats}")
        check(
            set(["note", "diagram", "cycle", "heading_missing", "broken"]).issubset(
                set(sc_stats["embed_kinds"])
            ),
            f"嵌入类型不完整: {sc_stats['embed_kinds']}",
        )

        mmd_stats = rep["tabs"]["L1-standalone.mmd"]
        check(mmd_stats["diagram_rendered"] == 1, f"独立 .mmd 渲染失败: {mmd_stats}")

        check(rep["ctrl_j_sync_ok"] is True, "Ctrl+J 底部面板按钮状态双向同步失败")
        check(rep["ctrl_e_preserved_ok"] is True, "Ctrl+E 阅读/编辑视图切换未完成")
        check(rep["collapse_persisted_ok"] is True and len(rep["collapsed_nodes_readback"]) >= 1, "Mermaid 节点折叠持久化核验失败")
        check(rep["focus_mode_ok"] is True and rep["focus_snapshot"] is not None, "Mermaid 专注模式与视口持久化核验失败")
        check(rep["export_svg_valid"] is True and rep["export_svg_bytes"] > 100, "导出 SVG 校验失败")
        check(rep["export_png_valid"] is True and rep["export_png_bytes"] > 100, "导出 PNG 校验失败")
        check(rep["export_pdf_valid"] is True and rep["export_pdf_bytes"] > 100, "导出 PDF 校验失败")
        check(rep["wikilink_target_resolved"] is not None, "WikiLink 解析失败")

        # 截图采集与验证
        screenshots = capture_l1_screenshots(gui_bin, vault, media_dir)

    summary = {
        "passed": True,
        "gui_bin": str(gui_bin),
        "katex_fonts_dir": katex_dir,
        "reading_showcase": run1,
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
        f"PASS: katex_fonts=20(off_main), diagrams={sc_stats['diagram_rendered']}+2fallback+.mmd, "
        f"math={sc_stats['math_rendered']}(cjk_fb={len(sc_stats['math_cjk_fallback_codepoints'])})+{sc_stats['math_fallback_error']}err, "
        f"embeds={sc_stats['embed_count']}, svg_clean=yes, exports=SVG/PNG/PDF, "
        f"0 net, rss={run1['peak_rss_mib']} MiB, screenshots={len(screenshots)}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as err:
        print(f"FAIL: {err}", file=sys.stderr)
        raise SystemExit(1)
