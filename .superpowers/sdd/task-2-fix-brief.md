# Task 2 fix brief — preserve hybrid + PTT stop while Starting

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Do not commit. Do not start Slice 3. Do not “fix” spool umask or SelectedPreviewDialog.

## 1. Recapture must not wipe hybrid

`HotkeyRecorder` / `useHotkeyCapture` reports combo captures as `activationMode: "tap"`. `RecordingSettings` then `save({ hotkey, activation_mode: mode })`, which overwrites a user-selected `hybrid`.

Combo capture cannot tell tap vs hybrid (same key). If the current mode is `hybrid` and the newly captured hotkey is **not** modifier-only, keep `hybrid`. If the new hotkey **is** modifier-only, force `double_tap` as today.

Add a frontend test (HotkeyRecorder or RecordingSettings, whichever owns the merge). If RecordingSettings is the merge point, test that `onChange("Command+Shift+Space", "tap")` while settings.activation_mode is hybrid still saves hybrid.

## 2. Hybrid hold-release during Starting must still stop

`claim_stop` only works in `Phase::Recording`. If PTT release happens while still `Starting`, recording continues after start completes.

Add `hybrid_stop_when_recording: bool` (or equivalent) on `DictationManager`. When `handle_hotkey_release` wants Stop but phase is Starting, set the flag instead of no-op. When start transitions Starting → Recording, if the flag is set, immediately take the stop path (same as a successful PTT release).

Unit-test the flag: Starting + Stop request → flag set; entering Recording with flag → stop claim.

## Tests

```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- dictation:: hotkey::
npm test
```

Write `.superpowers/sdd/task-2-fix-report.md`. Return status, tests, concerns only.
