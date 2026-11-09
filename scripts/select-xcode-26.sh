#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

xcode_apps_dir="${XCODE_APPS_DIR:-/Applications}"
[[ -d "${xcode_apps_dir}" ]] || fail "Xcode applications directory does not exist: ${xcode_apps_dir}"

xcode_app="$(python3 - "${xcode_apps_dir}" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
pattern = re.compile(r"^Xcode_(26\.\d+(?:\.\d+)?)\.app$")
candidates = []
for app in root.glob("Xcode_26*.app"):
    match = pattern.match(app.name)
    if match and app.is_dir():
        version = tuple(int(part) for part in match.group(1).split("."))
        candidates.append((version, app))

if candidates:
    print(max(candidates, key=lambda item: item[0])[1])
PY
)" || fail "could not inspect installed Xcode 26 versions"
[[ -n "${xcode_app}" ]] || fail "no stable Xcode 26 installation is available in ${xcode_apps_dir}"

developer_dir="${xcode_app}/Contents/Developer"
[[ -d "${developer_dir}" ]] || fail "Xcode developer directory is missing: ${developer_dir}"

export DEVELOPER_DIR="${developer_dir}"

xcode_version="$(xcodebuild -version)" || fail "xcodebuild -version failed"
grep -Eq '^Xcode 26(\.|$)' <<<"${xcode_version}" \
  || fail "selected Xcode is not version 26: ${xcode_version}"

swift_version="$(swift --version)" || fail "swift --version failed"
grep -Eq '(^| )Swift version 6\.2(\.|$)' <<<"${swift_version}" \
  || fail "selected Xcode does not provide Swift 6.2: ${swift_version}"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'DEVELOPER_DIR=%s\n' "${developer_dir}" >>"${GITHUB_ENV}"
fi

printf 'Selected %s\n%s\n%s\n' "${developer_dir}" "${xcode_version}" "${swift_version}"
