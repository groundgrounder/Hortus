#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=27.0
# 绕过 macOS 27 上 Rust 剥离调试符号导致的 LINKEDIT 对齐问题。
export CARGO_PROFILE_RELEASE_STRIP=none
# 保留调用方的 Rust 参数，用分隔符传递，避免带空格的目录被拆开。
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
# CARGO_HOME 可能经过符号链接，编译器看到的真实路径也一并映射。
if [[ "${cargo_home:A}" != "${cargo_home:a}" ]]; then
  rust_flags+=("--remap-path-prefix=${cargo_home:A}=/cargo")
fi
export CARGO_ENCODED_RUSTFLAGS="${(pj:\x1f:)rust_flags}"
cargo build --release -p hortus-ffi
cargo run --release -p hortus-ffi --features bindgen --bin hortus-bindgen -- generate \
  --library target/release/libhortus_ffi.dylib --language swift \
  --config crates/hortus-ffi/uniffi.toml --out-dir apps/macos/Generated/bindings
mkdir -p apps/macos/Generated/HortusCore apps/macos/Generated/HortusFFI
cp apps/macos/Generated/bindings/HortusCore.swift apps/macos/Generated/HortusCore/
cp apps/macos/Generated/bindings/HortusFFI.h apps/macos/Generated/HortusFFI/
cp apps/macos/Generated/bindings/HortusFFI.modulemap apps/macos/Generated/HortusFFI/module.modulemap
swift build --package-path apps/macos -c release \
  -Xswiftc -warnings-as-errors -Xlinker "$PWD/target/release/libhortus_ffi.a" \
  -Xlinker -framework -Xlinker CoreFoundation -Xlinker -framework -Xlinker CoreServices \
  -Xlinker -framework -Xlinker Security -Xlinker -liconv
mkdir -p build/Hortus.app/Contents/MacOS build/Hortus.app/Contents/Resources
cp apps/macos/.build/release/Hortus build/Hortus.app/Contents/MacOS/
# 最终链接后再剥离本地符号，保留全局符号供崩溃报告使用。
strip -x build/Hortus.app/Contents/MacOS/Hortus
cp apps/macos/Info.plist build/Hortus.app/Contents/
./tools/build-app-icon.sh build/Hortus.app/Contents/Resources
rm -rf build/Hortus.app/Contents/Resources/en.lproj build/Hortus.app/Contents/Resources/zh-Hans.lproj
xcstringstool="$(xcrun --find xcstringstool)"
"$xcstringstool" compile apps/macos/Localizable.xcstrings \
  --output-directory build/Hortus.app/Contents/Resources --language en --language zh-Hans \
  --serialization-format text
# 应用自身的许可证和第三方许可证声明随包分发，关于窗口里有入口
cp apps/LICENSE build/Hortus.app/Contents/Resources/LICENSE.txt
python3 -B tools/third-party-notices.py --output build/Hortus.app/Contents/Resources/ThirdPartyNotices.txt
build_number="$(git rev-parse --short HEAD)"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" build/Hortus.app/Contents/Info.plist
codesign --force --sign - build/Hortus.app
print "构建完成：$PWD/build/Hortus.app"
