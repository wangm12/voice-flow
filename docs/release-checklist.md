# VoiceFlow release checklist

This is a preflight checklist. GitHub Actions (`.github/workflows/release.yml`) builds `VoiceFlow.dmg` and attaches it to the GitHub Release. A build is not shippable until every unchecked item has an owner and evidence.

## Automated gates

- [ ] `npm ci`
- [ ] `npm test`
- [ ] `npm run lint`
- [ ] `npm run build`
- [ ] `npm audit --audit-level=high`
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings`
- [ ] `cargo check --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin`

## macOS signing

Default GitHub Releases use a stable self-signed `VoiceFlow` identity. That keeps Accessibility across updates on machines that install those DMGs. It is not notarized; first launch still needs Open Anyway or `xattr`.

- [ ] Run `make signing-cert` once on the maintainer Mac.
- [ ] Add `MACOS_CERT_P12` and `MACOS_CERT_PASSWORD` from `.build/signing/`. Never commit the `.p12`.
- [ ] `make dmg` produces `.build/release/VoiceFlow.dmg` signed with that identity.
- [ ] Hardened runtime and `src-tauri/Entitlements.plist` are reviewed for the exact shipped binary.
- [ ] Test first launch on a clean macOS user account and confirm microphone, accessibility, and browser permission prompts.

## Developer ID notarization (optional)

- [ ] Developer ID Application certificate is available in the signing keychain.
- [ ] `APPLE_SIGNING_IDENTITY` and `APPLE_TEAM_ID` are supplied through the environment, never committed.
- [ ] Build with `make notarize` (`scripts/release-macos.sh`).
- [ ] Verify the signed app with `codesign --verify --deep --strict --verbose=2`.
- [ ] Verify notarization with `xcrun stapler validate` and `spctl --assess --type execute`.

## Privacy and behavior acceptance

- [ ] Cursor/native editor uses the developer policy and preserves technical tokens.
- [ ] Gmail/mail host uses the professional email policy.
- [ ] Browser access off never queries or stores a host.
- [ ] ASR and cleanup may use different providers; keys stay in Keychain and are not written to settings JSON.
- [ ] Browser access on stores only a normalized host; no URL path, query, title, PID, or document body is sent to the LLM.
- [ ] Switching apps or browser tabs during processing results in clipboard fallback, never an uncertain paste.
- [ ] A retry from history is clipboard-only because the original target guard is no longer trustworthy.
- [ ] Recording limit and spool quota behavior are visible and recoverable.
- [ ] Reduced-motion, fullscreen, multi-monitor, and Dock placement are manually checked.

## Update and rollback decision

- [ ] If an updater is enabled, its signed public key, endpoint, channel policy, and rollback procedure are configured in `tauri.conf.json` and tested.
- [ ] If those values are not configured, distribute versioned DMG/ZIP artifacts manually and do not advertise in-app updates.
- [ ] Database/settings migration has a backup and downgrade plan before changing schema.

## Required CI secrets (names only)

For self-signed GitHub Releases:

- `MACOS_CERT_P12`
- `MACOS_CERT_PASSWORD`

For optional Developer ID notarization via `make notarize`:

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_TEAM_ID`
- `APPLE_ID`
- `APPLE_PASSWORD`

No secret values belong in this repository or in app logs.
