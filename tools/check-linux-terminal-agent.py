#!/usr/bin/env python3
"""L3 端到端离屏验证脚本：VTE4 多标签终端、安全发送、Agent 启动闸门与三级回滚、设置热重载与多窗口同步。"""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXPECTED_PNGS = [
    "l3-v3-dynamic-sidebar-tabs.png",
    "l3-v3-changes-context-menu-moved.png",
    "l3-v3-terminal-in-right-sidebar.png",
    "l3-v3-tool-add-menu-and-close.png",
    "l3-v3-agent-changes-and-multi-window.png",
    "l4-sidebars-resized-wide.png",
]


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def discover_binary(explicit: Path | None, candidates: list[Path], label: str) -> Path:
    if explicit is not None:
        check(explicit.exists(), f"未找到 {label}: {explicit}")
        return explicit.resolve()
    for path in candidates:
        if path.exists():
            return path.resolve()
    raise FileNotFoundError(f"未找到可用 {label}，请先构建或显式传参")


class PrivateDisplay:
    def __init__(self) -> None:
        self.proc: subprocess.Popen[bytes] | None = None
        self.display: str = ""

    def start(self) -> str:
        explicit = os.environ.get("HORTUS_TEST_DISPLAY")
        if explicit:
            self.display = explicit
            return explicit
        xvfb = shutil.which("Xvfb")
        check(xvfb is not None, "需要 Xvfb 以在私有离屏显示运行测试")
        assert xvfb is not None
        for num in range(95, 130):
            socket = Path(f"/tmp/.X11-unix/X{num}")
            if socket.exists():
                continue
            disp = f":{num}"
            proc = subprocess.Popen(
                [xvfb, disp, "-screen", "0", "1440x960x24", "-nolisten", "tcp"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            for _ in range(80):
                if socket.exists():
                    self.proc = proc
                    self.display = disp
                    return disp
                if proc.poll() is not None:
                    break
                time.sleep(0.1)
            proc.kill()
        raise RuntimeError("无法启动私有 Xvfb 显示")

    def stop(self) -> None:
        if self.proc is not None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            self.proc = None


def pick_interactive_shell() -> str:
    """选择支持 readline bracketed paste 的交互 Shell（规避 nix develop 注入的无 readline stdenv bash）。"""
    candidates = [
        os.environ.get("SHELL", ""),
        "/run/current-system/sw/bin/bash",
        shutil.which("bash") or "",
        "/bin/bash",
        "/bin/sh",
    ]
    for cand in candidates:
        if not cand or not Path(cand).exists():
            continue
        try:
            res = subprocess.run(
                [cand, "-i", "-c", "bind -V >/dev/null 2>&1"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=2,
            )
            if res.returncode == 0:
                return cand
        except (OSError, subprocess.SubprocessError):
            continue
    return os.environ.get("SHELL") or "/bin/sh"


def make_env(display: str, root_dir: Path, cli_dir: Path, mock_agent: Path) -> dict[str, str]:
    env = os.environ.copy()
    for key in ("WAYLAND_DISPLAY", "HORTUS_VAULT", "HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"):
        env.pop(key, None)
    cfg = root_dir / "config"
    cache = root_dir / "cache"
    data = root_dir / "data"
    cfg.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    data.mkdir(parents=True, exist_ok=True)
    env.update(
        {
            "DISPLAY": display,
            "GDK_BACKEND": "x11",
            "GSK_RENDERER": "cairo",
            "NO_AT_BRIDGE": "1",
            "GTK_A11Y": "none",
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "HORTUS_CONFIG_DIR": str(cfg),
            "HORTUS_CACHE_DIR": str(cache),
            "HORTUS_DATA_DIR": str(data),
            "XDG_CONFIG_HOME": str(cfg),
            "XDG_CACHE_HOME": str(cache),
            "XDG_DATA_HOME": str(data),
            "HORTUS_CLI_DIR": str(cli_dir),
            "HORTUS_L3_MOCK_AGENT": str(mock_agent),
            "SHELL": pick_interactive_shell(),
            "PATH": f"{cli_dir}:{env.get('PATH', '')}",
        }
    )
    return env


def write_vault_fixtures(vault1: Path, vault2: Path) -> None:
    (vault1 / "folder").mkdir(parents=True, exist_ok=True)
    (vault1 / "alpha.md").write_text(
        "# Alpha\n\nInitial alpha body.\n\nSelectable line A\n\n[[folder/sub note]]\n",
        encoding="utf-8",
    )
    (vault1 / "folder" / "sub note.md").write_text(
        "# Sub Note\n\nSpace in relative path check.\n",
        encoding="utf-8",
    )
    middle = "\n".join(f"Line {i:02d} filler content." for i in range(1, 30))
    (vault1 / "multi-hunk.md").write_text(
        f"# Multi Hunk Note\n\nSection A\nTOP_ORIGINAL\n\n{middle}\n\nSection B\nBOT_ORIGINAL\n",
        encoding="utf-8",
    )
    (vault1 / "to-delete.md").write_text(
        "# Delete Me\n\nWill be deleted by mock agent.\n",
        encoding="utf-8",
    )

    vault2.mkdir(parents=True, exist_ok=True)
    (vault2 / "beta.md").write_text("# Beta Vault 2\n\nIsolated second vault.\n", encoding="utf-8")


def write_mock_agent(path: Path) -> None:
    script = """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--sleep" ]]; then
  sleep 30
  exit 0
fi
GUIDE_TEXT="${1:-}"
if [[ -z "$GUIDE_TEXT" ]] || [[ "$GUIDE_TEXT" != *"Hortus"* ]]; then
  echo "missing expanded guide text" >&2
  exit 2
fi
if [[ -z "${HORTUS_AGENT_SESSION:-}" ]]; then
  echo "missing HORTUS_AGENT_SESSION" >&2
  exit 3
fi
cat > agent-created.md <<'EOF'
# Agent Created Note

Created inside snapshot-gated session.
EOF
python3 - <<'PY'
from pathlib import Path
p = Path("multi-hunk.md")
txt = p.read_text(encoding="utf-8")
txt = txt.replace("TOP_ORIGINAL", "TOP_AGENT_EDIT").replace("BOT_ORIGINAL", "BOT_AGENT_EDIT")
p.write_text(txt, encoding="utf-8")
PY
rm -f to-delete.md
echo "MOCK_AGENT_DONE"
"""
    path.write_text(script, encoding="utf-8")
    path.chmod(0o755)


def audit_binary(gui_bin: Path) -> dict[str, object]:
    wrapped = gui_bin.parent / f".{gui_bin.name}-wrapped"
    target = wrapped if wrapped.exists() else gui_bin
    ldd_out = subprocess.check_output(["ldd", str(target)], text=True, stderr=subprocess.STDOUT)
    has_webkit = bool(re.search(r"webkit|javascriptcore", ldd_out, re.IGNORECASE))
    has_vte = "libvte" in ldd_out.lower()
    check(not has_webkit, f"发现禁止的 WebKit 依赖: {ldd_out}")
    check(has_vte, f"未链接 libvte-gtk4: {ldd_out}")
    return {"inspected_binary": str(target), "has_webkit": has_webkit, "has_vte": has_vte}


def run_gui_process(cmd: list[str], env: dict[str, str], timeout_s: int = 90) -> tuple[int, str, str]:
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        out, err = proc.communicate(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        proc.send_signal(signal.SIGTERM)
        out, err = proc.communicate(timeout=5)
        raise TimeoutError(f"命令超时 ({timeout_s}s): {' '.join(cmd)}\nstderr:\n{err}")
    combined = out + "\n" + err
    check("panicked at" not in combined, f"检测到 Rust panic:\n{combined}")
    check("AdwToastOverlay" not in combined, f"检测到 AdwToastOverlay 警告:\n{combined}")
    check("Adwaita-CRITICAL" not in combined, f"检测到 Adwaita-CRITICAL 警告:\n{combined}")
    check("exceeds" not in combined, f"检测到 layout exceeds 警告:\n{combined}")
    return proc.returncode, out, err


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui-bin", type=Path, default=None)
    parser.add_argument("--cli-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=ROOT / "target/reports/linux-terminal-agent")
    parser.add_argument("--media", type=Path, default=None)
    args = parser.parse_args()

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    media_dir = (args.media if args.media is not None else output_dir / "media").resolve()
    media_dir.mkdir(parents=True, exist_ok=True)

    if args.gui_bin is not None:
        gui_bin = discover_binary(args.gui_bin, [], "hortus-gtk")
    else:
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
        gui_bin = (ROOT / "apps/linux/target/debug/hortus-gtk").resolve()
        check(gui_bin.exists(), f"未找到 automation 构建输出: {gui_bin}")
    cli_bin = discover_binary(
        args.cli_bin,
        [
            ROOT / "build/linux/result-cli/bin/hortus",
            ROOT / "target/debug/hortus",
            ROOT / "target/release/hortus",
        ],
        "hortus CLI",
    )
    bin_audit = audit_binary(gui_bin)

    disp = PrivateDisplay()
    display = disp.start()
    try:
        with tempfile.TemporaryDirectory(prefix="hortus-l3-") as tmp:
            tmp_path = Path(tmp)
            vault1 = tmp_path / "vault1"
            vault2 = tmp_path / "vault2"
            mock_agent = tmp_path / "mock-agent.sh"
            write_vault_fixtures(vault1, vault2)
            write_mock_agent(mock_agent)

            env = make_env(display, tmp_path / "xdg", cli_bin.parent, mock_agent)
            cfg_toml = Path(env["HORTUS_CONFIG_DIR"]) / "config.toml"
            cfg_toml.write_text(
                '# L3-PRESERVED-COMMENT\nfont_size = 15\ndefault_view = "reading"\n',
                encoding="utf-8",
            )

            main_json = output_dir / "terminal-agent.json"
            rc, out, err = run_gui_process(
                [
                    str(gui_bin),
                    "--vault",
                    str(vault1),
                    "--width",
                    "1280",
                    "--height",
                    "800",
                    "--terminal-agent-vault2",
                    str(vault2),
                    "--terminal-agent-media",
                    str(media_dir),
                    "--terminal-agent-json",
                    str(main_json),
                ],
                env,
            )
            (output_dir / "terminal-agent.stdout.log").write_text(out, encoding="utf-8")
            (output_dir / "terminal-agent.stderr.log").write_text(err, encoding="utf-8")
            check(rc == 0, f"hortus-gtk 非零退出 ({rc}):\n{err}")
            check(main_json.exists(), "未生成 terminal-agent.json")

            data = json.loads(main_json.read_text(encoding="utf-8"))
            check(data.get("ok") is True, f"L3 主套校验失败: {data}")
            for section in ("dock_v2", "dock_v3", "terminal", "settings", "agent_changes", "multi_window", "sidebar_resize", "screenshots"):
                sec_val = data.get(section, {})
                check(isinstance(sec_val, dict) and len(sec_val) > 0, f"缺少检查段 {section}: {data}")
                for k, v in sec_val.items():
                    if isinstance(v, bool):
                        check(v is True, f"检查项 {section}.{k} 未通过: {data}")
            for strip_key in ("left_strip_3", "right_strip_3", "bottom_strip_1", "right_strip_4"):
                strip_val = data["dock_v3"].get(strip_key, {})
                tabs = strip_val.get("tabs", [])
                check(len(tabs) > 0, f"{strip_key} 缺少标签测量数据: {strip_val}")
                for tab in tabs:
                    check(tab.get("has_close_button") is False, f"工具标签不应存在内联关闭按钮: {tab}")
                    check(tab.get("has_motion_controller") is False, f"工具标签不应存在悬停控制器: {tab}")
            for req_sec, req_key in (
                ("settings", "dlg_tracked"),
                ("settings", "config_subtitle_resolved"),
                ("settings", "dlg_cleared_after_close"),
                ("settings", "theme_switch_single_refresh"),
                ("multi_window", "dirty_missing_preserved_across_windows"),
                ("agent_changes", "running_agent_close_interrupted"),
                ("sidebar_resize", "layout_visibility_restored_ok"),
                ("sidebar_resize", "ui_prefs_persisted_ok"),
            ):
                check(data[req_sec].get(req_key) is True, f"缺少或未通过 L5a 校验 {req_sec}.{req_key}: {data}")
            check(
                data["terminal"]["quoted_space_path"] == "'folder/sub note.md' ",
                f"空格路径 shell 引用异常: {data['terminal']['quoted_space_path']}",
            )
            check(
                ("Session Changes" in data["agent_changes"]["changes_title"])
                or ("本次会话改动" in data["agent_changes"]["changes_title"]),
                f"改动面板文案未使用 Session Changes / 本次会话改动: {data['agent_changes']['changes_title']}",
            )

            # 检查 session.toml 含 windows 且不含任何动态终端 PTY 字段，并注入缺失路径验证恢复跳过
            session_toml = Path(env["HORTUS_CACHE_DIR"]) / "session.toml"
            check(session_toml.exists(), "未写入 session.toml")
            session_txt = session_toml.read_text(encoding="utf-8")
            check("pty" not in session_txt.lower(), f"session.toml 意外持久化了 PTY 字段:\n{session_txt}")
            injected = session_txt + '\n[[windows]]\nid = "missing-win"\npath = "/tmp/hortus-nonexistent-vault-l3"\ntabs = ["ghost.md"]\n'
            session_toml.write_text(injected, encoding="utf-8")

            restore_json = output_dir / "restore-session.json"
            rc2, out2, err2 = run_gui_process(
                [str(gui_bin), "--restore-session-json", str(restore_json)],
                env,
            )
            (output_dir / "restore-session.stdout.log").write_text(out2, encoding="utf-8")
            (output_dir / "restore-session.stderr.log").write_text(err2, encoding="utf-8")
            check(rc2 == 0, f"session restore 校验退出码异常 ({rc2}):\n{err2}")
            restore_data = json.loads(restore_json.read_text(encoding="utf-8"))
            check(restore_data.get("ok") is True, f"会话恢复检查失败: {restore_data}")
            check(
                restore_data.get("restored_count") == 1,
                f"缺失库未被跳过或窗口数不对: {restore_data}",
            )
            check(
                restore_data.get("zero_pty_restored") is True,
                f"恢复窗口意外恢复了终端 PTY: {restore_data}",
            )

            # 校验 4 张公开截图均存在且非空，并检查 --output 下零 .png 残留
            for name in EXPECTED_PNGS:
                p = media_dir / name
                check(p.exists() and p.stat().st_size > 4096, f"截图缺失或过小: {p}")
            stray_pngs = list(output_dir.rglob("*.png"))
            if media_dir != output_dir and output_dir not in media_dir.parents:
                check(not stray_pngs, f"--output 目录禁止出现 .png: {stray_pngs}")

            summary = {
                "ok": True,
                "binary_audit": bin_audit,
                "terminal_agent": data,
                "session_restore": restore_data,
                "screenshots": [str(media_dir / n) for n in EXPECTED_PNGS],
            }
            (output_dir / "summary.json").write_text(
                json.dumps(summary, ensure_ascii=False, indent=2),
                encoding="utf-8",
            )
            print(json.dumps(summary, ensure_ascii=False, indent=2))
            return 0
    finally:
        disp.stop()


if __name__ == "__main__":
    sys.exit(main())
