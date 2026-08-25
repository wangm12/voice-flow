# Follow-on fix: F1 + F2 from final-fix re-review

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Read: `.superpowers/sdd/final-fix-rereview.md` (F1, F2; optional cheap F4).

Do **not** commit. Do not touch spool umask or SelectedPreviewDialog `取消`. Do not reopen C1 (still never Cmd+Z after AX).

Write report to `.superpowers/sdd/final-fix-followup-report.md`.

## F1 — HUD Undo must match a real undo transaction

`emit_state_with_delivery_and_input_device` (`lib.rs` ~460) sets `undo_available` from `delivery_method == "paste"` alone. Verified AX insert reports method `"paste"` / state `"done"`, so VoicePill shows 撤销插入, but `arm_undo_transaction` did not arm (no `used_keyboard_paste`), so the button is a silent no-op.

**Fix:** `undo_available` is true only when a 3s undo was actually armed: `used_keyboard_paste && verified && delivery_method == "paste"` (and state is done/degraded as today). Preferred: consult `state.undo` (transaction present, unconsumed, unexpired) so the payload cannot drift from `arm_undo_transaction`. Thread `AppState` into the emit if needed.

Frontend: if `undo_last_delivery` returns a non-success string, do not pretend success (log or ignore is fine; do not show the button when `undo_available` is false).

Test: VoicePill already hides the button when `undoAvailable` is false — keep that. Add a Rust assertion/helper if you can unit-test the emit predicate without a live app.

## F2 — Unverified AX insert keeps clipboard fallback

AX success currently returns before any clipboard write. If `verified == false` (osascript timeout / unread AXValue), HUD says check the field but there is nothing to paste.

**Fix:** On the AX early-return in `paste.rs` `insert()`, after `build_insert_outcome(false, …)`, if `!outcome.verified`, write `text` to the clipboard (previous clipboard was never overwritten — this is the intentional fail-closed fallback, same as posted Cmd+V). If verified, still do **not** touch the clipboard.

Test: helper or outcome path: AX + verified → no clipboard side effect required in unit test; AX + unverified → `should_copy_clipboard_fallback` true. Do not arm undo (`used_keyboard_paste` stays false).

## Optional F4

EngineSettings remove-ASR ConfirmDialog copy should mention `api.groq.com` like the settings hint, plus i18n.

Skip F3/F5/F6.

## Tests

```
cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests
npx vitest run src/components/Island/VoicePill.test.tsx
```

cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo` if needed. Then `--lib`, `npm test`, `npm run lint`. Known unrelated failures: spool 0o755, SelectedPreviewDialog 取消.
