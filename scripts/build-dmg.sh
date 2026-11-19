#!/usr/bin/env bash
# Package a built VoiceFlow.app bundle into a styled compressed macOS DMG without
# Tauri's external create-dmg wrapper (which leaves Finder windows open and fails
# on hdiutil detach EBUSY / interstitial /Volumes/dmg.* mounts on newer macOS).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VOLNAME="${VOICEFLOW_DMG_VOLNAME:-VoiceFlow}"
BG_IMAGE="${ROOT}/src-tauri/icons/dmg-background.png"
VOL_ICON="${ROOT}/src-tauri/icons/icon.icns"

if [[ "$#" -ne 2 ]]; then
  echo "usage: build-dmg.sh <app-bundle-path> <output-dmg-path>" >&2
  exit 2
fi

APP_PATH="$1"
OUTPUT_DMG="$2"
if [[ ! -d "${APP_PATH}" || "${APP_PATH}" != *.app ]]; then
  echo "error: expected an existing .app bundle: ${APP_PATH}" >&2
  exit 1
fi
if [[ "${OUTPUT_DMG}" != *.dmg ]]; then
  echo "error: output path must end with .dmg: ${OUTPUT_DMG}" >&2
  exit 1
fi

APP_NAME="$(basename "${APP_PATH}")"
OUTPUT_DIR="$(dirname "${OUTPUT_DMG}")"
mkdir -p "${OUTPUT_DIR}"
OUTPUT_DMG="$(cd "${OUTPUT_DIR}" && pwd)/$(basename "${OUTPUT_DMG}")"

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/voiceflow-dmg.XXXXXX")"
staging_dir="${work_dir}/root"
rw_dmg="${work_dir}/rw.VoiceFlow.dmg"
attached_dev=""
applescript_file=""

cleanup() {
  local exit_status=$?
  set +e
  if [[ -n "${applescript_file}" && -f "${applescript_file}" ]]; then
    rm -f "${applescript_file}"
  fi
  if [[ -n "${attached_dev}" ]]; then
    hdiutil detach "${attached_dev}" -force >/dev/null 2>&1 || true
  fi
  rm -rf "${work_dir}"
  exit "${exit_status}"
}
trap cleanup EXIT

detach_dmg() {
  local dev="$1"
  local attempt=0
  local status=0
  while (( attempt < 3 )); do
    attempt=$((attempt + 1))
    if hdiutil detach "${dev}" >/dev/null 2>&1; then
      return 0
    fi
    status=$?
    if [[ "${status}" -ne 16 ]]; then
      break
    fi
    sleep "${attempt}"
  done
  echo "    Retrying DMG detach with -force (${dev})..."
  hdiutil detach "${dev}" -force >/dev/null
}

mkdir -p "${staging_dir}"
ditto "${APP_PATH}" "${staging_dir}/${APP_NAME}"
ln -s /Applications "${staging_dir}/Applications"

if [[ -f "${BG_IMAGE}" ]]; then
  mkdir -p "${staging_dir}/.background"
  cp "${BG_IMAGE}" "${staging_dir}/.background/dmg-background.png"
  chflags hidden "${staging_dir}/.background" || true
fi

if [[ -f "${VOL_ICON}" ]]; then
  cp "${VOL_ICON}" "${staging_dir}/.VolumeIcon.icns"
  chflags hidden "${staging_dir}/.VolumeIcon.icns" || true
  if command -v SetFile >/dev/null 2>&1; then
    SetFile -c icnC -a V "${staging_dir}/.VolumeIcon.icns" || true
  fi
fi

# Prevent Spotlight from indexing the transient read-write DMG and holding it EBUSY.
touch "${staging_dir}/.metadata_never_index"
chflags hidden "${staging_dir}/.metadata_never_index" || true

app_kb="$(du -sk "${staging_dir}" | awk '{print $1}')"
size_mb=$(( (app_kb / 1024) + 64 ))

"${ROOT}/scripts/detach-voiceflow-dmg.sh"

echo "==> Creating writable staging image (${size_mb} MB)"
hdiutil create \
  -quiet \
  -srcfolder "${staging_dir}" \
  -volname "${VOLNAME}" \
  -fs HFS+ \
  -fsargs "-c c=64,a=16,e=16" \
  -format UDRW \
  -size "${size_mb}m" \
  "${rw_dmg}"

attach_output="$(hdiutil attach -readwrite -noverify -noautoopen -nobrowse "${rw_dmg}")"
attached_dev="$(printf '%s\n' "${attach_output}" | awk '/^\/dev\// {print $1; exit}')"
mount_dir="$(printf '%s\n' "${attach_output}" | awk -F'\t' '/\/Volumes\// {print $NF; exit}')"
if [[ -z "${attached_dev}" || -z "${mount_dir}" || ! -d "${mount_dir}" ]]; then
  echo "error: failed to attach temporary DMG (${rw_dmg})" >&2
  exit 1
fi

mdutil -i off "${mount_dir}" >/dev/null 2>&1 || true

if [[ -f "${mount_dir}/.VolumeIcon.icns" ]] && command -v SetFile >/dev/null 2>&1; then
  SetFile -c icnC -a V "${mount_dir}/.VolumeIcon.icns" || true
  SetFile -a C "${mount_dir}" || true
fi
[[ -d "${mount_dir}/.background" ]] && chflags hidden "${mount_dir}/.background" || true

# Apply Finder aesthetics only in interactive local GUI sessions; CI skips AppleScript cleanly.
if [[ -z "${CI:-}" && -z "${GITHUB_ACTIONS:-}" && "${VOICEFLOW_SKIP_DMG_FINDER:-0}" != "1" ]]; then
  disk_name="$(basename "${mount_dir}")"
  applescript_file="$(mktemp "${work_dir}/finder.XXXXXX.applescript")"
  cat >"${applescript_file}" <<EOF
on run argv
  set volName to item 1 of argv
  tell application "Finder"
    tell disk volName
      open
      tell container window
        set current view to icon view
        set toolbar visible to false
        set statusbar visible to false
        set bounds to {160, 120, 820, 520}
      end tell
      set viewOptions to the icon view options of container window
      tell viewOptions
        set arrangement to not arranged
        set icon size to 128
        set text size to 16
      end tell
      try
        set background picture of viewOptions to file ".background:dmg-background.png"
      end try
      set position of item "${APP_NAME}" of container window to {180, 170}
      set position of item "Applications" of container window to {480, 170}
      try
        set the extension hidden of item "${APP_NAME}" to true
      end try
      try
        set position of item ".VolumeIcon.icns" of container window to {1000, 100}
      end try
      try
        set position of item ".background" of container window to {1000, 100}
      end try
      close
      open
      update without registering applications
      delay 1
      close
    end tell
  end tell
end run
EOF
  sleep 1
  if ! /usr/bin/osascript "${applescript_file}" "${disk_name}" >/dev/null 2>&1; then
    echo "warning: Finder DMG window layout skipped (osascript unavailable or denied)" >&2
  fi
fi

chmod -Rf go-w "${mount_dir}" >/dev/null 2>&1 || true
rm -rf "${mount_dir}/.fseventsd" || true
sync

detach_dmg "${attached_dev}"
attached_dev=""

rm -f "${OUTPUT_DMG}"
echo "==> Compressing final DMG: ${OUTPUT_DMG}"
hdiutil convert "${rw_dmg}" -quiet -format UDZO -imagekey zlib-level=9 -ov -o "${OUTPUT_DMG}"
hdiutil verify "${OUTPUT_DMG}" >/dev/null

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "==> Signing DMG with: ${APPLE_SIGNING_IDENTITY}"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY}" "${OUTPUT_DMG}"
  codesign --verify --verbose=2 "${OUTPUT_DMG}" >/dev/null
fi

echo "==> Built DMG: ${OUTPUT_DMG}"
