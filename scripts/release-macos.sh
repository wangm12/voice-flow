#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS is required for universal signing and notarization." >&2
  exit 1
fi

signing_identity="$(printenv APPLE_SIGNING_IDENTITY || true)"
team_id="$(printenv APPLE_TEAM_ID || true)"
certificate="$(printenv APPLE_CERTIFICATE || true)"
certificate_password="$(printenv APPLE_CERTIFICATE_PASSWORD || true)"
if [[ -z "$signing_identity" || -z "$team_id" ]]; then
  echo "APPLE_SIGNING_IDENTITY and APPLE_TEAM_ID are required." >&2
  exit 1
fi

if [[ -n "$certificate" ]]; then
  keychain_path="$(mktemp -u "$TMPDIR/voiceflow-signing-XXXXXX.keychain-db")"
  certificate_path="$(mktemp "$TMPDIR/voiceflow-signing-XXXXXX.p12")"
  printf '%s' "$certificate" | base64 --decode > "$certificate_path"
  security create-keychain -p "$certificate_password" "$keychain_path"
  security set-keychain-settings -lut 21600 "$keychain_path"
  security unlock-keychain -p "$certificate_password" "$keychain_path"
  security import "$certificate_path" -k "$keychain_path" -P "$certificate_password" -T /usr/bin/codesign -T /usr/bin/security
  security list-keychains -d user -s "$keychain_path" login.keychain-db
  security default-keychain -s "$keychain_path"
  security set-key-partition-list -S apple-tool:,apple: -s -k "$certificate_password" "$keychain_path"
fi

rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm ci
npm run build
APPLE_SIGNING_IDENTITY="$signing_identity" \
APPLE_TEAM_ID="$team_id" \
npm run tauri -- build --target universal-apple-darwin --bundles app,dmg

bundle="$(find src-tauri/target/universal-apple-darwin/release/bundle/macos -maxdepth 1 -name '*.app' -print -quit)"
if [[ -z "$bundle" ]]; then
  echo "Universal app bundle was not produced." >&2
  exit 1
fi

codesign --verify --deep --strict --verbose=2 "$bundle"
notarize="$(printenv VOICEFLOW_NOTARIZE || true)"
if [[ "$notarize" == "1" ]]; then
  apple_id="$(printenv APPLE_ID || true)"
  apple_password="$(printenv APPLE_PASSWORD || true)"
  if [[ -z "$apple_id" || -z "$apple_password" ]]; then
    echo "APPLE_ID and APPLE_PASSWORD are required for notarization." >&2
    exit 1
  fi
  xcrun notarytool submit "$bundle" --apple-id "$apple_id" --password "$apple_password" --team-id "$team_id" --wait
  xcrun stapler staple "$bundle"
  xcrun stapler validate "$bundle"
  spctl --assess --type execute --verbose=4 "$bundle"
else
  echo "Notarization was not requested; Gatekeeper verification is intentionally not claimed."
fi

echo "Verified macOS bundle: $bundle"
