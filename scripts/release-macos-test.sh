#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEMP_RAW="$(mktemp -d "${TMPDIR:-/tmp}/voiceflow-release-test.XXXXXX")"
TEMP="$(cd "$TEMP_RAW" && pwd -P)"
trap 'rm -rf "$TEMP_RAW"' EXIT

fail() {
  echo "error: $*" >&2
  exit 1
}

make_fixture() {
  local fixture="$1"
  mkdir -m 700 -p "$fixture/scripts" "$fixture/bin" "$fixture/ThirdPartyLicenses" "$fixture/tmp"
  # Retain the real notarize recipe without prepending a developer's host Rust
  # toolchain ahead of the command stubs. HOME remains inherited and unchanged.
  sed '/^RUST_BIN :=/d; /^export PATH :=/d' "$ROOT/Makefile" > "$fixture/Makefile"
  cp "$ROOT/scripts/release-macos.sh" "$fixture/scripts/release-macos.sh"
  cp "$ROOT/scripts/resolve-dmg-signing.sh" "$fixture/scripts/resolve-dmg-signing.sh"
  cp "$ROOT/scripts/make-signing-cert.sh" "$fixture/scripts/make-signing-cert.sh"
  cp "$ROOT/scripts/select-codesign-identity.sh" "$fixture/scripts/select-codesign-identity.sh"
  cp "$ROOT/scripts/stage-dmg.sh" "$fixture/scripts/stage-dmg.sh"
  cp "$ROOT/scripts/detach-voiceflow-dmg.sh" "$fixture/scripts/detach-voiceflow-dmg.sh"
  cp "$ROOT/scripts/list-voiceflow-dmg-mounts.sh" "$fixture/scripts/list-voiceflow-dmg-mounts.sh"
  cp "$ROOT/scripts/verify-dependency-texts.py" "$fixture/scripts/verify-dependency-texts.py"
  mkdir -p "$fixture/ThirdPartyLicenses/DependencyTexts/by-sha256" \
    "$fixture/ThirdPartyLicenses/Models" "$fixture/ThirdPartyLicenses/Upstream/nested" \
    "$fixture/src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm"
  printf '{}\n' > "$fixture/ThirdPartyLicenses/DependencyTexts/license-index.json"
  printf '# Synthetic dependency inventory\n' > "$fixture/ThirdPartyLicenses/DependencyTexts/DEPENDENCY_LICENSES.md"
  printf 'Synthetic license body\n' > "$fixture/ThirdPartyLicenses/DependencyTexts/by-sha256/stub.txt"
  printf 'Synthetic third-party notice for release fixture.\n' > "$fixture/THIRD_PARTY_NOTICES.md"
  printf 'Synthetic model notice; no model data.\n' > "$fixture/ThirdPartyLicenses/Models/stub-model-notice.txt"
  printf 'Synthetic upstream notice.\n' > "$fixture/ThirdPartyLicenses/Upstream/nested/NOTICE.txt"
  printf '{"pins":[],"fixture":"sidecar"}\n' > "$fixture/src-tauri/mlx-sidecar/Package.resolved"
  printf '{"pins":[],"fixture":"xcode-sidecar"}\n' > "$fixture/src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved"
  chmod +x "$fixture/scripts/release-macos.sh" "$fixture/scripts/resolve-dmg-signing.sh" \
    "$fixture/scripts/make-signing-cert.sh" "$fixture/scripts/select-codesign-identity.sh" "$fixture/scripts/stage-dmg.sh" \
    "$fixture/scripts/detach-voiceflow-dmg.sh" "$fixture/scripts/list-voiceflow-dmg-mounts.sh"

  cat > "$fixture/bin/uname" <<'STUB'
#!/usr/bin/env bash
[[ "$#" -eq 1 ]] || exit 1
case "$1" in
  -s) echo Darwin ;;
  -m) echo x86_64 ;;
  *) exit 1 ;;
esac
STUB
  cat > "$fixture/bin/mktemp" <<'STUB'
#!/usr/bin/env bash
printf 'mktemp: %s\n' "$*" >> "$STUB_LOG" || exit 1
if [[ "$#" -eq 1 && "$1" == -d ]]; then
  exec /usr/bin/mktemp -d "$STUB_ROOT/tmp/voiceflow-helper.XXXXXX"
elif [[ "$#" -eq 2 && "$1" == -d && "$2" == "$STUB_ROOT/.build/release/.voiceflow-notary.XXXXXX" ]]; then
  workdir="$(/usr/bin/mktemp -d "$STUB_ROOT/.build/release/.voiceflow-notary.XXXXXX")" || exit $?
  printf '%s\n' "$workdir" > "$STUB_ROOT/.voiceflow-notary-workdir" || exit 1
  printf '%s\n' "$workdir"
else
  exit 1
fi
STUB
  cat > "$fixture/bin/cargo" <<'STUB'
#!/usr/bin/env bash
exit 1
STUB
  cat > "$fixture/expected-VoiceFlow-producer" <<'STUB_PRODUCER'
#!/usr/bin/env python3
import json, os, stat, sys
from pathlib import Path

def reject():
    raise SystemExit(64)
args = sys.argv[1:]
try:
    root = Path(os.environ["STUB_ROOT"])
    if not root.is_absolute() or root.is_symlink() or not root.is_dir(): reject()
    root = root.resolve(strict=True)
    if root.stat().st_uid != os.geteuid(): reject()
    record = root / ".voiceflow-notary-workdir"
    record_stat = record.lstat()
    if not stat.S_ISREG(record_stat.st_mode): reject()
    raw_work = record.read_text(encoding="utf-8")
    if not raw_work.endswith("\n") or raw_work.count("\n") != 1: reject()
    work = Path(raw_work[:-1])
    parent = root / ".build" / "release"
    if not work.is_absolute() or work.name in (".", "..") or not work.name.startswith(".voiceflow-notary.") or not work.name[len(".voiceflow-notary."):] or "/" in work.name[len(".voiceflow-notary."):]: reject()
    for directory in (root / ".build", parent):
        if directory.is_symlink() or not directory.is_dir() or directory.resolve(strict=True) != directory: reject()
    if work.parent != parent or work.is_symlink() or not work.is_dir() or work.resolve(strict=True) != work or work.stat().st_uid != os.geteuid(): reject()
    target = os.environ["STUB_EXPECTED_TARGET"]
    if not target or "/" in target or target in (".", ".."): reject()
    built = root / "src-tauri" / "target" / target / "release/bundle/macos/VoiceFlow-1.app/Contents/MacOS/VoiceFlow"
    mounted = work / "mount/VoiceFlow-1.app/Contents/MacOS/VoiceFlow"
    executable = Path(sys.argv[0])
    if not executable.is_absolute() or executable.is_symlink(): reject()
    executable_stat = executable.lstat()
    if not stat.S_ISREG(executable_stat.st_mode) or executable_stat.st_uid != os.geteuid(): reject()
    executable = executable.resolve(strict=True)
    if executable not in (built, mounted): reject()
except (OSError, KeyError, ValueError):
    reject()
phase = "production-entry-mounted" if executable == mounted else "production-entry-prezip"
fault = os.environ.get("STUB_APP_PRODUCER_FAULT", "none")
allowed = {"none", "import-exit", "status-diagnostic", "stdio-stdout", "wav-exit", "mounted-import-exit"}
if fault not in allowed or phase not in {"production-entry-prezip", "production-entry-mounted"}: reject()
if len(args) == 1 and args[0] == "--import-test-groq-key-release-probe":
    rc, err = 2, "test_groq_key_cli_requires_research_harness_feature"
    if fault == "import-exit" or (fault == "mounted-import-exit" and phase == "production-entry-mounted"): rc = 7
elif len(args) == 1 and args[0] == "--test-groq-key-status-release-probe":
    rc, err = 2, "test_groq_key_cli_requires_research_harness_feature"
    if fault == "status-diagnostic": err = "wrong_status_diagnostic"
elif len(args) == 1 and args[0] == "--research-stdio":
    rc, err = 0, "research_stdio_requires_macos_research_harness_feature"
elif (len(args) == 4 and args[0] == "--research-owned-wav" and
      Path(args[1]).is_absolute() and not Path(args[1]).exists() and
      args[2] == "--target-nonce" and args[3] == "00000000-0000-4000-8000-000000000001"):
    expected_wav = work / phase / "nonexistent-release-probe.wav"
    if Path(args[1]) != expected_wav: reject()
    rc, err = 0, "owned_wav_replay_status=unsupported_build"
    if fault == "wav-exit": rc = 9
else:
    reject()
if sys.stdin.buffer.read() != b"": reject()
root = Path(os.environ["STUB_ROOT"])
log = root / "app-producer.jsonl"
row = {"phase": phase, "argv": args, "eof": True,
       "wav_path": args[1] if args and args[0] == "--research-owned-wav" else None,
       "wav_exists": False if args and args[0] == "--research-owned-wav" else None,
       "nonce": args[3] if args and args[0] == "--research-owned-wav" else None,
       "fault": fault, "rc": rc, "diagnostic": err}
with log.open("a", encoding="utf-8") as f: f.write(json.dumps(row, separators=(",", ":")) + "\n")
if fault == "stdio-stdout" and args == ["--research-stdio"]: sys.stdout.write("unexpected\n")
sys.stderr.write(err + "\n")
raise SystemExit(rc)
STUB_PRODUCER
  chmod +x "$fixture/expected-VoiceFlow-producer"
  cat > "$fixture/bin/rustup" <<'STUB'
#!/usr/bin/env bash
printf 'rustup:%s\n' "$*" >> "$STUB_LOG"
[[ "$#" -eq 3 && "$1" == target && "$2" == add \
  && "$3" == aarch64-apple-darwin ]] \
  || [[ "$#" -eq 4 && "$1" == target && "$2" == add \
    && "$3" == aarch64-apple-darwin && "$4" == x86_64-apple-darwin ]]
STUB
  cat > "$fixture/bin/npm" <<'STUB'
#!/usr/bin/env bash
printf 'npm:%s\n' "$*" >> "$STUB_LOG"
if [[ "$#" -eq 1 && "$1" == ci ]]; then exit 0; fi
if [[ "$#" -eq 2 && "$1" == run && "$2" == build ]]; then exit 0; fi
if [[ "$#" -eq 6 && "$1 $2 $3 $4 $5 $6" == 'run tauri build -- --bundles dmg' ]]; then
  printf 'build-env identity=%s team=%s developer=%s development_team=%s\n' \
    "${APPLE_SIGNING_IDENTITY:-}" "${APPLE_TEAM_ID:-}" "${DEVELOPER_IDENTITY:-}" "${DEVELOPMENT_TEAM:-}" >> "$STUB_LOG"
  root="$STUB_ROOT/src-tauri/target/release/bundle/dmg"
  mkdir -p "$root"
  printf 'stub dmg\n' > "$root/VoiceFlow.dmg"
  exit 0
fi
expected="run tauri -- build --target ${STUB_EXPECTED_TARGET} --bundles app"
if [[ "$#" -eq 8 && "$*" == "$expected" ]]; then
  if [[ -n "${APPLE_CERTIFICATE+x}" || -n "${APPLE_CERTIFICATE_PASSWORD+x}" ]]; then
    echo "temporary certificate variables reached the Tauri build" >&2; exit 31
  fi
  if [[ -e "$STUB_ROOT/expect-temporary-keychain" ]]; then
    python3 - "$STUB_ROOT" <<'PY_EXPECT_NPM'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
try:
    wrecord = root / ".voiceflow-notary-workdir"
    crecord = root / ".voiceflow-notary-state"
    workdir = wrecord.read_text().rstrip("\n")
    chain = crecord.read_text().rstrip("\n")
    expected = str(root / workdir[len(str(root)) + 1:] / "signing.keychain-db")
    state_path = pathlib.Path(chain)
    current_path = root / "current-items"
    originals_path = root / "original-items"
    current = current_path.read_bytes().splitlines()
    originals = originals_path.read_bytes().splitlines()
    prefix = str(root) + "/.build/release/.voiceflow-notary."
    suffix = workdir[len(prefix):] if workdir.startswith(prefix) else ""
    valid = (all(p.is_file() and not p.is_symlink() for p in
                 (wrecord, crecord, current_path, originals_path))
             and bool(suffix) and "/" not in suffix and ".." not in suffix
             and chain == expected and state_path.is_file() and not state_path.is_symlink()
             and all((root / name).is_file() and not (root / name).is_symlink()
                     for name in ("imported", "partitioned", "published"))
             and current == [chain.encode(), *originals])
except (OSError, ValueError, IndexError):
    valid = False
sys.exit(0 if valid else 83)
PY_EXPECT_NPM
    [[ "$?" -eq 0 ]] || exit 83
  elif [[ -f "$STUB_ROOT/.voiceflow-notary-state" ]]; then
    chain="$(<"$STUB_ROOT/.voiceflow-notary-state")"
    [[ -f "$chain" && -f "$STUB_ROOT/imported" && -f "$STUB_ROOT/published" ]] || exit 83
  fi
  if [[ "${STUB_NOTARY_TAURI_FAIL:-0}" == 1 ]]; then exit 82; fi
  if [[ -n "${APPLE_ID:-}${APPLE_PASSWORD:-}${APPLE_API_KEY:-}${APPLE_API_KEY_PATH:-}${APPLE_API_ISSUER:-}${APPLE_API_KEY_ID:-}${APPLE_API_ISSUER_ID:-}" ]]; then
    echo "notary credentials were passed into the Tauri build" >&2
    exit 31
  fi
  local_root="$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle"
  mkdir -p "$local_root/macos"
  app_count="${STUB_APP_COUNT:-1}"
  for ((i=1; i<=app_count; i++)); do
    app="$local_root/macos/VoiceFlow-$i.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$STUB_ROOT/expected-VoiceFlow-producer" "$app/Contents/MacOS/VoiceFlow"
    chmod +x "$app/Contents/MacOS/VoiceFlow"
    mkdir -p "$app/Contents/Resources/ThirdPartyLicenses"
    resources="$app/Contents/Resources"
    cp -R "$STUB_ROOT/ThirdPartyLicenses/DependencyTexts" "$resources/ThirdPartyLicenses/DependencyTexts"
    cp -R "$STUB_ROOT/ThirdPartyLicenses/Models" "$resources/ThirdPartyLicenses/Models"
    cp -R "$STUB_ROOT/ThirdPartyLicenses/Upstream" "$resources/ThirdPartyLicenses/Upstream"
    cp "$STUB_ROOT/THIRD_PARTY_NOTICES.md" "$resources/THIRD_PARTY_NOTICES.md"
    manifest="$resources/ThirdPartyLicenses/DependencyManifest"
    mkdir -p "$manifest"
    cp "$STUB_ROOT/src-tauri/mlx-sidecar/Package.resolved" "$manifest/Package.resolved"
    cp "$STUB_ROOT/src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" "$manifest/Xcode-Package.resolved"
    sidecar_sha="$(shasum -a 256 "$STUB_ROOT/src-tauri/mlx-sidecar/Package.resolved" | awk '{print $1}')"
    xcode_sha="$(shasum -a 256 "$STUB_ROOT/src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" | awk '{print $1}')"
    printf 'Sidecar Package.resolved SHA-256: %s\nXcode Package.resolved SHA-256: %s\n' "$sidecar_sha" "$xcode_sha" > "$manifest/dependency-identities.txt"
    case "${STUB_PROVENANCE_FAULT:-none}" in
      none) ;;
      malformed-declaration) printf 'Sidecar Package.resolved SHA-256: malformed\n' >> "$manifest/dependency-identities.txt" ;;
      *) exit 1 ;;
    esac
    mkdir -p "$app/Contents/Resources/mlx-swift_Cmlx.bundle/Contents/Resources"
    cat > "$resources/voiceflow-mlx-sidecar" <<'STUB_HELLO_WORKER_66'
#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path

FAULTS = {"none", "unsupported-null", "wrong-protocol", "bool-protocol", "cleanup-correlation", "malformed-json", "extra-jsonl", "nonzero-exit", "cleanup-timeout", "loaded-attestation", "missing-platform", "integer-platform"}
def reject(): raise SystemExit(64)
args = sys.argv[1:]
if len(args) == 4 and args[0] == "--models-root" and args[2:4] == ["--mode", "cleanup"]:
    models, mode, audio = args[1], "cleanup", None
elif len(args) == 6 and args[0] == "--models-root" and args[2:4] == ["--mode", "asr"] and args[4] == "--audio-root":
    models, mode, audio = args[1], "asr", args[5]
else: reject()
try:
    fault = os.environ.get("STUB_HELLO_FAULT", "none")
    if fault not in FAULTS: reject()
    root = Path(os.environ["STUB_ROOT"]).resolve(strict=True)
    if not root.is_dir() or root.is_symlink() or root.stat().st_uid != os.geteuid(): reject()
    record = root / ".voiceflow-notary-workdir"
    raw = record.read_text(encoding="utf-8")
    if not raw.endswith("\n") or raw.count("\n") != 1: reject()
    work = Path(raw[:-1]).resolve(strict=True)
    if (work.parent != (root / ".build/release").resolve(strict=True) or work.is_symlink()
        or work.stat().st_uid != os.geteuid()): reject()
    expected_models, expected_audio = work / "empty-models", work / "empty-audio"
    if Path(models) != expected_models or (mode == "asr" and Path(audio) != expected_audio): reject()
    for path in (expected_models, expected_audio):
        if (not path.is_dir() or path.is_symlink() or path.stat().st_uid != os.geteuid()
            or any(path.iterdir())): reject()
    line = sys.stdin.readline()
    if not line.endswith("\n") or not line or sys.stdin.readline() != "": reject()
    request = json.loads(line)
    rid = "release-asr" if mode == "asr" else "release-cleanup"
    if (not isinstance(request, dict) or set(request) != {"protocol_version", "request_id", "session_id", "model_version", "op"}
        or type(request.get("protocol_version")) is not int or request["protocol_version"] != 1
        or request.get("request_id") != rid or request.get("session_id") is not None
        or request.get("model_version") is not None or request.get("op") != "hello"): reject()
except (OSError, ValueError, KeyError, json.JSONDecodeError): reject()
with (root / "sidecar-hello.log").open("a", encoding="utf-8") as log:
    log.write(json.dumps({"mode": mode, "pid": os.getpid(), "models_root": str(expected_models),
        "audio_root": str(expected_audio) if mode == "asr" else None, "request": request, "eof": True}, separators=(",", ":")) + "\n")
    log.flush()
if fault == "cleanup-timeout" and mode == "cleanup": time.sleep(10)
response = {"protocol_version": 1, "request_id": rid, "session_id": None, "model_version": None,
    "op": "hello", "status": "ready", "platform_supported": True, "runtime_version": "1.1.0",
    "loaded_model_id": None, "loaded_model_version": None, "result": None, "cleanup_result": None,
    "warmup_result": None, "error": None}
if fault == "unsupported-null": response.update(status="unavailable", platform_supported=False, error={"code":"unsupported_platform"}, loaded_attestation=None)
if fault == "wrong-protocol" and mode == "asr": response["protocol_version"] = 2
if fault == "bool-protocol" and mode == "asr": response["protocol_version"] = True
if fault == "cleanup-correlation" and mode == "cleanup": response["request_id"] = "release-asr"
if fault == "loaded-attestation" and mode == "asr": response["loaded_attestation"] = {}
if fault == "missing-platform" and mode == "asr": del response["platform_supported"]
if fault == "integer-platform" and mode == "asr": response["platform_supported"] = 1
if fault == "malformed-json" and mode == "asr": print("{malformed", flush=True)
else:
    print(json.dumps(response, separators=(",", ":")), flush=True)
    if fault == "extra-jsonl" and mode == "asr": print("{}", flush=True)
    if fault == "nonzero-exit" and mode == "asr": raise SystemExit(39)
STUB_HELLO_WORKER_66
    chmod +x "$resources/voiceflow-mlx-sidecar"
    printf 'stub metallib\n' > "$app/Contents/Resources/mlx.metallib"
    cp "$app/Contents/Resources/mlx.metallib" \
      "$app/Contents/Resources/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
    printf '\n# MACHO-SYNTHETIC-SIDECAR\n' >> "$resources/voiceflow-mlx-sidecar"
    [[ "${STUB_NATIVE_FAULT:-none}" == missing-span ]] || printf 'MACHO-SYNTHETIC-SPAN\n' > "$resources/libswiftCompatibilitySpan.dylib"
    printf 'MACHO-SYNTHETIC-RUNTIME\n' > "$resources/libswiftCompatibilityConcurrency.dylib"
    mkdir -p "$resources/Frameworks/VoiceFlowFixture.framework"
    printf 'MACHO-SYNTHETIC-FRAMEWORK\n' > "$resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture"
    printf 'fixture text; classification only\n' > "$resources/fixture-text.txt"
  done
  printf '%s\n' "$local_root/macos/VoiceFlow-1.app" > "$STUB_APP_PATH_FILE"
  exit 0
fi
exit 1
STUB
  cat > "$fixture/bin/codesign" <<'STUB'
#!/usr/bin/env bash
printf 'codesign:%s\n' "$*" >> "$STUB_LOG"
if [[ "$#" -eq 5 && "$1 $2 $3 $4" == '--verify --deep --strict --verbose=2' \
  && ( "$5" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app" \
    || "$5" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount/VoiceFlow-1.app" ) ]]; then
  [[ "${STUB_CODESIGN_FAIL:-0}" != 1 ]] || exit 41
  exit 0
fi
if [[ "$#" -eq 3 && "$1" == -dv && "$2" == --verbose=4 \
  && ( "$3" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app" \
    || "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount/VoiceFlow-1.app" ) ]]; then
  printf 'Authority=%s\nTeamIdentifier=%s\n' "$APPLE_SIGNING_IDENTITY" "$APPLE_TEAM_ID"
  exit 0
fi
if [[ "$#" -eq 4 && "$1" == --verify && "$2" == --strict && "$3" == --verbose=2 \
  && "$4" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app/Contents/Resources/voiceflow-mlx-sidecar" ]]; then
  [[ "${STUB_CODESIGN_FAIL:-0}" != 1 ]] || exit 41
  exit 0
fi
if [[ "$#" -eq 5 && "$1" == --force && "$2" == --sign \
  && "$3" == "$APPLE_SIGNING_IDENTITY" && "$4" == --timestamp \
  && "$5" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]]; then
  [[ ! -e "$STUB_ROOT/expect-temporary-keychain" ]] || exit 91
  [[ "${STUB_DMGSIGN_FAIL:-0}" != 1 ]] || exit 59
  exit 0
fi
if [[ "$#" -eq 7 && "$1" == --force && "$2" == --sign \
  && "$3" == "$APPLE_SIGNING_IDENTITY" && "$4" == --keychain \
  && "$6" == --timestamp && "$7" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]]; then
  chain="$5"; [[ -e "$STUB_ROOT/expect-temporary-keychain" ]] || exit 91
  python3 - "$STUB_ROOT" "$chain" "$7" <<'PY_EXPECT_CODESIGN'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
chain = sys.argv[2]
dmg = pathlib.Path(sys.argv[3])
try:
    wrecord = root / ".voiceflow-notary-workdir"
    crecord = root / ".voiceflow-notary-state"
    current_path = root / "current-items"
    originals_path = root / "original-items"
    workdir = wrecord.read_text().rstrip("\n")
    recorded = crecord.read_text().rstrip("\n")
    expected = str(root / workdir[len(str(root)) + 1:] / "signing.keychain-db")
    current = current_path.read_bytes().splitlines()
    originals = originals_path.read_bytes().splitlines()
    prefix = str(root) + "/.build/release/.voiceflow-notary."
    suffix = workdir[len(prefix):] if workdir.startswith(prefix) else ""
    valid = (all(p.is_file() and not p.is_symlink() for p in
                 (wrecord, crecord, current_path, originals_path))
             and bool(suffix) and "/" not in suffix and ".." not in suffix
             and chain == recorded == expected
             and pathlib.Path(chain).is_file() and not pathlib.Path(chain).is_symlink()
             and all((root / name).is_file() and not (root / name).is_symlink()
                     for name in ("imported", "partitioned", "published"))
             and current == [chain.encode(), *originals]
             and dmg == pathlib.Path(workdir) / "VoiceFlow-notarized.dmg")
except (OSError, ValueError, IndexError):
    valid = False
sys.exit(0 if valid else 91)
PY_EXPECT_CODESIGN
  [[ "$?" -eq 0 ]] || exit 91
  printf 'codesign:DMG signing with owned keychain\n' >> "$STUB_LOG"
  [[ "${STUB_DMGSIGN_FAIL:-0}" != 1 ]] || exit 59
  exit 0
fi
if [[ "$#" -eq 3 && "$1" == --verify && "$2" == --verbose=2 \
  && "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]]; then
  [[ "${STUB_CODESIGN_FAIL:-0}" != 1 ]] || exit 41
  exit 0
fi
exit 1
STUB
  cat > "$fixture/bin/lipo" <<'STUB'
#!/usr/bin/env bash
printf 'lipo:%s\n' "$*" >> "$STUB_LOG"
[[ "$#" -eq 2 && "$1" == -archs && \
  ( "$2" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app/Contents/MacOS/VoiceFlow" \
    || "$2" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount/VoiceFlow-1.app/Contents/MacOS/VoiceFlow" ) ]] || exit 1
printf '%s\n' "${STUB_ARCHS:-arm64}"
STUB
  cat > "$fixture/bin/hdiutil" <<'STUB'
#!/usr/bin/env bash
printf 'hdiutil:%s\n' "$*" >> "$STUB_LOG"
case "$1" in
  info) [[ "$#" -eq 1 ]] || exit 1; exit 0 ;;
  verify)
    [[ "$#" -eq 2 && "$2" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]] || exit 1
    exit 0 ;;
  create)
    [[ "$#" -eq 9 && "$2 $3 $4 $5 $6 $7" == '-ov -format UDZO -volname VoiceFlow -srcfolder' \
      && "$8" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/image-root" \
      && "$9" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]] || exit 1
    [[ "${STUB_DMG_CREATE_FAIL:-0}" != 1 ]] || exit 58
    output="${@: -1}"
    source=""
    source_next=0
    for arg in "$@"; do
      if [[ "$arg" == "-srcfolder" ]]; then source_next=1; continue; fi
      if [[ "$source_next" == 1 ]]; then source="$arg"; source_next=0; fi
    done
    [[ -n "$(find "$source" -maxdepth 1 -name '*.app' -type d -print -quit)" && -L "$source/Applications" ]] || exit 51
    python3 - "$source" "$output" <<'PY'
import io,sys,tarfile
buf=io.BytesIO()
with tarfile.open(fileobj=buf,mode="w") as archive:
 archive.add(sys.argv[1],arcname=".",recursive=True)
open(sys.argv[2],"wb").write(b"IMAGE-MARKER\n"+buf.getvalue()+b"\nIMAGE-END\n")
PY
    ;;
  attach)
    [[ "$#" -eq 6 && "$2" == -nobrowse && "$3" == -readonly && "$4" == -mountpoint \
      && "$5" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount" \
      && "$6" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ]] || exit 1
    mountpoint=""
    while [[ "$#" -gt 0 ]]; do
      if [[ "$1" == "-mountpoint" ]]; then shift; mountpoint="$1"; break; fi
      shift
    done
    dmg="${@: -1}"
    python3 - "$dmg" "$STUB_ROOT/image-app-marker" <<'PY'
import io,sys,tarfile
b=open(sys.argv[1],"rb").read(); prefix=b"IMAGE-MARKER\n"; end=b"\nIMAGE-END\n"
assert b.startswith(prefix) and end in b
payload=b[len(prefix):].split(end,1)[0]
with tarfile.open(fileobj=io.BytesIO(payload),mode="r:") as archive:
 import os
 root=sys.argv[2] + ".dir"
 os.makedirs(root,exist_ok=True)
 all_members=archive.getmembers()
 assert all(not os.path.isabs(m.name) and ".." not in m.name.split("/") for m in all_members)
 app_roots=[m.name for m in all_members if m.name.endswith(".app")]
 assert len(app_roots)==1
 app_root=app_roots[0]
 apps=[m for m in all_members if m.name == app_root or m.name.startswith(app_root + "/")]
 links=[m for m in archive.getmembers() if m.name.rstrip("/") == "./Applications" or m.name.rstrip("/") == "Applications"]
 assert len(links)==1 and links[0].issym() and links[0].linkname=="/Applications"
 assert apps and all(m.name.startswith("./") for m in apps)
 archive.extractall(root,members=apps)
 os.symlink(links[0].linkname,os.path.join(root,"Applications"))
PY
    mkdir -p "$mountpoint"
    image_root="$STUB_ROOT/image-app-marker.dir"
    [[ -L "$image_root/Applications" ]] || exit 60
    image_app="$(find "$image_root" -maxdepth 1 -name '*.app' -type d -print -quit)"
    [[ -n "$image_app" ]] || exit 61
    cp -R "$image_app" "$mountpoint/"
    tamper="${STUB_MOUNT_TAMPER:-none}"
    case "$tamper" in
      none) ;;
      notice|models|upstream-path|sidecar-lock|xcode-lock)
        python3 - "$mountpoint/VoiceFlow-1.app/Contents/Resources" "$tamper" <<'PY_MOUNT_TAMPER_66'
import sys
from pathlib import Path
resources, kind = Path(sys.argv[1]), sys.argv[2]
if kind == "notice":
    (resources / "THIRD_PARTY_NOTICES.md").write_text("Tampered mounted fixture notice\\n")
elif kind == "models":
    (resources / "ThirdPartyLicenses/Models/stub-model-notice.txt").write_text("Tampered mounted fixture model notice\\n")
elif kind == "upstream-path":
    source = resources / "ThirdPartyLicenses/Upstream/nested/NOTICE.txt"
    source.rename(source.with_name("RENAMED-NOTICE.txt"))
elif kind == "sidecar-lock":
    (resources / "ThirdPartyLicenses/DependencyManifest/Package.resolved").write_bytes(b"tampered mounted sidecar lock\\n")
elif kind == "xcode-lock":
    (resources / "ThirdPartyLicenses/DependencyManifest/Xcode-Package.resolved").write_bytes(b"tampered mounted xcode lock\\n")
else:
    raise SystemExit(64)
PY_MOUNT_TAMPER_66
        ;;
      *) exit 1 ;;
    esac
    ;;
  detach)
    [[ "$#" -eq 3 && "$2" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount" && "$3" == -quiet ]] || exit 1
    if [[ "${STUB_DETACH_FAIL:-0}" == "1" ]]; then exit 45; fi
    exit 0
    ;;
  *) exit 1 ;;
esac
STUB
  cat > "$fixture/bin/xcrun" <<'STUB'
#!/usr/bin/env bash
printf 'xcrun:%s\n' "$*" >> "$STUB_LOG"
if [[ "$#" -eq 12 && "$1" == notarytool && "$2" == submit \
  && "$4" == --apple-id && "$6" == --password && "$8" == --team-id \
  && "${10}" == --wait && "${11}" == --output-format && "${12}" == json \
  && ( "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow.zip" \
    || "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ) ]]; then
  n="$(cat "$STUB_ROOT/submit-count" 2>/dev/null || echo 0)"; n=$((n+1)); printf '%s' "$n" > "$STUB_ROOT/submit-count"
  artifact="${@:3:1}"
  [[ -f "$artifact" ]] || exit 52
  python3 - "$artifact" "$STUB_ROOT/submit-$n.sha" <<'PY'
import hashlib,sys
open(sys.argv[2],"w").write(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())
PY
  cp "$artifact" "$STUB_ROOT/submit-$n.artifact"
  [[ "${STUB_NOTARY_FAIL:-0}" != "$n" && "${STUB_NOTARY_FAIL:-0}" != all ]] || exit 1
  status="${STUB_NOTARY_STATUS:-Accepted}"
  [[ "$n" != 1 || -z "${STUB_APP_STATUS:-}" ]] || status="$STUB_APP_STATUS"
  [[ "$n" != 2 || -z "${STUB_DMG_STATUS:-}" ]] || status="$STUB_DMG_STATUS"
  json_mode="${STUB_NOTARY_JSON:-valid}"
  [[ "$n" != 2 || -z "${STUB_DMG_NOTARY_JSON:-}" ]] || json_mode="$STUB_DMG_NOTARY_JSON"
  case "$json_mode" in
    malformed) printf '{bad json\n' ;;
    missing-id) printf '{"status":"%s"}\n' "$status" ;;
    *) printf '{"id":"stub-submission-%s","status":"%s"}\n' "$n" "$status" ;;
  esac
elif [[ "$#" -eq 3 && "$1" == stapler && "$2" == staple \
  && ( "$3" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app" \
    || "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" ) ]]; then
  [[ "${STUB_STAPLE_FAIL:-}" != "$(basename "$3")" && "${STUB_STAPLE_FAIL:-}" != all ]] || exit 53
  if [[ "$3" == *.dmg ]]; then printf '\nDMG-TICKET\n' >> "$3"; else printf '\nAPP-TICKET\n' >> "$3/Contents/MacOS/VoiceFlow"; fi
elif [[ "$#" -eq 3 && "$1" == stapler && "$2" == validate \
  && ( "$3" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app" \
    || "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow-notarized.dmg" \
    || "$3" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount/VoiceFlow-1.app" ) ]]; then
  [[ "${STUB_VALIDATE_FAIL:-}" != "$(basename "$3")" && "${STUB_VALIDATE_FAIL:-}" != all ]] || exit 54
  if [[ "$3" == *.dmg ]]; then rg -q 'DMG-TICKET' "$3" || exit 55; else rg -q 'APP-TICKET' "$3/Contents/MacOS/VoiceFlow" || exit 56; fi
else
  exit 1
fi
STUB
  cat > "$fixture/bin/ditto" <<'STUB'
#!/usr/bin/env bash
printf 'ditto:%s\n' "$*" >> "$STUB_LOG"
[[ "$#" -eq 6 && "$1 $2 $3 $4" == '-c -k --sequesterRsrc --keepParent' \
  && "$5" == "$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app" \
  && "$6" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/VoiceFlow.zip" ]] || exit 1
[[ "${STUB_DITTO_FAIL:-0}" != 1 ]] || exit 57
source="${@: -2:1}"; output="${@: -1}"
python3 - "$source" "$output" <<'PY'
import sys,zipfile,os
with zipfile.ZipFile(sys.argv[2],"w") as z:
 for base,_,files in os.walk(sys.argv[1]):
  for f in files:
   p=os.path.join(base,f); z.write(p,os.path.relpath(p,os.path.dirname(sys.argv[1])))
PY
STUB
  cat > "$fixture/bin/spctl" <<'STUB'
#!/usr/bin/env bash
printf 'spctl:%s\n' "$*" >> "$STUB_LOG"
[[ "$#" -eq 5 && "$1 $2 $3 $4" == '--assess --type execute --verbose=4' \
  && "$5" == "$STUB_ROOT/.build/release/.voiceflow-notary."*"/mount/VoiceFlow-1.app" ]] || exit 1
if [[ "${STUB_SPCTL_FAIL:-0}" == "1" ]]; then exit 46; fi
exit 0
STUB
  cat > "$fixture/bin/security" <<'STUB'
#!/usr/bin/env bash
if [[ "$#" -eq 5 && "$1" == find-identity && "$2" == -v && "$3" == -p \
  && "$4" == codesigning && -z "$5" ]]; then exit 1; fi
if [[ "$#" -eq 4 && "$1" == find-identity && "$2" == -v && "$3" == -p \
  && "$4" == codesigning ]]; then
  case "${STUB_IDENTITY_RESPONSE:-empty}" in
    empty) exit 0 ;;
    zero) printf '0 valid identities found\n'; exit 0 ;;
    valid) printf '1) ABCDEF1234567890 \"Developer ID Application: Stub (ABCDE12345)\"\n'; exit 0 ;;
    *) exit 1 ;;
  esac
fi
state="$STUB_ROOT/.voiceflow-notary-state"
owned=''
if [[ -f "$state" ]]; then IFS= read -r owned < "$state"; fi
current="$STUB_ROOT/current-items"
if [[ ! -e "$current" ]]; then
  if [[ -f "$STUB_ROOT/original-items" ]]; then cp "$STUB_ROOT/original-items" "$current"; else : > "$current"; fi
fi
write_items() {
  : > "$current"
  for item in "$@"; do printf '%s\n' "$item" >> "$current"; done
}
remove_owned_item() {
  local item
  local -a kept=()
  if [[ -f "$current" ]]; then
    while IFS= read -r item || [[ -n "$item" ]]; do
      [[ "$item" == "$owned" ]] || kept+=("$item")
    done < "$current"
  fi
  write_items "${kept[@]}"
}
fail_phase() {
  case "${STUB_NOTARY_PHASE_FAIL:-none}:$1" in
    snapshot:snapshot) exit 71 ;;
    create-empty:create|create-partial:create) exit 72 ;;
    settings:settings) exit 73 ;;
    unlock:unlock) exit 74 ;;
    import:import) exit 75 ;;
    partition:partition) exit 76 ;;
    publish:publish) exit 77 ;;
  esac
}
if [[ "$#" -eq 3 && "$1" == list-keychains && "$2" == -d && "$3" == user ]]; then
  printf 'security:list-keychains\n' >> "$STUB_LOG"; fail_phase snapshot
  while IFS= read -r item || [[ -n "$item" ]]; do printf '    "%s"\n' "$item"; done < "$current"
  exit 0
fi
if [[ "$#" -eq 4 && "$1" == create-keychain && "$2" == -p ]]; then
  key="$3"; chain="$4"; record="$STUB_ROOT/.voiceflow-notary-workdir"
  [[ -f "$record" && ! -L "$record" ]] || exit 91
  IFS= read -r workdir < "$record" || exit 91
  base="$STUB_ROOT/.build/release/.voiceflow-notary."
  [[ -n "$workdir" && "$workdir" == "$base"* ]] || exit 91
  work_suffix="${workdir#"$base"}"; parent="${workdir%/*}"; resolved="$(cd "$workdir" 2>/dev/null && pwd -P)" || exit 91
  [[ -n "$work_suffix" && "$work_suffix" != *'/'* && "$work_suffix" != *'..'* \
    && "$parent" == "$STUB_ROOT/.build/release" && -d "$workdir" && ! -L "$workdir" \
    && "$resolved" == "$workdir" && "$chain" == "$workdir/signing.keychain-db" \
    && "$key" == STUB-K-9f1c2a7e ]] || exit 91
  [[ ! -e "$chain" && ! -L "$chain" ]] || exit 91
  printf '%s\n' "$PPID" > "$STUB_ROOT/.voiceflow-notary-creator-pid"
  printf '%s\n' "$chain" > "$state"
  printf 'security:create-keychain [redacted]\n' >> "$STUB_LOG"
  local_items=()
  local_items+=("$chain")
  while IFS= read -r item || [[ -n "$item" ]]; do local_items+=("$item"); done < "$current"
  write_items "${local_items[@]}"
  rm -f "$STUB_ROOT/settings" "$STUB_ROOT/unlocked" "$STUB_ROOT/imported" "$STUB_ROOT/partitioned" "$STUB_ROOT/published"
  case "${STUB_NOTARY_PHASE_FAIL:-none}" in
    create-empty) exit 72 ;;
    create-partial) : > "$chain"; exit 72 ;;
  esac
  : > "$chain"; exit 0
fi
if [[ "$#" -eq 2 && "$1" == set-keychain-settings ]]; then
  [[ -n "$owned" && "$2" == "$owned" && -f "$owned" && ! -L "$owned" ]] || exit 91
  printf 'security:set-keychain-settings\n' >> "$STUB_LOG"; fail_phase settings; : > "$STUB_ROOT/settings"; exit 0
fi
if [[ "$#" -eq 4 && "$1" == unlock-keychain && "$2" == -p ]]; then
  [[ -n "$owned" && "$3" == STUB-K-9f1c2a7e && "$4" == "$owned" && -f "$owned" && ! -L "$owned" && -f "$STUB_ROOT/settings" ]] || exit 91
  printf 'security:unlock-keychain [redacted]\n' >> "$STUB_LOG"; fail_phase unlock; : > "$STUB_ROOT/unlocked"; exit 0
fi
if [[ "$#" -eq 8 && "$1" == import && "$3" == -k && "$5" == -P && "$7" == -T && "$8" == /usr/bin/codesign ]]; then
  p12="$2"; chain="$4"; pw="$6"
  [[ -n "$owned" && "$chain" == "$owned" && -f "$chain" && ! -L "$chain" \
    && "$p12" == "${owned%/*}/identity.p12" && -f "$p12" && ! -L "$p12" \
    && "$pw" == STUB-P-43b8d0c1 && -f "$STUB_ROOT/unlocked" ]] || exit 91
  python3 - "$p12" <<'PY_BLOB_CHECK'
import pathlib, sys
try:
    valid = pathlib.Path(sys.argv[1]).read_bytes() == b"STUB-BLOB-7041c9f6"
except OSError:
    valid = False
sys.exit(0 if valid else 91)
PY_BLOB_CHECK
  [[ "$?" -eq 0 ]] || exit 91
  printf 'security:import [redacted]\n' >> "$STUB_LOG"; fail_phase import; : > "$STUB_ROOT/imported"
  case "${STUB_NOTARY_AFTER_IMPORT_SIGNAL:-none}" in
    INT|TERM)
      creator_record="$STUB_ROOT/.voiceflow-notary-creator-pid"
      [[ -f "$creator_record" && ! -L "$creator_record" ]] || exit 91
      IFS= read -r creator_pid < "$creator_record" || exit 91
      [[ "$creator_pid" =~ ^[0-9]+$ && "$creator_pid" -gt 1 && "$creator_pid" == "$PPID" ]] || exit 91
      kill -"${STUB_NOTARY_AFTER_IMPORT_SIGNAL}" "$creator_pid" || exit 91
      ;;
    none) ;;
    *) exit 91 ;;
  esac
  exit 0
fi
if [[ "$#" -eq 7 && "$1" == set-key-partition-list && "$2" == -S && "$3" == apple-tool:,apple:,codesign: && "$4" == -s && "$5" == -k ]]; then
  key="$6"; chain="$7"; [[ -n "$owned" && "$chain" == "$owned" && "$key" == STUB-K-9f1c2a7e && -f "$owned" && -f "$STUB_ROOT/imported" ]] || exit 91
  printf 'security:set-key-partition-list [redacted]\n' >> "$STUB_LOG"; fail_phase partition; : > "$STUB_ROOT/partitioned"; exit 0
fi
if [[ "$#" -ge 4 && "$1" == list-keychains && "$2" == -d && "$3" == user && "$4" == -s ]]; then
  shift 4
  if [[ "$#" -gt 0 && -n "$owned" && "$1" == "$owned" ]]; then
    [[ -f "$owned" && -f "$STUB_ROOT/partitioned" ]] || exit 91
    shift
    if [[ -f "$STUB_ROOT/original-items" ]]; then
      python3 - "$STUB_ROOT/original-items" "$@" <<'PY'
import sys
expected=open(sys.argv[1]).read().splitlines()
if expected != sys.argv[2:]:
    sys.exit(91)
PY
      [[ "$?" -eq 0 ]] || exit 91
    elif [[ "$#" -ne 0 ]]; then exit 91; fi
    printf 'security:list-keychains publish\n' >> "$STUB_LOG"; fail_phase publish; : > "$STUB_ROOT/published"; write_items "$owned" "$@"; exit 0
  fi
  if [[ -f "$STUB_ROOT/original-items" ]]; then
    python3 - "$STUB_ROOT/original-items" "$@" <<'PY'
import sys
expected=open(sys.argv[1]).read().splitlines()
if expected != sys.argv[2:]:
    sys.exit(91)
PY
    [[ "$?" -eq 0 ]] || exit 91
  elif [[ "$#" -ne 0 ]]; then exit 91; fi
  printf 'security:list-keychains restore\n' >> "$STUB_LOG"
  case "${STUB_NOTARY_CLEANUP_FAIL:-none}" in restore|both) exit 81 ;; esac
  if [[ -f "$STUB_ROOT/original-items" ]]; then cp "$STUB_ROOT/original-items" "$current"; else : > "$current"; fi
  exit 0
fi
if [[ "$#" -eq 2 && "$1" == delete-keychain ]]; then
  chain="$2"; [[ -n "$owned" && "$chain" == "$owned" && ( -e "$chain" || ! -e "$chain" ) ]] || exit 91
  printf 'security:delete-keychain\n' >> "$STUB_LOG"
  case "${STUB_NOTARY_CLEANUP_FAIL:-none}" in delete|both) exit 80 ;; esac
  rm -f "$chain" "$STUB_ROOT/settings" "$STUB_ROOT/unlocked" "$STUB_ROOT/imported" "$STUB_ROOT/partitioned" "$STUB_ROOT/published"
  remove_owned_item
  exit 0
fi
exit 91
STUB
  cat > "$fixture/bin/openssl" <<'STUB'
#!/usr/bin/env bash
if [[ "$#" -eq 3 && "$1" == x509 && "$2" == -noout && "$3" == -subject ]]; then exit 0; fi
if [[ "$#" -eq 3 && "$1" == rand && "$2" == -hex && "$3" == 20 ]]; then
  printf 'openssl:rand -hex 20\n' >> "$STUB_LOG"
  [[ "${STUB_NOTARY_PHASE_FAIL:-none}" != rand ]] || exit 70
  printf 'STUB-K-9f1c2a7e\n'; exit 0
fi
exit 1
STUB
  cat > "$fixture/bin/file" <<'STUB'
#!/usr/bin/env python3
import os
import pathlib
import stat
import sys


FAULTS = {
    "none", "leak-main", "leak-sidecar", "leak-span", "leak-runtime",
    "leak-framework", "mounted-leak-framework", "mounted-leak-runtime",
    "strings-partial-fail", "rg-error", "file-io-extra", "find-fail",
    "missing-span", "nonmacho-span",
}
ROLES = (
    "MacOS/VoiceFlow",
    "Resources/voiceflow-mlx-sidecar",
    "Resources/libswiftCompatibilitySpan.dylib",
    "Resources/libswiftCompatibilityConcurrency.dylib",
    "Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture",
)


def fail():
    raise SystemExit(1)


def physical_directory(path):
    path = pathlib.Path(path)
    if not path.is_absolute():
        fail()
    current = pathlib.Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            fail()
    if path.resolve(strict=True) != path or path.stat().st_uid != os.getuid():
        fail()
    return path


def regular_canonical(path):
    path = pathlib.Path(path)
    if not path.is_absolute():
        fail()
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        fail()
    if path.resolve(strict=True) != path or info.st_uid != os.getuid():
        fail()
    # Check every parent component so an owned candidate cannot traverse a symlink.
    parent = path.parent
    while parent != parent.parent:
        if stat.S_ISLNK(parent.lstat().st_mode):
            fail()
        parent = parent.parent
    return path


def main():
    args = sys.argv[1:]
    fault = os.environ.get("STUB_NATIVE_FAULT", "none")
    root_value = os.environ.get("STUB_ROOT", "")
    root = pathlib.Path(root_value)
    root = physical_directory(root)
    work_record = root / ".voiceflow-notary-workdir"
    record_info = work_record.lstat()
    if stat.S_ISLNK(record_info.st_mode) or not stat.S_ISREG(record_info.st_mode) or record_info.st_uid != os.getuid():
        fail()
    work_bytes = work_record.read_bytes()
    if not work_bytes.endswith(b"\n") or work_bytes.count(b"\n") != 1 or b"\0" in work_bytes:
        fail()
    work_value = work_bytes[:-1].decode("utf-8")
    if not work_value or not pathlib.Path(work_value).is_absolute():
        fail()
    work = physical_directory(work_value)
    build_release = physical_directory(root / ".build" / "release")
    if work.parent != build_release or not work.name.startswith(".voiceflow-notary.") or not work.name[len(".voiceflow-notary."):]:
        fail()
    expected = os.environ.get("STUB_EXPECTED_TARGET", "")
    if not expected or "/" in expected or expected in (".", ".."):
        fail()
    built = root / "src-tauri" / "target" / expected / "release" / "bundle" / "macos" / "VoiceFlow-1.app" / "Contents"
    mounted = work / "mount" / "VoiceFlow-1.app" / "Contents"
    phase = "prezip"
    if args:
        logged_candidate = pathlib.Path(args[-1])
        try:
            if os.path.commonpath((str(mounted), str(logged_candidate))) == str(mounted):
                phase = "mounted"
        except ValueError:
            pass
    with open(os.environ["STUB_LOG"], "a", encoding="utf-8") as log:
        log.write("file:" + " ".join(args) + " fault=" + fault + " phase=" + phase + "\n")
    if fault not in FAULTS:
        fail()
    if len(args) == 3 and args[0:2] == ["-E", "-b"]:
        candidate_value = args[2]
        extended = True
    elif len(args) == 2 and args[0] == "-b" and fault == "file-io-extra":
        candidate_value = args[1]
        extended = False
    else:
        fail()
    candidate = regular_canonical(candidate_value)
    owner = None
    for app_contents in (built, mounted):
        try:
            if os.path.commonpath((str(app_contents), str(candidate))) == str(app_contents) and candidate != app_contents:
                owner = app_contents
                break
        except ValueError:
            pass
    if owner is None:
        fail()
    relative = candidate.relative_to(owner).as_posix()
    if relative not in ROLES and not candidate.is_file():
        fail()
    if fault == "file-io-extra" and relative == "Resources/libswiftCompatibilityConcurrency.dylib":
        if extended:
            sys.stderr.write("fixture classification I/O error\n")
            raise SystemExit(2)
        print("regular file, filesystem error")
        return
    if fault == "nonmacho-span" and relative == "Resources/libswiftCompatibilitySpan.dylib":
        print("ASCII text")
    elif relative in ROLES:
        print("Mach-O 64-bit fixture")
    else:
        print("ASCII text")


if __name__ == "__main__":
    main()
STUB
cat > "$fixture/bin/find" <<'STUB'
#!/usr/bin/env bash
python3 - "$@" <<'PY_FIND'
import os, pathlib, stat, subprocess, sys

def fail():
    raise SystemExit(1)

def physical_dir(path):
    p = pathlib.Path(path)
    if not p.is_absolute(): fail()
    cur = pathlib.Path(p.anchor)
    for part in p.parts[1:]:
        cur = cur / part
        st = cur.lstat()
        if stat.S_ISLNK(st.st_mode) or not stat.S_ISDIR(st.st_mode): fail()
    if p.resolve(strict=True) != p or p.stat().st_uid != os.geteuid(): fail()
    return p

def canonical_file(path):
    p = pathlib.Path(path)
    st = p.lstat()
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode) or st.st_uid != os.geteuid() or p.resolve(strict=True) != p: fail()
    physical_dir(p.parent)
    return p

try:
    args = sys.argv[1:]
    root = physical_dir(pathlib.Path(os.environ["STUB_ROOT"]))
    record = root / ".voiceflow-notary-workdir"
    canonical_file(record)
    raw = record.read_bytes()
    if b"\x00" in raw or not raw.endswith(b"\n") or raw.count(b"\n") != 1: fail()
    work = pathlib.Path(os.fsdecode(raw[:-1]))
    build = root / ".build"
    release = build / "release"
    physical_dir(build); physical_dir(release)
    if not work.is_absolute() or work.parent != release or not work.name.startswith(".voiceflow-notary.") or not work.name[len(".voiceflow-notary."):] or "/" in work.name: fail()
    physical_dir(work)
    fault = os.environ.get("STUB_NATIVE_FAULT", "none")
    allowed = {"none", "leak-main", "leak-sidecar", "leak-span", "leak-runtime", "leak-framework", "mounted-leak-framework", "mounted-leak-runtime", "strings-partial-fail", "rg-error", "file-io-extra", "find-fail", "missing-span", "nonmacho-span"}
    if fault not in allowed: fail()
    target = os.environ["STUB_EXPECTED_TARGET"]
    if not target or "/" in target or target in (".", ".."): fail()
    built_contents = root / "src-tauri" / "target" / target / "release/bundle/macos/VoiceFlow-1.app/Contents"
    mounted_contents = work / "mount/VoiceFlow-1.app/Contents"
    if len(args) == 9 and args[1:] == ["-maxdepth", "1", "-name", "*.app", "-type", "d", "-print", "-quit"] and pathlib.Path(args[0]) in (work / "image-root", root / "image-app-marker.dir"):
        phase = "prezip"
        searchroot = pathlib.Path(args[0])
        physical_dir(searchroot)
        app = searchroot / "VoiceFlow-1.app"
        physical_dir(app)
        with open(os.environ["STUB_LOG"], "a", encoding="utf-8") as f:
            f.write("find:" + " ".join(args) + " fault=" + fault + " phase=" + phase + "\n")
        if fault == "find-fail":
            sys.stdout.write(str(app) + "\n"); raise SystemExit(2)
        cmd = ["/usr/bin/find", str(searchroot), "-maxdepth", "1", "-name", "*.app", "-type", "d", "-print", "-quit"]
    elif len(args) == 4 and args[1:] == ["-type", "f", "-print0"]:
        contents = pathlib.Path(args[0])
        if contents not in (built_contents, mounted_contents): fail()
        phase = "mounted" if contents == mounted_contents else "prezip"
        physical_dir(contents)
        with open(os.environ["STUB_LOG"], "a", encoding="utf-8") as f:
            f.write("find:" + " ".join(args) + " fault=" + fault + " phase=" + phase + "\n")
        if fault == "find-fail":
            sys.stdout.buffer.write(os.fsencode(contents / "MacOS/VoiceFlow") + b"\0"); raise SystemExit(2)
        cmd = ["/usr/bin/find", str(contents), "-type", "f", "-print0"]
    else: fail()
    result = subprocess.run(cmd, stdout=sys.stdout.buffer, stderr=sys.stderr.buffer, check=False)
    raise SystemExit(result.returncode)
except (OSError, KeyError, ValueError, UnicodeError):
    fail()
PY_FIND
STUB
  chmod +x "$fixture/bin/find"
  cat > "$fixture/bin/strings" <<'STUB'
#!/usr/bin/env bash
[[ "$#" -eq 2 && "$1" == -a ]] || exit 1
candidate="$2"
fault="${STUB_NATIVE_FAULT:-none}"
case "$fault" in
  none|leak-main|leak-sidecar|leak-span|leak-runtime|leak-framework|mounted-leak-framework|mounted-leak-runtime|strings-partial-fail|rg-error|file-io-extra|find-fail|missing-span|nonmacho-span) ;;
  *) exit 2 ;;
esac
work="$(python3 -B - "$STUB_ROOT" <<'PY'
import os
import pathlib
import stat
import sys

root_arg = pathlib.Path(sys.argv[1])
if not root_arg.is_absolute() or root_arg != pathlib.Path(os.path.realpath(root_arg)):
    raise SystemExit(1)
root_stat = root_arg.lstat()
if not stat.S_ISDIR(root_stat.st_mode) or root_stat.st_uid != os.getuid():
    raise SystemExit(1)
record = root_arg / ".voiceflow-notary-workdir"
record_stat = record.lstat()
if not stat.S_ISREG(record_stat.st_mode) or record_stat.st_uid != os.getuid():
    raise SystemExit(1)
record_bytes = record.read_bytes()
if not record_bytes.endswith(b"\n") or record_bytes.count(b"\n") != 1 or b"\0" in record_bytes:
    raise SystemExit(1)
work_text = record_bytes[:-1].decode("utf-8")
if not work_text or not work_text.startswith("/") or "\n" in work_text:
    raise SystemExit(1)
work = pathlib.Path(work_text)
if work != root_arg / ".build" / "release" / work.name or not work.name.startswith(".voiceflow-notary.") or len(work.name) == len(".voiceflow-notary."):
    raise SystemExit(1)
for path in (root_arg / ".build", root_arg / ".build" / "release", work):
    item = path.lstat()
    if not stat.S_ISDIR(item.st_mode) or item.st_uid != os.getuid() or path != pathlib.Path(os.path.realpath(path)):
        raise SystemExit(1)
print(work_text)
PY
)" || exit 1
[[ -n "$work" ]] || exit 1
[[ -n "${STUB_EXPECTED_TARGET:-}" && "$STUB_EXPECTED_TARGET" != */* && "$STUB_EXPECTED_TARGET" != . && "$STUB_EXPECTED_TARGET" != .. ]] || exit 1
built="$STUB_ROOT/src-tauri/target/${STUB_EXPECTED_TARGET}/release/bundle/macos/VoiceFlow-1.app/Contents"
mounted="$work/mount/VoiceFlow-1.app/Contents"
case "$candidate" in
  "$built/MacOS/VoiceFlow"|"$mounted/MacOS/VoiceFlow") role=main ;;
  "$built/Resources/voiceflow-mlx-sidecar"|"$mounted/Resources/voiceflow-mlx-sidecar") role=sidecar ;;
  "$built/Resources/libswiftCompatibilitySpan.dylib"|"$mounted/Resources/libswiftCompatibilitySpan.dylib") role=span ;;
  "$built/Resources/libswiftCompatibilityConcurrency.dylib"|"$mounted/Resources/libswiftCompatibilityConcurrency.dylib") role=runtime ;;
  "$built/Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture"|"$mounted/Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture") role=framework ;;
  *) exit 1 ;;
esac
phase=prezip
[[ "$candidate" == "$mounted"/* ]] && phase=mounted
python3 -B - "$STUB_ROOT" "$work" "$built" "$mounted" "$candidate" <<'PY'
import os
import pathlib
import stat
import sys

root, work, built, mounted, candidate = map(pathlib.Path, sys.argv[1:])
try:
    relative = candidate.relative_to(built)
    current_owner = built
except ValueError:
    relative = candidate.relative_to(mounted)
    current_owner = mounted
for base in (root, work, current_owner):
    if not base.is_absolute() or base != pathlib.Path(os.path.realpath(base)):
        raise SystemExit(1)
    current = pathlib.Path(base.anchor)
    for part in base.parts[1:]:
        current /= part
        item = current.lstat()
        if stat.S_ISLNK(item.st_mode):
            raise SystemExit(1)
        if current == base and (not stat.S_ISDIR(item.st_mode) or item.st_uid != os.getuid()):
            raise SystemExit(1)
allowed = {
    pathlib.Path("MacOS/VoiceFlow"),
    pathlib.Path("Resources/voiceflow-mlx-sidecar"),
    pathlib.Path("Resources/libswiftCompatibilitySpan.dylib"),
    pathlib.Path("Resources/libswiftCompatibilityConcurrency.dylib"),
    pathlib.Path("Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture"),
}
if relative not in allowed:
    raise SystemExit(1)
item = candidate.lstat()
if not stat.S_ISREG(item.st_mode) or item.st_uid != os.getuid() or candidate != pathlib.Path(os.path.realpath(candidate)):
    raise SystemExit(1)
PY
validation_rc=$?
[[ "$validation_rc" -eq 0 ]] || exit "$validation_rc"
if ! printf 'strings:%s fault=%s phase=%s\n' "$*" "$fault" "$phase" >> "$STUB_LOG"; then exit 1; fi
if [[ "$fault" == "leak-$role" || ( "$phase" == mounted && "$fault" == mounted-leak-$role && ( "$role" == framework || "$role" == runtime ) ) ]]; then
  printf '/Users/fixture/private\n'
  exit $?
fi
/bin/cat "$candidate"
cat_rc=$?
if [[ "$cat_rc" -ne 0 ]]; then exit "$cat_rc"; fi
if [[ "$fault" == strings-partial-fail ]]; then exit 3; fi
exit 0
STUB
  cat > "$fixture/bin/rg" <<'STUB'
#!/usr/bin/env python3
import os
import pathlib
import stat
import subprocess
import sys


def reject():
    raise SystemExit(1)


FAULTS = {
    "none", "leak-main", "leak-sidecar", "leak-span", "leak-runtime",
    "leak-framework", "mounted-leak-framework", "mounted-leak-runtime",
    "strings-partial-fail", "rg-error", "file-io-extra", "find-fail",
    "missing-span", "nonmacho-span",
}


def physical_directory(path, uid, owned=True):
    if not path.is_absolute():
        reject()
    current = pathlib.Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            reject()
    if path.resolve(strict=True) != path or (owned and path.stat().st_uid != uid):
        reject()
    return path


def regular_candidate(path, uid):
    if not path.is_absolute():
        reject()
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        reject()
    if path.resolve(strict=True) != path or info.st_uid != uid:
        reject()
    physical_directory(path.parent, uid, owned=False)
    return path


def main():
    args = sys.argv[1:]
    uid = os.getuid()
    root_value = os.environ["STUB_ROOT"]
    root = pathlib.Path(root_value)
    if not root.is_absolute() or root_value != str(root) or root.resolve(strict=True) != root:
        reject()
    physical_directory(root, uid)
    record = root / ".voiceflow-notary-workdir"
    info = record.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or info.st_uid != uid:
        reject()
    raw = record.read_bytes()
    if b"\0" in raw or not raw.endswith(b"\n") or raw.count(b"\n") != 1:
        reject()
    raw_work = raw[:-1].decode("utf-8")
    work = pathlib.Path(raw_work)
    if not work.is_absolute() or raw_work != str(work) or work.name in (".", "..") or not work.name.startswith(".voiceflow-notary.") or not work.name[len(".voiceflow-notary."):] or "/" in work.name[len(".voiceflow-notary."):]:
        reject()
    build = physical_directory(root / ".build", uid)
    release = physical_directory(build / "release", uid)
    if work.parent != release or work.resolve(strict=True) != work or work.is_symlink():
        reject()
    physical_directory(work, uid)

    target = os.environ["STUB_EXPECTED_TARGET"]
    if not target or "/" in target or target in (".", ".."):
        reject()
    built_contents = root / "src-tauri" / "target" / target / "release/bundle/macos/VoiceFlow-1.app/Contents"
    mounted_contents = work / "mount/VoiceFlow-1.app/Contents"
    built_main = built_contents / "MacOS/VoiceFlow"
    mounted_main = mounted_contents / "MacOS/VoiceFlow"
    dmg = work / "VoiceFlow-notarized.dmg"
    scan_paths = {
        work / "production-entry-prezip/native-strings.txt": "prezip",
        work / "production-entry-mounted/native-strings.txt": "mounted",
    }
    scan_paths = {str(path): phase for path, phase in scan_paths.items()}
    fault = os.environ.get("STUB_NATIVE_FAULT", "none")
    if fault not in FAULTS:
        reject()
    phase = None
    if len(args) == 4 and args[:3] == ["-n", "-F", "/Users/"]:
        if args[3] not in scan_paths:
            reject()
        candidate = pathlib.Path(args[3])
        phase = scan_paths[args[3]]
        regular_candidate(candidate, uid)
        if fault == "rg-error":
            status = 2
            output = b""
        else:
            result = subprocess.run(["/usr/bin/grep", "-nF", "--", "/Users/", str(candidate)], check=False)
            status, output = result.returncode, None
    elif len(args) == 3 and args[0] == "-q" and args[1] == "APP-TICKET":
        if args[2] == str(built_main):
            candidate = built_main
            phase = "prezip"
        elif args[2] == str(mounted_main):
            candidate = mounted_main
            phase = "mounted"
        else:
            reject()
        regular_candidate(candidate, uid)
        result = subprocess.run(["/usr/bin/grep", "-qF", "--", "APP-TICKET", str(candidate)], check=False)
        status, output = result.returncode, None
    elif len(args) == 3 and args[0] == "-q" and args[1] == "DMG-TICKET" and args[2] == str(dmg):
        phase = "prezip"
        regular_candidate(dmg, uid)
        result = subprocess.run(["/usr/bin/grep", "-qF", "--", "DMG-TICKET", str(dmg)], check=False)
        status, output = result.returncode, None
    else:
        reject()

    if status not in (0, 1, 2):
        status = 2
    try:
        log = pathlib.Path(os.environ["STUB_LOG"])
        with log.open("a", encoding="utf-8") as stream:
            stream.write("rg:" + " ".join(args) + " fault=" + fault + " phase=" + phase + "\n")
    except OSError:
        raise SystemExit(2)
    if output:
        sys.stdout.buffer.write(output)
    raise SystemExit(status)


try:
    main()
except (OSError, KeyError, UnicodeError, ValueError, subprocess.SubprocessError):
    raise SystemExit(2)
STUB
  cat > "$fixture/bin/gh" <<'STUB'
#!/usr/bin/env bash
printf 'gh: %s\n' "$*" >> "$STUB_LOG" || exit 1
exit 1
STUB
  chmod +x "$fixture/bin/"*
}

run_fixture() {
  local fixture="$1"
  shift
  (cd "$fixture" && env -i HOME="$fixture/home" TMPDIR="$fixture/tmp/" \
    PATH="$fixture/bin:$PATH" \
    STUB_ROOT="$fixture" \
    STUB_LOG="$fixture/commands.log" \
    STUB_APP_PATH_FILE="$fixture/app-path" \
    "$@")
}

valid_env=(
  VOICEFLOW_NOTARIZE=1
  APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)'
  APPLE_TEAM_ID='ABCDE12345'
  APPLE_ID='stub@example.invalid'
  APPLE_PASSWORD='not-a-real-password'
)

make_fixture "$TEMP/unsupported-argv"
for stub in uname mktemp cargo rustup npm codesign lipo hdiutil xcrun ditto spctl security openssl file find strings rg gh; do
  if run_fixture "$TEMP/unsupported-argv" env STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)' APPLE_TEAM_ID=ABCDE12345 \
    "$TEMP/unsupported-argv/bin/$stub" definitely-unsupported; then
    fail "$stub accepted an unsupported command shape"
  fi
done
if run_fixture "$TEMP/unsupported-argv" env STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  "$TEMP/unsupported-argv/expected-VoiceFlow-producer" definitely-unsupported; then
  fail "the synthetic main producer accepted an unsupported argv shape"
fi

make_fixture "$TEMP/signing-zero-default-failure"
: > "$TEMP/signing-zero-default-failure/commands.log"
chmod 600 "$TEMP/signing-zero-default-failure/commands.log"
cat > "$TEMP/signing-zero-default-failure/scripts/make-signing-cert.sh" <<'STUB'
#!/usr/bin/env bash
exit 77
STUB
chmod +x "$TEMP/signing-zero-default-failure/scripts/make-signing-cert.sh"
if run_fixture "$TEMP/signing-zero-default-failure" env GITHUB_ACTIONS=1 \
  STUB_IDENTITY_RESPONSE=zero make dmg >"$TEMP/signing-zero-default-failure.out" 2>&1; then
  fail "zero-valid-identity discovery unexpectedly succeeded by default"
fi
if rg -q '^npm:run tauri build' "$TEMP/signing-zero-default-failure/commands.log"; then
  fail "zero-valid-identity default failure reached npm build"
else
  signing_log_rc=$?
  [[ "$signing_log_rc" == 1 ]] || fail "signing-zero-default-failure commands.log scan failed"
fi

for discovery in empty zero; do
  make_fixture "$TEMP/signing-${discovery}-unsigned"
  cat > "$TEMP/signing-${discovery}-unsigned/scripts/make-signing-cert.sh" <<'STUB'
#!/usr/bin/env bash
exit 77
STUB
  chmod +x "$TEMP/signing-${discovery}-unsigned/scripts/make-signing-cert.sh"
  run_fixture "$TEMP/signing-${discovery}-unsigned" env GITHUB_ACTIONS=1 \
    STUB_IDENTITY_RESPONSE="$discovery" VOICEFLOW_ALLOW_UNSIGNED=1 \
    STUB_EXPECTED_TARGET=aarch64-apple-darwin make dmg \
    >"$TEMP/signing-${discovery}-unsigned.out" 2>&1 \
    || fail "exact unsigned opt-in did not recover from ${discovery} identity discovery"
  [[ -s "$TEMP/signing-${discovery}-unsigned/.build/release/VoiceFlow-unsigned.dmg" ]] \
    || fail "${discovery} discovery did not stage a clearly named unsigned artifact"
  [[ ! -e "$TEMP/signing-${discovery}-unsigned/.build/release/VoiceFlow.dmg" ]] \
    || fail "${discovery} discovery staged an unsigned artifact at the signed path"
  rg -q '^build-env identity= team= developer= development_team=$' \
    "$TEMP/signing-${discovery}-unsigned/commands.log" \
    || fail "${discovery} discovery retained signing variables in unsigned build"
done
make_fixture "$TEMP/signing-default-failure"
: > "$TEMP/signing-default-failure/commands.log"
chmod 600 "$TEMP/signing-default-failure/commands.log"
cat > "$TEMP/signing-default-failure/scripts/make-signing-cert.sh" <<'STUB'
#!/usr/bin/env bash
exit 77
STUB
chmod +x "$TEMP/signing-default-failure/scripts/make-signing-cert.sh"
for allow in unset '' 0 true; do
  vars=(GITHUB_ACTIONS=1)
  case "$allow" in unset) ;; *) vars+=(VOICEFLOW_ALLOW_UNSIGNED="$allow") ;; esac
  if run_fixture "$TEMP/signing-default-failure" env "${vars[@]}" make dmg \
    >"$TEMP/signing-default-$allow.out" 2>&1; then
    fail "signing failure unexpectedly succeeded with VOICEFLOW_ALLOW_UNSIGNED=${allow}"
  fi
  if rg -q '^npm:run tauri build' "$TEMP/signing-default-failure/commands.log"; then
    fail "signing failure reached npm build without exact unsigned opt-in"
  else
    signing_log_rc=$?
    [[ "$signing_log_rc" == 1 ]] || fail "signing-default-failure commands.log scan failed"
  fi
done

make_fixture "$TEMP/signing-helper-failure"
: > "$TEMP/signing-helper-failure/commands.log"
chmod 600 "$TEMP/signing-helper-failure/commands.log"
cat > "$TEMP/signing-helper-failure/scripts/make-signing-cert.sh" <<'STUB'
#!/usr/bin/env bash
exit 77
STUB
chmod +x "$TEMP/signing-helper-failure/scripts/make-signing-cert.sh"
if run_fixture "$TEMP/signing-helper-failure" env make dmg >"$TEMP/signing-helper-failure.out" 2>&1; then
  fail "local certificate helper failure was treated as a successful build"
fi
if rg -q '^npm:run tauri build' "$TEMP/signing-helper-failure/commands.log"; then
  fail "local certificate helper failure reached npm build"
else
  signing_log_rc=$?
  [[ "$signing_log_rc" == 1 ]] || fail "signing-helper-failure commands.log scan failed"
fi
run_fixture "$TEMP/signing-helper-failure" env VOICEFLOW_ALLOW_UNSIGNED=1 make dmg \
  >"$TEMP/signing-helper-opt-in.out" 2>&1 || fail "explicit opt-in did not recover from local certificate helper failure"
[[ -s "$TEMP/signing-helper-failure/.build/release/VoiceFlow-unsigned.dmg" ]] \
  || fail "helper-failure opt-in did not produce a clearly named unsigned artifact"

make_fixture "$TEMP/signing-unsigned"
cat > "$TEMP/signing-unsigned/scripts/make-signing-cert.sh" <<'STUB'
#!/usr/bin/env bash
exit 77
STUB
chmod +x "$TEMP/signing-unsigned/scripts/make-signing-cert.sh"
run_fixture "$TEMP/signing-unsigned" env GITHUB_ACTIONS=1 VOICEFLOW_ALLOW_UNSIGNED=1 \
  APPLE_SIGNING_IDENTITY=inherited-partial APPLE_TEAM_ID= STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  make dmg >"$TEMP/signing-unsigned.out" 2>&1 || fail "exact unsigned opt-in did not build"
[[ -s "$TEMP/signing-unsigned/.build/release/VoiceFlow-unsigned.dmg" ]] \
  || fail "explicit unsigned artifact was not staged under its distinct name"
[[ ! -e "$TEMP/signing-unsigned/.build/release/VoiceFlow.dmg" ]] \
  || fail "unsigned artifact was staged at the signed artifact path"
rg -q '^build-env identity= team= developer= development_team=$' "$TEMP/signing-unsigned/commands.log" \
  || fail "partial inherited signing environment reached the unsigned build"

make_fixture "$TEMP/signing-team-missing"
: > "$TEMP/signing-team-missing/commands.log"
chmod 600 "$TEMP/signing-team-missing/commands.log"
if run_fixture "$TEMP/signing-team-missing" env APPLE_SIGNING_IDENTITY=UnknownIdentity \
  make dmg >"$TEMP/signing-team-missing.out" 2>&1; then
  fail "missing Team ID was accepted by default"
fi
if rg -q '^npm:run tauri build' "$TEMP/signing-team-missing/commands.log"; then
  fail "missing Team ID reached npm build"
else
  signing_log_rc=$?
  [[ "$signing_log_rc" == 1 ]] || fail "signing-team-missing commands.log scan failed"
fi

make_fixture "$TEMP/signing-signed"
run_fixture "$TEMP/signing-signed" env APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)' \
  APPLE_TEAM_ID=ABCDE12345 make dmg >"$TEMP/signing-signed.out" 2>&1 \
  || fail "resolved signed DMG build did not complete"
[[ -s "$TEMP/signing-signed/.build/release/VoiceFlow.dmg" \
  && ! -e "$TEMP/signing-signed/.build/release/VoiceFlow-unsigned.dmg" ]] \
  || fail "signed artifact did not retain its existing destination"
rg -q '^build-env identity=Developer ID Application: Stub \(ABCDE12345\) team=ABCDE12345 ' \
  "$TEMP/signing-signed/commands.log" || fail "signed identity/team were not passed to npm"
[[ "$(cat "$TEMP/signing-signed/src-tauri/target/release/bundle/dmg/VoiceFlow.dmg")" == 'stub dmg' ]] \
  || fail "signed DMG bytes did not match the fixture build output"
cmp "$TEMP/signing-signed/src-tauri/target/release/bundle/dmg/VoiceFlow.dmg" \
  "$TEMP/signing-signed/.build/release/VoiceFlow.dmg" \
  || fail "staged signed DMG bytes differed from the built fixture DMG"

make_fixture "$TEMP/signing-ci-resolved"
run_fixture "$TEMP/signing-ci-resolved" env GITHUB_ACTIONS=1 STUB_IDENTITY_RESPONSE=valid \
  VOICEFLOW_ALLOW_UNSIGNED=1 STUB_EXPECTED_TARGET=aarch64-apple-darwin make dmg \
  >"$TEMP/signing-ci-resolved.out" 2>&1 || fail "CI discovered signed identity did not build"
[[ -s "$TEMP/signing-ci-resolved/.build/release/VoiceFlow.dmg" \
  && ! -e "$TEMP/signing-ci-resolved/.build/release/VoiceFlow-unsigned.dmg" ]] \
  || fail "CI discovered identity was downgraded to unsigned"
rg -q '^build-env identity=Developer ID Application: Stub \(ABCDE12345\) team=ABCDE12345 ' \
  "$TEMP/signing-ci-resolved/commands.log" || fail "CI discovered signed identity was not passed to build"
cmp "$TEMP/signing-ci-resolved/src-tauri/target/release/bundle/dmg/VoiceFlow.dmg" \
  "$TEMP/signing-ci-resolved/.build/release/VoiceFlow.dmg" \
  || fail "CI staged signed DMG bytes differed from the built fixture DMG"

cmp "$TEMP/signing-unsigned/src-tauri/target/release/bundle/dmg/VoiceFlow.dmg" \
  "$TEMP/signing-unsigned/.build/release/VoiceFlow-unsigned.dmg" \
  || fail "staged unsigned DMG bytes differed from the built fixture DMG"

make_fixture "$TEMP/notarize-unsigned-opt-in"
: > "$TEMP/notarize-unsigned-opt-in/commands.log"
if run_fixture "$TEMP/notarize-unsigned-opt-in" env VOICEFLOW_ALLOW_UNSIGNED=1 \
  APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)' APPLE_TEAM_ID=ABCDE12345 \
  make notarize >"$TEMP/notarize-unsigned-opt-in.out" 2>&1; then
  fail "notarization accepted unsigned opt-in without Apple notarization credentials"
fi
[[ ! -e "$TEMP/notarize-unsigned-opt-in/src-tauri/target/release/bundle/dmg" ]] \
  || fail "unsigned opt-in reached the notarization build stage"
[[ -f "$TEMP/notarize-unsigned-opt-in/commands.log" ]] || fail "notarization command log was not created"
if rg -q '^npm:' "$TEMP/notarize-unsigned-opt-in/commands.log"; then
  fail "missing notarization credentials reached an npm command"
else
  rg_status=$?
  [[ "$rg_status" -eq 1 ]] || fail "could not inspect notarization command log (rg exit $rg_status)"
fi

make_fixture "$TEMP/notarize-missing-identity"
: > "$TEMP/notarize-missing-identity/commands.log"
if run_fixture "$TEMP/notarize-missing-identity" env VOICEFLOW_ALLOW_UNSIGNED=1 \
  APPLE_TEAM_ID=ABCDE12345 APPLE_ID=stub@example.invalid APPLE_PASSWORD=not-a-real-password \
  make notarize >"$TEMP/notarize-missing-identity.out" 2>&1; then
  fail "notarization accepted missing signing identity"
fi
[[ -f "$TEMP/notarize-missing-identity/commands.log" ]] || fail "notarization command log was not created"
if rg -q '^npm:' "$TEMP/notarize-missing-identity/commands.log"; then
  fail "missing signing identity reached an npm command"
else
  rg_status=$?
  [[ "$rg_status" -eq 1 ]] || fail "could not inspect notarization command log (rg exit $rg_status)"
fi

make_fixture "$TEMP/notarize-mismatched-team"
: > "$TEMP/notarize-mismatched-team/commands.log"
if run_fixture "$TEMP/notarize-mismatched-team" env VOICEFLOW_ALLOW_UNSIGNED=1 \
  APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)' APPLE_TEAM_ID=ZZZZZ99999 \
  APPLE_ID=stub@example.invalid APPLE_PASSWORD=not-a-real-password \
  make notarize >"$TEMP/notarize-mismatched-team.out" 2>&1; then
  fail "notarization accepted a mismatched signing team"
fi
[[ -f "$TEMP/notarize-mismatched-team/commands.log" ]] || fail "notarization command log was not created"
if rg -q '^npm:' "$TEMP/notarize-mismatched-team/commands.log"; then
  fail "mismatched signing team reached an npm command"
else
  rg_status=$?
  [[ "$rg_status" -eq 1 ]] || fail "could not inspect notarization command log (rg exit $rg_status)"
fi

make_fixture "$TEMP/certificate-helper"
mkdir -p "$TEMP/certificate-helper/fake-signing"
cat > "$TEMP/certificate-helper/bin/openssl" <<'STUB'
#!/usr/bin/env bash
case "$1" in
  rand) [[ "$#" -eq 3 && "$1" == rand && "$2" == -hex && "$3" == 20 ]] || exit 1
    printf 'openssl:rand -hex 20\n' >> "$STUB_LOG"; printf 'SYNTHETIC-P12-SECRET-MARKER\n' ;;
  req) [[ "$#" -eq 14 && "$1" == req && "$2" == -x509 && "$3" == -newkey \
      && "$4" == rsa:2048 && "$5" == -sha256 && "$6" == -days && "$7" == 7300 \
      && "$8" == -nodes && "$9" == -keyout && "${11}" == -out && "${13}" == -config ]] || exit 1
    helper_tmp="${10%/key.pem}"
    [[ "${10}" == "$helper_tmp/key.pem" && "${12}" == "$helper_tmp/cert.pem" \
      && "${14}" == "$helper_tmp/openssl.cnf" && "${helper_tmp%/*}" == "$STUB_ROOT/tmp" \
      && -n "${helper_tmp##*/}" && "${helper_tmp##*/}" != . && "${helper_tmp##*/}" != .. \
      && -d "$helper_tmp" && ! -L "$helper_tmp" ]] || exit 1
    printf '%s\n' "$helper_tmp" > "$STUB_ROOT/helper-tmp.path"
    printf 'openssl:req\n' >> "$STUB_LOG"; printf 'synthetic key\n' > "${10}"; printf 'synthetic cert\n' > "${12}" ;;
  pkcs12) [[ "$#" -eq 18 && "$1" == pkcs12 && "$2" == -export && "$3" == -inkey \
      && "$5" == -in && "$7" == -out && "$9" == -name && "${11}" == -passout \
      && "${12}" == pass:SYNTHETIC-P12-SECRET-MARKER && "${13}" == -keypbe \
      && "${14}" == PBE-SHA1-3DES && "${15}" == -certpbe && "${16}" == PBE-SHA1-3DES \
      && "${17}" == -macalg && "${18}" == sha1 ]] || exit 1
    helper_tmp="$(<"$STUB_ROOT/helper-tmp.path")"
    [[ "$4" == "$helper_tmp/key.pem" && "$6" == "$helper_tmp/cert.pem" \
      && "$8" == "$helper_tmp/identity.p12" && "${10}" == VoiceFlow ]] || exit 1
    printf 'openssl:pkcs12 -export [redacted]\n' >> "$STUB_LOG"; printf 'SYNTHETIC-P12-BLOB\n' > "$8" ;;
  *) exit 1 ;;
esac
exit 0
STUB
cat > "$TEMP/certificate-helper/bin/security" <<'STUB'
#!/usr/bin/env bash
keychain="$STUB_ROOT/fake-signing/fake.keychain-db"
case "$1" in
  default-keychain) [[ "$#" -eq 3 && "$1" == default-keychain && "$2" == -d && "$3" == user ]] || exit 1
    printf 'security:default-keychain\n' >> "$STUB_LOG"; printf '"%s"\n' "$keychain" ;;
  find-identity) [[ "$#" -eq 4 && "$1" == find-identity && "$2" == -v && "$3" == -p \
      && "$4" == codesigning ]] || exit 1
    printf 'security:find-identity\n' >> "$STUB_LOG"
    if [[ -e "$STUB_ROOT/cert-imported" ]]; then printf '1) ABCDEF1234567890 "VoiceFlow"\n'; fi ;;
  import) helper_tmp="$(<"$STUB_ROOT/helper-tmp.path")"
    [[ "$#" -eq 9 && "$1" == import && "$2" == "$helper_tmp/identity.p12" \
      && "$3" == -k && "$4" == "$keychain" && "$5" == -P \
      && "$6" == SYNTHETIC-P12-SECRET-MARKER && "$7" == -T \
      && "$8" == /usr/bin/codesign && "$9" == -A ]] || exit 1
    printf 'security:import [redacted]\n' >> "$STUB_LOG"; : > "$STUB_ROOT/cert-imported" ;;
  set-key-partition-list) [[ "$#" -eq 5 && "$1" == set-key-partition-list && "$2" == -S \
      && "$3" == apple-tool:,apple:,codesign: && "$4" == -s && "$5" == "$keychain" ]] || exit 1
    printf 'security:set-key-partition-list\n' >> "$STUB_LOG" ;;
  add-trusted-cert) helper_tmp="$(<"$STUB_ROOT/helper-tmp.path")"
    [[ "$#" -eq 8 && "$1" == add-trusted-cert && "$2" == -r \
      && "$3" == trustRoot && "$4" == -p && "$5" == codeSign && "$6" == -k \
      && "$7" == "$keychain" && "$8" == "$helper_tmp/cert.pem" ]] || exit 1
    printf 'security:add-trusted-cert\n' >> "$STUB_LOG" ;;
  *) exit 1 ;;
esac
exit 0
STUB
chmod +x "$TEMP/certificate-helper/bin/openssl" "$TEMP/certificate-helper/bin/security"
touch "$TEMP/certificate-helper/fake-signing/fake.keychain-db"
: > "$TEMP/certificate-helper/commands.log"
if ! run_fixture "$TEMP/certificate-helper" env STUB_ROOT="$TEMP/certificate-helper" \
  VOICEFLOW_SIGNING_DIR="$TEMP/certificate-helper/fake-signing" \
  bash scripts/make-signing-cert.sh >"$TEMP/certificate-helper.out" 2>&1; then
  head -c 2048 "$TEMP/certificate-helper.out" \
    | sed 's/SYNTHETIC-P12-SECRET-MARKER/<redacted>/g' >&2
  fail "synthetic certificate helper did not finish"
fi
[[ "$(stat -f '%Lp' "$TEMP/certificate-helper/fake-signing/password.txt")" == 600 ]] \
  || fail "certificate helper password file was not mode 600"
grep -qx 'SYNTHETIC-P12-SECRET-MARKER' "$TEMP/certificate-helper/fake-signing/password.txt" \
  || fail "synthetic password was not written to its existing file contract"
! rg -q 'SYNTHETIC-P12-SECRET-MARKER' "$TEMP/certificate-helper.out" \
  || fail "certificate helper exposed the synthetic password in its output"
! rg -q 'SYNTHETIC-P12-SECRET-MARKER' "$TEMP/certificate-helper/commands.log" \
  || fail "certificate helper exposed the synthetic password in its command log"
for category in 'openssl:rand -hex 20' 'openssl:req' 'openssl:pkcs12 -export [redacted]' \
  'security:default-keychain' 'security:find-identity' 'security:import [redacted]' \
  'security:set-key-partition-list' 'security:add-trusted-cert'; do
  rg -Fq "$category" "$TEMP/certificate-helper/commands.log" \
    || fail "certificate helper invocation category was not recorded: $category"
done
[[ -s "$TEMP/certificate-helper/commands.log" ]] || fail "certificate helper invocation log was empty"

# C1 certificate flow cases. These helpers deliberately share the release fixture
# and exercise the wrapper directly with isolated, fake Apple credentials.
init_notary_fixture() {
  local name="$1" mode="${2:-spaces}" presence="${3:-1}" fixture="$TEMP/$1"
  make_fixture "$fixture"
  : > "$fixture/commands.log"
  if [[ "$mode" == spaces ]]; then
    printf '%s\n' "$fixture/Developer Keychain One.keychain-db" "$fixture/Developer Keychain Two.keychain-db" > "$fixture/original-items"
  else
    : > "$fixture/original-items"
  fi
  cp "$fixture/original-items" "$fixture/current-items"
  if [[ "$presence" == 1 ]]; then : > "$fixture/expect-temporary-keychain"; fi
}

assert_no_notary_secrets() {
  local fixture="$1" label="$2" secret file status
  for file in "$fixture/wrapper.out" "$fixture/commands.log"; do
    [[ -r "$file" ]] || fail "$label cannot read secret-check input"
    for secret in 'STUB-K-9f1c2a7e' 'STUB-P-43b8d0c1' 'STUB-BLOB-7041c9f6' 'U1RVQi1CTE9CLTcwNDFjOWY2'; do
      if rg -Fq -- "$secret" "$file" >/dev/null 2>&1; then
        status=0
      else
        status=$?
      fi
      case "$status" in
        0) fail "$label leaked certificate material" ;;
        1) ;;
        *) fail "$label secret check could not read input" ;;
      esac
    done
  done
}

run_notary_case() {
  local fixture="$1" expected_rc="$2" label="$3" rc actual
  shift 3
  if run_fixture "$fixture" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 \
    "$@" /bin/bash scripts/release-macos.sh >"$fixture/wrapper.out" 2>&1; then
    rc=0
  else
    rc=$?
  fi
  if [[ "$expected_rc" == nonzero ]]; then
    [[ "$rc" -ne 0 ]] || fail "$label unexpectedly succeeded"
  else
    [[ "$rc" -eq "$expected_rc" ]] || fail "$label returned $rc, expected $expected_rc"
  fi
  actual="$(cat "$fixture/commands.log")" || fail "$label command log could not be read"
  assert_no_notary_secrets "$fixture" "$label"
  if [[ "$expected_rc" != 0 ]]; then
    notary_log_count "$fixture" 'npm:run tauri -- build --target aarch64-apple-darwin --bundles app' 0
  fi
  [[ -f "$fixture/current-items" ]] || fail "$label current keychain list is missing"
  cmp "$fixture/original-items" "$fixture/current-items" || fail "$label did not restore current keychain list exactly"
  for artifact in "$fixture/.build/release/VoiceFlow-notarized.dmg" "$fixture/.build/release/VoiceFlow-notarized.receipt.json"; do
    if [[ "$expected_rc" != 0 ]]; then [[ ! -e "$artifact" ]] || fail "$label published an artifact on failure"; fi
  done
  if [[ -e "$fixture/.voiceflow-notary-workdir" ]]; then
    local workdir
    IFS= read -r workdir < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir record could not be read"
    [[ ! -e "$workdir" ]] || fail "$label left its temporary work directory"
  fi
  if [[ -e "$fixture/.voiceflow-notary-state" ]]; then
    local chain recorded_workdir
    IFS= read -r recorded_workdir < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir provenance could not be read"
    IFS= read -r chain < "$fixture/.voiceflow-notary-state" || fail "$label keychain record could not be read"
    [[ "$chain" == "$recorded_workdir/signing.keychain-db" ]] || fail "$label keychain record has unexpected provenance"
    [[ ! -e "$chain" ]] || fail "$label left its temporary keychain"
    [[ ! -e "${chain%/signing.keychain-db}/identity.p12" ]] || fail "$label left its temporary certificate"
    [[ ! -e "${chain%/signing.keychain-db}" ]] || fail "$label left its keychain work directory"
  fi
  # Workdir record is retained fixture provenance; physical paths were checked above.
  [[ ! -e "$fixture/settings" && ! -e "$fixture/unlocked" && ! -e "$fixture/imported" && ! -e "$fixture/partitioned" && ! -e "$fixture/published" ]] || fail "$label left a phase marker"
}

notary_log_count() {
  local fixture="$1" line="$2" expected="$3" count
  count="$(rg -Fxc "$line" "$fixture/commands.log")" || { local status=$?; [[ "$status" -eq 1 ]] && count=0 || fail "could not inspect $fixture command log"; }
  [[ "$count" -eq "$expected" ]] || fail "$fixture expected $expected occurrences of $line, found $count"
}

assert_notary_order() {
  local fixture="$1" log line last=0 index=0
  log="$fixture/commands.log"
  while IFS= read -r line; do
    index=$((index + 1))
    case "$line" in
      'security:delete-keychain') last="$index" ;;
      'security:list-keychains restore')
        [[ "$last" -gt 0 && "$last" -lt "$index" ]] || fail "$fixture did not delete before restoring keychains"
        return 0 ;;
    esac
  done < "$log"
  fail "$fixture missing ordered delete and restore records"
}

assert_notary_phase_counts() {
  local fixture="$1" snapshot="$2" create="$3" settings="$4" unlock="$5" import="$6" partition="$7" publish="$8" delete="$9" restore="${10}"
  notary_log_count "$fixture" 'security:list-keychains' "$snapshot"
  notary_log_count "$fixture" 'security:create-keychain [redacted]' "$create"
  notary_log_count "$fixture" 'security:set-keychain-settings' "$settings"
  notary_log_count "$fixture" 'security:unlock-keychain [redacted]' "$unlock"
  notary_log_count "$fixture" 'security:import [redacted]' "$import"
  notary_log_count "$fixture" 'security:set-key-partition-list [redacted]' "$partition"
  notary_log_count "$fixture" 'security:list-keychains publish' "$publish"
  notary_log_count "$fixture" 'security:delete-keychain' "$delete"
  notary_log_count "$fixture" 'security:list-keychains restore' "$restore"
}

# The independently prepared expected blob reflects the raw non-empty certificate input.
init_notary_fixture cert-spaces spaces 1
run_notary_case "$TEMP/cert-spaces" 0 cert-spaces
assert_notary_phase_counts "$TEMP/cert-spaces" 1 1 1 1 1 1 1 1 1
assert_notary_order "$TEMP/cert-spaces"
[[ -s "$TEMP/cert-spaces/.build/release/VoiceFlow-notarized.dmg" && -s "$TEMP/cert-spaces/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "cert-spaces did not publish the complete artifact pair"

init_notary_fixture cert-empty empty 1
run_notary_case "$TEMP/cert-empty" 0 cert-empty
assert_notary_phase_counts "$TEMP/cert-empty" 1 1 1 1 1 1 1 1 1
assert_notary_order "$TEMP/cert-empty"
[[ -s "$TEMP/cert-empty/.build/release/VoiceFlow-notarized.dmg" && -s "$TEMP/cert-empty/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "cert-empty did not publish the complete artifact pair"

init_notary_fixture missing-password spaces 1
if run_fixture "$TEMP/missing-password" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 /bin/bash scripts/release-macos.sh >"$TEMP/missing-password/wrapper.out" 2>&1; then rc=0; else rc=$?; fi
[[ "$rc" -eq 2 && -f "$TEMP/missing-password/expect-temporary-keychain" ]] || fail "missing-password did not stop at the credential preflight boundary"
cmp "$TEMP/missing-password/original-items" "$TEMP/missing-password/current-items" || fail "missing-password changed current keychains"
assert_no_notary_secrets "$TEMP/missing-password" missing-password
notary_log_count "$TEMP/missing-password" 'npm:run tauri -- build --target aarch64-apple-darwin --bundles app' 0
assert_notary_phase_counts "$TEMP/missing-password" 0 0 0 0 0 0 0 0 0
[[ ! -e "$TEMP/missing-password/.build/release/VoiceFlow-notarized.dmg" && ! -e "$TEMP/missing-password/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "missing-password published artifacts"

init_notary_fixture missing-certificate spaces 0
if run_fixture "$TEMP/missing-certificate" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 /bin/bash scripts/release-macos.sh >"$TEMP/missing-certificate/wrapper.out" 2>&1; then rc=0; else rc=$?; fi
[[ "$rc" -eq 2 && ! -e "$TEMP/missing-certificate/expect-temporary-keychain" ]] || fail "missing-certificate did not stop at the credential preflight boundary"
cmp "$TEMP/missing-certificate/original-items" "$TEMP/missing-certificate/current-items" || fail "missing-certificate changed current keychains"
assert_no_notary_secrets "$TEMP/missing-certificate" missing-certificate
notary_log_count "$TEMP/missing-certificate" 'npm:run tauri -- build --target aarch64-apple-darwin --bundles app' 0
[[ ! -e "$TEMP/missing-certificate/.build/release/VoiceFlow-notarized.dmg" && ! -e "$TEMP/missing-certificate/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "missing-certificate published artifacts"
assert_notary_phase_counts "$TEMP/missing-certificate" 0 0 0 0 0 0 0 0 0

init_notary_fixture decode-invalid spaces 1
run_notary_case "$TEMP/decode-invalid" nonzero decode-invalid APPLE_CERTIFICATE='!'
assert_notary_phase_counts "$TEMP/decode-invalid" 1 0 0 0 0 0 0 0 0

for phase_case in rand:70 snapshot:71 create-empty:72 create-partial:72 settings:73 unlock:74 import:75 partition:76 publish:77; do
  phase="${phase_case%%:*}"; expected="${phase_case##*:}"
  init_notary_fixture "phase-$phase" spaces 1
  run_notary_case "$TEMP/phase-$phase" "$expected" "phase-$phase" STUB_NOTARY_PHASE_FAIL="$phase"
  case "$phase" in
    rand) assert_notary_phase_counts "$TEMP/phase-$phase" 1 0 0 0 0 0 0 0 0 ;;
    snapshot) assert_notary_phase_counts "$TEMP/phase-$phase" 1 0 0 0 0 0 0 0 0 ;;
    create-empty) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 0 0 0 0 0 0 1 ;;
    create-partial) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 0 0 0 0 0 1 1 ;;
    settings) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 1 0 0 0 0 1 1 ;;
    unlock) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 1 1 0 0 0 1 1 ;;
    import) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 1 1 1 0 0 1 1 ;;
    partition) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 1 1 1 1 0 1 1 ;;
    publish) assert_notary_phase_counts "$TEMP/phase-$phase" 1 1 1 1 1 1 1 1 1 ;;
  esac
  if [[ "$phase" == publish ]]; then [[ ! -e "$TEMP/phase-$phase/published" ]] || fail "publish failure left published marker"; fi
  if [[ "$phase" == create-empty ]]; then if [[ -e "$TEMP/phase-$phase/.voiceflow-notary-state" ]]; then
    chain="$(<"$TEMP/phase-$phase/.voiceflow-notary-state")"
    [[ ! -e "$chain" ]] || fail "empty create failure left physical keychain"
  fi; fi
done

# C2 late wrapper failures, cleanup failures, and owned-parent signal handling.
c2_run_direct() {
  local fixture="$1" expected="$2" label="$3" rc
  shift 3
  if run_fixture "$fixture" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 \
    "$@" /bin/bash scripts/release-macos.sh >"$fixture/wrapper.out" 2>&1; then rc=0; else rc=$?; fi
  if [[ "$expected" == nonzero ]]; then [[ "$rc" -ne 0 ]] || fail "$label unexpectedly succeeded"; else [[ "$rc" -eq "$expected" ]] || fail "$label returned $rc, expected $expected"; fi
  assert_no_notary_secrets "$fixture" "$label"
  if grep -q '^Notarized and verified' "$fixture/wrapper.out"; then fail "$label printed success banner"; else rc=$?; [[ "$rc" -eq 1 ]] || fail "$label banner check could not read output"; fi
  [[ ! -e "$fixture/.build/release/VoiceFlow-notarized.dmg" && ! -e "$fixture/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "$label published artifact pair on failure"
  if [[ "$label" == late-signal-int || "$label" == late-signal-term ]]; then
    assert_notary_phase_counts "$fixture" 1 1 1 1 1 0 0 1 1
    notary_log_count "$fixture" 'npm:run tauri -- build --target aarch64-apple-darwin --bundles app' 0
  else
    assert_notary_phase_counts "$fixture" 1 1 1 1 1 1 1 1 1
  fi
  assert_notary_order "$fixture"
  local marker workdir chain
  for marker in settings unlocked imported partitioned published; do [[ ! -e "$fixture/$marker" ]] || fail "$label left $marker phase marker"; done
  [[ -f "$fixture/current-items" ]] || fail "$label current keychain list missing"
  cmp "$fixture/original-items" "$fixture/current-items" || fail "$label failed to restore current keychains"
  IFS= read -r workdir < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir record missing"
  if [[ "$label" == late-final-detach ]]; then [[ -d "$workdir" ]] || fail "$label must retain work data while detach failed"; else [[ ! -e "$workdir" ]] || fail "$label left temporary work directory"; fi
  IFS= read -r chain < "$fixture/.voiceflow-notary-state" || fail "$label keychain record missing"
  [[ "$chain" == "$workdir/signing.keychain-db" ]] || fail "$label keychain provenance unexpected"
  [[ ! -e "$chain" && ! -e "${chain%/signing.keychain-db}/identity.p12" ]] || fail "$label left temporary keychain/certificate"
}

c2_run_cleanup_failure() {
  local fixture="$1" label="$2" restore_expected="$3" rc workdir chain
  shift 3
  if run_fixture "$fixture" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 \
    "$@" /bin/bash scripts/release-macos.sh >"$fixture/wrapper.out" 2>&1; then rc=0; else rc=$?; fi
  [[ "$rc" -ne 0 ]] || fail "$label unexpectedly succeeded"
  assert_no_notary_secrets "$fixture" "$label"
  if grep -q '^Notarized and verified' "$fixture/wrapper.out"; then fail "$label printed success banner"; else rc=$?; [[ "$rc" -eq 1 ]] || fail "$label banner check failed"; fi
  [[ ! -e "$fixture/.build/release/VoiceFlow-notarized.dmg" && ! -e "$fixture/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "$label published artifact pair"
  assert_notary_phase_counts "$fixture" 1 1 1 1 1 1 1 1 1
  assert_notary_order "$fixture"
  IFS= read -r workdir < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir record missing"
  [[ ! -e "$workdir" ]] || fail "$label left temporary work directory"
  IFS= read -r chain < "$fixture/.voiceflow-notary-state" || fail "$label keychain record missing"
  [[ "$chain" == "$workdir/signing.keychain-db" ]] || fail "$label keychain provenance unexpected"
  [[ ! -e "$chain" && ! -e "${chain%/signing.keychain-db}/identity.p12" && ! -e "${chain%/signing.keychain-db}" ]] || fail "$label left temporary keychain/certificate"
  if [[ "$restore_expected" == success ]]; then cmp "$fixture/original-items" "$fixture/current-items" || fail "$label did not restore current keychains"; fi
}

for c2_case in late-tauri:82 late-app-rejected:1 late-dmg-sign:59 late-final-detach:45 late-signal-int:130 late-signal-term:143; do
  c2_name="${c2_case%%:*}"; c2_expected="${c2_case##*:}"
  init_notary_fixture "$c2_name" spaces 1
  case "$c2_name" in
    late-tauri) c2_flags=(STUB_NOTARY_TAURI_FAIL=1) ;;
    late-app-rejected) c2_flags=(STUB_NOTARY_STATUS=Rejected) ;;
    late-dmg-sign) c2_flags=(STUB_DMGSIGN_FAIL=1) ;;
    late-final-detach) c2_flags=(STUB_DETACH_FAIL=1) ;;
    late-signal-int) c2_flags=(STUB_NOTARY_AFTER_IMPORT_SIGNAL=INT) ;;
    late-signal-term) c2_flags=(STUB_NOTARY_AFTER_IMPORT_SIGNAL=TERM) ;;
  esac
  c2_run_direct "$TEMP/$c2_name" "$c2_expected" "$c2_name" "${c2_flags[@]}"
  if [[ "$c2_name" == late-final-detach ]]; then
    c2_detaches="$(rg -c '^hdiutil:detach ' "$TEMP/$c2_name/commands.log")" || fail "detach log could not be read"
    [[ "$c2_detaches" -eq 2 ]] || fail "detach failure did not make exactly two detach attempts"
    IFS= read -r c2_workdir < "$TEMP/$c2_name/.voiceflow-notary-workdir" || fail "detach workdir record missing"
    [[ -d "$c2_workdir/mount" ]] || fail "detach failure removed potentially mounted verification data"
  fi
done

for c2_cleanup in delete restore both; do
  init_notary_fixture "cleanup-$c2_cleanup" spaces 1
  if [[ "$c2_cleanup" == delete ]]; then c2_restore=success; else c2_restore=failed; fi
  c2_run_cleanup_failure "$TEMP/cleanup-$c2_cleanup" "cleanup-$c2_cleanup" "$c2_restore" "STUB_NOTARY_CLEANUP_FAIL=$c2_cleanup"
done
init_notary_fixture tauri-and-both-cleanups spaces 1
if run_fixture "$TEMP/tauri-and-both-cleanups" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 \
  STUB_NOTARY_TAURI_FAIL=1 STUB_NOTARY_CLEANUP_FAIL=both /bin/bash scripts/release-macos.sh \
  >"$TEMP/tauri-and-both-cleanups/wrapper.out" 2>&1; then c2_rc=0; else c2_rc=$?; fi
[[ "$c2_rc" -eq 82 ]] || fail "tauri-and-both-cleanups returned $c2_rc, expected Tauri status 82"
assert_no_notary_secrets "$TEMP/tauri-and-both-cleanups" tauri-and-both-cleanups
if grep -q '^Notarized and verified' "$TEMP/tauri-and-both-cleanups/wrapper.out"; then fail "tauri-and-both-cleanups printed success banner"; else c2_rc=$?; [[ "$c2_rc" -eq 1 ]] || fail "tauri-and-both-cleanups banner check failed"; fi
[[ ! -e "$TEMP/tauri-and-both-cleanups/.build/release/VoiceFlow-notarized.dmg" && ! -e "$TEMP/tauri-and-both-cleanups/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "tauri-and-both-cleanups published artifact pair"
assert_notary_phase_counts "$TEMP/tauri-and-both-cleanups" 1 1 1 1 1 1 1 1 1
assert_notary_order "$TEMP/tauri-and-both-cleanups"
IFS= read -r c2_workdir < "$TEMP/tauri-and-both-cleanups/.voiceflow-notary-workdir" || fail "tauri-and-both-cleanups workdir record missing"
[[ ! -e "$c2_workdir" ]] || fail "tauri-and-both-cleanups left temporary work directory"
IFS= read -r c2_chain < "$TEMP/tauri-and-both-cleanups/.voiceflow-notary-state" || fail "tauri-and-both-cleanups keychain record missing"
[[ ! -e "$c2_chain" && ! -e "${c2_chain%/signing.keychain-db}/identity.p12" && ! -e "${c2_chain%/signing.keychain-db}" ]] || fail "tauri-and-both-cleanups left temporary keychain/certificate"

# Packaged sidecar and provenance verification regressions.
packaging_run() {
  local fixture="$1" rc
  shift
  if run_fixture "$fixture" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    APPLE_CERTIFICATE=U1RVQi1CTE9CLTcwNDFjOWY2 APPLE_CERTIFICATE_PASSWORD=STUB-P-43b8d0c1 \
    "$@" /bin/bash scripts/release-macos.sh >"$fixture/wrapper.out" 2>&1; then rc=0; else rc=$?; fi
  printf '%s\n' "$rc" > "$fixture/wrapper.rc"
}

packaging_prefix_count() {
  local fixture="$1" prefix="$2" expected="$3" count status
  [[ -r "$fixture/commands.log" ]] || fail "$fixture command log is missing or unreadable"
  if count="$(rg -c "^$prefix" "$fixture/commands.log")"; then status=0; else status=$?; fi
  case "$status" in 0) ;; 1) count=0 ;; *) fail "$fixture command log inspection failed for $prefix" ;; esac
  [[ "$count" -eq "$expected" ]] || fail "$fixture expected $expected command records for $prefix, found $count"
}

packaging_assert_hello_rows() {
  local fixture="$1" expected_rows="$2" label="$3"
  [[ -r "$fixture/sidecar-hello.log" ]] || fail "$label sidecar hello log is missing or unreadable"
  python3 - "$fixture" "$expected_rows" <<'PY_PACKAGED_HELLO_ROWS_66' || fail "$label hello request records or owned PIDs were invalid"
import json, os, sys
from pathlib import Path
root, count = Path(sys.argv[1]), int(sys.argv[2])
lines = (root / "sidecar-hello.log").read_text(encoding="utf-8").splitlines()
assert len(lines) == count
rows = [json.loads(line) for line in lines]
assert all(set(row) == {"mode", "pid", "models_root", "audio_root", "request", "eof"} for row in rows)
assert [row["mode"] for row in rows] == ["asr", "cleanup"]
work = Path((root / ".voiceflow-notary-workdir").read_text(encoding="utf-8").strip())
assert rows[0]["models_root"] == rows[1]["models_root"] == str(work / "empty-models")
assert rows[0]["audio_root"] == str(work / "empty-audio") and rows[1]["audio_root"] is None
for row, rid in zip(rows, ("release-asr", "release-cleanup")):
    assert type(row["pid"]) is int and row["pid"] > 0 and row["eof"] is True
    assert row["request"] == {"protocol_version": 1, "request_id": rid, "session_id": None,
                              "model_version": None, "op": "hello"}
    try:
        os.kill(row["pid"], 0)
    except ProcessLookupError:
        pass
    else:
        raise AssertionError("recorded worker PID remains alive")
PY_PACKAGED_HELLO_ROWS_66
}

packaging_assert_released_pair() {
  local fixture="$1" label="$2"
  [[ -s "$fixture/.build/release/VoiceFlow-notarized.dmg" && \
     -s "$fixture/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "$label did not publish final DMG and receipt"
  [[ -f "$fixture/current-items" ]] || fail "$label current keychain list is missing"
  cmp "$fixture/original-items" "$fixture/current-items" || fail "$label did not restore keychain list"
  IFS= read -r packaging_work < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir record is missing"
  [[ ! -e "$packaging_work" ]] || fail "$label left its temporary work directory"
  IFS= read -r packaging_chain < "$fixture/.voiceflow-notary-state" || fail "$label keychain record is missing"
  [[ "$packaging_chain" == "$packaging_work/signing.keychain-db" && ! -e "$packaging_chain" && \
     ! -e "${packaging_chain%/signing.keychain-db}/identity.p12" && \
     ! -e "${packaging_chain%/signing.keychain-db}" ]] || fail "$label left its temporary keychain or certificate"
}

packaging_assert_failure() {
  local fixture="$1" label="$2" diagnostic="$3" rc
  [[ -r "$fixture/wrapper.rc" && -r "$fixture/wrapper.out" ]] || fail "$label wrapper result is missing or unreadable"
  IFS= read -r rc < "$fixture/wrapper.rc" || fail "$label wrapper status could not be read"
  [[ "$rc" -ne 0 ]] || fail "$label unexpectedly succeeded"
  rg -Fq -- "$diagnostic" "$fixture/wrapper.out" || fail "$label did not reach diagnostic: $diagnostic"
  if rg -q '^Notarized and verified' "$fixture/wrapper.out"; then fail "$label printed success banner"; else rc=$?; [[ "$rc" -eq 1 ]] || fail "$label banner check could not read output"; fi
  [[ ! -e "$fixture/.build/release/VoiceFlow-notarized.dmg" && ! -e "$fixture/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "$label published a final artifact"
  assert_no_notary_secrets "$fixture" "$label"
  assert_notary_phase_counts "$fixture" 1 1 1 1 1 1 1 1 1
  assert_notary_order "$fixture"
  [[ -r "$fixture/current-items" ]] || fail "$label current keychain list is missing"
  cmp "$fixture/original-items" "$fixture/current-items" || fail "$label did not restore keychain list"
  IFS= read -r packaging_work < "$fixture/.voiceflow-notary-workdir" || fail "$label workdir record is missing"
  [[ ! -e "$packaging_work" ]] || fail "$label left its temporary work directory"
  IFS= read -r packaging_chain < "$fixture/.voiceflow-notary-state" || fail "$label keychain record is missing"
  [[ "$packaging_chain" == "$packaging_work/signing.keychain-db" && ! -e "$packaging_chain" && \
     ! -e "${packaging_chain%/signing.keychain-db}/identity.p12" && \
     ! -e "${packaging_chain%/signing.keychain-db}" ]] || fail "$label left its temporary keychain or certificate"
  for marker in settings unlocked imported partitioned published; do [[ ! -e "$fixture/$marker" ]] || fail "$label left $marker marker"; done
}

for packaging_success in default unsupported-null; do
  init_notary_fixture "packaging-$packaging_success" spaces 1
  if [[ "$packaging_success" == unsupported-null ]]; then
    packaging_run "$TEMP/packaging-$packaging_success" STUB_HELLO_FAULT=unsupported-null
  else
    packaging_run "$TEMP/packaging-$packaging_success"
  fi
  IFS= read -r packaging_rc < "$TEMP/packaging-$packaging_success/wrapper.rc" || fail "$packaging_success status missing"
  [[ "$packaging_rc" -eq 0 ]] || fail "$packaging_success packaging returned $packaging_rc"
  rg -q '^Notarized and verified' "$TEMP/packaging-$packaging_success/wrapper.out" || fail "$packaging_success success banner missing"
  packaging_assert_released_pair "$TEMP/packaging-$packaging_success" "$packaging_success"
  packaging_assert_hello_rows "$TEMP/packaging-$packaging_success" 2 "$packaging_success"
  assert_no_notary_secrets "$TEMP/packaging-$packaging_success" "$packaging_success"
  notary_log_count "$TEMP/packaging-$packaging_success" 'npm:run tauri -- build --target aarch64-apple-darwin --bundles app' 1
  assert_notary_phase_counts "$TEMP/packaging-$packaging_success" 1 1 1 1 1 1 1 1 1
  assert_notary_order "$TEMP/packaging-$packaging_success"
  packaging_prefix_count "$TEMP/packaging-$packaging_success" 'xcrun:notarytool submit ' 2
  packaging_prefix_count "$TEMP/packaging-$packaging_success" 'hdiutil:attach ' 1
done

for packaging_case in \
  'wrong-protocol|invalid protocol_version|asr' \
  'bool-protocol|invalid protocol_version|asr' \
  'cleanup-correlation|invalid request_id|both' \
  'malformed-json|returned invalid JSON|asr' \
  'extra-jsonl|did not return exactly one JSONL hello response|asr' \
  'nonzero-exit|exited with status 39|asr' \
  'cleanup-timeout|exceeded the 8-second limit|both' \
  'loaded-attestation|unexpectedly reports loaded model state|asr' \
  'missing-platform|missing required fields|asr' \
  'integer-platform|invalid platform_supported value|asr'; do
  packaging_fault="${packaging_case%%|*}"; packaging_rest="${packaging_case#*|}"
  packaging_diag="${packaging_rest%%|*}"; packaging_modes="${packaging_rest##*|}"
  init_notary_fixture "packaging-hello-$packaging_fault" spaces 1
  packaging_run "$TEMP/packaging-hello-$packaging_fault" "STUB_HELLO_FAULT=$packaging_fault"
  packaging_assert_failure "$TEMP/packaging-hello-$packaging_fault" "$packaging_fault" "$packaging_diag"
  packaging_prefix_count "$TEMP/packaging-hello-$packaging_fault" 'xcrun:notarytool submit ' 0
  packaging_prefix_count "$TEMP/packaging-hello-$packaging_fault" 'hdiutil:create ' 0
  packaging_prefix_count "$TEMP/packaging-hello-$packaging_fault" 'hdiutil:attach ' 0
  if [[ "$packaging_modes" == both ]]; then packaging_assert_hello_rows "$TEMP/packaging-hello-$packaging_fault" 2 "$packaging_fault"; \
  else
    [[ -r "$TEMP/packaging-hello-$packaging_fault/sidecar-hello.log" ]] || fail "$packaging_fault hello log missing"
    python3 - "$TEMP/packaging-hello-$packaging_fault" <<'PY_PACKAGED_ASR_ROW_66' || fail "$packaging_fault ASR hello record invalid or worker remained alive"
import json, os, sys
from pathlib import Path
root=Path(sys.argv[1]); rows=[json.loads(x) for x in (root/'sidecar-hello.log').read_text().splitlines()]
assert len(rows)==1 and rows[0]['mode']=='asr' and rows[0]['eof'] is True
assert set(rows[0])=={'mode','pid','models_root','audio_root','request','eof'}
assert rows[0]['request']=={'protocol_version':1,'request_id':'release-asr','session_id':None,'model_version':None,'op':'hello'}
try: os.kill(rows[0]['pid'],0)
except ProcessLookupError: pass
else: raise AssertionError('recorded ASR worker PID remains alive')
PY_PACKAGED_ASR_ROW_66
  fi
done

init_notary_fixture packaging-malformed-declaration spaces 1
packaging_run "$TEMP/packaging-malformed-declaration" STUB_PROVENANCE_FAULT=malformed-declaration
packaging_assert_failure "$TEMP/packaging-malformed-declaration" malformed-declaration 'Dependency identity manifest has a missing, malformed, duplicate, or incorrect Sidecar Package.resolved SHA-256: declaration.'
packaging_assert_hello_rows "$TEMP/packaging-malformed-declaration" 2 malformed-declaration
packaging_prefix_count "$TEMP/packaging-malformed-declaration" 'xcrun:notarytool submit ' 0
packaging_prefix_count "$TEMP/packaging-malformed-declaration" 'hdiutil:create ' 0
packaging_prefix_count "$TEMP/packaging-malformed-declaration" 'hdiutil:attach ' 0

for packaging_case in \
  'notice|Packaged third-party notice differs from its repository source.' \
  'models|Packaged Models file paths or contents differ from repository sources.' \
  'upstream-path|Packaged Upstream file paths or contents differ from repository sources.' \
  'sidecar-lock|Packaged Sidecar Package.resolved SHA-256: differs from its repository source.' \
  'xcode-lock|Packaged Xcode Package.resolved SHA-256: differs from its repository source.'; do
  packaging_fault="${packaging_case%%|*}"; packaging_diag="${packaging_case#*|}"
  init_notary_fixture "packaging-mounted-$packaging_fault" spaces 1
  packaging_run "$TEMP/packaging-mounted-$packaging_fault" "STUB_MOUNT_TAMPER=$packaging_fault"
  packaging_assert_failure "$TEMP/packaging-mounted-$packaging_fault" "$packaging_fault" "$packaging_diag"
  packaging_assert_hello_rows "$TEMP/packaging-mounted-$packaging_fault" 2 "$packaging_fault"
  packaging_prefix_count "$TEMP/packaging-mounted-$packaging_fault" 'xcrun:notarytool submit ' 2
  packaging_prefix_count "$TEMP/packaging-mounted-$packaging_fault" 'hdiutil:attach ' 1
  packaging_prefix_count "$TEMP/packaging-mounted-$packaging_fault" 'hdiutil:detach ' 1
  python3 - "$TEMP/packaging-mounted-$packaging_fault" "$packaging_fault" <<'PY_PACKAGED_MOUNT_PHASE_66' || fail "$packaging_fault mounted gate ordering, source, or built app changed"
import hashlib, sys
from pathlib import Path
root, kind = Path(sys.argv[1]), sys.argv[2]
lines=(root/'commands.log').read_text().splitlines()
attach=next(i for i,x in enumerate(lines) if x.startswith('hdiutil:attach '))
detach=[i for i,x in enumerate(lines) if x.startswith('hdiutil:detach ')]
assert len(detach)==1 and attach<detach[0]
assert not any(x.startswith('spctl:') for x in lines[attach+1:])
assert not any(x.startswith('xcrun:stapler validate ') and '/mount/' in x for x in lines[attach+1:])
source=root/'THIRD_PARTY_NOTICES.md'; resources=root/'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents/Resources'
def digest(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def tree(p): return {x.relative_to(p).as_posix():digest(x) for x in p.rglob('*') if x.is_file() and not x.is_symlink()}
if kind=='notice': assert digest(source)==digest(resources/'THIRD_PARTY_NOTICES.md')
elif kind=='models': assert tree(root/'ThirdPartyLicenses/Models')==tree(resources/'ThirdPartyLicenses/Models')
elif kind=='upstream-path': assert tree(root/'ThirdPartyLicenses/Upstream')==tree(resources/'ThirdPartyLicenses/Upstream')
elif kind=='sidecar-lock': assert digest(root/'src-tauri/mlx-sidecar/Package.resolved')==digest(resources/'ThirdPartyLicenses/DependencyManifest/Package.resolved')
elif kind=='xcode-lock':
    source=root/'src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved'
    assert digest(source)==digest(resources/'ThirdPartyLicenses/DependencyManifest/Xcode-Package.resolved')
else: raise AssertionError(kind)
PY_PACKAGED_MOUNT_PHASE_66
done
# End packaged sidecar and provenance verification regressions.

make_fixture "$TEMP/default-arm64"
mkdir -p "$TEMP/default-arm64/.build/release"
printf 'private artifact\n' > "$TEMP/default-arm64/.build/release/VoiceFlow.dmg"
run_fixture "$TEMP/default-arm64" env "${valid_env[@]}" \
  STUB_EXPECTED_TARGET=aarch64-apple-darwin make notarize \
  >"$TEMP/default-arm64.out" 2>&1 || fail "default arm64 notarization did not complete"
[[ -s "$TEMP/default-arm64/.build/release/VoiceFlow-notarized.dmg" ]] || fail "default arm64 DMG was not staged"
[[ -s "$TEMP/default-arm64/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "default arm64 receipt was not published"
grep -qx 'private artifact' "$TEMP/default-arm64/.build/release/VoiceFlow.dmg" || fail "notarization replaced the private DMG"
python3 - "$TEMP/default-arm64" <<'PYRECEIPT' || fail "receipt ids/statuses or post-staple final hash did not match published bytes"
import hashlib,json,sys,pathlib
root=pathlib.Path(sys.argv[1]); r=json.loads((root/'.build/release/VoiceFlow-notarized.receipt.json').read_text())
dmg=(root/'.build/release/VoiceFlow-notarized.dmg').read_bytes()
assert r['app']['submission_id']=='stub-submission-1' and r['app']['status']=='Accepted'
assert r['dmg']['submission_id']=='stub-submission-2' and r['dmg']['status']=='Accepted'
assert r['app']['zip_sha256']==(root/'submit-1.sha').read_text()
assert r['dmg']['submitted_sha256']==(root/'submit-2.sha').read_text()
assert r['final_dmg_sha256']==hashlib.sha256(dmg).hexdigest()
assert r['dmg']['submitted_sha256'] != r['final_dmg_sha256']
submitted_dmg=(root/'submit-2.artifact').read_bytes()
assert b'IMAGE-MARKER\n' in submitted_dmg and b'APP-TICKET' in submitted_dmg
assert b'DMG-TICKET' not in submitted_dmg and b'DMG-TICKET' in dmg
assert b'APP-TICKET' in dmg
import io,tarfile
payload=submitted_dmg[len(b'IMAGE-MARKER\n'):].split(b'\nIMAGE-END\n',1)[0]
with tarfile.open(fileobj=io.BytesIO(payload),mode='r:') as image:
 mounted_main=image.extractfile('./VoiceFlow-1.app/Contents/MacOS/VoiceFlow').read()
expected=(root/'expected-VoiceFlow-producer').read_bytes()
ticket=b'\nAPP-TICKET\n'
assert mounted_main == expected + ticket
assert mounted_main.count(b'APP-TICKET') == 1
import zipfile
with zipfile.ZipFile(root/'submit-1.artifact') as z:
 app_exe=next(z.read(n) for n in z.namelist() if n.endswith('/Contents/MacOS/VoiceFlow'))
assert app_exe == expected
built=(root/'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents/MacOS/VoiceFlow').read_bytes()
assert built == expected + ticket
assert b'\nAPP-TICKET\n' in built and built.count(b'APP-TICKET') == 1
PYRECEIPT
rg -q '^rustup:target add aarch64-apple-darwin$' "$TEMP/default-arm64/commands.log" \
  || fail "default arm64 target list was not used"
rg -q '^npm:run tauri -- build --target aarch64-apple-darwin --bundles app$' "$TEMP/default-arm64/commands.log" \
  || fail "default arm64 Tauri target was not used"
[[ -d "$TEMP/default-arm64/src-tauri/target/aarch64-apple-darwin/release/bundle" ]] \
  || fail "default arm64 output directory was not used"
[[ "$(rg -c '^lipo:' "$TEMP/default-arm64/commands.log")" == 2 ]] \
  || fail "default arm64 architecture was not checked on both original and mounted apps"
! rg -q '^security:' "$TEMP/default-arm64/commands.log" \
  || fail "release queried the global signing keychain"
[[ "$(rg -c '^xcrun:notarytool submit' "$TEMP/default-arm64/commands.log")" == 2 ]] || fail "app and DMG were not independently submitted"
python3 - "$TEMP/default-arm64/commands.log" <<'PYORDER' || fail "App/DMG notarization steps were out of order"
import sys
lines=open(sys.argv[1]).read().splitlines()
def at(prefix):
 hits=[i for i,x in enumerate(lines) if x.startswith(prefix)]
 assert hits, prefix
 return hits[0]
sequence=[at('xcrun:notarytool submit '),at('xcrun:stapler staple '),at('xcrun:stapler validate '),at('hdiutil:create ')]
assert sequence == sorted(sequence)
submits=[i for i,x in enumerate(lines) if x.startswith('xcrun:notarytool submit ')]
staples=[i for i,x in enumerate(lines) if x.startswith('xcrun:stapler staple ')]
validates=[i for i,x in enumerate(lines) if x.startswith('xcrun:stapler validate ')]
assert len(submits)==len(staples)==2 and len(validates)>=3
assert submits[0] < staples[0] < validates[0] < at('hdiutil:create ')
assert at('hdiutil:create ') < submits[1] < staples[1] < validates[1]
assert validates[1] < at('hdiutil:attach ')
assert at('hdiutil:attach ') < validates[2] < at('spctl:') < at('hdiutil:detach ')
PYORDER

make_fixture "$TEMP/universal"
run_fixture "$TEMP/universal" env "${valid_env[@]}" \
  VOICEFLOW_RELEASE_TARGET=universal-apple-darwin \
  STUB_EXPECTED_TARGET=universal-apple-darwin STUB_ARCHS='x86_64 arm64' make notarize \
  >"$TEMP/universal.out" 2>&1 || fail "explicit universal notarization did not complete"
[[ -s "$TEMP/universal/.build/release/VoiceFlow-notarized.dmg" ]] || fail "universal DMG was not staged"
rg -q '^rustup:target add aarch64-apple-darwin x86_64-apple-darwin$' "$TEMP/universal/commands.log" \
  || fail "universal Rust target list was not used"
rg -q '^npm:run tauri -- build --target universal-apple-darwin --bundles app$' "$TEMP/universal/commands.log" \
  || fail "universal Tauri target was not used"
[[ -d "$TEMP/universal/src-tauri/target/universal-apple-darwin/release/bundle" ]] \
  || fail "universal output directory was not used"
[[ "$(rg -c '^lipo:' "$TEMP/universal/commands.log")" == 2 ]] \
  || fail "universal architecture was not checked on both original and mounted apps"

make_fixture "$TEMP/direct-default"
mkdir -p "$TEMP/direct-default/.build/release"
printf 'private direct artifact\n' > "$TEMP/direct-default/.build/release/VoiceFlow.dmg"
run_fixture "$TEMP/direct-default" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  VOICEFLOW_NOTARIZE=1 bash scripts/release-macos.sh >"$TEMP/direct-default.out" 2>&1 \
  || fail "direct wrapper default destination did not complete"
[[ -s "$TEMP/direct-default/.build/release/VoiceFlow-notarized.dmg" && -s "$TEMP/direct-default/.build/release/VoiceFlow-notarized.receipt.json" ]] \
  || fail "direct wrapper did not use the separate notarized artifact pair"
grep -qx 'private direct artifact' "$TEMP/direct-default/.build/release/VoiceFlow.dmg" \
  || fail "direct notarization replaced the private DMG"

assert_positive_producer_records() {
  local fixture="$1"
  python3 -B - "$fixture" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
try:
    work_record = root / ".voiceflow-notary-workdir"
    work = Path(work_record.read_text(encoding="utf-8").strip())
    producer = root / "app-producer.jsonl"
    lines = producer.read_text(encoding="utf-8").splitlines()
    expected_fields = {"phase", "argv", "eof", "wav_path", "wav_exists", "nonce", "fault", "rc", "diagnostic"}
    phases = ("production-entry-prezip", "production-entry-mounted")
    normal_argv = (
        ["--import-test-groq-key-release-probe"],
        ["--test-groq-key-status-release-probe"],
        ["--research-stdio"],
    )
    diagnostics = (
        "test_groq_key_cli_requires_research_harness_feature",
        "test_groq_key_cli_requires_research_harness_feature",
        "research_stdio_requires_macos_research_harness_feature",
        "owned_wav_replay_status=unsupported_build",
    )
    wav = "00000000-0000-4000-8000-000000000001"
    reject = lambda: (_ for _ in ()).throw(ValueError("record assertion failed"))
    if len(lines) != 8:
        reject()
    rows = [json.loads(line) for line in lines]
    for phase_index, phase in enumerate(phases):
        chunk = rows[phase_index * 4:(phase_index + 1) * 4]
        for index, row in enumerate(chunk):
            if set(row) != expected_fields or row["phase"] != phase or row["eof"] is not True or row["fault"] != "none":
                reject()
            if index < 3:
                expected_rc = (2, 2, 0)[index]
                if row["argv"] != normal_argv[index] or row["rc"] != expected_rc or row["diagnostic"] != diagnostics[index]:
                    reject()
                if any(row[name] is not None for name in ("wav_path", "wav_exists", "nonce")):
                    reject()
            else:
                expected_path = str(work / phase / "nonexistent-release-probe.wav")
                if row["argv"] != ["--research-owned-wav", expected_path, "--target-nonce", wav] or row["wav_path"] != expected_path or row["wav_exists"] is not False or row["nonce"] != wav:
                    reject()
                if row["rc"] != 0 or row["diagnostic"] != diagnostics[3]:
                    reject()
except (OSError, UnicodeError, json.JSONDecodeError, ValueError, KeyError, IndexError, TypeError):
    raise SystemExit("positive producer JSONL assertion failed: " + str(root))
PY
}

assert_positive_producer_records "$TEMP/default-arm64"
assert_positive_producer_records "$TEMP/universal"
assert_positive_producer_records "$TEMP/direct-default"

assert_positive_native_and_wholebytes() {
  local fixture="$1" target="$2"
  python3 -B - "$fixture" "$target" <<'PYNATIVE' || fail "positive native/full-bytes assertion failed: $fixture"
import pathlib
import sys
import zipfile

root = pathlib.Path(sys.argv[1])
target = sys.argv[2]
try:
    raw_work = (root / ".voiceflow-notary-workdir").read_bytes()
    if not raw_work.endswith(b"\n") or raw_work.count(b"\n") != 1:
        raise ValueError("invalid workdir record")
    work = pathlib.Path(raw_work[:-1].decode("utf-8"))
    built = root / "src-tauri" / "target" / target / "release/bundle/macos/VoiceFlow-1.app/Contents"
    mounted = work / "mount/VoiceFlow-1.app/Contents"
    roles = (
        "MacOS/VoiceFlow",
        "Resources/voiceflow-mlx-sidecar",
        "Resources/libswiftCompatibilitySpan.dylib",
        "Resources/libswiftCompatibilityConcurrency.dylib",
        "Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture",
    )
    log_lines = (root / "commands.log").read_text(encoding="utf-8").splitlines()
    for phase, contents in (("prezip", built), ("mounted", mounted)):
        for role in roles:
            candidate = str(contents / role)
            expected_commands = (
                "file:-E -b " + candidate + " fault=none phase=" + phase,
                "strings:-a " + candidate + " fault=none phase=" + phase,
            )
            for command in expected_commands:
                if log_lines.count(command) != 1:
                    raise ValueError("native command log mismatch: " + command)
        notice = str(contents / "Resources/THIRD_PARTY_NOTICES.md")
        if log_lines.count("file:-E -b " + notice + " fault=none phase=" + phase) != 1:
            raise ValueError("notice classification log mismatch")
        if log_lines.count("strings:-a " + notice + " fault=none phase=" + phase) != 0:
            raise ValueError("notice was passed to strings")
        scan = work / ("production-entry-" + phase) / "native-strings.txt"
        scan_command = "rg:-n -F /Users/ " + str(scan) + " fault=none phase=" + phase
        if log_lines.count(scan_command) != 1:
            raise ValueError("native strings scan log mismatch")
    expected = (root / "expected-VoiceFlow-producer").read_bytes()
    with zipfile.ZipFile(root / "submit-1.artifact") as archive:
        member = "VoiceFlow-1.app/Contents/MacOS/VoiceFlow"
        if archive.namelist().count(member) != 1 or archive.read(member) != expected:
            raise ValueError("submitted ZIP executable differs from producer baseline")
    ticketed = expected + b"\nAPP-TICKET\n"
    candidates = (
        built / "MacOS/VoiceFlow",
        root / "image-app-marker.dir/VoiceFlow-1.app/Contents/MacOS/VoiceFlow",
    )
    for candidate in candidates:
        if candidate.read_bytes() != ticketed:
            raise ValueError("ticketed executable bytes mismatch: " + str(candidate))
except (OSError, UnicodeError, ValueError, zipfile.BadZipFile) as exc:
    raise SystemExit(str(exc))
PYNATIVE
}

assert_positive_native_and_wholebytes "$TEMP/default-arm64" aarch64-apple-darwin
assert_positive_native_and_wholebytes "$TEMP/universal" universal-apple-darwin
assert_positive_native_and_wholebytes "$TEMP/direct-default" aarch64-apple-darwin

assert_producer_negative_rows() {
  local fixture="$1" fault="$2" count="$3" fault_rc="$4" fault_diag="${5:-}" phase="${6:-prezip}"
  python3 -B - "$fixture" "$fault" "$count" "$fault_rc" "$fault_diag" "$phase" <<'PYPRODUCER' || fail "$fault producer rows did not match the expected boundary"
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
fault, count, fault_rc, fault_diag, final_phase = sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5], sys.argv[6]
expected = [
    (["--import-test-groq-key-release-probe"], 2, "test_groq_key_cli_requires_research_harness_feature"),
    (["--test-groq-key-status-release-probe"], 2, "test_groq_key_cli_requires_research_harness_feature"),
    (["--research-stdio"], 0, "research_stdio_requires_macos_research_harness_feature"),
    (["--research-owned-wav", None, "--target-nonce", "00000000-0000-4000-8000-000000000001"], 0, "owned_wav_replay_status=unsupported_build"),
]
try:
    rows = [json.loads(line) for line in (root / "app-producer.jsonl").read_text(encoding="utf-8").splitlines()]
    if len(rows) != count or count not in (1, 2, 3, 4, 5): raise ValueError("producer row count mismatch")
    for index, row in enumerate(rows):
        if set(row) != {"phase", "argv", "eof", "wav_path", "wav_exists", "nonce", "fault", "rc", "diagnostic"}: raise ValueError("producer schema mismatch")
        phase = "production-entry-mounted" if final_phase == "mounted" and index == 4 else "production-entry-prezip"
        argv, rc, diag = expected[index % len(expected)]
        if index == count - 1:
            if fault in ("import-exit", "mounted-import-exit"): rc = 7
            elif fault == "status-diagnostic": diag = "wrong_status_diagnostic"
            elif fault == "wav-exit": rc = 9
        if index == 3:
            work = pathlib.Path((root / ".voiceflow-notary-workdir").read_text(encoding="utf-8").rstrip("\n"))
            argv = [argv[0], str(work / "production-entry-prezip/nonexistent-release-probe.wav"), argv[2], argv[3]]
        if (row["phase"] != phase or row["argv"] != argv or row["eof"] is not True or
            row["fault"] != fault or row["rc"] != rc or row["diagnostic"] != diag):
            raise ValueError("producer row mismatch at index %d" % index)
        if index == 3:
            if row["wav_path"] != argv[1] or row["wav_exists"] is not False or row["nonce"] != argv[3]: raise ValueError("WAV probe fields mismatch")
        elif row["wav_path"] is not None or row["wav_exists"] is not None or row["nonce"] is not None: raise ValueError("unexpected WAV fields")
    row = rows[count - 1]
    if row["rc"] != fault_rc or (fault_diag and row["diagnostic"] != fault_diag): raise ValueError("selected fault result mismatch")
    if fault == "status-diagnostic" and row["diagnostic"] != "wrong_status_diagnostic": raise ValueError("status diagnostic fault absent")
except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as exc:
    raise SystemExit(str(exc))
PYPRODUCER
}

assert_producer_negative_log_boundaries() {
  local fixture="$1" phase="$2"
  python3 -B - "$fixture" "$phase" <<'PYPRODUCERLOG' || fail "$phase producer command log crossed its expected boundary"
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
phase = sys.argv[2]
try:
    if phase not in ("prezip", "mounted"):
        raise ValueError("invalid producer phase")
    raw_work = (root / ".voiceflow-notary-workdir").read_bytes()
    if not raw_work.endswith(b"\n") or raw_work.count(b"\n") != 1:
        raise ValueError("invalid workdir record")
    work = pathlib.Path(raw_work[:-1].decode("utf-8"))
    built = root / "src-tauri" / "target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents"
    mounted = work / "mount/VoiceFlow-1.app/Contents"
    mounted_app = work / "mount/VoiceFlow-1.app"
    roles = (
        "MacOS/VoiceFlow",
        "Resources/voiceflow-mlx-sidecar",
        "Resources/libswiftCompatibilitySpan.dylib",
        "Resources/libswiftCompatibilityConcurrency.dylib",
        "Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture",
    )
    lines = (root / "commands.log").read_text(encoding="utf-8").splitlines()
    built_prefix = str(built) + "/"
    mounted_prefix = str(mounted) + "/"
    prezip_scan = str(work / "production-entry-prezip" / "native-strings.txt")
    mounted_scan = str(work / "production-entry-mounted" / "native-strings.txt")
    built_native = tuple(
        command
        for role in roles
        for command in (
            "file:-E -b " + str(built / role) + " fault=none phase=prezip",
            "strings:-a " + str(built / role) + " fault=none phase=prezip",
        )
    )
    if phase == "prezip":
        if any(line.startswith(("file:-E -b " + built_prefix, "strings:-a " + built_prefix)) for line in lines):
            raise ValueError("prezip producer ran native file or strings inventory")
        if any(line.startswith("find:" + str(built) + " ") for line in lines):
            raise ValueError("prezip producer ran native find inventory")
        if any(line.startswith("rg:-n -F /Users/ ") and line.endswith(" phase=prezip") for line in lines):
            raise ValueError("prezip producer ran native strings scan")
        if any(line.startswith(("ditto:", "xcrun:notarytool submit ", "hdiutil:attach ")) for line in lines):
            raise ValueError("prezip producer reached ZIP or notarization/image attachment")
    else:
        for command in built_native:
            if lines.count(command) != 1:
                raise ValueError("built native command log mismatch: " + command)
        scan_command = "rg:-n -F /Users/ " + prezip_scan + " fault=none phase=prezip"
        if lines.count(scan_command) != 1:
            raise ValueError("prezip native strings scan log mismatch")
        if sum(line.startswith("hdiutil:attach ") for line in lines) != 1:
            raise ValueError("image attach command count mismatch")
        if sum(line.startswith("hdiutil:detach ") for line in lines) != 1:
            raise ValueError("image detach command count mismatch")
        if any(line.startswith(("file:-E -b " + mounted_prefix, "strings:-a " + mounted_prefix)) for line in lines):
            raise ValueError("mounted producer ran native file or strings inventory")
        if any(line.startswith("find:" + str(mounted) + " ") for line in lines):
            raise ValueError("mounted producer ran native find inventory")
        if "rg:-n -F /Users/ " + mounted_scan + " fault=none phase=mounted" in lines or any(
            line.startswith("rg:-n -F /Users/ ") and line.endswith(" phase=mounted") for line in lines
        ):
            raise ValueError("mounted producer ran native strings scan")
        if "xcrun:stapler validate " + str(mounted_app) in lines:
            raise ValueError("mounted app received a stapler validation")
        if any(line.startswith("spctl:") and str(mounted_app) in line for line in lines):
            raise ValueError("mounted app received gatekeeper assessment")
except (OSError, UnicodeError, ValueError) as exc:
    raise SystemExit(str(exc))
PYPRODUCERLOG
}

run_native_leak_case() {
  local name="$1" fault="$2" phase="$3" fixture gate
  gate='Packaged native binaries contain a developer-home path.'
  init_notary_fixture "$name" spaces 1
  fixture="$TEMP/$name"
  packaging_run "$fixture" "STUB_NATIVE_FAULT=$fault"
  packaging_assert_failure "$fixture" "$name" "$gate"
  rg -F -x -- "$gate" "$fixture/wrapper.out" >/dev/null || fail "$name wrapper output did not contain exact gate line"
  python3 -B - "$fixture" "$fault" "$phase" <<'PY NATIVELEAK' || fail "$name native leak boundary mismatch"
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
fault, phase = sys.argv[2:]
roles = (
    "MacOS/VoiceFlow",
    "Resources/voiceflow-mlx-sidecar",
    "Resources/libswiftCompatibilitySpan.dylib",
    "Resources/libswiftCompatibilityConcurrency.dylib",
    "Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture",
)
try:
    work = pathlib.Path((root / ".voiceflow-notary-workdir").read_text(encoding="utf-8").rstrip("\n"))
    built = root / "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents"
    mounted = work / "mount/VoiceFlow-1.app/Contents"
    log = (root / "commands.log").read_text(encoding="utf-8").splitlines()
    rows = [json.loads(line) for line in (root / "app-producer.jsonl").read_text(encoding="utf-8").splitlines()]
    phases = ["prezip"] if phase == "prezip" else ["prezip", "mounted"]
    expected_rows = []
    for row_phase in phases:
        wav = str(work / ("production-entry-" + row_phase) / "nonexistent-release-probe.wav")
        argv = [
            ["--import-test-groq-key-release-probe"],
            ["--test-groq-key-status-release-probe"],
            ["--research-stdio"],
            ["--research-owned-wav", wav, "--target-nonce", "00000000-0000-4000-8000-000000000001"],
        ]
        diagnostics = [
            "test_groq_key_cli_requires_research_harness_feature",
            "test_groq_key_cli_requires_research_harness_feature",
            "research_stdio_requires_macos_research_harness_feature",
            "owned_wav_replay_status=unsupported_build",
        ]
        for args, diagnostic in zip(argv, diagnostics):
            expected_rows.append({
                "phase": "production-entry-" + row_phase,
                "argv": args,
                "eof": True,
                "wav_path": args[1] if args[0] == "--research-owned-wav" else None,
                "wav_exists": False if args[0] == "--research-owned-wav" else None,
                "nonce": args[3] if args[0] == "--research-owned-wav" else None,
                "fault": "none",
                "rc": 2 if args[0] in ("--import-test-groq-key-release-probe", "--test-groq-key-status-release-probe") else 0,
                "diagnostic": diagnostic,
            })
    if len(rows) != len(expected_rows) or any(set(row) != {"phase", "argv", "eof", "wav_path", "wav_exists", "nonce", "fault", "rc", "diagnostic"} for row in rows):
        raise ValueError("producer row count or schema mismatch")
    if rows != expected_rows:
        raise ValueError("producer rows differ from independent normal-result table")
    for row_phase in phases:
        contents = built if row_phase == "prezip" else mounted
        find_line = "find:" + str(contents) + " -type f -print0 fault=" + fault + " phase=" + row_phase
        if log.count(find_line) != 1:
            raise ValueError("native file inventory count mismatch")
        for role in roles:
            path = str(contents / role)
            if log.count("file:-E -b " + path + " fault=" + fault + " phase=" + row_phase) != 1:
                raise ValueError("native file classification count mismatch: " + path)
            if log.count("strings:-a " + path + " fault=" + fault + " phase=" + row_phase) != 1:
                raise ValueError("native strings extraction count mismatch: " + path)
        scan = str(work / ("production-entry-" + row_phase) / "native-strings.txt")
        scan_line = "rg:-n -F /Users/ " + scan + " fault=" + fault + " phase=" + row_phase
        if log.count(scan_line) != 1:
            raise ValueError("native developer-home scan count mismatch")
    for row_phase in phases:
        contents = built if row_phase == "prezip" else mounted
        notice = str(contents / "Resources/THIRD_PARTY_NOTICES.md")
        if log.count("file:-E -b " + notice + " fault=" + fault + " phase=" + row_phase) != 1:
            raise ValueError("third-party notice classification count mismatch: " + row_phase)
        notice_strings = "strings:-a " + notice + " fault=" + fault + " phase=" + row_phase
        if any(line.startswith(notice_strings) for line in log):
            raise ValueError("third-party notice was passed to strings: " + row_phase)
    if phase == "prezip":
        if any(line.startswith(("strings:-a " + str(mounted) + "/", "file:-E -b " + str(mounted) + "/", "find:" + str(mounted) + " ")) for line in log):
            raise ValueError("built leak continued into mounted inspection")
        if any(line.startswith(("ditto:", "xcrun:notarytool submit ", "hdiutil:attach ", "hdiutil:detach ", "spctl:")) for line in log):
            raise ValueError("built leak crossed ZIP, notarization, image, or Gatekeeper boundary")
    else:
        if sum(line.startswith("hdiutil:attach ") for line in log) != 1 or sum(line.startswith("hdiutil:detach ") for line in log) != 1:
            raise ValueError("mounted leak attach/detach count mismatch")
        mounted_app = str(work / "mount/VoiceFlow-1.app")
        if "xcrun:stapler validate " + mounted_app in log or any(line.startswith("spctl:") and mounted_app in line for line in log):
            raise ValueError("mounted app received stapler or Gatekeeper validation")
        zip_submission = "xcrun:notarytool submit " + str(work / "VoiceFlow.zip")
        dmg_submission = "xcrun:notarytool submit " + str(work / "VoiceFlow-notarized.dmg")
        if sum(line.startswith(zip_submission + " ") for line in log) != 1 or sum(line.startswith(dmg_submission + " ") for line in log) != 1:
            raise ValueError("mounted leak did not observe exactly one App ZIP and DMG submission")
        submissions = [index for index, line in enumerate(log) if line.startswith("xcrun:notarytool submit ")]
        attachment = next(index for index, line in enumerate(log) if line.startswith("hdiutil:attach "))
        if len(submissions) != 2 or any(index >= attachment for index in submissions):
            raise ValueError("mounted leak submission count or attach ordering mismatch")
        if any(line.startswith("spctl:") for line in log):
            raise ValueError("mounted leak reached Gatekeeper assessment")
except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as exc:
    raise SystemExit(str(exc))
PY NATIVELEAK
}

run_native_leak_case native-leak-main leak-main prezip
run_native_leak_case native-leak-sidecar leak-sidecar prezip
run_native_leak_case native-leak-span leak-span prezip
run_native_leak_case native-leak-runtime leak-runtime prezip
run_native_leak_case native-leak-framework leak-framework prezip
run_native_leak_case native-mounted-leak-framework mounted-leak-framework mounted

run_native_io_case() {
  local name="$1" fault="$2" fixture gate candidate
  init_notary_fixture "$name" spaces 1
  fixture="$TEMP/$name"
  packaging_run "$fixture" "STUB_NATIVE_FAULT=$fault"
  if [[ "$fault" == strings-partial-fail ]]; then
    candidate="$(sed -n 's/^strings:-a \(.*\) fault=strings-partial-fail phase=prezip$/\1/p' "$fixture/commands.log")"
    [[ -n "$candidate" ]] || fail "$name has no logged strings candidate"
    gate="Could not extract strings from packaged Mach-O binary: $candidate"
  elif [[ "$fault" == file-io-extra ]]; then
    candidate="$fixture/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents/Resources/libswiftCompatibilityConcurrency.dylib"
    gate="Could not classify packaged app file: $candidate"
  else
    gate='Could not complete the packaged native string path scan (rg status 2).'
  fi
  packaging_assert_failure "$fixture" "$name" "$gate"
  rg -F -x -- "$gate" "$fixture/wrapper.out" >/dev/null || fail "$name wrapper output did not contain exact gate line"
  python3 -B - "$fixture" "$fault" <<'PY NATIVEIO' || fail "$name native I/O boundary mismatch"
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
fault = sys.argv[2]
roles = (
    "MacOS/VoiceFlow",
    "Resources/voiceflow-mlx-sidecar",
    "Resources/libswiftCompatibilitySpan.dylib",
    "Resources/libswiftCompatibilityConcurrency.dylib",
    "Resources/Frameworks/VoiceFlowFixture.framework/VoiceFlowFixture",
)
try:
    work = pathlib.Path((root / ".voiceflow-notary-workdir").read_text(encoding="utf-8").rstrip("\n"))
    built = root / "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents"
    log = (root / "commands.log").read_text(encoding="utf-8").splitlines()
    rows = [json.loads(line) for line in (root / "app-producer.jsonl").read_text(encoding="utf-8").splitlines()]
    expected_args = (
        ["--import-test-groq-key-release-probe"],
        ["--test-groq-key-status-release-probe"],
        ["--research-stdio"],
        ["--research-owned-wav", str(work / "production-entry-prezip/nonexistent-release-probe.wav"),
         "--target-nonce", "00000000-0000-4000-8000-000000000001"],
    )
    expected_diagnostics = (
        "test_groq_key_cli_requires_research_harness_feature",
        "test_groq_key_cli_requires_research_harness_feature",
        "research_stdio_requires_macos_research_harness_feature",
        "owned_wav_replay_status=unsupported_build",
    )
    if len(rows) != 4:
        raise ValueError("normal producer row count mismatch")
    for index, row in enumerate(rows):
        if (set(row) != {"phase", "argv", "eof", "wav_path", "wav_exists", "nonce", "fault", "rc", "diagnostic"}
            or row["phase"] != "production-entry-prezip" or row["argv"] != expected_args[index]
            or row["eof"] is not True or row["fault"] != "none"
            or row["rc"] != (2 if index < 2 else 0) or row["diagnostic"] != expected_diagnostics[index]):
            raise ValueError("producer row differs from independent normal result")
        if index == 3:
            if row["wav_path"] != expected_args[index][1] or row["wav_exists"] is not False or row["nonce"] != expected_args[index][3]:
                raise ValueError("owned WAV producer fields mismatch")
        elif row["wav_path"] is not None or row["wav_exists"] is not None or row["nonce"] is not None:
            raise ValueError("unexpected producer optional fields")
    find_line = "find:" + str(built) + " -type f -print0 fault=" + fault + " phase=prezip"
    if log.count(find_line) != 1:
        raise ValueError("native inventory command mismatch")
    mounted = work / "mount/VoiceFlow-1.app/Contents"
    if any(line.startswith(("file:-E -b " + str(mounted) + "/", "strings:-a " + str(mounted) + "/", "find:" + str(mounted) + " ")) for line in log):
        raise ValueError("prezip failure continued into mounted native inspection")
    if any(line.startswith("rg:-n -F /Users/ ") and line.endswith(" phase=mounted") for line in log):
        raise ValueError("prezip failure reached mounted native scan")
    notice = str(built / "Resources/THIRD_PARTY_NOTICES.md")
    for line in log:
        if line.startswith("file:-E -b ") and " phase=prezip" in line and not line.endswith(" fault=" + fault + " phase=prezip"):
            raise ValueError("native file classification fault/phase mismatch")
        if line.startswith("strings:-a ") and " phase=prezip" in line and not line.endswith(" fault=" + fault + " phase=prezip"):
            raise ValueError("native strings fault/phase mismatch")
    for role in roles:
        expected = "file:-E -b " + str(built / role) + " fault=" + fault + " phase=prezip"
        if fault != "file-io-extra" and log.count(expected) != 1:
            raise ValueError("native role classification missing: " + role)
    scan = "rg:-n -F /Users/ " + str(work / "production-entry-prezip/native-strings.txt") + " fault=" + fault + " phase=prezip"
    native_strings = [line for line in log if line.startswith("strings:-a ") and line.endswith(" fault=" + fault + " phase=prezip")]
    if any(line.startswith(("ditto:", "xcrun:notarytool submit ", "hdiutil:attach ", "hdiutil:detach ", "spctl:", "xcrun:stapler validate ")) for line in log):
        raise ValueError("native I/O failure crossed the prezip boundary")
    if fault == "strings-partial-fail":
        if len(native_strings) != 1 or sum(line.startswith("file:-E -b " + str(built / role) + " fault=" + fault + " phase=prezip") for line in log for role in roles) != 5:
            raise ValueError("partial strings failure did not follow complete role classification")
        if log.count("file:-E -b " + notice + " fault=" + fault + " phase=prezip") != 1:
            raise ValueError("notice classification missing")
        candidate = native_strings[0][len("strings:-a "):].split(" fault=", 1)[0]
        if candidate not in {str(built / role) for role in roles} or log.count(scan) != 0:
            raise ValueError("partial strings candidate or scan boundary mismatch")
        gate = "Could not extract strings from packaged Mach-O binary: " + candidate
    elif fault == "rg-error":
        if len(native_strings) != 5 or any(log.count("strings:-a " + str(built / role) + " fault=" + fault + " phase=prezip") != 1 for role in roles) or log.count(scan) != 1:
            raise ValueError("rg error did not follow full strings extraction and one scan")
        if log.count("file:-E -b " + notice + " fault=" + fault + " phase=prezip") != 1:
            raise ValueError("notice classification missing")
        gate = "Could not complete the packaged native string path scan (rg status 2)."
    elif fault == "file-io-extra":
        candidate = str(built / "Resources/libswiftCompatibilityConcurrency.dylib")
        expected = "file:-E -b " + candidate + " fault=" + fault + " phase=prezip"
        if log.count(expected) != 1 or native_strings or log.count(scan) != 0:
            raise ValueError("extra file classification error boundary mismatch")
        gate = "Could not classify packaged app file: " + candidate
    else:
        raise ValueError("unexpected native I/O fault")
    if gate not in (root / "wrapper.out").read_text(encoding="utf-8").splitlines():
        raise ValueError("exact native I/O gate missing")
except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as exc:
    raise SystemExit(str(exc))
PY NATIVEIO
}

run_native_io_case native-io-strings-partial strings-partial-fail
run_native_io_case native-io-rg-error rg-error
run_native_io_case native-io-file-extra file-io-extra

assert_native_inventory_negative_case() {
  local fixture="$1" name="$2" fault="$3" gate="$4"
  python3 -B - "$fixture" "$name" "$fault" "$gate" <<'PYNATIVEINVENTORY' || fail "$name native inventory boundary mismatch"
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
name, fault, gate = sys.argv[2:]
built = root / "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents"
span = built / "Resources/libswiftCompatibilitySpan.dylib"
try:
    log = (root / "commands.log").read_text(encoding="utf-8").splitlines()
    output = (root / "wrapper.out").read_text(encoding="utf-8").splitlines()
    work = pathlib.Path((root / ".voiceflow-notary-workdir").read_text(encoding="utf-8").rstrip("\n"))
    mounted = work / "mount/VoiceFlow-1.app/Contents"
    mounted_app = work / "mount/VoiceFlow-1.app"
    if output.count(gate) != 1:
        raise ValueError("expected exact gate line once")
    if any(line.startswith(("ditto:", "xcrun:notarytool submit ", "hdiutil:attach ", "hdiutil:detach ",
                           "spctl:", "xcrun:stapler validate ")) for line in log):
        raise ValueError("inventory failure crossed the prezip boundary")
    if any(line.startswith(("file:-E -b " + str(built), "strings:-a " + str(built), "find:" + str(built)))
           and " phase=mounted" in line for line in log):
        raise ValueError("prezip failure continued into mounted native inspection")
    if any(line.startswith(("file:-E -b " + str(mounted) + "/", "strings:-a " + str(mounted) + "/",
                           "find:" + str(mounted) + " ")) for line in log):
        raise ValueError("prezip failure ran mounted native inventory")
    if any(line.startswith("rg:-n -F /Users/ ") and line.endswith(" phase=mounted") for line in log):
        raise ValueError("prezip failure ran mounted native path scan")
    if "xcrun:stapler validate " + str(mounted_app) in log or any(
        line.startswith("spctl:") and str(mounted_app) in line for line in log
    ):
        raise ValueError("prezip failure validated or assessed the mounted app")
    if fault == "find-fail":
        expected_find = "find:" + str(built) + " -type f -print0 fault=find-fail phase=prezip"
        if log.count(expected_find) != 1:
            raise ValueError("failed native inventory command mismatch")
        if any(line.startswith(("file:-E -b " + str(built) + "/", "strings:-a " + str(built) + "/",
                               "rg:-n -F /Users/ ")) for line in log):
            raise ValueError("find failure continued to file, strings, or path scan")
        if not (root / "app-producer.jsonl").is_file():
            raise ValueError("find failure did not reach normal production probes")
    elif fault == "missing-span":
        if any(line.startswith(("find:" + str(built), "file:-E -b " + str(built) + "/",
                               "strings:-a " + str(built) + "/", "rg:-n -F /Users/ ")) for line in log):
            raise ValueError("missing span continued into native inventory or probes")
        if (root / "app-producer.jsonl").exists():
            raise ValueError("missing span unexpectedly created a producer log")
    elif fault == "nonmacho-span":
        expected_find = "find:" + str(built) + " -type f -print0 fault=nonmacho-span phase=prezip"
        expected_file = "file:-E -b " + str(span) + " fault=nonmacho-span phase=prezip"
        if log.count(expected_find) != 1 or log.count(expected_file) != 1:
            raise ValueError("non-Mach-O span inventory commands mismatch")
        if any(line.startswith(("strings:-a " + str(built) + "/", "rg:-n -F /Users/ ")) for line in log):
            raise ValueError("non-Mach-O span continued to strings or path scan")
        if not (root / "app-producer.jsonl").is_file():
            raise ValueError("non-Mach-O span did not reach normal production probes")
    else:
        raise ValueError("unexpected native inventory fault")
except (OSError, UnicodeError, ValueError) as exc:
    raise SystemExit(str(exc))
PYNATIVEINVENTORY
}

for native_inventory_case in \
  'find-fail|Could not enumerate packaged app files for the native binary scan.' \
  'missing-span|Required production native binary is missing or unreadable: ' \
  'nonmacho-span|Required production binary was not included in the Mach-O inventory: '; do
  native_inventory_fault="${native_inventory_case%%|*}"
  native_inventory_gate="${native_inventory_case#*|}"
  native_inventory_contents="$TEMP/native-inventory-$native_inventory_fault/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/VoiceFlow-1.app/Contents"
  case "$native_inventory_fault" in
    missing-span|nonmacho-span)
      native_inventory_gate+="$native_inventory_contents/Resources/libswiftCompatibilitySpan.dylib"
      ;;
  esac
  init_notary_fixture "native-inventory-$native_inventory_fault" spaces 1
  native_inventory_fixture="$TEMP/native-inventory-$native_inventory_fault"
  packaging_run "$native_inventory_fixture" "STUB_NATIVE_FAULT=$native_inventory_fault"
  packaging_assert_failure "$native_inventory_fixture" "native-inventory-$native_inventory_fault" "$native_inventory_gate"
  rg -F -x -- "$native_inventory_gate" "$native_inventory_fixture/wrapper.out" >/dev/null || fail "$native_inventory_fault wrapper output did not contain exact gate line"
  assert_native_inventory_negative_case "$native_inventory_fixture" "native-inventory-$native_inventory_fault" "$native_inventory_fault" "$native_inventory_gate"
  case "$native_inventory_fault" in
    find-fail)
      assert_producer_negative_rows "$native_inventory_fixture" none 4 0 '' prezip
      ;;
    missing-span)
      ;;
    nonmacho-span)
      assert_producer_negative_rows "$native_inventory_fixture" none 4 0 '' prezip
      ;;
  esac
done

run_producer_negative_case() {
  local name="$1" fault="$2" rows="$3" gate="$4" fault_rc="$5" fault_diag="${6:-}" phase="${7:-prezip}" fixture
  init_notary_fixture "$name" spaces 1
  fixture="$TEMP/$name"
  packaging_run "$fixture" "STUB_APP_PRODUCER_FAULT=$fault"
  packaging_assert_failure "$fixture" "$name" "$gate"
  rg -F -x -- "$gate" "$fixture/wrapper.out" >/dev/null || fail "$name wrapper output did not contain exact gate line"
  assert_producer_negative_rows "$fixture" "$fault" "$rows" "$fault_rc" "$fault_diag" "$phase"
  assert_producer_negative_log_boundaries "$fixture" "$phase"
}

run_producer_negative_case producer-import-exit import-exit 1   'Production CLI release probe returned an unexpected result (--import-test-groq-key-release-probe, status 7).' 7
run_producer_negative_case producer-status-diagnostic status-diagnostic 2   'Production CLI release probe returned an unexpected result (--test-groq-key-status-release-probe, status 2).' 2 wrong_status_diagnostic
run_producer_negative_case producer-stdio-stdout stdio-stdout 3   'Production CLI release probe returned an unexpected result (--research-stdio, status 0).' 0
run_producer_negative_case producer-wav-exit wav-exit 4   'Production owned-WAV release probe returned an unexpected result (status 9).' 9
run_producer_negative_case producer-mounted-import-exit mounted-import-exit 5   'Production CLI release probe returned an unexpected result (--import-test-groq-key-release-probe, status 7).' 7 '' mounted

for scenario in app-submit dmg-submit app-status app-rejected app-in-progress dmg-status dmg-invalid dmg-rejected malformed missing-id dmg-malformed dmg-missing-id ditto-fail app-staple app-validate dmg-create dmg-sign dmg-staple dmg-validate gatekeeper; do
  make_fixture "$TEMP/failure-$scenario"
  mkdir -p "$TEMP/failure-$scenario/.build/release"
  printf 'stale artifact\n' > "$TEMP/failure-$scenario/.build/release/VoiceFlow-notarized.dmg"
  printf '{"stale":true}\n' > "$TEMP/failure-$scenario/.build/release/VoiceFlow-notarized.receipt.json"
  case "$scenario" in
    app-submit) failure_env=(STUB_NOTARY_FAIL=1) ;;
    dmg-submit) failure_env=(STUB_NOTARY_FAIL=2) ;;
    app-status) failure_env=(STUB_APP_STATUS=Invalid) ;;
    app-rejected) failure_env=(STUB_APP_STATUS=Rejected) ;;
    app-in-progress) failure_env=(STUB_APP_STATUS='In Progress') ;;
    dmg-status) failure_env=(STUB_DMG_STATUS='In Progress') ;;
    dmg-invalid) failure_env=(STUB_DMG_STATUS=Invalid) ;;
    dmg-rejected) failure_env=(STUB_DMG_STATUS=Rejected) ;;
    malformed) failure_env=(STUB_NOTARY_JSON=malformed) ;;
    missing-id) failure_env=(STUB_NOTARY_JSON=missing-id) ;;
    dmg-malformed) failure_env=(STUB_DMG_NOTARY_JSON=malformed) ;;
    dmg-missing-id) failure_env=(STUB_DMG_NOTARY_JSON=missing-id) ;;
    ditto-fail) failure_env=(STUB_DITTO_FAIL=1) ;;
    app-staple) failure_env=(STUB_STAPLE_FAIL=VoiceFlow-1.app) ;;
    app-validate) failure_env=(STUB_VALIDATE_FAIL=VoiceFlow-1.app) ;;
    dmg-create) failure_env=(STUB_DMG_CREATE_FAIL=1) ;;
    dmg-sign) failure_env=(STUB_DMGSIGN_FAIL=1) ;;
    dmg-staple) failure_env=(STUB_STAPLE_FAIL=VoiceFlow-notarized.dmg) ;;
    dmg-validate) failure_env=(STUB_VALIDATE_FAIL=VoiceFlow-notarized.dmg) ;;
    gatekeeper) failure_env=(STUB_SPCTL_FAIL=1) ;;
  esac
  if run_fixture "$TEMP/failure-$scenario" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
    "${failure_env[@]}" make notarize >"$TEMP/failure-$scenario.out" 2>&1; then
    fail "$scenario failure was treated as releasable"
  fi
  [[ ! -e "$TEMP/failure-$scenario/.build/release/VoiceFlow-notarized.dmg" \
    && ! -e "$TEMP/failure-$scenario/.build/release/VoiceFlow-notarized.receipt.json" ]] \
    || fail "$scenario failure left a notarized artifact or receipt"
  if [[ "$scenario" == app-submit || "$scenario" == app-status || "$scenario" == app-rejected \
    || "$scenario" == app-in-progress || "$scenario" == malformed \
    || "$scenario" == missing-id || "$scenario" == ditto-fail || "$scenario" == app-staple \
    || "$scenario" == app-validate ]]; then
    ! rg -q '^hdiutil:create' "$TEMP/failure-$scenario/commands.log" \
      || fail "$scenario failure proceeded to DMG construction"
  fi
done

make_fixture "$TEMP/invalid-target"
if run_fixture "$TEMP/invalid-target" env "${valid_env[@]}" \
  VOICEFLOW_RELEASE_TARGET=arm64-apple-darwin \
  bash scripts/release-macos.sh "$TEMP/invalid-target/missing-parent/release.dmg" \
  >"$TEMP/invalid-target.out" 2>&1; then
  fail "an invalid target was accepted"
fi
grep -q 'Unsupported VOICEFLOW_RELEASE_TARGET' "$TEMP/invalid-target.out" \
  || fail "invalid target was not rejected by the whitelist"
[[ ! -e "$TEMP/invalid-target/missing-parent" && ! -e "$TEMP/invalid-target/src-tauri/target" ]] \
  || fail "invalid target created release output before rejection"
[[ ! -e "$TEMP/invalid-target/commands.log" ]] || fail "invalid target reached a stubbed system command"

make_fixture "$TEMP/missing"
if run_fixture "$TEMP/missing" env APPLE_SIGNING_IDENTITY='Developer ID Application: Stub (ABCDE12345)' \
  APPLE_TEAM_ID=ABCDE12345 APPLE_ID=stub@example.invalid APPLE_PASSWORD=not-a-real-password \
  bash scripts/release-macos.sh "$TEMP/missing/missing.dmg" >"$TEMP/missing.out" 2>&1; then
  fail "direct wrapper path accepted a missing VOICEFLOW_NOTARIZE gate"
fi
if run_fixture "$TEMP/missing" env make notarize >"$TEMP/missing-make.out" 2>&1; then
  fail "make notarize accepted missing Developer ID credentials"
fi
[[ ! -e "$TEMP/missing/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg" ]] \
  || fail "missing credentials reached the build stage"

make_fixture "$TEMP/certificate-pair"
if run_fixture "$TEMP/certificate-pair" env "${valid_env[@]}" APPLE_CERTIFICATE=stub-certificate \
  bash scripts/release-macos.sh "$TEMP/certificate-pair/release.dmg" \
  >"$TEMP/certificate-pair.out" 2>&1; then
  fail "an incomplete optional certificate/password pair was accepted"
fi
[[ ! -e "$TEMP/certificate-pair/commands.log" ]] \
  || fail "certificate-pair rejection reached a stubbed system command"

make_fixture "$TEMP/rejected"
if run_fixture "$TEMP/rejected" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_NOTARY_STATUS=Rejected make notarize >"$TEMP/rejected.out" 2>&1; then
  fail "a rejected notary result was treated as releasable"
fi
[[ ! -e "$TEMP/rejected/.build/release/VoiceFlow-notarized.dmg" ]] || fail "a rejected DMG was staged"
[[ -z "$(find "$TEMP/rejected/.build/release" -maxdepth 1 -name '.voiceflow-notary.*' -print -quit 2>/dev/null)" ]] \
  || fail "temporary notarization data survived rejection"

make_fixture "$TEMP/ambiguous-app"
if run_fixture "$TEMP/ambiguous-app" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_APP_COUNT=2 make notarize >"$TEMP/ambiguous-app.out" 2>&1; then
  fail "ambiguous current-build apps were accepted"
fi
! rg -q '^xcrun:notarytool submit' "$TEMP/ambiguous-app/commands.log" \
  || fail "notary submission started before app ambiguity was rejected"

make_fixture "$TEMP/wrong-arch-arm64"
if run_fixture "$TEMP/wrong-arch-arm64" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_ARCHS=x86_64 make notarize >"$TEMP/wrong-arch-arm64.out" 2>&1; then
  fail "an arm64 target accepted an x86_64-only App"
fi
! rg -q '^xcrun:notarytool submit' "$TEMP/wrong-arch-arm64/commands.log" \
  || fail "notary submission started before arm64 architecture validation"
make_fixture "$TEMP/extra-arch-arm64"
if run_fixture "$TEMP/extra-arch-arm64" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_ARCHS='arm64 x86_64' make notarize >"$TEMP/extra-arch-arm64.out" 2>&1; then
  fail "arm64 target accepted an unexpected extra architecture"
fi

make_fixture "$TEMP/wrong-arch-universal"
if run_fixture "$TEMP/wrong-arch-universal" env "${valid_env[@]}" \
  VOICEFLOW_RELEASE_TARGET=universal-apple-darwin STUB_EXPECTED_TARGET=universal-apple-darwin \
  STUB_ARCHS=arm64 make notarize >"$TEMP/wrong-arch-universal.out" 2>&1; then
  fail "universal target accepted a missing x86_64 slice"
fi
make_fixture "$TEMP/extra-arch-universal"
if run_fixture "$TEMP/extra-arch-universal" env "${valid_env[@]}" \
  VOICEFLOW_RELEASE_TARGET=universal-apple-darwin STUB_EXPECTED_TARGET=universal-apple-darwin \
  STUB_ARCHS='arm64 x86_64 arm64e' make notarize >"$TEMP/extra-arch-universal.out" 2>&1; then
  fail "universal target accepted an unexpected architecture"
fi

make_fixture "$TEMP/signature-failure"
if run_fixture "$TEMP/signature-failure" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_CODESIGN_FAIL=1 make notarize >"$TEMP/signature-failure.out" 2>&1; then
  fail "an invalid Developer ID app signature was accepted"
fi
! rg -q '^xcrun:notarytool submit' "$TEMP/signature-failure/commands.log" \
  || fail "notary submission started before signature validation"

make_fixture "$TEMP/gatekeeper-failure"
if run_fixture "$TEMP/gatekeeper-failure" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_SPCTL_FAIL=1 make notarize >"$TEMP/gatekeeper-failure.out" 2>&1; then
  fail "release succeeded after final Gatekeeper assessment failed"
fi
[[ ! -e "$TEMP/gatekeeper-failure/.build/release/VoiceFlow-notarized.dmg" ]] \
  || fail "a DMG was staged after Gatekeeper assessment failed"
! rg -q '^security:' "$TEMP/gatekeeper-failure/commands.log" \
  || fail "release changed or queried the global signing keychain"

make_fixture "$TEMP/detach-failure"
mkdir -p "$TEMP/detach-failure/.build/release"
printf 'stale notarized artifact\n' > "$TEMP/detach-failure/.build/release/VoiceFlow-notarized.dmg"
printf '{"stale":true}\n' > "$TEMP/detach-failure/.build/release/VoiceFlow-notarized.receipt.json"
if run_fixture "$TEMP/detach-failure" env "${valid_env[@]}" STUB_EXPECTED_TARGET=aarch64-apple-darwin \
  STUB_SPCTL_FAIL=1 STUB_DETACH_FAIL=1 make notarize >"$TEMP/detach-failure.out" 2>&1; then
  fail "release succeeded after verification volume cleanup failed"
fi
preserved_work="$(find "$TEMP/detach-failure/.build/release" -maxdepth 1 -type d -name '.voiceflow-notary.*' -print -quit)"
[[ -n "$preserved_work" && -d "$preserved_work/mount" ]] \
  || fail "temporary verification data was removed while its volume could still be mounted"
[[ ! -e "$TEMP/detach-failure/.build/release/VoiceFlow-notarized.dmg" && ! -e "$TEMP/detach-failure/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "detach failure left published notarized output"

mkdir -p "$TEMP/stage/current" "$TEMP/stage/other" "$TEMP/stage/destination"
printf 'current build\n' > "$TEMP/stage/current/VoiceFlow.dmg"
printf 'stale output\n' > "$TEMP/stage/other/VoiceFlow-old.dmg"
bash "$ROOT/scripts/stage-dmg.sh" "$TEMP/stage/current" "$TEMP/stage/destination/VoiceFlow.dmg" >/dev/null
cmp -s "$TEMP/stage/current/VoiceFlow.dmg" "$TEMP/stage/destination/VoiceFlow.dmg" \
  || fail "explicit current-build staging selected different bytes"
printf 'second candidate\n' > "$TEMP/stage/current/Other.dmg"
if bash "$ROOT/scripts/stage-dmg.sh" "$TEMP/stage/current" "$TEMP/stage/destination/ambiguous.dmg" \
  >"$TEMP/stage-ambiguous.out" 2>&1; then
  fail "staging accepted an ambiguous explicit build directory"
fi

# Legacy positive routes may retain workdir provenance, but leave no actual
# temporary workdir, keychain, certificate, or completed stage.
for legacy_fixture in default-arm64 universal direct-default; do
  legacy_root="$TEMP/$legacy_fixture"
  case "$legacy_fixture" in
    default-arm64|direct-default) [[ -s "$legacy_root/.build/release/VoiceFlow-notarized.dmg" && -s "$legacy_root/.build/release/VoiceFlow-notarized.receipt.json" ]] || fail "$legacy_fixture positive artifacts missing" ;;
    universal) [[ -s "$legacy_root/.build/release/VoiceFlow-notarized.dmg" ]] || fail "universal positive artifact missing" ;;
  esac
  if [[ -e "$legacy_root/.voiceflow-notary-workdir" ]]; then
    IFS= read -r legacy_workdir < "$legacy_root/.voiceflow-notary-workdir" || fail "$legacy_fixture workdir provenance unreadable"
    [[ ! -e "$legacy_workdir" ]] || fail "$legacy_fixture left actual temporary workdir"
  fi
  [[ ! -e "$legacy_root/.voiceflow-notary-state" && ! -e "$legacy_root/.voiceflow-notary-creator-pid" ]] || fail "$legacy_fixture left keychain/PID records"
  for legacy_marker in settings unlocked imported partitioned published; do [[ ! -e "$legacy_root/$legacy_marker" ]] || fail "$legacy_fixture left $legacy_marker stage"; done
  for legacy_log in 'security:create-keychain [redacted]' 'security:import [redacted]' 'security:delete-keychain'; do notary_log_count "$legacy_root" "$legacy_log" 0; done
 done

echo "release-macos tests passed."
