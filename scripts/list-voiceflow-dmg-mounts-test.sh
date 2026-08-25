#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIST="${ROOT}/scripts/list-voiceflow-dmg-mounts.sh"

fail() {
  echo "error: $*" >&2
  exit 1
}

[[ -x "${LIST}" ]] || fail "missing executable ${LIST}"

read -r -d '' fixture <<'EOF' || true
image-path      : /Users/x/Downloads/v2rayN-macos-arm64.dmg
/dev/disk8s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/v2rayN Installer
image-path      : /Users/x/mac-clippy/dist/MacClippy.dmg
/dev/disk15s1	41504653-0000-11AA-AA11-00306543ECAC	/Volumes/Mac Clippy
/dev/disk17s1	41504653-0000-11AA-AA11-00306543ECAC	/Volumes/Mac Clippy 1
image-path      : /Users/x/voice-flow/.build/release/VoiceFlow.dmg
/dev/disk34s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/VoiceFlow
/dev/disk35s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/VoiceFlow 1
/dev/disk36s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/VoiceFlow 6
/dev/disk99s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/VoiceFlowBackup
EOF

got="$(printf '%s\n' "${fixture}" | "${LIST}")"
expected=$'/Volumes/VoiceFlow\n/Volumes/VoiceFlow 1\n/Volumes/VoiceFlow 6'
[[ "${got}" == "${expected}" ]] || fail "expected VoiceFlow mounts only, got:
${got}"

got="$(printf '%s\n' "${fixture}" | VOICEFLOW_DMG_VOLNAME='Mac Clippy' "${LIST}")"
expected=$'/Volumes/Mac Clippy\n/Volumes/Mac Clippy 1'
[[ "${got}" == "${expected}" ]] || fail "expected Mac Clippy mounts only, got:
${got}"

got="$(printf '%s\n' $'image-path      : /tmp/empty.dmg\n' | "${LIST}")"
[[ -z "${got}" ]] || fail "expected no mounts, got: ${got}"

echo "list-voiceflow-dmg-mounts tests passed."
