# Task 3 report — HUD in-progress words

**Status:** DONE_WITH_CONCERNS

## What shipped

Completed non-warmup prefetch chunks now emit HUD-only `dictation://partial` with `{ session_generation, text }`. The text is concatenated completed chunk transcripts in index order, space-separated, trimmed, and capped at 280 characters with `...`.

IslandWindow listens, respects session generation, and passes `partialText` to VoicePill. During recording/starting/processing the caption can share the context label with those words (`WeChat · 口语 · 你好世界`). Error, degraded, fallback, rate-limit, recording-limit, and selected-action captions still win. Cancel / idle / generation bump clears the HUD partial. Prefetch failure and warmup stay silent. Partials are never written to clipboard, History, or paste.

## Product checks

| Requirement | Result |
|---|---|
| Event `dictation://partial` with `{ session_generation, text }` | Pass |
| Concatenate completed non-warmup chunks in index order, space-separated, trim | Pass (`hud_partial_text`) |
| Cap 280 chars with ellipsis | Pass (277 chars + `...`) |
| After successful non-warmup prefetch chunk, emit concatenated HUD text | Pass |
| Warmup never emitted | Pass |
| Prefetch failure stays silent | Pass |
| NEVER clipboard / History `raw_text`/`final_text` / paste | Pass (`emit_hud_partial` only; idle clear is empty HUD event) |
| IslandWindow listens and passes text to VoicePill | Pass |
| Partial may share/replace caption during recording/processing | Pass (shares with `contextLabel`) |
| Error / degraded captions still win | Pass |
| Cancel / idle / generation bump clears partial | Pass (idle state + empty HUD event; stale gen rejected) |
| Slice 1 context labels and Slice 2 hybrid hotkeys not reverted | Pass |
| No window titles in the partial payload | Pass |
| No commit, no later slices | Pass |

## Self-review

- Partials never go to clipboard, History, or the target app: prefetch stores transcripts in the existing HashMap and calls an injected HUD callback; `emit_hud_partial` only emits `dictation://partial`. Final ASR / paste / History paths are unchanged.
- Session generation is on the payload. IslandWindow rejects stale events via `acceptsSessionGeneration`. A newer generation or `idle` clears `partialText`.
- Idle `emit_state` also emits an empty `dictation://partial` so cancel/idle clears even if a late chunk callback races.

## TDD evidence

**RED (helper missing):**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib prefetch_asr::
error[E0425]: cannot find function `hud_partial_text` in this scope
```

Frontend RED (feature missing, not typos):

```
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts
Tests  5 failed | 41 passed (46)
- hudPartialFromEvent is not a function
- expected 'WeChat · 口语' to be 'WeChat · 口语 · 你好世界'
```

Spawn RED (emit not wired):

```
error[E0061]: this function takes 7 arguments but 9 arguments were supplied
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib prefetch_asr::
7 passed; 0 failed
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts src/components/Island/IslandWindow.events.test.tsx
48 passed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib prefetch_asr::
cargo test --manifest-path src-tauri/Cargo.toml --lib
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts src/components/Island/IslandWindow.events.test.tsx
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib prefetch_asr::` | **7 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **213 passed, 1 failed, 1 ignored** |
| Task 3 frontend (tokens, VoicePill, IslandWindow helpers + events) | **48 passed** |
| `npm test` (full) | **138 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–2)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not HUD partials.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/prefetch_asr.rs`
- `src-tauri/src/lib.rs`
- `src/components/Island/IslandWindow.tsx`
- `src/components/Island/IslandWindow.test.ts`
- `src/components/Island/IslandWindow.events.test.tsx` (new)
- `src/components/Island/VoicePill.tsx`
- `src/components/Island/VoicePill.test.tsx`
- `src/components/Island/voicePillTokens.ts`
- `src/components/Island/voicePillTokens.test.ts`

Not committed.

## Concerns

1. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
2. The visible caption chip is still `max-width: 164px` with CSS ellipsis, so most of a 280-character partial is only in `aria-label`. Spec asked for a 280-char payload cap, not a wider HUD.
3. `emit_state("idle")` sends an empty `dictation://partial` to clear the HUD. Other idle-like paths that only use `emit_state_with_delivery` do not send that empty event; the frontend still clears when it receives `state: "idle"`.
