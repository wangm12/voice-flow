# Task 3 brief — HUD in-progress words

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Do not commit. Do not paste partials into apps/clipboard/History. No window titles in events. Do not implement other slices. Run cargo test --lib, npm test, npm run lint.

## Task

Show prefetch chunk transcripts on the HUD only.

### Files

- `src-tauri/src/prefetch_asr.rs`
- `src-tauri/src/lib.rs` emit helper if needed
- `src/components/Island/IslandWindow.tsx`
- `src/components/Island/VoicePill.tsx` + tests

### Spec

- Event: `dictation://partial`
- Payload: `{ session_generation: u64, text: String }`
- Concatenate completed non-warmup chunk transcripts in index order, space-separated, trim, cap 280 chars with ellipsis
- NEVER write partials to clipboard, History, or paste
- Cancel / idle / generation bump clears HUD partial
- Prefetch failure stays silent; final batch ASR is source of truth

Depends on Task 1 already showing `contextLabel`. Partial text can share/replace caption during recording; keep error captions first.

Out of scope: streaming ASR, FunASR.

Write report to `.superpowers/sdd/task-3-report.md`.
