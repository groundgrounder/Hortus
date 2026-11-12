#!/bin/sh
# Public fake Agent for Hortus checks. It records the exact argv, cwd and
# selected environment outside the knowledge base, then optionally sleeps.
# It never touches the network and never writes inside the vault.
set -u
scratch="${FAKE_AGENT_SCRATCH:?FAKE_AGENT_SCRATCH is required}"
mkdir -p "$scratch"
printf '%s\n' "$$" > "$scratch/pid"
pwd -P > "$scratch/cwd"
printf '%s' "${HORTUS_VAULT-}" > "$scratch/vault"
printf '%s' "${HORTUS_AGENT_SESSION-}" > "$scratch/session"
printf '%s' "${TERM-}" > "$scratch/term"
printf '%s' "${LANG-}" > "$scratch/lang"
index=0
for argument in "$@"; do
  index=$((index + 1))
  printf '%s' "$argument" > "$scratch/arg-$index"
done
printf '%s' "$index" > "$scratch/argc"
sleep_seconds=0
for argument in "$@"; do
  case "$argument" in
    --sleep=*) sleep_seconds=${argument#--sleep=} ;;
  esac
done
if [ "$sleep_seconds" != "0" ]; then
  sleep "$sleep_seconds"
fi
exit "${FAKE_AGENT_EXIT_CODE:-0}"
