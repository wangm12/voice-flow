import type { Settings } from "../types/settings";
import {
  defaultModel,
  dashscopeEndpointForRegion,
  dashscopeRegionFromEndpoint,
  inferProviderFromHost,
  isKnownModel,
  isLoopbackUrl,
  isProviderId,
  providerById,
  type AsrModelProfile,
  type DashscopeRegion,
  type ProviderId,
} from "./providers";

export type EngineProvider = ProviderId;

export type EngineDraft = {
  asrProvider: ProviderId;
  cleanupProvider: ProviderId;
  asrModel: string;
  cleanupModel: string;
  customBaseUrl: string;
  customAsr: boolean;
  customLlm: boolean;
  ollamaBaseUrl: string;
  localWhisperBaseUrl: string;
  dashscopeRegion?: DashscopeRegion;
  providerKeys: Partial<Record<ProviderId, string>>;
};

export const DEFAULT_ASR_MODEL = defaultModel("groq", "asr");
export const DEFAULT_CLEANUP_MODEL = defaultModel("groq", "llm");

export const GROQ_ASR_MODELS = providerById("groq")?.asrModels ?? [];
export const GROQ_CLEANUP_MODELS = providerById("groq")?.llmModels ?? [];

export function providerOf(value?: string | null, url?: string): ProviderId {
  if (isProviderId(value)) return value;
  return inferProviderFromHost(url ?? "") ?? "groq";
}

export function inferProvider(value: string | undefined, url: string | undefined): ProviderId {
  return providerOf(value, url);
}

export function isGroqAsrModel(model: string): boolean {
  return isKnownModel("groq", "asr", model);
}

export function isGroqCleanupModel(model: string): boolean {
  return isKnownModel("groq", "llm", model);
}

export function hostnameOf(url: string, provider: ProviderId): string {
  const named = providerById(provider);
  if (named && !named.editableBaseUrl) {
    try {
      return new URL(named.defaultBaseUrl).hostname;
    } catch {
      return named.label;
    }
  }
  if (!url.trim()) return "";
  try {
    return new URL(url.includes("://") ? url : `https://${url}`).hostname.toLowerCase();
  } catch {
    return url.trim();
  }
}

export function providerUrl(id: ProviderId, draft: Pick<EngineDraft, "customBaseUrl" | "ollamaBaseUrl" | "localWhisperBaseUrl" | "dashscopeRegion">): string {
  const named = providerById(id);
  if (id === "custom") return draft.customBaseUrl.trim();
  if (id === "ollama") return draft.ollamaBaseUrl.trim() || named?.defaultBaseUrl || "";
  if (id === "local_whisper") return draft.localWhisperBaseUrl.trim() || named?.defaultBaseUrl || "";
  if (id === "dashscope") return dashscopeEndpointForRegion(draft.dashscopeRegion ?? "beijing");
  return named?.defaultBaseUrl ?? "";
}

export function providerConfigured(settings: Settings, id: ProviderId): boolean {
  if (settings.provider_keys?.[id]?.configured) return true;
  if (id === "groq") return Boolean(settings.api_key_configured);
  if (id === "custom") {
    return Boolean(settings.asr_api_key_configured || settings.cleanup_api_key_configured);
  }
  return false;
}

export function providerHint(settings: Settings, id: ProviderId): string {
  return settings.provider_keys?.[id]?.hint
    ?? (id === "groq" ? settings.api_key_hint : null)
    ?? (id === "custom" ? settings.asr_api_key_hint ?? settings.cleanup_api_key_hint : null)
    ?? "";
}

export function hasProviderSecret(
  id: ProviderId,
  draft: EngineDraft,
  settings: Settings,
  onDeviceReady = false,
): boolean {
  if (id === "on_device") return onDeviceReady;
  if (draft.providerKeys[id]?.trim()) return true;
  if (providerConfigured(settings, id)) return true;
  const named = providerById(id);
  return Boolean(named?.allowsEmptyKey && isLoopbackUrl(providerUrl(id, draft)));
}

export function isAsrReady(settings: Settings, onDeviceReady = false): boolean {
  const id = providerOf(settings.asr_provider, settings.asr_base_url);
  const draft = draftFromSettings(settings);
  return hasProviderSecret(id, draft, settings, onDeviceReady) && Boolean(settings.asr_model.trim());
}

export function isCleanupReady(settings: Settings): boolean {
  const id = providerOf(settings.cleanup_provider, settings.cleanup_base_url);
  const draft = draftFromSettings(settings);
  return hasProviderSecret(id, draft, settings) && Boolean(settings.cleanup_model.trim());
}

export function isEngineConnected(settings: Settings, onDeviceReady = false): boolean {
  const asrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const fusedAssemblyAiCanProvideCleanup = asrProvider === "assemblyai";
  return isAsrReady(settings, onDeviceReady)
    && (!settings.cleanup_enabled || fusedAssemblyAiCanProvideCleanup || isCleanupReady(settings));
}

export function draftFromSettings(settings: Settings): EngineDraft {
  const asrProvider = providerOf(settings.asr_provider, settings.asr_base_url ?? settings.custom_base_url);
  const cleanupProvider = providerOf(settings.cleanup_provider, settings.cleanup_base_url ?? settings.custom_base_url);
  return {
    asrProvider,
    cleanupProvider,
    asrModel: settings.asr_model || defaultModel(asrProvider, "asr"),
    cleanupModel: settings.cleanup_model || defaultModel(cleanupProvider, "llm"),
    customBaseUrl: settings.custom_base_url ?? settings.asr_base_url ?? settings.cleanup_base_url ?? "",
    customAsr: settings.custom_asr ?? true,
    customLlm: settings.custom_llm ?? true,
    ollamaBaseUrl: settings.ollama_base_url || providerById("ollama")?.defaultBaseUrl || "",
    localWhisperBaseUrl: settings.local_whisper_base_url || providerById("local_whisper")?.defaultBaseUrl || "",
    dashscopeRegion: dashscopeRegionFromEndpoint(settings.asr_base_url),
    providerKeys: {},
  };
}

export function switchProvider(draft: EngineDraft, side: "asr" | "cleanup", next: ProviderId): EngineDraft {
  if (side === "asr") {
    return {
      ...draft,
      asrProvider: next,
      asrModel: defaultModel(next, "asr"),
    };
  }
  return {
    ...draft,
    cleanupProvider: next,
    cleanupModel: defaultModel(next, "llm"),
  };
}

export function step2Ready(draft: EngineDraft, settings: Settings, onDeviceReady = false): boolean {
  if (!draft.asrModel.trim() || !hasProviderSecret(draft.asrProvider, draft, settings, onDeviceReady)) return false;
  if (draft.asrProvider === "custom" && !draft.customBaseUrl.trim()) return false;
  if (!settings.cleanup_enabled || draft.asrProvider === "assemblyai") return true;
  if (!draft.cleanupModel.trim() || !hasProviderSecret(draft.cleanupProvider, draft, settings)) return false;
  if (draft.cleanupProvider === "custom" && !draft.customBaseUrl.trim()) return false;
  return true;
}

function typedKeys(draft: EngineDraft): Record<string, string> {
  return Object.fromEntries(
    Object.entries(draft.providerKeys)
      .filter((entry): entry is [string, string] => Boolean(entry[1]?.trim()))
      .map(([id, key]) => [id, key.trim()]),
  );
}

export function persistPatch(draft: EngineDraft, settings: Settings): Record<string, unknown> {
  const patch: Record<string, unknown> = {
    asr_provider: draft.asrProvider,
    asr_model: draft.asrModel.trim(),
    custom_base_url: draft.customBaseUrl.trim(),
    custom_asr: draft.customAsr,
    custom_llm: draft.customLlm,
    ollama_base_url: draft.ollamaBaseUrl.trim(),
    local_whisper_base_url: draft.localWhisperBaseUrl.trim(),
  };
  if (draft.asrProvider === "assemblyai" || draft.asrProvider === "dashscope") {
    patch.asr_base_url = providerUrl(draft.asrProvider, draft);
  }
  if (settings.cleanup_enabled) {
    patch.cleanup_provider = draft.cleanupProvider;
    patch.cleanup_model = draft.cleanupModel.trim();
  }
  const keys = typedKeys(draft);
  if (Object.keys(keys).length > 0) {
    patch.provider_keys = keys;
    if (keys.groq) patch.api_key = keys.groq;
    if (keys.custom) {
      patch.asr_api_key = keys.custom;
      patch.cleanup_api_key = keys.custom;
    }
  }
  return patch;
}

export function probePayload(draft: EngineDraft, settings: Settings, overrides?: Partial<{
  asrProvider: ProviderId;
  cleanupProvider: ProviderId;
  cleanupEnabled: boolean;
}>) {
  const asrProvider = overrides?.asrProvider ?? draft.asrProvider;
  const cleanupProvider = overrides?.cleanupProvider ?? draft.cleanupProvider;
  const keys = typedKeys(draft);
  return {
    asr_provider: asrProvider,
    cleanup_provider: cleanupProvider,
    asr_base_url: asrProvider === "custom" ? draft.customBaseUrl.trim() : providerUrl(asrProvider, draft),
    cleanup_base_url: cleanupProvider === "custom" ? draft.customBaseUrl.trim() : providerUrl(cleanupProvider, draft),
    asr_model: asrProvider === draft.asrProvider ? draft.asrModel.trim() : defaultModel(asrProvider, "asr"),
    cleanup_model: cleanupProvider === draft.cleanupProvider ? draft.cleanupModel.trim() : defaultModel(cleanupProvider, "llm"),
    custom_base_url: draft.customBaseUrl.trim(),
    custom_asr: draft.customAsr,
    custom_llm: draft.customLlm,
    ollama_base_url: draft.ollamaBaseUrl.trim(),
    local_whisper_base_url: draft.localWhisperBaseUrl.trim(),
    provider_keys: keys,
    api_key: keys.groq ?? "",
    asr_api_key: keys.custom ?? "",
    cleanup_api_key: keys.custom ?? "",
    cleanup_enabled: overrides?.cleanupEnabled ?? settings.cleanup_enabled,
  };
}

export function maskSecret(value: string): string {
  const key = value.trim();
  if (!key) return "";
  const tail = Array.from(key).slice(-5).join("");
  return `••••${tail}`;
}

export function modelLabel(model: string): string {
  for (const provider of ["groq", "openai", "deepgram", "siliconflow", "fireworks", "mistral", "soniox", "assemblyai", "dashscope", "deepseek", "anthropic"] as const) {
    const definition = providerById(provider);
    const match = [...(definition?.asrModels ?? []), ...(definition?.llmModels ?? [])]
      .find((option) => option.value === model);
    if (match) return match.label;
  }
  return model;
}

export function asrLanguageDescription(
  profile: AsrModelProfile | undefined,
  language: string,
  t: (key: string) => string,
): string {
  if (!profile) return t("当前模型的语言行为尚未核实，请以服务商文档为准。");
  const fixed = language !== "auto";
  switch (profile.languageSupport) {
    case "auto_detect_only":
      return fixed
        ? t("此模型只自动检测语言；当前固定语言选择不会发送。")
        : t("此模型只使用自动语言检测。");
    case "auto_detect_or_fixed_language":
      return fixed
        ? `${t("当前按单一固定语言转写：")} ${language === "zh" ? t("中文") : t("English")}`
        : t("此模型可自动检测，也可按单一支持的语言转写。");
    case "optional_fixed_language":
      return fixed
        ? `${t("当前按单一固定语言转写：")} ${language === "zh" ? t("中文") : t("English")}`
        : t("此模型可自动检测，也可指定一种固定语言。");
    case "explicit_language_required":
      return language === "zh" || language === "en"
        ? `${t("此模型要求固定语言：")} ${language === "zh" ? t("中文") : t("English")}`
        : t("此模型要求固定使用中文或 English；自动语言检测不可用。请在录音设置中更改识别语言。");
    case "explicit_language_list":
      return language === "auto"
        ? t("自动设置会显式请求中文和 English 的语言列表；这不是服务端自动检测。")
        : `${t("当前明确发送单一识别语言：")} ${language === "zh" ? t("中文") : t("English")}`;
    case "candidate_language_hints":
      return fixed
        ? t("当前语言会作为候选提示发送，不会锁定识别语言。")
        : t("自动检测；固定语言可作为候选提示，不会锁定识别语言。");
    case "unsupported":
      return t("此模型不接受语言参数；当前选择不会发送。");
  }
}

export function hasSupportedAsrLanguage(profile: AsrModelProfile | undefined, language: string): boolean {
  return profile?.languageSupport !== "explicit_language_required" || language === "zh" || language === "en";
}
