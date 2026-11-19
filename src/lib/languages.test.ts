import { describe, expect, it } from "vitest";
import {
  SUPPORTED_LANGUAGES,
  coerceLanguageForModel,
  languageSupportedByModel,
  modelLanguageSupport,
  normalizePreferredLanguages,
} from "./languages";

describe("languages registry and per-model support", () => {
  it("normalizes aliases and caps preferred_languages at 4 without auto or duplicates", () => {
    expect(
      normalizePreferredLanguages(["zh-Hant-TW", "auto", "ja", "zh-HK", "en", "fr", "de"]),
    ).toEqual(["zh-TW", "ja", "en", "fr"]);
  });

  it("distinguishes per-model auto, hint, and explicit language bounds", () => {
    const groqSupport = modelLanguageSupport("groq", "whisper-large-v3-turbo");
    expect(groqSupport.autoAllowed).toBe(true);
    expect(groqSupport.hintsAllowed).toBe(false);
    expect(groqSupport.allowedCodes).toContain("ar");

    const sonioxSupport = modelLanguageSupport("soniox", "stt-rt-v5");
    expect(sonioxSupport.autoAllowed).toBe(true);
    expect(sonioxSupport.hintsAllowed).toBe(true);

    const ondeviceQwen = modelLanguageSupport("on_device", "qwen3-asr-1.7b");
    expect(ondeviceQwen.autoAllowed).toBe(true);
    expect(ondeviceQwen.hintsAllowed).toBe(false);
    expect(ondeviceQwen.allowedCodes).not.toContain("zh-TW");
    expect(languageSupportedByModel("on_device", "qwen3-asr-1.7b", "zh")).toBe(true);
    expect(languageSupportedByModel("on_device", "qwen3-asr-1.7b", "zh-TW")).toBe(false);
    expect(languageSupportedByModel("on_device", "qwen3-asr-1.7b", "ar")).toBe(false);

    const assemblyai = modelLanguageSupport("assemblyai", "universal-3-5-pro");
    expect(assemblyai.allowedCodes).toContain("zh-TW");
    expect(assemblyai.allowedCodes).toContain("ko");
    expect(assemblyai.allowedCodes).toContain("uk");

    const cohere = modelLanguageSupport("on_device", "cohere-transcribe-2b");
    expect(cohere.autoAllowed).toBe(false);
    expect(cohere.allowedCodes).toEqual(["zh", "en"]);
  });

  it("coerces unsupported languages cleanly when switching models", () => {
    expect(coerceLanguageForModel("on_device", "qwen3-asr-1.7b", "zh-TW")).toBe("zh");
    expect(coerceLanguageForModel("on_device", "qwen3-asr-1.7b", "ar")).toBe("auto");
    expect(coerceLanguageForModel("deepgram", "nova-3", "yue")).toBe("zh-TW");
    expect(coerceLanguageForModel("on_device", "cohere-transcribe-2b", "auto")).toBe("zh");

    const options = SUPPORTED_LANGUAGES.filter((entry) =>
      languageSupportedByModel("on_device", "qwen3-asr-1.7b", entry.code),
    );
    expect(options[0].code).toBe("auto");
    expect(options.some((entry) => entry.code === "ar")).toBe(false);
  });
});
