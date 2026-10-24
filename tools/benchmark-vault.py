#!/usr/bin/env python3
"""在临时目录生成 1 万篇笔记与 200 个文件夹，测真实 App 文件树的绘制。"""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "build/Hortus.app/Contents/MacOS/Hortus"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="hortus-benchmark-") as temporary:
        base = Path(temporary)
        vault = base / "vault"
        vault.mkdir()
        for folder in range(200):
            directory = vault / f"folder{folder}"
            directory.mkdir()
            for note in range(50):
                (directory / f"note{note}.md").write_text(f"# Note {note}\n\nPerformance fixture.\n")
        runs = []
        for index in range(3):
            run = base / f"run{index}"
            run.mkdir()
            metrics = run / "metrics.json"
            env = dict(os.environ, HORTUS_CONFIG_DIR=str(run / "config"), HORTUS_CACHE_DIR=str(run / "cache"))
            subprocess.run([str(APP), f"--vault={vault}", f"--metrics={metrics}"], env=env, check=True, timeout=25)
            result = json.loads(metrics.read_text())
            assert result["notes"] == 10000 and result["folders"] == 200, result
            runs.append(result)
        medians = {key: statistics.median(run[key] for run in runs) for key in
                   ["first_screen_ms", "scan_complete_ms", "scan_display_ms", "peak_rss_bytes"]}
        report = {"runs": runs, "median": medians,
                  "targets_pass": medians["first_screen_ms"] < 300 and medians["scan_complete_ms"] < 1000,
                  "measurement": "打开请求至 NSOutlineView 首次绘制非空树；Rust 扫描耗时；全量树绘制耗时；App 进程峰值 RSS（字节）。不含进程冷启动。"}
        text = json.dumps(report, ensure_ascii=False, indent=2)
        print(text)
        if args.output:
            args.output.write_text(text + "\n")
        if not report["targets_pass"]:
            raise SystemExit("性能目标未达成")


if __name__ == "__main__":
    main()
