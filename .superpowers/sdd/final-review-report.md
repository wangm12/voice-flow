# Final Review — VoiceFlow dictation slices (Tasks 1–8)

**Scope reviewed:** uncommitted working tree vs `HEAD 4d25811` (= merge-base with `main`) on `feat/dictation-slices`.
**Plan:** `docs/superpowers/plans/2026-08-21-voiceflow-all-slices.md`
**Reviewer mode:** read-only. No files were modified, no git state changed.

**Verification run locally**

| Command | Result |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 260 passed, 1 failed, 1 ignored — only `store::tests::spool_writes_are_atomic_and_path_bounded` (mode `0o755` vs `0o700`, umask-dependent, pre-existing/non-slice) |
| `npm test` | 151 passed, 1 failed — only `SelectedPreviewDialog` duplicate `取消` (non-slice component) |
| `npm run lint` (`tsc --noEmit`) | clean |

Neither failure is caused by slice code, per the controller's instruction and confirmed by reading both call sites.

**Scope note:** the working tree also contains substantial work that is *not* part of the eight slices — encrypted recovery spool + `encrypted-spool` feature and keychain history key (`store.rs`, `keychain.rs`), extraction of `dictation.rs` / `history_commands.rs` / `selected_action.rs` out of `lib.rs`, the settings-UI split (`src/components/settings/*`, `useSettingsPersistence.ts`), `SelectedPreviewDialog`, and `realtime_asr.rs` → `prefetch_asr.rs` rename. Both known suite failures come from that non-slice code. Findings below are limited to slice behaviour unless stated.

---

## Strengths

- **All eight slices are present and traceable to the plan.** The PersonalChat / WorkChat prompts are verbatim (`src-tauri/src/context.rs:387-388`), the policy split matches the spec (`context.rs:122-136`), `OLD_SHARED_CHAT_PROMPT` migration only touches builtin chat modes and leaves custom prompts alone (`context.rs:456-466`), and `llm.rs:854-861` routes scene guidance through the same two prompts so HUD/LLM cannot drift.
- **Hybrid hotkey is modelled as a pure function and tested at the boundary.** `hybrid_release_action` / `HYBRID_HOLD_MS = 280` (`src-tauri/src/hotkey.rs:13-30`), `combo_hotkey_event` keeps tap mode on `Pressed` only, and the Starting-phase race is handled explicitly by `request_hybrid_stop` + `enter_recording` (`src-tauri/src/dictation.rs:206-231`). Modifier-only hotkeys are still forced to `double_tap` in both `normalize` and `validate` (`store.rs:255-269`, `369-380`), and legacy `hold` migrates to `hybrid` only for combo keys.
- **HUD partials respect the privacy contract.** The payload is `{session_generation, text}` only (`lib.rs:292-299`), warmup chunks are excluded (`prefetch_asr.rs:186-198`), text is index-ordered/trimmed/capped at 280 chars with ellipsis, and partials never touch clipboard, History, or paste. Generation bump and `idle` clear the partial on both sides (`lib.rs:285-288`, `IslandWindow.tsx` `hudPartialAfterState`).
- **No global keylog anywhere in Task 6.** `observe_after_paste` polls only the already-locked focused field, re-checks `target_mismatch_reason` on every tick, and bails on `secure_input` or disabled learning — with tests that `panic!` if the fakes are ever called in those states (`dictionary_learn.rs:69-108`, `439-468`).
- **BYOK secret handling is correct.** `asr_api_key` goes to Keychain, is blanked in `settings.json` and in the legacy backup, is redacted in `SettingsView`, and an explicit `remove_asr_api_key` command exists (`store.rs:1097-1116`, `1159-1169`, `lib.rs:3751-3769`). URL resolution handles host / `/v1` / full-path forms with tests (`asr.rs:14-30`, `476-531`).
- **Fail-closed and RAII discipline in the new macOS FFI.** `AbcLayoutGuard` restores on drop including on panic, refuses to "restore" a switch that never happened, and refuses an ASCII-capable source that still looks CJK (`input_source.rs:141-218`, tests at `373-433`). `utf16_splice` refuses out-of-range AX ranges instead of guessing byte indices, and `ax_insert_decision` explicitly refuses secure fields, non-text roles, whole-value replacement, and any path that would need a keystroke typer (`paste.rs:379-427`).
- **Global constraints hold:** no meetings / Ask Anything / emoji IME / swear filter; no Python or sherpa-onnx added (only `chacha20poly1305`, and that belongs to the non-slice spool work); no telemetry; no window title / PID / URL added to any LLM payload; corpus fixtures added for personal chat with the required assertions (`cleanup_corpus.rs:430-456`, `477-508`); `docs/privacy.md` updated for the compatible ASR endpoint; all new literal `t()` keys are covered by `src/lib/i18n.coverage.test.ts`.

---

## Issues

### Critical

**C1. Undo after an AX insert fires a blind Cmd+Z, which can destroy the user's own text.**
`paste.rs:685-695` — when `try_ax_insert_if_safe` succeeds, the outcome is reported as `PasteAttempt::sent()` and `InsertOutcome.shortcut_sent = true`, so `arm_undo_transaction` arms the 3s undo exactly as for a Cmd+V delivery (`lib.rs:2133-2139`, `selected_action.rs:222-228`). `undo_last_delivery` then sends Cmd+Z (`lib.rs:1336`, `paste.rs:770-776`).

The `AttemptValueSplice` branch delivers text by `AXUIElementSetAttributeValue(AXValue, …)` (`paste.rs` `macos_ax::try_insert_inner`). Programmatic AX value sets are frequently **not** registered in the target application's undo stack, so Cmd+Z will undo whatever the user typed *before* dictating while VoiceFlow's inserted text stays in the field — the opposite of the intended undo, and it destroys user content. The `post_insert_input_fingerprint` check only proves the field is unchanged since insert; it cannot prove Cmd+Z will remove our text.

This directly contradicts Task 7's "Keep value verification and 3s undo" plus "Fail-closed: on any uncertainty, clipboard fallback, no blind Cmd+Z", and the global constraint "fail-closed paste … 3s undo".

Suggested fix (either is acceptable): do not arm the undo transaction when delivery used the AX path, or record the pre-insert value and make undo restore it through AX instead of Cmd+Z. At minimum, restrict AX delivery to `AttemptSelectedText` (which typically goes through `insertText:replacementRange:` and *is* undoable) and drop `AttemptValueSplice`.

### Important

**I1. The user's clipboard is clobbered even when the clipboard was never used.**
`paste.rs:696-700`. `insert()` always writes `text` to the clipboard first, then on the AX path returns `shortcut_sent = true`, so `should_restore_clipboard` is false and the previous clipboard is never restored. The comment immediately above says "Restore only when Cmd+V was never posted" — with the AX path that invariant is now false. The dictated text also silently persists in the system clipboard (a small leak into every other app). Fix: track whether Cmd+V was actually posted separately from `shortcut_sent`, and restore the clipboard on the AX path.

**I2. Post-paste learning compares the pasted text against the entire field value.**
`dictionary_learn.rs:98-105` calls `single_token_candidates(pasted, current)` where `current` is the whole focused-field value, not the inserted span. Any pre-existing text in the field is seen as "new tokens". The ±12-char delta guard bounds this, but a field that already held a short token (e.g. `OK `) plus a 1-token correction still yields exactly one candidate and is written to the dictionary silently. The same call in `selected_action.rs:229-235` compares the replacement against the whole document. Fix: diff against `value_before + pasted` (both are already available in `insert()`), or pass the pre-paste value through to the observer.

**I3. CJK tokenisation makes the dictionary feature both wrong and inert for typical Chinese.**
`dictionary_learn.rs:209-219` treats any CJK run of 2–8 chars as a single token. Chinese is unspaced, so:
- a 9+ char utterance produces **no** token at all — one-character corrections in normal sentences yield nothing (parent-noted, confirmed);
- a ≤8 char utterance produces exactly one token equal to the whole sentence, which then passes the `candidates.len() == 1` gate and is silently appended to the dictionary and fed into `build_asr_prompt`. Example: `今天去知呼看看吧` → `今天去知乎看看吧` learns the entire 8-char sentence.
Fix: locate the changed span (common prefix/suffix trim) and only propose tokens inside it, then cap CJK candidates at ~4 chars.

**I4. `persist_learned_word` writes settings without holding `settings_gate`.**
`dictionary_learn.rs:162-181` mutates `state.settings` and calls `store::save_settings` from a `spawn_blocking` task. Every other settings writer serialises on `state.settings_gate` (`lib.rs:3670`, `3680`, `3736`, `3756`), and `apply_settings` writes the file *before* updating the in-memory copy (`lib.rs:3615`, `3637`). A learn-write landing in that window persists a stale snapshot and silently reverts the user's just-saved setting. Fix: hand the word to an async task that takes `settings_gate` (or route it through `update_settings_patch`).

**I5. Spoken punctuation replaces substrings, not standalone tokens.**
`spoken_punctuation.rs:15-43` does prefix-matching at every character boundary, but the plan required "whole-utterance replacements only for those words as standalone tokens". `括号里` is safe only because bare `括号` is not in the table; the frequent words are not: `画个句号` → `画个。`, `打个问号` → `打个？`, `这个逗号` → `这个，`. A single spoken `引号` also emits an unbalanced `「`. It runs unconditionally on every transcript in all three pipelines (`lib.rs:1972`, `lib.rs:2551`, `history_commands.rs:258`) with no setting to disable it, and it rewrites the value stored as History `raw_text`. Fix: require a token boundary (whitespace, punctuation, or start/end) around the keyword, and add the corresponding negative tests.

**I6. The CJK→ABC layout is restored before the paste is guaranteed to be consumed.**
`paste.rs:86-88` acquires `AbcLayoutGuard` and drops it as soon as `send_command_shortcut` returns. `acquire_guard` sleeps 30 ms *before* the paste (`input_source.rs:163`) but nothing sleeps before the restore, and `CGEvent::post` is asynchronous. The target app can process Cmd+V after the input source has already flipped back to Pinyin — exactly the failure Task 7 exists to fix. Fix: sleep a short interval before the guard drops (mirroring the 30 ms settle), or keep the guard alive across the existing 250 ms post-paste settle in `insert()`.

**I7. Single-character delivery verification was weakened.**
`paste.rs:339-367`. The old `expected.chars().count() < 2` rejection is gone, and the deleted assertion `!input_value_verifies_delivery(None, Some("inserted"), "i")` was inverted into a positive assertion. With `before == None`, any readable field containing that character now counts as verified. A false `verified` marks the delivery successful in History *and* arms the 3s Cmd+Z undo (which, per C1, can then undo something else). The `expected_chars == 1 && before_count > 0` guard only covers the `Some(before)` branch. Fix: keep 1-char deliveries unverified when `before` is unknown.

**I8. The "compatible" ASR client is still hard-wired to Groq's Whisper request shape.**
`asr.rs:264-266` always sends `model=whisper-large-v3-turbo` and `response_format=verbose_json`, and there is no model setting. The UI text promises otherwise: `"可选，例如 http://127.0.0.1:8000/v1 或 FunASR server。留空则使用 Groq。"` (`EngineSettings.tsx:157`). Most local/FunASR servers will 400 on that model name. Either add an `asr_model` field or narrow the UI copy to "OpenAI-compatible Whisper endpoints".

**I9. The Groq key is forwarded to any user-supplied endpoint.**
`store.rs:434-442` (`asr_credential`) falls back to `api_key` whenever `asr_api_key` is empty — as the plan specified — and `asr_base_url` accepts any string including plain `http://` non-local hosts with no scheme or host check (`asr.rs:14-30`, `store.rs:323`). The UI does disclose the fallback (`EngineSettings.tsx:177`), which limits the blast radius, but a typo'd or hostile base URL exfiltrates the Groq key over cleartext. Fix: only reuse the Groq key when the resolved host is `api.groq.com`, and warn on non-loopback `http://`.

### Minor

- **Two parallel layout implementations.** `run_with_latin_layout_if_cjk` (`input_source.rs:43-61`) is exercised only by tests; production uses `AbcLayoutGuard`. The tested abstraction is not the shipping path. Either use it in `simulate_paste` or delete it.
- **Dead branch in caption tone.** `VoicePill.tsx` computes `state === "degraded" || fallbackReason || … ? "warning" : "warning"` — both arms are identical.
- **HUD labels mix languages regardless of `ui_language`.** `context.rs:309-320` now returns `邮件` / `工作短讯` / `口语` alongside `Code`, `Search`, `Clear`, `Command`, `Form`. These Rust-side strings bypass `i18n.tsx`, so an English-UI user sees a half-Chinese HUD. (The plan did prescribe the labels; the mixing is the by-product.)
- **Unrelated manifest edit.** `src-tauri/Cargo.toml:5` changes `authors` from `["you"]` to a corporate email address. Outside slice scope ("do not reformat unrelated files"), and it embeds an employer identity in a personal project.
- **Weak swear fixture.** `zh_wechat_swear_kept` uses `这破需求` (`cleanup_corpus.rs:439-447`); `破` is barely a swear, so the case is unlikely to catch real sanitisation. The corpus cases are static fixtures with no model assertion, so they document intent rather than verify it.
- **`resolve_transcription_url` accepts scheme-less input.** `127.0.0.1:8000` becomes `127.0.0.1:8000/v1/audio/transcriptions`; the failure only surfaces at transcription time, after the user has left Settings.
- **`{{clipboard}}` silently expands to an empty string** when the clipboard read fails or is non-text (`snippets.rs` `apply_placeholders`, `lib.rs:52-55`). A visible "clipboard unavailable" behaviour or leaving the placeholder intact would be less surprising.
- **Clipboard is read on every dictation** (`clipboard_text_for_snippets`, `lib.rs:52-55`) even when no snippet uses `{{clipboard}}`. It never reaches the LLM (snippet expansion bypasses cleanup, `lib.rs:1995`), but it does land in History when a snippet fires. Gate the read on the expansion actually containing the placeholder.
- **`docs/privacy.md` does not mention two new observable behaviours:** the 3-second post-paste polling of the focused field's contents, and on-screen HUD partial transcripts. Both are local-only, but both belong in the privacy doc.
- **`chat.focused` moved into the workplace-chat guidance arm** (`llm.rs:884-893`); a focused-window fallback for WeChat-style apps now gets work-chat tone. Low impact, easy to split.

---

## Recommendations

1. **Fix C1 before any merge.** Simplest safe version: skip `arm_undo_transaction` when the delivery used the AX path, or drop `AttemptValueSplice` and keep only `AttemptSelectedText`. Add a test asserting no undo transaction is armed for an AX delivery.
2. **Fix I1 and I7 in the same pass** — both are one-line invariants in `paste.rs` and both are fail-closed guarantees the plan names explicitly.
3. **Rework the dictionary candidate extraction (I2 + I3) around the changed span** rather than whole-string token sets, and add the two Chinese cases that currently fail: a 1-char fix inside a 10+ char sentence (should yield the corrected word) and an 8-char sentence (must not yield the whole sentence).
4. **Serialise the learn-write through `settings_gate` (I4)**; today it is the only settings writer that bypasses the gate.
5. **Add token-boundary matching plus negative tests to `spoken_punctuation` (I5)** — `画个句号`, `打个问号`, `这个逗号`, and a single unpaired `引号`.
6. **Hold the ABC layout across the post-paste settle (I6).**
7. **Either add an ASR model setting or narrow the UI copy (I8), and restrict the Groq-key fallback to Groq hosts (I9).**
8. **Non-blocking cleanup:** revert the `Cargo.toml` `authors` change, delete or adopt `run_with_latin_layout_if_cjk`, fix the dead caption-tone branch, and add the post-paste-observation + HUD-partial paragraphs to `docs/privacy.md`.

---

## Assessment

**Ready to merge? — With fixes.**

The slice work is well structured, well tested at the seams, and stays inside every stated product boundary (no meetings, no keylog, no telemetry, no Python/sherpa, no title/PID/URL to the LLM, chat prompts that actively forbid 您好/Hello/email expansion/swear sanitisation). Eight of eight slices land what the plan asked for, and the suites are green apart from the two known non-slice failures.

What blocks a clean merge is that Task 7's new AX insert path was added *in front of* the existing Cmd+V pipeline without re-deriving the two safety invariants that pipeline owned: the undo contract (C1 — a blind Cmd+Z that can delete the user's own text) and the clipboard-restore contract (I1). Task 4/6's dictionary learning is also not yet correct for the language it was built for (I2, I3) and can silently write whole sentences into the ASR prompt, and it is the one settings writer that races the settings gate (I4).

C1 is a must-fix. I1–I4 should be fixed before this reaches users, since all four write or destroy user-visible state silently. I5–I9 and the Minor list can follow in a cleanup pass. There are also no commits yet — the branch needs its eight logical commits before merge regardless.

**Counts: Critical 1 · Important 9 · Minor 10.**
