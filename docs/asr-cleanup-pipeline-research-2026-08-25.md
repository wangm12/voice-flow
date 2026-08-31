# ASR → AI Cleanup 管线研究（2026-08-25）

> **快照，不是现状。** 写于 2026-08-25。文中「今天 / 默认 gpt-oss / 聊天跳过 LLM / 没有幻觉袋」已过期。
>
> 当前代码：默认 cleanup 是 `llama-3.1-8b-instant`；LocalOnly 只剩 Terminal / 表单；prefetch 完成块可进 HUD；多服务商 ASR/LLM；`spoken_revision` 与 post-LLM lexicon 已在。活文档见 [README.md](README.md)、[asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md)。

范围：从麦克风到可粘贴文字的整条链路。对照闭源 Typeless / Wispr Flow / Willow Voice，以及 2026 年一批开源听写项目。目标不是再抄一份功能清单，而是解释**为什么 VoiceFlow 用起来仍对不上**，以及每一层具体该怎么改。

本文基于当前仓库代码（`src-tauri/src/{audio,chunker,asr,prefetch_asr,spoken_punctuation,spoken_layout,lexicon,llm,context,lib}.rs`），以及 2026-08-25 的公开资料。不宣称对 Groq 做过新的 live 评测。

相关旧文：[competitive-research.md](competitive-research.md)（产品分层，2026-08-20）、[cleanup-evaluation.md](cleanup-evaluation.md)、[.superpowers/sdd/harness-competitive-compare.md](../.superpowers/sdd/harness-competitive-compare.md)、[.superpowers/sdd/cleanup-three-layer-audit.md](../.superpowers/sdd/cleanup-three-layer-audit.md)。旧文已经把「手感 / 语气 / 词典学习」说清楚了。本文补的是**识别引擎与整理模型本身**——这才是质量对不上的主因。

---

## 1. 结论：对不上的不是功能，是两台机器

VoiceFlow 今天已经有：全局热键、App context、词典、lexicon 替换、口述标点/换行、prefetch HUD、选区改写、fail-closed 粘贴、History。功能清单并不短。

用户拿它和 Typeless / Wispr / Willow 比，感觉「完全不能匹配」，通常是这四件事同时发生：

1. **中文先听错。** 默认 ASR 是 Groq `whisper-large-v3-turbo`。Whisper 系在中文对话、方言、噪声上的 CER/WER 大约是豆包 / Qwen3-ASR / SenseVoice 的 **2–4 倍**。Cleanup 再强，也救不回没进过 beam 的「晓雯 / 知乎 / 配森」。
2. **整理用的是通用推理模型，不是听写专用 edit model。** Wispr 微调过 Llama，专门做 transcript cleanup，100+ token 要在 250ms 内出完，p99 端到端 <700ms。Willow 有 Frontier Pro / Mini 专用 edit pass。VoiceFlow 用 `openai/gpt-oss-20b` + 近 2000 字的 SYSTEM_PROMPT，还开了 `reasoning_effort=low`。模型在「删改口」和「不要总结、不确定就留原文」之间摇摆，结果就是直译。
3. **管线是停说 → 上传整段 → 等 ASR → 等 LLM → 粘贴。** Willow 边说边传；Wispr 把 ASR 和 cleanup 叠在同一条 inference chain 里。VoiceFlow 的 prefetch 只是 10 秒一块的假流式，短句仍然付两次完整 RTT。
4. **确定性层太薄，LLM 几乎每句都上。** `decide_cleanup` 在 `cleanup_enabled=true` 时对聊天也走 Provider。本地 `local_cleanup` 只会剥 `uh/um/嗯/啊`。没有静音裁剪、没有幻觉短语袋、没有改口状态机、没有中文标点模型。

抄功能解决不了这四件事。要匹配的是：**中文先听对，整理用专用/快速 instruct，确定性层先做完再决定要不要 LLM。**

不要做成会议笔记、联网 Agent，或和豆包拼「听得准」的云订阅。中文差异化仍然是：系统级 + 按 App 真换语气 + BYOK / 本地中文引擎。

---

## 2. VoiceFlow 今天真实在跑什么

短录音路径在 `process_short`（`lib.rs`）。长录音把同一套后处理套在分块上。

```text
热键 → 锁 App/窗口/焦点
     → 16 kHz PCM + 增益软限幅
     → 能量 VAD 只用于 15–60s 分块（默认 35s，1.5s overlap）
     → 录音中：10s warmup 块丢给 Groq batch（HUD 最多 280 字，不进剪贴板/History）
     → 松手：整段 WAV POST /audio/transcriptions
            model = whisper-large-v3-turbo
            prompt ≈ 200 token（词典 + 「不要翻译」种子）
            response_format = verbose_json + word/segment timestamps
     → spoken_punctuation（必须说出「逗号/句号/…」）
     → spoken_layout（必须说出「换行/第一/1.」等）
     → lexicon 替换（已晋升且在 dictionary 里的 before→after）
     → 解析口述意图（改写/缩短/翻译…）
     → 命中 snippet 则跳过 LLM
     → 否则：只要 cleanup_enabled，就调用 gpt-oss-20b
            temperature=0, reasoning_effort=low, max 4096 tokens
     → 保护 token / 语种检查失败则回退原文
     → 粘贴（AX → Cmd+V → 剪贴板）
```

`AsrCapabilities.realtime_streaming = false`。Groq 客户端是文件上传，不是 WebSocket。

### 2.1 音频层

| 项 | 现状 | 问题 |
| --- | --- | --- |
| 采样 | 重采样到 16 kHz mono WAV | 对 Whisper 正确 |
| 增益 | 用户滑条 + soft limit | 没有 AGC，低语容易饿死 |
| VAD | `chunker.rs` 能量 RMS，阈值 `max(noise*2.5, 0.01)` | 只切长录音块，**不裁首尾静音，不挡纯静音上传** |
| 静音 | ASR 后：>80% segment 的 `no_speech_prob > 0.9` 才当空 | 听写场景 0.9 太松；单段幻觉进正文 |
| 时间戳 | 已经向 Groq 要了 word/segment | **`process_short` 只用 `.text`，丢掉 logprob / no_speech / words** |

Whisper 对静音的经典幻觉是 `Thank you for watching` / `Sous-titres par…`。OpenWhispr 在 Groq 上复现过：30s decode 窗口附近尖峰。VoiceFlow 没有 leading/trailing trim，也没有幻觉短语袋。短句前后半秒思考停顿，就会变成「多出来的一句」。

Groq 文档：短于 10 秒仍按 10 秒计费；部分资料写 Whisper 服务端会把短音频 pad 到约 30 秒。Pad 静音 = 幻觉温床。客户端必须自己裁。

### 2.2 ASR 层

默认：`whisper-large-v3-turbo`（Whisper-large-v3 剪到 4 层 decoder）。Groq 公开数字大约是 multilingual WER 12%、最高约 247x 实时。这是**英文播客口径**，不是中文听写口径。

热词：`lexicon::build_asr_prompt` 已经按 ~200 token 做了、高优先级放后面（防 Whisper 从前面截断）。这是正确的 Whisper 用法。但它仍然只是 **soft decoder bias**。音频和 prompt 打架时，音频赢。`知呼` 不会因为 prompt 里有 `知乎` 就稳定变成 `知乎`。

语言：UI `auto` 不会发给 Groq。中英混合时 Whisper 经常把中文译成英文，或把专有名词拆开。种子句 `不要翻译。这个 API 的 latency 太高了。` 能压一点，压不住同音字。

Custom provider 已经能接 OpenAI-compatible `/v1/audio/transcriptions`（engine wizard）。**产品默认路径仍是 Groq Whisper。** 用户不自己填豆包 / funasr-server，就永远走弱中文引擎。

### 2.3 确定性后处理

三层，都偏「口令」而不是「听写默认」：

1. **口述标点**（`spoken_punctuation.rs`）：独立词 `逗号/句号/顿号/括号/引号` 和英文 `comma/period`。没说就不标。
2. **口述版式**（`spoken_layout.rs`）：`换行/新段落/第一/first/bullet`；`1.` `2.` `3.` 最近才部分覆盖。没说结构词就不分段。
3. **词典替换**（`lexicon.rs`）：最长优先、拉丁词边界、CJK 不嵌进 blocker。只应用 **已晋升且 after 在 dictionary 里** 的 pair。

`local_cleanup`（LLM 失败或 LocalOnly 时）：英文 `uh/um/you know/I mean`，中文独立 `嗯/啊`，以及独立的 `那个/就是说`（不吃掉「那个项目」「我就是这个意思」）。没有 `然后`，没有 ITN（四十二 → 42）。

### 2.4 Cleanup 路由（2026-08-30）

`lexicon::decide_cleanup`：

- 全局或 per-mapping 关整理 → `LocalOnly`
- 空白 transcript → `LocalOnly`
- 口述命令 → `Command`
- mapping.effort 若设置则覆盖
- **Terminal / FormFilling → `LocalOnly`**
- context 置信 <0.75 → `Light`
- 否则 family 默认：聊天/笔记/社交/代码 = Light，邮件/搜索等 = Standard

**短句不再 LocalOnly。** 微信 `好的哈哈我晚点回你` 走 Light，和邮件同一套 punct/filler/不对。不要把微信当成特例。

### 2.5 LLM 层

`SYSTEM_PROMPT` 是一篇法律文书：先 resolve 改口，又说不要 summarize，又说分不清就两句都留，又列了一堆 不对 / scratch that / actually 的例外。Light 只是在 user 末尾再贴一段。同一套 system，同一台推理模型。

请求参数：`temperature=0`，`reasoning_effort=low`，`max_completion_tokens=4096`。gpt-oss 即使 low reasoning 也会先想再写。Wispr 的目标是 **100+ 生成 token / 250ms**。推理模型天生对不上这个预算。

User 消息堆了：Intent、app_label、family、confidence、Style 五字段、writing prompt、output mode、style example、profile guidance、Must preserve、Transcript、optional spoken-layout 注、词典 pair、Effort。模型要同时当律师、编辑、翻译开关。结果偏向保守直译——和 corpus 审计里「cloud → 不对不对不对 → Cursor」失败态一致。

---

## 3. 闭源三家实际在卖什么（2026 更新）

### 3.1 Wispr Flow：专用 ASR + 微调 Llama，p99 <700ms

[Baseten 案例](https://www.baseten.co/resources/customers/wispr-flow/)（官方工程叙述，不是营销页）：

- 整条链在 AWS 上的 Baseten：**ASR 和 cleanup 是两条可独立扩缩的模型**，用 Chains 叠在一起，避免「ASR HTTP 返回后再另开 LLM 连接」。
- Cleanup 是 **微调过的 Llama**，不是通用 chat。「写出你会打的字」，按 App / 用户偏好可控。
- 延迟目标是 **p99 端到端 <700ms**，不是 p50 TTFT。Llama 一段要稳定打出 100+ token / <250ms（TensorRT-LLM）。
- 评测/营销口径：相对 Whisper 约 10% vs 27% error，85% 输出零改。独立评测对「over-edit」有抱怨——关小 Auto Cleanup 就回到原文。

产品层（文档，2026-08）：None / Light / Medium / High 是**全局**整理强度；Styles 按 Personal / Work / Email；词典 1× 自动学（≤4 词，分类器过滤）+ undo；snippets；Command Mode 是另一条改写链。

对 VoiceFlow 的含义：

- 他们赢在 **专用 edit model + 同机房流水线**，不是 prompt 写得更长。
- 用 groq-oss 当 cleanup，再怎么改 SYSTEM_PROMPT，都不会变成 Wispr。
- 他们的 ASR **不是**「再包一层 OpenAI Whisper」。评测文和案例都说 proprietary / 自管 speech 模型。继续默认 Whisper turbo，中文先输一截。

### 3.2 Willow Voice：边说边传 + 专用 edit pass

公开行为（Voice-list 2026 实测 + Frontier 博文）：

- **录音时就开始往 `api.willowvoice.com` 推流**，不是松手再传。取消时音频可能已经在路上。
- 首页「200ms」是**字开始出现**的体感，不是 stop-to-insert。独立测 free tier stop-to-insert 大约 **0–2s，均值 ~1s**。英文词准确率他们测到 99.1%（0.9% WER）——口径是「可发送文本 vs 多参考答案」，和 verbatim WER 不是一回事。
- [Frontier Pro / Mini](https://willowvoice.com/blog/introducing-willow-frontier-pro)：瓶颈从模型变成了「搬音频、交接、插回 App」。Pro 把预算花在最终 writing step（格式、名字、glossary、用户接受过的改法）。Mini 是同一架构的小 edit path，便宜到能做免费档。
- Scribe 是**独立热键**的意图起草，不是默认听写。Auto-cleanup 默认开；Scribe 才是整段 LLM rewrite。

对 VoiceFlow 的含义：

- 200ms 手感 = **流式 ASR + 边说边出字**，prefetch 10s 块做不到。
- 质量 = **专用 edit model**，而且他们承认「改完 ASR 之后的 writing step」才是 edit rate 的主因。
- 不要把 Scribe 默认塞进微信。

### 3.3 Typeless：强制 polish，中文口碑好，长文会编

Voice-list（2026-06-24，v1.8.0）：

- 云端 STT + **关不掉的 LLM**。英文词准确率 92.1%（7.9% WER），停说后约 2–5s（均 ~3s）。
- 长文 verbatim WER 被拉到 ~37%：模型加小标题、总结句、没说过的列表。
- 数字 / ITN 弱于 Superwhisper（76.9% vs 92.7%）。
- 中文社区（小红书 / 评测）反而夸繁体、中英夹杂、括号/顿号口述、悄悄话。卖点是「说完就能发」，不是忠实转写。

对 VoiceFlow 的含义：

- 用 WER 硬刚 Typeless 会误判：他们赢在「像一封写好的信」，输在忠实和幻觉。
- VoiceFlow 的忠实约束对 Cursor / 代码是对的。对微信要对的是 **轻整理 + 本地标点**，不是把 Typeless 的强制 polish 抄过来。
- 若默认也强制 gpt-oss Standard，会同时得到：慢、直译残留、偶发书面语。三家最差组合。

### 3.4 三家共同结构（VoiceFlow 缺的那一层）

```text
闭源标杆：
  流式/重叠 ASR（专用或强中文引擎）
       ↓
  确定性：ITN、标点模型、幻觉过滤、词典/热词
       ↓
  专用 edit model（微调 Llama / Frontier）  ← 按 App 换强度
       ↓
  插入（他们弱，VoiceFlow 强）

VoiceFlow：
  松手 → Groq Whisper batch
       ↓
  口令标点 + 薄 lexicon
       ↓
  通用推理 LLM + 长 prompt（几乎每句都上）
       ↓
  可验证粘贴（领先）
```

交付安全、Undo、preview-first 不要为了「更快盲粘贴」丢掉。质量要补的是上半截。

---

## 4. 开源 2026：中文引擎已经换代

8 月 20 日的文还以 FunASR / SenseVoice 为主。8 月 25 日再搜，桌面听写的默认中文引擎已经偏向 **Qwen3-ASR**。

### 4.1 数字：Whisper 不是中文听写引擎

**长中文（FunASR 博客，184 条 / 192 分钟，H100，去标点 CER）**

| 模型 | CER | 备注 |
| --- | ---: | --- |
| SenseVoice-Small | 7.81% | GPU 169x 实时；CPU 仍 17x |
| Fun-ASR-Nano | 8.20% | vLLM 340x |
| Paraformer-Large | 10.18% | 流式友好 |
| Whisper-large-v3 | 20.02% | |
| Whisper-large-v3-turbo | 21.71% | **VoiceFlow 默认** |

**对话 / 噪声 / 方言（Qwen3-ASR 技术报告内部集，越低越好）**

| 子集 | Whisper-lv3 | Fun-ASR-Nano | Doubao | Qwen3-0.6B | Qwen3-1.7B |
| --- | ---: | ---: | ---: | ---: | ---: |
| Dialog-Mandarin | 14.01 | 7.32 | 6.61 | 7.06 | **6.54** |
| Dialog-Cantonese | 31.04 | 5.85 | 7.56 | 4.80 | **4.12** |
| ExtremeNoise | 63.17 | 36.55 | 17.04 | 17.88 | **16.17** |
| Elders & Kids | 10.61 | 4.54 | 4.17 | 4.48 | **3.81** |

WenetSpeech meeting：Qwen3-1.7B 5.88，Fun-ASR-Nano 6.88，Doubao 19.11，GPT-4o-Transcribe 32.27。会议噪声上通用多模态 API 也不一定赢专用 ASR。

含义：默认 Whisper turbo 的中文听写，先输在「字对不对」。Cleanup 只能修 filler 和改口，**不能发明 ASR 没给的汉字**。

### 4.2 Qwen3-ASR（2026-01，Apache-2.0）

- 0.6B / 1.7B，52 语 + 22 种汉语方言，原生中英 code-switch。
- 0.6B：TTFT 可到 ~92ms；高并发下约 2000 秒音频 / 1 秒。
- 支持流式（短窗）和离线（长窗）同一套 AuT encoder。
- 热词走结构化 user prompt（语言 / ITN / hotwords），比 Whisper 逗号列表像样。
- 桌面接入：
  - 云：DashScope / 兼容 HTTP（和现有 Custom ASR 同一形状）。
  - 本地：`mlx-qwen3-asr`（HandyQwen、Qwen Scribe、Talkink）。不要在 Tauri 里嵌完整 Python 训练栈。

### 4.3 FunASR / sherpa-onnx（仍然要，尤其是标点和同音字）

| 能力 | 谁强 | VoiceFlow 怎么用 |
| --- | --- | --- |
| 中文 CER + 速度 | SenseVoice / Nano | 聊天档：ASR + 自带标点，跳过 LLM |
| 真流式 HUD | Paraformer | 比 10s prefetch 更接近 Willow |
| 同音字 | SenseVoice + `replace.fst`（拼音 → 汉字） | `知呼→知乎` 的正确机器，不是 Whisper prompt |
| 解码热词 | Contextual / Seaco Paraformer，≤1000 条 | 词典晋升后写进同一份 lexicon |
| 标点 | ct-punc / SenseVoice 自带 | 不用说「句号」 |

桌面只走 sidecar（funasr-server OpenAI-compat）或 sherpa-onnx C API。不嵌 Python。

### 4.4 英文本地：Parakeet，不是本机 Whisper

FluidVoice / Handy / Murmur / Babel：Apple Silicon 上 Parakeet Flash / TDT，松手几乎立刻出字。英文听写的 200ms 体感来自这里。中文不要用本机 Whisper 去追；英文离线备选再上 Parakeet / Apple SpeechAnalyzer。

### 4.5 2026 开源听写该看谁

| 项目 | 许可 | ASR | Cleanup | 该学 | 不抄 |
| --- | --- | --- | --- | --- | --- |
| [Handy](https://github.com/cjpais/Handy) | MIT | 本地 Whisper / Parakeet | 几乎无 | Tauri 结构、VAD、热键 | 没有 AI 整理的产品定位 |
| [FluidVoice](https://github.com/altic-dev/FluidVoice) | GPLv3 | Parakeet / Nemotron / Qwen3 / Whisper | 本地 Fluid Intelligence | 延迟；模型目录 | GPL 源码；闭源 runtime |
| [VoiceInk](https://github.com/Beingpax/VoiceInk) | GPLv3 | 本地+云 | 可选 LLM；短文本跳过 | **替换在 LLM 前**；Power Mode | NER 自动加词 |
| [TypeWhisper](https://github.com/TypeWhisper/typewhisper-mac) | GPLv3 | 本地+云 | Prompt Action 不在默认热路径 | Hybrid 热键、commit-gated 学习、600 字 Whisper cap | 插件市场、会议 |
| [HandyQwen](https://github.com/notalexbut/handyqwen) | 基于 Handy MIT | **Qwen3-ASR MLX** | 无 / 轻 | 中英夹杂本地路径 | Python 安装器进 Tauri |
| [Talkink](https://github.com/hasso5703/talkink) | MIT | Qwen3 / Nemotron / Voxtral，纯 Swift MLX | 无 | 无 Python runtime 的本地引擎 | 功能过窄 |
| [Phemy](https://github.com/AsapShadzy/phemy) | OSS | 本地 Whisper | 本地量化 Qwen | 「ASR + 小 LLM」离线闭环 | 中文仍受 Whisper 限制 |
| [localTypeless](https://github.com/GoToBoy/localTypeless) | — | WhisperKit ANE | MLX Qwen2.5-3B 4bit | 本机 polish 体量（~3B） | 中文仍是 Whisper |
| [Douvo](https://github.com/rhinoc/douvo) | — | **豆包** Web/Android/双路 | 可选本地/云 | 中文社区「听得准」的真实对照 | 非官方协议风险 |
| [typwrtr](https://github.com/kaidhar/typwrtr) | — | 本地 Whisper | **已删 LLM** | 800 字 prompt、tombstone、collapse_repeats | 按 App 隔离词典 |
| [TalaX](https://github.com/puretensor/talax-dictation) | BSL | 本地 Whisper | 无 LLM；L1 替换 <1ms | 3× 晋升、最长匹配 | profile 隔离库；L2/L3 先不做 |
| Superwhisper | 闭源 | 本地/云 | 按 Mode；Voice-to-Text 跳过 AI | **vocab 少、replace 为主** | 自动学仍未做 |

中文用户的真实对照物是 **微信输入法 / 豆包 / 千问**，不是 Superwhisper。Douvo 的存在本身说明：认真做中文的人已经放弃 Whisper。

---

## 5. 按层拆：差距和改法

### 5.1 音频 / VAD（P0，不换引擎也该做）

竞品和成熟 OSS（OpenWhispr #462、MetaWhisp、typwrtr）的共识：

1. **上传前**裁 leading / trailing 静音。
2. 中间长停顿（>1.5s）压到 300–500ms，保留「这里有停顿」给标点，不给 30s 幻觉窗。
3. 整段无语音：根本不要打 ASR。
4. 上传后用 `verbose_json`：**丢掉** `no_speech_prob` 高且 `avg_logprob` 很低的 segment；丢掉压缩比异常的重复段。
5. 短语袋：`Thanks for watching`、`请不吝点赞`、`字幕` 等，O(n) 杀掉。LLM 救不了这类，OpenWhispr 测过 cleanup 0/172。

VoiceFlow 已经在要 word timestamps，却只用了 `text`。这是现成的 P0。

VAD：能量阈值不够。下一步用 Silero / ten-vad / webrtcvad，仍保持「短停顿留给标点」。低语档把阈值和增益绑在一起，不要单独猛抬 VAD。

### 5.2 ASR 引擎（P1，质量主因）

**推荐默认策略（按语言，不按「再包一个 Whisper」）：**

| 用户情况 | ASR | 理由 |
| --- | --- | --- |
| 中文 / 中英混合（多数目标用户） | **云：豆包或 Qwen3-ASR API（BYOK）** | 对话 CER 接近或优于 FunASR，远好于 Whisper；现有 Custom endpoint 就能接 |
| 要本地、Apple Silicon | **Qwen3-ASR 0.6B MLX sidecar**（8bit ~1.2GB 内存） | HandyQwen 已验证；code-switch 是一等公民 |
| 只要英文、要 200ms | 本地 Parakeet Flash，或继续 Groq turbo | Whisper turbo 英文够用 |
| 聊天、要标点、不要 LLM | SenseVoice / Fun-ASR-Nano + 自带标点 | 少一层就少一次「写成邮件」 |
| 兼容层 | 保持 OpenAI-compat | 不要为每个厂商写一套 multipart |

不要：本机 Whisper-large 当中文方案；默认路径继续只暴露 Groq 三个 Whisper 名；把 Python FunASR 塞进 app。

热词：同一份 lexicon，按后端翻译。

- Whisper / Groq：继续 200 token、高优先级在后。
- Qwen3 / Fun-ASR-Nano：结构化 hotwords 字段。
- Paraformer / sherpa：`hotwords_file` + score。
- SenseVoice：`replace.fst` 用 before 的拼音 → after。

### 5.3 确定性整理（P0–P1，这是「跳过 LLM」的前提）

目标：微信 / 搜索 / 终端 **80%+ 轮次不打 LLM**，而且看起来不像残稿。

按顺序、全部本地、毫秒级：

1. **Segment 过滤**（5.1）。
2. **幻觉袋 + 连续句折叠**（typwrtr / YazSes）。
3. **口述标点**（已有）+ **浅自动句末**：明显问句补 `？/`?`，明显陈述补 `。/.`，已有句号不剥。不要停顿切行（那是 Wispr 默认听写，会毁代码和聊天）。
4. **口述版式**（已有）：`1.` / `一是` / `first` 且 ≥2 才列表。
5. **Lexicon 最长匹配**（已有）：所有 before 变体；cleanup 只看本句 hit 的 **pair**。
6. **改口状态机**（缺，P0）：  
   - 标记 + 重说：`哦不对` / `不对` / `不是` / `scratch that` / `I mean` + 后接完整句 → 丢掉被否定草稿。  
   - 无标记整句重说：只留后句。  
   - 内容里的「不对」：`看它对不对`、`你说不对的时候` 留下。  
   - 对比事实：`1250，不是 1500` 留下。  
   这层必须在 LLM **之前**。现在把整段丢给 gpt-oss，模型按「不确定就原文」把 cloud 和三个「不对」留下。
7. **ITN 轻量**：中文数字、金额、日期。可后做。不要一上来上 TalaX L3 语音模糊。

Superwhisper 的原话仍然是架构原则：**vocabulary 少而精，replace 才是主修复。**

### 5.4 Cleanup 模型（P0 换模型，P2 才微调）

| 方案 | 延迟 | 质量上限 | 和 VoiceFlow 的拟合 |
| --- | --- | --- | --- |
| 现状 gpt-oss-20b + 长 prompt | 差（推理） | 中：忠实有余，改口不足 | 先换掉 |
| Groq 上的快 instruct（Llama 3.3 70B / 8B，**关 reasoning**） | 好 | 中高：靠 few-shot | P0 就能换 |
| 本机 3B（Qwen2.5 / Llama 3.2 MLX） | 本机 0.5–1.5s | 中：Phemy / localTypeless | 隐私档 |
| 微调小 Llama/Qwen（Wispr 路线） | 最好 | 高 | P2；要自己的 (ASR, 用户接受稿) 对 |
| Typeless 式强制 polish | 3s 级 | 像信，会编 | 不要当默认 |

P0 具体建议：

1. **默认 cleanup 不要再用 gpt-oss。** 推理模型不适合「100 token 内出可粘贴文本」。
2. **SYSTEM_PROMPT 砍到 1/3。** 改口规则放到确定性层；prompt 只剩：保 token、保语种、按 Effort、按 family、few-shot。
3. **每个 family 3–6 条中英 few-shot。** 微信必须是「好的哈哈我晚点回你」，不是「好的，我会稍后回复您。」Cursor 必须是改口删除 + 保留 `1. 2. 3.`。
4. **Scene skip（已改，2026-08-30）。** 只有 Terminal / FormFilling 默认 LocalOnly。PersonalChat / Social / Search 走 LLM。PromptOrCode 为 Light，禁止发明列表/`##`。
5. **短文本不再跳过。** 低于 12 个汉字的聊天也整理。空白才 LocalOnly。

不要：把「更强模型」理解成换成 gpt-oss-120b。那只会更慢、更爱总结。Wispr 赢在 **小而专 + 很快**。

### 5.5 上下文（P2，谨慎）

Wispr 会截活动窗口、读光标前后。Willow 宣传读文档 / 代码。这是他们专有名词准的另一半。

VoiceFlow 已有：family、writing prompt、可选 style example、词典 pair。刻意不传 raw URL / 标题 / PID。

可加、且不破坏隐私：

- **光标前 200 字（用户 opt-in）**，只留在本机 prompt，History 不存。对 Cursor / 邮件专有名词有用。
- 不要默认截屏。旧安全清单仍然成立。

### 5.6 手感 / 重叠（P1）

| 体感 | 做法 |
| --- | --- |
| 字在说话时就出现 | 真流式 ASR（Paraformer / Qwen3 streaming / Parakeet Flash），HUD only |
| 松手几乎立刻出最终稿 | 边说边传 + 松手只补最后 300–800ms；cleanup 与最后一块 ASR 重叠 |
| 短按 / 按住 | Hybrid 热键（旧文 P0，产品手感，不是 CER） |

prefetch 10s warmup 对「说 4 秒松手」几乎没帮助：warmup 可能还没回来，已经走完整段 final ASR。短句路径应该：**松手后只传裁过静音的整段**（通常 <8s），不要再等 prefetch。

---

## 6. 建议优先级（按「对得上竞品」排序）

产品定义不变：macOS 系统级语音输入，可恢复、可验证、context 感知。不和豆包拼识别率广告，但**默认引擎不能再是中文最弱的那档**。

### P0 — 不换引擎也能感到「不那么直译 / 不那么脏」

1. **Scene skip + 短文本 skip。** 微信默认不再打 gpt-oss。
2. **静音管线。** 首尾裁切、长停顿压缩、segment `no_speech_prob`/`avg_logprob` 过滤、幻觉短语袋。用已经在要的 verbose_json。
3. **改口状态机**放在 LLM 前。覆盖「假开头 + 不对×N + 整句重说」。Corpus 补这条，不要只测「周四，不对，周五」。
4. **Cleanup 换快 instruct，砍 prompt，加 family few-shot。** 去掉 reasoning。
5. **`1.` / `一是` 列表**与浅句末标点进确定性层（审计里已点名）。

### P1 — 真正对上「听得准」和「松手即出」

6. **中文 ASR 默认可走 Qwen3-ASR 或豆包（BYOK），和 Groq 并列。** Wizard 里把「中文推荐」写死，不要只列三个 Whisper。
7. **可选本地 Qwen3-ASR 0.6B sidecar**（MLX 或 sherpa），不要 Python 全家桶。
8. **同一 lexicon → 热词 / FST / 本地 replace。** 引擎换了，词表不能换一份。
9. **真流式 HUD**（Paraformer 或 Qwen3 streaming）。prefetch 降级为无流式后端时的备胎。
10. **粘贴后学习**按旧 harness 文：3×、tombstone、commit-gated。质量闭环，不是 ASR 本身。

### P2

11. 本机或自托管 **cleanup LoRA**（自己的 ASR→接受稿）。这是 Wispr 护城河，没有语料不要假装能抄。
12. Opt-in 光标前文。
13. 英文离线 Parakeet。
14. Scribe 独立热键，preview-first，聊天仍口语。

### 明确不抄

会议笔记、Ask Anything、截屏默认开、全局 keylog、用用户音频微调权重、聊天默认屏蔽脏话、全局唯一 cleanup 强度、1× 自动学且没有 undo、把 gpt-oss-120b 当质量升级、在 Tauri 里嵌 Python FunASR。

---

## 7. 若开始做，改哪里

| 层 | 文件 | 做什么 |
| --- | --- | --- |
| 静音 / 幻觉 | `audio.rs`, `asr.rs`, `lib.rs` `process_short` | 裁切；消费 segments/words；短语袋 |
| 改口 | 新模块或 `spoken_layout.rs` 旁 | FSM + corpus |
| 路由 | `lexicon.rs` `decide_cleanup` | scene skip、短文本 |
| Prompt / 模型 | `llm.rs`, `store.rs`, engine wizard | 默认 instruct；短 system；few-shot |
| 中文 ASR | `engine.rs`, `asr.rs`, wizard UI | Qwen3 / 豆包 preset；模型列表不再只有 Whisper |
| 热词同步 | `lexicon.rs` + 未来 sidecar | 一份 pair，多种后端 |
| HUD 流式 | `prefetch_asr.rs` → 真流式 provider | 仅 HUD |

评测：

- 中文：同一组自己的听写（微信口语、Cursor 改口、人名、中英夹杂），比 **CER + 人工「敢不敢直接发」**，不要只跑英文 WER。
- Cleanup：现有 `cleanup_corpus.rs` 加上「不对×N + 整句重说」。换模型时只改一组变量。
- 延迟：`metrics` 已分 FinalAsr / Cleanup / Paste。发布门槛写 **短句 p95 stop-to-insert**，不要只看均值。

---

## 8. 和旧结论的关系

[competitive-research.md](competitive-research.md) 的 P0（Hybrid 热键、按 App 语气、HUD 出字、词典自学）仍然成立，那些是**手感和产品**。

本文多出来的判断：

- 只做那些，**质量仍然对不上**，因为默认 ASR 和默认 cleanup 模型都选错了品类。
- Scene skip 在 8 月 23 日的 harness 计划里，**代码里还没落地**。聊天仍在打 LLM。
- 2026 年开源侧的中文答案已经从「不要本机 Whisper，用 FunASR」升级为「**云豆包 / 千问，或本地 Qwen3-ASR 0.6B**；FunASR 继续负责标点和同音 FST」。

先做 P0（静音、改口、skip、换 instruct），用户会感到「干净、敢发」。再做 P1（换中文 ASR），才会感到「准」。没有 P1，P0 只能把直译收拾得好看一点，听错的名字还在。

---

## 9. 主要来源（2026-08-25）

| 来源 | 用于 |
| --- | --- |
| VoiceFlow `lib.rs` `process_short` / `process_long`，`asr.rs`，`llm.rs`，`lexicon.rs`，`chunker.rs`，`prefetch_asr.rs` | 当前管线 |
| [Wispr × Baseten](https://www.baseten.co/resources/customers/wispr-flow/) | 微调 Llama、p99 <700ms、Chains |
| [Willow Frontier](https://willowvoice.com/blog/introducing-willow-frontier-pro) | 专用 edit model、系统延迟 |
| [Voice-list Typeless](https://voice-list.com/reviews/typeless/) / [Willow](https://voice-list.com/reviews/willow-voice/) | 独立 WER / 延迟 |
| [Qwen3-ASR 报告](https://doi.org/10.48550/arxiv.2601.21337) | 中英对话 / 方言 / 噪声 |
| [FunASR vs Whisper](https://www.funasr.com/en/blog/funasr-vs-whisper-benchmark.html) | 长中文 CER |
| [Groq STT](https://console.groq.com/docs/speech-to-text) | 224 token prompt、verbose_json、16 kHz |
| OpenWhispr #462、MetaWhisp | Whisper 静音幻觉 |
| FluidVoice、Handy、HandyQwen、Talkink、VoiceInk、TypeWhisper、Douvo、Phemy、localTypeless | 开源路径 |
| 仓库内 2026-08-20/23/24 竞品与 cleanup 审计 | 产品与 prompt 冲突（仍有效） |
