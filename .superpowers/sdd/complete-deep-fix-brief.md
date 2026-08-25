# Follow-up fix after complete deep review

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

You are on **`main`** at `1d53e7c` (dictation slices already fast-forwarded). Do **not** create a branch unless you must. Do **not** commit. Do **not** push. Do **not** revert C1 undo/AX/Groq-host/punctuation-standalone (those must stay).

Read: `.superpowers/sdd/complete-deep-review.md`

Write report to: `.superpowers/sdd/complete-deep-fix-report.md`

## Must fix

### C1 — Do not learn pure insertions
`single_token_candidates` / `changed_spans`: if `before_span` is empty (append-only typing after paste), return `[]`. `"好的"` → `"好的明天见"` must not yield `明天见`. Substitutions still learn: `知呼`→`知乎`, `OK 知呼`→`OK 知乎`.

Keep silent post-paste path for **substitutions** (plan). Do not require History-style confirm for that path.

Update `DictionarySettings` copy + i18n: learning is (1) History confirm-to-add, and (2) a 3s same-field **correction** after paste — not arbitrary continued typing. Toggle still disables both.

Regression test for the append case.

### C2 — Make the two failing tests green
1. `spool_writes_are_atomic_and_path_bounded`: `ensure_private_dir` only chmods the leaf. Either chmod all created ancestors, or assert `dir/spool/session` is `0o700` and the file `0o600` — not the temp root if you never chmod it. Prefer asserting the spool directory you actually create.
2. `SelectedPreviewDialog`: X button `aria-label` must not be `取消`. Use `关闭` (i18n). Footer stays `取消`. Fix the test query.

### I1 — ASR key vs host change
When `asr_base_url` host changes (resolved transcription host), clear `asr_api_key` (Keychain + memory) so a key entered for host A is not sent to host B. Prefer saving the URL on blur/commit rather than every keystroke if that is a small change in EngineSettings.

### I2 — ASR URL scheme
`validate`/`normalize`: require absolute `http://` or `https://`. Reject `http://` unless host is loopback (`127.0.0.1`, `::1`, `localhost`). i18n error the form can show.

### I3 — Custom host without key
Reject custom `asr_base_url` + empty ASR key at settings apply (do not start recording). Change `AsrError::Unauthorized` message so it is not hardcoded "Groq authorization failed" when the endpoint is not Groq (name the host or say "ASR authorization failed").

### I4 — Don't drop auto-learned words
After `persist_learned_word_locked`, emit a settings-changed event the frontend already can handle, **or** re-fetch settings. Frontend must refresh `dictionary` so a later whole-array save does not clobber the learned word.

### I5 — Verify AX insert via the same AX element
`try_insert_inner` should return the post-set `AXValue` (or selected-text result). `build_insert_outcome` uses that for verification on the AX path. Keep `focused_input_value` (osascript) for Cmd+V. Fewer false `unverified` → less clipboard clobber.

### I8 — Paste suppress must nest
`PASTE_SUPPRESS` is a boolean; `capture_selected_text` sets it false and clears `run_paste_attempt`'s window before Cmd+V. Use a depth counter (or restore previous value) so nested capture cannot uns suppress the outer paste.

## Skip
- **I6** (observer osascript volume) unless a tiny change (don't call full `detect_snapshot` if you already have a cheap token check).
- **I7** (CJK as punctuation boundary) — conflicts with standalone-token rule that left `画个句号` unchanged. Do not reopen.
- Minors unless one-liners.

## Tests
cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo` if needed:

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::spool_writes_are_atomic
cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests
npx vitest run src/components/SelectedPreviewDialog.test.tsx src/components/DictionarySettings.test.tsx src/lib/i18n.coverage.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

Suites must be green except if you cannot fix spool without a design fight — then report. Goal: **0 failed** for C2.

Paste covering-test commands + output in the report.
