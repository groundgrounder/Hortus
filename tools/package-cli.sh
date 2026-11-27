#!/bin/zsh
# SPDX-License-Identifier: AGPL-3.0-only
set -euo pipefail
cd "${0:A:h:h}"

version="$(plutil -extract CFBundleShortVersionString raw -o - apps/macos/Info.plist)"

usage() {
  cat <<EOF
Usage: tools/package-cli.sh [--skip-build [--binary PATH]] [--output PATH] [--validate-only]

Defaults: build/hortus-cli-${version}-arm64.tar.gz (version from apps/macos/Info.plist).
Produces a standalone macOS arm64 CLI tarball containing hortus, LICENSE, and
ThirdPartyNotices.txt, plus a matching .sha256 checksum file.
EOF
}

fail() { printf '%s\n' "$*" >&2; exit 2; }

output="build/hortus-cli-${version}-arm64.tar.gz"
binary=""
skip_build=false
validate_only=false

while (($#)); do
  case "$1" in
    --binary)
      (($# >= 2)) && [[ -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
      binary="$2"
      shift 2 ;;
    --output)
      (($# >= 2)) && [[ -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
      output="$2"
      shift 2 ;;
    --skip-build) skip_build=true; shift ;;
    --validate-only) validate_only=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
done

[[ -z "$binary" ]] || $skip_build || fail "--binary requires --skip-build"
[[ "$output" == *.tar.gz ]] || fail "--output must end in .tar.gz"
[[ "$(uname -m)" == arm64 ]] || fail "Packaging requires an Apple Silicon Mac"
for command in tar shasum strip plutil lipo otool codesign python3; do
  command -v "$command" >/dev/null || fail "Missing tool: $command"
done

if $validate_only; then
  printf 'Arguments valid; output: %s\n' "$output"
  exit 0
fi

export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=26.0
export CARGO_PROFILE_RELEASE_STRIP=none

if ! $skip_build; then
  rust_flags=()
  if (( ${+CARGO_ENCODED_RUSTFLAGS} )); then
    if [[ -n "$CARGO_ENCODED_RUSTFLAGS" ]]; then
      rust_flags=("${(@ps:\x1f:)CARGO_ENCODED_RUSTFLAGS}")
    fi
  else
    rust_flags=(${=RUSTFLAGS:-})
  fi
  cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  rust_flags+=("--remap-path-prefix=${cargo_home:a}=/cargo"
               "--remap-path-prefix=$PWD=/hortus")
  if [[ "${cargo_home:A}" != "${cargo_home:a}" ]]; then
    rust_flags+=("--remap-path-prefix=${cargo_home:A}=/cargo")
  fi
  export CARGO_ENCODED_RUSTFLAGS="${(pj:\x1f:)rust_flags}"
  unset RUSTFLAGS
  binary="$(cargo build --release -p hortus-cli --message-format=json | python3 -c '
import json
import sys

executables = []
for line in sys.stdin:
    message = json.loads(line)
    if message["reason"] == "compiler-message":
        sys.stderr.write(message["message"].get("rendered") or "")
    if (message["reason"] == "compiler-artifact" and message.get("executable")
            and message["target"]["name"] == "hortus"
            and "bin" in message["target"]["kind"]):
        executables.append(message["executable"])
if len(executables) != 1:
    sys.exit("Cargo did not report exactly one hortus CLI binary")
print(executables[0])
')"
elif [[ -z "$binary" ]]; then
  binary="${CARGO_TARGET_DIR:-target}/release/hortus"
  if [[ ! -f "$binary" && -f "build/Hortus.app/Contents/Helpers/hortus" ]]; then
    binary="build/Hortus.app/Contents/Helpers/hortus"
  fi
fi
[[ -f "$binary" && -x "$binary" ]] || fail "Missing or non-executable hortus CLI binary: $binary"
[[ "$(lipo -archs "$binary")" == arm64 ]] || fail "hortus CLI must contain only arm64"
build_version="$(otool -l "$binary" | awk '
  $1 == "cmd" { build = ($2 == "LC_BUILD_VERSION") }
  build && $1 == "platform" { platform = $2 }
  build && $1 == "minos" { print platform, $2 }
')"
[[ "$build_version" == "1 26.0" || "$build_version" == "MACOS 26.0" ]] ||
  fail "hortus CLI must target macOS with minos 26.0"

mkdir -p "$(dirname "$output")"
work="$(mktemp -d "$(dirname "$output")/.hortus-cli-pkg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

stage_name="hortus-cli-${version}-arm64"
stage="$work/$stage_name"
mkdir -p "$stage"
cp "$binary" "$stage/hortus"
strip -x "$stage/hortus"
chmod 755 "$stage/hortus"
codesign --force --sign - "$stage/hortus"
codesign --verify --strict "$stage/hortus"

cp LICENSE "$stage/LICENSE"

python3 -B tools/third-party-notices.py --output "$stage/ThirdPartyNotices.txt"

COPYFILE_DISABLE=1 tar -czf "$work/archive.tar.gz" -C "$work" "$stage_name"
mv -f "$work/archive.tar.gz" "$output"

checksum="$(shasum -a 256 "$output")"
printf '%s  %s\n' "${checksum%% *}" "$(basename "$output")" | tee "$output.sha256"
