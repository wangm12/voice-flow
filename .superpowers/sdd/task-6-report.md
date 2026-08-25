# Task 6 report — Post-paste short-window dictionary learning

**Status:** DONE_WITH_CONCERNS

## What shipped

After a **verified** paste, VoiceFlow watches only the same focused field for 3s (~400ms polls). If the user makes an unambiguous single-token correction, that token is silently appended to the in-memory dictionary (dedupe, cap 256) and persisted via `store::save_settings`. Rewrites (0 or >1 candidates), target/PID/window/focus changes, `secure_input`, onboarding fake paste, unverified paste, and `dictionary_learn_enabled == false` all learn nothing.

The observer lives in `dictionary_learn.rs` with injectable value/target/sleep so CI uses **no live AX**. Production polling is `spawn_blocking` after `paste_text` returns, so delivery is not blocked. Selected-text replacement calls the same helper. CJK AX/IME paste in `paste.rs` `insert()` was not modified. History confirm-to-add is unchanged. No global key event tap.

## Product checks

| Requirement | Result |
|---|---|
| `observe_after_paste` in `dictionary_learn.rs` | Pass |
| Injectable readers (no live AX in CI) | Pass |
| 3000ms window, ~400ms interval | Pass (`OBSERVE_WINDOW` / `OBSERVE_INTERVAL`) |
| `single_token_candidates(pasted, current)`; exactly one → append | Pass |
| 0 or >1 candidates → ignore | Pass |
| Stop on PID/window/focus/target change (`target_mismatch_reason`) | Pass |
| Skip `secure_input` (no poll) | Pass |
| Skip `dictionary_learn_enabled == false` (no poll) | Pass |
| Skip onboarding fake paste (`should_use_onboarding_delivery` never calls `paste_text`) | Pass |
| Skip unverified paste | Pass |
| One helper after verified paste (both dictation sites + selected-text) | Pass (`maybe_observe_after_paste`) |
| Does not block paste return (`spawn_blocking`) | Pass |
| Persist: in-memory `settings.dictionary` + `save_settings`; dedupe; cap 256 | Pass |
| Reuse `focused_input_value` + `target_mismatch_reason` / `detect_snapshot` | Pass |
| Never send window title/PID/raw URL to LLM | Pass (observer never talks to LLM) |
| No global keylog; History confirm-to-add still exists | Pass |
| Do not learn style/formality | Pass (single dictionary token only) |
| Do not break AX/IME paste or 3s undo | Pass (`paste.rs` `insert()` and `arm_undo_transaction` untouched) |
| Tests: 知呼→知乎 learns; paragraph rewrite ignores; target change stops; secure/toggle off skip | Pass |
| No Task 8; no commit | Pass |

## Self-review

- No global key event tap. The observer polls `focused_input_value` on the same target only.
- History `suggest_dictionary_entries` + confirm-to-add still exists. This slice is silent auto-add for **exactly one** post-paste candidate.
- Style/formality is not learned; only a single dictionary surface form.
- `paste.rs` CJK AX set + `AbcLayoutGuard` Cmd+V path is unchanged. 3s undo still arms from `InsertOutcome` as before.
- Onboarding fake paste never enters `paste_text`. Unverified and `secure_input` return before spawn.

## TDD evidence

**RED (observer APIs missing):**

```
error[E0432]: unresolved imports `super::append_dictionary_entry`, `super::observe_after_paste`
no `observe_after_paste` in `dictionary_learn`
no `append_dictionary_entry` in `dictionary_learn`
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
16 passed; 0 failed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::` | **16 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **252 passed, 1 failed, 1 ignored** |
| `npm test` (full) | **148 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–5 / 7)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not dictionary learning.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/dictionary_learn.rs` (`observe_after_paste`, persist helper, tests; file comment updated)
- `src-tauri/src/lib.rs` (`maybe_observe_after_paste` after `paste_text` — covers both dictation paste sites)
- `src-tauri/src/selected_action.rs` (same helper after verified selected-text replacement)
- `.superpowers/sdd/task-6-report.md` (this report)

Not committed. `paste.rs` was not modified.

## Concerns

1. Candidates compare **pasted text** to the **full focused field**. Surrounding tokens already in the control can look like a rewrite (>1 candidate) and be ignored. Fail-closed, matches the spec.
2. Persist updates `state.settings` then `save_settings` without `settings_gate`. A concurrent Settings UI save can race; last writer wins. Dedupe/cap still apply.
3. The 3s window runs on `spawn_blocking` (sleep + AX reads). It does not block paste return, but it occupies a blocking-pool thread for ~3s.
4. Observer uses `detect_snapshot` + `target_mismatch_reason` (same identity rules as `verify_delivery_target`) but does **not** retry transient `target_unavailable`. That is fail-closed: a brief Spaces/focus blip stops learning.
5. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
