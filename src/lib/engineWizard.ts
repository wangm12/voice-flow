import type { Settings } from "../types/settings";

export type EngineProvider = "groq" | "custom";

export type EngineDraft = {
  asrProvider: EngineProvider;
  cleanupProvider: EngineProvider;
  asrBaseUrl: string;
  cleanupBaseUrl: string;
  asrModel: string;
  cleanupModel: string;
  apiKey: string;
  asrApiKey: string;
  cleanupApiKey: string;
};

export const DEFAULT_ASR_MODEL = "whisper-large-v3-turbo";
export const DEFAULT_CLEANUP_MODEL = "openai/gpt-oss-20b";

export const GROQ_ASR_MODELS = [
  { value: DEFAULT_ASR_MODEL, label: "Whisper Large v3 Turbo", note: "默认 · 更快" },
  { value: "whisper-large-v3", label: "Whisper Large v3", note: "质量更高 · 较慢" },
  { value: "distil-whisper-large-v3-en", label: "Distil-Whisper", note: "仅英语 · 更快" },
] as const;

export const GROQ_CLEANUP_MODELS = [
  { value: DEFAULT_CLEANUP_MODEL, label: "GPT-OSS 20B", note: "默认 · 更快" },
  { value: "openai/gpt-oss-120b", label: "GPT-OSS 120B", note: "质量更高 · 较慢" },
] as const;

export function providerOf(value?: string | null): EngineProvider {
  return value === "custom" ? "custom" : "groq";
}

export function inferProvider(value: string | undefined, url: string | undefined): EngineProvider {
  if (value === "custom" || value === "groq") return value;
  return url?.trim() ? "custom" : "groq";
}

export function isGroqAsrModel(model: string): boolean {
  return GROQ_ASR_MODELS.some((option) => option.value === model.trim());
}

export function isGroqCleanupModel(model: string): boolean {
  return GROQ_CLEANUP_MODELS.some((option) => option.value === model.trim());
}

function hostOf(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  try {
    return new URL(trimmed.includes("://") ? trimmed : `https://${trimmed}`).hostname.toLowerCase();
  } catch {
    return trimmed.toLowerCase();
  }
}

export function hostnameOf(url: string, provider: EngineProvider): string {
  if (provider === "groq") return "api.groq.com";
  return hostOf(url) || url.trim();
}

export function hostChanged(previous: string | undefined, next: string): boolean {
  return hostOf(previous ?? "") !== hostOf(next);
}

export function isAsrReady(settings: Settings): boolean {
  if (inferProvider(settings.asr_provider, settings.asr_base_url) === "custom") {
    return Boolean(settings.asr_base_url?.trim() && settings.asr_api_key_configured);
  }
  return Boolean(settings.api_key_configured);
}

export function isCleanupReady(settings: Settings): boolean {
  if (inferProvider(settings.cleanup_provider, settings.cleanup_base_url) === "custom") {
    return Boolean(settings.cleanup_base_url?.trim() && settings.cleanup_api_key_configured);
  }
  return Boolean(settings.api_key_configured);
}

export function isEngineConnected(settings: Settings): boolean {
  return isAsrReady(settings) && (!settings.cleanup_enabled || isCleanupReady(settings));
}

export function draftFromSettings(settings: Settings): EngineDraft {
  const asrProvider = inferProvider(settings.asr_provider, settings.asr_base_url);
  const cleanupProvider = inferProvider(settings.cleanup_provider, settings.cleanup_base_url);
  return {
    asrProvider,
    cleanupProvider,
    asrBaseUrl: asrProvider === "custom" ? settings.asr_base_url ?? "" : "",
    cleanupBaseUrl: cleanupProvider === "custom" ? settings.cleanup_base_url ?? "" : "",
    asrModel: asrProvider === "custom" ? settings.asr_model : (isGroqAsrModel(settings.asr_model) ? settings.asr_model : DEFAULT_ASR_MODEL),
    cleanupModel: cleanupProvider === "custom"
      ? settings.cleanup_model
      : (isGroqCleanupModel(settings.cleanup_model) ? settings.cleanup_model : DEFAULT_CLEANUP_MODEL),
    apiKey: "",
    asrApiKey: "",
    cleanupApiKey: "",
  };
}

export function switchProvider(draft: EngineDraft, side: "asr" | "cleanup", next: EngineProvider): EngineDraft {
  if (side === "asr") {
    return {
      ...draft,
      asrProvider: next,
      asrBaseUrl: "",
      asrModel: next === "groq" ? DEFAULT_ASR_MODEL : "",
    };
  }
  return {
    ...draft,
    cleanupProvider: next,
    cleanupBaseUrl: "",
    cleanupModel: next === "groq" ? DEFAULT_CLEANUP_MODEL : "",
  };
}

function hasGroqKey(draft: EngineDraft, settings: Settings): boolean {
  return Boolean(draft.apiKey.trim() || settings.api_key_configured);
}

function hasDedicatedKey(
  typed: string,
  configured: boolean | undefined,
  previousUrl: string | undefined,
  nextUrl: string,
): boolean {
  if (typed.trim()) return true;
  return Boolean(configured) && !hostChanged(previousUrl, nextUrl);
}

export function step2Ready(draft: EngineDraft, settings: Settings): boolean {
  const asrReady = draft.asrProvider === "groq"
    ? hasGroqKey(draft, settings) && isGroqAsrModel(draft.asrModel)
    : Boolean(draft.asrBaseUrl.trim() && draft.asrModel.trim())
      && hasDedicatedKey(draft.asrApiKey, settings.asr_api_key_configured, settings.asr_base_url, draft.asrBaseUrl);
  if (!asrReady) return false;
  if (!settings.cleanup_enabled) return true;
  if (draft.cleanupProvider === "groq") {
    return hasGroqKey(draft, settings) && isGroqCleanupModel(draft.cleanupModel);
  }
  return Boolean(draft.cleanupBaseUrl.trim() && draft.cleanupModel.trim())
    && hasDedicatedKey(
      draft.cleanupApiKey,
      settings.cleanup_api_key_configured,
      settings.cleanup_base_url,
      draft.cleanupBaseUrl,
    );
}

export function persistPatch(draft: EngineDraft, settings: Settings): Record<string, string> {
  const patch: Record<string, string> = {
    asr_provider: draft.asrProvider,
    asr_base_url: draft.asrProvider === "groq" ? "" : draft.asrBaseUrl.trim(),
    asr_model: draft.asrModel.trim(),
  };
  if (settings.cleanup_enabled) {
    patch.cleanup_provider = draft.cleanupProvider;
    patch.cleanup_base_url = draft.cleanupProvider === "groq" ? "" : draft.cleanupBaseUrl.trim();
    patch.cleanup_model = draft.cleanupModel.trim();
  }
  if (draft.apiKey.trim()) patch.api_key = draft.apiKey.trim();
  if (draft.asrApiKey.trim()) patch.asr_api_key = draft.asrApiKey.trim();
  if (settings.cleanup_enabled && draft.cleanupApiKey.trim()) {
    patch.cleanup_api_key = draft.cleanupApiKey.trim();
  }
  return patch;
}

export function probePayload(draft: EngineDraft, settings: Settings) {
  return {
    asr_provider: draft.asrProvider,
    cleanup_provider: draft.cleanupProvider,
    asr_base_url: draft.asrProvider === "groq" ? "" : draft.asrBaseUrl.trim(),
    cleanup_base_url: draft.cleanupProvider === "groq" ? "" : draft.cleanupBaseUrl.trim(),
    asr_model: draft.asrModel.trim(),
    cleanup_model: draft.cleanupModel.trim(),
    api_key: draft.apiKey.trim(),
    asr_api_key: draft.asrApiKey.trim(),
    cleanup_api_key: draft.cleanupApiKey.trim(),
    cleanup_enabled: settings.cleanup_enabled,
  };
}

export function maskSecret(value: string): string {
  const key = value.trim();
  if (!key) return "";
  const tail = Array.from(key).slice(-5).join("");
  return `••••${tail}`;
}

export function modelLabel(model: string): string {
  const asr = GROQ_ASR_MODELS.find((option) => option.value === model);
  if (asr) return asr.label;
  const cleanup = GROQ_CLEANUP_MODELS.find((option) => option.value === model);
  if (cleanup) return cleanup.label;
  return model;
}
