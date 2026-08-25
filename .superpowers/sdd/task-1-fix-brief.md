# Task 1 fix brief — LLM chat fallback still shared

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Do not commit. Do not implement later slices. Do not “fix” unrelated suite failures (spool umask, SelectedPreviewDialog).

## Problem

Slice 1 split PersonalChat vs WorkChat prompts, but `src-tauri/src/llm.rs` still collapses them when `writing_prompt` is empty:

- `scene_guidance`: `"chat_message"` uses `OLD_SHARED_CHAT_PROMPT`
- `profile_guidance`: `chat.personal` is grouped with Slack as “concise and natural”

That reintroduces “微信写成邮件” on the fallback path.

## Fix

1. `scene_guidance`: if `artifact_kind == "chat_message"` and `formality == "casual"`, return the **verbatim** PersonalChat prompt from Task 1. Otherwise for `chat_message` return the **verbatim** WorkChat prompt. Import or duplicate those two strings — prefer calling `context::default_writing_prompt` if that avoids duplication without creating a cycle. If `llm` cannot import `context` cleanly, duplicate the two verbatim strings and add a comment pointing at `default_writing_prompt`.

PersonalChat (verbatim):
```
Keep the user's casual chat voice. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Do not sanitize swearing, slang, or particles like 哈哈. Prefer light punctuation. Never turn the message into an email.
```

WorkChat (verbatim):
```
Keep the message short and conversational for workplace chat. Do not add greetings, sign-offs, or email structure. Keep names and project terms exact. Do not add emoji unless spoken.
```

2. `profile_guidance`: split personal vs work:
   - `chat.personal` | `chat.personal.window` → keep the user's casual chat voice; do not add greetings or sanitize swears
   - Slack/Teams/native work chat ids → short workplace chat, not email

3. Tests in `llm.rs`: empty `writing_prompt` + casual chat_message uses PersonalChat string (contains 哈哈 or 您好 prohibition); work chat_message does not equal personal. Personal profile id guidance differs from Slack.

Run:
```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- llm:: context:: cleanup_corpus::
```

Write report to `.superpowers/sdd/task-1-fix-report.md`. Return status, tests, concerns only.
