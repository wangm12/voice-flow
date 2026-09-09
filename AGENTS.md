# VoiceFlow agent notes

macOS-first system dictation: React/Vite UI in `src/`, Rust/Tauri backend in `src-tauri/src/`. Keys stay in Keychain. Default dictation does not screenshot or send window images to an LLM. Look-at-screen is a separate hotkey and needs a vision model plus Screen Recording.

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
- **Do not keep shipped task briefs, specs, plans, or SDD reports** in the repo. After the work lands, delete those files and drop their links. Git history is enough.

## Cleanup (do this when you touch the repo)

No GitHub workflow and no auto-opened PR. The trigger is this file.

1. Diff living docs against the code you are changing. Fix contradictions. Delete leftover task/spec/plan files from a finished change; do not rewrite research papers unless a link would break.
2. Delete only **proven-dead** code: no callers in `src/` or `src-tauri/`, and not a required Tauri command. Prefer deleting a unused wrapper over merging look-alikes.
3. Leave intentional cross-language duplicates (for example TS and Rust pill widths) unless both sides can share one generated source.
4. Do not do drive-by refactors, accuracy claims, or “while I am here” features.

Privacy and “later / won’t” boundaries: [`docs/privacy.md`](docs/privacy.md), [`docs/asr-cleanup-later-and-wont.md`](docs/asr-cleanup-later-and-wont.md).
