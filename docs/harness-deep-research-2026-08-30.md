# Harness-first：没有自有模型时，怎么「改完变准」

更新：2026-08-30

范围：系统级听写的自学循环。不是会议笔记，不是 Agent，也不是再训一个 ASR。

对照：[competitive-research.md](competitive-research.md)（产品分层）、[asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md)、[privacy.md](privacy.md)。

---

## 结论

VoiceFlow **没有自研 ASR / cleanup 模型**。默认是用户的 Groq Whisper + Groq `llama-3.1-8b-instant`；引擎可以 BYOK，权重永远是别人的。Wispr / Willow 靠微调 Llama 和自管 speech 模型赢；Idiolect 靠用户音频 LoRA 赢。这两条路我们都不走。

所以「越用越准」只能是 harness：

1. **学**：同框观察 / History 改正 → 本地 `learn_pairs`
2. **用**：确定性替换 + 只把打中的 pair 塞进别人的 API
3. **守**：LLM 改完再替换一遍，已知错词不能被通用模型写回
4. **不训**：不存音频微调、不 DPO/GRPO、不上传纠错去训练「我们的模型」

换更好的第三方引擎（SenseVoice / Qwen3-ASR）是用户选的 **插头**，不是护城河。护城河是：同一张 `learn_pairs` 在任何插头上都能观察、晋升、打中、撤销。Superwhisper 的公开结论也是这条——Vocabulary 少用，Replacements 才是主修复。

代码里学习循环已经比多数开源听写完整。还缺的是 harness 自己的打中率和「改完不被 LLM 毁掉」，不是再找一个更好的基座。

```text
观察 / History ──► learn_pairs（一张表）
                      │
          ┌───────────┼───────────┐
          ▼           ▼           ▼
     精确替换     谐音召回     按插头变形
          │           │        （Whisper prompt /
          └─────┬─────┘         Qwen context /
                ▼               Fun 热词）
         租来的 ASR / LLM
                ▼
         LLM 后再替换一遍
```

---

## 1. VoiceFlow 今天已经有的 harness

不要按 8 月 20 / 23 日文档里的「还没学」来对照。当前实现：

| 层 | 行为 | 代码 |
| --- | --- | --- |
| 学什么 | 单词语 + 短拉丁短语（2–4 词）+ 短 CJK（2–8 字）；纯插入不学；大段改写进 `Ambiguous`；标点密度进 `StyleSignal` | [`dictionary_learn.rs`](../src-tauri/src/dictionary_learn.rs) |
| 怎么晋升 | 默认 3 次或 History / 设置确认；分类器认作 2–3 字中文人名则 2 次；发送 / 焦点离开提交最后一次 settled 词。自动学丢掉 filler / 语法 / 仅大小写 | [`dictionary_learn.rs`](../src-tauri/src/dictionary_learn.rs) `classify_learn_pair` |
| 可逆 | HUD「已学 … · 撤销」；`ignore` / `undo` 写 `tombstoned_at`，同一 pair 不再累计 | [`store.rs`](../src-tauri/src/store.rs) `tombstone_learn_pair`；[`IslandWindow.tsx`](../src/components/Island/IslandWindow.tsx) |
| 按 App 关学习 | mapping `dictionary_learn_enabled`；1Password / HR / SSO **无 mapping 也默认关**；用户 mapping 可再打开 | [`lexicon.rs`](../src-tauri/src/lexicon.rs) `scene_allows_learn` |
| 置顶 / 口癖草稿 | pin 进 ASR 排序；`StyleSignal` → style draft，人审后写进该 mapping | `pin_dictionary_term` / `confirm_style_draft` |
| ASR 出口 | 硬顶 **200 token**。Whisper：虚构抄本（高优在后）+ `不要翻译。`；SenseVoice / Qwen / Deepgram：同一张表改成词表 / `keyterm` | [`lexicon.rs`](../src-tauri/src/lexicon.rs) `build_asr_prompt_shaped` |
| 替换 | 最长优先、拉丁词边界、CJK blocker；只套已晋升且 after 在 dictionary 里的 pair；cleanup **后再套一遍**；CJK 同拼音可召回 | `apply_lexicon_replacements` / `apply_promoted_replacements` |
| Cleanup | 只带本句 **hit pairs**（最多 8）；未命中不再回退 dictionary 前 32 词 | `hit_pairs` |
| 幻觉 | **短语袋** + segment `no_speech_prob` / `avg_logprob`。不是 Rhapsode 那种按时间戳对音频能量证伪 | [`spoken_revision.rs`](../src-tauri/src/spoken_revision.rs)、[`asr.rs`](../src-tauri/src/asr.rs) |
| 隐私 | 同一框，约 3 秒起、空闲延长到最多 12 秒；短词或短短语；Secure Input / 关学习则不观察；不上传纠错 | [privacy.md](privacy.md) |

和开源比，这一段已经领先：Open Typeless Harness 的 promotion 仍是 JSONL 候选；Handy 没有学习循环；Alowd 每条都要人审。

P0 / P1 主线已在代码里：cleanup 后再替换、谐音召回、分类器 + 人名 2×、1Password/HR 预设关学习、Whisper 虚构抄本、未命中不回退整表。后做的是 Idiolect 式独立 review 窗、搜狗导入、style draft held-out。

---

## 2. 开源：谁在做 harness，谁在做模型

抄交互与产品语义，不抄 GPLv3 / AGPL 源码。

### 2.1 真正做成闭环的（值得细看）

| 项目 | 许可 | 学什么 | 晋升 | 用到哪 | 对 VoiceFlow |
| --- | --- | --- | --- | --- | --- |
| [TypeWhisper](https://github.com/TypeWhisper/typewhisper-mac) | GPLv3 | History 词交换 + Premium 同框单词语 | History 1×；AX 在 Return / Tab / 失焦才 commit | Terms → 引擎 prompt（600 字）；Corrections → pipeline 替换 | 发送即学已有。可抄「词 vs 替换」分列。不抄源码 |
| [typwrtr](https://github.com/kaidhar/typwrtr) | 看仓库 | 同框 diff + 最多 4 词上下文 | 有 tombstone 后降到 1× | Whisper ~800 字；词边界替换 + Metaphone；**无 LLM** | 最好的开源规格：预算算术、Forget、Electron 修热键。我们用全局表 + 场景排序，不按 App 隔离词表 |
| [TalaX](https://github.com/puretensor/talax-dictation) | BSL 1.1 | History 审阅 diff | **3×** 才自动套 | L1 精确替换；L2 trigram；L3 Levenshtein + Double Metaphone | 3× 和薄版谐音已抄。不要 L2 n-gram |
| [Open Typeless Harness](https://github.com/OpenCodexLabs/open-typeless-harness) | MIT | 粘贴后编辑轨迹 → speech skills | README 说自动晋升；代码只写 `correction_candidate` JSONL | 技能进 **LLM polish**，不是本地替换 | 产品故事相同，实现更薄。不要抄「技能只进 LLM」——那是今天失败态 |
| [YazSes](https://github.com/MSKazemi/yazses) + [arXiv:2607.28878](https://arxiv.org/abs/2607.28878) | Apache-2.0 | 加密本地语料；tuner 出 **配置 diff** | 人审 + held-out 切片 | Whisper prompt = app + vocab；命令先 regex | 抄 held-out / 不学击键。不要抄常开音频语料 |
| [Alowd](https://github.com/nboai2026/alowd) | MIT | 粘贴后建议记词 | **每条都人审** | WhisperKit 本地 | 抄「字段不再包含刚贴的字就不学」。人审-only 太慢 |
| [Rhapsode](https://github.com/vishk23/rhapsode)（FreeFlow fork） | MIT | AX 观察 respell | 自动加入词典 | Whisper prompt + **确定性谐音纠正** + cleanup；幻觉用音频能量证伪 | 谐音层可抄。Voice Bank / 声音克隆 / 4s 竞速不是 harness 主线 |
| [voice-typed](https://github.com/nikhilm55/voice-typed) | 看仓库 | 手动 `vocab.txt` + `corrections.txt` | 无自动学 | 替换在 **LLM 之后再跑一遍** | **没有自有 edit model 时的否决权。** 我们已在 LLM 后再替换一遍 |

### 2.2 权重级自学（整条不抄）

[Idiolect](https://github.com/nick-tgcs/idiolect)（AGPL-3.0，Linux IME）：每条口述 = 音频 + 最终文本；review dialog 在自己窗口里改，Electron 也能拿到 gold；`idiolect-trainerctl` 用 Burn 训 Whisper LoRA（decoder q/v，rank 8），holdout 每 10 条，merge 成普通 ggml。作者强调小语料（100–500 条）怕过拟合；**改过的 take 才教词汇，没改的只教声学**。

这证明 harness 采集 gold 和训权重是两件事。我们只要采集和替换。不要存音频微调，不要 LoRA，不要 AGPL。可抄的只有：Electron 看不清 AX 时，用 History / 可选 review 当 gold。

### 2.3 有词典、几乎不学

- [FreeFlow](https://github.com/zachlatta/freeflow)（MIT）：手动词汇；有人用截图进 cleanup。Rhapsode 在它上面加了自学。
- [OpenTypeless](https://github.com/tover0314-w/opentypeless)、[Handy](https://github.com/cjpais/Handy)、[Calliop](https://github.com/Lappom/Calliop)、[Eqho](https://github.com/DanielMevit/Eqho)、[LocalVoice](https://github.com/iptoux/localvoice)：手动词典 / 替换，闭环弱。
- [VoiceInk](https://github.com/Beingpax/VoiceInk)：实验性 Apple NER 自动加词，用户报 junk。词汇主要进 LLM enhancement。
- [platx-ai/Talk]：观察后用后台 LLM 抽纠错。延迟和隐私都反着我们的目标。
- [MyVoiceTyping](https://github.com/botaruibo/MyVoiceTyping)：SenseVoice + 搜狗 scel 热词。导入路径可后做，仍喂同一张表。
- FunASR / sherpa-onnx：引擎层热词 / 谐音 FST，不是听写 App。用户选了这个插头，harness 再变形出口。

### 2.4 闭源标杆（规则，不是代码）

- **Wispr**：≤4 词、每编辑最多 4 条；分类器只要专有名词 / 产品 / 缩写；1× + undo；boost 和错拼替换分开。
- **Willow**：营销 1×–2×；中文人名进个人词典。
- **Superwhisper**：Vocabulary 少用、Replacements 才是主修复。我们没有自有模型时，这条比「把整本词典塞进 Whisper」更重要。

---

## 3. 论文：inference-time 才能进产品

按「能不能在不 fine-tune、不上传纠错、不养自有 runtime 的前提下用」分类。

### 3.1 能指导 harness

**SeACo-Paraformer + ASF**（[arXiv:2308.03266](https://arxiv.org/abs/2308.03266)）  
热词一多，attention 会散。工业数据上低召回热词从约 3% 拉到 CLAS 69%、SeACo 79%、ASF 后 87%。建议 ≤1000 词、每词 ≤10 字，推理时先滤 top-k。  
含义：以后用户接 FunASR / Paraformer，不要把 256 词整表灌进去；复用今天的 `collect_ranked_terms`。这是适配器，不是我们的模型。

**Qwen3-ASR Context Enhancement**（[QwenCloud](https://docs.qwencloud.com/developer-guides/accuracy-tuning/speech-recognition)）  
system 消息最多约 10,000 tokens，接受词表 / 段落 / 混合；对无关文本几乎不掉点。Fun-ASR 是另一套：预编译热词表（每表 ≤500）或请求内即时热词（权重 1–5）。  
含义：同一张 `learn_pairs` 按对方 API 变形。用户没选 Qwen / Fun 时，不必做。**不要把换引擎当成自学方案。**

**OpenAI Whisper prompting**（[Cookbook](https://github.com/openai/openai-cookbook/blob/main/examples/Whisper_prompting_guide.ipynb)）  
prompt 不是指令，是「上一句抄本」。只看最后 224 token。虚构抄本 + 拼写指南比逗号词表更像训练分布。  
含义：对 **当前默认插头** 的 harness 出口。今天已是 `不要翻译: 晓雯, 知乎 …` + 中英混合种子，高优在后、预算 200 token。P1 是把逗号词表织进一句虚构抄本（`晓雯今天下午在知乎看 TypeScript。不要翻译。`），不是从「Recognize these terms exactly」改起。

**DeRAGEC / RAGEC**（ACL 2025 Findings）  
用发音相近的命名实体检索，再过滤噪声候选；相对无后处理 WER 降约 28%。训练免费。  
含义：词典增长后，不要把 256 词全塞 LLM。用拼音从 1-best 召回 5–8 个可能实体，交给 `hit_pairs`。这是「只认精确 before」的补丁。

**Error-Aware TF-IDF RAG**（[arXiv:2606.24915](https://arxiv.org/abs/2606.24915)）  
用历史错词建稀疏检索；Persian FLEURS 上 error-aware hit 53.7%→90.9%，WER 23.06%→18.83%，延迟近零。  
含义：`learn_pairs` 就是历史错词库。精确替换 miss 时，用 before 的拼音 / 编辑距离召回 after。不必上向量库。

**Amazon Contextual ASR + RAG**  
用 1-best + 自定义词表检索，再让 LLM 做 contextual 纠错。和我们 hit-only pairs 同构；差在他们会检索语音相近但字符串不同的词。

**HyPoradise / GER / ClozeGER**  
N-best → LLM 生成纠错。Groq Whisper 默认 1-best；要 N-best 就要换引擎或自托管。短听写不允许再跑一轮大 GER。默认不做。

**TCPGen + Whisper**（Interspeech 2023）  
不改 Whisper 权重，外挂 pointer generator。挂不进 Groq API；要自建本地 runtime。和「没有自己的模型、靠 harness」冲突。默认不做。

### 3.2 训练论文，整类不做

| 论文 | 做法 | 为什么不做 |
| --- | --- | --- |
| Idiolect trainer / Whisper LoRA | 用户音频 + 改正文本训 adapter | 我们没有自己的模型；也不用用户录音微调 |
| Customizing ASR with LLM feedback（[arXiv:2506.11091](https://arxiv.org/abs/2506.11091)） | LLM logprob 当 reward，DPO/GRPO | 训练循环 |
| GRPO for Speech Recognition（[arXiv:2509.01939](https://arxiv.org/abs/2509.01939)） | RLHF 降幻觉 | 同上 |
| RLBR（[arXiv:2601.13409](https://arxiv.org/abs/2601.13409)） | 热词加权 reward 训 Speech LLM | 同上 |
| ASR-TRA（AAAI 2026） | Test-time RL 更新 decoder prompt | 每次推理更新参数 |
| RASTAR NEC（[arXiv:2602.12287](https://arxiv.org/abs/2602.12287)） | 自教 CoT + RAG 改命名实体 | 太重；NER 过拟合 VoiceInk 已踩过 |

---

## 4. Harness 缺口：已落地 vs 后做

只列不依赖自有模型的项。

### 已落地：LLM 之后再跑一遍 lexicon

`prepare_lexicon_transcript` → cleanup → `apply_promoted_replacements`。通用 LLM 可能把已经替换好的词再改回同音字；cleanup 返回后 harness 有最后否决权。

### 已落地：谐音召回，再精确替换

精确 `知呼→知乎` 救不了第一次出现的 `之乎` / `直呼`。文本层用拼音 / 编辑距离从 `learn_pairs` 召回：

- CJK：轻量 pinyin，对齐 `after` 和历史 `before`
- 拉丁：编辑距离 ≤2 且 after 已在 dictionary
- 召回进 `hit_pairs`（仍 ≤8）
- **自动替换只在拼音完全一致，或该 before 已被确认过时发生**
- 不要默认全局 Metaphone

### 已落地：分类器之后才能对人名 2×

`classify_learn_pair` 丢掉 filler / 语法 / 仅大小写；2–3 字且姓氏匹配的中文人名 2×，产品 / 缩写仍 3×。History 确认不受分类器限制。1Password / Workday / Okta 等无 mapping 时默认不观察；用户 mapping 可再打开。Whisper 出口是虚构抄本，不是逗号词表。Cleanup 未命中不再回退前 32 词。

### 已落地：适配器（同一张表，按插头变形）

出口：`asr_prompt_shape_for`。**不要第二张词表。** Fun-ASR `vocabulary_id` / sherpa `replace.fst` 仍要等用户选了那种 API。

| 用户选的插头 | harness 出口 | 预算 |
| --- | --- | --- |
| Groq / Whisper | 虚构抄本，高优在后 | ~200 token |
| Qwen3-ASR / SenseVoice（OpenAI-compat `prompt`） | scene-ranked 词表 | 前 64 / 200 token |
| Deepgram | 同一词表变成 `keyterm` | 前 64 |
| Fun-ASR 热词 API / sherpa FST | 后做 | — |

短语袋幻觉已在；按时间戳对音频能量证伪（Rhapsode）不进 harness 主线。N-best、TCPGen、4s 云/本地竞速同样依赖引擎或自建 runtime。

History 确认芯片已在。后做的是 Idiolect 式独立 review 窗（Electron 看不清 AX 时）、搜狗词库导入、style draft 的 held-out（第三条相似标点不能是同一句复制）。

---

## 5. 明确不立项

和 [asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md) 对齐：

- 任何「我们自己的模型」：LoRA、用户音频微调、DPO/GRPO、test-time RL、自研 edit model
- 把换更好的云 ASR 当成自学方案
- Talk / VoiceInk：LLM 或 NER 从字段里发现该学什么
- 截图进 LLM；Voice Bank / 声音克隆
- 团队云词典、把纠错上传训练模型
- 全局击键、密码框学习、30s 字段 JSONL
- 会议笔记、Ask Anything、控电脑
- 抄 GPLv3 / AGPL 源码
- 孤立 per-app 词典
- 无 tombstone 的 1× 静默晋升
- TCPGen / 自托管 N-best GER（等于开始养 ASR runtime）

---

## 6. 落地顺序

1–4 已落地。5 的 OpenAI-compat / Deepgram 出口已按插头变形；Fun-ASR `vocabulary_id` 和 sherpa FST 仍后做。

1. Post-LLM lexicon pass
2. Pinyin / phonetic retrieve → `hit_pairs`
3. 分类器 → 人名 2×
4. Whisper 逗号词表 → 虚构抄本
5. 已选插头的出口变形（Whisper / Qwen / SenseVoice prompt，Deepgram `keyterm`）

验证：`cargo test --manifest-path src-tauri/Cargo.toml --lib`。

相关实现入口：[`dictionary_learn.rs`](../src-tauri/src/dictionary_learn.rs)、[`lexicon.rs`](../src-tauri/src/lexicon.rs)。
