#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
set -euo pipefail
cd "$(dirname "$0")/.."
destination="${1:-build/Hortus.app/Contents/Resources}"
mkdir -p "$destination" build/app-icon
xcrun actool apps/macos/Resources/AppIcon/Hortus.icon \
  --compile "$destination" --platform macosx --minimum-deployment-target 26.0 \
  --app-icon Hortus --output-partial-info-plist build/app-icon/Info.plist \
  --warnings --errors --output-format human-readable-text
test -s "$destination/Assets.car"
test -s "$destination/Hortus.icns"
