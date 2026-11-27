#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""生成并校验 Hortus Linux 发行包第三方开源许可证声明。

覆盖范围：
1. `apps/linux`（默认生产特性，无 `automation`，非 build/dev 依赖）；
2. `crates/hortus-cli`（独立分发 CLI，非 build/dev 依赖）；
3. Rust 标准库（`rustc --print sysroot`）；
4. 内置 KaTeX 数学字体（OFL-1.1 + `FONT_NOTICE.txt`）；
5. GNU GPL-3.0-only 主程序许可（`apps/LICENSE`）及内部 Core 许可说明。
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-linux-gnu"
DEFAULT_OUTPUT = ROOT / "apps/linux/data/THIRD_PARTY_NOTICES.txt"
PREFERENCE = [
    "MIT",
    "Apache-2.0",
    "BSD-3-Clause",
    "BSD-2-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "CC0-1.0",
    "Unlicense",
    "MPL-2.0",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "GPL-3.0-only",
    "Apache-2.0 WITH LLVM-exception",
]
SIGNATURES = {
    "Unicode-3.0": r"UNICODE LICENSE V3|Unicode, Inc\.",
    "MPL-2.0": r"Mozilla Public License,? [Vv]ersion 2\.0",
    "Apache-2.0": r"Apache License,?\s+Version 2\.0",
    "CC0-1.0": r"CC0 1\.0 Universal|Creative Commons Zero|CREATIVE COMMONS CORPORATION IS NOT A LAW FIRM",
    "Unlicense": r"This is free and unencumbered software released into the public domain",
    "Zlib": r"This software is provided .as-is.,? without any express or implied",
    "ISC": r"Permission to use, copy, modify, and(/or)? distribute this software for any",
    "MIT": r"Permission is hereby granted, free of charge",
    "BSD-3-Clause": r"Neither the name of",
    "BSD-2-Clause": r"Redistributions in binary form must reproduce",
    "GPL-3.0-only": r"GNU GENERAL PUBLIC LICENSE\s+Version 3",
    "LGPL-2.1": r"GNU LESSER GENERAL PUBLIC LICENSE\s+Version 2\.1",
}
SEPARATOR = "=" * 78
LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|copyright|unlicense)")
NOTICE_FILE = re.compile(r"(?i)^notice")
BUNDLED = {
    "MPL-2.0": ROOT / "tools/licenses/MPL-2.0.txt",
    "GPL-3.0-only": ROOT / "apps/LICENSE",
}


def choose(expression: str) -> list[str]:
    expression = (expression or "").replace("/", " OR ")
    chosen = []
    for part in re.split(r"\s+AND\s+", expression.strip()):
        options = [option.strip(" ()") for option in re.split(r"\s+OR\s+", part.strip(" ()"))]
        options = [option for option in options if option]
        ranked = sorted(
            options,
            key=lambda option: PREFERENCE.index(option)
            if option in PREFERENCE
            else len(PREFERENCE),
        )
        if not ranked:
            raise SystemExit(f"无法解析许可证表达式：{expression!r}")
        chosen.append(ranked[0])
    return chosen


def classify(text: str) -> str | None:
    for spdx, pattern in SIGNATURES.items():
        if re.search(pattern, text):
            return spdx
    return None


def license_text(directory: Path, spdx: str, name: str) -> str:
    base = spdx.split(" WITH ")[0]
    if (
        name in {"ratex-font", "ratex-layout", "ratex-lexer", "ratex-parser", "ratex-types"}
        and base == "MIT"
    ):
        return (ROOT / "tools/licenses/RaTeX-MIT.txt").read_text(encoding="utf-8")
    files = sorted(
        path
        for path in directory.iterdir()
        if path.is_file() and LICENSE_FILE.match(path.name)
    )
    hint = {
        "Apache-2.0": "apache",
        "Unicode-3.0": "unicode",
        "CC0-1.0": "cc0",
        "Unlicense": "unlicense",
        "GPL-3.0-only": "gpl",
        "LGPL-2.1-only": "lgpl",
        "LGPL-2.1-or-later": "lgpl",
    }.get(base, base.lower())
    by_name = [path for path in files if hint in path.name.lower()]
    for path in by_name + files:
        text = path.read_text(encoding="utf-8", errors="replace")
        sig = SIGNATURES.get(base.split("-only")[0].split("-or-later")[0], r"(?!)")
        if (path in by_name and len(files) > 1) or re.search(sig, text):
            return text
    if len(files) == 1 and classify(files[0].read_text(encoding="utf-8", errors="replace")) is None:
        return files[0].read_text(encoding="utf-8", errors="replace")
    if base in BUNDLED and BUNDLED[base].exists():
        return BUNDLED[base].read_text(encoding="utf-8")
    if files:
        return files[0].read_text(encoding="utf-8", errors="replace")
    raise SystemExit(f"{name}：找不到 {spdx} 的许可证原文（{directory}）")


def find_notices(directory: Path) -> list[str]:
    return [
        path.read_text(encoding="utf-8", errors="replace")
        for path in sorted(directory.iterdir())
        if path.is_file() and NOTICE_FILE.match(path.name)
    ]


def collect_cargo_graph(manifest_path: Path, root_pkg_name: str, extra_args: list[str]) -> dict[tuple[str, str], dict]:
    cmd = [
        "cargo",
        "metadata",
        "--manifest-path",
        str(manifest_path),
        "--format-version",
        "1",
        "--locked",
        "--offline",
        "--filter-platform",
        TARGET,
    ] + extra_args
    metadata = json.loads(subprocess.run(cmd, cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    packages = {package["id"]: package for package in metadata["packages"]}
    start = next(
        package["id"]
        for package in metadata["packages"]
        if package["name"] == root_pkg_name
    )
    seen, pending = set(), [start]
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        for dependency in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    out: dict[tuple[str, str], dict] = {}
    for pkg_id in seen:
        pkg = packages[pkg_id]
        if not pkg.get("source"):
            continue
        out[(pkg["name"], pkg["version"])] = pkg
    return out


def rust_crates() -> list[dict]:
    merged = {}
    merged.update(
        collect_cargo_graph(
            ROOT / "apps/linux/Cargo.toml",
            "hortus-linux",
            ["--no-default-features"],
        )
    )
    merged.update(
        collect_cargo_graph(
            ROOT / "Cargo.toml",
            "hortus-cli",
            [],
        )
    )
    items = []
    for _, package in sorted(merged.items(), key=lambda kv: kv[0]):
        directory = Path(package["manifest_path"]).parent
        expr = package.get("license") or ""
        for spdx in choose(expr):
            items.append(
                {
                    "name": package["name"],
                    "version": package["version"],
                    "license": spdx,
                    "expression": expr,
                    "source": package.get("repository") or "",
                    "text": license_text(directory, spdx, package["name"]),
                    "notices": find_notices(directory),
                }
            )
    return items


def katex_fonts() -> dict:
    directory = ROOT / "apps/macos/Resources/Fonts/KaTeX"
    return {
        "name": "KaTeX fonts",
        "version": "0.1.15",
        "license": "OFL-1.1",
        "expression": "OFL-1.1",
        "source": "https://github.com/KaTeX/KaTeX",
        "text": (directory / "OFL.txt").read_text(encoding="utf-8"),
        "notices": [(directory / "FONT_NOTICE.txt").read_text(encoding="utf-8")],
    }


def rust_std() -> dict:
    sysroot = Path(
        subprocess.run(
            ["rustc", "--print", "sysroot"], check=True, capture_output=True, text=True
        ).stdout.strip()
    )
    sysroot_mit = sysroot / "share/doc/rust/licenses/MIT.txt"
    if sysroot_mit.is_file():
        template = sysroot_mit.read_text(encoding="utf-8")
        text = template.replace("<year> <copyright holders>", "The Rust Project Developers")
    else:
        template = (ROOT / "LICENSE-MIT").read_text(encoding="utf-8")
        text = template.replace("Copyright (c) 2026 groundgrounder", "Copyright (c) The Rust Project Developers")
    version = subprocess.run(
        ["rustc", "--version"], check=True, capture_output=True, text=True
    ).stdout.split()[1]
    return {
        "name": "Rust standard library",
        "version": version,
        "license": "MIT",
        "expression": "MIT OR Apache-2.0",
        "source": "https://github.com/rust-lang/rust",
        "text": text,
        "notices": [],
    }


def gpl_application_license() -> dict:
    return {
        "name": "Hortus Linux application (hortus-gtk)",
        "version": "1.0.0",
        "license": "GPL-3.0-only",
        "expression": "GPL-3.0-only",
        "source": "https://github.com/groundgrounder/Hortus",
        "text": (ROOT / "apps/LICENSE").read_text(encoding="utf-8"),
        "notices": [],
    }


def render(groups: list[dict]) -> str:
    lines = [
        "Hortus Linux third-party notices",
        "=" * 33,
        "",
        "Hortus Linux (hortus-gtk) is licensed under GNU GPL-3.0-only.",
        "Hortus shared crates and hortus-cli are dual-licensed under MIT OR Apache-2.0.",
        "Third-party Rust crates (apps/linux + hortus-cli, non-dev/non-build),",
        "the Rust standard library, and bundled KaTeX fonts follow below.",
        "",
    ]
    blocks: dict[str, dict] = {}
    for item in groups:
        key = re.sub(r"\s+", " ", item["text"]).strip()
        blocks.setdefault(key, {"text": item["text"], "items": []})["items"].append(item)
    for block in blocks.values():
        lines.append(SEPARATOR)
        for item in block["items"]:
            choice = (
                ""
                if item["expression"] == item["license"]
                else f' (chosen from {item["expression"]})'
            )
            lines.append(f'{item["name"]} {item["version"]} — {item["license"]}{choice}')
            if item["source"]:
                lines.append(f'  Source: {item["source"]}')
        if any(item["license"] == "MPL-2.0" for item in block["items"]):
            lines.append(
                "  These files are used unmodified. Their source code is available at the address above."
            )
        lines += ["", block["text"].strip(), ""]
        for notice in dict.fromkeys(
            n.strip() for item in block["items"] for n in item["notices"] if n.strip()
        ):
            lines += ["NOTICE:", notice, ""]
    return "\n".join(lines).rstrip() + "\n"


def generate_content() -> tuple[str, int]:
    items = rust_crates() + [rust_std(), katex_fonts(), gpl_application_license()]
    return render(items), len(items)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify existing notices file matches regenerated output",
    )
    args = parser.parse_args()
    content, count = generate_content()
    if args.check:
        if not args.output.is_file():
            raise SystemExit(f"缺少第三方许可声明文件：{args.output}")
        existing = args.output.read_text(encoding="utf-8")
        if existing != content:
            raise SystemExit(
                f"{args.output} 与当前依赖图不一致；请运行 tools/linux-third-party-notices.py 重新生成。"
            )
        print(f"OK: {args.output} matches ({count} components)")
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(content, encoding="utf-8")
    print(f"{args.output}: {count} components", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
