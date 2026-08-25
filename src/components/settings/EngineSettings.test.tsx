import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../../types/settings";
import { EngineSettings } from "./EngineSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const settings: Settings = {
  schema_version: 16,
  api_key_configured: true,
  api_key_hint: "••••yabcd",
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "openai/gpt-oss-20b",
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
  asr_base_url: "",
  asr_provider: "groq",
  asr_api_key_configured: false,
  asr_api_key_hint: null,
};

const unused = {
  saveApiKey: vi.fn(),
  removeApiKey: vi.fn(),
  saveAsrApiKey: vi.fn(),
  removeAsrApiKey: vi.fn(),
  removeCleanupApiKey: vi.fn(),
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

  it("shows providers and Groq fields on one page", () => {
    const save = vi.fn();
    renderEngine({}, { save });
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("groq");
    expect(screen.getByRole("combobox", { name: "整理服务" })).toHaveValue("groq");
    expect(screen.queryByRole("button", { name: "下一步" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "上一步" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重新配置" })).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "ASR 兼容地址" })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3-turbo");
    expect(screen.queryByRole("option", { name: /whisper-1/i })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "AI 文字整理模型" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始测试" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "测试整条链路" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Groq API Key（访问密钥）")).toHaveValue("••••yabcd");
    fireEvent.change(screen.getByRole("combobox", { name: "ASR 模型" }), { target: { value: "whisper-large-v3" } });
    expect(save).toHaveBeenCalledWith({ asr_model: "whisper-large-v3" });
  });

  it("reveals custom fields when the user picks another provider", () => {
    renderEngine({ api_key_configured: false });
    expect(screen.queryByRole("textbox", { name: "ASR 兼容地址" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "转写服务" }), { target: { value: "custom" } });
    expect(screen.getByRole("textbox", { name: "ASR 兼容地址" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "ASR 模型名" })).toBeInTheDocument();
    expect(screen.getByLabelText("ASR API Key")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始测试" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 兼容地址" }), { target: { value: "http://127.0.0.1:8000/v1" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 模型名" }), { target: { value: "whisper-1" } });
    fireEvent.change(screen.getByLabelText("ASR API Key"), { target: { value: "local-key" } });
    fireEvent.change(screen.getByLabelText("Groq API Key（访问密钥）"), { target: { value: "gsk_test" } });
    expect(screen.getByRole("button", { name: "开始测试" })).toBeEnabled();
  });

  it("clears a custom model when switching back to Groq", () => {
    renderEngine({ api_key_configured: false });
    fireEvent.change(screen.getByRole("combobox", { name: "转写服务" }), { target: { value: "custom" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 模型名" }), { target: { value: "whisper-1" } });
    fireEvent.change(screen.getByRole("combobox", { name: "转写服务" }), { target: { value: "groq" } });
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3-turbo");
    expect(screen.queryByRole("textbox", { name: "ASR 模型名" })).not.toBeInTheDocument();
  });

  it("shows a friendly message for unclassified provider errors", async () => {
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "provider", message: "Groq error: HTTP status 400 Bad Request" },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({ api_key_configured: false });
    fireEvent.change(screen.getByLabelText("Groq API Key（访问密钥）"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "开始测试" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("服务返回错误"));
    expect(screen.getByRole("alert")).not.toHaveTextContent("HTTP status 400");
  });

  it("does not persist when the probe fails", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "model", message: "nope" },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({ api_key_configured: false }, { commitEngine });
    fireEvent.change(screen.getByLabelText("Groq API Key（访问密钥）"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "开始测试" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("模型名不被这个接口接受"));
    expect(commitEngine).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "开始测试" })).toBeInTheDocument();
  });

  it("persists the draft once after a successful probe", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({ api_key_configured: false }, { commitEngine });
    fireEvent.change(screen.getByRole("combobox", { name: "转写服务" }), { target: { value: "custom" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 兼容地址" }), { target: { value: "http://127.0.0.1:8000/v1" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 模型名" }), { target: { value: "whisper-1" } });
    fireEvent.change(screen.getByLabelText("ASR API Key"), { target: { value: "local-key" } });
    fireEvent.change(screen.getByLabelText("Groq API Key（访问密钥）"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "开始测试" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("status")).toHaveTextContent("测试通过，设置已保存");
    expect(screen.getByLabelText("Groq API Key（访问密钥）")).toHaveValue("••••_test");
    expect(screen.getByLabelText("ASR API Key")).toHaveValue("••••l-key");
    expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: "custom",
      asr_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      asr_api_key: "local-key",
      api_key: "gsk_test",
      cleanup_provider: "groq",
    }));
  });

  it("mentions api.groq.com when confirming ASR key removal", () => {
    renderEngine({ asr_api_key_configured: true });
    fireEvent.click(screen.getByRole("button", { name: "删除 ASR 密钥" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("api.groq.com");
  });
});
