#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:${HOME}/.cargo/bin:/opt/homebrew/bin:${PATH}"

MODE=""
ARCHIVE_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --simulator|--ipa|--all)
      if [[ -n "$MODE" && "$MODE" != "$1" ]]; then
        echo "Usage: $0 [--simulator|--ipa|--all] [--archive-dir <dir>]" >&2
        exit 2
      fi
      MODE="$1"
      shift
      ;;
    --archive-dir)
      if [[ $# -lt 2 ]]; then
        echo "Missing value for --archive-dir" >&2
        exit 2
      fi
      ARCHIVE_DIR="$2"
      shift 2
      ;;
    --archive-dir=*)
      ARCHIVE_DIR="${1#*=}"
      shift
      ;;
    *)
      echo "Usage: $0 [--simulator|--ipa|--all] [--archive-dir <dir>]" >&2
      exit 2
      ;;
  esac
done

MODE="${MODE:---simulator}"

build_rust_ffi() {
  local target="$1"
  local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  local rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
  local remap=""
  remap+=" --remap-path-prefix=${ROOT}=."
  remap+=" --remap-path-prefix=${cargo_home}=/cargo"
  remap+=" --remap-path-prefix=${rustup_home}=/rustup"
  remap+=" --remap-path-prefix=${HOME}=/home"
  RUSTFLAGS="${RUSTFLAGS:-}${remap}" \
  IPHONEOS_DEPLOYMENT_TARGET=18.0 \
    cargo build --release --locked --offline --target "$target" -p hortus-ffi
}

build_simulator() {
  build_rust_ffi aarch64-apple-ios-sim
  xcodebuild build \
    -project apps/ios/Hortus.xcodeproj \
    -scheme Hortus \
    -configuration Debug \
    -sdk iphonesimulator \
    -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath apps/ios/DerivedData \
    ARCHS=arm64 \
    EXCLUDED_ARCHS=x86_64 \
    ONLY_ACTIVE_ARCH=NO \
    CODE_SIGNING_ALLOWED=NO

  mkdir -p build/ios
  rm -rf build/ios/Hortus-sim.app
  ditto apps/ios/DerivedData/Build/Products/Debug-iphonesimulator/Hortus.app build/ios/Hortus-sim.app
  test -x build/ios/Hortus-sim.app/Hortus
  echo "Built Simulator app: $ROOT/build/ios/Hortus-sim.app"
}

build_ipa() {
  build_rust_ffi aarch64-apple-ios
  local build_num
  build_num="$(git rev-list --count HEAD 2>/dev/null || echo 1)"

  xcodebuild build \
    -project apps/ios/Hortus.xcodeproj \
    -scheme Hortus \
    -configuration Release \
    -sdk iphoneos \
    -destination 'generic/platform=iOS' \
    -derivedDataPath apps/ios/DerivedData \
    CODE_SIGNING_ALLOWED=NO \
    CODE_SIGNING_REQUIRED=NO \
    CODE_SIGN_IDENTITY="" \
    CURRENT_PROJECT_VERSION="$build_num"

  local built_app="apps/ios/DerivedData/Build/Products/Release-iphoneos/Hortus.app"
  test -d "$built_app"

  mkdir -p build/ios/Payload
  rm -rf build/ios/Payload/Hortus.app build/ios/Hortus-unsigned.ipa build/ios/Hortus-unsigned.ipa.sha256
  ditto "$built_app" build/ios/Payload/Hortus.app
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_num" build/ios/Payload/Hortus.app/Info.plist
  rm -rf build/ios/Payload/Hortus.app/_CodeSignature

  (
    cd build/ios
    zip -qry Hortus-unsigned.ipa Payload
    shasum -a 256 Hortus-unsigned.ipa > Hortus-unsigned.ipa.sha256
    rm -rf Payload
  )

  if [[ -n "$ARCHIVE_DIR" ]]; then
    mkdir -p "$ARCHIVE_DIR"
    cp -f build/ios/Hortus-unsigned.ipa "$ARCHIVE_DIR/Hortus-unsigned.ipa"
    cp -f build/ios/Hortus-unsigned.ipa.sha256 "$ARCHIVE_DIR/Hortus-unsigned.ipa.sha256"
    (cd "$ARCHIVE_DIR" && shasum -a 256 -c Hortus-unsigned.ipa.sha256)
  fi
  echo "Built unsigned IPA: $ROOT/build/ios/Hortus-unsigned.ipa"
}

case "$MODE" in
  --simulator)
    build_simulator
    ;;
  --ipa)
    build_ipa
    ;;
  --all)
    build_simulator
    build_ipa
    ;;
esac
