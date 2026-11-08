#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
sidecar_dir="$(cd "$script_dir/.." && pwd -P)"
repo_root="$(cd "$sidecar_dir/../.." && pwd -P)"
project="$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj"
scheme="VoiceFlowMLXSidecar"
build_root="${VOICEFLOW_MLX_SIDECAR_BUILD_ROOT:-$repo_root/src-tauri/target/mlx-sidecar}"
derived_data="$build_root/xcode-derived"
products="$derived_data/Build/Products/Release"
stage="$build_root/stage"
dev_mode="${1:-}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  rm -rf "$stage"
  mkdir -p "$stage/ThirdPartyLicenses"
  cp -R "$repo_root/ThirdPartyLicenses/." "$stage/ThirdPartyLicenses/"
  python3 "$repo_root/scripts/verify-dependency-texts.py" \
    "$repo_root/ThirdPartyLicenses/DependencyTexts" \
    "$stage/ThirdPartyLicenses/DependencyTexts"
  python3 "$script_dir/normalize-staged-third-party-licenses.py" \
    "$stage/ThirdPartyLicenses" \
    "$repo_root/ThirdPartyLicenses"
  if [[ "$dev_mode" == "--dev" ]]; then
    dev_target="$repo_root/src-tauri/target/debug"
    rm -f "$dev_target/voiceflow-mlx-sidecar"
    rm -f "$dev_target/mlx.metallib"
    rm -f "$dev_target/libswiftCompatibilitySpan.dylib"
    rm -rf "$dev_target/ThirdPartyLicenses"
    if [[ -d "$dev_target" ]]; then
      find "$dev_target" -maxdepth 1 -type d -name '*.bundle' -exec rm -rf {} +
    fi
    mkdir -p "$dev_target"
    cp -R "$stage/ThirdPartyLicenses" "$dev_target/ThirdPartyLicenses"
  fi
  echo "Skipping the MLX sidecar build outside Apple silicon macOS."
  exit 0
fi

if ! xcodebuild -version >/dev/null 2>&1; then
  echo "Xcode is required to build the MLX sidecar and Metal resources." >&2
  exit 1
fi

mkdir -p "$build_root"
swiftpm_scratch="$build_root/swiftpm-scratch"
swiftpm_cache="$build_root/swiftpm-cache"
swiftpm_config="$build_root/swiftpm-config"
swiftpm_security="$build_root/swiftpm-security"
mkdir -p "$swiftpm_scratch" "$swiftpm_cache" "$swiftpm_config" "$swiftpm_security"
sidecar_lock_sha_before="$(shasum -a 256 "$sidecar_dir/Package.resolved" | awk '{print $1}')"
xcode_lock_sha_before="$(shasum -a 256 "$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" | awk '{print $1}')"
swift package \
  --package-path "$sidecar_dir" \
  --scratch-path "$swiftpm_scratch" \
  --cache-path "$swiftpm_cache" \
  --config-path "$swiftpm_config" \
  --security-path "$swiftpm_security" \
  --disable-keychain \
  --disable-netrc \
  --force-resolved-versions \
  --skip-update \
  resolve
python3 -B "$script_dir/apply-sdk-cancellation-overlay.py" \
  --source-root "$swiftpm_scratch/checkouts/mlx-audio-swift"

# Resolve Xcode's separate package graph before editing its actual SourcePackages
# checkout. The following build is locked to these resolved pins and cannot update.
xcodebuild -quiet \
  -project "$project" \
  -scheme "$scheme" \
  -resolvePackageDependencies \
  -clonedSourcePackagesDirPath "$derived_data/SourcePackages" \
  -onlyUsePackageVersionsFromResolvedFile \
  -skipPackageUpdates
xcode_sdk_checkout="$derived_data/SourcePackages/checkouts/mlx-audio-swift"
python3 -B "$script_dir/apply-sdk-cancellation-overlay.py" \
  --source-root "$xcode_sdk_checkout"
if [[ "$(shasum -a 256 "$sidecar_dir/Package.resolved" | awk '{print $1}')" != "$sidecar_lock_sha_before" \
  || "$(shasum -a 256 "$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" | awk '{print $1}')" != "$xcode_lock_sha_before" ]]; then
  echo "SwiftPM resolution changed a checked-in dependency lockfile; refusing to build." >&2
  exit 1
fi
# The pinned mlx-swift-lm 3.31.4 graph provides the local tokenizer macro used here.
xcodebuild -quiet \
  -skipMacroValidation \
  -disableAutomaticPackageResolution \
  -onlyUsePackageVersionsFromResolvedFile \
  -skipPackageUpdates \
  -project "$project" \
  -scheme "$scheme" \
  -configuration Release \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$derived_data" \
  ARCHS=arm64 \
  ONLY_ACTIVE_ARCH=YES \
  CODE_SIGNING_ALLOWED=NO \
  build
python3 -B "$script_dir/apply-sdk-cancellation-overlay.py" \
  --source-root "$swiftpm_scratch/checkouts/mlx-audio-swift" \
  --require-after
python3 -B "$script_dir/apply-sdk-cancellation-overlay.py" \
  --source-root "$xcode_sdk_checkout" \
  --require-after
if [[ "$(shasum -a 256 "$sidecar_dir/Package.resolved" | awk '{print $1}')" != "$sidecar_lock_sha_before" \
  || "$(shasum -a 256 "$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" | awk '{print $1}')" != "$xcode_lock_sha_before" ]]; then
  echo "The sidecar build changed a checked-in dependency lockfile; refusing to stage." >&2
  exit 1
fi

executable="$products/voiceflow-mlx-sidecar"
if [[ ! -x "$executable" ]]; then
  echo "Xcode did not produce the Release sidecar executable." >&2
  exit 1
fi
lipo "$executable" -verify_arch arm64

stage_temp="$(mktemp -d "$build_root/stage.XXXXXX")"
trap 'rm -rf "$stage_temp"' EXIT
install -m 0755 "$executable" "$stage_temp/voiceflow-mlx-sidecar"

# Resolve compatibility libraries from the active Xcode toolchain. The helper
# links this library through @rpath, so keep a sibling @executable_path rpath
# and package the selected toolchain's matching runtime next to the helper.
if ! command -v xcrun >/dev/null 2>&1; then
  echo "xcrun is required to locate the selected Xcode Swift runtime tools." >&2
  exit 1
fi
stdlib_tool="$(xcrun --find swift-stdlib-tool 2>/dev/null)" || {
  echo "The selected Xcode toolchain does not provide swift-stdlib-tool." >&2
  exit 1
}
if [[ ! -x "$stdlib_tool" ]]; then
  echo "The selected Xcode swift-stdlib-tool is not executable." >&2
  exit 1
fi
toolchain_root="$(cd "$(dirname "$stdlib_tool")/../.." && pwd -P)"
load_libraries="$(otool -L "$executable")"
if ! grep -Fq '@rpath/libswiftCompatibilitySpan.dylib' <<< "$load_libraries"; then
  echo "The Release sidecar does not declare its Swift compatibility runtime dependency." >&2
  exit 1
fi
if ! otool -l "$executable" | awk '
  /^Load command/ { in_rpath = 0 }
  /cmd LC_RPATH/ { in_rpath = 1; next }
  in_rpath && $1 == "path" && $2 == "@executable_path" { found = 1 }
  END { exit found ? 0 : 1 }
'; then
  echo "The Release sidecar is missing its link-time @executable_path runtime search path." >&2
  exit 1
fi
if ! swift_runtime_plan="$("$stdlib_tool" --print --scan-executable "$executable" --platform macosx 2>/dev/null)"; then
  echo "The selected Xcode toolchain could not scan the sidecar's Swift runtime dependencies." >&2
  exit 1
fi
if ! grep -Fq 'libswiftCompatibilitySpan.dylib' <<< "$swift_runtime_plan"; then
  echo "The selected Xcode toolchain did not resolve libswiftCompatibilitySpan.dylib." >&2
  exit 1
fi
runtime_source_path="$(grep -F 'libswiftCompatibilitySpan.dylib' <<< "$swift_runtime_plan" | head -n 1)"
case "$runtime_source_path" in
  "$toolchain_root"/*) ;;
  *)
    echo "The Swift compatibility runtime did not come from the selected Xcode toolchain." >&2
    exit 1
    ;;
esac
if ! "$stdlib_tool" --copy \
  --scan-executable "$executable" \
  --platform macosx \
  --destination "$stage_temp"; then
  echo "The selected Xcode toolchain could not stage the Swift runtime dependencies." >&2
  exit 1
fi
swift_runtime="$stage_temp/libswiftCompatibilitySpan.dylib"
if [[ ! -s "$swift_runtime" ]]; then
  echo "The selected Xcode Swift compatibility runtime was not copied into the sidecar stage." >&2
  exit 1
fi
lipo "$swift_runtime" -verify_arch arm64
runtime_arches="$(lipo -archs "$swift_runtime")"
runtime_build="$(vtool -arch arm64 -show-build "$swift_runtime")"
helper_build="$(vtool -arch arm64 -show-build "$executable")"
runtime_minos="$(awk '/platform MACOS/{in_macos=1; next} in_macos && /minos /{print $2; exit}' <<< "$runtime_build")"
helper_minos="$(awk '/platform MACOS/{in_macos=1; next} in_macos && /minos /{print $2; exit}' <<< "$helper_build")"
if [[ ! "$runtime_minos" =~ ^[0-9]+(\.[0-9]+)?$ || ! "$helper_minos" =~ ^[0-9]+(\.[0-9]+)?$ ]]; then
  echo "Could not determine the arm64 macOS minimum version for the helper and Swift runtime." >&2
  exit 1
fi
runtime_major="${runtime_minos%%.*}"
runtime_minor="${runtime_minos#*.}"
helper_major="${helper_minos%%.*}"
helper_minor="${helper_minos#*.}"
runtime_minor="${runtime_minor%%.*}"
helper_minor="${helper_minor%%.*}"
if (( runtime_major > helper_major || (runtime_major == helper_major && runtime_minor > helper_minor) )); then
  echo "The selected Xcode Swift runtime requires a newer macOS version than the helper." >&2
  exit 1
fi

bundle_count=0
while IFS= read -r -d '' bundle; do
  cp -R "$bundle" "$stage_temp/"
  bundle_count=$((bundle_count + 1))
done < <(find "$products" -maxdepth 1 -type d -name '*.bundle' -print0)

default_metallib="$stage_temp/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
if [[ "$bundle_count" -eq 0 || ! -s "$default_metallib" ]]; then
  echo "Xcode did not produce the MLX Cmlx Metal shader resource bundle." >&2
  exit 1
fi
install -m 0644 "$default_metallib" "$stage_temp/mlx.metallib"
if ! cmp -s "$default_metallib" "$stage_temp/mlx.metallib"; then
  echo "The colocated MLX Metal shader does not match the Xcode resource bundle." >&2
  exit 1
fi

while IFS= read -r -d '' metallib; do
  install -m 0644 "$metallib" "$stage_temp/$(basename "$metallib")"
done < <(find "$products" -maxdepth 1 -type f -name '*.metallib' -print0)

# Stage full pinned license texts only after verifying each against the local
# Xcode SourcePackages checkout used by this build. Never silently omit a text.
license_source_root="$repo_root/ThirdPartyLicenses/Upstream"
source_packages="$derived_data/SourcePackages"
checkouts="$source_packages/checkouts"
if [[ ! -d "$checkouts" ]]; then
  echo "The Xcode SourcePackages checkout directory is missing; cannot verify sidecar license texts." >&2
  exit 1
fi
mkdir -p "$stage_temp/ThirdPartyLicenses"
cp -R "$repo_root/ThirdPartyLicenses/." "$stage_temp/ThirdPartyLicenses/"
python3 "$repo_root/scripts/verify-dependency-texts.py" \
  "$repo_root/ThirdPartyLicenses/DependencyTexts" \
  "$stage_temp/ThirdPartyLicenses/DependencyTexts"
python3 "$script_dir/normalize-staged-third-party-licenses.py" \
  "$stage_temp/ThirdPartyLicenses" \
  "$repo_root/ThirdPartyLicenses"
license_count=0
while IFS= read -r -d '' license_file; do
  relative_license="${license_file#"$license_source_root"/}"
  checkout_license="$checkouts/$relative_license"
  if [[ ! -f "$checkout_license" ]]; then
    echo "Pinned license text is missing from the resolved checkout: $relative_license" >&2
    exit 1
  fi
  if ! cmp -s "$license_file" "$checkout_license"; then
    echo "Pinned license text differs from the resolved checkout: $relative_license" >&2
    exit 1
  fi
  license_count=$((license_count + 1))
done < <(find "$license_source_root" -type f -print0)
if [[ "$license_count" -ne 20 ]]; then
  echo "Expected 20 upstream license and notice files, found $license_count." >&2
  exit 1
fi
if ! cmp -s \
  "$repo_root/ThirdPartyLicenses/SwiftRuntime/LICENSE.txt" \
  "$checkouts/swift-syntax/LICENSE.txt"; then
  echo "The preserved Swift license reference text differs from the pinned SwiftSyntax source." >&2
  exit 1
fi

sidecar_resolved="$sidecar_dir/Package.resolved"
xcode_resolved="$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved"
for lockfile in "$sidecar_resolved" "$xcode_resolved"; do
  if [[ ! -s "$lockfile" ]]; then
    echo "A required SwiftPM dependency lockfile is missing: $lockfile" >&2
    exit 1
  fi
done
mlx_swift_checkout="$checkouts/mlx-swift"
if [[ ! -d "$mlx_swift_checkout" ]]; then
  echo "The pinned mlx-swift checkout is missing from Xcode SourcePackages." >&2
  exit 1
fi
read_gitlink_commit() {
  local checkout="$1"
  local path="$2"
  local entry mode type object actual_path
  entry="$(git -C "$checkout" ls-tree HEAD -- "$path")" || return 1
  mode=""
  type=""
  object=""
  actual_path=""
  read -r mode type object actual_path <<< "$entry"
  if [[ "$mode" != "160000" || "$type" != "commit" || "$actual_path" != "$path" ]]; then
    return 1
  fi
  printf '%s\n' "$object"
}
mlx_swift_revision="$(git -C "$mlx_swift_checkout" rev-parse HEAD)"
if ! grep -Fq "$mlx_swift_revision" "$sidecar_resolved" || ! grep -Fq "$mlx_swift_revision" "$xcode_resolved"; then
  echo "The mlx-swift checkout revision is not present in both resolved lockfiles." >&2
  exit 1
fi
mlx_gitlink="$(read_gitlink_commit "$mlx_swift_checkout" "Source/Cmlx/mlx")" || {
  echo "The mlx nested gitlink is missing from the pinned mlx-swift tree." >&2
  exit 1
}
mlx_c_gitlink="$(read_gitlink_commit "$mlx_swift_checkout" "Source/Cmlx/mlx-c")" || {
  echo "The mlx-c nested gitlink is missing from the pinned mlx-swift tree." >&2
  exit 1
}
mlx_actual="$(git -C "$mlx_swift_checkout/Source/Cmlx/mlx" rev-parse HEAD)" || {
  echo "The mlx nested submodule checkout is missing." >&2
  exit 1
}
mlx_c_actual="$(git -C "$mlx_swift_checkout/Source/Cmlx/mlx-c" rev-parse HEAD)" || {
  echo "The mlx-c nested submodule checkout is missing." >&2
  exit 1
}
if [[ "$mlx_actual" != "$mlx_gitlink" || "$mlx_c_actual" != "$mlx_c_gitlink" ]]; then
  echo "A nested MLX checkout does not match the pinned mlx-swift GitLink." >&2
  exit 1
fi
manifest_dir="$stage_temp/ThirdPartyLicenses/DependencyManifest"
mkdir -p "$manifest_dir"
cp "$sidecar_resolved" "$manifest_dir/Package.resolved"
cp "$xcode_resolved" "$manifest_dir/Xcode-Package.resolved"
cp "$sidecar_dir/sdk-overlay/patch-manifest.json" "$manifest_dir/mlx-audio-swift-cancellation-overlay.json"
overlay_patch_sha="$(shasum -a 256 "$sidecar_dir/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch" | awk '{print $1}')"
overlay_manifest_sha="$(shasum -a 256 "$sidecar_dir/sdk-overlay/patch-manifest.json" | awk '{print $1}')"
sidecar_lock_sha="$(shasum -a 256 "$sidecar_resolved" | awk '{print $1}')"
xcode_lock_sha="$(shasum -a 256 "$xcode_resolved" | awk '{print $1}')"
runtime_source_sha="$(shasum -a 256 "$runtime_source_path" | awk '{print $1}')"
runtime_source_relative="${runtime_source_path#"$toolchain_root"/}"
{
  printf 'VoiceFlow native MLX sidecar dependency identities\n'
  printf 'Sidecar Package.resolved SHA-256: %s\n' "$sidecar_lock_sha"
  printf 'Xcode Package.resolved SHA-256: %s\n' "$xcode_lock_sha"
  printf 'mlx-swift checkout revision: %s\n' "$mlx_swift_revision"
  printf 'mlx-audio-swift upstream revision: 01dec7c9bdce3088a6b6b7ab9f2e403458195efb\n'
  printf 'mlx-audio-swift cancellation overlay patch SHA-256: %s\n' "$overlay_patch_sha"
  printf 'mlx-audio-swift cancellation overlay manifest SHA-256: %s\n' "$overlay_manifest_sha"
  printf 'Source/Cmlx/mlx GitLink revision: %s\n' "$mlx_gitlink"
  printf 'Source/Cmlx/mlx-c GitLink revision: %s\n' "$mlx_c_gitlink"
  printf 'Swift compatibility runtime: libswiftCompatibilitySpan.dylib\n'
  printf 'Selected toolchain runtime relative path: %s\n' "$runtime_source_relative"
  printf 'Selected toolchain runtime SHA-256: %s\n' "$runtime_source_sha"
  printf 'Swift compatibility runtime architecture slices: %s\n' "$runtime_arches"
  printf 'Swift compatibility runtime arm64 macOS minimum: %s\n' "$runtime_minos"
  printf 'Helper arm64 macOS minimum: %s\n' "$helper_minos"
  printf 'Swift runtime library names selected by swift-stdlib-tool:\n'
  printf '%s\n' "$swift_runtime_plan" | sed 's#.*/##' | LC_ALL=C sort -u
} > "$manifest_dir/dependency-identities.txt"
(
  cd "$stage_temp/ThirdPartyLicenses"
  find Models Upstream SwiftRuntime -type f -print | LC_ALL=C sort | while IFS= read -r license_path; do
    shasum -a 256 "$license_path"
  done
) > "$manifest_dir/license-file-hashes.txt"

signing_identity="${APPLE_SIGNING_IDENTITY:-}"
if [[ -n "$signing_identity" && "$signing_identity" != "-" ]]; then
  # Sign nested runtime first, then the helper that loads it.
  codesign --force --options runtime --timestamp --sign "$signing_identity" \
    "$swift_runtime"
  codesign --force --options runtime --timestamp --sign "$signing_identity" \
    "$stage_temp/voiceflow-mlx-sidecar"
else
  # Development stages use ordinary ad hoc signatures for the sibling runtime
  # and helper. This does not establish older-system loader acceptance.
  codesign --force --sign - "$swift_runtime"
  codesign --force --sign - "$stage_temp/voiceflow-mlx-sidecar"
fi

# Record the final staged runtime bytes only after its final signature has been
# applied. Keep the selected-toolchain source hash above as separate provenance.
runtime_sha="$(shasum -a 256 "$swift_runtime" | awk '{print $1}')"
printf 'Staged Swift compatibility runtime SHA-256: %s\n' "$runtime_sha" \
  >> "$manifest_dir/dependency-identities.txt"

rm -rf "$stage"
mv "$stage_temp" "$stage"
trap - EXIT

if [[ "$dev_mode" == "--dev" ]]; then
  dev_target="$repo_root/src-tauri/target/debug"
  mkdir -p "$dev_target"
  install -m 0755 "$stage/voiceflow-mlx-sidecar" "$dev_target/voiceflow-mlx-sidecar"
  install -m 0644 "$stage/mlx.metallib" "$dev_target/mlx.metallib"
  install -m 0755 "$stage/libswiftCompatibilitySpan.dylib" "$dev_target/libswiftCompatibilitySpan.dylib"
  rm -rf "$dev_target/ThirdPartyLicenses"
  cp -R "$stage/ThirdPartyLicenses" "$dev_target/ThirdPartyLicenses"
  while IFS= read -r -d '' bundle; do
    rm -rf "$dev_target/$(basename "$bundle")"
    cp -R "$bundle" "$dev_target/"
  done < <(find "$stage" -maxdepth 1 -type d -name '*.bundle' -print0)
fi

echo "Staged arm64 MLX sidecar and $bundle_count Xcode resource bundles at $stage"
