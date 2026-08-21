# VoiceFlow All-Slices Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh implementer per slice. Parent reviews. Serial only. Do not commit unless the controller's dispatch explicitly allows it.

**Goal:** Land P0–P1 dictation upgrades as eight independently testable slices, without turning VoiceFlow into a meeting app or keylogger.

**Architecture:** Keep Groq batch ASR + optional LLM cleanup + fail-closed paste. Change prompts, hotkey state machine, HUD events, dictionary learning, ASR endpoint, and paste insertion. No Python FunASR. No sherpa-onnx in this plan.

**Tech Stack:** React 19 + Vitest, Tauri 2, Rust (`src-tauri` crate `voiceflow` / lib `tauri_app_lib`). Tests: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `npm test`.

**Work from:** `/Users/mingjie/Documents/github/personal-projects/voice-flow` on branch `feat/dictation-slices`.

## Global Constraints

- macOS system dictation only. No meetings, Ask Anything, emoji IME, swear-filter-as-a-feature.
- Copy UX, not GPL source (TypeWhisper / VoiceInk / FluidVoice).
- Fail-closed paste, target lock, 3s undo, preview-first, protected facts stay.
- Never send window title / PID / raw URL to the LLM. No telemetry. No global keylog.
- Chat families must not add 您好/Hello, expand into email prose, or sanitize swears.
- Cleanup prompt changes need corpus fixtures in `src-tauri/src/cleanup_corpus.rs`. No real History in eval data.
- Do not commit. Do not change license. Do not reformat unrelated files.
- Do not implement a later slice. If blocked, report BLOCKED.
- Follow existing code style. Prefer small focused modules over growing `lib.rs`.
- English UI strings must have `t()` keys covered by `src/lib/i18n.coverage.test.ts`.

**Verify after every slice:**
```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

---

### Task 1: Per-app tone (Slice 1)

**Files:**
- Modify: `src-tauri/src/context.rs` (`default_writing_prompt`, `ContextPolicy::for_family`, `display_label`, `normalize_writing_modes`, tests around line 2268)
- Modify: `src-tauri/src/cleanup_corpus.rs` (add personal_chat cases; extend `corpus_covers_the_p0_quality_shapes`)
- Modify: `src/components/Island/VoicePill.tsx` and `VoicePill.test.tsx`
- Modify: `src/components/Island/voicePillTokens.ts` if caption composition lives there
- Modify: `src/components/ContextSettings.tsx` (style-example copy)
- Modify: `src/lib/i18n.tsx` for new strings
- Test: existing `context.rs` unit tests, `VoicePill.test.tsx`, `cleanup_corpus.rs`

**Interfaces:**
- Keep `pub fn default_writing_prompt(family: ContextFamily) -> &'static str`
- PersonalChat and WorkChat prompts MUST differ
- `display_label` for PersonalChat uses `口语`; WorkChat uses `工作短讯`; Email stays professional-equivalent `邮件` or keep `Professional` only for Email. WeChat native fixture must become `WeChat · 口语` (app_label is `WeChat`).
- Slack fixture: `Slack · 工作短讯`
- `const OLD_SHARED_CHAT_PROMPT: &str = "Keep the message natural, short, and conversational. Do not turn it into an email or add greetings/sign-offs.";`
- `normalize_writing_modes`: if a saved builtin `work_chat` / `personal_chat` prompt equals `OLD_SHARED_CHAT_PROMPT`, replace with the new family default. Custom prompts stay.

**PersonalChat prompt (verbatim):**
```
Keep the user's casual chat voice. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Do not sanitize swearing, slang, or particles like 哈哈. Prefer light punctuation. Never turn the message into an email.
```

**WorkChat prompt (verbatim):**
```
Keep the message short and conversational for workplace chat. Do not add greetings, sign-offs, or email structure. Keep names and project terms exact. Do not add emoji unless spoken.
```

**Policy:** Split `WorkChat | PersonalChat` match. PersonalChat: `formality = "casual"`, `sentence_completeness = "preserve_fragments_when_intentional"`, extra forbidden_additions: `"greetings not spoken"`, `"swear-word sanitization"`. WorkChat: `formality = "neutral"`, keep concise, still forbid email greetings.

**HUD:** `VoicePill` currently receives `contextLabel` but tests require the HUD to be text-free while recording/processing. Change that: while `state` is `recording`, `recording_limited`, `starting`, or `processing`, render `contextLabel` in the caption (or a compact chip) when present. Do not show ASR transcript here. Update tests that currently `queryByText("Chrome Canary")` / `/General/` — they must now expect the label. Keep error/degraded captions taking priority over context label.

**Settings:** In mapping style-example UI, placeholder/help: `贴一条你平时微信怎么打` / English `Paste a typical WeChat message`. Do not invent a new intensity enum.

**Corpus:** Add at least:
- `zh_wechat_casual`: raw `好的哈哈我晚点回你` expected contains `哈哈` and `晚点`, must not contain `您好` or `稍后回复`
- `zh_wechat_swear_kept`: raw includes a mild swear; expected must not replace it with 书面语 like `有待商榷`
- `work_chat_not_email`: existing or new; expected must not look like an email greeting

`allow_rewrite: false` for personal_chat casual cases. Update `corpus_covers_the_p0_quality_shapes` to require `personal_chat`.

**Out of scope:** hybrid hotkey, FunASR, dictionary auto-learn, HUD transcripts.

**Report:** DONE / DONE_WITH_CONCERNS / BLOCKED plus tests run.

---

### Task 2: Hybrid hotkey (Slice 2)

**Files:**
- Modify: `src-tauri/src/hotkey.rs` (combo-key `on_shortcut` currently only `ShortcutState::Pressed` → `hotkey://toggle`)
- Modify: `src-tauri/src/dictation.rs` (`handle_hotkey_toggle`, possibly new `handle_hotkey_press` / `handle_hotkey_release`)
- Modify: `src-tauri/src/store.rs` and `src-tauri/src/lib.rs` settings patch that currently rejects `hold` and non tap/double_tap
- Modify: `src/lib/activationCopy.ts`, `ActivationModeSelector.tsx` + test, `RecordingSettings.tsx`, onboarding `TryItStep.tsx` / `tryItHint`
- Modify: `src/types/settings.ts` if needed

**Interfaces:**
- `activation_mode` allowed: `"tap" | "double_tap" | "hybrid"`
- Modifier-only hotkeys STILL force `double_tap`. Never hold/hybrid on fn/⌘-only.
- Legacy `hold` for non-modifier maps to `hybrid` (not tap).
- Constant `HYBRID_HOLD_MS: u64 = 280`

**Behavior for combo keys in hybrid:**
- Pressed: if idle → start recording and remember press time. If already recording from a prior short tap → stop (toggle off).
- Released: if this press started recording and elapsed >= 280ms → stop (PTT). If elapsed < 280ms → keep recording (short tap toggle-on).
- Tap mode unchanged: press still toggles via existing `hotkey://toggle`.
- Ignore synthetic key events during paste (existing modifier_hotkey comments).

Implement press/release with `ShortcutState::Pressed` and `ShortcutState::Released` if the plugin exposes Released; if not, report BLOCKED with evidence rather than guessing.

Unit-test the timing policy as a pure function, e.g.:
```rust
pub fn hybrid_release_action(elapsed_ms: u64, started_this_press: bool) -> HybridReleaseAction
// Hold { elapsed 400, started_this_press true } -> Stop
// Tap { elapsed 80, started_this_press true } -> KeepRecording
```

**Out of scope:** modifier-only PTT, HUD transcripts.

---

### Task 3: HUD in-progress words (Slice 3)

**Files:**
- Modify: `src-tauri/src/prefetch_asr.rs` — after a successful non-warmup chunk, emit HUD-only event
- Modify: `src-tauri/src/lib.rs` `emit_state_*` or a sibling `emit_hud_partial`
- Modify: `src/components/Island/IslandWindow.tsx` listen `dictation://partial`
- Modify: `VoicePill.tsx` show partial text during recording/processing
- Tests: prefetch unit tests; IslandWindow/VoicePill tests

**Interfaces:**
- Event name: `dictation://partial`
- Payload: `{ session_generation: u64, text: String }`
- Concatenate completed chunk transcripts in chunk index order, space-separated, trim, cap at 280 chars for HUD (ellipsis if longer)
- NEVER write partials to clipboard, History `raw_text`/`final_text`, or paste
- Cancel / idle / generation bump clears partial
- Prefetch failure stays silent; final batch ASR remains source of truth

**Privacy:** payload is transcript text only, no app title.

**Out of scope:** true streaming ASR, FunASR, changing paste.

---

### Task 4: History dictionary learning (Slice 4)

**Files:**
- Create: `src-tauri/src/dictionary_learn.rs` and `mod dictionary_learn` in `lib.rs`
- Create: `src-tauri/src/dictionary_learn.rs` tests
- Modify: History save path — prefer a Tauri command so Rust owns the algorithm
- Modify: `src/components/History/History.tsx` — stop using Latin-only `localDictionaryCandidates`; call the command
- Modify: `src/components/DictionarySettings.tsx` — toggle `dictionary_learn_enabled` (default true)
- Modify: `src-tauri/src/store.rs` add `dictionary_learn_enabled: bool` default true
- Modify: `src/types/settings.ts`

**Interfaces:**
```rust
pub fn single_token_candidates(before: &str, after: &str) -> Vec<String>
```
Rules:
- Max 3 candidates
- Accept: CJK runs of 2–8 chars, Latin tokens `[A-Za-z][A-Za-z0-9._-]{1,}`, mixed like `TypeScript`
- Candidate is a token present in `after` and not in `before` (normalize: lowercase Latin; CJK exact)
- If edit changes more than 3 tokens OR `after` length differs from `before` by > 12 chars, return empty (treat as rewrite)
- Never suggest tokens that look like passwords (contains `***`) or URLs

Command: `suggest_dictionary_entries { before: String, after: String } -> Vec<String>`

After History save, show candidates (CJK included). Confirm still writes `dictionary` via existing patch. Optional: if `dictionary_learn_enabled` and the same `(from,to)` is not needed — first version is confirm-to-add, not silent auto-add. Auto-add only when candidate set is exactly one CJK/Latin proper-noun token AND user already has `dictionary_learn_enabled`. Safer: keep confirm UI; add auto-add only for repeated identical candidate across saves if cheap. Minimum: CJK candidates appear and confirm works.

Tests:
- `知呼` → `知乎` yields `知乎`
- `配森` → `Python` yields `Python`
- paragraph rewrite yields `[]`

**Out of scope:** AX post-paste monitor (Task 6), FunASR hotwords.

---

### Task 5: OpenAI-compatible ASR BYOK (Slice 5)

**Files:**
- Modify: `src-tauri/src/asr.rs` — `GroqAsrProvider` already posts to OpenAI-shaped `/audio/transcriptions`. Add settings-driven endpoint.
- Modify: `src-tauri/src/store.rs`, `keychain.rs` if adding a second secret
- Modify: `src/components/settings/EngineSettings.tsx`
- Modify: `src/types/settings.ts`
- Modify: `docs/privacy.md` — where audio goes
- Tests: existing groq mock endpoint tests in `asr.rs`

**Interfaces:**
- Settings: `asr_base_url: String` (empty = `https://api.groq.com/openai/v1`)
- Transcription URL = `{asr_base_url.trim_end_matches('/')}/audio/transcriptions` if base already ends with `/v1`, else `{base}/v1/audio/transcriptions` if user pastes host only. Also accept a full URL that already contains `audio/transcriptions`.
- `asr_api_key` Keychain account `asr_api_key`. If missing, fall back to existing Groq key.
- UI: text field “ASR 兼容地址（可选，例如 http://127.0.0.1:8000/v1 或 FunASR server）” and optional ASR key field.
- Dictionary still goes through `build_asr_prompt` (Whisper-style prompt). Do not invent FunASR-Nano hotword JSON unless the mock shows a field already used.

**Out of scope:** bundling Python, sherpa-onnx, skipping LLM automatically for chat.

---

### Task 6: Post-paste short-window learning (Slice 6)

**Depends on:** Task 4 `single_token_candidates`.

**Files:**
- Modify: `src-tauri/src/paste.rs` after verified paste
- Create or extend: `src-tauri/src/dictionary_learn.rs` with `observe_after_paste`
- Tests with a fake value provider, not real AX in CI

**Behavior:**
- After verified paste of `pasted`, if `dictionary_learn_enabled` and not secure_input:
  - For 3000ms, at ~400ms interval, re-read `focused_input_value` only if target lock still matches (reuse existing verify_target / stale checks)
  - If focus/PID/window changed → stop
  - If value now differs, `single_token_candidates(pasted, current)` → if exactly one candidate, append to dictionary (dedupe, cap 256)
  - If many tokens changed → ignore (rewrite)
- Never install a global key event tap for this
- Skip password/secure fields (`secure_input`)

**Out of scope:** learning style/formality, uploading data.

---

### Task 7: CJK paste chain (Slice 7)

**Files:**
- Modify: `src-tauri/src/paste.rs`
- Possibly new `src-tauri/src/input_source.rs` (macOS TIS)
- Tests: mock/feature-gate non-macOS; macOS unit tests for “should switch input source” predicate

**Behavior:**
1. If Accessibility allows setting focused AXValue (or selected range insert) without Cmd+V, try that first. If you cannot do this safely in-process, skip AX set and document DONE_WITH_CONCERNS — do not half-implement a keystroke typer.
2. Else existing Cmd+V path. If current input source is CJK (name/id contains `Hans`, `Hant`, `Kotoeri`, `Pinyin`, `IMK`, `SCIM`, `TCIM`, `Hiragana`, `Korean` — use actual TIS IDs you find), switch to ABC (`com.apple.keylayout.ABC` or `US`) for the paste, then restore.
3. Keep value verification and 3s undo.
4. Fail-closed: on any uncertainty, clipboard fallback, no blind Cmd+Z.

**Out of scope:** fixing WeChat stealing Fn (Task 8).

---

### Task 8: Punctuation, whisper gain, Fn hint, snippet placeholders

Split internally but one implementer may do all four if small. Prefer four commits logically in one working tree without git commit.

**8a Spoken Chinese punctuation** — new `src-tauri/src/spoken_punctuation.rs` applied to ASR text before cleanup:
- 左括号/右括号 → （）
- 顿号 → 、
- 引号 → 「」 or “” (use 「」 for 中文)
- 斜杠 → /
- Also 逗号/句号/问号/感叹号 if easy
- Whole-utterance replacements only for those words as standalone tokens (don’t replace inside 括号里)
- Tests for those mappings
- Must not otherwise formalize chat

**8b Whisper / VAD** — settings `input_gain: f32` default 1.0, clamp 0.5–4.0 for quiet speech; apply in audio capture before chunker. Optional `vad_sensitivity` scaling `Vad.fixed_floor`. UI in RecordingSettings. Tests for gain multiply.

**8c Fn conflict hint** — in RecordingSettings, if hotkey is modifier-only fn/globe, show static help: WeChat / 微信输入法 may steal Fn. No OS hook required. i18n.

**8d Snippet placeholders** — in `snippets::resolve_exact`, after choosing expansion, replace:
- `{{date}}` → local `YYYY-MM-DD`
- `{{clipboard}}` — only if you can read clipboard without breaking paste restore; if paste pipeline already snapshots clipboard, use that snapshot. If unsafe, support `{{date}}` only and report concern.
- Tests for `{{date}}`

**Out of scope:** Scribe, screen assistant, sherpa-onnx.

---

## Controller notes

Execute Task 1 → parent review → 2 → … → 8. Never two implementers at once. Parent must not write feature code. Fix via fix subagent.
