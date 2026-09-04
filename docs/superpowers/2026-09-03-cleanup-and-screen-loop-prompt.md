# Loop prompt — cleanup + screen awareness

Paste the **Tick prompt** block into `/loop`. Keep this file next to the two specs; each tick must re-read the specs, not this header.

**Specs (source of truth):**

- [2026-09-03-dictation-cleanup-enhancement-design.md](specs/2026-09-03-dictation-cleanup-enhancement-design.md)
- [2026-09-03-screen-awareness-design.md](specs/2026-09-03-screen-awareness-design.md)

**Suggested invocation:**

```text
/loop dynamic

<paste Tick prompt below>
```

Use `dynamic` so the agent self-paces: implement a slice, verify, then wake when tests finish or after a short heartbeat. Do not use a tight `30s` interval; this is implementation, not a deploy poll.

**Stop the loop when** the completion gate at the bottom of the Tick prompt is all true, or the agent reports `BLOCKED` it cannot resolve.

---

## Tick prompt

Copy everything between `BEGIN_TICK_PROMPT` and `END_TICK_PROMPT`.

```
BEGIN_TICK_PROMPT

You are continuing VoiceFlow work in:
/Users/mingjie/Documents/github/personal-projects/voice-flow

This is one loop tick. Do a single vertical slice, verify it, then stop the tick.
Do not implement the whole program in one tick. Do not ask "should I continue?"
— the loop will wake you again.

================================================================
0. READ FIRST (every tick)
================================================================

Read, in this order:

1. docs/superpowers/specs/2026-09-03-dictation-cleanup-enhancement-design.md
2. docs/superpowers/specs/2026-09-03-screen-awareness-design.md
3. This file: docs/superpowers/2026-09-03-cleanup-and-screen-loop-prompt.md
4. docs/privacy.md, docs/asr-cleanup-later-and-wont.md, docs/end-to-end-workflows.md
5. git status + git diff (know what is already in progress)
6. If docs/superpowers/plans/ has an implementation plan for these specs, read it
   and pick the next unchecked task. If no plan exists yet, THIS TICK's only job
   is to write TWO implementation plans with writing-plans skill:
   - docs/superpowers/plans/2026-09-03-dictation-cleanup-enhancement.md
   - docs/superpowers/plans/2026-09-03-screen-awareness.md
   Then end the tick. Do not start coding until both plans exist.

Specs win over this prompt. This prompt wins over improvisation.
Do not "improve" the product beyond the specs.

================================================================
1. HARD CONSTRAINTS (verbatim)
================================================================

Product:
- macOS system dictation only. No meetings, Ask Anything, computer control,
  Gemini Spark, image generation, emoji IME, default swear-filter-as-feature.
- Copy UX and architecture from competitors. Do not paste GPLv3 source
  (VoiceInk, TypeWhisper, FluidVoice).
- Fail-closed paste, target lock, 3s undo, preview-first selected-text,
  protected facts: keep all of them. Never weaken them for speed.
- No telemetry. No global keylog. No user-audio weight fine-tune.
- Never send window title, PID, or raw URL to any LLM/ASR prompt.
- ScreenTextContext never crosses Tauri IPC as raw bubble/email text.
  HUD may show a count only ("看见 6 个词").
- Other people's on-screen sentences: this-utterance context only.
  Never style few-shots. Never History. Never exports.
- Default dictate path NEVER screenshots and NEVER calls Phase 3 capture.
- Phase 3 must not be callable from dictation::stop.
- Drafts (prefetch, cascade) stay in HUD/background. Target app is pasted
  ONCE per session. Do not paste-then-replace.
- Do not silently change the user's primary ASR provider.
- Do not dual-run Accurate ASR on every utterance. Gate only.
- Do not stream partials into the target field.
- Do not change default activation from tap to hold.
- Do not embed Python or FunASR-in-process.
- Do not reformat unrelated files. Do not change license.
- English UI strings need t() keys covered by i18n coverage tests.
- Cleanup prompt changes need fixtures in src-tauri/src/cleanup_corpus.rs.

Normative settings (do not rename):
- cleanup_intensity: "off" | "light" | "standard" | "heavy", default "heavy"
- accurate_asr_provider / accurate_asr_model / accurate_asr_base_url (optional)
- cascade_timeout_ms: default 5000
- cascade_proper_noun_threshold: default 3
- window_ocr_enabled: default false
- screen_action_hotkey: unset until user records it
- vision_provider / vision_model: unset until user configures

Resolved skip-LLM if cleanup_enabled is false OR cleanup_intensity is "off".
CleanupEffort::Command is selected-text only. Slider 重 = CleanupEffort::Heavy,
not Command.

================================================================
2. PHASE ORDER (do not skip ahead)
================================================================

A slice is done only when its automated tests exist and pass.

Wave 0 — Plans
  Write both implementation plans if missing. Stop.

Wave 1 — Spec 1 foundation + Spec 2 Phase 1 together
  1A. cleanup_intensity setting + UI 关/轻/中/重 + mapping override + HUD
      resolved label. Default Heavy including unmapped WeChat.
  1B. CleanupEffort::Heavy prompts + PersonalChat/WorkChat Heavy few-shots
      that stay chat-shaped (must include 好的哈哈我晚点回你; must not add 您好).
      visible_context instruction: spell/address only, do not answer the screen.
  1C. screen_text.rs Phase 1 extractors with fixtures. 350ms fail-open.
      Caps: 40 tokens, 2000 chars. Family table from screen spec.
      Terminal/Form/Secure/banking → empty.
  1D. Wire ScreenTextContext into ASR prompt (tokens first) and cleanup.
      Privacy tests: no window_title / raw URL / PID in assembled prompts.
  1E. Style + intensity learning from OWN post-paste edits only.
      3× short rewrite → mapping style pair, max 3 pairs (oldest drops).
      Paragraph / topic change / added facts: ignore.
      3× casualize Heavy → drop mapping intensity one step + HUD Undo.
      Screen names → lexicon 2× if classified person name else 3×.
      Other-person bubble fixture must not become a style pair.

Wave 2 — Spec 1 cascade
  2A. Cascade gate (low confidence / hallucination bag / CJK-English mix /
      tokens.len() >= threshold). No Accurate configured → no second HTTP.
  2B. Accurate ASR client + 5s timeout → primary wins. Cleanup ONCE on winner.
      Primary HTTP fail + Accurate configured → Accurate is the only shot.
      Both fail → existing recovery, no empty paste.
  2C. HUD: primary draft + 「精确重打中」. Paste still once.

Wave 3 — Spec 2 Phase 2
  3A. window_ocr_enabled + Screen Recording preflight + Info.plist usage string.
  3B. One-shot locked window_id capture, long edge ≤1280, Vision OCR, drop image.
      Thin gate: Phase 1 usable text ≥20 chars → skip OCR.
      Never full-display. Never upload image. Never write image to disk.

Wave 4 — Spec 2 Phase 3
  4A. In the SAME change set, rewrite asr-cleanup-later-and-wont.md:
      screenshot→LLM is allowed only on the look-at-screen hotkey + preview.
      Meetings/Spark/image-gen stay never.
  4B. screen_action_hotkey, vision provider, memory PNG, preview dialog
      (Replace / Copy only / Cancel). Stale target → copy-only.
  4C. Counter test: dictation stop does not call capture.

If a later wave's prerequisite types/APIs are missing, implement the
prerequisite in this tick; do not stub a fake screenshot on the dictate path.

================================================================
3. HOW TO PICK THIS TICK'S SLICE
================================================================

1. If plans are missing → Wave 0 only.
2. Else open the two plan files. Take the first unchecked task whose
   dependencies are checked. One task (or one tightly coupled pair like
   "failing test + impl + pass") per tick.
3. Prefer finishing a Wave 1 item before starting Wave 2, etc.
4. If the working tree is dirty with an unfinished slice, finish or
   fix THAT slice. Do not start a second feature on a dirty tree.
5. If tests are red from a previous tick, this tick is repair-only.

================================================================
4. HOW TO IMPLEMENT THE SLICE
================================================================

- TDD: write the failing test first (Rust lib test and/or frontend test).
  Run it. See it fail for the right reason. Then implement the minimum.
- Follow existing patterns in store.rs, llm.rs, lexicon.rs, dictionary_learn.rs,
  context.rs, paste.rs, ContextSettings.tsx, i18n.tsx.
- New screen extractors go in a new src-tauri/src/screen_text.rs (or the name
  in the spec). Do not grow context.rs by another thousand lines if you can
  keep the public surface small.
- CI must not require a live microphone, live AX tree, or real Screen Recording.
  Use fixtures.
- After code changes, run the smallest relevant tests, then:

  cargo test --manifest-path src-tauri/Cargo.toml --lib
  npm test -- --run
  npm run lint

  If you touched Rust formatting/clippy surface:

  cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
  cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings

- Do not claim "industry leading" or "accuracy improved" from unit tests.
  Gate 2 (WeChat / Gmail / Slack / Cursor / 晓雯) is owner-manual only.
- Do not commit unless the human explicitly asked this tick to commit.
- Do not push.
- Do not start a Tauri app or browser QA unless the slice is UI-only and
  you can verify without breaking the tick timebox.

================================================================
5. TICK REPORT (required, short)
================================================================

End the tick with exactly this structure:

### Loop tick
- Wave / plan task: <id or name>
- Done: <one sentence>
- Tests: <commands + pass/fail>
- Spec deltas: <none | what you discovered that contradicts the spec>
- Next tick: <exact next task id>
- Status: CONTINUE | BLOCKED <reason> | COMPLETE

Status COMPLETE only if the completion gate below is all true.

If BLOCKED (missing Accurate-ASR product decision, cannot get AX fixture
for WeChat, etc.), say what a human must decide. Do not invent a new
product rule.

================================================================
6. COMPLETION GATE (all must be true)
================================================================

Spec 1:
- [ ] cleanup_intensity exists, default heavy, UI 关/轻/中/重
- [ ] unmapped WeChat resolves Heavy; mapping Light wins; HUD shows one label
- [ ] Heavy few-shots stay chat-shaped; protected-facts / no 您好 tests pass
- [ ] intensity off skips cleanup HTTP
- [ ] cascade gate unit tests for 4 triggers; unset Accurate → no second call
- [ ] 5s timeout keeps primary; cleanup invoked once
- [ ] style learn 3× / max 3 pairs / paragraph ignored / foreign bubble ignored
- [ ] intensity 3× revert + Undo
- [ ] single-paste invariant still holds (no insert-then-replace helper)

Spec 2:
- [ ] Phase 1 fixtures per family + empty secure/terminal/banking
- [ ] prompt assembly privacy tests (no title/URL/PID)
- [ ] extractor error does not fail dictation
- [ ] Phase 2 thin-gate + no disk write + no full-display
- [ ] Phase 3 preview-only; dictate stop capture-count is 0
- [ ] later-and-wont.md red-line updated only with Phase 3
- [ ] privacy.md + end-to-end-workflows.md + context-e2e-checklist.md §18
      updated to match shipped phases

Process:
- [ ] cargo test --lib and npm test -- --run and npm run lint are green
- [ ] both implementation plans have all tasks checked or explicitly deferred

When COMPLETE, do not start extra features. Say the loop should be stopped.

END_TICK_PROMPT
```

---

## Optional: first-tick-only addendum

If you are the first tick in a fresh session and the human has not written
plans yet, you may replace step 6 of "READ FIRST" with: use
`superpowers:writing-plans` and produce both plan files, TDD-shaped, with
checkbox tasks that match Waves 1–4 above.

## Optional: review-only mode

If the human says `loop review` instead of implement, each tick:

1. Diff vs the two specs (not vs taste).
2. List P0 violations: paste-twice, screenshot on dictate, title/URL in LLM,
   style learned from others, silent engine swap, Heavy==email.
3. Fix P0 only. Leave nits.
4. Re-run the verify commands.

Do not mix review-only and implement-only in the same tick.
