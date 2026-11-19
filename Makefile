.PHONY: run release dmg signing-cert notarize build-signed

BUNDLES ?= app,dmg
RUST_BIN := $(firstword $(wildcard $(HOME)/.cargo/bin) $(wildcard $(HOME)/.rustup/toolchains/*/bin))
export PATH := $(if $(RUST_BIN),$(RUST_BIN):)$(PATH)
RELEASE_DMG := $(CURDIR)/.build/release/VoiceFlow.dmg
UNSIGNED_RELEASE_DMG := $(CURDIR)/.build/release/VoiceFlow-unsigned.dmg
NOTARIZED_DMG := $(CURDIR)/.build/release/VoiceFlow-notarized.dmg

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
	@test -n "$${APPLE_ID:-}" || { echo "error: APPLE_ID is required for notarization" >&2; exit 2; }
	@test -n "$${APPLE_PASSWORD:-}" || { echo "error: APPLE_PASSWORD is required for notarization" >&2; exit 2; }
	VOICEFLOW_NOTARIZE=1 bash scripts/release-macos.sh "$(NOTARIZED_DMG)"

build-signed:
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs" >&2; exit 1; }
	@set -euo pipefail; \
	bundle_dmg_dir="$(CURDIR)/src-tauri/target/release/bundle/dmg"; \
	bundle_macos_dir="$(CURDIR)/src-tauri/target/release/bundle/macos"; \
	./scripts/detach-voiceflow-dmg.sh; \
	rm -rf "$$bundle_dmg_dir"; \
	rm -f "$$bundle_macos_dir"/rw.*.dmg; \
	signing_env="$$(./scripts/resolve-dmg-signing.sh)"; \
	eval "$$signing_env"; \
	want_dmg=0; \
	case ",$(BUNDLES)," in *,dmg,*) want_dmg=1 ;; esac; \
	if [ -n "$${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "$${APPLE_TEAM_ID:-}" ]; then \
		echo "==> Signing VoiceFlow with: $$APPLE_SIGNING_IDENTITY"; \
		APPLE_SIGNING_IDENTITY="$$APPLE_SIGNING_IDENTITY" \
		APPLE_TEAM_ID="$$APPLE_TEAM_ID" \
		npm run tauri build -- --bundles app; \
		if [ "$$want_dmg" -eq 1 ]; then \
			APPLE_SIGNING_IDENTITY="$$APPLE_SIGNING_IDENTITY" \
			./scripts/build-dmg.sh "$$bundle_macos_dir/VoiceFlow.app" "$$bundle_dmg_dir/VoiceFlow.dmg"; \
			./scripts/stage-dmg.sh "$$bundle_dmg_dir" "$(RELEASE_DMG)"; \
		fi; \
	else \
		echo "warning: building explicitly allowed unsigned VoiceFlow bundles" >&2; \
		unset APPLE_SIGNING_IDENTITY APPLE_TEAM_ID DEVELOPER_IDENTITY DEVELOPMENT_TEAM; \
		npm run tauri build -- --bundles app; \
		if [ "$$want_dmg" -eq 1 ]; then \
			./scripts/build-dmg.sh "$$bundle_macos_dir/VoiceFlow.app" "$$bundle_dmg_dir/VoiceFlow-unsigned.dmg"; \
			./scripts/stage-dmg.sh "$$bundle_dmg_dir" "$(UNSIGNED_RELEASE_DMG)"; \
		fi; \
	fi
