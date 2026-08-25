# Cleanup 三层现状审计

Date: 2026-08-24  
Scope: Cursor / `prompt_or_code` 上的 prompt、标点、逻辑。不打 Groq，不把窗口标题 / URL / History 写入本文。  
改法见 Cursor plan「Cleanup 三层：审计 + 怎么改」。本文只记**改之前**的失败态。

## 结论

默认听写对不上 Wispr Backtrack。对得上的只有词级「周四，不对，周五」（corpus 口径，不是 live 证明）。

主用例「cloud → 不对不对不对 → 改口成 Cursor 测试」在确定性层原样通过，LLM 指令又把「删改口」和「不要总结 / 不确定就留原文」写成互斥。按当前指令，**直译是合理失败态**，不是 ASR 没听清。不宣称 groq `openai/gpt-oss-20b` 实际会输出什么。

| 层 | 分 | 原因 |
| --- | --- | --- |
| 逻辑 / prompt | P0 | `SYSTEM_PROMPT` 同时要求 resolve self-corrections、Do not summarize、plausible 则 preserve original |
| 标点 | P1 | 口述没说逗号/句号；Cursor 走 Standard，没有 Light 的补 `?`/`。` |
| 列表 | P1 | 说了 `1.` / `2.` / `3.`，matcher 只认「第一 / first / one」 |

## 主用例推演

ASR 近似（脱敏）：

```
现在做一个完整的这种 cloud 的，不对，不对，不对。我现在在做一种完整的这个 cursor 的这个测试，看一下它这个具体的 cleanup。看一下它具体的 cleanup。1. 是 prompt, 2. 是标点符号，还有 3. 是的逻辑，看上它对不对。参考上面的 response 你看我说不对的时候 这个 ai clean up 应该把之前的删了 现在就是直译 看一下竞品
```

Family = `PromptOrCode`。[`CleanupEffort::default_for_family`](../../src-tauri/src/llm.rs) 对 PromptOrCode 返回 **Standard**，不是 Light。

### 1. `spoken_punctuation::apply`

[`FIXED_TOKENS`](../../src-tauri/src/spoken_punctuation.rs) 只有中文「逗号/句号/…」和英文 `comma` / `period` / `question mark` / `exclamation point|mark`。这段里没有独立口令。「不对」不是标点 token。输出 = 输入。

### 2. `spoken_layout::apply`

- 无换行口令（换行 / new line / 新段落）。
- 无「第N」/ first / bullet。`1.` `2.` `3.` **不是**当前编号 matcher。
- Email 拆行只在 `ContextFamily::Email && confidence >= 0.75`。
- 输出仍是一行原文。`spoken_raw` 进 LLM 的 Transcript 就是这段直译。

### 3. LLM

System：[`SYSTEM_PROMPT`](../../src-tauri/src/llm.rs) 第 392 行：

- Resolve self-corrections first
- If more than one interpretation is plausible, preserve the original wording
- Do not summarize … or add a … list, line break … that was not spoken

User：PromptOrCode writing prompt（[`context.rs`](../../src-tauri/src/context.rs) 第 465 行）只说保代码 token、不发明代码，**不提**中文「不对」。`code.cursor` 的 `profile_guidance` 只说 developer tool。Standard **没有** Light 那段「Add a question mark … Drop superseded drafts」。

按指令最可能留下：cloud 假开头、三个「不对」、两句重复的 cleanup、以及「看它对不对」（这句留下是对的）。

成熟听写期望：丢掉 cloud 和改口「不对」；重复句只留一句；**保留**「看它对不对」和「你说不对的时候」里的「不对」；说了 1/2/3 才列表。

## Prompt 冲突（逻辑层）

同一句里三件事互斥：

1. 要先 resolve self-corrections
2. 不要 summarize
3. 两种理解都说得通就 preserve original

模型会把「留下 cloud + 不对不对不对」当成不总结、不确定就原文。删被否定草稿不是总结，是忠实清理。这不是正则能修的：「看它对不对」「预算是 1250，不是 1500」必须留下。

已有 corpus 只覆盖词级：

- `zh_self_correction`：周四，不对，周五
- `en_self_correction`：Tuesday, no Wednesday
- `chunk_boundary_sentence`：下一段，不对，先完成权限校验
- `amount_preservation`：1250，不是 1500（对比，不是改口）

缺口：重复「不对」+ 整句重说；「不对」作问句/话题；`scratch that` 重说；`actually` 作内容。

## 标点

确定性层对这条口述无操作。自动句末标点只写在 Light user 段和 PersonalChat writing prompt。Cursor Standard 只靠 SYSTEM_PROMPT 的 “fix punctuation”，没有「明显问句加 ?、明显句末加 。、不剥已有句号」。不把「这种 / 这个」当 filler。

## 竞品对照（公开页）

| | Wispr Backtrack | Willow | Typeless | VoiceFlow 现状 | 我们对齐的窄层 |
| --- | --- | --- | --- | --- | --- |
| 改口标记 | `actually` / `scratch that` | 自然改口 | mid-sentence revision | prompt 写了 resolve，但 plausible/summarize 把删草稿打回去 | 词级「周四，不对，周五」 |
| 直接重说 | 文档：可以不说触发词，重说即可 | 宣称更新上一句 | 只留 final intent | 无 restatement 口径 | 计划补：假开头 + 不对 + 重说只留后句 |
| 标记作内容 | `I actually enjoyed` 留下 | 未单列 | 未单列 | 无 fixture；「看它对不对」会和改口抢同一词 | 必须留问句/话题里的「不对」 |
| 自动标点 | Smart Formatting 不用说 period | 自动加标点 | 自动 polish | 口令有；Cursor Standard 弱于 Light | 口令 + 明显句末；不抄「不用说格式」 |
| 列表 | 说了编号就排 | 「听出序列就排」 | 自动 lists/steps | 第一/first/bullet，≥2 | 说了 `1.`/`一是` 且 ≥2 才排；不发明列表 |

不对齐：Wispr 默认整页、Willow 自动成信、Typeless 补没说的细节、按停顿切行、微信吃句号、对「不对」做正则。

## 不修清单（产品）

不停顿切行；不发明邮件；微信不吃句号；不跑 cloud 四组；不宣称 naturally speak。
