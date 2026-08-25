# Follow-on fix report (F1 + F2)

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`  
Source: `.superpowers/sdd/final-fix-followup-brief.md` / `.superpowers/sdd/final-fix-rereview.md`  
No commit. C1 not reopened (AX still never arms Cmd+Z). Spool umask and SelectedPreviewDialog `取消` untouched.

**Status: DONE_WITH_CONCERNS**

## Issue status

| Id | Status | Notes |
| --- | --- | --- |
| **F1** | fixed | `undo_available` now consults `state.undo`: HUD state is `done`/`degraded` **and** `undo_preflight` is `"available"` (present, unconsumed, unexpired, matching generation). Verified AX still reports method `"paste"` / `"done"` but does not arm a transaction, so the 撤销插入 button is hidden. `VoicePill` logs a warning when `undo_last_delivery` returns anything other than `"success"`. |
| **F2** | fixed | AX early-return in `insert()` still uses `used_keyboard_paste: false`. After `build_insert_outcome`, `should_copy_clipboard_fallback` is true only for `!used_keyboard_paste && !verified`; that path writes `text` to the clipboard. Verified AX still does not touch the clipboard. Undo is not armed. |
| **F4** | fixed (optional) | Remove-ASR ConfirmDialog now says `仅默认 Groq 或 api.groq.com`, matching the settings hint; English i18n updated. |
| **F3** | skipped | as instructed |
| **F5** | skipped | as instructed |
| **F6** | skipped | as instructed |

C1 still holds: AX outcomes keep `used_keyboard_paste: false`; `arm_undo_transaction` still returns unless `used_keyboard_paste && delivery_method == "paste"`. No Cmd+Z after AX.

---

## Covering tests

Commands used `PATH` with `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`.

### `cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests`

```
running 20 tests
test paste::tests::macos_shortcuts_use_layout_independent_ansi_keycodes ... ok
test paste::tests::paste_layout_is_held_until_cmd_v_can_be_consumed ... ok
test paste::tests::input_verification_accepts_single_ascii_and_cjk_characters ... ok
test paste::tests::input_verification_requires_a_new_occurrence_of_the_delivered_text ... ok
test paste::tests::selection_fingerprint_changes_when_selected_text_changes ... ok
test paste::tests::ax_insert_does_not_fall_back_to_a_keystroke_typer ... ok
test paste::tests::ax_insert_uses_selected_text_when_settable ... ok
test paste::tests::ax_insert_skips_secure_and_non_text_roles ... ok
test paste::tests::ax_insert_splices_value_only_with_a_known_range ... ok
test paste::tests::unverified_ax_insert_copies_clipboard_fallback_without_arming_undo ... ok
test paste::tests::ax_success_outcome_is_not_a_keyboard_paste ... ok
test paste::tests::single_char_unknown_before_is_unverified_even_for_keyboard_paste ... ok
test paste::tests::cancelled_delivery_is_rejected_before_clipboard_stage ... ok
test paste::tests::ax_insert_is_skipped_when_cancelled_or_target_changed ... ok
test paste::tests::utf16_splice_inserts_cjk_at_utf16_range ... ok
test paste::tests::successful_shortcut_keeps_dictation_clipboard ... ok
test paste::tests::cancelled_attempt_never_runs_keyboard_injection ... ok
test paste::tests::changed_target_never_runs_keyboard_injection ... ok
test paste::tests::cancellation_after_target_check_never_runs_keyboard_injection ... ok
test paste::tests::panicking_keyboard_injection_is_recovered ... ok

test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 254 filtered out; finished in 0.00s
```

New: `unverified_ax_insert_copies_clipboard_fallback_without_arming_undo` — AX verified → `should_copy_clipboard_fallback` false; AX unverified → true and `used_keyboard_paste` false; keyboard paste (verified or not) → false.

### `cargo test --manifest-path src-tauri/Cargo.toml --lib undo_available_matches_armed`

F1 predicate (no live app): `done`/`degraded` with an armed transaction is true; `done` with no transaction (AX) is false; unverified/copied/expired/stale/consumed are false.

```
running 1 test
test tests::undo_available_matches_armed_unexpired_transaction_not_paste_method ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out; finished in 0.00s
```

### `npx vitest run src/components/Island/VoicePill.test.tsx`

```
 RUN  v4.1.10 /Users/mingjie/Documents/github/personal-projects/voice-flow

 Test Files  1 passed (1)
      Tests  22 passed (22)
   Start at  01:43:27
   Duration  614ms (transform 68ms, setup 44ms, import 106ms, tests 152ms, environment 252ms)
```

Keeps hiding 撤销插入 when `undoAvailable` is false. Non-success `undo_last_delivery` (`"not_available"`) logs `console.warn` and is not treated as success.

### `npx vitest run src/components/settings/EngineSettings.test.tsx` (F4)

```
 Test Files  1 passed (1)
      Tests  3 passed (3)
   Start at  01:43:28
   Duration  578ms (transform 72ms, setup 42ms, import 115ms, tests 108ms, environment 252ms)
```

---

## Full suites

| Command | Result |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **272 passed, 1 failed, 1 ignored** — only `store::tests::spool_writes_are_atomic_and_path_bounded` (`left: 493` / `right: 448` = `0o755` vs `0o700`, known non-slice; not fixed) |
| `npm test` | **154 passed, 1 failed** — only `SelectedPreviewDialog` duplicate `取消` (known non-slice; not fixed) |
| `npm run lint` (`tsc --noEmit`) | clean |

Pre-existing warning: unused `paste_selected_text` in `lib.rs` (live path is `selected_action.rs`). Not introduced here.

---

## Files changed

- `src-tauri/src/lib.rs` — `undo_available_for_hud` / `undo_available_from_app`; emit consults `state.undo` instead of `delivery_method == "paste"`
- `src-tauri/src/paste.rs` — `should_copy_clipboard_fallback`; unverified AX writes clipboard; verified AX still does not
- `src/components/Island/VoicePill.tsx` — log non-success `undo_last_delivery` results
- `src/components/Island/VoicePill.test.tsx` — hide undo when not armed; non-success invoke
- `src/components/settings/EngineSettings.tsx` — F4 ConfirmDialog copy
- `src/components/settings/EngineSettings.test.tsx` — dialog mentions `api.groq.com`
- `src/lib/i18n.tsx` — matching English key
- `.superpowers/sdd/final-fix-followup-report.md` — this report

---

## Concerns

1. **Known unrelated suite failures remain** (explicitly not fixed): spool mode `0o755` vs `0o700`, SelectedPreviewDialog duplicate `取消`.
2. **F3 / F5 / F6 skipped** as instructed (whole-sentence guard vs after-span; I6 constant-only test; no `settings_gate` harness).
3. **Clipboard write on unverified AX is best-effort** (`let _ = app.clipboard().write_text(text)`). A pasteboard error still returns the AX outcome; History remains the last resort. No live `AppHandle` clipboard assertion — the unit test covers the predicate.
4. **Island interactivity still keys off `delivery_method == "paste"`** for `done` / `unverified` / `degraded`. A verified AX insert keeps the island clickable for the 3s dwell even though Undo is hidden. Harmless leftover; not in this brief.
5. **C1 not reopened:** unverified AX copies to the clipboard but `used_keyboard_paste` stays false, so no 3s Cmd+Z.
