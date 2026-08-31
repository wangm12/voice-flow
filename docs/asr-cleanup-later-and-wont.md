# ASR / Cleanup：以后再做，以及明确不做

更新：2026-08-30

来源：竞品与开源研究（Typeless / Wispr / Willow / MacWhisper / Daisy / yw-transcribe / VoiceInk / Handy / TypeWhisper）。质量标杆是 **Typeless 的中英混合听写 + 整理**。2026-08-30 周期的引擎插头和 cleanup prompt 已经落地；下面两张表仍是边界，避免下次会话又把 Agent / 真流式 / 会议笔记塞进听写 PR。

对照实现计划：[2026-08-25-asr-cleanup-pipeline.md](superpowers/plans/2026-08-25-asr-cleanup-pipeline.md)。旧分层见 [competitive-research.md](competitive-research.md)。隐私红线见 [privacy.md](privacy.md)。

---

## 研究过、故意留到后面

这些都值得做，但**不要**和已落地的 prompt / SenseVoice 插头混在同一个 PR。单独立项。

### Daisy 控 Mac（打开应用 / 备忘录 / 日历）

Daisy 是语音 **Agent**，不是听写。参考：`forestai123456/Daisy-Voice-Agent` 的 `src/main/command/router.ts`、`src/main/control/macos.ts`。

以后若做：独立热键，文案写「命令」不写「听写」。本地正则先匹配（打开/退出/音量/站内搜索 + 中文别名），再用很小的工具集（Notes / Reminders / Calendar 的 AppleScript）。打字复用 VoiceFlow 的 AX 粘贴，不要抄 Daisy 的 `System Events keystroke`。

不要把「打开微信」做进默认听写松手路径。

### 真流式 ASR（Daisy / Willow / FluidVoice）

Daisy：豆包 WebSocket，100ms 帧，orb 上出 partial。Willow：边说边传，宣传约 200ms 出字。FluidVoice：本地流式草稿。

**已上线：** prefetch 完成块可以进 HUD（最多约 280 字），不进目标 App / 剪贴板 / History。

以后：真流式 ASR（WebSocket / 本地流式草稿）。当前不嵌 Daisy 的二进制 WS 协议。

### Wispr 用户可见的 None / Light / Medium / High 滑条

Wispr 设置里是全局整理强度。我们内部已有 `CleanupEffort::Light | Standard | Command`。以后再做成设置里用户能看见、能改的四档；HUD 显示当前档。

不要加新滑条；内部档位已经够用。

### 改正即学词典（循环已在；缺打中率）

Wispr：改正一次、≤4 词、分类器过滤。Willow：中文人名/声调记入个人词典。TypeWhisper：按目标 App 纠错学习。Open Typeless Harness：粘贴后短窗口回读。

**已上线：** History 确认、同框观察、3× 晋升、最长优先替换、undo/tombstone、按 App 关学习。只观察自己刚写入的框，禁止全局击键。

**已落地（harness，不是模型）：** post-LLM 再替换、拼音/谐音召回、分类器后的人名 2×、1Password/HR 预设关学习、Whisper 虚构抄本。没有自有模型时只能靠这些，不能靠 LoRA / 微调 / 换「我们的引擎」。见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。

不要重做观察循环。下一刀如果要做，是打中率和 review 窗，不是再写一套学习。

### TypeWhisper 按网站 workflow

TypeWhisper：Chrome + github.com 可以盖过「所有 Chrome」。我们已有 bundle / host mapping。

以后：browser host 模式匹配（`github.com` 含 docs/gist），每个 workflow 可覆盖语言、引擎、prompt。已有 mapping 已能在设置里选；不新建 workflow 引擎。

### 默认改成按住说话

**已上线：** 设置里可选 `hybrid`（短按切换、按住说话）。组合键的旧 `hold` 会迁到 `hybrid`；修饰键仍只能双击。

默认仍是 tap。以后若要把默认改成按住，单独立项，不要和引擎 / prompt 混做。

### Volcengine 正式接入

Daisy 流式 `bigmodel` + `enable_itn` / `enable_punc`。yw-transcribe 文件识别 Seed ASR 2.0（`volc.seedasr.auc`，Speech Key ≠ Ark Key）。

**已上线：** SenseVoice / Qwen3-ASR 的 OpenAI-compat 预设。不把豆包 WS/submit-query 做成一等 provider。

### 500ms 预滚

Daisy 本地 Whisper：VAD 前保留 500ms，避免吃掉句首。以后若做本地 VAD 停录再加。当前 batch「松手再传」不需要。

### 其它以后（研究里提过、仍不本轮）

- Willow Scribe 第二热键（选区改写已经能覆盖「说意图改选区」）
- Handy Secure Input 提示
- yw-transcribe 式 `raw.json` 不可变证据包（听写产品不需要字幕包）

---

## 明确不做

不要立项，不要「顺便做一点」。

| 不做 | 为什么 | 常见伪装 |
|---|---|---|
| Typeless / Wispr **Ask Anything** 联网动作 | 听写会变成 Agent，延迟和安全模型都变 | 「让 LLM 搜一下再贴」 |
| **截图进 LLM**（Wispr `screenshot` / screen assistant） | 隐私；窗口里有邮件和密钥 | 「看看屏幕上的字好整理」 |
| **会议录音** / 日历自动开录（MacWhisper / Wispr Notetaker） | 另一条产品，不是松手粘贴 | 「顺手录 Zoom」 |
| Daisy **`run_shell_command`** | 任意命令 = 不可接受的桌面权限 | 「工具失败就用 shell 兜底」 |
| 在 Tauri 里 **嵌 Python FunASR** | 体积、崩溃、模型下载都变成我们的运行时 | 「本机中文准就要带 Python」 |
| 用用户录音 **微调 ASR 权重** | 隐私；不是词典学习 | 「越用越准」若指出租权重 |
| **全局击键** / 密码框 / Secure Input 里学习 | 键盘记录 | 「粘贴后看用户怎么改」若变成全局 listen |
| 把用户风格 **上传云端** | 和 Keychain-only、无 telemetry 冲突 | 「云端越来越像你」 |
| 聊天默认 **屏蔽脏话** 或默认表情包 | 中文用户会觉得假 | 「最文明的输入法」 |
| 抄 **GPLv3** 源码（VoiceInk / TypeWhisper / FluidVoice） | 许可证 | 「把他们的 prompt 文件贴进来」 |

Daisy 的 Notes / Calendar AppleScript 可以以后做；**不要**带着 `run_shell_command`、Firecrawl、Control Center 刮「勿扰」一起做。

---

## 2026-08-30 周期已落地

计划已收窄并写进代码。**不区别对待微信**：短句也整理，和邮件/Slack 同一套 punct/filler/不对。

引擎预设是 **插头**（用户选 SenseVoice / Qwen 时用好 OpenAI-compat），不是自学方案。Harness P0（post-LLM 替换 + 谐音召回）已在，和选哪个引擎无关。

- SiliconFlow SenseVoice、自定义 Qwen3-ASR / 本机 FunASR 预设（现有 OpenAI-compat，不假装传 Daisy ITN）
- `skips_llm_scene`：LocalOnly 只剩 Terminal / 表单 / 用户关整理
- 窄 tagged prompt + `local_cleanup` 安全去「那个/就是说」
- 口述 冒号/分号/破折号
- 默认 cleanup 模型 `llama-3.1-8b-instant`

**验收：**

- Gate 1（合并）：`cargo test` / `npm test` 证明路由、filler、口述符号、prompt 标签。**不能**证明听得准或 llama 整理得好。
- Gate 2（准确度）：仍要 owner 实听 6 句（微信两条、邮件/Slack、Cursor、中英混合、晓雯/知乎）。打分前不声称 accuracy。

未做、且不要混进下一刀：光标 AX 进 LLM、mapping UI 重建、Volcengine、`evals/` live harness 当合并门槛、本机 MLX 一等 provider（用自定义端点即可）。

若要开「以后」清单，一次只开一项，并另写计划。
