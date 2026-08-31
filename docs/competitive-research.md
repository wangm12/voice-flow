# VoiceFlow 竞品与开源对照

更新时间：2026-08-30（8 月 20 日初稿；8 月 30 日补 harness-first；对照代码清理过期「今天」）

范围：系统级语音输入（按快捷键说话 → 文字进入当前 App）。对照闭源产品 Typeless、Wispr Flow、Willow Voice；开源项目 Handy、FluidVoice、VoiceInk、TypeWhisper、OpenWhispr 等；中文引擎 [FunASR](https://github.com/modelscope/FunASR)；纠错学习循环 [Open Typeless Harness](https://github.com/OpenCodexLabs/open-typeless-harness)；以及小红书 / 少数派 / V2EX 等中文用户反馈。

没有自有 ASR / cleanup 模型时，「改完变准」只能靠 harness，不能靠训权重或换「我们的引擎」。完整检索见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。

抄功能 = 抄交互与产品语义，不抄 GPL 源码。TypeWhisper、VoiceInk、FluidVoice 为 GPLv3；Handy、OpenWhispr 为 MIT。本仓库 license 尚未确定。

---

## 结论先行

VoiceFlow 已经具备品类入场券：全局热键、AI cleanup、App context、选区 preview、fail-closed 粘贴、History 可恢复。真正落后的不是「功能清单长度」，而是四件事：

1. **手感**：默认仍是 tap，不是按住说话；真流式 ASR 没有。HUD 已能显示 prefetch 预览字。
2. **发出去像不像你**：检测得到微信 / Slack，但整理强度几乎一样，聊天容易写成书面语。
3. **中文基本功**：人名同音字、标点口述、CJK 粘贴、微信 Mac 注入。Groq Whisper 中文 CER 大约是 SenseVoice / Fun-ASR-Nano 的两到三倍。不和豆包拼「听得准」，但本地中文不要继续绑死 Whisper。
4. **改完还不够准**：同框观察、3× / 人名 2×、最长优先替换、谐音召回、cleanup 后再替换、undo/tombstone、按 App 关学习已经在代码里。没有自有模型，harness 就是护城河；换 SenseVoice / Qwen 只是用户插头。见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。

不要做成会议笔记、联网 Agent 或表情包输入法。

---

## 市场分层

| 产品 | 类型 | 平台 | 引擎默认 | 价格印象 | 定位 |
| --- | --- | --- | --- | --- | --- |
| [Wispr Flow](https://wisprflow.ai/features) | 闭源 SaaS | Mac / Win / iOS / Android | 云端 | Free 2k 词/周；Pro $12–15/用户/月 | 品类标杆：清理 + 词典自学 + snippets + 会议笔记 |
| [Typeless](https://www.typeless.com/) | 闭源 SaaS | Mac / Win / iOS / Android | 云端 | Free 约 8k 词/周；Pro $12/月年付或 $30/月 | 最广平台 + 个性化语气 + Speak-to-edit |
| [Willow Voice](https://willowvoice.com/) | 闭源 YC | Mac / Win / iPhone | 云端，可选离线 | Free 约 2k 词/周；Pro ~$12–15/月 | 按住说话 + 约 200ms 体感 + Scribe |
| [Handy](https://github.com/cjpais/Handy) | MIT OSS | Mac / Win / Linux | 本地 Whisper / Parakeet | 免费 | 与 VoiceFlow 同为 Tauri + Rust + React |
| [FluidVoice](https://github.com/altic-dev/FluidVoice) | GPLv3 OSS | macOS | 本地 Parakeet / Nemotron | 免费 | 延迟标杆 |
| [VoiceInk](https://github.com/Beingpax/VoiceInk) | GPLv3 | macOS Apple Silicon | 本地 + 云 | 源码免费；二进制约 $29–69 | Power Modes |
| [TypeWhisper](https://github.com/TypeWhisper/typewhisper-mac) | GPLv3 | macOS | 本地 + 云（含 Groq） | 免费 + Premium | 功能最全的 macOS OSS |
| [OpenWhispr](https://github.com/OpenWhispr/openwhispr) | MIT OSS | Mac / Win / Linux | 本地或 BYOK | 免费 | 口述 + 会议 + Agent，边界过大 |
| [Typeflux](https://github.com/mylxsw/typeflux) | 中文开源 | macOS | 多 STT + 人设 | 核心开源 | 智谱式人设；V2EX 平替讨论的典型 |
| [FunASR](https://github.com/modelscope/FunASR) | 阿里工具包 MIT | 自托管 / 边缘 | SenseVoice / Paraformer / Nano | 免费（模型许可证各异） | 中文本地 ASR；不要当 Whisper 用 |
| [Open Typeless Harness](https://github.com/OpenCodexLabs/open-typeless-harness) | MIT 实验 | 桌面 | ASR + polish + 回读 | 免费 | 改完变准；不是 Agent |
| [Rhapsode](https://github.com/vishk23/rhapsode) | MIT OSS | macOS | Groq + 本地 fallback | 免费 | FreeFlow fork；谐音纠正 + 能量幻觉；Voice Bank 不抄 |
| [Alowd](https://github.com/nboai2026/alowd) | MIT OSS | macOS | WhisperKit 本地 | 免费 | 粘贴后建议记词，每条人审 |
| [Idiolect](https://github.com/nick-tgcs/idiolect) | AGPL | Linux IME | 本地 Whisper + LoRA | 免费 | **反面教材**：用户音频训权重；我们只要它的 gold 采集思路 |

VoiceFlow 今天是 macOS-only。默认 Groq 批量 ASR + tap 录音 + `llama-3.1-8b-instant` cleanup；转写/润色可换服务商或本机端点；可选 hybrid 热键；context 写作策略、选区 preview、fail-closed 粘贴。

命名提醒：另有 chatbot 产品 Voiceflow；Wispr 也叫 Flow。公开分发时要评估品牌冲突。

---

## VoiceFlow 已经领先、不要丢掉

- 目标锁定 + fail-closed：处理中目标变了只走剪贴板。见 [end-to-end-workflows.md](end-to-end-workflows.md)。
- 粘贴可验证：`Cmd+V` 成功不等于控件收到。
- 3 秒 Undo：目标 stale 绝不盲目 `Cmd+Z`。
- 选区 preview-first：比竞品「直接改写」更安全。
- Protected facts + cleanup corpus。见 [cleanup-evaluation.md](cleanup-evaluation.md)。
- History retry 只复制。
- Keychain 存 key；不把 raw URL / 窗口标题 / PID 发给 LLM；无 telemetry。
- Screen assistant 明确未上线，避免半成品截图上传。

能抄手感，不能抄「更快地盲粘贴」。

---

## 闭源三家

### Typeless

核心是「说完就是可发送的文字」，不是逐字转录。

值得抄：filler / 改口处理、短信 vs Gmail 的词汇选择、悄悄话、Speak-to-edit、中文标点口述（括号、顿号、引号）。

不要默认抄：Ask Anything 联网动作、跨端、把屏蔽脏话当聊天卖点。

### Wispr Flow

品类定义者。用户买的是「不用说标点、说完就能发」。

值得抄：cleanup 强度档（None / Light / Medium / High）、改正即学词典、snippets、按 App 换风格、低语。

明确不做：会议 Notetaker、MCP 笔记、团队共享词典。

### Willow Voice

默认交互是按住说话、松开即贴；双击 hands-free。Scribe 是独立热键：说意图，写出完整回复。

值得抄：Hold / Hybrid 热键、Scribe 与口述拆开、约 200ms 的体感目标、auto-learn dictionary。

Scribe 在聊天 App 里仍应偏口语，不要写成客服稿。

---

## 开源该看谁

### Handy（技术栈 twin）

Manager 拆分、热键 coordinator、Secure Input 警告、本机 Whisper / Parakeet、VAD、overlay Minimal vs Live。抄实现纪律，不必抄「几乎没有 AI cleanup」的产品定位。

### TypeWhisper（功能最全）

Hybrid 热键、按 App / 网站 Workflows、词典自学、snippet 占位符、AX 直接插入再回退 Cmd+V、HUD 多种指示器。现在不要抄插件市场、日历会议、Widgets。

### VoiceInk

管线：`ASR → filter → 段落 → 词典 → 可选 LLM`。Power Mode 把「口述」和「增强」拆开。流式失败回退 batch。

### FluidVoice

目标是松开后几乎立刻出字。可选路径：本地流式草稿 + Groq/LLM 最终 cleanup。不要依赖其闭源 Fluid Intelligence runtime。

### 中文社区平替

Typeflux（人设）、OpenTypeless、各类 Voice Paste。中文用户要 BYOK + 按场景 prompt，不是再交一份云订阅。

### 2026 后补的 harness 对照

[Rhapsode](https://github.com/vishk23/rhapsode)（确定性谐音 + 能量幻觉）、[typwrtr](https://github.com/kaidhar/typwrtr)（tombstone 后 1×）、[TalaX](https://github.com/puretensor/talax-dictation)（3× + L3 谐音）、[voice-typed](https://github.com/nikhilm55/voice-typed)（LLM **之后**再替换）、[Alowd](https://github.com/nboai2026/alowd)（人审建议）、[YazSes](https://github.com/MSKazemi/yazses)（配置 diff，不学击键）。[Idiolect](https://github.com/nick-tgcs/idiolect) 用用户音频训 LoRA——那是「有自己的模型」路线，整条不抄。分层和论文见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。

---

## 阿里 FunASR：中文引擎怎么接

[FunASR](https://github.com/modelscope/FunASR) 是阿里达摩院 / ModelScope 的工具包，不是「又一个 Whisper」。工具包 MIT，**模型许可证各异**。公开数字（中文 CER）：Whisper-large-v3 大约 20%；SenseVoice-Small / Fun-ASR-Nano / Paraformer 大约 8–10%，CPU 也能跑到十倍实时。SayIt 作者的中文体感排序：豆包 > Typeless ≈ Qwen3-ASR > FunASR-Nano >= FireRedASR2 > Whisper。

### 对 VoiceFlow 有用的能力

| 模型 | 适合 | 热词 |
| --- | --- | --- |
| SenseVoice-Small | 中英日韩粤，CPU 约 17x 实时，体积小 | 谐音 FST（`lexicon.txt` + `replace.fst`），不是 beam-search 热词 |
| Paraformer | 低延迟流式；自带 FSMN-VAD + ct-punc 标点 | 工具包热词；桌面侧看 sherpa-onnx 是否暴露 |
| Fun-ASR-Nano | 中英日 + 方言口音 | sherpa-onnx 已支持写进 user prompt |

聊天档可以走「ASR + 自带标点、跳过 LLM」。少过一层整理，从根上减少微信被写成邮件。Paraformer 流式也可以喂 HUD 进行中的字，比 Groq prefetch 更接近真流式。

`funasr-server` 已提供 OpenAI 兼容 `/v1/audio/transcriptions`，和现有 Groq 客户端同一形状。

### 不要把 Python FunASR 塞进 Tauri

完整工具包是 Python + 模型下载。桌面只走两条路：

1. **先做 BYOK sidecar**：本机 `funasr-server`，或豆包 / 千问 / 阿里云 ASR。VoiceFlow 多一个 OpenAI-compatible endpoint，和 Groq 并列。
2. **再做 sherpa-onnx 进程内**：k2-fsa 已有 [Tauri 麦克风示例](https://k2-fsa.github.io/sherpa/onnx/tauri/vad-asr-mic.html)，模型含 SenseVoice、Paraformer、Fun-ASR-Nano、Qwen3-ASR。下载 ONNX 到 app data，Rust 调 C API。这才是真正离线。

不要：在 app 里嵌 Python；默认用用户音频微调权重；把情感标签当产品功能。

学到的词典必须能灌进热词，不能只给 LLM。`晓雯`、`知乎` 要在识别阶段就偏置。

---

## Voice Harness：学习用户，但不要变成键盘记录

指的是 [Open Typeless Harness](https://github.com/OpenCodexLabs/open-typeless-harness)（MIT，实验项目）这类循环，不是再做一个 Agent：

`ASR → 用已有 speech skills 做 polish → 插入当前输入框 → 短窗口观察用户怎么改 → 稳定纠错晋升为本地 skill`

同类：TalaX（同一替换出现 3 次才自动生效）、typwrtr（粘贴后用 UI Automation 回读同一控件）、YazSes 论文（加密本地语料，tuner 只产出配置 diff，**明确不用击键记录**）。Wispr / Willow 的改正即学词典是闭源版同一需求。

### VoiceFlow 已经有主循环，不是半截

2026-08-30：同框观察、短短语、发送即提交、token 预算排序、最长优先替换、undo/tombstone、按 App 关学习、History 确认芯片、cleanup 后再替换、谐音召回、分类器 + 人名 2×、1Password/HR 预设关学习、Whisper 虚构抄本已在 [`dictionary_learn.rs`](../src-tauri/src/dictionary_learn.rs) / [`lexicon.rs`](../src-tauri/src/lexicon.rs)。Cleanup 只带本句 hit pairs，未命中不再回退 dictionary。

没有自有模型，就不能靠 LoRA / 微调 / 换「我们的引擎」补这两刀。换 SenseVoice / Qwen 是用户插头；同一张 `learn_pairs` 按对方 API 变形即可。

粘贴后只延长「自己刚写入的那个框」的观察窗，不新开全局监听。Secure Input / 关学习则不观察。

### 该学什么

| 置信 | 例子 | 去向 |
| --- | --- | --- |
| 高，可自动晋升 | `知呼 → 知乎`、`晓雯`、`配森 → Python`、`type script → TypeScript` | 本地替换 + Whisper prompt 偏置；FunASR 热词要等用户选了该插头 |
| 低，设置里待审 | 同一 App 里反复出现的短句口气、标点习惯 | 人审后再进 few-shot |
| 永不自动学 | 大段改写、扩写、变正式 | 会把微信再次写成邮件 |

不学：全局击键、密码框、Secure Input、目标已变之后的编辑（沿用 `stale_target`）、上传云端「越来越像你」。

晋升规则抄 TypeWhisper / TalaX：重复出现的单词语或短短语（拉丁 2–4 词、CJK 2–8 字）默认 3× 自动晋升。分类器认作 2–3 字中文人名则 2×。不要 1× 静默晋升。

Harness 学的是**词汇和口癖**；writing mode 管的是**这一次可以改多狠**。微信档即使已经认识 TypeScript，也仍然禁止加「您好」。

---

## 小红书与中文用户

2026-08-20 打开了小红书搜索 [语音输入 Typeless](https://www.xiaohongshu.com/search_result?keyword=%E8%AF%AD%E9%9F%B3%E8%BE%93%E5%85%A5%20Typeless) 与笔记 [这是我见过最文明的语音输入法](https://www.xiaohongshu.com/explore/693107a4000000001e00efa3)。辅以少数派、V2EX、虎嗅、Play 商店中文评测。

### 搜索页反复出现的主题

| 信号 | 含义 |
| --- | --- |
| Typeless vs 微信语音输入法 | 国民级对照是微信，不是 Superwhisper |
| 「最文明的语音输入法」 | 屏蔽脏话 / 过度润色被当卖点，也是槽点 |
| 停用 Typeless，国产 AI 输入法到底行 | 价格敏感，豆包 / 千问随时能替 |
| 开源平替、零成本替代（高赞） | BYOK 是明确需求 |
| Qwen Omni / 「狗家杀死比赛」 | 国内大模型免费入口是威胁 |
| Typeless + Codex / Claude `/voice` | 重度场景是 vibe coding |
| 相关搜索「微信语音输入和 typeless 哪个好」 | 聊天场景是决策点 |

「最文明」笔记正文：悄悄话、自动润色、自动生成邮件格式、说「括号 / 斜杠 / 引号」出符号、自动屏蔽脏话。评论：中文灵吗；12 美金就为个输入法；速度慢还会打乱已有编辑；麦克风图标常亮碍眼。

### 微信 / Slack 太正式

这是中文评测里最高频的「用了也不敢发」：

- 虎嗅：跟同事沟通过于干净，缓冲被删，对方觉得不够有人味；用户切回键盘补口语、表情、笑声。结论：这类工具更适合跟 AI 说话，不适合闲聊。
- 拾穗：「这什么破需求」被改成「这个需求有待商榷」。聊天场景这是产品事故。
- 少数派：短信保留 `kinda wanna`，Gmail 改成书面语——这是**想要的差异**。
- Wispr 中文评测：LINE / Messenger 口语，Notion / Docs 正式。
- Play 商店：Wispr 中文不如免费豆包，界面只有英文。
- V2EX：贵、首尾吞字、标点难说、麦克风不释放、Fn 被微信劫持、CJK 输入法拦截 Cmd+V。

### VoiceFlow 代码缺口（语气 / 粘贴，不是 harness 循环）

[`src-tauri/src/context.rs`](../src-tauri/src/context.rs) 已把微信映射到 `PersonalChat`（`formality=casual`）、Slack / Teams 映射到 `WorkChat`（`neutral`）。HUD 已显示「微信 · 口语」。中文标点口述（顿号 / 冒号 / 分号 / 破折号）已在 [`spoken_punctuation.rs`](../src-tauri/src/spoken_punctuation.rs)。

仍弱的是：两条聊天档都走 `CleanupEffort::Light`，few-shot 还不够像「贴一条你的微信」；粘贴没有「CJK 先切 ABC」。不要再把「prompt 完全一样 / HUD 没语气 / 没有标点口述」当缺口。

### 按 App 语气应长成什么样

三层，用户能看见：

| 目标 | 强度 | 允许 | 禁止 |
| --- | --- | --- | --- |
| 微信 / iMessage / Discord | 最轻 | 去嗯啊、改口只留最后一句、最少标点 | 加「你好」、扩成完整句、和谐脏话 |
| Slack / 飞书 / Teams | 轻 | 短句、可列表、专有名词 | 变成邮件、加签名 |
| Gmail / 文档 | 中到强 | 成段、列表、用户说过的问候 | 编造未说事实 |
| Cursor / Terminal | 几乎关 | 只修 ASR 谐音 | 任何润色 |

HUD 应显示「微信 · 口语」。微信 few-shot 应是「好的哈哈我晚点回你」，不是「好的，我会稍后回复您。」

---

## 能力对照

| 能力 | VoiceFlow | Typeless | Wispr | Willow | 开源标杆 |
| --- | --- | --- | --- | --- | --- |
| Hold PTT | 可选 hybrid；默认 tap | 有 | 有 | 默认 | 几乎全部 |
| Toggle / hands-free | 有 | 有 | 有 | 双击 | TypeWhisper Hybrid |
| 选区语音改写 | preview-first | 直接改 | Command mode | Scribe edit | VoiceInk |
| 意图起草 | 无 | Ask anything | 弱 | 独立热键 | VoiceInk Email mode |
| App 语气 | 检测有、HUD 有标签；两边都是 Light | 有 | Styles | Style-matching | Power Modes / 人设 |
| 词典 | 自动 3× + 手动 + pin | 自动 + 手动 | 改正即学习 | 自动学习 | TypeWhisper |
| Snippets | 有 | 无 | 有 | Shortcuts | TypeWhisper 占位符 |
| 流式 HUD | prefetch 完成块可进 HUD | 宣传 real-time | 有 | ~200ms | FluidVoice |
| 本地 ASR | local Whisper + 自定义 FunASR / MLX 端点 | 无 | 无 | 可选 | Handy / FluidVoice；中文应看 FunASR / SenseVoice，不要本机 Whisper |
| 多 provider | Groq / OpenAI / Deepgram / SiliconFlow / DeepSeek / Anthropic / Ollama / 本机 / 自定义 | 云闭源 | 云闭源 | 云 + 离线 | TypeWhisper / Handy |
| 纠错学习 | 同框观察 + History 确认 + 谐音 + post-LLM 替换 | 自动 + 手动 | 改正即学 | 自动学习 | TypeWhisper；Open Typeless Harness 短窗口回读 |
| 交付安全 / Undo | 强 | 弱宣传 | 弱宣传 | 弱宣传 | TypeWhisper AX |
| 会议笔记 | 无 | 无 | 有 | 无 | OpenWhispr |

---

## 建议优先级

产品定义保持：**macOS 系统级语音输入，可恢复、可验证、context 感知。** 中文差异化是系统级 + 按 App 真换语气，不是和豆包拼识别率。

### 已落地（不要再当缺口）

- Hybrid 热键：`tap` / `double_tap` / `hybrid`；旧 `hold` 组合键迁到 hybrid。默认仍是 tap。
- HUD prefetch 预览字：只进 HUD，不进目标 App / 剪贴板 / History。
- 中文 ASR 插头：SiliconFlow SenseVoice、自定义 Qwen3-ASR / FunASR；同一张 `learn_pairs` 按 Whisper / Qwen·SenseVoice / Deepgram 变形。
- Harness 主循环：cleanup 后再跑 lexicon、谐音召回、分类器 + 人名 2×、Whisper 虚构抄本。详见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。
- 粘贴后短窗口学习、mapping「在这个 App 学习」、1Password / HR 预设关学习。
- 中文标点口述：顿号 / 冒号 / 分号 / 破折号。

### 仍弱

1. **按 App 真正换语气**：WorkChat / PersonalChat prompt 已拆，HUD 有标签；few-shot 还不够像「贴一条你的微信」，两边默认仍是 `CleanupEffort::Light`。
2. **Harness 打中率**：循环在，缺独立 review 窗、搜狗导入、style draft held-out。
3. 粘贴链：AX → CJK 切 ABC 后 Cmd+V → 剪贴板。重点回归微信 Mac。
4. Fun-ASR `vocabulary_id` / sherpa FST（等用户选了那种 API）。
5. Snippet 占位符。
6. 录音反馈、释放麦克风、Secure Input 提示、低语 VAD、Fn 与微信输入法冲突提示。

### 以后

- Scribe 独立热键，preview-first；聊天里仍偏口语。
- Screen assistant，按现有安全清单，默认关。
- Apple Speech / Parakeet 作为英文离线备选（中文主路径是 FunASR 系）。

### 不抄

会议笔记、日历自动开录、MCP 笔记、iOS / Android 完整产品、联网 Ask Anything、插件市场、把用户风格上传云端、Command Mode 控电脑、聊天默认表情包、聊天默认屏蔽脏话、全局击键监听、在密码框或 Secure Input 里学习、用用户录音微调 FunASR 权重、在 Tauri 里嵌 Python FunASR 全家桶。

2026-08-30 研究后的完整边界见 [asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md)。质量标杆改为 Typeless 中英混合，不把「跳过微信 LLM」当长期方向。

---

## 若开始做，改哪里

- 热键：`src-tauri/src/hotkey.rs`、`modifier_hotkey.rs`、`ActivationModeSelector.tsx`（hybrid 已在）
- 语气：`src-tauri/src/context.rs` 的 `default_writing_prompt`；mapping `cleanup_effort`；设置里的 style example
- HUD 预览：`prefetch_asr.rs` 的 HUD-only 通道已在；真流式另立项
- 词典 / harness：`dictionary_learn.rs` + `lexicon.rs`。后做 review 窗 / 搜狗导入，不新开 event tap。完整对照见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)
- ASR：`providers.rs` / `engine.rs`；sherpa-onnx 和 Fun-ASR `vocabulary_id` 仍后做
- 粘贴：`paste.rs` 增加 AX + CJK 输入源切换

相关工作流与隐私边界见 [end-to-end-workflows.md](end-to-end-workflows.md)、[privacy.md](privacy.md)。
