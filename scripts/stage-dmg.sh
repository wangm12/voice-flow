#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$#" -lt 1 || "$#" -gt 2 ]]; then
  echo "usage: stage-dmg.sh <exact-dmg-or-bundle-dmg-directory> [destination]" >&2
  exit 2
fi

INPUT="$1"
DEST="${2:-${ROOT}/.build/release/VoiceFlow.dmg}"
if [[ -d "$INPUT" ]]; then
  shopt -s nullglob
  candidates=("$INPUT"/*.dmg)
  if [[ "${#candidates[@]}" -ne 1 ]]; then
    echo "error: expected exactly one DMG in the explicit build directory" >&2
    exit 1
  fi
  SOURCE="${candidates[0]}"
elif [[ -f "$INPUT" && "$INPUT" == *.dmg ]]; then
  SOURCE="$INPUT"
else
  echo "error: the explicit DMG source is missing or invalid" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
if [[ "$(cd "$(dirname "$SOURCE")" && pwd)/$(basename "$SOURCE")" != \
      "$(cd "$(dirname "$DEST")" && pwd)/$(basename "$DEST")" ]]; then
  cp -f "$SOURCE" "$DEST"
fi
echo "==> Wrote ${DEST}"
