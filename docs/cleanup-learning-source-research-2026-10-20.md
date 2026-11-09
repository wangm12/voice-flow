# Cleanup 与纠错学习：固定源码对照

2026-10-20 的源码研究。样本覆盖 9 个仓库、39 个固定版本文件。方法是阅读源码和文档，比较 prompt 的构造、反馈的采集、学习结果的写入和下一次的读取。研究没有运行外部产品或模型。表内观察仅针对所列版本。

| 项目 | 源码观察 | 可借鉴的设计 |
| --- | --- | --- |
| [Handy](https://github.com/cjpais/Handy/blob/93c9ee05efe294cf2a56bd534b77ea1079f21b77/src-tauri/src/settings.rs) | 默认整理约束保留词序。 | 作为保守整理对照组，单独评估段落和列表策略。 |
| [VoiceInk](https://github.com/Beingpax/VoiceInk/blob/22e5ec5479b712797c3b338fcb875961e58800b6/VoiceInk/Features/Dictionary/AutoLearn/AutoLearnAIReviewer.swift) | 整理规则包含明确改口和版式，学习审核区分替换、词汇与拒绝。 | 让裁判输出有限的分类及可审查的证据。 |
| [OpenWhispr](https://github.com/OpenWhispr/openwhispr/blob/2aaa7bcb23699daf65feecf7ca2a18d141174279/src/utils/correctionLearner.js) | 过滤大幅改写，部分候选提取依赖空白分词和短词过滤。 | 对整段重写弃权，中文候选另测分词与短人名边界。 |
| [Open Typeless Harness](https://github.com/OpenCodexLabs/open-typeless-harness/blob/28a933be50f52bf5a9b0942f0aedda54cbf2e5a9/openless-all/app/src-tauri/src/learning_probe.rs) | 高置信纠正可写入持久化规则，后续整理有限量检索。部分正式语气示例可能加强原话确定性。 | 验证规则确实影响后续输出，并用不确定语气、否定和数字做保真回归。 |
| [TypeFree](https://github.com/Charlo-O/typefree/blob/c6b0c769b897ebc49efcb48e2850d4b94c483d74/src-tauri/src/commands/postprocessing.rs) | Rust 实际请求使用的模式指令与前端默认展示分开维护，prompt 文字评分用规则匹配。 | 评估实际发出的请求和模型输出，单独记录 prompt 文本检查结果。 |
| [TypeWhisper](https://github.com/TypeWhisper/typewhisper-mac/blob/45c37e0711a0499307c3f6bce7e348bee1e42efd/TypeWhisper/Services/TargetAppCorrectionLearningService.swift) | 跟踪焦点输入框的编辑和提交，持久记忆另有提取逻辑。 | 将用户明确纠正的词对和长久个人偏好分开处理。 |
| [Harness2](https://github.com/google-research/harness2/blob/1cfe254e8d25b15f37048c3730134686abe60335/harness2/core/recursion.py) | 固定模型权重，通过提出、试验和组合策略积累经验；无凭据本机 demo 使用确定性编辑器。 | 把候选规则迭代放到离线实验，实时听写使用已选定、可回退的版本。 |

这次对照记录的是上游设计，不提供跨产品整理质量排名或中文个性化质量结论。Open Typeless Harness 是表中所链接的开源项目，观察范围限于该仓库。其余采集的参考源码也列在[固定来源清单](cleanup-learning-source-index-2026-10-20.tsv)中，包含仓库、提交、文件路径、字节数和 SHA-256。可从对应提交取回文件后核对 `shasum -a 256`。

## 对 VoiceFlow 评估的启发

- 把明确词对、场景风格偏好、已批准示例和保真约束分开评估。用户没有修改输入框时，真值仍然未知。
- 用已确认交付的片段和明确的编辑证据定位差异。测试单独覆盖用户新增信息、ASR 漏听、Cleanup 改坏原意和无法归因四种情况。对证据不足的情况衡量弃权，避免把错误归因的候选当作学习收益。
- 固定输入和模型，比较同一文本使用与不使用候选规则的结果。保留一组未参与规则选择的负例，分别检查数字、否定、语气、引号和列表，以及跨 App 或网站时规则是否正确停用。
- 本机词典替换、模型整理和 ASR 解码分别记结果。词典把输出词修正后，应把收益归给实际执行替换的阶段。检索到的示例需有界并保持固定 prompt 前缀稳定。

VoiceFlow 的现行协议和确定性测试入口见[整理评估](cleanup-evaluation.md)与[端到端流程](end-to-end-workflows.md)。
