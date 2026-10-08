import { describe, expect, it } from "vitest";
import {
  DEFAULT_ASR_MODEL,
  DEFAULT_CLEANUP_MODEL,
  draftFromSettings,
  hostnameOf,
  isEngineConnected,
  maskSecret,
  persistPatch,
  step2Ready,
  switchProvider,
  type EngineDraft,
} from "./engineWizard";
import type { Settings } from "../types/settings";

const groqConnected: Settings = {
  schema_version: 17,
  api_key_configured: true,
  api_key_hint: "••••abcd",
  asr_model: DEFAULT_ASR_MODEL,
  cleanup_model: DEFAULT_CLEANUP_MODEL,
  language: "auto",
  ui_language: "zh",
  theme: "system",
  dictionary: [],
  chunk_threshold_secs: 25,
  chunk_length_secs: 35,
  long_output_mode: "paste",
  delivery_policy: "auto",
  keep_audio_days: 7,
  keep_history_days: 90,
  onboarded: true,
  cleanup_enabled: true,
  show_tray_icon: true,
  hotkey: "CmdOrControl+Alt+Space",
  activation_mode: "tap",
  context_enabled: true,
  browser_access_enabled: false,
  context_mappings: [],
  writing_modes: [],
  snippets: [],
  output_mode: "auto",
  translation_target_language: "en",
  input_device: "",
  asr_provider: "groq",
  cleanup_provider: "groq",
  provider_keys: {
    groq: { configured: true, hint: "••••abcd" },
  },
};

function emptyDraft(overrides: Partial<EngineDraft> = {}): EngineDraft {
  return {
    asrProvider: "groq",
    cleanupProvider: "groq",
    asrModel: DEFAULT_ASR_MODEL,
    cleanupModel: DEFAULT_CLEANUP_MODEL,
    customBaseUrl: "",
    customAsr: true,
    customLlm: true,
    ollamaBaseUrl: "http://127.0.0.1:11434/v1",
    localWhisperBaseUrl: "http://127.0.0.1:9000/v1",
    providerKeys: {},
    ...overrides,
  };
}

describe("engine wizard rules", () => {
  it("treats an onboarded Groq key as connected", () => {
    expect(isEngineConnected(groqConnected)).toBe(true);
    expect(isEngineConnected({ ...groqConnected, api_key_configured: false, provider_keys: {} })).toBe(false);
  });

  it("treats selected OpenAI ASR and cleanup keys as connected without a Groq key", () => {
    expect(isEngineConnected({
      ...groqConnected,
      api_key_configured: false,
      asr_provider: "openai",
      asr_model: "whisper-1",
      cleanup_provider: "openai",
      cleanup_model: "gpt-4o-mini",
      provider_keys: { openai: { configured: true, hint: "••••token" } },
    })).toBe(true);
  });

  it("allows ASR-only when cleanup is disabled", () => {
    expect(
      isEngineConnected({
        ...groqConnected,
        api_key_configured: false,
        provider_keys: { custom: { configured: true, hint: "••••ocal" } },
        asr_provider: "custom",
        custom_base_url: "http://127.0.0.1:8000/v1",
        asr_api_key_configured: true,
        cleanup_enabled: false,
      }),
    ).toBe(true);
  });

  it("requires a cleanup key when cleanup is OpenAI and enabled", () => {
    expect(
      isEngineConnected({
        ...groqConnected,
        cleanup_provider: "openai",
        cleanup_model: "gpt-4o-mini",
        provider_keys: {
          groq: { configured: true, hint: "••••abcd" },
          openai: { configured: false, hint: null },
        },
      }),
    ).toBe(false);
  });

  it("resets the model and keeps pool keys when switching a side", () => {
    const draft = switchProvider(
      emptyDraft({
        asrProvider: "custom",
        cleanupProvider: "openai",
        asrModel: "whisper-1",
        cleanupModel: "gpt-4o-mini",
        providerKeys: { groq: "gsk_keep", openai: "sk-keep" },
      }),
      "asr",
      "groq",
    );
    expect(draft.asrProvider).toBe("groq");
    expect(draft.asrModel).toBe(DEFAULT_ASR_MODEL);
    expect(draft.cleanupModel).toBe("gpt-4o-mini");
    expect(draft.providerKeys.groq).toBe("gsk_keep");
    expect(draft.providerKeys.openai).toBe("sk-keep");
  });

  it("does not copy Groq model ids when switching to custom", () => {
    const draft = switchProvider(draftFromSettings(groqConnected), "cleanup", "custom");
    expect(draft.cleanupProvider).toBe("custom");
    expect(draft.cleanupModel).toBe("");
  });

  it("requires URL, model, and key for custom step 2", () => {
    const draft = emptyDraft({
      asrProvider: "custom",
      asrModel: "",
      providerKeys: { groq: "gsk_test" },
    });
    expect(step2Ready(draft, groqConnected)).toBe(false);
    expect(step2Ready({ ...draft, customBaseUrl: "https://relay.example.com/v1", asrModel: "whisper-1" }, groqConnected)).toBe(false);
    expect(
      step2Ready(
        { ...draft, customBaseUrl: "https://relay.example.com/v1", asrModel: "whisper-1", providerKeys: { custom: "local" } },
        groqConnected,
      ),
    ).toBe(true);
    expect(
      step2Ready(
        { ...draft, customBaseUrl: "http://127.0.0.1:8000/v1", asrModel: "whisper-1" },
        groqConnected,
      ),
    ).toBe(true);
  });

  it("omits cleanup fields from persist when cleanup is disabled", () => {
    const patch = persistPatch(
      emptyDraft({
        asrProvider: "custom",
        cleanupProvider: "custom",
        customBaseUrl: "http://127.0.0.1:8000/v1",
        asrModel: "whisper-1",
        cleanupModel: "gpt-4o-mini",
        providerKeys: { custom: "local" },
      }),
      { ...groqConnected, cleanup_enabled: false },
    );
    expect(patch.asr_provider).toBe("custom");
    expect(patch.custom_base_url).toBe("http://127.0.0.1:8000/v1");
    expect(patch.cleanup_provider).toBeUndefined();
    expect(patch.provider_keys).toEqual({ custom: "local" });
  });

  it("masks a secret with bullets and the last five characters", () => {
    expect(maskSecret("gsk_abcdefghij")).toBe("••••fghij");
    expect(maskSecret("only")).toBe("••••only");
    expect(maskSecret("   ")).toBe("");
  });

  it("treats OnDevice ready as a disk flag, not a loopback key", () => {
    const draft = emptyDraft({
      asrProvider: "on_device",
      asrModel: "sensevoice-small",
    });
    const settings = {
      ...groqConnected,
      api_key_configured: false,
      provider_keys: {},
      asr_provider: "on_device" as const,
      asr_model: "sensevoice-small",
      cleanup_enabled: false,
    };
    expect(step2Ready(draft, settings)).toBe(false);
    expect(step2Ready(draft, settings, false)).toBe(false);
    expect(step2Ready(draft, settings, true)).toBe(true);
    expect(isEngineConnected(settings)).toBe(false);
    expect(isEngineConnected(settings, true)).toBe(true);
  });

  it("shows Groq hostname without exposing a custom URL", () => {
    expect(hostnameOf("", "groq")).toBe("api.groq.com");
    expect(hostnameOf("https://api.openai.com/v1", "custom")).toBe("api.openai.com");
  });
});
