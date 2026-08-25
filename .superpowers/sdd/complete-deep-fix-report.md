# Complete-deep follow-up fix report

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`  
Source: `.superpowers/sdd/complete-deep-fix-brief.md` / `.superpowers/sdd/complete-deep-review.md`  
Branch: `main`. No commit. No push. AX-undo, Groq-host fallback, and spoken-punctuation standalone tokens left unchanged. I6/I7 skipped.

**Status: DONE_WITH_CONCERNS**

## Issue status

| Id | Status | Notes |
| --- | --- | --- |
| **C1** | fixed | `single_token_candidates` returns `[]` when `before_span` is empty (pure insertion). `"好的"` → `"好的明天见"` is not learned; `知呼`→`知乎` and `OK 知呼`→`OK 知乎` still are. Silent post-paste path kept for substitutions only. DictionarySettings + i18n now describe History confirm-to-add **and** a 3s same-field correction after paste; toggle disables both. |
| **C2** | fixed | Spool test asserts `dir/spool/session` is `0o700` and the file `0o600`, not the temp root. SelectedPreviewDialog X button `aria-label` is `关闭`; footer stays `取消`. |
| **I1** | fixed | `bind_asr_key_to_host` / `asr_host_changed`: when the resolved transcription host changes and the patch does not include a new ASR key, the key is cleared in memory and Keychain. Empty Groq default and `api.groq.com` are the same host. EngineSettings commits `asr_base_url` on blur/Enter, not every keystroke. Saving a key includes the URL draft so host+key can land in one patch. |
| **I2** | fixed | `validate_asr_base_url` requires absolute `http://` or `https://`. `http://` is allowed only for `127.0.0.1`, `::1`, `localhost`. Form-facing Chinese errors are i18n keys. |
| **I3** | fixed | Custom host + empty `asr_api_key` is rejected in `Settings::validate` (settings apply) and again in `start_claimed` so recording does not start. `AsrError::Unauthorized` is now `ASR authorization failed ({host})`, not `Groq authorization failed`. |
| **I4** | fixed | After a successful `persist_learned_word_locked` save, emit `settings://changed` with `SettingsView`. App merges the payload so a later whole-array dictionary save sees the learned word. Same event is emitted when I1 clears the ASR key. |
| **I5** | fixed | `try_insert_inner` returns the post-set `AXValue` (splice estimate if the re-read is empty). `build_insert_outcome` on the AX path uses that value. Cmd+V still verifies via `focused_input_value` (osascript). AX path no longer sleeps 250ms waiting for osascript. |
| **I8** | fixed | `PASTE_SUPPRESS` is a depth counter with `PasteSuppressGuard`. Nested `capture_selected_text` cannot clear an outer `run_paste_attempt` window. Extra `false` pops saturate at zero. |
| **I6** | skipped | per brief |
| **I7** | skipped | per brief (standalone-token rule) |

---

## Covering tests

Commands used `PATH` with `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`.

### `cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::`

```
running 21 tests
test dictionary_learn::tests::observe_does_not_learn_pure_insertion_after_paste ... ok
test dictionary_learn::tests::pure_insertion_is_not_a_candidate ... ok
test dictionary_learn::tests::cjk_correction_zhihu ... ok
test dictionary_learn::tests::observe_diffs_post_insert_field_not_the_pasted_snippet ... ok
...
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 261 filtered out; finished in 0.00s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::spool_writes_are_atomic`

```
running 1 test
test store::tests::spool_writes_are_atomic_and_path_bounded ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 281 filtered out; finished in 0.01s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests`

```
running 22 tests
test paste::tests::ax_path_verifies_against_the_returned_ax_value ... ok
test paste::tests::nested_selection_capture_does_not_clear_outer_paste_suppress ... ok
test paste::tests::ax_success_outcome_is_not_a_keyboard_paste ... ok
...
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 260 filtered out; finished in 0.00s
```

### `npx vitest run src/components/SelectedPreviewDialog.test.tsx src/components/DictionarySettings.test.tsx src/lib/i18n.coverage.test.ts`

(run together with EngineSettings + App covering tests)

```
Test Files  5 passed (5)
     Tests  22 passed (22)
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib`

```
test result: ok. 281 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.19s
```

Pre-existing warning only: `paste_selected_text` in `lib.rs` is unused (M6, not in this brief).

### `npm test`

```
Test Files  25 passed (25)
     Tests  156 passed (156)
```

### `npm run lint`

```
> tsc --noEmit
(exit 0)
```

**Full cargo/npm: green. 0 failed.**

---

## Files changed

```
src-tauri/src/asr.rs
src-tauri/src/dictionary_learn.rs
src-tauri/src/lib.rs
src-tauri/src/modifier_hotkey.rs
src-tauri/src/paste.rs
src-tauri/src/store.rs
src/App.test.tsx
src/App.tsx
src/components/DictionarySettings.test.tsx
src/components/DictionarySettings.tsx
src/components/SelectedPreviewDialog.test.tsx
src/components/SelectedPreviewDialog.tsx
src/components/settings/EngineSettings.test.tsx
src/components/settings/EngineSettings.tsx
src/lib/i18n.tsx
```

15 files, +490 / −64. Not committed.

---

## Concerns

1. **I1 + I3 together.** Changing to a custom host without a new `asr_api_key` in the same patch fails settings apply (key is cleared, then validate rejects keyless custom). The URL is not saved until the user commits a key (EngineSettings sends URL draft with “保存 ASR 密钥”, and blur sends the key draft if present). Same-host edits (including port on `127.0.0.1`) keep the key.
2. **I4 race.** `settings://changed` refreshes the React snapshot. A dictionary whole-array patch already queued in the 300ms debounce can still omit a word learned in that window.
3. **Optimistic URL.** `save()` still applies `asr_base_url` locally before persist. A rejected I2/I3 save can leave the field showing a URL the backend did not accept until reload/retry.
4. **AX re-read vs osascript.** AX verification no longer uses `focused_input_value`. If `AXValue` after `AXSelectedText` is empty, we fall back to a splice estimate or the inserted text; a wrong estimate could mark verified when the app ignored the set, or unverified and copy the clipboard. Cmd+V still uses osascript.
5. **Skipped.** I6 (observer `detect_snapshot` volume) and I7 (CJK punctuation boundaries) untouched. Minors M1–M6 untouched, including dead `paste_selected_text` (warns on `cargo test --lib`).
6. **Loopback still needs an ASR key.** `http://127.0.0.1` is allowed by I2 but I3 still requires `asr_api_key` (dummy key is enough for a local server that ignores Authorization).
