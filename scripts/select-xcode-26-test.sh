#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SELECTOR="${ROOT}/scripts/select-xcode-26.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

fail() {
  echo "error: $*" >&2
  exit 1
}

mock_apps="${tmp}/apps"
mock_bin="${tmp}/bin"
mkdir -p "${mock_apps}/Xcode_26.2.app/Contents/Developer" \
  "${mock_apps}/Xcode_26.10.app/Contents/Developer" \
  "${mock_apps}/Xcode_26.99_beta.app/Contents/Developer" \
  "${mock_apps}/Xcode_26.11-beta.1.app/Contents/Developer" \
  "${mock_bin}"

cat >"${mock_bin}/xcodebuild" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "-version" ]] || exit 2
[[ "${DEVELOPER_DIR}" == "${MOCK_EXPECTED_DEVELOPER_DIR}" ]] || exit 3
printf '%s\n' "${MOCK_XCODE_VERSION_OUTPUT}"
SH

cat >"${mock_bin}/swift" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "--version" ]] || exit 2
[[ "${DEVELOPER_DIR}" == "${MOCK_EXPECTED_DEVELOPER_DIR}" ]] || exit 3
printf '%s\n' "${MOCK_SWIFT_VERSION_OUTPUT}"
SH
chmod +x "${mock_bin}/xcodebuild" "${mock_bin}/swift"

github_env="${tmp}/github-env"
expected_developer_dir="${mock_apps}/Xcode_26.10.app/Contents/Developer"
output="$(
  PATH="${mock_bin}:${PATH}" \
    XCODE_APPS_DIR="${mock_apps}" \
    GITHUB_ENV="${github_env}" \
    MOCK_EXPECTED_DEVELOPER_DIR="${expected_developer_dir}" \
    MOCK_XCODE_VERSION_OUTPUT='Xcode 26.10' \
    MOCK_SWIFT_VERSION_OUTPUT='Apple Swift version 6.2.1 (swiftlang-6.2.1.1.1 clang-1700.0.13.3)' \
    "${SELECTOR}"
)" || fail "compatible Xcode 26 / Swift 6.2 should be selected"
[[ "${output}" == *"${mock_apps}/Xcode_26.10.app/Contents/Developer"* ]] \
  || fail "expected highest stable Xcode 26 to be selected, got: ${output}"
grep -Fqx "DEVELOPER_DIR=${expected_developer_dir}" "${github_env}" \
  || fail "the verified developer directory was not written to GITHUB_ENV"

if PATH="${mock_bin}:${PATH}" XCODE_APPS_DIR="${tmp}/missing" "${SELECTOR}" >/dev/null 2>&1; then
  fail "missing Xcode installation should fail"
fi

failure_env="${tmp}/failure-env"
if PATH="${mock_bin}:${PATH}" \
  XCODE_APPS_DIR="${mock_apps}" \
  GITHUB_ENV="${failure_env}" \
  MOCK_EXPECTED_DEVELOPER_DIR="${expected_developer_dir}" \
  MOCK_XCODE_VERSION_OUTPUT='Xcode 27.0' \
  MOCK_SWIFT_VERSION_OUTPUT='Apple Swift version 6.2.1' \
  "${SELECTOR}" >/dev/null 2>&1; then
  fail "a non-26 selected Xcode should fail"
fi
[[ ! -s "${failure_env}" ]] || fail "failed Xcode validation must not update GITHUB_ENV"

if PATH="${mock_bin}:${PATH}" \
  XCODE_APPS_DIR="${mock_apps}" \
  GITHUB_ENV="${failure_env}" \
  MOCK_EXPECTED_DEVELOPER_DIR="${expected_developer_dir}" \
  MOCK_XCODE_VERSION_OUTPUT='Xcode 26.10' \
  MOCK_SWIFT_VERSION_OUTPUT='Apple Swift version 6.3.0' \
  "${SELECTOR}" >/dev/null 2>&1; then
  fail "a selected Xcode without Swift 6.2 should fail"
fi
[[ ! -s "${failure_env}" ]] || fail "failed Swift validation must not update GITHUB_ENV"

echo "select-xcode-26 tests passed."
