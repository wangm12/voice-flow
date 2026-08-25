# Final-review fix report

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`  
Source: `.superpowers/sdd/final-fix-brief.md` / `.superpowers/sdd/final-review-report.md`  
No commit. Cargo.toml `authors` left unchanged. No `asr_model` / Python / sherpa-onnx.

**Status: DONE_WITH_CONCERNS**

## Issue status

| Id | Status | Notes |
| --- | --- | --- |
| **C1** | fixed | `InsertOutcome.used_keyboard_paste` is true only if Cmd+V was posted. `arm_undo_transaction` returns unless `used_keyboard_paste && delivery_method == "paste"`. AX success never looks like a keyboard paste. |
| **I1** | fixed | AX insert runs *before* any clipboard write. Previous clipboard is restored whenever Cmd+V was not posted. Dictated text stays on the clipboard only after a posted Cmd+V. |
| **I2** | fixed | `insert()` stores `value_after`. `maybe_observe_after_paste` / `observe_after_paste` diff `post_insert_field` vs the later same-field value. Missing post-insert value skips learning. Selected-action uses the same helper. |
| **I3** | fixed | Candidates come from the changed span (prefix/suffix trim, Latin token completion, 1-char CJK grow to 2). Whole-sentence CJK >4 chars is rejected. `知呼`→`知乎` still `["知乎"]`; `今天去知呼看看吧`→`今天去知乎看看吧` yields `["知乎"]`, not the sentence. |
| **I4** | fixed | After the 3s poll (no gate held), persist is an async task that `lock().await`s `settings_gate`, then updates memory and `save_settings`. |
| **I5** | fixed | Spoken punctuation requires a token boundary on both sides (start/end, whitespace, or punctuation — not CJK letters). `画个句号` / `打个问号` / `这个逗号` / `括号里` unchanged. Unpaired `引号` left unchanged. |
| **I6** | fixed | After posting Cmd+V, `AbcLayoutGuard` is held for 40 ms (same band as the 30 ms pre-switch settle) before drop. |
| **I7** | fixed | `before == None` and 1-char `expected` is unverified. Multi-char `None`/`Some` still accepted. Inverted test updated. |
| **I8** | fixed | EngineSettings + i18n narrowed to OpenAI-compatible Whisper `/audio/transcriptions`. Empty still Groq. No `asr_model` field. |
| **I9** | fixed | `asr_credential` reuses `api_key` only when `asr_base_url` is empty/whitespace or the resolved host is `api.groq.com`. Custom hosts with empty `asr_api_key` send `""`. |
| privacy.md HUD + 3s poll | fixed | Local-only HUD partials and same-field post-paste poll documented. |
| `{{clipboard}}` gate | fixed | Clipboard is read only if the matched expansion contains `{{clipboard}}`. Read failure leaves the placeholder intact. |
| VoicePill dead ternary | fixed | Else arm is `status` (warning reserved for degraded / fallback / accessibility). |

Skipped as instructed: spool `0o700` vs `0o755`, SelectedPreviewDialog duplicate `取消`, `run_with_latin_layout_if_cjk`, HUD EN/ZH mix, swear fixture, `chat.focused`, Cargo.toml `authors`.

---

## Covering tests

Commands used `PATH` with `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` (`cargo` / `rustc` are not on the default PATH).

### `cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests`

```
running 19 tests
test paste::tests::macos_shortcuts_use_layout_independent_ansi_keycodes ... ok
test paste::tests::selection_fingerprint_changes_when_selected_text_changes ... ok
test paste::tests::paste_layout_is_held_until_cmd_v_can_be_consumed ... ok
test paste::tests::input_verification_accepts_single_ascii_and_cjk_characters ... ok
test paste::tests::input_verification_requires_a_new_occurrence_of_the_delivered_text ... ok
test paste::tests::ax_insert_splices_value_only_with_a_known_range ... ok
test paste::tests::ax_insert_does_not_fall_back_to_a_keystroke_typer ... ok
test paste::tests::ax_insert_skips_secure_and_non_text_roles ... ok
test paste::tests::ax_insert_uses_selected_text_when_settable ... ok
test paste::tests::single_char_unknown_before_is_unverified_even_for_keyboard_paste ... ok
test paste::tests::cancelled_delivery_is_rejected_before_clipboard_stage ... ok
test paste::tests::ax_success_outcome_is_not_a_keyboard_paste ... ok
test paste::tests::ax_insert_is_skipped_when_cancelled_or_target_changed ... ok
test paste::tests::utf16_splice_inserts_cjk_at_utf16_range ... ok
test paste::tests::cancelled_attempt_never_runs_keyboard_injection ... ok
test paste::tests::cancellation_after_target_check_never_runs_keyboard_injection ... ok
test paste::tests::successful_shortcut_keeps_dictation_clipboard ... ok
test paste::tests::changed_target_never_runs_keyboard_injection ... ok
test paste::tests::panicking_keyboard_injection_is_recovered ... ok

test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 253 filtered out; finished in 0.00s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::`

```
running 19 tests
test dictionary_learn::tests::observe_skips_secure_input ... ok
test dictionary_learn::tests::observe_skips_when_learning_disabled ... ok
test dictionary_learn::tests::observe_stops_when_focus_changes ... ok
test dictionary_learn::tests::observe_stops_when_target_changes ... ok
test dictionary_learn::tests::paragraph_rewrite_yields_empty ... ok
test dictionary_learn::tests::observe_ignores_paragraph_rewrite ... ok
test dictionary_learn::tests::skips_urls_and_password_masks ... ok
test dictionary_learn::tests::long_unspaced_sentence_with_one_char_fix_yields_the_word ... ok
test dictionary_learn::tests::more_than_three_new_tokens_is_a_rewrite ... ok
test dictionary_learn::tests::observe_diffs_post_insert_field_not_the_pasted_snippet ... ok
test dictionary_learn::tests::latin_comparison_is_case_insensitive ... ok
test dictionary_learn::tests::mixed_latin_token_typescript ... ok
test dictionary_learn::tests::observe_learns_zhihu_correction ... ok
test dictionary_learn::tests::cjk_correction_zhihu ... ok
test dictionary_learn::tests::observe_ignores_multi_token_rewrite ... ok
test dictionary_learn::tests::cjk_sentence_correction_uses_changed_span_not_whole_utterance ... ok
test dictionary_learn::tests::latin_correction_python ... ok
test dictionary_learn::tests::three_new_tokens_are_returned ... ok
test dictionary_learn::tests::append_dictionary_dedupes_and_respects_cap ... ok

test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 253 filtered out; finished in 0.00s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib spoken_punctuation::`

```
running 5 tests
test spoken_punctuation::tests::pairs_quotes_opening_then_closing ... ok
test spoken_punctuation::tests::unpaired_quote_is_left_unchanged ... ok
test spoken_punctuation::tests::leaves_embedded_punctuation_words_unchanged ... ok
test spoken_punctuation::tests::does_not_rewrite_surrounding_chat ... ok
test spoken_punctuation::tests::maps_standalone_spoken_punctuation ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 267 filtered out; finished in 0.00s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib snippets::`

```
running 6 tests
test snippets::tests::reads_clipboard_only_when_the_matched_expansion_needs_it ... ok
test snippets::tests::disabled_and_duplicate_triggers_are_safe ... ok
test snippets::tests::leaves_clipboard_placeholder_when_read_fails ... ok
test snippets::tests::expands_clipboard_placeholder_when_provided ... ok
test snippets::tests::expands_date_placeholder ... ok
test snippets::tests::only_full_phrase_matches ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 266 filtered out; finished in 0.00s
```

### `cargo test --manifest-path src-tauri/Cargo.toml --lib asr::tests::`

Cargo’s filter is a substring, so this also ran `prefetch_asr::tests::*`. All 18 matching tests passed, including `asr::tests::groq_key_fallback_is_limited_to_groq_hosts`.

```
running 18 tests
test asr::tests::auto_language_is_omitted ... ok
test asr::tests::parses_retry_after_fractional_and_fallback ... ok
test asr::tests::empty_base_url_resolves_to_groq_transcriptions ... ok
test asr::tests::resolves_host_v1_and_full_transcriptions_urls ... ok
test asr::tests::groq_key_fallback_is_limited_to_groq_hosts ... ok
test prefetch_asr::tests::hud_partial_text_does_not_include_warmup_and_is_display_only ... ok
test prefetch_asr::tests::hud_partial_text_joins_completed_chunks_in_index_order ... ok
test prefetch_asr::tests::hud_partial_text_caps_at_280_chars_with_ellipsis ... ok
test prefetch_asr::tests::prefetch_inbox_counts_dropped_chunks_without_blocking ... ok
test asr::tests::parses_headers ... ok
test prefetch_asr::tests::successful_non_warmup_chunks_emit_concatenated_hud_partials ... ok
test prefetch_asr::tests::warmup_and_prefetch_failure_stay_silent_on_the_hud ... ok
test prefetch_asr::tests::finish_retains_completed_chunks_after_an_inbox_drop ... ok
test asr::tests::mock_provider_can_model_empty_and_rate_limited_results ... ok
test asr::tests::provider_from_base_url_posts_to_resolved_mock_endpoint ... ok
test asr::tests::groq_provider_exposes_batch_and_prefetch_capabilities ... ok
test asr::tests::provider_success_and_empty_response_are_safe ... ok
test asr::tests::provider_auth_rate_limit_and_server_errors_are_classified ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 254 filtered out; finished in 0.11s
```

---

## Full suites

| Command | Result |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **270 passed, 1 failed, 1 ignored** — only `store::tests::spool_writes_are_atomic_and_path_bounded` (`0o755` vs `0o700`, umask-dependent, known non-slice; not fixed) |
| `npm test` | **151 passed, 1 failed** — only `SelectedPreviewDialog` duplicate `取消` (known non-slice; not fixed) |
| `npm run lint` (`tsc --noEmit`) | clean |

Pre-existing warning: unused `paste_selected_text` in `lib.rs` (live path is `selected_action.rs`). Not introduced by this wave.

---

## Files changed

- `src-tauri/src/paste.rs` — C1, I1, I6, I7; `used_keyboard_paste`; AX before clipboard
- `src-tauri/src/lib.rs` — arm undo only for keyboard paste; observe `value_after`; gated clipboard read
- `src-tauri/src/selected_action.rs` — same undo + observe wiring
- `src-tauri/src/dictionary_learn.rs` — I2, I3, I4
- `src-tauri/src/spoken_punctuation.rs` — I5 token boundaries
- `src-tauri/src/snippets.rs` — clipboard placeholder gate
- `src-tauri/src/history_commands.rs` — gated clipboard read
- `src-tauri/src/asr.rs` — I9 Groq-host helper + tests
- `src-tauri/src/store.rs` — I9 `asr_credential`
- `src/components/settings/EngineSettings.tsx` — I8/I9 copy
- `src/lib/i18n.tsx` — matching English keys
- `src/components/Island/VoicePill.tsx` — caption tone else-arm
- `docs/privacy.md` — HUD partials + 3s post-paste poll
- `.superpowers/sdd/final-fix-report.md` — this report

---

## Concerns

1. **Known unrelated suite failures remain** (explicitly not fixed): spool mode `0o755` vs `0o700`, SelectedPreviewDialog duplicate `取消`.
2. **I6 is a timed hold, not a consume-ack.** 40 ms after `CGEvent::post` matches the pre-switch settle; a slow target could still see paste after ABC restore. Did not hold the extra 250 ms post-insert sleep.
3. **I4 persist is fire-and-forget.** The 3s poll does not hold `settings_gate`; the follow-up async task does. There is no AppState harness test that the gate is taken — only the call-shape change.
4. **I8 is copy-only.** Compatible clients still send `model=whisper-large-v3-turbo` (plan: do not add `asr_model`). Non-Whisper OpenAI-compatible servers can still 400; UI no longer claims FunASR-native.
5. **Odd counts of standalone `引号` (>1) leave every quote unchanged** rather than pairing all but the last. Fail-closed vs emitting a lone `「`.
6. **Scheme-less `asr_base_url`** (e.g. `127.0.0.1:8000`) has no host, so Groq-key fallback is denied (safe). Failure still surfaces at transcription time.
7. **`lib.rs::paste_selected_text` is dead** (duplicate of `selected_action.rs`). Pre-existing; not cleaned up here.
