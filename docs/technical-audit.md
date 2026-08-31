# VoiceFlow Technical Quality Audit

> **2026-08-04 快照。** 分数和测试数量不要当现状。没有仓库根目录的 `findings.md`。

更新时间：2026-08-04

范围：当时的 React/Tauri settings UI、always-on-top HUD、交互状态和静态资源。该审计不替代真实 macOS 权限、麦克风、WindowServer 或外部 App 验收。

## Audit health score

| Dimension | Score | Finding |
| --- | ---: | --- |
| Accessibility | 4/4 | API Key 输入已补程序化 label；compact action 已提供至少 44px 命中区域和 visible focus。 |
| Performance | 4/4 | HUD 固定几何、opacity/transform-only motion、inactive animation paused；已有 headless probe 无 >20ms frame。 |
| Theming | 3/4 | settings surface 使用集中 token 并支持 light/dark；HUD 有独立 OLED token，但 action foreground 和部分色值仍是局部硬编码。 |
| Responsive design | 4/4 | 设置窗口现在限制为至少 720×520，避免侧栏和 onboarding 内容被任意缩小裁剪；窄屏 onboarding 仍隐藏侧栏。 |
| Anti-patterns | 4/4 | 没有 gradient text、blur 堆叠、装饰性 metrics、重复卡片网格或 layout-property animation；HUD gradient 是明确的状态表面。 |
| **Total** | **19/20** | **Excellent — 仍需完成真实 macOS 外部状态验收。** |

## Severity findings

### P0/P1 blockers

没有发现当前静态 P0/P1 blocker。此前发现的 terminal HUD 状态问题已在 2026-08-04 修复，但仍需要在真实 macOS WindowServer 环境确认最终视觉表现：

### Resolved P1 — terminal copied state reused the processing visual path

- Location: `src/components/Island/VoicePill.tsx:59-74`
- Category: Interaction state / motion
- Resolution: `copied` now remains a terminal state, no longer renders the processing progress/loading path, and exits through the short terminal fade. The stop/cancel/completion glyphs now use the bundled Lucide SVG family instead of ad-hoc CSS marks.
- Remaining verification: confirm the native panel's final fade and click behavior during a real authorized recording.

真实 macOS E2E、签名、公证、Gatekeeper 和 clean-user 安装证据仍是整体 prod-readiness 的独立阻塞项。

### P2 — HUD theme values are intentionally isolated but not fully tokenized

- Location: `src/island.css:2-6, 102, 124-126`
- Category: Theming
- Impact: the always-on-top HUD remains dark in light system mode and uses local white/black values. This is visually intentional for glanceability, but future palette changes can diverge from the settings surface.
- Recommendation: keep a separate HUD palette, but expose action foreground/background as named HUD variables and use `light-dark()` or documented fixed dark-mode semantics if a user-selectable HUD theme is added.

## Positive findings

- Native focusable controls use semantic `button`, `input`, `select`, `label`, `role="switch"`, `aria-live`, and `role="alert"` in the primary flows.
- API Key is kept out of the settings DTO in plaintext; the UI receives only configured state and a masked hint.
- HUD state changes use fixed slots and pause hidden animation; the live panel remains click-through.
- Reduced-motion media rules disable onboarding/HUD motion.
- Error and fallback states are visible and generally actionable; failed saves expose a retry action.
- Settings and HUD avoid blur/backdrop-filter, which protects WindowServer/WebKit frame time.

## Verification evidence

- `npm run build` — pass
- `npm run lint` — pass
- `npm test -- --run` — 15 passed
- `cargo fmt --check` — pass
- `cargo test --locked` — 108 passed, 1 ignored
- `cargo clippy --all-targets --all-features -- -D warnings` — pass
- `cargo check --locked --target aarch64-apple-darwin` — pass
- `cargo check --locked --target x86_64-apple-darwin` — pass
- `npm audit --registry=https://registry.npmjs.org --audit-level=high` — 0 vulnerabilities
- `cargo audit` — pass; reports non-macOS GTK3/`glib` unmaintained/unsound warnings only. `enigo` was upgraded to 0.6.1, removing the affected `memmap2 0.8` dependency from the lockfile.
- Native settings window minimum size is enforced at 720×520 in `src-tauri/tauri.conf.json`.

## Scope boundary

This UI audit does not prove microphone capture, external-App insertion, Apple Events behavior, WindowServer smoothness, local-data encryption, or signed distribution. Historical notes are in `docs/acceptance-evidence.md`.
