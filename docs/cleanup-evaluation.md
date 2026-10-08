# AI cleanup evaluation protocol

This page describes the evaluation protocol for deterministic local production-path behavior. Fixture references are review targets; they are not provider output, human ratings, runtime/E2E acceptance, or ASR accuracy claims. Independent Astra runtime/E2E acceptance remains pending; this documentation pass did not run the evaluation or make provider requests.

## De-identified corpus

The 160 text fixtures live in [`src-tauri/evals/cleanup-cases.json`](../src-tauri/evals/cleanup-cases.json). Each row has a stable `id`, reporting `group`, `input`, `expected`, optional `reference_variants` and `protected` values, and the production routing fields `cleanup`, `family`, `focus_kind`, and `mode`. The twelve groups cover Chinese, English, mixed language, code, commands, entities, negation, identity, layout, cleanup failure, Auto scenes, and authorized snippets. Inputs include multiline code and form values, empty/silence-like results, filler-only speech, repeated referents, and metadata-looking literal content.

The offline runner is `cleanup_corpus::grouped_production_eval::grouped_160_case_runner_exercises_production_cleanup_fallback_and_guard`. For every case it uses the same `prepare_cleanup_transcript_for_scene` preparation as dictation, then exercises scene mapping and cleanup routing, local failure/Off behavior, and final text guard. Preparation provenance validates the spoken-layout decision and source-range authorized corrections; the final guard checks protected content against that prepared source. This lets the runner recognize layout syntax that production explicitly consumed without exempting those words globally or weakening the 160-case preservation assertions. Identity cases must remain exact. Authorized local correction, layout, and snippet expansion are scored separately from ordinary raw-source invariants.

Run the deterministic suite with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib grouped_160_case_runner_exercises_production_cleanup_fallback_and_guard
```

Set `VOICEFLOW_CLEANUP_EVAL_REPORT` to write the aggregate report outside the repository. The test itself does not need provider credentials or network access.

## Metrics and interpretation

- Exact reference match accepts `expected` or one of the explicitly declared `reference_variants`. CER and WER use the primary `expected` string. CER lowercases Unicode alphanumeric code points and ignores punctuation and spacing; WER groups contiguous Unicode alphanumeric runs, treating each CJK character as an individual token. These are code-point/token proxies, not grapheme-aware language metrics.
- Declared protected **cases** count rows with at least one `protected` value. Protected **spans** count each listed value separately. The live candidate/final text proxy is a case-insensitive substring-presence check, so it does not enforce token boundaries or prove factual equivalence. The separately reported production final-guard result checks typed source spans, local correction authorization consistency, explicit layout content, and attached demonstrative references during Cleanup (for example, `这个项目` and `那个项目` must retain their reference markers in order). It leaves standalone fillers such as `那个，麻烦你…` and English complementizer phrases such as `I think that it is ready` editable. These checks are narrow lexical / structural heuristics, not semantic verification; Rewrite, Shorten, Formalize, Casualize, and Translate do not lock the Cleanup-only reference markers.
- Mixed-script checks require CJK and Latin characters to remain in a mixed input. Negation coverage is evaluated through the production protected-span/final-guard path and source-preservation checks; equal marker counts do not prove semantic equivalence.
- Identity rewrites must be zero. Snippets and explicitly authorized local corrections/layouts are reported separately because their transformations are intentional.
- Offline latency is local Rust preparation, routing, fallback, and finalization only. It excludes ASR/LLM service time, audio capture, audio encoding, delivery, and operating-system scheduling. Do not compare it with provider latency.

The offline score measures whether deterministic VoiceFlow behavior matches these fixtures. It does not measure hearing, model cleanup quality, user preference, or broad factual correctness.

## Reviewed audio pipeline entry point

`npm run eval:audio -- --help` provides an explicit WAV → ASR → cleanup runner. It never records the microphone or reads History. Manifests, WAV files, reports and checkpoints must be outside the repository, including resolved symlink targets. `--init` creates a template with `reviewed: false`; fill in human-checked references and source/rights/de-identification notes before changing that flag. The template contains no recordings or quality evidence.

```bash
# Create the directories outside the repository first.
npm run eval:audio -- --init /absolute/fixtures/manifest.json
# Local validation: no credentials or network requests.
npm run eval:audio -- --manifest /absolute/fixtures/manifest.json --validate
# Explicitly upload the listed reviewed audio to selected models.
# Supply VOICEFLOW_EVAL_GROQ_KEY_FILE (or VOICEFLOW_EVAL_GROQ_API_KEY) separately.
npm run eval:audio -- --manifest /absolute/fixtures/manifest.json \
  --asr groq_whisper_large_v3_turbo --cleanup groq_gpt_oss_20b \
  --out /absolute/results/audio.json --gap-ms 250 --run
```

The manifest uses `schema_version: 1`, `provenance: {kind, note, reviewed}` and a `cases` array. Provenance `kind` is `human_recorded` or `synthetic`. Each case supplies a unique `id`, adjacent `.wav` filename, verbatim `reference`, reviewed `expected_final`, `scenario`, context `family`, `focus_kind`, and `cleanup` intensity (`auto/off/light/standard/heavy`). Optional `protected` values and `reference_variants` apply to cleanup evaluation. WAV input must be 16 kHz mono 16-bit PCM, nonempty, at most ten minutes / 25 MiB; manifests contain 1–200 cases and are at most 1 MiB. The entire selection is validated before any provider call.

ASR candidate IDs are `groq_whisper_large_v3_turbo`, `groq_whisper_large_v3`, `openai_gpt_transcribe`, `qwen3_asr_flash_dashscope`; cleanup IDs use the existing four candidates below. Comma-separated IDs allow paired comparisons. Existing per-model environment overrides still apply, and reports identify the actual selected model. Every selected provider requires an explicit `VOICEFLOW_EVAL_<PROVIDER>_API_KEY` or `_KEY_FILE` (`GROQ`, `OPENAI`, `QWEN`); sidecar-derived credentials are rejected. ASR requests are sequential; `--gap-ms` (0–5000) paces cleanup requests with the existing bounded rate-limit retry behavior. An existing report is preserved by choosing an unused filename.

For each ASR model, **actual nonempty sanitized ASR output** enters the existing cleanup adapter/finalizer evaluator. ASR failures and empty results remain visible in `asr_models` and never count as cleanup successes. Reports separate raw/sanitized ASR CER/WER against the verbatim reference, model-candidate quality, finalized/fallback quality against `expected_final`, declared entity proxies, guard rejections, error categories and stage timings. `additive_stage_latency` sums the matched ASR request and cleanup total, including cleanup pacing/retry waits; it excludes time between batch stages, microphone capture, prefetch, encoding, paste and macOS target verification. This is adapter evaluation with fixture-supplied scene metadata, rather than native stop-to-insert measurement. The ASR evaluator's fixed vocabulary hints remain in use. Checkpoints and reports contain fixture/reference/transcript text, never credentials or copied audio; manage them according to the recordings' data requirements.

The additive calculation is `ASR latency_ms + cleanup total_ms + cleanup provider_wait_ms`, matched by case ID. Cleanup `total_ms` contains preparation, HTTP attempts and finalization; `provider_wait_ms` contains both pacing and retry backoff. `retry_wait_ms` is already part of that wait and is not added again. Missing ASR timings are excluded from the timing sample count. All accepted focus kinds, including `coding_prompt`, pass through live cleanup preparation in deterministic tests.

Normal `npm test` / `cargo test` validates the entry point and fixture routing without live calls. The live Rust test stays ignored unless explicitly selected by `--run`. A passing call or newly available runner is not broad accuracy, user-microphone or native App acceptance evidence. No live dataset or benchmark runs automatically.

## Optional paired live provider evaluation

`cleanup_corpus::live_paired_eval::live_paired_cleanup_and_asr_eval_uses_production_adapters` is ignored by default. It sends the de-identified cleanup fixture text and, only when an audio manifest is explicitly supplied, synthetic WAV files through the production adapters. It never uses microphone audio or History. Keys are resolved in memory from explicit environment/file inputs or existing provider secret sidecars; the report records only provider, model, sanitized host, and key-source category, never credential contents.

The live runner stratifies early cases across Chinese, mixed-language and provider-routed identity groups, then follows the remaining corpus order. Requests use a bounded inter-request gap. A `429` receives one bounded `Retry-After` retry; if still rate-limited, only that model candidate stops. An authorization failure blocks later candidates that share the provider credentials. Use `VOICEFLOW_LIVE_CLEANUP_CANDIDATE_IDS` to select comma-separated IDs (`groq_gpt_oss_120b`, `groq_gpt_oss_20b`, `openai_gpt_4o_mini`, `qwen_plus_dashscope`) and `VOICEFLOW_LIVE_CLEANUP_CANDIDATE_ORDER` to set their order. The existing `openai_gpt_4o_mini` candidate uses `VOICEFLOW_EVAL_OPENAI_MODEL` as its model override, so it can run GPT-6 Luna without a harness change; reports should identify the actual model as GPT-6 Luna because the candidate ID is legacy. Checkpoints contain only compatible accepted adapter candidates and request/corpus/code fingerprints. A resume accepts only matching fingerprints and re-runs the current finalizer; old `final_text` is never reused. Reports and checkpoints default to the system temporary directory. Configure paths outside the repository with `VOICEFLOW_LIVE_CLEANUP_REPORT`, `VOICEFLOW_LIVE_CLEANUP_CHECKPOINT`, and `VOICEFLOW_LIVE_CLEANUP_RESUME`; `VOICEFLOW_LIVE_MIN_REQUEST_GAP_MS` is capped at five seconds. `VOICEFLOW_LIVE_ASR_MANIFEST` accepts an explicitly selected manifest using the existing `{ cases: [{ id, audio, reference }] }` schema. It may contain synthetic audio or separately approved, rights-cleared human audio; the manifest itself does not record licensing or provenance, so the final report must identify audio provenance and coverage. Never use microphone recordings or History audio.

The live report separates accepted adapter candidates from finalized output, resumed samples from fresh calls, provider time from pacing/retry wait, and cleanup evaluation from ASR evaluation. Candidate CER/WER uses the lowest-error declared reference variant per successful candidate; finalization/fallback metrics cannot stand in for a provider candidate. With zero samples, timing percentiles are null. Entity-preservation checks are declared-value proxies, not semantic fact checks. A successful HTTP request only proves that a request completed; it does not prove that its transcript or cleanup is correct.

Run only after explicitly choosing fixture text and any synthetic or rights-cleared human audio, credentials, report path, rate limit, and whether ASR is enabled:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib live_paired_cleanup_and_asr_eval_uses_production_adapters -- --ignored --nocapture
```

Any report must state sample count, whether coverage is complete or partial, stop reason, provider/model, and audio provenance (synthetic or rights-cleared human). Missing keys, `401`/`403`, `429`, and unsupported model/endpoint behavior remain separate outcomes. Do not label a same-provider model pair a multi-provider comparison, or describe selected public human clips as user-microphone or broad ASR accuracy evidence.

## Model availability note

The default cleanup model is Groq `openai/gpt-oss-20b` because Groq lists the older Llama 3.1 8B and Llama 3.3 70B IDs as retired for free/Developer accounts from 2026-08-16. This is an availability correction, not a quality improvement claim. Saved model IDs remain intact for existing configurations. See [Groq model deprecations](https://console.groq.com/docs/deprecations). Account-specific availability must be verified through the selected provider; catalog visibility or successful adapter tests do not guarantee a given account can use a model.
