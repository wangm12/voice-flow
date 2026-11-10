import type { Settings } from "../types/settings";
import type { JsonObject } from "./customCleanupParameters";
import {
  defaultModel,
  dashscopeEndpointForRegion,
  dashscopeRegionFromEndpoint,
  inferProviderFromHost,
  isLoopbackUrl,
  isProviderId,
  providerById,
  type AsrModelProfile,
  type DashscopeRegion,
  type ProviderId,
} from "./providers";

export type EngineDraft = {
  asrProvider: ProviderId;
  cleanupProvider: ProviderId;
  asrModel: string;
  cleanupModel: string;
  customBaseUrl: string;
  customAsr: boolean;
  customLlm: boolean;
  customCleanupParameters: JsonObject;
  customCleanupParametersText: string;
  ollamaBaseUrl: string;
  localWhisperBaseUrl: string;
  dashscopeRegion?: DashscopeRegion;
  providerKeys: Partial<Record<ProviderId, string>>;
};

export const DEFAULT_ASR_MODEL = defaultModel("groq", "asr");
export const DEFAULT_CLEANUP_MODEL = defaultModel("groq", "llm");

export function providerOf(value?: string | null, url?: string): ProviderId {
  if (isProviderId(value)) return value;
  return inferProviderFromHost(url ?? "") ?? "groq";
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

export function customOriginChanged(previous: string, next: string): boolean {
  const originOf = (value: string) => {
    const raw = value.trim();
    if (!raw) return null;
    try {
      const url = new URL(raw.includes("://") ? raw : `https://${raw}`);
      return url.protocol === "http:" || url.protocol === "https:"
        ? url.origin.toLowerCase()
        : null;
    } catch {
      return null;
    }
  };
  const previousOrigin = originOf(previous);
  const nextOrigin = originOf(next);
  if (previousOrigin && nextOrigin) return previousOrigin !== nextOrigin;
  return previous.trim() !== next.trim();
}

export function customHostChanged(draft: EngineDraft, settings: Settings): boolean {
  const savedUrl = settings.custom_base_url?.trim() || settings.asr_base_url?.trim() || settings.cleanup_base_url?.trim() || "";
  const nextUrl = draft.customBaseUrl.trim();
  return customOriginChanged(savedUrl, nextUrl);
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
  // Native local cleanup routes have no server URL or API key. Model readiness
  // is checked independently by their model-store/runtime boundaries.
  if (id === "on_device_mlx" || id === "on_device_llama_cpp") return true;
  if (draft.providerKeys[id]?.trim()) return true;
  if (id === "custom") {
    const nextUrl = draft.customBaseUrl.trim();
    if (customHostChanged(draft, settings)) return isLoopbackUrl(nextUrl);
  }
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
    customBaseUrl: settings.custom_base_url?.trim() || settings.asr_base_url?.trim() || settings.cleanup_base_url?.trim() || "",
    customAsr: asrProvider === "custom" || (settings.custom_asr ?? true),
    customLlm: cleanupProvider === "custom" || (settings.custom_llm ?? true),
    customCleanupParameters: settings.custom_cleanup_parameters ?? {},
    customCleanupParametersText: settings.custom_cleanup_parameters && Object.keys(settings.custom_cleanup_parameters).length > 0
      ? JSON.stringify(settings.custom_cleanup_parameters, null, 2)
      : "",
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
      ...(next === "custom" ? { customAsr: true } : {}),
    };
  }
  return {
    ...draft,
    cleanupProvider: next,
    cleanupModel: defaultModel(next, "llm"),
    ...(next === "custom" ? { customLlm: true } : {}),
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
    custom_asr: draft.asrProvider === "custom",
    custom_llm: draft.cleanupProvider === "custom",
    custom_cleanup_parameters: draft.customCleanupParameters,
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
  probeStage: "asr" | "cleanup" | "both";
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
    custom_asr: asrProvider === "custom",
    custom_llm: cleanupProvider === "custom",
    custom_cleanup_parameters: cleanupProvider === "custom" ? draft.customCleanupParameters : {},
    probe_stage: overrides?.probeStage ?? "both",
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
