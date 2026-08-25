# Task 5 report — OpenAI-compatible ASR BYOK

**Status:** DONE_WITH_CONCERNS

## What shipped

Configurable OpenAI-compatible transcription endpoint. Empty `asr_base_url` still resolves to Groq (`https://api.groq.com/openai/v1/audio/transcriptions`). A dedicated Keychain account `asr_api_key` is optional and falls back to the Groq key. Engine Settings exposes ASR 兼容地址 plus an optional ASR key. Audio in `docs/privacy.md` is described as going to the configured ASR endpoint, not always Groq. Dictionary still goes through `build_asr_prompt`. No Python/sherpa-onnx runtime.

`GroqAsrProvider` is rebuilt from the current base URL at startup and whenever `asr_base_url` or the ASR key changes.

## Product checks

| Requirement | Result |
|---|---|
| `asr_base_url: String` empty = Groq `https://api.groq.com/openai/v1` | Pass |
| Full URL containing `audio/transcriptions` used as-is | Pass |
| Base ending in `/v1` appends `/audio/transcriptions` | Pass |
| Host-only appends `/v1/audio/transcriptions` | Pass |
| Keychain account `asr_api_key`; fall back to Groq key | Pass |
| ASR key never written to `settings.json` | Pass |
| Provider rebuilt/replaced when base URL or ASR key changes | Pass |
| UI: ASR 兼容地址 + optional ASR key | Pass |
| i18n coverage for new copy | Pass |
| Dictionary still via `build_asr_prompt` | Pass |
| `docs/privacy.md` audio destination | Pass |
| No Python / sherpa-onnx; no Task 6–8 | Pass |
| Slices 1–4 not reverted; no commit | Pass |

## Self-review

- Empty / whitespace `asr_base_url` still POSTs to Groq `/audio/transcriptions`.
- Personal dictionary is still a Whisper-style prompt via `build_asr_prompt`. No FunASR-Nano hotword JSON.
- No Python runtime and no bundled sherpa-onnx.
- Hybrid activation, HUD partials, tone split, and dictionary learning were not reverted.
- LLM cleanup still uses the Groq key; onboarding still requires it.

## TDD evidence

**RED (resolver missing):**

```
error[E0425]: cannot find function `resolve_transcription_url` in this scope
error[E0599]: no function or associated item named `from_base_url` found for struct `asr::GroqAsrProvider`
```

**RED (settings fields missing):**

```
error[E0609]: no field `asr_base_url` on type `store::Settings`
error[E0599]: no method named `asr_credential` found for struct `store::Settings`
```

**RED (EngineSettings UI missing):**

```
Unable to find an accessible element with the role "textbox" and name "ASR 兼容地址"
Unable to find an element with the label of: ASR API Key（可选）
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib asr::tests::
17 passed; 0 failed
store::tests::asr_base_url_defaults_empty_and_falls_back_to_groq_key
store::tests::save_settings_never_writes_plaintext_asr_key
npx vitest run src/components/settings/EngineSettings.test.tsx src/lib/i18n.coverage.test.ts
3 passed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib asr::tests::
cargo test --manifest-path src-tauri/Cargo.toml --lib
npx vitest run src/components/settings/EngineSettings.test.tsx src/lib/i18n.coverage.test.ts
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib asr::tests::` | **17 passed** |
| `store::tests::asr_base_url_defaults_empty_and_falls_back_to_groq_key` | **Pass** |
| `store::tests::save_settings_never_writes_plaintext_asr_key` | **Pass** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **229 passed, 1 failed, 1 ignored** |
| Task 5 frontend (EngineSettings + i18n coverage) | **3 passed** |
| `npm test` (full) | **148 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–4)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not ASR BYOK.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/asr.rs`
- `src-tauri/src/store.rs`
- `src-tauri/src/keychain.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/history_commands.rs`
- `src/types/settings.ts`
- `src/components/settings/EngineSettings.tsx`
- `src/components/settings/EngineSettings.test.tsx` (new)
- `src/App.tsx`
- `src/lib/i18n.tsx`
- `docs/privacy.md`

Not committed.

## Concerns

1. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
2. Compatible endpoints still receive the existing Groq Whisper multipart payload (`whisper-large-v3-turbo`, `verbose_json`, word/segment timestamps). Servers that reject those fields will fail until a later slice.
3. Onboarding still requires a Groq API key because LLM cleanup is unchanged. A local-only ASR setup cannot finish onboarding without Groq.
4. An empty `asr_api_key` patch keeps the previously stored ASR key (leave-blank-to-keep). Clearing it requires the dedicated Remove ASR key control.
