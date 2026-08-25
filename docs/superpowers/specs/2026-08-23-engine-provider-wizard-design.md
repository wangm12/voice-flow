# Engine Provider Wizard

**Date:** 2026-08-23  
**Status:** Draft for user review  
**Product:** VoiceFlow (macOS dictation, React 19 + Tauri 2 + Rust)

## Problem

Engine Settings currently mixes one Groq key, an optional ASR URL, an ASR model dropdown (including OpenAI `whisper-1` on Groq), and a Groq-only cleanup model. Users can pair a Groq model name with a custom host, or a custom host with no key. Cleanup always posts to Groq `chat/completions`, so a non-Groq cleanup provider cannot work.

## Goal

Users pick an ASR provider and a cleanup provider (default Groq). Groq never shows a URL and only offers known models. Other providers require a URL, key, and typed model name. A wizard tests the real ASR → cleanup path before those values become live settings. Wrong host, key, or model name fails in the wizard with a specific reason.

## Non-goals

- No vendor SDK besides the existing OpenAI-compatible HTTP clients.
- No second-provider catalog (OpenAI is “Custom” with their URL and model).
- Onboarding stays Groq-only. The wizard lives in Settings → 语音服务.
- No live Groq calls in CI. No real microphone in the probe.

## User flow

### Unconnected vs connected

The Engine page is either the **wizard** or the **summary**.

**ASR ready:** Groq ASR + Groq key, or custom ASR + URL + ASR key.

**Cleanup ready:** Groq cleanup + Groq key, or custom cleanup + URL + cleanup key.

**Connected** (show summary): ASR is ready, and either cleanup is disabled or cleanup is ready.

A settings file created by today’s app (Groq key from onboarding, empty ASR URL, cleanup on) is connected. Existing custom ASR that already has URL + ASR key is ASR-ready without a new probe. Existing Groq cleanup with a Groq key is cleanup-ready without a new probe.

**Unconnected:** ASR is not ready, or cleanup is enabled and not ready. Open the wizard at step 1.

### Wizard

Three steps. Next is disabled until the current step is valid. Back is always allowed. Closing the window or leaving the page discards the draft. Live dictation keeps using the last committed settings.

1. **Providers.** ASR provider always. Cleanup provider only when `cleanup_enabled` is true. Values: `groq` | `custom`. Defaults: `groq`.
2. **Credentials and models.** Only fields required by step 1. Format checks only. No network. When cleanup is disabled, do not show or edit cleanup fields.
3. **Probe.** Real HTTP. Every stage the wizard collected must succeed. Only then persist those sides. When cleanup is disabled, persist ASR only; leave cleanup provider, URL, model, and key unchanged.

Reconfigure from the summary starts at step 1 with providers and non-secret fields prefilled from committed settings. Keys stay blank; placeholders show the existing hint. Changing provider on step 1 resets that side’s model and URL as specified below.

### Step 2 fields

**Both Groq (default):**

- One Groq API key field (writes `api_key`). Required unless a Groq key is already stored.
- ASR model `<select>`: `whisper-large-v3-turbo` (default), `whisper-large-v3`, `distil-whisper-large-v3-en`. No `whisper-1`. No URL field.
- Cleanup model `<select>`: `openai/gpt-oss-20b` (default), `openai/gpt-oss-120b`. No URL field.

**ASR custom:**

- ASR API key (required unless an ASR key is already stored and the host did not change).
- ASR base URL (required). Same rules as today: `http://` or `https://`; non-loopback must be `https://`. Empty is not allowed in this mode.
- ASR model: text input, required, max 256 chars. Empty placeholder, example text only (`whisper-1`). Do not prefill a Groq Whisper id.

**Cleanup custom:**

- Cleanup API key (required unless a cleanup key is already stored and the host did not change).
- Cleanup base URL (required). Same scheme rules as ASR. Resolved to `…/chat/completions` the same way ASR resolves `…/audio/transcriptions`.
- Cleanup model: text input, required, max 256 chars. Do not prefill `openai/gpt-oss-20b`.

**Switch Groq → custom (that side):** clear URL and model for that side. Do not copy Groq model ids into the text field.

**Switch custom → Groq (that side):** clear that side’s custom URL and typed model. Restore the Groq default model for that side. Do not send the custom key to Groq; Groq uses `api_key` only.

Custom sides never fall back to the Groq key.

If ASR is Groq and cleanup is Groq, there is a single Groq key field, not two copies.

### Summary (connected)

Two rows:

- ASR: provider label, model id, hostname only (not full URL), key status + hint.
- Cleanup: same.

Hostname for Groq is `api.groq.com`. Hostname for custom comes from the stored base URL.

**AI 文字整理** toggle stays on the summary. Off: dictation skips the LLM. On: reuse the last successfully saved cleanup config; do not force the wizard.

Groq model `<select>`s may appear on the summary for a Groq side. Changing a Groq allowlisted model writes immediately and does not re-probe.

Custom URL, custom model, and custom keys are read-only on the summary. Edit them through 重新配置.

重新配置 opens the wizard. 删除本机密钥 / 删除 ASR 密钥 / 删除整理密钥 clear that secret and, if the page is no longer connected, return to the wizard.

## Persistence and isolation

Wizard state is React memory plus the probe command arguments. It is not written to `settings.json` or the keychain until step 3 succeeds.

On success, one atomic persist of the sides the wizard collected:

- ASR provider, URL, model, and any typed ASR/Groq key
- Cleanup provider, URL, model, and any typed cleanup/Groq key, only when `cleanup_enabled` is true

Blank key fields keep the stored secret when the host did not change. If the user abandons the wizard, committed settings are unchanged. When cleanup is disabled, the persist payload must not include cleanup provider, URL, model, or cleanup key.

When the ASR host changes, drop the stored ASR key unless the draft includes a new one (same rule as `bind_asr_key_to_host` today). Same for cleanup host → `cleanup_api_key`.

## Settings schema

Bump `SETTINGS_SCHEMA_VERSION` to 16.

New persisted fields (Rust `Settings` + frontend `Settings` view):

| Field | Default | Meaning |
|---|---|---|
| `asr_provider` | `groq` | `groq` or `custom` |
| `cleanup_provider` | `groq` | `groq` or `custom` |
| `cleanup_base_url` | `""` | Empty means Groq chat endpoint |
| `cleanup_api_key` | `""` | Keychain-backed, like `asr_api_key`. Used only when cleanup is custom |

Migration from v15:

- Empty `asr_base_url` → `asr_provider = groq`
- Non-empty `asr_base_url` → `asr_provider = custom`
- `cleanup_provider = groq`, `cleanup_base_url = ""`, `cleanup_api_key = ""`

Normalize on load:

- `asr_provider == groq` forces `asr_base_url = ""`. If `asr_model` is not in the Groq ASR allowlist (including a leftover `whisper-1`), reset to `whisper-large-v3-turbo`.
- `cleanup_provider == groq` forces `cleanup_base_url = ""`. If `cleanup_model` is not in `llm::SUPPORTED_MODELS`, reset to `openai/gpt-oss-20b`.
- `asr_provider == custom` does **not** reset an unknown `asr_model`.
- `cleanup_provider == custom` does **not** run `is_supported_model` reset. Reject empty model on validate.
- Custom ASR still requires `asr_api_key`. Custom cleanup requires `cleanup_api_key`.
- Groq key fallback for ASR remains only when `asr_provider == groq` (equivalent to empty URL / `api.groq.com` today).

Frontend view exposes `cleanup_api_key_configured` and `cleanup_api_key_hint` the same way as ASR.

## Runtime resolution

Dictation and History retry use the same resolvers as the probe. No second URL builder.

**ASR**

- Groq: `https://api.groq.com/openai/v1/audio/transcriptions`, model from `asr_model`, credential `api_key` (or `asr_api_key` if set).
- Custom: `resolve_transcription_url(asr_base_url)`, model from `asr_model`, credential `asr_api_key` only.

**Cleanup** (`llm.rs`)

- Groq: current endpoint `https://api.groq.com/openai/v1/chat/completions`, model must be in `SUPPORTED_MODELS`, credential `api_key`.
- Custom: `resolve_chat_url(cleanup_base_url)` (mirror ASR: honor a path that already contains `chat/completions`; if the base ends with `/v1`, append `/chat/completions`; otherwise append `/v1/chat/completions`). Model is the stored string. Credential `cleanup_api_key` only. Do not call `normalized_model` (that remaps unknown ids to 20B).

`cleanup_enabled == false`: skip LLM as today. Probe step 3 tests ASR only.

## Probe

New Tauri command `probe_engine_draft` that accepts the draft (providers, URLs, models, optional raw keys). It must not persist. If a key field is empty, use the already-stored key for that slot when the draft host matches the stored host; otherwise fail with “missing key”.

Stages, in order:

1. **ASR.** `POST` a bundled tiny WAV (silence or a short spoken fixture in `src-tauri` resources, a few KB) to the resolved transcription URL with `model`. Success: HTTP 2xx and a JSON body the existing ASR parser accepts (empty text is OK).
2. **Cleanup** (skipped when `cleanup_enabled` is false). `POST` the fixed string `嗯 那个 你好` through the existing cleanup client (Light effort, no dictionary, no profile) to the resolved chat URL. Success: HTTP 2xx and a parseable completion.

The UI runs one command that returns `{ asr: Result, cleanup: Result | skipped }`. The first failing stage stops the persist path. The user stays on step 3.

Error mapping (user-visible, Chinese via `t()`):

| Signal | Copy intent |
|---|---|
| DNS / connect / TLS / timeout | 地址连不上 |
| HTTP 401 / 403 | 密钥无效 |
| HTTP 404, or body mentioning unknown/invalid model | 模型名不被这个接口接受 |
| HTTP 400 with URL/path hints | 地址路径不对 |
| Other 4xx/5xx | 服务返回错误 + truncated provider message |

Do not persist on any failure.

Onboarding `validate_api_key` stays Groq `/models` as today. It does not replace this probe.

## Onboarding

`EngineConfigStep` still collects one Groq key and validates it with `validate_api_key`. Defaults remain Turbo + 20B. After finish, Engine Settings opens on the summary. Changing provider happens only in the Settings wizard.

## UI copy (Engine)

Page title remains 语音服务. Wizard step titles: 选择服务 / 填写密钥和模型 / 测试整条链路. Summary description names the two models currently in use. Strings go through `t()` with English keys in `i18n.tsx`.

## Testing

CI: `cargo test --lib`, `npm test`, `tsc`. No live network.

**Frontend (`EngineSettings` + wizard tests)**

- Step 1 defaults to Groq / Groq; Next enabled.
- Groq/Groq step 2: no URL inputs; ASR select has three Groq Whisper ids only; cleanup select has 20B/120B.
- ASR custom step 2: URL + key + model text required; Next disabled while any is empty (unless a stored key covers the unchanged host).
- Cleanup custom: same three fields.
- Switching a side to Groq clears that side’s URL and typed model from the draft.
- Connected Groq-key user sees summary, not wizard.
- 重新配置 returns to step 1.
- Probe failure stays on step 3 and does not call `update_settings_patch` / key save.
- Probe success calls persist once with the draft providers, URLs, and models.

**Rust**

- `resolve_chat_url` cases matching `resolve_transcription_url` (empty → Groq, `/v1`, already `chat/completions`, trailing slash, loopback).
- `validate_asr_base_url` unchanged; add the same helper for cleanup URLs (shared or duplicated with one test table).
- Groq cleanup still rejects unknown models; custom cleanup accepts `gpt-4o-mini` and does not rewrite it to 20B.
- `asr_credential` / new `cleanup_credential`: custom host + empty dedicated key is empty (no Groq fallback).
- `probe_engine_draft` with a mock HTTP server: connect fail → address error; 401 → key error; 404 or model-not-found body → model error; 2xx ASR + 2xx cleanup → ok. Command does not write settings.
- Persist-after-probe is a UI contract; Rust validate still rejects custom ASR without ASR key and custom cleanup without cleanup key.

**Out of test scope**

- Real Groq. Real mic. Onboarding rewrite.

## Files likely touched

- `src/components/settings/EngineSettings.tsx` (+ tests)
- `src/types/settings.ts`, `src/lib/i18n.tsx`
- `src-tauri/src/store.rs` (schema 16, fields, validate, view)
- `src-tauri/src/asr.rs` (unchanged URL rules; Groq ASR allowlist helper)
- `src-tauri/src/llm.rs` (`resolve_chat_url`, stop hardcoding Groq for custom, skip `normalized_model` for custom)
- `src-tauri/src/lib.rs` (`probe_engine_draft`, settings patch, keychain for cleanup key)
- Keychain helper next to existing ASR/Groq key functions
- Small WAV fixture under `src-tauri`

Onboarding files stay Groq-key-only unless a string must mention Settings for custom providers.
