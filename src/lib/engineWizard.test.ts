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
} from "./engineWizard";
import type { Settings } from "../types/settings";

const groqConnected: Settings = {
  schema_version: 16,
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
};

describe("engine wizard rules", () => {
  it("treats an onboarded Groq key as connected", () => {
    expect(isEngineConnected(groqConnected)).toBe(true);
    expect(isEngineConnected({ ...groqConnected, api_key_configured: false })).toBe(false);
  });

  it("allows ASR-only when cleanup is disabled", () => {
    expect(
      isEngineConnected({
        ...groqConnected,
        api_key_configured: false,
        asr_provider: "custom",
        asr_base_url: "http://127.0.0.1:8000/v1",
        asr_api_key_configured: true,
        cleanup_enabled: false,
      }),
    ).toBe(true);
  });

  it("requires a cleanup key when cleanup is custom and enabled", () => {
    expect(
      isEngineConnected({
        ...groqConnected,
        cleanup_provider: "custom",
        cleanup_base_url: "https://api.openai.com/v1",
        cleanup_api_key_configured: false,
      }),
    ).toBe(false);
  });

  it("clears custom URL and model when switching a side back to Groq", () => {
    const draft = switchProvider(
      {
        asrProvider: "custom",
        cleanupProvider: "custom",
        asrBaseUrl: "http://127.0.0.1:8000/v1",
        cleanupBaseUrl: "https://api.openai.com/v1",
        asrModel: "whisper-1",
        cleanupModel: "gpt-4o-mini",
        apiKey: "",
        asrApiKey: "",
        cleanupApiKey: "",
      },
      "asr",
      "groq",
    );
    expect(draft.asrProvider).toBe("groq");
    expect(draft.asrBaseUrl).toBe("");
    expect(draft.asrModel).toBe(DEFAULT_ASR_MODEL);
    expect(draft.cleanupModel).toBe("gpt-4o-mini");
  });

  it("does not copy Groq model ids when switching to custom", () => {
    const draft = switchProvider(draftFromSettings(groqConnected), "cleanup", "custom");
    expect(draft.cleanupProvider).toBe("custom");
    expect(draft.cleanupBaseUrl).toBe("");
    expect(draft.cleanupModel).toBe("");
  });

  it("requires URL, model, and key for custom step 2", () => {
    const draft = {
      asrProvider: "custom" as const,
      cleanupProvider: "groq" as const,
      asrBaseUrl: "",
      cleanupBaseUrl: "",
      asrModel: "",
      cleanupModel: DEFAULT_CLEANUP_MODEL,
      apiKey: "gsk_test",
      asrApiKey: "",
      cleanupApiKey: "",
    };
    expect(step2Ready(draft, groqConnected)).toBe(false);
    expect(step2Ready({ ...draft, asrBaseUrl: "http://127.0.0.1:8000/v1", asrModel: "whisper-1" }, groqConnected)).toBe(false);
    expect(
      step2Ready(
        { ...draft, asrBaseUrl: "http://127.0.0.1:8000/v1", asrModel: "whisper-1", asrApiKey: "local" },
        groqConnected,
      ),
    ).toBe(true);
  });

  it("omits cleanup fields from persist when cleanup is disabled", () => {
    const patch = persistPatch(
      {
        asrProvider: "custom",
        cleanupProvider: "custom",
        asrBaseUrl: "http://127.0.0.1:8000/v1",
        cleanupBaseUrl: "https://api.openai.com/v1",
        asrModel: "whisper-1",
        cleanupModel: "gpt-4o-mini",
        apiKey: "",
        asrApiKey: "local",
        cleanupApiKey: "sk",
      },
      { ...groqConnected, cleanup_enabled: false },
    );
    expect(patch.asr_provider).toBe("custom");
    expect(patch.asr_base_url).toBe("http://127.0.0.1:8000/v1");
    expect(patch.cleanup_provider).toBeUndefined();
    expect(patch.cleanup_api_key).toBeUndefined();
  });

  it("masks a secret with bullets and the last five characters", () => {
    expect(maskSecret("gsk_abcdefghij")).toBe("••••fghij");
    expect(maskSecret("only")).toBe("••••only");
    expect(maskSecret("   ")).toBe("");
  });

  it("shows Groq hostname without exposing a custom URL", () => {
    expect(hostnameOf("", "groq")).toBe("api.groq.com");
    expect(hostnameOf("https://api.openai.com/v1", "custom")).toBe("api.openai.com");
  });
});
