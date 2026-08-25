#!/usr/bin/env bash
# Unmount leftover VoiceFlow installer volumes so `make dmg` can reuse
# /Volumes/VoiceFlow. Opening the staged DMG in Finder leaves it attached;
# the next create-dmg run then fails inside bundle_dmg.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIST="${ROOT}/scripts/list-voiceflow-dmg-mounts.sh"

if [[ ! -x "${LIST}" ]]; then
  echo "error: missing executable ${LIST}" >&2
  exit 1
fi

found=0
while IFS= read -r mount; do
  [[ -n "${mount}" ]] || continue
  if [[ "${found}" -eq 0 ]]; then
    echo "==> Detaching leftover VoiceFlow DMG mounts"
  fi
  found=1
  echo "    ${mount}"
  hdiutil detach "${mount}" -force >/dev/null
done < <(hdiutil info | "${LIST}")
