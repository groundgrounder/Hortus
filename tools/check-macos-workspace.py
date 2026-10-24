#!/usr/bin/env python3
"""用隔离目录验证真实 App 的恢复、外部刷新与笔记库只读约束。"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "build/Hortus.app/Contents/MacOS/Hortus"


def snapshot(vault):
    paths = [vault, *vault.rglob("*")]
    return {str(path.relative_to(vault)): (path.stat().st_mtime_ns, path.stat().st_size) for path in paths}


def main():
    with tempfile.TemporaryDirectory(prefix="hortus-workspace-check-") as temporary:
        base = Path(temporary)
        vault = base / "vault"
        (vault / "chapter").mkdir(parents=True)
        (vault / ".obsidian").mkdir()
        (vault / "node_modules").mkdir()
        for name in ["2.md", "10.md", "chapter/diagram.mmd", ".obsidian/hidden.md", "node_modules/hidden.md"]:
            (vault / name).write_text("# Initial\n")
        env = dict(os.environ, HORTUS_CONFIG_DIR=str(base / "config"), HORTUS_CACHE_DIR=str(base / "cache"))
        before = snapshot(vault)
        first_path = base / "first.json"
        subprocess.run([str(APP), f"--vault={vault}", "--note=2.md", "--note=chapter/diagram.mmd",
                        f"--metrics={first_path}"], env=env, check=True, timeout=20)
        first = json.loads(first_path.read_text())
        assert snapshot(vault) == before, "扫描、读取、退出改变了笔记库"
        assert (first["notes"], first["folders"]) == (3, 1), first
        layout_path = base / "config/layout.toml"
        layout_path.write_text(layout_path.read_text().replace("size = 260.0", "size = 310.0"))
        session = base / "cache/session.toml"
        session_time = session.stat().st_mtime_ns
        restored_path = base / "restored.json"
        with subprocess.Popen([str(APP), f"--metrics={restored_path}"], env=env) as process:
            deadline = time.monotonic() + 10
            # 会话再次写入表示恢复已完成，随后由外部进程更改笔记库。
            while session.stat().st_mtime_ns == session_time:
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError("App 没有恢复会话")
                time.sleep(0.01)
            changed_at = time.monotonic()
            text = "# Updated externally\n\nFresh content.\n"
            (vault / "chapter/diagram.mmd").write_text(text)
            (vault / "10.md").rename(vault / "chapter/moved.md")
            (vault / "2.md").unlink()
            (vault / "new.markdown").write_text("# New\n")
            (vault / "last.md").write_text("# Last\n")
            (vault / "new-folder").mkdir()
            after_external_change = snapshot(vault)
            assert process.wait(timeout=20) == 0
            refresh_bound = (time.monotonic() - changed_at) * 1000
        restored = json.loads(restored_path.read_text())
        assert restored["tabs"] == first["tabs"] == ["2.md", "chapter/diagram.mmd"], restored
        assert restored["selected"] == "chapter/diagram.mmd", restored
        assert restored["title"] == "diagram.mmd", restored
        assert restored["current_characters"] == len(text), restored
        assert (restored["notes"], restored["folders"]) == (4, 2), restored
        assert restored["tree_version"] > first["tree_version"], restored
        assert snapshot(vault) == after_external_change, "监听和关闭改变了笔记库"
        layout = tomllib.loads(layout_path.read_text())
        assert layout["regions"]["left"]["size"] == 310.0, layout
        assert refresh_bound < 1000, f"界面刷新验收用时 {refresh_bound:.1f} ms"
        print(json.dumps({"session_restore": True, "layout_restore": True, "vault_unchanged": True,
                          "external_refresh_verified_within_ms": round(refresh_bound, 1),
                          "notes": restored["notes"], "folders": restored["folders"]}, indent=2))


if __name__ == "__main__":
    main()
