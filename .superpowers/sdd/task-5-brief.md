# Task 5 brief — OpenAI-compatible ASR BYOK

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Do not commit. Do not bundle Python or sherpa-onnx. Update privacy.md. Run tests. i18n.

## Task

Configurable OpenAI-compatible transcription endpoint (funasr-server / 豆包 / 千问).

### Files

- `src-tauri/src/asr.rs` (GroqAsrProvider already posts to `/audio/transcriptions`)
- store.rs, keychain.rs if second secret
- EngineSettings.tsx, settings.ts
- docs/privacy.md

### Spec

- `asr_base_url: String` empty = `https://api.groq.com/openai/v1`
- Resolve transcription URL:
  - if value contains `audio/transcriptions`, use as-is
  - else if ends with `/v1`, append `/audio/transcriptions`
  - else append `/v1/audio/transcriptions`
- Keychain account `asr_api_key`; fall back to Groq key
- UI: ASR 兼容地址 + optional ASR key
- Dictionary still via `build_asr_prompt`

Write report to `.superpowers/sdd/task-5-report.md`.
