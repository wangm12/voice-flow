# Task 6 brief — Post-paste short-window learning

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

Depends on Task 4 `single_token_candidates`.

## Global constraints

Do not commit. NEVER global keylog. Skip secure_input. Fail-closed target lock. Run tests.

## Task

After verified paste, watch only the same focused field for 3s and promote a single-token correction into the dictionary.

### Files

- paste.rs after verified paste
- dictionary_learn.rs `observe_after_paste`
- Tests with a fake value provider (no live AX in CI)

### Spec

If `dictionary_learn_enabled` and not secure_input:
- 3000ms window, ~400ms poll `focused_input_value`
- Stop if target/PID/window/focus changed
- `single_token_candidates(pasted, current)` — exactly one candidate → append dictionary (dedupe, cap 256)
- Many-token rewrite → ignore

Write report to `.superpowers/sdd/task-6-report.md`.
