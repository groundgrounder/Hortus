#!/usr/bin/env python3
"""P1-12 search evidence helper.

The UI runs are intentionally serialized because AppKit/LaunchServices can only
reliably initialize one inspection process at a time on the test host.
"""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "build/Hortus.app/Contents/MacOS/Hortus"


def environment(base):
    return dict(os.environ, HORTUS_CONFIG_DIR=str(base / "config"),
                HORTUS_CACHE_DIR=str(base / "cache"))


def percentile95(values):
    values = sorted(values)
    return values[max(0, (95 * len(values) + 99) // 100 - 1)]


FILLER = "Plain fixture text about reading, notes and folders. 普通的填充段落，用来接近真实笔记的长度。\n"


def make_large_vault(vault, note_bytes=0):
    # note_bytes > 0 时每篇补到大约这个长度，且只有十分之一的笔记含搜索词，接近真实笔记库
    for folder in range(200):
        directory = vault / f"folder{folder:03d}"
        directory.mkdir(parents=True)
        for note in range(50):
            term = "中文 " if note_bytes == 0 or note % 10 == 0 else ""
            text = f"# Note {note:02d}\n\n{term}performance fixture in folder {folder:03d}.\n"
            while len(text.encode()) < note_bytes:
                text += FILLER
            (directory / f"note{note:02d}.md").write_text(text)


def run_app(base, vault, args, timeout=180):
    subprocess.run([str(APP), f"--vault={vault}", *args],
                   env=environment(base), check=True, timeout=timeout)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/search")
    parser.add_argument("--screenshots-only", action="store_true")
    parser.add_argument("--performance-only", action="store_true")
    parser.add_argument("--note-bytes", type=int, default=0,
                        help="pad each generated note to about this many bytes (0 keeps the minimal P1-02 notes)")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    shots = output / "screenshots"
    shots.mkdir(exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="hortus-search-") as temporary:
        base = Path(temporary)
        sample = base / "sample"
        sample.mkdir()
        (sample / "Notes").mkdir()
        (sample / "Notes/Architecture.md").write_text("# Architecture\n\nA concise design note.\n")
        (sample / "Notes/Chinese.md").write_text("# 中文搜索\n\n快速找到中文内容。\n")
        (sample / "Tasks.md").write_text("# Tasks\n\nReview the search panel.\n")

        if not args.performance_only:
            for name, appearance, query in [
                ("quick-open-light", "--light", "--quick-open-query=architecture"),
                ("quick-open-dark", "--dark", "--quick-open-query=architecture"),
                ("search-light", "--light", "--search-query=中文"),
                ("search-dark", "--dark", "--search-query=中文"),
            ]:
                run_app(base / name, sample,
                        ["--search-wait-for-scan", appearance, query,
                         f"--snapshot={shots / (name + '.png')}"])
                image = shots / f"{name}.png"
                assert image.is_file() and image.stat().st_size > 10_000, image
            if args.screenshots_only:
                return

        vault = base / "vault"
        vault.mkdir()
        make_large_vault(vault, args.note_bytes)
        metrics = output / "search-performance.json"
        metrics.unlink(missing_ok=True)
        run_app(base / "performance", vault,
                ["--search-wait-for-scan", f"--search-performance={metrics}",
                 "--search-performance-query=中文"])
        raw = json.loads(metrics.read_text())
        quick = raw["quick_open_ui_ms"]
        full = raw["full_text_ui_ms"]
        report = {
            "fixture": {"notes": raw["notes"], "folders": raw["folders"], "note_bytes": args.note_bytes},
            "quick_open_ms": {"count": len(quick), "median": statistics.median(quick),
                              "p95": percentile95(quick), "samples": quick},
            "full_text_chinese_ms": {"count": len(full), "median": statistics.median(full),
                                     "p95": percentile95(full), "samples": full},
            "index_ready_ms": raw["index_ready_ms"],
            "index_peak_rss_delta_bytes": raw["index_peak_rss_delta_bytes"],
            "measurement": "查询从 Workspace 更新开始计时，至新结果数组发布完成；索引时间从打开请求计至扫描完成；RSS 为进程峰值相对打开请求前的增量。",
        }
        (output / "performance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
