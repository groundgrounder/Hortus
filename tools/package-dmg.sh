#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  cat <<'EOF'
Usage: tools/package-dmg.sh [--skip-build] [--app PATH] [--output PATH]
       [--developer-id "Developer ID Application: …"] [--notary-profile NAME]
       [--validate-only]

Defaults: ad-hoc signing, build/Hortus.app, build/Hortus-0.1.0-arm64.dmg.
Developer ID mode requires a notarytool keychain profile created beforehand with
xcrun notarytool store-credentials. Credentials are never passed on this command line.
--validate-only checks arguments and tool availability without building, signing,
accessing a certificate, submitting to Apple, or creating an image.
EOF
}
fail() { printf '%s\n' "$*" >&2; exit 2; }
app="build/Hortus.app"
output="build/Hortus-0.1.0-arm64.dmg"
identity=""
profile=""
skip_build=false
validate_only=false
while (($#)); do
  case "$1" in
    --developer-id|--notary-profile|--app|--output)
      (($# >= 2)) && [[ -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
      case "$1" in
        --developer-id) identity="$2" ;;
        --notary-profile) profile="$2" ;;
        --app) app="$2"; skip_build=true ;;
        --output) output="$2" ;;
      esac
      shift 2 ;;
    --skip-build) skip_build=true; shift ;;
    --validate-only) validate_only=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
done
[[ "$output" == *.dmg ]] || fail "--output must end in .dmg"
[[ "$app" == *.app ]] || fail "--app must end in .app"
if [[ -n "$identity" ]]; then
  [[ "$identity" == "Developer ID Application: "* ]] || fail "Use a Developer ID Application certificate name"
  [[ -n "$profile" ]] || fail "--developer-id requires --notary-profile"
elif [[ -n "$profile" ]]; then
  fail "--notary-profile requires --developer-id"
fi
[[ "$(uname -m)" == arm64 ]] || fail "Packaging requires an Apple Silicon Mac"
for command in hdiutil codesign ditto shasum xcrun plutil; do
  command -v "$command" >/dev/null || fail "Missing tool: $command"
done
if [[ -n "$identity" ]]; then
  xcrun --find notarytool >/dev/null
  xcrun --find stapler >/dev/null
fi
if $validate_only; then
  printf 'Arguments valid; signing: %s\n' "${identity:-ad-hoc}"
  exit 0
fi
$skip_build || ./tools/build-macos.sh
[[ -f "$app/Contents/MacOS/Hortus" ]] || fail "Missing Hortus application: $app"
[[ "$(lipo -archs "$app/Contents/MacOS/Hortus")" == arm64 ]] || fail "Hortus must contain only arm64"
if [[ -n "$identity" ]]; then
  identities="$(security find-identity -v -p codesigning)"
  [[ "$identities" == *"\"$identity\""* ]] || fail "Signing identity not found: $identity"
fi
mkdir -p "$(dirname "$output")"
work="$(mktemp -d "$(dirname "$output")/.hortus-dmg.XXXXXX")"
trap 'rm -rf "$work"' EXIT
notarize() {
  local receipt="$work/notary-$2.json"
  if ! xcrun notarytool submit "$1" --keychain-profile "$profile" --wait --output-format json >"$receipt"; then
    cat "$receipt" >&2
    fail "Notarization submission failed: $2"
  fi
  cat "$receipt"
  [[ "$(plutil -extract status raw -o - "$receipt")" == Accepted ]] || fail "Notarization was not accepted: $2"
}
mkdir "$work/staging"
ditto "$app" "$work/staging/Hortus.app"
ln -s /Applications "$work/staging/Applications"
if [[ -n "$identity" ]]; then
  codesign --force --sign "$identity" --options runtime --timestamp "$work/staging/Hortus.app"
  ditto -c -k --keepParent "$work/staging/Hortus.app" "$work/Hortus.zip"
  notarize "$work/Hortus.zip" app
  xcrun stapler staple "$work/staging/Hortus.app"
  xcrun stapler validate "$work/staging/Hortus.app"
else
  codesign --force --sign - "$work/staging/Hortus.app"
fi
codesign --verify --deep --strict --verbose=2 "$work/staging/Hortus.app"
hdiutil create -volname Hortus -srcfolder "$work/staging" -format UDZO \
  -ov "$work/Hortus.dmg"
if [[ -n "$identity" ]]; then
  codesign --sign "$identity" --timestamp "$work/Hortus.dmg"
  notarize "$work/Hortus.dmg" dmg
  xcrun stapler staple "$work/Hortus.dmg"
  xcrun stapler validate "$work/Hortus.dmg"
fi
hdiutil verify "$work/Hortus.dmg"
mv -f "$work/Hortus.dmg" "$output"
checksum="$(shasum -a 256 "$output")"
printf '%s  %s\n' "${checksum%% *}" "$(basename "$output")" | tee "$output.sha256"
