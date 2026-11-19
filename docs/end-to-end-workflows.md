# VoiceFlow End-to-End Workflows

## 语气试跑与会话翻译

「编辑哪种语气」只选择编辑对象，不切换当前听写语气；试跑标明使用当前草稿或已保存配置。设置里的语气选择与单个手动试跑预览位于 Prompt 折叠区外；展开或收起 Prompt 不会重新挂载预览、触发试跑或取消正在进行的请求。普通折叠保留草稿及现有失焦提交，错误所在分组自动展开；快捷键录制所在分组从准备暂停热键到恢复完成期间不能收起。

后台设置快照按语气 id 和内容同步；相同内容不会重置编辑器。正在编辑的未保存 Prompt、名称和新建语气保留，其余语气接收更新；明确放弃草稿后使用最新已保存配置。错误在所属操作旁显示；试跑不可用时说明需要补全的字段，取消试跑后焦点返回试跑按钮。等待暂停热键期间离开页面，迟到的暂停完成会恢复全局热键，不进入录制或保存。

设置里的语气试跑是文字样例测试，不启动录音。请求携带当前语气草稿、用户输入文本和可选的已保存语气比较标记；样例最多 16 KiB，Prompt 最多 8000 字符。后端为该语气 family 构造场景，复用本机准备、整理路由、配置的 provider adapter、请求队列和最终保护检查。它清除测试快照里的片段和 App 映射，不获取真实窗口、上下文 grant 或风格示例；Prompt / Code 试跑使用 CodingPrompt，Terminal / Form / Search 使用对应字段类型。输出模式固定为自动整理以测语气，而 AI Off、强度和严格离线边界保留。试跑与听写共享服务限额，限流 / 重试事件独立，不改变录音 HUD。结果区分模型、本地-only、provider 回退和 guard 回退，包含试跑耗时；比较按已保存、草稿顺序进行，不将单次差异当作质量证据。它只返回内存结果，不进入交付、History 或学习。请求 ID 绑定取消；旧请求的取消 / 完成不能清除新试跑，取消早于请求启动时也不会重新发起。配置变更会废弃结果，90 秒到期终止试跑。本机 provider 错误仅在状态、准入阶段、发送标记和边界计数一致时显示为已确认的发送前容量拒绝；未知或矛盾字段不会被解释成已发送或未发送。

`translation_hotkey` 是可选独立录音快捷键（Fn 或组合键），旧配置和新安装默认空值。注册、按键录制、冲突校验与失败恢复都包括这条热键；使用全局 tap / hold_to_talk / hybrid_hold_lock 录音方式，设置它不会修改全局按法。开始成功后，将 `translation_target_language` 固定在 DictationManager / StopClaim，分别应用到录音预取和停止处理的 settings 副本，只将本次 `output_mode` 覆盖为 translation。全局 `output_mode=translation` 也在开始时捕获目标；录音中的目标语言设置只影响下一次会话，不会替换当前会话目标。结束、取消、启动失败或下一次普通听写会清除临时值；按住与锁定模式只有同一来源、同一按压 ID、同一会话的 release 才能结束；其他热键和过期释放不会停止它。开始前检查实际场景的整理路由和凭据：云端 provider 需要可用密钥，On Device MLX、VoiceFlow llama.cpp 和 loopback Ollama 可无密钥运行；场景、离线策略或路由不满足时不采集音频。主 HUD 在录音时显示实际设备与计时，在处理时显示统一处理状态；翻译目标保留为会话辅助功能 / 详情元数据，不显示在主胶囊中。结果按现有交付状态处理，不把目标标签当作翻译成功证据。正常听写的截图边界、目标复核和 History 恢复策略保持生效。

翻译输出模式将完整的本机准备稿声明为明确的 `Translate` intent，并携带固定目标语言；正文不必加「翻译成……」前缀，正文里提及其他语言也不会切换本次目标。轻量、标准和重度整理都传递该授权，provider 与最终保护检查使用同一操作。AssemblyAI 翻译听写走 raw Sync ASR，再通过当前获准且可用的共同整理路由翻译完整准备稿，不使用 fused Dictation 候选；只有所选 provider 路线要求 API 凭据时才需要有效凭据，keyless 本机路线无需 API key。AI Off、保守场景和严格离线仍按现有策略限制路由。History 重试使用当前处理设置，临时热键模式不写入 History。

## 首次使用、自检和学习反馈

如果 Groq 或其它必需密钥的云端 provider 因 macOS Keychain 授权暂时不可读，用户可在前台 Settings 的相应 provider 行明确点击「从钥匙串重新读取」。后端只读取当前应用固定的 provider Keychain 项；成功时只把值放进当前进程内存，Settings 不接收完整密钥、不保存设置或凭据，也不连接 provider。授权窗口可能让主窗口短暂失焦；最多容许 150 ms 的本进程前台状态 settle，之后只允许 `com.apple.SecurityAgent` 持续最多 10 秒。授权后须在 2 秒内返回同一个已聚焦主窗口，且 VoiceFlow 必须仍是前台进程；离开 API Key 页面、切换到其他应用、听写 / 操作开始、设置变化或超时都会取消读取。重启后仍可能需要再次授权；取消、不可读或缺少项目不会更改现有设置。Custom 仍使用绑定 origin 和路由用途的专属恢复路径。启动与普通后台读取保持非交互。

主引导只有欢迎、权限、连接服务、听写试用、完成五步；完成页可主动进入选中文本试用。未进入可选试用时，结束引导不写入 `selected_actions_enabled` / `selected_action_hotkey`。连接服务默认显示 Groq 与 On Device 两条路线，服务商 / 模型 / 区域收在高级配置中；已有非默认服务商自动展开。On Device 在开始试用与最终保存时明确写入 `cleanup_enabled=false`，不把本机转写静默接到默认云端整理；运行时与 Cohere 语言检查仍是继续条件。

连接服务会结合明确识别语言与系统 locale 的显式地区码显示本机 / 多语言云端路线建议；只有选中自动检测时才用地区单独推荐，固定语言若与已知建议路线不兼容则不显示地区建议。建议不会自动改动设置，只有用户点击建议按钮才更新引导草稿；选择仅支持自动检测的本机模型时，引导会把识别语言切换为自动。可选模型未就绪或当前设备不支持时，本机建议禁用；严格离线模式始终禁用 Soniox 云端建议。模型速度和识别质量在此设备尚未实测。

引擎保存与探测分开：服务商、端点、模型、凭据和首次设置完成仍检查所需凭据、host 绑定与本机模型状态，并执行既有 Keychain / 热键事务；主题、HUD 外观、输入设备、增益和快捷键等非服务偏好只做结构验证，不要求服务就绪，也不读取、写入或删除未更改的凭据。首次 Finish 或跳过试用不发起合成音频或整理文本请求；服务可用性保留为未测试。用户主动 Probe 和试用才可能发送对应请求。`onboarded=true` 只代表完成设置，不代表转写或外部插入通过。麦克风自检读取已保存的输入设备、增益和合盖麦克风配置；其中任一更改仍在保存中或保存失败时，设置页标明未提交并禁用自检，只有保存确认后才可测试。

应用内试用以 `set_onboarding_test_mode(enabled, trialId)` 获取原生租约，返回 UUID `trial_id` 与后端 `config_generation` 十进制字符串。录音开始保存不可变试用快照；停止声明保留独立 `recording_generation`，处理 / 状态仍使用既有 `session_generation`。开始、采集、处理、错误和结果带同一试用身份、配置版本、有效性与 `trigger_source`；`native_hotkey` 只证明 native 回调接收，实体按键另需人工验证。选中文本由带 `trialId` 的命令绑定，过期或未选择时拒绝，不改读外部选区。释放必须匹配 trial ID；旧 React 清理不能撤销新试用。选中文本操作在接受声明时、任何异步准备之前固定试用与录音身份；读取试用选区前按同一租约和录音版本核对，旧操作不能消费新试用的文字或把新录音身份写到旧错误上。试用样例通过本机指纹和活动操作校验，普通选中文本继续复核外部原生目标与选区。只有持有声明身份的事件能携带试用事实；通用 HUD 状态不借用全局当前试用身份。试用在任何普通剪贴板 / History-only / paste 路由之前返回预览，撤销后仍保持预览路由，迟到结果不改成外部交付。成功试用不写普通 History 或启动纠错学习；真实外部交付必须单独确认。

受控外部插入测试和生产焦点检查共用一个 PID 绑定的原生 Accessibility 捕获：同一焦点元素提供瞬态角色、窗口几何和 `CFEqual` 焦点身份，窗口号只在 CoreGraphics 候选唯一时记录。候选有歧义时，只有窗口标题和几何指纹仍匹配才继续粘贴；标题为空或指纹变化、缺失时退回复制和 History。两个窗口标题与几何都相同时仍无法识别切换。粘贴后的同字段回读不受此歧义规则影响。捕获前后复核前台应用身份。窗口标题等原始元数据仅在进程内用于匹配和生成本机指纹；receipt 只返回有限的捕获阶段 / 原因枚举，不返回目标文字、标题或指纹。捕获或目标检查失败且尚未进入插入交付流程时，剪贴板恢复明确为 `not_needed`；进入交付流程只证明该 owner 已开始执行，不证明按键已经发送。只有固定目标的同字段回读确认精确一次插入才报告 `exact_once`。辅助功能状态无法确认时，receipt 保留未知观察原因，不推断具体权限或系统故障。

自动粘贴提示可通过 `show_permission_guide(permission, guideId)` 打开 macOS 原生伴随窗，`permission` 仅接受 `accessibility` 或用户主动选择的 `screen`。当前 App 卡片使用 NSDraggingSession 与规范 NSURL 文件，不使用 HTML 图片拖拽或固定安装路径。原生窗口按当前中英 / 系统语言显示，可移动 / 调整大小，提供系统设置、Finder、重新检测和关闭。唯一系统设置窗口的几何元数据可确认且屏幕有空间时相邻摆放，否则保持独立可移动提示；不读取系统控件或截图来定位。权限仅以本机读回判断。无有效 bundle、签名未知或系统页不接受拖拽时提供说明与添加 / Open 替代路径。`close_permission_guide(guideId)` 只关闭匹配提示；主窗口关闭和进程退出也清理伴随窗。普通语音引导不触发屏幕权限。

麦克风自检通过 `start_microphone_check` / `stop_microphone_check` 主动开启，要求已有麦克风权限、空闲操作状态，以及输入设备、增益和合盖选择均已保存；未提交或保存失败的选择会标为未保存并禁用自检，避免用旧后端设置测试新标签。自检不发起隐式授权或服务请求。音频线程串行管理自检、录音和常开输入流。自检按保存的输入设备、合盖选择和增益打开实际 CPAL 输入，仅测量单声道音量、累计峰值与是否收到帧 / 信号，不缓存或编码样本。自检最多 30 秒，停止、离开设置、窗口隐藏或失去焦点、设备 / 增益改变或正式听写时关闭；正式录音优先，自检不能关闭新录音。结束后恢复原先获准的常开空闲流。自检事件独立于 HUD。会话 ID 过滤迟到结果和取消，取消先于启动也不会复活采集。开始命令五秒未返回时会发送取消。

词典反馈复用待确认与生效替换记录；实际收益由生产替换器记录它确实选择并应用的目标词，不把提示词命中或已正确拼写的文字算成替换。每个本机处理过程按目标词去重，存在 `learned_term_usage` 的次数 / 最近时间中；History 重新整理也可能计入。History schema v13 新建此表，不回填旧记录。统计发生在本机准备阶段，不能证明最终候选被接受、交付成功或识别质量提升。保守场景与无数据目录的语气试跑不写入记录。仅展示仍有生效学习规则的词条，忽略 / 忘记后隐藏；清空全部数据删除记录。读取失败保留已有数据并提供重试，不把未知统计冒充零。

完成页保存期间禁用可选试用入口，保存与读取设置都结束后才允许导航。词典页在听写完成后刷新学习反馈，覆盖粘贴确认、复制、仅历史、未确认和降级交付。

## 录音快捷键与设置事务

录音方式为 `tap`、`hold_to_talk` 和 `hybrid_hold_lock`，新安装默认 tap，默认绑定保持 `CmdOrControl+Alt+Space`。Fn/🌐 在点按模式下只在无其他按键的完整按下/松开后切换；组合快捷键在按下时切换，直到松开前忽略重复事件。按住模式在按下时开始，松开结束；混合模式按下即开始，按住至少 220ms 后松开结束，短按在 320ms 内再次按下则锁定免提录音，之后一次干净按下结束。只有单次短按时，双击窗口到期会取消该会话；在第一次短按后加入组合键、或锁定确认时加入组合键，会取消该次会话，锁定后的结束按键若变为组合键则不结束锁定录音。Fn 组合操作不会触发点按；按住时加入其他键会取消该次会话。监听只检查必要的物理按键和修饰状态，不读取或保留输入文字。Fn 需要 Input Monitoring；普通组合键缺少该权限时沿用 Carbon 释放通知，可能要等整组按键松开才结束。

自定义服务设置使用一个共享地址和 API Key，并为语音转写、AI 整理分别保存模型；只有两阶段都选择自定义服务时才将其显示为共享连接。远程服务需要对应服务或 Router 的 API Key；仅 loopback 本机服务允许留空。更换服务 origin（协议、主机或有效端口）后不会沿用旧 Custom 凭据；仅更改同一 origin 下的路径不会解除绑定。阶段测试只探测草稿中的对应阶段，不保存 key、修改设置或切换当前路线；「保存并使用」单独应用草稿。AI 整理的高级请求参数是默认留空的、有界 JSON 对象，只对自定义 OpenAI-compatible cleanup 生效，并进入日常短 / 长整理、History 重试 / 重新整理、写作预览及对应阶段的测试请求。空对象保持现有请求默认值；不能覆盖 VoiceFlow 管理的模型、消息、流式协议、认证、工具或输出形状。结构、类型、已知参数范围和凭据字段会在前端与后端校验；未知兼容字段在限制内原样保留，用户不得在值中放入密钥或其他秘密。非自定义路线和转写请求不使用这些参数。

Router 配置示例：在自定义服务地址输入 `https://router.example/llm/v1`，在 API Key 输入 Router 提供的密钥；转写模型填 `groq/whisper-large-v3-turbo`，AI 整理模型填 `groq/openai/gpt-oss-20b`。VoiceFlow 会在该地址前缀下分别请求 `/audio/transcriptions` 与 `/chat/completions`。语音转写使用 multipart `file`、完整模型 ID、`verbose_json` 和 word / segment 时间戳；固定语种会发送对应受模型支持的 BCP-47 `language` 参数，自动检测会省略该字段（若 provider 支持常用语种提示则会附带 `preferred_languages`）。整理高级 JSON 只影响 `/chat/completions` 请求，不能填入 API Key 或其他秘密。这里的 `router.example` 是占位地址，需替换成你的 Router 服务地址。

原生输入通过 FIFO 队列先领取状态转换，再执行麦克风或转写工作。每次按压都有唯一标识；启动麦克风时收到松手或第二次点按会记住停止请求，启动完成后领取停止。空音频回到空闲，停止和处理期间忽略新录音手势。Esc 仍走通用取消流程；休眠、合盖休眠和监听中断清空按键状态并废弃排队输入，只取消 Starting / Recording，会话处于 Idle、Stopping 或 Processing 时不取消当前工作。中断时当前仍按下的键须先松开才能重新触发。翻译和跳过整理沿用同一录音方式，选中文本及看屏幕保持各自一次性流程。

schema 25 将旧 `double_tap`、`hybrid`、历史 `hold` 迁为 tap，保留合法绑定。旧单独 ⌘/⌥/⌃/⇧ 不被替换，保留显示并暂停注册，用户可改成 Fn 或组合键。捕获只提交绑定和目标，模式选择独立保存。录音或处理期间禁止修改。绑定事务先注册、再持久化、最后更新页面；失败恢复旧绑定，取消或完成捕获等待物理按键释放后恢复监听。仅绑定、模式或更新说明的修改使用结构校验，迁移也可在未读取到密钥时保存；服务修改、完成引导和开始录音仍要求相应就绪检查。

设置和引导共用就地快捷键编辑器：点击紧凑按键标签后，显示当前绑定、只读录入框、取消及适用的 Fn / 清除操作；主听写还可显式恢复默认 ⌘⌥Space。键盘捕获限于录入框，Tab / Shift+Tab 可导航；捕获期间的 Esc（含修饰键仍按下时）、点击外部、焦点离开该组、窗口失焦或隐藏均取消。WebKit 点击按钮时可能没有失焦目标，不能据此取消组内 Fn / 取消操作；外部点击与窗口失焦仍独立取消。物理键码避免 Option 字符与 Shift 标点变成另一枚键。新绑定要求 ⌘/⌥/⌃ 之一与其他键组合，或功能键；录音的 Fn 使用显式按钮，已有合法的普通单键 / Shift 绑定不被改写。重复事件忽略，WebView 在收到释放事件后将候选交给原生；WebKit 可能漏发普通键释放或保留旧修饰状态，因此原生等待所有物理按键释放后才注册、保存并恢复监听。准备、保存、取消各有进度；页面值只在原生确认后更新，错误在原位说明并可重试。全局捕获队列包含离开页面的未完成暂停或恢复，旧请求不能恢复下一次编辑的监听；尚未执行的暂停若已取消则跳过。明确取消可返回标签焦点，但外部操作后完成不会抢回焦点。

听写 HUD 收到设置发出的 UI 语言事件后更新译文而保留当前窗口与子组件状态；语言切换不重建胶囊。partial、progress、录音上限和限额事件只有通过各自的状态、代次与选中操作准入后，才会接管新 session generation 并清除旧会话的音频 / History 恢复入口及其详情操作状态；idle 中被忽略的 partial 和早于当前选中操作代次的限额事件不改变恢复 owner。同代次的已准入事件保留恢复入口，过时代次仍被拒绝。

## 1. 普通口述：默认自动交付

```mermaid
flowchart LR
  A[用户按快捷键] --> B[捕获音频]
  B --> C[ASR]
  C --> D[本机标点、布局、词典与授权改口准备]
  D --> E[本地解析 CleanupIntent]
  E --> F{Cleanup route}
  F -->|支持且启用| G[AI cleanup]
  F -->|Off 或场景本地策略| H[保留已准备的转录]
  G -->|成功| I[最终布局与 protected-span guard]
  G -->|失败/超时/空响应/未完整结束| J[local fallback]
  H --> I
  J --> I
  I --> K[delivery policy]
  K -->|尝试输入框| L[目标与焦点 guard + native clipboard snapshot]
  L --> M[最多发出一次 Cmd+V]
  M --> N{焦点 value 可验证?}
  N -->|是| O[paste confirmed]
  O --> P{VoiceFlow 仍拥有临时 marker?}
  P -->|是| Q[有条件恢复原 native pasteboard]
  P -->|否| R[保留较新的外部剪贴板]
  N -->|否：键盘可能已生效| Y[paste_unverified：History 持久保存；快捷键可能已发出时 HUD 立即收起并保持静默]
  S -->|是| T[剪贴板文字已确认保留，用户可手动粘贴]
  S -->|否| U[History + 唯一的复制恢复入口（提交后）]
  M -->|发送快捷键前失败| S
  L -->|发送快捷键前失败且原 snapshot 未变| T
  L -->|发送前的其他错误或所有权不确定| U
  L -->|临时写入后、快捷键前取消| V{marker 仍属于 VoiceFlow?}
  V -->|是| W[经最终所有权复核恢复原剪贴板并写入 History]
  V -->|否 / 无法确认| X[保留当前剪贴板并写入 History]
  M -->|可能已发出后取消| Z[按取消路径保留 History，不自动复制或重试]
  K -->|只复制| T
  K -->|仅历史| U
```

普通听写在所有 App 都优先使用一次键盘粘贴；Accessibility 只用于确认目标与读回结果，不直接编辑字段。准备临时剪贴板时会读取当前焦点字段可用的 AX value 和 UTF-16 选区范围；发出 `Cmd+V` 前复核其中已知的值和范围。macOS 系统安全输入开启时，VoiceFlow 会读取其 owner PID 并与已捕获的目标 PID 比较：目标 App 持有时拒绝自动粘贴；其他 App 持有、owner 不明或探针不可用时都不发送快捷键，并进入现有剪贴板 / History 恢复路径。AX secure text field 检查仍独立保留。若其他 App 持有 secure input，或系统状态持续开启，自动粘贴会在此期间退回复制流程。已知 AX 值或范围改变、或变得无法读取时，同样不发出粘贴快捷键；原本无法读取的 AX 字段仍保持 unknown，按原有能力继续处理。`Cmd+V` 发出不等于目标控件接收成功：读回沿用同一保留字段锚点并做有界观察；首次 AXValue 与预期完整 splice 不符时，最多再用一次完整范围 AX 读取，且必须精确匹配全文、UTF-16 字符数和插入位置后的折叠光标，同时前后复核目标与焦点身份；轮询期间只复核前台 PID、bundle、窗口 ID 与焦点锚点，不再查询浏览器 URL / 标签页（不运行 AppleScript）；最后一次读回之后只做一次最终目标复核，浏览器目标按 URL 加标签页序号比较，不含页面标题，因此粘贴后标题变化不会让结果变成未确认，在已启用浏览器访问时，同站点切换标签页仍会被拒绝（粘贴前的目标复核仍包含标题）；光标原本无选区（折叠光标）或字段为空时同样可用。若替换文字与原选区完全相同，AXValue 不变而无法证明粘贴发生，此时 AXValue 相等不算确认，只有完整范围光标证明可确认：在轮询的第 1 次、第 5 次和最后一次最多各尝试一次（共至多 3 次），完整范围不可用时立即停止轮询；没有该证明时结果为未确认（读回不可用），不算成功，不恢复旧剪贴板、不开放撤销；空替换文字永不确认。该证明不移除换行或归一化正文；缺少粘贴前完整 AXValue 时仍为未确认。无论读回延迟或不确定，都不会发送第二次快捷键。VoiceFlow 先保存 native pasteboard snapshot 和 ownership marker，正常粘贴在确认后，会在 marker 仍由 VoiceFlow 持有时恢复旧剪贴板。只有键盘粘贴确定未发出且 marker 仍归 VoiceFlow 所有时，才会把听写文字留在剪贴板作为手动恢复路径；当 guarded `Cmd+V` 可能已经发出时，普通听写 HUD 立即收起；AX 读回继续在 paste owner 的后台线程运行，处理 phase 与 operation lease 保持到读回和剪贴板 ownership 结算完成。结果若为 `paste_unverified`，仍持久化 History、保留诊断结果，但 HUD 不再弹出检查提示或复制入口，也不自动复制或重发；用户可自行检查目标，再决定是否打开 History 复制。若其他 App 已更新剪贴板或所有权无法确认，VoiceFlow 保留当前剪贴板并把文字放入 History；最终交付方法为 `history` 且文本可恢复时，History 写入后提供一个精确事务的复制恢复操作。临时剪贴板只声明 plain-text、owner marker 与 `org.nspasteboard.TransientType` / `org.nspasteboard.AutoGeneratedType` 提示；提示对剪贴板管理器是建议，不保证其隐藏内容。快捷键确定未发出且 marker 仍通过最终所有权复核时，取消会经既有恢复 owner 恢复原 snapshot 并写入 History；所有权变化或未知时保留当前剪贴板并由 History 恢复。快捷键可能已经发出时取消绝不恢复旧 snapshot，继续采用读回与 History 路径，避免误覆盖或重复插入。尚未准备临时剪贴板时发生的失败，只会在原 snapshot 仍未变化时尝试安全复制回退。

最后的 guarded dispatch gate 与 global / local processing watchdog 在同一 manager 锁下核对代次与取消状态。watchdog 先取得锁时，旧 paste owner 不能再发出 native 快捷键；owner 先记录 `may-have-posted` 时，watchdog 只取消并静默等待原 writer 结算，不发前台错误、音频恢复提示、复制或重试。paste owner 线程明确启动失败属于确定未发送，沿现有 History transaction 提供一次手动复制恢复，且不额外写剪贴板；result receiver 丢失属于未知交付，记为 `paste_unverified` 并保持静默。

VoiceFlow 在每次冷模型加载前，以主机 `hw.memsize` 的 80%（向下取整）作为两个自有 sidecar 的合计 footprint 上限。ASR 冷加载 reserve 为 Qwen3-ASR 0.6B 2.25 GiB、Qwen3-ASR 1.7B 10 GiB、Parakeet TDT v3 8-bit 4 GiB、FireRedASR2-AED 10 GiB、Fun-ASR-Nano 2512 5 GiB；Cohere 2B 仅保留已保存路线，reserve 为 10 GiB。cleanup 冷加载 reserve 为 Qwen3.8 27B 25.75 GiB、Qwen3.5 2B 6 GiB、Qwen3.5 4B 8 GiB、Gemma 4 E4B IT 8 GiB 和 Ministral 3 3B Instruct 8 GiB。这些 reserve 是 admission guard，不是实测峰值、最低硬件保证或质量评测。模型下载还有固定容量预检：reserve 单独与主机 80% cap 比较；容量不足或 `hw.memsize` 未知时拒绝新下载，且下载命令在传输前重复检查。该预检不使用瞬时内存压力或当前 sidecar footprint，实际冷加载仍按完整进程 footprint 和压力状态动态准入。32 GiB 主机 cap 为 27,487,790,694 B，低于 27B reserve，因此 Qwen3.8 27B 冷加载会被拒绝；其他模型能否加载还取决于完整 footprint 样本和双 child 总成本。已驻留模型走原有快速路径，不重复预留；切换 ASR 或 cleanup 模型时先卸载旧模型，再进行新模型采样和准入。

预算仅统计 VoiceFlow 注册且 `models_root` 相同的 sidecar 子进程。采样需取得完整的进程 birth identity、`phys_footprint` 和物理内存；任一事实未知、身份改变、算术溢出或预算超限时，拒绝冷加载；cleanup 记录为 `local_memory_unavailable`，ASR 走本机识别不可用错误路径。每个 pending LOAD 以 admission 时的 footprint 加对应 reserve 得到目标 `T`，该 child 的成本为 `max(当前 footprint, T)`；其他 child 的已结算成本为其当前 footprint。一个 pending token 只计一次；未知大小的旧 LOAD reservation 会阻止新准入，直到匹配 LOAD 终态或 supervisor 实际 wait/exit。采样期间 registry epoch 或 child identity 改变时拒绝该次准入并由调用方重试或 defer；锁不跨系统采样、IPC 或 await。

主机 pressure 通过严格的 `kern.memorystatus_vm_pressure_level` `uint32` 采样读取：1 为 Normal、2 为 Warning、4 为 Critical，其他值、长度错误或系统调用失败都为 Unknown。冷 ASR 只在 Normal / Warning 且完整预算可证时准入；Critical / Unknown 不加载。冷 cleanup 和 startup warmup 只在 Normal 且完整预算可证时准入；Warning / Unknown 跳过 warmup。已知 Critical 时仍保留原有 cleanup load / inference 保护，即使模型已驻留也不开始 cleanup。

如果 cleanup 的冷加载因 `local_memory_unavailable` 被拒绝，且已有非空 ASR 原稿，短 / 长听写和 History retry 会逐字交付原稿，绕过本地 cleanup、`local_cleanup_or_raw`、词典替换与最终文本改写；History 标记「本机内存预算不足，未运行本地整理；已保留 ASR 原稿」。不根据空原稿制造结果。History 对选中文本的重新整理在创建 revision 或改动剪贴板前返回明确错误，保留选区与已有记录。已知 Critical 继续使用独立的 `critical_memory_pressure` 状态和原有文案。

交付诊断先记录观察到的失败原因，不在最终目的地确认前承诺已复制或已保存。终态使用 paste owner 的最终 `delivery_method`：只有 clipboard ownership 已确认才显示已复制；History 只有在实际写入成功后才显示已保存，写入失败会显示保存错误。目标在写入前变化且没有剪贴板副作用时走现有 History owner，不会重定向、恢复焦点或再试粘贴。

Stop-to-insert 的既有汇总仍只计入目标读回已确认的粘贴；另有按七种交付结果分组的有界延迟汇总，包含未确认粘贴、复制、仅 History、预览、失败和取消。两类数据都只保留最近 128 个样本 / 组，并受 32 组上限约束。指标不包含听写文本或其他目标内容。每个已准入 stop 的终态另会 best-effort 写入本机 `diagnostics/dictation-performance.jsonl`，仅保留固定分类 / 结果 / 阶段枚举、捕获的 session generation、时间戳和有界耗时；最多 30 天、10 MiB，清空全部数据时随 `diagnostics/` 删除。它不表示读回已确认，也不包含转写、prompt、模型 / provider 标签或目标身份；繁忙、写入失败或进程退出可能导致记录缺失。On Device ASR 模型在任务完成后等待 15 分钟空闲，再尝试卸载；新请求会使过期的 idle-unload 任务失效。该计时器本身不证明模型已成功卸载。若 unload 请求失败或其响应无法验证，runtime 会终止并等待回收当前自有 sidecar 子进程、清除该 session，并把状态设为 `runtime_failed`；后续请求会创建新 session 再尝试加载模型。 MLX cleanup runtime 另在前台 cleanup demand 或最后一个 startup warmup 完成后等待 15 分钟空闲，再向实际驻留的 child/model/revision 发起卸载；启动新请求或 warmup 会取消旧 timer，活跃请求、未结算 child 与 warmup owner 会阻止卸载。有效 unload 释放模型权重但保留 sidecar session 与四个 durable prefix-cache slots（sidecar 内存常驻上限为 2 个 KV slot，持久化磁盘 LRU 为 4 个 slot）；失败响应沿现有安全退役路径处理，计时器不作为成功 receipt。

On Device ASR 的本机诊断按阶段记录模型文件校验、runtime / sidecar 启动与握手、模型加载或复用、runtime 排队、推理交换和终态耗时。正式听写阶段还携带当前已准入 stop 的 `session_generation`，用于与该会话关联；独立预热和状态探测不伪造听写代次。记录只含固定阶段 / 结果码、耗时和模型标识，不含音频、转录、提示词或路径。失败、取消或未完成的阶段仍按实际终态记录。

普通听写的 `paste_text` / `copy_text` 使用录音开始时创建并贯穿停止处理的同一事务身份。剪贴板 writer 在排入阻塞 worker 前取得 lease；终态、取消或超时封闭新准入，所有已准入 writer 退出后才记为 closed。closed 只证明该 writer 生命周期结束，不证明粘贴成功、剪贴板已恢复或下一次录音已就绪。普通听写在粘贴前先写入带稳定 transaction ID 的 `history-pending/` intent，状态为 `delivery_pending`；必要的预交付 I/O 记录为 `HistoryJournal`。当当前音频保留策略要求保留源音频时，还必须等实际 spool FIFO 尾部 ACK，未确认就不交付。交付 owner 返回结果后，同一 intent 更新为实际 outcome 并提交 SQLite History；History、gold audio、spool 关闭和恢复各由有界 retained owner 持有并分别记录阶段耗时。Guarded 键盘快捷键可能已发出时，原 paste owner 继续在后台完成 AX 读回与剪贴板 ownership 结算；常规路径中的 Processing phase 与 live dictation operation lease 在这段时间保持占用。若 global / local watchdog 在 dispatch gate 已开放后触发，它封闭 writer 准入并取消处理，但保持 HUD 静默且不重发；live operation lease 仍等已准入 writer 关闭后释放。读回确认粘贴后，live operation 才结束，且不等待可选的 SQLite History、gold 或恢复清理；History-only 仍须等 History ACK 才报告已保存。若进程在粘贴和 History ACK 之间退出，启动恢复按稳定 ID 幂等保存精确文字并标为 delivery-unconfirmed / unknown，不自动 ASR、整理、再次粘贴或重试。后续存储失败会保留 transaction intent 和可恢复来源并显示待确认 / 错误，不伪报已保存。捕获与 retained History owner 均有界；实时 spool 会话使用租约推导的路径 scope，旧事务的 History 数据库提交不会占用新会话的输入读取和实际 CaptureWorker。下一次录音仍须通过真实 start 与 input-frame 回执；public readiness 持续为 Unverified，不能只凭 manager Idle 或 writer closed 宣称 Ready。旧回调依完成代次和 owner 身份验证，不能发布到新录音或隐藏新 HUD。

常驻的听写完成状态在 Island 文档中位于会被隐藏的胶囊可访问性子树之外，并由后端正常快照命令和 owner 生命周期事件更新。macOS 原生发布任务在主线程实际执行时重新读取当前 registry snapshot，而不重放排队时捕获的事务文字；同一进程内的原生标识修订号也不会倒退。它把实际后端快照发布到保留在现有 NSWindow 可访问性子节点中的原生 `NSAccessibilityElement`：人类可读值与可读标签分开，原生标识源包含进程身份、十进制字符串修订号、当前 / 上一终态身份、结果类别、准入封闭、writer 关闭和 writer 数量；剪贴板恢复与下一次录音 readiness 明确保持未确认。原生人类标签和值跟随 Island WebView 实际使用的解析后界面语言；语言变化只重新本地化，不改变事务证据或修订号。标识和值都不含转录或剪贴板正文。前端先订阅再读取快照，并按进程身份与十进制修订号拒绝迟到或重复状态；修订号耗尽会失败关闭。原生 `NSAccessibilityValueChangedNotification` 尚未发布，避免在 WebView polite live region 和原生观察路径的实际播报关系未经检查前引入重复播报。此 source implementation 尚未通过最终打包应用的原生 AX 检查，WebKit / AppKit 的实际可观察性、原生通知与 VoiceOver 行为仍未验证；它不是剪贴板读回或恢复授权，也不把 `Inserted` 解释为 clipboard restoration、`Ready` 或下一次录音可用。可选 History、音频与 spool 存储仍可能阻塞后续录音。

当实际交付方法为 `history`，或发送前的失败已留下可恢复 History transaction 时，HUD 可提供一个「复制听写文字」入口；它覆盖 History-only 的已保存、待确认或失败状态、明确未插入后采用 History 的结果，以及有确切 History 来源的发送前恢复失败。若快捷键可能已发出但读回未确认（`paste_unverified`），History 仍持久保存，普通听写 HUD 在快捷键可能发出时已收起，终态保持静默且不提供复制恢复入口。面板仅携带当前 delivery owner 与确切 History transaction 身份，不把 transcript 放入事件或命令参数；用户点击后才从该 transaction 的 pending intent 或同 ID History 行读取文字，并只写剪贴板，不会重新 ASR、整理、自动粘贴或重试交付。ACK / 失败只更新仍显示同一 transaction 的面板；用户关闭后或新录音开始后，旧事件不能重建或覆盖新 HUD。剪贴板结果不确定时显示未知并禁用再次复制；仅在文本读取失败或明确未写入时允许用户主动重试。复制状态不代替 History ACK。


录音 HUD 与识别、整理、交付及结束状态共用同一深色 220×48、R24 胶囊；折叠时原生窗口为 244×64，恢复和详情面板从主胶囊上方展开，最多使用 320×116 窗口。按住或点按启动录音后，前 250ms 对极短的 `starting` 与等待首帧状态保持静默，直接显示「录音中」（已知设备时直接展示设备名），仅当等待首帧超过 250ms 才提示「准备录音」/「等待麦克风」，真实输入异常或无声仍立即提示；第二行显示采集状态、实际打开的设备和计时。实时录音电平只由当前会话最近收到的真实 CoreAudio 样本（经去平峰压缩曲线、静音门限与用户灵敏度缩放）驱动；底部使用 `voice-glow`（`VoiceBeam` `type="pill"`，支持 8 种独立配色、`bandColors` 调色板传递与 4 种声浪风格预设）随真实音节起伏并横向流动，设置页「HUD 配置」默认外观为 Classic，提供左预览、右控制面板的联动调试台及仅使用数学合成波形的「模拟声浪」预览（不打开麦克风），改动通过 `"settings://changed"` 实时同步到原生 HUD。动效在 Classic 与 Liquid Glass 外观下均随对应明暗主题生效，但只在输入可用且收到 750ms 内真实帧、未开启降低动态效果时激活；超过 750ms 未收到新帧、设备出错/无音频或降低动态效果时动效回到静止并淡出。停止按钮在录音和处理中保持同一位置：录音时停止并进入处理，处理中取消当前会话；Esc 仍可取消。它不会激活目标 App 或改变输入焦点。处理开始后隐藏设备与计时，淡出语音声浪并保留左侧工作光球：所有常规识别、长录音分段与整理阶段统一展示稳定的「处理中」，仅在 429 限流退避时显示「限流重试」；底部沿胶囊轮廓运行 `border-beam` 扫光（`size="line"`，颜色与语音声浪独立配置并支持 8 种配色及 3 种速度/亮度预设，对非原生调色板通过稳定基础色板与色相偏转渲染，Classic 与 Liquid Glass 均开启，限流重试及降低动态效果模式下暂停并隐藏；不暴露倒计时、分数字样或设备信息）。静态终态、错误、恢复和学习提示统一使用黑灰表面、白灰文字及单条中性细边，不使用彩色描边。普通听写的 guarded 键盘快捷键一旦可能已发出，HUD 零延迟立即收起；AX 读回、History、诊断与学习 owner 继续按各自事务完成。读回未确认时不谎报插入成功或确定失败，但 HUD 保持静默、不提供复制入口，也不自动复制或重试；精确转写仍写入 History，用户可自行检查目标后决定是否从 History 复制。只有实际 `history` 交付或有确切 History transaction 的发送前恢复失败且文字可恢复时，才显示唯一的复制恢复入口。选中文本操作不会写入 History，因此其未确认 HUD 不提供 History 入口，详情文案与无障碍播报也不引导用户查看 History。确认键盘粘贴且没有回退后，主 HUD 立即收起；空闲和取消也不留主提示。复制、历史、未确认、降级、错误和恢复提示按实际结果显示。恢复详情提供有证据支持的历史、麦克风、模型或服务设置入口；不自动重试或再次粘贴。录音动画仅在本代次收到 750ms 内的新麦克风帧时播放；减少动态效果或没有新帧时动画暂停。连续五秒没有首帧时会提示尚未收到麦克风音频；新帧到达后恢复为正在接收。正常停止后没有可用音频会明确提示检查麦克风，快速取消仍走取消路径。设备选择只影响下一次录音；显式选择设备不可用时本次启动失败并显示设备设置入口，不会在录音中切换到另一台设备。会话级流错误只恢复对应录音的音频；如果首次 History 保存失败，HUD 可显式重试该会话的 History 保存，成功后打开该记录；失败时仅在同一 spool 仍可恢复的情况下保留音频和重试入口。若 spool 已过期、被清除或无法恢复，结果显示为未确认，只提供检查 History，不声称音频仍在或已经保存。此恢复不自动重转写或粘贴。跟随系统默认设备的选择会在下一次录音启动时重新解析。Idle 预热流不按设备名称/采样率冒充设备身份复用；每次正式录音重新解析并打开所选输入流。录音 owner 保存实际打开设备可确认的 CoreAudio 稳定身份与 transport；Stop 时若当前 lid 已关闭且该身份明确为内置麦，capture 会关闭并排空已接受音频，保留部分录音后返回设备错误，不进入正常 finalization / ASR。外置、身份未知和 owned-WAV 回放不因该检查误拒；已有采集设备错误优先保留。物理流错误按实际 stream ID 归属，worker 错误同时绑定 stream ID 与录音代次；旧会话迟到事件不能污染新会话。当前 App / 语气、翻译目标和上下文来源仍可通过无障碍描述读取，不增加可视行或显示上下文文字。词典学习提示使用同一 220×48 胶囊，展示建议或已学改正并提供撤销 / 关闭；待确认候选另提供「全局记住」及（仅当当前候选具备明确 native App、网站域名或 mapping 范围时）「仅此 App」/「仅此网站」确认入口，否则只显示「全局记住」；撤销或确认仅影响学习规则，不撤销已交付文本。麦克风电平反馈设置不会关闭文字状态提示。上下文整理也区分自然语言撰写提示与代码编辑、终端和安全输入；只有可确认的自然语言撰写字段使用 coding-prompt 整理意图，字段身份不明时保守处理。内置 Codex 的精确身份是窄例外：当 AX 仍将字段观察为 Code 时，只有完整 ASR 转录具备明确自然语言请求且无代码、命令或结构化数据特征，才在当前事务临时按 coding-prompt 整理；目标分类和自动词典学习规则保持原样。历史录音中没有保留的 AX 字段内容不会被追溯推断。

粘贴失败或未确认时，History 会保存稳定错误码和不含听写文字的操作提示。详细本机诊断位于 `app_data_dir/diagnostics/paste-delivery.jsonl`，记录失败阶段、目标 App bundle ID、Accessibility / 焦点检查、clipboard snapshot 与 ownership、键盘尝试、读回验证和恢复结果；不记录 transcript、窗口标题、URL、PID 或密钥。启动和写入时会清理超过 30 天的记录，并从最旧记录开始裁剪到 10 MiB；“清空全部数据”也会删除该日志。

History-only、临时剪贴板所有权丢失 / 未知和 post-write cancellation 都以 History 作为恢复路径。若 SQLite 未能写入这些结果，HUD 会报告保存失败，并对确切 transaction 提供 clipboard-only 文字复制；这不代表 History 已保存。pending intent 使用与 History 相同的正文和上下文过滤，不额外记录转写文字。当 `keep_audio_days` 允许时，短 / 长录音的源 capture spool 保留到启动恢复重建；精确 History 文字由 pending intent 单独保护。History 写入确认后，未选择保留音频的成功录音删除临时 spool；用户 opt-in 的成功音频写入 gold；需要显式恢复的失败、未确认或降级结果保留 manifest recovery session。清理等待真实 owner I/O 结束；clear 的 root lease 跨越 journal、delivery、History ACK 与最终 spool I/O，存储错误不会被报告成已保存。

录音捕获的 16 kHz F32 轨迹保留在最多 15 分钟的有界内存缓冲中；采样率转换在结束时按源样本数冲洗 sinc 尾部，使输出覆盖完整的有理数采样区间。首帧到达后，若相邻真实采集帧超过 750ms，新鲜度跟踪器会锁存该录音的不完整状态；短暂静音不算作帧间隔，恢复来帧也不会清除锁存。Stop 仍会关闭 capture、排空已接受音频并走既有音频保留 / 恢复路径，但该会话返回捕获错误，不把存在缺口的音频交给 ASR 当作完整成功。首帧之前的等待不触发此缺口判定。捕获队列溢出代表样本丢失，也会在 ASR 前将整段录音标为不完整；spool 队列溢出只影响落盘确认，内存里的完整轨迹仍可继续处理。录音开始只在内存中预留 spool session 路径；有界 spool owner 随后独占创建目录和 manifest，并写入 F32 恢复分块。该 owner pool 最多三个会话；每个 owner 的队列最多 16 条、合计 40 MiB；创建失败不会覆盖已有恢复目录。生成提交用 WAV 不再同步写出并重读临时 raw 文件。捕获停止的应答窗口为 200ms；超时后迟到结果不再选为本次停止结果，owner 与其 session lease 保留到实际退出。普通成功 finalization 将 spool lease 与 WAV 一起交给 History handoff；known no-side-effect 路径明确请求完成清理，已 journal 的交付则在确认 History 或保留恢复源之后关闭 owner。长录音 finalizer 的完成、取消和删除仍直接封闭 producer admission，采样队列关闭不会自行把录音选成 Recoverable。spool worker 与最多三个 retained History store owner 分开限额；History transaction 仍用 scoped I/O gate。每 session 实时 spool 写入按租约推导的路径 scope 隔离；扫描、完整 quota / retention 枚举、清空和兼容旧路径保留 root-wide scope，并与所有 session 冲突。共享 spool quota 在互斥预留账本中同时计入磁盘内容和未完成写入，reservation 直到真实 I/O 返回才释放。恢复扫描、保留清理、History 提交和 gold 写入在覆盖其完整 quota / enumeration 的 scope 下运行；争用时有界 spool owner 保留已接收命令并有限间隔重试，不在 capture callback 等待，也不建无界 worker。文件系统操作或 History ACK 超过 5 秒时，调用方只报告待确认；已入队操作的 owner 与 root mutation lease 继续保留，不会把超时当作已保存。清空数据先取得 root 级排他租约；进行中的录音、恢复、保留、History 修改或 retained 写入会使清空返回 busy。session lease 使用代次标识，旧 owner 退出时不能删除后来复用的同一路径。lease drop 只请求实际 owner 收尾；I/O 返回前 owner 与 mutation lease 不释放。此基础不启用生产语音过滤，也不代表麦克风端到端验证；spool 是否完整可恢复仍需实际确认，不能按已保存显示。

真实硬件采集另检查采样时钟漂移：首个成功接受的回调建立单调时钟基线，排除该首块源帧和基线之前的启动等待；之后只累计成功入队的原生采样率 mono 源帧，不使用重采样后的 16 kHz 输出。源帧时长与基线后的实际单调经过时间相差绝对值严格超过 750ms，且同方向连续严格超过 750ms，才锁存捕获错误并发送一次 `CaptureError`。任一差值恰为 750ms 或持续恰为 750ms 都不会触发；误差回到阈值内或方向变化会重置持续窗口。Stop 竞态下仍排空并保留已接受 PCM，但在 ASR 选择前按不完整录音失败；错误事实也会进入 capture close trace。Owned-WAV 回放不启用硬件墙钟漂移判定，因此不会因 feeder 调度延迟而被误拒。

交付警告使用 HUD 的紧凑两行提示，说明原因和下一步操作，并显示 6 秒后淡出。

录音手势：默认 tap 点按切换，再按一次结束；也可选 hold_to_talk 按住说话，松开后结束；或选 hybrid_hold_lock，按住至少 220ms 后松开结束，短按两次且间隔小于 320ms 则锁定免提，之后再次按下结束。单次短按在双击窗口到期时取消，不交付。三种方式都保留 Esc 取消和旧 tap / hold 配置。Fn 单键需要 Input Monitoring；普通组合键在没有 Input Monitoring 时继续注册，但修饰键先松开时会等到组合键整体释放。该监听只读按键状态，不读取输入内容。设置会提示回退方式并提供系统设置入口；更换绑定不改变方式。

服务页把当前生效路线与待应用草稿分开。provider、模型、区域、AI 整理开关和本机预设修改后只改变草稿；阶段测试只检查对应草稿路线，不保存密钥或切换当前路线。「保存并使用」才提交设置与对应凭据。Soniox 密钥随共用「保存并使用」操作写入钥匙串；设置界面没有 Soniox 连接测试，连接在开始听写时建立。严格离线开关立即执行保护策略。左侧「模型配置」下有「服务配置」「API Key」「本地模型」「高级设置」四个独立页面；四页在同一设置视图中切换，保留路线草稿与进行中的本机模型操作；共享保存栏只在有待处理操作时出现。模型下载与管理位于「本地模型」；Custom、本机服务地址、区域、预设和请求参数位于默认收起的「高级设置」。API Key 页面默认显示所有支持凭据输入的 provider，每项只有一个默认收起的编辑器；Ollama、本机 Whisper 和 Custom 的密钥为可选输入。四个页面之间切换保留草稿。App 映射的高级条件可折叠，独立授权始终显示且保持原值。

设置页（`EngineSettings.tsx` 协调 `src/components/settings/engine/*` 四页签；`ContextSettings.tsx` 协调 `src/components/context/*` 三区块；`LatencyMetricsViewer.tsx` 在调试页与引擎高级页共用）与首次引导的本机模型状态均通过 `useLocalModelsStatus` 订阅后端统一事件流 `"local-models://status"`（覆盖 `on_device_asr`、`on_device_runtime`、`cleanup_store`、`mlx_cleanup` 与 `gguf_cleanup`，并兼容现有单项事件名），不再使用前端定时轮询；读取异常时保留手动刷新恢复入口。后端 IPC 命令与持久化分别组织在 `src-tauri/src/commands/{mod,keychain,context,models,settings}.rs` 及 `src-tauri/src/store.rs` 子模块 `src-tauri/src/store/{spool,settings_model,learn_pairs,history_export}.rs`；设置、模型与词典变更命令返回结构化 `SettingsCommandError`（`{ code, message }`），供前端按稳定错误码做本地化反馈。

Installable Test 的一次性 Groq 凭据导入由最终签名 App 的固定 `--import-test-groq-key-stdin-v2` 命令提供：调用方把单行密钥经 stdin 送入并关闭输入，App 在启动 Tauri 和单实例分流前退出导入命令。它只向新的 Test Groq Keychain v7 主 / 回退项执行 create-only 写入并读回确认；同值重试幂等，不同值冲突或验证未知时不覆盖。旧 Test Groq v1 / v2 / v3 / v4 / v5 / v6 项保留且不会被该路径读取、删除或修改。固定只读 `--test-groq-key-status-v1` 命令可查询两个 v7 槽位；输出仅含固定状态、阶段、等待结果、耗时、数字 OSStatus 与 `prior_add=unknown`，不含密钥或 Keychain 标识。stdin 有 512 字节上限与明确超时，导入输出为固定状态码和有界脱敏诊断；之后由测试流程正常重启后台 App。自签名 Test build 的 Keychain partition 可能绑定可执行文件的 CDHash，不同 build 不保证能读取前一 build 写入的凭据，测试流程可能需要由该 build 的最终 App 重新配置。该路径不调用 provider、不接收 argv / 环境变量 / 文件中的密钥，也不影响普通应用、其他 Test provider 或 History 凭据。

同一 Installable Test bundle 在 macOS `research-harness` build 中可用固定 `--research-owned-wav <absolute-path> --target-nonce <uuid>` 入口执行有界的已知 WAV。入口默认要求固定本机 ASR 与本机 cleanup，也可通过严格解析的 `--research-owned-route` 明确选择 native/Groq 的四种组合；云端只使用 Groq `whisper-large-v3-turbo` ASR 和 `openai/gpt-oss-20b` cleanup，并要求当前已保存设置、凭据和非严格离线状态完全匹配。它将经过固定哈希和 PCM 格式校验的同一份 WAV 字节送入现有 capture / finalization / ASR / cleanup / paste 路径，不调用麦克风、不为测试修改已保存 provider，也不把该 session 送入词典学习；实际重试与 cascade 仍由生产 provider owner 决定并记录。开始与插入前都必须在同一 PID 绑定的受控可编辑字段中读到完整 nonce fixture 和精确 UTF-16 选区，授权有效期为三分钟；只允许固定的本机 Chrome 测试页或私有的 nonce 绑定 TextEdit `.txt` fixture。TextEdit 还必须匹配固定私有 fixture 目录下的规范文件 URL、UUID 文件名、窗口标题和完整字段值；个人文档不在允许范围内。任何插入检查失败都不会恢复焦点、改写目标或复制到剪贴板。只有同一已授权来源和仍有效的原目标租约下，明确的 prewrite `target_changed` 且诊断证明没有剪贴板写入或键盘尝试，才继续走同一个 History owner；取消、过期租约、未知诊断及任何写入副作用仍失败关闭。History 只在保存成功后报告为已保存。Window ID 通常要求 AX window title 与 geometry 严格匹配；只有字段 `AXWindow` 与应用 `AXFocusedWindow` 的直接对象相等证明，才允许用完整有效 geometry 唯一匹配同 PID、屏幕内、layer 0 的 CoreGraphics window，而忽略可变标题；明确不等会失败关闭，证明未知时保留严格标题路径，geometry 缺失或有歧义时不猜测。`audio://owned-replay` 生命周期记录和成功时的 stderr 摘要只含固定源 hash、generation、帧 / 采样计数与阶段；目标准入失败的 stderr 诊断另含静态 admission / focus 阶段与原因码、稳定前台身份布尔值及有界 paste 错误码，不含源路径、nonce、PID、窗口、字段内容、音频或转录。此入口只验证软件回放路径，不能替代物理麦克风验证。

ASR 默认保持 Groq `whisper-large-v3-turbo`；cleanup 默认保持 Groq `openai/gpt-oss-20b`。新增预设不覆盖已保存的 provider/model/custom endpoint。SiliconFlow 仍默认 `FunAudioLLM/SenseVoiceSmall`，另有可选 `Qwen/Qwen3-ASR-1.7B`。新 provider IDs 为 `fireworks`（`whisper-v3-turbo`，base host `https://audio-turbo.api.fireworks.ai`）、`mistral`（`voxtral-mini-2602`，base host `https://api.mistral.ai`）、`soniox`（实时流式专用 `stt-rt-v5`，WebSocket `wss://stt-rt.soniox.com/transcribe-websocket`）、`assemblyai`（Universal-3.5 Pro Dictation / raw Sync）和 `dashscope`（只接入 `qwen-audio-3.1-asr-flash-message`）。Fireworks 可用性尚未核实；Qwen 账户与地区可用性尚未核实；Soniox 的真实麦克风生命周期尚未验证；本阶段未验证 AssemblyAI / DashScope 帐号与服务可用性。Deepgram 预设不启用 `multi`，此处不声称它支持中文或中英混合。

On Device 是单独的本机 ASR provider。当前可供新选择的五款模型为 `qwen3-asr-0.6b`、`qwen3-asr-1.7b`、`parakeet-tdt-0.6b-v3-8bit`、`firered-asr2-aed-mlx` 和 `fun-asr-nano-2512-mlx`；文件下载由用户在模型管理页主动发起。后端按固定模型清单中的 `download_available` 标记是否开放新下载（其中 `qwen3-asr-1.7b` 保持可选与可下载，同时通过 `license_unreviewed=true` 公开转换权重仓库许可尚未完成审查的非阻断提示，指向 `THIRD_PARTY_NOTICES.md`）；该 curated availability 不以当前平台支持、文件状态、运行时握手或 `inference_ready` 为条件。下载操作另按固定 reserve 与主机 80% `hw.memsize` cap 做静态容量预检；内存不足或未知时禁用下载，命令在传输前复核。此预检不看瞬时压力或当前 sidecar footprint；实际加载仍由动态 admission 检查。平台、文件、运行时握手和 `inference_ready` 能力分别展示并在加载 / 推理路径中检查。五款模型尚未完成真实模型推理或转写质量评测；Swift route 的编译确认不等于真实模型推理或转写质量评测。Qwen 0.6B / 1.7B 可自动检测或固定中文 / English；Parakeet 和 FireRed 使用自动语言策略，Parakeet 来源说明列出 25 种欧洲语言，FireRed 来源说明列出普通话、中文方言 / 口音、English 与 code-switching，但当前 runtime 不回报检测到的语言；Fun-ASR Nano 可使用 auto / zh / en prompt，转录 DTO 的 language 保持 nil，因为 SDK 只回显 prompt。Cohere Transcribe 2B 只保留当前已保存选择和已有文件管理，不提供新下载或选择；SenseVoice Small 保留旧设置和文件管理，没有 MLX 推理支持。MLX 运行要求 Apple Silicon 与 macOS 14 或更新版本；设置页分别显示文件状态、sidecar 能力握手、当前模型 loading 和已 loaded 的具体模型，握手不等于已加载，也不等于真实转写成功。用户可取消本次加载；取消不会更改保存的 provider / model 或删除模型文件。普通配置控件仍可在加载期间使用。

本地模型设置将功能分为“本机转写”（语音转成文字）和“本机文字整理”（转写后在这台 Mac 上调整文字和排版）。本机文字整理的新配置选择由 VoiceFlow MLX 提供：Qwen3.5 2B、Qwen3.5 4B、Gemma 4 E4B IT、Ministral 3 3B Instruct 与原有 Qwen3.8 27B。新增模型按固定 revision 和 SHA-256 校验；Qwen3.8 27B 仍支持匹配目录导入，新增模型不开放导入。管理页显示每款模型的下载与文件校验状态；`download_available` 与 `release_qualified` 在目录元数据中分别记录，下载可用不代表整理质量已通过评测。其它旧版 MLX / GGUF 文件只在已安装、部分下载或当前已保存路线需要时显示，用于文件管理与保留当前设置；不会作为新下载或新选择提供。已保存的 GGUF 路线仍使用随包 llama.cpp（Apple Silicon、macOS 13.3+，并通过设备内存预算检查），不提供新 GGUF 下载。VoiceFlow MLX 要求 Apple Silicon 与 macOS 14 或更新版本。下载由用户主动发起并需要网络，不自动切换当前路线；本机模型不可用时沿用本地规则或保留原稿，不静默切换其它 provider。操作进度、错误、兼容状态、来源与固定版本在管理页显示；管理页不显示质量评测结论，文件校验只确认文件符合固定清单。

ASR 模型文件首次校验时会按固定清单完整计算 SHA-256。成功后可在模型目录外保存 app-private durable proof；后续只有模型来源、安装 marker 原始字节、根目录 / 模型目录和全部固定文件的身份（含设备、inode、大小、mtime、ctime、权限、所有者与链接数）都匹配时才跳过模型 payload 散列。proof 缺失、损坏或身份不匹配会触发完整重校验；写 proof 失败不会改变已通过完整校验的本次结果。显式研究来源准入继续完整散列并保留输入文件描述符，不使用该快捷路径。

严格离线模式只允许 On Device ASR，但允许本机 cleanup：经验证的 loopback Ollama `qwen3.5:4b`、VoiceFlow `on_device_mlx`（五款固定 MLX cleanup routes，详见 [privacy.md](privacy.md)），以及 VoiceFlow `on_device_llama_cpp`（Qwen3.5 2B / 4B、Gemma 4 E4B IT）。后两者由 VoiceFlow 随包运行时管理，无需启动 Ollama。LM Studio 路线已在设置 schema 27 退役，旧选择加载时迁移到运行同一模型的 `on_device_mlx`，见 [privacy.md](privacy.md)。它阻止其他 HTTP ASR 路由与探测、云端 cleanup、History 重试、cascade / prefetch 和视觉请求；开启时会取消进行中的云请求，并保留已保存的 provider 选择。模型文件不可用时使用本地规则或关闭 AI cleanup，不自动改选 Ollama 或云端；所有 ASR / cleanup 模型下载都要用户主动发起并需要网络，启动不会自动下载。默认 Groq ASR / cleanup 与现有 provider、model 和 key 不因新增选项而覆盖。

`on_device_mlx` cleanup 使用 VoiceFlow 打包的 Swift/MLX 独立子进程，不依赖 LM Studio 应用 / CLI 或 Python；它支持隐私文档列出的五款固定模型路线，包含 `lmstudio-community/Qwen3.8-27B-MLX-6bit` revision `05f38c1f39e2f39619653616fe8e96a5d676358e` 及四款固定 revision / manifest 的新增模型。27B 支持匹配清单目录导入，四款新增模型只支持用户主动下载。该路线要求 Apple Silicon 与 macOS 14 或更新版本；应用仍以 macOS 13 为最低系统版本，HTTP / cloud 及其他现有本机 cleanup 路线不受此 MLX 条件影响。平台不支持或状态未知时，设置页禁用导入和下载，后端也会在打开文件夹选择器或启动网络请求前拒绝；已存在的模型文件保留，取消和删除管理副本仍可用。用户可显式下载或导入匹配清单的目录，APFS clone 建立管理副本；取消保留已验证的 partial，删除只移除管理副本。首次加载与驻留状态分开展示，不代表模型质量。听写与 History cleanup 共用现有路由和最终保护检查。本机 MLX cleanup 超时保留其 typed timeout 失败，不由共享队列自动重试。本机诊断按请求 ID 与 History 行 ID 记录队列、子进程启动 / 复用、模型校验与加载、生成、最终保护、revision 写入、剪贴板和终态耗时；校验摘要包含缓存命中 / 原因、文件数、实际散列字节数及 SHA-256 后端。首次迁移或证明缺失时会完整校验固定清单中的模型文件；成功校验后，应用把证明保存在管理模型目录之外，并在后续启动逐项核对固定模型来源、安装清单和文件的设备 / inode / 大小 / 修改与变更时间 / 权限 / 链接 / 所有者等身份。身份相同且目录中的 `*.safetensors` 与固定清单完全一致时才跳过大文件 SHA-256；证明损坏、清单、shard 集合或任一身份变化都会保守地重新完整校验。证明缓存无法写入不会使已经通过完整校验的本次加载失败。五款固定 MLX cleanup routes 均可复用安全的 model-bound MLX stable-prompt-prefix cache：生产 Prompt owner 为 Light 请求标出 transcript 前的固定任务、档案与词典配置；Standard/Code builder 在无 previous-chunk 实时 context 时标出 Transcript 前的实际配置边界（profile/style 等精确 tokens 会绑定 cache key），有 previous-chunk context 时明确退回 system-only 边界。完整 Prompt、profile、词典、模板及生成设置保持原样，持久化候选仍由完整实际 Prompt tokenization 与两个不同特殊 token 后缀探针共同确定，并遵守 producer 给定的 UTF-8 边界。缓存 key 绑定确切模型 ID、repository、revision、模型清单、打包 sidecar 运行时、锁定的 `mlx-swift` 0.31.4 / `mlx-swift-lm` 3.31.4 / `swift-transformers` 1.3.4、所有生成参数、recipe、固定前缀 tokens 和 512-token prefill 设置；实际层数、cache 类型、形状 / dtype、offset 与 Gemma rotating-cache 元数据按已加载模型动态校验，不使用 64 层假设；Transcript、原始 ASR、实时上下文、选择区、窗口文字与生成结果留在每次请求的未缓存后缀中。每次请求从官方 MLX cache 的独立副本继续处理后缀，完整 Prompt 仍按 8192-token 总预算验收。缓存快照位于 app-private `cleanup/.voiceflow-mlx-cache/prompt-prefix`，与模型目录及耐久校验证明分开；最多四个 candidate-key LRU durable slot，每份快照上限 512 MiB，目录 / 文件权限为 0700 / 0600，stage 通过原子发布。首次 durable recovery 只会删除完全验证为 owner-private schema v2 的旧 `active` 缓存；不匹配、损坏或符号链接保持未跟随 / 未删除并按 miss 处理。读取前验证有界 safetensors header、类型、形状、偏移、缓存类、状态摘要与实际文件 SHA-256；损坏、过期或不匹配时安全视为 miss 并重新计算。缓存不改变模型、Prompt、生成参数或整理保护规则。符合现有边界的前缀可在首次请求时 build；512-token 对齐路径沿用现有 snapshot/persist/restore 行为。若该路径不适用，生产 Prompt、实际完整 tokenization 与两条空 user-prefix 的 system-only 探针共享且长度为 128–511 tokens 的前缀可作为仅内存 resident；此路径跳过 snapshot recovery、读取和写入，使用同一 state 大小上限和最多两个内存 resident。resident cache 每次请求复制后只处理新的完整 Prompt 后缀。缓存只接受安全稳定前缀，不包含 transcript、原始 ASR、实时上下文、选区、窗口文字或截图。

Onboarded、启用 cleanup、选择五款固定 `on_device_mlx` 路由之一且该模型已 ready / verified 时，应用会在启动、完成引导或保存会改变 warmup 路由 / Prompt 的设置后，按当前模型 revision 最多排队两个有界 warmup；启动不会下载模型。原有 27B 管理模型导入 / 下载完成仍会触发可用性重查；四款新增目录模型若在启动时尚未就绪，下载 / 校验完成本身不会启动 warmup，需下一次启动或相关设置变化。重复触发以相同的相关设置签名、固定模型 revision、模型操作代次和 `processing_configuration_generation` 合并；不会使用每次保存都会变化的 `settings_generation`。较新的相关设置或模型操作会使旧任务失效。普通观察到的 Code 场景仍跳过隐式 live cleanup，并可使用 Code History/Reclean 变体；若另有空位且 General implicit route 符合本机策略，会加入 General implicit 变体。General Auto warmup 与 live 请求使用相同输出计划和 native recipe builder；仅在支持的模型与兼容多行 prose 条件下启用 model-selected paragraph 分支。每个 warmup 使用生产 prompt builder、固定合成输入和独立的一 token 内部评估，丢弃生成内容，不写入 History、剪贴板或学习记录。预热不读取麦克风、History、窗口或所选文本，也不下载模型。每个合成 prompt 只在 dictation phase、操作租约和前台 MLX cleanup demand 都空闲时开始；普通录音开始不会取消或等待当前 warmup，当前 prompt 可继续运行，后续 prompt 会等录音 / 处理结束再开始。实际 On Device MLX cleanup 请求（听写、History cleanup、Writing Preview 或明确的本机 cleanup probe）建立贯穿前台 future 生命周期的 demand，并优先使用同一 runtime、cleanup gate 和子进程。若 warmup 正在加载，runtime 保留 load 与 child，加载结束后跳过尚未派发的 eval；若 warmup eval 已派发，则通过同一 generation owner 发送绑定原请求身份的协作取消，并等待原请求 terminal 与匹配 cancel acknowledgement，才释放 gate。成功结算保留已加载 child 和模型 lease，取消中的步骤不计为已完成；terminal 先完成时按完成处理。协议错误、身份不符、两秒绝对期限超时、自然退出、设置 / 模型失效和 shutdown 仍走既有退役或失效路径。每个空闲等待最多五分钟，同一设置签名的前台打断最多重试两次；超出后等待下次触发。设置保存不等待 warmup；研究模式与 owned replay 不启动它。后台预热不会阻塞录音，但启动预热本身不保证首次整理延迟。

应用会把这些结构化元数据写到本应用的 `app_data_dir/diagnostics/mlx-cleanup-performance.jsonl`，最多保留 1 MiB 当前文件和 1 MiB 轮转文件；单条记录最多 4 KiB。超过 6 秒的本机整理事件还带有 sidecar 在准备前、准备后、生成计划后与终止时采集的 MLX 活跃 / 缓存 / 峰值内存、进程累计 CPU、minor / major 缺页与 page-in、物理内存占用、设备、任务优先级、线程 QoS 与热状态快照；这些进程级计数用于后续诊断，不能单独证明慢请求的原因。前台本机整理使用允许系统空闲睡眠的 user-initiated 活动范围；模型、生成设置和缓存策略不因采样改变。日志不包含口述文字、完整 Prompt、密钥、模型路径或 token IDs。stderr 未设置 `RUST_LOG` 时默认为 INFO；`RUST_LOG=info,voiceflow::mlx::modelstore=debug` 可额外在 stderr 显示最多 15 条按模型文件序号标识的散列阶段事件。JSONL 接收本诊断事件（包括这些文件级事件），不收集通用 stderr；Rust 与 sidecar 使用各自的单调时钟报告阶段耗时，不比较两个进程的绝对时间。

普通本机 ASR 的同一诊断文件会在部分 `inference_terminal` 失败事件中记录固定 `terminal_code`：`inference_exchange_failed`、`response_identity_rejected`、`response_status_failed`、`loaded_model_identity_rejected`、`transcription_result_missing`、`local_completion_incomplete`、`local_coverage_incomplete`、`local_generation_budget_invalid` 或 `local_empty_output_not_accepted`。没有对应分类的事件省略该字段；它不包含转写文字或原始 runtime 错误文本。

本机 ASR 请求被调用方取消后，独立 Rust owner 保留原 request/session/model revision、单一 sidecar I/O、Child 与该请求的私有 WAV。仅在读取匹配原转写的 `cancelled` 终态、随后匹配新 request ID 的 `cancel` 确认，且两帧均验证 session、model revision 和已加载模型 ID 后，才复用当前子进程；整个配对共用两秒宽限期。超时、错误响应、身份不符、EOF 或帧顺序异常会请求结束子进程；owner 持续保留 session 与 WAV，直至 `Child::wait` 观察到退出，再允许后续请求启动。被取消的 Rust caller 或模型加载 future 也不会提前清除活动 PID；只有保留真实 Child 的路径观察到退出后才能清除它。Swift ASR dispatcher 保留并取消确切 worker Task；固定版本 SDK 的 Qwen、Cohere 与 DSP 取消回调覆盖实际分块、预处理、token / evaluation 检查点。同步 MLX 原生调用仍不可被中途抢占；任务返回且 DTO 已转换后，模型 owner 同步其实际使用的 CPU 与 GPU stock streams，再按序写出原转写终态和匹配确认。模型在有效取消后继续驻留。Rust fake-child 和 Swift 合成 owner 测试只验证源和 fixture 行为，不证明真实模型取消时延、Metal 静止状态或打包 App E2E。

Light 请求不把 System 已有的任务和布局规则重复塞进 user 指令；它完整保留准备稿，并在不同的时候附上完整、仅供参考的原始转录，不截断文本来适配容量。本机 MLX 请求超出 8192-token 总预算时，会在模型请求发送前以 `input_too_large` 结果拒绝。其他错误只有其 owner 能确认时才报告请求阶段或发送状态。

HTTP ASR 上传 16 kHz、单声道、16-bit PCM WAV。VoiceFlow 每个直接 ASR 请求最多发送 10 分钟；更长录音分段后完整覆盖，不截断尾部。单次录音本身最多 15 分钟。默认分段阈值为 25 秒（设置范围 5–3600 秒），目标段长默认 35 秒（范围 15–60 秒）；达到阈值或 10 分钟直接请求上限时会进入长录音分段路径。SiliconFlow 的 adapter 另限制单文件 50 MB、音频 1 小时；Mistral 文档上限为 3 小时；Fireworks 没有在当前接线中声明 provider 文件或时长上限，10 分钟是 app 的直接请求上限。

| Provider / preset | 请求与已知能力 |
|---|---|
| SiliconFlow SenseVoice / Qwen3-ASR | multipart 仅 `file`、`model`；只自动检测语言，不发送语言或术语字段，也不请求 `verbose_json`、时间戳或置信度。 |
| Fireworks `whisper-v3-turbo` | multipart `file`、`model`；只自动检测，不请求术语、时间戳或置信度；服务可用性尚未核实。 |
| Mistral `voxtral-mini-2602` | multipart `file`、`model`；省略 `language` 时自动检测，也可指定单一固定语言；最多 100 个术语通过 `context_bias` 发送；只在自动检测时请求片段时间戳，不发送 Whisper `prompt` 或置信度字段。 |
| Deepgram Nova-3 / HTTP | 仅在 Nova-3 选择 `language == "auto"`、配置了 `preferred_languages` 且全部属于其官方 `multi` 支持集（`en`/`es`/`fr`/`de`/`hi`/`ru`/`pt`/`ja`/`it`/`nl`）时启用 `multi`，否则使用 `detect_language=true`；同时保留 `keyterm` 多词短语完整传参不拆词；固定语言发送对应支持语言参数。 |
| Soniox `stt-rt-v5` | WebSocket 实时流；4,096 样本合并帧入队，48 帧队列；`auto` 且未设常用语言时使用无偏向自动检测，否则发送 `preferred_languages` 或固定语言提示，均不限制识别；出站帧按不超过 4 倍实时的有界追赶节奏发送；不发送自定义 context/terms。服务端 provisional token 与 final token 分开处理。 |
| AssemblyAI Universal-3.5 Pro | 选择固定的 Dictation 与 Sync endpoint；Dictation 最长 120 秒且仅在 `auto` 输出模式启用，非 `auto` 结构化模式（`email`/`bullets`/`meeting_notes`）、AI Off、本地-only 或长录音走同公司的 raw Sync 与共享 cleanup。当 `language == "auto"` 且 `preferred_languages` 为空时，仅 Sync 发送 `language_detection: true`，Dictation 不发送 `language_codes` 或 `language_detection`；设置常用语言或固定语言时两者均发送对应 `language_codes`。Sync 的术语只用 `keyterms_prompt`，最多 100 项 / 8000 字符，避免通用 prompt 覆盖语言检测。 |
| DashScope Qwen Audio 3.1 Message | 只接入 `qwen-audio-3.1-asr-flash-message` 完成音频 WebSocket 路径；原始识别关闭 disfluency removal，原生 polish 不启用，并经 `AsrPromptShape::InstantVocabulary` 注入术语词汇。Beijing / Singapore 区域保存为相应 HTTPS origin 并解析到官方 `/api-ws/v1/inference` WSS endpoint；不使用 Workspace ID。 |

ASR 不会因服务错误静默改用其他 provider。若用户另行配置了准确识别级联，满足该级联条件时才会额外请求其指定 provider。HTTP ASR 使用 30 秒请求超时；429 最多重试两次，网络错误和 5xx 最多重试三次，`Retry-After` 等待最多 60 秒。401/403 不重试；取消会停止排队、重试等待或正在进行的请求。HTTP ASR 不跟随重定向。History 中重试恢复音频时，超过直接请求上限的音频会拆分并完整识别，不丢弃片段。

### Soniox 实时流式

Soniox 是用户单独选择的 streaming-only ASR 路径；它不会调用 HTTP ASR，也不会对同一录音启动 batch chunk prefetch。录音开始时，VoiceFlow 先启动非阻塞、有界音频队列和后台 WebSocket 握手，麦克风采集不等待网络连接；握手期间的音频仍按样本顺序入队。队列满或覆盖校验失败会把流标为不完整，停止使用该流结果，并保留完整本机录音供恢复。每个录音会话只有一个 finalization 与交付所有者。保存 Soniox 凭据只写入 Keychain 并显示为已配置；保存本身不连接 provider。设置界面没有 Soniox 连接测试，实时连接只在用户开始听写时建立。开启严格离线会取消当前云请求；进行中的 Soniox 握手、实时流和完整 WAV 重放会停止，不再发送后续音频帧或接受转录结果。

Soniox 的 provisional token 仅在当前会话内使用，不显示为已完成结果，也不启动 cleanup、写入目标 App、剪贴板、History 或学习。发送音频帧按不超过 4 倍实时的有界追赶节奏（live 实时帧零额外延迟，积压帧间隔受限）进行，且 actor 仍通过同一 select 循环读取 WebSocket 消息，不在等待下一帧发送时阻塞接收。停止录音后，VoiceFlow 发送结束信号并等待 final transcript；只有最终转录进入已有本地准备、cleanup、保护检查与交付流程。历史记录保留 provider final 的 `asr_text`、本地准备后的 `raw_text` 与保护检查后的 `final_text`，不保存流式 provisional token。

若初次流的传输、音频覆盖或 finalization 失败，VoiceFlow 取消 / 拒绝该结果，用本机保留的完整裁剪前录音向同一 Soniox WebSocket 按不超过 4 倍实时的有界追赶节奏（bounded <= 4x real-time catch-up cadence）重放一次；重放期间 HUD 显示“重转写中”。它不会把不确定的 partial 与重试结果拼接，也不会跨 provider 回退。两次尝试之间只允许这一次完整重放，不叠加 HTTP 队列重试；认证、请求格式或计费失败会作为可处理错误返回。此协议按有界追赶节奏分帧发送并维持连接存活，可由取消终止，处理 watchdog 有界且最长 30 分钟。完整重放可能再处理并收费一次全录音；Soniox 按完整流时长计费，包括静音。实际价格依账户而异，参见 [Soniox 定价页](https://soniox.com/pricing) 和 [WebSocket API 说明](https://soniox.com/docs/api-reference/stt/websocket-api)。

### AssemblyAI 集成转写与原始 Sync

AssemblyAI 使用一把 Keychain key 和固定 endpoint，不是 OpenAI-compatible provider。保存 AssemblyAI 设置不连接服务；本机校验允许短 Dictation 的共同 cleanup 凭据保持可选。用户主动探测或实际试用的结果按其真实覆盖解释。对于总时长不超过 120 秒、且 CleanupIntent 允许 AI 整理的录音，VoiceFlow 发送 `POST https://dictation.assemblyai.com/v1/transcribe/live`。原始 `Authorization` header 携带 key；multipart 先发 `application/json` 配置，再发 `audio/wav` WAV 音频。此 endpoint 默认总会执行改写；省略或置空 `llm_instruction` 也会执行，接口没有 `cleanup=false`。

`text` 保留独立的 ASR 原稿，`llm_response` 只作为整理候选，`llm_error` 区分整理失败。候选通过与 Phase 2 一致的本机准备稿授权和最终保护检查后，才作为最终文字使用；接受候选会跳过第二次 LLM 请求。候选缺失、为空或 `llm_error` 时保留原稿；当前共同整理路由可用时，对本地准备稿只发起一次共同 cleanup，且只有所选 provider 路线要求 API 凭据时才需要有效凭据，keyless 本机路线无需 API key；没有可用路由时使用本地规则回退。成功的短 Dictation 候选本身可提供整理，不要求共同整理凭据。候选被最终保护检查拒绝时保留受保护原稿 / 本地回退，不再发第二次模型请求。

当 cleanup 为 Off / local-only、使用 `email`/`bullets`/`meeting_notes` 等结构化输出模式，或完整录音超过 120 秒时，单路改用 `POST https://sync.assemblyai.com/v1/transcribe` 获取原始 ASR；同一 key 以原始 `Authorization` header 发送，并带 `X-AAI-Model: universal-3-5-pro`。配置 part 放在 WAV 音频之前。每次请求最多 120 秒；更长音频由有界重叠 chunk 完整覆盖，并要求每块都成功后才合并原始全文并最多做一次共同 cleanup；任一块失败时跳过全稿 cleanup 和交付，把原始完整 WAV 存入失败 History 供显式全音频重试。该 all-or-nothing 规则只用于 AssemblyAI 长 Sync；其它 batch 路线保留现有部分结果复制与完整音频恢复策略。长录音的模型整理使用当前获准且可用的共同整理路由；只有所选 provider 路线要求 API 凭据时才需要有效凭据，keyless 本机路线无需 API key。没有可用路由时保留 raw / 本地准备稿并用本地规则回退，严格离线仍阻止云端整理。保存的 key 本身不是探测结果；只有用户主动探测或试用才产生请求事实，单次 ASR 请求不验证共同 cleanup 服务或识别质量。当设定具体语言或 `preferred_languages`（最多 4 项）时，Dictation 和 Sync 都明确发送 `language_codes`；当处于无偏向 `auto`（未设常用语种）时，仅 Sync 发送 `language_detection: true`，Dictation 不发送 `language_codes` 或 `language_detection`。Sync 只通过 `keyterms_prompt` 发送最多 100 个、合计最多 8000 字符的相关术语；不附带会覆盖 `language_codes` 的通用 `prompt`。

### Qwen Audio 3.1 Message 原始转写

DashScope 设置显式保存 Beijing 或 Singapore：`https://dashscope.aliyuncs.com` 或 `https://dashscope-intl.aliyuncs.com`。这条路径在录音结束后上传已完成音频，不是 Soniox 式麦克风实时流。backend 将所选 HTTPS origin 转成对应官方 WSS host 加 `/api-ws/v1/inference`。连接使用 `Authorization: Bearer`，通过带 UUID 的 `run-task` / duplex 配置启动，等待 `task-started` 后才发完整音频；全部音频发完后发送 `finish-task`，仅在收到明确的 `task-finished` 时接受完成。模型固定为 `qwen-audio-3.1-asr-flash-message`，`disfluency_removal_enabled:false` 保留原始识别，interim results 不进入最终转录、cleanup 或 History。普通 HTTP `qwen-audio-3.1-asr-flash` 未接入此功能，也不标为可 raw-preserving。

Qwen Message 不提供已查证的固定语言提示字段，因此该路径使用多语言自动识别，不造出语言提示设置。只接收最终句子；服务若返回句子 / 词时间戳则按毫秒保留，heartbeat 不是转录，task-finished 的累计 usage 只记录一次。7168 输入 / 1024 输出 token 上限带来整段处理风险；VoiceFlow 对较长录音使用 15 秒目标、20 秒上限和 1.5 秒重叠的分段，并保留完整采集音频（包括安静片段）以保证覆盖。若任何块失败或达到输出预算上限，整段转写失败，不交付部分结果，完整 WAV 留在 History 供重试。此实现未验证实际账号权限、识别质量或延迟。严格离线会阻止这些云端路由、History retry、provider probe、预取和仍在进行的请求。

## 2. 长录音

1. 超过分段阈值后，音频按 chunk 处理（OnDevice MLX 对齐 30 秒分块上限）；AssemblyAI 的每个 Sync 请求最多 120 秒，长录音与 DashScope 的保守 token 限额路径都必须由有界重叠 chunk 完整覆盖；HUD 在所有常规识别、重叠分块、重转写与整理阶段统一显示稳定的「处理中」，仅在 429 限流退避时显示「限流重试」（不暴露分块计数）。
2. 所有 chunk 完成后按来源音频的时间戳（对无时间戳或非空格文字如 CJK/泰语另在 24 个字素边界窗口做词素级后缀-前缀重叠去重）合并 transcript，再执行一次 cleanup 和最终 protected-span guard。AssemblyAI 不会逐 chunk 改写；它只合并 Sync 原稿后做一次全稿 cleanup。Qwen Message 也使用保守 token 分段并完整覆盖较长录音。
3. Settings 的“长录音输出”使用 `delivery_policy`：

   - 自动粘贴：优先发送一次键盘粘贴；临时剪贴板准备前就失败时，仅当原剪贴板未变化才安全复制回退；
   - 写入当前输入框：同样使用 fail-closed target guard；键盘快捷键确定未发出且 VoiceFlow 仍拥有临时剪贴板时，会按实际 clipboard 结果保留文字供手动恢复；若在快捷键前取消，则按原 marker 最终所有权复核恢复旧剪贴板。快捷键可能已发出但没有精确读回证明时，按 `paste_unverified` 持久化 History；普通听写 HUD 在 dispatch 后立即收起并保持静默，不提供复制入口或自动复制 / 重试；用户可自行检查目标后打开 History。快捷键确定未发出但最终交付方法为 `history` 时，由精确事务的复制恢复入口提供手动恢复；剪贴板所有权丢失或未知时保留当前内容；
   - 复制到剪贴板：跳过键盘注入；
   - 仅保存到历史：不修改当前 App，也不覆盖剪贴板。

4. 任何部分 ASR、AI cleanup 或 delivery 降级都会保留 raw/final、原因和可恢复音频到 History。

长录音 spool 为每个分块记录来源样本起点和样本数；恢复时按整数样本区间合并相同的重叠部分，并保留每个来源样本一次。来源区间有缺口、重叠样本内容冲突或新旧区间元数据混用时，恢复会安全失败。旧的非重叠 spool 仍按分块索引串接；旧 manifest 如果显示重叠、但没有精确样本区间，则不能精确重建，也会被拒绝。

Spool 保留期按 manifest 的 session 创建时间计算，不会因启动恢复写入或重建失败而延长；重建失败时，未到期的源分块会保留到原定到期时间。短录音为 History 写入失败准备的单 WAV session 也使用 manifest 和同一保留期限。`keep_audio_days=0` 表示不保留 recovery 音频；启动清理会删除新建的同秒 session 与旧式平面 WAV，不会因秒级年龄仍为 0 而重建它们。创建时间缺失、无效或位于未来的 session 会保守清理。

HTTP providers 使用 batch ASR，且当前都报告 `streaming_partial_results=false`、`streaming_final_results=false`；只有 Soniox 报告 partial 与 final streaming capability。该 capability 描述协议接线，不代表真实录音行为或延迟收益已验证。Provider 时长与文件限制由 adapter 能力决定：SiliconFlow 报告 1 小时 / 50 MB，Mistral 报告 3 小时；未声明的 `max_audio_duration_secs=null` 不代表无限制。HTTP batch 路径下，VoiceFlow 的直接请求都不超过 10 分钟，录音不超过 15 分钟；超出直接请求上限的处理采用完整覆盖分段。batch prefetch 只服务这些 HTTP providers：它在录音期间于后台转写已完成的非 warmup 分块，但不向 HUD 显示预取文字，也不会写入剪贴板、外部 App 或 History。Soniox 真流式路径不再对相同音频启动 batch prefetch。HTTP 分块规划按经 cue 屏蔽后的音频上的语音起始校准样本（低于 0.003 RMS 门限的极轻首词会在结束时保守回退 VAD 全稿以防截断）和固定最大边界运行，不受音频设备 push 大小影响。对 HTTP provider，录音结束后只有最终分块与预取块的来源区间、精确样本内容、ASR 请求设置、录音 generation、配置 generation 和录音目标绑定都相同时才复用完整 transcript；缓存包含原始文字、分段、词时间、语言、置信度和 quota 元数据。取消、新录音或会影响当前处理的设置变化会丢弃迟到结果。若录音仍有效但交付目标改变，会保留可恢复的转写结果并通过现有剪贴板回退，不会向新目标注入；绑定到旧目标的上下文 / 视觉证据会丢弃。读取旧缓存 quota 不会覆盖较新的实时限流状态。静音裁剪、压缩或设置变化导致身份不一致时会重新转写，确保尾部和变化后的分块仍有识别覆盖。

录音预处理压缩长静音并裁掉数字静音，同时根据本次录音较安静的帧自适应判断语音边界；单凭固定音量门槛不会删除整段低音量语音。数字静音仍返回无语音。

Groq Whisper 的 `verbose_json` 若返回顶层 transcript 与 segments，至少一个非空 segment 保留时，provider 顶层文字会原样进入后续准备流程；segment 置信度仍过滤其元数据，供现有级联判断使用。若没有非空 segment 保留（顶层文字为空时仍从保留段重建），只有 segment 文本忽略空白后完整覆盖顶层文字时才按空结果抑制；覆盖不完整时保留顶层文字。空 segments 仍沿用 provider 顶层文字。该规则适用于 Groq Whisper 原生路线及显式 `groq/` Whisper 模型路由；其他 provider 与未带 `groq/` 前缀的自定义端点沿用其现有流程。

普通与长录音先在本机按同一顺序执行语音标点 / 有界布局、用户词典与局部明确改口，再由本地解析 CleanupIntent。独立的中文口述标点词可以映射成符号；紧跟「把 / 用 / 加 / 换成 / 改成 / 删掉 / 输入」等编辑动词的标点词保持原文。局部改口排除引号与 Markdown code literal 区域。列表改口只在同一块中用显式 ordinal cue 唯一指向较早的编号项、且目标项、占位项、cue、reference、replacement source span 均逐字验证后，才替换那一项；cue 仅匹配完整措辞（包括 `spoken_revision` 的标记及其内部标点），proof 的来源字节范围绑定到标点保护检查通过后布局解析实际接收的输入；若标点变更未通过保护检查，布局会回退到布局入口文本并从该文本建立来源范围。proof 向最终保护 owner 提供这份布局输入的来源证据，不表示每个范围仍对应 provider 原始 ASR 字节。尾项明确计数保留也接受“算了吧，只要前两个”及“只留前两个”，仍须产生布局输入绑定的 retention proof；明确“不要第 N 个”或“删掉第 N 个”仅在同一块有连续编号列表且 N 指向实际列表项时删除该项，剩余项按原顺序重新编号。列表渲染会去掉项目末尾的分隔逗号，但 proof 仍保留布局入口 list_source 的原始 value 与 byte span；该坐标位于标点处理之后，不保证对应 provider 原始 ASR 的字节；数字、日期、代码、否定和其他 protected 内容不因渲染变化而改写。目标缺失、重复、歧义或修正语句处于 literal 中时，原项与改口 cue 都保留；解释性、假设和引用文字不会授权改写。没有显式回指的尾部“不对”仅在相邻的下一编号且既有 backtrack parser 确认时作为 false start 处理；其他情况不会猜作 forward correction。标点与每个布局阶段分别以该阶段完整输入定位引号 / code literal；只有解析器实际成功消费的精确语法范围可豁免自身 protected-span 匹配，每步检查通过后才进入下一步。payload 中的同词、部分重叠实体、引号内容和文件路径仍受保护；Markdown code 的外层 fence / backtick 可省略，但 code payload 必须作为完整 token / expression 逐字节保留，不能用扩展后的标识符、flag、路径、成员或调用中的子串代替，并保持原顺序、重复次数和代码块缩进；省略包装符号时，只要求保留 literal 边缘原有的 code attachment（比较局部语法时规范化空白），新接入的成员、调用、flag 后缀或运算仍会被拒绝，周围普通 prose 仍可按现有 cleanup 规则编辑；紧邻 CJK 正文时，只用原边缘相邻字符作最小锚点；重复或重叠正文必须能按顺序唯一绑定，否则保守拒绝；每条 transcript 最多绑定 64 个 literal、每个正文最多枚举 256 个候选出现，超限保守拒绝；布局标签只移除空白和明确的命令分隔符，保留文件名等 payload 点号；code literal payload 不计作 prose 的布局内容，口述布局恢复只以候选中已逐字绑定的 occurrence 作占位并从候选原样移回，不会从原稿补入缺失或修改的 payload。启用 cleanup 时，准备稿是模型编辑目标。仅在 Groq 的隐式 Cleanup 且完整 ASR 原稿与准备稿不同时，共享请求构造器会省略原始 ASR reference，以准备稿作为唯一文本依据；原稿仍保留在本机 History 与来源记录。其他 provider 和明确选区 / 操作沿用现有 reference 行为。语气试跑内存响应在最终 guard 前分别保留完整解析后的 provider candidate 和共享 cleanup adapter 接受后的文本；二者均不写入普通成功 History，adapter candidate 不代表未经处理的模型 wire response。最终验证不再对所有出现位置追加全局词典改写，并会拒绝丢失结构化行内容或改变代码缩进的候选。当整理关闭或场景被路由为本地处理时，不发起整理请求；短、长录音的 Off 结果都经过同一最终本地保护检查，并保持已准备的文字不变。provider 失败或验证拒绝时使用保守 fallback 并保留准备稿布局。未携带共享 cleanup plan 的遗留 Groq Cleanup 路径仍会追加固定列表保真提醒，并只在候选因遗漏编号列表前言触发布局拒绝时尝试有限恢复；恢复后的文字仍经过完整保护检查。该 legacy 提示不适用于普通短录音、长录音和 History 音频重试的共享 plan 路径；这些路径不追加该 reminder，且被拒绝的 `StructureSafe` 候选不会通过 preamble recovery 复活。其它非 StructureSafe 路径仍保留既有的受证明恢复规则，失败后走对应的本地 fallback。`asr_text` 保留 provider 原始结果，便于区分原始识别与本地准备后的草稿。

普通录音与 History 音频重试共享一份 effective cleanup policy：Auto 仍按既有强度解析，并只选择一个场景输出 add-on；手动 Email、Bullets、Meeting Notes 仅在有已捕获多行能力的兼容目标应用。本机只在来源 cue 足够明确时渲染推断段落 / 列表；证据不足时保留原呈现。History 音频重试没有活动目标的 multiline 证据，使用当前保存的场景 / 强度和翻译目标，但手动 layout mode 回退到该场景的 Auto add-on，不推断新布局。提供或本地渲染的结构使用 `StructureSafe`：provider 只能调整受保护 literal 外的安全标点，必须逐字保留所有词、大小写、空白字节、换行、列表前缀和缩进；canonical 和 projection 的既有 source authorization、literal、实体、列表 proof 与 finalizer 检查仍有效。普通 Auto cleanup 仅在有原生 multiline 证据的 prose editor 中允许模型选择句末主题段落，唯有精确 Codex target 在原生多行角色和 ASR 自然语言推断共同确认 CodingPrompt 时可从观测到的 Code route 进入；provider 与 final guard 按顺序逐段对齐准备稿正文，只容许既有句子边界和已允许的安全标点，无法证明时回退到 canonical 准备稿。单段结果继续使用普通 cleanup；Code mode、未知 / secure target、StructureSafe 来源投影、S1-mini 和 AssemblyAI fused 不启用此段落策略。Code 模式的普通听写与音频重试不调用共同 cleanup；AssemblyAI 此时走 raw Sync，不走 fused Dictation；既有明确片段匹配仍按原路径处理，未匹配片段时识别稿原样通过。没有输出投影的模型请求（口述或 History 的 rewrite / shorten / formalize / casualize 命令、History 已选择文字 Reclean、语气试跑、评测和启动 warmup）同样由共享 prompt 构造：Cleanup / Translate 按有效强度选择 Light / Standard / Heavy 预算（命令强度按 Standard），其他已授权操作使用单一 authorized-operation 预算；add-on 取当前 profile 场景的 Auto add-on（Translate 用 Translation add-on），不使用手动 layout add-on，也不选择 `StructureSafe`。翻译仍使用现有固定目标的 `Translate` operation。AssemblyAI 其余 fused 行为和指令保持独立；History 的已选择文字 Reclean 也不使用 live-mode Code 路由。

在没有 `StructureSafe` plan 的 adapter 路径中，若同一 code literal body / attachment 绑定失败并同时导致布局检查拒绝，adapter 报告 `protected_token_changed`；其他布局拒绝仍报告 `explicit_layout_changed`。`StructureSafe` 路径先运行标点专用 guard，这类候选会按结构拒绝处理，不走该 taxonomy。provider 回复与最终输出各由同一个 guard owner 检查一次布局、文字体系和受保护 token；最终边界另外保留只在本机准备阶段可得的 proof 与已晋升词对检查。

列表 owner 也只消费有 proof 的明确连接词、误起填充词和同一列表块里的保留指令；它不会全局删除「是 / 才是」，而会保留「是否 / 是不是 / 是什么 / 是的」、否定、强调和不确定语气。仅当 2–10 项的计数保留指令有连续序号证明，且相邻前一项的明确改口把一个值改成新值、后一项以「才是」开头并逐字复用该旧值时，来源 proof 才记录该连接词的精确来源与输出范围；准备稿 owner 在模型请求前按 proof 同步规范化该处，更新保留项和修订输出范围。provider adapter 与最终布局检查因此都检查同一份规范化准备稿，其他同词仍须保留。明确的「啊算了…只保留前两个」会按同一组 cue、动作、数量和连续序号事实解析：同一块内必须有末尾编号项中的直接撤回 cue，保留数量在 2–10 之间且小于当前编号项数，才撤回该数量之后的项；旧式「前面两个，第一个和第二个」仍要求唯一匹配前两项的显式回指。疑问标点组合、缺项或序号不符，以及角色不明的指令都会完整保留。指令里的「如果分支」等内容词本身不会把紧邻的实际指令改判为假设。跨段落或先前已交付的内容不受影响。通过解析的列表项正文可清理边界清楚的呃 / 嗯 / uh / um 等填充词和单个明确误起；「嗯嗯」等有意义的重复、普通词语前缀（例如 `umbrella`）与重复强调（如「非常非常」）会保留。模型遵循同一清理边界，但最终输出仍须通过来源、实体、否定和布局验证。

词典设置允许用户手动创建明确的「听写识别为 → 应替换为」纠正规则，并支持三种作用域：全局规则（`GlobalRule`，允许云端热词、`pairs_hint` 与 CJK 谐音 / ASCII 模糊召回）、原生 App 范围（从已安装与正在运行的原生 App 列表选择 `bundle_id`）以及网站域名范围（`browser_host`）；不要求先建语气 / context mapping，也不编辑旧 History。作用域规则通过当前场景适用集进入本地替换、提示、使用计数和最终来源保护，而不会外发至云端；全局规则写入 `DictionarySource::GlobalRule` 并参与云端词表与模糊/谐音召回。未知目标范围不匹配。History schema v20 为普通词规则使用包含 scope、词面和匹配策略的 opaque identity（`word:v2:<sha256>`），因此同一词对可由 A、B 各自持有；同一有效范围内别名重叠而目标不同的规则统一 abstain。生产 History / live 弱证据候选不会按类别门槛自动晋升；若旧 lower-case pair identity 已有一条 live 规则，另一 scope 的同 pair 候选继续 pending；用户仍可在 History 或单字 HUD 确认胶囊选择「全局记住」或仅当前范围生效。修改 mapping 身份或删除 mapping 会暂停旧绑定词规则，须经 Settings 明确 rebind。旧 v19 词对行不重键、也不向 v2 行转移计数。

普通 History 捕获额外保存所选 user mapping 的 selector 指纹，仅用于验证 bundle、executable、browser host/path 与 focused-field selector 是否仍是产生这条记录的同一声明。旧 History 行没有该指纹时，History 学习建议与手动编辑学习会 fail closed；Settings 中的显式 rebind 仍可把已暂停的规则绑定到当前 mapping。mapping 删除或 selector 改动后，SQLite 先提交 binding invalidation，并取消该 scope 的待执行 Add、排入只移除对应 Rule owner 的 Remove；通知与在途听写取消在数据库提交后发生。若 Settings 文件写入失败，Remove 保持 pending，后续 Settings 同步 / 启动可重放；Manual、Screen 等其他词条来源不受影响。主窗口 capability 已授予 `rebind_invalidated_learn_pair`，普通学习审查会严格展示匹配策略、mapping binding、manual/auto owner 与 rebind / 跨范围候选决定。当前同一 App 范围出现别名重叠但目标不同的已晋升规则时，两条均从“已生效替换”移入冲突暂停组，用户可忘记其中一条解除冲突。大小写纠正规则在 admission 与最终 cleanup 比较都按 exact casing 检查，并跳过引号和 code literal；普通规则沿用 ASCII 大小写不敏感匹配。每规则 replacement count 与既有 `learned_term_usage` 词条汇总并行维护。v19 原始 key、hits、tombstone 与证据不会重键或清除；与 v20 live 词规则发生同范围冲突时会共同 abstain，v19 tombstone 不会被 passive v20 upsert 绕过。V2 与 V3 phase fixture（`research-harness`）现已对齐 C1 `word:v2:<sha256>` 键协议（`c1_word_rule_v2`），跨 scope 各自独立成行且清除了 `LEARNING_PHASE_V3_KNOWN_GAPS`。相关外部研究仓脚本可通过环境变量 `VOICEFLOW_RESEARCH_CHECKOUT` 指定本地路径。

共同云端整理只有协议明确成功终止时才产生候选：OpenAI-compatible 流式需要 `finish_reason: stop`，已收到 stop 后的正常 EOF 可接受；`[DONE]` 本身不能替代完成状态。Anthropic 需要 `end_turn` 或 `stop_sequence`，工具内容不作为整理结果。缺失完成状态、过滤、工具调用或 token 预算耗尽走完整准备稿的本地回退。共同 LLM 和视觉客户端与 ASR 一样不跟随 HTTP 重定向。

Groq 的 Cleanup 候选若只因省略编号列表前的准备稿前言而未通过布局检查，adapter 会尝试一次保守恢复：准备稿与候选必须各自只有非空前言及同一连续编号块；编号、顺序、数量、分隔和每项内容必须匹配，候选前言只能是准备稿内容的删减。嵌套列表、尾注、引号或代码 literal、数字、否定、实体或文字系统变化等歧义都会拒绝恢复。恢复后仍运行原有完整布局、脚本和 protected-span 检查；失败仍按原拒绝与回退处理。仅 Groq 的 Cleanup 使用此路径；解析后的 provider 候选保持原样，只有通过全部检查的组合文字被接受，并记录固定的 `restore_prepared_preamble` 转换。它不增加请求，也不改变本机或其他 provider 路径。

History 将 provider 返回的原始文字写入 nullable `asr_text`；如果 ASR provider 在同一响应中明确给出独立融合候选，则另存为 nullable `provider_cleaned_candidate`。本地置信度过滤、口述修订和词典替换后的识别草稿在 `raw_text`，最终保护检查后的输出在 `final_text`，并记录实际产出识别草稿的 ASR provider/model。语义保真检查拒绝的完整 cleanup provider 文本与 typed reason 分别保存在 nullable `cleanup_rejected_candidate` 和 `cleanup_guard_reason`，与可能来自 ASR 融合路径的 `provider_cleaned_candidate` 分开。两列只记完整合法文本被语义 guard 拒绝的结果，不保存取消、协议错误或截断内容；History 折叠展示并沿用现有本地数据保留、删除与 JSON 导出。候选不写入普通日志或诊断，也不再发送给 provider；复制仍只使用 `final_text`。各字段保留各自来源，不会用本地或最终文字伪造 provider 候选。正常听写不会持久化已接受的 cleanup 结果；语气试跑的内存响应会分别返回完整解析后的 provider 候选、cleanup adapter 接受的候选（在最终保护检查前）、最终 `text`、实际 adapter 转换，以及适用时的 typed rejection stage/reason，方便比较各个阶段。provider 候选是解析后的完整消息，不代表未经处理的 wire response。`provider_cleaned_candidate` 仍表示 ASR 响应中的独立融合候选，不充当 cleanup 模型候选。History 重新整理与音频重试会绑定启动时的处理配置代次；provider/model、AI Off 或严格离线等相关设置变化会取消旧操作，避免旧请求重试或写入剪贴板 / History 修订，也不会把取消当作本地回退。History 重新整理从 `asr_text` 原稿重新执行本机准备与共同 adapter/finalizer，不使用已处理的 `raw_text` 或 `final_text` 作为源。旧记录若缺少、为空或仅含空白 ASR 原稿，会沿用“没有可重新整理的原文”错误，不回退到处理稿。完整序列化请求仍由本机 4096 UTF-8 字节准入校验，不截断。超限时保留回退稿并保存 `native_input_too_large` 原因，History 提示实际与上限字节数。重新整理在等待 provider 前捕获剪贴板 lease；只有成功结果且原剪贴板仍匹配该 lease 时才复制。失败或降级不复制；剪贴板变化、所有权未知或写入失败时，结果保留在 History 并记录交付状态。音频重试会保留本次重新识别的完整原始转录供请求路由判断；如果目标是 Groq 且实际 intent 为隐式 Cleanup、原稿与本机准备稿不同，共享请求构造器会省略 raw ASR reference，最终请求只含准备稿。原稿仍保留在本地 History 与来源记录；其他 provider / 明确操作沿用现有 reference 行为。只重新整理选中文本时，不附加旧的 ASR 转录。操作 lease 保持到该操作返回。旧记录迁移后 `asr_text`、候选和缺失的 provider provenance 保持为空；History 单独展示这些来源，JSON 导出会保留 nullable 字段。 新记录还可在 History 详情中显示整理请求类别、首次 provider 失败原因、独立的最终输出保真检查原因和已观察到的响应调整代码；provider 候选仍只在真实候选存在时显示。旧行的缺失 provenance 保持未知；最终保真检查拒绝不覆盖先前 provider 失败事实，失败也不会伪造 provider 候选。响应调整描述实际接受回复的代码，不保证该调整后的文字成为最终输出。编辑状态支持取消与 Esc，保存前停用复制、重新整理及删除，避免操作旧版本；`unverified` 显示交付未确认，不能冒充已复制。macOS History 音频重试在开始时捕获有界的本机 pasteboard snapshot；现有 owner 在写入前核对所有 item/type/data，并用临时文本和 owner marker 验证写入。快照仅保留在内存，不发给 provider 或持久化。起始捕获失败或写入前发现变化时转写只记入 History；其他平台缺少相同 native 准入时也只记入 History。写入 owner 的结果以 typed delivery fact 保存，成功才显示复制；AppKit 无原子 compare-and-write，最终复核与声明写入间保留窄竞态。词典加载独立保留各来源的最后成功数据，失败显示重试；学习撤销只在成功后关闭通知，旧操作迟到不关闭新通知。

普通听写的本机性能诊断只在当前进程保留最多 32 个 provider/model/path 分组和每阶段最近 128 个延迟样本。它分别汇总静默预识别、最终 ASR、整理、最终验证、粘贴提交、读回确认和停止到已确认插入，并区分已发送、已确认、未确认、复制、仅历史、预览、失败与取消。ASR 用量记录实际 HTTP 请求与失败数（包括队列重试）、提交 WAV 时长和可解码时长的请求数；流式路径另记录尝试 / 失败数、session 墙钟时长、发送音频秒数和重放音频秒数。墙钟时长与发送音频量是诊断指标，不等同于服务商报告用量或费用；Soniox 按实时流完整时长计费，包含静音。provider 返回的最多 8 种固定单位和数量保持独立，不换算为费用。键盘快捷键只发送一次；目标 / 焦点改变、取消或读回 unknown 不会触发第二次注入。当已知预期 splice 且 `value_before` 可读时，在同一个 AX field anchor 上按固定间隔轮询最多 10 次；当 `value_before` 不可读时（如终端 / Ghostty / Canvas），发送快捷键并等待 40 ms 稳定后立即以 `paste_unverified` 结算而不轮询。AXValue 只接受完整预期 splice。只有键盘快捷键确定未发出且临时 marker 仍由 VoiceFlow 持有时，才会按 `clipboard` 方法保留文字供手动恢复；取消在同样的发送前和所有权条件下会恢复原剪贴板。键盘可能已发出但没有精确读回证明时记为 `paste_unverified`，继续持久化 History；普通听写 HUD 在快捷键可能已发出时收起并保持静默，不提供复制入口或自动复制 / 重试；快捷键确定未发出且最终方法为 `history` 时，用户可通过精确事务的复制恢复入口手动复制。剪贴板所有权丢失或未知时保留当前内容。停止到插入只统计读回确认成功的粘贴，AX 安全编辑会记入读回确认时间。诊断不包含文本、音频、上下文、图片、目标、URL、endpoint 或凭据；用户可在设置中读取并主动复制 JSON，不会自动上传。

## 3. 选中文本助手：preview-first

```mermaid
sequenceDiagram
  participant U as 用户
  participant V as VoiceFlow
  participant T as 原目标 App
  U->>T: 选中文字，或聚焦可编辑字段 / 空回复框
  U->>V: 按独立快捷键并口述明确操作
  V->>T: 通过 Accessibility 捕获来源与目标标识
  Note over V,T: 来源上限 16 KiB；无来源或超限时安全失败
  V->>V: ASR + 六类操作规划
  opt 空回复框中的起草回复
    V->>T: 检查当前 App 规则并捕获有界附近 AX 文字
  end
  V->>V: 请求整理服务并运行有限事实保护检查
  V-->>U: 以事务 ID / 序号显示可编辑预览
  alt 用户确认
    U->>V: 可选地编辑文字，再按按钮或 ⌘+Return 确认（非 IME 组合态）
    Note over V: 手动编辑按用户提供的最终文字处理，不标为模型已验证
    V->>T: 由 restore_after_preview 隐藏自动打开的主窗口并恢复目标 App 焦点，复核 App / window / field / source
    alt 精确来源和交付目标仍可验证
      V->>T: 替换已验证选区、字段全文或空回复框
    else 来源 / 目标变化或交付不可验证
      V->>U: 复制预览文字并提示只复制
    end
  else 用户只复制
    U->>V: 复制预览（并恢复目标 App 焦点）
    V->>V: 写入剪贴板
  else 用户取消或开始新动作
    U->>V: 取消（并恢复目标 App 焦点）/ 再次触发
    V->>V: 使该事务失效并清除临时内容
  end
```

支持的操作是改写、精简、翻译、结构整理、起草回复和按指令修改一个值或术语；模糊指令或保护检查不支持的结果会安全失败，不创建预览。捕获不到 AXValue 或精确选区范围时，选中内容只能用于生成和复制。完整字段替换要求可验证的辅助功能字段值，不会写到不确定的插入光标。系统不从环境剪贴板读取来源，也不按 Enter 发送内容。

结构整理可由明确的“整理成开发任务 / 工作进展 / 待办”指令选择三个封闭 recipe；quoted cue、否定 cue、冲突选择或其它操作不会携带 recipe。recipe 只加一个 Rust 定义的中英固定标题，并按来源原有空行给正文段落加列表标记；不改写、拆分、合并或重排正文，含代码 literal 的正文保持原样。预览携带有限 recipe ID / 版本，并显示模板和来源。该布局 proof 不能证明全文语义等价；有限否定 guard 会将受支持的否定标记绑定到后续最近的内容 token，允许有限形式变化 `don't` → `do not`（完整 `do not` 作为一个表达式范围），并忽略标点和空白差异。不同内容序列只接受所有最高分纯内容 LCS 对齐都把否定目标映射到同一预期位置的情况；重复词产生歧义时保守拒绝。该规则是有限词面证据，不是完整语义证明；否定目标 token 本身的同义替换不能建立词面对齐；未匹配或歧义沿原 preservation fallback。G2 的冻结任务、原生 App 和用户验收仍需独立完成。
模板在 16 KiB 结果预算内预留固定标题、换行和列表标记开销；无法完整容纳的来源会在请求整理服务前报来源过长，不会截断正文。

这些操作的语音指令经过共享录音 / ASR 路径；长音频仍完整覆盖分段。若 ASR 失败、需要重试，完整录音按恢复策略保存在 History；History 重试使用完整音频并且只复制结果，不恢复原来的目标字段，剪贴板准入与结果状态沿用前述 History owner 规则。操作来源文字、指令和预览仍不写入普通 History。

每个动作有一次性事务 ID 和递增序号。后端只消费一次匹配预览；重复确认无第二次效果。取消会立即关闭并 tombstone 该预览，并向后端发送匹配事务的取消请求，即使确认 / 复制 IPC 已经开始等待；是否已提交由后端 owner 的 commit 边界裁定：commit（含 ⌘V 与读回）不持有取消用的锁，取消立即触发令牌；若取消时 commit 正在进行，预览取消命令返回错误且不发 `cancelled`；全局取消（Esc、取消命令、设置变更、CLI）同样只触发令牌、不认领该动作，也不发 `cancelled`，由 commit 自己的结果（`replaced` / `unverified` / 过期 / 取消）为准，⌘V 发出前被取消则 commit 返回取消并由其补发 `cancelled`。取消 IPC 未确认时显示状态未知，不把它说成已取消；迟到预览不会重建对话框。有效的迟到确认 / 复制回复只展示其返回的 outcome，不会重放动作。用户确认前，选中文本、指令、页面文字和预览结果均为临时数据；操作本身不写入普通 History、纠错学习、HUD 或日志。确认时在修改前发现来源或目标变化会复制结果；如果已尝试写入但系统不能确认结果，或 ⌘V 可能已发出，则安全报告未验证（不发 `replaced`，也不再复制或重试）。事实保护是有限规则而非语义证明：例如否定标记按有限规则检查；翻译只识别受支持的日期 / 金额表示，其它无法验证的变化会失败。DraftReply 仅在空回复框和当前权限允许的有界附近文字下运行，不承诺开放式事实核查。

## 4. Screen assistant / 看屏幕

自动上下文是逐 App opt-in，与手动「看屏幕」热键分开：

设置与空闲预览仅在引导完成、`context_enabled` 打开且操作租约空闲时运行；关闭总开关或尚未完成引导时跳过前台焦点 / 浏览器检测。设置变更会在本机把缓存预览改成 General 策略并应用当前手动覆盖，浏览器状态标为关闭；被跳过的忙碌预览不增加检测代次，也不改写录音会话已冻结的目标。听写、选中文本和看屏幕各自在用户触发后执行自己的新鲜目标捕获，自动上下文关闭或引导中的「试用」不会取消该次手势的输入类型分类。

1. `context_enabled` 打开后，场景与当前目标仍在本机分类；AppMapping 的 bundle / executable、网站 host / path、focused field 所有已填写条件必须同时匹配，缺少信号就不匹配。具体站点 / 路径 / 字段规则优先于通用浏览器规则，同等条件按 mapping ID 决胜。
2. 新旧 mapping 的 AX 文字、OCR、云端视觉和 `context_text_to_providers` 权限缺省关闭。只有匹配规则许可 `ax_text` 才读取有界 AX 文字；ASR / cleanup 只有在 `context_text_to_providers` 同时开启时才可收到有界 terms / 选中或邻近片段。获准的 OCR 派生文字也可能离开 Mac。
3. 如果 AX 内容不足，只有具体 App / executable 规则获准 `local_ocr`、全局窗口 OCR 打开且有屏幕录制权限时，才截当前窗口一张内存图供 OCR。若仍不足，且 `cloud_vision`、`context_text_to_providers` 和用户配置的 vision provider / model 都就绪，同一张图才会作为云端回退；不捕获整屏，不选择其他 provider。AX / OCR 足够时跳过云端。
4. 请求发出和结果返回时都会验证权限、录音 session、目标与页面 fingerprint。撤销权限 / 取消 / 目标或页面变化后丢弃证据或结果；图像不落盘、不进 History / 导出 / 日志。
5. HUD 只显示实际送入当前 provider 请求的来源（AX、设备端 OCR、云端视觉或无），不显示原始内容。raw URL、query、fragment、窗口标题、PID 和目标 identity 只用于本地路由与绑定。

Context Settings 的三个逐映射隐私预设只修改当前草稿，保存规则后生效。「关闭上下文读取」关闭 AX、OCR 和全部来源外发批准；「仅本机使用」允许 AX / 本机 OCR，但关闭所有文字、图片、风格样例和结构化偏好外发；「允许文字发送给转写与整理服务」使用现有共享 `context_text_to_providers` grant，允许获准的有限文字进入 ASR 术语提示与当前 Cleanup 请求。此预设不打开 OCR 或云端视觉，也不批准风格样例、结构化任务偏好；这些权限保持独立。Off 与 Local 预设保存后均 fail closed，阻止所有上下文外发。

### App × writing task 显式偏好

「写作与整理」合并全局整理和原语气设置。四张核心场景卡分别覆盖个人聊天、工作沟通、邮件与文档、代码与终端；展开后可选择全部 15 个细分场景。规则优先级从高到低为本次快捷键指令、App 专属规则、场景语气卡、全局默认档位。个人聊天默认提示保留各语言口语语气词，并允许单句不加句号；本机失败回退对该场景保留准备稿，不运行通用口语词清理或自动补终止标点。

Context Settings 可在 `AppMapping` 中显式选择 `writing_task_id`，并按 `task_id × dimension` 保存、暂停或忘记受支持的结构化偏好。任务身份可由唯一匹配的 verified focused-field selector（同时有活动输入 token 和窗口 identity）、verified browser host/path selector（同时有当前 URL / target token），或已知原生 profile 的 bundle ID 唯一限定；bundle-only 仅对已知原生 App（例如 Slack、Mail、Codex）开放。浏览器和未知 bundle ID（包括 Chromium）仅凭 App 身份不符合条件。同特异性歧义、重复匹配或缺少正向 identity 时保持 unknown，偏好不召回。相同 App 可通过不同 verified field/path 规则区分任务。

列表偏好只在本机整理路线影响有逐项来源边界、顺序和事实可对齐的 implicit cleanup；云端请求始终使用未应用任务列表偏好的文本。称呼只处理原话已有且紧跟标点或空白边界的 `你好/您好`；句末标点只处理支持任务中的单句、无歧义陈述句，并拒绝含有内部中英文句号的内容。只在 Auto 模式、无显式单次命令且整理 plan 允许时生成结构化指导；本次 mode / command、任务 / 内容不兼容、StructureSafe 预算和 S1-mini 都跳过。云端结构化偏好 permission 单独显式开启且默认 false，与 `context_text_to_providers` 和 `style_examples_approved` 各自独立；撤销样例授权不会改动结构化偏好授权，反之亦然。

发送前和每次 retry 都会重新读取 `context_enabled`、当前 App / task、敏感目标和 cloud permission，并对照偏好 revision。目标或授权变化会拒绝 queued request 并走 canonical 本地 fallback。mapping save 使用 `preference_revision` compare-and-save；pause/forget 记录 task-scoped 状态并保留 forgotten tombstone，删除 mapping 会清除其余内容并仅保留 mapping ID 与 revision tombstone，阻止旧设置视图重建原 mapping。通用 `update_settings_patch` 与后台 learning snapshot 不能覆盖或复活已忘记或已删除的偏好。

History 的人工改字现在可产生 metadata-only 弱证据候选。每个 History 页面会生成编辑 session UUID；保存 payload 记录有限输入类别、composition 状态和 trusted Save 动作，IME 仍组字时 Save 禁用。只有 VoiceFlow 自有 History 编辑器、可信任务及 mapping identity/revision、无冲突的最新 History 来源和 classifier 支持的有限表面编辑可进入弱证据；同一听写只有对原始交付 History 文本的首次可信手动编辑可以贡献样本，而且原始交付后必须尚无任何 History revision；先前的 `ai_reclean`、恢复或其他 action revision 均使该来源不合格。后续保存不增加独立数。此限制只用于写作偏好弱证据，不改变既有 lexical History 学习行为。单次 Verbatim 绕过会被排除，但全局关闭整理后仍经本机 `PreparedTranscript` 路径的记录可以进入分类。AX 和 paste observation 不能作为 author / IME proof。分类只记录候选值、origin/content/template/session 指纹及时间，不复制原文，不自动改变后续输出。候选窗口 14 天，至少 3 个不同 origin/content/template、2 个自然日和 2 个编辑 session；自然日按原始听写 `dictations.created_at` 通过 SQLite `localtime` 计算，编辑时间单独计量编辑证据的 14 天窗口；候选读取和复核时，原始听写来源也必须仍在滚动 14 天内。元数据在编辑后 14 天或来源满 14 天时较早到达的期限清理，因此第 13 天编辑的来源到第 15 天时已过期。同 origin、内容或模板家族重复不增加独立数。同一有效 mapping selector × task × dimension scope 中，当前 14 天窗口内未忽略的反向值证据会阻止候选就绪，即使跨 generation；忽略只关闭当前 generation，该 generation 不再支持或阻塞后续候选。候选证据有界保存（最多 256 行）。

这里的 256 行只计算 `writing_preference_evidence` 中的弱证据元数据行；达到上限时拒绝新增证据，不会裁剪或删除对应的 History 来源记录。来源内容仍由常规 History 保留期和用户删除操作管理。

模板族 hash 是粗略结构指纹：同一分句中的同脚本短语折叠为一个形状标记，单独改变词面或词数不能制造不同家族；数字槽、内部标点和段落边界保留宽泛结构，句末标点变化本身不会增加多样性。它只保守去重，可能把语义不同的文本合并，不能证明模板或长期意图独立；无法计算 hash 的文本不能满足模板多样性。

用户在 Context Settings 的“待复核的写作偏好”区主动打开列表后，UI 才请求候选和当前 mapping snapshot；每项按 stable mapping ID 显示准确的 App 选择器范围、task、dimension、predicted value、聚合计数，以及固定“无原文样例”状态。每批最多选择 64 项；列表更多时，先处理当前批次，再继续选择其余候选。可混合批量批准或忽略；结果逐项返回。mapping revision 变化、权限失效、候选过期、已有偏好冲突和持久化失败会显示对应结果；最新 History source 重查时遇到 I/O 错误返回逐项 `source_recheck_failed`，该 mapping 组不提交 Settings，但同批其他 mapping 可继续；若来源已变化或过期则返回 `stale_candidate`。UI 要求刷新候选后才可重新选择。整批 IPC 未确认时也必须刷新候选后才可再次处理。批准后刷新 mapping，只在当前编辑器仍对应同一 mapping/task、范围未变且目标偏好 / 云端授权没有脏草稿时合并批准项；否则保留草稿、阻止保存并提供显式重载获批规则的操作。批准只保存本机结构化偏好，不授权 style sample 或 cloud guidance；短称呼 / 句末标点审批会撤销 mapping 级结构化偏好 provider grant。忽略只关闭当前候选代次；该 generation 的证据不再支持或阻塞后续候选，也不忘记或撤销已批准偏好。当前 14 天窗口内同一有效 mapping selector × task × dimension scope 的未忽略反向值会阻止候选就绪。

命名的 owner-chain regression `writing_preference_drafts::tests::history_edit_candidate_review_owner_chain_persists_and_applies` 在隔离 History DB 中覆盖先单项批准、再批准同 mapping 其他维度与另一 mapping 的候选、decision marker / Settings reload 和兼容的本机 bullet projection，可通过 `cargo test --manifest-path src-tauri/Cargo.toml --lib writing_preference_drafts::tests::history_edit_candidate_review_owner_chain_persists_and_applies -- --exact` 运行。它用合成 History 事务和 editor-commit fixture 调用实际 revision 保存、History classifier / metadata evidence writer、候选派生及审批 helper。此测试不是运行中的 Tauri IPC、Test bundle/macOS Host、真人输入或 IME 验收；自动推断启用仍关闭，独立 shadow、holdout 与用户收益尚待完成。

候选收集与处理要求现有 opt-ins：全局词典“学习词条”、默认关闭的“本机观察插入后的改动”、全局上下文开关、该 App mapping 的学习开关，以及可信的显式 writing task；browser mapping 还需当时可用的浏览器访问。关闭全局词典学习、本机改动观察或 App mapping 学习会清除未批准证据和 decision markers；删除 mapping 清除此 mapping 的元数据。关闭全局上下文、暂停 / 禁用 mapping 或浏览器访问暂不可用仅阻止采集 / 复核，不清除元数据。Clear All 删除候选与忽略水印，但保留已批准的 AppMapping 设置。独立 shadow、冻结 holdout、真人收益试验、真实 Tauri IPC / Test bundle acceptance 仍待完成；代码实现和定向 UI tests 不构成这些结果的证据。

手动「看屏幕」仍须用户自己录入 `screen_action_hotkey` 和配置视觉模型。触发后只截当前窗口一张内存 PNG（长边 ≤1280），连同语音指令发给用户配置的 vision provider，再弹出一次性事务预览（替换 / 只复制 / 取消）；确认会复核当前目标，目标变化或不能验证时只复制；若 ⌘V 可能已发出或结果未确认，则报告 `unverified`，不再复制，取消会丢掉 PNG。屏幕结果沿用相同的确认 / 复制 / 取消事务命令，但不会被描述为经过选中文本操作的事实保护检查。手动动作不依赖自动上下文 grant，也不会启用后续自动截图。会议录音、Spark、生图仍然不做。

## 跨 Mac 学习配置迁移

设置中的「学习配置迁移」使用原生保存 / 打开文件面板，不接受 renderer 提供的任意文件路径。导出写入用户选择的本机 JSON 文件；用户可自行用 AirDrop 等本机方式将文件送到另一台 Mac。文件版本化并有 1 MiB 上限，只包含当前可进入全局 ASR / fuzzy 词表且来源可证明为 Manual / Screen / Other owner 或仍有效全局 Rule owner 的个人词条、仍处于 live / promoted 状态且可安全重建 scope 的纠正规则，以及仍处于 active 状态的结构化写作偏好。Paused / forgotten / tombstoned 纠正规则和非 active 写作偏好不导出。仅由 scoped Rule 或 `legacy_unknown` 支持的词条不导出；无法证明来源或范围、来源 mapping 已禁用的 mapping-owned pair / 偏好，以及单项本地词面无效的全局词条 / pair 会跳过并计入 skipped 数；无效项不会阻止其余项目导出。生成文件仍会执行完整格式校验，校验失败则不写文件；即使全局词条、纠正规则和偏好都为 0，界面仍显示 skipped 数。文件不包含 History、音频、候选、审计、Keychain 凭据、权限或来源授权。

导入先用原生文件选择器打开文件并完整验证格式、版本和所有项目，再生成带短期本机 session 的逐项预览。格式错误的 bundle / host、同一 selector 自相矛盾的 selected task 等会拒绝整个文件。预览记录文件与相关本机学习状态指纹；应用时重新读取并核对，过期或发生变化就要求重新预览。纠正规则按文件里的 selector 与 pair 自身观察到的 family / host / native bundle scope 恢复，不依赖 mapping 当前选中的 writing task；写作偏好则要求 selector 和 selected task 都匹配目标 mapping，偏好自己的 `taskId` 可以不同，合法 selector 也可带 `selectedTask: null`。目标 mapping 必须唯一对应；精确目标若已禁用会阻止该项，目标缺失时与任何未删除的本机 selector 重叠（包括禁用 mapping）也会阻止创建。同一文件内重叠的目标 selector 若按运行时特异性有唯一胜者，可共同创建且结果不依赖文件顺序；特异性相同会阻止相关项，单独已阻止的项不会预留 selector。来源 mapping 若与另一条启用中的重叠 selector 特异性相同，其绑定纠正规则和偏好会跳过，因为本机 mapping ID 的平局无法移植。selector 或观察到的 host / native bundle 不一致、缺失或无法重建时会跳过。新 mapping 不替换现有映射，默认启用以参与目标匹配，但学习开关、AX / OCR / 云端视觉 / provider 文字和其它授权均关闭；预览会标明将新建 mapping。已有 mapping 的 selector、task 和 grant 不会被静默改写。

预览使用当前纠正规则快照；目的 Mac 尚无 History SQLite 时按空快照预览，不会因此创建数据库。应用前会重读并验证文件、本机状态及 pair 快照，快照覆盖已存在和用户选择保留本机的 pair；任何变化都要求重新预览。冲突默认保留本机；用户可对支持的冲突明确选替换，或为全局词条明确选择加入全局词典。实际导入的全局词条会在目的 Mac 增加 `Manual` owner，原 Rule owner identity 不会移植；纠正规则按 pair 项写入目的 Mac 本机规则库，并保留词面匹配策略与具体 scope。同一文件可为同一 scope / alias 携带 Exact 与 IgnoreCase 两条同输出规则；若目的地只有另一匹配策略的同范围规则，仍显示冲突，须用户明确选择替换。应用先保存 Settings，再写入 SQLite 纠正规则；若 Settings 已保存后原生数据库写入失败，会返回 partial 和逐项失败结果。写作偏好只在目标和 task 匹配时生效，不继承云端结构化指导许可。逐项结果区分已导入、已保留、已存在、阻止和失败；原生面板取消或关闭预览均不执行导入。导入只写入获选的本机学习配置，不迁移或改写听写 History、音频、候选、审计或用户同意记录；不访问 Keychain、不调用 provider，也不改变云端授权。

## 可选录音策略

成功领取会话启动时绑定跳过整理策略，StopClaim 携带它进入短／长录音处理；普通启动明确关闭，失败、取消和结束清理。JSON press / release / toggle 事件经类型解析，非法事件忽略。Idle release 不启动也不遗留策略；用辅助键停止普通会话不会重写它。

音频引擎可以保留一个空闲预热流，但样本直接丢弃；每次录音都重新解析所选设备并打开本次流，再创建本次 worker / spool / provider 通道。当前 CPAL Device 没有可用的稳定相等身份，因此不会凭设备显示名或采样配置相同就把空闲流重新绑定到录音会话。跟随系统默认输入每次开始时重新解析；预热不可用时按需打开，打开失败会保留实际错误并显示不可用状态。录音事件分别记录最初请求的输入选择、启动时读取的系统默认设备名，以及本次实际打开的设备名；空字符串请求表示跟随系统默认。合盖策略可能将请求替换为另一输入设备，HUD 副标题显示实际打开名，提示信息保留原请求或系统默认名称供区分；只有实际打开名来自本次流，不会把默认设备快照冒充为已打开设备。设备选择元数据进入 opt-in 研究诊断，不新增 History 持久字段。设备身份和计时只在采集阶段显示。更改选择只影响下一次录音。显式选择的设备无法打开时会提供输入设备设置入口；会话中断时仅为该会话创建原生恢复 ticket，终态提示保持可见，直到用户打开目的地、关闭提示或开始新会话。恢复 tray 初次出现时展开；主胶囊的箭头和关闭按钮控制同一代次绑定的详情 owner，折叠时原 ticket 仍拥有原生面板交互，但窗口回到 244×64，不留下空白点击带。History 重试必须匹配当前 ticket 版本和确切 spool，会与新录音及保留设置变更串行；写入前按当前 `keep_audio_days` 重新验证该音频。保留设置不允许重试或无法确认文件时，界面不再声称音频仍可恢复。恢复只写入同一会话的 History，不自动重转写或粘贴。

开始提示音在录音首个输入回调 readiness 和当前会话/取消/stop 检查之后才尝试播放。当默认输出设备确认为耳机/非外放通路时，跳过麦克风采集端的提示音屏蔽与 fanout 扣留；对于外放及未确认设备（fail-closed），采集 owner 向异步提示音 waiter 提供该代次的小型 cue handle，并在 `(cue_duration + 240ms).min(300ms)` 的有界窗口内等待子进程退出并校验成功退出才确认屏蔽。实际 child spawn 后和窗口结束或子进程提前退出时读取 macOS Mach absolute clock，超时仍留给后台收割子进程但不阻塞恢复麦克风采集。每个实际接受的 CPAL packet 使用 `InputCallbackInfo.timestamp().capture` 及 native frame 数 / 采样率，将 observed process envelope 与 packet 时间跨度求交，再映射到已有 16 kHz source cursor（worker 对齐与预取分块同样只在屏蔽后的样本上运行）；尚未有 packet 可映射时只保留小型时间 / cursor 状态并延后 fanout，不提交空范围或缓存额外 PCM。成功范围在 worker fanout 和最终 VAD 前静音，原始 PCM spool 保留。CPAL 的 capture 时间是 CoreAudio 提供的软件估计，不能证明声学采样或扬声器物理 envelope；样本级声学覆盖与物理 R4 仍 UNKNOWN。时钟或时间映射失败、spawn/wait 失败、非零退出或 admission 在 child 完成前关闭时保持原始音频并记录提示音 warning，不将可选提示音升级为采集失败。Stop 不等待提示音 child，闭合 admission 会冻结该代次的未确认窗口，迟到 waiter 不能修改它。不能把 bundled WAV 的 60 ms 长度当作屏蔽预算。

停止等待单调时钟 deadline（默认预算按 `<= 200ms` 自适应尾音排空：至少等待 `40ms` 后若已检测到语音且尾部静音持续 `80ms` 提前结束，最长 `200ms`；若用户在调试设置中显式配置了大于 `200ms` 的缓冲则完整尊重至 `2000ms`），普通命令按顺序暂存，取消 token 独立于 recorder 锁；设备故障和自动上限优先终止等待。采集槽关闭后播放结束提示音，随后完成编码和有界可选上下文刷新。可选 VAD 使用自适应能量阈值及 onset / pre-roll / hangover，完整覆盖非整帧尾部。VAD 打开时取消最终预取复用，完整音频和 Soniox 流保持全稿。

精确词典后、共同准备稿中的可选 ASCII 模糊词典阈值为 0.18，只匹配同行普通词语；换行（含 Unicode 行／段落分隔符）、制表符、代码和受保护片段是边界。先用未套模糊替换、已应用获准口述修正的准备稿识别精确片段与明确文字操作，再仅对普通听写应用模糊词典。片段匹配和操作类型绑定到替换前的输入，模糊替换不能创建新的片段触发或文字操作。短录音、长录音和 History 重试共享该阶段，原始 asr_text 保持 provider 原稿，片段展开和明确文字操作不套用模糊替换。

PromptOrCode 场景和 Code 输入中的「请 / 帮我 / please」请求作为口述内容保留，不解析成显式整理操作；普通聊天中的明确口述操作仍按原规则解析。「请 / 帮我 / please」开头时，只有操作词紧跟在前缀之后（可隔一个「把这封邮件写得」「make it」这类短宾语）才算口述操作，正文从操作词之后的第一个逗号或冒号开始；句中其他位置提到「简洁」「改写」「casual」等词时仍按普通内容整理。Codex 的自然语言请求识别允许请求正文包含 `--force` 等命令 token，同时独立的 shell 命令仍识别为代码。

历史列表只对相邻记录添加本地日期标题，不重新排序或改变查询和分页；加载更多后同日记录共用标题。复制、编辑与可用的重试保持直接可见，原文、版本、重新整理和删除由支持方向键、Escape 与焦点返回的「更多操作」菜单进入。进入编辑时输入框获焦；输入框、保存版本或取消按钮上的 Esc 均放弃编辑，保存或取消后焦点回到编辑入口。取消删除回到该行更多操作按钮。复制成功只在对应按钮显示两秒，失败保留实际错误，按钮宽度保持稳定。记录或音频导出期间，原按钮显示「导出中…」，暂时停用两个导出入口和清空操作；完成或失败后恢复，反馈紧邻页面标题与导出区。

文字操作预览先显示可编辑结果，来源、截图与指令默认折叠；展开共用正文滚动区。语音服务把输入中的密钥与已保存凭据分开，连接检查状态仅表示本次页面会话的检查结果；删除密钥失败在对应服务旁显示错误并保留输入草稿。严格离线模式会在当前生效摘要标出被阻止的路线。

## 显式文件评测的参数与来源

`voiceflow-eval --experiment ... --run` 使用同一 production ASR/cleanup、准备和最终保护 owner。当前固定候选可选择 auto/zh/en、SHA 绑定的词表/context 与逐 case 本地规则 fixture；实际 multipart/JSON/config builder 返回构造字段和 requested/constructed model facts，不能用候选显示名推测模型权重或服务端采纳参数。Assembly fused Dictation 与 raw Sync 在 producer facts 中分别标识，尚未作为本批 file evaluator 候选开放；Soniox/Message/local 候选也保留独立接线边界。

Opt-in 观察记录 provider parsed、post-filter、prepared、parsed cleanup reply、adapter accepted 与 app finalized 阶段。已确认空与本地抑制不同；speech 无可用输出是失败，未确认来源保持 unknown。cleanup reply 只在显式 recipe capture 时写外部研究文件；普通 History/metrics 不增加这些内容。保护拒绝使用 typed GuardReason，queue/preview/短长听写/选中操作/History retry 消费同一分类 owner；不从错误文案重新猜拒绝种类。空、截断、tool、未正常完成和保护拒绝不会绕过原有 late-cancel、配置代次或目标验证。具体 fixture 契约和未接线范围见 [cleanup-evaluation.md](cleanup-evaluation.md)。

SHA 候选重放需要 recipe 显式 `quality_replay` 和 TTL，并选择仓库外新 checkpoint/report 路径。输入与 actual constructed request 通过共享 builder 匹配后才可省略对应调用/key 读取；历史 ASR/cleanup 候选、fresh attempts 和当前 guard/finalizer 分开记录。fresh reliability / cloud anchor / performance 不能重放；V3 本机 ASR 的显式 replay 使用独立 schema2 checkpoint，每次仍需本次 owned load attestation；完整 runtime/Metal/image、实际 parameter dtype/layout、model pins/tokenizer 与有效 request 必须匹配。缺事实明确拒绝 replay，普通 fresh 与旧 sidecar 兼容。V1 local 和独立 V2 negative-only 不开放 checkpoint。具体失败、原子持久化和崩溃限制见[文件评测协议](cleanup-evaluation.md#explicit-sha-candidate-replay)。

## 显式本机文件评测

研究者可在仓库外声明 reviewed WAV、experiment、只读 `--models-root` 与本次新建的 `--models-work-root`，选择本机 Qwen3-ASR 0.6B / 1.7B。验证只校验固定源文件；运行将独立副本交给现有 OnDevice provider / runtime，不自动下载、不使用默认 App 模型目录、不回退云 ASR。本机 ASR 和 cleanup Off 不需要云 key。复制失败或完成后工作目录均保留；复制使用可协作取消并等待 join 的 worker，setup 事实先于 keys/provider 记入同一个 RunRecord journal。已处理取消不读 key、不调用 provider；强制终止/崩溃没有 durable terminal 或 resume checkpoint 承诺。模型文件校验、运行时握手、实际模型 ID/revision、加载、推理与音频覆盖分别记录，不互相替代。空或不完整的本机结果继续被生产 guard 拒绝；不把超时或错误当静音成功。详细契约与当前验收限制见 [文件评测](cleanup-evaluation.md)。这条路径不读取麦克风或私人 History，不证明外部粘贴质量。

## 开发者显式 fixture trace

macOS 可用 `cargo build --manifest-path src-tauri/Cargo.toml --locked --offline --features research-harness --bin voiceflow` 构建单独的研究入口。默认 feature 与正常启动不产生研究事件。由外部 launcher 保持新启动进程、内核出生身份、二进制 SHA 和专用 stdio 管道；若 singleton 将启动转发给其他实例或本进程退出，必须记 owner unknown，不附着既有进程。

入口为 `--background --research-stdio`，协议 `voiceflow_research_stdio_v1`。每行至多 8192 bytes，最多 256 个请求、输入期限 20 分钟，stdout 背压 2 秒后关闭；不要通过 shell 管道无人值守地开始录音。请求形状为 `{"request_id":"canonical-uuid","command":{"op":"prepare","lease":{"run_id":"canonical-uuid","case_id":"canonical-uuid","job_id":"canonical-uuid","ttl_ms":60000,"fixture_target_pid":123,"allowed_capture":{"fixture_microphone":true,"audio_identity":false}}}}`。request UUID 不复用；同一个 run UUID 不能在此进程内另建 lease。`start`、`stop`、`drain`、`cancel`、`end` 的 command 均带相同 `run_id`；`quit` 仅在 run 已结束且现有 dictation owner Idle 时有效。所有代际、序号和单调纳秒为十进制字符串。prepare 的 `expires_monotonic_ns` 来自创建 lease 的同一 owner；普通消费者不要将其与另一语言时钟相减。

`start` 回执只表示 claim 接受；用 `input_first_accepted` 和 `recording_ready` 判断真实输入已进入 worker / 录音状态。stop 沿用原生产 stop claim，携带分别记录的 recording / processing generation。drain 返回 `events`、`dropped`、`local_terminal`、`capture_closed`、`delivery_writers`、`clipboard_settled` 和 `final_clipboard_read_allowed`。外部必须核对不可变身份、连续序号和 owner 闭合事实，不能只检查一个 success 字段。`provider_termination` 始终明确 unknown。end 撤销本 run；若实际 owner 未终止会返回 `owner_not_terminal_drain_and_retry` 并保留 lease。输入关闭进入已有有界退出 / 取消流程，不发送额外粘贴。

这是显式 stdio 操作覆盖，不能声称系统热键接受或人类物理按键端到端覆盖。当前集成只启动普通听写；独立的选区 / 看屏幕 / Onboarding preview / History 重整不从此入口授权。精确 wire 请求值、真实 provider effective model 和远端停止计费没有可靠来源时仍须单独验证。私有外部 harness 的 `research_stdio` recipe 可消费实时 owner 回执；独立 AX / DOM 文字判断和最终剪贴板比较继续由它完成，持久化 JSON 文件单独不能开放最终读回。有关模型、capture identity、质量、热键及压力矩阵的完整验收仍需独立运行证据。

The optional `final_clipboard_boundary` in a live `drain` response is produced by the same research owner only after its existing final-read gate and before lease expiry. It retains run/case/job, accepted recording/processing/configuration generations, target PID and owner-hook coverage. When the native restore owner has an exact sampled commit association, it includes the original snapshot generation and ordered-fixture SHA, temporary-write generation and final restore generation. The SHA hashes bytes already captured by the product; it does not request clipboard bodies again or certify equality. Unknown commit generations omit this association. A later delivery writer or unresolved/changed clipboard result clears it.

The restore protocol returns its sampled commit generation as an immutable result to the snapshot owner. That owner associates it with the captured snapshot once; consumers do not inspect mutable backend state or adopt a later sampled generation. A successful restore call can still have an unknown commit association. Terminal/quiescent owner facts and that association do not prove target insertion or final clipboard bytes; the separately authorized harness must establish those observations independently.

The external oracle must retain the original child and private request/response pipes, validate contiguous events and current identities, and obtain fresh matching owner boundaries before and after its final declared-value observation. Persisted JSON, a timer, a PID/path or `local_terminal` alone cannot create this authority. Only the declared ordered item/type pairs may be fetched after terminal; native fetches and generation checks remain non-atomic and have no prefetch allocation cap. Independent exact byte comparison and a fresh post-terminal target sample determine their separate results; product terminal, restoration association and product readback never establish independent paste success. Normal clipboard preparation/preservation must be separately controlled; no automatic ambient backup or cleanup overwrite is introduced.

The same lease accepts optional `learning`, owned by `learning_research::LearningConsent`: selected_state_id, nullable baseline_run, edit_fixture_id, purpose (correction/baseline/unseen/negative/undo/delete/restart), mapping_id, bundle_id, observer_enabled, post_insert_field, settled_field and at most eight terms[{fixture_id,before,after}]. Test state must be explicitly created/selected; this entry creates no parallel profile, imports no private History and never clears a normal dictionary. Prepare returns selected_learning_state with a content SHA and initial selected-pair facts, not a claim that the whole DB is empty. Missing consent keeps the original disabled-learning fixture rules.

Consented native dictation uses the real paste observer/classifier/candidate SQL/settings save owners, but live native observations now remain pending and do not automatically promote names or terms; activation requires explicit confirmation. The controlled V3 fixture retains its two-hit name and three-hit term thresholds. Each declared family, mapping, browser-host and native-bundle constraint must be known and match the saved context. A mismatch or unknown required dimension disables that pair. One applicable-pair set drives replacement, hints, usage and final protection; exact replacement and hit detection skip quoted and code literals. Version-1 learning_record fields retain null/not_observed/not_measured for unknown stages. SQL promoted is distinct from completed settings persistence; hint/replacement usage is actual local prepare and can precede later cancellation. The 3-second initial/400ms sample/1.5-second idle/12-second total observer window is unchanged. Drain adds learning_tasks and pipeline_closed; session_terminal waits for all observer/persistence descendants. Cancellation, expiry and configuration changes cannot attribute late work to a subsequent run.

Private Driver learning_edit pressure requires the real eligible-registration receipt plus fresh independent AX window/control/text/UTF-16 caret proof. Host rechecks its own foreground/key-window/control/caret before its declared programmatic setter. This is synthetic actor evidence, not human action or ASR quality. Independent original-insertion scoring stays unchanged; learning-readback.json separately describes the controlled final field, and unseen quality still needs independent text/labels. Prepared-only learning_action(run_id,fixture_id,action=undo|delete) calls the existing undo/Forget owner and returns selected tombstone/dictionary-retention and partial failure facts, then closes the unstarted lease. It supplies no recording terminal or clipboard read authority. Driver consumes purpose=undo/delete and cleans up before clipboard/go/recording.

### Standalone deterministic-text learning fixture

The feature-gated `voiceflow-learning-harness` binary is a separate stdin/stdout process. It does not initialize Tauri, load normal `app_data_dir` settings, access Keychain, route to cloud providers or observe a native target. An explicitly declared WAV task may call the existing on-device ASR provider. The caller must pass `--arm-root` as a fresh, absolute, private (mode `0700`) directory beneath `std::env::temp_dir()`; the arm owner creates only its own `learning-arms-v2/<arm_state_id>` settings and SQLite state there. It creates fixture settings from `Settings::default()` and reopens only that arm's settings and selected pair rows. It never clears an existing arm or imports History.

Build and run it with the explicit research feature. The positive fixture supplies a version-2 `VoiceFlo` → `VoiceFlow` episode, three declared `apply_text` operations, and a bounded follow-up sequence. Each JSON line needs a unique request UUID. Keep the declared operation IDs stable when replaying their exact operation. Three distinct learning operation IDs reach this term's threshold; the next operations exercise the production local replacement and undo owners. `learning-v2-text-trajectory.json` separately declares six complete-field snapshots. The protocol accepts two through six ordered states; actual stdio checks consumed 2/4/6 states with 2/4/6 polls for the matching-length fixtures, and rejected seven states before creating arm state. It reports actual states consumed and observer polls, so a stable early exit does not mean later declared states were observed:

```sh
fixture_path=src-tauri/src/fixtures/learning-v2-text.json
arm_root="$(mktemp -d "${TMPDIR:-/tmp}/voiceflow-learning-arm.XXXXXX")"
chmod 700 "$arm_root"
{
  jq -c --arg request_id 60000000-0000-4000-8000-000000000001 '{request_id:$request_id,command:{op:"prepare",episode:.episode}}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000002 --arg operation_id 50000000-0000-4000-8000-000000000009 '(.apply_text_template | .operation.operation_id=$operation_id) as $template | {request_id:$request_id,command:{op:"apply_text",operation:$template.operation}}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000003 --arg operation_id 50000000-0000-4000-8000-000000000010 '(.apply_text_template | .operation.operation_id=$operation_id) as $template | {request_id:$request_id,command:{op:"apply_text",operation:$template.operation}}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000004 --arg operation_id 50000000-0000-4000-8000-000000000011 '(.apply_text_template | .operation.operation_id=$operation_id) as $template | {request_id:$request_id,command:{op:"apply_text",operation:$template.operation}}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000005 '(.followup_templates[0] | del(.expected_output_text)) as $command | {request_id:$request_id,command:$command}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000006 '(.followup_templates[0] | del(.expected_output_text)) as $command | {request_id:$request_id,command:$command}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000007 '{request_id:$request_id,command:.followup_templates[1]}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000008 '{request_id:$request_id,command:.followup_templates[1]}' "$fixture_path"
  jq -c --arg request_id 60000000-0000-4000-8000-000000000009 '(.followup_templates[2] | del(.expected_output_text)) as $command | {request_id:$request_id,command:$command}' "$fixture_path"
  jq -nc --arg request_id 60000000-0000-4000-8000-000000000010 --arg episode_id 50000000-0000-4000-8000-000000000001 '{request_id:$request_id,command:{op:"end",episode_id:$episode_id}}'
} | cargo run --manifest-path src-tauri/Cargo.toml --locked --offline --features research-harness --bin voiceflow-learning-harness -- --arm-root "$arm_root"
```

`prepare` binds the fixture identity, exact text SHA/byte lengths, source rights declaration, local-only grant, two-to-six ordered edit states and one mapping/scope. `edit_states[].layer=settled` is a predeclared authority label; it does not mean each snapshot was read or independently shown stable. `apply_text` passes the predeclared full-field trajectory through the same bounded observer, production local-edit guard, classifier and candidate/SQLite owner used by learning. Early observer termination is reported with its actual poll and consumed-state counts. The selected pair and operation observation are saved only in the named arm. The local-only owner uses an `auto_scoped` rule and does not add its target to global `Settings.dictionary`; no settings save is needed for the operation, and the receipt says `not_required_local_only_scoped_pair`. A legal preexisting dictionary word remains unchanged. Receipts report observer and candidate outcomes, current hit/promotion state and SQL row readback, plus `not_run` for insertion/AX/independent target oracle and `not_measured` for ASR quality. Full field text is kept out of the arm manifest and training receipts; the production candidate row still stores the selected word pair.

Two actual early-stop paths distinguish the consumed prefix from declared future states. `[A, B, B, C]` ended after two polls and three consumed states with `trajectory_settled=false`; the observed candidate did not match the authorized pair, so it wrote no row. `[A, B, B, D, B]` also ended after two polls and three consumed states; the observed prefix did match its authorized candidate, so the production owner persisted it. In both cases the remaining declared states were not read. The second case establishes only the observed prefix candidate, not the declared future state or any later task's benefit.

`apply_followup` accepts only a declared `task_id`, its bound `operation_id`, and text whose exact UTF-8 length and SHA match that task's deterministic source declaration. The first task runs the production local replacement owner against the selected applicable scoped pair and records `last_used_at` plus `learned_term_usage` in one transaction with its bounded operation receipt. The output text and hash are actual local-owner results; an expected-output mismatch remains a mismatch and never substitutes fixture text. Replaying the same operation returns current output but does not increment usage again while the receipt remains retained. `undo` requires that positive selected-pair usage and calls the production tombstone owner, recording the undo receipt and tombstone in the same transaction; duplicate undo does not refresh tombstone timestamps. For this `auto_scoped` path, undo retains the global dictionary and returns `not_required_auto_scoped_undo`; there is no dictionary/settings reconciliation step. The second declared task expects the tombstoned state and exercises that the old pair no longer applies. Only the fixture-authorized `final_output` layer is returned, from this local replacement owner; pipeline final, ASR quality, insertion, AX and independent target oracle remain `not_run` / `not_measured`. Actual output is bounded to 1 KiB before usage or receipt writes.

The fixture's `followup_templates[].expected_output_text` is test metadata only. Do not forward it in the stdin command: commands reject undeclared fields. The `jq` examples above remove that field before sending the command. `learning-v2-text-cobalt-invalid-local-only.json` preserves the earlier `cobolt` → `cobalt` case with an explicit invalid-for-local-only-parity label; the production local-edit lexical guard rejects it. B1b closes the observed sentence-final Latin period boundary in the bounded text path: `VoiceFlo.` is classified without treating the trailing period as part of the alias, while an internal period remains part of the token. The earlier fixture mismatch remains historical evidence; this deterministic coverage does not establish native paste-to-AX behavior. The original native owned-WAV learning suppression remains in force. A real public WAV and on-device Qwen model produced an ASR transcript in the W1 research probe, but its attestation is still pending the shared completion gate and the learning owner did not run; the present local-only parity runtime reused that ASR evidence without a fresh inference. Human/unknown actors, multiscope/casing rule identity, native insertion/AX, independent target readback and recognition/cleanup quality are separate evidence and are not established by this text slice. Reopening the same arm was verified through a new process; that does not prove native App restart behavior.

### Versioned multi-scope phase fixture (V3)

The same feature-gated research binary also accepts `prepare_plan` with a version-3 phase plan. An ordinary learning plan binds its complete phase order, mappings, source payload hashes and byte lengths, operation IDs, per-phase grants and TTLs to one identity hash. It creates or reopens one `learning-arms-v3/<arm_state_id>` beneath the caller's private `--arm-root`; all declared mappings remain in that arm's settings. `enter_phase` opens only the next declared phase grant, one phase lease may be active at a time, and `end_phase` requires an admitted operation receipt for every operation declared for that phase. `LearnText`, `ApplyFollowup` and `Undo` use the existing production learning, local-replacement and tombstone owners. The phase scope remains plan-declared metadata (`identity_kind=plan_declared_not_observed_target`), not proof of a focused native app. The separate target-only `ObserveTarget` plan does not create or reopen a learning arm or Settings.

The fixtures `learning-v3-multiscope.json`, `learning-v3-pending-apply.json` and `learning-v3-postundo-multiscope.json` exercise the real stdin dispatcher. They use synthetic programmatic text only. A/B phases keep the same family but use separate mapping IDs and bundle IDs in one arm. For replacement, V3 obtains the candidate set from production `LearnPairScope::applicable_promoted_records`; an A-owned row applied under B therefore yields the ordinary scope mismatch, no selected-pair application and no usage. It may add one `research_fixture_noop` operation receipt, but that receipt is not a learning hit/proof or usage. V3 now uses the same C1 `word:v2:<sha256>` (`c1_word_rule_v2`) identity as production History and paste learning (`known_gap: null`): a B-side attempt to learn the same `(before, after)` pair writes an independent `scope_b` row rather than colliding with `scope_a`.

V3 keeps local-only rules `auto_scoped`, preserves the existing global dictionary byte-for-byte (including a legal preseeded word), and does not save Settings for a local rule or its undo. Application and no-op operations have distinct retained observation kinds; Undo requires the exact positive receipt for its referenced application phase, so a no-op receipt cannot borrow usage from another operation. A later A- or B-scoped follow-up after Undo observes the tombstoned A row and does not apply it. A one-hit pending row likewise produces no-op receipts for both its owner scope and the other scope. Reopening the same arm from a new process preserves the stored owner scope and tombstone.

Wrong-plan, undeclared-operation, text-SHA, missing-learning-grant and plan-identity-change requests leave the selected SQL tables and settings unchanged, and a follow-up whose local-replacement output exceeds the research dispatcher's output bound is rejected before an application receipt or usage write. The ordinary `LearnText` / `ApplyFollowup` / `Undo` plans require `target_oracle=none`. A separate target-only `ObserveTarget` plan connects the research binary to the registered, source-pinned owned FixtureHost and AX reader: it samples the registered target value, matches its SHA-256 and byte length, confirms the same held Host identity before and after the AX read, and waits for helper/Host exit; wrong, duplicate or expired operations and missing manifests or grants are rejected, and a deadline returns unknown only after Host exit is confirmed. It does not cover VoiceFlow product insertion, product learning, pipeline final output or a same-Host insertion-to-correction chain. The synchronous stdin dispatcher has no in-flight CLI cancellation command. IME, browser fields, independent benefit judgment and ASR/cleanup quality remain outside this slice. Exact casing remains subject to the C1 rule-identity contract.

The research binary also has clipboard-guard research entrypoints: a named-only `--clipboard-guard-component` for fixed synthetic seeds on a unique named NSPasteboard, a `--mainqueue-probe` for the OS-main callback and owner join, a RemoteNamed recorder CLI that exercises protocol ordering with a recorder/fake callback, and a no-shortcut `--clipboard-guard-general-remote-probe` whose parent restores the General clipboard snapshot only after the child exits and ownership is re-checked. None of them sends `CGEvent` or `Cmd+V`, creates `ConfirmedInsertion`, reads back or exports original General clipboard contents, or runs Host, AX, TIS, WAV, model or SQL learning paths. `general_guard_active_for` remains false, so `RunOwnedPasteSession` still rejects before target or clipboard effects. These entrypoints are protocol tests, not native paste, product delivery, user correction or quality evidence.

The existing pending/promoted/usage/draft UI remains. EN10 per-observation eligibility/persistence UI has not yet consumed these authoritative receipts and cannot infer outcomes from teaching copy, counts or Toasts. Native GUI observer to unseen utterance, actual target editing, scope resolution from real foreground Apps, and full deletion acceptance still require separate runtime evidence.

四项按需教学的持久化入口为 `get_guide_acknowledgements` 与 `acknowledge_guide({guideId,version})`。读回是固定四项裸数组 `{guide_id,current_version,acknowledged_version}`；ID 为 `dictionary_learning`、`context_screen`、`selected_translation`、`scene_preferences`，各项当前独立版本为 1，旧配置 `acknowledged_version=null`。只在明确“知道了”后调用确认；关闭不写，重复确认幂等，未知 ID 或过期教学版本拒绝。确认经过现有 settings 保存 owner，不发送 `settings://changed`，不改处理配置代际或权限 / 功能开关。UI 据未确认版本在真实功能入口展开教学，功能实际成功仍需自己的独立状态。


## 就地教学与返回设置

四类说明直接复用生产入口，不增加必经向导步骤或捕获前弹窗：

| 入口 | 说明与实际操作 owner |
| --- | --- |
| 个人词典 | 手动术语、固定错词示例、同框纠错观察 / 分类 / 晋升、候选确认 / 忽略、已生效替换 / 忘记和实际使用计数。添加仍由现有词典命令完成；错词建议由 History 的显式编辑保存产生；候选提取比较编辑前已采用版本，并按同一事务捕获的 scene / App mapping 保持范围，只接受分类器认可的单词纠正，不读取 AX / raw ASR，也不追加到全局个人词典；同一 History 行的同一 pair 只计一次，粘贴观察与 History 编辑通过数据库中的同一听写事务来源去重。旧 receipt 没有可恢复来源时保持 unknown，不猜为独立证据。Latin 自动纠正还要求 CamelCase、全大写缩写或下划线等标识符形状；Latin 普通单词、书写数字 / 量词、否定反转、单复数和跨文字体系替换不会生成候选；用户可在现有 History 候选中明确确认。连续中文长串中没有明确分隔边界的同音人名编辑会放弃词条与风格学习，避免把邻近上下文字当作姓名；完整有界的 2–3 字姓名仍可生成按当前 mapping 隔离的待确认候选；生产 History 与 live-observer 弱证据即使达到原人名命中数也不会自动晋升，同一听写事务来源跨两路只计一次；用户在 History 或词典中明确确认后，规则才在对应范围内生效。若学习存储在版本已保存后暂时失败，保持编辑并再次保存会复用原版本来源与 History 回执，不增加版本或命中。现场同框观察默认关闭独立的「本机观察插入后的改动」许可（已有 AX 文字许可也仍可允许）；它只观察通过原 paste owner 确认的 UTF-16 插入范围，前后原文变化时不学习，不读取其他屏幕文字或发送改动。人名 2 次、术语 3 次的晋升阈值只保留在 research/test fixture owner，不改变生产候选行为；tombstone 保持原规则；family 仅用于区分宽泛类型，单独的 family 不构成已知作用域；family 缺失时仍保留既有全局作用域语义。作用域中的 mapping ID、browser host、native bundle 必须由当前记录提供并逐项匹配；缺少所需上下文或与其他 App / 网站不符时不应用。旧的 family-only 已晋升词对会降为候选，不再应用于同类所有 App。经 History / 本机局部观察候选明确确认的 scene pair 通过 `auto_scoped` 资格在对应范围内替换并生成 cleanup 提示，无需把目标词写入全局词典；此标记不跳过当前 scope 匹配，也不解除既有目标 / 任务保护：普通 Code、Secure、Terminal、Form、Search 等仍不会仅因 scope 匹配而自动替换或生成提示。只有有效且明确的 `PromptOrCode` 声明可将 Code 的整理输入按 `CodingPrompt` 处理后，才会按现有规则使用；本机观察许可本身不会解除这些保护。旧记录默认没有 `auto_scoped` 资格，忘记后 tombstone 会停止应用。替换、提示、使用计数和最终保护共用同一份适用词条；逐字替换与命中检查跳过引号和 code literal。 |
| 写作与整理的 App 来源规则及上下文隐私预设；更多快捷键的看屏幕入口 | 分别解释 AX 文字、OCR、provider 文字用途、云端视觉和独立风格样例批准。导航到系统权限本身不请求授权，用户还需主动打开对应原生提示；不会因为阅读说明截图。配置、权限读回和本次捕获不能互相替代。 |
| 可选选中文本试用；更多快捷键；真实来源捕获后的事务预览 | 标记固定原稿与可选任务；真实试用复用当前 lease / generation / 选区快照，预览使用独立的 480×300 原生窗口，窄权限只提供当前事务 payload、UI 语言与主题，并显示操作、来源、结果和可由实际事实支持的来源回执；不授予全局事件监听或 Settings / History / Keychain 读取。没有选区需回原输入框重选；来源或目标改变保留结果供复制；取消 / 失败后的迟到结果不成为新成功。教学不会按旧选区执行，不增加发送或 Enter。 |
| 写作与整理的语气卡与 Prompt；词典中的风格草稿入口 | 区分核心场景卡、全部 15 个细分场景、保存的 Prompt、未保存草稿、待确认样例及已批准的 App 样例。比较复用用户主动点击的 WritingPreview，可能请求已配置整理服务；阅读说明不请求。App / executable / host / path / focused field 条件仍由现有映射 owner 同时匹配。 |

相关页面、试用与预览中的就地说明默认折叠，由用户主动展开，不会取得焦点；快捷键页折叠组内也保持手动展开。收起不写完成状态，随时可重看。只有明确点击「知道了」才调用原生 `acknowledge_guide`，读取与保存均验证四类 ID、教学版本和回执；保存失败保持未确认。阅读状态由现有原生设置 owner 持久化，不使用 localStorage，不修改来源 grants、onboarded 或处理配置 generation，也不根据成功 toast 猜测已读。

教学中的固定原稿只是演示，不自动成为词条、样例、识别结果或质量证明。风格草稿确认框显示实际前后文本并提供默认不勾选的云端样例共享选项；默认确认只将样例保存到本机，除非用户在该确认框明确勾选授权或稍后在「写作与整理」的 App 规则中单独批准样例发送。样例内容变化后需再次批准。当下确认草稿不发请求。手动输入的样例仍需单独批准并保存；在 App 规则关掉样例批准并保存后停止后续使用，旧预览不表示当前授权。其他 AI 账户不需要导入。

就地说明用有标签的按钮和 region，隐藏内容退出键盘顺序；关闭只在当前窗口仍有焦点且焦点仍在说明内时返回入口。说明导航到设置页时，在窗口仍活动的情况下聚焦新标题；语气、映射、服务、词典和片段的未提交表单在同一窗口内跨页保留，不会因导航自动保存；关闭窗口后丢弃。语气切换编辑对象、清空规则或开始编辑另一条规则时，仍使用现有放弃修改确认。说明没有新增动画，窄窗口可换行并沿用页面 / 预览的滚动区域。ARIA 和源 fixture 不证明真实 VoiceOver、多屏、外部选区或粘贴验收通过，这些仍需真实 macOS 验证。

## Week-one dictionary learning preflight and isolated verification

Build the Installable Test app with the exact test identifier. The Tauri CLI merges this JSON override into the repository config; Tauri derives a separate AppData / Keychain namespace only when the identifier is exactly `com.voiceflow.installable-test`:

```sh
npm run tauri -- build --features research-harness --bundles app --config '{"identifier":"com.voiceflow.installable-test"}'
test_app="src-tauri/target/release/bundle/macos/VoiceFlow.app"
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$test_app/Contents/Info.plist"
```

The final command must print `com.voiceflow.installable-test`. Verify that identifier before opening the app. `instance.rs` also takes a process-wide lock from the current macOS user's temporary directory, shared by dev and packaged builds; while another VoiceFlow process is running, a second launch may exit successfully without opening its own window. Do not quit or kill a personal VoiceFlow process to make room. Launch the Test app only after confirming that no VoiceFlow process owns the lock, or from a separate macOS login user with a fresh Test AppData directory. Never clear an existing AppData or Keychain item. Native participant preflight remains pending until an isolated Test process is independently verified. After verifying the lock condition, launch the inspected bundle with `open "$test_app"`.

For an explicit Teach batch, freeze and record the existing Settings readback before the first task. Select an already-installed, verified on-device ASR model; set `cleanup_enabled=false`, `fuzzy_dictionary_enabled=false`, `dictionary_learn_enabled=false`, and `local_edit_learning_enabled=false`. Keep `context_enabled=true` and the native App mappings enabled so App A/B scope resolution remains available, but disable every `context_mappings[].dictionary_learn_enabled` and set every mapping's `source_permissions` (`ax_text`, `local_ocr`, `cloud_vision`, `context_text_to_providers`) to false. Set `browser_access_enabled=false` and `window_ocr_enabled=false`; do not invoke Look-at-screen or provider-context capture. This keeps passive dictionary learning and Screen/provider seeding out of the Teach arm while leaving explicit user approval of the selected App pair available. Freeze mapping scope/enabled state, style-example approval and content, writing modes, output mode, ASR model, cleanup settings, and fuzzy setting for the whole batch. Use a fresh Test AppData root with an empty global dictionary and the existing explicitly scoped App pair. If any global Screen, Manual, or legacy word is already authorized, preserve it; the no-term-injection Teach arm is `blocked` until it has a separately prepared clean Test root. Do not clear existing AppData or Keychain items. If no verified local ASR model is already installed, mark the arm `blocked` instead of downloading one or calling it a no-hint run.

Passive observation is a separate arm with its own fresh Test AppData root and its own consented mappings. The private V3 research arm is a deterministic programmatic-text fixture, not a passive-observation arm: it does not start the normal App observer, inspect a native target or provide human/IME evidence. A Test app process must not be running against the same root as another arm. With no participant available, human correction, independent author/host proof, IME commit, and native target outcome remain pending; synthetic fixture results do not fill those fields.

Do not restart the Test app after each task. Assign each task an ID and retain its dictation/delivery generation, audit `runtime_id`, pre/post `audit_revision`, `clear_generation`, `observer_epoch`, sequence bounds, dropped counters, matching `activity_id` and `pair_ref`, lifecycle, registration/outcome, candidate-store and Settings-store statuses, plus the expected task-specific state changes. Wait for that task's observer activity to leave `in_progress` as `closed` or `superseded`, check persistence statuses and the corresponding Settings/SQLite owner readback, and record the independent target result. `unknown`, a missing activity, dropped audit entries/updates, an unfinished lifecycle, or incomplete persistence blocks that task; do not infer closure from a stable audit revision. The normal audit has only the current process's latest 128 activities and is not a cross-store transaction. This gives per-task owner settlement evidence without forcing a process restart. At batch boundaries, close the Test app normally and take a final closed-owner fingerprint; never terminate a personal process to obtain one.

For a reproducible rule/state fingerprint, run this bounded read-only projection only against the exact Test AppData root. Both initial and confirmation reads of `settings.json` read at most `MAX_SETTINGS_BYTES + 1` bytes and block on overflow; each SQLite table is preflighted with `MAX_ROWS_PER_TABLE + 1` rows before its bounded payload is read. It reports per-component Settings hashes, per-mapping hashes, stable per-row hashes over rule/source/scope/tombstone state, and separate hashes for the declared mutable usage fields (`learn_pairs.last_used_at` and `replacement_runs`, plus `learned_term_usage.replacement_runs` and `last_replaced_at`). C1 updates the per-rule `learn_pairs.replacement_runs` counter when a selected rule is used. Other row fields, including candidate evidence, remain in the stable digest. Source text, credential fields, dictionary values, and raw row contents are never printed.

```sh
VOICEFLOW_TEST_DATA_ROOT="$HOME/Library/Application Support/com.voiceflow.installable-test" python3 - <<'PY'
import hashlib, json, os, sqlite3, tempfile
from pathlib import Path

MAX_SETTINGS_BYTES = 2 * 1024 * 1024
MAX_ROWS_PER_TABLE = 1024
MAX_SQL_BYTES = 2 * 1024 * 1024
MAX_OUTPUT_BYTES = 16 * 1024 * 1024
MUTABLE_USAGE_FIELDS = {
    "learn_pairs": ("last_used_at", "replacement_runs"),
    "learn_observations": (),
    "learned_term_usage": ("replacement_runs", "last_replaced_at"),
    "style_drafts": (),
}

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

def sha_bytes(value):
    return hashlib.sha256(value).hexdigest()

def sha_value(value):
    return sha_bytes(canonical(value).encode())

def read_bounded_bytes(path, limit):
    with path.open("rb") as source:
        data = source.read(limit + 1)
    if len(data) > limit:
        raise SystemExit("blocked: Settings exceeds the read bound")
    return data

def fingerprint_rows(table, rows, key_column):
    mutable_fields = MUTABLE_USAGE_FIELDS[table]
    stable_rows = []
    usage_rows = []
    for row in rows:
        identity = row.get(key_column)
        if not isinstance(identity, str):
            raise SystemExit("blocked: row identity is invalid for " + table)
        ref = sha_value(identity)[:24]
        stable = {key: value for key, value in row.items() if key not in mutable_fields}
        stable_rows.append({"ref": ref, "sha256": sha_value(stable)})
        if mutable_fields:
            usage = {field: row.get(field) for field in mutable_fields}
            usage_rows.append({"ref": ref, "sha256": sha_value(usage)})
    stable_rows.sort(key=lambda row: row["ref"])
    usage_rows.sort(key=lambda row: row["ref"])
    stable = {
        "rows": len(stable_rows),
        "sha256": sha_value(stable_rows),
        "row_fingerprints": stable_rows,
    }
    usage = None if not mutable_fields else {
        "fields": list(mutable_fields),
        "rows": len(usage_rows),
        "sha256": sha_value(usage_rows),
        "row_fingerprints": usage_rows,
    }
    return stable, usage

def read_rows_for_test(connection, table):
    cursor = connection.execute(f'SELECT * FROM "{table}" ORDER BY rowid')
    columns = [item[0] for item in cursor.description]
    return [dict(zip(columns, row)) for row in cursor.fetchall()]

def fingerprint_self_test():
    with tempfile.TemporaryDirectory(prefix="voiceflow-fingerprint-self-test-") as temp:
        db_path = Path(temp) / "synthetic.sqlite"
        connection = sqlite3.connect(db_path)
        connection.executescript("""
            CREATE TABLE learn_pairs (
                pair_key TEXT PRIMARY KEY, native_bundle TEXT, tombstoned_at TEXT,
                ignored INTEGER, last_used_at TEXT, replacement_runs INTEGER NOT NULL
            );
            CREATE TABLE learned_term_usage (
                word TEXT PRIMARY KEY, replacement_runs INTEGER, last_replaced_at TEXT
            );
            INSERT INTO learn_pairs VALUES ('synthetic-rule-ref', 'scope-a', NULL, 0, 'before', 0);
            INSERT INTO learned_term_usage VALUES ('synthetic-word-ref', 0, NULL);
        """)
        before_rule, before_rule_usage = fingerprint_rows(
            "learn_pairs", read_rows_for_test(connection, "learn_pairs"), "pair_key"
        )
        before_usage, before_usage_counters = fingerprint_rows(
            "learned_term_usage", read_rows_for_test(connection, "learned_term_usage"), "word"
        )
        connection.execute("UPDATE learn_pairs SET last_used_at='allowed-use', replacement_runs=1 WHERE pair_key='synthetic-rule-ref'")
        connection.execute("UPDATE learned_term_usage SET replacement_runs=1, last_replaced_at='allowed-use' WHERE word='synthetic-word-ref'")
        usage_only_rule, usage_only_rule_usage = fingerprint_rows(
            "learn_pairs", read_rows_for_test(connection, "learn_pairs"), "pair_key"
        )
        usage_only, usage_only_counters = fingerprint_rows(
            "learned_term_usage", read_rows_for_test(connection, "learned_term_usage"), "word"
        )
        if usage_only_rule["sha256"] != before_rule["sha256"] or usage_only["sha256"] != before_usage["sha256"]:
            raise SystemExit("fingerprint self-test failed: allowed usage changed invariant digest")
        if usage_only_rule_usage["sha256"] == before_rule_usage["sha256"] or usage_only_counters["sha256"] == before_usage_counters["sha256"]:
            raise SystemExit("fingerprint self-test failed: declared usage change was not isolated")

        connection.execute("UPDATE learn_pairs SET native_bundle='scope-b', tombstoned_at='unexpected-tombstone', ignored=1, last_used_at='allowed-use-2', replacement_runs=2 WHERE pair_key='synthetic-rule-ref'")
        connection.execute("UPDATE learned_term_usage SET replacement_runs=2, last_replaced_at='allowed-use-2' WHERE word='synthetic-word-ref'")
        combined_rule, combined_rule_usage = fingerprint_rows(
            "learn_pairs", read_rows_for_test(connection, "learn_pairs"), "pair_key"
        )
        combined_usage, combined_usage_counters = fingerprint_rows(
            "learned_term_usage", read_rows_for_test(connection, "learned_term_usage"), "word"
        )
        connection.close()
        if combined_rule["sha256"] == before_rule["sha256"] or combined_usage["sha256"] != before_usage["sha256"]:
            raise SystemExit("fingerprint self-test failed: combined unauthorized change was hidden")
        if combined_rule_usage["sha256"] == before_rule_usage["sha256"] or combined_usage_counters["sha256"] == before_usage_counters["sha256"]:
            raise SystemExit("fingerprint self-test failed: allowed usage counter was not reported")
        bounded_path = Path(temp) / "bounded-settings.json"
        bounded_path.write_bytes(b"12345")
        try:
            read_bounded_bytes(bounded_path, 4)
        except SystemExit:
            pass
        else:
            raise SystemExit("fingerprint self-test failed: MAX+1 read was admitted")
    print(json.dumps({"fingerprint_self_test": "passed"}))

if os.environ.get("VOICEFLOW_FINGERPRINT_SELF_TEST") == "1":
    fingerprint_self_test()
    raise SystemExit(0)

root = Path(os.environ["VOICEFLOW_TEST_DATA_ROOT"]).resolve(strict=True)
expected = (Path.home() / "Library/Application Support/com.voiceflow.installable-test").resolve(strict=True)
if root != expected or root.name != "com.voiceflow.installable-test":
    raise SystemExit("blocked: not the exact Installable Test AppData directory")

settings_path = root / "settings.json"
if settings_path.is_symlink() or not settings_path.is_file():
    raise SystemExit("blocked: Settings owner is unavailable or redirected")
settings_before = settings_path.stat()
settings_bytes = read_bounded_bytes(settings_path, MAX_SETTINGS_BYTES)
settings = json.loads(settings_bytes)
if not isinstance(settings.get("dictionary_sources"), dict):
    raise SystemExit("blocked: dictionary source ownership is unavailable")
mappings = settings.get("context_mappings", [])
if not isinstance(mappings, list):
    raise SystemExit("blocked: mapping rows are unavailable")

components = {
    "asr_cleanup": {key: settings.get(key) for key in (
        "asr_provider", "asr_model", "cleanup_enabled", "cleanup_provider",
        "cleanup_model", "cleanup_intensity", "fuzzy_dictionary_enabled", "output_mode",
    )},
    "dictionary": {key: settings.get(key) for key in (
        "dictionary", "dictionary_sources", "dictionary_learn_enabled",
        "local_edit_learning_enabled",
    )},
    "screen_context": {key: settings.get(key) for key in (
        "context_enabled", "browser_access_enabled", "window_ocr_enabled",
    )},
    "writing": {"writing_modes": settings.get("writing_modes")},
}
mapping_rows = []
for mapping in mappings:
    if not isinstance(mapping, dict) or not isinstance(mapping.get("id"), str):
        raise SystemExit("blocked: mapping identity is unavailable")
    mapping_rows.append({"ref": sha_value(mapping["id"])[:24], "sha256": sha_value(mapping)})
mapping_rows.sort(key=lambda row: row["ref"])

db_path = root / "history.sqlite"
if db_path.is_symlink() or not db_path.is_file():
    raise SystemExit("blocked: Test AppData learning database is unavailable or redirected")
db_stat_before = db_path.stat()
connection = sqlite3.connect(db_path.as_uri() + "?mode=ro", uri=True, timeout=1.0)
connection.execute("PRAGMA query_only=ON")
data_version_before = connection.execute("PRAGMA data_version").fetchone()[0]
connection.execute("BEGIN")
tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
table_keys = {
    "learn_pairs": "pair_key", "learn_observations": "observation_key",
    "learned_term_usage": "word", "style_drafts": "draft_key",
}
missing = sorted(set(table_keys) - tables)
if missing:
    raise SystemExit("blocked: learning tables unavailable: " + ",".join(missing))
stable_tables = {}
mutable_usage_tables = {}
total_sql_bytes = 0
for table, key_column in table_keys.items():
    info = connection.execute(f'PRAGMA table_info("{table}")').fetchall()
    columns = [row[1] for row in info]
    if key_column not in columns:
        raise SystemExit("blocked: row identity unavailable for " + table)
    quoted = ['"' + column.replace('"', '""') + '"' for column in columns]
    size_expr = " + ".join(f"COALESCE(length(CAST({column} AS BLOB)),0)" for column in quoted)
    sizes = connection.execute(
        f'SELECT rowid, {size_expr} FROM "{table}" ORDER BY rowid LIMIT ?',
        (MAX_ROWS_PER_TABLE + 1,),
    ).fetchall()
    if len(sizes) > MAX_ROWS_PER_TABLE:
        raise SystemExit("blocked: row count exceeds bound for " + table)
    total_sql_bytes += sum(size for _, size in sizes)
    if total_sql_bytes > MAX_SQL_BYTES:
        raise SystemExit("blocked: SQLite source projection exceeds byte bound")
    cursor = connection.execute(f'SELECT * FROM "{table}" ORDER BY rowid')
    row_columns = [column[0] for column in cursor.description]
    rows = [dict(zip(row_columns, values)) for values in cursor.fetchall()]
    stable, usage = fingerprint_rows(table, rows, key_column)
    stable_tables[table] = stable
    if usage is not None:
        mutable_usage_tables[table] = usage
connection.commit()
data_version_after = connection.execute("PRAGMA data_version").fetchone()[0]
connection.close()
settings_after = settings_path.stat()
settings_bytes_after = read_bounded_bytes(settings_path, MAX_SETTINGS_BYTES)
db_stat_after = db_path.stat()
if settings_bytes != settings_bytes_after or (settings_before.st_ino, settings_before.st_size, settings_before.st_mtime_ns) != (settings_after.st_ino, settings_after.st_size, settings_after.st_mtime_ns):
    raise SystemExit("blocked: Settings changed during fingerprint read")
if data_version_before != data_version_after or (db_stat_before.st_ino, db_stat_before.st_size, db_stat_before.st_mtime_ns) != (db_stat_after.st_ino, db_stat_after.st_size, db_stat_after.st_mtime_ns):
    raise SystemExit("blocked: SQLite changed during fingerprint read")
result = {
    "consistency": "per_file_stable_read_not_cross_store_atomic",
    "settings_components": {name: sha_value(value) for name, value in components.items()},
    "mapping_rows": mapping_rows,
    "sqlite_stable_tables": stable_tables,
    "sqlite_declared_mutable_usage": mutable_usage_tables,
    "sqlite_source_bytes": total_sql_bytes,
}
encoded = canonical(result).encode()
if len(encoded) > MAX_OUTPUT_BYTES:
    raise SystemExit("blocked: fingerprint output exceeds byte bound")
print(json.dumps({"sha256": sha_bytes(encoded), **result}, sort_keys=True))
PY
```

Run the same code block with `VOICEFLOW_FINGERPRINT_SELF_TEST=1` to execute its adversarial check before any Test-root validation or read. It creates a temporary SQLite database, confirms changes to per-rule `last_used_at`/`replacement_runs` and term-level usage counters leave the stable digest unchanged while changing their usage digest, then combines those usage changes with an unauthorized scope/tombstone change and confirms the stable digest changes; it also checks the `MAX+1` settings read rejection. It prints only a fixed pass status and removes its temporary directory on exit.

At each task, pair the fingerprint with the exact task's effective hint evidence: capture the actual serialized ASR `prompt` and/or cleanup `DICTIONARY_AND_STYLE_REFERENCES` field when the route sends one; record `not_requested` only for an established local/off route. The production audit does not expose request fields, so unavailable live request bytes are `unknown`, not inferred from the row projection. Keep fingerprints, task results, and audit receipts outside the repository. If a full owner-settled fingerprint is unavailable at a batch boundary, mark `blocked`; an App B no-replacement result alone is not isolation evidence.

Run the isolated machine checks without a provider key or remote endpoint:

```sh
cargo test --manifest-path src-tauri/Cargo.toml manual_app_pair_replaces_without_a_global_dictionary_word_and_stays_scoped -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml learning_a0_baseline_runs_production_probes_and_emits_observed_facts -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml asr_wire_and_fixed_transcript_replace_only_in_matching_bundle -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml cleanup_wire_blocks_local_pair_edits_and_keeps_unmatched_source -- --nocapture
```

The A0 command prints observed production-owner facts and fails when any frozen `target_met` is false. The ASR and cleanup wire tests hold family/mapping/input fixed and vary App A vs App B bundle. Both ASR calls return the same fixed synthetic transcript; exact multipart `prompt` inspection proves independently Manual-owned global words are sent while scoped-only canonical terms, aliases, `legacy_unknown`, orphaned Rule owners with no current SQLite row, unowned dictionary values, transcript markers and audio markers are absent. Within App A, a test-only exact-replacement switch compares the same ASR text with only final replacement execution enabled or disabled. The cleanup test captures the full cloud and loopback Ollama requests: App A scoped manual, auto and legacy pairs change its prepared text; a recognized unscoped legacy pair with only a `LegacyUnknown` canonical owner also stays out of cloud hints and blocks its changed transcript. The shared route guard records no-call for both cases and the cloud collector receives no App A request; the local Ollama wire carries the complete local references. App B leaves those scoped aliases untouched, sends the natural original text to the selected cloud route, and carries only the independent global pair hint. Assertions inspect `TRANSCRIPT` and `DICTIONARY_AND_STYLE_REFERENCES` and prove the scoped pair associations are absent from the cloud wire. The audio marker is a byte string, not evidence of speech. These checks preserve independently owned global Screen / Manual egress; they do not count as human trials, IME tests or quality evidence. The current owner preflight has no independent IME-commit or human/host author proof; stable AX readback therefore remains `unknown` for those acceptance dimensions.

Run the existing V3 multi-scope fixture through its real stdin CLI in a new private temporary arm. Its programmatic text and declared scope are synthetic and do not create a passive observer arm or native App evidence. Keep output under `/tmp`; inspect the final completion response, because intermediate fixture requests intentionally include negative cases:

```sh
fixture_path=src-tauri/src/fixtures/learning-v3-multiscope.json
arm_root="$(mktemp -d "${TMPDIR:-/tmp}/voiceflow-learning-v3-arm.XXXXXX")"
chmod 700 "$arm_root"
result_path="$(mktemp "${TMPDIR:-/tmp}/voiceflow-learning-v3-result.XXXXXX")"
jq -c '.stdin_requests[]' "$fixture_path" \
  | cargo run --manifest-path src-tauri/Cargo.toml --locked --offline --features research-harness --bin voiceflow-learning-harness -- --arm-root "$arm_root" \
  > "$result_path"
jq -e 'select(.result.complete == true)' "$result_path" >/dev/null
```

The final `jq` is the machine gate. A shell exit alone is insufficient if the final `result.complete` response is absent. This invocation never touches the Installable Test AppData or Keychain; discard only the newly created private temp arm/result after retaining the needed digest and receipt outside the repo.

## Deterministic learning baseline boundary

The A0 replay fixture at [`learning-a0-baseline.json`](../src-tauri/evals/learning-a0-baseline.json) contains bounded inputs, frozen targets, and case classifications only. `dictionary_learn::tests::learning_a0_baseline_runs_production_probes_and_emits_observed_facts` executes the existing observer, candidate store, receipt, promotion, tombstone-owner, and replacement functions against isolated temporary state. It prints one `VOICEFLOW_LEARNING_A0_BASELINE_JSON` line with observed facts and target comparisons; save that run output outside the repository. The fixture itself is not a report of current behavior.

These probes are deterministic production-function replays, not native end-to-end acceptance, microphone capture, ASR quality, or independent human evidence. A0 holds the current ordinary-term threshold at three and the Chinese-name threshold at two; it does not invoke or change the ASR model or cleanup prompt. The native microphone smoke remains not run. Owned-WAV evaluation continues to suppress learning. A target miss records a known gap for later stages; it does not make the incorrect observed behavior a permanent expected result.

The observer now requires a successful read of the same changed inserted segment at or after the full idle grace before finalizing a dirty edit. Missing values do not count as unchanged; if a dirty observation reaches the 12-second total limit without that read, it abstains as ambiguous and creates no word or style candidate. No-change and unreadable observations retain the existing initial-window boundary. The 3-second initial window, 400ms polling interval, and 1.5-second idle grace remain in place. This timed text evidence does not establish an IME commit, and native IME behavior remains unverified; no AX notification or guessed composition API is used.

## Normal dictionary learning audit boundary

The backend adds `get_dictionary_learning_audit` using facts from the existing dictionary-learning registration, same-field observer, classifier, candidate store and review/save owners. It creates no second learning loop. One metadata token moves into the observer's actual selected persistence descendant and closes on its return; stale epochs, unknown aborts, evictions and clear generations remain distinct. Rejected unpersisted edits retain no text or invented pair link. Source UI wiring is present; runtime acceptance remains separately gated.

An actual SQLite read or write error in the weak live-candidate path records `candidate_decision: unknown` with `candidate_store: failed/store_error`, then keeps propagating the error to its existing caller. A policy skip leaves candidate persistence `not_attempted`.

The paste-to-observer handoff carries an internal, non-serialized `ConfirmedInsertionV1` minted only when the original paste owner confirms the exact expected UTF-16 splice in the same focused field with `PasteVerificationFact::Verified`; `VerifiedFullRange` does not authorize learning. The observer reads through that original anchor and uses the confirmed UTF-16 range with unchanged prefix/suffix anchors to localize the observed segment; it does not recapture the current field or locate the inserted text by searching. B1b now extracts up to three non-overlapping local word corrections from a bounded observed edit. It checks every hunk before deduplication; the lexical path rejects the entire edit when any hunk is unsafe, mixed with content changes, ambiguous, moved/reordered, insertion/deletion-only, or beyond the limit. A sentence-final period is excluded from the alias surface while periods inside identifiers remain part of the token; unsupported Unicode boundaries abstain. Explicit style handling uses a narrow whole-edit allowlist, so content-changing rewrites do not fall through to style. Structural suggestions do not imply automatic eligibility: every pair still passes the existing automatic gate, and `Reakt` → `React` has no evidence for automatic learning. History and local-only learning keep their existing conservative single-pair gate. The same owner holds separate grants: one single-use 3-second Undo and one absolute 12-second read lease. It checks observer epoch, cancellation and deadline on the owner side before and after each synchronous AX call; a call already in progress cannot be interrupted. The research learning task closes after all in-flight learning AX calls have actually returned and the read lease closes; the independent Undo owner may continue until its own grant is consumed, released or expires. This does not validate a real AppState multi-pair paste-to-native-AX chain, a same-Host delivery→correction→final-read sequence, native IME behavior, human correction, or learning benefit.

SQLite promotion, settings-file call outcome, in-memory dictionary membership and the lexicon eligibility predicate are separate facts. A completed call does not establish all-row mutation or all-leg success; failure does not imply no side effects. Style/intensity records are distinguished from word classification and the existing two-name/three-term thresholds. Each learned rule keeps its saved scope; replacement, prompt hints, usage and final protection use the same applicable set, with missing required context remaining unknown. The projection calls per-rule `last_used_at` bookkeeping `last_output_surface_match_at`; `learn_pairs.replacement_runs` is the separate per-rule replacement count. `learned_term_usage.replacement_runs` remains the term-level local-prepare counter. Both usage counters can precede cancellation or unconfirmed delivery; multiple same-after rules must not multiply or reattribute the term-level count.

C0 keeps public `Settings.dictionary` as a string list and persists separate per-word source ownership for manual entries, legacy words with unknown origin, and learning rules. A whole-dictionary settings patch remains a replacement: unchanged words keep their sources, new words become manual, and explicit removals clear the word's global effect and cancel pending additions. Add/import sends the unique valid words accepted by that action, including an existing learned spelling when the user explicitly adds it; it does not resend the merged dictionary. A full list still accepts an existing valid entry for manual ownership, while new words beyond the cap remain rejected. Forgetting a learning rule removes only that rule's source; manual or legacy ownership keeps the word. Paste observation and History edits use the database-owned dictation transaction as their shared origin; the same rule and origin count once. Retained legacy hit counts stay visible but do not combine with new known origins to satisfy automatic thresholds; only distinct eligible correction proofs count, while operation receipts and unknown-origin rows do not. Explicit confirmation is separate from evidence counting. Pending independent proof count comes from retained receipt rows, so History deletion may reduce a pending count; a promoted rule is not demoted. SQLite records the finite dictionary effect/version with the learning action before syncing Settings. If that second step fails, the command reports a partial result and UI says the source/rule was saved but the dictionary projection still needs retry. The corrected A0 probe exercises Manual source ownership, rule-source removal, and shared paste/History dictation origin through isolated production owners. It does not exercise cross-store partial sync or establish C0 acceptance; its deterministic evidence is not quality evidence.

The content-free `dictionary_learning_audit://changed {runtime_id,audit_revision}` event only invalidates a snapshot. Audit revision stability does not certify a coherent DB/settings interval or authorize historic actions. A historic entry may only focus separately resolved existing current controls; changed/stale/error states do not enable audit-derived actions. Existing confirm/undo owners retain their validation. The API adds no mutation action. Activity is limited to 128 current-process entries; complete pair reads and actual encoding have separate 1024-row/2-MiB-source/16-MiB-JSON admission budgets and return typed unavailable on excess.

The separate Settings form supports explicit global, native-app and website-host before / after rules. A native App is selected from the installed and running native-app catalog (`get_manual_learn_applications`); a website host is entered directly. The form does not require or create a style/context mapping, append a local rule target to the global personal dictionary, or edit a History row. Global rules use `GlobalRule`; native-app and website rules are stored in `learn_pairs` with their declared scope. Scope resolution is fail closed; replacement, local prompt hints, local usage and final pair protection reuse the single applicable promoted-pair set. Scoped rules are excluded from cloud pair hints and global ASR terms; if a matching local pair changes text for a remote Cleanup request, that request is skipped with `local_pair_egress_blocked`. Default confirmation of a pending row with a declared scope, including a legacy row whose scope flags are false, stays local; the user can separately choose the explicit global action. C1 stores each mapping-scoped word rule under an opaque v2 identity containing scope, surfaces and match policy, so explicit A/B rules can coexist. If live rules in one applicable scope overlap on an alias but have different targets, the shared applicable set abstains. Existing automatic categories and thresholds remain unchanged; another automatic scope stays pending when the same legacy-normalized pair already has a live owner. Mapping identity changes and deletion pause bound word rules until an explicit Settings rebind. Existing v19 learning rows are not rekeyed and their evidence is not transferred. Forgetting tombstones only this scoped row and does not remove another rule owner or a global dictionary word.


### Onboarding check ownership and current readiness

The microphone self-check owns no recording buffer or provider request. Stop is acknowledged by the audio thread: its matching self-check stream is dropped before a terminal `stream_released` event or stop receipt is published. An IPC completion without that fact does not close the UI lease. Enqueue/ack failures keep the same session and listener owned for retry, including across guide reentry; a known terminal-after-drop event can close ownership even if the later stop reply is lost. Prewarm/recording streams have separate owners, so this is not a claim that every audio device stream is closed.

An in-app selected-action result applies only to the same trial/recording, full source text and current UTF-16 selection range captured at listening start. Selection-only drift preserves the result for recovery without modifying the new range; duplicate lifecycle/results cannot rebase the captured source. Once applicable, one synchronous DOM range replacement commits text and its end-of-insertion selection together; controlled React state consumes that value. No deferred selected-trial caret callback can later overwrite a user selection or newer recording. This in-app guard is distinct from production preview confirmation, which rechecks the native source/selection/target and retains its own copy fallback and clipboard ownership.

Permission readback has one presentation owner shared by polling, refresh and explicit permission requests. Revoke/change, unavailable/unknown/conflicting readback or guide window exit/reentry invalidates permission-dependent Finish facts and the existing trial lease key. Original receipt provenance remains; current display becomes unknown. Regrant requires a fresh trial receipt and cannot reactivate old passes. Native configuration generations, saved credentials/model facts and hotkey registration remain their own facts. Polling is bounded and non-interactive; these are scoped observations, not a continuous OS permission monitor. Normal completion/learning/delivery/clipboard owners remain unchanged.

The controlled external-insertion entry (ON09) has an opt-in product trial for a new plain-text TextEdit document or the specifically prepared VoiceFlow local Chrome test page. It still requires an independent oracle integration and real acceptance evidence; product readback does not establish external paste/clipboard restoration. This section does not reduce the16 onboarding +16 enhancement scope.


### Opt-in controlled external onboarding test (ON09)

Finish offers an explicit fixed-target test in a new plain-text TextEdit document or a specifically prepared VoiceFlow local Chrome test page. A new matching-ID onboarding lease prepares one nonce fixture for three minutes. For Chrome, the fixture must be in the controlled local page's named plain-text field; personal pages and unrelated tabs are outside the trial. The user copies it only by clicking the copy action, pastes it into the selected test target, selects the fixed placeholder and requests a five-second target-switch interval. No recorder, ASR/LLM provider, History write, learning example, personal document discovery or automatic target launch is part of this route. A new attempt requires a new lease.

TrialOwner owns the prepared continuation; DictationManager and TextActionControl retain the existing claim, cancellation and commit boundary. The delivery callback admits the selected TextEdit or Chrome bundle only after current configuration/permission/lease/deadline checks, complete native target identity and exact fixed full text/UTF-16 selection checks. Target or selection drift rejects before insertion. The same paste owner checks the field after insertion; exact expected full value establishes only the product’s displayed readback fact. Clipboard restoration is a separate confirmed/not-needed/conflict/unknown fact derived from the actual clipboard owner. An explicit fixture-copy action changes the clipboard and is not undone to an earlier personal value.

Expected blur/reentry is permitted only while the backend-prepared external continuation is held. Ordinary onboarding retains permission-window invalidation. Actual changed/unknown readback clears the continuation and the existing lease key; regrant cannot revive old facts. Returning to the guide requires a fresh permission readback and a native current-owner confirmation before either fact appears in Finish. Closing/cancelling requests cancellation and waits for the existing matching-ID lease release; IPC success alone never means the delivery resource was released. Native main-window close continues to revoke. Unconfirmed insertion instructs inspection, never automatic retry. Independent oracle acceptance remains a separate workflow; product self-readback is not that evidence.

### Typed writing-preview outcomes (ON10)

The shared LlmError owner serializes a bounded kind and its existing GuardReason; transport detail is not rendered. PreviewResult retains that typed fact, an available accepted model reply separately from local/final fallback text, and the final preservation-guard outcome. Empty, truncated, incomplete, tool, capacity, timeout, network, credential, authorization and unknown failures remain distinct. Cancellation does not become fallback success. Off and other local-only routes invoke zero provider closures and do not resolve credentials for a model request. Preview still shares preparation, queue/retry, parser and finalizer owners and never enables History/learning/context by adding a test.


## 设置交互与状态反馈

日常设置标明自动保存；语气与 App 规则需明确保存，服务路线通过「保存并使用」应用，阶段测试不保存或切换当前路线。首次设置的完成页先汇总麦克风自检、听写试用及外部写入三个独立结果，完整配置和检查项按需展开。检查通过不互相替代。

HUD 配置默认使用 `classic` 外观，提供左右分栏的 Playground 调试台：左侧预览舞台居中展示生产 `VoicePill`（不绘制模拟应用窗口，也不读取活动桌面或窗口像素；仅在主胶囊内叠加单一 Glasswork Liquid Glass 表面，详情与词典提示保持不透明），右上角切换「浅色 / 深色」纯色背景（以与 Glasswork 一致的 620ms ease 过渡），底部切换「模拟声浪」以及「录音中」（默认）/「处理中」/「隐藏」/「自动循环」状态；右侧控制面板提供 `Classic`（亚光磨砂）/ `Liquid Glass`（遵循 Glasswork `glass.css` 四声明、`lens.js` 边缘折射、`adapt.js` 4.5:1 自适应可读性采样与 `refract.js` WebGL2 折射）外观标签、独立的「语音声浪」与「处理扫光」配色色块与风格标签、声浪灵敏度控制，以及「保存并应用」主按钮和「恢复默认光效」按钮。每次点选控件会立即更新本地预览并沿用版本化设置队列即时保存；点击「保存并应用」也会强制同步并确认设置，且实际听写 HUD 在每次会话启动时向后端复读 `get_settings` 与 `get_hud_appearance_status`，快速反复切换只应用最新请求；点击右侧声浪或扫光选项时会自动切换左侧预览到对应状态。自动循环依次展示录音约 3 秒、处理约 2 秒、隐藏约 1 秒并重复；循环仅使用一个页面内计时器，页面不可见时暂停当前状态的计时，不写入设置；演示设备与计时为样例，不操作原生窗口，也不访问麦克风或剪贴板。浏览器预览在关闭折射或处于隐藏状态时通过 CSS 隐藏材质输出，手动隐藏、Classic 或离开页面时释放渲染资源；原生 NSGlassEffectView 仍只由 AppKit 显示。设置页不显示常规保存、应用或有效外观状态；仅在后端确认所选外观发生回退或未能应用时，将原因内联显示在外观偏好说明处，状态读取失败时仅在相关选项下显示简短提示。页面打开时读取一次原生状态并订阅后续事件；每次 HUD 外观保存结束后静默重读状态，以覆盖未发事件的原生调度失败分支。原生更新排入 AppKit 主线程后异步完成，不会启动录音或 provider 请求。只有 macOS 26 或更新版本、系统未启用减少透明度且原生安装成功时，Liquid Glass 才是有效外观；其他情况保留用户请求并使设置页在确认状态后显示 classic 回退原因。原生材质只在主胶囊或恢复操作可见时显示，空闲暖面板、学习提示单独显示、生成代次已分发粘贴或会话收起时保持隐藏。WebView 主胶囊保留 Glasswork `0.12`（浅色）/ `0.24` `#1e1e22`（深色）低不透明度 tint 与方向性内斜边高光，隐藏原生材质或降低透明度时回退 `96%` 实色表面；单一外轮廓仍来自 NSGlassEffectView，现有 WebView 仍拥有胶囊、恢复和关闭操作的点击。

权限检查失败显示无法检测和重试入口，不停留在检测中；辅助功能的说明包含自动粘贴及另行授权的读取或文字操作。历史文字保留值 0 表示保留至手动删除；音频保留值 0 表示立即清理。清空历史期间确认框保留并禁止重复提交，失败可就地重试。

多个模态窗口同时出现时，只有最上层接收键盘和 Escape；关闭后恢复下层或原页面。删除行后，焦点移到邻近存活行或添加入口。隐私开关与选择器通过可访问描述关联说明，HUD 的操作和恢复失败加入状态播报。
本机模型下载错误保留原始错误供详情查看；操作系统报告 ENOSPC / “No space left on device” 时，状态另带 `disk_full` 类别。设置与首次配置显示检查可用空间的本地化提示，并保留手动重试下载入口；VoiceFlow 不会自动重试。

应用内听写试用失败以当前租约回执的 `code` 选择本地化主提示；未知代码使用通用本地化提示。后端原始 `message` 只显示在可展开的详情内，不作为主提示或回退文案。租约、配置代次和录音代次校验仍先于错误呈现。
