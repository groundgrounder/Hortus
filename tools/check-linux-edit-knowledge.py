#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""L2 Linux 样式化编辑、安全保存、外部刷新、知识导航与文件管理端到端自检。"""

import argparse
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

SCENE_EXPECTATIONS = {
    "search-backlinks": {
        "filename": "l2-search-results-and-backlinks.png",
        "label": "左侧全文搜索结果 + 右侧反链面板",
        "open": ["Start here.md"],
        "extra": [],
    },
    "tags": {
        "filename": "l2-tags-browser-and-reading.png",
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
    for candidate in (
        ROOT / "result/bin/hortus-gtk",
        ROOT / "apps/linux/target/release/hortus-gtk",
        ROOT / "apps/linux/target/debug/hortus-gtk",
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

    external = report["external"]
    check(external["clean_reload"] is True, "干净态外部修改未实时刷新")
    check(external["scroll_preserved"] is True, f"外部刷新后滚动位置跳动：{external}")
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
    check(find["closed"] is True, "Esc 未关闭查找条")
    check(find["reading_view"]["open"] is True, "阅读视图未打开查找条")
    check(find["reading_view"]["editable"] is False, "阅读视图不应展开替换行")
    check(find["reading_view"]["matches"] > 0, "阅读视图查找无匹配")

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
    check(completion["stats"]["accepted"] > 0, "[[ 补全未记录接受次数")
    check(completion["target"], "[[ 补全未给出插入目标")

    files = report["files"]
    create = files["create"]
    check(create["note_opened"] is True, "新建笔记未自动打开")
    check(create["note_on_disk"] is True, "新建笔记未落盘")
    check(create["folder_on_disk"] is True, "新建文件夹未落盘")
    rename = files["rename_note"]
    check(rename["tab_migrated"] is True, "重命名后标签页未迁移")
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
    """A5 宽度适配断言：默认窗、收起侧栏扩栏与 994px 窄平铺。"""
    base = layout_1280["report"]["after"]
    base_widgets = base.get("diagram_widget_widths") or []
    base_scenes = base.get("diagram_scene_widths") or []
    base_content = base.get("content_width_measured", 0)
    check(base_widgets and base_widgets[0] >= 520, f"1280 图表控件被压扁：{base}")
    check(base_scenes and base_scenes[0] >= 520, f"1280 图表场景宽不足：{base}")
    check(
        base_scenes[0] <= base_content + 24,
        f"1280 图表横向溢出：scene={base_scenes[0]} content={base_content}",
    )

    wide = layout_wide["report"]["after"]
    wide_widgets = wide.get("diagram_widget_widths") or []
    wide_scenes = wide.get("diagram_scene_widths") or []
    wide_content = wide.get("content_width_measured", 0)
    check(wide.get("left_show") is False and wide.get("right_show") is False, f"侧栏未收起：{wide}")
    check(wide_scenes and wide_scenes[0] >= 850, f"宽屏图表未扩栏：{wide}")
    check(
        wide_widgets and wide_widgets[0] >= 850,
        f"宽屏图表控件未扩宽：{wide}",
    )
    check(
        wide_scenes[0] - base_scenes[0] >= 200,
        f"侧栏收起后图表未明显变宽：base={base_scenes[0]} wide={wide_scenes[0]}",
    )
    check(
        wide_scenes[0] <= wide_content + 24,
        f"宽屏图表横向溢出：scene={wide_scenes[0]} content={wide_content}",
    )

    narrow = layout_narrow["report"]["after"]
    narrow_widgets = narrow.get("diagram_widget_widths") or []
    narrow_scenes = narrow.get("diagram_scene_widths") or []
    narrow_content = narrow.get("content_width_measured", 0)
    check(narrow_widgets and narrow_widgets[0] >= 320, f"994px 图表被压扁：{narrow}")
    check(
        narrow_scenes and narrow_scenes[0] <= narrow_content + 24,
        f"994px 图表横向溢出：scene={narrow_scenes} content={narrow_content}",
    )
    return {
        "base_1280": {"widget": base_widgets[0], "scene": base_scenes[0], "content": base_content},
        "wide": {"widget": wide_widgets[0], "scene": wide_scenes[0], "content": wide_content},
        "narrow_994": {
            "widget": narrow_widgets[0],
            "scene": narrow_scenes[0],
            "content": narrow_content,
        },
    }


def verify_media_widths(layout_media: dict):
    """表格、本地图片与嵌入卡片不再被 180/120/220px 压扁。"""
    after = layout_media["report"]["after"]
    content = after.get("content_width_measured", 0)
    tables = after.get("table_widget_widths") or []
    images = after.get("image_widget_widths") or []
    embeds = after.get("embed_widget_widths") or []
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
            media_dir / "l2-learning-map-1280-adapted.png",
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
            screenshot=media_dir / "l2-learning-map-wide-adapted.png",
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
        )
        layout_verified = verify_diagram_widths(layout_1280, layout_wide, layout_narrow)
        media_widths = verify_media_widths(layout_media)

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
            "reports": {
                "default_1280": layout_1280["report_path"],
                "wide": layout_wide["report_path"],
                "narrow_994": layout_narrow["report_path"],
                "media_1280": layout_media["report_path"],
            },
            "screenshots": {
                "wide": layout_wide["screenshot"],
                "narrow_994": layout_narrow["screenshot"],
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
