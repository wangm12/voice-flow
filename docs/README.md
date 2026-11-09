# VoiceFlow 文档

对照当前代码阅读。研究稿里的「今天 / 现状」可能过期，以本页「活文档」和源码为准。Agent 约定见仓库根目录 [AGENTS.md](../AGENTS.md)。

## 活文档（应对齐代码）

| 文档 | 内容 |
| --- | --- |
| [README.md](../README.md) | 快速开始、日常用法、开发与发布入口 |
| [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) | MLX sidecar 的上游 LICENSE/NOTICE、Swift 参考文本、Handy 组件及固定本地模型的来源说明 |
| [privacy.md](privacy.md) | 数据流、本地保留、Keychain、删除 |
| [end-to-end-workflows.md](end-to-end-workflows.md) | 首次配置 / 本机麦克风自检 / 词典反馈 / batch 预取 / Soniox 实时流与恢复 / AssemblyAI Dictation + raw Sync / Qwen Audio Message / 口述 / 长录音与完整音频恢复 / 六类选中文本操作 / 手动看屏幕 / PromptOrCode 与 Code 中的请求边界 / 后端交付撤销命令 |
| [cleanup-evaluation.md](cleanup-evaluation.md) | 确定性本地整理评测、test-only 编辑归因裁判、feature-gated WAV evaluator、strict corpus/recipe/negative 契约、显式 SHA 候选重放、当前执行范围与可选 live provider 协议（不代表 runtime/E2E 验收） |
| [public-corpus-preparation.md](public-corpus-preparation.md) | 显式有界公共语料下载、ASCEND 待审候选、独立人工标签导入和 YODAS 元数据准备 |
| [asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md) | 以后再做 / 明确不做 |
| [release-checklist.md](release-checklist.md) | 发布前检查 |
| [context-e2e-checklist.md](context-e2e-checklist.md) | Context / 交付手工验收 |

## 维护工具

- [依赖归属与原文生成工具](../scripts/research/third-party-notices/tools/README.md)：按锁定依赖生成外部 staging，保留原文、引用类型、缺失和来源身份；测试使用离线 fixtures。

## 仍进行中的计划（不代表当前行为）

这些计划仍有未关闭目标；已实现的切片不代表整体完成。实现或决策终止后，再删除计划和本索引入口。

| 文档 | 内容 |
| --- | --- |
| [Chatterfly 改进计划](chatterfly-improvement-plan-2026-10-26.md) | 尚未关闭的分阶段改进、验证协议、成功门槛与回退条件 |
| [Cleanup 与纠错学习 harness 改进计划](cleanup-learning-harness-plan-2026-11-05.md) | 尚未关闭的编辑观察、归因、作用域规则、晋升/召回、prompt 消融与学习 episode 门槛 |

## 设计参考（归档，不作为实现规范）

| 文档 | 内容 |
| --- | --- |
| [Voice HUD 设计参考](design/voice-hud/README.md) | 唯一批准的 Beam v10 状态图集与可重建预览；当前行为以本页活文档和源码为准 |

## 研究对照（结论可能仍有效，实现状态会过期）

| 文档 | 内容 |
| --- | --- |
| [competitive-research.md](competitive-research.md) | 竞品分层与能力对照 |
| [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md) | 无自有模型时的纠错 harness |
| [cleanup-learning-source-research-2026-10-20.md](cleanup-learning-source-research-2026-10-20.md) | Cleanup prompt、编辑归因与学习的固定源码对照、评估设计及来源索引 |
| [speaker-filter-pilot-research-2026-10-25.md](speaker-filter-pilot-research-2026-10-25.md) | 滤声试验结论、反例复现、证据限制和离线评估入口 |
| [asr-cleanup-pipeline-research-2026-08-25.md](asr-cleanup-pipeline-research-2026-08-25.md) | 2026-08-25 管线研究快照 |
| [asr-leaderboard-2026-09.md](asr-leaderboard-2026-09.md) | 2026-09 英文 / 中文 ASR 榜单与插头取舍 |

已落地的 task brief、spec、plan、验收快照不要留在仓库。持续研究保留可复用的方法和结论，不把历史状态改写成当前事实。实现状态以活文档和源码为准；测试数量以当场跑出来的 `cargo test --manifest-path src-tauri/Cargo.toml` 和 `npm test` 为准。
