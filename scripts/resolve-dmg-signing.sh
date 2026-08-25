#!/usr/bin/env bash
# Print APPLE_SIGNING_IDENTITY and APPLE_TEAM_ID for make dmg.
# Locally, creates the project self-signed identity when it is missing so
# DMGs are not tied to a personal Apple Development certificate.
# On GitHub Actions without imported secrets, stay unsigned so each release
# does not mint a new certificate and silently reset TCC.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SELF_SIGN_IDENTITY="${VOICEFLOW_SELF_SIGN_IDENTITY:-VoiceFlow}"
SELF_SIGN_TEAM="${VOICEFLOW_SELF_SIGN_TEAM:-VOICEFLOW1}"

print_env() {
  printf 'APPLE_SIGNING_IDENTITY=%q\n' "$1"
  printf 'APPLE_TEAM_ID=%q\n' "$2"
}

extract_team() {
  local identity="$1"
  if [[ "${identity}" =~ \(([A-Z0-9]{10})\)$ ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
    return
  fi
  security find-certificate -c "${identity}" -p 2>/dev/null \
    | openssl x509 -noout -subject 2>/dev/null \
    | sed -n 's/.*OU[ =]*\([[:alnum:]]\{10\}\).*/\1/p' \
    | head -1
}

identity="${APPLE_SIGNING_IDENTITY:-${DEVELOPER_IDENTITY:-}}"
if [[ -z "${identity}" ]]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null | "${ROOT}/scripts/select-codesign-identity.sh" || true)"
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-${DEVELOPER_IDENTITY:-}}" && "${identity}" != "${SELF_SIGN_IDENTITY}" ]]; then
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    if [[ -z "${identity}" ]]; then
      echo "warning: no stable signing identity on the runner; building unsigned DMG" >&2
      echo "         add MACOS_CERT_P12 and MACOS_CERT_PASSWORD from scripts/make-signing-cert.sh" >&2
      exit 0
    fi
  else
    echo "==> Creating ${SELF_SIGN_IDENTITY} so local DMGs match GitHub releases" >&2
    "${ROOT}/scripts/make-signing-cert.sh" >&2
    identity="$(security find-identity -v -p codesigning 2>/dev/null | "${ROOT}/scripts/select-codesign-identity.sh" || true)"
  fi
fi

if [[ -z "${identity}" ]]; then
  echo "warning: could not resolve a signing identity; building unsigned DMG" >&2
  exit 0
fi

team="${APPLE_TEAM_ID:-${DEVELOPMENT_TEAM:-$(extract_team "${identity}")}}"
if [[ -z "${team}" && "${identity}" == "${SELF_SIGN_IDENTITY}" ]]; then
  team="${SELF_SIGN_TEAM}"
fi
if [[ -z "${team}" ]]; then
  echo "warning: signing identity found but Team ID could not be determined; building unsigned DMG" >&2
  exit 0
fi

print_env "${identity}" "${team}"
