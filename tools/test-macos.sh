#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
./tools/build-macos.sh
swift test --package-path apps/macos -Xswiftc -warnings-as-errors \
  -Xlinker "$PWD/target/release/libhortus_ffi.a" \
  -Xlinker -framework -Xlinker CoreFoundation -Xlinker -framework -Xlinker CoreServices \
  -Xlinker -framework -Xlinker Security -Xlinker -liconv
