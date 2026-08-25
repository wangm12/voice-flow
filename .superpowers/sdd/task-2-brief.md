# Task 2 brief — Hybrid hotkey

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Same as Task 1: no meetings/keylog; fail-closed paste; do not commit; do not implement other slices; i18n coverage; run cargo test --lib, npm test, npm run lint.

## Task

Combo keys: short press = toggle, hold = PTT. Modifier-only stays double_tap.

### Files

- `src-tauri/src/hotkey.rs` — combo `on_shortcut` currently only Pressed → `hotkey://toggle`
- `src-tauri/src/dictation.rs` — `handle_hotkey_toggle`; add press/release handlers if needed
- `src-tauri/src/store.rs`, `src-tauri/src/lib.rs` settings patch that rejects `hold` / non tap|double_tap
- `src/lib/activationCopy.ts`, `ActivationModeSelector.tsx` + test, `RecordingSettings.tsx`, onboarding TryItStep / tryItHint
- `src/types/settings.ts` if needed

### Spec

- `activation_mode`: `"tap" | "double_tap" | "hybrid"`
- Modifier-only still forced to `double_tap`. Never hybrid on fn/⌘-only.
- Legacy `hold` for non-modifier → `hybrid` (not tap)
- `HYBRID_HOLD_MS: u64 = 280`

Hybrid combo:
- Pressed + idle → start, remember press time
- Pressed + already recording from prior short tap → stop
- Released + this press started recording + elapsed >= 280ms → stop (PTT)
- Released + elapsed < 280ms → keep recording
- `tap` mode unchanged (toggle on press)

If `ShortcutState::Released` is missing from the plugin, report BLOCKED with evidence.

Pure function tests:
```rust
pub fn hybrid_release_action(elapsed_ms: u64, started_this_press: bool) -> HybridReleaseAction
```
400ms + started → Stop; 80ms + started → KeepRecording.

Ignore synthetic keys during paste.

Out of scope: modifier-only PTT, HUD transcripts.

Write report to `.superpowers/sdd/task-2-report.md`. Return status, tests, concerns only.
