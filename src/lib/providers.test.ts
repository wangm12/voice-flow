import { describe, expect, it } from "vitest";
import {
  defaultModel,
  inferProviderFromHost,
  isKnownModel,
  isLoopbackUrl,
  PROVIDERS,
  providersFor,
} from "./providers";

describe("provider catalog", () => {
  it("filters routing dropdowns by capability", () => {
    const asr = providersFor("asr").map((provider) => provider.id);
    const llm = providersFor("llm").map((provider) => provider.id);
    expect(asr).toEqual([
      "groq",
      "openai",
      "deepgram",
      "siliconflow",
      "fireworks",
      "mistral",
      "soniox",
      "assemblyai",
      "dashscope",
      "local_whisper",
      "on_device",
      "custom",
    ]);
    expect(llm).toEqual(["groq", "openai", "siliconflow", "deepseek", "anthropic", "ollama", "custom"]);
    expect(asr).not.toContain("deepseek");
    expect(asr).not.toContain("anthropic");
    expect(llm).not.toContain("deepgram");
    expect(llm).not.toContain("local_whisper");
  });

  it("hides custom from a side the user did not enable", () => {
    expect(providersFor("asr", false, true).some((provider) => provider.id === "custom")).toBe(false);
    expect(providersFor("llm", true, false).some((provider) => provider.id === "custom")).toBe(false);
  });

  it("infers named providers from saved custom hosts", () => {
    expect(inferProviderFromHost("https://api.openai.com/v1")).toBe("openai");
    expect(inferProviderFromHost("https://api.deepseek.com")).toBe("deepseek");
    expect(inferProviderFromHost("http://127.0.0.1:11434/v1")).toBe("ollama");
    expect(inferProviderFromHost("http://localhost:9000/v1")).toBe("local_whisper");
    expect(inferProviderFromHost("https://relay.example.com/v1")).toBeNull();
    expect(inferProviderFromHost("http://127.0.0.1:8080/v1")).not.toBe("on_device");
    expect(inferProviderFromHost("http://localhost/v1")).not.toBe("on_device");
  });

  it("lists OnDevice as ASR-only with its default MLX model", () => {
    const onDevice = PROVIDERS.find((provider) => provider.id === "on_device");
    expect(onDevice?.capabilities).toEqual(["asr"]);
    expect(onDevice?.allowsEmptyKey).toBe(true);
    expect(onDevice?.hasHttpAsr).toBe(false);
    expect(onDevice?.asrModelField).toBe("select");
    expect(onDevice?.defaultAsrModel).toBe("qwen3-asr-0.6b");
    expect(defaultModel("on_device", "asr")).toBe("qwen3-asr-0.6b");
    expect(providersFor("asr").map((provider) => provider.id)).toContain("on_device");
    expect(providersFor("llm").map((provider) => provider.id)).not.toContain("on_device");
  });

  it("treats loopback hosts as empty-key eligible", () => {
    expect(isLoopbackUrl("http://127.0.0.1:11434/v1")).toBe(true);
    expect(isLoopbackUrl("https://api.openai.com/v1")).toBe(false);
  });

  it("keeps known dropdown models and accepts typed models", () => {
    expect(isKnownModel("groq", "asr", "whisper-large-v3-turbo")).toBe(true);
    expect(isKnownModel("groq", "asr", "whisper-1")).toBe(false);
    expect(isKnownModel("siliconflow", "llm", "Qwen/Qwen2.5-7B-Instruct")).toBe(true);
    expect(defaultModel("openai", "llm")).toBe("gpt-4o-mini");
    expect(defaultModel("groq", "llm")).toBe("openai/gpt-oss-20b");
    expect(defaultModel("deepgram", "asr")).toBe("nova-3");
  });

  it("defaults OpenAI file ASR to gpt-transcribe and keeps whisper-1 as a legacy option", () => {
    const openai = PROVIDERS.find((provider) => provider.id === "openai");
    expect(defaultModel("openai", "asr")).toBe("gpt-transcribe");
    expect(openai?.asrModels.map((item) => item.value)).toEqual([
      "gpt-transcribe",
      "gpt-4o-mini-transcribe",
      "whisper-1",
    ]);
    expect(openai?.asrModels[0]?.note).toBe("默认 · 文件转写");
    expect(openai?.asrModels.find((item) => item.value === "whisper-1")?.note).toBe("旧模型");
  });

  it("lists Handy-style local Whisper filenames without pretending the app downloads them", () => {
    const local = PROVIDERS.find((provider) => provider.id === "local_whisper");
    expect(defaultModel("local_whisper", "asr")).toBe("whisper-large-v3-turbo");
    expect(local?.asrModelField).toBe("text");
    expect(local?.asrModels.map((item) => item.value)).toEqual([
      "whisper-large-v3-turbo",
      "ggml-large-v3-turbo.bin",
      "ggml-small.bin",
      "whisper-medium-q4_1.bin",
      "ggml-large-v3-q5_0.bin",
    ]);
  });

  it("defaults to a non-retired Groq cleanup model and retains retired IDs for saved settings", () => {
    const groq = PROVIDERS.find((provider) => provider.id === "groq");
    expect(defaultModel("groq", "llm")).toBe("openai/gpt-oss-20b");
    for (const id of ["llama-3.1-8b-instant", "llama-3.3-70b-versatile"]) {
      const model = groq?.llmModels.find((item) => item.value === id);
      expect(isKnownModel("groq", "llm", id)).toBe(true);
      expect(model?.retiredForNewSelection).toBe(true);
      expect(model?.note).toContain("2026-08-16");
    }
    expect(groq?.llmModels.find((item) => item.value === "openai/gpt-oss-20b")?.retiredForNewSelection).toBeUndefined();
  });
});
