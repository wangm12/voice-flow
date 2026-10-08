# VoiceFlow 文档

对照当前代码阅读。研究稿里的「今天 / 现状」以本页「活文档」和源码为准。Agent 约定见仓库根目录 [AGENTS.md](../AGENTS.md)。

## 活文档（应对齐代码）

| 文档 | 内容 |
| --- | --- |
| [README.md](../README.md) | 产品说明、设置页、本地开发、签名 |
| [privacy.md](privacy.md) | 数据流、本地保留、Keychain、删除 |
| [end-to-end-workflows.md](end-to-end-workflows.md) | 首次配置 / 本机麦克风自检 / 词典反馈 / batch 预取 / Soniox 实时流与恢复 / AssemblyAI Dictation + raw Sync / Qwen Audio Message / 口述 / 长录音与完整音频恢复 / 六类选中文本操作 / 手动看屏幕 / 后端交付撤销命令 |
| [cleanup-evaluation.md](cleanup-evaluation.md) | 确定性本地整理评测、显式 WAV → ASR → 整理评测入口与可选 live provider 协议（不是当前 runtime/E2E 验收结果） |
| [asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md) | 以后再做 / 明确不做 |
| [release-checklist.md](release-checklist.md) | 发布前检查 |
| [context-e2e-checklist.md](context-e2e-checklist.md) | Context / 交付手工验收 |

## 研究对照（结论可能仍有效，实现状态会过期）

| 文档 | 内容 |
| --- | --- |
| [competitive-research.md](competitive-research.md) | 竞品分层与能力对照 |
| [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md) | 无自有模型时的纠错 harness |
| [asr-cleanup-pipeline-research-2026-08-25.md](asr-cleanup-pipeline-research-2026-08-25.md) | 2026-08-25 管线研究快照 |
| [asr-leaderboard-2026-09.md](asr-leaderboard-2026-09.md) | 2026-09 英文 / 中文 ASR 榜单与插头取舍 |

已落地的 task brief、spec、plan、验收快照不要留在仓库。实现状态以活文档和源码为准。测试数量以当场跑出来的 `cargo test --manifest-path src-tauri/Cargo.toml` 和 `npm test` 为准。
