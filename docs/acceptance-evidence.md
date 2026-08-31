# VoiceFlow Acceptance Evidence

> **2026-08-11 快照。** 下面的测试数量和 gate 结果不要当现状。当前门禁以 `cargo test --manifest-path src-tauri/Cargo.toml` 和 `npm test -- --run` 为准。产品行为见 [end-to-end-workflows.md](end-to-end-workflows.md)。

更新时间：2026-08-11（当时的确定性审计）

## P0/P1 implementation delta

- Silent ASR prefetch is bounded, cancellation-aware, and never delivers partial text to the HUD, clipboard, or external apps. The internal latency output separates `prefetch_asr`, `final_asr`, `cleanup`, `paste`, and `stop_to_insert`.
- Selected-text actions are opt-in and use an independent combination hotkey. Selection capture restores the previous clipboard; the replacement path rechecks target identity and selected text, and falls back to copying the result when validation fails. Selected actions do not create ordinary History entries.
- Delivery policy is now explicit (`auto`, `paste_shortcut`, `clipboard_only`, `history_only`). Cmd+V returns a local best-effort focused-value verification result; an unverified shortcut is recorded as `paste_unverified` instead of ordinary `done`. Long recordings still default to input delivery.
- Selected-text actions are preview-first outside onboarding: the user can edit the generated result, replace the original selection only after confirmation, copy only, or cancel. Confirmation reactivates the original target before rechecking it.
- The end-to-end state machine is documented in `docs/end-to-end-workflows.md`.
- The cleanup corpus is stored in `src-tauri/src/cleanup_corpus.rs` and contains de-identified regression targets for filler removal, self-correction, mixed language, code/URL preservation, email, repetition, and chunk-boundary continuity.
- Current deterministic gates for this implementation: Rust `cargo test --manifest-path src-tauri/Cargo.toml` — 151 passed, 1 ignored; frontend `npm test -- --run` — 55 passed; `npm run lint`, `npm run build`, and strict Clippy — pass.

这份矩阵对应权威验收 brief。`Automated` 表示当前代码和本地测试已经证明；`Real macOS` 表示仍需要在用户授权的 macOS 外部状态中执行。

| Requirement | Automated evidence | Real macOS evidence | Status |
| --- | --- | --- | --- |
| Clean dependency/startup path | `npm ci --dry-run`; `cargo metadata --locked --no-deps`; frontend build | Clean macOS user-account launch and onboarding | Partial |
| Recording state/cancel/race safety | audio/state tests, generation checks, repeated gesture lock, queue cancellation | Real microphone start/stop/cancel and rapid repeated input | Partial |
| ASR success/failure | Local HTTP responder: success, empty, invalid JSON, network, 401, 429, 5xx; `auto` omitted | Chinese, English, mixed-language samples through Groq | Partial |
| AI Clean Up | Prompt/profile tests, streaming SSE, empty/invalid response, protected-token rejection, raw fallback | Accuracy across code/email/search/chat/long mixed-language corpus | Partial |
| Dynamic context | Native/browser signal and target-guard unit tests; unknown fallback; authorized same-browser preview refreshes every 5s so Tab changes are not permanently cached | Cursor, VS Code, Gmail, Chrome, Safari, Slack, unknown App switching | Partial |
| Safe insertion | Target guard, cancellation, changed-target, panic recovery, clipboard fallback tests | Actual focused input insertion, App/window/Tab switching, long text and special characters | Partial |
| Retry/history recovery | Spool, migration, failed/degraded history and clipboard-only retry tests; retry respects cleanup-disabled mode and preserves degraded status when cleanup fails | Real provider failure followed by UI retry | Partial |
| HUD smoothness | Headless probe: 172×48, 3 center slots/1 active, max frame 10.4ms over 1.2s, 0 frames >20ms, no filter/backdrop-filter/will-change, inactive states paused; macOS panel is warmed once and remains transparent/click-through while idle | Native NSPanel during authorized live recording | Partial |
| Permissions/error UX | Native microphone request/check code, clipboard fallback without Accessibility, frontend build/typecheck | Actual microphone, Accessibility, and browser permission prompts | Partial |
| Dependency security | Public npm audit: 0 vulnerabilities; `cargo audit` exits 0. It reports only non-macOS GTK3/`glib` maintenance and unsoundness warnings; macOS dependency trees no longer include the affected X11 `memmap2` path. | N/A | Pass with non-macOS warnings |

## Current local gate results

- `npm run build` — pass (latest audit run)
- `npm run lint` — pass (latest audit run)
- `npm test -- --run` — 55 passed (latest audit run)
- `npm audit --registry=https://registry.npmjs.org --audit-level=high` — 0 vulnerabilities
- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` — pass (latest audit run)
- `cargo test --manifest-path src-tauri/Cargo.toml` — 151 passed, 1 ignored (152 tests discovered; ignored test is the real OS Keychain round-trip)
- `cargo clippy --locked --all-targets --all-features -- -D warnings` — pass (latest audit run)
- `cargo check --target aarch64-apple-darwin` — pass (latest audit run)
- `cargo check --target x86_64-apple-darwin` — pass (latest audit run)
- `cargo audit` — pass with non-macOS GTK3/`glib` warnings; `enigo` is now 0.6.1 and the affected `memmap2` path is 0.9.11.
- `npm run tauri -- build --debug --bundles app` — debug `VoiceFlow.app` compiled. It is ad-hoc/linker-signed; `codesign --verify --deep --strict` and `spctl` fail because no sealed resources are present, and `xcrun stapler validate` reports no stapled ticket. This is expected non-release evidence, not Developer ID signing/notarization proof.

## Release blocker

The product cannot be marked prod-ready until one authorized real macOS run proves:

`microphone → ASR → cleanup → context → focused external-App insertion`

The run must also cover App/window/Tab changes, clipboard fallback, cancellation, retry, and at least Chinese, English, and mixed-language input. No microphone, Accessibility, browser Automation, or external-App paste action has been performed without explicit authorization.

## Latest deterministic changes

- History retry now keeps `degraded`/`degraded_reason` when AI cleanup fails, honors the cleanup-disabled setting, and clears the consumed spool reference.
- Idle context preview refreshes an authorized browser every five seconds even when the frontmost PID/bundle is unchanged, allowing active-tab changes to update the preview. Recording start still performs a fresh fail-closed target capture.
- Onboarding no longer blocks on optional Accessibility permission; without it, delivery is explicitly clipboard-only. The onboarding Try It flow recognizes `copied` completion.
- Device-failure cancellation waits long enough for the bounded capture/spool flush before immediate recovery scans the spool.
- Warmed macOS NSPanel reconciliation now bypasses native hide/show scheduling while the panel remains ordered in; if panel conversion fails, the fallback window still hides normally. History SQLite connections use a two-second busy timeout and persistence failures are logged for recovery diagnosis.
- Context preview skips the System Events window/focus probe until Accessibility is granted; native App identity can still be detected through NSWorkspace, while delivery remains clipboard-only without a verified target.
- Failed debounced settings patches remain queued and are replayable from the visible “重试保存” action instead of being silently discarded.
- macOS Keychain reads and writes use the current non-interactive Data Protection item; both paths explicitly skip authentication UI, and legacy login-Keychain services are not probed.
- The `keyring` dependency is now target-scoped to non-macOS; Apple Silicon and Intel macOS dependency trees contain no `keyring` backend, so macOS uses only the explicit `security-framework` path.
- ASR rate-limit headers are kept in the in-memory quota gate only; they are no longer emitted to application logs.
- Delivery keeps both fail-closed target checks immediately inside `paste_text`; the removed outer duplicate probe avoids an extra browser/System Events round trip before insertion.
- Browser target fingerprints now include adapter-provided active-tab index/title metadata in addition to the URL, hashed locally so same-URL tab switches cannot silently reuse the old target.
- Context settings now expose a non-persistent manual family override; it changes only the policy/profile and preserves the actual target guard.
- Hotkey-capture validation and app-data-path failures restore the previous binding before unsuspending global shortcuts.
- The installed and development binaries were rebuilt from the current source and contain only `com.voiceflow.desktop.credentials.v3`; the former legacy app was moved out of `/Applications` so it cannot be launched accidentally.
- Earlier gate history: frontend build/lint/tests, public npm audit, Rust fmt/tests/strict Clippy, both locked macOS architecture checks, and cargo audit all passed at an earlier checkpoint. The current totals are maintained in the local gate table above; cargo audit retains non-shipping Linux GTK3/`glib` warnings.
- API Key controls now expose a programmatic label, `autoComplete="off"`, and keyboard-visible focus on the show/hide control; fake `gsk_`-shaped test strings were removed so secret scanners do not report them.
- Clean Up protected-token validation now recognizes protocol-less domains such as `docs.example.com`, with regression coverage against domain mutation.
- Settings window configuration now enforces a 720×520 minimum; compact settings actions use a 44px hit area and visible keyboard focus.
- Paste delivery now rejects cancellation before the first clipboard write; the long-recording clipboard branch has the same processing-generation gate.
- HUD listener registration is cleaned up one registration at a time, including listeners that resolve after the WebView unmounts; App/onboarding listener cleanup also tolerates shutdown-time rejection.
- Streaming cleanup rejects `finish_reason: length` so partial LLM output falls back to the raw transcript instead of being reported as success.
- Provider cleanup receives only context family/policy guidance and transcript; the context profile identifier is kept local.
- Terminal HUD state cleanup: `copied` no longer reuses the processing/loading visual path; active controls and onboarding preview now use bundled Lucide line icons. Frontend build/lint/tests pass after this change.
- Earlier deterministic checkpoint: `npm ci --dry-run`, public `npm audit` (0 vulnerabilities), frontend build/lint/tests (4 passed), Rust fmt, 88 Rust tests passed / 1 ignored, strict Clippy, and locked `aarch64-apple-darwin` check. Superseded by the final rerun below.
- The Rust security workflow now runs `cargo audit` from `src-tauri`; the previous `--manifest-path` invocation was incompatible with cargo-audit 0.22.2 and has been corrected.
- Earlier deterministic continuation: native browser adapter resolution no longer has a runtime `expect`; quota updates retain or synthesize a reset when low remaining requests arrive without a reset header; processing clipboard writes re-check cancellation inside their blocking worker; cancelled short recovery files and long spool sessions are discarded before History persistence; macOS excludes the unused `keyring` backend and ASR no longer logs rate-limit headers; same-URL browser tabs now receive distinct local target fingerprints; temporary context overrides preserve target identity; failed hotkey updates restore the previous binding. Superseded test totals are retained only as historical change notes.
