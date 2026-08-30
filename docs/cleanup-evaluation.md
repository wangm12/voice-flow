# AI Cleanup 评测协议

## Corpus

脱敏案例位于 `src-tauri/src/cleanup_corpus.rs`。`expected` 是评审目标，不是「llama 实际返回了什么」。不得把真实录音、窗口标题、URL 或用户 History 放入评测数据。

默认整理模型是 `llama-3.1-8b-instant`。不要用过时的 gpt-oss A/B 当默认对照。

## 两道门

**Gate 1（合并，CI，无 key）：** `cargo test --lib` 里的 `llm` / `lexicon` / `spoken_*` / `cleanup_corpus`，加上 `providers.test.ts` / `EngineSettings.test.tsx`。证明路由、filler、口述符号、prompt 标签。**不能**证明听得准或模型整理得好。

**Gate 2（准确度，owner 实听）：** 6 句口播卡，见计划 Verification contract。打分前不声称 accuracy。不把 live Groq 或 `evals/` harness 当合并门槛。

## 评分（Gate 2 或可选 live 文本评测）

- 高风险 token（URL、邮箱、路径、命令参数、版本号、日期、金额、数字）保留率必须 100%。
- 不得新增事实；自我纠正不得保留被否定内容。
- 微信/短讯不得发明 您好 / Hello / Best / Subject。
- PromptOrCode / Terminal 不得发明 `##` 或未口播的列表。
- 中英混合不得把中文从句译成英文。
