# Task 7 report — CJK paste chain

**Status:** DONE_WITH_CONCERNS

## What shipped

Paste now tries a **safe in-process AX insert first**, then the existing Cmd+V path. Cmd+V switches a CJK input source to `com.apple.keylayout.ABC` (then US, then a non-CJK ASCII-capable source) and **always restores** the previous source, including when paste fails or panics.

AX insert is used only when the focused role is an editable text control, it is not a secure field, and either `AXSelectedText` is settable or `AXValue` can be spliced at a known UTF-16 selected range (or the field is empty). Wrong roles, secure fields, whole-field replace of existing text, and any path that would need a character-by-character typer are skipped; Cmd+V still runs. After either path, `input_value_verifies_delivery` and the 3s fingerprint undo are unchanged. If the IME switch cannot be performed, Cmd+V still uses physical ANSI keycode `0x09` and fail-closed clipboard behavior is unchanged.

TIS calls are `cfg(target_os = "macos")`. Tests use a stubbed switch/restore helper and the pure `should_switch_input_source` predicate — no live TIS or live AX in CI.

## Product checks

| Requirement | Result |
|---|---|
| AX insert first when safe (selected text or UTF-16 value splice) | Pass |
| Skip AX for secure / wrong role / unsettable / would replace all / typer | Pass |
| No character-by-character keystroke typer | Pass |
| Cmd+V still uses ANSI keycode `0x09` | Pass |
| CJK IME → `com.apple.keylayout.ABC` or US around Cmd+V | Pass |
| Restore previous input source even on paste failure/panic | Pass |
| `should_switch_input_source` unit-tested (CJK vs ABC/US) | Pass |
| Keep `input_value_verifies_delivery` + 3s undo fingerprint | Pass |
| Fail-closed: uncertainty → clipboard, never blind Cmd+Z | Pass |
| Non-macOS TIS/AX calls `cfg`'d | Pass |
| Fake/stub TIS; no live AX in CI | Pass |
| No Task 6 post-paste observer; no Fn/WeChat steal fix | Pass |
| Slices 1–5 not reverted; no commit | Pass |

## Self-review

- No global key event tap and no Task 6 post-paste dictionary observer.
- IME restore is RAII (`AbcLayoutGuard` / `run_with_latin_layout_if_cjk`); restore runs on failure and panic.
- If TIS cannot identify the current source or cannot select ABC/US, paste still uses the layout-independent Cmd+V keycode. Verification and 3s undo still apply. Failed paste still keeps the clipboard fallback and does not send Cmd+Z.
- Fn / WeChat stealing was not touched.

## TDD evidence

**RED (predicate and restore helper missing):**

```
error[E0432]: unresolved import `super::should_switch_input_source`
error[E0425]: cannot find function `run_with_latin_layout_if_cjk` in this scope
```

**RED (AX decision helpers missing):**

```
error[E0425]: cannot find function `ax_insert_decision` in this scope
error[E0433]: use of undeclared type `AxInsertDecision`
error[E0425]: cannot find function `utf16_splice` in this scope
```

**GREEN:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib input_source::
9 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests
16 passed; 0 failed
```

## Tests

Commands used (cargo via `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin` because `cargo`/`rustc` were not on PATH):

```
cargo test --manifest-path src-tauri/Cargo.toml --lib input_source::
cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm test
npm run lint
```

| Command | Result |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml --lib input_source::` | **9 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib paste::tests` | **16 passed** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **244 passed, 1 failed, 1 ignored** |
| `npm test` (full) | **148 passed, 1 failed** |
| `npm run lint` (`tsc --noEmit`) | **Pass** |

### Unrelated failures (same as Tasks 1–5)

1. `store::tests::spool_writes_are_atomic_and_path_bounded` — asserts temp dir mode `0o700` (448), observed `0o755` (493). Umask/environment, not the paste chain.
2. `SelectedPreviewDialog` — `getByRole("button", { name: "取消" })` matches both the X control and the footer Cancel button. File not touched.

## Files changed

- `src-tauri/src/input_source.rs` (new)
- `src-tauri/src/paste.rs`
- `src-tauri/src/lib.rs` (`mod input_source`)
- `.superpowers/sdd/task-7-report.md` (this report)

Not committed.

## Concerns

1. AX insert is skipped for web/Electron/`AXWebArea` and any control where selected text and a ranged `AXValue` splice are unsettable. Those sessions still use Cmd+V. That is the intended fail-closed path, but it means many CJK chat/browser fields never take the AX shortcut.
2. After a successful AX set, 3s undo still posts Cmd+Z gated by the focused-value fingerprint. Cocoa text views usually put `AXSelectedText` on the undo stack; some apps may not. The fingerprint still prevents undo after later user edits, and we never send Cmd+Z without that match.
3. `TISSelectInputSource` is process/session-wide. If macOS is set to a per-document input source, switching ABC inside VoiceFlow may not change the target app’s IME. Cmd+V still uses physical keycode `0x09`, so paste should still work; verification remains the proof.
4. Full `cargo test --lib` and `npm test` are not green because of the two unrelated failures above.
