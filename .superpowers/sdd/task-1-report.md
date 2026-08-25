# Task 1 report — Per-app tone

**Status:** DONE_WITH_CONCERNS

## What shipped

WeChat/PersonalChat, Slack/WorkChat, and Email now use distinct HUD style labels and writing prompts. Builtin `work_chat` / `personal_chat` prompts that still match the old shared chat string are migrated to the new family defaults; customized prompts stay. The HUD caption shows `contextLabel` while recording/starting/processing (and alongside the recording-limit caption). Mapping style-example placeholder is the WeChat help string from the brief.

## Product checks

| Requirement | Result |
|---|---|
| PersonalChat prompt ≠ WorkChat prompt (verbatim strings) | Pass |
| `display_label`: WeChat · 口语, Slack · 工作短讯, Email · 邮件 | Pass |
| `OLD_SHARED_CHAT_PROMPT` migrates builtin chat modes; custom prompts stay | Pass |
| PersonalChat policy: casual, preserve_fragments_when_intentional, extra forbidden_additions | Pass |
| WorkChat policy: neutral, concise, still forbids greetings | Pass |
| VoicePill shows `contextLabel` in recording / recording_limited / starting / processing | Pass |
| Error / degraded captions take priority over context label | Pass |
| No ASR transcript on HUD | Pass (slice 3 out of scope) |
| Settings help `贴一条你平时微信怎么打` / `Paste a typical WeChat message` | Pass |
| Corpus `zh_wechat_casual`, `zh_wechat_swear_kept`, `work_chat_not_email`; `personal_chat` required | Pass |
| `allow_rewrite: false` on personal_chat casual cases | Pass |
| No cleanup intensity enum, no later slices | Pass |
| English `t()` keys covered | Pass after adding two already-used ContextSettings keys |

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo` was not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib -- context:: cleanup_corpus::` | **29 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **188 passed, 1 failed, 1 ignored** |
| Task 1 frontend (VoicePill, voicePillTokens, ContextSettings, i18n.coverage) | **44 passed** |
| `npm test` (full) | **120 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (not Task 1)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not tone logic.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

`cargo test --lib` did not compile until four mechanical fixes in already-dirty files (missing `Emitter` import, `take_gesture_lock` moved into `dictation`, spool path/`set_history_key` type mismatches). Those were not product changes for this slice.

## Files changed

Task 1:

- `src-tauri/src/context.rs`
- `src-tauri/src/cleanup_corpus.rs`
- `src/components/Island/VoicePill.tsx`
- `src/components/Island/VoicePill.test.tsx`
- `src/components/Island/voicePillTokens.ts`
- `src/components/Island/voicePillTokens.test.ts`
- `src/components/ContextSettings.tsx`
- `src/components/ContextSettings.test.tsx`
- `src/lib/i18n.tsx`

Compile-unblocking only (pre-existing dirty tree):

- `src-tauri/src/lib.rs` (test imports for `take_gesture_lock` / `GESTURE_LOCK_MS`)
- `src-tauri/src/dictation.rs` (`use tauri::{Emitter, State}`)
- `src-tauri/src/history_commands.rs` (`Path::new` for spool read)
- `src-tauri/src/store.rs` (`set_history_key(Some(...))`, `encryption_key.as_ref().map(...)`)

Not committed.

## Concerns

1. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
2. `src-tauri/src/llm.rs` `scene_guidance("chat_message")` still uses the old shared chat sentence as a **fallback** when `writing_prompt` is empty. Production snapshots set `writing_prompt` from writing modes, so PersonalChat vs WorkChat still differ on the live path. `llm.rs` was out of this slice’s file list.
3. `profile_guidance` still groups `chat.personal` with Slack as “concise and natural.” Same fallback caveat.
4. Compile-unblocking edits were required to run the mandated `cargo test --lib`; they are outside the Task 1 file list.
5. npm install needed the public registry; `package-lock.json` `resolved` URLs pointed at `unpm.uberinternal.com` and 401’d. Lockfile was restored after install.
