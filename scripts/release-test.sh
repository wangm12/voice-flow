#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_SCRIPT="${ROOT}/scripts/release.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

fail() {
  echo "error: $*" >&2
  exit 1
}

mock_bin="${tmp}/bin"
work="${tmp}/work"
mkdir -p "${mock_bin}" "${work}/artifacts"
cat >"${mock_bin}/git" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "check-ref-format" ]]; then exit 0; fi
[[ "$1" == "ls-remote" ]] || exit 2
case "${MOCK_GIT_MODE}" in
  missing)
    if [[ -n "${MOCK_GIT_CREATED_FILE:-}" && -f "${MOCK_GIT_CREATED_FILE}" ]]; then
      printf '%s\trefs/tags/%s\n' "${MOCK_TARGET_SHA:-${GITHUB_SHA}}" "${RELEASE_TAG}"
    fi
    ;;
  lightweight)
    printf '%s\trefs/tags/%s\n' "${MOCK_TARGET_SHA:-${GITHUB_SHA}}" "${RELEASE_TAG}"
    ;;
  annotated)
    printf '%s\trefs/tags/%s\n' '1111111111111111111111111111111111111111' "${RELEASE_TAG}"
    printf '%s\trefs/tags/%s^{}\n' "${MOCK_TARGET_SHA:-${GITHUB_SHA}}" "${RELEASE_TAG}"
    ;;
  mismatch)
    printf '%s\trefs/tags/%s\n' '1111111111111111111111111111111111111111' "${RELEASE_TAG}"
    printf '%s\trefs/tags/%s^{}\n' '2222222222222222222222222222222222222222' "${RELEASE_TAG}"
    ;;
  race-mismatch)
    if [[ -n "${MOCK_GIT_RACE_FILE:-}" && -f "${MOCK_GIT_RACE_FILE}" ]]; then
      printf '%s\trefs/tags/%s\n' '1111111111111111111111111111111111111111' "${RELEASE_TAG}"
      printf '%s\trefs/tags/%s^{}\n' '2222222222222222222222222222222222222222' "${RELEASE_TAG}"
    fi
    ;;
  error)
    echo 'mock remote unavailable' >&2
    exit 128
    ;;
  *)
    echo "unexpected mock git mode: ${MOCK_GIT_MODE}" >&2
    exit 2
    ;;
esac
SH

cat >"${mock_bin}/gh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${MOCK_GH_LOG}"
if [[ "$1" == api ]]; then
  if [[ "$*" == *"/git/refs"* ]]; then
    case "${MOCK_REF_CREATE_STATUS}" in
      201)
        touch "${MOCK_GIT_CREATED_FILE}"
        printf 'HTTP/2 201 Created\n\n{}\n'
        exit 0
        ;;
      422)
        [[ -z "${MOCK_GIT_RACE_FILE:-}" ]] || touch "${MOCK_GIT_RACE_FILE}"
        printf 'HTTP/2 422 Reference already exists\n\n{"message":"Reference already exists"}\n'
        exit 1
        ;;
      500)
        printf 'HTTP/2 500 Server Error\n\n{"message":"Server Error"}\n'
        exit 1
        ;;
      *)
        echo "unexpected mock ref status: ${MOCK_REF_CREATE_STATUS}" >&2
        exit 2
        ;;
    esac
  fi
  case "${MOCK_RELEASE_STATUS}" in
    200)
      printf 'HTTP/2 200 OK\n\n{}\n'
      exit 0
      ;;
    404)
      printf 'HTTP/2 404 Not Found\n\n{"message":"Not Found"}\n'
      exit 1
      ;;
    500)
      printf 'HTTP/2 500 Server Error\n\n{"message":"Server Error"}\n'
      exit 1
      ;;
    *)
      echo "unexpected mock release status: ${MOCK_RELEASE_STATUS}" >&2
      exit 2
      ;;
  esac
fi
[[ "$1" == release ]] || exit 2
SH
chmod +x "${mock_bin}/git" "${mock_bin}/gh"

version="$(cd "${ROOT}" && node -p "require('./package.json').version")"
target_sha='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
tag="v${version}"
log="${tmp}/gh.log"
outputs="${tmp}/outputs"
created_file="${tmp}/created-tag"
race_file="${tmp}/raced-tag"
touch "${work}/release-notes.md" "${work}/artifacts/VoiceFlow-unsigned.dmg"

assert_log_contains() {
  grep -Fq -- "$1" "${log}" || fail "expected gh command log to contain: $1"
}

assert_log_lacks() {
  if grep -Fq -- "$1" "${log}"; then
    fail "did not expect gh command log to contain: $1"
  fi
}

run_prepare() {
  env PATH="${mock_bin}:${PATH}" \
    MOCK_GIT_MODE="${MOCK_GIT_MODE}" \
    GITHUB_EVENT_NAME="${GITHUB_EVENT_NAME}" \
    GITHUB_REF="${GITHUB_REF}" \
    GITHUB_REF_NAME="${GITHUB_REF_NAME}" \
    GITHUB_SHA="${target_sha}" \
    GITHUB_OUTPUT="${outputs}" \
    "${RELEASE_SCRIPT}" prepare
}

run_publish() {
  (
    cd "${work}"
    PATH="${mock_bin}:${PATH}" \
      MOCK_GH_LOG="${log}" \
      MOCK_GIT_MODE="${MOCK_GIT_MODE}" \
      MOCK_GIT_CREATED_FILE="${created_file}" \
      MOCK_GIT_RACE_FILE="${MOCK_GIT_RACE_FILE:-${race_file}}" \
      MOCK_TARGET_SHA="${target_sha}" \
      MOCK_RELEASE_STATUS="${MOCK_RELEASE_STATUS}" \
      MOCK_REF_CREATE_STATUS="${MOCK_REF_CREATE_STATUS:-201}" \
      GITHUB_REPOSITORY='wangm12/voice-flow' \
      RELEASE_TAG="${tag}" \
      RELEASE_TARGET_SHA="${target_sha}" \
      REQUIRE_EXISTING_TAG="${REQUIRE_EXISTING_TAG:-0}" \
      DMG_PATH='.build/release/VoiceFlow-unsigned.dmg' \
      RELEASE_ARTIFACT_DIR='artifacts' \
      RELEASE_NOTES_PATH='release-notes.md' \
      "${RELEASE_SCRIPT}" publish
  )
}

# Preflight compares annotated tags by their peeled commit, and manual releases may
# create a missing version tag at the exact dispatched commit.
: >"${outputs}"
MOCK_GIT_MODE=annotated GITHUB_EVENT_NAME=push \
  GITHUB_REF="refs/tags/${tag}" GITHUB_REF_NAME="${tag}" \
  run_prepare || fail "matching annotated tag push should pass preflight"
grep -Fqx "release_tag=${tag}" "${outputs}" || fail "release tag output is missing"
grep -Fqx "target_sha=${target_sha}" "${outputs}" || fail "target SHA output is missing"
grep -Fqx 'require_existing_tag=1' "${outputs}" || fail "tag event must require an existing tag"
grep -Fqx 'tag_state=existing' "${outputs}" || fail "annotated tag state output is missing"

: >"${outputs}"
MOCK_GIT_MODE=missing GITHUB_EVENT_NAME=workflow_dispatch \
  GITHUB_REF=refs/heads/main GITHUB_REF_NAME=main \
  run_prepare || fail "manual main release with no tag should pass preflight"
grep -Fqx "release_tag=${tag}" "${outputs}" || fail "manual tag output is missing"
grep -Fqx 'require_existing_tag=0' "${outputs}" || fail "manual release should allow a new tag"
grep -Fqx 'tag_state=missing' "${outputs}" || fail "manual missing tag state output is missing"

if MOCK_GIT_MODE=missing GITHUB_EVENT_NAME=workflow_dispatch \
  GITHUB_REF=refs/heads/feature GITHUB_REF_NAME=feature \
  run_prepare >/dev/null 2>&1; then
  fail "manual release from a non-main branch should fail"
fi
if MOCK_GIT_MODE=mismatch GITHUB_EVENT_NAME=push \
  GITHUB_REF="refs/tags/${tag}" GITHUB_REF_NAME="${tag}" \
  run_prepare >/dev/null 2>&1; then
  fail "tag resolving to a different build commit should fail preflight"
fi
if MOCK_GIT_MODE=lightweight GITHUB_EVENT_NAME=push \
  GITHUB_REF=refs/tags/v9.9.9 GITHUB_REF_NAME=v9.9.9 \
  run_prepare >/dev/null 2>&1; then
  fail "tag not matching package.json should fail preflight"
fi

# A missing manual tag is atomically created at the commit just built. The release
# command verifies the ref and cannot silently attach to a concurrently created tag.
: >"${log}"
rm -f "${created_file}" "${race_file}"
MOCK_GIT_MODE=missing MOCK_RELEASE_STATUS=404 MOCK_REF_CREATE_STATUS=201 \
  REQUIRE_EXISTING_TAG=0 run_publish \
  || fail "new release publication should succeed with a missing tag and release"
assert_log_contains "api --include --method POST repos/wangm12/voice-flow/git/refs -f ref=refs/tags/${tag} -f sha=${target_sha}"
assert_log_contains "release create ${tag} artifacts/VoiceFlow-unsigned.dmg"
assert_log_contains "--verify-tag"
assert_log_lacks "--target"
assert_log_lacks "release upload"
assert_log_lacks "--clobber"

# An existing annotated tag is rechecked before upload; duplicate asset handling stays
# gh's default fail behavior because --clobber is never passed.
: >"${log}"
MOCK_GIT_MODE=annotated MOCK_RELEASE_STATUS=200 REQUIRE_EXISTING_TAG=1 run_publish \
  || fail "matching annotated tag should allow publishing to an existing release"
assert_log_contains "release upload ${tag} artifacts/VoiceFlow-unsigned.dmg"
assert_log_contains "release edit ${tag} --notes-file release-notes.md"
assert_log_lacks "release edit ${tag} --notes-file release-notes.md --latest"
assert_log_lacks "--clobber"

# If another publisher creates the missing tag during the atomic create-ref call,
# publication proceeds only when that ref peels to this build commit.
: >"${log}"
rm -f "${created_file}" "${race_file}"
if MOCK_GIT_MODE=race-mismatch MOCK_RELEASE_STATUS=404 MOCK_REF_CREATE_STATUS=422 \
  MOCK_GIT_RACE_FILE="${race_file}" REQUIRE_EXISTING_TAG=0 \
  run_publish >/dev/null 2>&1; then
  fail "concurrently created tag with a different target should block publication"
fi
assert_log_contains "api --include --method POST repos/wangm12/voice-flow/git/refs"
assert_log_lacks "release create"
assert_log_lacks "release upload"

: >"${log}"
if MOCK_GIT_MODE=mismatch MOCK_RELEASE_STATUS=200 REQUIRE_EXISTING_TAG=1 run_publish >/dev/null 2>&1; then
  fail "tag mismatch should block publication"
fi
[[ ! -s "${log}" ]] || fail "tag mismatch should stop before gh is called"

: >"${log}"
if MOCK_GIT_MODE=missing MOCK_RELEASE_STATUS=200 REQUIRE_EXISTING_TAG=0 run_publish >/dev/null 2>&1; then
  fail "existing release without a remote tag should block upload"
fi
assert_log_contains "api --include repos/wangm12/voice-flow/releases/tags/${tag}"
assert_log_lacks "release upload"

: >"${log}"
if MOCK_GIT_MODE=lightweight MOCK_RELEASE_STATUS=500 REQUIRE_EXISTING_TAG=1 run_publish >/dev/null 2>&1; then
  fail "release API failure should block publication"
fi
assert_log_lacks "release create"
assert_log_lacks "release upload"

# The website points to the latest release page, where signed and unsigned asset names
# remain visible instead of assuming one fixed DMG filename exists.
grep -Fq 'https://github.com/wangm12/voice-flow/releases/latest"' "${ROOT}/website/src/content.ts" \
  || fail "website download entry must open the latest release page"
if grep -Fq '/releases/latest/download/VoiceFlow.dmg' "${ROOT}/website/src/content.ts"; then
  fail "website must not assume the signed DMG exists for every latest release"
fi

echo "release workflow helper tests passed."
