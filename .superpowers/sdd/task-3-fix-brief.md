# Task 3 fix brief — make in-progress words actually visible

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Do not commit. Do not start Slice 4. Do not touch spool umask or SelectedPreviewDialog.

## Problem

`dictation://partial` works, but `.voice-pill-caption` is `max-width: 164px` and the island window is 172×60 (`voicePillWindowWidth` / `notch.rs` `PILL_WIDTH`). With `WeChat · 口语 · ` already in the caption, almost none of the transcript is visible (CSS ellipsis). Slice 3’s user-visible goal is seeing in-progress words.

## Fix

When `partialText` is present (recording/starting/processing):

- Caption `max-width` at least **360px** (enough for ~18–24 CJK chars after the context label).
- Widen the island window to match: update `voicePillWindowWidth` **or** add `voicePillWindowWidthWithPartial` (~400) and use it from island placement (`src-tauri/src/notch.rs` `PILL_WIDTH`, `island_window.rs` / any `set_size` that hardcodes 172). Prefer one shared constant path; if Rust and TS must duplicate, keep both numbers identical and tested.
- Idle / no partial: keep the compact 172×60 pill so the idle HUD does not stay huge.
- Do not dump 280 characters on screen; payload cap stays 280. Visible line may ellipsis after the wider width.

If native window size is only set at creation, emit a resize when partials appear/clear (frontend invoke or backend on `emit_hud_partial`). Do not leave a 400px transparent hit target when idle.

## Tests

- Token/CSS or Island test: caption with partial is allowed to be wider than 164.
- Notch/placement test: partial width constant used when expected.
- Existing VoicePill caption tests still pass.

```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- notch::
npx vitest run src/components/Island/voicePillTokens.test.ts src/components/Island/VoicePill.test.tsx src/components/Island/IslandWindow.test.ts
```

Write `.superpowers/sdd/task-3-fix-report.md`. Return status, tests, concerns only.
