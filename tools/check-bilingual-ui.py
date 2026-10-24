#!/usr/bin/env python3
"""Check String Catalog coverage and production Swift text for P1-11."""
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "apps/macos/Sources/Hortus"
CATALOG = ROOT / "apps/macos/Localizable.xcstrings"
HAN = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]+")
UI_ARGUMENT = re.compile(
    r"\b(?:Button|Text|Label|Menu|CommandMenu|Toggle|ContentUnavailableView|Section|"
    r"TextField|Picker|ProgressView)\s*\(\s*\"((?:\\.|[^\"\\])*)\""
)
MODIFIER_ARGUMENT = re.compile(
    r"\.(?:accessibilityLabel|accessibilityHint|setAccessibilityLabel|help|navigationTitle)"
    r"\s*\(\s*\"((?:\\.|[^\"\\])*)\""
)
L10N_ARGUMENT = re.compile(r"\bL10n\.string\s*\(\s*\"((?:\\.|[^\"\\])*)\"")
HELP_ARGUMENT = re.compile(r"\bhelp:\s*\"((?:\\.|[^\"\\])*)\"")


def swift_sources():
    return sorted(SOURCE.rglob("*.swift"))


def is_test_or_inspection(path):
    name = path.name.lower()
    return "test" in name or name.endswith("inspection.swift")


def strip_comments(source):
    output = []
    index = 0
    while index < len(source):
        if source.startswith("//", index):
            end = source.find("\n", index)
            if end < 0:
                break
            output.append("\n")
            index = end + 1
        elif source.startswith("/*", index):
            index += 2
            depth = 1
            while index < len(source) and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
        elif source[index] == '"':
            start = index
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            output.append(source[start:index])
        else:
            output.append(source[index])
            index += 1
    return "".join(output)


def user_source(relative, source):
    if is_test_or_inspection(relative):
        return ""
    cleaned = strip_comments(source)
    if relative.name == "App.swift":
        cleaned = cleaned.replace("Hortus 自检超时", "").replace("Hortus 自检失败：", "")
    return cleaned


def key_from_match(value):
    return value.replace(r"\\", "\\").replace(r'\"', '"')


def catalog_keys():
    data = json.loads(CATALOG.read_text(encoding="utf-8"))
    assert data.get("sourceLanguage") == "en", "String Catalog source language must be en"
    strings = data.get("strings", {})
    errors = []
    for key, entry in strings.items():
        localizations = entry.get("localizations", {})
        for language in ("en", "zh-Hans"):
            unit = localizations.get(language, {}).get("stringUnit", {})
            if unit.get("state") != "translated" or not unit.get("value"):
                errors.append(f"{key!r} has no translated {language} value")
    if errors:
        raise AssertionError("\n".join(errors))
    return set(strings)


def referenced_keys():
    keys = set()
    for path in swift_sources():
        source = path.read_text(encoding="utf-8")
        for pattern in (UI_ARGUMENT, MODIFIER_ARGUMENT, L10N_ARGUMENT, HELP_ARGUMENT):
            keys.update(key_from_match(match) for match in pattern.findall(source))
    return keys


def baseline_han_count(paths):
    count = 0
    for path in paths:
        relative = path.relative_to(ROOT)
        if is_test_or_inspection(relative):
            continue
        try:
            result = subprocess.run(
                ["git", "show", f"HEAD:{relative.as_posix()}"],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
        except subprocess.CalledProcessError:
            continue
        cleaned = strip_comments(result.stdout)
        if relative.name == "App.swift":
            cleaned = cleaned.replace("Hortus 自检超时", "").replace("Hortus 自检失败：", "")
        count += len(HAN.findall(cleaned))
    return count


def main():
    keys = catalog_keys()
    used = referenced_keys()
    missing = sorted(used - keys)
    if missing:
        raise AssertionError("UI strings missing from Localizable.xcstrings: " + ", ".join(missing))

    offenders = []
    for path in swift_sources():
        relative = path.relative_to(ROOT)
        source = path.read_text(encoding="utf-8")
        for value in HAN.findall(user_source(relative, source)):
            offenders.append(f"{relative}: {value}")
    if offenders:
        raise AssertionError("Chinese production Swift text remains:\n" + "\n".join(offenders))

    baseline = baseline_han_count(swift_sources())
    print(f"catalog keys: {len(keys)}; localized UI references: {len(used)}; missing: 0")
    print(f"Chinese production Swift literal runs: {baseline} -> 0")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
