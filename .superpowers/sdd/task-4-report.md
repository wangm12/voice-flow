# Task 4 report — History dictionary learning

**Status:** DONE_WITH_CONCERNS

## What shipped

CJK-capable dictionary suggestions from History edits. Rust owns `single_token_candidates(before, after) -> Vec<String>` in `dictionary_learn.rs`. After a History revision save, the UI calls `suggest_dictionary_entries` instead of Latin-only `localDictionaryCandidates`. Candidates still require confirm-to-add. Confirm writes `dictionary` through `mergeDictionary` (cap 256). Settings `dictionary_learn_enabled` defaults true; when false the command returns `[]` and History shows no chips.

## Product checks

| Requirement | Result |
|---|---|
| `src-tauri/src/dictionary_learn.rs` + `mod dictionary_learn` | Pass |
| `pub fn single_token_candidates(before, after) -> Vec<String>` | Pass |
| Max 3 candidates; >3 new tokens → `[]` | Pass |
| CJK runs 2–8; Latin `[A-Za-z][A-Za-z0-9._-]{1,}` | Pass |
| Token in after not in before (Latin casefold, CJK exact) | Pass |
| Length delta > 12 chars → `[]` (rewrite) | Pass |
| No URL or `***` suggestions | Pass |
| Command `suggest_dictionary_entries { before, after } -> Vec<String>` | Pass |
| History shows CJK candidates; confirm-to-add kept | Pass |
| Confirm respects dictionary cap 256 | Pass (`mergeDictionary`; backend still truncates) |
| `dictionary_learn_enabled` default true; hide when false | Pass (command returns `[]`) |
| Tests: 知呼→知乎; 配森→Python; paragraph rewrite → `[]` | Pass |
| i18n coverage | Pass |
| No global keylog; no FunASR hotwords; no Task 6 paste observer | Pass |
| Slices 1–3 not reverted; no commit | Pass |

## Self-review

- No global key event tap and no AX post-paste observer. Learning is History-edit only.
- Dictionary is still a Whisper-style prompt list via `build_asr_prompt`. Nothing FunASR-specific was added.
- Confirm never appends past 256: frontend `mergeDictionary(..., limit = 256)` skips the patch when `added === 0`; `Settings::normalize` still truncates.

## TDD evidence

**RED (algorithm stub returned empty):**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
cjk_correction_zhihu FAILED  left: []  right: ["知乎"]
latin_correction_python FAILED  left: []  right: ["Python"]
```

**RED (History still used Latin-only local candidates):**

```
npx vitest run src/components/History/History.test.tsx
expected "vi.fn()" to be called with "suggest_dictionary_entries"
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
8 passed; 0 failed
npx vitest run src/components/History/History.test.tsx src/components/DictionarySettings.test.tsx src/lib/i18n.coverage.test.ts
21 passed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::
cargo test --manifest-path src-tauri/Cargo.toml --lib
npx vitest run src/components/History/History.test.tsx src/components/DictionarySettings.test.tsx src/lib/i18n.coverage.test.ts
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib dictionary_learn::` | **8 passed** |
| `store::tests::dictionary_learn_enabled_defaults_true` | **Pass** (via `--lib`) |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **224 passed, 1 failed, 1 ignored** |
| Task 4 frontend (History + DictionarySettings + i18n coverage) | **21 passed** |
| `npm test` (full) | **146 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–3)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not dictionary learning.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/dictionary_learn.rs` (new)
- `src-tauri/src/lib.rs`
- `src-tauri/src/store.rs`
- `src/components/History/History.tsx`
- `src/components/History/History.test.tsx`
- `src/components/DictionarySettings.tsx`
- `src/components/DictionarySettings.test.tsx` (new)
- `src/types/settings.ts`
- `src/lib/i18n.tsx`

Not committed.

## Concerns

1. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
2. CJK tokens are maximal runs of 2–8 characters. A one-character fix inside a 9+ character unspaced sentence yields no candidate, because that run is skipped rather than split into n-grams.
3. URL filtering is conservative (`://`, `www.`, `/`). A bare host like `example.com` can still be suggested if the length/token caps allow it.
