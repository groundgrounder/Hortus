#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DEFAULT_VERSION="$(python3 -B -c 'import pathlib, tomllib; print(tomllib.loads(pathlib.Path("apps/linux/Cargo.toml").read_text(encoding="utf-8"))["package"]["version"])')"
VERSION="$DEFAULT_VERSION"
ARCH="x86_64-linux"
DIST_DIR="${ROOT}/build/linux/dist"
mkdir -p "$DIST_DIR"

usage() {
  cat <<'EOF'
Usage: tools/package-linux.sh [--version X.Y.Z] [--nix] [--tarball] [--deb] [--rpm] [--all]
EOF
}

if [[ "$#" -eq 0 ]]; then
  usage >&2
  exit 2
fi

DO_NIX=0
DO_TARBALL=0
DO_DEB=0
DO_RPM=0

while [[ "$#" -gt 0 ]]; do
  arg="$1"
  case "$arg" in
    --version)
      if [[ "$#" -lt 2 ]]; then
        echo "Missing value for --version" >&2
        usage >&2
        exit 2
      fi
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${arg#--version=}"
      shift
      ;;
    --nix) DO_NIX=1; shift ;;
    --tarball) DO_TARBALL=1; shift ;;
    --deb) DO_DEB=1; shift ;;
    --rpm) DO_RPM=1; shift ;;
    --all) DO_NIX=1; DO_TARBALL=1; DO_DEB=1; DO_RPM=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "ERROR: resolved package version is empty" >&2
  exit 2
fi

if [[ "$DO_NIX" == "0" && "$DO_TARBALL" == "0" && "$DO_DEB" == "0" && "$DO_RPM" == "0" ]]; then
  usage >&2
  exit 2
fi

BINS_READY=0
ensure_release_bins() {
  if [[ "$BINS_READY" == "1" ]]; then
    return 0
  fi
  # Always run production build (--no-default-features) so automation flags cannot leak into packages.
  bash "${ROOT}/tools/build-linux.sh"
  BINS_READY=1
}

write_sha256() {
  local artifact="$1"
  (
    cd "$(dirname "$artifact")"
    sha256sum "$(basename "$artifact")" > "$(basename "$artifact").sha256"
  )
}

run_nix() {
  nix build .#hortus-cli -o "${ROOT}/build/linux/result-cli"
  nix build .#hortus-linux -o "${ROOT}/build/linux/result-linux"
  echo "Nix outputs ready:"
  echo "  ${ROOT}/build/linux/result-cli"
  echo "  ${ROOT}/build/linux/result-linux"
}

run_tarball() {
  python3 "${ROOT}/tools/linux-third-party-notices.py" --check
  ensure_release_bins

  local gui_stage="${ROOT}/build/linux/stage-gui/hortus-linux-${VERSION}-${ARCH}"
  local cli_stage="${ROOT}/build/linux/stage-cli/hortus-cli-${VERSION}-${ARCH}"
  rm -rf "${ROOT}/build/linux/stage-gui" "${ROOT}/build/linux/stage-cli"

  mkdir -p \
    "${gui_stage}/bin" \
    "${gui_stage}/share/applications" \
    "${gui_stage}/share/metainfo" \
    "${gui_stage}/share/icons/hicolor/scalable/apps" \
    "${gui_stage}/share/licenses/hortus-linux" \
    "${gui_stage}/share/hortus/fonts/KaTeX"
  cp -f "${ROOT}/build/linux/bin/hortus-gtk" "${gui_stage}/bin/hortus-gtk"
  cp -f "${ROOT}/build/linux/bin/hortus" "${gui_stage}/bin/hortus"
  chmod 755 "${gui_stage}/bin/hortus-gtk" "${gui_stage}/bin/hortus"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.desktop" \
    "${gui_stage}/share/applications/io.github.groundgrounder.Hortus.desktop"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.metainfo.xml" \
    "${gui_stage}/share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml"
  cp -f "${ROOT}/apps/linux/data/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg" \
    "${gui_stage}/share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg"
  cp -f "${ROOT}/apps/LICENSE" "${gui_stage}/share/licenses/hortus-linux/LICENSE"
  cp -f "${ROOT}/apps/linux/data/THIRD_PARTY_NOTICES.txt" \
    "${gui_stage}/share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt"
  cp -f "${ROOT}/apps/macos/Resources/Fonts/KaTeX/"KaTeX_*.ttf \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/OFL.txt" \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt" \
    "${gui_stage}/share/hortus/fonts/KaTeX/"

  local gui_tgz="${DIST_DIR}/hortus-linux-${VERSION}-${ARCH}.tar.gz"
  tar -C "${ROOT}/build/linux/stage-gui" -czf "$gui_tgz" "hortus-linux-${VERSION}-${ARCH}"
  write_sha256 "$gui_tgz"

  mkdir -p "${cli_stage}/bin" "${cli_stage}/share/licenses/hortus-cli"
  cp -f "${ROOT}/build/linux/bin/hortus" "${cli_stage}/bin/hortus"
  chmod 755 "${cli_stage}/bin/hortus"
  cp -f "${ROOT}/LICENSE-APACHE" "${cli_stage}/share/licenses/hortus-cli/LICENSE-APACHE"
  cp -f "${ROOT}/LICENSE-MIT" "${cli_stage}/share/licenses/hortus-cli/LICENSE-MIT"

  local cli_tgz="${DIST_DIR}/hortus-cli-${VERSION}-${ARCH}.tar.gz"
  tar -C "${ROOT}/build/linux/stage-cli" -czf "$cli_tgz" "hortus-cli-${VERSION}-${ARCH}"
  write_sha256 "$cli_tgz"

  echo "Tarballs written:"
  echo "  ${gui_tgz}"
  echo "  ${cli_tgz}"
}

run_deb() {
  python3 "${ROOT}/tools/linux-third-party-notices.py" --check
  ensure_release_bins

  local out_deb="${DIST_DIR}/hortus-linux_${VERSION}_amd64.deb"
  if command -v cargo-deb >/dev/null 2>&1; then
    (
      cd "${ROOT}/apps/linux"
      cargo deb --no-build --offline -o "$out_deb"
    )
    write_sha256 "$out_deb"
    echo "Debian package written (cargo-deb): ${out_deb}"
    return 0
  fi

  local stage="${ROOT}/build/linux/stage-deb"
  rm -rf "$stage"
  mkdir -p \
    "${stage}/DEBIAN" \
    "${stage}/usr/bin" \
    "${stage}/usr/share/applications" \
    "${stage}/usr/share/metainfo" \
    "${stage}/usr/share/icons/hicolor/scalable/apps" \
    "${stage}/usr/share/licenses/hortus-linux" \
    "${stage}/usr/share/hortus/fonts/KaTeX"

  cp -f "${ROOT}/build/linux/bin/hortus-gtk" "${stage}/usr/bin/hortus-gtk"
  cp -f "${ROOT}/build/linux/bin/hortus" "${stage}/usr/bin/hortus"
  chmod 755 "${stage}/usr/bin/hortus-gtk" "${stage}/usr/bin/hortus"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.desktop" \
    "${stage}/usr/share/applications/io.github.groundgrounder.Hortus.desktop"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.metainfo.xml" \
    "${stage}/usr/share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml"
  cp -f "${ROOT}/apps/linux/data/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg" \
    "${stage}/usr/share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg"
  cp -f "${ROOT}/apps/LICENSE" "${stage}/usr/share/licenses/hortus-linux/LICENSE"
  cp -f "${ROOT}/apps/linux/data/THIRD_PARTY_NOTICES.txt" \
    "${stage}/usr/share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt"
  cp -f "${ROOT}/apps/macos/Resources/Fonts/KaTeX/"KaTeX_*.ttf \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/OFL.txt" \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt" \
    "${stage}/usr/share/hortus/fonts/KaTeX/"

  local installed_kib
  installed_kib="$(du -sk "${stage}/usr" | awk '{print $1}')"
  cat > "${stage}/DEBIAN/control" <<EOF
Package: hortus-linux
Version: ${VERSION}
Section: editors
Priority: optional
Architecture: amd64
Maintainer: groundgrounder
Installed-Size: ${installed_kib}
Depends: libgtk-4-1 (>= 4.16), libadwaita-1-0 (>= 1.6), libvte-2.91-gtk4-0 (>= 0.76), librsvg2-common, webp-pixbuf-loader, gsettings-desktop-schemas, adwaita-icon-theme, hicolor-icon-theme
Description: Native GTK4 and libadwaita Markdown reader and editor
 Hortus Linux GUI with local Mermaid diagrams, math rendering, terminal, and Agent change review.
EOF

  if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb --root-owner-group --build "$stage" "$out_deb"
    dpkg-deb -I "$out_deb" >/dev/null
    dpkg-deb -c "$out_deb" >/dev/null
    write_sha256 "$out_deb"
    echo "Debian package written (host dpkg-deb): ${out_deb}"
    return 0
  fi

  local rt=""
  if command -v podman >/dev/null 2>&1; then rt="podman"
  elif command -v docker >/dev/null 2>&1; then rt="docker"
  fi
  if [[ -n "$rt" ]] && "$rt" image exists "docker.m.daocloud.io/library/debian:stable" >/dev/null 2>&1; then
    "$rt" run --rm --network=none \
      -v "${stage}:/work/stage:ro" \
      -v "${DIST_DIR}:/work/dist" \
      docker.m.daocloud.io/library/debian:stable \
      bash -c "dpkg-deb --root-owner-group --build /work/stage /work/dist/hortus-linux_${VERSION}_amd64.deb && dpkg-deb -I /work/dist/hortus-linux_${VERSION}_amd64.deb && dpkg-deb -c /work/dist/hortus-linux_${VERSION}_amd64.deb"
    write_sha256 "$out_deb"
    echo "Debian package written (offline container dpkg-deb): ${out_deb}"
    return 0
  fi

  echo "ERROR: no cargo-deb, host dpkg-deb, or cached debian:stable container available" >&2
  return 1
}

run_rpm() {
  python3 "${ROOT}/tools/linux-third-party-notices.py" --check
  ensure_release_bins

  local out_rpm="${DIST_DIR}/hortus-linux-${VERSION}-1.x86_64.rpm"
  if command -v cargo-generate-rpm >/dev/null 2>&1; then
    (
      cd "${ROOT}/apps/linux"
      cargo generate-rpm -o "$out_rpm"
    )
    if [[ ! -s "$out_rpm" ]]; then
      echo "ERROR: cargo-generate-rpm did not produce ${out_rpm}" >&2
      return 1
    fi
    write_sha256 "$out_rpm"
    echo "RPM package written (cargo-generate-rpm): ${out_rpm}"
    return 0
  fi

  local rpm_top="${ROOT}/build/linux/stage-rpm"
  rm -rf "$rpm_top"
  mkdir -p "${rpm_top}"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
  cat > "${rpm_top}/SPECS/hortus-linux.spec" <<EOF
%global debug_package %{nil}
%define _build_id_links none

Name:           hortus-linux
Version:        ${VERSION}
Release:        1
Summary:        Native GTK4 and libadwaita Markdown reader and editor
License:        GPL-3.0-only
URL:            https://github.com/groundgrounder/Hortus
BuildArch:      x86_64
Requires:       gtk4 >= 4.16, libadwaita >= 1.6, vte291-gtk4 >= 0.76, librsvg2, webp-pixbuf-loader, gsettings-desktop-schemas, adwaita-icon-theme, hicolor-icon-theme

%description
Hortus Linux GUI with local Mermaid diagrams, math rendering, terminal, and Agent change review.

%prep
# Prebuilt binaries and assets are staged directly in %{_sourcedir}.

%build
# Binaries are already compiled by tools/build-linux.sh.

%install
mkdir -p %{buildroot}/usr/bin %{buildroot}/usr/share/applications %{buildroot}/usr/share/metainfo %{buildroot}/usr/share/icons/hicolor/scalable/apps %{buildroot}/usr/share/licenses/hortus-linux %{buildroot}/usr/share/hortus/fonts/KaTeX
install -m 755 %{_sourcedir}/hortus-gtk %{buildroot}/usr/bin/hortus-gtk
install -m 755 %{_sourcedir}/hortus %{buildroot}/usr/bin/hortus
install -m 644 %{_sourcedir}/io.github.groundgrounder.Hortus.desktop %{buildroot}/usr/share/applications/io.github.groundgrounder.Hortus.desktop
install -m 644 %{_sourcedir}/io.github.groundgrounder.Hortus.metainfo.xml %{buildroot}/usr/share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml
install -m 644 %{_sourcedir}/io.github.groundgrounder.Hortus.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg
install -m 644 %{_sourcedir}/LICENSE %{buildroot}/usr/share/licenses/hortus-linux/LICENSE
install -m 644 %{_sourcedir}/THIRD_PARTY_NOTICES.txt %{buildroot}/usr/share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt
install -m 644 %{_sourcedir}/katex/* %{buildroot}/usr/share/hortus/fonts/KaTeX/

%files
/usr/bin/hortus-gtk
/usr/bin/hortus
/usr/share/applications/io.github.groundgrounder.Hortus.desktop
/usr/share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml
/usr/share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg
/usr/share/licenses/hortus-linux/LICENSE
/usr/share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt
/usr/share/hortus/fonts/KaTeX/*
EOF
  cp -f "${ROOT}/build/linux/bin/hortus-gtk" "${rpm_top}/SOURCES/hortus-gtk"
  cp -f "${ROOT}/build/linux/bin/hortus" "${rpm_top}/SOURCES/hortus"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.desktop" "${rpm_top}/SOURCES/"
  cp -f "${ROOT}/apps/linux/data/io.github.groundgrounder.Hortus.metainfo.xml" "${rpm_top}/SOURCES/"
  cp -f "${ROOT}/apps/linux/data/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg" "${rpm_top}/SOURCES/"
  cp -f "${ROOT}/apps/LICENSE" "${rpm_top}/SOURCES/LICENSE"
  cp -f "${ROOT}/apps/linux/data/THIRD_PARTY_NOTICES.txt" "${rpm_top}/SOURCES/THIRD_PARTY_NOTICES.txt"
  mkdir -p "${rpm_top}/SOURCES/katex"
  cp -f "${ROOT}/apps/macos/Resources/Fonts/KaTeX/"KaTeX_*.ttf \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/OFL.txt" \
    "${ROOT}/apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt" \
    "${rpm_top}/SOURCES/katex/"

  if command -v rpmbuild >/dev/null 2>&1; then
    rpmbuild --define "_topdir ${rpm_top}" -bb "${rpm_top}/SPECS/hortus-linux.spec"
    local built_rpm
    built_rpm="$(find "${rpm_top}/RPMS" -name '*.rpm' | head -n 1)"
    if [[ -z "$built_rpm" || ! -s "$built_rpm" ]]; then
      echo "ERROR: host rpmbuild did not produce an RPM artifact" >&2
      return 1
    fi
    cp -f "$built_rpm" "$out_rpm"
    write_sha256 "$out_rpm"
    echo "RPM package written (host rpmbuild): ${out_rpm}"
    return 0
  fi

  local rt=""
  if command -v podman >/dev/null 2>&1; then rt="podman"
  elif command -v docker >/dev/null 2>&1; then rt="docker"
  fi
  if [[ -n "$rt" ]] && "$rt" image exists "registry.fedoraproject.org/fedora:latest" >/dev/null 2>&1; then
    if "$rt" run --rm --network=none registry.fedoraproject.org/fedora:latest bash -c 'command -v rpmbuild >/dev/null 2>&1'; then
      "$rt" run --rm --network=none \
        -v "${rpm_top}:/work/rpm" \
        -v "${DIST_DIR}:/work/dist" \
        registry.fedoraproject.org/fedora:latest \
        bash -c "rpmbuild --define '_topdir /work/rpm' -bb /work/rpm/SPECS/hortus-linux.spec && cp -f /work/rpm/RPMS/x86_64/*.rpm /work/dist/hortus-linux-${VERSION}-1.x86_64.rpm"
      if [[ ! -s "$out_rpm" ]]; then
        echo "ERROR: container rpmbuild did not produce ${out_rpm}" >&2
        return 1
      fi
      write_sha256 "$out_rpm"
      echo "RPM package written (offline container rpmbuild): ${out_rpm}"
      return 0
    fi
    echo "ERROR: offline RPM packaging requires cargo-generate-rpm or rpmbuild (not installed in cached fedora:latest without network)" >&2
    return 1
  fi

  echo "ERROR: no cargo-generate-rpm, host rpmbuild, or cached fedora:latest container available" >&2
  return 1
}

if [[ "$DO_NIX" == "1" ]]; then run_nix; fi
if [[ "$DO_TARBALL" == "1" ]]; then run_tarball; fi
if [[ "$DO_DEB" == "1" ]]; then run_deb; fi
if [[ "$DO_RPM" == "1" ]]; then run_rpm; fi
