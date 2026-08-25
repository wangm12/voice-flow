#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${1:-${ROOT}/.build/release/VoiceFlow.dmg}"

mkdir -p "$(dirname "${DEST}")"

src="$(find "${ROOT}/src-tauri/target" -path '*bundle/dmg/*.dmg' -type f -print 2>/dev/null | sort | tail -1 || true)"
if [[ -z "${src}" || ! -f "${src}" ]]; then
  echo "error: no VoiceFlow DMG found under src-tauri/target" >&2
  exit 1
fi

cp "${src}" "${DEST}"
echo "==> Wrote ${DEST}"
