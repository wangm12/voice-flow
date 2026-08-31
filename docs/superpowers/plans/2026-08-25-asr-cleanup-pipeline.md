# ASR → Cleanup Pipeline Implementation Plan

> **Status:** Implemented in tree. Do not re-execute as if these tasks are still open. Current defaults and remaining later/wont items: [asr-cleanup-later-and-wont.md](../../asr-cleanup-later-and-wont.md).

> **For agentic workers:** Execute in this session. Do not wait for a second approval — the user asked to start all improvements.

**Goal:** Ship the research P0 quality path (silence, restatement, scene skip, fast instruct, local punctuation) plus in-app P1 (Chinese ASR recommendations). Do not embed Python, stream ASR, or fine-tune.

**Architecture:** Deterministic layers run on every transcript before routing. Chat/search/form/terminal skip the LLM unless the user spoke a command or set a mapping override. Groq cleanup defaults to a fast instruct model; reasoning_effort is only sent for gpt-oss.

**Tech Stack:** Rust (Tauri), existing Groq/OpenAI-compat clients, React settings.

## Global Constraints

- No global keylog, screenshots, or cloud training.
- Do not nest Python FunASR in the app.
- Preserve fail-closed paste and protected tokens.
- Same lexicon table feeds replace and ASR prompt; no second word list.
- Existing gpt-oss settings remain valid; new defaults use llama-3.1-8b-instant.
- Lists `1.` / `一是` already exist — do not regress them.
- User did not ask for a git commit.

## Tasks

1. `spoken_revision` — restatement FSM, hallucination bag, repeat collapse
2. `silence` + audio finalize — trim/compress before encode
3. ASR segment sanitize — use verbose_json scores
4. `decide_cleanup` scene skip + short text
5. Local terminal punctuation; wire prepare path
6. Fast instruct default + shorter prompt + few-shots
7. Chinese ASR labels/presets in wizard
