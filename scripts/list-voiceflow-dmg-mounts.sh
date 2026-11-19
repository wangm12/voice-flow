#!/usr/bin/env bash
# Read `hdiutil info` on stdin and print VoiceFlow installer mount points.
# Finder names duplicates "VoiceFlow 1", "VoiceFlow 2", … when the volume
# is already attached. create-dmg / bundle_dmg.sh fails if those exist.
set -euo pipefail

VOLNAME="${VOICEFLOW_DMG_VOLNAME:-VoiceFlow}"
prefix="/Volumes/${VOLNAME}"
image_base=""

while IFS= read -r line; do
  if [[ "${line}" == ====* ]]; then
    image_base=""
    continue
  fi
  if [[ "${line}" == image-path*:* ]]; then
    image_path="${line#*: }"
    image_base="${image_path##*/}"
    continue
  fi
  [[ "${line}" == /dev/* ]] || continue
  mount="${line##*$'\t'}"
  [[ "${mount}" == /Volumes/* ]] || continue
  if [[ "${mount}" == "${prefix}" || "${mount}" =~ ^${prefix}\ [0-9]+$ ]]; then
    printf '%s\n' "${mount}"
  elif [[ "${mount}" == /Volumes/dmg.* && -n "${image_base}" ]] && [[ "${image_base}" == rw.*."${VOLNAME}"*.dmg || "${image_base}" == "${VOLNAME}"_*.dmg || "${image_base}" == "${VOLNAME}".dmg || "${image_base}" == "${VOLNAME}"-*.dmg ]]; then
    printf '%s\n' "${mount}"
  fi
done
