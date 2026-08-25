# Task 1 fix report — LLM chat fallback still shared

**Status:** DONE

## What shipped

`scene_guidance` no longer uses the old shared chat sentence when `writing_prompt` is empty. Casual `chat_message` returns `default_writing_prompt(PersonalChat)` (verbatim 您好/哈哈 PersonalChat string). Other `chat_message` policies return `default_writing_prompt(WorkChat)`.

`profile_guidance` splits personal vs work: `chat.personal` / `chat.personal.window` keep casual voice and do not sanitize swears; Slack/Teams/native work chat ids stay short workplace chat, not email.

## Tests

```
cargo test --manifest-path src-tauri/Cargo.toml --lib -- llm:: context:: cleanup_corpus::
```

**47 passed**, 0 failed (via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo` is not on default PATH).

New tests (failed on the shared fallback first, then passed):

- `llm::tests::empty_writing_prompt_casual_chat_uses_personal_chat_guidance`
- `llm::tests::empty_writing_prompt_work_chat_differs_from_personal`
- `llm::tests::personal_profile_guidance_differs_from_slack`

Did not run full `cargo test --lib` / `npm test`. Did not touch spool umask or SelectedPreviewDialog.

## Files changed

- `src-tauri/src/llm.rs`
- `.superpowers/sdd/task-1-fix-report.md`

Not committed.

## Concerns

1. `chat.focused` remains WorkChat in the resolver, so its profile overlay is workplace chat. That matches current family mapping.
2. `chat.team` (native Slack/Teams process id) now gets workplace guidance. It was previously unmatched and fell through to “No additional application-specific rewrite.”
3. Live snapshots still send a writing-mode `writing_prompt` when set; this fix only covers the empty-prompt fallback plus selected-text profile overlay.
