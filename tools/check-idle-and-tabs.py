#!/usr/bin/env python3
"""用隔离的 release App 验收空闲 CPU、连续切库线程及标签栏截图。"""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "build/Hortus.app/Contents/MacOS/Hortus"
SAMPLE = ROOT / "fixtures/vault-sample"


def environment(base):
    return dict(os.environ, HORTUS_CONFIG_DIR=str(base / "config"),
                HORTUS_CACHE_DIR=str(base / "cache"))


def idle_cpu(base, output, name, arguments):
    with subprocess.Popen([str(APP), *arguments], env=environment(base)) as process:
        try:
            # 会话落盘表示打开请求已完成；空状态只等初始化。
            ready = base / ("cache/session.toml" if arguments else "config/config.toml")
            deadline = time.monotonic() + 10
            while not ready.exists():
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError(f"{name} 初始化失败")
                time.sleep(0.01)
            time.sleep(5)
            command = ["top", "-l", "11", "-s", "1", "-pid", str(process.pid),
                       "-stats", "pid,cpu,threads"]
            raw = subprocess.check_output(command, text=True, timeout=20)
            (output / f"{name}-top.txt").write_text("\n".join(line.rstrip() for line in raw.splitlines()) + "\n")
            rows = re.findall(rf"^\s*{process.pid}\s+([\d.]+)\s+(\d+)", raw, re.MULTILINE)
            # top 第一帧是启动以来的统计，丢弃后保留连续 10 秒的增量样本。
            assert len(rows) == 11, raw
            samples = [float(cpu) for cpu, _ in rows[1:]]
            return {"cpu_percent": samples, "median_cpu_percent": statistics.median(samples),
                    "threads": [int(threads) for _, threads in rows[1:]],
                    "command": "top -l 11 -s 1 -pid <App PID> -stats pid,cpu,threads"}
        finally:
            process.terminate()
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/idle-and-tabs")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hortus-idle-check-") as temporary:
        base = Path(temporary)
        empty = idle_cpu(base / "empty", output, "empty", [])
        opened = idle_cpu(base / "opened", output, "opened", [f"--vault={SAMPLE}"])
        difference = opened["median_cpu_percent"] - empty["median_cpu_percent"]
        cpu = {"empty": empty, "opened": opened, "difference_percentage_points": difference,
               "target_pass": abs(difference) <= 0.1,
               "method": "release .app，各自独立临时配置和缓存；初始化后等 5 s，top 连续采样 10 s，丢弃首帧。"}
        (output / "idle-cpu.json").write_text(json.dumps(cpu, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(cpu, ensure_ascii=False, indent=2), flush=True)
        assert cpu["target_pass"], "打开库后的空闲 CPU 相对基线偏差超过 0.1 个百分点"

        vaults = []
        for index in range(2):
            vault = base / f"vault{index}"
            vault.mkdir()
            for note in range(30):
                (vault / f"note{note}.md").write_text(f"# Fixture {index} / {note}\n")
            vaults.append(vault)
        lifecycle_path = output / "lifecycle.json"
        subprocess.run([str(APP), "--empty", f"--check-vault-lifecycle={lifecycle_path}",
                        *[f"--cycle-vault={vault}" for vault in vaults]],
                       env=environment(base / "lifecycle"), check=True, timeout=30)
        lifecycle = json.loads(lifecycle_path.read_text())
        for key in ["open_threads", "switch_threads", "closed_threads"]:
            counts = lifecycle[key]
            assert len(counts) == 20, lifecycle
            assert max(counts[5:]) - min(counts[5:]) <= 2, lifecycle
            assert statistics.median(counts[-5:]) <= statistics.median(counts[5:10]) + 1, lifecycle
        print(json.dumps(lifecycle, indent=2), flush=True)

        for name in ["one-tab", "overflow-tabs"]:
            vault = SAMPLE
            tabs = ["欢迎.md"]
            if name == "overflow-tabs":
                # 溢出用额外的合成笔记，不写示例库。
                vault = base / "tabs-vault"
                vault.mkdir()
                tabs = [f"Note-{index:02d}-layout-check.md" for index in range(12)]
                for tab in tabs:
                    (vault / tab).write_text("# Tab layout fixture\n\nHorizontal scrolling check.\n")
            subprocess.run([str(APP), f"--vault={vault}", "--light", *[f"--note={tab}" for tab in tabs],
                            f"--snapshot={output / (name + '.png')}"],
                           env=environment(base / name), check=True, timeout=20)


if __name__ == "__main__":
    main()
