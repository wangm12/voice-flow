# VoiceFlow release checklist

This is a preflight checklist. GitHub Actions (`.github/workflows/release.yml`) builds `VoiceFlow.dmg` and attaches it to the GitHub Release. A build is not shippable until every unchecked item has an owner and evidence.

Independent Astra runtime/E2E acceptance remains pending. Static gates, protocol code, saved credentials, and model file status do not replace runtime, inference, account, or end-to-end evidence.

## Automated gates

- [ ] `npm ci`
- [ ] `npm test`
- [ ] `npm run lint`
- [ ] `npm run build`
- [ ] `npm audit --audit-level=high`
- [ ] `cargo audit` in `src-tauri/`
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings`
- [ ] `cargo check --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin`
- [ ] Run Rust gates from a clean checkout with no prebuilt `target/mlx-sidecar/stage`; debug builds create only an empty resource directory. Apple Silicon release packaging still builds and validates the executable and Metal bundles through `mlx-sidecar/scripts/package.sh`.

## macOS signing

Default GitHub Releases use a stable self-signed `VoiceFlow` identity. That keeps Accessibility across updates on machines that install those DMGs. It is not notarized; first launch still needs Open Anyway or `xattr`.

- [ ] Run `make signing-cert` once on the maintainer Mac.
- [ ] Add `MACOS_CERT_P12` and `MACOS_CERT_PASSWORD` from `.build/signing/`. Never commit the `.p12`.
- [ ] `make dmg` produces `.build/release/VoiceFlow.dmg` signed with that identity.
- [ ] Hardened runtime and `src-tauri/Entitlements.plist` are reviewed for the exact shipped binary.
- [ ] Test first launch on a clean macOS user account and confirm microphone, accessibility, and browser permission prompts.

## Developer ID notarization (optional)

- [ ] Developer ID Application certificate is available in the signing keychain.
- [ ] `APPLE_SIGNING_IDENTITY` and `APPLE_TEAM_ID` are supplied through the environment, never committed.
- [ ] Build with `make notarize` (`scripts/release-macos.sh`).
- [ ] Verify the signed app with `codesign --verify --deep --strict --verbose=2`.
- [ ] Verify notarization with `xcrun stapler validate` and `spctl --assess --type execute`.

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
- [ ] On Device offers Qwen3-ASR 0.6B, Qwen3-ASR 1.7B and Cohere Transcribe 2B; MLX selection/download is gated by Apple Silicon and macOS 14, while HTTP/cloud ASR remains available on supported older Macs when strict offline is off.
- [ ] Model file state, runtime capability handshake, per-model loading state, and loaded model are shown separately; handshake success is not presented as actual inference. Loading cancellation stops the current load while preserving saved provider/model settings and downloaded files. SenseVoice is labelled legacy files-only, cannot run MLX inference, and fails the local inference probe.
- [ ] AssemblyAI eligible short Dictation uses its fused cleanup candidate without requiring the shared cleanup credential. If the candidate fails, shared cleanup is attempted only when its credential is available; otherwise raw/local fallback is used. Long recordings use full-coverage raw Sync chunks and at most one whole-transcript common cleanup, with local/raw fallback when that cleanup route is unavailable.
- [ ] AssemblyAI save-time probe checks its raw Sync ASR route and skips optional shared cleanup verification. Soniox settings save stores the key as configured without a provider request; connection occurs only during dictation or an explicit provider test. Saved credentials do not imply general service verification.
- [ ] Strict offline permits only On Device ASR; loopback LocalWhisper/custom HTTP routes and their probes are blocked, while On Device setup probing remains local and available. It also blocks cloud cleanup, retries, cascades/prefetch, vision and cloud text actions, while separately labeled model downloads remain network actions; configured loopback Ollama cleanup remains local.
- [ ] Cohere Transcribe requires a fixed supported language (Chinese or English) and cannot proceed in automatic language mode.
- [ ] Local cleanup uses the explicit Ollama `qwen3.5:4b` selection and status path; VoiceFlow does not install Ollama or download its model, and unavailable cleanup uses local rules / Off without cloud fallback in strict offline mode.
- [ ] Browser host/path selection stays local. Raw URL, URL credentials, query, fragment, title, PID, target identity, screen images, and unapproved style examples never enter provider payloads, History, exports, or logs.
- [ ] Switching apps or browser tabs during processing results in clipboard fallback, never an uncertain paste.
- [ ] A retry from history is clipboard-only because the original target guard is no longer trustworthy.
- [ ] Selected-text and screen-action ASR failures retain complete recovery audio; long recordings and History retries cover the whole audio by bounded chunks. Retrying never restores the original action source or delivery target.
- [ ] Normal quit cancels active recording/text work and cloud requests, stops the local sidecar through bounded shutdown, and cleans its private temporary audio without deleting model files, saved choices, History, or retained recovery audio.
- [ ] Recording limit and spool quota behavior are visible and recoverable.
- [ ] Reduced-motion, fullscreen, multi-monitor, and Dock placement are manually checked.

- [ ] Cloud LLM rejects missing/filtered/tool/budget completion and falls back to the complete prepared transcript; valid stop + EOF is accepted. Anthropic stop reasons are checked.
- [ ] ASR, shared cloud cleanup and both vision paths reject 307/308 without sending a private body to the redirect target.
- [ ] Clear-all after an old-schema migration removes owned rollback backups. Retention limits them to at most 7 days (or shorter history retention), preserving settings, models and external exports.
- [ ] The retained backend `undo_last_delivery` command rejects a different field with identical contents; actual delivery field, generation, native identity, value and expiry are checked before Cmd+Z. The HUD has no delivery Undo button; dictionary learning Undo remains available.
- [ ] Fn and ⌘⌥Space offer exactly two enabled recording modes in both languages, themes and onboarding. Binding capture preserves the mode; old single modifiers stay visible with rebind guidance; Fn opens Keyboard settings without changing system preferences.
- [ ] Shortcut chips open on normal click/keyboard activation. The inline editor captures only from its field, maps Option/Shift physical keys, allows Tab/Shift+Tab navigation, and exposes cancel/Fn/main-default reset/optional clear. Escape with held modifiers, outside interaction, blur and page changes restore the old binding; native keyboard state confirms all physical keys release before registration/save even when WebKit omits a keyup or retains old flags. Pointer clicks with null WebKit blur targets still allow in-group actions. Missing keys do not block legal saves; registration/persistence errors keep the old page value and offer retry. Verify late native ACKs across unmount/new capture, cancelling a queued pause, repeat events and outside focus during save/cancel; completion never steals focus from another control. New ordinary typing/navigation bindings are rejected without rewriting existing legal bindings.
- [ ] Test repeats, two fast taps, either release order, release/Esc during microphone startup, stale/cross-source releases, Fn chords, simulated paste and ordinary ⌘C/⌘V/Shift. Stopping/processing ignores new recording gestures; sleep/listener interruption cancels and rearms only after release.
- [ ] Schema 24→25 migration is idempotent, preserves bindings and migrates old gestures to tap. The update notice explains the new Fn rule. Missing unchanged credentials do not block binding/mode saves; registration/write failures restore old bindings and page values.
- [ ] Settings default to 1035×750, reject edge resizing, zoom and native fullscreen, and fit smaller display work areas including scaling, native frame and Dock. Moving between displays and reopening settings recompute the fit. Small-screen and enlarged-text layouts retain scrollable navigation; both themes show readable help, selection and keyboard focus without nested shortcut/activation outlines.
- [ ] On macOS, the settings content reaches the top with no separate titlebar, repeated window title or separator. Native controls sit above the sidebar brand, stay reachable during onboarding/loading/errors, and retain close/minimize behavior. Top whitespace drags the window, double-click does not resize it, and navigation/inputs remain interactive. Other platforms keep their native frame.
- [ ] All shared controls follow the Codex screenshot reference in light/dark themes: faint panel boundaries, 12px controls, 16px menus/dialogs, 40px standard actions, quiet fills, and one keyboard/validation indicator. Check all fixed dropdowns and editable suggestions, empty defaults, arbitrary model/host input, long lists, typeahead, arrows/Home/End/Enter/Esc, outside-click dismissal, focus return, disabled/error states, and small-window collision handling. Pointer-opened menus show a selected fill/checkmark without a focus box. Fn help creates no empty separator row.
- [ ] History edits can be cancelled; dictionary load/action and learning Undo failures stay visible and can be retried.
- [ ] Service drafts do not change the active route before apply; provider-row tests do not switch routes; advanced mapping collapse preserves settings and leaves independent grants visible.

## Update and rollback decision

- [ ] If an updater is enabled, its signed public key, endpoint, channel policy, and rollback procedure are configured in `tauri.conf.json` and tested.
- [ ] If those values are not configured, distribute versioned DMG/ZIP artifacts manually and do not advertise in-app updates.
- [ ] Database/settings migration has a backup and downgrade plan before changing schema.

## Required CI secrets (names only)

For self-signed GitHub Releases:

- `MACOS_CERT_P12`
- `MACOS_CERT_PASSWORD`

For optional Developer ID notarization via `make notarize`:

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
- [ ] On Device 就绪后引导关闭云端整理，Cohere 固定语言与运行时能力检查不能跳过；完成听写试用后可直接完成，也可主动试用选中文本。
- [ ] 只进入主流程时保留已有文字操作开关与快捷键，录音 / 处理期间不能用导航离开试用。
- [ ] 完成保存和读取设置期间不能进入可选试用；复制、仅历史、未确认和降级完成后，已打开的词典页刷新实际替换次数。
- [ ] 麦克风自检不触发服务请求或创建音频文件；实际设备、合盖选择、增益与显示一致；验证安静输入、较轻声音、过高峰值、断开设备、权限撤销。
- [ ] 自检最多 30 秒；停止 / 关闭设置 / 隐藏窗口或失去焦点 / 切换设备或增益时结束；正式听写优先且迟到自检取消不能停止录音；开启常开麦克风时恢复空闲流。
- [ ] 真实确认或自动晋升的词在后续本机替换时累计处理次数；正确原稿、提示词命中、保守场景与语气试跑不计入。History 重新整理按同一语义统计，不把次数当作交付或准确率。
- [ ] 学习反馈读取失败显示重试且保留最近成功数据；没有生效规则的词条隐藏收益；旧数据库迁移不回填收益，清空全部数据删除新表及迁移备份。
