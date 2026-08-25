# Task 8 report — Punctuation, gain, Fn hint, snippet placeholders

**Status:** DONE_WITH_CONCERNS

## What shipped

**8a.** `spoken_punctuation::apply` runs on the ASR transcript immediately after merge (short batch, long chunk merge, and history retry), before `parse_cleanup_intent`, snippets, and LLM cleanup. It still runs when cleanup is off. Mappings: 左括号/右括号 → （）, 顿号 → 、, 引号 → 「」 (opening then closing), 斜杠 → /, 逗号/句号/问号/感叹号 → ，。？！. Surrounding chat is left as-is; `括号里` is not rewritten.

**8b.** Settings `input_gain` defaults to 1.0 (no-op), clamps to 0.5–4.0 in `normalize`. Gain is multiplied onto resampled samples in `audio.rs` after `resampler.push` / `finish`, before the capture buffer and chunker. Threaded through `EngineCmd::Start` and `RecorderBackend::start`. RecordingSettings has a 0.5–4.0 number control.

**8c.** RecordingSettings shows static help when the hotkey is modifier-only Fn / Function / Globe: WeChat / 微信输入法 may steal Fn. No OS hook.

**8d.** `snippets::resolve_exact` replaces `{{date}}` with local `YYYY-MM-DD`. `{{clipboard}}` is filled from a **read-only** clipboard snapshot taken during processing, before `paste_text`. That does not write the clipboard, so paste restore is unchanged.

## Product checks

| Requirement | Result |
|---|---|
| Spoken punctuation after ASR merge, before cleanup/snippets/LLM | Pass |
| Standalone mappings + 括号里 unchanged + chat not formalized | Pass |
| `input_gain` default 1.0, clamp 0.5–4.0, applied after resample | Pass |
| RecordingSettings gain UI + multiply unit test | Pass |
| Fn hint for Fn/Function/Globe only; no OS hook | Pass |
| `{{date}}` local YYYY-MM-DD | Pass |
| `{{clipboard}}` read before paste, no restore break | Pass |
| i18n English coverage for new strings | Pass |
| No Scribe / screen assistant / sherpa-onnx; no commit | Pass |
| Optional `vad_sensitivity` scaling `Vad.fixed_floor` | Skipped (see concerns) |

## Self-review

- Punctuation only substitutes the listed tokens; casual chat without those words is unchanged.
- Gain 1.0 returns immediately and does not scale samples.
- No global key event tap. Fn hint is static copy.
- Clipboard is a read during snippet resolve, not a write, and happens before `paste.rs` snapshots/restores.

## TDD evidence

**RED (punctuation missing):**

```
not implemented: spoken punctuation
```

**RED (gain / settings / snippets missing):**

```
error[E0425]: cannot find function `apply_input_gain`
error[E0609]: no field `input_gain` on type `store::Settings`
error[E0425]: cannot find function `resolve_exact_with_clipboard`
```

**RED (RecordingSettings UI missing):**

```
Unable to find an element with the text: 微信 / 微信输入法可能会占用 Fn 键…
Unable to find an accessible element with the role "spinbutton" and name "输入增益"
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib spoken_punctuation::
4 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib input_gain
2 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib snippets::
4 passed; 0 failed

npm test -- src/components/settings/RecordingSettings.test.tsx src/lib/hotkeyFormat.test.ts src/lib/i18n.coverage.test.ts
10 passed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib spoken_punctuation::
cargo test --manifest-path src-tauri/Cargo.toml --lib input_gain
cargo test --manifest-path src-tauri/Cargo.toml --lib snippets::
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

| Command | Result |
|---|---|
| `spoken_punctuation::` | **4 passed** |
| `input_gain` | **2 passed** |
| `snippets::` | **4 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **260 passed, 1 failed, 1 ignored** |
| `npm test` (full) | **151 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–7)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not this task.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/spoken_punctuation.rs` (new)
- `src-tauri/src/lib.rs`
- `src-tauri/src/history_commands.rs`
- `src-tauri/src/audio.rs`
- `src-tauri/src/store.rs`
- `src-tauri/src/dictation.rs`
- `src-tauri/src/snippets.rs`
- `src/types/settings.ts`
- `src/components/settings/RecordingSettings.tsx`
- `src/components/settings/RecordingSettings.test.tsx`
- `src/lib/hotkeyFormat.ts`
- `src/lib/hotkeyFormat.test.ts`
- `src/lib/i18n.tsx`
- `.superpowers/sdd/task-8-report.md` (this report)

Not committed.

## Concerns

1. Optional `vad_sensitivity` scaling of `Vad.fixed_floor` was skipped. Wiring a new setting through chunker/VAD plus UI is more than a small add-on; quiet-speech volume is handled by `input_gain` instead.
2. `{{clipboard}}` uses a processing-time read, not the later `paste.rs` restore snapshot (that snapshot does not exist yet at snippet resolve). The read does not write, so restore is unaffected. If the user copies something else during cleanup, the expansion can be stale relative to paste time.
3. Spoken punctuation matches the listed keyword strings left-to-right anywhere in the transcript (needed for unspaced Chinese ASR). It does not match `括号` inside `括号里`. A fused compound that contains `逗号` / `句号` as a substring would still be rewritten.
