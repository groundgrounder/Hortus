#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""L2 Linux 样式化编辑、安全保存、外部刷新、知识导航与文件管理端到端自检。"""

import argparse
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

ROOT = Path(__file__).resolve().parents[1]

SCENE_EXPECTATIONS = {
    "search-backlinks": {
        "filename": "l2-search-results-and-backlinks.png",
        "label": "左侧全文搜索结果 + 右侧反链面板",
        "open": ["Start here.md"],
        "extra": [],
    },
    "tags": {
        "filename": "l2-typography-table-and-tags.png",
        "label": "左侧标签浏览 + 正文阅读视图",
        "open": ["L2-编辑目标.md"],
        "extra": [],
    },
    "quick-open": {
        "filename": "l2-quick-open-dialog.png",
        "label": "Ctrl+P 快速打开弹窗",
        "open": ["Start here.md"],
        "extra": [],
    },
    "edit-find-completion": {
        "filename": "l2-edit-find-and-completion.png",
        "label": "编辑视图：查找替换条 + [[ 双链补全候选",
        "open": ["L2-补全场景.md"],
        "extra": [],
    },
    "rename-links": {
        "filename": "l2-rename-link-preview.png",
        "label": "重命名改链预览对话框",
        "open": ["L2-被引用.md"],
        "extra": [],
    },
}

ACCEL_EXPECTATIONS = {
    "win.save-note": "<Control>s",
    "win.find-in-note": "<Control>f",
    "win.replace-in-note": "<Control>h",
    "win.quick-open": "<Control>p",
    "win.new-note": "<Control>n",
    "win.focus-search": "<Control><Shift>f",
    "win.align-table": "<Control><Shift>t",
    "win.toggle-edit-mode": "<Control>e",
}


def normalize_accel(accel: str) -> str:
    """GTK 返回的修饰键顺序不固定：按修饰键集合 + 主键归一化比较。"""
    parts = [part for part in accel.split(">") if part]
    modifiers = sorted(part.lower() for part in parts[:-1])
    key = parts[-1] if parts else ""
    return "".join(f"<{modifier}>" for modifier in modifiers) + key


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def start_private_display():
    """在私有 Xvfb 中运行 GUI：测试窗口不会出现在真实桌面上，也不抢焦点。"""
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
    return proc


def discover_gui_bin(explicit: Path | None) -> Path:
    if explicit is not None:
        check(explicit.is_file(), f"指定的 GUI 二进制不存在: {explicit}")
        return explicit.resolve()
    cmd = [
        "cargo",
        "build",
        "--manifest-path",
        str(ROOT / "apps/linux/Cargo.toml"),
        "--features",
        "automation",
        "--offline",
    ]
    if shutil.which("cargo") is None:
        cmd = ["nix", "develop", "--command"] + cmd
    subprocess.run(cmd, cwd=ROOT, check=True)
    built = ROOT / "apps/linux/target/debug/hortus-gtk"
    check(built.is_file(), f"构建 automation GUI 二进制后未找到输出: {built}")
    return built.resolve()


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
    for directory in (cfg, cache, data):
        directory.mkdir(parents=True, exist_ok=True)
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


def count_files(directory: Path):
    if not directory.is_dir():
        return []
    return sorted(path.name for path in directory.iterdir())


def populate_l2_vault(vault: Path):
    """在隔离副本中生成 L2 专用样本；不修改仓库 fixtures。"""
    # 1 MiB 真实密度文档：每节约 5 KiB 正文，少量标题与样式，避免病态块数量拖慢阅读渲染。
    big_parts = ["# L2 大文档\n\n"]
    plain = (
        "这是一段用于线性样式映射与滚动定位的普通正文，包含中文与 ASCII 混排，"
        "用于让 1 MiB 文档保持真实笔记的内容密度。"
    )
    styled = (
        "本节包含 **粗体**、`代码`、[链接](https://example.org) 与 #demo 标签，"
        "用于验证样式映射。\n\n"
    )
    index = 0
    size = 0
    while size < 1024 * 1024:
        section = [f"## 章节 {index}\n\n"]
        section_size = len(section[0].encode("utf-8"))
        while section_size < 5 * 1024:
            section.append(plain)
            section_size += len(plain.encode("utf-8"))
        section.append("\n\n")
        section.append(styled)
        fragment = "".join(section)
        big_parts.append(fragment)
        size += len(fragment.encode("utf-8"))
        index += 1
    (vault / "L2-大文档.md").write_text("".join(big_parts), encoding="utf-8")

    (vault / "L2-编辑目标.md").write_text(
        """---
tags: [demo, l2target]
---

# L2 编辑与保存目标

Hortus 编辑目标用于验证增量同步、原子保存与外部刷新。第二次出现 Hortus 供替换测试。

## 列表与任务

- [ ] 待办任务一
- [x] 已完成任务
1. 有序条目

## 表格与外链

| 项目 | 说明 |
| :--- | :--- |
| 规范指南 | [规范指南](https://example.org) |
| 公式 | 坏行内 $\\unknowninlinecmd{x}$ 后仍保持正文 |

## 外部刷新锚点

Hortus 第三次出现，用于滚动位置保持检查。

- 内容一行
- 内容二行
- 内容三行

## 结尾

最后一节用于让页面可滚动。
""",
        encoding="utf-8",
    )
    (vault / "L2-补全场景.md").write_text(
        """# L2 补全场景

Hortus 补全场景用于截图，正文保持在首屏内。

- 列表项一
- 列表项二

任务：- [ ] 待办

""",
        encoding="utf-8",
    )
    (vault / "L2-被引用.md").write_text("# 被引用笔记\n\n这是被其它笔记引用的目标。\n", encoding="utf-8")
    (vault / "L2-引用者.md").write_text(
        "# 引用者\n\n这里引用 [[L2-被引用]]，重命名后应自动更新。\n",
        encoding="utf-8",
    )
    (vault / "L2-组").mkdir(exist_ok=True)
    (vault / "L2-组/子笔记.md").write_text(
        "# 组内子笔记\n\n用于文件夹重命名改链测试。\n", encoding="utf-8"
    )
    (vault / "L2-引用组.md").write_text(
        "# 引用组\n\n嵌入 ![[L2-组/子笔记]] 应在文件夹重命名后更新。\n", encoding="utf-8"
    )
    (vault / "L2-待删除.md").write_text("# 待删除笔记\n\n将移入隔离回收站。\n", encoding="utf-8")
    # 阅读态双链点击回归样本：已解析、缺失、歧义与带标题四类目标。
    (vault / "L2-双链边界.md").write_text(
        """# 双链边界

- [[Learning map]]
- [[Study loop]]
- [[L2 不存在的目标]]
- [[Learning map#Learning map|带标题的链接]]
- [[同名]]
""",
        encoding="utf-8",
    )
    (vault / "L2-重名一").mkdir(exist_ok=True)
    (vault / "L2-重名二").mkdir(exist_ok=True)
    (vault / "L2-重名一/同名.md").write_text("# 同名 A\n\n歧义目标 A。\n", encoding="utf-8")
    (vault / "L2-重名二/同名.md").write_text("# 同名 B\n\n歧义目标 B。\n", encoding="utf-8")
    (vault / "L2-独立图表.mmd").write_text(
        "flowchart LR\n  A[Start] --> B[Analyze]\n  B --> C[Synthesize]\n  C --> D[Review]\n",
        encoding="utf-8",
    )


def supervise(cmd, env, label: str, timeout: float = 180.0):
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    sockets = []
    samples = 0
    peak_rss_kib = 0
    deadline = time.monotonic() + timeout
    timed_out = False
    while proc.poll() is None:
        if time.monotonic() > deadline:
            timed_out = True
            proc.kill()
            break
        sockets.extend(process_network_sockets(proc.pid))
        rss = read_rss_kib(proc.pid)
        if rss and rss > peak_rss_kib:
            peak_rss_kib = rss
        samples += 1
        time.sleep(0.05)
    stdout, stderr = proc.communicate(timeout=15)
    if timed_out:
        tail = "\n".join(stderr.strip().splitlines()[-25:])
        raise AssertionError(f"{label} 运行超时 ({timeout}s)\nstderr tail:\n{tail}")
    return {
        "label": label,
        "returncode": proc.returncode,
        "stdout": stdout,
        "stderr": stderr,
        "sockets": sockets,
        "network_samples": samples,
        "peak_rss_kib": peak_rss_kib,
        "peak_rss_mib": round(peak_rss_kib / 1024.0, 2),
    }


def assert_clean_run(result, label: str):
    check(
        result["returncode"] == 0,
        f"{label} 退出码 {result['returncode']}\nstdout={result['stdout']}\nstderr={result['stderr']}",
    )
    check(not result["sockets"], f"{label} 检测到网络连接：{result['sockets'][:4]}")
    check(
        "exceeds AdwApplicationWindow width" not in result["stderr"],
        f"{label} 出现 ToastOverlay 最小宽度警告：\n{result['stderr']}",
    )
    check(
        "AdwToastOverlay" not in result["stderr"],
        f"{label} 出现 AdwToastOverlay 宽度警告：\n{result['stderr']}",
    )
    check(
        "exceeds" not in result["stderr"].lower(),
        f"{label} 出现最小宽度 exceeds 警告：\n{result['stderr']}",
    )
    check(
        "panicked at" not in result["stderr"],
        f"{label} 出现 panic：\n{result['stderr']}",
    )


def png_dimensions(path: Path):
    data = path.read_bytes()
    check(data[:8] == b"\x89PNG\r\n\x1a\n", f"非合法 PNG: {path}")
    return struct.unpack(">II", data[16:24])


def audit_static_compliance(gui_bin: Path):
    linux_ttf = [
        path for path in (ROOT / "apps/linux").rglob("*.ttf") if "target" not in path.parts
    ]
    check(not linux_ttf, f"apps/linux 内不得复制 KaTeX TTF：{linux_ttf}")
    html_js = [
        str(path.relative_to(ROOT))
        for path in (ROOT / "apps/linux").rglob("*")
        if "target" not in path.parts and path.suffix.lower() in {".html", ".js"}
    ]
    check(not html_js, f"apps/linux 内不得包含 HTML/JS 资源：{html_js}")

    elf = gui_bin
    if not elf.read_bytes().startswith(b"\x7fELF"):
        sibling = gui_bin.parent / f".{gui_bin.name}-wrapped"
        if sibling.is_file():
            elf = sibling
    ldd = subprocess.run(
        ["ldd", str(elf)], capture_output=True, text=True, check=False
    )
    linked = ldd.stdout.lower()
    check("webkit" not in linked, f"GUI 二进制链接了 WebKit：\n{ldd.stdout}")
    check("javascriptcore" not in linked, f"GUI 二进制链接了 JavaScriptCore：\n{ldd.stdout}")

    # Nix 产物通过 --remap-path-prefix 消除本机路径；开发目录构建会保留源码路径，
    # 因此只在 Nix store 产物上执行路径泄漏审计（正式验收使用 .#hortus-linux）。
    nix_build = str(gui_bin).startswith("/nix/store/")
    data = elf.read_bytes()
    leaks = []
    if nix_build:
        leaks = [
            needle
            for needle in (b"/home/", b"/Users/", bytes(str(Path.home()), "utf-8"), b"/build/source")
            if needle in data
        ]
        check(not leaks, f"GUI 二进制包含本机路径泄漏：{leaks}")
    return {
        "webkit": False,
        "javascriptcore": False,
        "path_leaks": len(leaks),
        "path_audit_skipped_dev_build": not nix_build,
        "html_js_files": 0,
    }


def assert_report(report: dict, vault: Path, env: dict, real_trash_before: list[str]):
    check(report.get("initial_ready") is True, "界面未在 20s 内完成初始加载")
    check(report.get("temp_dir_outside_vault") is True, "保存临时目录位于笔记库内")
    accels = {item["action"]: item["accels"] for item in report.get("accels", [])}
    for action, expected in ACCEL_EXPECTATIONS.items():
        values = accels.get(action) or []
        normalized = [normalize_accel(value) for value in values]
        check(
            normalize_accel(expected) in normalized,
            f"{action} 缺少快捷键 {expected}：{values}",
        )

    edit = report["edit"]
    check(edit["typed_synced"] is True, "编辑输入未同步到 DocumentSession")
    check(edit["session_matches_buffer"] is True, "会话正文与编辑器不一致")
    check(edit["dirty_marker"] is True, "脏状态未在标签标题显示圆点")
    check(edit["styles_clear_of_undo"] is True, "样式标签进入了原生撤销栈")
    check(edit["ctrl_s_saved"] is True, "Ctrl+S 未完成原子保存")
    check(edit["disk_matches_session"] is True, "磁盘内容与会话正文不一致")
    preview_sync = edit.get("edit_preview_sync") or {}
    check(preview_sync.get("unsaved_reflected") is True, f"未保存编辑切回阅读未立即刷新：{preview_sync}")
    check(preview_sync.get("saved_ok") is True, f"编辑后保存失败：{preview_sync}")
    check(preview_sync.get("saved_reflected") is True, f"已保存编辑切回阅读未刷新：{preview_sync}")
    check(preview_sync.get("outline_updated") is True, f"编辑保存切回阅读后右侧大纲未更新：{preview_sync}")
    check(edit["temp_leftovers_in_vault"] == [], f"笔记库内残留临时文件：{edit['temp_leftovers_in_vault']}")
    check(edit["checkpoint_written"] is True, "脏状态未写入恢复稿")
    check(edit["checkpoint_cleared_after_save"] is True, "保存成功后恢复稿未清理")
    check(edit["undo_redo"]["undo_changed"] is True, "原生撤销未回退正文")
    check(edit["undo_redo"]["redo_restored"] is True, "原生重做未恢复正文")
    check(edit["list_continuation_helper"] is True, "列表续写推断失败")
    check(edit["list_continuation_live"] is True, "真实回车未续写列表前缀")
    check(edit["task_toggle_live"] is True, "任务勾选框切换未生效")
    check(edit["align_table_patch"] is True, "表格自动对齐未改写正文")
    check(edit["autosaved"] is True, "停止输入后未自动保存")
    check(900 <= edit["autosave_ms"] <= 6000, f"自动保存延迟异常：{edit['autosave_ms']}ms")
    check(edit.get("queued_save_followup") is True, f"save_inflight 期间尾随输入补保存失败：{edit}")

    external = report["external"]
    check(external["clean_reload"] is True, "干净态外部修改未实时刷新")
    check(external["scroll_preserved"] is True, f"外部刷新后滚动位置跳动：{external}")
    rec_ro = external.get("recovery_readonly") or {}
    check(
        rec_ro.get("seeded") is True and rec_ro.get("locked") is True and rec_ro.get("unlocked") is True,
        f"恢复稿未决期间只读锁定失败：{rec_ro}",
    )
    ro_lock = external.get("readonly_disk_lock") or {}
    check(
        ro_lock.get("opened_editable") is True and ro_lock.get("locked") is True and ro_lock.get("unlocked") is True,
        f"磁盘只读刷新锁定/解锁失败：{ro_lock}",
    )
    conflict = external["conflict"]
    check(conflict["shown"] is True, "脏态外部修改未显示 Conflict 提示条")
    check(conflict["local_preserved"] is True, "冲突时本地输入被覆盖")
    check(conflict["keep_mine_saved"] is True, "保留我的修改未保存")
    check(conflict["disk_has_local"] is True, "保留我的修改后磁盘缺少本地内容")
    check(conflict["disk_has_external_only"] is False, "外部冲突内容覆盖了用户正文")
    missing = external["missing"]
    check(missing["shown"] is True, "外部删除未显示 Missing 提示条")
    check(missing["text_preserved"] is True, "外部删除后内存正文丢失")
    check(missing["recreated"] is True and missing["disk_recreated"] is True, "重新保存到原路径失败")
    guard = external["close_guard"]
    check(guard["alert_shown"] is True, "关闭未保存标签页时未弹出原生对话框")
    check(guard["tab_kept"] is True, "关闭保护失败：标签页被静默关闭")
    check(guard["text_kept"] is True, "关闭保护失败：未保存正文丢失")
    rec_draft_close = external.get("recovery_draft_closes_cleanly") or {}
    check(rec_draft_close.get("closed_cleanly") is True and rec_draft_close.get("alert_shown") is False, f"RecoveryDraft 关闭未免弹窗放行：{rec_draft_close}")
    edit_reload = external.get("edit_reload_preserve") or {}
    check(edit_reload.get("cursor_preserved") is True and edit_reload.get("scroll_preserved") is True, f"编辑态干净重载未保留光标或滚动位置：{edit_reload}")
    window_guard = external["window_close_guard"]
    check(window_guard["alert_shown"] is True, "关闭窗口时未弹出未保存保护对话框")
    check(window_guard["window_kept"] is True, "关闭窗口保护失败：窗口被静默退出")
    folder_missing = external["folder_missing"]
    check(folder_missing["loaded"] is True, "文件夹级删除样本未加载")
    check(folder_missing["missing_shown"] is True, f"文件夹级外部删除未提示缺失：{folder_missing}")
    check(folder_missing["tabs_kept"] is True, "文件夹级外部删除后标签页丢失")

    find = report["find"]
    check(find["opened"] is True and find["matches_found"] is True, "Ctrl+F 查找未打开或无匹配")
    check(find["next_advances"] is True, "查找的下一个匹配未前进")
    check(find["replace_all"]["session_has_replacement"] is True, "全部替换未写入正文")
    check(find["replace_all"]["session_has_no_query"] is True, "全部替换后仍残留旧查询文本")
    check(find.get("stale_guard_ok") is True, f"编辑态修改正文后查找替换 stale 防护失败：{find}")
    check(find["closed"] is True, "Esc 未关闭查找条")
    check(find["reading_view"]["open"] is True, "阅读视图未打开查找条")
    check(find["reading_view"]["editable"] is False, "阅读视图不应展开替换行")
    check(find["reading_view"]["matches"] > 0, "阅读视图查找无匹配")
    cross_find = find.get("reading_cross_segment") or {}
    check(cross_find.get("open") is True and cross_find.get("editable") is False, f"跨段阅读查找条状态异常：{cross_find}")
    check(cross_find.get("matches", 0) >= 2 and cross_find.get("segments", 0) >= 2, f"阅读视图未跨多 TextSegment 命中：{cross_find}")
    check(cross_find.get("visited_segments", 0) >= 2, f"Next 未跨越多个 TextSegment：{cross_find}")

    big = report["big_doc"]
    check(big["loaded"] is True, "1 MiB 文档未打开")
    check(big["bytes"] >= 1_000_000, f"大文档字节数偏小：{big['bytes']}")
    check(big["style_runs"] > 0, "大文档未产出样式片段")
    check(
        big["plan_under_200ms"] is True,
        f"线性偏移映射超过 200ms：{big['style_plan_best_micros']}us",
    )
    check(
        big["load_style_under_200ms"] is True,
        f"1 MiB 打开时的样式应用超过 200ms：{big['load_style_micros']}us",
    )
    check(big["style_tag_best_micros"] > 0, "未记录标签应用耗时")

    knowledge = report["knowledge"]
    search = knowledge["search"]
    check(search["initial_groups"] > 0, "全文搜索没有结果")
    check(search["concurrent_result_matches"] is True, f"并发搜索串结果：{search}")
    check(search["cancellations"] >= 1, "并发输入未取消旧查询")
    check(search["clicked_group"] is True, "搜索结果点击未被处理")
    tags = knowledge["tags"]
    check(tags["total_tags"] > 0, "标签面板没有标签")
    check(tags["tag_clicked"] is True and tags["selected_after_click"], "标签点击未选中")
    check(tags["selected_notes"] > 0, "选中标签后没有笔记")
    check(tags["reading_click_switches_panel"] is True, "阅读视图 #标签 点击未切到标签面板")
    check(bool(tags["filter_after_reading_click"]), "阅读视图标签点击未设置过滤")
    check(tags["clicked_note"] is True, "标签面板笔记点击未处理")
    backlinks = knowledge["backlinks"]
    check(backlinks["first_rows"] > 0, "反链面板没有数据")
    check(backlinks["second_path"] == "README.md", f"切换标签后反链未更新：{backlinks}")
    check(backlinks["switched"] is True, "反链未随当前标签页刷新")
    check(backlinks["clicked_row"] is not None, "反链行点击未处理")
    quick = knowledge["quick_open"]
    check(quick["hits"] > 0, "快速打开没有命中")
    check(quick.get("pending_blocked_enter") is True, f"QuickOpen 未在查询未就绪前拦截陈旧命中：{quick}")
    check(quick["closed_after_enter"] is True, "快速打开在 Enter 后未关闭")
    check(quick["opened_first"] is True, "快速打开未打开首个命中")
    completion = knowledge["completion"]
    check(completion["visible"] is True and completion["items"] > 0, "[[ 补全未出现候选")
    check(completion["typed_after_popup"] is True, "补全弹出后继续输入被打断")
    check(completion["popover_focusable"] is False, "补全弹层可获取键盘焦点")
    if completion.get("focus_baseline"):
        check(
            completion["view_kept_focus"] is True,
            "补全弹层弹出后编辑视图失去了键盘焦点",
        )
    check(completion["accepted"] is True, "[[ 补全接受失败")
    check(completion.get("embed_kept_bang") is True, f"![[ 嵌入补全丢失感叹号前缀：{completion}")
    check(completion["stats"]["accepted"] > 0, "[[ 补全未记录接受次数")
    check(completion["target"], "[[ 补全未给出插入目标")

    files = report["files"]
    create = files["create"]
    check(create["note_opened"] is True, "新建笔记未自动打开")
    check(create["note_on_disk"] is True, "新建笔记未落盘")
    check(create["folder_on_disk"] is True, "新建文件夹未落盘")
    rename_standalone = files.get("rename_standalone") or {}
    rename_standalone_op = rename_standalone.get("op") or {}
    check(
        rename_standalone.get("ok") is True
        and rename_standalone_op.get("op") == "rename"
        and rename_standalone_op.get("link_error") is None
        and rename_standalone_op.get("rewrites") == 0,
        f"无引用孤立笔记重命名失败：{rename_standalone}",
    )
    rename = files["rename_note"]
    check(rename["tab_migrated"] is True, "重命名后标签页未迁移")
    check(rename.get("renamed_closure_ok") is True, f"改名后标签页视口重排/自动保存闭合失败：{rename}")
    check(rename["referrer_updated"] is True, "重命名未更新引用笔记双链")
    check(rename["referrer_keeps_old"] is False, "重命名后仍残留旧双链")
    check(rename["op"] and rename["op"]["updated_notes"] >= 1, f"改链统计缺失：{rename}")
    folder = files["rename_folder"]
    check(folder["tab_migrated"] is True, "文件夹重命名后子笔记标签未迁移")
    check(folder["embed_updated"] is True, "文件夹重命名未更新嵌入链接")
    trash = files["trash"]
    check(trash["done"] is True and trash["gone"] is True, "移到回收站失败")
    check(trash["tab_closed"] is True, "回收站删除后标签页未关闭")
    check(trash["real_trash_untouched"] is True, "污染了用户真实回收站")
    check(trash["op"], "回收站操作缺少结构化结果")
    check("L2-待删除.md" in trash["op"]["trash_files"], "隔离回收站内未找到文件")

    misc = report["misc"]
    check(misc["inline_fallback_baseline"] is True, "行内错误公式未使用基线对齐")
    check(misc["table_external_links"]["detected"] >= 1, "表格单元格未识别标准外链")
    check(report["temp_leftovers_final"] == [], f"结束时库内残留临时文件：{report['temp_leftovers_final']}")

    # 文件系统终态
    check((vault / "L2-新笔记.md").is_file(), "新建笔记文件缺失")
    check((vault / "L2-新建文件夹").is_dir(), "新建文件夹缺失")
    check((vault / "L2-被引用改名.md").is_file(), "重命名后的笔记文件缺失")
    check(not (vault / "L2-被引用.md").exists(), "重命名后旧文件仍存在")
    check((vault / "L2-组新/子笔记.md").is_file(), "文件夹重命名后子笔记缺失")
    check(not (vault / "L2-待删除.md").exists(), "回收站删除后原路径仍存在")
    referrer = (vault / "L2-引用者.md").read_text(encoding="utf-8")
    check("[[L2-被引用改名" in referrer, "引用者笔记未更新双链")
    group_referrer = (vault / "L2-引用组.md").read_text(encoding="utf-8")
    check("![[L2-组新/子笔记" in group_referrer, "引用组笔记未更新嵌入链接")
    edited = (vault / "L2-编辑目标.md").read_text(encoding="utf-8")
    check("冲突前本地输入" in edited, "保留我的修改未落盘")
    check("L2Find" in edited, "全部替换结果未落盘")
    trash_dir = Path(env["XDG_DATA_HOME"]) / "Trash/files"
    check((trash_dir / "L2-待删除.md").is_file(), f"隔离回收站缺失文件：{count_files(trash_dir)}")
    real_trash_after = count_files(Path.home() / ".local/share/Trash/files")
    check(
        real_trash_after == real_trash_before,
        f"用户真实回收站被污染：{real_trash_before} -> {real_trash_after}",
    )
    check(report["real_trash_after"] == len(real_trash_before), "应用侧真实回收站计数不一致")
    return {
        "edit": edit,
        "external": external,
        "find": find,
        "big_doc": big,
        "knowledge": knowledge,
        "files": files,
        "misc": misc,
    }


def capture_scenes(gui_bin: Path, vault: Path, media_dir: Path, tmp_path: Path):
    media_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for index, (scene, spec) in enumerate(SCENE_EXPECTATIONS.items()):
        out_png = media_dir / spec["filename"]
        out_png.unlink(missing_ok=True)
        for stale in media_dir.glob(f"{out_png.stem}-dialog*.png"):
            stale.unlink(missing_ok=True)
        env = isolated_env(tmp_path / f"scene-{index}")
        cmd = [
            str(gui_bin),
            "--vault",
            str(vault),
            "--width",
            "1280",
            "--height",
            "860",
            "--color-scheme",
            "light",
            "--edit-knowledge-scene",
            scene,
            "--screenshot",
            str(out_png),
            *spec["extra"],
        ]
        for rel in spec["open"]:
            cmd.extend(["--open", rel])
        run = supervise(cmd, env, f"截图 {scene}", timeout=90)
        assert_clean_run(run, f"截图 {scene}")
        check(out_png.is_file(), f"截图缺失: {out_png}")
        width, height = png_dimensions(out_png)
        check(width >= 1200 and height >= 800, f"截图尺寸异常: {width}x{height}")
        check(out_png.stat().st_size > 40_000, f"截图内容过于简单: {out_png.stat().st_size} bytes")
        dialogs = []
        if scene in {"quick-open", "rename-links"}:
            dialog_png = out_png.with_name(f"{out_png.stem}-dialog1.png")
            check(dialog_png.is_file(), f"{scene} 未捕获原生 AdwDialog 表面：{dialog_png}")
            dw, dh = png_dimensions(dialog_png)
            check(dw >= 320 and dh >= 180, f"{scene} 对话框截图尺寸异常：{dw}x{dh}")
            check(
                dialog_png.stat().st_size > 5000,
                f"{scene} 对话框截图内容过于简单：{dialog_png.stat().st_size} bytes",
            )
            dialogs.append(
                {
                    "filename": dialog_png.name,
                    "path": str(dialog_png.resolve()),
                    "width": dw,
                    "height": dh,
                    "size_bytes": dialog_png.stat().st_size,
                    "sha256": hashlib.sha256(dialog_png.read_bytes()).hexdigest(),
                }
            )
        results.append(
            {
                "scene": scene,
                "label": spec["label"],
                "filename": spec["filename"],
                "path": str(out_png.resolve()),
                "width": width,
                "height": height,
                "size_bytes": out_png.stat().st_size,
                "sha256": hashlib.sha256(out_png.read_bytes()).hexdigest(),
                "dialogs": dialogs,
                "peak_rss_mib": run["peak_rss_mib"],
            }
        )
    return results


def narrow_width_run(gui_bin: Path, vault: Path, tmp_path: Path, media_dir: Path):
    """窄屏截图属于过程媒体：一律写 media 目录，--output 只保留 JSON/日志。"""
    results = []
    media_dir.mkdir(parents=True, exist_ok=True)
    for index, extra in enumerate(([], ["--edit"])):
        env = isolated_env(tmp_path / f"narrow-{index}")
        png = media_dir / f"narrow-994-{index}.png"
        cmd = [
            str(gui_bin),
            "--vault",
            str(vault),
            "--open",
            "L2-编辑目标.md",
            "--width",
            "994",
            "--height",
            "700",
            "--screenshot",
            str(png),
            *extra,
        ]
        run = supervise(cmd, env, f"窄屏 994px 运行 {index}", timeout=90)
        assert_clean_run(run, f"窄屏 994px 运行 {index}")
        width, height = png_dimensions(png)
        check(
            960 <= width <= 1000,
            f"窄屏截图宽度异常: {width}",
        )
        results.append(
            {
                "label": "994px 无 AdwToastOverlay 宽度警告",
                "stderr_exceeds_warning": False,
                "png": str(png.resolve()),
                "width": width,
                "height": height,
                "edit_mode": bool(extra),
            }
        )
    return results


def run_wikilink_regression(
    gui_bin: Path, vault: Path, tmp_path: Path, output_dir: Path, screenshot: Path | None
):
    """仅打开 Start here.md，用真实 GestureClick 释放事件点击阅读视图双链。"""
    env = isolated_env(tmp_path / "wikilink")
    report_path = output_dir / "wikilink-report.json"
    report_path.unlink(missing_ok=True)
    cmd = [
        str(gui_bin),
        "--vault",
        str(vault),
        "--open",
        "Start here.md",
        "--width",
        "1280",
        "--height",
        "860",
        "--color-scheme",
        "light",
        "--wikilink-json",
        str(report_path),
    ]
    if screenshot is not None:
        screenshot.unlink(missing_ok=True)
        cmd.extend(["--screenshot", str(screenshot)])
    run = supervise(cmd, env, "L2 双链点击回归", timeout=150)
    assert_clean_run(run, "L2 双链点击回归")
    check(report_path.is_file(), "双链回归未生成 JSON 报告")
    report = json.loads(report_path.read_text(encoding="utf-8"))
    check(report.get("initial_ready") is True, "双链回归初始笔记未就绪")
    check(
        report.get("completed") is True,
        f"双链回归未完成：stages={report.get('stages')}",
    )
    initial_tabs = report.get("initial_tabs") or []
    check(initial_tabs == ["Start here.md"], f"双链回归应仅打开 Start here.md：{initial_tabs}")
    click = report.get("learning_map_click") or {}
    check(click.get("clicked") is True, f"未命中 Learning map 双链：{click}")
    opened = report.get("learning_map_open") or {}
    check(opened.get("loaded") is True, f"Learning map 未加载：{opened}")
    check(opened.get("active_is_target") is True, f"点击后新标签未激活：{opened}")
    check(
        opened.get("diagram_ok") is True and opened.get("diagram_nodes", 0) > 0,
        f"Mermaid 大图未渲染节点：{opened}",
    )
    study = report.get("study_loop_click") or {}
    check(
        study.get("clicked") is True and study.get("active_is_target") is True,
        f"Study loop 双链点击失败：{study}",
    )
    missing = report.get("missing_target_click") or {}
    check(
        missing.get("clicked") is True and missing.get("toast_shown") is True,
        f"缺失目标未走 toast 分支：{missing}",
    )
    check(missing.get("tab_count_unchanged") is True, f"缺失目标意外打开标签：{missing}")
    ambiguous = report.get("ambiguous_target_click") or {}
    check(
        ambiguous.get("clicked") is True and ambiguous.get("toast_shown") is True,
        f"歧义目标未走 toast 分支：{ambiguous}",
    )
    heading = report.get("heading_link_click") or {}
    check(
        heading.get("clicked") is True and heading.get("active_is_map") is True,
        f"带标题双链未激活目标笔记：{heading}",
    )
    bare = report.get("bare_target_resolve") or {}
    check(bare.get("active_is_study_loop") is True, f"裸目标解析分支失败：{bare}")
    check(
        report.get("learning_map_widths_ready") is True,
        "Learning map 宽度未达到默认窗口目标（>=520px）",
    )
    widths = report.get("learning_map_widths") or {}
    widget_widths = widths.get("diagram_widget_widths") or []
    scene_widths = widths.get("diagram_scene_widths") or []
    check(widget_widths and widget_widths[0] >= 520, f"Learning map 图表控件被压扁：{widths}")
    check(scene_widths and scene_widths[0] >= 520, f"Learning map 图表场景宽不足：{widths}")
    content_width = widths.get("content_width_measured", 0)
    check(
        scene_widths[0] <= content_width + 24,
        f"Learning map 图表横向溢出：scene={scene_widths[0]} content={content_width}",
    )
    shot = None
    if screenshot is not None:
        check(screenshot.is_file(), f"双链公开复核截图缺失：{screenshot}")
        width, height = png_dimensions(screenshot)
        check(width >= 1200 and height >= 800, f"双链截图尺寸异常：{width}x{height}")
        check(screenshot.stat().st_size > 40_000, f"双链截图内容过于简单：{screenshot.stat().st_size} bytes")
        shot = {
            "path": str(screenshot.resolve()),
            "width": width,
            "height": height,
            "size_bytes": screenshot.stat().st_size,
            "sha256": hashlib.sha256(screenshot.read_bytes()).hexdigest(),
        }
    return {
        "returncode": run["returncode"],
        "network_connections": len(run["sockets"]),
        "peak_rss_mib": run["peak_rss_mib"],
        "report_path": str(report_path.resolve()),
        "learning_map_nodes": opened.get("diagram_nodes"),
        "learning_map_widths": widths,
        "screenshot": shot,
        "report": report,
    }


def run_layout_width(
    gui_bin: Path,
    vault: Path,
    tmp_path: Path,
    output_dir: Path,
    name: str,
    note: str,
    width: int,
    height: int,
    hide_sidebars: bool = False,
    screenshot: Path | None = None,
):
    """私有离屏测量阅读栏真实宽度与内联块分配宽；可选收起侧栏验证动态重排。"""
    env = isolated_env(tmp_path / f"layout-{name}")
    report_path = output_dir / f"layout-{name}.json"
    report_path.unlink(missing_ok=True)
    cmd = [
        str(gui_bin),
        "--vault",
        str(vault),
        "--open",
        note,
        "--width",
        str(width),
        "--height",
        str(height),
        "--color-scheme",
        "light",
        "--layout-width-json",
        str(report_path),
    ]
    if hide_sidebars:
        cmd.append("--layout-hide-sidebars")
    if screenshot is not None:
        screenshot.unlink(missing_ok=True)
        cmd.extend(["--screenshot", str(screenshot)])
    label = f"宽度适配 {name} ({width}x{height})"
    run = supervise(cmd, env, label, timeout=120)
    assert_clean_run(run, label)
    check(report_path.is_file(), f"{label} 未生成 JSON 报告")
    report = json.loads(report_path.read_text(encoding="utf-8"))
    check(report.get("ready") is True, f"{label} 首屏未就绪：{report}")
    check(report.get("allocated") is True, f"{label} 阅读栏未分配宽度：{report}")
    if hide_sidebars:
        check(report.get("reflowed") is True, f"{label} 收起侧栏后未完成动态重排：{report}")
    after = report.get("after") or {}
    check(after.get("found") is True, f"{label} 缺少阅读表面：{report}")

    shot = None
    if screenshot is not None:
        check(screenshot.is_file(), f"{label} 截图缺失：{screenshot}")
        shot_width, shot_height = png_dimensions(screenshot)
        check(
            shot_width >= width - 30 and shot_height >= height - 30,
            f"{label} 截图尺寸异常：{shot_width}x{shot_height}",
        )
        check(
            screenshot.stat().st_size > 30_000,
            f"{label} 截图内容过于简单：{screenshot.stat().st_size} bytes",
        )
        shot = {
            "path": str(screenshot.resolve()),
            "width": shot_width,
            "height": shot_height,
            "size_bytes": screenshot.stat().st_size,
            "sha256": hashlib.sha256(screenshot.read_bytes()).hexdigest(),
        }
    return {
        "returncode": run["returncode"],
        "network_connections": len(run["sockets"]),
        "peak_rss_mib": run["peak_rss_mib"],
        "report_path": str(report_path.resolve()),
        "screenshot": shot,
        "report": report,
    }


def verify_diagram_widths(layout_1280: dict, layout_wide: dict, layout_narrow: dict):
    """A5 宽度适配与阅读区左留白断言：默认窗、收起侧栏扩栏与 994px 窄平铺。"""
    base = layout_1280["report"]["after"]
    base_widgets = base.get("diagram_widget_widths") or []
    base_scenes = base.get("diagram_scene_widths") or []
    base_content = base.get("content_width_measured", 0)
    base_left_x = base.get("first_text_scrolled_x", 0.0)
    check(base.get("surface_rebuilds", -1) == 0, f"1280 不应重建 ReadingSurface：{base}")
    check(
        base_left_x >= 20.0,
        f"1280x860 双侧栏阅读区左留白不足（需 >=20px，实测 {base_left_x}px）：{base}",
    )
    check(base_widgets and base_widgets[0] >= 520, f"1280 图表控件被压扁：{base}")
    check(base_scenes and base_scenes[0] >= 520, f"1280 图表场景宽不足：{base}")
    check(
        base_scenes[0] <= base_content + 24,
        f"1280 图表横向溢出：scene={base_scenes[0]} content={base_content}",
    )
    check(
        not any(base.get("diagram_horizontal_overflows") or []),
        f"1280 冷启动出现 <16px 多余横向滚动条：{base.get('diagram_horizontal_overflows')}",
    )

    wide = layout_wide["report"]["after"]
    wide_edit = layout_wide["report"].get("edit_after") or {}
    wide_widgets = wide.get("diagram_widget_widths") or []
    wide_scenes = wide.get("diagram_scene_widths") or []
    wide_diag_bounds = wide.get("diagram_box_bounds") or []
    wide_content = wide.get("content_width_measured", 0)
    wide_text_widths = wide.get("text_view_widths") or []
    wide_left_x = wide.get("first_text_scrolled_x", 0.0)
    check(wide.get("surface_rebuilds", -1) == 0, f"1440px 不应重建 ReadingSurface：{wide}")
    check(wide.get("left_show") is False and wide.get("right_show") is False, f"侧栏未收起：{wide}")
    check(
        wide_left_x >= 20.0,
        f"1440px 阅读区左留白不足（实测 {wide_left_x}px）：{wide}",
    )
    check(
        wide_text_widths and all(abs(w - 920) <= 4 for w in wide_text_widths),
        f"1440px 收起侧栏后正文未统一居中在 920px 列内：{wide_text_widths}",
    )
    check(
        wide.get("text_view_centered") is True,
        f"宽屏正文段未居中排版：{wide}",
    )
    check(
        wide_diag_bounds and abs(wide_diag_bounds[0]["width"] - 920) <= 4,
        f"1440px 收起侧栏后 Mermaid 外框未对齐 920px 列宽：{wide_diag_bounds}",
    )
    check(
        abs(wide_diag_bounds[0]["x"] - wide_left_x) <= 3.0,
        f"1440px Mermaid 左边线未对齐正文：diag_x={wide_diag_bounds[0]['x']} text_x={wide_left_x}",
    )
    check(
        not any(wide.get("diagram_horizontal_overflows") or []),
        f"1440px Mermaid 出现多余内部横向滚动条：{wide.get('diagram_horizontal_overflows')}",
    )
    check(
        wide_scenes and wide_scenes[0] >= 890,
        f"1440px Mermaid 场景宽未扩展至 ~910px：{wide_scenes}",
    )
    check(
        wide_scenes[0] - base_scenes[0] >= 200,
        f"侧栏收起后图表未比双边栏态明显变宽 >=200px：base={base_scenes[0]} wide={wide_scenes[0]}",
    )
    check(
        wide_scenes[0] <= wide_content + 4,
        f"宽屏图表横向溢出：scene={wide_scenes[0]} content={wide_content}",
    )
    check(
        layout_wide["report"].get("edit_ready") is True
        and abs(wide_edit.get("edit_column_width", 0) - 920) <= 4
        and abs(wide_edit.get("edit_left_margin", 0) - wide_left_x) <= 4.0,
        f"1440px 编辑视图未对齐 920px 居中列：{wide_edit}",
    )

    narrow = layout_narrow["report"]["after"]
    narrow_widgets = narrow.get("diagram_widget_widths") or []
    narrow_scenes = narrow.get("diagram_scene_widths") or []
    narrow_content = narrow.get("content_width_measured", 0)
    narrow_left_x = narrow.get("first_text_scrolled_x", 0.0)
    check(
        narrow_left_x >= 20.0,
        f"994px 阅读区左留白不足（需 >=20px，实测 {narrow_left_x}px）：{narrow}",
    )
    check(narrow_widgets and narrow_widgets[0] >= 320, f"994px 图表被压扁：{narrow}")
    check(
        narrow_scenes and narrow_scenes[0] <= narrow_content + 24,
        f"994px 图表横向溢出：scene={narrow_scenes} content={narrow_content}",
    )
    return {
        "base_1280": {
            "widget": base_widgets[0],
            "scene": base_scenes[0],
            "content": base_content,
            "left_margin_x": base_left_x,
        },
        "wide": {
            "widget": wide_widgets[0],
            "outer_width": wide_diag_bounds[0]["width"],
            "scene": wide_scenes[0],
            "content": wide_content,
            "left_margin_x": wide_left_x,
            "edit_column_width": wide_edit.get("edit_column_width"),
        },
        "narrow_994": {
            "widget": narrow_widgets[0],
            "scene": narrow_scenes[0],
            "content": narrow_content,
            "left_margin_x": narrow_left_x,
        },
    }


def verify_wide_all_elements(layout_field_guide_wide: dict):
    """1440px 收起侧栏打开 Markdown field guide.md：全元素与编辑视图统一对齐 <= 920px 居中列。"""
    report = layout_field_guide_wide["report"]
    after = report["after"]
    edit_after = report.get("edit_after") or {}
    col_left = after.get("first_text_scrolled_x", 0.0)
    col_width = after.get("text_view_width", 0)
    col_right = col_left + col_width
    check(after.get("left_show") is False and after.get("right_show") is False, f"侧栏未收起：{after}")
    check(abs(col_width - 920) <= 4, f"Markdown field guide 正文列宽应为 920px，实测 {col_width}")

    for field, label in (
        ("text_box_bounds", "正文段"),
        ("diagram_box_bounds", "Mermaid 图表"),
        ("table_box_bounds", "GFM 表格"),
        ("embed_box_bounds", "嵌入卡片"),
        ("math_box_bounds", "块级公式"),
        ("hr_box_bounds", "分隔线"),
    ):
        items = after.get(field) or []
        check(items, f"Markdown field guide 缺少 {label} 边界数据 ({field})")
        for idx, b in enumerate(items):
            check(
                abs(b["x"] - col_left) <= 4.0 and abs(b["right"] - col_right) <= 4.0,
                f"{label}[{idx}] 未对齐 920px 左右边线：item={b} expected=[{col_left}, {col_right}]",
            )

    images = after.get("image_box_bounds") or []
    check(images, "Markdown field guide 缺少图片边界数据 (image_box_bounds)")
    for idx, b in enumerate(images):
        check(
            b["width"] <= 920 + 4 and b["x"] >= col_left - 4.0 and b["right"] <= col_right + 4.0,
            f"图片[{idx}] 超出 920px 居中列：item={b} col=[{col_left}, {col_right}]",
        )

    check(
        not any(after.get("diagram_horizontal_overflows") or []),
        f"Markdown field guide Mermaid 出现多余横向滚动条：{after.get('diagram_horizontal_overflows')}",
    )
    check(
        abs(after.get("edit_column_width", 0) - col_width) <= 4,
        f"阅读态预同步编辑列宽未对齐：edit={after.get('edit_column_width')} read={col_width}",
    )
    check(report.get("edit_ready") is True, f"切换编辑视图失败：{report}")
    check(
        edit_after.get("edit_allocated_width", 0) > 240
        and abs(edit_after.get("edit_column_width", 0) - col_width) <= 4
        and abs(edit_after.get("edit_left_margin", 0) - col_left) <= 4.0
        and abs(edit_after.get("edit_right_margin", 0) - col_left) <= 4.0
        and edit_after.get("edit_top_margin") == 24,
        f"真实编辑视图未与阅读列宽对齐：{edit_after}",
    )
    return {
        "column_left_x": col_left,
        "column_width": col_width,
        "text_count": len(after.get("text_box_bounds") or []),
        "diagram_bounds": after.get("diagram_box_bounds"),
        "table_bounds": after.get("table_box_bounds"),
        "embed_bounds": after.get("embed_box_bounds"),
        "math_bounds": after.get("math_box_bounds"),
        "hr_bounds": after.get("hr_box_bounds"),
        "image_bounds": images,
        "edit_column_width": edit_after.get("edit_column_width"),
        "edit_left_margin": edit_after.get("edit_left_margin"),
        "edit_right_margin": edit_after.get("edit_right_margin"),
        "edit_top_margin": edit_after.get("edit_top_margin"),
    }


def verify_mmd_full_width(layout_mmd_wide: dict):
    """独立 .mmd 纯图表文件保持 set_max_content_width(None)，铺满窗口宽度。"""
    after = layout_mmd_wide["report"]["after"]
    diag_bounds = after.get("diagram_box_bounds") or []
    scenes = after.get("diagram_scene_widths") or []
    check(after.get("max_content_width") is None, f".mmd 应保持无列宽封顶：{after}")
    check(
        diag_bounds and diag_bounds[0]["width"] >= 1200,
        f"1440px 收起侧栏后独立 .mmd 外框未铺满窗口：{diag_bounds}",
    )
    check(
        scenes and scenes[0] >= 1200,
        f"1440px 收起侧栏后独立 .mmd 场景宽未铺满窗口：{scenes}",
    )
    check(
        not any(after.get("diagram_horizontal_overflows") or []),
        f"独立 .mmd 出现多余横向滚动条：{after.get('diagram_horizontal_overflows')}",
    )
    return {
        "max_content_width": after.get("max_content_width"),
        "diagram_outer_width": diag_bounds[0]["width"],
        "diagram_scene_width": scenes[0],
        "scrolled_width": after.get("scrolled_width"),
    }


def verify_media_widths(layout_media: dict):
    """表格、本地图片与嵌入卡片不再被 180/120/220px 压扁，且保留阅读区左留白。"""
    after = layout_media["report"]["after"]
    content = after.get("content_width_measured", 0)
    tables = after.get("table_widget_widths") or []
    images = after.get("image_widget_widths") or []
    embeds = after.get("embed_widget_widths") or []
    left_x = after.get("first_text_scrolled_x", 0.0)
    check(
        left_x >= 20.0,
        f"1280x860 Markdown field guide 阅读区左留白不足（需 >=20px，实测 {left_x}px）：{after}",
    )
    check(tables and max(tables) >= 360, f"表格被压扁（需 >=360px）：{after}")
    check(images and max(images) >= 320, f"图片被压扁（需 >=320px）：{after}")
    check(
        images[0] <= content + 24,
        f"图片横向溢出：image={images[0]} content={content}",
    )
    check(embeds and max(embeds) >= 300, f"嵌入卡片未跟随可用宽：{after}")
    return {
        "table_widget_widths": tables,
        "image_widget_widths": images,
        "embed_widget_widths": embeds,
        "content_width_measured": content,
        "left_margin_x": left_x,
    }


def _pad4(n: int) -> int:
    return (4 - (n % 4)) % 4


class X11Client:
    """纯 Python 标准库 X11 协议客户端：无外部依赖，直接向 Xvfb 发送 ConfigureWindow。"""

    def __init__(self, display: str | None = None):
        disp = display or os.environ.get("DISPLAY") or ":0"
        if ":" not in disp:
            raise AssertionError(f"无效 DISPLAY：{disp}")
        _, rest = disp.split(":", 1)
        dnum = int(rest.split(".", 1)[0])
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        unix_path = f"/tmp/.X11-unix/X{dnum}"
        try:
            self.sock.connect(unix_path)
        except OSError:
            self.sock.connect(f"\x00/tmp/.X11-unix/X{dnum}")
        self.sock.settimeout(5.0)
        self.seq = 0
        self.root_window = self._handshake()

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def _recv_exact(self, n: int) -> bytes:
        buf = bytearray()
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise AssertionError(f"X11 连接提前关闭（期望 {n} 字节，实得 {len(buf)}）")
            buf.extend(chunk)
        return bytes(buf)

    def _handshake(self) -> int:
        hello = struct.pack("<BxHHHHxx", ord("l"), 11, 0, 0, 0)
        self.sock.sendall(hello)
        head = self._recv_exact(8)
        status, reason_len, _, _, extra_words = struct.unpack("<BBHHH", head)
        body = self._recv_exact(extra_words * 4)
        if status != 1:
            reason = body[:reason_len].decode("utf-8", errors="replace")
            raise AssertionError(f"X11 握手失败 status={status}: {reason}")
        vendor_len, _, _num_roots, num_formats = struct.unpack_from("<HHBB", body, 16)
        offset = 32 + vendor_len + _pad4(vendor_len) + num_formats * 8
        return struct.unpack_from("<I", body, offset)[0]

    def _send_req(self, data: bytes) -> int:
        self.seq = (self.seq + 1) & 0xFFFF
        self.sock.sendall(data)
        return self.seq

    def _read_reply(self, expected_seq: int) -> bytes:
        while True:
            head = self._recv_exact(32)
            kind = head[0]
            if kind == 1:
                _, _, seq, extra_words = struct.unpack_from("<BBHI", head, 0)
                extra = self._recv_exact(extra_words * 4) if extra_words else b""
                if seq == expected_seq:
                    return head + extra
            elif kind == 0:
                _, code, seq, val, minor, major = struct.unpack_from("<BBHIHB", head, 0)
                raise AssertionError(f"X11 错误 code={code} seq={seq} val={val} op={major}.{minor}")
            else:
                # 忽略异步事件通知，继续等待对应序号的响应包。
                continue

    def sync(self):
        seq = self._send_req(struct.pack("<BBH", 43, 0, 1))
        self._read_reply(seq)

    def intern_atom(self, name: str) -> int:
        raw = name.encode("utf-8")
        padded = raw + b"\x00" * _pad4(len(raw))
        req = struct.pack("<BBHHH", 16, 0, 2 + len(padded) // 4, len(raw), 0) + padded
        seq = self._send_req(req)
        rep = self._read_reply(seq)
        return struct.unpack_from("<I", rep, 8)[0]

    def query_tree(self, wid: int) -> list[int]:
        seq = self._send_req(struct.pack("<BBHI", 15, 0, 2, wid))
        rep = self._read_reply(seq)
        n_children = struct.unpack_from("<H", rep, 16)[0]
        if n_children == 0:
            return []
        return list(struct.unpack_from(f"<{n_children}I", rep, 32))

    def is_viewable(self, wid: int) -> bool:
        seq = self._send_req(struct.pack("<BBHI", 3, 0, 2, wid))
        rep = self._read_reply(seq)
        return rep[26] == 2

    def get_property(self, wid: int, prop_atom: int) -> tuple[int, int, bytes]:
        seq = self._send_req(struct.pack("<BBHIIIII", 20, 0, 6, wid, prop_atom, 0, 0, 64))
        rep = self._read_reply(seq)
        fmt = rep[1]
        nitems = struct.unpack_from("<I", rep, 16)[0]
        unit = fmt // 8 if fmt in (8, 16, 32) else 0
        payload = rep[32 : 32 + nitems * unit]
        return fmt, nitems, payload

    def find_window_for_pid(self, pid: int, title_hint: str = "Hortus") -> int | None:
        pid_atom = self.intern_atom("_NET_WM_PID")
        wm_name_atom = 39
        candidates = list(reversed(self.query_tree(self.root_window)))
        # 一并扫描两层（无 WM 直接挂 root，有 WM 时再向下一层）。
        queue = list(candidates)
        visited = set()
        fallback = None
        while queue:
            wid = queue.pop(0)
            if wid in visited:
                continue
            visited.add(wid)
            try:
                if not self.is_viewable(wid):
                    continue
                fmt, nitems, pid_bytes = self.get_property(wid, pid_atom)
                if fmt == 32 and nitems >= 1 and len(pid_bytes) >= 4:
                    win_pid = struct.unpack_from("<I", pid_bytes, 0)[0]
                    if win_pid == pid:
                        return wid
                _, _, name_bytes = self.get_property(wid, wm_name_atom)
                if title_hint.encode("utf-8") in name_bytes:
                    fallback = wid
                queue.extend(self.query_tree(wid))
            except AssertionError:
                continue
        return fallback

    def configure_window(self, wid: int, width: int, height: int):
        # value-mask: Width (0x0004) | Height (0x0008) = 0x000C
        req = struct.pack("<BBHIHHII", 12, 0, 5, wid, 0x000C, 0, width, height)
        self._send_req(req)
        self.sync()


def run_live_resize_regression(
    gui_bin: Path,
    vault: Path,
    tmp_path: Path,
    output_dir: Path,
    media_dir: Path,
):
    """用真实 X11 ConfigureWindow 执行 1400 -> 1020 -> 760 -> 1520 实时缩放回归。"""
    env = isolated_env(tmp_path / "live-resize")
    report_path = output_dir / "layout-live-resize.json"
    stage_path = report_path.with_suffix(".stage")
    cmd_path = report_path.with_suffix(".cmd")
    shot_1020 = media_dir / "l2-block-reading-live-resize-1020.png"
    shot_1520 = media_dir / "l2-block-reading-live-resize-1520.png"
    media_dir.mkdir(parents=True, exist_ok=True)
    for path in (report_path, stage_path, cmd_path, shot_1020, shot_1520):
        path.unlink(missing_ok=True)

    cmd = [
        str(gui_bin),
        "--vault",
        str(vault),
        "--open",
        "Learning/Learning map.md",
        "--width",
        "1400",
        "--height",
        "860",
        "--live-resize-json",
        str(report_path),
    ]
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    sockets = []
    peak_rss_kib = 0

    def sample_proc():
        nonlocal peak_rss_kib
        sockets.extend(process_network_sockets(proc.pid))
        rss = read_rss_kib(proc.pid)
        if rss and rss > peak_rss_kib:
            peak_rss_kib = rss

    def wait_stage(expected_seq: int, timeout: float = 20.0) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if stage_path.is_file():
                try:
                    data = json.loads(stage_path.read_text(encoding="utf-8"))
                    if data.get("seq", -1) >= expected_seq:
                        return data
                except json.JSONDecodeError:
                    pass
            sample_proc()
            if proc.poll() is not None:
                if stage_path.is_file():
                    try:
                        data = json.loads(stage_path.read_text(encoding="utf-8"))
                        if data.get("seq", -1) >= expected_seq:
                            return data
                    except json.JSONDecodeError:
                        pass
                raise AssertionError(f"实时缩放进程意外提前退出 code={proc.returncode}")
            time.sleep(0.05)
        raise AssertionError(f"等待实时缩放阶段 seq={expected_seq} 超时")

    x11 = X11Client(env.get("DISPLAY"))
    try:
        init_stage = wait_stage(0, timeout=25.0)
        wid = None
        find_deadline = time.monotonic() + 10.0
        while time.monotonic() < find_deadline and wid is None:
            wid = x11.find_window_for_pid(proc.pid, "Hortus")
            if wid is None:
                time.sleep(0.05)
        check(wid is not None, "未能通过 X11 QueryTree 定位 Hortus 主窗口")

        resize_plan = [
            (1, 1020, 860, str(shot_1020)),
            (2, 760, 860, ""),
            (3, 1520, 900, str(shot_1520)),
        ]
        stages = {}
        for seq, target_w, target_h, shot in resize_plan:
            x11.configure_window(wid, target_w, target_h)
            tmp_cmd = cmd_path.parent / (cmd_path.name + ".tmp")
            tmp_cmd.write_text(
                json.dumps({"seq": seq, "target_width": target_w, "screenshot": shot}),
                encoding="utf-8",
            )
            os.replace(tmp_cmd, cmd_path)
            stages[seq] = wait_stage(seq, timeout=20.0)
            sample_proc()

        stdout, stderr = proc.communicate(timeout=20)
    except Exception:
        proc.kill()
        proc.communicate(timeout=10)
        raise
    finally:
        x11.close()
        stage_path.unlink(missing_ok=True)
        cmd_path.unlink(missing_ok=True)

    run = {
        "label": "L2 X11 实时缩放回归",
        "returncode": proc.returncode,
        "stdout": stdout,
        "stderr": stderr,
        "sockets": sockets,
        "peak_rss_mib": round(peak_rss_kib / 1024.0, 2),
    }
    assert_clean_run(run, "L2 X11 实时缩放回归")
    check(report_path.is_file(), f"X11 实时缩放未生成 JSON：{report_path}")

    init_snap = init_stage["snapshot"]
    s1 = stages[1]
    s2 = stages[2]
    s3 = stages[3]
    snap_1020 = s1["snapshot"]
    snap_760 = s2["snapshot"]
    snap_1520 = s3["snapshot"]

    check(init_stage.get("ready") is True and init_stage.get("allocated") is True, f"实时缩放首屏未就绪：{init_stage}")
    check(init_snap.get("left_collapsed") is False and init_snap.get("right_collapsed") is False, f"1400px 不应折叠侧栏：{init_snap}")
    check(init_snap.get("surface_rebuilds") == 0, f"首屏不应重建 ReadingSurface：{init_snap}")
    check(
        init_snap.get("first_text_scrolled_x", 0.0) >= 20.0,
        f"1400px 双侧栏阅读区左留白不足：{init_snap}",
    )
    init_scene = (init_snap.get("diagram_scene_widths") or [0])[0]
    check(init_scene >= 520, f"1400px 初始图表宽不足：{init_snap}")

    check(s1.get("resized_and_reflowed") is True, f"1020px 实时重排未完成：{s1}")
    check(snap_1020.get("surface_rebuilds") == 0, f"1020px 缩窗不应重建 ReadingSurface：{snap_1020}")
    check(
        snap_1020.get("left_collapsed") is False and snap_1020.get("right_collapsed") is False,
        f"1020px 不应折叠双侧栏：{snap_1020}",
    )
    left_1020 = snap_1020.get("first_text_scrolled_x", 0.0)
    check(
        left_1020 >= 20.0,
        f"1020x860 双侧栏阅读区左留白不足（需 >=20px，实测 {left_1020}px）：{snap_1020}",
    )
    scene_1020 = (snap_1020.get("diagram_scene_widths") or [0])[0]
    check(
        snap_1020.get("reflow_count", 0) > init_snap.get("reflow_count", 0),
        f"1020px reflow_count 未递增：{init_snap} -> {snap_1020}",
    )
    check(scene_1020 < init_scene - 150, f"1020px 未随 X11 缩窗真实收缩：{init_scene} -> {scene_1020}")

    check(s2.get("resized_and_reflowed") is True, f"760px 实时重排未完成：{s2}")
    check(snap_760.get("surface_rebuilds") == 0, f"760px 缩窗不应重建 ReadingSurface：{snap_760}")
    check(
        snap_760.get("left_collapsed") is True and snap_760.get("right_collapsed") is True,
        f"760px 未触发 819px 侧栏断点折叠：{snap_760}",
    )
    check(
        snap_760.get("first_text_scrolled_x", 0.0) >= 20.0,
        f"760px 阅读区左留白不足：{snap_760}",
    )
    check(
        snap_760.get("reflow_count", 0) > snap_1020.get("reflow_count", 0),
        f"760px reflow_count 未递增：{snap_1020} -> {snap_760}",
    )

    check(s3.get("resized_and_reflowed") is True, f"1520px 实时重排未完成：{s3}")
    check(snap_1520.get("surface_rebuilds") == 0, f"1520px 扩窗不应重建 ReadingSurface：{snap_1520}")
    check(
        snap_1520.get("first_text_scrolled_x", 0.0) >= 20.0,
        f"1520px 阅读区左留白不足：{snap_1520}",
    )
    check(
        snap_1520.get("in_place_relayouts", 0) >= 3,
        f"三次实时缩放后 in_place_relayouts 不足 3 次：{snap_1520}",
    )
    scene_1520 = (snap_1520.get("diagram_scene_widths") or [0])[0]
    check(
        snap_1520.get("left_collapsed") is False and snap_1520.get("right_collapsed") is False,
        f"1520px 扩窗后未自动恢复双侧栏：{snap_1520}",
    )
    check(scene_1520 > init_scene + 50, f"1520px 图表未随之扩展：{init_scene} -> {scene_1520}")
    check(
        snap_1520.get("reflow_count", 0) > snap_760.get("reflow_count", 0),
        f"1520px reflow_count 未递增：{snap_760} -> {snap_1520}",
    )

    shots = {}
    for key, shot_path, expected_w in (
        ("1020", shot_1020, 1020),
        ("1520", shot_1520, 1520),
    ):
        check(shot_path.is_file(), f"实时缩放截图缺失：{shot_path}")
        w, h = png_dimensions(shot_path)
        check(abs(w - expected_w) <= 12 and h >= 700, f"{shot_path.name} 尺寸异常：{w}x{h}")
        check(shot_path.stat().st_size > 40_000, f"{shot_path.name} 内容过于简单")
        shots[key] = {
            "path": str(shot_path.resolve()),
            "width": w,
            "height": h,
            "size_bytes": shot_path.stat().st_size,
            "sha256": hashlib.sha256(shot_path.read_bytes()).hexdigest(),
        }

    return {
        "returncode": run["returncode"],
        "network_connections": len(sockets),
        "peak_rss_mib": run["peak_rss_mib"],
        "report_path": str(report_path.resolve()),
        "init_scene": init_scene,
        "scene_1020": scene_1020,
        "scene_760": (snap_760.get("diagram_scene_widths") or [0])[0],
        "scene_1520": scene_1520,
        "left_margin_1020": left_1020,
        "reflow_counts": [
            init_snap.get("reflow_count", 0),
            snap_1020.get("reflow_count", 0),
            snap_760.get("reflow_count", 0),
            snap_1520.get("reflow_count", 0),
        ],
        "screenshots": shots,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-edit-knowledge")
    parser.add_argument("--media", type=Path, default=None)
    parser.add_argument("--skip-scenes", action="store_true")
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()

    gui_bin = discover_gui_bin(args.gui_bin)
    static = audit_static_compliance(gui_bin)
    display = start_private_display()

    real_trash_before = count_files(Path.home() / ".local/share/Trash/files")

    with tempfile.TemporaryDirectory(prefix="hortus-l2-check-") as tmp:
        tmp_path = Path(tmp)
        vault = tmp_path / "vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", vault)
        populate_l2_vault(vault)
        before = snapshot_tree(vault)

        env = isolated_env(tmp_path / "automation")
        report_path = output_dir / "l2-report.json"
        report_path.unlink(missing_ok=True)
        cmd = [
            str(gui_bin),
            "--vault",
            str(vault),
            "--open",
            "Start here.md",
            "--open",
            "Learning/Learning map.md",
            "--width",
            "1280",
            "--height",
            "860",
            "--edit-knowledge-json",
            str(report_path),
        ]
        run = supervise(cmd, env, "L2 自动化", timeout=240)
        assert_clean_run(run, "L2 自动化")
        check(report_path.is_file(), "L2 自动化未生成 JSON 报告")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        check(
            not report.get("timeout"),
            f"L2 自动化在阶段 {report.get('stage')} 超时（报告未完成）",
        )
        verified = assert_report(report, vault, env, real_trash_before)

        after = snapshot_tree(vault)
        allowed_changes = {
            "L2-编辑目标.md",
            "L2-引用者.md",
            "L2-引用组.md",
            "L2-被引用.md",
            "L2-被引用改名.md",
            "L2-孤立笔记.md",
            "L2-孤立改名.md",
            "L2-恢复稿锁定.md",
            "L2-只读回滚.md",
            "L2-待删除.md",
            "L2-关闭保护.md",
            "L2-窗口保护.md",
            "L2-新笔记.md",
            "L2-组/子笔记.md",
            "L2-组新/子笔记.md",
        }
        unexpected = [
            name
            for name in (set(before) | set(after))
            if before.get(name, {}).get("sha256") != after.get(name, {}).get("sha256")
            and name not in allowed_changes
        ]
        check(not unexpected, f"未预期的库内文件变化（只读流程应 0 写入）：{unexpected}")
        leftovers = [
            name
            for name in after
            if any(token in name.lower() for token in ("tmp", ".hortus", "temporaryitems"))
        ]
        check(not leftovers, f"库内残留临时文件：{leftovers}")

        scenes = []
        narrow = []
        if not args.skip_scenes:
            # 截图与窄屏检查使用独立的全新副本，避免自动化已重命名的文件影响场景。
            scene_vault = tmp_path / "scene-vault"
            shutil.copytree(ROOT / "fixtures/demo-vault", scene_vault)
            populate_l2_vault(scene_vault)
            scenes = capture_scenes(gui_bin, scene_vault, media_dir, tmp_path)
            narrow = narrow_width_run(gui_bin, scene_vault, tmp_path, media_dir)
        # 双链点击回归使用独立副本：只读流程，不允许受前面重命名/删除影响。
        wikilink_vault = tmp_path / "wikilink-vault"
        shutil.copytree(ROOT / "fixtures/demo-vault", wikilink_vault)
        populate_l2_vault(wikilink_vault)
        wikilink = run_wikilink_regression(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            media_dir / "l2-wikilink-learning-map-1280.png",
        )
        # 阅读视图宽度适配回归：默认窗、收起侧栏、994px 窄平铺与表格/图片/嵌入卡片。
        layout_1280 = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "learning-map-1280",
            "Learning/Learning map.md",
            1280,
            860,
            screenshot=media_dir / "l2-typography-1280-learning-map.png",
        )
        layout_wide = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "learning-map-wide",
            "Learning/Learning map.md",
            1440,
            860,
            hide_sidebars=True,
            screenshot=media_dir / "l2-typography-wide-920prose.png",
        )
        layout_narrow = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "learning-map-994",
            "Learning/Learning map.md",
            994,
            700,
            screenshot=media_dir / "l2-learning-map-narrow-994.png",
        )
        layout_media = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "media-1280",
            "Markdown field guide.md",
            1280,
            860,
            screenshot=media_dir / "l2-typography-markdown-guide.png",
        )
        layout_field_guide_wide = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "field-guide-wide",
            "Markdown field guide.md",
            1440,
            860,
            hide_sidebars=True,
            screenshot=media_dir / "l2-typography-wide-field-guide.png",
        )
        layout_mmd_wide = run_layout_width(
            gui_bin,
            wikilink_vault,
            tmp_path,
            output_dir,
            "standalone-mmd-wide",
            "L2-独立图表.mmd",
            1440,
            860,
            hide_sidebars=True,
            screenshot=media_dir / "l2-standalone-mmd-wide.png",
        )
        layout_verified = verify_diagram_widths(layout_1280, layout_wide, layout_narrow)
        media_widths = verify_media_widths(layout_media)
        all_elements_wide = verify_wide_all_elements(layout_field_guide_wide)
        mmd_wide = verify_mmd_full_width(layout_mmd_wide)
        live_resize = run_live_resize_regression(gui_bin, wikilink_vault, tmp_path, output_dir, media_dir)

    if display is not None:
        display.terminate()
        try:
            display.wait(timeout=5)
        except subprocess.TimeoutExpired:
            display.kill()
    summary = {
        "passed": True,
        "display": os.environ.get("DISPLAY"),
        "offscreen": display is not None,
        "gui_bin": str(gui_bin),
        "static_compliance": static,
        "automation": {
            "returncode": run["returncode"],
            "network_connections": len(run["sockets"]),
            "network_samples": run["network_samples"],
            "peak_rss_kib": run["peak_rss_kib"],
            "peak_rss_mib": run["peak_rss_mib"],
            "report_path": str(report_path.resolve()),
        },
        "verified": verified,
        "scenes": scenes,
        "narrow_width": narrow,
        "wikilink": wikilink,
        "layout_width": {
            "learning_map": layout_verified,
            "media": media_widths,
            "field_guide_wide_all_elements": all_elements_wide,
            "standalone_mmd_wide": mmd_wide,
            "live_resize": live_resize,
            "reports": {
                "default_1280": layout_1280["report_path"],
                "wide": layout_wide["report_path"],
                "narrow_994": layout_narrow["report_path"],
                "media_1280": layout_media["report_path"],
                "field_guide_wide": layout_field_guide_wide["report_path"],
                "standalone_mmd_wide": layout_mmd_wide["report_path"],
            },
            "screenshots": {
                "default_1280": layout_1280["screenshot"],
                "wide": layout_wide["screenshot"],
                "narrow_994": layout_narrow["screenshot"],
                "markdown_guide": layout_media["screenshot"],
                "field_guide_wide": layout_field_guide_wide["screenshot"],
                "standalone_mmd_wide": layout_mmd_wide["screenshot"],
            },
        },
        "vault_unchanged_except": sorted(allowed_changes),
        "real_trash_untouched": True,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    if args.media is not None:
        stray_pngs = sorted(str(path.relative_to(output_dir)) for path in output_dir.rglob("*.png"))
        check(not stray_pngs, f"--output 目录内出现 PNG（应全部写入 --media）：{stray_pngs}")
    print(
        "PASS: edit/save/conflict/missing ok, "
        f"big_doc_style={verified['big_doc']['style_worst_micros']}us, "
        f"search_groups={verified['knowledge']['search']['final_groups']}, "
        f"completion={verified['knowledge']['completion']['items']}, "
        f"rename_rewrites={verified['files']['rename_note']['op']['rewrites']}, "
        f"trash_isolated=yes, 0 net, rss={run['peak_rss_mib']} MiB, scenes={len(scenes)}, "
        f"wikilink_nodes={wikilink['learning_map_nodes']}, "
        f"diagram_1280={layout_verified['base_1280']['scene']}px, "
        f"diagram_wide={layout_verified['wide']['scene']}px, "
        f"diagram_994={layout_verified['narrow_994']['widget']}px, "
        f"table={media_widths['table_widget_widths'][0]}px"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as err:
        print(f"FAIL: {err}", file=sys.stderr)
        raise SystemExit(1)
