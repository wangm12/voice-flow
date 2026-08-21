import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../../types/settings";
import { EngineSettings } from "./EngineSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const settings: Settings = {
  schema_version: 4,
  api_key_configured: true,
  api_key_hint: "gsk_…abcd",
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
  asr_api_key_configured: false,
  asr_api_key_hint: null,
};

describe("EngineSettings ASR BYOK", () => {
  afterEach(() => {
    cleanup();
  });

  it("saves the compatible ASR base URL, including empty Groq default", () => {
    const save = vi.fn();
    const { rerender } = render(
      <EngineSettings
        settings={settings}
        save={save}
        saveApiKey={vi.fn()}
        removeApiKey={vi.fn()}
        saveAsrApiKey={vi.fn()}
        removeAsrApiKey={vi.fn()}
      />,
    );

    const url = screen.getByRole("textbox", { name: "ASR 兼容地址" });
    fireEvent.change(url, { target: { value: "http://127.0.0.1:8000/v1" } });
    expect(save).toHaveBeenCalledWith({ asr_base_url: "http://127.0.0.1:8000/v1" });

    rerender(
      <EngineSettings
        settings={{ ...settings, asr_base_url: "http://127.0.0.1:8000/v1" }}
        save={save}
        saveApiKey={vi.fn()}
        removeApiKey={vi.fn()}
        saveAsrApiKey={vi.fn()}
        removeAsrApiKey={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 兼容地址" }), {
      target: { value: "" },
    });
    expect(save).toHaveBeenCalledWith({ asr_base_url: "" });
  });

  it("mentions api.groq.com when confirming ASR key removal", () => {
    render(
      <EngineSettings
        settings={{ ...settings, asr_api_key_configured: true }}
        save={vi.fn()}
        saveApiKey={vi.fn()}
        removeApiKey={vi.fn()}
        saveAsrApiKey={vi.fn()}
        removeAsrApiKey={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "删除 ASR 密钥" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("api.groq.com");
  });

  it("saves an optional ASR key without requiring Groq validation", async () => {
    const saveAsrApiKey = vi.fn().mockResolvedValue(undefined);
    render(
      <EngineSettings
        settings={settings}
        save={vi.fn()}
        saveApiKey={vi.fn()}
        removeApiKey={vi.fn()}
        saveAsrApiKey={saveAsrApiKey}
        removeAsrApiKey={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("ASR API Key（可选）"), {
      target: { value: "sk-compat" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存 ASR 密钥" }));
    expect(saveAsrApiKey).toHaveBeenCalledWith("sk-compat");
  });
});
