# Task 2 report — Hybrid hotkey

**Status:** DONE_WITH_CONCERNS

## What shipped

Combo-key activation now supports `hybrid`: short press toggles, hold ≥ 280ms is push-to-talk (stop on release). Tap mode still toggles on `ShortcutState::Pressed` only. Modifier-only hotkeys stay `double_tap` (event tap); they cannot select hybrid. Legacy `hold` on a non-modifier combo migrates to `hybrid`, not `tap`.

`ShortcutState::Released` is available. Evidence:

- `tauri-plugin-global-shortcut` 2.3.2 re-exports `global_hotkey::hotkey::HotKeyState` as `ShortcutState`
- `global-hotkey` 0.8.0: `HotKeyState { Pressed, Released }`
- macOS backend handles `kEventHotKeyReleased` (`platform_impl/macos/mod.rs`)

No while-key-down timer PTT. Release events drive hold-to-talk.

## Product checks

| Requirement | Result |
|---|---|
| `activation_mode`: `tap` \| `double_tap` \| `hybrid` | Pass |
| Combo hybrid: Pressed + idle → start, remember press time | Pass (`handle_hotkey_press` + `mark_hybrid_press`) |
| Pressed + already recording from prior short tap → stop | Pass (same toggle action as tap, `started_this_press = false`) |
| Released + this press started recording + elapsed ≥ 280ms → stop | Pass (`hybrid_release_action` / `take_hybrid_release` / `handle_hotkey_release`) |
| Released + elapsed < 280ms → keep recording | Pass |
| Tap mode unchanged: toggle on press only | Pass (`combo_hotkey_event("tap", Pressed) → hotkey://toggle`; Released ignored) |
| Modifier-only forced to `double_tap`; never hybrid on fn/⌘-only | Pass (normalize, clamp, validate, UI radios disabled) |
| Legacy `hold` for non-modifier → `hybrid` | Pass |
| `HYBRID_HOLD_MS = 280` | Pass |
| `hybrid_release_action(400, true) → Stop`; `(80, true) → KeepRecording` | Pass |
| Ignore synthetic keys during paste | Pass via existing modifier event-tap suppress; combo Carbon hotkeys are not Cmd+V |
| UI copy: activationCopy, selector, RecordingSettings, onboarding | Pass (RecordingSettings shows hybrid through shared selector + usage guide) |
| English `t()` keys covered | Pass (`i18n.coverage.test.ts`) |
| Slice 1 (per-app tone) not reverted | Pass |
| No commit, no later slices | Pass |

## Self-review

- Tap still toggles on press: combo handler emits `hotkey://toggle` only for non-hybrid `Pressed`.
- Hybrid hold-release stops: release path calls `claim_stop_entry` (no 400ms gesture lock), so PTT can stop at 280ms.
- Short tap keeps recording: elapsed < 280ms returns `KeepRecording` and does not stop.
- Modifier-only cannot select hybrid: radios for tap/hybrid are disabled; backend clamp/normalize force `double_tap`.

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo` was not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib hybrid_` | **9 passed** |
| tap / hold-migration / clamp tests | **5 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **204 passed, 1 failed, 1 ignored** |
| Task 2 frontend (activationCopy, ActivationModeSelector, i18n.coverage, App, Onboarding, HotkeyRecorder) | **25 passed** |
| `npm test` (full) | **126 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Task 1)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not hybrid logic.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/hotkey.rs`
- `src-tauri/src/dictation.rs`
- `src-tauri/src/store.rs`
- `src-tauri/src/lib.rs`
- `src/lib/activationCopy.ts`
- `src/lib/activationCopy.test.ts` (new)
- `src/lib/i18n.tsx`
- `src/components/ActivationModeSelector.tsx`
- `src/components/ActivationModeSelector.test.tsx`
- `src/components/HotkeyUsageGuide.tsx`
- `src/components/Onboarding/HotkeyStep.tsx`
- `src/components/Onboarding/TryItStep.tsx`
- `src/components/Onboarding/Onboarding.tsx`

`src/components/settings/RecordingSettings.tsx` was not edited: it already hosts `ActivationModeSelector` and `HotkeyUsageGuide`, which now expose hybrid.

Not committed.

## Concerns

1. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
2. If recorder setup is still `Starting` when a hybrid hold is released (≥280ms), `claim_stop` no-ops (`phase != Recording`). Recording continues once start completes; the user must press again to stop. Typical start is faster than 280ms.
3. Recapturing a combo in `HotkeyRecorder` still commits `activation_mode: "tap"`, which can overwrite a previously selected `hybrid` until the user picks hybrid again.
4. The existing 400ms gesture lock still applies to a *second press* after a short hybrid tap, so toggle-off can be delayed the same way as tap mode. PTT release bypasses that lock.
