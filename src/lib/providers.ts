export type ProviderId =
  | "groq"
  | "openai"
  | "deepgram"
  | "siliconflow"
  | "deepseek"
  | "anthropic"
  | "ollama"
  | "local_whisper"
  | "custom";

export type ProviderCapability = "asr" | "llm";
export type ProviderProtocol = "openai_compat" | "deepgram" | "anthropic";
export type ModelField = "select" | "text";

export type ModelOption = {
  value: string;
  label: string;
  note?: string;
};

export type ProviderDefinition = {
  id: ProviderId;
  label: string;
  capabilities: readonly ProviderCapability[];
  protocol: ProviderProtocol;
  defaultBaseUrl: string;
  allowsEmptyKey: boolean;
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
  "deepseek",
  "anthropic",
  "ollama",
  "local_whisper",
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
      { value: "whisper-large-v3-turbo", label: "Whisper Large v3 Turbo", note: "默认 · 更快" },
      { value: "whisper-large-v3", label: "Whisper Large v3", note: "质量更高 · 较慢" },
      { value: "distil-whisper-large-v3-en", label: "Distil-Whisper", note: "仅英语 · 更快" },
    ],
    llmModels: [
      { value: "openai/gpt-oss-20b", label: "GPT-OSS 20B", note: "默认 · 更快" },
      { value: "openai/gpt-oss-120b", label: "GPT-OSS 120B", note: "质量更高 · 较慢" },
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
      { value: "whisper-1", label: "Whisper", note: "默认" },
      { value: "gpt-4o-mini-transcribe", label: "GPT-4o mini Transcribe", note: "更快" },
    ],
    llmModels: [
      { value: "gpt-4o-mini", label: "GPT-4o mini", note: "默认 · 更快" },
      { value: "gpt-4o", label: "GPT-4o", note: "质量更高" },
    ],
    defaultAsrModel: "whisper-1",
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
      { value: "nova-3", label: "Nova 3", note: "默认" },
      { value: "nova-2", label: "Nova 2" },
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
      { value: "FunAudioLLM/SenseVoiceSmall", label: "SenseVoice Small" },
    ],
    llmModels: [
      { value: "deepseek-ai/DeepSeek-V3", label: "DeepSeek V3" },
    ],
    defaultAsrModel: "FunAudioLLM/SenseVoiceSmall",
    defaultLlmModel: "deepseek-ai/DeepSeek-V3",
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
      { value: "claude-sonnet-4-5", label: "Claude Sonnet 4.5", note: "默认 · 更快" },
      { value: "claude-opus-4-6", label: "Claude Opus 4.6", note: "质量更高" },
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
    defaultLlmModel: "llama3.2",
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
    asrModels: [],
    llmModels: [],
    defaultAsrModel: "whisper-1",
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
  ["api.deepseek.com", "deepseek"],
  ["api.anthropic.com", "anthropic"],
];

export function isProviderId(value: string | undefined | null): value is ProviderId {
  return PROVIDER_IDS.some((id) => id === value);
}

export function providerById(id: string | undefined | null): ProviderDefinition | undefined {
  if (!id) return undefined;
  return PROVIDERS.find((provider) => provider.id === id);
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
