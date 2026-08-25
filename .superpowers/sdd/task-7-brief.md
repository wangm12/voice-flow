# Task 7 brief — CJK paste chain

Work from: `/Users/mingjie/Documents/github/personal-projects/voice-flow`

## Global constraints

Do not commit. Keep value verification and 3s undo. Fail-closed: uncertainty → clipboard, never blind Cmd+Z. Do not implement Fn stealing fix.

## Task

AX insert if safe; else switch CJK IME to ABC around Cmd+V then restore.

### Files

- paste.rs
- optional `src-tauri/src/input_source.rs` (macOS TIS)

### Spec

1. If Accessibility can set focused AXValue safely, try that first. If not safe, skip and note DONE_WITH_CONCERNS — do not half-build a keystroke typer.
2. Cmd+V path: if input source is CJK (Hans/Hant/Kotoeri/Pinyin/IMK/SCIM/TCIM/Hiragana/Korean or real TIS ids), switch to `com.apple.keylayout.ABC` (or US), paste, restore.
3. Keep verification + undo.

Unit-test the “should switch input source” predicate. Non-macOS must still compile.

Write report to `.superpowers/sdd/task-7-report.md`.
