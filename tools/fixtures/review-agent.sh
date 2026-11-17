#!/bin/sh
# Public fake Agent for the session-review check. It mutates the opened vault
# after the snapshot, exactly like a real writing agent would, and never
# touches the network or writes outside the vault except a small scratch copy.
set -u
vault="${HORTUS_VAULT:?HORTUS_VAULT is required}"
scratch="${FAKE_AGENT_SCRATCH:-/tmp/hortus-review-agent}"
mkdir -p "$scratch"
mode="${REVIEW_AGENT_MODE:-full}"

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

# 新增：嵌套笔记、0 字节文件、隐藏文件、空目录与二进制附件。
mkdir -p "$vault/Added" "$vault/EmptyAdded" "$vault/Assets"
printf '# New note\n\nAdded during a session.\n' > "$vault/Added/New note.md"
: > "$vault/AddedZero.bin"
printf 'hidden addition\n' > "$vault/.hidden-added"
dd if=/dev/zero of="$vault/Assets/added.png" bs=128 count=1 2>/dev/null

# 修改：同长度、恢复原修改时间，证明不依赖时间戳。
if [ -f "$vault/Edit.md" ]; then
  cp -p "$vault/Edit.md" "$scratch/edit-backup"
  printf 'alpha\nBETA\ngamma\n' > "$vault/Edit.md"
  touch -r "$scratch/edit-backup" "$vault/Edit.md"
fi
if [ -f "$vault/Assets/picture.png" ]; then
  size=$(stat -f %z "$vault/Assets/picture.png")
  dd if=/dev/zero of="$vault/Assets/picture.png" bs="$size" count=1 2>/dev/null
fi
chmod 600 "$vault/Mode.md" 2>/dev/null || true

# 重命名：纯重命名与重命名并小幅编辑。
if [ -f "$vault/Rename.md" ]; then
  mv "$vault/Rename.md" "$vault/Renamed.md"
fi
if [ -f "$vault/Shift.md" ]; then
  mv "$vault/Shift.md" "$vault/Shifted.md"
fi
if [ -f "$vault/Shifted.md" ]; then
  printf 'one\ntwo\nthree\nfour\nFIVE\nsix\n' > "$vault/Shifted.md"
fi

# 删除与类型变化。
rm -f "$vault/Delete.md"
if [ -f "$vault/Type.md" ]; then
  rm -f "$vault/Type.md"
  mkdir -p "$vault/Type.md"
  printf 'inside\n' > "$vault/Type.md/inside.md"
fi

# 符号链接：修改目标、纯重命名与新增。
rm -f "$vault/LinkEdit"
ln -s "Keep.md" "$vault/LinkEdit"
if [ -L "$vault/LinkDrop" ]; then
  rm -f "$vault/LinkNew"
  ln -s "Keep.md" "$vault/LinkNew"
  rm -f "$vault/LinkDrop"
fi

# 版本库内容不属于用户改动。
mkdir -p "$vault/.git"
printf '[core]\nreviewed\n' > "$vault/.git/config"
printf 'ignored\n' > "$vault/.git/ignored"
exit 0
