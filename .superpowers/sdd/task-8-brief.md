# Task 8 brief — Punctuation, gain, Fn hint, snippet placeholders

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Do not commit. Do not formalize chat as a side effect. i18n coverage. Run tests. No Scribe / screen assistant / sherpa-onnx.

## Task (four small parts)

**8a** Create `src-tauri/src/spoken_punctuation.rs`, apply to ASR text before cleanup:
- 左括号/右括号 → （）
- 顿号 → 、
- 引号 → 「」
- 斜杠 → /
- 逗号/句号/问号/感叹号 if easy
- Standalone tokens only. Tests required.

**8b** `input_gain: f32` default 1.0, clamp 0.5–4.0, apply in audio capture before chunker. Optional vad_sensitivity scaling Vad.fixed_floor. RecordingSettings UI. Gain multiply test.

**8c** RecordingSettings: if hotkey is modifier-only fn/globe, static help that WeChat / 微信输入法 may steal Fn. No OS hook.

**8d** `snippets::resolve_exact`: replace `{{date}}` with local YYYY-MM-DD. `{{clipboard}}` only if you can use an existing clipboard snapshot without breaking paste restore; else date-only + concern.

Write report to `.superpowers/sdd/task-8-report.md`.
