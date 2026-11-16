# VoiceFlow 词典学习 Harness 执行方案

**工作区状态（2026-11-12）：当前 checkout 包含 History-owned provenance、metadata-only 写作偏好候选、Context Settings 用户发起的批量复核和 revision-safe UI reconciliation。最新 WritingPreferenceReview 定向测试 14/14、`npm run lint` 通过；backend named owner-chain regression 在隔离 History DB 中以合成文本通过 1/1（命令见 §7.3）。这些是当前工作区证据，不声称已合入 `main`。词对 History / live 生产弱证据只保存待确认候选，显式确认后仅在记录 scope 本机生效；2/3 词对门槛仅由 research/test fixture 使用，已有已生效规则保持原状态。该 owner test 不验证真实 Tauri IPC / Test bundle、macOS Host / IME 或真人结果；独立 shadow / holdout / 用户收益仍未完成，自动推断启用仍关闭。列表结构偏好只走本机输出投影，不进入云端指引。**

```text
[第 1 周：预检与真人闭环]
  隔离 Test bundle，冻结构建、模型和规则，记录状态指纹
  预检 App A 的 Verified 插入和 App B 的跨作用域对照
  本机 ASR，关闭术语注入、模型 cleanup 和 fuzzy
  |
  +--> [显式 Teach]
  |      录音 -> 粘贴 -> 真人改字 -> 确认 App A 词对
  |      -> 说一句新话 -> 独立核对目标字段 -> 忘记并重启
  |      同一份 ASR 原稿另做规则开关对照
  |
  +--> [被动观察：独立状态]
  |      核对实际纠正、合格机会、捕获、正确和未知
  |      日常改字无需逐次确认，疑似词对可在词典页主动复核
  |      至少 5 次 IME 组字 + 5 次切焦，中间态和错字段写入为 0
  |      漏捕记入报告，显式 Teach 可继续验收
  |
  +--> [术语出口：独立验收]
  |      用合成标记词检查请求，开放云端提示前须通过
  v
[第 2 周：技术验收 | 2 人，40 次真实听写]
  +-- 12 次 App A 正例：至少 8 次含合格 alias，合格试次全部改对
  +--  8 次 App B 负例：至少 4 次出现 alias，误用为 0
  |                     B 登记同词对须明确报冲突，A 规则整行不变
  +--  8 次正确原稿及边界：大小写、引号、代码，新增误改为 0
  +-- 12 次忘记与重启：两个状态都遇到旧 alias，复活为 0
  |
  +-- 未通过或证据不足 --> 停该批，保留结果，按冻结方案另开批
  |
  +-- 通过
        |
        v
[收益试点 | 4-6 人，每人 40 条新录音]
  20 目标词 + 10 安全负例 + 10 普通听写
  按人匹配词族，再随机分配 Teach / 对照，冻结规则
  |
  +-- 终态证据 >= 95%
  +-- 目标词正确免改交付率提升 >= 15 个百分点，至少 N-1 人改善
  +-- 扣除 Teach、重教和撤销成本后，净省时中位数 > 0
  +-- 新增关键误改、越域、越权 = 0
  |
  +-- 全部达标 --> 扩大试验，正式发布另做独立留出验收

[C1：独立触发]
  同词对需要跨 App 并存 / 发现状态串扰 / 要扩大自动生效
  --> 先完成 C1 迁移和隔离验收 --> 再开放相关行为

[列表 / 语气偏好：后续独立验证，不并入上述词典两周与收益试点]
  当前能力与新增设计分开记录
  --> 本机 shadow 草稿 --> 独立标签与冻结 holdout --> 用户收益试验
  --> 显式保存 / 一次批量批准后，在同 App x 任务的兼容场景应用
  --> 可选自动学习：另有类别授权、身份隔离与独立留出门槛
```

## 1. 适用范围与证据基线

本文规定前两周技术验收及后续收益试点的执行顺序；[旧计划](../cleanup-learning-harness-plan-2026-11-05.md)保留详细技术合同，顺序冲突以本文为准。配套参阅[端到端流程](../end-to-end-workflows.md)、[清理评估](../cleanup-evaluation.md)、[隐私约束](../privacy.md)及[V3 多作用域夹具](../../src-tauri/src/fixtures/learning-v3-multiscope.json)。

基线已有本地 C0 定向 4/4、归因 18/18、161 条 cleanup 聚合通过，以及 V3 stdin 26 次、重启后 2 次验证。C0 来源记录、origin 去重和部分失败重试已实现。旧 A0 两项探针缺现代 owner/origin，先修夹具，不据此新增生产缺陷。这些记录不等于本方案已验收。

受控 native Host 的粘贴、合成编辑与 AX 证据，和真实 AppState 中的人工纠正、后续录音属于不同证据层，分别报告。

本文的「当前」指本节基线与已合入的产品能力。第 7 节区分显式偏好设置/本机列表应用和仍待验证的推断学习及独立评估；列表、语气偏好不继承 C0、归因 judge 或词典试点的通过结果，也不改变第 1/2 周顺序、样本数及第 5 节门槛。

## 2. 首轮环境、隔离与术语出口

第一步是建立有界状态指纹并预检真实字段。使用隔离 Installable Test bundle、现有词典 UI、audit、V3 和独立目标终态；确认原生纯文本 App A 能产出 `Verified`，仅有 `VerifiedFullRange` 不通过，并预检 App B。

冻结构建、模型、权限和初始状态；采用无术语注入的本机 ASR，cleanup Off、fuzzy Off。选择未进入全局词典、不会触发 CJK 同音衍生的 Latin 专名，使用现有显式原生 App 词对。机制对照固定同一 ASR 原稿，仅切换精确替换执行，其他策略不变；不能通过删除整条词对连带改变提示或 guard。

每个任务在开始前和本次 owner 结算后取指纹。开始前先确认上一任务已结算。指纹覆盖规则行及来源、scope、计数、tombstone、全局词典和有效 hint。事先声明批准、使用计数、忘记等允许变化。未声明变化视为污染。

被动观察使用独立隔离状态，可写自身数据，但不得改变收益试验的规则、全局词典或有效 hint。无法证明隔离则标记 `blocked`。保持现有权限和隐私边界，不建立影子表或新 runner。

术语出口单独用合成标记词验证请求序列化及本机测试收集器：仅在规则库中的词不得越权注入；自然出现在输入中的词可以作为正文，但不因此获得 hint 权限。`manual_scoped`、`auto_scoped`、任何声明 scope 的行及无法确认全局身份的旧 pair 都不得成为云端 pair hint；完整适用规则仍用于本机匹配和冲突阻断。若本机应用的 pair 不具备完整云端资格（scoped / local-only，或规范词缺少独立全局 owner）且实际改变发往远端 Cleanup 的正文，则不发起该次远端请求，保留本机结果并记录 `local_pair_egress_blocked`；scope 不匹配且原稿未被本机 pair 改写时仍可按原选择云端路由发送自然原文。本机 Ollama、MLX、llama.cpp 可继续获得完整本机提示。ASR / fuzzy 只外发独立全局 owner 允许的词；仅由 scoped Rule、已无当前对应规则的孤儿 Rule、`legacy_unknown` 持有或没有 owner 的词不进入全局提示，SQLite 规则库缺失或读取失败时也 fail closed；独立有效的 Manual / Screen / Other 或全局 Rule owner 仍可授权该词。当前 ASCII fuzzy 在各整理路线复用同一严格全局词表；仍存在的旧 `LegacyUnknown` pair 可本机 exact 替换，但不会参与 fuzzy，需显式恢复可验证 owner 才能进入全局 ASR / fuzzy。scope 排序不能代替排除，B 不替换不能证明未外发。开放云端提示前独立验证出口，本地试验不外发真实语料。

### 2.1 本机与云端路由及资源测量

ASR 和 cleanup 分别选择路由。产品默认 ASR 是云端 Groq `whisper-large-v3-turbo`，已保存的 provider 和模型选择继续生效。云端 ASR 按路由发送音频。On Device ASR 在本机转写，若选择云端 cleanup，整理阶段仍按授权和路由发送文本；若本机 scoped / local-only pair 已改变待发正文，则该次 Cleanup 保留本机结果并跳过远端请求，不更改设置的整理路线。严格离线限制为受支持的本机路线。粘贴后的有界改字观察和规则判断在本机执行，派生词条进入后续模型请求时适用各自的出口授权。本计划首轮固定本机 ASR 和 cleanup Off。 [ASR、cleanup 与离线模式](../privacy.md#数据流)

本机模型冷加载按 `phys_footprint` 和内存压力准入。VoiceFlow 自有 sidecar 的合计预算以物理内存的 80% 为上限，另有按模型和 revision 固定的冷加载预留。例如 Qwen3-ASR 0.6B 为 2.25 GiB、Parakeet V3 8-bit 为 4 GiB、Qwen3-ASR 1.7B 为 10 GiB。预留值用于判断能否开始加载。实测峰值、热态占用、磁盘制品大小各自记录。 [内存准入实现](../../src-tauri/src/model_resources.rs)

第 1 周先测同机、同构建和同模型的空闲、首次听写和热态听写。记录停说到目标确认的 p50/p95 及交付失败数，同时记录主进程与实际启用的 sidecar 的 CPU、合计峰值 `phys_footprint`、内存压力和 swap。再在相同路线与任务组合下比较观察开启和关闭，单看观察耗时、AX 读次数及 CPU 增量，避免把 ASR 冷加载算成学习开销。复用已有本机听写诊断和系统采样，报告硬件、模型 revision、任务数与采样覆盖。后续开启本机 cleanup 时，再测 ASR 和 cleanup 同时驻留的资源峰值。 [本机性能记录与交付口径](../end-to-end-workflows.md#1-普通口述默认自动交付) · [本地模型性能矩阵](../local-model-expansion-plan-2026-11-09.md)

执行时先依[隔离预检流程](../end-to-end-workflows.md#week-one-dictionary-learning-preflight-and-isolated-verification)确认精确 Test bundle、独立 AppData 和已安装模型；用 Settings → Engine →「本机性能诊断」复制 `get_latency_metrics` 的内容无关汇总，并在获准的 Test AppData 下关联 `diagnostics/dictation-performance.jsonl`。只有确实使用本机 MLX cleanup 时才一并收集 `diagnostics/mlx-cleanup-performance.jsonl`。将每项任务的路由、ASR/cleanup 模型与 revision、build hash、任务 ID 和诊断覆盖情况写入仓库外的私有记录；缺失事件保留为缺失，不由摘要补造。

物理采样在同一预注册的 300 秒任务块内并行启动。把 `APP_PID`、`ASR_PID`、`CLEANUP_PID` 替换为从该 Test 进程树确认的实际 PID；不存在的 sidecar 省略对应 `--pid`。输出目录必须是仓库外新目录并限制为当前用户读写。无需提升权限，采样权限不足或 PID 身份在块前后变化时记为 `unknown`。

```sh
RESOURCE_ARTIFACT_DIR=/absolute/private/resource-run-001
APP_PID=12345
ASR_PID=12346
CLEANUP_PID=12347
mkdir -m 700 "$RESOURCE_ARTIFACT_DIR"
sysctl -n hw.memsize > "$RESOURCE_ARTIFACT_DIR/hw-memsize-bytes.txt"
sysctl -n kern.memorystatus_vm_pressure_level > "$RESOURCE_ARTIFACT_DIR/pressure-before.txt"
ps -o pid=,ppid=,lstart= -p "$APP_PID,$ASR_PID,$CLEANUP_PID" > "$RESOURCE_ARTIFACT_DIR/processes-before.txt"
```

在另外三个终端并行运行以下采样；任务块结束后再保存 `processes-after.txt` 和 `pressure-after.txt`。按进程 ID 汇总主进程及实际 sidecar 的峰值 footprint 与 CPU，不把全机 `vm_stat` 计数误称为 VoiceFlow 单进程用量。

```sh
footprint --format bytes --sample 1 --sample-duration 300 --pid "$APP_PID" --pid "$ASR_PID" --pid "$CLEANUP_PID" --json "$RESOURCE_ARTIFACT_DIR/footprint.json"
top -l 300 -s 1 -stats pid,ppid,cpu,time,mem,rprvt -pid "$APP_PID" -pid "$ASR_PID" -pid "$CLEANUP_PID" > "$RESOURCE_ARTIFACT_DIR/top.txt"
vm_stat 1 300 > "$RESOURCE_ARTIFACT_DIR/vm-stat.txt"
```

任务块结束后在 setup 终端执行：

```sh
sysctl -n kern.memorystatus_vm_pressure_level > "$RESOURCE_ARTIFACT_DIR/pressure-after.txt"
ps -o pid=,ppid=,lstart= -p "$APP_PID,$ASR_PID,$CLEANUP_PID" > "$RESOURCE_ARTIFACT_DIR/processes-after.txt"
```

pressure 前后值是采样时点快照（`1` Normal、`2` Warning、`4` Critical；其他值或读取失败为 `unknown`），不是 300 秒内连续压力证明；记录实际采样覆盖，未观察区间保持未知。

首次加载只在本机 ASR 诊断实际出现 load 阶段时标为冷加载；等待时间或新进程本身不证明模型已卸载。任务和端点不得改变系统内存压力，不调用分配压力的工具。收集的诊断与进程采样只留在私有本地目录；不提交、不上传，也不保存听写原文或音频。

## 3. 第 1 周：真实人工链路

通过真实 AppState 跑通现场录音、实际粘贴、真人纠正和独立终稿，再经现有词典批准，验证同 App 的新句子、跨 App 负例、忘记及重启。

真人任务必须使用全新现场录音；固定原稿开关对照仅用于机制归因。发出 Cmd+V、AX 确认、独立终态和内容正确分别记录，不以内部产物代替目标字段。复用已有组件，不重写 clipboard supervisor，也不以合成 Host 成功代替真实链路证据。

### 3.1 监控听写和词典状态

验收开始前给每个任务分配编号。真实人工试验使用冻结的普通 Installable Test bundle 与 bounded `get_dictionary_learning_audit`；该 bundle 不含 `research-harness`，`--research-stdio` 回执不能作为真人验收证据。每个任务记录目标 App、交付、观察结果和原因、候选决定、规则及 Settings 写入、替换和独立终态，并用签名 build-owner receipt 与内容无关的 owner receipt 绑定到对应原生 audit activity。owner receipt 核对同一运行身份、审计代次、稳定投影及遗漏计数。普通 audit 仅保留当前进程最近 128 条活动，重启会清空；重启前记录所需的审计元数据，重启后用持久化规则和独立目标读回核对。缺少活动、回执或唯一任务关联时记 `unknown`，不能用超时或未读回补造捕获。 [审计和研究入口](../end-to-end-workflows.md) · [审计隐私边界](../privacy.md#normal-dictionary-learning-activity)

同一任务同一 runtime 与 observer epoch 可有多条 bounded-audit activity，按一次物理 capture 汇总。owner receipt 必须绑定任务与普通 audit；audit 证明观察到词对候选时记 capture，同一 capture 内多个 token 的候选 activity 可分组。独立标签绑定 `observer_owner_receipt_sha256`、`observer_capture_id` 和与 owner receipt 完全一致的 sorted `observer_candidate_refs`。`observer_correct=yes` 只有在独立标签有效、`eligible_correction=yes`、normal audit 为正向捕获，且绑定字段与唯一 candidate ref 完全匹配时才可计正确；`observer_correct=no` 可在单一 ref 完整绑定的捕获中表示已核实负例，即使 `eligible_correction=no`。分组多候选仍算捕获，但正确性记 `unknown`。候选持久化失败不抹去已证实的 capture。已核实的 `no_grant`、timeout、unreadable、无合格候选或无写入的 superseded/stale activity 记为捕获未发生，不是缺失证据；不支持的 multiple-token 结果、事实冲突或缺失/未绑定的 activity 与 receipt 记 `unknown`。

| 每批复核什么 | 取证和分母 |
| --- | --- |
| 交付和观测覆盖 | 从全部分配任务算独立终态覆盖率。观察组由参与者终稿、改动说明和独立标签确定真实纠正及合格纠正。合格但 `no_grant`、timeout、未启动或读不回的纠正仍留在合格分母并算漏捕；活动、receipt 或独立标签缺失则记 `unknown`，保留在分配总数并列明原因。 |
| 捕获和归因质量 | 捕获召回率为 `eligible_captured_corrections / eligible_corrections`；捕获精确率为 `correct_captures / captured_corrections`。误捕仍计入捕获分母，另列误捕和正确性未知数；缺少资格/捕获或正确性未定时对应比率不可用。分母为 0 时显示不可计算。 |
| 写入和规则生效 | 对照任务前后的状态指纹，分别核对候选保存、SQLite 晋升、Settings 同步和实际替换。输出出现规范词或 usage 增加只说明对应的本机信号。正确免改交付须由独立目标终态和任务参考判定。 |
| 日常打扰与误学 | 报告主动弹窗数、用户打开词典后批准的词对数，以及内容编辑、宿主改写和意图未知中进入候选或生效的数量。首轮收益路径逐次确认弹窗为 0。audit 的 128 条窗口用于定位原因，分母来自预先分配的任务及独立标签。 |
| 需要停批的异常 | 发现越权读取、错字段学习、状态污染、越域应用、忘记后复活或未经授权的术语注入，暂停该批，保留任务编号和原因，定位后按冻结方案重跑。普通漏捕用于排定修复优先级。 |

每批结束、进入下一批前复核这些计数。验收表保留任务编号、时间、状态码、计数和所需的状态摘要。阶段原文、音频或终稿仅在明确授权的受控任务中按需本地取证。普通审计沿用现有保留边界。偏好试验另用自己的任务清单、状态与分母，不把其记录混入词典验收；新增归因维度的取证要求见第 7 节，不能假定当前 audit 已提供全部字段。

### 3.2 判断改字的意图与错误来源

日常听写继续交付，用户可以直接改字。已批准的 App 词对在后续符合 scope 时自动应用。对新词对，本机观察先产生证据，词典页是用户需要时主动查看和批准的入口。

1. **确认看到的是这次插入范围内的稳定变化。** 使用已授权的同一字段和插入锚点，核对事务、焦点、观察代次及范围。已知宿主自动格式化、输入法组字中、范围外编辑或无法证明提交的变化不进入拼写候选。稳定读回只证明文字稳定。未修改也不给学习证据加分。
2. **判断是否有足够证据留下疑似纠错。** 同事务 ASR 原稿已出现同一错拼，它沿实际交付路径保留到插入，稳定终稿只有一个可对齐的词发生替换，且目标拼法有独立词汇依据时，可以进入候选评估。同一 scope 内、不同真实听写事务的一致改动提高优先级，按现有 origin 去重。ASR 候选或置信度仅在当前路由提供且能对齐这个词时作为辅助信号。词典未收录、单次快速替换和重复次数本身都无法证明原意。生产 History / live 弱证据只产生待确认候选，不按姓名 2 次、术语 3 次门槛自动晋升；这两个计数只留在 research/test fixture。首轮收益组只应用已明确批准的规则。
3. **证据歧义时保留终稿并停止学习。** 两个都成立的专名、同音对象、日期、数字、否定和内容改写不得仅靠 diff 晋升。同样的 `陈晨 → 陈宸` 既可能恢复原话，也可能换了收件人。缺少区分两者的证据时记 `unknown`，无需向用户弹窗追问。若能证明已生效规则改动了对应位置，随后被稳定改回，则复核原规则版本，避免累积反向词对。
4. **把授权放在用户主动复核时。** 有足够证据的疑似错拼可沿用有界候选记录，保留词对、scope、事务来源、阶段依据和缺证原因。计划让用户在词典页一次复核多项，每项显示词对及适用 App。批准授权未来替换，历史上的真实意图仍可保持 `unknown`。若未来重新提出自动生效，须先通过 C1 和独立留出验证；届时首个可评估的自动晋升范围限于用户已授权该类学习、同 scope 有已确认的唯一规范拼法、只漏了一个字母或重复了一个字母的 Latin 专名，并且能验证提交、排除宿主改写及代码或引号。缺任一证据时只留合格候选或静默放弃。

### 3.3 受控验收的独立标签

同一字段的稳定变化只能定位编辑。验收时先用预先冻结的任务参考或参与者对原话的独立确认，建立「这次原本想写什么」。再让参与者说明改动是恢复刚才说的内容、改变内容或写法，还是无法确定。获授权的原始音频可用于核对原话。同音专名的具体拼法须由任务参考或参与者确认。独立终稿用于核对实际修改，不能单独充当原话参考。

将这份参考与同一任务的 ASR 原稿、本机准备稿、实际采用的 cleanup 路径、实际插入文字和稳定终稿逐层对齐。以沿实际交付路径首次引入且保留到插入时的差异定位错误。某层改坏后已回退的情况可单报该层异常。按下表处理观察到的改字：

| 证据 | 归因和动作 |
| --- | --- |
| 用户确认原本说的是 `VoiceFlow`，ASR 输出 `VoiceFlo`，错拼进入实际插入文字，终稿恢复 `VoiceFlow` | ASR 拼写替换。可作为合格词对证据，首轮仍由用户通过现有词典界面批准该 App 的词对。漏词单独计入 ASR 错误。 |
| ASR 已符合原意，实际采用的本机准备或 cleanup 改坏，用户恢复原意 | 记对应阶段回归。若能关联到实际应用的词对，记录为该规则的负反馈。 |
| 用户确认是临时改变事实、对象、措辞或写作偏好，例如说了「周五发」，随后决定改为「周一发」 | 记为用户编辑，保持本次终稿，排除在 ASR 拼写证据和词对晋升分母之外。 |
| 缺原话参考、阶段数据或稳定终态，或发现 IME 中间态、App 自动格式化、错字段、两种意图混合且无法分辨 | 对缺证据或混合编辑记 `unknown`。可明确识别的 App 自动格式化单列为非人工编辑。首轮均不计入 ASR 拼写证据或自动晋升。 |

验收加入同一 diff、不同来源的配对。用 `Voice Flow → VoiceFlow` 对比 ASR 拆词与 cleanup 拆词。用「周五 → 周一」对比 ASR 听错与用户后来改日期，两者都排除在词典晋升之外。用 `陈晨 → 陈宸` 验证产品证据相同但真实意图不同的情况，日常判定应保持 `unknown`。用现有 test-only 归因 judge 的夹具验证阶段和放弃边界。真人标签来自独立参考和参与者确认，在查看系统分类结果前定下。上述表格定义词对归因，不把一次改写直接转换为长期风格证据；明确本次指令、临时模板和临时改变写法均不计长期偏好的正反证据。长期偏好需按第 7 节独立验证。 [归因 judge](../cleanup-evaluation.md#deterministic-edit-attribution-judge-test-only)

### 3.4 从用户改字到下一次听写

图中词对采用首轮显式批准路径。「现有」风格样例与强度能力按基线记录，词典收益试点固定这两个因素；「拟」为第 7 节后续偏好设计。标注「云端」的请求由用户下一次主动听写触发，内容取决于所选路由与授权，保存本机偏好本身不触发请求。

```text
[已验证插入 -> 用户在同一字段改字]
  |
  v
[本机：有界观察、对齐原稿和编辑、判断提交与证据]
  +--> 疑似错拼 -> 本机候选 -> 用户在词典批准 -> 本机 App 词对
  +--> 现有风格信号 -> 本机草稿 -> 用户批准样例 -> 本机 App 风格样例
  +--> 现有强度学习 -> 合格短改写达到条件 -> 本机 App 强度设置
  |                    词典首轮冻结，不以新偏好证据驱动此路径
  +--> 拟：列表 / 语气 / 标点弱证据 -> 本机草稿，不改变下一次输出
  |      -> 现有设置入口显式保存 / 一次批量批准
  |      -> 用户 + App x 任务 x 维度的偏好，后续独立验收
  +--> 本次临时指令 / 模板 / 本次覆盖 -> 仅本条，不计长期正反证据
  +--> 改主意 / IME 或宿主歧义 / 缺证 -> 保留终稿，放弃相应学习

[下一次主动听写]
  |
  +--> 本机 ASR：本机转写
  +--> 云端 ASR：发送本次音频
  |               支持且获准时另带筛选后的规范词
  v
[ASR 原稿 -> 本机准备：scope 命中词对后替换；保留原文来源]
  |
  v
[拟：按本次明确意图、已保存偏好及任务兼容性选择]
  +--> 本次指令 / 模式优先，显式设置不被推断覆盖
  +--> 列表：有原话项目边界 -> 现有本地布局；无边界则跳过偏好
  +--> 语气 / 标点：仅为已选路线提供允许的有界指引
  +--> 任务不明 / 规则冲突 / 不兼容 -> 沿现有行为，不扩大 scope
  |
  v
[按已授权场景选择 cleanup 路由，不因学习偷偷换路线]
  +--> Off 或本机规则路径 -> 使用允许的本机准备与布局
  +--> 本机模型 -> 选择现有模板，在本机推理
  +--> 云端模型 -> 本次正文 + 已获准的结构化指引
                   含历史原文的批准样例须有相应云出口授权
                   未批准候选、编辑 diff、音频重放不得因学习上云
  |
  v
[最终保护检查 -> 实际交付 -> 受控验收独立终态]
  |
  +--> 已提交的逆向编辑 -> 核对实际触发版本，质疑 / 暂停推断规则
                           不自动学反向，不改写显式设置
```

现有系统 prompt 由代码根据场景、输出模式和整理强度选用模板。全局词对命中、已批准的样例及用户保存的语气指导会按各自路由与出口授权进入本次请求的参考字段；scoped / local-only pair 只给本机匹配与提示，若它实际改写待发 Cleanup 文本则本次远端请求不发送。强度变化影响下次选用的整理模板。之后选择云端整理时，已批准的风格样例可能包含过去修改前后的短文本；样例批准、未来应用与云端出口授权需分别核对，不能由 AX 读字许可推导。

新增偏好仅拟控制现有本地布局或本次请求的受限指引，不在线修改基础 prompt、模型权重，不增加第二个模型。严格离线只走受支持的本机路线；路线不能表达某偏好时跳过该偏好，不能自动转云。详见第 7 节。 [整理请求组装](../../src-tauri/src/llm.rs) · [路由和隐私边界](../privacy.md#数据流)

### 3.5 词典和词对各自保存什么

| 本机数据或资源 | 学习写入与后续用途 |
| --- | --- |
| 个人词典 `Settings.dictionary` | 初始为空。用户可添加或导入规范词，后续由支持的 ASR 路由选作术语提示，也可供显式开启的 ASCII 模糊词典使用。词对 History / live 弱证据现在只保存待确认候选；显式确认后按记录 scope 本机生效，不自动追加到全局词典。 |
| 纠正规则 `learn_pairs` | 保存 `识别为 → 应替换为`、适用范围、证据及规则状态。手工 App 词对、History / 本机观察待确认候选及已确认规则可保存在对应 scope 内；显式确认的规则可在本机替换或提供本句本地 cleanup 拼写参考，无需把规范词加进全局词典。只有已证明为全局的规则会成为云端 pair hint；若本机 scoped pair 改写将发给远端 Cleanup 的正文，该请求不外发，保留本机结果并记录回退原因。忘记规则时，其他来源仍持有的规范词继续保留。 |
| 内置拼音和匹配逻辑 | 用固定的拼音转换、词边界及冲突判断辅助筛选和替换。学习积累的是个人词条或词对。当前 On Device ASR 直接运行所选模型，应用词对在 ASR 原稿出来后处理。 |

ASR 对获准的全局词表使用 scope 排序。App 词对只在该 App 替换；其规范词仅由 scoped Rule、已无当前对应规则的孤儿 Rule、`legacy_unknown` 持有或没有 owner 时不会作为全局 ASR / fuzzy 提示，SQLite 规则库缺失或读取失败时也 fail closed；仍有独立 Manual / Screen / Other 或当前有效的全局 Rule owner 时则按该全局授权处理。当前 ASCII fuzzy 在各整理路线复用同一严格全局词表；仍存在的旧 `LegacyUnknown` pair 可本机 exact 替换，但不会参与 fuzzy，需显式恢复可验证 owner 才能进入全局 ASR / fuzzy。按第 2 节分别验收替换范围与云端术语出口。 [个人词典与规则所有权](../end-to-end-workflows.md#normal-dictionary-learning-audit-boundary) · [本机词对应用](../../src-tauri/src/lib.rs) · [词表排序与拼音匹配](../../src-tauri/src/lexicon.rs)

### 3.6 列表、语气和用户习惯：现状与待做边界

| 维度 | 当前能力 | 本方案边界与后续设计 |
| --- | --- | --- |
| 列表或 bullet points | 明确说出「第一、第二」或重复的项目符号标记时，符合条件的口述结构可由本机解析；「要点 / 步骤」输出模式可在适用路线整理有原文依据的列表。已保存的 App × 任务列表偏好可在兼容时经本机输出投影变成 bullets，不进入云端指引；History weak candidate 与用户批量复核已实现，合成 owner-chain 测试通过；真实 App / Test bundle 验收、独立 shadow 与 holdout 仍待执行。 | 显式设置与本机列表投影已实现；用户批准的 History 弱证据可转为显式设置，候选不自动生效；自动推断及 shadow/holdout 另按第 7 节验证，不计入词典首轮。 |
| 语气和标点 | 已有场景语气和用户可编辑的语气指令；已支持按 App × 任务保存显式偏好，并对云端指导单独授权。部分标点、短句称呼或口语变化可形成既有风格草稿，批准的输入／输出样例可作为对应 App 的整理参考。History weak candidate 与用户复核链已实现并有合成 owner-chain 测试；真实 App / Test bundle provenance、独立评估和人类标注仍未完成。 | 显式设置/应用已实现；只在候选经用户批准后成为显式偏好。自动推断仍关闭，且任务范围不能可靠表达时不启用新路径。 |
| 整理幅度 | 现有窄路径按合格短改写信号累计条件调整场景 cleanup intensity；现行 3 次机制仍须与新设计分开记录。 | 词典首轮冻结强度。第 7 节的 3 笔候选证据门槛不调用、不验证也不替代该机制；语气或列表信号不得顺带改变强度。 |

词汇替换、语气、标点、列表和强度分别验收。显式偏好设置与本机列表投影已实装，不代表从真实修改形成的推断候选、复核或收益已通过；冻结须覆盖实际状态写入，不只固定界面显示。词典主试验中的未经声明变化仍按污染处理。 [口述列表](../../src-tauri/src/spoken_layout.rs) · [输出模式](../../src-tauri/src/cleanup_output_resolver.rs) · [风格草稿与强度](../../src-tauri/src/dictionary_learn.rs)

## 4. 第 2 周：技术验收

由 2 名操作者完成 40 次真实听写，至少一人未参与实现；冻结任务构成及判定口径。

| 任务 | 次数 | 通过条件 |
| --- | ---: | --- |
| A 正例 | 12 | 至少 8 条原稿实际含合格 alias；所有合格 alias 正确替换，并有独立终态。 |
| B 异作用域 | 8 | 至少 4 条实际暴露 alias，误应用 0；B 登记同词对明确报冲突，A 整行不变。 |
| 原稿已正确、大小写、引号与代码边界 | 8 | 新增误改 0。 |
| 忘记与重启 | 12 | 忘记后及随后重启后均实际遇到 alias，均不得再应用，复活 0。 |

另在观察状态完成至少 5 次真实 IME 组字、5 次切焦；中间态及错字段学习写入均为 0。单报实际纠正、合格机会、捕获、正确、未知的计数与对应分母。捕获召回率为 `eligible_captured_corrections / eligible_corrections`，捕获精确率为 `correct_captures / captured_corrections`；证据未知时相应比率不可用，超时和读不回保留在总数，不能算成功。

技术批使用脚本在新隔离目录中从固定 commit 构建并签名的同一份 arm64 pre-C1 Test bundle；冻结其 build-owner receipt、SSH 签名、整包摘要及受信公钥指纹，seal 与 analyze 均用同一份独立固定的公钥复核。`initial_state_sha256` 固定 active/effective 状态；每个 snapshot 另保留 full `state_fingerprint_sha256`、`active_state_fingerprint_sha256`、`global_dictionary_sha256`、`rule_inventory_sha256`、`active_rule_inventory_sha256`、`inactive_candidate_inventory_sha256` 与 `tombstone_inventory_sha256`。只有 A/B 被动观察任务可发生 pending-only 差异；需由同一 owner audit 的精确 `pair_ref` 写入证明，且 status/reason 为 `completed` / `candidate_db_returned`，active/dictionary/tombstone projection 和其他 state 均须不变。完整 canonical hash 投影见[真人试点工具说明](../../scripts/research/dictionary-learning-pilot.md)。观察计数按任务和 runtime+observer epoch 去重：`no_grant`、timeout、unreadable、无合格候选及无写入的 superseded/stale activity 是漏捕；不支持的多 token 结果或证据缺失是 `unknown`；独立标签绑定 owner receipt hash、capture ID 与准确 candidate refs，多候选捕获仍计捕获但正确性记未知，误捕留在全部捕获分母。

B 冲突探测可在普通 bounded-audit before/after delta 中留下一个 closed `review_action:add_manual_scoped_pair`：`source=dictionary_command`、`history_reference=not_provided`、`outcome=failed`、`candidate_store={status:failed,reason:store_error}`、`settings_store={status:not_attempted,reason:null}`、无 `pair_ref` 且 `observer_epoch=null`。该失败命令本身不写规则，必须保留 A 的规则行、active/tombstone 和 dictionary projection；若同一 A/B 被动观察任务另有 pending-only 写入，只能按上文精确绑定至观察候选，不得归因于该 `review_action`。独立 B label 单独记录 `b_conflict_probe_result: manual_pair_scope_conflict`，并须与 exact command response、`b_conflict_probe=yes`、`explicit_conflict=yes` 和 `a_rule_unchanged=yes` 对齐。只有 owner observer fact 是 `bounded_audit` 且 `human_acceptance_authority=true`、review action 验证通过、label 的 owner hash/capture ID 同时绑定 owner/source receipt 且 recording distinct，才计为 verified probe。`store_error` 本身是通用持久化错误，不能当作冲突证明。

漏捕不阻塞显式 Teach；越权、错字段或状态污染阻塞。交付失败、独立终态缺失或真实机会不足均不得凑样本、替换失败记录或补造原稿；停止并记录失败或不足，按冻结方案另开批，原批结果保留。满足全部条件才通过技术门槛，不据此宣称用户收益。

## 5. 技术通过后的真实收益试点

招募 4–6 人，每人 40 条新录音：20 条目标词任务、10 条安全负例、10 条普通听写。目标词任务中 Teach 与对照均须有任务；负例至少含 3 条 CJK 同音、2 条拉丁普通词大小写。CJK 作为不得误改的反例，不因此开启同音衍生。

收益批沿用技术批已验证的独立 build-owner receipt、签名和同一受信公钥，并复用 ASR model ID/revision；`allocate-benefit`、`seal` 和 benefit `analyze` 都要重新核验技术报告及其上游 manifest、evidence 和 artifact root。收益批冻结每人的 full state fingerprint，所有任务前后 snapshot 均须匹配，不允许技术观察阶段的 pending-only 例外。技术报告或上游证据改变后先重跑技术分析并另建收益草稿，不复用旧 gate。

按人匹配词族后随机分配 Teach 或对照，同词族保持分组；每人仅一份冻结规则状态，只批准 Teach 词族，禁止被动学习污染，并沿用本机隔离条件。仅同一份固定 ASR 原稿可作机制配对；真人结果披露词族差异，不称为相同原稿的真人配对实验。

主指标为目标词任务中独立确认正确、实际交付且无需手改的比例，各组全部已分配目标词任务均进分母；失败、未知、弃权和读不回不剔除。另报告全部 40 条及普通听写结果。

继续门槛须同时满足：

- 全部分配任务的终态证据完整度 ≥95%。
- 目标词主指标中 Teach 比对照提升 ≥15 个百分点；至少 N−1 人该差值 >0，N 为入组人数。
- 各人净节省时间的中位数 >0：按冻结的词族匹配口径对齐实测完成时间，扣除 Teach、重教及撤销成本；已计入完成时间的成本不重复扣。
- 关键误改、越域及越权均为 0。

未满足则记录未通过，不更改分母或事后换门槛。以上只允许扩大试验；正式发布仍需独立留出和更大样本。

## 6. C1 触发与后续顺序

首轮不迁移 schema。产品需要 A/B 各自持有同词对、发现跨 scope 的计数/撤销/覆盖串扰，或准备扩大自动生效时，先完成 C1，再开放相关行为。

C1 独立验证 scope＋词面＋匹配策略身份、重叠规则冲突、大小写、人工所有权、旧 tombstone、重启、mapping 变更及部分失败。术语出口授权另行验证，迁移不能代替出口控制。

词对确认有收益但捕获不足时，先改显式 Teach 与最高漏捕路径；错误主要由 cleanup 引入时，另做 prompt 消融。自动 intensity 最后评估，不混入首轮归因。

### 6.1 后训练与 Harness 的优化顺序

1. **先验证标签和规则。** 借鉴后训练的同输入偏好对。对同一份有授权的原稿，用独立参考标注「保留」和「替换」哪项符合原意，加入合法同音名、改主意、宿主改写等反例。运行时证据相同且无法区分的样本标注为应放弃。按参与者、会话和词族冻结留出，检查减少误晋升所损失的覆盖。用户日常 diff 保持为弱信号。 [InstructGPT](https://arxiv.org/abs/2203.02155) · [DPO](https://arxiv.org/abs/2305.18290) · [可拒绝的分类与覆盖](https://arxiv.org/abs/1705.08500)
2. **用 Harness 决定优化哪一层。** 先跑固定原稿的替换开关、跨 scope、反例和撤销。再跑新录音与独立目标终态，逐层判断瓶颈在交付、观察、ASR、cleanup 还是规则。若之后评估术语提示，用同 scope 相关术语和相近干扰词做对照，同时测专名收益和原本正确词的误改。 [Anthropic 的 Harness 评估](https://www.anthropic.com/research/demystifying-evals-for-ai-agents) · [Interspeech 的 ASR 术语筛选研究](https://www.isca-archive.org/interspeech_2025/hou25_interspeech.html)
3. **满足证据条件再比较轻量模型。** 首轮改字判断沿用本机有界观察及确定性规则，资源增量按第 2.1 节测量。若高价值漏捕主要来自规则处理不了、但现有可见证据确实能判定的案例，再在隔离离线评估中比较轻量分类器与规则基线。候选要在同一留出集提高可判定覆盖，同时守住关键误改和资源预算。只有残余错误稳定、标签量足够且有独立留出时，再评估微调。

## 7. 列表与语气偏好：弱证据复核与后续独立验证

本节区分已实现的 History 弱证据候选与用户主动复核流程，以及仍待验证的自动推断学习和独立试验。当前流程只把候选交给用户审核；它不自动改变输出。后续自动推断验证另开状态、任务集和结果报告，不并入词典第 1/2 周或第 5 节收益试点。目标交互是不逐次确认；任何可选自动学习仍须另过本节门槛。

### 7.1 临时意图、长期范围与执行优先级

长期偏好绑定当前用户的隔离域、App、写作任务及偏好维度；来源、授权和版本分别可追溯。项目更新、邮件、聊天是不同任务，不能只凭同一 bundle 或 host 互相泛化。任务只取用户明确选择或现有流程中可验证的任务信息；不为识别任务新增窗口扫描、常驻监听或模型调用。任务未知时不召回新增任务偏好，沿用现有行为与原有显式设置。

一次性语音指令、本次模式、临时模板和明确的单条覆盖只影响本条，不作为长期正证或反证。「这条列成要点」「这次正式一点」不触发持久化。只把现有指令机制识别的明确指令当作本次意图，引用或转述的命令文本不构成授权。若无法区分临时意图与长期偏好，记 `unknown`；不向用户逐次追问，未编辑也不表示同意。

按下列顺序处理，同层冲突无法确定时沿现有安全行为交付，跳过争议偏好，不把冲突交给学习机制猜测：

| 优先级 | 决策 |
| --- | --- |
| 1 | 隐私、路由授权、来源与 literal/layout 保护，以及事实保真约束始终成立。 |
| 2 | 明确的本次语音指令或本次输出模式优先；它们互相冲突且现有解析无法确定时，跳过新增偏好。 |
| 3 | 用户显式保存或批量批准的同 App × 任务 × 维度偏好；保留原有显式禁用与范围限制。 |
| 4 | 将来经过独立验证、具有类别授权的推断偏好，仅填补未被显式设置控制的同域维度。 |
| 5 | 无匹配、任务未知、项目边界不足或不兼容时沿用现有行为。 |

用户主动保存的长期设置无需先积累改字次数。现有设置入口可复用，但 App × 任务的准确表达、保存与回读必须完成相应实现和验收；不把当前 App 级样例宣称为已有任务级支持。

同 diff 对照至少包含以下三组，标签由研究者独立确认，不让 diff 决定真实原因：

| 相同可见变化 | 临时原因 | 长期原因及应有判定 |
| --- | --- | --- |
| 同样的三项从连续段落改为三行 bullet，文字与顺序不变 | 这次要粘进固定周报模板，或本次明确要求列表 | 用户希望「App A × 项目更新」默认列表。仅有 diff 时仍为未知或弱证据，不能推广到邮件、聊天。 |
| 短句开头「你好，」改为「您好，」，其余相同 | 本次写给特殊对象，或临时要求正式 | 用户明确设定该邮件任务的窄礼貌形式。称呼形式改变不能推断收件人身份，当前未支持的转换不得启用。 |
| 同一句末尾删除句号 | 当前聊天模板或宿主格式化造成 | 用户显式选择该聊天任务的句末标点习惯。不能据此提高口语强度，也不能把自动格式化当人工偏好。 |

### 7.2 合格证据、候选门槛与去重

仅使用现有许可覆盖的同字段、有界 paste 后观察；验证插入归属、事务、范围、owner、观察代次及已提交的稳定编辑。沿用既有观察窗和取消条件，不为凑次数延长读取。停顿或稳定读回不等于 IME commit；不能排除 IME、宿主自动格式化、重复粘贴、模板填充或来源歧义时不计正向证据。任务来源不明的编辑不累积为 App 级默认，也不并入任一任务的长期支持；用户可另行明确保存，但不追认为过去已自动识别正确。

对齐同事务 ASR 原稿、本机准备稿、实际采用的 cleanup 结果、实际插入与终稿。先区分用户在恢复处理前文字，还是仅调整表达形式；恢复被 cleanup 改坏的内容首先记对应阶段回归，不能直接变成新风格偏好。同一变化只归入有证据支持的维度。

| 维度 | 可进入草稿的最窄证据 | 不学习的边界 |
| --- | --- | --- |
| 列表结构 | 仅换行、项目符号等结构变化；逐项可对齐，保留原有事项、顺序、事实和限定语。结构所需标记只计列表证据。 | 新增、删去、合并、重排事项；把模糊段落切成无法核对的项目；夹杂措辞或事实改写。 |
| 语气 | 仅限现有实现已支持、能逐项核对的短句称呼或口语表面标记；启用前列出实际支持的允许集合。 | 从一句变化推断抽象的「更专业/更热情」；新增称谓、收件人、态度内容或承诺；支持范围未证。 |
| 标点 | 仅限已支持且语义无歧义的标点差异，作为独立维度记录。 | 数字、小数、日期、否定、缩写、网址、代码、引号或引用边界可能被改变。 |
| 强度 | 只记录与其他维度分开的来源和结果，不由本节弱信号自动改变强度。 | 以语气、标点或列表变化触发原有 3 次 intensity 路径；把大段改写当作纯风格。 |

混合维度且不能唯一归因、夹杂内容增删或改事实的事务，整次不计偏好正反证据。明确的本次覆盖即使只改结构，也排除于长期统计。对两个合法对象、数字、日期、否定、承诺和事实变化不作偏好泛化。

起始候选门槛拟为：**滚动 14 天内，至少 3 笔独立且不同内容的合格听写，跨至少 2 个自然日和 2 个独立会话。** 当前 History 弱证据的手动复核候选使用这组门槛，但这不是已验证的收益阈值，也不会触发自动生效；它也不是当前产品的 3 次强度机制。候选自然日取原始听写记录的 `dictations.created_at` 并由 SQLite `localtime` 计算；History 编辑时间 `occurred_at_ms` 单独计量编辑证据的 14 天窗口；候选读取和复核时，原始听写本身也必须仍处于滚动 14 天内。证据 metadata 在编辑后 14 天或来源满 14 天时较早到达的期限清理，所以编辑第 13 天的来源到第 15 天时不会继续成为候选。

- 第一次合格变化可形成本机弱证据草稿；达到门槛只提升批量复核优先级，不能单靠重复自动生效。
- 按现有 origin 对同一次听写的 History、AX、重复读取和重复粘贴去重；同一事务的多次改字只贡献一笔。
- 模板族用本机粗略形状 SHA-256 仅作保守去重：同一分句里的同脚本短语折叠成一个形状标记，单独改变词面或词数不能制造新家族；数字槽、内部标点和段落边界仍保留宽泛形状，句末标点变化本身不制造差异。该形状可能把语义不同的句子过度合并，不是语义模板识别或独立意图证明；无法生成形状 hash 时不计入多样性，也不把所有 bullet 布局误归成同一模板。
- 14 天窗口约束编辑后证据年龄，且候选读取 / 复核时要求原始听写仍在滚动 14 天内；metadata 的有效期为编辑后 14 天与来源满 14 天两个期限中的较早者。这不要求延长来源正文的 History 保留期，也不扩大当前保存期限、128 项 audit 窗口或授权用途；必要来源元数据不足时保持未知。过期弱证据不参与晋升，显式保存偏好不因未使用而自动失效。
- 反向编辑、未知和被排除事件另计；没有反例或用户没有修改，均不增加正向支持。

### 7.3 无逐次弹窗的保存、状态迁移与反例处理

现有 Context Settings 映射页包含用户主动展开后才读取的候选复核区，不弹窗、不自动跳转。每项按稳定 `mapping_id` 联结当前 AppMapping，并显示 App 选择器范围、任务、维度、预测值、聚合来源 / 日期 / 会话数和固定 `has_text_sample=false`；候选 IPC 不提供原文片段或样例。用户可混合选择后批量批准或忽略，每批最多 64 项；列表更多时，处理当前批次后可继续选择剩余候选。每项返回独立结果，批准以 mapping revision 乐观校验。同一个 App 名称和 task 可对应多个规则，UI 通过 mapping ID 选出当前范围；当前规则范围无法联结时，该候选不可选择。候选或 mapping 已过期、授权撤销、已有偏好冲突和写入失败均按项展示；mapping 的最新 History source 重新检查遇到 I/O 错误时该组不写 Settings、返回 `source_recheck_failed`，同批其他 mapping 组可继续；若来源已变化或过期则返回 `stale_candidate`。UI 要求刷新候选后才允许重试；整条 IPC 结果不确定时也必须先刷新。候选资格按当前 14 天窗口计算：同一有效 mapping selector × task × dimension scope 的未忽略反向值会阻止候选就绪，即使位于不同 generation；忽略只丢弃对应 generation，该 generation 的证据不再支持或阻塞后续候选。关闭复核区不代表批准或忽略；忽略也不忘记已有设置或建立 Forgotten tombstone。

显式结构化偏好归属于 AppMapping 声明的目标身份。相同 mapping ID 下更改 family、bundle、executable、browser host/path 或 focused field 时，显式保存会清除该 mapping 所有 task 的偏好（含 forgotten tombstone）并撤销结构化偏好云端批准；同一次保存随带的旧偏好也不会迁入新范围。用户可在范围保存后重新录入手动偏好，或由新范围的有效 History 候选复核后批准。Host 与 path 按规范化值比较，等价大小写 / 尾斜线拼写不会重置范围。只切换当前选中的 writing task 不清除其他 task 的偏好或 tombstone。通用 Settings patch 不能写入 context mappings；显式保存依赖当前 mapping preference revision，旧设置视图不能恢复已清除的偏好。

批准写入本机 AppMapping 结构化偏好，不创建样例授权或云端授权。批准新的短称呼或句末标点偏好会撤销该 mapping 的结构化偏好 provider grant；现有 style-example grant 独立保留。批准后 UI 重新读取 mappings；当用户仍编辑同一 mapping / task 且该偏好和 provider grant 草稿未改变时，只合并批准项并保留其他草稿。范围、task、同一维度或授权草稿发生冲突时，UI 保留草稿、阻止保存并提供显式重新加载获批规则的操作。关闭全局词典学习、本机改动观察或该 App 的学习开关会清除未批准证据及 decision markers；关闭全局上下文、禁用 mapping 或撤销需要的浏览器访问只暂时阻止收集 / 复核，不清除证据。删除 mapping 会清除其候选元数据。Clear All 清除弱证据和 decision markers，但保留用户已批准的 Settings 偏好。

前端定向验证命令：`npx vitest run src/components/WritingPreferenceReview.test.tsx src/components/SettingsLayout.test.tsx src/components/ContextSettings.test.tsx`，以及 `npm run lint`。最新 WritingPreferenceReview 定向回归为 14/14，`npm run lint` 通过。Backend owner-chain regression 已落地并通过 1/1：`cargo test --manifest-path src-tauri/Cargo.toml --lib writing_preference_drafts::tests::history_edit_candidate_review_owner_chain_persists_and_applies -- --exact`。该测试在隔离 History DB 中使用合成文本和 editor-commit fixture，调用实际 History revision、classifier/evidence writer、候选派生和 review owner；先批准一个候选，再批量批准同 mapping 其他维度及另一 mapping 的候选，检查 decision marker、Settings reload 和兼容的本机 bullet projection。它不运行真实 History UI / Tauri IPC / Test bundle，也不证明 macOS Host、真实 IME 或真人改字结果。

History source 只接受 VoiceFlow 自有 History 编辑器对原始交付文本进行的首次可信手动 Save，且原始交付后尚无任何 History revision；先前的 `ai_reclean`、恢复或其他 action revision 均使该来源不合格。任务和 mapping identity / revision 还必须可验证，IME / 输入类别满足 classifier 边界；同一听写后续保存不增加独立样本。该门槛只约束写作偏好弱证据；词对 History / live 生产弱证据仍只形成待确认候选，明确确认后仅在记录 scope 本机生效，2/3 词对阈值不用于生产晋升。单次 Verbatim 绕过会被排除，但全局关闭整理后仍经本机 `PreparedTranscript` 路径的记录可以进入分类。AX / paste observation 不提供 author / IME proof。弱证据只产生待人工复核 metadata candidate，不自动改变下一次输出。以下状态表保留候选与未来自动启用的边界；自动 promotion / inference-to-active 仍需独立 shadow、holdout 和收益验收，不由当前 UI 批量批准流程代替。

| 状态或事件 | 允许的迁移与行为 |
| --- | --- |
| 合格弱证据 → 草稿 | 仅本机记录，保存有界变化、scope、维度、origin、证据缺口；不进入生成、提示或自动设置。 |
| 草稿达到候选门槛 | 进入待复核候选，不改变下一次输出；高歧义可静默放弃。 |
| 用户直接保存 / 一次批量批准 | 成为显式偏好；保存明确范围和来源后，才在下一次同域兼容场景应用。直接保存不受 3 笔门槛约束。 |
| 本次明确覆盖 | 仅本条跳过或替代对应偏好，长期状态不变，也不记正反证据。 |
| 用户暂停 | 停止该偏好应用，保留来源供主动恢复；不因更多重复证据自行恢复。 |
| 已应用的推断规则遭可靠反例 | 记录实际触发版本及 span，先核对提交、宿主和本次意图；高可信反例使该版本待复核并暂停，不生成反向规则。 |
| 用户撤销 / 忘记 | 撤销本次输出与忘记长期偏好分别处理；忘记后停止召回该项及其自有示例，保留阻止重建的撤销语义，重试/重启不得复活。其他来源独立持有的数据不被误删。 |

显式偏好不会因一次明确的临时覆盖被改掉。对没有临时覆盖说明、但可能改变了意图的逆向编辑，记录疑点，不静默覆写显式设置。事实或授权保护失败仍阻止本次违规处理；保护失败不等于授权系统学习一个相反的默认值。

**可选自动学习仅在后续讨论。** 用户须一次明确授权具体类别及范围；晋升策略、匹配范围、冲突回退与撤销机制先经独立留出验证，具体草稿再满足冻结的候选门槛、来源和授权条件，才可评估转为单独标记的「推断启用」。存在竞争候选、人工设置冲突或 tombstone 时保持草稿或放弃；已启用规则在应用时遇到冲突则回到既有行为。起始范围只限有来源边界的列表布局，或已支持的无歧义表面标记；语气、标点、强度不得打包授权或相互代推。该授权不扩大读字范围，不授权云端出口，也不代表任意改写。

若该路径复用共享的 scope、持久化或自动生效设施，复用并补齐 C1 的原因是：同一 App 下不同任务/维度必须能独立计数、覆盖、撤销和重试，不能重现仅按词面或 App 识别条目的冲突。需核对用户隔离域、App × 任务 × 维度、来源/版本、重叠规则、人工所有权和 tombstone。不能用自由文本词对字段冒充偏好身份。词典 C1 通过不自动证明偏好通过；该要求不提前触发词典首两周迁移，也不改变第 6 节原触发条件。

### 7.4 下一次应用、云端出口与实际效果取证

下一次听写只读取当时有效的已批准设置或将来获准的推断版本。候选仍是惰性数据。先处理本次意图与授权，再判断 App、任务、维度与原话是否兼容；同 scope 命中但内容不兼容时不应用，不把跳过算成用户反对该偏好。

列表优先复用现有 `spoken_layout` 和有来源的本地布局。当前原话必须有可证项目边界，且事项可逐项对齐；历史上把一段话改成列表，不能为新一句话提供不存在的边界。不得凭「喜欢列表」增项、漏项、重排、总结或合并。无边界时沿既有行为，不额外调用模型。若基线本已生成相同列表，该次是无新增效果，不计新增收益。

语气和标点只选择当前路线支持的有界写作指引或现有 cleanup 模板。只替换所测试的指导维度，固定强度和其他模板因素；不在线修改基础 prompt 或模型权重，不把偏好加入 guard 的内容改写豁免。cleanup Off 或本机规则路径不能表达该指导时，本条跳过；不能借偏好自动升级强度、加载第二模型或切换到云端。

| 数据类型 | 本机使用 | 云端出口 |
| --- | --- | --- |
| 已批准的纯结构化偏好，如「该任务使用 bullets」 | 按同域兼容条件作用于已有布局或模板选择。 | 只有已选云 cleanup 路线且授权覆盖本次写作指引时，才提供最小所需指导；不附候选历史、证据计数或额外 App 内容。 |
| 含原文的已批准样例 | 仅按批准范围作为参考，保留来源和忘记关系。 | 保存/批准本机样例不等于同意向云端发送；须有覆盖该内容、用途及所选路线的明确授权，否则省略样例。 |
| 未批准候选、编辑 diff、未知归因和原始观察历史 | 仅在已有许可及保留边界内用于本机候选与受控验收。 | 不因偏好学习进入模型请求，不把其摘录伪装成结构化指引。 |
| 音频 | 仅按已有听写和获准研究用途处理。 | 本次主动听写的 ASR 路由独立适用；偏好学习不触发音频重放、再次上传或第二次模型分析。 |

严格离线下，所有应用和取证留在受支持的本机路线。结构偏好授权不授权术语进入 ASR hint；样例出口与第 2 节术语出口分别验证。复用序列化检查和本机测试收集器，用合成标记检查有/无样例出口授权、不同 scope、严格离线及忘记后的请求；本地试验不发送真实学习语料。

偏好取证复用既有审计、研究 owner 回执及独立目标读回；缺字段记未知，不能宣称当前接口已经实现新增事件。分别记录：

| 事实 | 判断依据 |
| --- | --- |
| 配置存在 | 该版本已保存，标明用户、App、任务、维度与显式/推断来源；不代表本次使用。 |
| 有效命中 | scope、任务、模式、授权和来源边界均满足；不代表输出发生变化。 |
| 准备稿或 cleanup 实际改变 | 能定位新增布局/表面标记的来源，或指引进入实际请求；指引被发送不证明模型采用。 |
| 实际插入 | 最终保护后的目标字段确实包含该变化；发键、内部输出或 usage 不能替代。 |
| 提交后被改回 | 关联实际应用版本、原插入范围和可靠终态；再区分本次覆盖、内容修改、宿主行为及可用反例。 |
| 正确且有收益 | 由独立偏好/任务标签、目标终态和对照判定；未编辑、命中和计数增加不单独构成收益。 |

资源测量沿用第 2.1 节。新增确定性候选筛选仅在有界 paste 后运行，不扫描历史全库或扩展为常驻观察；分别测观察耗时、AX 次数、CPU 和请求长度增量。模型路线、冷加载与强度不作为偏好实验的伴随变化。

### 7.5 独立 shadow、holdout 与用户收益阶段

偏好各维度单独试验，列表优先，其次有明确允许集合的表面语气/标点，自动强度最后。下面的「偏好首轮」指本节后续独立评估，不指第 1/2 周词典技术验收。人数、样本量、任务构成、效益与资源非劣门槛须在查看正式结果前预注册；不直接借用第 5 节的词典人数或提升门槛。

| 阶段 | 冻结与取证 | 通过含义 |
| --- | --- | --- |
| Shadow | 使用独立状态，只生成本机草稿，不改变交付；预先标注临时意图、长期任务偏好、兼容性及不可判定情况，统计误捕与漏捕。 | 只验证证据筛选，不声称已改善输出；无法证明状态隔离则阻塞。 |
| 独立 holdout | 冻结构建、路由、模型、指引/模板、强度、规则状态、候选阈值和标签口径。同一固定 ASR 原稿只切待测偏好，验证来源、事实、scope、回退和撤销。 | 仅支持机制与门控结论。改阈值或模板后原留出成为开发数据，需新留出。 |
| 实际用户收益 | 冻结已批准偏好与基线；同人同任务内预先匹配内容/模板族并随机分配，真人使用新录音，两组均禁止继续学习写入。 | 比较可用终稿、修订及净耗时；人工批准路径通过不等于可选自动学习已通过。 |

#### 执行路径与当前可运行证据

所有正式任务先使用独立 Test bundle/AppData 和仓库外私有证据目录；按[隔离预检与状态指纹流程](../end-to-end-workflows.md#week-one-dictionary-learning-preflight-and-isolated-verification)冻结 build、权限、Settings、SQLite 与有效 hint。任务清单、盲化标签和目标参考在看系统结果前冻结；通常只保存不含正文的 task ID、类别、文件 SHA-256、状态摘要、独立终态和诊断引用。音频、转录、阶段文本仅在任务明确授权且本机留存时取证，不进入 Git 或请求载荷。资源和内容无关时延取证沿用 §2.1 的 Settings 摘要、`dictation-performance.jsonl` /（本机 MLX cleanup 时）`mlx-cleanup-performance.jsonl`，不为偏好试验增加常驻收集器。

当前 checkout 包含 C1 与显式偏好设置 / 应用。§7 首个 runtime 切片把录音时保存的 task、mapping identity / preference revision、intent source、output mode 和实际准备 / cleanup 路径写入本机 History metadata；只有原始交付 History 文本的首次手动编辑、原始交付后尚无任何 History revision（任何先前 `ai_reclean`、恢复或其他 action revision 都会排除该来源）、VoiceFlow 自有 textarea 的明确 Save、可信输入类别 / IME 结束、成功 paste 或 History-only 交付、当前 source / mapping / permission 再校验以及纯格式 diff 全部通过时，才写入不带原文或摘录的弱候选证据；词对 History / live 生产弱证据仍只形成待确认候选，明确确认后仅在记录 scope 本机生效，2/3 词对阈值不用于生产晋升。外部 AX 不作为作者或 IME 证明。有限分类器只接受 source-backed bullet 投影、`你好/您好` 开头称呼和无歧义单句句末句号；不捕获 one-shot intent、snippet、非 Auto mode、恢复原文、复制 / 未确认 / 失败交付、受保护字段或 task / provenance 缺失。滚动 14 天内按 origin、ASR 内容 hash、粗略文本形状模板 hash、local day 和 History 页面会话去重；每分句内的同脚本短语折叠为单个形状标记，词面 / 词数变化不能独自造出不同模板家族；标点、数字槽和段落边界仍保留宽泛形状，句末标点变化本身不增加家族。该 hash 是可能过度合并的保守去重线索，不是语义模板或独立意图证明；形状无法可靠计算时不满足模板多样性。达到 3 个 origin/content/template families、2 天和 2 个 History session 只进入一次候选复核，不自动启用。显式偏好列表结构仍只走本机输出投影，不进入云端指引；结构化语气 / 标点保留原有独立出口授权。

命名的 synthetic owner-chain regression 已通过 1/1（见 §7.3）：它从隔离 History DB 的合成事务 / revision 和 editor-commit fixture 开始，贯通实际 classifier、metadata evidence store、candidate derivation、单项和跨 mapping 批量 review、持久 Settings reload 与兼容列表投影。当前没有真实 Tauri IPC / Test bundle / macOS runtime 验收、独立 shadow / holdout 或真人收益结果；History 保存不能证明外部宿主最终采纳文本，自动推断启用仍关闭。

本节 weak-evidence owner 链的合成隔离 History DB 检查已运行，但尚未执行隔离 Test AppData 中真实 App 的 History → 候选 → 复核链；也没有独立长期偏好标签。独立 shadow、冻结 holdout 与用户收益仍标为 `not_run`；真人收益保持 `pending`（当前无参与者），自动启用仍关闭。可复用但范围有限的隔离 owner 检查是[确定性文本 V3 夹具](../end-to-end-workflows.md#versioned-multi-scope-phase-fixture-v3)：在新的私有 `--arm-root` 下运行 `learning-v3-multiscope.json`、`learning-v3-pending-apply.json` 和 `learning-v3-postundo-multiscope.json`，验证现有词对 owner 的 scope、pending、usage 和 tombstone 事实。它只用程序化文本；不是偏好 shadow、真实 ASR、独立 holdout 标签或用户收益证据。`dictionary-learning-pilot.py` 只分析本计划 §4–5 的词典批次，不用于 §7.5。Test bundle 原生验收和 §2.1 的实机资源测量也仍待执行；这些状态均不改变原冻结门槛，也不能报告为通过。

研究标签在看系统判定前确定：分别记录该用户对指定 App × 任务的长期偏好、当次临时要求及期望保留的内容；获授权时用音频和阶段文本核对。独立终稿只证明最终采用内容，不能倒推出长期偏好。相同可见证据但意图不同的样本应标不可判定，不给模型构造互相矛盾的强标签。

调参资料与留出按参与者、会话和模板族分隔。个性化评估允许使用每人的前置显式示教，但未来录音、终稿和标签不得回流。分别验证跨用户隔离、同 App 不同任务的范围边界，以及新模板的兼容泛化；不能因模板需要特殊格式而推成整个任务的默认。现有 test-only 归因 judge 的通过记录不覆盖这些新增偏好标签。

最低负例集合必须覆盖：

| 负例 | 预期结果 |
| --- | --- |
| 本次要求列表/正式一点，下一次恢复普通表达 | 只影响当前任务，不累计长期正反证据。 |
| 同 App 的项目更新、邮件、聊天与任务未知 | 不跨任务召回；未知时沿现有行为。 |
| 同一项目模板连续改 3 次，或换日期后重复粘贴 | 不满足独立内容证据门槛，不误计不同来源。 |
| IME 停顿、宿主自动项目符号、自动标点、切焦 | 无可靠提交/作者归属则放弃，不写成用户偏好。 |
| 改列表同时换日期、数字、对象、否定或承诺 | 整次排除偏好学习，事实保护独立成立。 |
| 用户偏好列表，但新原话没有项目边界 | 跳过偏好，不虚构项目、总结或拆项。 |
| 多维偏好冲突、本次明确覆盖显式默认 | 跳过冲突维度或执行本次明确意图，长期设置不变。 |
| 用户 A 的示例进入 B，任务 A 规则进入 B，模板标签泄漏到留出 | 隔离失败，停止该批；不得用高平均分抵消。 |
| 无云出口授权、严格离线、忘记后重启或部分同步重试 | 不发送受限示例，不复活已忘记偏好，不污染词典。 |

分母在任务分配前定义，不能从系统命中或成功读回反推：

- **观察精度/覆盖：**实际编辑总数、独立确认的合格机会、捕获数、正确归因数、未知数分别报告；超时、未启动和读不回保留。合格集合由独立标签确定，分母为 0 时不可计算。
- **主要效益：**在预先标为目标 App × 任务兼容的全部分配任务中，独立满足预定偏好、内容保真、实际交付且无需手改的比例。未知和失败不进成功分子，不从分母删除；另报包含不兼容与安全负例的全部任务结果。
- **纠正与时间：**报告需要主动纠正的任务比例、修订量，以及停说到独立确认可用终稿的实测时间。配置、批量复核、重教、暂停和撤销成本计入净收益，同一时间只扣一次；文本差异不冒充实际按键数。
- **打断：**逐次主动确认弹窗为 0；主动打开设置的次数和耗时另报，不能把用户未看草稿算作接受。
- **伤害：**新增误格式、误语气、项目增删/重排、事实破坏、越域、越权、污染及复活分别计数，附实际暴露机会和未知数，不只报告已应用规则的准确率。

出现可归因的误格式或误语气，若违反已冻结的明确任务要求，暂停对应偏好版本/分支，保留该批失败记录；细微偏好分歧且无独立标签时记未知，不擅自判成功。出现任一新增事实破坏、项目虚构/丢失、跨用户/任务应用、越权外发、状态污染或忘记后复活，立即停止相关试验批次。当前输入仍可按既有安全路径处理，不为学习反复打断用户。

主动逐次确认、标签或终态不足、兼容机会不足及无法隔离其他变量，均不能宣称该阶段通过；不得删除失败任务补足配额。修复后重新冻结并另开批。只有预注册的独立质量、净收益及资源门槛同时通过，才可扩大该偏好试验；不据此承诺发布，也不替代词典 C1 或词典收益验收。
