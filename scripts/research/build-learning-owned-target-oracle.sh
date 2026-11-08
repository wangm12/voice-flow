#!/usr/bin/env bash
set -euo pipefail
umask 077

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EXTERNAL_SOURCE="/Users/mingjie/Documents/voice-dictation-research/tools/voice-harness/macos"
RECIPE_SOURCE="$SCRIPT_DIR/learning-native-oracle/Recipe.json"
INFO_PLIST_SOURCE="$SCRIPT_DIR/learning-native-oracle/Info.plist"
HELPER_SOURCE="$SCRIPT_DIR/learning-owned-target-oracle.swift"
SOURCE_MANIFEST_SHA256="4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f"
EXPECTED_RECIPE_SHA256="1d2f3f722c0232bc78e5896fe80c4a328c800e6813e34ead983167ab0a27ce15"
EXPECTED_INFO_PLIST_SHA256="8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5"
EXPECTED_VALUE_SHA256="e7b17776dd843cfaa300566ee48aaa3a260364bb8edaebe7401ae749c1e9bfe1"
EXPECTED_VALUE_BYTE_LENGTH="29"
BUNDLE_IDENTIFIER="local.voiceflow.learning-oracle-fixture.v1"

SOURCE_MANIFEST="$(cat <<'EOF'
AXReadback.swift 0d7e004e5449f59a68d5f43e19a45920ac0b5396997e07338a1beef7218fa7b2
AXReadbackFixtures.swift 7f13a5affe61669a2ab791fe46f8b0c094f1e2400814f0da0840e3a054ea8407
Clipboard.swift 1a5e16d9d74b3e67cfa0a68edbab7560fbe1c5192486e42bfeb3de8ebc0bcd28
Driver.swift a74e0251515609e01a41d890dbe86a6f0390828488d30faf6d06f31711468982
FixtureHost.swift 250e3d09830df4ae48022f13a92d9f9bebe41a399eb56f86ec46944a85740da3
ForegroundPreparation.swift a5ac6b6ec1255aa77b5df41df6fdf9b1a0c3a7cb2783b3c300dce590897fee1d
HarnessContracts.swift 6eaa53875c8face0949d15bfaf5f8994235fa14da2a5f11f5993ab7cc9e797ce
NativeApp.swift 6d19a3f887b04ae38f646fec13ae2e97f332b6954aca5f7a6a8836c619e1bb85
NativeAppFixtures.swift 14e5ef852ff6ea66029a57e7ffbb25de1654d07257dd4c1d3bd07829ccdf8698
Oracle.swift 5df791dc7db2f776978e0b268b5a0fce7cedb245cfb82ba12d5483a3f5bc4259
ProcessIdentity.swift d9214d6767f1978c2acfba1c601a6930f374519a4d2738372710a15039b202a6
ProductResearch.swift 4f67c64b3b0d8645c841a3cb5305c51d0e8d09db85053e69431999b9bd26df62
SourceSelection.swift 141aa94bb1e25b2769c820250e1e8429aa932bb510cc7786d0df0bd2406c8c02
SourceSelectionFixtures.swift ad5b3c741e3a17f510d4eea78dee707040123847f5d6064d568acf5d041eeb90
EOF
)"

fail() {
  printf '%s\n' "$1" >&2
  exit 2
}

[[ "$(uname -s)" == "Darwin" ]] || fail "native oracle preparation requires macOS"
[[ -d "$EXTERNAL_SOURCE" && ! -L "$EXTERNAL_SOURCE" ]] || fail "pinned voice-harness source directory is unavailable or a symlink"
[[ -f "$HELPER_SOURCE" && ! -L "$HELPER_SOURCE" ]] || fail "owned native oracle helper source is unavailable or a symlink"
[[ -f "$RECIPE_SOURCE" && ! -L "$RECIPE_SOURCE" ]] || fail "fixed owned target recipe is unavailable or a symlink"
[[ -f "$INFO_PLIST_SOURCE" && ! -L "$INFO_PLIST_SOURCE" ]] || fail "fixed owned Host Info.plist is unavailable or a symlink"

RECIPE_SHA256="$(shasum -a 256 "$RECIPE_SOURCE" | awk '{print $1}')"
INFO_PLIST_SHA256="$(shasum -a 256 "$INFO_PLIST_SOURCE" | awk '{print $1}')"
HELPER_SOURCE_SHA256="$(shasum -a 256 "$HELPER_SOURCE" | awk '{print $1}')"
BUILD_SCRIPT_SHA256="$(shasum -a 256 "$SCRIPT_DIR/$(basename "${BASH_SOURCE[0]}")" | awk '{print $1}')"
[[ "$RECIPE_SHA256" == "$EXPECTED_RECIPE_SHA256" ]] || fail "fixed owned target recipe hash changed"
[[ "$INFO_PLIST_SHA256" == "$EXPECTED_INFO_PLIST_SHA256" ]] || fail "fixed owned Host Info.plist hash changed"

BASE_DIR="$REPO_ROOT/src-tauri/target/research/learning-native-oracle"
[[ ! -L "$REPO_ROOT/src-tauri/target" ]] || fail "task cache root must not traverse a symlink"
[[ ! -L "$REPO_ROOT/src-tauri/target/research" ]] || fail "research cache parent must not be a symlink"
mkdir -p "$BASE_DIR"
[[ -d "$BASE_DIR" && ! -L "$BASE_DIR" ]] || fail "task cache root must be a real directory"
TMP_ROOT="$(mktemp -d "$BASE_DIR/.prepare.XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT
mkdir -p "$TMP_ROOT/owned-source" "$TMP_ROOT/macos-source" "$TMP_ROOT/bin" "$TMP_ROOT/FixtureHost.app/Contents/MacOS"
cp "$HELPER_SOURCE" "$TMP_ROOT/owned-source/OwnedTargetOracle.swift"
cp "$RECIPE_SOURCE" "$TMP_ROOT/owned-source/Recipe.json"
cp "$INFO_PLIST_SOURCE" "$TMP_ROOT/owned-source/Info.plist"
[[ "$(shasum -a 256 "$TMP_ROOT/owned-source/OwnedTargetOracle.swift" | awk '{print $1}')" == "$HELPER_SOURCE_SHA256" ]] || fail "owned helper changed while its source snapshot was copied"
[[ "$(shasum -a 256 "$TMP_ROOT/owned-source/Recipe.json" | awk '{print $1}')" == "$EXPECTED_RECIPE_SHA256" ]] || fail "owned recipe changed while its source snapshot was copied"
[[ "$(shasum -a 256 "$TMP_ROOT/owned-source/Info.plist" | awk '{print $1}')" == "$EXPECTED_INFO_PLIST_SHA256" ]] || fail "owned Info.plist changed while its source snapshot was copied"
chmod a-w "$TMP_ROOT/owned-source/OwnedTargetOracle.swift" "$TMP_ROOT/owned-source/Recipe.json" "$TMP_ROOT/owned-source/Info.plist"
CACHE_IDENTITY="$(printf '%s\n%s\n%s\n%s\n%s' "$SOURCE_MANIFEST_SHA256" "$HELPER_SOURCE_SHA256" "$BUILD_SCRIPT_SHA256" "$RECIPE_SHA256" "$INFO_PLIST_SHA256")"
CACHE_ID="v1-$(printf '%s' "$CACHE_IDENTITY" | shasum -a 256 | awk '{print $1}')"
CACHE_ROOT="$BASE_DIR/$CACHE_ID"

if [[ -e "$CACHE_ROOT" ]]; then
  [[ -d "$CACHE_ROOT" && ! -L "$CACHE_ROOT" ]] || fail "existing prepared cache path is not a real directory"
  [[ -f "$CACHE_ROOT/prepared.json" && ! -L "$CACHE_ROOT/prepared.json" ]] || fail "existing prepared cache is incomplete; preserve it for review"
  python3 - "$CACHE_ROOT/prepared.json" "$CACHE_ID" "$CACHE_ROOT" <<'PY'
import hashlib
import json
import os
import sys

manifest_path, expected_cache_id, cache_root = sys.argv[1:]
with open(manifest_path, "rb") as source:
    manifest = json.load(source)
if manifest.get("artifact_type") != "voice_flow_owned_target_oracle_prepared" or manifest.get("schema_version") != 1 or manifest.get("cache_id") != expected_cache_id:
    raise SystemExit("existing prepared manifest does not match this source cache")
identity = "\n".join((manifest.get("source_manifest_sha256", ""), manifest.get("helper_source_sha256", ""),
                     manifest.get("build_script_sha256", ""), manifest.get("recipe_sha256", ""),
                     manifest.get("host_info_plist_sha256", "")))
if "v1-" + hashlib.sha256(identity.encode("utf-8")).hexdigest() != expected_cache_id:
    raise SystemExit("existing prepared cache identity is inconsistent")
expected_paths = {
    "helper_path": os.path.join(cache_root, "bin", "OwnedTargetOracle"),
    "host_app_path": os.path.join(cache_root, "FixtureHost.app"),
    "host_executable_path": os.path.join(cache_root, "FixtureHost.app", "Contents", "MacOS", "FixtureHost"),
    "recipe_path": os.path.join(cache_root, "Recipe.json"),
    "source_manifest_path": os.path.join(cache_root, "source-manifest.txt"),
    "info_plist_path": os.path.join(cache_root, "FixtureHost.app", "Contents", "Info.plist"),
}
if any(manifest.get(key) != path for key, path in expected_paths.items()):
    raise SystemExit("existing prepared manifest contains an unregistered path")
for key, path_key in (("helper_executable_sha256", "helper_path"), ("host_executable_sha256", "host_executable_path"),
                      ("recipe_sha256", "recipe_path"), ("source_manifest_sha256", "source_manifest_path"),
                      ("host_info_plist_sha256", "info_plist_path")):
    path = manifest.get(path_key)
    if not isinstance(path, str) or not os.path.isfile(path) or os.path.islink(path):
        raise SystemExit("existing prepared artifact is missing or a symlink")
    with open(path, "rb") as artifact:
        actual = hashlib.sha256(artifact.read()).hexdigest()
    if actual != manifest.get(key):
        raise SystemExit("existing prepared artifact hash mismatch: " + path_key)
PY
  printf '%s\n' "$CACHE_ROOT/prepared.json"
  exit 0
fi

printf '%s\n' "$SOURCE_MANIFEST" > "$TMP_ROOT/source-manifest.txt"
ACTUAL_MANIFEST_SHA256="$(shasum -a 256 "$TMP_ROOT/source-manifest.txt" | awk '{print $1}')"
[[ "$ACTUAL_MANIFEST_SHA256" == "$SOURCE_MANIFEST_SHA256" ]] || fail "pinned source manifest text is invalid"

while IFS=' ' read -r source_name expected_sha256; do
  [[ -n "$source_name" ]] || continue
  SOURCE_PATH="$EXTERNAL_SOURCE/$source_name"
  [[ -f "$SOURCE_PATH" && ! -L "$SOURCE_PATH" ]] || fail "pinned Swift source is unavailable or a symlink: $source_name"
  actual_sha256="$(shasum -a 256 "$SOURCE_PATH" | awk '{print $1}')"
  [[ "$actual_sha256" == "$expected_sha256" ]] || fail "pinned Swift source hash changed: $source_name"
  cp "$SOURCE_PATH" "$TMP_ROOT/macos-source/$source_name"
  [[ "$(shasum -a 256 "$TMP_ROOT/macos-source/$source_name" | awk '{print $1}')" == "$expected_sha256" ]] || fail "copied Swift source hash changed: $source_name"
done <<< "$SOURCE_MANIFEST"
chmod a-w "$TMP_ROOT/macos-source"/*.swift "$TMP_ROOT/source-manifest.txt"

SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
[[ -d "$SDK_PATH" ]] || fail "installed macOS SDK path is unavailable"
xcrun swiftc -sdk "$SDK_PATH" -D TARGET_ORACLE_ONLY "$TMP_ROOT/macos-source"/*.swift "$TMP_ROOT/owned-source/OwnedTargetOracle.swift" -o "$TMP_ROOT/bin/OwnedTargetOracle"
xcrun swiftc -sdk "$SDK_PATH" -D FIXTURE_HOST "$TMP_ROOT/macos-source"/*.swift "$TMP_ROOT/owned-source/OwnedTargetOracle.swift" -o "$TMP_ROOT/FixtureHost.app/Contents/MacOS/FixtureHost"
cp "$TMP_ROOT/owned-source/Recipe.json" "$TMP_ROOT/Recipe.json"
cp "$TMP_ROOT/owned-source/Info.plist" "$TMP_ROOT/FixtureHost.app/Contents/Info.plist"
[[ "$(shasum -a 256 "$TMP_ROOT/Recipe.json" | awk '{print $1}')" == "$EXPECTED_RECIPE_SHA256" ]] || fail "prepared recipe hash changed after snapshot copy"
[[ "$(shasum -a 256 "$TMP_ROOT/FixtureHost.app/Contents/Info.plist" | awk '{print $1}')" == "$EXPECTED_INFO_PLIST_SHA256" ]] || fail "prepared Info.plist hash changed after snapshot copy"
chmod a-w "$TMP_ROOT/Recipe.json" "$TMP_ROOT/FixtureHost.app/Contents/Info.plist"
chmod a-w "$TMP_ROOT/bin/OwnedTargetOracle" "$TMP_ROOT/FixtureHost.app/Contents/MacOS/FixtureHost"

HELPER_EXECUTABLE_SHA256="$(shasum -a 256 "$TMP_ROOT/bin/OwnedTargetOracle" | awk '{print $1}')"
HOST_EXECUTABLE_SHA256="$(shasum -a 256 "$TMP_ROOT/FixtureHost.app/Contents/MacOS/FixtureHost" | awk '{print $1}')"

python3 - "$TMP_ROOT/prepared.json" "$CACHE_ROOT" "$CACHE_ID" "$SOURCE_MANIFEST_SHA256" "$HELPER_SOURCE_SHA256" "$BUILD_SCRIPT_SHA256" "$HELPER_EXECUTABLE_SHA256" "$HOST_EXECUTABLE_SHA256" "$RECIPE_SHA256" "$INFO_PLIST_SHA256" "$EXPECTED_VALUE_SHA256" "$EXPECTED_VALUE_BYTE_LENGTH" "$BUNDLE_IDENTIFIER" <<'PY'
import json
import sys

(manifest_path, cache_root, cache_id, source_manifest_sha256, helper_source_sha256,
 build_script_sha256, helper_executable_sha256, host_executable_sha256, recipe_sha256,
 info_plist_sha256, expected_value_sha256, expected_value_byte_length, bundle_identifier) = sys.argv[1:]
manifest = {
    "artifact_type": "voice_flow_owned_target_oracle_prepared",
    "schema_version": 1,
    "cache_id": cache_id,
    "source_manifest_sha256": source_manifest_sha256,
    "helper_source_sha256": helper_source_sha256,
    "build_script_sha256": build_script_sha256,
    "helper_executable_sha256": helper_executable_sha256,
    "host_executable_sha256": host_executable_sha256,
    "recipe_sha256": recipe_sha256,
    "host_info_plist_sha256": info_plist_sha256,
    "expected_value_sha256": expected_value_sha256,
    "expected_value_byte_length": int(expected_value_byte_length),
    "host_bundle_identifier": bundle_identifier,
    "helper_path": cache_root + "/bin/OwnedTargetOracle",
    "host_app_path": cache_root + "/FixtureHost.app",
    "host_executable_path": cache_root + "/FixtureHost.app/Contents/MacOS/FixtureHost",
    "recipe_path": cache_root + "/Recipe.json",
    "source_manifest_path": cache_root + "/source-manifest.txt",
    "info_plist_path": cache_root + "/FixtureHost.app/Contents/Info.plist",
}
with open(manifest_path, "w", encoding="utf-8") as output:
    json.dump(manifest, output, sort_keys=True, separators=(",", ":"))
    output.write("\n")
PY

mv -n "$TMP_ROOT" "$CACHE_ROOT"
[[ ! -e "$TMP_ROOT" ]] || fail "prepared cache was created concurrently; existing output preserved"
trap - EXIT
printf '%s\n' "$CACHE_ROOT/prepared.json"
