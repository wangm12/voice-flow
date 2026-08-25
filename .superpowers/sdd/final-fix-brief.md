# Final-review fix brief

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Source: `.superpowers/sdd/final-review-report.md` ([终审](005e52b8-2472-41a3-9632-41a5a2149866)).

Do **not** commit. Do **not** “fix” spool `0o700` vs `0o755` or SelectedPreviewDialog duplicate `取消`. Do **not** revert unrelated non-slice work. Do **not** change Cargo.toml `authors`. Do **not** change HUD Chinese labels (plan-mandated). Do **not** add FunASR `asr_model` / Python / sherpa-onnx.

Write report to `.superpowers/sdd/final-fix-report.md`.

## Must fix

### C1 — No Cmd+Z after AX insert
AX `SetAttributeValue` is often **not** on the target undo stack. `paste.rs` currently treats a successful AX insert as `PasteAttempt::sent()`, so `arm_undo_transaction` arms a 3s Cmd+Z (`lib.rs` / `selected_action.rs`).

**Fix:** Distinguish AX delivery from Cmd+V. Do **not** arm undo for the AX path (fail-closed: never send Cmd+Z unless we know it will undo *our* insert). Keep 3s undo for verified Cmd+V.

Add `InsertOutcome` field such as `used_keyboard_paste: bool` (true only if Cmd+V was posted). `arm_undo_transaction` only when `verified && used_keyboard_paste`. Tests: AX-success outcome does not look like a keyboard paste; verification tests still pass.

### I1 — Restore clipboard on AX path
`insert()` always writes `text` to the clipboard, then AX success sets `shortcut_sent` so restore is skipped.

**Fix:** Restore the previous clipboard when Cmd+V was **not** posted (AX success, AX skip then cancelled, etc.). Keep VoiceFlow text on the clipboard only after a posted Cmd+V (manual fallback). Do not write the clipboard *before* a successful AX insert if you can avoid it; if you still write it, restore after AX.

### I7 — 1-char verification when `before` is unknown
`input_value_verifies_delivery(None, Some("x"), "x")` currently returns true. A false verified arms Cmd+Z (C1).

**Fix:** If `before` is `None` and `expected` is a single character, return false (unverified). Update the inverted test. Multi-char `None`/`Some` behavior can stay.

### I2 — Observe field-after-paste, not the pasted snippet vs whole field
`observe_after_paste` calls `single_token_candidates(pasted, current)` where `current` is the full field.

**Fix:** Baseline is the focused value **immediately after verified insert** (`value_after` from `insert()`, already read). Pass that string into `maybe_observe_after_paste` / `observe_after_paste`. Then `single_token_candidates(post_insert_field, later_field)`. If post-insert value is missing, skip learning. Selected-action uses the same helper.

### I3 — Do not learn a whole unspaced sentence
CJK 2–8 char runs make `今天去知呼看看吧` → `今天去知乎看看吧` a single token.

**Fix:** Trim common prefix/suffix (char-wise) and extract candidates only from the **changed span**. Keep CJK 2–8 and Latin rules inside that span. If the only candidate equals the entire `after` string and is longer than 4 CJK chars, return `[]`. Tests:
- `知呼` → `知乎` still `["知乎"]`
- `今天去知呼看看吧` → `今天去知乎看看吧` → `[]` or `["知乎"]` from the span, **not** the whole sentence
- 9+ unspaced sentence with one-char fix inside the span **may** now yield the corrected word (improvement over inert)

### I4 — Persist learned words under `settings_gate`
`persist_learned_word` mutates settings and `save_settings` without the gate. `apply_settings` writes disk then memory.

**Fix:** Persist via an async task that `lock().await`s `state.settings_gate` (same as `update_settings_patch`), then updates memory + `save_settings`. Do not hold the gate during the 3s poll.

### I5 — Spoken punctuation: standalone tokens only
Prefix match at every char turns `画个句号` into `画个。`.

**Fix:** Replace only when the keyword has a token boundary on both sides: start/end, whitespace, or punctuation (not CJK letters). Negative tests: `画个句号`, `打个问号`, `这个逗号`, `括号里` unchanged. Positive: whole utterance `句号`; `你好 逗号 还好吗`. Unpaired single `引号`: leave unchanged or do not emit a lone `「` (prefer leave unchanged).

### I6 — Hold ABC until paste is consumed
`AbcLayoutGuard` drops as soon as `CGEvent::post` returns.

**Fix:** After posting Cmd+V, sleep ~30–50 ms (same order as the pre-switch settle) **before** the guard drops. Do not hold 250 ms extra unless tests need it.

### I8 — UI copy, not a new ASR model field
Compatible client still sends `whisper-large-v3-turbo`. Do **not** add `asr_model`.

**Fix:** Narrow EngineSettings + i18n: this is an OpenAI-compatible Whisper `/audio/transcriptions` endpoint, not a FunASR-native server. Empty still Groq.

### I9 — Groq key only to Groq
`asr_credential` sends the Groq key to any custom URL.

**Fix:** Fall back to `api_key` only when `asr_base_url` is empty/whitespace **or** the resolved transcription URL host is `api.groq.com`. Custom hosts require `asr_api_key` (empty key → existing ASR errors, not Groq-key exfil). Test the fallback.

## Cheap minors (do if small)

- `docs/privacy.md`: mention local HUD partials and 3s same-field post-paste poll (not sent to LLM).
- Read clipboard for snippets only if the matched expansion contains `{{clipboard}}`. If read fails, leave `{{clipboard}}` intact (do not expand to `""`).
- `VoicePill.tsx` dead ternary `warning : warning` — pick one arm.

Skip: `run_with_latin_layout_if_cjk` (test helper), HUD EN/ZH mix, swear fixture, `chat.focused`, Cargo.toml authors.

## Tests

Covering tests (must run and paste output in the report):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
cargo test --manifest-path src-tauri/Cargo.toml --lib spoken_punctuation::
cargo test --manifest-path src-tauri/Cargo.toml --lib snippets::
cargo test --manifest-path src-tauri/Cargo.toml --lib asr::tests::
```

Use `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo` if `cargo` is missing. Then `--lib`, `npm test`, `npm run lint`.

## Report

Status DONE | DONE_WITH_CONCERNS | BLOCKED. List each C/I id as fixed or deferred with reason. Paste covering-test commands and results.
