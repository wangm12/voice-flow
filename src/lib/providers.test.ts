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
    expect(asr).toEqual(["groq", "openai", "deepgram", "siliconflow", "local_whisper", "custom"]);
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
    expect(defaultModel("groq", "llm")).toBe("llama-3.1-8b-instant");
    expect(defaultModel("deepgram", "asr")).toBe("nova-3");
  });

  it("does not label GPT-OSS 120B as a quality upgrade", () => {
    const groq = PROVIDERS.find((provider) => provider.id === "groq");
    const model = groq?.llmModels.find((item) => item.value === "openai/gpt-oss-120b");
    expect(model?.note).toBe("较慢 · 不是质量升级");
  });
});
