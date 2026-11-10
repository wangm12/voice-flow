# VoiceFlow 本地模型扩充计划

2026-11-09。本文定义接入和验收顺序。Parakeet V3 是第一款拟发布的新 ASR。本机整理按模型分别接入。通过自身交付验收的模型直接开放下载和选择，用户无需安装 Ollama。本文列出的硬件档和数值是拟定验收条件，当前均待实测。

## 1. 给用户提供哪些模型

| 用途 | 模型 | 本轮决策 |
| --- | --- | --- |
| 本机转写 | Qwen3-ASR 0.6B BF16 | 现有默认和基线。用于中文及中英混合听写。 |
| 本机转写 | NVIDIA Parakeet TDT 0.6B v3，MLX 格式 | 第一款计划新增的一键下载转写模型。面向英语及模型覆盖的欧洲语言。先验参考权重，再择一验证通过的发布权重。 |
| 本机文字整理 | VoiceFlow MLX Qwen 3.8 27B 6-bit | 保留现有模型、下载与配置。 |
| 本机文字整理 | Qwen3.5 2B、Qwen3.5 4B、Gemma 4 E4B IT | 首批分别接入通用整理，验收后逐款开放一键下载。 |
| 本机文字整理 | Ministral 3 3B Instruct、Gemma 4 E2B IT、Qwen3.5 9B | 后续分别接入。9B 按实测兼容的内存档开放。 |
| 本机文字整理（纯英文） | Superwhisper S1-mini | 单独提供英文整理，使用模型原生控制格式，注明支持的模式。 |
| 本机标点 | FunASR `ct-punc` | 独立提供中英标点处理，放在「仅标点」选项。 |
| 本机转写研究 | Qwen3-ASR 1.7B、FireRedASR2-AED MLX 与 Fun-ASR-Nano-2512 | 与现有 0.6B 在同一批中文和中英混合音频上比较。按结果选择是否发布高质量中文选项。 |

Qwen3-ASR 1.7B 的研究跑次使用隔离环境，产品仍执行现有内存准入。Cohere、Whisper、Parakeet V2 作为后续对照，SenseVoice 仍是旧文件管理项。现有评测器的 `vf14-qwen3-4b-instruct-2507-untrained` 是 Qwen3 4B Instruct 2507，不能用它的产物或结果替代本次 Qwen3.5 4B。 [ASR 目录](../src-tauri/src/ondevice_models.rs) · [内存准入](../src-tauri/src/model_resources.rs) · [现有整理模型](../src-tauri/src/modelstore.rs) · [Ollama 制品](https://ollama.com/library/qwen3.5:4b) · [现有研究候选](../src-tauri/src/evaluation/local_cleanup.rs)

### G0. 冻结机型、数据与判定规则

| 机器档 | 测什么 | 拟定绝对门槛 |
| --- | --- | --- |
| Apple M1、16 GiB、macOS 14 最新补丁 | 最低拟支持档。Parakeet 短长音频。每个拟覆盖该档的整理模型分别单跑，以及分别搭配驻留的 Qwen 0.6B、Parakeet。 | ASR 热态文件推理 RTF p95 ≤ 0.30，1–10 秒听写停说到写入 p95 ≤ 1.2 秒，冷启动首个完整结果最大值 ≤ 8 秒，ASR 进程族峰值 ≤ 4 GiB。轻量整理热态 p95 ≤ 1.5 秒，冷启动首个完整结果最大值 ≤ 10 秒，ASR＋单个轻量整理候选的进程族峰值 ≤ 9 GiB。 |
| Apple M4 Max、64 GiB、macOS 27.0.1 | 在同机对比参考与量化 Parakeet。逐款验证整理模型及各自与 ASR 叠加的资源用量。 | 标为轻量的模型须满足相同延迟和内存上限。27B 执行现有冷加载预算及全机压力策略。 |
| Intel 或低于 macOS 14 | 核对设置页和后端准入。 | Parakeet MLX 不可下载、选择或加载。每款整理模型的最低平台取其最终制品实际验过的范围。 |

先记录实机型号、系统 build、是否可用、后台负载上限、每组 WAV 和文本清单 SHA、报告版本及模型身份。内存组合测量要求无持续黄色或红色内存压力，无新增长时间 swap 活动，且通过现有模型资源准入。未取得低档实机时保留该档待验收。延迟门槛覆盖固定时长和长度分布的样例，ASR 文件组包括 1–10 秒及 30–120 秒，Cleanup 的 p95 来自 B3 冻结的长度分布。冷启动从请求触发且目标进程尚未启动开始。阈值只在调试集阶段修订并记录理由，锁定后才打开独立验收集。ASR 继续执行其独立筛选条件。Cleanup 按每款制品及所声明的支持档做验收。

## 2. Parakeet V3 一键下载

### A0. 确定权重和本地推理方式

- 参考 Handy 的模型分工和下载体验。Handy 的 Parakeet V3 约 478 MB，MLX 社区的参考仓库约 2.51 GB，属于两个不同制品。先用 `mlx-community/parakeet-tdt-0.6b-v3` 验证 Swift 推理，再对比一个可追溯的 8-bit 候选，例如 `transcriber-app/parakeet-tdt-0.6b-v3-mlx-8bit`。发布只选一个制品。磁盘大小、加载峰值和推理用时分别测量。制品选择统一执行下方规则。 [Handy](https://handy.computer/docs/models#parakeet) · [参考权重](https://huggingface.co/mlx-community/parakeet-tdt-0.6b-v3/tree/main) · [8-bit 候选](https://huggingface.co/transcriber-app/parakeet-tdt-0.6b-v3-mlx-8bit)
- 复用 `Package.swift` 已锁定的 MLX Audio Swift。SDK 提供 `ParakeetModel.fromDirectory`，由 VoiceFlow 传入已校验的模型目录。下载由现有 Rust 下载器在用户点击后执行，Swift 运行时只读本地文件。验证 tokenizer、配置和权重全部可从该目录加载，并核对实际加载的参数精度，避免把磁盘量化标记当成运行内存结论。 [Swift 依赖](../src-tauri/mlx-sidecar/Package.swift) · [已锁定的加载实现](https://raw.githubusercontent.com/Blaizzy/mlx-audio-swift/01dec7c9bdce3088a6b6b7ab9f2e403458195efb/Sources/MLXAudioSTT/Models/Parakeet/ParakeetModel.swift)
- 给最终制品固定完整仓库 revision、必要文件清单、每个文件的字节数及 SHA-256。确认原模型、MLX 转换和量化产物的来源、CC BY 4.0 归属及修改说明。记录发布包所需的许可文本和 NOTICE，清单通过后才开放下载。 [NVIDIA 原模型](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) · [仓库归属说明](../THIRD_PARTY_NOTICES.md)

**A0 先选权重，A1 再接下载。** A0 通过隔离 WAV 路线在 G0 机型上对比参考版、8-bit 和 Qwen 0.6B。A0 选型需要两档机器的冷启动、热态 RTF 和内存实测，M1 数据缺失时可在高档做探索，但不能选定本轮发布制品。隔离阶段没有等价桌面入口时，先要求 1–10 秒热态文件推理耗时 p95 ≤ 1.2 秒。A2 再独立实测停说到写入，文件计时不抵扣桌面门槛。每种首发语言准备两个互斥数据集，筛选集至少 10 分钟且 1,000 个参考词，独立验收集至少 20 分钟且 2,000 个参考词。英语、西班牙语和德语是首批拟测语言。用相同样例和归一化规则计算各语言聚合 WER。候选须先过许可和来源核验、A0 可测的 G0 门槛及下述价值门槛。8-bit 还须相对参考版每个语种的 WER 劣化 ≤ 1 个百分点，并至少减少 25% 下载字节。两者都合格时选 8-bit，只有一款合格时选该款，都不合格时暂停产品接入。

价值门槛为相对 Qwen 0.6B 至少在一个拟发布语种降低 10% 相对 WER，或在 WER 劣化 ≤ 1 个百分点的前提下，ASR 热态 RTF p95 至少改善 25%。其余拟发布语种的 WER 劣化均不得超过 1 个百分点。任何候选出现整段漏听、长音频缺尾或未处理失败，先修复并重新验证。选择完成后才冻结发布 revision、文件 SHA 和下载大小。A2 用独立数据复核同一选择。失败回到 A0，打开过的数据移入诊断集，下一次发布验收重新封存独立数据。

### A1. 贯通下载、选择与运行

1. **Rust 模型目录。** 在 `ondevice_models.rs` 为同一个 Parakeet V3 制品建立稳定 ID（拟用 `parakeet-tdt-0.6b-v3`）、模型清单和语言能力。`ondevice_download.rs` 复用现有临时下载、进度、取消、验证后安装及删除流程。对磁盘不足、损坏、重复点击和取消后重试逐项验证。保持已装模型与正在下载的不同状态。
2. **Swift 推理。** 在 `SidecarServer.swift` 加明确的模型分支，复用模型目录校验、请求 ID、16 kHz mono WAV、工作进程和现有响应契约。首版使用现有分段流程。决定由哪一层切片并在 decoder recipe 固定策略，避免外层分段再触发 SDK 重叠分段，造成重复词或缺尾。测试音频尾段和取消边界。若 SDK 单段调用无法及时取消，按现有终止子进程契约验证上限，直到实际进程退出才报告已取消。协议改动如有需要，应保证旧 sidecar 缺少该能力时能被明确识别。
3. **语言。** 首发使用 `auto`，UI 只将通过 A2 的语言列为已验证。锁定 SDK 的 `STTOutput.language` 取自请求参数，因此不把回显写成模型检测结果。若未来能证明语言参数实际约束解码，再开放显式选项。现有显式语言与 `auto` 冲突时提示用户切换设置或保留 Qwen，下载与选择均不自动改语言。验证中英混说的边界行为，严格离线时不回退到云端。
4. **内存。** 在现有 `model_resources.rs` 按模型 ID 与 revision 准入。先测冷加载及长音频峰值，再填生产预算。额外测 Qwen 与 Parakeet 切换，以及 Parakeet 转写后启动 27B 整理时的两进程叠加峰值、压力和回收。拒绝加载、取消、子进程退出后都要验证预算释放。
5. **设置页。** 「本地模型 → 本机转写」增加一行 `Parakeet V3`，副标题写适用语言、实际下载大小和状态，点击「下载模型」即启动托管下载。下载成功后在「服务配置 → 转写 → On Device」选用并保存。下载动作本身不改当前服务，默认仍为 Qwen。已有引导页若列出同类模型，应复用同一可选条件。严格离线模式下运行全程在本机，模型下载仍是显式联网动作。

接入点见 [Rust 模型目录](../src-tauri/src/ondevice_models.rs) · [下载器](../src-tauri/src/ondevice_download.rs) · [Swift 转写入口](../src-tauri/mlx-sidecar/Sources/VoiceFlowMLXSidecarCore/SidecarServer.swift) · [模型资源管理](../src-tauri/src/model_resources.rs) · [设置页](../src/components/settings/EngineSettings.tsx)。

### A2. 验收并开放下载

| 层级 | 验收内容 |
| --- | --- |
| 文件与权限 | 好包能安装，缺文件、错版本和坏 SHA 拒绝。取消、重试、删除和磁盘不足可从 UI 观察。平台不支持或 sidecar 过旧时给出原因。 |
| 推理和资源 | Apple Silicon 上实际从本地目录完成冷、热、长短 WAV 推理，校验覆盖到最后一段。空输入、静音、截断、取消和内存拒绝按原契约处理。用最低拟支持内存档及至少一台对照机记录峰值和可用内存。 |
| 质量 | 固定同一批外部 WAV，英语和另外至少两种支持语言分开报告 WER、专名和数字错误、空结果及失败率。可从 [FLEURS](https://huggingface.co/datasets/google/fleurs) 选独立公开样例，同时加入有断句与尾段的受控用例。将 Qwen 0.6B 作为产品基线，候选量化包还需与 Parakeet 参考权重配对。中文或中英混合样例单列为边界观察。 |
| 用户路径 | 从无文件到点击下载、校验完成、选择并保存、成功离线转写，再切回 Qwen。先走隔离 WAV 与临时模型目录，正式的桌面手工验收使用测试安装和受控数据。 |

首发同时满足 G0、A0 的质量与价值门槛、文件和运行失败用例，以及独立桌面验收。验收语种未达到 A0 的数据量或门槛时，不将其标为已验证。对通过验收的机器档记录实际冷加载预算，再开放该档。文件评测与停说到写入输入框的计时分别报告。

## 3. Cleanup 模型目录、下载与选择

### B0. 产品入口

- 「模型配置 → 本地模型 → 本机文字整理」列出可下载的模型。每行显示模型名、实际下载大小、用途和语言、安装状态。内存与系统要求放进技术信息。不兼容时保留说明并禁用下载。默认显示一项主要操作，下载中显示进度与取消，完成后显示已下载，移除入口沿用本地模型页的样式。
- 下载完成后不切换当前模型。「模型配置 → 服务配置 → AI 文字整理」列出当前设备已下载、已校验且兼容的本机模型。用户选择后通过现有「保存并使用」生效。升级保留当前已保存的配置。并列展示可用模型，按用途与下载大小描述，用户自行决定。
- 通用整理模型按实际能力开放现有场景、轻中重与输出模式。S1-mini 标注「仅英文」，按其原生控制格式映射已验证的强度和排版；中英混合或其他语言无法安全路由时保留原转写。FunASR `ct-punc` 单列为「仅标点」，其界面与运行时都限制到标点处理。对明确选择的专用模型，若语言或模式不适用，返回可解释的本地保底结果；切换到其他模型由用户决定。

### B1. 下载和运行

- 为每个可发布的制品固定模型 ID、来源、完整 revision、文件大小及 SHA-256，核验原模型、转换产物的再分发许可和署名要求。下载先写临时文件，校验完整后再安装；验证取消、重试、损坏文件、应用重启和删除。删除当前在用的模型前提示，并要求先改选或关闭整理，避免保存的设置指向已删除文件。
- 通用新增模型优先采用文本 GGUF 和同一版随包 llama.cpp。首批计划 Qwen3.5 4B 用 Q4_K_M、Gemma 4 E4B IT 用 Q4_0，Qwen3.5 2B 在接入时锁定具体量化制品。仅加载文本所需权重。27B 继续用现有 MLX。专用模型分别核实模板或推理依赖，符合打包和运行要求后开放下载。 [Qwen3.5 4B](https://huggingface.co/Qwen/Qwen3.5-4B) · [Gemma 4 E4B GGUF](https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF) · [S1-mini GGUF](https://huggingface.co/superwhisper/s1-mini-GGUF) · [FunASR ct-punc](https://huggingface.co/funasr/ct-punc)
- 模型下载、状态、运行及内存准入以模型 ID 定位。切换后按实际生命周期回收旧整理模型；只用已校验的本地权重推理。通用模型复用生产 prompt、App 追加指令、输出模式及保真 guard。专用模型只接收其支持的结构化控制，仍复用适用的输出检查。超时、空回复、预算截断或加载失败按现有本地保底路径处理，并记录真实执行结果。
- 下载由用户显式发起并需要联网，已安装后的推理可以在严格离线模式下使用。验证打开严格离线后从服务配置到实际模型进程的调用，无云端回退。

**历史快照（2026-11-09，部分状态已被后续实现取代）：** 当时已为 Qwen3.5 2B/4B/9B、Gemma 4 E2B/E4B IT 与替代 Ministral 3 3B 固定 MLX 源 revision、必要文件大小及 SHA-256，并在开发构建中接入模型管理页；旧快照所称“正式发行构建不开放”已不再描述当前模型目录。当前代码开放的新下载与选择由独立清单控制：Qwen3.5 2B / 4B、Gemma 4 E4B IT、Ministral 3 3B Instruct，以及既有 Qwen3.8 27B；新增模型的下载可用性与 `release_qualified` 分开，新增模型仍未通过完整质量评测。Qwen3.5 9B 与 Gemma 4 E2B IT 仍是固定来源的开发候选，尚未开放新下载或选择。模型文件夹导入只对既有 Qwen3.8 27B 开放。Qwen3.5 MLX 与同名 GGUF 是不同格式和来源。早期 Oscilla Ministral 3 3B 固定 MLX revision 在观察运行中重复输出 `ipi` 直至生成上限，作为失败历史记录保留。替代的 `mlx-community/Ministral-3-3B-Instruct-2512-4bit@a962dcb09eee4169c890e544c9eb938f1113fdee` 已固定文件清单，并通过 pinned `mlx-swift-lm` 3.31.4 的文本整理 smoke 与 EOS 检查；其制品包含 vision/projector 张量，但 Swift 文本整理路线实测可运行。该 smoke 只证明 runtime 兼容性，未完成内存、质量或发布验收。S1-mini by Superwhisper 的独立 descriptor 仅用于 test-only 路径；在受限 prompt、English/script、control 与 fallback 端到端检查通过前不进入可选择目录。本段为开发快照记录；计划中的 G0 实机资源验收与 B3 逐模型质量 / 兼容验收仍未关闭。

S1-mini by Superwhisper 的 MLX 来源为 [`mlx-community/S1-mini-MLX-4bit`](https://huggingface.co/mlx-community/S1-mini-MLX-4bit/tree/5cbd7aec3401144f88a331d385c40b65fd2548eb)。该 revision 的 LICENSE 标为 `other` / `s1-mini-license`，包含 Apache-2.0 及附加命名条款：任何使用、分发或集成都须保留原名 “S1-mini” by “Superwhisper”（大小写完全一致）；其 README 说明模型源自 Qwen3-0.6B，并由 `mlx-lm` 0.31.3 量化，但未固定原始 BF16 输入 revision。完整来源 LICENSE 和归属要求见 [`S1-mini-MLX/`](../ThirdPartyLicenses/Models/S1-mini-MLX/NOTICE.md)。独立 test-only descriptor 不进入常规模型目录；在受限 prompt、明确 English、保守 script screening、原生控制和空/噪声等输入的原稿 fallback 端到端检查通过前不开放选择或发布下载。

### B2. 随包 llama.cpp

- VoiceFlow 管理本地 helper 的启动、就绪、取消、超时、退出和资源回收。若使用 loopback HTTP，只绑定 `127.0.0.1`，每次启动用随机短期凭据认证并校验进程身份与就绪响应。验证无凭据请求被拒绝、端口冲突及进程崩溃回收。凭据不落盘、不写日志，避免通过进程参数暴露。
- 按 Tauri 发布路径验证签名、公证、hardened runtime 和所需权限，附上 runtime 与模型许可。以 llama.cpp 构建号和 GGUF SHA 固定验收身份；升级后复验模型加载、模板、停止条件、取消和内存释放。用实际打包产物验证，用户只需安装 VoiceFlow。 [llama.cpp](https://github.com/ggml-org/llama.cpp) · [Metal 构建](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md#metal-build)

### B3. 按模型独立验收

- 用隔离的文本夹具验证「下载 → 校验 → 选择 → 保存 → 本地整理」整条路径。先用临时目录测状态和失败分支，再用实际制品验证能加载并产出结果。夹具覆盖所声明支持的语言、轻中重、输出模式和保真案例。重点检查专名、数字、否定、自我纠正、换行和列表。S1-mini 另验非英文输入；`ct-punc` 检查仅改标点、保留原词。翻译等模式只在所选模型支持且验证过时开放。
- 测量各自支持档的冷启动、热请求 p95、整理进程族峰值，以及与 ASR 驻留叠加时的系统内存压力。固定样例长度分布、设备、运行时和权重版本。各路线用同一 `phys_footprint` 采样口径按进程族合计，记录实际间隔、全机压力和 swap；磁盘大小单列。轻量模型按 G0 的绝对预算验收，其他模型按各自实测档配置准入及提示。 [现有内存准入](../src-tauri/src/model_resources.rs)
- 验证超时、取消、校验失败、模型损坏、内存不足、重启恢复和严格离线。每款过线后独立开放，某款证据不足或失败只影响该款。候选之间的排名、相对 27B 改善幅度和统一盲测均不作为发布条件。

## 4. 交付顺序

1. ASR 沿用 A0–A2，冻结并验证 Parakeet 制品后接入下载、选择和运行。中文 ASR 候选沿用第 5 节的独立路线。
2. 完成本机整理的多模型目录、下载状态、服务配置选择和共享运行路径。先贯通 Qwen3.5 2B、Qwen3.5 4B、Gemma 4 E4B，分别验收、分别开放。
3. 沿用同一目录接入其余通用模型，再按专用能力接入 S1-mini 和 `ct-punc`。更新模型和发布活文档，完成后按文档约定移除已交付的计划条目。

## 5. ASR 补充调研

### ASR 榜单与候选顺序

2026-11-09 查阅 Hugging Face 官方榜，最新登记版本为 2026-11-06。以下是该版本英语短音频的平均 WER，越低越好。它反映榜单模型与运行方式，不能代替 Mac 上的 MLX 测量。 [官方榜](https://huggingface.co/spaces/hf-audio/open_asr_leaderboard) · [版本登记](https://huggingface.co/spaces/hf-audio/open_asr_leaderboard/blob/main/init.py)

| 排名 | 模型 | WER | 对 VoiceFlow 的用途 |
| ---: | --- | ---: | --- |
| 1 | Zoom Scribe v2.1 Pro | 4.87% | 云端精度参照 |
| 9 | NVIDIA Nemotron 3.5 Transcribe 30B-A4B | 6.13% | 评测参照，单独评估本机资源成本 |
| 10 | Qwen3-ASR 1.7B-hf | 6.15% | 优先复测已有 MLX 路线的内存准入 |
| 13 | IBM Granite Speech 5.0 2B | 6.68% | 后续英文精度参照，需另核实部署路线 |
| 14 | Cohere Transcribe | 6.72% | 已有 MLX 路线，待内存和许可核验 |
| 25 | Parakeet V2 | 6.93% | 英语专用对照 |
| 31 | Parakeet V3 | 7.04% | 首发候选，兼顾英语和多种欧洲语言 |
| 33 | Qwen3-ASR 0.6B-hf | 7.07% | 当前本机基线 |
| 47 | OpenAI Whisper Large v3 Turbo | 8.75% | 多语言对照，独立评估本机体验 |

中文另看 GigaSpeechBench 的中文领域子榜。FunASR-Realtime 为 3.12% CER，Qwen3-ASR 1.7B 为 3.95%，FunASR-MLT-Nano 为 4.67%，Whisper Large v3 为 9.83%。FireRed 团队报告在另外四套普通话数据上，**FireRedASR2-AED** 为 3.05%，同表 Qwen3-ASR 1.7B 为 3.76%。这两套数字分别看。FireRed 的 Swift 接口已在 VoiceFlow 锁定的 SDK 中，候选制品是 **AED MLX**，团队报告的 **LLM** 版成绩单列。 [GigaSpeechBench 中文领域榜](https://github.com/SpeechColab/GigaSpeechBench#-leaderboard) · [FireRed 官方对照](https://github.com/FireRedTeam/FireRedASR2S) · [锁定版本的 AED 接口](https://raw.githubusercontent.com/Blaizzy/mlx-audio-swift/01dec7c9bdce3088a6b6b7ab9f2e403458195efb/Sources/MLXAudioSTT/Models/FireRedASR2/README.md)

先完成 Parakeet V3 的英语与欧洲语言路线，同时把 **Qwen3-ASR 1.7B、FireRedASR2-AED 和 Fun-ASR-Nano-2512** 纳入中文候选对照。中文组统一测普通话、口音、中英混说、专名与数字，再结合 Mac 上的长音频覆盖、峰值内存和停说后出字时间，决定是否新增一个高质量中文选项。Parakeet V2、Cohere 与 Whisper 可放在对照组，用来判断哪些语言或硬件场景确实需要进一步扩充。


### 中文候选独立验收

- 基线固定为现有 Qwen3-ASR 0.6B BF16。Qwen3-ASR 1.7B 先核实已有研究加载入口，拟使用已有的 BF16 固定制品。FireRed 先验证锁定 SDK 能加载确切的 AED MLX 权重，并核实原模型、转换产物、授权及长音频行为。各路线记录实际加载的 dtype、后端、revision 和 decoder recipe。FireRed 的精度以实测为准。
- Fun-ASR-Nano-2512 先用发布方提供的 `llama-funasr-cli` 和 GGUF 做隔离 WAV 测试。固定 Mac 运行时版本及 SHA-256、F16 音频编码器和 Q4_K_M 解码器的 revision 和 SHA-256。若启用 VAD，固定其权重和配置。先统一音频与分段、关闭热词，再单列同一词表的热词诊断，记录专名召回和误插入。榜单中的 Fun-ASR-Realtime 云服务与 MLT-Nano 属于不同参照，离线 Nano 以实测结果为准。产品接入前单独验证制品许可、子进程取消、超时和回收。 [官方 Nano GGUF](https://huggingface.co/FunAudioLLM/Fun-ASR-Nano-GGUF) · [FunASR 模型许可说明](https://github.com/modelscope/FunASR/blob/main/model_zoo/huggingface_models.md#model-license)
- 使用相同的隔离 WAV。普通话、常见口音、中英混说每组至少 60 条且 20 分钟，至少 3 名说话人，清单按说话人分组。已有 ASCEND 资料按实际审核状态使用，未审核资料只作功能观察。先冻结评分脚本与参考文本。主评分做 Unicode NFKC、ASCII 小写和统一空白，去除标点，使用固定版本繁简映射。中文按字符，连续英文按词，阿拉伯数字串作独立 token，不自动把中文数字改成阿拉伯数字。普通话与口音报 CER，中英混说报合并 token 编辑率并分报中文 CER、英文 WER，原样标点、专名、数字与单位另审。对整段遗漏、失败和拒绝单列，质量通过率的分母保留这些样例。
- 进入产品接入的拟定门槛是至少一组 CER 或中英混合错误率相对基线下降 ≥ 10%，其余各组劣化 ≤ 0.5 个绝对百分点，零整段漏听，且过 G0 的 ASR 延迟和内存门槛。未过 16 GiB 档但过 64 GiB 档时，只评估对已经验收的高内存档开放。证据不足时保留为研究对照。
- 选中的中文制品再走 A1/A2 等价流程。固定文件清单、许可与内存预算，另备符合相同分层和样本量的独立验收集。完成下载、校验、UI 选择、语言、分段、取消、严格离线及桌面验收后才发布。若有多个候选通过，优先选在目标场景配对准确率更高且覆盖目标机型更多的一款。若候选在准确率和机型覆盖上各有优势，保留为候选并记录需要补测的差异。
