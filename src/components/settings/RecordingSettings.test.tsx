import { selectOption } from "../../test/selectOption";
import { invoke } from "@tauri-apps/api/core";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../../types/settings";
import { RecordingSettings } from "./RecordingSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

vi.mock("../HotkeyRecorder", () => ({
  HotkeyRecorder: ({
    onChange,
    captureTarget = "dictation",
    disabled = false,
  }: {
    onChange: (
      hotkey: string,
      options?: { persist?: boolean },
    ) => void;
    disabled?: boolean;
    captureTarget?: "dictation" | "selected_action" | "screen_action" | "verbatim_action" | "translation_action";
  }) => (
    <>
    <button
      type="button"
      disabled={disabled}
      onClick={() => onChange("Command+Shift+Space", { persist: false })}
    >
      {`recapture-${captureTarget}`}
    </button>
    <button
      type="button"
      disabled={disabled}
      onClick={() => onChange("Fn", { persist: false })}
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
  activation_mode: "hold_to_talk",
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
  invokeMock.mockReset();
  });

  it("saves a mode independently and commits page values only after native success", async () => {
    let acknowledge!: () => void;
    invokeMock.mockImplementation(() => new Promise<void>((resolve) => { acknowledge = resolve; }));
    const save = vi.fn();
    render(<RecordingSettings settings={{ ...settings, api_key_configured: false }} save={save} />);
    fireEvent.click(screen.getByRole("radio", { name: "点按切换" }));
    expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", { patch: { activation_mode: "tap" } });
    expect(save).not.toHaveBeenCalled();
    expect(screen.getByRole("radio", { name: "按住说话" })).toBeChecked();
    expect(screen.getByRole("button", { name: "recapture-dictation" })).toBeDisabled();
    acknowledge();
    await waitFor(() => expect(save).toHaveBeenCalledWith({ activation_mode: "tap" }, { persist: false }));
  });

  it("retains the previous mode when native save fails", async () => {
    invokeMock.mockRejectedValueOnce(new Error("write failed"));
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);
    fireEvent.click(screen.getByRole("radio", { name: "点按切换" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("write failed");
    expect(screen.getByRole("radio", { name: "按住说话" })).toBeChecked();
    expect(save).not.toHaveBeenCalled();
  });

  it("blocks binding and mode changes while dictation is busy", () => {
    render(<RecordingSettings settings={settings} save={vi.fn()} dictationBusy />);
    for (const radio of screen.getAllByRole("radio")) expect(radio).toBeDisabled();
    expect(screen.getByRole("button", { name: "recapture-dictation" })).toBeDisabled();
  });

  it("opens keyboard settings only after an explicit Fn action", () => {
    invokeMock.mockResolvedValue(undefined);
    render(<RecordingSettings settings={{ ...settings, hotkey: "Fn" }} save={vi.fn()} />);
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "打开键盘设置" }));
    expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "keyboard" });
  });

  it("sets the translation shortcut and target without changing the global output mode", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);
    fireEvent.click(screen.getByRole("button", { name: /更多快捷键/ }));
    fireEvent.click(screen.getByRole("button", { name: "recapture-translation_action" }));
    expect(save).toHaveBeenCalledWith({ translation_hotkey: "Command+Shift+Space" }, { persist: false });
    selectOption(screen.getByLabelText("快捷翻译目标语言"), "ja");
    expect(save).toHaveBeenCalledWith({ translation_target_language: "ja" });
    expect(save.mock.calls.every(([patch]) => !("output_mode" in patch) && !("activation_mode" in patch))).toBe(true);
  });

  it("saves a separate optional skip cleanup shortcut", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);
    fireEvent.click(screen.getByRole("button", { name: /更多快捷键/ }));
    fireEvent.click(screen.getByRole("button", { name: "recapture-verbatim_action" }));
    expect(save).toHaveBeenCalledWith({ verbatim_hotkey: "Command+Shift+Space" }, { persist: false });
  });

  it("preserves the selected mode when a combination is recaptured", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);

    fireEvent.click(screen.getByRole("button", { name: "recapture-dictation" }));

    expect(save).toHaveBeenCalledWith(
      { hotkey: "Command+Shift+Space" },
      { persist: false },
    );
  });

  it("preserves the selected mode when Fn is selected", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);

    fireEvent.click(screen.getByRole("button", { name: "recapture-modifier-dictation" }));

    expect(save).toHaveBeenCalledWith(
      { hotkey: "Fn" },
      { persist: false },
    );
  });

  it("explains the macOS Fn conflict and offers keyboard settings", () => {
    const save = vi.fn();
    const { rerender } = render(
      <RecordingSettings settings={{ ...settings, hotkey: "Fn", activation_mode: "hold_to_talk" }} save={save} />,
    );
    expect(screen.getByText("Fn / 🌐 可能用于切换输入法、表情或系统听写。若有冲突，请在 macOS 键盘设置中调整；VoiceFlow 不会修改系统设置。")).toBeTruthy();

    rerender(<RecordingSettings settings={settings} save={save} />);
    expect(screen.queryByText("Fn / 🌐 可能用于切换输入法、表情或系统听写。若有冲突，请在 macOS 键盘设置中调整；VoiceFlow 不会修改系统设置。")).toBeNull();
  });

  it("records an off-by-default look-at-screen hotkey", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} />);
    expect(screen.getByText("未设置快捷键，看屏幕不会触发")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /更多快捷键/ }));
    fireEvent.click(screen.getByRole("button", { name: "recapture-screen_action" }));
    expect(save).toHaveBeenCalledWith({ screen_action_hotkey: "Command+Shift+Space" }, { persist: false });
  });

  it("saves input gain from the recognition control", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={{ ...settings, input_gain: 1 }} save={save} />);
    fireEvent.click(screen.getByRole("button", { name: /高级录音设置/ }));
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
