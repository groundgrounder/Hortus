#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="${ROOT}/build/linux"
mkdir -p "${OUT_DIR}/bin"

WITH_AUTOMATION=0
USE_NIX=0
for arg in "$@"; do
  case "$arg" in
    --nix) USE_NIX=1 ;;
    --automation) WITH_AUTOMATION=1 ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

if [[ "$USE_NIX" == "1" ]]; then
  nix build .#hortus-cli -o "${OUT_DIR}/result-cli"
  nix build .#hortus-linux -o "${OUT_DIR}/result-linux"
  echo "Nix 构建完成（无 automation 生产包）："
  echo "  CLI: ${OUT_DIR}/result-cli/bin/hortus"
  echo "  GUI: ${OUT_DIR}/result-linux/bin/hortus-gtk"
  exit 0
fi

CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
REAL_CARGO_HOME="$(readlink -f "$CARGO_HOME_DIR" 2>/dev/null || echo "$CARGO_HOME_DIR")"
REAL_ROOT="$(readlink -f "$ROOT" 2>/dev/null || echo "$ROOT")"

FLAGS=""
add_flag() {
  if [[ -z "$FLAGS" ]]; then
    FLAGS="$1"
  else
    FLAGS="${FLAGS}"$'\x1f'"$1"
  fi
}

if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  FLAGS="$CARGO_ENCODED_RUSTFLAGS"
fi
add_flag "--remap-path-prefix=${CARGO_HOME_DIR}=/cargo"
if [[ "$REAL_CARGO_HOME" != "$CARGO_HOME_DIR" ]]; then
  add_flag "--remap-path-prefix=${REAL_CARGO_HOME}=/cargo"
fi
add_flag "--remap-path-prefix=${ROOT}=/hortus"
if [[ "$REAL_ROOT" != "$ROOT" ]]; then
  add_flag "--remap-path-prefix=${REAL_ROOT}=/hortus"
fi

export CARGO_ENCODED_RUSTFLAGS="$FLAGS"

# 在 nix develop 环境中，bintools wrapper 会把不存在的 $PWD/outputs/out/lib 写入 DT_RUNPATH，
# 既清理构建前 NIX_LDFLAGS，也在复制产物后用 patchelf 移除任何用户目录 rpath。
if [[ -n "${NIX_LDFLAGS:-}" && -n "${out:-}" ]]; then
  NIX_LDFLAGS="${NIX_LDFLAGS//-rpath ${out}\/lib/}"
  export NIX_LDFLAGS
fi

sanitize_rpath() {
  local bin="$1"
  if ! command -v patchelf >/dev/null 2>&1; then
    return 0
  fi
  local current
  current="$(patchelf --print-rpath "$bin" 2>/dev/null || true)"
  if [[ -z "$current" ]]; then
    return 0
  fi
  local cleaned=""
  local entry
  IFS=':' read -r -a entries <<<"$current"
  for entry in "${entries[@]}"; do
    case "$entry" in
      /home/*|/Users/*|"") continue ;;
    esac
    if [[ -z "$cleaned" ]]; then
      cleaned="$entry"
    else
      cleaned="${cleaned}:${entry}"
    fi
  done
  if [[ "$cleaned" != "$current" ]]; then
    chmod u+w "$bin"
    patchelf --set-rpath "$cleaned" "$bin"
  fi
}

cargo build --release -p hortus-cli
sanitize_rpath "${ROOT}/target/release/hortus"
cp -f "${ROOT}/target/release/hortus" "${OUT_DIR}/bin/hortus"
sanitize_rpath "${OUT_DIR}/bin/hortus"

(
  cd "${ROOT}/apps/linux"
  if [[ "$WITH_AUTOMATION" == "1" ]]; then
    cargo build --release --locked --features automation
  else
    cargo build --release --locked --no-default-features
  fi
)
sanitize_rpath "${ROOT}/apps/linux/target/release/hortus-gtk"
cp -f "${ROOT}/apps/linux/target/release/hortus-gtk" "${OUT_DIR}/bin/hortus-gtk"
sanitize_rpath "${OUT_DIR}/bin/hortus-gtk"

echo "Release 构建完成："
echo "  CLI: ${OUT_DIR}/bin/hortus"
echo "  GUI: ${OUT_DIR}/bin/hortus-gtk"
