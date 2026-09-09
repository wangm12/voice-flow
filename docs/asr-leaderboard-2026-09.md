# ASR 榜单与插头取舍（2026-09）

> 研究对照，不是活文档。数字会过期。下面是公开榜单 / 论文口径，**不是** VoiceFlow 实测，也不证明产品已经更准。

两张榜，**不要混第一名**。HF Open ASR 几乎没有中文；中文近讲 / 方言另看 FireRedASR2 与 Qwen3-ASR 论文。Granite 赢英文向均值，不赢中文听写。

---

## 英文向：HF Open ASR Leaderboard

评测集偏 AMI / Earnings-22 / GigaSpeech / LibriSpeech，几乎没有中文。均值是 **WER，越低越好**。

| 模型 | Mean WER | 备注 |
| --- | ---: | --- |
| IBM Granite Speech 4.1 2B | 5.33 | EN/FR/DE/ES/PT/**JA**，**无中文** |
| Cohere Transcribe | 5.42 | |
| NVIDIA Canary-Qwen 2.5B | 5.63 | |
| Qwen3-ASR-1.7B | 5.76 | 也覆盖中文 + 方言 |
| Whisper large-v3 / turbo | ~7.4–7.8 | VoiceFlow 默认 |
| Parakeet TDT 0.6B v3 | （快） | 欧洲语言，不是中文 |

Granite 是这张英文向榜的第一，不是中文近讲第一。不要据此把默认引擎换成 Granite。

---

## 中文近讲 / 方言（论文口径，不是 HF 英文榜）

来源：FireRedASR2、Qwen3-ASR 论文。数字是 **Mandarin / 近讲 CER，越低越好**。不要和上一张 WER 混排第一名。

| 模型 | CER | 备注 |
| --- | ---: | --- |
| FireRedASR2-LLM | Mandarin-4 **2.89** | 8B+，没有 OpenAI-compat 托管 API |
| Doubao ASR | ~3.69 | 不做一等 Volcengine provider |
| Qwen3-ASR-1.7B / Flash | ~3.76 | 52 语 + 22 方言，DashScope 托管 |
| Fun-ASR API | ~4.16 | |
| SenseVoice Small | （好于 Whisper，不是 2026 中文 SOTA） | SiliconFlow `/audio/transcriptions` 已经能用 |
| Whisper（中文） | 常常差 **2–10×** | 例：WenetSpeech Meeting Whisper ~18.9 vs FireRed ~4.3 |

---

## 不采用：VibeVoice / BitNet

会议 + 说话人切片，不是近讲听写。Fleurs-zh：SenseVoice 5.56 vs BitNet 8.35。这一片不接。

---

## 产品决策

- 只改进我们能 **HTTP 调用** 的模型。
- **不要**把默认换成 Granite。
- **不要**改 Groq 全局默认。
- 有百炼钥匙时，**DashScope 上的 Qwen3 是质量插头**。官方形态：`POST …/chat/completions`，base64 音频 + system 里的 context terms + `enable_itn`；`auto` 时省略 `language`。设置里点「阿里云百炼 Qwen3-ASR」即可填北京站预设。工作区 `*.maas.aliyuncs.com` 也走同一条 chat 路径。
- 没有百炼钥匙时，**SenseVoice 仍是中文插头**。

不和豆包拼「听得准」，也不把论文 CER 当成已经验收的产品准确率。

---

## 链接

- [HF Open ASR Leaderboard](https://huggingface.co/spaces/hf-audio/open_asr_leaderboard)
- [DashScope Qwen-ASR API](https://help.aliyun.com/zh/model-studio/qwen-asr-api-reference)
- 本仓库设置页百炼预设：[`src/components/settings/EngineSettings.tsx`](../src/components/settings/EngineSettings.tsx)（按钮「阿里云百炼 Qwen3-ASR」）
