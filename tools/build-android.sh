#!/bin/zsh
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
cd "${0:A:h:h}"

mode="debug"
while (( $# > 0 )); do
  case "$1" in
    --release)
      mode="release"
      shift
      ;;
    --debug)
      mode="debug"
      shift
      ;;
    -h|--help)
      print "用法: ./tools/build-android.sh [--debug | --release]"
      exit 0
      ;;
    *)
      print -u2 "未知参数: $1"
      exit 2
      ;;
  esac
done

# 1. 自动探测 Rust、JDK、Android SDK 与 Android NDK
if ! command -v cargo >/dev/null 2>&1 &&
   [[ -d "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin" ]]; then
  export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
fi

if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
  print -u2 "错误：未找到 cargo / rustc 工具链"
  exit 1
fi

if [[ -z "${JAVA_HOME:-}" || ! -x "${JAVA_HOME}/bin/java" ]]; then
  for candidate in \
    "/Applications/Android Studio.app/Contents/jbr/Contents/Home" \
    "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home"; do
    if [[ -x "$candidate/bin/java" ]]; then
      export JAVA_HOME="$candidate"
      break
    fi
  done
fi
if [[ -z "${JAVA_HOME:-}" || ! -x "${JAVA_HOME}/bin/java" ]]; then
  print -u2 "错误：未找到可用的 JDK（请设置 JAVA_HOME）"
  exit 1
fi
export PATH="$JAVA_HOME/bin:$PATH"

if [[ -z "${ANDROID_HOME:-}" || ! -d "$ANDROID_HOME" ]]; then
  if [[ -n "${ANDROID_SDK_ROOT:-}" && -d "${ANDROID_SDK_ROOT}" ]]; then
    export ANDROID_HOME="$ANDROID_SDK_ROOT"
  elif [[ -d "$HOME/Library/Android/sdk" ]]; then
    export ANDROID_HOME="$HOME/Library/Android/sdk"
  fi
fi
if [[ -z "${ANDROID_HOME:-}" || ! -d "$ANDROID_HOME" ]]; then
  print -u2 "错误：未找到 Android SDK（请设置 ANDROID_HOME 或 ANDROID_SDK_ROOT）"
  exit 1
fi
export ANDROID_SDK_ROOT="$ANDROID_HOME"

ndk_dir=""
if [[ -n "${ANDROID_NDK_HOME:-}" && -d "${ANDROID_NDK_HOME}" ]]; then
  ndk_dir="$ANDROID_NDK_HOME"
elif [[ -d "$ANDROID_HOME/ndk" ]]; then
  ndk_candidates=( "$ANDROID_HOME/ndk/"*(N/nOn) )
  if (( ${#ndk_candidates} > 0 )); then
    ndk_dir="${ndk_candidates[1]}"
  fi
fi
if [[ -z "$ndk_dir" && -d "/opt/homebrew/share/android-ndk" ]]; then
  ndk_dir="/opt/homebrew/share/android-ndk"
fi
if [[ -z "$ndk_dir" || ! -d "$ndk_dir" ]]; then
  print -u2 "错误：未找到 Android NDK（请设置 ANDROID_NDK_HOME）"
  exit 1
fi
export ANDROID_NDK_HOME="${ndk_dir:A}"

ndk_bin=""
for host_tag in darwin-arm64 darwin-x86_64 linux-x86_64; do
  if [[ -x "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/aarch64-linux-android29-clang" ]]; then
    ndk_bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin"
    break
  fi
done
if [[ -z "$ndk_bin" ]]; then
  print -u2 "错误：在 $ANDROID_NDK_HOME 中未找到 aarch64-linux-android29-clang"
  exit 1
fi

# 2. 路径重映射，防止本机用户路径或临时目录进入二进制
base_rust_flags=()
if (( ${+CARGO_ENCODED_RUSTFLAGS} )); then
  if [[ -n "$CARGO_ENCODED_RUSTFLAGS" ]]; then
    base_rust_flags=("${(@ps:\x1f:)CARGO_ENCODED_RUSTFLAGS}")
  fi
else
  base_rust_flags=(${=RUSTFLAGS:-})
fi
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rust_sysroot="$(rustc --print sysroot)"
target_dir="${CARGO_TARGET_DIR:-target}"
base_rust_flags+=(
  "--remap-path-prefix=$HOME=/home"
  "--remap-path-prefix=${cargo_home:a}=/cargo"
  "--remap-path-prefix=$PWD=/hortus"
  "--remap-path-prefix=${rust_sysroot:a}=/rustc-sysroot"
)
if [[ -n "${TMPDIR:-}" ]]; then
  temporary_dir="${TMPDIR%/}"
  base_rust_flags+=("--remap-path-prefix=${temporary_dir:a}=/tmp")
  if [[ "${temporary_dir:A}" != "${temporary_dir:a}" ]]; then
    base_rust_flags+=("--remap-path-prefix=${temporary_dir:A}=/tmp")
  fi
fi
if [[ "${cargo_home:A}" != "${cargo_home:a}" ]]; then
  base_rust_flags+=("--remap-path-prefix=${cargo_home:A}=/cargo")
fi
if [[ "${PWD:A}" != "$PWD" ]]; then
  base_rust_flags+=("--remap-path-prefix=${PWD:A}=/hortus")
fi
if [[ "${rust_sysroot:A}" != "${rust_sysroot:a}" ]]; then
  base_rust_flags+=("--remap-path-prefix=${rust_sysroot:A}=/rustc-sysroot")
fi

# 3. 交叉编译 aarch64-linux-android cdylib (API 29+, 16KB 页对齐)
android_rust_flags=("${base_rust_flags[@]}" "-C" "link-arg=-Wl,-z,max-page-size=16384")
print "交叉编译 hortus-ffi (aarch64-linux-android, API 29)..."
env \
  CARGO_ENCODED_RUSTFLAGS="${(pj:\x1f:)android_rust_flags}" \
  CC_aarch64_linux_android="$ndk_bin/aarch64-linux-android29-clang" \
  CXX_aarch64_linux_android="$ndk_bin/aarch64-linux-android29-clang++" \
  AR_aarch64_linux_android="$ndk_bin/llvm-ar" \
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ndk_bin/aarch64-linux-android29-clang" \
  cargo build --locked --release --target aarch64-linux-android -p hortus-ffi --lib

jni_dir="apps/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$jni_dir"
cp "$target_dir/aarch64-linux-android/release/libhortus_ffi.so" "$jni_dir/libhortus_ffi.so"
"$ndk_bin/llvm-strip" --strip-unneeded "$jni_dir/libhortus_ffi.so"

# 4. 使用主机库通过 hortus-bindgen 生成 Kotlin UniFFI 绑定
print "生成 Kotlin UniFFI 绑定..."
host_ext="dylib"
if [[ "$(uname -s)" == "Linux" ]]; then
  host_ext="so"
fi
env CARGO_ENCODED_RUSTFLAGS="${(pj:\x1f:)base_rust_flags}" \
  cargo build --locked --release -p hortus-ffi --lib
kotlin_out="apps/android/app/build/generated/uniffi/kotlin"
rm -rf "$kotlin_out"
mkdir -p "$kotlin_out"
env CARGO_ENCODED_RUSTFLAGS="${(pj:\x1f:)base_rust_flags}" \
  cargo run --locked --release -p hortus-ffi --features bindgen --bin hortus-bindgen -- generate \
    --library "$target_dir/release/libhortus_ffi.${host_ext}" \
    --language kotlin \
    --config crates/hortus-ffi/uniffi.toml \
    --out-dir "$kotlin_out" \
    --no-format

test -f "$kotlin_out/io/github/groundgrounder/hortus/core/hortus_ffi.kt"

# 5. 汇聚单一事实来源资产（KaTeX 字体、OFL、GPL 许可与第三方声明）
print "准备 Android 资产..."
assets_out="apps/android/app/build/generated/hortusAssets"
rm -rf "$assets_out"
mkdir -p "$assets_out/Fonts/KaTeX"
cp apps/macos/Resources/Fonts/KaTeX/KaTeX_*.ttf "$assets_out/Fonts/KaTeX/"
cp apps/macos/Resources/Fonts/KaTeX/OFL.txt "$assets_out/Fonts/KaTeX/"
if [[ -f apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt ]]; then
  cp apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt "$assets_out/Fonts/KaTeX/"
fi
cp apps/LICENSE "$assets_out/LICENSE.txt"
python3 -B tools/third-party-notices.py --rust-only --output "$assets_out/ThirdPartyNotices.txt"

# 6. 通过 Gradle Wrapper 组装 APK
build_number="$(git rev-list --count HEAD)"
has_release_signing=0
if [[ "$mode" == "release" ]]; then
  if [[ -n "${HORTUS_ANDROID_KEYSTORE:-}" &&
        -n "${HORTUS_ANDROID_KEYSTORE_PASSWORD:-}" &&
        -n "${HORTUS_ANDROID_KEY_ALIAS:-}" &&
        -n "${HORTUS_ANDROID_KEY_PASSWORD:-}" &&
        -f "${HORTUS_ANDROID_KEYSTORE}" ]]; then
    has_release_signing=1
  fi
  gradle_task=":app:assembleRelease"
  if (( has_release_signing == 1 )); then
    apk_src="apps/android/app/build/outputs/apk/release/app-release.apk"
    apk_dst="build/android/hortus-release-arm64.apk"
  else
    apk_src="apps/android/app/build/outputs/apk/release/app-release-unsigned.apk"
    apk_dst="build/android/hortus-release-unsigned.apk"
  fi
else
  gradle_task=":app:assembleDebug"
  apk_src="apps/android/app/build/outputs/apk/debug/app-debug.apk"
  apk_dst="build/android/hortus-debug.apk"
fi

print "构建 Android APK ($mode, versionCode=$build_number)..."
(
  cd apps/android
  ./gradlew "$gradle_task" "-PhortusVersionCode=$build_number" "-PhortusNdkPath=$ANDROID_NDK_HOME" --no-daemon
)

mkdir -p build/android
cp "$apk_src" "$apk_dst"
if [[ "$mode" == "release" ]] && (( has_release_signing == 1 )); then
  (
    cd build/android
    shasum -a 256 "hortus-release-arm64.apk" > "hortus-release-arm64.apk.sha256"
  )
  print "生成校验文件：$PWD/${apk_dst}.sha256"
fi
print "构建完成：$PWD/$apk_dst"
