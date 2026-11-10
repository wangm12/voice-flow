#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
runtime_dir="$(cd "$script_dir/.." && pwd -P)"
tauri_dir="$(cd "$runtime_dir/.." && pwd -P)"
repo_root="$(cd "$tauri_dir/.." && pwd -P)"
build_root="$tauri_dir/target/llama-runtime"
source_dir="$build_root/source"
build_dir="$build_root/build"
stage="$build_root/stage"
pin_file="$runtime_dir/runtime-pin.env"

mkdir -p "$build_root"

# This is a repository-owned pin file, not user configuration.
# shellcheck disable=SC1090
source "$pin_file"

stage_temp="$(mktemp -d "$build_root/stage.XXXXXX")"
cleanup() {
  rm -rf "$stage_temp"
}
trap cleanup EXIT

write_unavailable_identity() {
  local platform="$1" reason="$2"
  mkdir -p "$stage_temp/llama-runtime"
  python3 - "$stage_temp/llama-runtime/BUILD-IDENTITY.json" \
    "$platform" "$reason" "$LLAMA_CPP_RELEASE_TAG" \
    "$LLAMA_CPP_TAG_OBJECT_SHA1" "$LLAMA_CPP_COMMIT" <<'PY'
import json
import sys
from pathlib import Path

output, platform, reason, tag, tag_object, commit = sys.argv[1:]
Path(output).write_text(json.dumps({
    "schema_version": 1,
    "status": "unavailable",
    "runtime": "llama.cpp",
    "reason": reason,
    "host_platform": platform,
    "source": {
        "repository": "https://github.com/ggml-org/llama.cpp",
        "release_tag": tag,
        "annotated_tag_object_sha1": tag_object,
        "commit": commit,
        "license": "MIT",
    },
}, indent=2) + "\n", encoding="utf-8")
PY
}

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-x86_64) reason="unsupported_platform" ;;
    *) reason="unsupported_platform" ;;
  esac
  write_unavailable_identity "$(uname -s)-$(uname -m)" "$reason"
  rm -rf "$stage"
  mv "$stage_temp" "$stage"
  trap - EXIT
  echo "Skipping the bundled llama.cpp runtime outside native Apple silicon macOS."
  exit 0
fi

for tool in git cmake python3 xcrun xcodebuild; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool is required to build the bundled llama.cpp runtime." >&2
    exit 1
  fi
done
if ! xcrun --find metal >/dev/null 2>&1; then
  echo "error: the selected Xcode toolchain does not provide the Metal compiler." >&2
  exit 1
fi

escape_compiler_path() {
  local path="${1//\\/\\\\}"
  path="${path// /\\ }"
  printf '%s' "$path"
}
source_dir_for_flags="$(escape_compiler_path "$source_dir")"
build_dir_for_flags="$(escape_compiler_path "$build_dir")"

mkdir -p "$build_root" "$source_dir"
if [[ ! -d "$source_dir/.git" ]]; then
  git -C "$source_dir" init -q
fi
if ! git -C "$source_dir" remote get-url origin >/dev/null 2>&1; then
  git -C "$source_dir" remote add origin "$LLAMA_CPP_REPOSITORY"
elif [[ "$(git -C "$source_dir" remote get-url origin)" != "$LLAMA_CPP_REPOSITORY" ]]; then
  echo "error: cached llama.cpp source has an unexpected Git remote." >&2
  exit 1
fi

if ! git -C "$source_dir" cat-file -e "$LLAMA_CPP_COMMIT^{commit}" 2>/dev/null; then
  git -C "$source_dir" -c protocol.version=2 fetch --depth=1 --filter=blob:none origin "$LLAMA_CPP_COMMIT"
fi
if ! git -C "$source_dir" show-ref --verify --quiet "refs/tags/$LLAMA_CPP_RELEASE_TAG"; then
  git -C "$source_dir" fetch --depth=1 origin \
    "refs/tags/$LLAMA_CPP_RELEASE_TAG:refs/tags/$LLAMA_CPP_RELEASE_TAG"
fi

tag_object="$(git -C "$source_dir" rev-parse "refs/tags/$LLAMA_CPP_RELEASE_TAG")"
tag_commit="$(git -C "$source_dir" rev-parse "$LLAMA_CPP_RELEASE_TAG^{commit}")"
if [[ "$tag_object" != "$LLAMA_CPP_TAG_OBJECT_SHA1" || "$tag_commit" != "$LLAMA_CPP_COMMIT" ]]; then
  echo "error: llama.cpp release tag does not match the pinned tag object and commit." >&2
  exit 1
fi
if git -C "$source_dir" rev-parse --verify HEAD >/dev/null 2>&1; then
  source_head="$(git -C "$source_dir" rev-parse HEAD)"
  if [[ "$source_head" != "$LLAMA_CPP_COMMIT" ]]; then
    echo "error: cached llama.cpp source is at an unexpected commit: $source_head" >&2
    exit 1
  fi
  if [[ -n "$(git -C "$source_dir" status --porcelain)" ]]; then
    echo "error: cached llama.cpp source has local changes; refusing to build it." >&2
    exit 1
  fi
else
  git -C "$source_dir" checkout --quiet --detach "$LLAMA_CPP_COMMIT"
fi

cmake_options=(
  -DCMAKE_BUILD_TYPE=Release
  -DCMAKE_OSX_ARCHITECTURES="$LLAMA_RUNTIME_ARCH"
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$LLAMA_RUNTIME_MINIMUM_MACOS"
  "-DCMAKE_C_FLAGS=-ffile-prefix-map=$source_dir_for_flags=llama.cpp -fmacro-prefix-map=$source_dir_for_flags=llama.cpp -ffile-prefix-map=$build_dir_for_flags=build -fmacro-prefix-map=$build_dir_for_flags=build"
  "-DCMAKE_CXX_FLAGS=-ffile-prefix-map=$source_dir_for_flags=llama.cpp -fmacro-prefix-map=$source_dir_for_flags=llama.cpp -ffile-prefix-map=$build_dir_for_flags=build -fmacro-prefix-map=$build_dir_for_flags=build"
  -DBUILD_SHARED_LIBS=OFF
  -DGGML_BACKEND_DL=OFF
  -DGGML_METAL=ON
  -DGGML_METAL_EMBED_LIBRARY=ON
  -DGGML_OPENMP=OFF
  -DGGML_NATIVE=OFF
  -DLLAMA_BUILD_COMMON=ON
  -DLLAMA_BUILD_TOOLS=ON
  -DLLAMA_BUILD_SERVER=ON
  -DLLAMA_BUILD_TESTS=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF
  -DLLAMA_BUILD_APP=OFF
  -DLLAMA_BUILD_UI=OFF
  -DLLAMA_USE_PREBUILT_UI=OFF
  -DLLAMA_OPENSSL=OFF
  -DLLAMA_SUBPROCESS=OFF
  -DLLAMA_LLGUIDANCE=OFF
)
cmake -S "$source_dir" -B "$build_dir" "${cmake_options[@]}"
build_jobs="${VOICEFLOW_LLAMA_BUILD_JOBS:-4}"
if [[ ! "$build_jobs" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: VOICEFLOW_LLAMA_BUILD_JOBS must be a positive integer." >&2
  exit 2
fi
cmake --build "$build_dir" --target llama-server --config Release --parallel "$build_jobs"

binary="$build_dir/bin/llama-server"
if [[ ! -x "$binary" ]]; then
  echo "error: CMake did not produce the pinned llama-server executable." >&2
  exit 1
fi
if [[ "$(lipo -archs "$binary")" != "$LLAMA_RUNTIME_ARCH" ]]; then
  echo "error: llama-server must contain only the pinned arm64 architecture." >&2
  exit 1
fi
build_load="$(vtool -arch arm64 -show-build "$binary")"
if ! grep -Fq "    minos $LLAMA_RUNTIME_MINIMUM_MACOS" <<< "$build_load"; then
  echo "error: llama-server does not have the pinned minimum macOS deployment target." >&2
  exit 1
fi

dependencies="$(otool -L "$binary" | tail -n +2 | sed -E 's/^[[:space:]]*([^ ]+).*/\1/')"
while IFS= read -r dependency; do
  [[ -z "$dependency" ]] && continue
  case "$dependency" in
    /System/Library/*|/usr/lib/*) ;;
    *)
      echo "error: llama-server has an unbundled non-system dependency: $dependency" >&2
      exit 1
      ;;
  esac
done <<< "$dependencies"

version_output="$("$binary" --version 2>&1)"
if ! grep -Fq "commit ${LLAMA_CPP_COMMIT:0:7}" <<< "$version_output"; then
  echo "error: llama-server --version does not report the pinned source commit." >&2
  exit 1
fi
help_output="$("$binary" --help 2>&1)"
for option in --model --host --alias --no-ui --offline --jinja --reasoning --ctx-size --predict --gpu-layers; do
  if ! grep -Fq -- "$option" <<< "$help_output"; then
    echo "error: pinned llama-server does not expose required runtime option $option." >&2
    exit 1
  fi
done

mkdir -p "$stage_temp/llama-runtime/bin" "$stage_temp/llama-runtime/ThirdPartyLicenses"
install -m 0755 "$binary" "$stage_temp/llama-runtime/bin/llama-server"
license_root="$stage_temp/llama-runtime/ThirdPartyLicenses"
mkdir -p "$license_root/llama.cpp" "$license_root/cpp-httplib" "$license_root/nlohmann-json" "$license_root/llama-vendor"
install -m 0644 "$source_dir/LICENSE" "$license_root/llama.cpp/LICENSE"
install -m 0644 "$source_dir/vendor/cpp-httplib/LICENSE" "$license_root/cpp-httplib/LICENSE"
install -m 0644 "$repo_root/ThirdPartyLicenses/Runtime/nlohmann-json-LICENSE.MIT" \
  "$license_root/nlohmann-json/LICENSE.MIT"
install -m 0644 "$source_dir/vendor/hash/rotate-bits/LICENSE.md" "$license_root/llama-vendor/rotate-bits-LICENSE.md"
install -m 0644 "$source_dir/vendor/hash/sha256/LICENSE" "$license_root/llama-vendor/sha256-LICENSE"
install -m 0644 "$source_dir/vendor/hash/xxhash/LICENSE" "$license_root/llama-vendor/xxhash-LICENSE"

python3 - "$source_dir" "$license_root/llama-vendor" <<'PY'
import sys
from pathlib import Path

source_root, output_root = map(Path, sys.argv[1:])

def extract_block(source: Path, marker: str, name: str) -> None:
    text = source.read_text(encoding="utf-8")
    marker_at = text.find(marker)
    if marker_at < 0:
        raise SystemExit(f"Required pinned license marker is missing from {source}.")
    start = text.rfind("/*", 0, marker_at)
    end = text.find("*/", marker_at)
    if start < 0 or end < 0:
        raise SystemExit(f"Could not isolate the pinned license block in {source}.")
    output = output_root / name
    output.write_text(text[start:end + 2].strip() + "\n", encoding="utf-8")

extract_block(source_root / "vendor/miniaudio/miniaudio.h",
              "This software is available as a choice of the following licenses.",
              "miniaudio-LICENSE.txt")
extract_block(source_root / "vendor/stb/stb_image.h",
              "This software is available under 2 licenses -- choose whichever you prefer.",
              "stb-image-LICENSE.txt")
extract_block(source_root / "vendor/sheredom/subprocess.h",
              "This is free and unencumbered software released into the public domain.",
              "sheredom-subprocess-Unlicense.txt")
extract_block(source_root / "vendor/hash/sha1/sha1.c",
              "100% Public Domain", "sha1-PUBLIC-DOMAIN-NOTICE.txt")

header = (source_root / "vendor/nlohmann/json.hpp").read_text(encoding="utf-8")
if "#define NLOHMANN_JSON_VERSION_MAJOR 3" not in header \
        or "#define NLOHMANN_JSON_VERSION_MINOR 12" not in header \
        or "#define NLOHMANN_JSON_VERSION_PATCH 0" not in header:
    raise SystemExit("Pinned llama.cpp source no longer contains the reviewed nlohmann/json 3.12.0.")
PY

install -m 0644 "$runtime_dir/NOTICE.md" "$stage_temp/llama-runtime/NOTICE.md"

cmake_version="$(cmake --version | head -n 1)"
xcode_version="$(xcodebuild -version | tr '\n' ' ')"
compiler_version="$(clang++ --version | head -n 1)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-version)"
python3 - "$stage_temp/llama-runtime/BUILD-IDENTITY.json" "$stage_temp/llama-runtime/bin/llama-server" \
  "$LLAMA_CPP_SOURCE_URL" "$LLAMA_CPP_RELEASE_TAG" "$LLAMA_CPP_TAG_OBJECT_SHA1" \
  "$LLAMA_CPP_COMMIT" "$LLAMA_CPP_LICENSE" "$LLAMA_RUNTIME_ARCH" "$LLAMA_RUNTIME_MINIMUM_MACOS" \
  "$cmake_version" "$xcode_version" "$compiler_version" "$macos_sdk" "$version_output" \
  "$source_dir" "$build_dir" \
  "${cmake_options[@]}" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

(output, binary, source_url, tag, tag_object, commit, license_name,
 architecture, min_macos, cmake, xcode, compiler, sdk, version_output, *cmake_options) = sys.argv[1:]
source_root, build_root = cmake_options[:2]
cmake_options = cmake_options[2:]

def public_option(option: str) -> str:
    return option.replace(source_root, "<llama.cpp-source>").replace(build_root, "<llama.cpp-build>")
binary_path = Path(binary)
data = {
    "schema_version": 1,
    "status": "ready",
    "runtime": "llama.cpp",
    "source": {
        "repository": source_url,
        "release_tag": tag,
        "annotated_tag_object_sha1": tag_object,
        "commit": commit,
        "license": license_name,
    },
    "artifact": {
        "path": "bin/llama-server",
        "architecture": architecture,
        "minimum_macos": min_macos,
        "size_bytes_pre_signing": binary_path.stat().st_size,
        "sha256_pre_signing": hashlib.sha256(binary_path.read_bytes()).hexdigest(),
    },
    "build": {
        "configuration": "Release",
        "cmake_options": [public_option(option) for option in cmake_options],
        "metal": True,
        "metal_embedded": True,
        "static_native_libraries": True,
        "openmp": False,
        "openssl": False,
        "web_ui": False,
        "llguidance": False,
    },
    "toolchain": {
        "cmake": cmake,
        "xcode": xcode.strip(),
        "compiler": compiler,
        "macos_sdk": sdk,
    },
    "version_output": version_output.strip(),
    "runtime_contract": {
        "transport": "AF_UNIX",
        "host_argument_suffix": ".sock",
        "model_network_access": "disabled by --offline at process launch",
    },
}
Path(output).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

signing_identity="${APPLE_SIGNING_IDENTITY:-}"
if [[ -n "$signing_identity" && "$signing_identity" != "-" ]]; then
  codesign --force --options runtime --timestamp --sign "$signing_identity" \
    "$stage_temp/llama-runtime/bin/llama-server"
  if [[ "${VOICEFLOW_NOTARIZE:-}" == "1" ]]; then
    signing_mode="developer_id"
  else
    signing_mode="signed"
  fi
else
  codesign --force --sign - "$stage_temp/llama-runtime/bin/llama-server"
  signing_mode="ad_hoc"
fi
if ! signature_info="$(codesign -dv --verbose=4 "$stage_temp/llama-runtime/bin/llama-server" 2>&1)"; then
  echo "error: could not inspect the staged llama-server code signature." >&2
  exit 1
fi
python3 - "$stage_temp/llama-runtime/BUILD-IDENTITY.json" \
  "$stage_temp/llama-runtime/bin/llama-server" "$signing_mode" "$signature_info" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

identity_path, binary_path, signing_mode, signature_info = sys.argv[1:]
identity = json.loads(Path(identity_path).read_text(encoding="utf-8"))
binary = Path(binary_path)
signature_lines = signature_info.splitlines()
authority = next((line.removeprefix("Authority=") for line in signature_lines
                  if line.startswith("Authority=")), None)
team_identifier = next((line.removeprefix("TeamIdentifier=") for line in signature_lines
                        if line.startswith("TeamIdentifier=")), None)
code_directory = next((line for line in signature_lines if "CodeDirectory" in line), "")
hardened_runtime = "(runtime)" in code_directory
ad_hoc_signature = "Signature=adhoc" in signature_lines or "(adhoc)" in code_directory
if signing_mode == "ad_hoc" and not ad_hoc_signature:
    raise SystemExit("Expected an ad-hoc llama-server signature.")
if signing_mode != "ad_hoc" and (ad_hoc_signature or not authority):
    raise SystemExit("Expected a certificate-signed llama-server signature.")
if signing_mode == "developer_id" and (not authority.startswith("Developer ID Application:")
        or not team_identifier or not hardened_runtime):
    raise SystemExit("The notarization helper must use a hardened Developer ID signature.")
identity["artifact"]["size_bytes"] = binary.stat().st_size
identity["artifact"]["sha256_signed"] = hashlib.sha256(binary.read_bytes()).hexdigest()
identity["signing"] = {
    "mode": signing_mode,
    "hardened_runtime": hardened_runtime,
    "authority": authority,
    "team_identifier": team_identifier,
}
Path(identity_path).write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
verify_args=(--runtime-dir "$stage_temp/llama-runtime" --expected-signing-mode "$signing_mode")
if [[ "$signing_mode" == "developer_id" ]]; then
  verify_args+=(--expected-authority "$signing_identity" --expected-team-id "${APPLE_TEAM_ID:-}")
fi
python3 "$script_dir/verify-runtime-identity.py" "${verify_args[@]}"

rm -rf "$stage"
mv "$stage_temp" "$stage"
trap - EXIT
echo "Staged pinned llama.cpp runtime at $stage/llama-runtime/bin/llama-server."
