export type ProviderId =
  | "groq"
  | "openai"
  | "deepgram"
  | "siliconflow"
  | "fireworks"
  | "mistral"
  | "soniox"
  | "assemblyai"
  | "dashscope"
  | "deepseek"
  | "anthropic"
  | "ollama"
  | "local_whisper"
  | "on_device"
  | "custom";

export type ProviderCapability = "asr" | "llm";
export type ProviderProtocol = "openai_compat" | "deepgram" | "anthropic" | "on_device_local" | "soniox_realtime" | "assemblyai_dictation_sync" | "dashscope_message";
export type AsrTransport = "http_batch" | "websocket_stream" | "websocket_completed_audio" | "on_device";
export type ModelField = "select" | "text";
export type AsrLanguageSupport =
  | "auto_detect_only"
  | "auto_detect_or_fixed_language"
  | "optional_fixed_language"
  | "explicit_language_required"
  | "explicit_language_list"
  | "candidate_language_hints"
  | "unsupported";

export type AsrModelProfile = {
  languageSupport: AsrLanguageSupport;
  requestNote?: string;
  responseNote?: string;
  limitNote?: string;
  availabilityNote?: string;
  contextNote?: string;
  capabilityNote?: string;
};

export type ModelOption = {
  value: string;
  label: string;
  note?: string;
  retiredForNewSelection?: boolean;
  asrProfile?: AsrModelProfile;
};

export type ProviderDefinition = {
  id: ProviderId;
  label: string;
  capabilities: readonly ProviderCapability[];
  protocol: ProviderProtocol;
  defaultBaseUrl: string;
  allowsEmptyKey: boolean;
  hasHttpAsr?: boolean;
  asrTransport?: AsrTransport;
  editableBaseUrl: boolean;
  asrModels: readonly ModelOption[];
  llmModels: readonly ModelOption[];
  asrModelField: ModelField;
  llmModelField: ModelField;
  defaultAsrModel: string;
  defaultLlmModel: string;
};

export const PROVIDER_IDS = [
  "groq",
  "openai",
  "deepgram",
  "siliconflow",
  "fireworks",
  "mistral",
  "soniox",
  "assemblyai",
  "dashscope",
  "deepseek",
  "anthropic",
  "ollama",
  "local_whisper",
  "on_device",
  "custom",
] as const satisfies readonly ProviderId[];

export const PROVIDERS: readonly ProviderDefinition[] = [
  {
    id: "groq",
    label: "Groq",
    capabilities: ["asr", "llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://api.groq.com/openai/v1",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "whisper-large-v3-turbo",
        label: "Whisper Large v3 Turbo",
        asrProfile: { languageSupport: "optional_fixed_language" },
      },
      {
        value: "whisper-large-v3",
        label: "Whisper Large v3",
        asrProfile: { languageSupport: "optional_fixed_language" },
      },
      { value: "distil-whisper-large-v3-en", label: "Distil-Whisper", note: "仅英语" },
    ],
    llmModels: [
      {
        value: "llama-3.1-8b-instant",
        label: "Llama 3.1 8B Instant",
        note: "免费和 Developer 账户已于 2026-08-16 停止",
        retiredForNewSelection: true,
      },
      {
        value: "llama-3.3-70b-versatile",
        label: "Llama 3.3 70B",
        note: "免费和 Developer 账户已于 2026-08-16 停止",
        retiredForNewSelection: true,
      },
      { value: "openai/gpt-oss-20b", label: "GPT-OSS 20B", note: "默认" },
      { value: "openai/gpt-oss-120b", label: "GPT-OSS 120B" },
    ],
    defaultAsrModel: "whisper-large-v3-turbo",
    defaultLlmModel: "openai/gpt-oss-20b",
  },
  {
    id: "openai",
    label: "OpenAI",
    capabilities: ["asr", "llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://api.openai.com/v1",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "gpt-transcribe",
        label: "GPT Transcribe",
        note: "默认 · 文件转写",
        asrProfile: { languageSupport: "candidate_language_hints" },
      },
      {
        value: "gpt-4o-mini-transcribe",
        label: "GPT-4o mini Transcribe",
        asrProfile: { languageSupport: "optional_fixed_language" },
      },
      {
        value: "whisper-1",
        label: "Whisper",
        note: "旧模型",
        asrProfile: { languageSupport: "optional_fixed_language" },
      },
    ],
    llmModels: [
      { value: "gpt-4o-mini", label: "GPT-4o mini", note: "默认" },
      { value: "gpt-4o", label: "GPT-4o" },
      { value: "gpt-6-luna", label: "GPT-6 Luna", note: "低延迟整理" },
    ],
    defaultAsrModel: "gpt-transcribe",
    defaultLlmModel: "gpt-4o-mini",
  },
  {
    id: "deepgram",
    label: "Deepgram",
    capabilities: ["asr"],
    protocol: "deepgram",
    defaultBaseUrl: "https://api.deepgram.com",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "nova-3",
        label: "Nova 3",
        note: "默认",
        asrProfile: {
          languageSupport: "auto_detect_or_fixed_language",
          requestNote: "Deepgram Listen：16 kHz 单声道 PCM WAV 作为原始请求体发送。",
          capabilityNote: "此预设未启用 Deepgram 的多语言模式。",
        },
      },
      {
        value: "nova-2",
        label: "Nova 2",
        asrProfile: {
          languageSupport: "auto_detect_or_fixed_language",
          requestNote: "Deepgram Listen：16 kHz 单声道 PCM WAV 作为原始请求体发送。",
          capabilityNote: "此预设未启用 Deepgram 的多语言模式。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "nova-3",
    defaultLlmModel: "",
  },
  {
    id: "siliconflow",
    label: "SiliconFlow",
    capabilities: ["asr", "llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://api.siliconflow.cn/v1",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "text",
    llmModelField: "text",
    asrModels: [
      {
        value: "FunAudioLLM/SenseVoiceSmall",
        label: "SenseVoice Small",
        note: "默认",
        asrProfile: {
          languageSupport: "auto_detect_only",
          requestNote: "SiliconFlow 转写将 16 kHz 单声道 PCM WAV 作为 multipart 文件上传，字段为 file 和 model。",
          responseNote: "文档只定义转写文字；不声明语言、术语、时间戳或置信度元数据。",
          limitNote: "SiliconFlow 文件上限：1 小时、50 MB。",
        },
      },
      {
        value: "Qwen/Qwen3-ASR-1.7B",
        label: "Qwen3-ASR 1.7B",
        note: "可选预设",
        asrProfile: {
          languageSupport: "auto_detect_only",
          requestNote: "SiliconFlow 转写将 16 kHz 单声道 PCM WAV 作为 multipart 文件上传，字段只有 file 和 model。",
          responseNote: "当前文档只定义转写文字；不声明语言、术语、时间戳或置信度元数据。",
          limitNote: "SiliconFlow 文件上限：1 小时、50 MB。",
          availabilityNote: "Qwen 预设服务可用性尚未核实。",
        },
      },
    ],
    llmModels: [
      { value: "deepseek-ai/DeepSeek-V3", label: "DeepSeek V3" },
    ],
    defaultAsrModel: "FunAudioLLM/SenseVoiceSmall",
    defaultLlmModel: "deepseek-ai/DeepSeek-V3",
  },
  {
    id: "fireworks",
    label: "Fireworks",
    capabilities: ["asr"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://audio-turbo.api.fireworks.ai",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "whisper-v3-turbo",
        label: "Whisper v3 Turbo",
        note: "可选预设",
        asrProfile: {
          languageSupport: "auto_detect_only",
          requestNote: "Fireworks Whisper 将 16 kHz 单声道 PCM WAV 作为 multipart 文件上传，字段为 file 和 model。",
          limitNote: "VoiceFlow 单次请求最多发送 10 分钟；更长录音会完整分段处理。",
          availabilityNote: "Fireworks 服务可用性尚未核实。",
          responseNote: "此接线不请求术语提示、时间戳或置信度元数据。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "whisper-v3-turbo",
    defaultLlmModel: "",
  },
  {
    id: "mistral",
    label: "Mistral",
    capabilities: ["asr"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://api.mistral.ai",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "voxtral-mini-2602",
        label: "Voxtral Mini Transcribe 2",
        note: "可选预设",
        asrProfile: {
          languageSupport: "optional_fixed_language",
          requestNote: "Mistral audio/transcriptions 将 16 kHz 单声道 PCM WAV 作为 multipart 音频文件上传，字段为 file 和 model。",
          limitNote: "Mistral 文档列出的单次请求上限为 3 小时。",
          contextNote: "词汇提示最多 100 个词条，通过 context_bias 发送。",
          capabilityNote: "指定语言时不请求片段时间戳；自动检测时才请求片段时间戳。此接线不发送 Whisper prompt，也不使用置信度元数据。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "voxtral-mini-2602",
    defaultLlmModel: "",
  },
  {
    id: "soniox",
    label: "Soniox",
    capabilities: ["asr"],
    protocol: "soniox_realtime",
    defaultBaseUrl: "wss://stt-rt.soniox.com/transcribe-websocket",
    allowsEmptyKey: false,
    hasHttpAsr: false,
    asrTransport: "websocket_stream",
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "stt-rt-v5",
        label: "Soniox Real-time v5",
        note: "实时流式 · 可选",
        asrProfile: {
          languageSupport: "candidate_language_hints",
          requestNote: "选择后使用 WebSocket，在录音期间持续发送麦克风音频。VoiceFlow 按保存的识别语言发送候选提示：自动模式为中文和 English，固定模式为所选语言；提示不会限制识别。当前不发送自定义上下文或术语。",
          responseNote: "临时识别结果只属于当前会话；VoiceFlow 只把最终结果交给整理与历史记录。",
          limitNote: "费用按完整实时音频流时长计算，包括静音；完整音频恢复重放可能再次产生整段费用。",
          availabilityNote: "VoiceFlow 的真实 Soniox 录音连接尚未验证。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "stt-rt-v5",
    defaultLlmModel: "",
  },
  {
    id: "assemblyai",
    label: "AssemblyAI",
    capabilities: ["asr"],
    protocol: "assemblyai_dictation_sync",
    defaultBaseUrl: "https://dictation.assemblyai.com",
    allowsEmptyKey: false,
    hasHttpAsr: true,
    asrTransport: "http_batch",
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "universal-3-5-pro",
        label: "Universal-3.5 Pro",
        note: "最多 120 秒 · 原稿 + 可选整理候选",
        asrProfile: {
          languageSupport: "explicit_language_list",
          requestNote: "专用 AssemblyAI 接口，不是 OpenAI-compatible。总音频不超过 120 秒且允许 AI 整理时使用 Dictation；AI Off、本地-only 或更长录音改用同 provider 的原始 Sync。",
          responseNote: "Dictation 的 text 是独立 ASR 原稿；llm_response 仅作整理候选，llm_error 表示整理失败。候选通过本地最终保护检查后才采用，并跳过第二次 LLM 请求。",
          limitNote: "Dictation 与每次 Sync 请求最多 120 秒。更长录音会用有界重叠分段完整覆盖全稿，再进行一次全稿整理，不逐段改写。",
          capabilityNote: "Dictation 与 Sync 都显式发送 language_codes：auto 为 zh + en，固定语言为单项。Sync 只用 keyterms_prompt 提供术语（最多 100 项 / 8000 字符），不使用会覆盖语言字段的通用 prompt。",
          contextNote: "Dictation 与 Sync 共用同一 provider 和一把 Keychain key；两个 endpoint 均固定。",
          availabilityNote: "保存密钥只表示凭据已配置，不表示服务访问已验证。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "universal-3-5-pro",
    defaultLlmModel: "",
  },
  {
    id: "dashscope",
    label: "DashScope · Qwen Audio",
    capabilities: ["asr"],
    protocol: "dashscope_message",
    defaultBaseUrl: "https://dashscope.aliyuncs.com",
    allowsEmptyKey: false,
    hasHttpAsr: false,
    asrTransport: "websocket_completed_audio",
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "qwen-audio-3.1-asr-flash-message",
        label: "Qwen Audio 3.1 ASR Flash Message",
        note: "Message WebSocket · 原始转写 · 完整覆盖分段",
        asrProfile: {
          languageSupport: "auto_detect_only",
          requestNote: "录音结束后上传已完成音频，使用 qwen-audio-3.1-asr-flash-message Message WebSocket；这是完整音频转写，不是 Soniox 式麦克风实时流。保留原始 ASR，关闭 disfluency removal。该协议按多语言自动识别工作，不提供固定语言提示字段。",
          responseNote: "只使用最终句子结果；若提供句子 / 词时间戳，单位为毫秒。累计 usage 只记录最终值一次。",
          limitNote: "文档给出 7168 输入 / 1024 输出 token 限额，长录音有输出预算风险；使用有界重叠分段并完整覆盖音频。",
          capabilityNote: "此设置只启用 Message 原始 ASR。普通 HTTP qwen-audio-3.1-asr-flash 不属于此接线；原生润色关闭，整理仍由 VoiceFlow 独立完成。",
          availabilityNote: "保存密钥只表示凭据已配置，不表示账号、地区或服务访问已验证。",
        },
      },
    ],
    llmModels: [],
    defaultAsrModel: "qwen-audio-3.1-asr-flash-message",
    defaultLlmModel: "",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    capabilities: ["llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "https://api.deepseek.com",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [],
    llmModels: [
      { value: "deepseek-chat", label: "DeepSeek Chat", note: "默认" },
      { value: "deepseek-reasoner", label: "DeepSeek Reasoner" },
    ],
    defaultAsrModel: "",
    defaultLlmModel: "deepseek-chat",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    capabilities: ["llm"],
    protocol: "anthropic",
    defaultBaseUrl: "https://api.anthropic.com",
    allowsEmptyKey: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [],
    llmModels: [
      { value: "claude-sonnet-4-5", label: "Claude Sonnet 4.5", note: "默认" },
      { value: "claude-opus-4-6", label: "Claude Opus 4.6" },
    ],
    defaultAsrModel: "",
    defaultLlmModel: "claude-sonnet-4-5",
  },
  {
    id: "ollama",
    label: "Ollama",
    capabilities: ["llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "http://127.0.0.1:11434/v1",
    allowsEmptyKey: true,
    editableBaseUrl: true,
    asrModelField: "select",
    llmModelField: "text",
    asrModels: [],
    llmModels: [],
    defaultAsrModel: "",
    defaultLlmModel: "qwen3.5:4b",
  },
  {
    id: "local_whisper",
    label: "Local Whisper",
    capabilities: ["asr"],
    protocol: "openai_compat",
    defaultBaseUrl: "http://127.0.0.1:9000/v1",
    allowsEmptyKey: true,
    editableBaseUrl: true,
    asrModelField: "text",
    llmModelField: "select",
    asrModels: [
      { value: "whisper-large-v3-turbo", label: "whisper-large-v3-turbo", note: "faster-whisper / 多数本机服务" },
      { value: "ggml-large-v3-turbo.bin", label: "Whisper Turbo ggml", note: "Handy 同款 · 约 1.5 GB" },
      { value: "ggml-small.bin", label: "Whisper Small ggml", note: "Handy 同款 · 约 465 MB" },
      { value: "whisper-medium-q4_1.bin", label: "Whisper Medium Q4", note: "Handy 同款 · 约 469 MB" },
      { value: "ggml-large-v3-q5_0.bin", label: "Whisper Large Q5", note: "Handy 同款 · 约 1.0 GB" },
    ],
    llmModels: [],
    defaultAsrModel: "whisper-large-v3-turbo",
    defaultLlmModel: "",
  },
  {
    id: "on_device",
    label: "本机模型",
    capabilities: ["asr"],
    protocol: "on_device_local",
    defaultBaseUrl: "",
    allowsEmptyKey: true,
    hasHttpAsr: false,
    editableBaseUrl: false,
    asrModelField: "select",
    llmModelField: "select",
    asrModels: [
      {
        value: "qwen3-asr-0.6b",
        label: "Qwen3-ASR 0.6B",
        note: "默认 · 需 Apple Silicon 与 macOS 14 或更新版本",
        asrProfile: { languageSupport: "auto_detect_or_fixed_language" },
      },
      {
        value: "qwen3-asr-1.7b",
        label: "Qwen3-ASR 1.7B",
        note: "需 Apple Silicon 与 macOS 14 或更新版本",
        asrProfile: { languageSupport: "auto_detect_or_fixed_language" },
      },
      {
        value: "cohere-transcribe-2b",
        label: "Cohere Transcribe 2B",
        note: "需 Apple Silicon、macOS 14 或更新版本，以及中文或 English 固定语言",
        asrProfile: {
          languageSupport: "explicit_language_required",
          capabilityNote: "此模型不支持自动语言检测；请选择中文或 English。",
        },
      },
      {
        value: "sensevoice-small",
        label: "SenseVoice Small",
        note: "旧版文件保留；没有 MLX 推理支持",
        retiredForNewSelection: true,
        asrProfile: { languageSupport: "auto_detect_only" },
      },
    ],
    llmModels: [],
    defaultAsrModel: "qwen3-asr-0.6b",
    defaultLlmModel: "",
  },
  {
    id: "custom",
    label: "OpenAI Compatible",
    capabilities: ["asr", "llm"],
    protocol: "openai_compat",
    defaultBaseUrl: "",
    allowsEmptyKey: true,
    editableBaseUrl: true,
    asrModelField: "text",
    llmModelField: "text",
    asrModels: [],
    llmModels: [],
    defaultAsrModel: "",
    defaultLlmModel: "",
  },
];

const HOST_ALIASES: ReadonlyArray<readonly [string, ProviderId]> = [
  ["api.groq.com", "groq"],
  ["api.openai.com", "openai"],
  ["api.deepgram.com", "deepgram"],
  ["api.siliconflow.cn", "siliconflow"],
  ["audio-turbo.api.fireworks.ai", "fireworks"],
  ["api.mistral.ai", "mistral"],
  ["stt-rt.soniox.com", "soniox"],
  ["api.deepseek.com", "deepseek"],
  ["api.anthropic.com", "anthropic"],
];

export type DashscopeRegion = "beijing" | "singapore";

export function dashscopeEndpointForRegion(region: DashscopeRegion): string {
  return region === "singapore"
    ? "https://dashscope-intl.aliyuncs.com"
    : "https://dashscope.aliyuncs.com";
}

export function dashscopeRegionFromEndpoint(endpoint?: string | null): DashscopeRegion {
  try {
    return new URL(endpoint ?? "").hostname.toLowerCase() === "dashscope-intl.aliyuncs.com"
      ? "singapore"
      : "beijing";
  } catch {
    return "beijing";
  }
}

export function isProviderId(value: string | undefined | null): value is ProviderId {
  return PROVIDER_IDS.some((id) => id === value);
}

export function providerById(id: string | undefined | null): ProviderDefinition | undefined {
  if (!id) return undefined;
  return PROVIDERS.find((provider) => provider.id === id);
}

export function asrModelProfile(id: ProviderId, model: string): AsrModelProfile | undefined {
  const definition = providerById(id);
  return definition?.asrModels.find((option) => option.value === model.trim())?.asrProfile;
}

export function providersFor(capability: ProviderCapability, customAsr = true, customLlm = true): ProviderDefinition[] {
  return PROVIDERS.filter((provider) => {
    if (provider.id === "custom") {
      return capability === "asr" ? customAsr : customLlm;
    }
    return provider.capabilities.includes(capability);
  });
}

export function defaultModel(id: ProviderId, side: ProviderCapability): string {
  const provider = providerById(id);
  if (!provider) return "";
  return side === "asr" ? provider.defaultAsrModel : provider.defaultLlmModel;
}

export function isKnownModel(id: ProviderId, side: ProviderCapability, model: string): boolean {
  const provider = providerById(id);
  if (!provider) return false;
  const options = side === "asr" ? provider.asrModels : provider.llmModels;
  const field = side === "asr" ? provider.asrModelField : provider.llmModelField;
  const trimmed = model.trim();
  if (!trimmed) return false;
  if (field === "text") return true;
  return options.some((option) => option.value === trimmed);
}

export function hostOf(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  try {
    return new URL(trimmed.includes("://") ? trimmed : `https://${trimmed}`).hostname.toLowerCase();
  } catch {
    return trimmed.toLowerCase();
  }
}

export function isLoopbackUrl(url: string): boolean {
  const host = hostOf(url);
  return host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "[::1]";
}

export function inferProviderFromHost(url: string): ProviderId | null {
  const host = hostOf(url);
  if (!host) return null;
  const named = HOST_ALIASES.find(([candidate]) => host === candidate || host.endsWith(`.${candidate}`));
  if (named) return named[1];
  if (isLoopbackUrl(url)) {
    if (url.includes("11434")) return "ollama";
    if (url.includes("9000")) return "local_whisper";
  }
  return null;
}

export function capabilityLabel(capabilities: readonly ProviderCapability[]): "asr" | "llm" | "both" {
  const asr = capabilities.includes("asr");
  const llm = capabilities.includes("llm");
  if (asr && llm) return "both";
  return asr ? "asr" : "llm";
}
