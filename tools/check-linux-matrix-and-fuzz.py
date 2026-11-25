#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""L5c 三层巡检门禁驱动：全库矩阵不变量 + 确定性有种子 GUI Fuzzer + 0 GTK 告警断言。"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]

FORBIDDEN_STDERR_TOKENS = (
    "Gtk-CRITICAL",
    "Gtk-WARNING",
    "Adwaita-CRITICAL",
    "GLib-CRITICAL",
    "panicked at",
)


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def snapshot_tree(root: Path) -> dict[str, tuple[str, int]]:
    snap: dict[str, tuple[str, int]] = {}
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        dirnames.sort()
        base = Path(dirpath)
        for name in sorted(filenames):
            p = base / name
            rel = p.relative_to(root).as_posix()
            snap[rel] = (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_size)
    return snap


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


def start_private_xvfb() -> tuple[subprocess.Popen[bytes] | None, str]:
    explicit = os.environ.get("HORTUS_TEST_DISPLAY")
    if explicit:
        return None, explicit
    xvfb = shutil.which("Xvfb")
    check(xvfb is not None, "缺少 Xvfb（请在 nix develop 中运行）")
    disp_num = next(
        (n for n in range(95, 145) if not Path(f"/tmp/.X11-unix/X{n}").exists()),
        None,
    )
    check(disp_num is not None, "找不到空闲的 X 显示编号")
    proc = subprocess.Popen(
        [xvfb, f":{disp_num}", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    x_sock = Path(f"/tmp/.X11-unix/X{disp_num}")
    for _ in range(200):
        if x_sock.exists():
            break
        time.sleep(0.05)
    check(x_sock.exists(), f"Xvfb :{disp_num} 未在 10s 内就绪")
    return proc, f":{disp_num}"


def make_isolated_env(base_dir: Path, display: str) -> dict[str, str]:
    env = dict(os.environ)
    for k in ("WAYLAND_DISPLAY", "XAUTHORITY", "HORTUS_VAULT", "HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"):
        env.pop(k, None)
    cfg = base_dir / "config"
    cache = base_dir / "cache"
    data = base_dir / "data"
    for d in (cfg, cache, data):
        d.mkdir(parents=True, exist_ok=True)
    env["DISPLAY"] = display
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


def run_supervised(cmd: list[str], env: dict[str, str], label: str, timeout_s: float = 180.0) -> tuple[int, str, str]:
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        stdout, stderr = proc.communicate(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        proc.kill()
        stdout, stderr = proc.communicate(timeout=10)
        raise AssertionError(f"{label} 超时 ({timeout_s}s)\nstderr tail:\n{stderr[-1500:]}")
    return proc.returncode, stdout, stderr


def count_forbidden_stderr(stderr_text: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for tok in FORBIDDEN_STDERR_TOKENS:
        counts[tok] = stderr_text.count(tok)
    return counts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-matrix-fuzz")
    parser.add_argument("--media", type=Path, default=None)
    parser.add_argument("--seed", type=int, default=20261126)
    parser.add_argument("--steps", type=int, default=200)
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()
    media_dir.mkdir(parents=True, exist_ok=True)

    src_vault = ROOT / "fixtures/demo-vault"
    check(src_vault.is_dir(), f"找不到示例库: {src_vault}")
    src_before = snapshot_tree(src_vault)

    gui_bin = discover_gui_bin(args.gui_bin)
    xvfb_proc, display = start_private_xvfb()

    try:
        with tempfile.TemporaryDirectory(prefix="hortus-l5c-mf-") as tmp:
            tmp_path = Path(tmp)

            # 1. Layer 1: 全库矩阵巡检
            matrix_vault = tmp_path / "matrix-vault"
            shutil.copytree(src_vault, matrix_vault)
            matrix_json = output_dir / "matrix-audit.json"
            matrix_json.unlink(missing_ok=True)
            matrix_env = make_isolated_env(tmp_path / "matrix-env", display)
            m_rc, m_out, m_err = run_supervised(
                [
                    str(gui_bin),
                    "--vault",
                    str(matrix_vault),
                    "--open",
                    "Start here.md",
                    "--width",
                    "1280",
                    "--height",
                    "820",
                    "--matrix-audit-json",
                    str(matrix_json),
                ],
                matrix_env,
                "全库矩阵巡检",
                timeout_s=180.0,
            )
            (output_dir / "matrix.stdout.log").write_text(m_out, encoding="utf-8")
            (output_dir / "matrix.stderr.log").write_text(m_err, encoding="utf-8")
            check(m_rc == 0, f"matrix-audit 退出码非零 ({m_rc}):\n{m_err}")
            check(matrix_json.is_file(), f"未生成 {matrix_json}")
            matrix_data = json.loads(matrix_json.read_text(encoding="utf-8"))
            m_warn_counts = count_forbidden_stderr(m_err)
            check(
                all(v == 0 for v in m_warn_counts.values()),
                f"matrix-audit stderr 存在禁止告警: {m_warn_counts}\n{m_err}",
            )
            check(
                matrix_data.get("passed") is True,
                f"matrix-audit 不变量失败: {json.dumps(matrix_data.get('failures'), ensure_ascii=False, indent=2)}",
            )

            # 2. Layer 2: 确定性有种子 GUI Fuzzer
            fuzz_vault = tmp_path / "fuzz-vault"
            shutil.copytree(src_vault, fuzz_vault)
            (fuzz_vault / "FuzzScratch.md").write_text(
                "# Fuzz Scratch\n\n> [!note] Scratch\n> Editable target for [[Start here]].\n",
                encoding="utf-8",
            )
            fuzz_json = output_dir / "fuzz.json"
            fuzz_json.unlink(missing_ok=True)
            fuzz_env = make_isolated_env(tmp_path / "fuzz-env", display)
            f_rc, f_out, f_err = run_supervised(
                [
                    str(gui_bin),
                    "--vault",
                    str(fuzz_vault),
                    "--open",
                    "Start here.md",
                    "--width",
                    "1280",
                    "--height",
                    "820",
                    "--fuzz-json",
                    str(fuzz_json),
                    "--fuzz-seed",
                    str(args.seed),
                    "--fuzz-steps",
                    str(args.steps),
                    "--fuzz-media",
                    str(media_dir),
                ],
                fuzz_env,
                "GUI 状态机 Fuzz",
                timeout_s=180.0,
            )
            (output_dir / "fuzz.stdout.log").write_text(f_out, encoding="utf-8")
            (output_dir / "fuzz.stderr.log").write_text(f_err, encoding="utf-8")
            check(f_rc == 0, f"fuzz 退出码非零 ({f_rc}):\n{f_err}")
            check(fuzz_json.is_file(), f"未生成 {fuzz_json}")
            fuzz_data = json.loads(fuzz_json.read_text(encoding="utf-8"))
            f_warn_counts = count_forbidden_stderr(f_err)
            check(
                all(v == 0 for v in f_warn_counts.values()),
                f"fuzz stderr 存在禁止告警: {f_warn_counts}\n{f_err}",
            )
            check(
                fuzz_data.get("passed") is True,
                f"GUI Fuzz 失败 (seed={args.seed}, step={fuzz_data.get('failed_step')}): {fuzz_data.get('failure_reason')}",
            )
            check(
                fuzz_data.get("steps_completed") == args.steps,
                f"GUI Fuzz 步数未达标: {fuzz_data.get('steps_completed')} != {args.steps}",
            )
    finally:
        if xvfb_proc is not None:
            xvfb_proc.terminate()
            try:
                xvfb_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                xvfb_proc.kill()

    src_after = snapshot_tree(src_vault)
    check(src_before == src_after, "fixtures/demo-vault 源目录在巡检期间被意外修改")

    if args.media is not None:
        stray_pngs = sorted(str(p.relative_to(output_dir)) for p in output_dir.rglob("*.png"))
        check(not stray_pngs, f"--output 目录内出现残留 PNG: {stray_pngs}")

    summary = {
        "passed": True,
        "gui_bin": str(gui_bin),
        "display": display,
        "source_vault_untouched": True,
        "matrix": {
            "note_count": matrix_data.get("note_count"),
            "cases_total": matrix_data.get("cases_total"),
            "failures_count": matrix_data.get("failures_count"),
            "stderr_warnings": m_warn_counts,
            "report_path": str(matrix_json.resolve()),
        },
        "fuzz": {
            "seed": args.seed,
            "steps_completed": fuzz_data.get("steps_completed"),
            "action_counts": fuzz_data.get("action_counts"),
            "stderr_warnings": f_warn_counts,
            "report_path": str(fuzz_json.resolve()),
        },
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(
        f"PASS: matrix_notes={matrix_data.get('note_count')}, "
        f"matrix_cases={matrix_data.get('cases_total')}, "
        f"fuzz_seed={args.seed}, fuzz_steps={fuzz_data.get('steps_completed')}, "
        f"0 GTK critical/warning, demo_vault_untouched=yes"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as err:
        print(f"FAIL: {err}", file=sys.stderr)
        raise SystemExit(1)
