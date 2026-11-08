# Cleanup 编辑归因 Harness 实施计划

本计划是 cleanup 与纠错学习总计划中的一个实施切片。目标是在合成文本回放中区分用户补充、ASR 漏听、本机规则误改和 cleanup 误改，并验证只有合格的 ASR 词汇纠正能给隔离学习组增加候选证据。归因裁判是确定性的 Rust 函数。它消费输入证据，输出归因、原因和学习准入结果。真实用例仍暂停，本轮验收范围是文本 fixture、模拟字段回读和临时 SQLite。

## 1. 入口与数据流

```text
 reviewed speech + edit intent facts
                |
                v
 ASR -> prepare -> cleanup candidate -> guarded output
  |        |              |                   |
  +--------+--------------+-------------------+
                         |
                         v
 fixture snapshots -> existing observer -> stable edit
                         |                    |
                         +--------+-----------+
                                  v
                       deterministic attribution
                                  |
                   +--------------+---------------+
                   v                              v
             verdict + reason              eligible ASR term pair
             no evidence write              existing learning gate
                                                   |
                                                   v
                                             isolated SQLite
```

现有入口 `record_controlled_fixture_edit_trajectory` 在 `src-tauri/src/dictionary_learn.rs` 中负责研究授权、轨迹回放、候选校验和学习写入。它目前要求先给出获准的预期词对。补写、内容编辑和 `UNKNOWN` 用例即使没有词对，也必须能走到观察和归因并产生回执。新增一个内部研究入口，复用观察函数和已存在的词对准入及写入逻辑。归因发生在稳定编辑之后、写入之前。既有 V2/V3 入口及其持久化契约保持原有语义。归因输入采用内部测试类型，第一阶段通过 Rust 测试直接驱动。

## 2. 同一次听写的证据

所有文本与编辑证据绑定同一 `origin_id`，学习作用域另行明确。阶段文本区分 `Present`、`Skipped`、`Missing`。同时记录实际交付路径 `selected_path`，标明采用 cleanup 候选、回退到哪份已记录文本，或从哪一层直通。校验本次插入片段与所选路径的文本一致，origin 不一致或交付不匹配分别输出 `UNKNOWN(origin_mismatch)`、`UNKNOWN(delivery_stage_mismatch)`。有独立证据证明 guard 生成另一份文本时，需要记录该输出和来源才能使用该路径。缺失阶段无法用空文本或上一层补齐。

| 层 | 记录内容 | 用途 |
| --- | --- | --- |
| 口述参考 | 预先审核的逐字文本，来源标为合成或人工审核。后补标签来自独立的编辑意图标注 | 证明目标片段是否在原话中，或确认为后来补写 |
| ASR 原稿 | 本机规则和 cleanup 之前的实际原稿 | 区分 ASR 漏听和错词 |
| 本机准备稿 | cleanup 实际输入、可用的规则 ID 与该规则实际改动的局部范围 | 定位准备层差异。只有能唯一绑定到该片段时才归给具体规则 |
| Cleanup 候选 | 模型候选、guard 结论及最终采用的路径。拒绝或回退也是一条明确记录 | 区分坏候选和到达用户的错误 |
| 交付 | 最终文本、经验证的交付基线、原字段标识和插入范围。隔离测试用 fixture 明确模拟这些事实 | 建立可比较的编辑起点 |
| 编辑 | 同字段轨迹、观察终态、实际读到的稳定文本、可证明的局部编辑范围及来源 | 区分稳定补写、恢复、替换和无法判定 |

每个 fixture 有一份独立的预期结果，只供断言。归因裁判不读取预期归因、预期词对或写入许可。独立的编辑标注可以证明“后来补充”或“主动改写”，也要接受原话和阶段证据的交叉检查。有冲突时输出 `UNKNOWN(intent_conflict)`。缺少这种标注时，原话正确交付后发生的变动仍可报告为内容变化，具体意图记为未知。诊断回执记录 origin、归因、首次差异层、是否交付、原因、学习动作与隔离状态读回。原文片段只保留在授权的测试数据和必要的运行内存中。

## 3. 归因规则

1. 第一版集成 fixture 约束为本次插入占据整个字段，核对第一份快照和实际交付文本完全一致。在这个明确范围内，字段末尾新增可以归入同一次编辑。带既有前缀或后缀的部分字段用例暂报 `UNKNOWN(range_unsupported)`，包括边界新增和顺手改到原有字段内容。第一版不把这些字段传进当前整字段回放器当作成功覆盖。错字段或观察未稳定时输出 `UNKNOWN`。
2. 在固定上下文中比较稳定编辑与交付文本，再将同一处改动对齐到**实际交付路径**及口述参考。第一版支持单处且对齐唯一的目标。多处混合、重复短语造成位置歧义、跨越字段边界或已知编辑来源不可信时输出 `UNKNOWN`。保留大小写、词界和有意义的标点。guard 未采用的候选另记阶段观察，从用户编辑的交付路径中排除。
3. 按下表判定。证据不足时记录具体原因，例如 `reference_missing`、`stage_missing`、`alignment_ambiguous`、`edit_unsettled`。若已能证明某层输出有差异，但不能证明它就是用户这次编辑的原因，分别记录阶段观察与 `UNKNOWN` 编辑归因。

| 条件 | 归因与结果 |
| --- | --- |
| 原话不含新增片段，独立标注确认后来补写，已知链路也没有漏掉原话 | `USER_ADDITION`，零学习写入 |
| 原话正确交付，之后用户改写或删除内容，或新增内容但缺明确补写标注 | `CONTENT_EDIT`，零学习写入。无独立标注时，用户意图仍为未知 |
| 原话有这个片段，ASR 首次漏掉，直到交付都仍缺，稳定编辑补回 | `ASR_OMISSION`，计入漏听。纯补字或短语补回不写词对 |
| ASR 首次把参考词写成另一词，错误持续到交付，稳定编辑在同处恢复参考词 | `ASR_SUBSTITUTION`，交给词汇准入判定 |
| ASR 正确，准备层首次产生差异并传到交付，稳定编辑恢复参考 | `LOCAL_PREP_REGRESSION`，零学习写入。只有具体规则的局部改动与该差异唯一对应时才填规则 ID |
| ASR 与准备稿正确，cleanup 候选首次产生差异且交付采用，稳定编辑恢复参考 | `CLEANUP_REGRESSION`，进 cleanup 评测，零学习写入 |
| Cleanup 候选变坏，guard 拒绝并回退 | 记录候选回归且 `delivered=false`。用户编辑按实际回退路径另判。若回退稿仍有 ASR 错词，可独立形成 `ASR_SUBSTITUTION` |
| 参考、阶段、编辑来源或唯一对齐的证据不足，或出现多阶段修好后再改坏等第一版未覆盖情形 | `UNKNOWN`，零学习写入 |

## 4. 学习写入条件

- 只有 `ASR_SUBSTITUTION` 可进入第一版自动学习准入。实际观察器提取的单个完整词对，必须和归因恢复的词对完全一致，并通过既有词汇类别、权限、作用域和来源绑定校验。fixture 的词对许可只控制写入资格。
- 通过后记录一条带作用域的候选证据。现有晋升策略负责判断何时生效。第一版的固定 fixture 清单为每个 `origin_id` 绑定参考、各阶段及选择路径、交付基线、编辑轨迹和作用域摘要。每次执行及重开后按同一份清单核对，改动证据但沿用同一 origin 时拒绝，记 `origin_evidence_conflict`。写入时由已验证的 `origin_id` 派生 `stable_source_origin_id`，不接受独立的调用方别名。复用现有 SQLite 的 observation key 和唯一约束；同源同规则换 operation ID 或重开都不增加计数，拒绝回执不占用已落库证据的计数名额。第一版只验证隔离组中固定作用域下的去重，产品 AX 和 History 的共同 origin 留给 C0/C1。
- 补写、漏听的纯插入、内容变化、本机规则回归、cleanup 回归及 `UNKNOWN` 不调用学习写入入口。运行回执可记录拒绝，学习数据库中不创建候选或学习证据，也不增加计数、晋升、usage 或修改词典设置。
- 第一版验证既有作用域行为和单一规则在授权场景的应用。跨作用域规则身份、产品跨通道去重及设置持久化扩展仍由总计划的 C0/C1 另行处理。

## 5. 回放和临时 SQLite

现有轨迹接受 2 到 6 个完整字段快照。第一份作为交付基线，观察器每次轮询读取下一份，耗尽后会重复最后一份。测试以空 `sleep` 回调代替真实等待，当前回放默认同一字段持续有效。新入口保留这个观察器，并在归因准入时要求稳定所需的确认读数来自显式快照。耗尽后的合成重复单独计数，不能补足确认读数。正例按模拟时钟显式提供达到稳定窗口的读数，负例覆盖只有一次变化后读数、未消费的后续快照、失焦和缺读数。通过回调控制这些情况，断言实际消费数和稳定状态。

复用 `LearningArmV2::open` 及真实 store。每组在系统临时目录的独立私有子目录中创建 `learning-arms-v2/<arm_state_id>/`，首次初始化时保存绑定 episode 的 `arm.json` 和 fixture `settings.json`。重开时读取并验证原组配置，保留已有状态。SQLite 由 `store` 在该组目录下打开 `history.sqlite`。保留既有路径、权限与符号链接、硬链接及 sidecar 校验。组内允许明确重放和重开，组间从相同的审核初始设置和状态分别初始化。

在初始化与 schema 打开完成后读取初始学习状态。判定完成后再通过 store 读回规则、证据、计数、晋升、作用域及相关 usage，并核对整份 fixture `settings.json`。预期零学习写入的用例同时要求产生正确的归因回执、记录写入未被调用，以及学习逻辑状态和行数不变。SQLite 文件存在或辅助文件变化不构成学习写入。正例检查实际候选状态及有效规则。

## 6. 隔离验收矩阵

先直接调用纯裁判，独立断言归因、原因和准入。再用合成字段轨迹接真实 store，断言端到端隔离状态。矩阵前四条固定从 `Deploy today` 变为 `Deploy today after lunch`，只换上游证据。词汇门槛另用相同的 `VoiceFlo → VoiceFlow` 编辑、相同授权和同一作用域做正反例，确认它们都能提取出同一词对，只有 ASR 纠正通过归因门槛。

| Fixture | 核心断言 |
| --- | --- |
| 原话没有 `after lunch`，有独立补写标签 | `USER_ADDITION`，零学习状态变化 |
| 原话有，ASR 漏掉 | `ASR_OMISSION`，零词对及计数 |
| ASR 与准备稿保留，cleanup 删除且放行 | `CLEANUP_REGRESSION`、`delivered=true`，零学习状态变化 |
| 同样编辑但缺原话参考 | `UNKNOWN(reference_missing)`，零学习状态变化 |
| guard 拒绝坏候选，交付 `Deploy today after lunch`，轨迹保持该值 | 候选错误记为未交付，`UNCHANGED` 轨迹产生零学习写入 |
| guard 拒绝坏候选，回退稿仍写作 `VoiceFlo`，用户纠正为 `VoiceFlow` | 候选回归记为未交付，实际编辑沿回退路径归因 ASR |
| 同一合格词对由准备层或 cleanup 首次改错，或原话参考缺失 | 实际提取的词对和授权都通过。准备层回归、cleanup 回归及 `UNKNOWN` 分别有回执，归因门槛阻止调用写入 |
| 准备层命中多条规则，局部证据无法唯一定位责任 | 准备层回归，具体规则未知，零学习状态变化 |
| `VoiceFlo` 被稳定纠正为参考中的 `VoiceFlow` | 首次独立证据 `+1`；验证前提是实际候选、作用域与授权均匹配 |
| ASR 归因成立，但无词对许可、作用域不匹配，或观察提取的词对与归因不一致 | 保留归因回执，独立准入拒绝，零学习状态变化 |
| 同源换 operation ID 重放或重开后重放；另测同源篡改上游阶段、轨迹或作用域 | 完全相同的重放计数不增加；不同证据被拒绝且不写入 |
| 后续独立 origin 达到既有规则门槛，fixture 使用门槛至少为 2 的词汇类 | 门槛前未晋升，达到时才晋升。关闭重开仍实际应用，新句同作用域命中，其他作用域不应用 |
| 撤销后重开同一个隔离 arm | 撤销状态能从存储读回，旧规则不应用。用新的独立 origin 检查既有阻止被动重学的契约 |
| 错字段、缺阶段、对齐歧义、编辑未稳定、origin 不一致或交付与选中路径不一致 | 有具体原因，零学习状态变化 |
| 补写标注与原话冲突、缺补写标注、字段开头或尾端是部分插入边界 | 分别断言冲突未知、内容变化但意图未知，以及部分字段范围未支持，均零学习写入 |

验收顺序为纯裁判矩阵、观察回放接裁判、临时 SQLite 状态读回。测试结果注明使用合成证据、模拟回读和真实隔离数据库。运行范围限于 Rust 定向纯测试及隔离集成测试。

## 7. 实施索引

- `src-tauri/src/dictionary_learn.rs` 的 `observe_controlled_fixture_trajectory`、`record_controlled_fixture_edit_trajectory` 与候选写入前边界。
- `src-tauri/src/learning_research.rs` 的 `LearningArmV2::open`、`validate_arm_base_root`、`validate_owned_arm_storage` 及 `v2_text_arm_records_deduplicates_promotes_and_reopens_isolated_state`。
- `src-tauri/src/store.rs` 的 `open_history`、学习证据写入和规则读回。
- [Cleanup 与纠错学习 harness 改进总计划](cleanup-learning-harness-plan-2026-11-05.md) 中的错误归因、独立计数与作用域契约。
