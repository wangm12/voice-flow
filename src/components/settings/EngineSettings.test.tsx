import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
import type { Settings } from "../../types/settings";
import { EngineSettings } from "./EngineSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const settings: Settings = {
  schema_version: 17,
  api_key_configured: true,
  api_key_hint: "••••yabcd",
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "gpt-4o-mini",
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
  hotkey: "CmdOrControl+Shift+Space",
  activation_mode: "hybrid",
  context_enabled: true,
  browser_access_enabled: false,
  context_mappings: [],
  writing_modes: [{ id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true }],
  snippets: [],
  output_mode: "auto",
  translation_target_language: "en",
  input_device: "",
  asr_provider: "groq",
  cleanup_provider: "openai",
  asr_api_key_configured: false,
  asr_api_key_hint: null,
  provider_keys: {
    groq: { configured: true, hint: "••••yabcd" },
    openai: { configured: true, hint: "••••i-key" },
  },
};

const unused = {
  saveApiKey: vi.fn(),
  removeApiKey: vi.fn(),
  saveAsrApiKey: vi.fn(),
  removeAsrApiKey: vi.fn(),
  removeCleanupApiKey: vi.fn(),
  removeProviderKey: vi.fn(),
};

function renderEngine(overrides: Partial<Settings> = {}, handlers: Partial<Parameters<typeof EngineSettings>[0]> = {}) {
  return render(
    <EngineSettings
      settings={{ ...settings, ...overrides }}
      save={handlers.save ?? vi.fn()}
      commitEngine={handlers.commitEngine ?? vi.fn()}
      {...unused}
      {...handlers}
    />,
  );
}

describe("EngineSettings form", () => {
  afterEach(() => {
    cleanup();
    vi.mocked(invoke).mockReset();
  });

  it("keeps ASR and cleanup dropdowns filtered by capability", () => {
    renderEngine();
    const asr = screen.getByRole("combobox", { name: "转写服务" });
    const cleanup = screen.getByRole("combobox", { name: "整理服务" });
    expect(asr).toHaveValue("groq");
    expect(cleanup).toHaveValue("openai");
    expect(within(asr).queryByRole("option", { name: "DeepSeek" })).not.toBeInTheDocument();
    expect(within(asr).queryByRole("option", { name: "Anthropic" })).not.toBeInTheDocument();
    expect(within(cleanup).queryByRole("option", { name: "Deepgram" })).not.toBeInTheDocument();
    expect(within(cleanup).queryByRole("option", { name: "Local Whisper" })).not.toBeInTheDocument();
    expect(within(asr).getByRole("option", { name: "Deepgram" })).toBeInTheDocument();
    expect(within(cleanup).getByRole("option", { name: "DeepSeek" })).toBeInTheDocument();
  });

  it("shows one key field each for Groq and OpenAI", () => {
    renderEngine();
    expect(screen.getByLabelText("Groq API Key")).toHaveValue("••••yabcd");
    expect(screen.getByLabelText("OpenAI API Key")).toHaveValue("••••i-key");
    expect(screen.queryByLabelText("Deepgram API Key")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "删除本机密钥" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "删除 ASR 密钥" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "删除密钥" })).toHaveLength(2);
  });

  it("keeps model notes in the interface language", () => {
    renderEngine();
    expect(screen.getByRole("option", { name: "Whisper Large v3 Turbo · 默认 · 英文更快，中文较弱" })).toBeInTheDocument();
    cleanup();
    render(
      <I18nProvider initialLanguage="en">
        <EngineSettings settings={settings} save={vi.fn()} commitEngine={vi.fn()} {...unused} />
      </I18nProvider>,
    );
    expect(screen.getByRole("option", { name: "Whisper Large v3 Turbo · Default · Faster English, weaker Chinese" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "GPT-4o mini · Default · Faster" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /默认/ })).not.toBeInTheDocument();
  });

  it("places provider controls under each title and keeps test in its own box", () => {
    renderEngine();
    const asrTitle = screen.getByText("转写", { selector: "p.text-sm" });
    const asrSelect = screen.getByRole("combobox", { name: "转写服务" });
    expect(asrTitle.compareDocumentPosition(asrSelect) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const testButton = screen.getByRole("button", { name: "测试当前配置" });
    expect(screen.getByText("正在使用").closest("section")).not.toContainElement(testButton);
    expect(testButton.parentElement?.className).not.toMatch(/border/);
  });

  it("uses a quiet ready status instead of a green key-valid line", () => {
    renderEngine();
    expect(screen.getAllByText("已就绪")).toHaveLength(2);
    expect(screen.queryByText(/已就绪 ·/)).not.toBeInTheDocument();
    expect(screen.queryByText(/密钥有效/)).not.toBeInTheDocument();
    expect(screen.getAllByText("已就绪")[0].className).toContain("text-success");
    expect(screen.getAllByText("使用中")[0].className).toContain("text-accent");
    expect(screen.getAllByText("未添加")[0].className).toContain("text-tertiary");
  });

  it("saves a known Groq model immediately", () => {
    const save = vi.fn();
    renderEngine({}, { save });
    fireEvent.change(screen.getByRole("combobox", { name: "ASR 模型" }), { target: { value: "whisper-large-v3" } });
    expect(save).toHaveBeenCalledWith({ asr_model: "whisper-large-v3" });
  });

  it("does not persist when the probe fails", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "model", message: "nope" },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({}, { commitEngine });
    fireEvent.click(screen.getByRole("button", { name: "测试当前配置" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("模型名不被这个接口接受"));
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("persists the draft once after a successful probe", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({
      cleanup_provider: "groq",
      cleanup_model: "openai/gpt-oss-20b",
      provider_keys: { groq: { configured: true, hint: "••••yabcd" } },
    }, { commitEngine });
    fireEvent.change(screen.getByRole("combobox", { name: "转写服务" }), { target: { value: "custom" } });
    fireEvent.change(screen.getByLabelText("兼容地址"), { target: { value: "http://127.0.0.1:8000/v1" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 模型" }), { target: { value: "whisper-1" } });
    fireEvent.change(screen.getByLabelText("OpenAI Compatible API Key"), { target: { value: "local-key" } });
    fireEvent.click(screen.getByRole("button", { name: "测试当前配置" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("status")).toHaveTextContent("测试通过，设置已保存");
    expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      provider_keys: { custom: "local-key" },
    }));
  });

  it("warns and disables the primary action when a routed provider has no key", () => {
    renderEngine({
      cleanup_provider: "openai",
      provider_keys: {
        groq: { configured: true, hint: "••••yabcd" },
        openai: { configured: false, hint: null },
      },
    });
    expect(screen.getAllByText("未配置密钥").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "测试当前配置" })).toBeDisabled();
    expect(screen.getByLabelText("OpenAI API Key")).toBeInTheDocument();
  });
});
