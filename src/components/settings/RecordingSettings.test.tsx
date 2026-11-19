import { selectOption } from "../../test/selectOption";
import { invoke } from "@tauri-apps/api/core";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GuideAcknowledgement } from "../../lib/onboardingGuides";
import type { Settings } from "../../types/settings";
import { RecordingSettings } from "./RecordingSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

// Independent native contracts must not compete for a one-shot invoke response.
const guideAcknowledgements = (): GuideAcknowledgement[] => [
  { guide_id: "dictionary_learning", current_version: 1, acknowledged_version: null },
  { guide_id: "context_screen", current_version: 1, acknowledged_version: null },
  { guide_id: "selected_translation", current_version: 1, acknowledged_version: null },
  { guide_id: "scene_preferences", current_version: 1, acknowledged_version: null },
];
type NativeFixtureHandlers = {
  updateSettingsPatch?: (args: unknown) => Promise<void>;
  openPrivacySettings?: (args: unknown) => Promise<void>;
};
let nativeHandlers: NativeFixtureHandlers;
let unexpectedNativeCalls: string[];

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
  beforeEach(() => {
    invokeMock.mockReset();
    nativeHandlers = {};
    unexpectedNativeCalls = [];
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_guide_acknowledgements") {
        if (args !== undefined) {
          unexpectedNativeCalls.push(`${command}: unexpected arguments`);
          throw new Error("Guide acknowledgement reads do not take arguments");
        }
        return guideAcknowledgements();
      }
      if (command === "update_settings_patch" && nativeHandlers.updateSettingsPatch) return nativeHandlers.updateSettingsPatch(args);
      if (command === "open_privacy_settings" && nativeHandlers.openPrivacySettings) return nativeHandlers.openPrivacySettings(args);
      unexpectedNativeCalls.push(command);
      throw new Error(`Unexpected native command: ${command}`);
    });
  });

  afterEach(() => {
    cleanup();
    // Fail even if a consumer catches an unexpected native rejection in its UI.
    expect(unexpectedNativeCalls).toEqual([]);
    invokeMock.mockReset();
  });

  it("routes mode updates immediately through shared settings persistence without direct IPC", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={{ ...settings, api_key_configured: false }} save={save} />);
    expect(screen.getByText(/按住 .*Space 说话，松开后结束并转成文字/)).toBeVisible();
    expect(screen.getByText("Esc 取消。")).toBeVisible();
    selectOption(screen.getByRole("combobox", { name: "录音方式" }), "tap");
    expect(save).toHaveBeenCalledWith({ activation_mode: "tap" }, { immediate: true });
    selectOption(screen.getByRole("combobox", { name: "录音方式" }), "hybrid_hold_lock");
    expect(save).toHaveBeenCalledWith({ activation_mode: "hybrid_hold_lock" }, { immediate: true });
    expect(invokeMock.mock.calls.filter(([command]) => command === "update_settings_patch")).toEqual([]);
    expect(screen.getByRole("button", { name: "recapture-dictation" })).toBeEnabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("renders inline mode error when shared persistence fails", () => {
    const save = vi.fn();
    render(<RecordingSettings settings={settings} save={save} saveFailure={{ fields: ["activation_mode"], message: "write failed" }} />);
    expect(screen.getByRole("alert")).toHaveTextContent("write failed");
    expect(screen.getByRole("combobox", { name: "录音方式" })).toHaveTextContent("按住说话");
    expect(screen.getByRole("button", { name: "recapture-dictation" })).toBeEnabled();
  });

  it("blocks binding and mode changes while dictation is busy", () => {
    render(<RecordingSettings settings={settings} save={vi.fn()} dictationBusy />);
    expect(screen.getByRole("combobox", { name: "录音方式" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "recapture-dictation" })).toBeDisabled();
  });

  it("opens keyboard and Input Monitoring settings only after explicit Fn actions", () => {
    nativeHandlers.openPrivacySettings = async (args) => {
      expect(args).toEqual(expect.objectContaining({ pane: expect.any(String) }));
      expect(["keyboard", "input_monitoring"]).toContain((args as { pane: string }).pane);
    };
    render(<RecordingSettings settings={{ ...settings, hotkey: "Fn" }} save={vi.fn()} />);
    expect(invokeMock).toHaveBeenCalledWith("get_guide_acknowledgements");
    // No permission, capture, selection or provider operation is allowed on mount.
    expect(invokeMock.mock.calls.filter(([command]) => command !== "get_guide_acknowledgements")).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "打开键盘设置" }));
    expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "keyboard" });
    fireEvent.click(screen.getByRole("button", { name: "打开输入监控设置" }));
    expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "input_monitoring" });
    expect(invokeMock.mock.calls.filter(([command]) => command === "open_privacy_settings")).toHaveLength(2);
  });

  it("shows the Carbon release fallback as a nonblocking notice and opens Input Monitoring only on click", () => {
    nativeHandlers.openPrivacySettings = async (args) => { expect(args).toEqual({ pane: "input_monitoring" }); };
    render(<RecordingSettings settings={{ ...settings, hotkey_notice: "combo_release_uses_carbon_fallback" }} save={vi.fn()} />);
    expect(screen.getByText(/组合快捷键仍可使用/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "open_privacy_settings")).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "打开输入监控设置" }));
    expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "input_monitoring" });
  });

  it("shows an unbound shortcut badge outside the collapsed More shortcuts section", () => {
    render(<RecordingSettings settings={{ ...settings, selected_actions_enabled: true, selected_action_hotkey: "" }} save={vi.fn()} />);
    expect(screen.getByText("有快捷键尚未绑定")).toBeVisible();
    expect(screen.getByRole("button", { name: /更多快捷键/ })).toHaveAttribute("aria-expanded", "false");
  });

  it("routes the translation target to Writing while keeping shortcut binding here", () => {
    const save = vi.fn();
    const onOpenMappings = vi.fn();
    render(<RecordingSettings settings={settings} save={save} onOpenMappings={onOpenMappings} />);
    fireEvent.click(screen.getByRole("button", { name: /更多快捷键/ }));
    fireEvent.click(screen.getByRole("button", { name: "recapture-translation_action" }));
    expect(save).toHaveBeenCalledWith({ translation_hotkey: "Command+Shift+Space" }, { persist: false });
    expect(screen.queryByRole("combobox", { name: "快捷翻译目标语言" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更改" }));
    expect(onOpenMappings).toHaveBeenCalledOnce();
    expect(save.mock.calls.every(([patch]) => !("translation_target_language" in patch) && !("output_mode" in patch) && !("activation_mode" in patch))).toBe(true);
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

  it("caps preferred spoken languages at 4, supports clearing, and saves immediately on hint-capable models", () => {
    const save = vi.fn();
    const { rerender } = render(
      <RecordingSettings
        settings={{ ...settings, asr_provider: "soniox", asr_model: "stt-rt-v5", preferred_languages: ["zh", "en", "ja", "ko"] }}
        save={save}
      />,
    );
    expect(screen.getByRole("group", { name: "常用口述语言（可选）" })).toBeInTheDocument();
    const spanish = screen.getByRole("button", { name: "Español" });
    expect(spanish).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "清空（无偏向自动检测）" }));
    expect(save).toHaveBeenCalledWith({ preferred_languages: [] }, { immediate: true });

    rerender(
      <RecordingSettings
        settings={{ ...settings, asr_provider: "soniox", asr_model: "stt-rt-v5", preferred_languages: [] }}
        save={save}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "日本語" }));
    expect(save).toHaveBeenCalledWith({ preferred_languages: ["ja"] }, { immediate: true });
  });

  it("hides preferred language chips on non-hint models and warns on unsupported language selection", () => {
    render(
      <RecordingSettings
        settings={{
          ...settings,
          asr_provider: "on_device",
          asr_model: "qwen3-asr-1.7b",
          language: "ar",
        }}
        save={vi.fn()}
      />,
    );
    expect(screen.queryByRole("group", { name: "常用口述语言（可选）" })).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "当前所选识别语言不受当前转写模型支持，请改选支持列表中的语言。",
    );
    expect(screen.getByRole("combobox", { name: "识别语言" })).toHaveTextContent("当前模型不支持");

    render(
      <RecordingSettings
        settings={{
          ...settings,
          asr_provider: "deepgram",
          asr_model: "nova-3",
          language: "auto",
          preferred_languages: [],
        }}
        save={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: "العربية" })).toBeNull();
    expect(screen.queryByRole("button", { name: "粵語" })).toBeNull();
  });
});
