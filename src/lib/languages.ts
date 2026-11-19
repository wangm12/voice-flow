export type LanguageOption = {
  code: string;
  nativeName: string;
  englishName: string;
  chineseName: string;
};

export const MAX_PREFERRED_LANGUAGES = 4;

export const SUPPORTED_LANGUAGES: readonly LanguageOption[] = [
  { code: "auto", nativeName: "自动检测", englishName: "Auto-detect", chineseName: "自动检测" },
  { code: "zh", nativeName: "中文（简体）", englishName: "Chinese (Simplified)", chineseName: "中文（简体）" },
  { code: "zh-TW", nativeName: "繁體中文", englishName: "Chinese (Traditional)", chineseName: "繁体中文" },
  { code: "yue", nativeName: "粵語", englishName: "Cantonese", chineseName: "粤语" },
  { code: "en", nativeName: "English", englishName: "English", chineseName: "英语" },
  { code: "ja", nativeName: "日本語", englishName: "Japanese", chineseName: "日语" },
  { code: "ko", nativeName: "한국어", englishName: "Korean", chineseName: "韩语" },
  { code: "es", nativeName: "Español", englishName: "Spanish", chineseName: "西班牙语" },
  { code: "fr", nativeName: "Français", englishName: "French", chineseName: "法语" },
  { code: "de", nativeName: "Deutsch", englishName: "German", chineseName: "德语" },
  { code: "pt", nativeName: "Português", englishName: "Portuguese", chineseName: "葡萄牙语" },
  { code: "it", nativeName: "Italiano", englishName: "Italian", chineseName: "意大利语" },
  { code: "ru", nativeName: "Русский", englishName: "Russian", chineseName: "俄语" },
  { code: "ar", nativeName: "العربية", englishName: "Arabic", chineseName: "阿拉伯语" },
  { code: "hi", nativeName: "हिन्दी", englishName: "Hindi", chineseName: "印地语" },
  { code: "vi", nativeName: "Tiếng Việt", englishName: "Vietnamese", chineseName: "越南语" },
  { code: "th", nativeName: "ไทย", englishName: "Thai", chineseName: "泰语" },
  { code: "id", nativeName: "Bahasa Indonesia", englishName: "Indonesian", chineseName: "印度尼西亚语" },
  { code: "nl", nativeName: "Nederlands", englishName: "Dutch", chineseName: "荷兰语" },
  { code: "pl", nativeName: "Polski", englishName: "Polish", chineseName: "波兰语" },
  { code: "tr", nativeName: "Türkçe", englishName: "Turkish", chineseName: "土耳其语" },
  { code: "uk", nativeName: "Українська", englishName: "Ukrainian", chineseName: "乌克兰语" },
  { code: "sv", nativeName: "Svenska", englishName: "Swedish", chineseName: "瑞典语" },
];

export const PREFERRED_LANGUAGE_OPTIONS: readonly LanguageOption[] = SUPPORTED_LANGUAGES.filter(
  (language) => language.code !== "auto",
);

export function canonicalLanguageCode(raw: string | null | undefined): string | null {
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const normalized = trimmed.replace(/_/g, "-").toLowerCase();
  if (normalized === "auto") return "auto";
  if (["zh", "zh-cn", "zh-sg", "zh-hans", "zh-hans-cn", "zh-hans-sg", "cmn"].includes(normalized)) return "zh";
  if (["zh-tw", "zh-hk", "zh-mo", "zh-hant", "zh-hant-tw", "zh-hant-hk"].includes(normalized)) return "zh-TW";
  if (["yue", "yue-hant", "yue-hk", "zh-yue"].includes(normalized)) return "yue";
  if (normalized === "in") return "id";
  const exact = SUPPORTED_LANGUAGES.find((item) => item.code.toLowerCase() === normalized);
  if (exact) return exact.code;
  const primary = normalized.split("-")[0] ?? "";
  if (!primary || primary === "auto") return null;
  if (primary === "zh") return "zh";
  const byPrimary = SUPPORTED_LANGUAGES.find((item) => item.code !== "auto" && item.code.toLowerCase() === primary);
  return byPrimary ? byPrimary.code : null;
}

export function normalizePreferredLanguages(values: readonly string[] | undefined): string[] {
  if (!values?.length) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of values) {
    const code = canonicalLanguageCode(raw);
    if (!code || code === "auto" || seen.has(code)) continue;
    seen.add(code);
    out.push(code);
    if (out.length >= MAX_PREFERRED_LANGUAGES) break;
  }
  return out;
}

export function languageDisplayLabel(code: string | null | undefined, t: (key: string) => string): string {
  const canonical = canonicalLanguageCode(code) ?? "auto";
  if (canonical === "auto") return t("自动检测");
  const entry = SUPPORTED_LANGUAGES.find((item) => item.code === canonical);
  if (!entry) return canonical;
  if (canonical === "zh") return t("中文（简体）");
  if (canonical === "zh-TW") return t("繁體中文");
  return entry.nativeName;
}

export type ModelLanguageSupport = {
  allowedCodes: readonly string[];
  autoAllowed: boolean;
  hintsAllowed: boolean;
};

const ALL_LANGUAGE_CODES: readonly string[] = SUPPORTED_LANGUAGES.map((item) => item.code);
const ON_DEVICE_AUTO_OR_EXPLICIT_CODES: readonly string[] = ["auto", "zh", "en"];
const ON_DEVICE_COHERE_CODES: readonly string[] = ["zh", "en"];
const AUTO_ONLY_CODES: readonly string[] = ["auto"];
const EN_ONLY_CODES: readonly string[] = ["en"];
const DEEPGRAM_ALLOWED_CODES: readonly string[] = [
  "auto", "zh", "zh-TW", "en", "ja", "ko", "es", "fr", "de", "pt", "it", "ru", "hi", "vi", "th",
  "id", "nl", "pl", "tr", "uk", "sv",
];

export function modelLanguageSupport(
  provider: string | undefined,
  model: string | undefined,
): ModelLanguageSupport {
  const trimmedModel = (model ?? "").trim().toLowerCase();
  switch (provider) {
    case "on_device":
      if (
        trimmedModel === "parakeet-tdt-0.6b-v3-8bit"
        || trimmedModel === "firered-asr2-aed-mlx"
        || trimmedModel === "sensevoice-small"
      ) {
        return { allowedCodes: AUTO_ONLY_CODES, autoAllowed: true, hintsAllowed: false };
      }
      if (trimmedModel === "cohere-transcribe-2b") {
        return { allowedCodes: ON_DEVICE_COHERE_CODES, autoAllowed: false, hintsAllowed: false };
      }
      return { allowedCodes: ON_DEVICE_AUTO_OR_EXPLICIT_CODES, autoAllowed: true, hintsAllowed: false };
    case "siliconflow":
    case "fireworks":
    case "dashscope":
      return { allowedCodes: AUTO_ONLY_CODES, autoAllowed: true, hintsAllowed: false };
    case "groq":
      if (trimmedModel === "distil-whisper-large-v3-en") {
        return { allowedCodes: EN_ONLY_CODES, autoAllowed: false, hintsAllowed: false };
      }
      return { allowedCodes: ALL_LANGUAGE_CODES, autoAllowed: true, hintsAllowed: false };
    case "openai":
      return {
        allowedCodes: ALL_LANGUAGE_CODES,
        autoAllowed: true,
        hintsAllowed: trimmedModel === "gpt-transcribe" || trimmedModel.startsWith("gpt-transcribe-") || trimmedModel === "",
      };
    case "soniox":
      return { allowedCodes: ALL_LANGUAGE_CODES, autoAllowed: true, hintsAllowed: true };
    case "assemblyai":
      return { allowedCodes: ALL_LANGUAGE_CODES, autoAllowed: true, hintsAllowed: true };
    case "deepgram":
      return {
        allowedCodes: DEEPGRAM_ALLOWED_CODES,
        autoAllowed: true,
        hintsAllowed: trimmedModel === "nova-3" || trimmedModel.startsWith("nova-3-") || trimmedModel === "",
      };
    default:
      return { allowedCodes: ALL_LANGUAGE_CODES, autoAllowed: true, hintsAllowed: false };
  }
}

export function languageSupportedByModel(
  provider: string | undefined,
  model: string | undefined,
  language: string | undefined,
): boolean {
  const canonical = canonicalLanguageCode(language);
  if (!canonical) return false;
  const support = modelLanguageSupport(provider, model);
  return support.allowedCodes.includes(canonical);
}

export function coerceLanguageForModel(
  provider: string | undefined,
  model: string | undefined,
  language: string | undefined,
): string {
  const support = modelLanguageSupport(provider, model);
  const canonical = canonicalLanguageCode(language);
  if (canonical) {
    if (support.allowedCodes.includes(canonical)) return canonical;
    if (canonical === "yue" && support.allowedCodes.includes("zh-TW")) {
      return "zh-TW";
    }
    if ((canonical === "zh-TW" || canonical === "yue") && support.allowedCodes.includes("zh")) {
      return "zh";
    }
  }
  return support.autoAllowed ? "auto" : (support.allowedCodes[0] ?? "en");
}
