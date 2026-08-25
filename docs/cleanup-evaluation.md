# AI Cleanup Prompt v2 评测协议

## Corpus

脱敏案例位于 `src-tauri/src/cleanup_corpus.rs`，当前至少 40 条，覆盖中文、英文、中英混合、filler、重复、自我纠正、邮件、聊天、搜索、文档、代码、终端、日期、金额、人名、URL、路径、命令、版本号、明确改写、模糊表达、低置信度 context、chunk boundary，以及口述换行/段落、序数列表、列表假阳性和邮件称呼拆行。

每次 Prompt 或模型变更都使用同一 corpus、同一输入顺序和同一 context fixture。不得把真实录音、窗口标题、URL 或用户 History 放入评测数据。

## 四组对照

| 组别 | Prompt | 模型 |
| --- | --- | --- |
| A | 当前 Prompt | `openai/gpt-oss-20b` |
| B | Prompt v2 | `openai/gpt-oss-20b` |
| C | 当前 Prompt | `openai/gpt-oss-120b` |
| D | Prompt v2 | `openai/gpt-oss-120b` |

固定 temperature、reasoning effort、最大输出 token、请求超时和重试策略。每组记录质量、p50/p95 stop-to-insert、token 用量和估算成本；不要在一次实验中同时更换 Prompt、模型和参数。

## 评分与 release gates

- 高风险 token（URL、邮箱、路径、命令参数、版本号、日期、金额、数字）的保留率必须为 100%。
- Faithful Cleanup 人工通过率至少 95%。
- 不得新增事实；自我纠正案例不得保留被否定内容。
- Prompt v2 的明确 rewrite 接受率高于当前 Prompt。
- 同模型 p95 stop-to-insert 不得比基线恶化超过 15%。
- 20B / 120B 的默认模型选择必须同时基于质量、延迟和成本；评测完成前继续使用默认 20B。
- **口述结构保留：** Transcript 里已有的换行、空行、列表行前缀不得被 merge；说了至少两个序数或显式 bullet 才排成列表；没说结构词时不得发明分段/列表/邮件骨架。这一层对齐的是口令 + 序数列表 + 忠实整理，**不等于** Wispr / Willow / Typeless 的默认听写手感。

## 当前状态

自动化测试只验证 corpus fixture、意图解析、Prompt 约束、protected-token 校验和口述 layout 行序，不声称替代真实 Groq 请求。四组模型结果、真实 stop-to-insert 和真实成本仍需在有授权 API key 的 macOS 环境执行，并作为 release blocker 记录到验收证据中。
