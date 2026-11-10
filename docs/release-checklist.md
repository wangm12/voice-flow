# VoiceFlow release checklist

This is a preflight checklist. GitHub Actions (`.github/workflows/release.yml`) first runs the reusable four-job CI workflow; only a passing run can build and publish the release DMG. macOS jobs use the supported `macos-15` runner, select the newest installed stable Xcode 26, and fail unless it provides Swift 6.2. Tag releases must match `package.json`; manual releases must run from `main`. Before certificate import, the workflow compares the release tag's resolved commit with the exact commit being built; annotated tags are checked through their peeled commit. A missing tag is created at the commit under test, while a mismatch stops the release. The asset and release notes reflect the signing identity and Team ID actually resolved: signed builds use `VoiceFlow.dmg`, explicit unsigned builds use `VoiceFlow-unsigned.dmg`, and duplicate asset uploads fail without replacing the existing file. The website opens the latest release page so the actual signed or unsigned asset name remains visible. The notes describe code signing only, not notarization. A build is not shippable until every unchecked item has an owner and evidence.

Tie each runtime and end-to-end result to the exact build and final artifact being reviewed; static gates, saved credentials, and model-file status do not substitute for runtime or provider evidence. Default local and self-signed builds are not Apple-notarized. Claim notarization only after the explicit Developer ID path below accepts, staples, and validates the exact final DMG and its contained app.

## Automated gates

- [ ] `scripts/select-xcode-26-test.sh` proves runner selection chooses a stable Xcode 26 and rejects a missing Xcode 26 or Swift 6.2 compiler.
- [ ] `scripts/release-test.sh` covers tag/version/commit preflight and mocked release create/upload paths, including annotated-tag peeling, no asset clobber, and the website's latest-release route; it does not call GitHub or publish artifacts.
- [ ] `npm ci`
- [ ] `npm test`
- [ ] `npm run lint`
- [ ] `npm run build`
- [ ] `npm audit --audit-level=high`
- [ ] Production-capability tests prove a stored `debug_mode` cannot expose or persist the debug page and that formal Rust resolution ignores an external sidecar override, requiring the canonical bundled resource.
- [ ] Default production build rejects reserved Test Groq import/status flags before Tauri startup or single-instance forwarding; the single-instance callback rejects already-forwarded reserved arguments before ordinary actions, replay, or focus. Import/status handlers and Keychain diagnostics are compiled only with `research-harness`.
- [ ] `cargo audit` in `src-tauri/`
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings`
- [ ] `cargo check --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin`
- [ ] Run Rust gates from a clean checkout with no prebuilt `target/mlx-sidecar/stage`; debug builds create only an empty resource directory. Apple Silicon release packaging still builds and validates the executable and Metal bundles through `mlx-sidecar/scripts/package.sh`.
- [ ] On Apple Silicon, `llama-runtime/scripts/package.sh` builds the pinned llama.cpp `v0.9.0` commit with embedded Metal, checks its arm64/macOS 13.3+ Mach-O, and writes a `BUILD-IDENTITY.json` matching the actual helper signature. The app must contain `Contents/Resources/llama-runtime/bin/llama-server` and its runtime license files; the helper's native strings must contain no developer-home path.
- [ ] The three fixed GGUF source-manifest entries, repository revisions, byte sizes, SHA-256 values, quantization and license notices match `ThirdPartyLicenses/Models/source-manifest.json`. GGUF remains limited to existing installed files and legacy saved selections; new GGUF downloads are not offered. No model weights are part of the app bundle; model compatibility and memory-budget admission do not certify performance or quality on a particular Mac.

## macOS signing

Default GitHub Releases use a stable self-signed `VoiceFlow` identity. That keeps Accessibility across updates on machines that install those DMGs. It is not notarized; first launch still needs Open Anyway or `xattr`. Signing identity or Team ID resolution and local certificate creation failures stop the build by default. Set `VOICEFLOW_ALLOW_UNSIGNED=1` only when an unsigned build is intended; it produces `VoiceFlow-unsigned.dmg`.

- [ ] Run `make signing-cert` once on the maintainer Mac.
- [ ] Add `MACOS_CERT_P12` and `MACOS_CERT_PASSWORD` from `.build/signing/` (read the password with `gh secret set MACOS_CERT_PASSWORD < .build/signing/password.txt`). Never commit the `.p12`.
- [ ] `make dmg` produces `.build/release/VoiceFlow.dmg` signed with that identity.
- [ ] The nested llama-server helper is signed before Tauri packages generic Resources. The explicit notarization path validates that helper's actual Developer ID Authority and Team ID, hardened-runtime flag, source commit, architecture and signed-file SHA against its packaged identity.
- [ ] Hardened runtime and `src-tauri/Entitlements.plist` are reviewed for the exact shipped binary.
- [ ] Manually inspect the exact production `VoiceFlow.app` main Mach-O: record its source identity, artifact SHA-256, and architecture; confirm it uses the default feature set and contains none of `voiceflow_research_stdio_v1`, `launcher_must_verify_owned_binary`, `request_id_reused`, or `owner_not_terminal_drain_and_retry`. A scan failure or missing artifact is an error; this static check does not replace packaged-App runtime acceptance.
- [ ] Test first launch on a clean macOS user account and confirm microphone, accessibility, and browser permission prompts.

## Explicit Developer ID notarization

- [ ] `VOICEFLOW_ALLOW_UNSIGNED=1` only opts into the distinct unsigned `VoiceFlow-unsigned.dmg` for `make dmg` when no signing identity is available; it never bypasses notarization's Developer ID identity, matching Team ID, Apple credentials, or strict App/DMG validation.
- [ ] A valid Developer ID Application identity, matching `APPLE_TEAM_ID`, `APPLE_ID`, and `APPLE_PASSWORD` are supplied through the environment, never committed.
- [ ] Run `make notarize`; it explicitly enables the wrapper's notarization path. When `APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD` are supplied together, the wrapper decodes the pair inside its private temporary directory and imports it into an owned temporary keychain before the Tauri build. That keychain remains available for app and manual DMG signing; cleanup restores the original user keychain search list and deletes the owned keychain. Import or cleanup failures stop publication.
- [ ] On a real macOS release host, confirm the shipped app has the Apple Events automation entitlement required for optional browser host/path access; this entitlement check remains a release verification item.
- [ ] `VOICEFLOW_RELEASE_TARGET` defaults to `aarch64-apple-darwin` (arm64); set it explicitly to `universal-apple-darwin` only when a universal artifact is intended. The wrapper requires the exact architecture set and exactly one current app in its target output; missing or ambiguous apps stop the release. Retaining the universal build option does not claim Intel acceptance.
- [ ] Platform acceptance remains pending until a real release host confirms the shipped baseline: macOS 13 for cloud and existing HTTP routes, Apple Silicon with macOS 14 or later for bundled MLX. Intel support is not accepted by the universal build option alone. Do not state a minimum-memory requirement or RSS guarantee without measured and reviewed evidence; no such figure is currently established.
- [ ] Before App ZIP submission, the wrapper performs ASR and cleanup JSONL hello requests against the bundled MLX sidecar with private empty model/audio roots. It checks the protocol response without loading a model or running inference; launch failures, malformed responses, nonzero exits, and timeouts stop publication. Before ZIP submission and again against the final mounted App, it requires both packaged SwiftPM locks and their declared SHA-256 values, the repository `THIRD_PARTY_NOTICES.md`, the complete `Models/` and `Upstream/` relative-path/content inventories, and `DependencyTexts/` to match their sources. These gates are packaging provenance and readiness checks; they do not prove model inference quality, runtime performance, model redistribution rights, or clean-Mac Gatekeeper acceptance.
- [ ] Before App ZIP submission and again on the final mounted App, the `make notarize` wrapper probes the production main executable with the reserved Test Groq import/status arguments, `--research-stdio`, and a nonexistent private owned-WAV path. Each probe must retain its documented production diagnostic and exit status; they use EOF stdin and must not access credentials, user audio, models, or inference. The wrapper inventories packaged Mach-O payloads including the main executable, sidecar, Swift runtime, and framework payloads, then scans strings for `/Users/` paths. Missing/unreadable binaries, `strings`/`file`/`find` errors, or `rg` errors fail closed; only `rg` status 1 is a clean no-match result.
- [ ] Confirm the App ZIP is submitted first and receives a nonempty submission ID with `Accepted`; the same App is stapled and validated before the wrapper builds a fresh DMG containing that App and an `/Applications` link.
- [ ] Confirm the newly created DMG is signed, independently submitted with its own nonempty ID and `Accepted` result, then stapled and validated. The mounted App must still validate its stapled ticket, signature, team, architectures, and Gatekeeper policy before detach and publication.
- [ ] Review `.build/release/VoiceFlow-notarized.receipt.json`: it records the App ZIP submission ID/status/SHA-256 and DMG submission ID/status/submission SHA-256 separately from the final post-staple DMG SHA-256. The final digest must match the published `VoiceFlow-notarized.dmg`; the private `VoiceFlow.dmg` remains a separate `make dmg` output.
- [ ] Test the quarantined final DMG on a clean supported Mac before claiming first-launch trust. A completed local script run alone does not establish public Gatekeeper acceptance.

## Privacy and behavior acceptance

- [ ] Writing-tone trials require an explicit click, compare saved/draft prompts with the same sample, distinguish model/local/provider fallback/guard fallback results, and cancel on edits or close. No History, clipboard, window, style-example capture or automatic paste occurs.
- [ ] Optional translation shortcut defaults off, captures the target per session and preserves global output/activation settings. Plain dictation becomes a Translate intent through light/standard/heavy cleanup and the final guard; short and merged long transcripts translate without a spoken command prefix. AssemblyAI uses raw Sync followed by shared cleanup. Test tap / hold_to_talk, cross-key releases, cancel, unavailable cleanup and recovery after registration failure. Strict offline keeps cloud requests blocked.
- [ ] Audio evaluation defaults to validation only. An explicit live run selects reviewed external fixtures, model IDs and credentials; actual ASR output enters cleanup, errors remain visible, and candidate/fallback metrics are separate. Adapter timings are not native stop-to-insert evidence.

- [ ] Cursor/native editor uses the developer policy and preserves technical tokens.
- [ ] Gmail/mail host uses the professional email policy.
- [ ] Browser access off never queries or stores a host.
- [ ] New per-App AX text, local OCR, cloud vision, provider-text, and style-example grants default off; the existing global OCR setting is only a master switch. Legacy OCR migration preserves local OCR only for mappings that existed when the user's old OCR setting was on.
- [ ] Text-to-provider permission is checked independently from local OCR; authorized OCR-derived text may leave the Mac for configured ASR / cleanup providers.
- [ ] Automatic cloud vision requires a concrete App / executable mapping, provider text permission, configured vision model, Screen Recording permission, and insufficient authorized AX/OCR context; one current-window image is shared with local OCR and cloud fallback.
- [ ] ASR and cleanup may use different providers; new or replaced keys are stored only in Keychain and verified before settings are scrubbed. A failed secure write reports an error and never creates a plaintext sidecar; legacy settings/sidecars remain until their migration is verified.
- [ ] On Device offers exactly five local ASR models for new selection and explicit download: Qwen3-ASR 0.6B / 1.7B BF16, Parakeet TDT v3 0.6B 8-bit, FireRedASR2-AED MLX, and Fun-ASR-Nano 2512 MLX. Backend pins each manifest to an immutable revision and verifies every file's byte count and SHA-256; Fun tokenizer files remain in the pinned `Qwen3-0.6B/` subdirectory. Cold-load admission uses revision-bound conservative provisional reserves plus the host-pressure and 80% physical-memory budget gate; before transfer, ASR download preflight checks the same fixed per-model reserve against the 80% physical-memory cap, while transient host pressure remains runtime-only. These reserves are not field-calibrated, and none of the five models has completed VoiceFlow transcription-quality evaluation; a compiled route is not evidence of real-model inference or transcript quality. Cohere Transcribe 2B and SenseVoice stay available only as legacy saved-settings/files records, with no new selection or download; existing files remain visible and deletable. MLX selection/download is gated by Apple Silicon and macOS 14, while HTTP/cloud ASR remains available on supported older Macs when strict offline is off.
- [ ] Model file state, runtime capability handshake, per-model loading state, and loaded model are shown separately; handshake success is not presented as actual inference. Loading cancellation stops the current load while preserving saved provider/model settings and downloaded files. SenseVoice is labelled legacy files-only, cannot run MLX inference, and fails the local inference probe.
- [ ] AssemblyAI eligible short Dictation uses its fused cleanup candidate without requiring the shared cleanup credential. If the candidate fails, shared cleanup is attempted only when the current route is enabled and available; credentials are required only for providers that need them, while keyless local routes need none. Long recordings use full-coverage raw Sync chunks and at most one whole-transcript common cleanup, with local/raw fallback when that cleanup route is unavailable.
- [ ] AssemblyAI save-time probe checks its raw Sync ASR route and skips optional shared cleanup verification. Soniox settings save stores the key as configured without a provider request; connection occurs only during dictation or an explicit provider test. Saved credentials do not imply general service verification.
- [ ] Strict offline permits only On Device ASR; loopback LocalWhisper/custom HTTP routes and their probes are blocked, while On Device setup probing remains local and available. It also blocks cloud cleanup, retries, cascades/prefetch, vision and cloud text actions; verified local Ollama, VoiceFlow `on_device_mlx` and VoiceFlow `on_device_llama_cpp` cleanup remain local. Explicit model downloads remain separate network actions.
- [ ] The five offered local ASR models expose only the language controls their pinned sidecar route uses. Parakeet, FireRed, and Fun-ASR do not report detected language; Parakeet and FireRed run auto-only, while Fun-ASR accepts auto or a Chinese / English prompt. Saved legacy Cohere settings still require a fixed Chinese or English language.
- [ ] Local cleanup may use Ollama `qwen3.5:4b`, keyless `on_device_mlx` (Qwen 3.8 27B MLX 6-bit, Qwen3.5 2B/4B MLX 4-bit, Gemma 4 E4B IT MLX 4-bit, or Ministral 3 3B Instruct MLX 4-bit), or keyless `on_device_llama_cpp` with existing legacy GGUF installs (Qwen3.5 2B, Qwen3.5 4B, and Gemma 4 E4B IT); GGUF downloads are not offered for new installs. A schema-26 settings file that selected the retired LM Studio route loads as `on_device_mlx` with model `qwen3_8_27b_mlx_6bit` and keeps a key-free pre-migration backup. VoiceFlow does not install Ollama. MLX requires Apple Silicon and macOS 14+; the GGUF helper requires Apple Silicon and macOS 13.3+ and checks a separate memory reserve per model. Neither compatibility check certifies performance on a particular Mac or raises the app's macOS 13 minimum. Unsupported/unknown platform or memory status keeps the relevant model download and route apply unavailable; existing files remain unless the user deletes the managed copy. The five MLX models allow explicit download; only Qwen 3.8 27B also allows verified-folder import with an APFS managed clone, while the other four are download-only. Downloads require a user action and network access. Cancellation and deletion affect only managed files. First load and warm status do not establish quality. If local cleanup fails during dictation, existing local rules / raw-text fallback apply; the app never silently switches to Ollama or a cloud route.
- [ ] Browser host/path selection stays local. Raw URL, URL credentials, query, fragment, title, PID, target identity, screen images, and unapproved style examples never enter provider payloads, History, exports, or logs.
- [ ] Switching apps or browser tabs during processing never causes an uncertain paste. Recovery offers only a destination confirmed by its owner; a target change alone does not prove clipboard content.
- [ ] A retry from History does not reuse the original target. Report copied or saved only after the corresponding clipboard or History owner confirms it; preserve an honest unknown/error state otherwise.
- [ ] Selected-text and screen-action ASR failures retain complete recovery audio; long recordings and History retries cover the whole audio by bounded chunks. Retrying never restores the original action source or delivery target.
- [ ] Normal quit cancels active recording/text work and cloud requests, stops the local sidecar through bounded shutdown, and cleans its private temporary audio without deleting model files, saved choices, History, or retained recovery audio.
- [ ] Recording limit and spool quota behavior are visible and recoverable.
- [ ] Reduced-motion, fullscreen, multi-monitor, and Dock placement are manually checked.

- [ ] Cloud LLM rejects missing/filtered/tool/budget completion and falls back to the complete prepared transcript; valid stop + EOF is accepted. Anthropic stop reasons are checked.
- [ ] ASR, shared cloud cleanup and both vision paths reject 307/308 without sending a private body to the redirect target.
- [ ] Clear-all after an old-schema migration removes owned rollback backups. Retention limits them to at most 7 days (or shorter history retention), preserving settings, models and external exports.
- [ ] Fn and ⌘⌥Space offer exactly two enabled recording modes in both languages, themes and onboarding. Binding capture preserves the mode; old single modifiers stay visible with rebind guidance; Fn opens Keyboard settings without changing system preferences.
- [ ] Shortcut chips open on normal click/keyboard activation. The inline editor captures only from its field, maps Option/Shift physical keys, allows Tab/Shift+Tab navigation, and exposes cancel/Fn/main-default reset/optional clear. Escape with held modifiers, outside interaction, blur and page changes restore the old binding; native keyboard state confirms all physical keys release before registration/save even when WebKit omits a keyup or retains old flags. Pointer clicks with null WebKit blur targets still allow in-group actions. Missing keys do not block legal saves; registration/persistence errors keep the old page value and offer retry. Verify late native ACKs across unmount/new capture, cancelling a queued pause, repeat events and outside focus during save/cancel; completion never steals focus from another control. New ordinary typing/navigation bindings are rejected without rewriting existing legal bindings.
- [ ] Test repeats, two fast taps, either release order, release/Esc during microphone startup, stale/cross-source releases, Fn chords, simulated paste and ordinary ⌘C/⌘V/Shift. Stopping/processing ignores new recording gestures; sleep/listener interruption cancels and rearms only after release.
- [ ] Migrating an older settings schema to the current schema (27) is idempotent, preserves bindings and migrates retired gestures to tap. The update notice explains the new Fn rule. Missing unchanged credentials do not block binding/mode saves; registration/write failures restore old bindings and page values.
- [ ] Settings default to 1035×750, reject edge resizing, zoom and native fullscreen, and fit smaller display work areas including scaling, native frame and Dock. Moving between displays and reopening settings recompute the fit. Small-screen and enlarged-text layouts retain scrollable navigation; both themes show readable help, selection and keyboard focus without nested shortcut/activation outlines.
- [ ] On macOS, the settings content reaches the top with no separate titlebar, repeated window title or separator. Native controls sit above the sidebar brand, stay reachable during onboarding/loading/errors, and retain close/minimize behavior. Top whitespace drags the window, double-click does not resize it, and navigation/inputs remain interactive. Other platforms keep their native frame.
- [ ] All shared controls follow the Codex screenshot reference in light/dark themes: faint panel boundaries, 12px controls, 16px menus/dialogs, 40px standard actions, quiet fills, and one keyboard/validation indicator. Check all fixed dropdowns and editable suggestions, empty defaults, arbitrary model/host input, long lists, typeahead, arrows/Home/End/Enter/Esc, outside-click dismissal, focus return, disabled/error states, and small-window collision handling. Pointer-opened menus show a selected fill/checkmark without a focus box. Fn help creates no empty separator row.
- [ ] History edits can be cancelled; dictionary load/action and learning Undo failures stay visible and can be retried.
- [ ] Service drafts do not change the active route before apply; provider-row tests do not switch routes; advanced mapping collapse preserves settings and leaves independent grants visible.

## Update and rollback decision

- [ ] If an updater is enabled, its signed public key, endpoint, channel policy, and rollback procedure are configured in `tauri.conf.json` and tested.
- [ ] If those values are not configured, distribute versioned DMG/ZIP artifacts manually and do not advertise in-app updates.
- [ ] Database/settings migration has a backup and downgrade plan before changing schema.

## Required secrets (names only)

GitHub Actions secrets for self-signed releases:

- `MACOS_CERT_P12`
- `MACOS_CERT_PASSWORD`

Local environment variables for optional Developer ID notarization via `make notarize` (not used by `release.yml`):

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_TEAM_ID`
- `APPLE_ID`
- `APPLE_PASSWORD`

No secret values belong in this repository or in app logs.

## 可选录音与启动验收

以下项目需在实际 macOS 应用包上手工执行；单元测试不能替代这些结论。

- [ ] 麦克风连续三次录音：同设备复用，Idle 不产生 spool、预取或上传；关闭选项和权限撤销释放设备。
- [ ] 合盖切换到指定输入设备；不存在的设备明确报错；开启／关闭盖后下一会话重新选择。
- [ ] 普通、翻译及跳过整理热键的点按、纯按住、短按松手、Idle release、取消和重新注册行为正确；AssemblyAI raw Sync 和共同 LLM 禁用可从本次请求验证。
- [ ] 普通与跳过整理热键交叉按压不会提前停止；主热键注册冲突后原有辅助键仍可触发；可选热键 Backspace / Delete 可清空，Esc 保留原绑定。
- [ ] 模糊词典开启时保留原片段触发词和明确文字操作；替换不能创建片段触发；Unicode 行／段落分隔符保持原样。
- [ ] 250ms 尾部音频、取消即时性、自动上限、设备断开和 Soniox 完整恢复。
- [ ] 冷启动及第二实例 CLI 动作、登录 --background 不显示或抢焦点；手动打开显示设置；系统登录启动状态与 UI 一致。
- [ ] 系统登录启动状态与保存值不一致时，显式开关能纠正两个方向的差异；保存失败回滚，其他设置保存不改变系统状态。
- [ ] Secure Input + 已确认复制、剪贴板写入失败、输入状态不确定的文案分别正确。
- [ ] macOS 打包后存在反馈 WAV、唯一 release-notes 来源、MLX sidecar、Metal 资源和第三方声明；开始／结束提示音及更新说明可用。

## 首次使用、麦克风自检和学习反馈验收

- [ ] 新配置默认只显示云端 / 本机路线，展开高级项能选择服务商、模型和区域；已有非默认服务商仍保留并自动展开。
- [ ] On Device 就绪后引导关闭云端整理，已展示的本机 ASR 语言约束与运行时能力检查不能跳过；完成听写试用后可直接完成，也可主动试用选中文本。
- [ ] 只进入主流程时保留已有文字操作开关与快捷键，录音 / 处理期间不能用导航离开试用。
- [ ] 完成保存和读取设置期间不能进入可选试用；复制、仅历史、未确认和降级完成后，已打开的词典页刷新实际替换次数。
- [ ] 麦克风自检不触发服务请求或创建音频文件；实际设备、合盖选择、增益与显示一致；验证安静输入、较轻声音、过高峰值、断开设备、权限撤销。
- [ ] 自检最多 30 秒；停止 / 关闭设置 / 隐藏窗口或失去焦点 / 切换设备或增益时结束；正式听写优先且迟到自检取消不能停止录音；开启常开麦克风时恢复空闲流。
- [ ] 真实确认或自动晋升的词在后续本机替换时累计处理次数；正确原稿、提示词命中、保守场景与语气试跑不计入。History 重新整理按同一语义统计，不把次数当作交付或准确率。
- [ ] 学习反馈读取失败显示重试且保留最近成功数据；没有生效规则的词条隐藏收益；旧数据库迁移不回填收益，清空全部数据删除新表及迁移备份。
