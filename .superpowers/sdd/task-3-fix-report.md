# Task 3 fix report — make in-progress words actually visible

**Status:** DONE_WITH_CONCERNS

## What shipped

While `partialText` is present during recording/starting/processing (and recording-limited), the caption chip uses `max-width: 360px` (`voice-pill-caption--partial` plus the shared token). The native island surface resizes to 400×60 via `PILL_WIDTH_WITH_PARTIAL` / `voicePillWindowWidthWithPartial`, recentered on the active display.

Idle / empty / stale / non-live phases keep the compact 172×60 pill. `emit_hud_partial` drives `island_window::set_has_partial`; `show_overlay` placement reads the same flag so a later reconcile does not snap back to 172 while words are showing. Payload cap stays 280 with ellipsis.

## Product checks

| Requirement | Result |
|---|---|
| Caption max-width ≥ 360px when partial is present | Pass (`voicePillCaptionMaxWidthForPartial(true) === 360`, VoicePill `--partial` + inline maxWidth) |
| Island window ~400 while partial is present | Pass (`pill_window_width(true) === 400`, placement 400×60) |
| Idle / no partial stays 172×60 | Pass (`pill_window_width(false) === 172`; empty/idle/stale do not expand) |
| Do not dump 280 characters; ellipsis after wider width | Pass (CSS ellipsis kept; payload cap unchanged) |
| Resize when partials appear/clear; no 400px idle hit target | Pass (`set_has_partial` on `emit_hud_partial`; expand only for live Starting/Recording/Stopping/Processing + matching generation) |
| Existing VoicePill caption tests still pass | Pass |
| No commit, no Slice 4, no spool umask / SelectedPreviewDialog | Pass |

## TDD evidence

**RED:**

```
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx
TypeError: voicePillCaptionMaxWidthForPartial is not a function
TypeError: voicePillWindowWidthForPartial is not a function
Error: expect(element).toHaveClass("voice-pill-caption--partial")
Received: voice-pill-caption voice-pill-caption--warning

cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch::
error[E0425]: cannot find value `PILL_WIDTH_WITH_PARTIAL`
error[E0425]: cannot find function `pill_window_width`
error[E0425]: cannot find function `placement_for_monitor_at_scale_with_width`
```

**GREEN:**

```
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts src/components/Island/IslandWindow.events.test.tsx
51 passed

cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch:: hud_partial_expands_window_only_during_live_dictation
6 passed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo` is not on default PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch::
cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch:: hud_partial_expands_window_only_during_live_dictation
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts src/components/Island/IslandWindow.events.test.tsx
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch::` | **5 passed** (then 6 with expand-window helper) |
| `npx vitest run` tokens + VoicePill + IslandWindow + events | **51 passed** |

Did not run full `cargo test --lib` / `npm test`. Did not touch spool umask or SelectedPreviewDialog.

New tests (failed on missing width APIs / missing `--partial` class first, then passed):

- `allows a wider caption than 164px when in-progress words are present`
- `uses a wider island window while in-progress words are present`
- `keeps the compact caption width when recording without in-progress words`
- `notch::tests::uses_partial_width_when_expected`
- `tests::hud_partial_expands_window_only_during_live_dictation`

## Files changed

- `src/components/Island/voicePillTokens.ts`
- `src/components/Island/voicePillTokens.test.ts`
- `src/components/Island/VoicePill.tsx`
- `src/components/Island/VoicePill.test.tsx`
- `src/components/Island/IslandWindow.events.test.tsx`
- `src/island.css`
- `src-tauri/src/notch.rs`
- `src-tauri/src/island_window.rs`
- `src-tauri/src/lib.rs`
- `.superpowers/sdd/task-3-fix-report.md`

Not committed.

## Concerns

1. TS `voicePillWindowWidthWithPartial = 400` and Rust `PILL_WIDTH_WITH_PARTIAL = 400.0` are duplicated. Both are asserted as 400; there is no codegen shared constant.
2. Native `set_size` hops to the main thread. The first compositor frame after a partial arrives can still be 172px wide before the resize lands.
3. `recording_limited` with leftover `partialText` still widens the chip even if the status caption won and hid the words.
4. Full `cargo test --lib` and `npm test` were not re-run. The known unrelated failures (spool umask `0o755` vs `0o700`, SelectedPreviewDialog duplicate 取消) were left untouched.
