#!/bin/sh
# Public fake Agent for the P1-35 rollback check. It writes the reviewed change
# set after the snapshot, then exits; it never touches the network.
set -u
vault="${HORTUS_VAULT:?HORTUS_VAULT is required}"
mode="${ROLLBACK_AGENT_MODE:-full}"

if [ "$mode" = "large" ]; then
  mkdir -p "$vault/notes"
  count=0
  for file in "$vault"/notes/*.md; do
    [ -e "$file" ] || break
    printf 'large edit %s\n' "$$" >> "$file"
    count=$((count + 1))
    [ "$count" -ge 100 ] && break
  done
  printf '# Large addition\n' > "$vault/large-added.md"
  exit 0
fi

if [ "$mode" = "second" ]; then
  case "$vault" in
    *second*)
      printf '# Second only\n' > "$vault/SecondOnly.md"
      exit 0
      ;;
  esac
fi

mkdir -p "$vault/Added" "$vault/EmptyAdded" "$vault/Assets"
printf '# New note\n\nAdded during a session.\n' > "$vault/Added/New note.md"
: > "$vault/AddedZero.bin"
printf 'hidden addition\n' > "$vault/.hidden-added"
dd if=/dev/zero of="$vault/Assets/added.png" bs=128 count=1 2>/dev/null

printf 'alpha\nBETA\ngamma\n' > "$vault/Edit.md"
if [ -f "$vault/Assets/picture.png" ]; then
  size=$(stat -f %z "$vault/Assets/picture.png")
  dd if=/dev/zero of="$vault/Assets/picture.png" bs="$size" count=1 2>/dev/null
fi
chmod 600 "$vault/Mode.md" 2>/dev/null || true

mv "$vault/Rename.md" "$vault/Renamed.md"
mv "$vault/Shift.md" "$vault/Shifted.md"
printf 'one\ntwo\nthree\nfour\nFIVE\nsix\n' > "$vault/Shifted.md"
rm -f "$vault/Delete.md"
rm -f "$vault/Type.md"
mkdir -p "$vault/Type.md"
printf 'inside\n' > "$vault/Type.md/inside.md"

rm -f "$vault/LinkEdit"
ln -s "Keep.md" "$vault/LinkEdit"
rm -f "$vault/LinkNew"
ln -s "Keep.md" "$vault/LinkNew"
rm -f "$vault/LinkDrop"

# CRLF 与无末尾换行：第 1、2 个 hunk 相隔足够远，不能合并。
printf 'one\r\nchanged2\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10' > "$vault/Hunks.md"
printf 'agent changed clean\n' > "$vault/OpenClean.md"
printf 'agent changed dirty\n' > "$vault/OpenDirty.md"

mkdir -p "$vault/.git"
printf '[core]\nrollback\n' > "$vault/.git/config"
printf 'ignored\n' > "$vault/.git/ignored"
exit 0
