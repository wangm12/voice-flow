#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
release_target="${VOICEFLOW_RELEASE_TARGET:-aarch64-apple-darwin}"
case "$release_target" in
  aarch64-apple-darwin)
    rust_targets=(aarch64-apple-darwin)
    expected_architectures=(arm64)
    requires_apple_silicon_resources=1
    ;;
  universal-apple-darwin)
    rust_targets=(aarch64-apple-darwin x86_64-apple-darwin)
    expected_architectures=(arm64 x86_64)
    requires_apple_silicon_resources=1
    ;;
  *)
    echo "Unsupported VOICEFLOW_RELEASE_TARGET: ${release_target}. Choose aarch64-apple-darwin or universal-apple-darwin." >&2
    exit 2
    ;;
esac
default_destination="${ROOT}/.build/release/VoiceFlow-notarized.dmg"
DEST="${1:-$default_destination}"
if [[ "$#" -gt 1 ]]; then
  echo "usage: release-macos.sh [destination.dmg]" >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS is required for Developer ID signing and notarization." >&2
  exit 1
fi
if [[ "${VOICEFLOW_NOTARIZE:-}" != "1" ]]; then
  echo "VOICEFLOW_NOTARIZE=1 is required; use make dmg for private signed builds." >&2
  exit 2
fi

signing_identity="${APPLE_SIGNING_IDENTITY:-}"
team_id="${APPLE_TEAM_ID:-}"
apple_id="${APPLE_ID:-}"
apple_password="${APPLE_PASSWORD:-}"
certificate="${APPLE_CERTIFICATE:-}"
certificate_password="${APPLE_CERTIFICATE_PASSWORD:-}"
if [[ -z "$signing_identity" || "$signing_identity" != "Developer ID Application:"* ]]; then
  echo "A Developer ID Application identity is required for notarization." >&2
  exit 2
fi
if [[ -z "$team_id" || "$signing_identity" != *"(${team_id})" ]]; then
  echo "APPLE_TEAM_ID must match the Developer ID Application identity." >&2
  exit 2
fi
if [[ -z "$apple_id" || -z "$apple_password" ]]; then
  echo "APPLE_ID and APPLE_PASSWORD are required for notarization." >&2
  exit 2
fi
if [[ -n "$certificate" && -z "$certificate_password" ]] \
  || [[ -z "$certificate" && -n "$certificate_password" ]]; then
  echo "APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD must be supplied together." >&2
  exit 2
fi

destination_parent="$(dirname "$DEST")"
mkdir -p "$destination_parent"
destination_parent="$(cd "$destination_parent" && pwd -P)"
DEST="${destination_parent}/$(basename "$DEST")"
RECEIPT="${DEST%.dmg}.receipt.json"
# Clear only the notarized output pair after validating the release request.
rm -f "$DEST" "$RECEIPT"
work_dir="$(mktemp -d "${destination_parent}/.voiceflow-notary.XXXXXX")"
mountpoint="${work_dir}/mount"
staging_dir="${work_dir}/image-root"
candidate_dmg="${work_dir}/VoiceFlow-notarized.dmg"
app_zip="${work_dir}/VoiceFlow.zip"
receipt_tmp="${work_dir}/receipt.json"
temporary_keychain="${work_dir}/signing.keychain-db"
temporary_p12="${work_dir}/identity.p12"
temporary_keychain_attempted=0
temporary_keychain_created=0
search_list_captured=0
original_search_list=("")
if [[ -n "$certificate" ]]; then
  if original_search_list_text="$(security list-keychains -d user)"; then
    :
  else
    snapshot_status=$?
    echo "Could not capture the original user keychain search list." >&2
    rm -rf "$work_dir" || :
    exit "$snapshot_status"
  fi
  original_search_list=("")
  while IFS= read -r keychain_line || [[ -n "$keychain_line" ]]; do
    keychain_line="${keychain_line#${keychain_line%%[![:space:]]*}}"
    keychain_line="${keychain_line%${keychain_line##*[![:space:]]}}"
    keychain_line="${keychain_line#\"}"
    keychain_line="${keychain_line%\"}"
    [[ -n "$keychain_line" ]] && original_search_list+=("$keychain_line")
  done <<< "$original_search_list_text"
  search_list_captured=1
fi
mkdir -p "$mountpoint" "$staging_dir"
mounted=0
published=0
cleanup() {
  local exit_status=$?
  local cleanup_failed=0
  local preserve_work_dir=0
  trap - EXIT INT TERM
  if [[ "$mounted" == "1" ]]; then
    if ! hdiutil detach "$mountpoint" -quiet >/dev/null 2>&1; then
      echo "error: could not detach verification volume; temporary files are preserved at $work_dir" >&2
      cleanup_failed=1
      preserve_work_dir=1
    else
      mounted=0
    fi
  fi
  if [[ -f "$temporary_p12" ]] && ! rm -f "$temporary_p12"; then
    echo "error: could not remove temporary signing certificate data." >&2
    cleanup_failed=1
  fi
  if [[ "$temporary_keychain_attempted" == "1" ]] && { [[ "$temporary_keychain_created" == "1" ]] || [[ -e "$temporary_keychain" ]]; }; then
    if ! security delete-keychain "$temporary_keychain"; then
      echo "error: could not delete the temporary signing keychain." >&2
      cleanup_failed=1
    fi
  fi
  if [[ "$temporary_keychain_attempted" == "1" && "$search_list_captured" == "1" ]] && ! security list-keychains -d user -s "${original_search_list[@]:1}"; then
    echo "error: could not restore the original user keychain search list." >&2
    cleanup_failed=1
  fi
  if [[ "$cleanup_failed" == "1" ]]; then
    [[ "$exit_status" -ne 0 ]] || exit_status=1
  fi
  if [[ "$published" != "1" || "$exit_status" -ne 0 ]]; then
    if ! rm -f "$DEST" "$RECEIPT"; then
      echo "error: could not remove the notarized output pair." >&2
      cleanup_failed=1
      [[ "$exit_status" -ne 0 ]] || exit_status=1
    fi
  fi
  if [[ "$preserve_work_dir" != "1" ]] && ! rm -rf "$work_dir"; then
    echo "error: could not remove temporary verification data at $work_dir" >&2
    cleanup_failed=1
    [[ "$exit_status" -ne 0 ]] || exit_status=1
    if ! rm -f "$DEST" "$RECEIPT"; then
      echo "error: could not remove the notarized output pair." >&2
    fi
  fi
  if [[ "$published" == "1" && "$exit_status" -eq 0 && "$cleanup_failed" == "0" ]]; then
    echo "Notarized and verified ${release_target} DMG: $DEST"
  fi
  exit "$exit_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -n "$certificate" ]]; then
  printf '%s' "$certificate" | base64 --decode > "$temporary_p12"
  keychain_password="$(openssl rand -hex 20)"
  temporary_keychain_attempted=1
  if security create-keychain -p "$keychain_password" "$temporary_keychain"; then
    temporary_keychain_created=1
  else
    create_status=$?
    echo "Could not create the temporary signing keychain." >&2
    exit "$create_status"
  fi
  security set-keychain-settings "$temporary_keychain"
  security unlock-keychain -p "$keychain_password" "$temporary_keychain"
  security import "$temporary_p12" -k "$temporary_keychain" -P "$certificate_password" -T /usr/bin/codesign
  rm -f "$temporary_p12"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$temporary_keychain"
  security list-keychains -d user -s "$temporary_keychain" "${original_search_list[@]:1}"
fi

target_root="${ROOT}/src-tauri/target/${release_target}/release"
bundle_root="${target_root}/bundle"
app_dir="${bundle_root}/macos"
rm -rf "$app_dir" "${bundle_root}/dmg"

"${ROOT}/scripts/detach-voiceflow-dmg.sh"
rustup target add "${rust_targets[@]}"
npm ci
npm run build

# Build only the signed app; Tauri must not create a pre-staple disk image.
(
  unset APPLE_ID APPLE_PASSWORD APPLE_API_KEY APPLE_API_KEY_PATH
  unset APPLE_API_ISSUER APPLE_API_KEY_ID APPLE_API_ISSUER_ID
  export APPLE_SIGNING_IDENTITY="$signing_identity"
  export APPLE_TEAM_ID="$team_id"
  unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
  npm run tauri -- build --target "$release_target" --bundles app
)

shopt -s nullglob
apps=("$app_dir"/*.app)
if [[ "${#apps[@]}" -ne 1 ]]; then
  echo "Expected exactly one current app for ${release_target}." >&2
  exit 1
fi
bundle="${apps[0]}"
python3 "${ROOT}/scripts/verify-dependency-texts.py" \
  "${ROOT}/ThirdPartyLicenses/DependencyTexts" \
  "${bundle}/Contents/Resources/ThirdPartyLicenses/DependencyTexts"

verify_developer_id_app() {
  local app="$1"
  local signature_info
  if ! codesign --verify --deep --strict --verbose=2 "$app" >/dev/null 2>&1; then
    echo "The notarization app signature is invalid." >&2
    return 1
  fi
  if ! signature_info="$(codesign -dv --verbose=4 "$app" 2>&1)"; then
    echo "Could not inspect the notarization app signature." >&2
    return 1
  fi
  if ! printf '%s\n' "$signature_info" | grep -Fqx "Authority=${signing_identity}" \
    || ! printf '%s\n' "$signature_info" | grep -Fqx "TeamIdentifier=${team_id}"; then
    echo "The app is not signed by the requested Developer ID team." >&2
    return 1
  fi
  local executables=("$app"/Contents/MacOS/*)
  local executable=""
  local count=0 candidate
  for candidate in "${executables[@]}"; do
    if [[ -f "$candidate" && -x "$candidate" ]]; then
      executable="$candidate"
      count=$((count + 1))
    fi
  done
  if [[ "$count" -ne 1 ]]; then
    echo "Expected exactly one executable in the app bundle." >&2
    return 1
  fi
  local architectures
  local -a actual_architectures=()
  if ! architectures="$(lipo -archs "$executable" 2>/dev/null)"; then
    echo "Could not inspect the notarized app architectures." >&2
    return 1
  fi
  read -r -a actual_architectures <<< "$architectures"
  if [[ "${#actual_architectures[@]}" -ne "${#expected_architectures[@]}" ]]; then
    echo "The notarized app must contain exactly: ${expected_architectures[*]}." >&2
    return 1
  fi
  local expected_architecture actual_architecture count
  for expected_architecture in "${expected_architectures[@]}"; do
    count=0
    for actual_architecture in "${actual_architectures[@]}"; do
      [[ "$actual_architecture" == "$expected_architecture" ]] && count=$((count + 1))
    done
    if [[ "$count" -ne 1 ]]; then
      echo "The notarized app must contain exactly: ${expected_architectures[*]}." >&2
      return 1
    fi
  done
}

verify_production_entry_and_native_strings() {
  local app="$1" scratch="$2"
  local executables=("$app"/Contents/MacOS/*)
  local main="" candidate status output
  local count=0
  local -a native_binaries=()

  for candidate in "${executables[@]}"; do
    if [[ -f "$candidate" && -x "$candidate" ]]; then
      main="$candidate"
      count=$((count + 1))
    fi
  done
  if [[ "$count" -ne 1 ]]; then
    echo "Production-entry checks require exactly one executable in the app bundle." >&2
    return 1
  fi
  local sidecar="$app/Contents/Resources/voiceflow-mlx-sidecar"
  local swift_runtime="$app/Contents/Resources/libswiftCompatibilitySpan.dylib"
  local llama_server="$app/Contents/Resources/llama-runtime/bin/llama-server"
  local llama_identity="$app/Contents/Resources/llama-runtime/BUILD-IDENTITY.json"
  for candidate in "$main" "$sidecar" "$swift_runtime" "$llama_server" "$llama_identity"; do
    if [[ ! -f "$candidate" || ! -r "$candidate" ]]; then
      echo "Required production native binary is missing or unreadable: $candidate" >&2
      return 1
    fi
  done

  mkdir -p "$scratch"
  local -a probe_args=(
    "--import-test-groq-key-release-probe"
    "--test-groq-key-status-release-probe"
    "--research-stdio"
  )
  local -a expected_diagnostics=(
    "test_groq_key_cli_requires_research_harness_feature"
    "test_groq_key_cli_requires_research_harness_feature"
    "research_stdio_requires_macos_research_harness_feature"
  )
  local -a expected_statuses=(2 2 0)
  local probe_index=0
  for output in "${probe_args[@]}"; do
    probe_index=$((probe_index + 1))
    local stdout_file="$scratch/probe-${probe_index}.stdout"
    local stderr_file="$scratch/probe-${probe_index}.stderr"
    if "$main" "$output" </dev/null >"$stdout_file" 2>"$stderr_file"; then
      status=0
    else
      status=$?
    fi
    if [[ "$status" -ne "${expected_statuses[$((probe_index - 1))]}" ]] \
      || [[ -s "$stdout_file" ]] \
      || [[ "$(<"$stderr_file")" != "${expected_diagnostics[$((probe_index - 1))]}" ]]; then
      echo "Production CLI release probe returned an unexpected result ($output, status $status)." >&2
      return 1
    fi
  done

  local owned_wav="$scratch/nonexistent-release-probe.wav"
  local owned_stdout="$scratch/owned-wav.stdout"
  local owned_stderr="$scratch/owned-wav.stderr"
  if [[ -e "$owned_wav" ]]; then
    echo "Owned WAV release probe path unexpectedly exists: $owned_wav" >&2
    return 1
  fi
  if "$main" --research-owned-wav "$owned_wav" --target-nonce \
    00000000-0000-4000-8000-000000000001 </dev/null >"$owned_stdout" 2>"$owned_stderr"; then
    status=0
  else
    status=$?
  fi
  if [[ "$status" -ne 0 ]] || [[ -s "$owned_stdout" ]] \
    || [[ "$(<"$owned_stderr")" != "owned_wav_replay_status=unsupported_build" ]]; then
    echo "Production owned-WAV release probe returned an unexpected result (status $status)." >&2
    return 1
  fi

  local files_file="$scratch/packaged-files.list"
  local strings_file="$scratch/native-strings.txt"
  : >"$strings_file"
  if ! find "$app/Contents" -type f -print0 >"$files_file"; then
    echo "Could not enumerate packaged app files for the native binary scan." >&2
    return 1
  fi
  while IFS= read -r -d '' candidate; do
    local description
    if ! description="$(file -E -b "$candidate")"; then
      echo "Could not classify packaged app file: $candidate" >&2
      return 1
    fi
    if [[ "$description" == *"Mach-O"* ]]; then
      native_binaries+=("$candidate")
    fi
  done <"$files_file"
  for candidate in "$main" "$sidecar" "$swift_runtime" "$llama_server"; do
    local found=0 binary
    for binary in "${native_binaries[@]}"; do
      [[ "$binary" == "$candidate" ]] && found=1
    done
    if [[ "$found" -ne 1 ]]; then
      echo "Required production binary was not included in the Mach-O inventory: $candidate" >&2
      return 1
    fi
  done
  if [[ "${#native_binaries[@]}" -eq 0 ]]; then
    echo "No packaged Mach-O binaries were found to scan." >&2
    return 1
  fi
  for candidate in "${native_binaries[@]}"; do
    if [[ ! -r "$candidate" ]]; then
      echo "Packaged Mach-O binary is unreadable: $candidate" >&2
      return 1
    fi
    if ! strings -a "$candidate" >>"$strings_file"; then
      echo "Could not extract strings from packaged Mach-O binary: $candidate" >&2
      return 1
    fi
  done
  if rg -n -F '/Users/' "$strings_file"; then
    echo "Packaged native binaries contain a developer-home path." >&2
    return 1
  else
    status=$?
    if [[ "$status" -ne 1 ]]; then
      echo "Could not complete the packaged native string path scan (rg status $status)." >&2
      return 1
    fi
  fi
}

verify_packaged_provenance() {
  local app="$1"
  python3 - "$ROOT" "$app" <<'PY'
import hashlib
import sys
from pathlib import Path

root, app = map(Path, sys.argv[1:])
resources = app / "Contents/Resources"

def require_file(path):
    if not path.is_file():
        raise SystemExit(f"Required packaged provenance file is missing: {path}")
    return path.read_bytes()

def sha256(data):
    return hashlib.sha256(data).hexdigest()

def compare_file(source, packaged, label):
    source_bytes = require_file(source)
    packaged_bytes = require_file(packaged)
    if source_bytes != packaged_bytes:
        raise SystemExit(f"Packaged {label} differs from its repository source.")
    return sha256(source_bytes)

manifest = resources / "ThirdPartyLicenses/DependencyManifest"
identities = require_file(manifest / "dependency-identities.txt").decode("utf-8")
lock_pairs = (
    (root / "src-tauri/mlx-sidecar/Package.resolved", manifest / "Package.resolved",
     "Sidecar Package.resolved SHA-256:"),
    (root / "src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved",
     manifest / "Xcode-Package.resolved", "Xcode Package.resolved SHA-256:"),
)
for source, packaged, declaration in lock_pairs:
    digest = compare_file(source, packaged, declaration)
    matches = [line for line in identities.splitlines() if line.startswith(declaration)]
    if matches != [declaration + " " + digest]:
        raise SystemExit(f"Dependency identity manifest has a missing, malformed, duplicate, or incorrect {declaration} declaration.")

compare_file(root / "THIRD_PARTY_NOTICES.md", resources / "THIRD_PARTY_NOTICES.md",
             "third-party notice")

def tree(root_dir, label):
    if not root_dir.is_dir():
        raise SystemExit(f"Required {label} directory is missing: {root_dir}")
    result = {}
    for path in root_dir.rglob("*"):
        if path.is_symlink():
            raise SystemExit(f"Unexpected symlink in {label}: {path}")
        if path.is_file():
            result[path.relative_to(root_dir).as_posix()] = sha256(path.read_bytes())
    if not result:
        raise SystemExit(f"Required {label} directory is empty: {root_dir}")
    return result

for name in ("Models", "Upstream"):
    source = tree(root / "ThirdPartyLicenses" / name, f"source {name}")
    packaged = tree(resources / "ThirdPartyLicenses" / name, f"packaged {name}")
    if source != packaged:
        raise SystemExit(f"Packaged {name} file paths or contents differ from repository sources.")
PY
  python3 "${ROOT}/scripts/verify-dependency-texts.py" \
    "${ROOT}/ThirdPartyLicenses/DependencyTexts" \
    "${app}/Contents/Resources/ThirdPartyLicenses/DependencyTexts"
}

verify_sidecar_hello() {
  local mode="$1" sidecar="$2" models_root="$3" audio_root="$4"
  python3 - "$sidecar" "$mode" "$models_root" "$audio_root" <<'PY'
import json
import subprocess
import sys

sidecar, mode, models_root, audio_root = sys.argv[1:]
args = [sidecar, "--models-root", models_root, "--mode", mode]
if mode == "asr":
    args += ["--audio-root", audio_root]
request_id = "release-asr" if mode == "asr" else "release-cleanup"
request = {"protocol_version": 1, "request_id": request_id,
           "session_id": None, "model_version": None, "op": "hello"}
process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
try:
    stdout, stderr = process.communicate(json.dumps(request, separators=(",", ":")) + "\n", timeout=8)
except subprocess.TimeoutExpired:
    process.kill()
    process.communicate()
    raise SystemExit(f"Bundled MLX {mode} sidecar hello exceeded the 8-second limit.")
if process.returncode != 0:
    raise SystemExit(f"Bundled MLX {mode} sidecar hello exited with status {process.returncode}.")
lines = stdout.splitlines()
if len(lines) != 1 or not lines[0]:
    raise SystemExit(f"Bundled MLX {mode} sidecar did not return exactly one JSONL hello response.")
try:
    response = json.loads(lines[0])
except json.JSONDecodeError as error:
    raise SystemExit(f"Bundled MLX {mode} sidecar returned invalid JSON: {error}")
if not isinstance(response, dict):
    raise SystemExit(f"Bundled MLX {mode} sidecar hello response is not an object.")
required = {"protocol_version", "request_id", "session_id", "model_version", "op",
            "status", "platform_supported", "runtime_version", "loaded_model_id",
            "loaded_model_version", "error"}
if not required.issubset(response):
    raise SystemExit(f"Bundled MLX {mode} sidecar hello response is missing required fields.")
for key, expected in (("protocol_version", 1), ("request_id", request_id),
                      ("session_id", None), ("model_version", None), ("op", "hello")):
    if type(response.get(key)) is not type(expected) or response.get(key) != expected:
        raise SystemExit(f"Bundled MLX {mode} sidecar hello response has an invalid {key}.")
if not isinstance(response.get("runtime_version"), str) or not response["runtime_version"].strip():
    raise SystemExit(f"Bundled MLX {mode} sidecar hello response has no runtime version.")
for key in ("loaded_model_id", "loaded_model_version", "loaded_attestation"):
    if response.get(key) is not None:
        raise SystemExit(f"Bundled MLX {mode} sidecar hello unexpectedly reports loaded model state.")
supported = response.get("platform_supported")
if type(supported) is not bool:
    raise SystemExit(f"Bundled MLX {mode} sidecar hello has an invalid platform_supported value.")
if not isinstance(response.get("status"), str):
    raise SystemExit(f"Bundled MLX {mode} sidecar hello has an invalid status.")
error = response.get("error")
if supported is True:
    if response.get("status") != "ready" or error is not None:
        raise SystemExit(f"Bundled MLX {mode} sidecar did not report a ready supported platform.")
elif response.get("status") != "unavailable" or not isinstance(error, dict) \
        or error.get("code") != "unsupported_platform":
    raise SystemExit(f"Bundled MLX {mode} sidecar did not report the exact unsupported-platform state.")
PY
}

verify_developer_id_app "$bundle"
verify_production_entry_and_native_strings "$bundle" "${work_dir}/production-entry-prezip"
if [[ "$requires_apple_silicon_resources" == "1" ]]; then
  sidecar="$bundle/Contents/Resources/voiceflow-mlx-sidecar"
  llama_server="$bundle/Contents/Resources/llama-runtime/bin/llama-server"
  llama_identity="$bundle/Contents/Resources/llama-runtime/BUILD-IDENTITY.json"
  colocated_metallib="$bundle/Contents/Resources/mlx.metallib"
  metallib="$bundle/Contents/Resources/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
  if [[ ! -x "$sidecar" || ! -s "$colocated_metallib" || ! -s "$metallib" ]] \
    || ! cmp -s "$colocated_metallib" "$metallib"; then
    echo "The Apple silicon app is missing its signed MLX sidecar or Metal resources." >&2
    exit 1
  fi
  codesign --verify --strict --verbose=2 "$sidecar" >/dev/null 2>&1 || {
    echo "The Apple silicon MLX sidecar signature is invalid." >&2
    exit 1
  }
  python3 "$ROOT/src-tauri/llama-runtime/scripts/verify-runtime-identity.py" \
    --runtime-dir "$bundle/Contents/Resources/llama-runtime" \
    --expected-signing-mode developer_id \
    --expected-authority "$signing_identity" \
    --expected-team-id "$team_id"
  models_root="${work_dir}/empty-models"
  audio_root="${work_dir}/empty-audio"
  mkdir -p "$models_root" "$audio_root"
  verify_sidecar_hello asr "$sidecar" "$models_root" "$audio_root"
  verify_sidecar_hello cleanup "$sidecar" "$models_root" "$audio_root"
fi
verify_packaged_provenance "$bundle"

app_zip_sha=""
app_submission_id=""
dmg_submission_id=""
dmg_submission_sha=""
submit_accepted() {
  local artifact="$1" label="$2" result parsed
  if ! result="$(xcrun notarytool submit "$artifact" \
    --apple-id "$apple_id" --password "$apple_password" --team-id "$team_id" \
    --wait --output-format json 2>&1)"; then
    echo "${label} notary submission failed." >&2
    return 1
  fi
  if ! parsed="$(printf '%s' "$result" | python3 -c \
    'import json,sys; d=json.load(sys.stdin); i=d.get("id"); s=d.get("status"); assert isinstance(i,str) and i.strip(); assert s == "Accepted"; print(i.strip())' 2>/dev/null)"; then
    echo "${label} notary submission must return a nonempty id and Accepted status." >&2
    return 1
  fi
  printf '%s' "$parsed"
}
sha256_file() {
  python3 - "$1" <<'PY'
import hashlib, sys
digest = hashlib.sha256()
with open(sys.argv[1], "rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
print(digest.hexdigest())
PY
}

# Notarize and attach the ticket to this exact app before image construction.
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$app_zip"
app_zip_sha="$(sha256_file "$app_zip")"
app_submission_id="$(submit_accepted "$app_zip" "App")"
xcrun stapler staple "$bundle" >/dev/null
xcrun stapler validate "$bundle" >/dev/null

# Construct a fresh image from the already-stapled app, then notarize that image.
cp -R "$bundle" "$staging_dir/"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -ov -format UDZO -volname "VoiceFlow" -srcfolder "$staging_dir" "$candidate_dmg" >/dev/null
if [[ -n "$certificate" ]]; then
  codesign --force --sign "$signing_identity" --keychain "$temporary_keychain" --timestamp "$candidate_dmg"
else
  codesign --force --sign "$signing_identity" --timestamp "$candidate_dmg"
fi
codesign --verify --verbose=2 "$candidate_dmg" >/dev/null 2>&1 || {
  echo "The rebuilt DMG signature is invalid." >&2
  exit 1
}
hdiutil verify "$candidate_dmg" >/dev/null
dmg_submission_sha="$(sha256_file "$candidate_dmg")"
dmg_submission_id="$(submit_accepted "$candidate_dmg" "DMG")"
xcrun stapler staple "$candidate_dmg" >/dev/null
xcrun stapler validate "$candidate_dmg" >/dev/null
hdiutil verify "$candidate_dmg" >/dev/null
hdiutil attach -nobrowse -readonly -mountpoint "$mountpoint" "$candidate_dmg" >/dev/null
mounted=1
mounted_apps=("$mountpoint"/*.app)
if [[ "${#mounted_apps[@]}" -ne 1 ]]; then
  echo "The final DMG must contain exactly one app bundle at its root." >&2
  exit 1
fi
verify_developer_id_app "${mounted_apps[0]}"
verify_production_entry_and_native_strings "${mounted_apps[0]}" "${work_dir}/production-entry-mounted"
verify_packaged_provenance "${mounted_apps[0]}"
xcrun stapler validate "${mounted_apps[0]}" >/dev/null
spctl --assess --type execute --verbose=4 "${mounted_apps[0]}" >/dev/null
hdiutil detach "$mountpoint" -quiet >/dev/null
mounted=0

final_dmg_sha="$(sha256_file "$candidate_dmg")"
python3 - "$receipt_tmp" "$release_target" "$app_submission_id" "$app_zip_sha" \
  "$dmg_submission_id" "$dmg_submission_sha" "$final_dmg_sha" <<'PY'
import json, sys
path, target, app_id, app_sha, dmg_id, dmg_sha, final_sha = sys.argv[1:]
receipt = {
    "target": target,
    "app": {"submission_id": app_id, "status": "Accepted", "zip_sha256": app_sha},
    "dmg": {"submission_id": dmg_id, "status": "Accepted", "submitted_sha256": dmg_sha},
    "final_dmg_sha256": final_sha,
}
with open(path, "w", encoding="utf-8") as f:
    json.dump(receipt, f, indent=2)
    f.write("\n")
PY
mv -f "$candidate_dmg" "$DEST"
mv -f "$receipt_tmp" "$RECEIPT"
published=1
