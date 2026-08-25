# Task 4 brief — History dictionary learning

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Do not commit. No global keylog. Do not implement Task 6 paste observer. Run cargo test --lib, npm test, npm run lint. i18n coverage.

## Task

CJK-capable dictionary suggestions from History edits.

### Files

- Create `src-tauri/src/dictionary_learn.rs` + `mod dictionary_learn` in `lib.rs`
- History.tsx — replace Latin-only `localDictionaryCandidates`
- DictionarySettings.tsx — `dictionary_learn_enabled` default true
- store.rs + settings.ts

### Spec

```rust
pub fn single_token_candidates(before: &str, after: &str) -> Vec<String>
```

- Max 3 candidates
- CJK runs 2–8 chars; Latin `[A-Za-z][A-Za-z0-9._-]{1,}`
- Token in after not in before (Latin casefold, CJK exact)
- If >3 tokens changed OR length delta > 12 → empty
- No URLs or `***` tokens

Command: `suggest_dictionary_entries { before, after } -> Vec<String>`

Minimum: CJK candidates appear; user confirms to write `dictionary`. Keep confirm UI.

Tests: `知呼`→`知乎`; `配森`→`Python`; paragraph rewrite → `[]`

Write report to `.superpowers/sdd/task-4-report.md`.
