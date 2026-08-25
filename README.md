# VoiceFlow

VoiceFlow 是一个 macOS-first 的系统级语音输入工具。按一下全局快捷键开始说话，再按一次结束录音；VoiceFlow 将语音转换为文字，经过可选的 AI 整理，再安全地插入当前获得焦点的 App。功能键（⌘ ⌃ ⌥ ⇧ fn）使用双击开始和结束。按 Esc 可取消当前录音。

它适合 Cursor、VS Code、浏览器、邮件、聊天、文档和终端等场景，重点解决三个问题：输入速度、技术词准确性，以及文本交付过程中的可恢复性。

> 当前版本是 macOS 优先的个人项目，最低支持 macOS 13。ASR 使用 Groq 的批量音频接口，不提供逐字实时字幕。

## 界面与交互

VoiceFlow 的设置窗口采用紧凑的双栏布局：左侧负责导航，右侧负责当前设置。整体视觉是低干扰的深色 OLED 风格，也支持浅色模式和跟随系统。

```mermaid
flowchart LR
    A[菜单栏 VoiceFlow] --> B[设置窗口]
    B --> C[录音与输出]
    B --> D[智能整理]
    B --> E[语音服务]
    B --> F[历史记录]
    B --> G[个人词典]
    B --> H[语音片段]
    B --> I[系统设置]
    B --> J[系统权限]
```

### 设置窗口

- 左侧导航按「核心设置」「数据」「系统」分组。
- 「录音与输出」管理全局快捷键、识别语言、长录音分段、输出方式和本地保留策略。
- 「智能整理」管理 App context、写作模式、输出模式和翻译目标语言。
- 「语音服务」管理 Groq API Key、ASR 配置和 AI 文字整理模型。
- 「历史记录」支持查看、重试、导出 JSON、删除单条记录和清空本地数据。
- 「个人词典」支持手动添加词条，以及导入 CSV、TXT、TSV 文件。
- 「语音片段」用于保存和管理可快速展开的常用片段。
- 「系统设置」管理麦克风输入设备和菜单栏图标。
- 「系统权限」集中显示麦克风、辅助功能和浏览器上下文所需权限。

### 菜单栏 HUD

录音时 VoiceFlow 使用一个常驻但不抢焦点的透明浮动 HUD，显示当前状态和语音波形：

- 空闲：保持安静，不遮挡当前 App。
- 录音中：显示实时波形和录音状态。
- 处理中：提示语音识别或文字整理正在进行。
- 完成：短暂显示交付结果。
- 失败或降级：保留文本并提示复制到剪贴板。

### 首次启动引导

首次使用会依次完成：

1. 麦克风权限。
2. 辅助功能权限，用于把文字插入当前输入框。
3. Groq API Key 验证。
4. 全局快捷键设置。
5. 一次完整的试用录音。

## 核心工作流

```mermaid
sequenceDiagram
    participant U as 用户
    participant V as VoiceFlow
    participant G as Groq
    participant A as 当前 App

    U->>V: 按一下全局快捷键并说话
    V->>V: 锁定当前 App、窗口和输入目标
    U->>V: 再按一次快捷键结束
    V->>G: 上传音频进行 ASR
    G-->>V: 返回原始转录
    V->>G: 可选：请求 AI 文字整理
    G-->>V: 返回整理结果
    V->>A: 自动粘贴
    V-->>U: 失败时复制到剪贴板并保存历史
```

### 语音输入

- 组合键按一下切换录音：第一次开始，第二次结束并转换成文字；按 Esc 取消。
- 功能键（⌘ ⌃ ⌥ ⇧ fn）只能双击开始，再双击结束。
- 全局快捷键可在不同 App 中使用。
- 支持自动检测中文、英文和中英混合语音，也可以手动指定语言。
- 长录音会自动分段，避免单次请求过大。
- 如果目标 App、窗口或焦点输入框在处理期间发生变化，VoiceFlow 会停止自动注入，改为剪贴板交付。
- ASR 或粘贴失败时不会丢失结果，原始转录会保留在本地历史中。

### AI 文字整理

AI 整理默认使用 Groq 上的 `openai/gpt-oss-20b`，也可以选择 `openai/gpt-oss-120b`。

整理逻辑会尽量：

- 移除口头禅、重复和明显的语音识别噪声。
- 处理自我纠正，例如「周四，不对，周五」。
- 修正标点、大小写、空格和段落。
- 保留 URL、邮箱、路径、命令、参数、版本号、错误信息和专业术语。
- 保留原始语言和中英文混合表达，不擅自翻译、总结或扩写。
- 在模型请求失败或输出为空时回退到原始转录或本地规则整理。

也可以关闭 AI 整理，此时只使用本地规则，不请求文字整理服务。

### 选中文本操作

选中文本后，可以通过独立快捷键说出操作指令，例如改写、缩短、翻译或总结。VoiceFlow 会将结果交付回当前 App；如果选区或目标发生变化，则只复制结果，不直接替换文字。

## Context-aware 输出

VoiceFlow 可以根据当前 App、窗口、焦点控件和经过授权的浏览器 host 选择写作策略，例如：

- Code
- Search
- Email
- Team collaboration
- Chat
- Document
- Terminal
- Calendar / task
- Form
- Notes
- Social
- General

Context 只作为文字整理的软提示，用户实际说出的内容优先。未知 App 会使用更保守的 General 策略。

默认不会把 raw URL、窗口标题、PID 或文档内容发送给 LLM。浏览器上下文需要单独开启权限，并只保留本地 fingerprint 和 host 信息。

## 隐私与安全

- Groq API Key 存储在 macOS Keychain，不写入 settings JSON，也不返回给前端。
- 语音只在用户主动触发录音后采集。
- 音频只上传到用户配置的 ASR 服务，文字只上传到用户配置的 LLM 服务。
- 当前版本不收集 telemetry。
- 录音目标在开始时锁定；目标改变会触发 fail-closed，结果改走剪贴板。
- history retry 始终是 clipboard-only，不会把旧记录注入到当前可能不同的 App。
- 支持配置恢复音频和历史文字的保留时间，也支持导出、删除和清空。
- 默认 history SQLite 和恢复音频依赖 macOS 文件权限；可选的 recovery spool 应用层加密不会默认开启。

完整数据流和删除说明见 [`docs/privacy.md`](docs/privacy.md)。

## 技术栈

- **Frontend**：React 19、TypeScript、Vite、Tailwind CSS、Lucide React
- **Desktop runtime**：Tauri 2
- **Backend**：Rust、Tokio、Reqwest、SQLite（rusqlite）
- **Audio**：CPAL、Hound、Rubato
- **ASR**：Groq `whisper-large-v3-turbo`
- **LLM cleanup**：Groq `openai/gpt-oss-20b` / `openai/gpt-oss-120b`
- **macOS integration**：Keychain、Accessibility、microphone、menu bar、transparent floating panel

前端通过 Tauri commands 与 Rust 后端通信；音频采集、权限检查、上下文快照、Keychain、网络请求、历史记录和文本交付都由后端负责。

## 本地开发

环境要求：

- macOS 13+
- Node.js
- Rust toolchain
- Xcode Command Line Tools

安装依赖并启动开发模式：

```bash
npm ci
npm run tauri dev
```

常用命令：

```bash
npm run dev          # 只启动 Vite 前端
npm run build        # TypeScript 检查并构建前端
npm run lint         # TypeScript 类型检查
npm test             # 运行前端测试

cd src-tauri
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

构建 macOS 安装包：

```bash
make dmg
```

产物写到 `.build/release/VoiceFlow.dmg`（不用仓库根目录的 `dist/`，那是 Vite 前端输出）。把 `VoiceFlow.app` 拖进 `/Applications`。

签名、第一次打开被拦截、以及为什么以前每次更新都要在系统设置里移出再添加权限，见下面的 [签名与权限](#签名与权限)。

## 签名与权限

macOS 的 TCC（辅助功能等）绑的是**代码签名身份**，不是 bundle id，也不是「看起来还叫 VoiceFlow」。未签名或 ad-hoc（`codesign -s -`）的包按**这一份二进制的哈希**识别。一重新编译或换包，哈希就变了。系统设置里旧条目可能还是开着的，但已经对不上新 copy，只能移出再添加。

这就是以前每次 update 都要重新授权的原因。

### 这个仓库怎么做

默认走**稳定自签证书**（不是 Apple Developer Program，也不是公证）。

| 做法 | 结果 |
|---|---|
| 未签名 / ad-hoc | 绕过 Gatekeeper 后能跑。每次更新权限都会丢。 |
| **稳定自签（本仓库默认）** | 每次 `make dmg`、以及导入同一张 `.p12` 的 GitHub Release，都是同一个身份。权限能保住。第一次打开仍要自己放行。 |
| Apple Development | Xcode 免费证。只在**你这台 Mac**上稳。不能公证。除非导出去，否则不能当 GitHub Release 的身份。 |
| Developer ID + 公证（`make notarize`） | 别人双击就能开，权限也能保住。需要每年 $99 的 Developer Program。 |

不把 ad-hoc 当默认。它最多把「已损坏」变成「无法验证开发者」，**保不住** TCC。

证书名称是 `VoiceFlow`，Team ID 是 `VOICEFLOW1`。designated requirement 变成这张叶子证书，之后用同一张证签的包会继承授权。

**这不是公证。** 别人第一次打开仍会被 Gatekeeper 拦。自签解决不了双击即用。

不要和别的 app 共用这张证。Mac Clippy 用的是另一套身份（`Mac Clippy` / `MCLIPPY001`）。

### 生成一次证书

```bash
make signing-cert
```

macOS 可能会要登录钥匙串密码，以便信任这张证。私钥写在 gitignore 里：

```text
.build/signing/VoiceFlow.p12
.build/signing/VoiceFlow.p12.base64
.build/signing/password.txt
```

不要把这些文件提交进仓库。如果钥匙串里已经有 `VoiceFlow`，脚本会直接退出，不会再做一张新的。

`make dmg` 发现没有这张身份时也会走同一条创建路径，所以第一次打 DMG 也可以顺便建证。

### 构建并安装签过名的 DMG

```bash
make dmg
```

未设置 `APPLE_SIGNING_IDENTITY` 时的选择顺序：

1. `VoiceFlow` 自签（保证本地包和 GitHub Release 是同一个身份）
2. Developer ID Application
3. Apple Development

可用 `APPLE_SIGNING_IDENTITY` / `APPLE_TEAM_ID` 覆盖。`make release` 默认打 `app,dmg`；`make dmg` 只打 DMG。

只保留一份：把权限授给 `/Applications/VoiceFlow.app`。`npm run tauri dev` 仍是未签名。日常用 dev 覆盖运行，权限还是会丢。

### 第一次打开（Gatekeeper）

浏览器或 GitHub 下来的包会带 `com.apple.quarantine`。没有公证时，系统可能说无法验证开发者；若完全未签名，还可能说「已损坏」。

1. 系统设置 → 隐私与安全性 → 仍要打开
2. 或只清这一个 app 的隔离属性：

```bash
xattr -dr com.apple.quarantine /Applications/VoiceFlow.app
```

不要关全机 Gatekeeper。

### 从旧的未签名 copy 迁过来

如果辅助功能开关是开的，但签过名的新包仍说没权限，那是旧的哈希条目还在。清一次再授：

```bash
tccutil reset Accessibility com.voiceflow.desktop
```

用 `VoiceFlow` 这张证签出来的 DMG 覆盖 `/Applications/VoiceFlow.app`，打开后再授权。之后同一张证的更新应能保住。

### GitHub Release

推送 `v*` tag（或在 **Actions → macOS Release** 手动跑）会构建并上传 `.build/release/VoiceFlow.dmg`。

要让别人更新时权限还在，Actions 必须用**同一张**证：

```bash
make signing-cert
gh secret set MACOS_CERT_P12 < .build/signing/VoiceFlow.p12.base64
gh secret set MACOS_CERT_PASSWORD --body "$(cat .build/signing/password.txt)"
```

```bash
git tag v0.1.0
git push origin v0.1.0
```

没有这两个 secrets 时，workflow 仍会发 DMG，但是未签名，每次下载更新都可能要重加辅助功能。

CI **不会**在 runner 上现做一张新证。每次新证都会让所有人的 TCC 悄悄失效。

### 可选：Developer ID + 公证

别人双击、无警告，仍然需要付费账号：

```bash
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name" \
APPLE_TEAM_ID="TEAM_ID" \
make notarize
```

这会走 `scripts/release-macos.sh`（可再加 `VOICEFLOW_NOTARIZE=1` 以及 `APPLE_ID` / `APPLE_PASSWORD`）。和默认的自签 `make dmg` 是两条路。完整勾选见 [`docs/release-checklist.md`](docs/release-checklist.md)。

### 相关脚本

| 脚本 | 作用 |
|---|---|
| `scripts/make-signing-cert.sh` | 生成一次 `VoiceFlow` 身份并导出 `.p12` |
| `scripts/select-codesign-identity.sh` | 优先自签，然后 Developer ID，然后 Apple Development |
| `scripts/resolve-dmg-signing.sh` | 本地缺证就创建；打印 `APPLE_SIGNING_IDENTITY` 和 `APPLE_TEAM_ID` |
| `scripts/import-signing-cert.sh` | Release runner 导入 `MACOS_CERT_P12` |
| `scripts/stage-dmg.sh` | 把 Tauri 打出来的 DMG 拷到 `.build/release/VoiceFlow.dmg` |
| `scripts/select-codesign-identity-test.sh` | 身份优先级的 fixture 测试 |
| `scripts/release-macos.sh` | 可选的 Developer ID / 公证 |

CI 的 macOS job 会跑身份选择测试。

## 项目结构

```text
.
├── src/                 # React 设置界面、HUD 和前端测试
├── src-tauri/src/       # 音频、ASR、LLM、权限、Keychain、历史记录
├── src-tauri/icons/     # 应用和菜单栏图标
├── public/              # 前端静态资源
├── scripts/             # 自签、DMG 发布、可选公证
├── docs/                # 隐私、发布和技术说明
├── package.json         # 前端脚本和依赖
└── src-tauri/Cargo.toml # Rust 依赖和 Tauri 配置
```

## 当前限制

- 目前以 macOS 为主要目标平台，Windows/Linux 尚未作为完整产品体验验证。
- Groq ASR 当前采用批量音频上传，不提供逐字实时转录。
- 自动粘贴依赖 macOS Accessibility 权限；没有权限时仍可复制到剪贴板。
- 浏览器上下文能力依赖用户明确授权。
- 本项目处于早期版本。默认 DMG 使用稳定自签；Developer ID 公证仍是可选的公开分发路径。

## License

License 尚未确定。项目依赖及其许可证说明见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
