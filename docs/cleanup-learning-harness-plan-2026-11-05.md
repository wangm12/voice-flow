# Cleanup 与纠错学习 harness 改进计划

日期：2026-11-05（America/Los_Angeles）

状态：实施中；部分确定性、隔离研究与 RemoteNamed recorder 子切片已有工程证据，A1 全部能力、生产 paste/learning 链及质量验收仍未完成。本文描述目标与实验，不表示整体能力已实现或质量已提升。

生命周期：工作落地或终止后，将实际行为合并到活文档，删除本文及索引链接。

## 1. 目标与结论

让用户在目标 App 中纠正一次听写之后，系统能可靠地观察这次编辑，区分拼写纠正与内容修改，在适当范围内形成可撤销的规则，并用之后的新句子、新录音证明用户少改了字。

本轮以现有 harness 为基础。优先改善编辑覆盖、错误归因、作用域与闭环测量，然后用独立实验调整检索、晋升与 cleanup prompt。第一阶段固定模型、prompt、晋升次数和权限边界，避免同时改多个因素后无法归因。

完成条件包括四种不同事实：

1. **捕获**：观察到了本次插入范围内、已稳定的用户修改。
2. **学习**：有足够独立证据形成某条、某范围的规则。
3. **应用**：后续请求实际使用了这条规则，且最终输出保留了正确结果。
4. **收益**：新任务所需的人工编辑减少，原本正确的内容没有被额外改坏。

数据库写入、候选晋升、术语进入 prompt 或本机替换，分别只能证明对应步骤，不能单独证明第四项。

## 2. 范围、边界与已有计划的关系

### 2.1 包含

- 自动输入后的同字段学习、History 明确编辑、手动词对确认之间的证据关系。
- 观察窗口、编辑范围定位、局部候选提取与错误类别归因。
- 场景规则身份、独立计数、冲突、撤销和重启后的持久化。
- 词汇提示、已知错拼替换、风格偏好三种用途的明确区分。
- 后续真实机会的应用验证、负样本、误改、净编辑量与资源开销。
- 保持现有保护检查的 cleanup prompt 消融实验。
- 让用户理解候选、已生效、实际使用、未学习原因的有限文案改进。

### 2.2 不包含

- 训练自有 ASR/cleanup 权重、LoRA、上传纠正供训练。
- 默认截图、常开音频采集、跨窗口扫描、全局键盘监听。
- 会议记录、工具执行、跨 App Agent 或新增语音供应商。
- 默认更换模型，或为命中率放宽实体、否定、代码、布局与交付保护。
- 自动读取私人 History/剪贴板补研究数据，或新增普通用户完整字段日志。
- 新向量数据库、后台 LLM 纠错分类服务、动态自我改写的长期 system prompt。
- PR、提交、发布、部署，以及顺手修复无关代码。

### 2.3 必须保留

- 学习权限与窗口内容读取、OCR、云端视觉、文字发送权限独立；本机学习不扩大 provider 数据授权。
- 仍要求验证交付、当前目标、同一字段、当前会话代际与未取消状态。权限撤销、Clear All Data、开始不兼容操作和退出必须使相应观察失效。
- 只处理本次插入关联的内容。字段不可读、身份不明或范围歧义保持 unknown/abstain。
- 临时 ASR partial 不进入学习。模型结果不自动成为人工 gold。
- 保留现有 tombstone、通道内观察去重和独立选区操作边界。人工词条的独立所有权及跨通道来源去重是当前缺口，须在 C0 补齐，不能称为已有保证。
- 学词撤销只改变后续规则，不宣称撤回已经输入到其他 App 的文字。
- 不在 cleanup 返回后重新全局替换所有词对；保留准备稿来源锚定与最终保护。

### 2.4 复用已有项目工作

与 [Chatterfly 改进计划](chatterfly-improvement-plan-2026-10-26.md) 的 M0a/M0b、M1、M4 共用构建身份、数据准入、真实语音基线与学习说明。本计划只细化 cleanup/学习工作流，不重建另一套语音评测或项目分区系统。

确定性 episode 可以先推进；原生交付、人类纠正和真实语音收益分别取得证据。完整语音招募不阻塞确定性契约工作，但缺失时不得宣称真实用户质量验收完成。

B0 优先修正现有 `observe_after_paste_with_identity` 的终止与稳定判定，不另建生产观察系统。AX 通知只作能力研究，采用前必须有原生覆盖证据，并在同一行为变更中更新活文档“不要重做观察循环”的边界。与 Chatterfly M1 共用的文件与合入顺序由实施负责人协调；本文不推断另一项并行工作的完成状态。

## 3. 公开资料：实际可确认的做法

资料在本次研究中查询；外部网页和 main 分支可能继续变化。实施实验前应冻结实际使用的源码 commit/文件摘要与资料访问日期。以下区分官方产品声明、可读源码与本项目建议，不推断闭源内部模型或 prompt。

| 来源 | 可确认做法 | 可借鉴之处 | 未知或不适用 |
| --- | --- | --- | --- |
| [Typeless Dictionary](https://www.typeless.com/help/quickstart/history-and-dictionary) | 纠正听写中的词后，自动记住偏好拼写并放入 Auto-added；提供手工词条管理 | 拼写学习有明确可管理对象 | 本次找到的官方资料没有公开生产 prompt、阈值或候选算法 |
| [Typeless Personalization](https://www.typeless.com/help/quickstart/personalization) | 官方描述学习正式/随意、简洁/详细等抽象写作模式，可关闭 | 风格偏好与专名拼写分别处理 | 数据处理声明属于厂商说明，不等于本次独立验证其内部实现 |
| [Wispr Dictionary](https://docs.wisprflow.ai/articles/4052411709-Teach-Flow-your-words-with-the-dictionary) | 自动学习筛选普通词、数字、日期、路径等；区分词汇与错拼替换；完整词匹配，重叠偏好长短语；删除后不会自动重新加入 | 记词、替换、删除语义与反例筛选 | 没有据此获得其完整 cleanup prompt 或精确学习模型 |
| [Handy 默认 prompt](https://github.com/cjpais/Handy/blob/main/src-tauri/src/settings.rs) | 拼写/大小写/标点、数字和口述标点规范化、去填充词、保持语言/意思/顺序、不回答转写问题 | 单次 cleanup 的清晰任务契约 | 默认 prompt 不等于纠正学习闭环；数字规范化策略不能直接照搬 |
| [FreeFlow 默认 prompt](https://github.com/zachlatta/freeflow/blob/main/Sources/PostProcessingService.swift) | 分硬约束、改口、指令保留、场景格式、开发语法；上下文用于已说词的拼写参考；选中文本编辑另有 prompt | cleanup 与显式文本编辑分开，防止补写未说出的内容 | 其改口推断比 VoiceFlow 的本机证明边界宽，不能直接移植 |
| [TypeWhisper 学习实现](https://github.com/TypeWhisper/typewhisper-mac/blob/main/TypeWhisper/Services/TargetAppCorrectionLearningService.swift) | 有界观察、提交信号、1/2/3/5 次选项、30 天 pending 证据有效期，以及主动纠正入口 | 生命周期与失败状态值得参考 | 包含全局按键监听；VoiceFlow 不采用该机制，也不复制其实现代码 |
| [Amazon contextual ASR 研究](https://www.amazon.science/publications/contextual-asr-with-retrieval-augmented-large-language-model) | 使用首遍 ASR 假设与词汇检索进行上下文纠正，研究也涉及训练条件 | 先召回相关候选，再约束纠正 | 论文收益不能当作本项目收益，也不能将其方法一概称为无训练 |

没有充分证据证明 Typeless 的效果来自某一段特别的 prompt。可执行的参考是：自动学习稀有词、区分风格、限制纠正范围、保留撤销，以及验证后续使用效果。

### 3.1 Text cleanup data and implementation references (checked 2026-11-06)

这些页面用于界定候选文本输入与权利/评测边界。本次仅按固定 commit 的 Disfl-QA README 许可与来源说明核对来源，并将 `dev.json` 捕获到仓库外私有临时目录；没有读取 train/test、导入项目或实验、运行模型或生成分数。其他页面仅查阅公开说明。

| 来源 | 页面明确的信息 | 允许的研究用途与限制 |
| --- | --- | --- |
| [Disfl-QA（固定 commit）](https://github.com/google-research-datasets/Disfl-QA/tree/1f0c16171c77b3d3408be92c485f11b8998a9189) | 页面标注 CC BY 4.0；基于 SQuAD-v2 问题的 `dev.json`（201,742 bytes，SHA-256 `b60e075b810b27a5130fd0aa2cfbc85753b71a0b30dcd2585f540f0a6afe6492`）已在仓库外私有目录准备。只读验证确认 1,000 个 original/disfluent 非空文本对、1,000 个不同 original、无重复 original 或相同文本对；未读取 train/test。它是问答文字，不是录音或用户纠正。 | 可另作文本 disfluency 能力案例；不代表真实听写、真人纠正、held-out 评估或 VoiceFlow 质量收益。当前只准备了输入快照，未导入实验或运行模型。 |
| [SpeakoFlow dictation-cleanup-examples](https://huggingface.co/datasets/SpeakoFlow/dictation-cleanup-examples) | 页面标注 CC BY 4.0、105 个手写案例和 15 类；作者说明这些是训练池案例、不是 benchmark 或 held-out，真实 held-out 未公开，且不是采集语音。 | 仅可考虑用于约定检查和 negative coverage；不得作 held-out、报告分数或真实语音/用户质量证据。 |
| [SpeakoFlow Mini model card](https://huggingface.co/SpeakoFlow/speakoflow-mini) 与其[规则层](https://github.com/AbhishekBarali/dictation-cleanup-rules) | 作者公开模型专属 prompt，并说明确定性 filler、重复、标点、大小写、数字等规则先于模型运行；规则仓库页面标注 MIT，并提供 spec、conformance cases 与 Python/TypeScript 实现。 | 只供未来独立 E 变量设计和人工查阅；不安装、vendor、复制或移植规则代码。不得把该模型的 spoken-command policy、全局 em dash 禁令或不设 token cap 的建议带入当前 prompt/P2，也不采用其自报分数作为本项目证据。 |
| [Transcription-Cleanup-Trainer](https://huggingface.co/datasets/danielrosehill/Transcription-Cleanup-Trainer) | 公开卡片描述 audio、Whisper transcript 与手工 cleanup 配对，但 license 字段标为 Private。 | 不因页面可见而下载、导入或使用其样例。 |
| [dictation-speech](https://huggingface.co/datasets/ductai199x/dictation-speech) | 卡片说明语音为合成 TTS，并按记录保留来源及 license 信息。 | 可作为另行审查权利后的合成 stress input；不能算真人语音、人工纠错或真实场景质量证据。 |

任何后续使用仍须单独固定来源 revision、核对权利与归属，并在实验前划分开发和真正 held-out 输入。Disfl-QA 快照只保存在仓库外；本次没有把任何材料加入仓库或当前评测集。

## 4. 当前代码基线

以下以工作区当前源码为准，不以旧研究稿的“今天”作为实现状态。主要文件已有大量未提交改动，实施与评审必须读取工作区内容，不只看 HEAD。

| 环节 | 当前行为 | 源码 owner |
| --- | --- | --- |
| 本机准备 | 布局解析、场景内词典/同音替换、有限明确改口，再形成同一份准备稿 | [lib.rs](../src-tauri/src/lib.rs)：`prepare_cleanup_transcript_with_resolved_rules` |
| 场景筛选 | 已晋升词对按已声明 scope 的各维度匹配，未知值与不匹配分开 | [store.rs](../src-tauri/src/store.rs)：`LearnPairScope::applicable_promoted_records` |
| 替换 | 最长优先、拉丁词边界、CJK blocker、literal 保护；本机准备记录实际应用事实 | [lexicon.rs](../src-tauri/src/lexicon.rs) |
| Cleanup 提示 | 本句 hit pairs，有数量/字符预算，不回退倾倒整本词典 | `lexicon.rs::hit_pairs`、`llm.rs` |
| 最终保护 | 准备稿中的来源锚定词对、实体、否定、语言和布局受保护；破坏时拒绝候选 | `lib.rs::guard_final_output_with_reason`、[protected_span.rs](../src-tauri/src/protected_span.rs) |
| 观察准入 | 验证交付后，同一目标字段；需要现有 context/mapping/学习授权；敏感目标拒绝 | [dictionary_learn.rs](../src-tauri/src/dictionary_learn.rs)：`maybe_observe_after_paste` |
| 观察时序 | 初始 3000ms，间隔 400ms，编辑后 idle grace 1500ms，上限 12000ms；失焦返回 LeftTarget | `observe_after_paste_with_identity`、`ObserveLimits` |
| 范围定位 | 本次插入文本须唯一匹配，原有非空 prefix/suffix 须一致；suffix 为空时后续追加内容会并入 segment | `edited_inserted_segment` |
| 词对累计 | 普通词默认 3 次，中文人名默认 2 次；支持人工确认、忽略、撤销 | `record_learn_pair_with_observation_audited`、`promote_hits_for` |
| 观察去重 | paste session 和 History 各有收据；同一 History 的同词对多次保存不重复加证据；两通道没有共同 dictation origin | `store.rs::LearnPairObservation` |
| History 自动学习 | 从前一 adopted text 比较；对数字、否定、跨文字体系、普通拉丁词等保守拒绝 | `dictionary_learn.rs::history_lexical_correction` |
| 本机场景规则 | local-only 路径与 History 的自动学习可仅生成 scoped rule，不自动加入全局个人词典 | `persist_learn_pair_locked`、`learn_from_manual_history_edit` |
| AX 文字授权路径 | 自动晋升还会追加全局个人词典；ASR 术语收集对该词使用 scope 排序而非 scope 排除；全局词也可供显式启用的 fuzzy 使用 | `persist_learn_pair_locked`、`lexicon.rs::collect_ranked_terms` |
| 词条所有权 | 全局词典为字符串列表，没有完整的独立人工所有权；scoped flags 不能替代该信息 | `add_dictionary_entries`、`should_remove_dictionary_entry_after_undo` |
| 风格 | 已有 style draft 人审；部分短改写方向累计后可改变场景 cleanup intensity | `apply_post_paste_style_learn_for_session` |
| 审计 | 普通运行有界元数据；来源/历史撤销理由可为 unknown，不证明原生交付或质量 | [dictionary_learning_audit.rs](../src-tauri/src/dictionary_learning_audit.rs) |
| 研究 | 显式 fixture lease、selected pairs、固定映射和目标约束；已有 Unseen 等 purpose | [learning_research.rs](../src-tauri/src/learning_research.rs)、[research_trace.rs](../src-tauri/src/research_trace.rs) |

### 4.1 已知限制及其触发

| 编号 | 触发与现有结果 | 后果 | 性质 |
| --- | --- | --- | --- |
| L1 | `next_unseen_utterance` 是 `not_measured` 或本机准备质量未知 | 无法仅凭目前记录判断下次是否变准 | 已确认的测量缺口 |
| L2 | 读完文本后才开始改，超过初始窗口；或改完很快失焦 | 这次编辑可能没有学习结果 | 明确行为；实际损失比例待测 |
| L3 | 同句两处修改、纠正后继续输入、重复出现相同插入文本 | B1b bounded text 路径对可唯一定位的 1–3 个局部 hunk 提取完整词对；移动、重复上下文歧义或越界编辑整体 abstain。真实 native continuation / IME 仍未验证 | 文本算法反例已在 deterministic slice 覆盖；原 A0 记录保留为历史基线 |
| L4 | 相同 before/after 在第二个 scope 学习 | `pair_key` 不含 scope，已有记录触发 scope conflict | 已确认的数据表达限制 |
| L5 | 跨文字体系、纯大小写、连续中文名字边界不明 | History/local-only 保守路径拒绝或不形成自动词对 | 有意保护带来的覆盖缺口 |
| L6 | 多条规则在同一 scope 对同一 before 给出不同 after | 需要显式定义竞争状态，不能依赖查询顺序决定语义 | 必须覆盖的规则冲突路径 |
| L7 | 短句内容变化、纠正后追加口语或新内容 | 长度/口语启发式可能计入强度上调或下调 | 静态可见风险，需反例与人工标签验证 |
| L8 | 手动加入某词后，AX 学到相同 after 的非 scoped 词对，再撤销最后一条相关词对 | 全局人工词也可能被删除，当前无完整所有权依据 | 已确认的调用链缺口；C0 明确修复 |
| L9 | 同一次听写先在目标字段被纠正，再在对应 History 保存相同词对 | 两种 source_kind 的收据可各计一次，尚非独立听写证据 | 已确认的跨通道去重缺口 |
| L10 | 多 hunk 被公共前后缀包成一个 span，或 AX 路径出现内容/日期变化 | B1b 对 bounded text 路径逐 hunk 做严格资格检查；任一不安全 hunk 会使整次编辑 abstain，不落安全旁词。History/local-only 仍保持自己的单候选 gate | 针对性 extraction / content tests 已通过；不代表真实 AppState 多 hunk end-to-end |
| L11 | 观察到达总时限前仍有 dirty edit，或 IME 选词停顿 | B0 当前在 dirty edit 未取得 idle-grace 后稳定读回时以 ambiguous abstain，不把 deadline 的最后样本当作稳定结果；IME composition/commit 状态仍未知 | dirty-deadline 路径已由当前回归覆盖；真实 IME 提交行为仍待实测 |
| L12 | 活跃 A→B 后用户反复把 B 改回 A | 当前没有专门的反向关系冲突策略，可能形成同时活跃的 B→A | D 需明确 challenge/冲突处理，不能假设已有保护 |

现有本机应用事实不等于观察器已经拿到完整归因：其调用没有各层文本或可靠 History 关联；派生同音规则、fuzzy 命中也不总能映射回具体已学词对。归因覆盖缺失须报告 unknown。

不能把 no_grant/no_mapping/unreadable 当作分类器漏学，也不能把 local-only 规则不进入云端 ASR 提示当作故障。

## 5. 目标架构与责任划分

```mermaid
flowchart TD
    A[ASR 原稿] --> B[现有本机准备与来源证明]
    B --> C[受约束的 cleanup]
    C --> D[现有最终保护]
    D --> E[验证交付与本次插入身份]
    E --> F[有界同字段观察]
    F --> G[局部 diff 与编辑归因]
    G --> H[候选 / 冲突 / 不适用]
    H --> I[独立证据与作用域内晋升]
    I --> J[精确 alias / 有限召回 / 已批准偏好]
    J --> B
    J --> K[已有权限允许的 ASR 提示]
    E --> L[受控 episode 裁判]
    I --> L
    D --> L
    L --> M[后续新句收益与误改报告]
```

图中观察/提取/多 scope/闭环裁判的增强是待实施内容；现有准备、最终保护、交付与研究基础继续复用。

| 职责 | 继续由谁拥有 | 本轮扩展方向 |
| --- | --- | --- |
| 交付与目标身份 | `paste.rs`、`delivery.rs`、`lib.rs` | 如需复用插入范围，由现有 owner 输出最小事实；不由学习层重造交付判断 |
| 观察与候选 | `dictionary_learn.rs` | 有界生命周期、局部 edit、类型分类和重复证据 |
| 持久化、原子更新 | `store.rs` | scoped identity、迁移、冲突和 tombstone；明确 DB/settings 跨存储结果 |
| 替换、召回与预算 | `lexicon.rs` | 唯一有效规则集、歧义处理、可归因使用事实 |
| 请求和最终编排 | `llm.rs`、`lib.rs` | prompt 实验、来源保护、原有取消与权限复验 |
| 正常用户说明 | 现有 Dictionary/History/学习审计 | 从真实 owner 事实派生，缺失不猜测 |
| 受控研究 | `learning_research.rs`、`research_trace.rs`、`evaluation/` | 独立 episode 协议与外部裁判，不重复生产算法 |

## 6. A：先建立学习 episode 基线

### 6.1 一条完整 episode

以下描述目标协议；A0 只能执行当前入口能表达的子集，完整步骤的独立裁判、输入源和来源回执由 A1 补齐。

1. 在隔离研究状态中读取所选规则的初始身份、scope、计数和 tombstone；不自动清空用户数据。
2. 固定构建、模型、prompt、强度、权限和目标，执行第一次听写或明确标为 deterministic 的转写输入。
3. 确认目标中实际出现了什么；保留产品回读与独立 AX/DOM 裁判的区别。
4. 执行已审核的编辑序列，或由同意参与的人实际纠正；程序编辑与人类编辑分别标注。
5. 等待观察与持久化 owner 终止，读取实际候选、独立计数、scope 与结果；不靠固定 sleep 猜完成。
6. 使用不同句子/录音再次出现目标词，检查 raw、prepared、候选、最终输出和目标实际内容。
7. 运行相似普通词、不同场景、代码/literal、原本正确词等反例。
8. 执行显式撤销，核实后续不再由该规则产生效果；人工词典或其他规则仍存在时单独说明其贡献。
9. 需要时重启隔离研究 App，核对持久化与撤销状态，不将内存审计丢失误认为规则丢失。

### 6.2 当前研究入口的缺口

旧 `LearningConsent` 约束单一 native mapping/bundle、最多 8 个 selected term、每个字段最多 1024 字节，并拒绝浏览器、style 样例及未授权文字发送。真实麦克风研究 lease 仍要求麦克风授权，一个 lease 只能 Start 一次。另有 feature-gated `voiceflow-learning-harness` 独立 stdio 入口，不启动 Tauri、加载普通 app data 或访问凭据。确定性文本使用 `local_edit_learning` 路线：学习候选和撤销保存在隔离 arm 的 SQLite 中，promote/tombstone 不写全局 Settings.dictionary；合法预置词原值保持，操作回执注明不需要 Settings 保存。原生入口没有放宽已有 owned-WAV 学习抑制。

source 文本与 2–6 个有序完整字段状态须提前声明并与实际输入匹配；每个 `edit_states[].layer=settled` 是层级授权标签，不表示每个状态已读或单独稳定。回执给实际 observer poll、consumed state 数与 `trajectory_settled`。实时 ASR/cleanup 未命中预测时，记录 fixture prediction mismatch 和准入覆盖，不将它混入产品的词对分类失败。未读取的未来状态不冒充已观察，也不推断未来收益；有授权的已观察稳定候选仍按当前生产 owner 正常进入 SQL。

standalone V2 selected-state 回执会给出已选行状态、hits、tombstone、行摘要、固定 fixture scope 和 arm settings hash/一致性；operation 回执区分 SQL 结果与 `not_required_local_only_scoped_pair`，不伪造没有的 Settings 写入。受控 WAV receipt 可保留实际 raw ASR 文本与摘要；它不采集 prepared/provider-candidate 与完整 pipeline 层级，也不提供实际 target readback。文本 fixture 的 source、编辑状态和预期输出是程序预声明内容；实际 local replacement 输出由 owner 计算并与预期独立比较。摘要和 `purpose=Unseen` 不是这些能力的替代品。

研究协议 V3 增加同 arm 的有界 scope phase plan：全部 mapping 身份随计划固定在一份隔离 Settings 中；每个 phase 有独立 source、scope、权限与 TTL，且最多一个活动 lease。`LearnText`、`ApplyFollowup`、`Undo` 可在不同明确 scope 中顺序调用原有 owner，并保留实际 rule ID、stored/requested scope、应用结果和独立 operation receipt。当前 production `pair_key` 对相同词对仍只支持一个 owner：其他 scope 的应用会被真实适用集合排除；同词对跨 scope 学习冲突会如实记录 C1 known gap，不创建第二数据库或伪造新 key。普通 V3 学习计划接通 programmatic deterministic text（本轮只验 synthetic fixture），并要求 `target_oracle=none`。单独的 target-only `ObserveTarget` 计划现已接通固定注册的 owned FixtureHost 与 AX reader：每个操作启动新的 Host，读取已注册 target control 并独立比较预期 hash / UTF-8 长度，随后等待 helper 与 Host 退出；该组件采样不调用产品 insertion / learning owner，也不形成同 Host 的 paste-to-correction 链。

因此 A 分两步：

- **A0：现有实时麦克风的单 App/两态 fixture 冒烟，加确定性单元与集成契约测试**。复用当前可访问的生产函数和隔离临时状态检查候选、累计、准备与 guard；现有 evaluator 的固定 `local_rules.fixture` 可测后续文本应用，但不证明学习、scope 或原生交付。当前入口不能取得的最终质量保持 unknown。
- **A1：逐个扩展版本化研究协议能力**：deterministic-text→本机 scoped 候选→local replacement/usage→undo/tombstone→同 arm 重开、local-only Settings 保持，以及有界 multi-scope phase stdio slice 已由 Sol 实际验收。2/4/6 状态轨迹分别实际 poll/consume 2/4/6 次；N7 在 arm 写入前拒绝。4 态 `[A,B,B,C]` 提前稳定时 poll 2 次、consume 3 态、`trajectory_settled=false`，实际词对 mismatch 且零 row；5 态 `[A,B,B,D,B]` 同样在未读后续状态时提前稳定，消费的词对与授权项相符，因此 owner 写入该候选。两例均不把未读 future state 或 future benefit 记为 observed。V3 同一 arm 保留 A/B 两套 mapping：A 的生产 local-only 规则可被 A 适用；B 应用通过生产 scope filter no-op；B 学同一 pair 返回真实 legacy-key 冲突且不增 SQL；pending 与 post-undo 两 scope 均只产生 no-op；Undo 只接受引用的精确 positive application receipt。合法预置 global word 保持原值，local-only learning/undo 不写 Settings。真实 public WAV 曾走通现有 on-device ASR 并返回 raw transcript，但 `loaded_attestation` 仍停在共享 completion gate；本次实际入口没有学习 SQL/Settings 写入，因此不能签 WAV→学习。独立 owned native Host / AX reader 已接入 V3 target-only `ObserveTarget` 并通过固定目标的一次真实组件采样；剩余 A1 包括同 Host 的真实 paste→预声明 programmatic correction→独立 final AX/read-owner 关闭链，以及 complete-attestation 后的受控 WAV 学习。人类/浏览器/真实 IME/产品 paste 及 ASR/cleanup 质量必须分别由对应真实输入与裁判证据签收。产品 scoped/casing rule identity 留给 C1，不由 V3 模拟解决。

A1 对每种新增 source 和 follow-up 都须先绑定准确来源身份及学习授权，再接线；不能关闭 `settings_match`、fixture mismatch 或 target guard 来让测试通过。V3 的一个 immutable plan 可声明多个 mapping scope phase，但每次仅打开当前 phase 的独立 grant/TTL，不继承上一步权限；同一 pair 的跨 scope owner 仍受 production legacy identity 限制。普通学习 phase 继续要求 `target_oracle=none`；独立 target-only `ObserveTarget` phase 使用 source-pinned helper、固定 Host bundle / control binding 与独立 target grant。该单次组件采样现已在真实 stdio 入口复验：Host identity 前后匹配，sample 与固定 expected SHA / 长度相符，owned child exit 已确认，且 arm root 无学习 DB / Settings 写入。每个 operation 重新启动 Host；它没有 product insertion、same-Host edit 或 final pipeline observation；stdio 也没有 in-flight CLI cancel 输入。后续 B1 same-target unit 必须从原 delivery owner 持有同一真实 AX anchor/Host identity，不能把此 one-shot sample 冒充产品 paste 或纠错证据。

standalone 的研究 WAV 来源声明内容 SHA/长度/权利和学习授权，经只读 source root 到 research-only mutable work root 的受控复制，并使用现有取消/owner-completion 路径。原生 owned-WAV 学习抑制未解除；研究 source 的学习仍需完整 attestation gate。每次研究输入使用独立 private arm root；不在普通日常 bundle 的数据目录运行实验。

### 6.3 研究输入最小概念契约

以下仍是完整研究计划概念，不代表全部已实现。V2 episode 已表达 deterministic-text source、rights/grant、固定单一 scope、2–6 个有序 source/edit states、operation IDs、可选 follow-up source/output hash、undo operation 与 research WAV/model identity。V3 已表达并实际运行 programmatic deterministic-text 的多 scope phase、单活动 lease、独立 phase grants、production applicability/no-op、pair-key conflict facts 与带独立授权的 undo；target-only `ObserveTarget` 也已通过固定注册 FixtureHost 的一次独立 AX 采样。产品 rule identity、独立收益裁判和 WAV learning gate 仍待实现或授权；该 one-shot target sample 不构成产品插入或同 Host 纠正链：

| 对象 | 必需信息 |
| --- | --- |
| Episode | schema/protocol version、episode ID、目的、构建/配置/prompt 摘要、独立 arm state ID |
| Source | audio 或 text 的准确类型、来源与权利、内容摘要、speaker/session/template/term family |
| Edit | human/programmatic、所选目标身份、插入范围、审核过的有界编辑步骤与最终预期、可接受分类 |
| Rules | 被选规则的初始状态/版本、scope、别名、预期证据次数；所有状态缺失明确记录 |
| Follow-up | 新任务 ID、是否同实体/新句/新录音、正例或何种反例、预期允许的规则作用域 |
| Oracle | 独立目标读取、人工判断、产品 owner 事实分别记录，不相互冒充 |

所有输入在访问 provider、模型或目标前校验；文本轨迹、候选和人工 gold 放仓库外。普通运行不新增文本研究记录。

### 6.4 错误归因

优先读取现有 `asr_text`、raw/prepared、provider candidate、最终输出、History manual revision 与 trace。正常记录缺失的层级保持 unknown；完整层级只在明确授权 fixture 中采集。

| 来源情况 | 归因及学习动作 |
| --- | --- |
| ASR 已错，准备与 cleanup 都未修复，用户纠正 | 可形成 alias/术语证据，仍需判断是否拼写纠正 |
| ASR 正确，本机替换改坏 | 关联实际应用规则的负反馈；不创建反向全局替换 |
| 准备稿正确，cleanup 改坏，用户恢复 | cleanup 回归证据；仅凭最终 diff 不能称 ASR 学习 |
| 所有层文字一致，用户改变事实/决定 | 本次内容编辑，不建立长期词对 |
| 层级数据不足或范围不明 | unknown，不推断模型原因 |

仅凭 AXValue 变化不能证明是人类而非目标编辑器自动格式化。fixture 标签、真实人类编辑与来源未知字段变化分开报告，不用“同字段发生变化”作为人工 gold。

## 7. B：改善观察覆盖与编辑提取

### 7.1 生命周期

候选设计状态：`admitted → observing → dirty → stable_candidate → closed`。任何阶段都可因取消、授权撤销、身份丢失、范围歧义或 superseded 转为 abstain/cancelled。

- 测量首次编辑延迟与最后编辑到离开字段的间隔，比较当前窗口与候选窗口；先不增加一组用户设置。
- 对目标实际支持的 AX 变更通知做能力实验。复用有界轮询兼容路径，避免每次通知创建独立 observer 或重复计数。
- 离开目标后立刻停止读取，不到新 App/新字段读取“最后一次值”。
- 只有离开前已经取得充分稳定、范围唯一、权限仍有效的编辑才能考虑完成候选。失焦/提交是停止信号，不是正确性证明。
- 改变当前“循环结束即 finalize”的行为：timeout 到达时 dirty 不自动升级为 stable；IME/composition 状态未知时不把最后一个片段认作完成纠正。此变化是 B0 的单独回归与回退项。
- 不把 Return 一概当作发送，不安装全局按键监听。无可靠提交证据时依然可按稳定编辑策略结束。

“稳定”需要在实现前定义最小采样、时间与输入法处理条件；不能仅凭两次相同文字宣称不是组合输入。原生 IME 不可判定时保守拒绝，或通过现有 History 确认流程取得明确证据。

### 7.2 插入范围

优先复用交付 owner 已掌握的目标 anchor、插入范围及选区事实。范围只供当前有界观察使用，不把完整字段做成长期记忆。

第一版 bounded text path 已支持最多 3 个互不重叠局部 hunk，并以完整词边界提取词对；词对提取遇到超过上限、仅插入 / 删除、文本移动 / 重排、对齐仍有歧义的重复上下文或无法唯一定位时整次 abstain。若重复 hunk 各自通过资格检查且指向同一 pair，可在此后去重。确定性及隔离 stdio slice 覆盖了有界编辑输入；显式 style allowlist 独立处理获准的整次转换。真实 native continuation、IME 和同 Host 产品交付链仍未验证。

开始 B1 前先列出交付到观察的最小事实契约：现有目标 anchor 引用、会话/权限代际、已确认插入的 UTF-16 范围及来源长度、必要的短期锚点和 readback 状态。范围转换必须覆盖 surrogate pair、组合字符和 CJK；未能取得这些事实时维持当前保守路径，不发明成功的插入区间，也不持久化完整字段。

每个 hunk 的 before/after 都须通过类别边界判断。标注为两个独立词纠正的 `TypeScrit and Reakt → TypeScript and React`，不能因符合 2–4 词长度就合并成含未修改连接词的替换。`jonn tomorrow please → John tomorrow` 也不能学成整段 before→John。真正的短语纠正单独标注与计分，不从单词候选数推断。

允许范围外继续打字不等于学习范围外内容：只有能用原插入范围身份和一致锚点证明某个 hunk 属于本次输入，才提取该 hunk。不能证明时继续沿用现有保守结果。

### 7.3 分类与晋升分开

| 编辑 | 候选类别 | 默认动作 |
| --- | --- | --- |
| `TypeScrit → TypeScript` | 拉丁专名/标识符拼写 | 经过独立证据后场景内晋升 |
| `知呼 → 知乎` | 中文词语/同音拼写 | 保守自动证据 |
| `配森 → Python` | 跨文字体系 alias | 提供具体词对确认；先不自动放宽 |
| `github → GitHub` | canonical casing | 独立类别，不混入普通 ignore-case key |
| 连续中文中的局部名字修正 | 有限候选边界 | 只有边界唯一才自动；否则确认具体词语 |
| 日期、金额、否定、动作对象变化 | 内容编辑 | 不创建长期替换；只作为明确实验标签 |
| 标点/口语偏好 | style candidate | 走独立批准流程 |
| 大段重写、粘入内容、来源不明 | ambiguous/not_applicable | 不晋升 |

纯插入不自动学词。候选增多不能直接等价于自动学习变宽；每一类别分别记录 precision、recall 与确认成本。

History/local-only 与 AX 文字授权路径的分类覆盖须分别测量。B1b 更新的是 AX bounded text 提取与整次内容拒绝；History/local-only 仍使用原有保守单候选 gate，不能将两条路径描述为行为相同。

B1b 在 bounded text path 中关闭了已观察到的句末 Latin 句点边界问题：`VoiceFlo.` → `VoiceFlow.` 将句末句点排除在 alias 之外，而标识符内部的句点仍属于 token。早期 fixture-term mismatch 保留为历史缺陷证据；新的 deterministic coverage 已覆盖修复后的边界，但不证明真实 AppState paste-to-native-AX 链也通过。

## 8. C：作用域、规则身份与冲突

### 8.1 最小表达目标

逻辑上区分 canonical term、observed alias、scoped rule、独立 evidence。优先扩展现有表与 owner，不先引入通用记忆服务或强制拆成四张表。

目前 `pair_key(before, after)` 只表达文本关系。候选设计让稳定的规则 identity 关联文本关系、实际 scope 与规则类别，避免相同 pair 在第二个 App 发生全局 key 冲突。具体字段、索引与 Tauri 参数变更需在 C 开始时形成最小 schema/API diff，再实施。

大小写类别不能只复用现有全部 lower-case 的文本 key；rule identity、alias match policy 与 receipt identity 必须共同表达该类别需要的大小写差异。相同 origin 的不同合法 hunk/rule 可以各记一次，同一 rule 的重复通知不能多计。

### 8.2 行为契约

1. 新 identity 下，同一 before/after 可以在不同 scope 各自累计证据、应用和撤销；旧全局 tombstone 的保守例外见 8.4。
2. 一个 App 的证据不自动晋升另一个 App 的替换规则；此约束不把现有全局词汇提示误称为 scoped 替换。
3. 保持现有“AX 文字授权路径的全局词汇 + scoped 替换”行为，local-only/History 规则继续不自动进入全局 ASR 词表。scope 匹配的有界 cleanup pairs 仍按当前 route/权限发送；identity 迁移不新增禁用或扩权。若要将 AX 新学词也改为全 scoped，另列行为、隐私文档和 ASR/fuzzy 召回消融，不混入 C。
4. 当前声明 scope 的各维度依然必须匹配；unknown 不变成通配符。
5. 同一有效范围内同一 alias 的多个目标形成 conflict；自动应用 abstain，允许明确确认。不能用 `ORDER BY`、最新时间或列表顺序悄悄选目标。
6. 嵌套或重叠规则只在现有授权匹配集合内比较。长度优先解决匹配范围，不解决两个目标的语义竞争。
7. 忘记一条规则不删除其他作用域规则，也不删除独立人工词条所有权。
8. 修改 mapping 不自动让旧证据获得新目标的权限；删除/重建 mapping 的身份变化必须有明确失效或迁移策略。

### 8.3 C0：补齐来源与人工词条所有权

- 新的手动添加/导入须留下持久化的独立词条所有权事实；一个词可同时有人工和多个学习来源，scoped flags、pin 或 after 恰好存在均不能替代所有权。
- 旧 `Settings.dictionary` 无法证明来源的项迁为 `legacy_unknown`，保留原词。撤销学习规则时不删除人工或 legacy_unknown 的贡献；只有用户明确移除该词，或已证明仅由被撤销来源拥有，才按相应契约处理。
- 关联同一次听写的 paste、History、明确 review 使用共同 opaque dictation origin。同 origin、同 rule 的同一纠正最多贡献一次，revision/session/tool channel 不独立增加证据；人工“确认规则立即生效”与重复证据累计分开。
- 旧 receipt/origin 不可恢复时保留 legacy 计数与 unknown，不猜来源。新策略的自动晋升只依据已证明独立的计数，不能把旧 pending hits 与新 origin 直接相加后称为满足独立次数门槛。既有已晋升规则不追溯清空、降级或复活。
- 当前所有权缺口先在 A0 建立已知失败反例；C0 修复后才能签收相应 G0。包括“手工词→同 after 学习晋升→撤销”、同一次听写经两个通道以及 legacy pending 加一次新 origin 的案例。

### 8.4 C1：迁移与可回退性

- 旧行保留 scope、before/after、hits、manual/auto 标记、忽略与 tombstone；来源未知继续 unknown。
- 旧文本 pair_key 的 tombstone/ignored 今天会跨 scope 阻止被动重学；迁移保留这种全局阻止语义，不只复制成旧 scope 的局部 tombstone。新 identity 创建的忘记按声明范围生效；恢复旧阻止必须由明确恢复动作授权。曾被明确手动重绑的 live 行按当前可证明状态保留，不重建已丢失的历史 tombstone。
- 初期仅迁移 word rule identity；同表的 style/intensity、空 before 的 pin/屏幕词记录按各自 owner 保留，不把它们当作新的词汇证据，也不因 key 编码变化误分类。
- 为旧 key 到新 identity 建立可验证映射，不通过重新观察虚增计数。
- 历史 receipt 去重、History 删除级联、pending 上限和审计投影预算一起验证。
- 原子 schema 迁移后做行数/唯一性/所有权核对，失败不能留下半迁移数据库。
- DB/settings 本来分属不同存储，不能将一次 SQL 成功表述为两者均成功。晋升快路径必须支持已完成部分的明确重试而不多算证据。
- 现有旧二进制会拒绝打开更高 `user_version` 的整个 History。C 升级 schema 后不能直接降级应用；基本回退是在新版本内停用新行为。发布回滚方案须预先验证兼容备份与恢复边界，不覆盖或丢弃迁移后新产生的用户数据。

C 涉及持久化和可能的公开命令参数契约。本文批准的是研究方向；实施时依既有授权判断具体 schema/API 变更边界，不能以“只是内部重构”跳过契约说明。

### 8.5 C0/C1 持久化与 API 候选合同（待 ask-first 确认）

**状态：候选合同，尚未获实施授权。** 本节用于让用户确认具体公共 patch、持久化、IPC 和迁移边界；它不描述已实现行为，也不授权 migration。字段名和流程均为建议，实施前须按当时工作区提出准确 diff。

| 阶段 | 范围 | 候选合同 |
| --- | --- | --- |
| C0 | Settings 与词条来源 | 保留公开 `Settings.dictionary` 的 `Vec` 形态。增加内部持久化 `dictionary_sources`，按词面保留 `manual`、`legacy_unknown`、贡献的 rule IDs，以及 screen 等非 word owner 类型。迁移时无法证明来源的旧词归为 `legacy_unknown` 并保留。明确 add/import 即使词已由自动学习加入，也单独增加 `manual` 所有权；撤销规则只删除该 rule 的贡献，manual、legacy_unknown 和其他来源仍保留词。 |
| C0 | Settings 写入语义 | 当前 add/import 会提交合并后的词典；候选 UI 改为仅提交本次明确添加的词。公开 `update_settings_patch.dictionary` 仍可整表替换：新出现的词取得 manual 所有权，显式删掉的词清除其全局词典效果并取消该词未完成的 `add`，未改变的词不重分类。普通 patch 不可直接覆盖内部 `dictionary_sources`。 |
| C0 | Origin 与 receipts | 复用 paste 前已设置的 `pending_history_transaction_id`，它持久化为 `dictations.transaction_id`；内部传给 paste observer，History 的 origin 从 DB 读取，不能接受前端传入的可信 origin。receipt 增 nullable `rule_key`、`dictation_origin`，并以 `channel` 作 metadata。独立计数只纳入通过学习分类、并被授权 owner 接纳的 correction proof；`learn_observations` 中 `research_fixture_apply/undo` 等操作收据、未准入记录和 unknown 均不计。partial unique index `(rule_key, dictation_origin)` 的条件也只覆盖这类可计 proof，操作收据不得占用 correction-proof 的唯一名额。旧 opaque receipts 保留 unknown，不猜 origin；沿用现有 observation/owner 存储，不新增服务。 |
| C0 | 证据计数与阈值 | 保留旧 `hits` 作历史值；`independent_hits` 从保留中的 distinct proof 行实时 SQL `COUNT` 得出，不另存可漂移累计值。新自动门槛只看当前保留的 distinct proof，不把 legacy hits 加进去。保留现有 4096 裁剪及 History 级联；删除 History 可使 pending 计数下降，已晋升规则不降级。跨 channel duplicate 须引用原 History 行以遵守删除契约。普通词 3 次、人名 2 次和明确确认立即生效维持；确认不增加证据。 |
| C0 | DB / Settings 协调 | 在 `learn_pairs` 增 `dictionary_pending_action`（`none/add/remove`）和 `dictionary_effect_version`。晋升/tombstone 与 action 在同一 SQL 事务写入；持有 settings gate 时按当前 version 幂等同步词权和 dictionary，保存成功后只对相同 version 条件 ack。显式删词在同一 gate 取消该词旧 `add` 并推进 version，旧任务不能复活词。DB 失败不发布 Settings；DB 已成功而 Settings 保存失败时返回明确 partial 状态，不能多记 receipt 或证据。只用上述有限 action/version，不加通用 journal 或永久 ledger。 |
| C1 | Word identity | 普通 `before` 非空的 word rule 候选迁至 `word:v2:<digest>`。digest 输入采用无歧义、有界编码，包含 `kind`、`match_policy`、alias identity、精确 `canonical_after` 及所有 scope 维度；NULL 有单独标记。增加 `rule_kind`、`match_policy`、nullable unique `legacy_pair_key`（旧 key 到新 identity 的唯一映射）；C0 proof、ownership、pending action 引用新 ID。迁移保留 hits、promoted/manual/auto、scoped flags、ignored/tombstone、pin、usage。style/intensity 前缀与空 before 的 pin/屏幕词继续由原 owner/key 管理，不变成 word 或独立证据。 |
| C1 | Alias 与旧阻止 | 普通 ASCII alias 保持 ignore-case；canonical casing 候选用 exact alias，例如 `github → GitHub` 不匹配已经正确的 `GitHub`，此 exact 限定只约束 casing 类候选。普通规则现有同音/近形检索及全局词表资格按既定边界保留，本候选不把它们全局禁用或改变其资格。新增窄表 `learn_legacy_blocks(legacy_pair_key PRIMARY KEY, ignored, tombstoned_at)`，只迁移可证明曾阻止被动重学的旧普通 word。该 block 按旧 lower-case 文本关系继续全局阻止被动学习，不能被 casing policy 绕过；曾重绑且当前可证为 live 的行不伪造旧 tombstone。新 identity 的 forget 只作用于本 scope。恢复旧 global block 必须使用明确的新 restore IPC 或显式参数，并向用户说明全局影响；scoped confirm 不清除它。 |
| C1 | Scope 与 conflict | 先逐维匹配实际 scope；unknown 不作 wildcard。随后只在匹配集合内判断 alias 匹配域相交且 target 不同的规则为 conflict，匹配策略需同时考虑 ignore-case 与 exact。replacement、cleanup hints、guard 和 usage 共用这一个适用集合。长度排序只选匹配范围，不选择目标。确认冲突需明确 disarm 竞争规则或替换它。 |
| C1 | Mapping 生命周期 | `binding_invalidated_at` / `binding_invalidated_reason` 仅表达 `mapping_deleted`、`declared_identity_changed`、`missing_binding_at_migration`。保留 promoted/hits/manual 词和 receipts，不转成用户 forget 或 global tombstone。所有 mapping save/delete 与 `update_settings_patch.context_mappings` 都由同一 settings gate 比较旧、新身份：ID 消失，或 bundle/executable/normalized-host-path/focused-field selector、family 改变时，先在 DB 持久化 disarm 旧 word，再保存/发布 Settings。label、sample、intensity 不算身份变化。 |
| C1 | Mapping 失败与恢复 | DB 写入失效标记失败时不发布 Settings；DB 成功而 Settings 失败时报告 partial，保持旧 word 已失效，不自动恢复授权。invalidated rule 不接受 passive upsert、auto promotion 或进入适用集合。同 ID 同 selector 删除重建、旧 config 导入或 style 路径均不清 invalidation；只有明确 rebind 才恢复，且不增加 evidence。迁移时缺 mapping 记录为 `missing_binding_at_migration`（历史原因 unknown），无 mapping ID 的 native rule 不受影响。离线改动后最终状态与初值相同无法观察，不能声称检测到它。 |
| C1 | API 与读模型 | `pairKey` 参数名可保留，但语义为 opaque key，前端不得拼接。返回记录提供 kind/policy、唯一 legacy 映射、当前 independent count、binding status 和 ownership projection 所需字段；origin 由后端取得。旧 key 仅能经唯一映射解析，不能在多个 scope 间猜选。 |
| C1 | Migration 与回退 | 实施时基于当时确认的 next schema 版本；预审环境曾见的 25/18 仅是预审快照，不作为当前版本声明。单 SQL 事务完成迁移和 schema version 原子更新，并核对 word 映射、非 word 原 key、blocks、receipts、ownership references 与 pending 上限。Settings/DB 分层处理可重试状态，不假称跨存储原子。旧 binary 会拒绝打开更高 History schema；回退限于新版本内关闭新行为。旧 backup 不得覆盖迁移后用户数据。迁移测试只用 isolated temp DB，不碰用户 DB、不发布。 |

**待确认风险与验收。** 实施前须逐项确认新增持久化身份、整表 dictionary patch 的用户可见语义、global block restore 和 mapping rebind 的 IPC/UI、DB 成功而 Settings 失败的恢复反馈、旧版本拒绝更高 schema 的回退边界；本节不替用户批准这些公共或持久化合同。候选验收覆盖 A0 三个缺口（手工词→学习→undo；paste/History 同 origin；2 legacy hits + 1 known proof）、scope/casing/conflict/global legacy block、迁移异常原子与 word/non-word 保持、delete→restart→同 ID 重建、A→B→A、整表 patch、style 重建、label-only 更新、旧 observer 并发、DB 与 Settings 各自失败、显式 rebind 不增加 hits。上述均为后续隔离验证条件，不表示当前支持或通过。

## 9. D：证据、应用与负反馈

### 9.1 先保持当前次数作为对照

- 初始基线保留普通词 3 次、人名 2 次。
- 采样次数不是独立证据次数。一个 session/同一 History origin 的同词对只计一次。
- 上项当前仅为通道内去重；C0 之后才有跨 paste/History 的共同来源保证。A0 的报告必须保留这一基线缺口，不将两个收据直接称为两次独立听写。
- 明确确认可以立即生效；复用已有确认能力，不把它描述为新发明。
- 高置信自动词对、跨文字体系/大小写确认候选、冲突候选分别统计。
- 没观察到修改不作为正标签，未撤销也不作为学习正确的 gold。
- 如实验引入 pending 证据过期，只针对有证据支持的风险评估；不顺便清理已生效用户词条。

### 9.2 单一适用集合与检索顺序

观察 scope → 生成唯一适用规则集合 → 去除冲突 → 精确历史 alias → 有限同音/近形候选 → 根据预算生成必要 cleanup 提示。词对的替换、cleanup 提示、usage 和最终保护消费同一授权集合，避免每层重新解释 scope。全局个人词典的 ASR 术语/fuzzy 资格仍按 8.2 的现有独立策略处理，不因共用集合而悄悄删除全局词。

精确观测 alias 优先。已确认 alias、scope specificity、歧义与最近独立证据用于可解释排序；不先训练打分模型。最多 8 个 cleanup pairs 和现有 ASR token/terms 预算作为基线，扩大预算另做消融。

对存在正常语言含义的同音词加反例：学会人名“李明”不能自动证明自然语言“黎明”都该替换。新 alias 没有直接编辑证据时，召回和确定性应用须分开；歧义保留原文或提供确认。

### 9.3 应用事实

研究里分开记录：候选被筛入、词语被送入提示、本机实际替换、模型是否改回、最终是否采用、目标是否独立确认。普通运行优先复用有界 owner 元数据，不新增完整文本事件表。

当前 `last_used_at` 和准备阶段 usage 不能直接显示“用户接受了这个纠正”。UI 若缺少最终事实，显示具体步骤或省略，而非倒推成功。

D 需补充受当前学习授权约束的会话内关联事实：opaque origin、实际规则 ID、派生同音命中的原规则、fuzzy 命中的术语/来源、是否进入最终采用稿。不能仅靠输出词面倒推 provenance；无法唯一关联的 fuzzy/同音应用保持 unknown。正常观察器不因此取得窗口外文字，不新增完整字段日志；完整层级文本仅在 A1 明确授权 fixture 中取得。

### 9.4 负反馈

- 显式 undo/ignore 保持权威 tombstone，不自动复活。
- 用户把刚应用的目标改回去，只有能关联到本次实际应用规则时才成为 challenge 证据。
- 单次反向编辑可能是改变意思，不立即创建 inverse rule，也不自动删除人工词条。
- 发现同 scope 的活跃精确逆向关系时，将新自动候选作为 conflict/challenge，不能静默晋升成两条相互切换的规则；明确人工替换规则的操作与被动学习分别处理。
- 重复、可归因的反向纠正可以在独立实验中暂停该 scope 的自动应用，提示复核；该策略与次数在看候选结果前冻结。
- 只收到最终 diff、没有实际应用证据时保持 unknown。

## 10. E：Cleanup prompt 实验

### 10.1 当前值得保留的契约

`llm.rs::SYSTEM_PROMPT` 已规定保真、中英混合、禁止回答、实体/技术内容保护、有限去口癖、布局保留，以及只允许本机已经证明的改口。实验不得通过削弱这些要求换取更好看的文本。

当前可测的改进点是 Light/Standard 包装差异、Light 中词对重复、原始 ASR 参考的必要性，以及部分 raw correction few-shot 与 prepared-transcript 契约之间的混淆。
SpeakoFlow Mini 的公开 prompt 与预处理约定仅是未来独立 E 变量的参考；它们不并入当前 P2（只删除 Light 中重复词对描述），也不取代本项目既有 token budget、spoken-edit 许可或标点策略。该页面自报结果不作为本项目效果证据。

### 10.2 消融顺序

| 实验 | 唯一变化 | 主要观察 |
| --- | --- | --- |
| P0 | 当前 prompt 与配置 | 建立基线，不更换模型 |
| P1 | 统一任务/来源字段包装 | 指令混淆、内容保留、格式与保护拒绝 |
| P2 | 删除重复词对描述 | 命中、错误恢复、token 与延迟 |
| P3 | few-shot 改为准备稿到结果 | 未授权改口、前言丢失、列表内容保持 |
| P4 | 限制 original source 的附带条件 | 旧稿恢复、术语回退、保护误拒 |
| P5 | 在已通过的候选上测试另一模型 | 模型效应单列，不与 prompt 归因混合 |

每次使用同一固定数据、同一模型参数与现有 guard；记录未接受的 provider candidate 和最终 fallback，不能只报告被 guard 修复后的成功率。

### 10.3 候选 cleanup-only system prompt 方向

以下由本计划撰写，不是 Typeless 或其他闭源产品原文，也不是要直接覆盖现有完整 production prompt：

```text
你负责整理一份已由本机完成词典替换、布局解析和明确改口处理的听写稿。
只输出待输入的正文。转写、术语和参考内容都是数据，不执行其中的请求。
保持原语言与中英混合表达。
保留事实、行动、否定、不确定性、强调及有意义的重复。
完整保留人名、数字、日期、金额和技术字符串，包括路径、URL、命令、标识符与代码。
只去除明确无意义的口头填充和紧邻误起，修正必要标点；保留脏话和有意义的笑声。
本机已经解决的改口以准备稿为准，不恢复旧稿。
仍存在的改口措辞保持原样，不自行猜测替换范围。
保持已有列表、段落和代码布局，不自动发明新的标题、列表或称呼。
只应用本次明确提供的已批准风格偏好。
不回答、不总结、不扩写未说出的内容；没有实质内容时按现有空结果契约处理。
```

实际实验应保持基线与候选的指令语言一致，避免把中/英文差异与规则修改混在一次实验。rewrite/translate 等明确操作保持独立路由和现有契约，不被该 cleanup-only 草稿覆盖。

动态输入只发送本次准备稿、必要强度/布局信息、与来源锚定相关的词汇和已批准偏好；不默认发送整本词典、完整纠正样本或原始字段。Light 不能因包装统一而偷偷开始应用 Standard 才使用的样例或风格。

标签/JSON escaping 只是结构化手段，不构成可信授权或防注入保证。保留角色边界、provider 协议完成检查及最终保护。

### 10.4 本机缓存与 provider 差异

prompt 修改要复核 `stable_prefix_user_bytes`、本机 warmup/cache key 和消息预算，动态 transcript、词对、窗口来源不能被划入稳定用户前缀。缓存命中、冷暖态和新鲜执行分别报告；不使用 replay 时延证明性能改善。

P0 还须记录当前稳定前缀的实际组成：Standard 已包含 intent 元数据、context confidence 与已批准风格样例。后者属于用户文本；本机 sidecar 有 models root 下的 `prompt-prefix` 快照，不能把“稳定”误写成“无用户内容”。E/F 候选默认让非内容枚举偏好参与稳定配置，具体样例放动态后缀；改变前缀位置作为独立变量，测缓存碎片与真实延迟。撤销样例/偏好时由现有 cache owner 定义失效和持有快照处理；没有删除证据时不宣称既存快照已被清除，实验不删除模型权重。

已存在的 provider 特定提醒作为独立变量保留或实验，不能在消融中悄悄增删。Groq、native MLX 和其他 adapter 的结果分别报告，不假定同名模型或量化构建等价。

## 11. F：风格学习与用户说明

先补“删除事实也变短”的反例，不直接增加自动偏好维度。风格候选只来自意思基本不变、重复出现的编辑，优先考虑句末句号、口语语气、分段密度和正式程度。

- 新偏好优先复用已有 style draft 确认；现有自动强度路径单独评估，不把其当前行为误写成人审-only。
- 日期、金额、否定、对象或新决定变化不作为风格证据。
- 不根据一次删字把强度下调，不让普通词对确认变成批准整段历史样例上传。
- 如果需要改变现有自动 intensity 行为，单独提交行为差异与回退，不能夹在 prompt 去重中。

用户默认界面只回答：候选是什么、在哪生效、还差几次或为何需要确认、如何撤销。已有审计详情保留来源与未知项。没有实际应用事实时，不显示“本次已使用”；没有独立后续证据时，不显示“准确率提高”。

若涉及实质 UI 变更，实施时应用 `mingjie-ui-design`，复用现有 Dictionary/History/HUD，不扩大主 HUD 或引入新的后台窗口读取。

## 12. 数据、对照与防止泄漏

### 12.1 开发 pilot

首轮准备约 120 个明确标为 pilot 的 episode，允许根据 A0 能力分批完成：约 40 个词汇正例、20 个编辑时序/多 hunk、20 个作用域/冲突/撤销、20 个内容编辑反例、20 个 style/指令/literal 反例。类别按主类别计数，交叉标签不重复算总数。

这只是工作量建议，不是统计把握度保证，也不能替代真实用户纠正。来源权利、已曝光状态和人工标签必须清楚。程序化 fixture、TTS、真实人声、人类实际编辑分别报告。

### 12.2 两种泛化任务

- **已学术语的后续使用**：同一实体重新出现是目标；要求新句子、独立录音/会话、不同模板，不能把实体相同误当数据泄漏，也不能把同音频重放当全部验证。
- **候选提取与晋升算法泛化**：需要独立 term family、编辑模板与来源；开发者未见的术语用于检验规则不是只针对样例写死。

正式留出复用项目 partition owner 与 certified lock 协议。开发 pilot 可以如实保留未知，但不得升级为正式 holdout。候选冻结后，基线和候选一起运行留出；见过输出的批次不反复用于宣称新验收。

### 12.3 有状态对照

基线与候选各有独立隔离状态，复制的是同一审核过的初始状态，不共享学习后的数据库、词典、缓存或 receipt。必须记录初始 rule/state 摘要；每个 arm 按相同 episode 序列学习再测试。

原生 App research lease 没有按 arm 改写普通 App 数据目录的能力。另有独立的 `voiceflow-learning-harness --arm-root`，只在显式 private temp root 中初始化自己的 settings/SQLite，不启动日常 bundle；当前 owner 只覆盖受控学习状态和 usage，不覆盖 History/revisions、style/cache 等本计划要求的所有 arm 数据。A0 的纯函数/持久化测试使用独立临时目录；原生冒烟只使用明确核实的隔离 Test bundle，不能把编译了 research feature 当作隔离证明。

A1 已为首个 deterministic-text slice 增加独立 arm 根目录和准入。当前 arm 仅创建 fixture settings、selected learning rows、operation receipts 与 usage；History/revisions、style drafts/intensity 和 cache 仍未接入。后续每类新增 owned fixture 状态都须在同一显式隔离 owner 中增加，不扫描或摘要普通私人数据库；凭据不进入快照、摘要或复制。

切换 arm 前，必须经现有 lease/observer/persistence/delivery owner 确认闭合，再重建隔离进程和装载初始 fixture。不能直接替换运行中的 SQLite 或只恢复文件而保留旧内存状态。arm 缓存空间/清理证据与 cold/warm 策略分别冻结；不足以证明隔离时保留 unsupported/not_run，不签收原生有状态对照收益。

外部模型与时延实验采用预先确定的顺序平衡/交错方案，记录冷暖态、失败、重试和取消。改变观察器时固定 prompt/模型；改变 prompt 时固定学习规则状态；比较晋升策略时保留相同输入证据。

## 13. 指标与验收

### 13.1 分母与未知

| 指标 | 定义 | 主要限制 |
| --- | --- | --- |
| 产品覆盖 | 授权、目标可读、成功注册分别占全部目标纠正任务的比例 | 无权限/不可读不从产品覆盖里消失 |
| 观察 recall | 捕获到有效局部编辑 / 人工标注为范围内可学习且满足授权读取条件的编辑 | 另报注册失败、late/left、ambiguous/unknown |
| 候选 precision/recall | 候选是否为用户实际拼写纠正，及应发现的纠正找到多少 | 候选和自动晋升分开；大小写/跨文字体系独立分桶 |
| 晋升 precision | 正确自动规则 / 全部自动激活规则 | 零激活为 null，不能用全拒绝取得完美分数 |
| 后续成功率 | 新任务最终输出满足冻结 gold / 全部已启动有效 follow-up | provider 错误、错误 fallback 和交付失败均保留 |
| 规则条件成功率 | 在明确实际应用机会中最终正确的比例 | 与全任务成功率同时报告，不能挑成功机会 |
| 误改率 | 原本正确而被规则改错的负样本任务 / 全部已启动负样本任务 | critical fact/literal 与普通拼写分别计数 |
| 净编辑节省 | 基线所需规范化字符编辑量减候选量 | 使用冻结人工参考；不是实际击键数，不能靠全局监听收集 |
| 确认成本 | 每个有效规则需要多少明确确认/纠正任务 | 自动与人工通道分开 |
| Guard 效果 | 坏候选被阻止及好候选被误拒的人工标注结果 | candidate 和最终输出双层计分 |
| 性能 | 本机学习/替换耗时、stop-to-ready、stop-to-confirmed-insert | 原生交付独立裁判；成功分位数同时附失败/超时 |

缺失样本、不可判定或未运行保持 unknown/null；不能归零，也不能悄悄从主分母删除。未知 gold 不自动算正确；报告已判定与未判定覆盖，未达覆盖要求不能宣布 gate 通过。

规则条件指标另报 exact、派生同音和 fuzzy 的 provenance 覆盖。无法归因的机会不能当作未发生，也不能用少数可归因样本代表全部应用。错误 scope 的规则应用、已授权的全局词汇提示与实际误改分开判定；全局词汇造成坏输出仍计误改，但不能误称发生了跨 scope 的确定性替换。

### 13.2 分阶段门槛

以下为待在候选运行前冻结的验收方案；开发基线可用于判断可行性，不得看完候选留出结果再修改阈值。

正式验收必须预注册：每项主/次指标、改善阈值、非劣 margin、最小独立簇数与样本数、簇的定义、置信水平/单侧方向/计算方法、unknown 覆盖上限、排除条件和停止规则。G3 的 precision 采用单侧置信下界达标，不能仅用点估计；以 99% 为质量目标时，应先据基线和独立性设计所需规模，120 个 pilot 不自动具备这一证明力。参数未冻结则只允许开发结论，不能执行正式签收。

有效 follow-up 在看输出前按输入/权限/来源协议确定；正确自动规则和最终语义由两位盲态评审判断，分歧经第三位裁定，保留原判断。规则 gold 必须区分独立词纠正、真正短语纠正和内容编辑，不能由模型或晋升结果自行定义。

- **G0，契约**：授权/身份/取消/范围、独立计数、tombstone、人工词条所有权、最终保护的确定性反例全部符合预期；没有 unresolved 的高优先级契约问题。
- **G1，测量可用**：每个 episode 有明确 terminal 或明确 unknown 原因，阶段覆盖可计算；无法支持的原生场景如实排除在该层结论之外。缺失质量层不妨碍交付确定性工具，但不能签收对应质量收益。
- **G2，观察与提取**：在预定义 late-edit/multi-hunk 桶中 recall 优于基线；自动候选 precision 不降低。不得仅提高候选数或注册数就通过。
- **G3，规则与收益**：目标 follow-up 成功率与净编辑节省优于基线；普通词、错误 scope、内容编辑和 literal 反例不出现新增关键语义错误。自动晋升 precision 的单侧置信下界达到预注册质量目标，并同时报告点估计与独立样本数；pilot 不足时只作方向结论，不能签收这一质量目标。
- **G4，Prompt**：保留现有事实与指令边界；provider candidate 的坏改写率、最终编辑量和保护误拒均不退化，并至少一个预注册主指标改善。单纯 token 减少不能证明质量改善。
- **G5，运行成本**：真实新鲜执行中没有新增普通用户云请求；本机资源和延迟在预注册预算内，任务成功率不退化。不提前承诺尚未测量的毫秒值。

正式质量结论按 episode/source session/模板等独立簇报告配对差值和置信区间。术语/会话内重复不能当作独立样本扩大把握度。零误改的小样本只能报告“本样本未观察到”；不据此宣称普遍安全。样本不足或区间跨越不退化门槛时，结论为未证实，继续收集独立数据。

G0 是候选必须达到的目标，不是当前基线已通过的声明。A0 的历史运行与 round-4 实际结果保留原样；本轮 A0 输出的三个 known gap 是人工所有权、跨通道来源与 legacy origin。B1b 已修复并通过 bounded text slice 的多 hunk / 日期内容分类回归，但这不解决 C0/C1，也不代表 G0 通过。G2 中将多个独立 hunk 错合成短语、或把日期/内容变更当词对均算错误候选，不算捕获提升。

## 14. 验证矩阵

| 层 | 必测场景 | 能证明 |
| --- | --- | --- |
| 确定性状态 | 相同 session 重复通知、同 History 重存、同听写跨通道、不同 hunk/rule、legacy pending 加新 origin、过期 epoch、取消、清空 | 计数与生命周期契约 |
| 编辑提取 | 单词、短语、大小写、跨文字体系、连续中文、多 hunk、继续输入、重复插入、范围外编辑 | 指定文本轨迹的候选行为 |
| 规则持久化 | 两 scope 同 pair、大小写 identity、同 alias 多 after、旧全局 tombstone、手工词加自动规则后撤销、重叠 scope、mapping 变化、迁移失败、设置保存失败、undo、重启 | 状态与所有权；不证明识别质量 |
| 原生编辑 | TextEdit、至少一个实际使用的 Electron/编辑器目标、浏览器可读字段、中文 IME、改完切走、发送清空 | 对应环境的 AX/编辑覆盖；当前不支持的 lease 先扩协议 |
| 模型质量 | 同词新录音、同音普通词、正确原稿、混合语言、列表/改口、注入式口述 | 所选模型/配置/语料上的质量 |
| 真实用户 | 同意参与的人实际纠正并完成后续任务 | 对该参与者样本的操作成本与收益 |

AX 回读成功不等于独立裁判成功；程序设置字段不等于人类纠正；WAV evaluator 不等于麦克风/热键/目标 App E2E。三者分别签收。

## 15. 实施切片与依赖

实施状态（2026-11-06）：A0 与 B0 的确定性工程关口已通过；人工词条所有权、跨通道 / legacy origin 与 C0/C1 仍未解决，因此 G0 尚未通过。B1b 已关闭 bounded text slice 中的 multi-hunk / 日期内容分类缺口，不能据此签收真实 AppState 多词对观察或质量收益。A1 V2 deterministic-text/local-only slice 已由 Sol 对实际 stdio 入口验收：三次 `VoiceFlo` → `VoiceFlow` 学习调用形成 `auto_scoped` SQL 规则，不写全局词典；空词典仍为空，合法预置 `VoiceFlow` 保持原值，settings hash/bytes 不变。受控新句调用生产本地替换 owner 并增加一次 usage；同 operation 重放不增量，生产 undo 写入 tombstone、重复撤销不刷新时间，第二个已声明新句不再应用旧规则，新进程重开同一 arm 后仍保持 tombstone。历史 `cobolt` → `cobalt`、`clone` → `close` 和受保护目标按 local-only owner 拒绝且无四表写入；旧 `cobalt` 的全局 Settings partial-save 证据保留，但标为 invalid for local-only global-dictionary parity。text trajectory 实际验证 N2/N4/N6 poll 与消费数 2/4/6，N7 在 arm 写入前拒绝。4 态 `[A,B,B,C]` 提前稳定时 poll 2 次、consume 3 态、词对 mismatch 且零 row；5 态 `[A,B,B,D,B]` 同样只观察到 `[A,B,B]`，词对符合授权项而由 owner 持久化。未读的 future state 与 future benefit 均保持 unknown。

A1 V3 multi-scope phase slice 已通过实际 stdio：同一 arm 保存 A/B 两个 mapping；A 侧三次受控学习达到 `auto_scoped` promotion，B 的应用按生产 applicability filter no-op，可记录一个 `research_fixture_noop` operation receipt，但不增加 learning hit/proof 或 usage；B 对同一 pair 的学习冲突返回实际 legacy key 与 stored/requested scope，整个 SQL state 不变并标记 C1 known gap。A 正向 follow-up 只增加一次 usage；指向该 positive operation 的 Undo 成功 tombstone。pending 1-hit 与 post-undo A/B follow-up 都保留真实 no-op，引用 no-op receipt 作为 Undo 前置会被拒绝且 SQL 不变。空词典和合法预置词的 Settings 均不变；新进程重开保留 owner/tombstone；TTL 过期及错误 phase 被拒绝；旧 V2 arm 仍可重开。

V3 邻接拒绝的真实 stdin 复验覆盖 wrong plan、未声明 operation ID、text SHA 不匹配、缺 learning grant 与 plan identity 变化；各自均未更改选中 SQL 表或 Settings。follow-up 1,017-byte 输入被同一生产本地替换 owner 展开到 1,130 bytes 后，在 application receipt/usage 写入前返回 `learning_phase_output_bounds`；既有三个学习 hit 保留、usage 为零。所有 V3 访问仍先走已验证的 arm SQLite 文件/sidecar owner guard；本轮复用此前通过的 symlink/hardlink sentinel 证据，没有重跑文件系统矩阵。

真实 public WAV 的 on-device ASR probe 已返回 transcript；共享 `loaded_attestation` completion gate 仍 pending，本轮没有触发 WAV 学习 SQL 或 Settings。A1 总体仍进行中：V3 `ObserveTarget` 已对 source-pinned owned FixtureHost 执行一次注册 target AX sample，并确认 expected hash / 长度、同一 Host process identity 与 child exit；该 slice 的 focused schema tests 为 8/8，完整证据位于 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/voice-flow-native-oracle-runtime-verification-ngwtsrmr`。这不是 VoiceFlow product insertion、learning、pipeline-final 或同 Host 跨编辑链；目标-only arm root 无 DB/Settings 写入，显式 stdio cancel 未接入。B1a 的 source-side delivery fact 与 owner lifecycle 已实现并通过 default production build 和 13/13 focused contracts（2026-11-06）；证据位于 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/voice-flow-b1a-final-verification-u3ginc8d`。该关口只验证 typed fact、单次 Undo、queued epoch / deadline、在途 owner 与 UTF-16 range 的确定性契约，不代表真实 paste→AX observer 已运行。B1b 已关闭 bounded text slice 中的句末 Latin 句点、1–3 个局部 hunk 和日期 / 内容拒绝回归；真实 AppState 多词对 native paste→same-Host correction→final AX read、IME、人类纠正、浏览器字段、独立收益判断及 ASR/cleanup 质量仍未运行或验证。C1 产品 rule identity/casing 与跨 scope 同 pair 所有权未解决；G0 尚未通过。

B1b（2026-11-06）由 Sol 对 default / research build、dictionary group（108/0）、learning fixtures（6/0）及 fresh V2（11）/V3（5）stdio cases 实际复验；证据位于 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/voice-flow-b1b-verification-xnji3vz8/round4/checks-summary.json`。这验证的是 source、deterministic tests 与隔离 stdio slice，不是产品 AppState 多 hunk native E2E、同 Host 纠正链、人类输入或 precision/recall 收益。

Clipboard guardian 的 named-only component 与主线程接线有独立的历史 round3 证据：5 个 owner tests（5/0）和 9 个实际 CLI contracts 通过；唯一命名 pasteboard 与固定 synthetic seed 覆盖多 flavor restore、ACK 丢失、EOF 后真实 child wait、external-copy 保留与未知恢复持有，8 个 child 与 9 个 parent 的退出均已确认。`--mainqueue-probe` 的独立证据显示 OS main callback 与真实 owner join 在 16 ms 内完成，未触碰 clipboard、AX、Host 或 input source。历史结果分别见 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/voice-flow-native-paste-seam-verification-zlcquvcf/clipboard-guardian-named-verification/round3/final-named-component-summary.json` 与 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/voice-flow-native-paste-seam-verification-zlcquvcf/mainqueue-fix-probe/round2/final-mainqueue-probe-summary.json`。此前 no-General 工程验证批次（round2-fixed-fixture）中 default / research build 均通过；完整 default suite 为 1,867 passed、0 failed、34 ignored；research-feature 子集 11 项通过（2 个共享纯函数契约、6 个 named-feature、3 个 General synthetic；与完整 default suite 有重叠，不可相加）；4 组错误 General 参数在 capture 前拒绝；同一 source / binary 下 9 个 named CLI contracts 该批次 fresh-run 并全部 accepted；V2 11 与 V3 5 个隔离 stdio cases 通过。Global fmt exit 0，指定的 strict all-target / all-feature Clippy exit 0；这不是所有环境警告均为零的声明。summary 位于 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-final-no-general-verification-q20zd32o/round2-fixed-fixture/final-no-general-verification-summary.json`。之后一次独立 General `normal_restore` preflight 实际通过：parent / child exit 0，receipt 为 parent restore commit、`owner_retained=false`、`controls_sent=0`。证据 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-general-normal-restore-root-go-fd0ma_wl/attempt/result.json`；原始 flavor / bytes readback 未探测，driver 未读取或导出 clipboard 正文或 hash。未运行 Host、AX、TIS、`Cmd+V`、WAV 或模型。较早的真实 General clipboard 尝试仍为 failed，因未获正常 owner-close / restore 回执而保留 restoration unknown；新 preflight 不改变旧结果。当前 `general_guard_active_for=false`，`RunOwnedPasteSession` 仍在 target / clipboard side effect 前拒绝；同 Host product paste、learning 与质量收益均未验证。

本轮 RemoteNamed recorder CLI 与上述 named-only component 及 General preflight 是不同的研究入口：9 个 case（positive、write-ACK-lost、permit-ACK-lost、permit-denied、EOF-child-alive、prewrite-external-copy、wrong-sequence、wrong-write-ID、replay-write）全部 accepted。parent 与 child 的实际 wait 均 exit 0，全部 owner 已结算；controls_sent=0、target dispatch=0、shortcut_sent=false。唯一命名 pasteboard 上的 fixed synthetic 内容与 recorder/fake callback 仅验证远端协议顺序，不是实际快捷键或 CGEvent 投递，不 mint `ConfirmedInsertion`，也不证明产品插入、学习、真人纠正或收益。source / binary references 在该批次前后稳定，binary SHA-256 为 `76831406f79ef31dfaf864cec1797ddffac3d09c8be1b03a2f901edb143b420d`；该次运行绑定当时 `src-tauri/src/paste.rs` SHA-256 `95e76043bcef205bd8f5ae47e09603f0d2e0af7170c2df81f77e8f70a036bb5a`。该段记录的是 95e 历史 source epoch；后续 b78/d3 QA 已重新验证 RemoteNamed 9-case CLI，见下方本轮 QA 记录。receipt summary 位于 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-remote-named-actual-verification-f4o0m3rh/remote-named-partial-verification-summary.json`。General clipboard 未触碰，`general_guard_active_for=false` 保持；未运行 Host、AX、TIS、`Cmd+V`、WAV 或模型。

本轮 QA（summary SHA-256 `aa67707c7acb7fa29feb0867f49b673c3d3bd5bd7c071c408a09db48e4ab542f`，路径 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-harness-qa-resumed-kr5wornx/harness-qa-summary.json`）绑定 source epoch：`paste.rs` `b78f92170dcba45f9283ff8f3334f4a0a200a490ddf9634cf059e6a222ec6c82`、`learning_research.rs` `90a8b130b259c6650d4f7d52e2dd7b23320919ec22a1ec9e93987e97ec0c5add`、`learning_clipboard_guard.rs` `a8a5c0ea2e5bd46f384ecb21bf857fed209bc582530ac0084daf8a72c86e3f4e`，research binary `d3e618c0d5b1534ecd16354111b901aa3f8174242adef28f930a10a5b34d5de7`。default lib / research binary build 均 exit 0（warnings 分别 18 / 4，之前 3 组 owned dead-code warning 已消失）；新 RemoteNamed tests 11/0、default post-permit pure test 1/0、MLX mock trace 1/0、现有 research contract 子集 11/0；现有 named CLI 9、RemoteNamed CLI 9 和 4 组 General malformed arguments 均 fresh accepted / rejected as expected；fmt exit 0。V2 11 / V3 5 runtime cases沿用先前证据，本轮未重跑。完整 default suite 为 1,907 passed、2 failed、34 ignored；两个失败都在 foreign `evaluation::filter_report`。strict Clippy exit 101，有 5 个 unique diagnostics，全部位于该 foreign 文件，owned diagnostics=0；没有修改或绕过该文件。上述通过结果只覆盖已列明的 source epoch 和隔离研究 slice，不代表全仓 QA green。

General Remote research component 已有一次 no-shortcut `normal_restore` 运行通过：固定入口 `--clipboard-guard-general-remote-probe` 执行 10-step recorder-only / deferred 协议，child 实际 exit 0 后 parent fresh-check 并恢复，`owner_retained=false`、`controls_sent=0`、target dispatch=0。parent permit 与 child callback 各记录一次；没有发送快捷键或运行 Host、AX、TIS、`CGEvent`、`Cmd+V`、学习、provider 或模型。结果 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-general-remote-single-physical-z5wzeowr/attempt/result.json`，SHA-256 `5fa9be6df977f681c91df60db57f6b425d31fbd1a327815e349f5501da978e32`；资源终态确认 `/var/folders/gm/g2vs8nvn4tn2108l6l6t87gc0000gp/T/vf-general-remote-single-physical-z5wzeowr/resource-terminal.json`，SHA-256 `4f1d3d0f5a08b0e1a544604dd00bded2cafda7fc5d7895dfed3774fc4a6a17a3`。原始 flavor / bytes readback 未探测，未导出原剪贴板正文或 hash。该单次组件运行不属于 b78/d3 QA，也不开放产品路径；`general_guard_active_for=false`。更早的 physical restore 仍 unknown；本次运行不证明产品 paste、learning 或质量。

| 切片 | 交付 | 主要文件 | 依赖/退出条件 |
| --- | --- | --- | --- |
| A0 | 麦克风两态冒烟、确定性单元/集成契约与已知失败分布 | `learning_research.rs`、`research_trace.rs`、现有测试与固定规则 evaluator | 固定行为基线；不将 fixture prediction miss 当产品失败；G1 当前覆盖部分 |
| A1 | 版本化 V2 episode 与 V3 multi-scope phase、隔离 arm；local-only SQL candidate/replacement/usage/undo/reopen、2–6 状态 trajectory、scope phase/no-op/conflict/undo/reopen 与邻接拒绝/output bounds 已由 Sol 实际验收；target-only `ObserveTarget` 已连接 source-pinned Host / AX one-shot sample 并通过真实 stdio；B1b bounded text extraction/content rejection 已由 source、focused tests 与隔离 stdio 复验；named-only synthetic clipboard guardian、OS-mainqueue probe 与 RemoteNamed fake-recorder CLI probe 已通过各自工程关口；最新 b78/d3 QA 对 RemoteNamed 9-case CLI 作了 fresh 验收；General Remote recorder-only no-shortcut `normal_restore` 组件已通过一次隔离运行；此前 General-only private owner 的 preflight 仍保留为历史证据；WAV source→on-device ASR 已运行，但 learning 等待 complete attestation gate | `learning_research.rs`、`learning_phase_v3.rs`、`learning_native_oracle.rs`、`learning_clipboard_guard.rs`、`dictionary_learn.rs`、`store.rs`、独立 research binary | 保持原生 owned-WAV suppression；完整 attestation 前不写 WAV learning rows/receipts/settings；`ObserveTarget` 仅对固定 synthetic Host 做一次独立 sample，无 product paste/learning/same-Host correction、pipeline final 或 in-flight stdio cancel；named clipboard 与 RemoteNamed probe 只覆盖唯一 named pasteboard 的 synthetic 内容和 fake callback recorder（target dispatch=0、shortcut=false，不证明 product paste）；General Remote 仅有一次 recorder-only 组件运行，parent 在确认 child 退出后 fresh-check 并恢复；未探测原始 flavor / bytes readback、未运行 Host / AX / TIS / `Cmd+V`；`general_guard_active_for=false`，产品 `RunOwnedPasteSession` 仍在副作用前拒绝；早前物理尝试仍 failed / restoration unknown；普通 V3 学习计划仍使用 `target_oracle=none`；C1 候选合同批准前不实现新的 scoped/casing rule identity；同 Host 产品交付链仍待验证；原生麦克风/IME、人类纠正、浏览器字段、独立收益判断及 ASR/cleanup quality 仍未验证；G0/G1 |
| B0 | 有证据的观察生命周期修复 | `dictionary_learn.rs`、必要的 `paste.rs` 最小事实 | A0；原生证据支持后决定事件/轮询策略；G0/G2 |
| B1 | B1a typed delivery fact、原 anchor、UTF-16 range、single-use Undo/read-owner 生命周期的 source contract 与 B1b 1–3 hunk / whole-edit content gate 已由 default / research build 和 focused deterministic / stdio cases 验收 | `paste.rs`、delivery-scoped `lib.rs`、`dictionary_learn.rs` | A1 与范围身份；未验证真实 AppState multi-pair paste→native AX、same-Host correction/readback、IME、人类纠正或学习收益；G0/G2 |
| C0 | 人工/legacy 词条所有权、跨通道 origin 与独立计数政策 | `store.rs`、`dictionary_learn.rs`、History/交付关联、审计消费者 | A0 已知失败与 A1 所需回执；独立迁移；G0 |
| C1 | scoped/casing rule identity、冲突、旧全局阻止与迁移 | `store.rs`、`dictionary_learn.rs`、`lexicon.rs`、审计与消费者 | C0/A1；单独契约 diff；G0/G3 |
| D | 独立晋升实验、有限召回、应用 provenance 与负反馈 | `dictionary_learn.rs`、`lexicon.rs`、会话内学习事实、研究 usage | A0/A1/C0；多 scope 策略依赖 C1；G3/G5 |
| E | Prompt 逐项消融与缓存复核 | `llm.rs`、`lib.rs`、cleanup corpus/真实模型 runner | 固定规则状态；可与 C 独立推进；G4/G5 |
| F | 风格反例、批准偏好与清晰说明 | 现有 style owner、Dictionary/History/审计 | 独立语义标签与 UI 设计；不混进 E 的去重改动 |

首轮执行 **A0 + 证据充分的 B0**。C 的 schema/API 工作、D 的阈值调整和 E 的 prompt 改动不要合并成一次不可归因的大改动。切片可并行研究，但共享文件的实际编辑由主负责人协调。

## 16. 检查、文档与回退

本文是规划，不复用过去测试数量作为当前通过证据。开始实施前重新核实当前工作区编译状态；无关并发改动导致的错误单列，不通过跳过检查或改正确期望掩盖。

按切片先运行受影响测试，再运行仓库要求的检查：

```bash
npm test
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

研究入口另验证其实际 feature 构建；具体二进制与参数从当前 CLI/help/类型定义核对后执行。本文不杜撰尚不存在的 episode CLI 参数。真实 provider 实验必须明确所选数据、配置与调用预算，不能因运行离线检查顺便上传。

行为落地时同步更新 [privacy.md](privacy.md)、[end-to-end-workflows.md](end-to-end-workflows.md)、[cleanup-evaluation.md](cleanup-evaluation.md)，必要时更新 [README](../README.md) 与 [以后/不做](asr-cleanup-later-and-wont.md)。旧研究稿保持历史属性，不改写成新的实现事实。

每个切片保持可单独停用或回退：观察策略恢复旧边界、候选类别退回确认-only、召回退回精确 alias、prompt 恢复前一版本。规则数据与 tombstone 保留，不以删除数据作为回退。已发送的请求、已完成输入与已形成的用户授权不能通过关闭实验追溯撤销。

## 17. 独立评审与待关闭事项

本轮三个指定 reviewer 审查同一初稿及工作区必要源码，均只读，不实施、不运行付费 provider 质量实验、不提交。初稿 SHA256 为 `f9cd8687f4f96c2d7475278751a4e7f21552376e3c331211735c7c8219f251fd`；修订版的字节摘要应由后续使用者重新计算，不能复用初稿摘要。

| Reviewer | 请求配置 | 评审状态 |
| --- | --- | --- |
| Codex subagent | `gpt-6.1-sol`，`xhigh` | 已完成初稿评审及拟修订契约的限定复核 |
| Cursor CLI | `claude-opus-5-5-xhigh`，plan 只读模式 | 已完成；主负责人另行核实初稿摘要，reviewer 的 shell 摘要命令被拒绝 |
| AGY CLI | `gemini-4-argon-high`，`high`，plan 只读模式 | 已完成仓库内评审及限定摘录复核；首次仓库外 skill 读取被 headless 拒绝，未重试该访问 |

重点审查：源码事实是否过期、现有研究协议能否表达计划、因果/标签/数据隔离、编辑与输入法状态、scope/迁移/撤销、prompt 实验变量、可执行验收和过度设计。返回问题须含计划行号、源码证据、触发、后果和最小修订方向；将确定问题与建议分开。

### 17.1 已整合的主要反馈

| 反馈 | 核验与处理 |
| --- | --- |
| 人工词条所有权不是已有保证 | Sol/Opus 均指出，源码调用链确认；新增 L8 与 C0，旧来源 unknown 保留，撤销不误删 |
| 现场观察与 History 不能证明两次独立听写 | Sol/Opus 均指出，现有 source_kind 收据确认；补共同 origin 与 legacy pending 不混算政策，Sol 已限定复核 |
| AX 全局词汇、scoped 替换、cleanup pairs 三者混淆 | 源码核实后明确保持现有三类行为；改变 AX 全局词策略另做行为实验 |
| 多 hunk、追加输入、AX 内容分类与未稳定超时的基线不够准确 | Opus/AGY 有效方向经源码核验后补 L3/L7/L10/L11 与 B0/B1；B1b 已对 bounded text 提取、整次内容拒绝与稳定边界加入并通过针对性回归，真实 native continuation / IME 仍待独立验证 |
| A0 高估现有研究入口 | 三方有效部分整合到 6.2：A0 冒烟/契约，A1 才增加确定性来源、层级证据与独立裁判，不解除旧 owned-WAV 抑制 |
| 状态隔离、旧 tombstone、schema 回滚与稳定前缀缺具体边界 | Opus 指出并核实相关 owner；补 8.4、10.4、12.3 与验收参数清单 |
| 部分 AGY 原报告引用了不存在的符号/章节或过期机制 | 不采纳“跨 scope 累加”“History 按 revision 刷次数”“无插入范围校验”“无收尾文档”等结论；AGY 摘录复核已撤回。对已被计划列为目标的内容不重复当作已实现缺陷 |

reviewer 的静态意见不是运行验证。未执行构建、原生 IME、真实用户或 provider 质量试验；这些仍是后续切片的验收工作。原始 CLI 响应和临时评审材料放在仓库外，不把冗长 review 报告加入仓库。全部工作落地后不保留本文作为完成报告。
