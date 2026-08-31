# VoiceFlow 文档

对照当前代码阅读。研究稿和计划里的「今天 / 现状」以本页「活文档」和源码为准。

## 活文档（应对齐代码）

| 文档 | 内容 |
| --- | --- |
| [README.md](../README.md) | 产品说明、设置页、本地开发、签名 |
| [privacy.md](privacy.md) | 数据流、本地保留、Keychain、删除 |
| [end-to-end-workflows.md](end-to-end-workflows.md) | 口述 / 长录音 / 选区 / Undo |
| [cleanup-evaluation.md](cleanup-evaluation.md) | AI 整理评测协议 |
| [asr-cleanup-later-and-wont.md](asr-cleanup-later-and-wont.md) | 以后再做 / 明确不做 |
| [release-checklist.md](release-checklist.md) | 发布前检查 |
| [context-e2e-checklist.md](context-e2e-checklist.md) | Context / 交付手工验收 |

## 研究对照（结论可能仍有效，实现状态会过期）

| 文档 | 内容 |
| --- | --- |
| [competitive-research.md](competitive-research.md) | 竞品分层与能力对照 |
| [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md) | 无自有模型时的纠错 harness |
| [asr-cleanup-pipeline-research-2026-08-25.md](asr-cleanup-pipeline-research-2026-08-25.md) | 2026-08-25 管线研究快照 |

## 历史快照（不要当现状）

| 文档 | 内容 |
| --- | --- |
| [acceptance-evidence.md](acceptance-evidence.md) | 2026-08-11 验收笔记 |
| [technical-audit.md](technical-audit.md) | 2026-08-04 UI 审计 |
| [superpowers/specs/](superpowers/specs/) | 当时的设计稿 |
| [superpowers/plans/](superpowers/plans/) | 当时的实现计划 |

测试数量以当前 `cargo test --manifest-path src-tauri/Cargo.toml` 和 `npm test -- --run` 为准，不要抄快照里的旧数字。
