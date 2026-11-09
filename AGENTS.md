# VoiceFlow agent notes

macOS-first system dictation: React/Vite UI in `src/`, Rust/Tauri backend in `src-tauri/src/`. Keys stay in Keychain. Default dictation does not screenshot or send window images to an LLM. Look-at-screen is a separate hotkey and needs a vision model plus Screen Recording. The app supports macOS 13+; bundled MLX routes require Apple Silicon and macOS 14+.

## Entrypoints

- **Desktop app:** `make run` starts `npm run tauri dev`; `npm run dev` runs only the root Vite UI. `npm test`, `npm run lint`, and `npm run build` cover the root frontend. Tauri commands and Rust behavior live in `src-tauri/src/`; `src-tauri/build.rs` generates the command permission manifest.
- **Website:** `website/` is a separate React/Vite app with its own `package.json` and lockfile. Use `npm ci --prefix website`; its scripts are `npm run dev --prefix website`, `npm run lint --prefix website`, `npm run build --prefix website`, and `npm run preview --prefix website`. Shared product UI can be imported by the website, but its build does not launch Tauri.
- **Audio evaluation:** `npm run eval:audio -- ...` enters `scripts/evaluate-audio.mjs`, then the feature-gated `voiceflow-eval` Rust binary (`evaluation` feature). The default path validates external fixtures; only an explicit run can call selected providers. See [`docs/cleanup-evaluation.md`](docs/cleanup-evaluation.md).
- **Optional research:** `scripts/research/prepare-public-corpus.py` prepares explicitly selected public corpus material; `scripts/research/speaker-filter-pilot.py` and `scripts/research/speaker-filter-candidates/` are research pilots. `scripts/research/build-learning-owned-paste-session.sh` and `scripts/research/build-learning-owned-target-oracle.sh` build separate source-pinned native helpers from the neighboring Swift sources; these do not depend on the Rust research feature. The `voiceflow-learning-harness` Rust binary is a separate stdin/stdout entrypoint gated by the `research-harness` Cargo feature. Its invocation and limits are documented in [`docs/end-to-end-workflows.md`](docs/end-to-end-workflows.md#standalone-deterministic-text-learning-fixture); the broader status is indexed in [`docs/README.md`](docs/README.md). Do not treat these as normal product behavior.
- **Third-party attribution tooling:** `scripts/research/third-party-notices/tools/` generates/reviews dependency text inventories; it is linked from `docs/README.md` and its own README. Release packaging also calls `scripts/verify-dependency-texts.py`.
- **Release:** `Makefile`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and the referenced `scripts/*.sh` are active build/release entrypoints. CI selects Xcode 26 and requires Swift 6.2 on its macOS runner.

## Commands

```bash
npm test
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

Quote test counts from a run you just did. Do not copy numbers from snapshots or old reviews.

## Docs

Classification lives in [`docs/README.md`](docs/README.md).

- **Living docs** must match current code. If you change behavior, update those pages in the same change.
- **Research** may still have useful conclusions; treat “today / current status” as stale.
- **Ongoing plans** contain active goals, acceptance gates, and implementation contracts. Keep them until their outstanding work is delivered or explicitly stopped; do not mistake a partially shipped slice for a completed overall plan.
- **Do not keep shipped task briefs, specs, plans, or SDD reports** in the repo. After the work lands, delete those files and drop their links. Git history is enough.

## Cleanup (do this when you touch the repo)

This cleanup rule is triggered by this file. Do not add a dedicated hygiene workflow or automatically open a PR; the existing CI and Release workflows remain active.

1. Diff living docs against the code you are changing. Fix contradictions. Delete leftover task/spec/plan files from a finished change; do not rewrite research papers unless a link would break.
2. Delete only **proven-dead** code. Check callers across `src/`, `src-tauri/`, scripts, package/Make targets, CI and release workflows, Rust feature-gated and test-only paths, Tauri commands/IPC and generated permission manifests, build/resource packaging, and documented commands or data contracts. A missing call from the main UI is not proof of dead code when a script, CLI, optional feature, test, build, or documented external entrypoint still uses it. Reconcile any stale public documentation before deleting an entrypoint. Prefer deleting an unused wrapper over merging look-alikes.
3. Leave intentional cross-language duplicates (for example TS and Rust pill widths) unless both sides can share one generated source.
4. Do not do drive-by refactors, accuracy claims, or “while I am here” features.

Privacy and “later / won’t” boundaries: [`docs/privacy.md`](docs/privacy.md), [`docs/asr-cleanup-later-and-wont.md`](docs/asr-cleanup-later-and-wont.md).
