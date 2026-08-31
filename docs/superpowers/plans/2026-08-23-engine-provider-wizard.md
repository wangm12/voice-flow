# Engine Provider Wizard Implementation Plan

> **Status:** Implemented and evolved. Provider catalog is larger than this plan (OpenAI, Deepgram, SiliconFlow, etc.). Defaults moved from gpt-oss to `llama-3.1-8b-instant`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Settings → 语音服务 becomes a three-step wizard that picks ASR and cleanup providers, then probes the real HTTP path before those values go live.

**Architecture:** Persist `asr_provider` / `cleanup_provider` plus `cleanup_base_url` and a keychain-backed `cleanup_api_key`. Groq sides use built-in URLs and allowlisted models. Custom sides use OpenAI-compatible `/audio/transcriptions` and `/chat/completions`. `probe_engine_draft` talks to the draft, never writes settings. The React page is summary-or-wizard; drafts stay in memory until probe succeeds.

**Tech Stack:** React 19 + Vitest, Tauri 2, Rust (`src-tauri` crate `voiceflow` / lib `tauri_app_lib`). Tests: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `npm test`, `npx tsc --noEmit`.

## Global Constraints

- Do not commit unless the user asks.
- Do not edit the Voice Harness plan file.
- WeChat / PersonalChat still uses Light LLM cleanup. Do not skip it.
- No live Groq in CI. No real microphone in the probe.
- Onboarding stays Groq-key-only.
- English `t()` keys must be in `src/lib/i18n.tsx`.
- Follow existing settings/keychain patterns. Prefer a small `engine.rs` over growing `lib.rs`.
- TDD: failing test first for each behavior.

## File map

- Create: `src-tauri/src/engine.rs` — provider enum, draft, probe, error kinds
- Create: `src/lib/engineWizard.ts` + `src/lib/engineWizard.test.ts` — connected/draft/step rules
- Modify: `src-tauri/src/asr.rs` — Groq ASR allowlist helper
- Modify: `src-tauri/src/llm.rs` — `resolve_chat_url`, endpoint-aware cleanup
- Modify: `src-tauri/src/store.rs` — schema 16, fields, normalize/validate, view, credentials
- Modify: `src-tauri/src/keychain.rs` — cleanup API key
- Modify: `src-tauri/src/lib.rs` — patch allowlist, probe command, persist keys, cleanup call sites
- Modify: `src-tauri/src/history_commands.rs` — cleanup credential + endpoint
- Modify: `src/components/settings/EngineSettings.tsx` + tests
- Modify: `src/types/settings.ts`, `src/lib/i18n.tsx`, `src/App.tsx`

---

### Task 1: Chat URL resolver + Groq ASR allowlist

**Files:** `src-tauri/src/asr.rs`, `src-tauri/src/llm.rs`

**Produces:** `llm::resolve_chat_url(&str) -> String`, `asr::is_groq_asr_model(&str) -> bool`, `asr::GROQ_ASR_MODELS`

- [x] TDD `resolve_chat_url` (empty → Groq, `/v1`, already `chat/completions`, trailing slash)
- [x] TDD Groq allowlist includes turbo/large/distil, excludes `whisper-1`

### Task 2: Settings schema 16 + credentials

**Files:** `src-tauri/src/store.rs`, `src-tauri/src/engine.rs`

**Produces:** `EngineProvider`, settings fields, `cleanup_credential()`, normalize/validate rules from the spec

- [x] Migration: non-empty `asr_base_url` → custom ASR
- [x] Groq provider clears URL and resets unknown models; custom keeps unknown models
- [x] Custom cleanup requires URL + key + non-empty model; no Groq key fallback

### Task 3: Runtime cleanup uses resolved endpoint + key

**Files:** `src-tauri/src/llm.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/history_commands.rs`

**Produces:** dictation, history, selected-text use `cleanup_endpoint` + `cleanup_request_model` + `cleanup_credential`

### Task 4: Keychain + settings patch for cleanup key

**Files:** `src-tauri/src/keychain.rs`, `src-tauri/src/store.rs`, `src-tauri/src/lib.rs`

**Produces:** `set/get/resolve_cleanup_api_key`, `remove_cleanup_api_key`, patch fields `asr_provider`, `cleanup_provider`, `cleanup_base_url`, `cleanup_api_key`

### Task 5: `probe_engine_draft`

**Files:** `src-tauri/src/engine.rs`, `src-tauri/src/lib.rs`

**Produces:** command that does not persist; maps address/key/model/path errors; ASR empty transcript is OK

### Task 6: Frontend wizard logic + Engine Settings UI

**Files:** `src/lib/engineWizard.ts`, `EngineSettings.tsx`, `i18n.tsx`, `settings.ts`, `App.tsx`

**Produces:** connected summary vs 3-step wizard; persist only after successful probe

---

Spec coverage: wizard flow, fields, probe, draft isolation, schema, runtime resolution, onboarding unchanged, CI tests, no vendor SDK.
