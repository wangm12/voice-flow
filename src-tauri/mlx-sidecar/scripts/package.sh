#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sidecar_dir="$(cd "$script_dir/.." && pwd)"
repo_root="$(cd "$sidecar_dir/../.." && pwd)"
project="$sidecar_dir/xcode/VoiceFlowMLXSidecar.xcodeproj"
scheme="VoiceFlowMLXSidecar"
build_root="$repo_root/src-tauri/target/mlx-sidecar"
derived_data="$build_root/xcode-derived"
products="$derived_data/Build/Products/Release"
stage="$build_root/stage"
dev_mode="${1:-}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  rm -rf "$stage"
  mkdir -p "$stage"
  if [[ "$dev_mode" == "--dev" ]]; then
    dev_target="$repo_root/src-tauri/target/debug"
    rm -f "$dev_target/voiceflow-mlx-sidecar"
    rm -f "$dev_target/mlx.metallib"
    if [[ -d "$dev_target" ]]; then
      find "$dev_target" -maxdepth 1 -type d -name '*.bundle' -exec rm -rf {} +
    fi
  fi
  echo "Skipping the MLX sidecar build outside Apple silicon macOS."
  exit 0
fi

if ! xcodebuild -version >/dev/null 2>&1; then
  echo "Xcode is required to build the MLX sidecar and Metal resources." >&2
  exit 1
fi

mkdir -p "$build_root"
swift package --package-path "$sidecar_dir" resolve
xcodebuild -quiet \
  -project "$project" \
  -scheme "$scheme" \
  -configuration Release \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$derived_data" \
  ARCHS=arm64 \
  ONLY_ACTIVE_ARCH=YES \
  CODE_SIGNING_ALLOWED=NO \
  build

executable="$products/voiceflow-mlx-sidecar"
if [[ ! -x "$executable" ]]; then
  echo "Xcode did not produce the Release sidecar executable." >&2
  exit 1
fi
lipo "$executable" -verify_arch arm64

stage_temp="$(mktemp -d "$build_root/stage.XXXXXX")"
trap 'rm -rf "$stage_temp"' EXIT
install -m 0755 "$executable" "$stage_temp/voiceflow-mlx-sidecar"

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

signing_identity="${APPLE_SIGNING_IDENTITY:-}"
if [[ -n "$signing_identity" && "$signing_identity" != "-" ]]; then
  codesign --force --options runtime --timestamp --sign "$signing_identity" \
    "$stage_temp/voiceflow-mlx-sidecar"
fi

rm -rf "$stage"
mv "$stage_temp" "$stage"
trap - EXIT

if [[ "$dev_mode" == "--dev" ]]; then
  dev_target="$repo_root/src-tauri/target/debug"
  mkdir -p "$dev_target"
  install -m 0755 "$stage/voiceflow-mlx-sidecar" "$dev_target/voiceflow-mlx-sidecar"
  install -m 0644 "$stage/mlx.metallib" "$dev_target/mlx.metallib"
  while IFS= read -r -d '' bundle; do
    rm -rf "$dev_target/$(basename "$bundle")"
    cp -R "$bundle" "$dev_target/"
  done < <(find "$stage" -maxdepth 1 -type d -name '*.bundle' -print0)
fi

echo "Staged arm64 MLX sidecar and $bundle_count Xcode resource bundles at $stage"
