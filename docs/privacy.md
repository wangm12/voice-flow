# VoiceFlow 数据与隐私说明

## 数据流

「语气」中的试跑仅在用户点击按钮时运行。使用手动输入的样例文字、当前 / 已保存 Prompt、场景与语言配置，并复用本机准备、手动词典提示、整理路由和事实保护规则；不会捕获当前 App、屏幕、剪贴板或 History，不附加 App 风格示例、自动上下文或语音片段。允许调用模型时只请求已配置的整理服务；AI Off、保守场景和严格离线约束仍生效。草稿比较最多顺序执行两次请求，取消、关闭试跑或编辑样例 / Prompt 会使迟到结果失效；关闭后的样例和结果不保存。配置变更后的结果不会显示，试跑最长 90 秒。它不写入 History、词典学习、日志或其他 App。

独立翻译快捷键默认未设置。它只为自己启动的录音固定目标语言，将完整本机准备稿作为正文交给明确的翻译操作，不需要额外口述翻译指令；正文里的指令或语言名称不改变固定目标。沿用普通听写的音频、授权、保留和目标核对规则，不修改全局输出模式，也不赋予额外的截图或文字读取权限。开始前检查当前场景整理路由和凭据；严格离线模式继续阻止云端整理。

音频评测 CLI 与日常录音分开。默认只校验外部 WAV 清单；只有显式 `--run`、审核过的来源说明、指定模型和显式环境 / 文件凭据齐备时才调用所选服务。它不取用麦克风或 History，也不自动使用 Keychain / 历史 sidecar 密钥。清单、WAV 和报告必须在仓库外，报告包含参考文本、ASR 转录和整理结果，须按样本的数据要求管理；报告不包含 API Key，不复制音频。

VoiceFlow 的会话录音只在用户主动触发 dictation 后开始。默认按需打开麦克风；明确开启可选常开麦克风后，空闲输入流持续打开，但样本直接丢弃、不保存、不发送。若麦克风权限仍处于未决定状态，首次触发会请求 macOS 显示系统授权提示；只有用户允许后才开始采集。拒绝授权或权限已关闭时，录音不会启动，可从「系统权限」页前往 macOS 隐私设置。默认 ASR 仍是 Groq `whisper-large-v3-turbo`，不会因增加预设而改变已有选择。用户也可选择 OpenAI `gpt-transcribe`、Deepgram、SiliconFlow、Fireworks、本机 Whisper HTTP、Mistral、Soniox、AssemblyAI Dictation / raw Sync、DashScope Qwen Audio 3.1 Message、On Device 或自定义 endpoint；已有 provider、model 与 custom endpoint 配置会保留。SiliconFlow 仍以 `FunAudioLLM/SenseVoiceSmall` 为默认 ASR model，并增加可选 `Qwen/Qwen3-ASR-1.7B`。新增的独立 provider ID 是 `fireworks`（`whisper-v3-turbo`，base host `https://audio-turbo.api.fireworks.ai`）、`mistral`（`voxtral-mini-2602`，base host `https://api.mistral.ai`）、`soniox`（实时流式专用 `stt-rt-v5`，WebSocket `wss://stt-rt.soniox.com/transcribe-websocket`）、`assemblyai`（Universal-3.5 Pro）和 `dashscope`（`qwen-audio-3.1-asr-flash-message`）。Fireworks 服务可用性尚未核实；Qwen 预设的账户和地区可用性也尚未核实。Soniox 的真实麦克风连接尚未验证；本阶段未验证 AssemblyAI / DashScope 账号与服务可用性。Deepgram 预设不启用 `multi`，不据此宣称中文或中英混合支持。

可选集成 provider `assemblyai` 提供 Universal-3.5 Pro 的 Dictation / raw Sync 双路径；`dashscope` 只提供 Qwen Audio 3.1 ASR Flash Message 原始 ASR。DashScope region 在设置和引导页显式选择并保存为 Beijing / Singapore 的 HTTPS origin，再映射到官方 WSS host；不使用 Workspace ID。保存 key 只代表凭据已配置，真实账号 / 地区可用性与服务质量未在本阶段验证。默认 Groq `whisper-large-v3-turbo` 和 cleanup `openai/gpt-oss-20b` 不变。

On Device 提供 `qwen3-asr-0.6b`（新选择默认）、`qwen3-asr-1.7b` 和 `cohere-transcribe-2b` 三种显式 MLX 模型。运行要求 Apple Silicon 与 macOS 14 或更新版本。Cohere 模型必须选择中文或 English 固定语言；自动语言检测不可用。旧 `sensevoice-small` 配置与本机文件继续保留，但该文件路径没有 MLX 推理支持，即使文件已就绪也不能通过 MLX 推理探测或启动 MLX 转写。设置页将文件校验状态、sidecar 能力握手、当前模型加载中和已加载模型分别展示；加载状态绑定具体模型。握手只确认 sidecar 能力，不代表所选模型已加载或一次实际转写成功。用户可取消当前模型加载；取消会停止这次加载，不更改已保存的 provider / model 选择，也不删除下载文件。

“严格离线模式”只允许 On Device ASR；所有 HTTP ASR provider 都会被阻止，包括 loopback LocalWhisper / 自定义 endpoint；这些 HTTP 路线的引擎探测也不可用，On Device 的本机设置探测仍可用。该模式还阻止云端 cleanup、视觉、History 重试、cascade / prefetch 和云端文字操作，并取消进行中的云请求，同时保留已保存的 provider 选择；关闭模式后原设置仍可用。cleanup 可单独显式配置为经验证的 loopback Ollama 上的 `qwen3.5:4b`。VoiceFlow 不安装 Ollama 或下载 cleanup 模型；本机服务或模型不可用时使用本地规则或关闭 AI 整理。首次 On Device ASR 下载由用户在设置或引导页主动发起，模型文件来自固定版本的公开仓库，下载需要网络连接；启动时不会自动下载。该文件下载操作与严格离线听写分开。

Soniox 只出现在 ASR 选择中。开始听写时先建立非阻塞、有界音频队列，再于后台建立 WebSocket；麦克风采集不等待网络握手。队列溢出会标记实时流不完整，完整本机录音仍保留用于同 provider 恢复。保存设置或凭据只把 API Key 放入 macOS Keychain，并显示为已配置；保存本身不连接服务。用户开始听写或明确执行 provider 测试时才建立服务连接。VoiceFlow 按保存的识别语言发送候选语言提示：自动模式提示中文与 English，固定模式提示所选语言；这些提示不限制识别。当前 Soniox 路径不发送自定义上下文或术语。临时 token 保留在当前录音会话，不进入 cleanup、目标 App、剪贴板、History 或词典学习；只有服务端完成后得到的最终 transcript 才进入现有 raw / final 处理流程。若流连接、音频覆盖或 finalization 失败，VoiceFlow 取消这次流并用本机保留的完整、裁剪前录音按近实时节奏向同一 Soniox WebSocket 重放一次；HUD 会显示完整录音正在重转写，不会拼接未确认的 partial，也不会跨 provider 静默回退。重放会再处理完整录音，因此可能产生第二次整段费用。Soniox 按完整实时流时长计费，包含静音；诊断中的 session 墙钟时长和发送音频秒数不是 provider 账单用量。实际价格以 [Soniox 定价页](https://soniox.com/pricing) 和账户合同为准。该服务的 [WebSocket 接口说明](https://soniox.com/docs/api-reference/stt/websocket-api)列出了实时协议与模型能力。

### AssemblyAI Dictation 与 raw Sync

可选 ASR provider `assemblyai` 固定使用 Universal-3.5 Pro。对于总音频不超过 120 秒、且当前允许 AI 整理的录音，VoiceFlow 使用 `POST https://dictation.assemblyai.com/v1/transcribe/live`。请求以原始 API key 放在 `Authorization` header；multipart 的第一个 part 是 `application/json` 配置，随后是 `audio/wav` WAV 音频。该 endpoint 每次最多 120 秒，且即使省略或置空 `llm_instruction` 也会执行默认改写；没有文档支持的 `cleanup=false` 参数。

Dictation 响应中的 `text` 是独立 ASR 原稿，`llm_response` 只是 provider 整理候选，`llm_error` 表示候选整理失败。VoiceFlow 将候选作为候选而非原稿，先与 Phase 2 本地准备稿及授权信息对齐，再经过最终 protected-span guard；只有 guard 接受后才使用候选，并跳过第二次 LLM 请求。候选缺失、为空或带有 `llm_error` 时保留 `text`；若共同整理凭据可用，对本地准备稿执行一次共同 cleanup，否则使用本地规则回退。若候选被最终保护检查拒绝，则保留受保护的原稿 / 本地回退，不再发第二次模型请求。HTTP 200 本身不代表整理成功。

AI Off、本地-only 场景、翻译输出模式或全录音超过 120 秒时，VoiceFlow 改用同一 AssemblyAI provider 的 raw Sync：`POST https://sync.assemblyai.com/v1/transcribe`，使用同一把 Keychain key 的原始 `Authorization` header 和 `X-AAI-Model: universal-3-5-pro`。翻译在 raw ASR 后通过已配置的共同整理服务处理完整准备稿，并沿用最终保护规则。配置在 WAV part 前发送；每个请求最多 120 秒。Dictation 与 Sync 都始终显式发送 `language_codes`：识别语言为 auto 时发送 `['zh','en']`，固定中文发送 `['zh']`，固定 English 发送 `['en']`。这是中英预期语言列表，不是未限定的服务端自动检测；API 文档列出 32 个语言代码，而产品设置仍只有 auto / 中文 / English。Sync 只使用专用 `keyterms_prompt` 发送有界术语，最多 100 项、总长 8000 字符；不发送会使 `language_codes` 被忽略的通用 `prompt`。

长录音由有界重叠 chunk 完整覆盖，每段走 raw Sync；全稿合并后最多运行一次共同 cleanup，不逐段改写再拼接。长录音的模型整理需要已配置的共同 cleanup 服务；若凭据缺失或服务整理不可用，应用保留 raw / 本地准备稿并使用本地规则回退。关闭 cleanup 时只保留本地准备稿。短录音的 fused Dictation 候选不依赖共同 cleanup 密钥；候选缺失、为空或带有 `llm_error` 时，只有共同 cleanup 凭据可用才会再请求该服务，否则保留原稿并使用本地规则回退。若候选被最终保护检查拒绝，则保留受保护原稿 / 本地回退，不再请求第二个模型。请求取消、严格离线切换或设置版本变化会阻止迟到的云端结果进入 History 或交付；History 重试与引擎探测也必须遵守同一离线与取消策略。AssemblyAI 的设置保存探测只请求 raw Sync ASR，不检查可选共同 cleanup 凭据或服务；Soniox 的设置保存不请求 provider。设置保存探测只覆盖其说明的 provider 路径，不能证明真实语音识别质量或完整 Dictation / cleanup 端到端表现。

### DashScope Qwen Audio 3.1 Message

可选 provider `dashscope` 只接入精确模型 `qwen-audio-3.1-asr-flash-message`，用于保留独立原始 ASR 的 Message WebSocket 路径。设置中的 Beijing / Singapore 选择保存为 HTTPS origin `https://dashscope.aliyuncs.com` 或 `https://dashscope-intl.aliyuncs.com`；请求解析为相应官方 WSS host 的 `/api-ws/v1/inference`，使用 `Authorization: Bearer`。VoiceFlow 不使用 Workspace ID。连接先发送带 UUID 的 `run-task` 与 duplex mode，等待 `task-started` 后上传完整音频，再发送 `finish-task` 并等待明确的 `task-finished`。

该接线发送 `disfluency_removal_enabled:false`、关闭 interim results，只将最终句子作为原始 ASR；原生润色没有接入，后续 cleanup 仍由 VoiceFlow 单独执行。普通 HTTP `qwen-audio-3.1-asr-flash` 不是这条 Message 接线，VoiceFlow 不据此宣称它能关闭润色并取得原稿。句子或词的时间戳仅在服务实际返回时使用，单位为毫秒；`task-finished` 的累计 usage 只记录一次，不把逐条消息的累计值相加。模型文档给出的 7168 输入 / 1024 输出 token 限制使整段长音频有输出预算风险；VoiceFlow 以保守大小做有界重叠分段并完整覆盖音频，不截断尾部。服务账号、区域权限、识别质量和延迟均未在实现阶段验证。

HTTP ASR 上传 16 kHz、单声道、16-bit PCM WAV。应用单次直接 ASR 请求上限为 10 分钟；更长的录音会按完整覆盖分段处理，录音本身最多 15 分钟，不会截断尾部。用户可把分段阈值设在 5–3600 秒、目标段长设在 15–60 秒；长路径仍完整覆盖录音。SiliconFlow adapter 另执行 1 小时与 50 MB provider 文件限制，Mistral 文档限制为单次 3 小时；Fireworks 没有在此接线中声明 provider 时长或文件上限，10 分钟是 VoiceFlow 的请求限制，不是 Fireworks 服务声明。

SiliconFlow 的 SenseVoice 与 Qwen 请求只发送 multipart `file` 和 `model` 字段，语言行为为自动检测；此接线不发送 language、context/terms 或 `verbose_json` 等 metadata。Fireworks Whisper 同样以 multipart `file` 和 `model` 上传，使用自动检测，不请求术语提示、时间戳或置信度。Mistral 使用 multipart `file` 和 `model`：省略 language 时自动检测，也可指定单一固定语言；可发送最多 100 个词条的 `context_bias`，不会发送 Whisper `prompt`，也不使用置信度。Mistral 只在自动检测时请求片段时间戳，指定固定语言时不请求时间戳。除非用户另外配置了准确识别级联，ASR 只请求当前选定的 provider；该级联若触发，也只请求用户另行配置的 provider。失败不会静默切换到其他服务。ASR HTTP 请求不跟随重定向。

当用户选择 OpenAI `gpt-transcribe` 时，已晋升的专有名词会作为 `keywords` 发送，而不是整本词典。本机 Whisper 不会把 ggml 下载进 App，需要本机已有 whisper.cpp 或 speaches。启用 AI 文字整理时，转录文本会发送到用户配置的整理服务（默认为 Groq；也可改为 OpenAI、SiliconFlow、DeepSeek、Anthropic、Ollama 或自定义端点）。转写和润色可以不是同一家。普通和长录音在发起整理请求前，会在本机执行语音标点、有限布局解析、已授权词典替换和同短语明确改口；请求只包含整理所需的准备稿与现有已授权上下文，不附带修订范围元数据。

若用户显式选择 Ollama 本机整理，VoiceFlow 使用本机 Ollama 上的 `qwen3.5:4b`，不会自动安装 Ollama 或拉取模型。设置里的状态检查由用户主动触发；它检查已保存的本机地址和模型状态，不会更改服务端配置。单个请求的消息内容最多 4096 UTF-8 字节（包含系统提示）；超限、输出预算耗尽、工具响应或不完整回复均失败关闭，并按本地规则处理完整准备稿，不把模型未完成的前缀作为成功候选。严格离线模式开启时只允许 On Device ASR；它会拒绝其他 HTTP ASR 路由（包括 loopback LocalWhisper / 自定义 endpoint）和这些路线的 provider probe，但仍允许 On Device 本机设置探测；同时拒绝云端 cleanup（包括 AssemblyAI 与 DashScope）、History 重试、cascade / prefetch 以及视觉请求，并取消已经进行中的云端请求，同时保留保存的云端选择；若本机 Ollama cleanup 不可用，应用保留准备稿并使用本地规则或关闭 AI cleanup。On Device 模型下载另需网络，是设置中的显式管理动作，不作为听写回退。

用户主动启动选中文本操作后，VoiceFlow 通过辅助功能读取这次选区、受支持的可编辑字段全文或空回复框，并把捕获到的文本和语音指令发给用户配置的整理服务。来源超过 16 KiB 会在请求前被拒绝；无法取得精确选区或原字段版本时，来源只能生成预览，不能用于替换。VoiceFlow 不从现有剪贴板猜测来源。确认或复制前，结果和用户编辑只存在于这次内存预览；它们不会进入普通 History、HUD、日志、导出、个人词典或风格学习。确认前会复核目标和来源；在交付前发现变化或无法验证时复制预览结果，不修改目标。若已经尝试写入但系统不能确认交付状态，会安全报告未验证，不盲目重试。

起草回复是选中文本操作中的独立受限类型：目标必须是空回复框，并且当前 App 规则须分别允许读取辅助功能文字和将该文字发送给 provider。只有这时才会捕获有界的附近页面文字；这是一次明确操作，不代表全局或默认上下文许可。发送请求前（包括等待限额和重试）会再次核对权限、页面目标和来源版本；撤销许可或目标变化会丢弃这次请求，即使随后重新授权也不会恢复旧请求。目标标题、原始 URL、查询参数、片段和页面身份只用于本机路由与目标安全检查，不发送给整理模型。手动「看屏幕」热键仍是另一项独立操作：需配置视觉模型和屏幕录制权限，截取当前窗口一张内存 PNG，发给用户配置的 vision provider 并先弹出预览（替换 / 只复制 / 取消）。取消或关闭预览会丢掉图片；History 不保存截图。选中文本或看屏幕的语音指令也走现有录音 / ASR 流程；ASR 失败时完整录音按恢复策略保留在 History，长录音按完整覆盖分段重试。History 重试只复制，不恢复原操作目标；成功操作的选区、指令与预览仍不进入普通 History。会议录音、Spark 和生图仍不提供。

自动上下文的文字和图像权限单独逐 App 管理。`context_enabled` 是总开关；每条现有 AppMapping 的 `source_permissions` 分别控制 AX 文字、本机 OCR、云端视觉和将已识别的上下文文字发给配置的 ASR / cleanup provider。四项新增权限缺省均为关闭；旧的总上下文开关、App mapping 或 `window_ocr_enabled` 不会授予 AX 文字、provider 文字或云端视觉。升级前若用户已显式打开全局窗口 OCR，则仅为迁移时已有的规则保留 `local_ocr=true`，仍由全局 `window_ocr_enabled` 总开关控制。本机 OCR 描述的是识别位置，不代表其派生文字永远留在本机：只有另外打开 `context_text_to_providers` 后，受允许的 AX / OCR / 云端视觉识别文字才会进入用户配置的 ASR / cleanup 请求。

规则可要求 App bundle、可执行文件、host、页面路径前缀、焦点字段的任意组合；所有已填写的条件必须同时匹配。host 和路径仅在本机用于路由；raw URL（含凭据、查询、片段）、窗口标题、PID 和目标 identity 不会进入 provider payload、普通 History 或导出。具体网站 / 路径 / 字段规则按确定的特异性顺序胜过通用浏览器规则；缺失信号不满足已填写的条件。场景会使用正向字段证据细化：IDE 的通用编辑字段不会自动当作 chat prompt；GitHub 的 issue / pull request 标题、正文或评论只在对应页面和可识别字段标签同时匹配时区分；Slack / WeChat 的搜索字段按 Search 处理。原始辅助功能字段标签只用于本机短暂分类，不保存在上下文或发给 provider。

自动内容提取只读取匹配规则明确许可的有界 AX 字段；云端视觉只允许绑定到具体 App / 可执行文件规则。若授权的 AX 证据已足够，不截屏；若仍不足且 `local_ocr` 与全局 OCR 开关都开启，则检查屏幕录制权限后至多截取当前窗口一张内存图用于本机 OCR。若结果仍不足，且同时有具体 App 的 `cloud_vision`、`context_text_to_providers`、已配置的 vision provider / model 和屏幕录制权限，则把同一张图作为自动云端视觉回退。AX 与 OCR 内容足够时跳过云端请求。不会捕获整屏、不会静默更换 provider / model；权限被撤销、会话取消或目标 / 页面变化后会丢弃待用证据和结果，保留原始录音用于正常安全回退。图像只在内存中短暂存在，不进 History / 导出 / 日志。

mapping 保留的风格样例另有 `style_examples_approved` 明确许可，与一般文字 provider 许可互相独立。旧记录中来源不明的样例会留在设置中，但缺少该字段时按未批准处理，不会进入 prompt，直到用户复核批准。自动观察到的短风格改写先进入待确认草稿；用户明确确认后才成为获准样例。密码字段、银行 / HR / SSO / 密码管理器和关闭学习的预设仍受保护。手动选中文本快捷键只授权本次选区，不自动附带无关的邻近 AX 文字；手动看屏幕权限不会授予自动上下文截图。

场景和目标检测在本机完成。浏览器 host / path 匹配只有在用户开启浏览器访问后才会执行。当前页面身份（包括查询 / 片段变化）只在本机用不可逆 fingerprint 绑定请求结果与交付目标；原始 URL、凭据、查询、片段不序列化。

HTTP batch provider 可以在录音期间批量预取已完成分块；这些预取文字不显示在 HUD，也不会写入目标 App、剪贴板或 History。Soniox 使用独立的真流式路径，不启动相同音频的 batch 预取；录音中的 provisional token 不进入 cleanup 或持久化数据。停止录音后，如果配置的准确识别级联正在运行，HUD 可能短暂显示最多约 280 字的主识别草稿；它只存在于灵动岛窗口，不会写入 History 或剪贴板，也不会发送给 LLM。

粘贴成功后，若开启词典学习，VoiceFlow 会轮询同一个已锁定输入框的当前值（约 3 秒起，空闲可延长到最多 12 秒），用来发现用户当场改写的短词或短短语。该轮询只在本机进行，不会发送给 LLM。密码框 / Secure Input、目标已变、该 App mapping 关闭学习、或 1Password / HR / SSO 等预设关学习的目标不观察。

## 本机麦克风自检与词典反馈

用户主动点击麦克风自检后，应用最多打开输入流 30 秒，只在内存测量音量、峰值、收到帧 / 信号状态和保存的输入增益。自检不会创建录音文件、恢复 spool、History 或 transcript，不调用 ASR / 整理 / 视觉服务。需要已获麦克风权限；未获准时提示前往权限页，不在自检中自动申请。停止、离开页面、窗口隐藏或失去焦点、音频设置变化或正式听写开始时结束。用户已明确开启常开麦克风时，自检结束后仍恢复该空闲流，其样本继续丢弃。

首次引导的本机路线明确关闭 AI 整理，只做本机转写；用户可之后另行选择整理服务。选择 On Device 不等于启用严格离线，模型下载仍是用户主动发起的网络操作。

个人词典反馈保存在 history SQLite 的 `learned_term_usage`（schema v13），仅包含实际本机词典替换的目标词、按处理次数和最近替换时间；不新增整段转录、音频或目标 App 数据。每段处理按词条去重，包括用户主动进行的 History 重新整理；从功能启用后记录，不推测旧收益。记录在本机准备阶段生成，可能发生于后来取消、回退或未确认交付的处理，因此不用于声称准确率或成功插入。词条没有生效规则后不再展示对应收益，所有本机学习记录在「清空全部数据」时删除；不会自动上传。

## 本地数据

以下数据保存在 Tauri app_data_dir：

- history SQLite：provider 原始 ASR 文字（`asr_text`）、provider 明确返回时才保存的独立整理候选（`provider_cleaned_candidate`）、本地处理后的识别草稿（`raw_text`）、通过最终保护检查的听写结果（`final_text`）、实际产出识别草稿的 ASR provider/model（`engine`）、状态、delivery/fallback 信息和经过过滤的 context policy。AX/OCR/视觉原文、图片、raw URL、窗口标题、PID、目标 identity 和风格样例文字不序列化；风格样例的当前批准状态只从设置重新读取。新候选列允许为空，旧记录迁移后 `asr_text` 和 `provider_cleaned_candidate` 均为空；不会猜测补写缺失的候选或 provider provenance。History schema v12 为粘贴交付新增 nullable 稳定错误码和安全用户提示列；旧记录保持为空，交付失败、未确认或 post-write cancellation 时保存这些字段而不保存诊断中的听写文字；历史 JSON 导出包含这些 nullable 字段。若 History-only 等交付结果写入失败，HUD 会显示保存失败而不显示已保存；recovery spool 可用时，短录音的 manifest WAV 或长录音 spool 会留在本机，供下次启动恢复；
- recovery spool：失败或中断录音的 WAV/F32 分块。短录音在仅保存 History 或取消后的 History 写入期间会临时使用带 manifest 的恢复 WAV；History 成功写入后删除这份临时副本，若 History 写入失败则保留到下次启动恢复为可重试的 History 记录。长录音在 History-only / 取消后的写入失败时保留 recoverable session 目录供同一启动恢复流程重建；保留期限遵循 `keep_audio_days`；
- gold wav：仅在设置中主动打开「成功听写也保留音频」后，成功听写才会写入本机 `gold/`（默认关；配额 4 GB；满则停止新写，不挡听写）；
- usage：当天的请求数量和录音时长；
- `diagnostics/paste-delivery.jsonl`：仅记录粘贴失败、读回未确认或临时剪贴板写入后的取消诊断。字段包括阶段、目标 App bundle ID、Accessibility / 焦点验证、剪贴板 snapshot 与 ownership、键盘粘贴是否尝试 / 可能发送、读回验证和旧剪贴板恢复结果；不包含 transcript、窗口标题、URL、PID、密钥或原始系统错误。临时剪贴板写入后的取消会保留当前剪贴板并在 History 留存听写结果；若 marker 仍归 VoiceFlow 所有，保留听写文字，不恢复旧剪贴板；若所有权已变化或未知，则保留当前内容并让 History 成为恢复路径。原剪贴板只会在粘贴读回已验证且 marker 仍由 VoiceFlow 持有时恢复。取消前尚未写临时剪贴板不会产生此类记录。新记录写入时删除超过 30 天的行，并从最旧记录开始裁剪，使文件不超过 10 MiB；启动时也会执行保留期清理。文件夹 / 文件在 Unix 上限制为 `0700` / `0600`；
- settings：新建或替换的 API Key 只写入 macOS Keychain，并在读回验证成功后才清除旧来源；安全存储失败会报 `credential_storage` 错误，不会新建或覆盖明文 sidecar。升级留下的 `settings.json` / `secrets/` 明文密钥会在 Keychain 写入并验证成功前保留；读取失败不会当作密钥不存在，也不会用旧 sidecar 覆盖现有安全凭据；
- `models/`：用户主动下载的 On Device ASR 权重，按固定模型 revision 和文件 SHA-256 清单校验；新模型包含 Qwen3-ASR 0.6B、Qwen3-ASR 1.7B 和 Cohere Transcribe 2B。旧 SenseVoice Small 的 `models/sensevoice-small/` 文件也会保留，但没有 MLX 推理支持。MLX 模型需要 Apple Silicon 与 macOS 14 或更新版本。设置显示运行时能力握手和 loaded 状态；文件已校验不表示模型已加载或实际转写成功。VoiceFlow 不在启动时下载文件，不会自动安装 Ollama 或下载 `qwen3.5:4b`。清空全部 history / spool / gold **不会**删除模型。
- `models/audio-tmp/`：仅供本机 sidecar 使用的私有临时音频目录，与可恢复的 History / recovery spool 分开。每次音频处理结束、失败或取消后会清理对应临时文件；正常退出也会进入有界的 sidecar 终止和临时音频清理流程。

本机性能诊断只保存在当前进程的内存中，不写入上述文件。它通过 `get_latency_metrics` 返回有界的阶段延迟摘要、分组交付计数和固定错误 / 回退原因；最多保留 32 个 provider/model/path 分组，每阶段最多 128 个延迟样本。ASR 用量还记录实际 HTTP 请求数（包括队列重试）、失败请求数、可解码 WAV 的提交时长与对应请求数；Soniox 另记录流式尝试 / 失败数、session 墙钟时长、发送音频秒数和重放音频秒数。这些流式时长是诊断值，不等同于账单用量；provider 明确返回的最多 8 种固定用量单位和数量保持分开，不转换为费用。分组标签会过滤并截短。性能诊断不包含转录、音频、上下文、图片、目标 identity、URL、endpoint 或密钥。应用不会自动上传这些指标；只有用户主动选择“复制 JSON”时才会写入系统剪贴板，重启后样本清空。粘贴交付 JSONL 是独立的本机诊断记录，按上述 30 天 / 10 MiB 边界保留。

成功听写的 gold 音频默认不保存。打开后仍跳过密码框 / Secure Input，以及 1Password、HR、SSO 等预设关学习的目标。音频只留在本机，不会上传，也不会写进普通 History JSON 导出。删除单条 History 或清空全部数据时，对应 gold wav 一并删除。导出训练音频时附带识别草稿，不使用 AI 整理后的 `final_text`，也不会自动写入词典学习。

当前默认保留策略：

- recovery audio：7 天；
- gold audio（仅 opt-in）：沿用同一保留天数；训练建议至少 90 天或 1 年；
- history text：365 天；
- usage：按天存储，清空全部数据会一并删除。

History 数据库升级会创建应用自己的 `history.sqlite.vN.bak` 回滚副本，可能包含升级前的转写。它最多保留 7 天；历史文字保留期更短时按更短期限清理，选择永久保留文字也不会永久保留回滚副本。只处理应用数据目录内规范命名的普通备份文件，不递归或跟随符号链接；用户导出、外部备份、设置与模型不在这个清理范围内。清空全部数据会立即删除这些应用备份。

Recovery spool 的保留期从 manifest 记录的 session 创建时间计算，启动恢复写入不会重置计时。重建失败时，未过期的源分块仍会保留到原定到期时间；缺少、无效或位于未来的创建时间会被保守清理。

设置中的保留策略会在启动时执行，也会在修改历史保留时间后立即执行。历史文字可以选择“永久”，这表示不会自动清理，直到你手动删除单条记录或清空全部数据。

## 删除和导出

History 页面提供：

- 导出全部 history JSON（不含音频）；
- 导出保留的训练音频：wav + Qwen JSONL（识别草稿，不含整理结果）；
- 删除单条记录（含对应 gold wav）；
- 清空全部 history、应用创建的数据库迁移备份、recovery audio、gold wav、本地 usage 和 `diagnostics/paste-delivery.jsonl`（不含 `models/`）。

清空操作不可撤销。删除某个服务商的 API Key 是独立操作；若当前 ASR 依赖该密钥，设置会回到未完成状态，未选用的 Groq 密钥移除不会影响另一家服务商，已配置的本机 keyless 路径也会保留。本机模型要在语音服务里单独删除。

## 第三方服务和 telemetry

VoiceFlow 当前不收集 telemetry、广告标识或用户行为分析。网络请求只发往用户选择并配置的 provider；HTTP 429 最多重试两次，网络错误和 5xx 最多重试三次，`Retry-After` 最长等待 60 秒，401/403 不重试。ASR 单次 HTTP 请求最多等待 30 秒；取消会停止排队、重试等待或正在进行的请求。ASR、共同 LLM 整理（包括 Anthropic）及两条视觉请求均不跟随 HTTP 重定向；3xx 返回失败，不向另一个端点重发音频、文字、授权上下文或图像请求体。用户主动下载 MLX ASR 模型时，VoiceFlow 按固定 revision 访问所选模型的公开文件仓库，启动时不静默拉取。SenseVoice 是旧版下载项，归档来自 GitHub Releases（`k2-fsa/sherpa-onnx` 的 `asr-models`）；该旧路径不会因此改走 Groq。SenseVoice 的 [模型卡](https://huggingface.co/FunAudioLLM/SenseVoiceSmall)与[权重许可](https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE)仅适用于这条旧路径。用户需要分别遵守所配置 ASR 与整理服务商及所选模型的许可和服务条款。

正常退出会取消活动 dictation / 文字操作与云请求，停止模型加载并封住新的 sidecar 子进程创建，再通过有界等待关闭本机 sidecar 和清理 `models/audio-tmp/`。这项清理不删除已下载模型、设置、History 或仍在保留期内的恢复录音。

## 安全边界

VoiceFlow 的本地 SQLite 和 recovery audio 默认依赖 macOS 用户账户和应用数据目录的文件权限；应用数据目录、SQLite 文件和 recovery spool 文件会尽量使用 `0700`/`0600` 权限。默认不启用应用层加密，因而共享 macOS 用户账户或未加密备份仍可能暴露本地转录。

应用层 recovery spool 加密是显式 opt-in 的发布能力：构建时启用 `encrypted-spool` feature，并设置 `VOICEFLOW_ENCRYPT_SPOOL=1` 后，新写入的 recovery 音频和 gold wav 会使用 XChaCha20-Poly1305 加密，32-byte 密钥单独保存在 Keychain 的 `history-key` 项中。若 Keychain 不可用，VoiceFlow 会 fail-closed，不会把密文当作 WAV 发送，也不会退回写明文；旧的明文 recovery 文件仍可读取。History SQLite 当前仍未做应用层加密。

## 可选常开麦克风与本次跳过整理

默认按需打开输入设备。隐藏调试页的「常开麦克风」只有用户明确开启且已有麦克风权限时才运行；应用启动不为预热主动请求权限。它持续打开设备，空闲回调直接丢弃样本，不写 spool、不预取、不上传。录音开始后才绑定本次采集 worker，停止或取消后解除绑定；关闭选项、权限撤销和退出关闭空闲流。

「本次跳过 AI 整理」仍会调用所选 ASR，并做本地处理和词典替换。AssemblyAI 选择 raw Sync，共同整理模型（含 Ollama）不调用。这个会话标记不写入 History；History 重试使用当前设置。VAD 只过滤允许裁剪的最终 batch 音频，不撤回停止前已发送的实时或预取音频，也不删减完整音频恢复路径。

录音按键监听仅检查 Fn/组合快捷键所需的物理按下、松开和修饰状态；不读取字符、不保存输入文字。Fn 系统冲突通过键盘设置入口交给用户调整，VoiceFlow 不自动更改 macOS 键盘设置。
