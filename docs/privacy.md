# VoiceFlow 数据与隐私说明

## 数据流

VoiceFlow 的录音只在用户主动触发 dictation 后开始。音频会发送到用户配置的 ASR 服务（默认为 Groq Whisper；也可改为 OpenAI、Deepgram、SiliconFlow SenseVoice、本机 Whisper，或兼容 OpenAI `/audio/transcriptions` 的端点，例如本机 FunASR / 阿里云百炼 Qwen3-ASR）。启用 AI 文字整理时，转录文本会发送到用户配置的整理服务（默认为 Groq；也可改为 OpenAI、SiliconFlow、DeepSeek、Anthropic、Ollama 或自定义端点）。转写和润色可以不是同一家。VoiceFlow 不发送 raw URL、窗口标题、PID 或目标 identity 给 LLM。用户主动启用 selected-text action 时，当前选中文本和语音操作会作为该次整理请求的输入；它不会写入普通 History，也不会被 VoiceFlow 自动保存为个性化数据。

上下文检测在本机完成。浏览器 host 检测只有在用户开启浏览器访问后才会执行；本地只保留必要的 host/profile 信息和不可逆的目标 fingerprint，用于防止录音期间误粘贴到变化后的窗口或 Tab。

录音过程中 HUD 会在本机显示最多约 280 字的实时 partial transcript。这些预览只存在于灵动岛窗口，不会写入 History、剪贴板，也不会发送给 LLM。

粘贴成功后，若开启词典学习，VoiceFlow 会轮询同一个已锁定输入框的当前值（约 3 秒起，空闲可延长到最多 12 秒），用来发现用户当场改写的短词或短短语。该轮询只在本机进行，不会发送给 LLM。密码框 / Secure Input、目标已变、该 App mapping 关闭学习、或 1Password / HR / SSO 等预设关学习的目标不观察。

## 本地数据

以下数据保存在 Tauri app_data_dir：

- history SQLite：原始转录、整理结果、状态、delivery/fallback 信息和 context policy；
- recovery spool：失败或中断录音的 WAV/F32 分块；
- gold wav：仅在设置中主动打开「成功听写也保留音频」后，成功听写才会写入本机 `gold/`（默认关；配额 4 GB；满则停止新写，不挡听写）；
- usage：当天的请求数量和录音时长；
- settings：不包含明文 API Key；每个服务商的密钥单独存在 macOS Keychain。

成功听写的 gold 音频默认不保存。打开后仍跳过密码框 / Secure Input，以及 1Password、HR、SSO 等预设关学习的目标。音频只留在本机，不会上传，也不会写进普通 History JSON 导出。删除单条 History 或清空全部数据时，对应 gold wav 一并删除。导出训练音频时附带识别草稿，不使用 AI 整理后的 `final_text`，也不会自动写入词典学习。

当前默认保留策略：

- recovery audio：7 天；
- gold audio（仅 opt-in）：沿用同一保留天数；训练建议至少 90 天或 1 年；
- history text：365 天；
- usage：按天存储，清空全部数据会一并删除。

设置中的保留策略会在启动时执行，也会在修改历史保留时间后立即执行。历史文字可以选择“永久”，这表示不会自动清理，直到你手动删除单条记录或清空全部数据。

## 删除和导出

History 页面提供：

- 导出全部 history JSON（不含音频）；
- 导出保留的训练音频：wav + Qwen JSONL（识别草稿，不含整理结果）；
- 删除单条记录（含对应 gold wav）；
- 清空全部 history、recovery audio、gold wav 和本地 usage。

清空操作不可撤销。删除 API Key 是独立操作，会从 Keychain 删除凭据并将 onboarding 标记为未完成。

## 第三方服务和 telemetry

VoiceFlow 当前不收集 telemetry、广告标识或用户行为分析。网络请求只发往用户选择并配置的 provider。用户需要分别遵守所配置 ASR 与整理服务商的服务条款和隐私政策。

## 安全边界

VoiceFlow 的本地 SQLite 和 recovery audio 默认依赖 macOS 用户账户和应用数据目录的文件权限；应用数据目录、SQLite 文件和 recovery spool 文件会尽量使用 `0700`/`0600` 权限。默认不启用应用层加密，因而共享 macOS 用户账户或未加密备份仍可能暴露本地转录。

应用层 recovery spool 加密是显式 opt-in 的发布能力：构建时启用 `encrypted-spool` feature，并设置 `VOICEFLOW_ENCRYPT_SPOOL=1` 后，新写入的 recovery 音频和 gold wav 会使用 XChaCha20-Poly1305 加密，32-byte 密钥单独保存在 Keychain 的 `history-key` 项中。若 Keychain 不可用，VoiceFlow 会 fail-closed，不会把密文当作 WAV 发送，也不会退回写明文；旧的明文 recovery 文件仍可读取。History SQLite 当前仍未做应用层加密。
