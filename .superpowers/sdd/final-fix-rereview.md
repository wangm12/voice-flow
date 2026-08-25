# Final-review FIX WAVE re-review

**Scope:** only the fix wave described in `.superpowers/sdd/final-fix-brief.md`, verified against the implementer's claims in `.superpowers/sdd/final-fix-report.md` and the original findings in `.superpowers/sdd/final-review-report.md`.
**Mode:** read-only. No files were modified, no git state was changed. The full suites were not re-run (the implementer's covering-test output was taken as given, but every claim was checked against the code).
**Not reopened:** spool `0o700` vs `0o755`, `SelectedPreviewDialog` duplicate `取消`, `Cargo.toml` `authors`, HUD EN/ZH label mix, `run_with_latin_layout_if_cjk`, swear fixture, `chat.focused`. None of these were made worse by this wave.

---

## 1. Spec verdict — did they close C1 and I1–I9 as specified?

**Yes. ✅ 10 / 10 required items closed, plus all three "cheap minors".**

### C1 — No Cmd+Z after AX insert ✅

`InsertOutcome` gained `used_keyboard_paste`, set from a single `build_insert_outcome(used_keyboard_paste, …)` argument (`paste.rs:200-216`, `301-318`). The AX-success early return passes `false` (`paste.rs:711-719`); the Cmd+V return passes `true` (`paste.rs:752-759`). `arm_undo_transaction` now bails on `!used_keyboard_paste || delivery_method != "paste"` before it touches `state.undo` (`lib.rs:1239-1244`), and both call sites — dictation (`lib.rs:2138-2145`) and selected-action (`selected_action.rs:222-229`) — forward the flag. The onboarding synthetic outcome also sets `used_keyboard_paste: false` (`lib.rs:2111-2117`). Tests `ax_success_outcome_is_not_a_keyboard_paste` and `single_char_unknown_before_is_unverified_even_for_keyboard_paste` cover the shape. The blind Cmd+Z on the AX path is genuinely gone.

*Follow-on gap this created — see F1 below.*

### I1 — Restore clipboard on the AX path ✅ (strongest possible form)

Rather than restoring after the fact, `insert()` was reordered so the AX attempt runs **before** any clipboard read or write (`paste.rs:706-719`). On AX success the function returns without ever touching the pasteboard, so there is nothing to restore and no dictated text left in the system clipboard. The Cmd+V path is unchanged and still restores whenever the shortcut was never posted (`paste.rs:295-299`, `744-748`). The stale comment the original review flagged is now accurate.

*Follow-on gap — see F2 below.*

### I7 — 1-char verification when `before` is unknown ✅

`input_value_verifies_delivery` returns `false` for `before == None && expected.chars().count() == 1` (`paste.rs:399-405`). The inverted assertion was restored and extended: `!(None, Some("inserted"), "i")`, `!(None, Some("x"), "x")`, `!(None, Some("字"), "字")`, while `(None, Some("inserted"), "inserted")` still verifies (`paste.rs:1003-1015`). Multi-char `None`/`Some` behaviour is untouched, as the brief allowed.

### I2 — Observe field-after-paste, not the pasted snippet ✅

`insert()` now records `value_after` (`paste.rs:213-215`, populated in `build_insert_outcome`), and both delivery paths pass `outcome.value_after.as_deref()` into `maybe_observe_after_paste` (`lib.rs:1208-1214`, `selected_action.rs:230-236`). `observe_after_paste` takes `post_insert_field` as its baseline and diffs it against the later same-field read (`dictionary_learn.rs:135-174`). A missing or empty post-insert value skips learning entirely (`dictionary_learn.rs:201-203`, `145-147`). Test `observe_diffs_post_insert_field_not_the_pasted_snippet` uses the exact `OK ` prefix scenario from the original finding and yields `知乎`, not the surrounding text.

### I3 — Do not learn a whole unspaced sentence ✅ (literal rule met; narrow bypass remains — F3)

`changed_spans` trims the common char prefix/suffix, then widens the span back out for Latin token continuation and grows a 1-char CJK fix to the 2-char minimum (`dictionary_learn.rs:61-114`). Candidates are extracted only from that span. The whole-sentence rejection is implemented as specified: single candidate equal to the entire `after`, more than 4 CJK chars → `[]` (`dictionary_learn.rs:52-57`). All three required cases hold, and I traced them by hand:

- `知呼` → `知乎` ⇒ `["知乎"]` (span grows left to reach 2 chars; 2 CJK ≤ 4, kept)
- `今天去知呼看看吧` → `今天去知乎看看吧` ⇒ `["知乎"]`, and the test explicitly asserts the sentence is *not* returned
- `我今天想去知呼看看风景` → `我今天想去知乎看看风景` ⇒ `["知乎"]` — the 9+ char inert case is now an improvement, as the brief hoped

`changed_spans` is also index-safe: every subtraction is bounded by the prefix/suffix computation and the final slices use `.get(..)`, so no panic path.

### I4 — Persist learned words under `settings_gate` ✅

`persist_learned_word` now spawns onto the Tauri async runtime and `persist_learned_word_locked` takes `state.settings_gate.lock().await` before mutating memory and calling `store::save_settings` (`dictionary_learn.rs:231-259`). This matches every other settings writer (`lib.rs:3292`, `3463`, `3679`, `3689`, `3745`, `3765`), so it can no longer interleave with `apply_settings`'s write-file-then-update-memory window. The gate is taken *after* the 3s poll completes — the `spawn_blocking` poll loop itself holds nothing (`dictionary_learn.rs:214-228`), as required. `tauri::async_runtime::spawn` (not `tokio::spawn`) is the correct choice from inside a blocking worker.

### I5 — Spoken punctuation: standalone tokens only ✅

`match_token_at` now requires `is_left_boundary && is_right_boundary`, where a boundary is start/end of string, whitespace, ASCII punctuation, or CJK/fullwidth punctuation ranges — explicitly *not* CJK letters (`spoken_punctuation.rs:73-117`). All four negative cases from the brief are asserted and pass by inspection: `画个句号`, `打个问号`, `这个逗号`, `括号里` (plus `写在括号里`). Positives `句号` and `你好 逗号 还好吗` hold. An odd count of standalone `引号` drops **all** quote replacements rather than emitting a lone `「` (`spoken_punctuation.rs:24-31`) — the brief's preferred "leave unchanged" behaviour.

### I6 — Hold ABC until paste is consumed ✅ (as specified)

`PASTE_CONSUME_SETTLE = 40ms` sits between `send_command_shortcut` and the end of `simulate_paste`, and `_latin_layout` is a named binding so the `AbcLayoutGuard` drops only after the sleep (`paste.rs:84-95`). 40 ms is inside the 30–50 ms band the brief asked for, and the extra 250 ms hold was correctly *not* added.

### I8 — UI copy, not a new ASR model field ✅

`EngineSettings.tsx:157` now reads "可选，例如 http://127.0.0.1:8000/v1。须为 OpenAI 兼容的 Whisper /audio/transcriptions 接口。留空则使用 Groq。" — the FunASR claim is gone, empty still means Groq, and the English key exists (`i18n.tsx:44`). No `asr_model` setting was added; `store.rs:468` `asr_model` is the pre-existing read-only `SettingsView` display field, not a new configurable.

### I9 — Groq key only to Groq ✅

`asr_credential` falls back to `api_key` only when `groq_key_fallback_allowed(&self.asr_base_url)` (`store.rs:434-445`); otherwise it returns `""`, which surfaces as the existing ASR auth error rather than key exfiltration. `groq_key_fallback_allowed` allows empty/whitespace and otherwise requires the resolved *host* to equal `api.groq.com` (`asr.rs:32-38`). `host_from_url` is written defensively and I could not break it: it strips userinfo via `rsplit_once('@')` (so `https://api.groq.com@evil.com/v1` resolves to `evil.com`, denied), handles bracketed IPv6, strips numeric ports, and lowercases. Suffix tricks (`api.groq.com.evil.com`) and path tricks (`evil.com/api.groq.com/v1`) are both denied. `groq_key_fallback_is_limited_to_groq_hosts` covers the empty, whitespace, case-insensitive, loopback, and third-party cases.

### Cheap minors ✅ ✅ ✅

- `docs/privacy.md:9-11` now documents both the local-only ~280-char HUD partials and the ~3s same-field post-paste poll, and states neither is sent to the LLM.
- Clipboard is read only when the *matched* expansion contains `{{clipboard}}` (`snippets.rs:106-129`), wired into all three pipelines (`lib.rs:1977-1981`, `lib.rs:2562-2569`, `history_commands.rs:259-263`). A failed read leaves the placeholder intact instead of expanding to `""` (`snippets.rs:131-139`), with a test that asserts the read closure is never called.
- `VoicePill.tsx:136-140` dead ternary resolved: the else arm is now `"status"`, and `.voice-pill-caption--status` exists in `island.css:118`.

---

## 2. Quality verdict — remaining issues in THIS wave

**Issues (not clean approval). Critical 0 · Important 2 · Minor 4.**

### Important

**F1. The HUD still advertises Undo for AX deliveries, so the button is now dead.** *(new, introduced by the C1 fix)*
`emit_state_with_delivery_and_input_device` computes `payload["undo_available"] = delivery_method == "paste" && matches!(state, "done" | "degraded")` (`lib.rs:460-461`). It was never taught about `used_keyboard_paste`. A verified AX insert produces `DeliveryMethod::Paste` → method string `"paste"` → state `"done"`, so `undoAvailable` is `true` and `VoicePill` renders the 撤销插入 button (`VoicePill.tsx:232-242`). But `arm_undo_transaction` returned early, so `state.undo` is empty and `undo_last_delivery` returns `"not_available"` (`lib.rs:1290-1292`). The frontend awaits `invoke` and discards the result (`VoicePill.tsx:125-135`), so the click is a completely silent no-op.

This is not a corner case: `ax_insert_decision` takes `AttemptSelectedText` whenever `AXSelectedText` is settable on a text field / text area / combo box / search field (`paste.rs:437-439`), which is the normal shape of native macOS text controls. The AX path is the *common* path, so the primary undo affordance becomes non-functional in most native apps — a straight regression from pre-wave behaviour where the button worked.

Fix: thread the same signal into the emit. Either add an `undo_available: bool` argument derived from `outcome.used_keyboard_paste && outcome.verified`, or have the emit consult `state.undo` (a transaction exists, unconsumed, unexpired) instead of inferring from the method string. Also worth surfacing a caption when `undo_last_delivery` returns anything other than `"ok"`, so a stale click is not silent.

**F2. AX success with `verified == false` leaves the user with no clipboard fallback.** *(new, introduced by the I1 fix)*
The AX early return happens before any clipboard write (`paste.rs:711-719`), which is exactly right for a verified insert. But `verified` is computed from `crate::context::focused_input_value()`, which shells out to `osascript` with a 350 ms timeout and returns `None` on AX error or timeout (`context.rs:1938-1983`). `AttemptSelectedText` in particular sets `AXSelectedText` on fields that may not expose a readable `AXValue`, so "AX call succeeded, verification unavailable" is a realistic outcome.

In that case delivery is reported as `paste_unverified` → HUD state `"unverified"` → "已尝试写入，请确认输入框内容" (`lib.rs:2148`, `3018-3022`), and nothing in that path copies the text (I checked every `paste_unverified` branch in `lib.rs`). Pre-wave, the Cmd+V path deliberately kept VoiceFlow's text on the clipboard precisely as this manual fallback (`paste.rs:744-748` comment). The user is now told to go check the field, with no copy to paste manually — only History. That weakens the same fail-closed delivery contract I1 was meant to strengthen.

Fix (cheap and contained): on the AX branch, when `!verified`, write `text` to the clipboard before returning — the previous clipboard has not been touched yet, so this is the one case where clobbering it is the desired behaviour, matching the existing Cmd+V rule.

### Minor

**F3. The whole-sentence guard compares against `after`, not the after-span.** `dictionary_learn.rs:52-57` rejects a lone candidate only when it equals the *entire* `after` string. Because candidates now come from the changed span, a long CJK run that is appended inside a larger string slips past: `single_token_candidates("。", "今天去知乎看看吧。")` yields `["今天去知乎看看吧"]` (prefix 0, suffix 1, span = the 8-char run, which is ≠ `after`). The brief's rule was implemented literally and the three required cases pass, so this is not a spec miss — but the underlying "don't learn a sentence" intent is only ~90% enforced. Comparing the candidate against the *after-span* instead of `after` would close it. Related and pre-existing: any text the user types into the same field within 3 s is still treated as a "correction", which is inherent to the feature's design rather than to this wave.

**F4. Inconsistent Groq-fallback copy between the two strings.** `EngineSettings.tsx:177` correctly says "仅默认 Groq 或 api.groq.com"; the removal ConfirmDialog at `EngineSettings.tsx:223` still says "仅 Groq 默认地址会改用上面的 Groq 密钥", which under-describes the `api.groq.com` custom-URL case. Both English keys exist and match their Chinese source, so `i18n.coverage.test.ts` will not catch the drift.

**F5. `paste_layout_is_held_until_cmd_v_can_be_consumed` tests a constant, not the hold.** `paste.rs:1038-1043` asserts only `30ms <= PASTE_CONSUME_SETTLE <= 50ms`. Nothing prevents a future edit from moving the sleep above `send_command_shortcut` or dropping the guard early. The implementer flagged this as concern 2 and it is honest — I6 is a timed hold, not a consume acknowledgement — but the guard placement itself is currently correct (`paste.rs:89-95`).

**F6. No test proves `settings_gate` is taken on the learn-write.** Acknowledged by the implementer as concern 3. The call shape is correct by inspection (`dictionary_learn.rs:239-241`), but there is no AppState harness asserting it, so a future refactor could silently drop the gate again.

Also still present, pre-existing and not caused by this wave: the `paste_selected_text` dead-code warning in `lib.rs` (live path is `selected_action.rs`), and `resolve_transcription_url` accepting scheme-less input. On the latter, the I9 fix now makes a scheme-less base like `127.0.0.1:8000` fail *closed* on the Groq-key fallback (no parseable host ⇒ denied), which is the safe direction.

---

## 3. Assessment

**Ready to merge? — With fixes.**

The wave did what it was asked to do. C1 is properly closed: `used_keyboard_paste` is a real, separately-derived signal rather than a rename of `shortcut_sent`, and both delivery call sites plus the synthetic onboarding outcome respect it, so a blind Cmd+Z can no longer follow an AX value set. I1 was closed in the strongest available form by reordering `insert()` so the AX attempt precedes the clipboard entirely. I3's span-based candidate extraction is the genuine rework the original finding asked for — not a length cap bolted on top — and it turns the previously inert 9+ char case into a working one. I9's host check is written defensively enough that I could not construct a URL that leaks the Groq key. I4, I5, I6, I7, I8 and all three cheap minors are each verifiable in the code, and the covering tests named in the report line up with functions that actually exist and assert the claimed behaviour.

What holds it back is that both of the two big fixes stopped one call site short of the surface. C1 removed the undo transaction but left `undo_available` in the state payload deriving purely from the delivery-method string, so the HUD keeps offering an Undo button that now silently does nothing on what is the most common delivery path in native macOS text fields (F1). I1 removed the clipboard write from the AX path but did not re-add it for the AX-succeeded-but-unverified case, so the one situation where the user is explicitly told to go check the field is also the situation where they have no copy to paste (F2). Both are small, local, and testable — F1 needs the flag threaded into the emit (or the emit reading `state.undo`), F2 needs a conditional clipboard write on the AX branch. Neither reopens a design question.

The three Minor items and the two known non-slice suite failures can follow in cleanup. The branch also still has no commits, which is orthogonal to this review but unchanged since the last one.

**Counts for this fix wave: Critical 0 · Important 2 · Minor 4.**
