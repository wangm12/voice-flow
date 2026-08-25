# Task 2 fix report — preserve hybrid + PTT stop while Starting

**Status:** DONE

## What shipped

1. Recapturing a combo no longer overwrites `hybrid`. Capture still reports `tap` (combo keys cannot tell tap vs hybrid). `resolveCapturedActivationMode` keeps `hybrid` when the current mode is hybrid and the new hotkey is not modifier-only. Modifier-only recapture still forces `double_tap`. Wired in `RecordingSettings` (merge point) and onboarding. Backend `set_hotkeys_suspended` uses the same keep-hybrid rule so the unsuspend handshake does not persist tap before the UI re-save.

2. Hybrid PTT release during `Phase::Starting` sets `hybrid_stop_when_recording`. `enter_recording` (Starting → Recording) claims stop immediately if the flag is set, then `start_claimed` takes the same `stop_claimed` path as a successful PTT release. Flag is cleared on start claim, cancel, reset, and stop.

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo` is not on default PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- dictation:: hotkey:: captured_tap_does_not_wipe_hybrid_combo
npm test -- src/components/settings/RecordingSettings.test.tsx src/lib/activationCopy.test.ts src/components/Onboarding/Onboarding.test.tsx src/components/HotkeyRecorder.test.tsx src/components/ActivationModeSelector.test.tsx
npm test
```

| Command | Result |
|---|---|
| cargo `dictation::` `hotkey::` `captured_tap_does_not_wipe_hybrid_combo` | **21 passed**, 0 failed |
| targeted frontend (RecordingSettings, activationCopy, Onboarding, HotkeyRecorder, ActivationModeSelector) | **17 passed**, 0 failed |
| `npm test` (full) | **129 passed, 1 failed** |

New tests (failed on the wipe / missing-flag behavior first, then passed):

- `RecordingSettings` `keeps hybrid when recapture reports tap for a combo`
- `RecordingSettings` `forces double_tap when recapture is modifier-only`
- `activationCopy` `keeps hybrid when combo recapture reports tap`
- `dictation::tests::hybrid_stop_during_starting_sets_pending_flag`
- `dictation::tests::entering_recording_with_pending_hybrid_stop_claims_stop`
- `dictation::tests::hybrid_stop_while_recording_claims_immediately`
- `tests::captured_tap_does_not_wipe_hybrid_combo`

Did not run full `cargo test --lib`. Did not touch spool umask or SelectedPreviewDialog.

## Files changed

- `src/lib/activationCopy.ts`
- `src/lib/activationCopy.test.ts`
- `src/components/settings/RecordingSettings.tsx`
- `src/components/settings/RecordingSettings.test.tsx` (new)
- `src/components/Onboarding/Onboarding.tsx`
- `src-tauri/src/dictation.rs`
- `src-tauri/src/lib.rs`
- `.superpowers/sdd/task-2-fix-report.md`

Not committed.

## Concerns

1. `npm test` still has the unrelated `SelectedPreviewDialog` failure (`getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button). Not touched.
2. Recapturing the selected-action hotkey still sends `capturedActivationMode` into `set_hotkeys_suspended`, which can rewrite dictation `activation_mode`. Keep-hybrid avoids wiping hybrid; a modifier-only selected-action recapture can still clamp dictation to `double_tap` (pre-existing handshake).
3. Pending-stop during start skips the recording HUD/prefetch setup and goes straight to `stop_claimed` (processing). Same stop path as PTT after Recording, without a recording-state flash.
