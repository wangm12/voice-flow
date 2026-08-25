# VoiceFlow — Complete Deep Review of the Uncommitted `dictation-slices` Work

- **Repo:** `/Users/mingjie/Documents/github/personal-projects/voice-flow`
- **Diff reviewed:** `4d25811..1d53e7c` — 88 files, +10 595 / −2 705
- **Plan:** `docs/superpowers/plans/2026-08-21-voiceflow-all-slices.md`
- **Mode:** read-only; working tree, index, HEAD and branch untouched

> **State note.** The brief described this work as uncommitted on `feat/dictation-slices`.
> By the time of this review the tree was already on `main` at commit `1d53e7c`
> ("Add dictation slices for tone, hybrid hotkey, HUD, learning, and paste."), whose parent
> is `4d25811`, with a clean working tree and no `feat/*` branch present. The reviewed
> content is identical — the same 88-file, +10 595/−2 705 change set — so every finding
> below applies unchanged; it now describes a landed commit on `main` rather than pending
> work. Nothing here was committed, checked out or modified by this review.

**Counts: 2 Critical (P0) · 8 Important (P1) · 6 Minor (P2)**

**Ready to merge? No.** Both test suites are red, and the dictionary learner silently
captures text the user typed in other applications and then uploads it to Groq. Everything
else is fixable without redesign. Since the change set has already landed on `main` as
`1d53e7c`, treat this as "needs follow-up fixes before release" rather than a gate on a
pending merge.

---

## Verification performed

| Gate | Result |
| --- | --- |
| `cargo test --lib` | **FAILED** — 272 passed, 1 failed, 1 ignored |
| `npm test -- --run` | **FAILED** — 154 passed, 1 failed (25 files, 1 failed) |

`cargo` is not on the interactive `PATH` on this machine; the suite was run through
`~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`.

---

## Strengths

These held up under direct reading of the code, not just the reports.

- **HUD partials are genuinely display-only.** `prefetch_asr.rs` inserts completed chunk
  transcripts into an in-memory map and invokes a callback; the only consumer is
  `emit_hud_partial` (`lib.rs:292`), which emits a Tauri event. Nothing on that path
  touches the clipboard, History or paste. Warmup output is deliberately kept out of the
  indexed results (`prefetch_asr.rs:186-189`) and a failed prefetch stays silent
  (`:203-208`). Both are covered by tests (`warmup_and_prefetch_failure_stay_silent_on_the_hud`).
- **Partial-event freshness is correct on both sides.** The backend gates the window
  resize on `event_generation == current_generation` and a live phase
  (`hud_partial_expands_window`, `lib.rs:315-327`); the HUD clears partials on a
  generation bump or `idle` (`voicePillTokens.ts`, `IslandWindow.tsx`). The 280-char cap is
  enforced in Rust and again in the pill.
- **The hybrid hotkey state machine is sound.** `hybrid_release_action` (280 ms), the
  `started_this_press` guard, the `Starting`-phase pending stop
  (`dictation.rs:206-231`) and the 400 ms `gesture_lock` compose correctly: a hold
  stops on release, a short tap latches recording, a press while recording toggles off,
  and a release with no recorded press is ignored. A missed key-up degrades to
  "recording continues until the next press", which is recoverable.
- **Undo remains fail-closed.** `used_keyboard_paste` is set only when Cmd+V was actually
  posted (`paste.rs:720`, `:766`), so an AX value-set never arms a 3-second Cmd+Z. The
  prior C1 fix holds, and `undo_available_matches_armed_unexpired_transaction_not_paste_method`
  still passes.
- **AX insertion is conservative in the right places.** `ax_insert_decision`
  (`paste.rs:411-439`) refuses secure fields and non-text roles, refuses a whole-value
  replacement without a known selection range, and never falls back to a keystroke typer.
  `utf16_splice` (`:443-461`) validates the range in UTF-16 units rather than guessing a
  byte or `char` index, which is exactly right for CJK.
- **Context detection does not leak identity.** `TargetAppGuard` carries PID plus opaque
  window/input/browser-target tokens and a host, never a window title, a full URL or a
  tab title, and none of those reach the LLM prompt. `display_label` and the applied
  policy use the *same* 0.75 confidence threshold (`context.rs:307`, `lib.rs:194`, `:210`),
  so the HUD never claims a tone that was not applied.
- **Secrets stay out of `settings.json`.** `SettingsView` exposes only
  `asr_api_key_configured` + a masked hint (`store.rs:506-508`), `save_settings` blanks
  `asr_api_key` before writing (`store.rs:1169`), and there is a test asserting the
  serialized view never contains the key.
- **`input_gain` is correctly plumbed.** Clamped to 0.5–4.0 with a NaN guard
  (`store.rs:318-322`), re-validated (`:428`), and applied once, post-resample and
  pre-chunk (`audio.rs:353`).
- **Snippets are safe.** Exact normalized matching, and the clipboard is read only when
  the matched expansion actually contains `{{clipboard}}`.
- **Settings writes are properly serialized.** `update_settings_patch` merges the patch
  **server-side** onto the current in-memory settings (`lib.rs:3754-3761`) under
  `settings_gate`, with an allow-list of fields. The frontend adds a 300 ms debounce and a
  promise chain (`useSettingsPersistence.ts`). Concurrent writes to *different* fields
  cannot clobber each other.

---

## Critical (P0)

### C1. The dictionary learner silently captures ordinary typing from other apps and uploads it to Groq

`observe_after_paste` diffs the focused field against the post-insert baseline and, when
exactly one token appears, returns it (`dictionary_learn.rs:167-170`);
`persist_learned_word_locked` appends it to the dictionary and writes settings to disk
(`:239-259`) with **no confirmation, no notification and no UI event**.

The diff has no insertion guard. `changed_spans` returns an empty `before_span` for a pure
append, so any new token in the appended text is a "candidate":

```
baseline "好的"  →  field "好的明天见"   ⇒ single_token_candidates → ["明天见"]  ⇒ persisted
```

Delta is 3 chars, well under `MAX_LENGTH_DELTA = 12`. The only related guard
(`:52-57`) rejects a candidate solely when it equals the *entire* `after` string, which a
suffix never does. So **continuing to type after a dictation is indistinguishable from
correcting it**, and for Chinese — the primary audience — any 2–8 character run qualifies.

Two consequences make this Critical rather than Important:

1. **It contradicts the consent the UI gives.** `DictionarySettings.tsx:156` reads
   "在历史里改正识别结果后，建议把新词加入个人词典，**需确认后才会写入**。" Two promises are broken:
   learning is not limited to History edits, and nothing is confirmed. The module doc
   itself says "silently appends" (`dictionary_learn.rs:4-5`).
2. **The captured text is exported to a third party.** The dictionary is injected into the
   ASR prompt on every subsequent dictation (`lib.rs:3112-3134`, "Recognize these terms
   exactly when spoken: …") and into the cleanup prompt (`llm.rs:629-631`, "Personal
   dictionary: …"). So a phrase the user typed into an unrelated application's text field
   is uploaded to Groq on the next dictation, without ever having been shown to them.

The existing guards (verified paste only, same target, same focused element, 3 s window,
`secure_input` skipped, single token, 12-char delta) bound the *volume* but not the
*category* of what is captured.

Tests miss it: every `observe_*` test uses a substitution
(`知呼`→`知乎`, `OK 知呼`→`OK 知乎`). There is no pure-insertion case.

**Fix.** In `single_token_candidates`, return empty when `before_span` is empty (a pure
insertion is never a correction). Then either drop the silent path entirely and route
post-paste candidates through the same confirm affordance History uses, or add an explicit,
separately-worded toggle plus a visible "已加入词典：X · 撤销" notice — and align the
`DictionarySettings` copy with whichever behaviour ships. Add the regression test.

**Files:** `src-tauri/src/dictionary_learn.rs:23-59`, `:135-174`, `:239-259`;
`src/components/DictionarySettings.tsx:156`; `src-tauri/src/lib.rs:3112-3134`;
`src-tauri/src/llm.rs:629-631`

### C2. Both test suites fail on this tree

The plan makes `cargo test` and `npm test` the gate after every slice. Neither passes.

**Rust — `store::tests::spool_writes_are_atomic_and_path_bounded`** (`store.rs:2201`)

```
assertion `left == right` failed
  left: 493   (0o755)
 right: 448   (0o700)
```

The test asserts the app-data **root** is `0o700` after a spool write, but
`write_spool_file_internal` only calls `ensure_private_dir(parent)` on the deepest
directory (`store.rs:792-797`), and `ensure_private_dir` does `create_dir_all` followed by
a chmod of the **final component only** (`store.rs:19-24`). Intermediate directories keep
the umask default. In production the root does end up `0o700` because settings/history
open paths call `ensure_private_dir(dir)` — but this test never goes through them, so it is
asserting a precondition it does not establish. Both `ensure_private_dir` and the assertion
are introduced by this tree, so this is in scope. Either harden every component in
`ensure_private_dir`, or assert the spool leaf (`dir/spool/session`) instead of the root.

**Frontend — `SelectedPreviewDialog.test.tsx:86`**

```
Found multiple elements with the role "button" and name "取消"
```

`SelectedPreviewDialog.tsx:66` gives the X icon `aria-label={t("取消")}` while `:93`
renders a footer button whose text is also `取消`. Duplicate accessible names break the
query, and — independently of the test — two differently-shaped controls with the same
name is an accessibility defect. Label the icon `关闭`. (The brief flags the duplicate
label itself as out of scope; the *red suite* is not, since both the component and its
test are new here.)

**Files:** `src-tauri/src/store.rs:19-24`, `:792-797`, `:2184-2204`;
`src/components/SelectedPreviewDialog.tsx:64-71`, `:93`;
`src/components/SelectedPreviewDialog.test.tsx:86`

---

## Important (P1)

### I1. A stored ASR key is not bound to the host it was entered for

`asr_credential()` returns `asr_api_key` for **any** `asr_base_url`
(`store.rs:436-444`), and `asr_base_url` is free text saved on every keystroke via
`save({ asr_base_url })` (`EngineSettings.tsx:159-168`). Nothing re-confirms or clears the
key when the host changes.

So: configure a key for host A → later point the base URL at host B → the key for A is
silently sent to B. Worse, because the field saves per keystroke behind a 300 ms debounce,
transient partial hosts typed on the way to the real one can become live endpoints that
receive the key. The prior round correctly scoped the *Groq* key to `api.groq.com`
(`asr.rs:33-38`); the dedicated ASR key has no host binding at all.

**Fix.** Store the host alongside the key and refuse to use it when
`transcription_host(asr_base_url)` differs, prompting for re-entry. At minimum, commit the
base URL on blur/explicit save rather than per keystroke, and clear the stored key when the
host changes.

### I2. No scheme or transport validation on `asr_base_url`

`normalize` only trims and truncates (`store.rs:323`); `validate` only checks length
(`:397-399`); `resolve_transcription_url` blindly concatenates (`asr.rs:16-30`).

- `http://remote-host/v1` sends the recorded **audio and the bearer token in plaintext**,
  with no warning anywhere in the UI.
- A scheme-less value (`mycloud.example.com`) produces a relative URL: `host_from_url`
  returns `None` (`asr.rs:44-45`), so `groq_key_fallback_allowed` is false and reqwest
  fails on a relative URL — surfaced to the user as a generic network error.

**Fix.** Require an absolute `http(s)://` URL in `validate`, and reject `http://` unless
the host is loopback (`127.0.0.1`, `::1`, `localhost`), which is the legitimate local-server
case the UI copy advertises.

### I3. A custom ASR host with no key fails only after the recording, with a Groq-branded error

When the host is custom and `asr_api_key` is empty, `asr_credential()` returns `""`
(`store.rs:441-443`). There is no preflight: `start_claimed` builds the prefetch options
with the empty key (`lib.rs:1090-1099`) and recording proceeds. The user speaks, then gets
`AsrError::Unauthorized`, whose message is hardcoded `"Groq authorization failed"`
(`asr.rs:77-78`) — pointing at the wrong service. Meanwhile the prefetch worker has already
**uploaded audio chunks to the custom host with an empty bearer** before giving up.

**Fix.** Reject the combination in `apply_settings` (settings-time error the form can show),
and make the unauthorized message name the resolved host.

### I4. Dictionary writes from the UI can drop backend auto-learned words

`update_settings_patch` merges server-side, which protects unrelated fields — but every
dictionary write sends **the whole array from the React snapshot**:
`DictionarySettings.tsx:44` (import), `:139` (add), `:206` (delete), and the History
confirm path. `persist_learned_word_locked` emits no event (`dictionary_learn.rs:239-259`),
and nothing in the frontend listens for backend-initiated settings changes — the only
`settings://` event is `ui-language`, and `App.tsx` re-reads settings solely after its own
actions.

So a word learned in the background between the last frontend read and the next dictionary
edit is silently discarded by that edit.

**Fix.** Emit a settings-changed event after `persist_learned_word_locked` and refresh the
frontend snapshot, or make dictionary mutations delta-based (`add_dictionary_word` /
`remove_dictionary_word`) instead of whole-array patches.

### I5. AX-insert verification uses `osascript` instead of the native AX read it already holds

`try_insert_inner` reads `AXValue`, `AXSelectedTextRange` and settability through a live
`AXUIElementRef` with a 350 ms messaging timeout — then, on success, `insert` verifies via
`crate::context::focused_input_value()` (`paste.rs:718-731`), which spawns `osascript` and
asks System Events. Two different AX mechanisms for the same read, and the second one is
both slower and much more failure-prone (Electron apps, browsers).

When that read fails, `verified` is false, and:

- `should_copy_clipboard_fallback` (`paste.rs:295-298`) overwrites the clipboard —
  and on the AX branch there is no `previous_clipboard` snapshot at all, so the user's
  clipboard is destroyed with no restore path;
- the HUD reports "unverified" and invites a manual paste, so the user pastes again into a
  field that already received the text → duplicate insertion.

It also adds a fixed 250 ms sleep plus an `osascript` spawn to every delivery.

**Fix.** Re-read `AXValue` through the same element inside `try_insert_inner` and return
the observed value, so `build_insert_outcome` verifies against a read that is known to
work. Keep the `osascript` read only as a fallback for the Cmd+V path.

### I6. Post-paste observation is expensive and re-runs the whole context detector

`maybe_observe_after_paste` spawns a blocking task that loops up to 7 times at 400 ms
(`dictionary_learn.rs:214-228`, `OBSERVE_WINDOW = 3 s`, `OBSERVE_INTERVAL = 400 ms`). Each
iteration performs:

- `context::focused_input_value()` — one `osascript` process, 350 ms budget;
- `context::detect_snapshot(...)` — a fresh AX probe **and**, when
  `browser_access_enabled`, the AppleScript browser-URL query.

That is up to ~14 `osascript`/AppleScript invocations in the 3 seconds after **every**
verified paste, when the target guard being compared against is already in hand.

**Fix.** Compare against the cached `TargetAppGuard` (window/input tokens) instead of
calling `detect_snapshot`, and break out of the loop on the first observed change rather
than polling the full window.

### I7. Spoken punctuation almost never fires, and produces wrong spacing when it does

`is_boundary_char` accepts only whitespace and ASCII/CJK punctuation
(`spoken_punctuation.rs:105-117`), and a match requires a boundary on **both** sides
(`:78`). Whisper's Chinese output is unspaced, so the realistic input
`你好逗号还好吗` never matches — `好` is not a boundary. The feature is effectively inert
in its main use case.

When it does match, adjacent spaces survive:

```rust
apply("你好 逗号 还好吗") == "你好 ， 还好吗"   // spoken_punctuation.rs:160
```

Full-width punctuation padded with spaces is wrong Chinese typography — and the test at
`:160` **asserts that wrong output as expected**, so the defect is locked in.

**Fix.** Treat a CJK ideograph as a valid left/right boundary, and collapse whitespace
adjacent to a substituted full-width mark. Update the test to expect `你好，还好吗`.

### I8. Paste suppression is a boolean, and a nested capture clears it before Cmd+V

`run_paste_attempt` opens the suppression window, then calls `verify_target()` *inside* it —
the comment at `paste.rs:353-354` states the intent explicitly: "Keep the final target check
inside the suppression window so the focused app cannot change between verification and
Cmd+V."

On the selected-text path, `verify_target` **is** `capture_selected_text`
(`selected_action.rs:149-165`), and that function ends with an unconditional
`set_paste_suppressed(false)` (`paste.rs:250`). `PASTE_SUPPRESS` is a plain `AtomicBool`
(`modifier_hotkey.rs:41`), not a depth counter, so the inner call closes the outer window.
`simulate_paste()` at `paste.rs:360` then posts Cmd+V **unsuppressed**.

With a modifier-only hotkey, the event tap sees that synthetic Cmd as a physical
press+release: `on_modifier_release` sets `awaiting_second_tap` for 400 ms
(`modifier_hotkey.rs:176-190`), so a real tap shortly after can register as a phantom
double-tap and start an unwanted dictation. The comment at `modifier_hotkey.rs:37-40` also
warns this re-entrancy previously aborted the main runloop.

The window itself predates this tree, but this tree lengthens it: `insert` now calls
`verify_target()` a **third** time (`paste.rs:718`, inside `try_ax_insert_if_safe`), and the
new `simulate_paste` additionally acquires an `AbcLayoutGuard` TIS switch inside the
now-unsuppressed region (`paste.rs:88-95`).

**Fix.** Make suppression a depth counter with an RAII guard so nested captures cannot
clear an outer window.

---

## Minor (P2)

### M1. `dictation://partial` is broadcast to every webview
`emit_hud_partial` uses `app.emit` (`lib.rs:292-299`), so in-progress transcripts are
delivered to the settings window as well as the island. Only `IslandWindow` subscribes, but
the narrower `emit_to("island", …)` matches the "HUD-only" contract in the doc comment.

### M2. `clear_all_data` recreates the spool without the new hardening
`store.rs:1604` uses `fs::create_dir_all(spool)` where every other spool path now goes
through `ensure_private_dir`, so the directory reverts to the umask default after a
"clear all data".

### M3. `input_gain` has no limiter
`apply_input_gain` multiplies and returns (`audio.rs:1097-1104`); the WAV encoder hard-clips
to `i16`. At the maximum 4.0, anything above 0.25 amplitude clips, which degrades ASR for
users who are not actually quiet. Neither the code nor `RecordingSettings.tsx` warns.
A soft limiter, or a level-meter clip indicator, would make the control safe to explore.

### M4. The ASR model is hardcoded while the endpoint is user-configurable
`asr_model` is not in the `update_settings_patch` allow-list (`lib.rs:3719-3750`) and
`EngineSettings.tsx:123` only displays it. Any OpenAI-compatible server that does not host
`whisper-large-v3-turbo` will fail on the model name after the user has correctly configured
the URL and key.

### M5. `scene_guidance` distinguishes personal from work chat by formality, not family
`llm.rs:857-860` selects the PersonalChat prompt via `policy.formality == "casual"` rather
than matching `ContextFamily`. Correct today, but any future family with
`artifact_kind == "chat_message"` and casual formality silently inherits the personal-chat
prompt. Match on the family.

### M6. Dead duplicate of `paste_selected_text` left behind by the module extraction
`lib.rs:1671-1712` is byte-identical to the live `selected_action.rs:130-171` and has zero
callers. 42 lines of delivery/undo/observe logic that can silently diverge from the copy
that actually runs. Delete the `lib.rs` copy. (Related: on the selected-text path
`insert` now performs three `verify_target()` calls — `paste.rs:711`, `:718`, `:355` — and
each one is a synthetic Cmd+C with a clipboard read and restore, one more round trip than
before this tree.)

---

## Prior-round fixes: still holding

| Prior finding | Status | Evidence |
| --- | --- | --- |
| Blind Cmd+Z after AX (C1) | Holds | `used_keyboard_paste` false on the AX branch (`paste.rs:720`); `ax_success_outcome_is_not_a_keyboard_paste` |
| Clipboard on the AX path | Holds, with a gap | Verified AX never writes the clipboard (`paste.rs:295-298`); unverified AX writes it with no restore path — see I5 |
| HUD `undo_available` vs armed transaction | Holds | `undo_available_for_hud` requires an armed, unexpired, preflight-clean transaction; test passes |
| Groq key only to `api.groq.com` | Holds | `groq_key_fallback_allowed` (`asr.rs:33-38`); the *dedicated* ASR key is unscoped — see I1 |
| Punctuation token boundaries | Holds, but over-tight | No mid-word rewriting; the boundary rule is now so strict the feature rarely fires — see I7 |
| Dictionary changed-span | Holds for substitutions | `changed_spans` protects Latin tokens and grows 1-char CJK fixes; pure insertions are unguarded — see C1 |

---

## Test-quality findings

- `store.rs:2201` asserts a directory mode the code under test never sets — **failing**.
- `spoken_punctuation.rs:160` asserts `"你好 ， 还好吗"`, locking in wrong CJK spacing.
- `SelectedPreviewDialog.test.tsx:86` queries an ambiguous accessible name — **failing**.
- No `observe_after_paste` test covers a pure insertion, which is exactly the C1 path.
- No test covers `asr_base_url` pointing at a custom host with no key (the I3 failure mode),
  nor a host change while a key is stored (I1).
- No test covers a dictionary patch racing a backend auto-add (I4).

---

## Recommendations, in order

1. Guard `single_token_candidates` against pure insertions and reconcile the silent
   post-paste write with the confirm-to-add promise in `DictionarySettings`. Add the
   regression test. **(C1)**
2. Get both suites green: fix or retarget the spool-mode assertion, and rename the dialog's
   icon label to `关闭`. **(C2)**
3. Bind the stored ASR key to its host, require an absolute URL, and refuse plaintext
   `http://` for non-loopback hosts. **(I1, I2)**
4. Validate the custom-host/no-key combination at settings time and stop branding that
   failure as Groq. **(I3)**
5. Verify AX inserts through the AX element already held, and give the AX branch the same
   clipboard snapshot/restore discipline as the Cmd+V branch. **(I5)**
6. Make paste suppression a counted RAII guard. **(I8)**
7. Cheapen the post-paste observer (cached guard, early exit) and emit a settings-changed
   event so the UI cannot clobber learned words. **(I4, I6)**
8. Make spoken punctuation fire on unspaced Chinese and normalize spacing around
   substitutions; fix the test. **(I7)**
9. Sweep the minors: scope the partial event, `ensure_private_dir` in `clear_all_data`,
   delete the dead `paste_selected_text`, match `scene_guidance` on family, and either
   expose `asr_model` or document the fixed model.

## Assessment

The architecture underneath this work is good. The concurrency discipline is real and
consistent — `settings_gate` around every settings mutation with a server-side patch merge,
`hotkey_gate` serializing phase entry, session generations and cancellation tokens threaded
through every async boundary, an operation lease preventing overlapping deliveries. The
privacy boundaries that were designed for are respected: no window title, PID or URL
reaches the LLM; HUD partials never touch the clipboard, History or paste; secrets stay out
of `settings.json`. The AX insertion path is unusually careful about the things that
normally break CJK, and the hybrid hotkey machine handles the awkward
release-during-`Starting` case correctly.

The blocking problems are at the edges where the new features touch persistence and the
network. The dictionary learner is the serious one: it is not a correction learner, it is a
"anything you type in the next three seconds" learner, and whatever it captures is uploaded
to Groq on the next dictation while the settings screen tells the user nothing is written
without confirmation. That is a consent defect, not a tuning problem, and it needs a
behaviour decision rather than a threshold change. The BYOK surface is the other cluster:
the key is unbound from its host, the URL is unvalidated, and the one invalid combination
that is easy to hit fails after the recording with the wrong service named.

Fix C1 and C2, land I1–I3 and I5, and this is mergeable. The remaining P1s and all P2s are
follow-up work that does not need to block.
