# VoiceFlow

VoiceFlow 是一个 macOS-first、privacy-first 的系统级语音输入工具：在当前获得焦点的 App 中录音，使用 Groq 完成 ASR 和可选的 AI Clean Up，再安全地插入原输入框；如果目标发生变化或粘贴权限不可用，则保留结果并复制到剪贴板。

## 开发启动

需要 macOS 13+、Node.js、Rust 和 Xcode Command Line Tools。

```bash
npm ci
npm run tauri dev
```

`npm run tauri dev` 会启动 Vite 和 Tauri 后端。设置窗口关闭后仍会保留菜单栏后台进程；从菜单栏 VoiceFlow 图标可以重新打开设置。

## 首次使用

首次启动会依次引导：

1. 请求麦克风权限。录音只在用户主动开始 dictation 后进行。
2. 开启辅助功能权限，用于将文字粘贴到当前焦点输入框。
3. 输入并验证 Groq API Key。Key 存在 macOS Keychain，前端只收到 configured 状态和 masked hint。
4. 设置全局快捷键并完成一次 Try It。

没有有效 API Key 时不能完成 onboarding。没有辅助功能权限时，VoiceFlow 仍会把结果复制到剪贴板，不会向未经确认的目标注入文字。

## Context-aware 输出

VoiceFlow 在录音开始时捕获当前目标，并使用本地 App identity、bundle/process、窗口/焦点信息以及用户授权的浏览器 host 识别场景。支持的 profile 包括代码、搜索、邮件、团队协作、聊天、文档、终端、日历/任务、表单、笔记、社交和 General。

context 只是 Clean Up 的软提示，用户实际表达优先。未知 App 仍使用最小化 General cleanup。raw URL、window title、PID 和文档内容不会发送给 LLM；浏览器读取需要单独开启权限，并且只保留本地 fingerprint/host。

“上下文模式”设置还提供临时模式覆盖，可在当前运行中手动选择 Code、Email、Chat 等策略；清除后恢复自动检测。覆盖只改变写作 policy，不改变真实的 App/window/input target guard，也不会写入设置文件。

## 安全交付和恢复

- 录音目标在开始时锁定；处理期间切换 App、窗口、浏览器 Tab 或焦点输入框会 fail closed，并复制到剪贴板。
- history retry 永远 clipboard-only，不会把历史音频插入到当前可能不同的 App。
- ASR、LLM、paste 失败都会保留可恢复文本；失败或 degraded 录音会保存到本地 spool/history，允许重试。
- 取消后不会启动迟到的 provider 请求或 paste；API Key 不写入 settings JSON，也不写入日志。

音频、history 和使用统计存储在 Tauri `app_data_dir`。音频只上传到用户配置的 ASR 服务，文本只上传到配置的 LLM 服务；当前版本不收集 telemetry。

默认保留 recovery audio 7 天、history 文字 365 天；可以在“常规”中分别调整，history 文字也支持“永久”保留。History 页面支持导出 JSON、删除单条记录和清空全部本地 history/audio/usage。完整数据流和删除说明见 docs/privacy.md。当前 history SQLite 和 recovery audio 依赖 macOS 文件权限，未做应用层加密。

发布构建需要 Developer ID、notarization 和 App Store Connect 凭据；本地 tauri dev 或 ad-hoc bundle 不能作为 Gatekeeper 发布证据。CI 的 release job 只会在配置签名 secrets 后执行。

## 本地质量检查

```bash
npm run build
npm run lint
npm test -- --run

cd src-tauri
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo check --target aarch64-apple-darwin
cargo check --target x86_64-apple-darwin
cargo audit
```

`cargo audit` may report maintenance warnings from Tauri's non-macOS GTK3 backend; the macOS dependency tree is checked separately in CI.

Provider 测试使用本地 test-only HTTP responder 覆盖成功、空响应、401、429、5xx、streaming cleanup 和 protected-token fallback，不需要真实 API Key。真实麦克风、辅助功能、浏览器自动化和外部 App 粘贴仍必须在授权的 macOS 真机上验收。

## 常见问题

- 快捷键无反应：打开 VoiceFlow 设置，确认辅助功能权限和快捷键注册错误提示；菜单栏进程只能运行一个实例。
- 只能复制不能自动插入：检查辅助功能权限，或确认录音期间没有切换目标窗口/Tab。
- 首次浏览器 context 显示 `needs permission`：在“上下文模式”中启用浏览器访问，然后回到目标浏览器重新检测。
- 网络失败：文本会回退到原始转录或本地 cleanup，并出现在 history 中；恢复网络后可以重试。
