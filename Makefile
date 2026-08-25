.PHONY: run release dmg signing-cert notarize build-signed

BUNDLES ?= app,dmg
RUST_BIN := $(firstword $(wildcard $(HOME)/.cargo/bin) $(wildcard $(HOME)/.rustup/toolchains/*/bin))
export PATH := $(if $(RUST_BIN),$(RUST_BIN):)$(PATH)
RELEASE_DMG := $(CURDIR)/.build/release/VoiceFlow.dmg

run:
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs" >&2; exit 1; }
	npm run tauri dev

signing-cert:
	./scripts/make-signing-cert.sh

release:
	@$(MAKE) build-signed BUNDLES="$(BUNDLES)"

dmg: BUNDLES := dmg
dmg: release

notarize:
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs" >&2; exit 1; }
	@test -n "$${APPLE_SIGNING_IDENTITY:-}" || { echo "error: APPLE_SIGNING_IDENTITY is required for notarization" >&2; exit 2; }
	@test -n "$${APPLE_TEAM_ID:-}" || { echo "error: APPLE_TEAM_ID is required for notarization" >&2; exit 2; }
	bash scripts/release-macos.sh

build-signed:
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs" >&2; exit 1; }
	@set -euo pipefail; \
	eval "$$(./scripts/resolve-dmg-signing.sh)"; \
	./scripts/detach-voiceflow-dmg.sh; \
	if [ -n "$${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "$${APPLE_TEAM_ID:-}" ]; then \
		echo "==> Signing VoiceFlow with: $$APPLE_SIGNING_IDENTITY"; \
		APPLE_SIGNING_IDENTITY="$$APPLE_SIGNING_IDENTITY" \
		APPLE_TEAM_ID="$$APPLE_TEAM_ID" \
		npm run tauri build -- --bundles $(BUNDLES); \
	else \
		echo "warning: building unsigned VoiceFlow bundles" >&2; \
		npm run tauri build -- --bundles $(BUNDLES); \
	fi; \
	./scripts/stage-dmg.sh "$(RELEASE_DMG)"
