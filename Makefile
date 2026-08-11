.PHONY: dev release dmg

BUNDLES ?= app,dmg

dev:
	npm run tauri dev

release:
	@set -e; \
	identity="$${APPLE_SIGNING_IDENTITY:-$$(security find-identity -v -p codesigning 2>/dev/null | awk -F'"' '/^[[:space:]]*[0-9]+\)/ { if ($$2 ~ /^Developer ID Application:/) { print $$2; found=1; exit } if (!first) first=$$2 } END { if (!found && first) print first }')}"; \
	if [ -z "$$identity" ]; then \
		echo "No macOS signing identity found. Set APPLE_SIGNING_IDENTITY or install an Apple Development/Developer ID certificate." >&2; \
		exit 1; \
	fi; \
	case "$$identity" in \
		Developer\ ID\ Application:*) ;; \
		*) echo "Warning: $$identity is suitable for local installation but not public distribution/notarization." >&2 ;; \
	esac; \
	echo "Signing VoiceFlow with: $$identity"; \
	APPLE_SIGNING_IDENTITY="$$identity" npm run tauri build -- --bundles $(BUNDLES)

dmg: BUNDLES := dmg
dmg: release
