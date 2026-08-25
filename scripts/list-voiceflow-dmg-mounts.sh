#!/usr/bin/env bash
# Read `hdiutil info` on stdin and print VoiceFlow installer mount points.
# Finder names duplicates "VoiceFlow 1", "VoiceFlow 2", … when the volume
# is already attached. create-dmg / bundle_dmg.sh fails if those exist.
set -euo pipefail

VOLNAME="${VOICEFLOW_DMG_VOLNAME:-VoiceFlow}"
prefix="/Volumes/${VOLNAME}"

while IFS= read -r line; do
  [[ "${line}" == /dev/* ]] || continue
  mount="${line##*$'\t'}"
  [[ "${mount}" == /Volumes/* ]] || continue
  if [[ "${mount}" == "${prefix}" || "${mount}" =~ ^${prefix}\ [0-9]+$ ]]; then
    printf '%s\n' "${mount}"
  fi
done
