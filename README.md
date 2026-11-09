# VoiceFlow

VoiceFlow 是 macOS 优先的系统听写工具。默认点按 `⌘⌥Space` 开始和结束录音，也可选择按住说话或配置 Fn；识别完成后，VoiceFlow 按当前整理设置处理文字，并安全地交付到当前 App。Esc 取消录音或处理。

[下载最新发行版](https://github.com/wangm12/voice-flow/releases/latest)。

## 快速开始

要求 macOS 13+、Node.js 22.13+ 或 24 LTS、Rust 和 Xcode Command Line Tools。Apple Silicon 上构建 MLX sidecar 与 Metal 资源还需要完整 Xcode。默认产品支持 macOS 13；随包 MLX 本机路线要求 Apple Silicon 与 macOS 14+。universal 构建选项不代表 Intel 已通过发布验收。

```bash
npm ci
make run
```

首次启动按引导完成权限、路线设置和听写试用；之后可在「模型配置 → 服务配置」调整识别 / 整理路线。新建或更换的 provider key 存在 macOS Keychain。听写按需请求麦克风；自动粘贴需要辅助功能权限，也可以选择复制结果。具体首次设置步骤见[端到端工作流](docs/end-to-end-workflows.md)。

## 日常使用与边界

- 默认 ASR 是 Groq `whisper-large-v3-turbo`，默认 AI 整理是 Groq `openai/gpt-oss-20b`。ASR 与整理可选不同路线；保存设置或 key 本身不会发送请求。
- 云端 ASR 路线会把录音发送到所选 provider；On Device 路线在本机识别。默认 batch 预取不显示为实时字幕；Soniox 是独立的可选实时路线。
- 默认听写不会截图或向 LLM 发送窗口图片。「看屏幕」是独立的用户动作，需要视觉模型和 Screen Recording 权限。逐 App 的文字与图像授权默认关闭。
- 选用 AssemblyAI 时，符合条件的短录音可使用它自己的 Dictation 整理候选；候选不可用时会按当前整理路线回退。长录音走 raw Sync 后最多做一次共同整理。keyless 本机整理路线不需要云端整理 key。
- Apple Silicon 本机 MLX 模型需要用户主动下载或导入；下载、文件校验、运行时握手、模型加载和识别质量是不同状态。VoiceFlow 不自动下载模型，也不安装 Ollama 或 LM Studio。
- 「模型配置」分为服务配置、API Key、本地模型和高级设置四页；六类选中文本操作是精确修改、改写、缩短、翻译、整理和拟答，均先预览再确认。
- HUD 使用 220 × 48 pt、R24 胶囊；批准方案与关键尺寸演进见 [Beam v10 设计参考](docs/design/voice-hud/README.md)。
- 自动粘贴依赖 Accessibility；看屏幕依赖 Screen Recording。产品主要支持 macOS，Windows/Linux 尚未作为完整体验验收。质量、最低内存和 RSS 没有未测量的保证。

隐私、保留与删除边界见[隐私说明](docs/privacy.md)；其余产品说明、限制和研究分类见[文档索引](docs/README.md)。

## 开发与验证

桌面应用使用 React/Vite 与 Rust/Tauri。常用检查：

```bash
npm test
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

官网在 `website/` 独立安装依赖和构建，不启动 Tauri：

```bash
npm ci --prefix website
npm run dev --prefix website      # http://127.0.0.1:5173
npm run lint --prefix website
npm run build --prefix website   # website/dist/
npm run preview --prefix website
```

音频文件评测从 `npm run eval:audio -- --help` 进入；默认只校验仓库外的显式 fixture，只有 `--run` 才按所选模型与凭据发请求。它不读取麦克风、Keychain 或 History，也不代替真实设备与目标 App 验收。输入协议和限制见[评测文档](docs/cleanup-evaluation.md)。CI 与发布门槛见[发布检查表](docs/release-checklist.md)；macOS runner 选择 Xcode 26 并要求 Swift 6.2。

可选的 `voiceflow-learning-harness` 是独立研究入口，不属于日常产品路径；其 deterministic-text slice 使用明示的临时 arm 与合成文本契约。运行边界和示例见[独立 deterministic-text learning fixture](docs/end-to-end-workflows.md#standalone-deterministic-text-learning-fixture)。

## 构建、签名与首次打开

```bash
make dmg
```

默认产物为 `.build/release/VoiceFlow.dmg`，使用可复用的签名身份；本机缺少身份时会尝试创建 `VoiceFlow` 自签证书。把 App 安装到 `/Applications`，首次打开若被 Gatekeeper 拦截，在「系统设置 → 隐私与安全性」选择「仍要打开」。自签不等于公证；授予 Accessibility 时，选择安装后的 `/Applications/VoiceFlow.app`。签名身份、TCC 权限、unsigned 产物与迁移说明见[发布检查表](docs/release-checklist.md)。

只有在明确允许未签名构建且没有可用签名身份时，`VOICEFLOW_ALLOW_UNSIGNED=1 make dmg` 才输出 `.build/release/VoiceFlow-unsigned.dmg`。Developer ID 公证是独立流程，需要 Developer ID、匹配的 Team ID 和 Apple 公证凭据：

```bash
APPLE_SIGNING_IDENTITY="Developer ID Application: Name (TEAM_ID)" \
APPLE_TEAM_ID="TEAM_ID" \
APPLE_ID="developer@example.com" \
APPLE_PASSWORD="app-specific-password" \
make notarize
```

公证产物为 `.build/release/VoiceFlow-notarized.dmg`。完整 App / DMG 验证与回执检查见[发布检查表](docs/release-checklist.md)。GitHub Release 由 `.github/workflows/release.yml` 发布，签名只表示代码签名，不表示公证。

## 目录

```text
src/                    React 设置界面、HUD 与前端测试
src-tauri/src/          Rust/Tauri 后端、provider、权限与持久化
website/                独立的本地演示官网
scripts/                评测、打包、签名和发布辅助入口
docs/                   活文档、持续研究与仍进行中的计划
ThirdPartyLicenses/     第三方许可证和模型来源材料
.github/workflows/      CI 与 GitHub Release
```

项目许可证尚未确定；依赖与随包声明见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
