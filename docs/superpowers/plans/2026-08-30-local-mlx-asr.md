# Local MLX ASR + Cloud Cleanup Implementation Plan

> **Status:** Not shipped as a first-class preset. Engine Settings has 本机 FunASR and 阿里云百炼 Qwen3-ASR, not「本机 MLX-Audio」. Users can already point `custom` at a local `mlx_audio.server`. Do not treat this file as current product docs.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let VoiceFlow transcribe on a local `mlx-audio` OpenAI-compatible endpoint (SenseVoice / Whisper on Apple Silicon) while keeping AI cleanup on a cloud provider.

**Architecture:** Do not embed Python or spawn a sidecar. The user starts `mlx_audio.server` themselves. VoiceFlow already splits ASR and cleanup providers; this work adds a settings preset, loopback-friendly timeouts, and copy. The HTTP contract is the existing `POST …/v1/audio/transcriptions` + `verbose_json` path in `asr.rs`.

**Tech Stack:** Tauri 2 / Rust `reqwest` multipart ASR client, React Engine Settings, existing `custom` OpenAI-compat provider, external `mlx-audio` 0.5.0 FastAPI server.

## Feasibility (already verified on this machine)

| Question | Answer |
|---|---|
| Can VoiceFlow talk to a local MLX HTTP server? | **Yes.** `custom` / `local_whisper` already POST OpenAI-compat transcriptions. Loopback `http://` is allowed. Empty key is allowed on loopback. Cleanup is independently Groq / OpenAI / SiliconFlow / etc. |
| Does `mlx-audio` expose that API? | **Yes, in 0.5.0.** `POST /v1/audio/transcriptions` accepts `response_format=verbose_json` and returns `{ text, segments, … }`. VoiceFlow already requests `verbose_json`. No auth middleware. |
| Can we use `~/.lmstudio/models/mlx-community`? | **As weight files, yes. As an LM Studio server, no.** LM Studio is an LLM host; it does not implement `/v1/audio/transcriptions`. Point `mlx_audio.server`'s `model` at the folder or at `mlx-community/SenseVoiceSmall`. |
| Is the local SenseVoice usable? | **Yes.** `SenseVoiceSmall/` has `config.json`, `model.safetensors` (~936MB), BPE, `am.mvn`. |
| Is the local Whisper turbo usable? | **Probably not as-is.** `whisper-large-v3-turbo/` is only `config.json` + `weights.safetensors`. mlx-audio's Whisper loader wants a processor/tokenizer. Prefer `mlx-community/whisper-large-v3-turbo-asr-fp16` if English local Whisper is needed later. |
| Can we start the server today? | **Almost.** `mlx_audio.server` exists, but the venv is missing the `server` extra (`uvicorn`). Install `[stt,server]`, then bind `127.0.0.1:8000`. |

Do **not** nest Python FunASR/MLX inside the Tauri binary. Do **not** auto-launch LM Studio. Do **not** add a second ASR HTTP client.

## Approaches (locked)

1. **Recommended — custom preset + loopback timeout.** Add a “本机 MLX-Audio” button next to 本机 FunASR. Default `http://127.0.0.1:8000/v1` + `mlx-community/SenseVoiceSmall`. Cleanup stays whatever is already selected (Groq / OpenAI / …). Smallest change, matches the FunASR pattern.
2. **Rejected for v1 — new `local_mlx` provider id.** Duplicates `local_whisper` (port 9000) and expands `EngineProvider::ALL`, TypeScript `ProviderId`, keychain accounts. Revisit only if we want it in the ASR dropdown as a first-class row.
3. **Rejected — VoiceFlow spawns `mlx_audio.server`.** Violates the no-nested-Python constraint, fights LM Studio for unified memory, and makes crash/lifecycle our problem.

**Default live setup after this plan:** ASR = custom → MLX-Audio on `:8000` / SenseVoiceSmall. Cleanup = existing cloud (keep Groq `llama-3.1-8b-instant` or current OpenAI). Scene skip / spoken_revision unchanged.

## Global Constraints

- Fail-closed paste, no raw URL/title/PID to the LLM, no keylog.
- Do not embed Python or scrape vendor UIs.
- ASR and cleanup stay independently routed; this plan does not change cleanup defaults.
- Loopback HTTP may omit a key; non-loopback still requires HTTPS + key.
- Do not hardcode `/Users/mingjie/...` in product code. Model id is the Hugging Face repo or a user-typed local path.
- Do not treat GPT-OSS 120B as a quality upgrade.
- User did not ask for a git commit; skip commit steps unless they ask.
- Cargo is not on default PATH. Prefix: `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"`.

## File map

- Modify: `src/components/settings/EngineSettings.tsx` — MLX preset button
- Modify: `src/components/settings/EngineSettings.test.tsx` — click fills URL + model
- Modify: `src/lib/i18n.tsx` — preset + hint strings
- Modify: `src/lib/i18n.coverage.test.ts` is automatic if keys are added
- Modify: `src-tauri/src/asr.rs` — per-request 120s timeout on loopback transcriptions
- Modify: `src-tauri/src/asr.rs` tests — loopback timeout helper
- Modify: `src-tauri/src/providers.rs` — optional `infer` for port `8000` stays `custom` (no new enum)
- Create: nothing. No new Rust module, no sidecar.

## Manual server (author machine, not an app task)

Run this **before** live-testing the preset. VoiceFlow will not start the process.

```bash
cd /Users/mingjie/Documents/mlx_audio
source .venv/bin/activate
python -m pip install --upgrade 'mlx-audio[stt,server]'
python -m mlx_audio.server --host 127.0.0.1 --port 8000
```

Warm the model once so the first dictation is not the download/load:

```bash
curl -sS -X POST http://127.0.0.1:8000/v1/audio/transcriptions \
  -F "file=@/absolute/path/to/a-short.wav" \
  -F "model=mlx-community/SenseVoiceSmall" \
  -F "response_format=verbose_json"
```

To force the already-downloaded LM Studio folder (avoids a second HF download):

```text
model=/Users/mingjie/.lmstudio/models/mlx-community/SenseVoiceSmall
```

Do not point VoiceFlow at LM Studio's own port (usually 1234). That is chat completions, not transcriptions.

---

### Task 1: Settings preset for 本机 MLX-Audio

**Files:**
- Modify: `src/components/settings/EngineSettings.tsx:377-404`
- Modify: `src/components/settings/EngineSettings.test.tsx`
- Modify: `src/lib/i18n.tsx` (keys listed in the step)

**Interfaces:**
- Consumes: existing `draft.asrProvider`, `draft.customAsr`, `draft.customBaseUrl`, `draft.asrModel`
- Produces: clicking the button sets `asrProvider: "custom"`, `customAsr: true`, `customBaseUrl: "http://127.0.0.1:8000/v1"`, `asrModel: "mlx-community/SenseVoiceSmall"`
- Cleanup fields are not touched

- [ ] **Step 1: Write the failing frontend test**

Add to `src/components/settings/EngineSettings.test.tsx` (open the custom provider row the same way existing URL tests do; if the custom card is collapsed, click 添加 / 展开 first):

```tsx
  it("fills the local MLX-Audio ASR preset without changing cleanup", () => {
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "本机 MLX-Audio" }));
    expect(screen.getByLabelText("兼容地址")).toHaveValue("http://127.0.0.1:8000/v1");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("mlx-community/SenseVoiceSmall");
    expect(screen.getByLabelText("整理服务")).toHaveValue("openai");
  });
```

If the current page does not expose 兼容地址 until Custom is selected, click the existing “本机 FunASR” control first in other tests as a reference, then assert this new button.

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/settings/EngineSettings.test.tsx`
Expected: FAIL — no button named `本机 MLX-Audio`

- [ ] **Step 3: Add i18n keys**

In `src/lib/i18n.tsx` next to `"本机 FunASR"`:

```ts
  "本机 MLX-Audio": "Local MLX-Audio",
  "本机 mlx-audio 服务，默认 127.0.0.1:8000。模型可填 Hugging Face id，或本机目录（例如 LM Studio 下的 mlx-community/SenseVoiceSmall）。密钥可空。":
    "Local mlx-audio server, default 127.0.0.1:8000. Model can be a Hugging Face id or a local folder (for example mlx-community/SenseVoiceSmall under LM Studio). Key may be empty.",
```

Update the existing Chinese ASR hint string to mention the new preset:

```ts
  "Groq Whisper 英文更快，中文人名和专有名词较弱。中文推荐 SiliconFlow SenseVoice、兼容接口的 Qwen3-ASR，或本机 MLX-Audio SenseVoice。":
    "Groq Whisper is faster for English and weaker on Chinese names. For Chinese, prefer SiliconFlow SenseVoice, a compatible Qwen3-ASR endpoint, or local MLX-Audio SenseVoice.",
```

- [ ] **Step 4: Add the preset button**

In `EngineSettings.tsx`, immediately after the 本机 FunASR button:

```tsx
                        <button
                          type="button"
                          className="rounded-lg px-2 py-1 text-xs text-secondary hover:bg-elevated"
                          onClick={() => setDraft((current) => ({
                            ...current,
                            asrProvider: "custom",
                            customAsr: true,
                            customBaseUrl: "http://127.0.0.1:8000/v1",
                            asrModel: "mlx-community/SenseVoiceSmall",
                          }))}
                        >
                          {t("本机 MLX-Audio")}
                        </button>
```

Under the custom URL field (or under the preset row), add one hint line:

```tsx
                        <p className="w-full text-xs text-tertiary">
                          {t("本机 mlx-audio 服务，默认 127.0.0.1:8000。模型可填 Hugging Face id，或本机目录（例如 LM Studio 下的 mlx-community/SenseVoiceSmall）。密钥可空。")}
                        </p>
```

Do not change cleanup provider, model, or keys.

- [ ] **Step 5: Run frontend tests**

Run: `npx vitest run src/components/settings/EngineSettings.test.tsx src/lib/providers.test.ts src/lib/i18n.coverage.test.ts && npm run lint`
Expected: PASS

---

### Task 2: Loopback ASR timeout (first MLX load)

**Files:**
- Modify: `src-tauri/src/asr.rs` (`http_client`, `transcribe_at`)
- Test: `src-tauri/src/asr.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: transcription endpoint URL
- Produces: `transcription_timeout(endpoint) -> Duration` — loopback 120s, otherwise 30s. Applied on the request, not by replacing the shared client.

Why: `http_client()` is a process-wide 30s `OnceLock`. SenseVoice first load from disk into unified memory regularly exceeds 30s. Cloud Groq must stay at 30s.

- [ ] **Step 1: Write the failing Rust test**

```rust
    #[test]
    fn loopback_transcriptions_get_a_longer_timeout_than_cloud() {
        assert_eq!(
            transcription_timeout("http://127.0.0.1:8000/v1/audio/transcriptions"),
            Duration::from_secs(120)
        );
        assert_eq!(
            transcription_timeout("http://localhost:8000/v1/audio/transcriptions"),
            Duration::from_secs(120)
        );
        assert_eq!(
            transcription_timeout("https://api.groq.com/openai/v1/audio/transcriptions"),
            Duration::from_secs(30)
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo test --manifest-path src-tauri/Cargo.toml --lib --offline asr::tests::loopback_transcriptions -- --nocapture
```

Expected: FAIL — `transcription_timeout` not found

- [ ] **Step 3: Implement timeout helper and use it on the request**

Add next to `is_loopback_host`:

```rust
fn transcription_timeout(endpoint: &str) -> Duration {
    match host_from_url(endpoint).as_deref() {
        Some(host) if is_loopback_host(host) => Duration::from_secs(120),
        _ => Duration::from_secs(30),
    }
}
```

In `transcribe_at`, after building `client.post(endpoint)`:

```rust
    let response = client
        .post(endpoint)
        .timeout(transcription_timeout(endpoint))
        .bearer_auth(key)
        .multipart(form)
        .send()
```

Keep `probe_transcription` on the same path so Engine “测试当前配置” can wait out first load.

- [ ] **Step 4: Run ASR tests**

Run:

```bash
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo test --manifest-path src-tauri/Cargo.toml --lib --offline asr:: -- --test-threads=4
```

Expected: PASS (including existing mock HTTP tests)

---

### Task 3: Empty Authorization on loopback (only if probe 401s)

**Files:**
- Modify: `src-tauri/src/asr.rs` `transcribe_at` Authorization header
- Test: mock server that 401s on `Authorization: Bearer ` and 200s with no header

mlx-audio 0.5.0 has no auth. Only do this task if Task 2's live probe returns 401 because reqwest sends `Authorization: Bearer ` for an empty key.

- [ ] **Step 1: Write a mock test that omits Authorization when the key is empty**

Reuse `crate::test_http::spawn_response_with_request_capture`. Assert the captured request has no `Authorization` header when `key` is `""`.

- [ ] **Step 2: Run it (expect fail if we still always `bearer_auth`)**

- [ ] **Step 3: Minimal fix**

```rust
    let mut request = client
        .post(endpoint)
        .timeout(transcription_timeout(endpoint))
        .multipart(form);
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(...)?;
```

- [ ] **Step 4: Re-run `cargo test --lib --offline asr::`**

If live probe against mlx-audio already returns 200 with `Bearer `, skip this task entirely.

---

### Task 4: Engine copy + probe contract (no new command)

**Files:**
- Modify: `src/lib/i18n.tsx` only if Task 1 hint is still unclear
- No new Tauri command. `probe_engine_draft` already POSTs a tiny WAV with the draft model to `resolve_transcription_url`. Empty-text success is already OK (`probe_transcription` treats `EmptyResult` as success).

- [ ] **Step 1: Confirm probe mapping**

Read `src-tauri/src/engine.rs` probe ASR stage. If the server is down, the user must see 地址连不上 (connect/timeout), not 密钥无效. Loopback + empty key must not fail validate (`store.rs` already allows this).

- [ ] **Step 2: Add a store/engine unit test if missing**

```rust
    #[test]
    fn loopback_custom_asr_does_not_require_a_key() {
        let settings = Settings {
            asr_provider: "custom".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_model: "mlx-community/SenseVoiceSmall".into(),
            asr_api_key: String::new(),
            ..Settings::default()
        };
        assert!(settings.validate().is_ok());
        assert_eq!(settings.asr_credential(), "");
    }
```

Adjust field names to match the current `Settings` struct (provider pool may store the custom key separately). If an equivalent test already exists (`loopback_without_key`), do not duplicate — just run it.

- [ ] **Step 3: Run**

```bash
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo test --manifest-path src-tauri/Cargo.toml --lib --offline store::tests::loopback engine:: -- --test-threads=4
```

Expected: PASS

---

### Task 5: Manual verification checklist (author)

Not automated. After Tasks 1–2 (and 3 if needed):

1. Start `mlx_audio.server` on `127.0.0.1:8000` with `[stt,server]` installed.
2. Settings → 语音服务 → 兼容接口 → 本机 MLX-Audio.
3. Cleanup remains the current cloud provider. Do not switch cleanup to custom/Ollama unless intended.
4. 测试当前配置: ASR 2xx. First call may take >30s while SenseVoice loads; after Task 2 it should succeed.
5. Dictate a Chinese sentence into Notes/Cursor. Raw text should come from local SenseVoice; cleanup (when not scene-skipped) should still hit Groq/OpenAI.
6. Dictate into WeChat: still LocalOnly / spoken. MLX does not bypass `spoken_revision`.
7. If HUD says 地址连不上: server is not running.
8. If HUD says 没有检测到语音 after the mic moved: likely silence trim or empty SenseVoice text — not an MLX routing bug.
9. Optional: set ASR model to the absolute SenseVoice folder to avoid HF. Do not use the incomplete `whisper-large-v3-turbo` folder until tokenizer files exist.

---

## Out of scope

- Streaming / WebSocket mlx-audio realtime. VoiceFlow is batch WAV today.
- Bundling or auto-updating `mlx-audio`.
- New `local_mlx` provider enum.
- Using LM Studio as the ASR HTTP server.
- Changing Groq/OpenAI cleanup defaults.
- Fixing remaining spoken_revision comma-`不对` issues (separate review).

## Spec coverage

- Local MLX endpoint: Tasks 1–2 + manual server block.
- Use `~/.lmstudio/models/mlx-community`: documented as a `model=` path, not a server.
- Cloud cleanup: Task 1 explicitly leaves cleanup untouched.
- No Python-in-app: Global Constraints + rejected approach 3.

## Residual risk

- First SenseVoice load can still exceed 120s on a cold disk + busy unified memory (LM Studio + mlx-vlm + mlx-audio). Warm the model; quit extra MLX UIs.
- mlx-audio `verbose_json` may omit Whisper-style `no_speech_prob`. `sanitize_transcript` already accepts empty segments and uses `text`.
- Official Whisper turbo id differs from the LM Studio folder name. English local Whisper is a follow-up, not this plan.
