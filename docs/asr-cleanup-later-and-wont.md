# ASR / Cleanup：以后再做，以及明确不做

更新：2026-09-26

来源：竞品与开源研究（Typeless / Wispr / Willow / MacWhisper / Daisy / yw-transcribe / VoiceInk / Handy / TypeWhisper）。质量标杆是 **Typeless 的中英混合听写 + 整理**。引擎插头、cleanup prompt、可选 Soniox 真流式、AssemblyAI Dictation / raw Sync 与 Qwen Audio 3.1 Message 原始 ASR 已接入；下列边界继续避免把默认听写扩成 Agent 或会议笔记。

旧分层见 [competitive-research.md](competitive-research.md)。管线研究快照见 [asr-cleanup-pipeline-research-2026-08-25.md](asr-cleanup-pipeline-research-2026-08-25.md)。隐私红线见 [privacy.md](privacy.md)。

---

## 研究过、故意留到后面

这些都值得做，但**不要**和已落地的 prompt / ASR provider 预设混在同一个 PR。单独立项。

### Daisy 控 Mac（打开应用 / 备忘录 / 日历）

Daisy 是语音 **Agent**，不是听写。参考：`forestai123456/Daisy-Voice-Agent` 的 `src/main/command/router.ts`、`src/main/control/macos.ts`。

以后若做：独立热键，文案写「命令」不写「听写」。本地正则先匹配（打开/退出/音量/站内搜索 + 中文别名），再用很小的工具集（Notes / Reminders / Calendar 的 AppleScript）。打字复用 VoiceFlow 的 AX 粘贴，不要抄 Daisy 的 `System Events keystroke`。

不要把「打开微信」做进默认听写松手路径。

### 真流式 ASR（Daisy / Willow / FluidVoice）

Daisy：豆包 WebSocket，100ms 帧，orb 上出 partial。Willow：边说边传，宣传约 200ms 出字。FluidVoice：本地流式草稿。

**已实现的可选路径：** Soniox `stt-rt-v5` 通过 `wss://stt-rt.soniox.com/transcribe-websocket` 真正按序流式接收麦克风音频。它仅用于 ASR，不提供 HTTP batch 接口，也不会启动同一音频的 batch prefetch。麦克风不等待 WebSocket 握手；有界队列满会使实时尝试失败，同时保留完整录音以供恢复。保存密钥不连接服务；真实生命周期只在用户开始听写时发起，目前真实麦克风行为尚未验证，因此不宣称延迟或准确率收益。

Soniox provisional token 只留在当前会话；它们不进入 cleanup、HUD 完成态、目标 App、剪贴板、History 或词典学习。最终 transcript 才进入既有 raw/final 流程。若初次流传输、音频覆盖或 finalization 失败，保留的完整裁剪前录音会按近实时节奏向同一 provider 重放一次，HUD 会显示完整录音重转写；不拼接未确认 partial、不跨 provider 回退，也不叠加 HTTP 重试。取消可中止恢复，处理 watchdog 有界且最长 30 分钟。重放可能再次产生整段费用，Soniox 计入整段实时流时长，包括静音；诊断 session 时长与发送音频量不是账单用量。服务协议见 [Soniox WebSocket API](https://soniox.com/docs/api-reference/stt/websocket-api)，价格见 [Soniox 定价页](https://soniox.com/pricing)。

**其他 HTTP provider：** 录音期间仍可后台 batch prefetch 完整分块；这不是 WebSocket 流式，预取文字不显示在 HUD，也不会写入目标 App / 剪贴板 / History。两条路径按当前选中的 ASR provider 分开使用。

### 用户可见的整理策略

当前设置提供 Auto、Off、Light、Standard、Heavy，并允许按 App 设置继承、Auto 或显式档位。Auto 对聊天和 coding prompt 使用 Light，对邮件 / 文档使用 Standard，对代码、终端和表单采取本地保守策略；已有显式 Off / 强度会保留。这个路由不宣称模型质量提升，也不复制竞品档位名称。

### 改正即学词典（循环已在；缺打中率）

Wispr：改正一次、≤4 词、分类器过滤。Willow：中文人名/声调记入个人词典。TypeWhisper：按目标 App 纠错学习。Open Typeless Harness：粘贴后短窗口回读。

**已上线：** History 确认、同框观察、3× 晋升、最长优先替换、undo/tombstone、按 App 关学习。只观察自己刚写入的框，禁止全局击键。

**已落地（harness，不是模型）：** post-LLM 再替换、拼音/谐音召回、分类器后的人名 2×、1Password/HR 预设关学习、Whisper 虚构抄本。没有自有模型时只能靠这些，不能靠 LoRA / 微调 / 换「我们的引擎」。见 [harness-deep-research-2026-08-30.md](harness-deep-research-2026-08-30.md)。

不要重做观察循环。下一刀如果要做，是打中率和 review 窗，不是再写一套学习。

### TypeWhisper 按网站 workflow

TypeWhisper：Chrome + github.com 可以盖过「所有 Chrome」。我们已有 bundle / host mapping。

**已实现的窄匹配：** 现有 mapping 可按 host 和路径前缀匹配网站，且与已填写的 App / executable / focused-field 条件做 AND 组合。它不会覆盖每个 workflow 的语言、引擎或 prompt。

以后若需要：每个 workflow 单独覆盖语言、引擎和 prompt；继续复用现有 provider 与 mapping，不新建 workflow 引擎。

### 默认改成按住说话

**已上线：** 设置里提供独立的 `tap`（点按切换）与 `hold_to_talk`（按住说话），Fn 单键和合法组合键都可使用。更换快捷键只修改绑定，保留已选录音方式。

新安装默认 tap，默认绑定保持 `⌘⌥Space`。schema 25 将旧 `double_tap`、`hybrid` 和历史 `hold` 统一迁为 tap；旧单独 ⌘/⌥/⌃/⇧ 保留显示并暂停注册。独立翻译和跳过整理热键沿用主录音方式，不将默认改成纯按住录音。

### Volcengine 正式接入

Daisy 流式 `bigmodel` + `enable_itn` / `enable_punc`。yw-transcribe 文件识别 Seed ASR 2.0（`volc.seedasr.auc`，Speech Key ≠ Ark Key）。

**已上线：** SiliconFlow SenseVoice 与 Qwen3-ASR 文件转写预设。不把豆包 WS/submit-query 做成一等 provider。

### 500ms 预滚

Daisy 本地 Whisper：VAD 前保留 500ms，避免吃掉句首。以后若做本地 VAD 停录再加。当前 batch「松手再传」不需要。

### 其它以后（研究里提过、仍不本轮）

- 更广泛的 Willow Scribe 式 Agent 动作（独立选中文本热键已支持下文所列的有限文字操作；联网搜索、执行工具或跨 App 操作仍不在范围内）
- Handy Secure Input 提示
- yw-transcribe 式 `raw.json` 不可变证据包（听写产品不需要字幕包）

---

## 明确不做

不要立项，不要「顺便做一点」。

| 不做 | 为什么 | 常见伪装 |
|---|---|---|
| Typeless / Wispr **Ask Anything** 联网动作 | 听写会变成 Agent，延迟和安全模型都变 | 「让 LLM 搜一下再贴」 |
| **未授权的默认听写截图 / 窗口文字进 provider**（Wispr `screenshot` / 松手整理） | 默认听写仍不捕获图像；AX、OCR、云端视觉文字各自需要具体 App 规则权限 | 「开了 context 就默认读窗口 / 上传图片」 |
| **会议录音** / 日历自动开录（MacWhisper / Wispr Notetaker） | 另一条产品，不是松手粘贴 | 「顺手录 Zoom」 |
| Daisy **`run_shell_command`** | 任意命令 = 不可接受的桌面权限 | 「工具失败就用 shell 兜底」 |
| 在 Tauri 里 **嵌 Python FunASR** | 体积、崩溃、模型下载都变成我们的运行时 | 「本机中文准就要带 Python」 |
| 用用户录音 **微调 ASR 权重** | 隐私；不是词典学习 | 「越用越准」若指出租权重 |
| **全局击键** / 密码框 / Secure Input 里学习 | 键盘记录 | 「粘贴后看用户怎么改」若变成全局 listen |
| 未经明确批准把用户风格样例发到 provider | 保留样例不等于批准；每条 mapping 有独立 `style_examples_approved`，自动观察先进入待确认草稿 | 「开了文字上下文就把所有风格例子一起发送」 |
| 聊天默认 **屏蔽脏话** 或默认表情包 | 中文用户会觉得假 | 「最文明的输入法」 |
| 抄 **GPLv3** 源码（VoiceInk / TypeWhisper / FluidVoice） | 许可证 | 「把他们的 prompt 文件贴进来」 |

Daisy 的 Notes / Calendar AppleScript 可以以后做；**不要**带着 `run_shell_command`、Firecrawl、Control Center 刮「勿扰」一起做。

**图像边界（不是默认听写）：** 默认听写不截屏。独立 `screen_action_hotkey` 在用户已配置 vision model 后可发一张当前窗口图并先预览。另有逐 App opt-in 自动回退：匹配的具体 App / executable 规则须同时允许 cloud vision 与 provider text use；AX/OCR 内容不足且 configured vision capability、Screen Recording 权限可用时，只使用一张当前窗口图。`local_ocr` 单独只控制本机识别，OCR 派生文字仍需 `context_text_to_providers` 才能进入 ASR / cleanup 请求。目标变了只复制；取消立刻丢掉内存 PNG。会议录音、Spark、生图仍然不做。

---

## 当前 ASR / Cleanup 行为

计划已收窄并写进代码。场景按明确的焦点字段分类：微信等聊天 composer 的短句可走 Light cleanup；邮件 / 文档使用 Standard；Search、Code、Terminal、Form 与 Secure 默认 Off。应用名称或页面标题不会覆盖模糊焦点字段的保守处理。

ASR provider 预设按 endpoint、model 与实际协议匹配，不把每个 endpoint 都当作相同的 OpenAI Whisper 请求。默认 provider/model 仍为 Groq `whisper-large-v3-turbo`；SiliconFlow 的默认仍是 `FunAudioLLM/SenseVoiceSmall`。已有 provider/model 与自定义 endpoint/model 不会因新增预设被覆盖。

- 可选的一等本机 ASR provider 使用 `qwen3-asr-0.6b`（新选择默认）、`qwen3-asr-1.7b` 或 `cohere-transcribe-2b`。MLX 推理要求 Apple Silicon 与 macOS 14 或更新版本；模型由用户在语音服务设置或引导页显式下载，VoiceFlow 不会启动时下载。SenseVoice Small 只保留旧配置和文件，不提供 MLX 推理；即使旧文件完整，也不能通过 MLX 推理探测。Cohere 模型要求固定选择中文或 English，不支持自动语言检测。
- 严格离线模式是单独的设置：ASR 只允许 On Device；所有 HTTP ASR 路由（包括配置为 loopback 的 LocalWhisper / 自定义 endpoint）及这些路线的 provider probe 都被阻止，On Device 本机设置探测仍可用。该模式也阻止云端 cleanup / 视觉请求与 History 重试等云请求路径，并取消进行中的云请求，保留原先选中的 provider 配置。本机模型下载仍是需要网络的独立管理操作。cleanup 可显式选用经验证的 loopback Ollama 上的 `qwen3.5:4b`；VoiceFlow 不安装 Ollama 或下载该模型。状态不可用时使用本地规则或关闭 AI 整理，不切换到云端。
- On Device 的文件校验、sidecar capability handshake、当前模型 loading 和已加载模型分别显示。loading 状态绑定模型 ID，可由用户取消；取消停止本次加载但保留 provider/model 选择与下载文件。能力握手与 `inference_ready` 只表示运行时可用性，不表示模型已加载或一次实际识别成功；产品质量与延迟仍需单独评估。
- SiliconFlow 另有可选 `Qwen/Qwen3-ASR-1.7B`；新增 provider ID `fireworks` 使用 `whisper-v3-turbo`，新增 provider ID `mistral` 使用 `voxtral-mini-2602`。Fireworks 可用性尚未核实；Qwen 预设的账户与地区可用性尚未核实。本机 Whisper HTTP、本机 FunASR 和用户自定义 endpoint/model 保留。
- 新增可选 Soniox `stt-rt-v5` WebSocket 路径，只在 ASR 选择中；自动语言模式发送 `zh`/`en` 候选提示，固定模式发送相应单一提示，不限制模型识别；不发送 custom context/terms。费用按完整实时流时长计算。保存设置只写 Keychain 并显示 configured，不发起服务请求；实际连接发生在用户开始听写或明确执行 provider 测试时，当前未通过真实录音验证。
- 新增可选 AssemblyAI provider `assemblyai`，固定使用 Universal-3.5 Pro 与一把 Keychain key。总录音不超过 120 秒且当前允许 AI 整理时走 Dictation；通过保护检查的短 Dictation 候选本身可提供整理，不需要共同 cleanup 凭据。AI Off、本地-only 或长录音走同公司的 raw Sync，长录音完整覆盖分段后最多做一次全稿 cleanup；这一路径的模型整理需要共同 cleanup 凭据，缺失或不可用时保留 raw / 本地准备稿并用本地规则回退。设置保存时的 ASR 探测只检查该 AssemblyAI ASR 路径，不验证可选共同 cleanup 服务。Dictation `text` 保留原稿，`llm_response` 是候选，`llm_error` 标记整理失败；候选通过现有本机准备稿最终保护检查后采用并跳过第二次 LLM。候选缺失、为空或 `llm_error` 时，有共同整理凭据才对准备稿执行一次共同 cleanup，否则使用本地规则；最终保护检查拒绝候选时保留受保护原稿 / 本地回退，不再请求第二个模型。Dictation 和 Sync 的 `auto` 都显式发送 `language_codes` `zh` + `en`；固定中文 / English 发送对应单项。Sync 用 `keyterms_prompt` 附带最多 100 项 / 8000 字符术语，不用会覆盖语言字段的通用 prompt。Dictation 不支持关闭 provider 改写的 `cleanup=false`。保存设置时的 AssemblyAI ASR 探测不验证 cleanup credential 或 fallback；探测通过也不是识别质量或 E2E 验收结果。
- 新增可选 DashScope provider `dashscope`，仅接入精确模型 `qwen-audio-3.1-asr-flash-message` 原始 ASR。用户选择 Beijing 或 Singapore，host 与模型一并保存在设置；使用 Message WebSocket 的 `disfluency_removal_enabled:false`，不启用原生 polish。该协议自动多语言识别，没有查证到 `language_hints` 请求字段；模型文档的 7168 input / 1024 output token 限额使全长单请求有风险，因此较长录音使用保守分段并完整覆盖。普通 HTTP `qwen-audio-3.1-asr-flash` 不属于此接线，也未被标为支持 raw 开关。时间戳仅在服务返回时按毫秒使用，最终累计 usage 只记录一次；账户、地区权限、质量与延迟未在实现阶段验证。
- HTTP ASR 上传 16 kHz、单声道、16-bit PCM WAV。VoiceFlow 单次直接 ASR 请求最多 10 分钟；更长的录音分段后完整覆盖，不截断；单次录音最多 15 分钟。默认分段阈值 25 秒（5–3600 秒），目标段长 35 秒（15–60 秒）。SiliconFlow adapter 的上限为 1 小时 / 50 MB，Mistral 文档上限为 3 小时；Fireworks 此接线不声明 provider 自身的文件或时长上限，10 分钟是应用请求上限。
- SiliconFlow SenseVoice / Qwen 使用 multipart `file` 与 `model`，自动检测语言；不发送 language、context/terms，也不请求 `verbose_json`、时间戳或置信度。Fireworks Whisper 使用 multipart `file` 与 `model`，自动检测，不请求术语、时间戳或置信度。Mistral 使用 multipart `file` 与 `model`，自动检测时省略 language，也支持单一固定语言；最多 100 个词条通过 `context_bias` 发送，自动检测时请求片段时间戳，固定语言时不请求时间戳；不发送 Whisper `prompt`，也不使用置信度。Deepgram preset 不启用 `multi`，不据此宣称支持中文或中英混合。
- ASR 失败不会静默换到另一家 provider；用户另行配置的准确识别级联只会请求其指定 provider。HTTP ASR 等待上限 30 秒；429 最多重试两次，网络错误 / 5xx 最多重试三次，`Retry-After` 最多等待 60 秒；401/403 不重试。取消会停止排队、退避等待和正在进行的请求；ASR HTTP 不跟随重定向。History 恢复音频若超过 10 分钟，会完整分段重识别。
- 本机性能诊断在当前进程保留，最多 32 个 provider/model/path 分组、每阶段 128 个延迟样本。ASR 用量含实际 HTTP 请求与失败数（队列重试也计入）、提交音频秒数、可解码时长的请求数；Soniox 另记流尝试 / 失败、session 墙钟时间、发送音频秒数与重放音频秒数，和 provider 返回的最多 8 种固定用量单位 / 数量分开保存；不用来推算费用，也不包含转录、音频、上下文、图片、endpoint、URL 或密钥。
- 普通与长录音使用统一的本地准备顺序：语音标点与有限布局解析、已晋升词典和谐音对、局部明确改口，然后把同一份准备稿交给 cleanup provider 与最终保护检查。对于当前场景 family 匹配或未限定 family、且没有 mapping / host / app 限定的已晋升词对，最终 Cleanup guard 只在准备稿确实含有 `after` 而没有 `before` 时保护这次匹配；provider 若重新引入对应 `before` 或删除该来源术语，候选会被拒绝并保留准备稿。它不把其他词对加入全局黑名单，选中文本等非 Cleanup 操作也不使用这条限制。词典不再在 provider 返回后对所有出现位置全局重写。
- 局部改口只在同一短语内找到相同类型、可识别且非重叠的日期、金额、版本或技术 / 专名时应用；授权保存原值、目标值和双方在准备稿中的字节范围。未知、类型不匹配、引用中或否定语境不自动改写。其余实体继续由 ProtectedSpan 与否定保护检查。
- 明确的段落、三点列表、标题 / 正文和已识别列表字段会转成有界布局约束；普通词语和引号内内容不会被当作命令。候选若删掉或重排结构化行的内容，或改变代码缩进 / 预格式化布局，会回退到准备稿。
- `CleanupDecision::Disabled` 在清理阶段不发起整理请求；此前按用户授权完成的口述布局 / 修订、词典和语音片段处理仍然有效。Provider 失败或布局 / 实体校验拒绝时保留可恢复准备稿及其布局。
- 独立选中文本热键支持六种明确操作：改写、精简、翻译、结构整理、空回复框起草回复、按指令修改一个值或术语。它单独捕获选区、受支持字段全文或空回复框（来源上限 16 KiB），并先显示可编辑、一次性事务预览；确认前会复核目标和来源，失配或不能验证时只复制。无法支持的指令或保护条件安全失败。成功操作的来源、指令和预览不会写入普通 History / 纠错学习；如果选中文本或看屏幕操作的语音 ASR 失败，完整录音按恢复策略保存在 History，之后可用完整覆盖 chunked retry，结果只复制。该功能不执行命令。看屏幕仍由另一个明确热键触发。
- 这不是语义事实证明：操作保护基于有限实体、否定标记与少数可识别翻译等价项；翻译里未知的日期 / 金额变化以及回复草稿中无法验证的陈述会拒绝预览。起草回复仅使用空回复框和当前 App 权限允许的有界附近 AX 文字。
- Auto 在 Chat / coding prompt 使用 Light，Email / Document 使用 Standard，Code / Terminal / Form / Secure 使用 Off；mapping 的显式设置优先于全局强度。
- 用户选择的全局与 mapping 强度继续生效：Light 做窄幅清理，Standard 使用当前场景策略，Heavy 才附加润色要求；结构化来源的内容与布局约束在三种强度下都适用。
- 窄 tagged prompt + `local_cleanup` 可去掉独立口头填充词「那个/就是说」；最终 Cleanup guard 会保留「那个项目」等带名词的指示引用线索。
- 口述 冒号/分号/破折号
- 默认 cleanup 模型仍是 Groq `openai/gpt-oss-20b`；因 Groq 在 2026-08-16 对 free / Developer 账户停用 Llama 3.1 8B 与 Llama 3.3 70B 而作出的服务可用性调整，不表示质量提升。GPT-OSS 20B / 120B 请求使用 Groq 支持的 low reasoning effort；OpenAI 的 GPT-6 Luna 可选并使用 Chat Completions 支持的 `none` effort。OpenAI 默认仍是 GPT-4o mini。已保存的旧模型 ID 仍保留给已有配置。[Groq 模型停用说明](https://console.groq.com/docs/deprecations) · [GPT-6 Luna API](https://developers.openai.com/api/docs/models/gpt-6-luna)
- 分组离线 harness 通过真实准备、路由、fallback 和最终 guard 执行 160 个合成 / 脱敏文本场景；live provider 结果单独记录，文本语料不会被描述为 ASR 准确率。

**评测边界：**

- Rust / frontend 单测和 160-case 离线语料检查生产路径上的确定性保护规则、路由与 fallback；它们不证明听得准或服务端质量。
- 可选 live harness 通过真实 provider adapter 使用合成文本 / WAV，报告 provider 调用、失败分类、CER/WER 和最终 fallback 结果。账号权限、HTTP 429 或缺少凭证会限制覆盖；成功的 HTTP 调用不等于准确度证据。
- `npm run eval:audio` 提供显式审核过的外部 WAV → 实际 ASR 输出 → 整理评测；默认只校验，来源、参考稿、模型和凭据须明确指定，`--run` 才上传。它不读取麦克风或 History，不自动改变模型默认值；入口和单测不代替真实人声或原生交付验收。
- 合成 WAV 来自 macOS TTS，不代表真实人声、麦克风或硬件表现。任何模型默认变更必须有同音频配对结果和独立审查；本次 GPT-OSS 20B 只是停用模型的可用性默认修复，不是质量升级。

**已实现的窄 opt-in 上下文边界：** mapping UI 可按 App / executable、网站 host / path 和 focused field 组合规则；AX 文字、本机 OCR、云端视觉及向 provider 使用识别文字分开授权。云端自动图像要求具体 App / executable 规则且只作为 AX / OCR 不足时的一图回退。默认听写仍不捕获图像或窗口内容；raw URL、标题、PID 和目标 identity 不进入 provider / History。

未做、且不要混进下一刀：未获准 App 的窗口内容进入 provider、更广泛的自动窗口扫描、Volcengine、`evals/` live harness 当合并门槛。

若要开「以后」清单，一次只开一项，并另写计划。
