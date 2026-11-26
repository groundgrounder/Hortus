#!/bin/zsh
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
cd "${0:A:h:h}"

version="$(plutil -extract CFBundleShortVersionString raw -o - apps/macos/Info.plist)"

usage() {
  cat <<EOF
Usage: tools/package-cli.sh [--skip-build] [--output PATH] [--validate-only]

Defaults: build/hortus-cli-${version}-arm64.tar.gz (version from apps/macos/Info.plist).
Produces a standalone macOS arm64 CLI tarball containing hortus, LICENSE, and
ThirdPartyNotices.txt, plus a matching .sha256 checksum file.
EOF
}

fail() { printf '%s\n' "$*" >&2; exit 2; }

output="build/hortus-cli-${version}-arm64.tar.gz"
skip_build=false
validate_only=false

while (($#)); do
  case "$1" in
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

[[ "$output" == *.tar.gz ]] || fail "--output must end in .tar.gz"
[[ "$(uname -m)" == arm64 ]] || fail "Packaging requires an Apple Silicon Mac"
for command in tar shasum strip plutil lipo python3; do
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
  cargo build --release -p hortus-cli
fi

binary="target/release/hortus"
if [[ ! -f "$binary" && -f "build/Hortus.app/Contents/Helpers/hortus" ]]; then
  binary="build/Hortus.app/Contents/Helpers/hortus"
fi
[[ -f "$binary" ]] || fail "Missing hortus CLI binary: $binary"
[[ "$(lipo -archs "$binary")" == arm64 ]] || fail "hortus CLI must contain only arm64"

mkdir -p "$(dirname "$output")"
work="$(mktemp -d "$(dirname "$output")/.hortus-cli-pkg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

stage_name="hortus-cli-${version}-arm64"
stage="$work/$stage_name"
mkdir -p "$stage"
cp "$binary" "$stage/hortus"
strip -x "$stage/hortus"
chmod 755 "$stage/hortus"

{
  printf 'Hortus CLI is dual-licensed under MIT OR Apache-2.0.\n\n'
  cat LICENSE-MIT
  printf '\n---\n\n'
  cat LICENSE-APACHE
} > "$stage/LICENSE"

python3 -B tools/third-party-notices.py --output "$stage/ThirdPartyNotices.txt"

COPYFILE_DISABLE=1 tar -czf "$work/archive.tar.gz" -C "$work" "$stage_name"
mv -f "$work/archive.tar.gz" "$output"

checksum="$(shasum -a 256 "$output")"
printf '%s  %s\n' "${checksum%% *}" "$(basename "$output")" | tee "$output.sha256"
