import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../../types/settings";
import { RecordingSettings } from "./RecordingSettings";

vi.mock("../HotkeyRecorder", () => ({
  HotkeyRecorder: ({
    onChange,
    captureTarget = "dictation",
  }: {
    onChange: (
      hotkey: string,
      activationMode?: "tap" | "double_tap" | "hybrid",
      options?: { persist?: boolean },
    ) => void;
    captureTarget?: "dictation" | "selected_action";
  }) => (
    <>
    <button
      type="button"
      onClick={() => onChange("Command+Shift+Space", "tap", { persist: true })}
    >
      {`recapture-${captureTarget}`}
    </button>
    <button
      type="button"
      onClick={() => onChange("Fn", "double_tap", { persist: true })}
    >
      {`recapture-modifier-${captureTarget}`}
    </button>
  </>
  ),
}));

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
};

describe("RecordingSettings", () => {
  afterEach(() => {
    cleanup();
  });

  it("keeps hybrid when recapture reports tap for a combo", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);

    fireEvent.click(screen.getByRole("button", { name: "recapture-dictation" }));

    expect(save).toHaveBeenCalledWith(
      { hotkey: "Command+Shift+Space", activation_mode: "hybrid" },
      { persist: true },
    );
  });

  it("forces double_tap when recapture is modifier-only", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);

    fireEvent.click(screen.getByRole("button", { name: "recapture-modifier-dictation" }));

    expect(save).toHaveBeenCalledWith(
      { hotkey: "Fn", activation_mode: "double_tap" },
      { persist: true },
    );
  });

  it("explains that WeChat may steal a Fn-only hotkey", () => {
    const save = vi.fn();
    const { rerender } = render(
      <RecordingSettings settings={{ ...settings, hotkey: "Fn", activation_mode: "double_tap" }} save={save} />,
    );
    expect(screen.getByText("微信 / 微信输入法可能会占用 Fn 键，VoiceFlow 可能收不到这个快捷键。")).toBeTruthy();

    rerender(<RecordingSettings settings={settings} save={save} />);
    expect(screen.queryByText("微信 / 微信输入法可能会占用 Fn 键，VoiceFlow 可能收不到这个快捷键。")).toBeNull();
  });

  it("saves input gain from the recognition control", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={{ ...settings, input_gain: 1 }} save={save} />);
    fireEvent.change(screen.getByRole("spinbutton", { name: "输入增益" }), {
      target: { value: "2" },
    });
    fireEvent.blur(screen.getByRole("spinbutton", { name: "输入增益" }));
    expect(save).toHaveBeenCalledWith({ input_gain: 2 });
  });

  it("warns that gain above 1 is limited to avoid clipping", () => {
    render(<RecordingSettings settings={{ ...settings, input_gain: 2 }} save={vi.fn()} />);
    expect(
      screen.getByText("增益大于 1 时，过大的声音会被压限，避免削波。说话很轻再提高。"),
    ).toBeTruthy();
  });

  it("keeps success-audio off by default and warns when retention is still 7 days", () => {
    const save = vi.fn();
    const { rerender } = render(<RecordingSettings settings={settings} save={save} />);
    const toggle = screen.getByRole("switch", { name: "成功听写也保留音频" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(screen.queryByText("训练建议把音频保留至少 90 天或 1 年。")).toBeNull();

    fireEvent.click(toggle);
    expect(save).toHaveBeenCalledWith({ keep_success_audio: true });

    rerender(<RecordingSettings settings={{ ...settings, keep_success_audio: true }} save={save} />);
    expect(screen.getByText("训练建议把音频保留至少 90 天或 1 年。")).toBeTruthy();

    rerender(
      <RecordingSettings
        settings={{ ...settings, keep_success_audio: true, keep_audio_days: 90 }}
        save={save}
      />,
    );
    expect(screen.queryByText("训练建议把音频保留至少 90 天或 1 年。")).toBeNull();
  });
});
