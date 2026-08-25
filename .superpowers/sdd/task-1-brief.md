# Task 1 brief — Per-app tone

Read this first. These are the requirements. Use the values verbatim.

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

- macOS system dictation only. No meetings, Ask Anything, emoji IME, swear-filter-as-a-feature.
- Copy UX, not GPL source.
- Fail-closed paste, target lock, 3s undo, preview-first, protected facts stay.
- Never send window title / PID / raw URL to the LLM. No telemetry. No global keylog.
- Chat families must not add 您好/Hello, expand into email prose, or sanitize swears.
- Cleanup prompt changes need corpus fixtures in `src-tauri/src/cleanup_corpus.rs`.
- Do not commit. Do not change license. Do not reformat unrelated files.
- Do not implement a later slice.
- English UI strings must have `t()` keys covered by `src/lib/i18n.coverage.test.ts`.

Verify:
```
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

## Task

Per-app tone: WeChat/PersonalChat vs Slack/WorkChat vs Email.

### Files

- Modify: `src-tauri/src/context.rs` (`default_writing_prompt`, `ContextPolicy::for_family`, `display_label`, `normalize_writing_modes`, tests around native_profiles_keep_the_active_app_label)
- Modify: `src-tauri/src/cleanup_corpus.rs`
- Modify: `src/components/Island/VoicePill.tsx` and `VoicePill.test.tsx`
- Modify: `src/components/Island/voicePillTokens.ts` if needed
- Modify: `src/components/ContextSettings.tsx`
- Modify: `src/lib/i18n.tsx`

### Interfaces

- Keep `pub fn default_writing_prompt(family: ContextFamily) -> &'static str`
- PersonalChat and WorkChat prompts MUST differ
- `display_label`: PersonalChat → `口语`; WorkChat → `工作短讯`; Email → `邮件`
- WeChat native fixture: `WeChat · 口语` (app_label is `WeChat`)
- Slack fixture: `Slack · 工作短讯`
- `const OLD_SHARED_CHAT_PROMPT: &str = "Keep the message natural, short, and conversational. Do not turn it into an email or add greetings/sign-offs.";`
- `normalize_writing_modes`: if saved builtin `work_chat` / `personal_chat` prompt equals OLD_SHARED_CHAT_PROMPT, use the new family default. Custom prompts stay.

PersonalChat prompt (verbatim):
```
Keep the user's casual chat voice. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Do not sanitize swearing, slang, or particles like 哈哈. Prefer light punctuation. Never turn the message into an email.
```

WorkChat prompt (verbatim):
```
Keep the message short and conversational for workplace chat. Do not add greetings, sign-offs, or email structure. Keep names and project terms exact. Do not add emoji unless spoken.
```

Policy: Split `WorkChat | PersonalChat`. PersonalChat: `formality = "casual"`, `sentence_completeness = "preserve_fragments_when_intentional"`, extra forbidden_additions: `"greetings not spoken"`, `"swear-word sanitization"`. WorkChat: `formality = "neutral"`, concise, still forbid email greetings.

HUD: VoicePill currently receives `contextLabel` but tests require text-free HUD while recording. Change: while state is `recording`, `recording_limited`, `starting`, or `processing`, render `contextLabel` in the caption when present. Do not show ASR transcript. Update tests that currently assert Chrome Canary / General are absent — they must now expect the label. Error/degraded captions take priority.

Settings: style-example help `贴一条你平时微信怎么打` / English `Paste a typical WeChat message`. No cleanup intensity enum.

Corpus: add
- `zh_wechat_casual`: raw `好的哈哈我晚点回你` expected contains `哈哈` and `晚点`, must not contain `您好` or `稍后回复`
- `zh_wechat_swear_kept`: mild swear kept, not replaced with `有待商榷`
- `work_chat_not_email`: must not look like an email greeting

`allow_rewrite: false` for personal_chat casual cases. `corpus_covers_the_p0_quality_shapes` must require `personal_chat`.

Out of scope: hybrid hotkey, FunASR, dictionary auto-learn, HUD transcripts.

Write your full report to `.superpowers/sdd/task-1-report.md`. Return only: status, files changed, test commands+results, concerns.
