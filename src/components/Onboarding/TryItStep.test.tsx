import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { TryItStep } from "./TryItStep";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

describe("TryItStep", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    vi.mocked(invoke).mockResolvedValue(undefined);
    listenMock.mockImplementation((event: string, handler: (event: { payload: unknown }) => void) => {
      handlers.set(event, handler);
      return Promise.resolve(vi.fn());
    });
  });

  afterEach(() => {
    cleanup();
    listenMock.mockReset();
  });

  function renderDictationTrial() {
    return render(
      <TryItStep
        trial="dictation"
        recording={false}
        processing={false}
        hotkeyDisplay="⌘ ⇧ Space"
        selectedActionHotkeyDisplay="⌘ ⇧ /"
        activationMode="tap"
        trialReady
        trialLease={{ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42" }}
        configurationGeneration="ui-current"
      />,
    );
  }

  it("inserts onboarding dictation results into the trial box", async () => {
    renderDictationTrial();
    await waitFor(() => expect(handlers.has("dictation://onboarding-result")).toBe(true));

    act(() => {
      handlers.get("dictation://onboarding-result")!({
        payload: { final_text: "你好世界", trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true },
      });
    });

    expect(screen.getByLabelText("语音输入试用框")).toHaveValue("你好世界");
  });

  it("rejects unidentifiable and superseded results instead of marking native delivery", async () => {
    const onTrialResult = vi.fn();
    render(<TryItStep trial="dictation" recording={false} processing={false} hotkeyDisplay="⌘ ⇧ Space" selectedActionHotkeyDisplay="⌘ ⇧ /" activationMode="tap" trialReady trialLease={{ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42" }} onTrialResult={onTrialResult} />);
    await waitFor(() => expect(handlers.has("dictation://onboarding-result")).toBe(true));
    for (const payload of [{ final_text: "legacy" }, { final_text: "old", trial_id: "22222222-2222-4222-8222-222222222222", config_generation: "42", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true }, { final_text: "stale", trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "41", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true }]) {
      act(() => handlers.get("dictation://onboarding-result")!({ payload }));
    }
    expect(screen.getByLabelText("语音输入试用框")).toHaveValue("");
    expect(onTrialResult).not.toHaveBeenCalled();
    expect(screen.getByText("试用结果缺少当前会话回执，未作为本次成功。请重新试用。")).toBeInTheDocument();
  });

  it("ignores an old listener after leaving the trial", async () => {
    const view = renderDictationTrial();
    await waitFor(() => expect(handlers.has("dictation://onboarding-result")).toBe(true));
    const oldHandler = handlers.get("dictation://onboarding-result")!;
    view.unmount();
    act(() => oldHandler({ payload: { final_text: "late", trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true } }));
    expect(screen.queryByLabelText("语音输入试用框")).not.toBeInTheDocument();
  });
  it.each([
    { name: "same UTF16 range", start: 1, end: 3, replaced: true, focused: true, edited: false },
    { name: "same text/selected content but another emoji range", start: 4, end: 6, replaced: false, focused: true, edited: false },
    { name: "selection collapsed only", start: 3, end: 3, replaced: false, focused: true, edited: false },
    { name: "focus left but range/text retained", start: 1, end: 3, replaced: false, focused: false, edited: false },
    { name: "source edited while pending", start: 1, end: 3, replaced: false, focused: true, edited: true },
  ])("checks the actual DOM selection before applying: $name", async ({ start, end, replaced, focused, edited }) => {
    render(<TryItStep trial="selected_action" recording={false} processing={false} hotkeyDisplay="key" selectedActionHotkeyDisplay="other" activationMode="tap" trialReady trialLease={{ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42" }} />);
    const input = screen.getByLabelText("选中文本操作试用框") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "A🙂B🙂C" } });
    input.focus(); input.setSelectionRange(1, 3); fireEvent.select(input);
    const metadata = { trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true };
    await waitFor(() => expect(handlers.has("selected-action://state")).toBe(true));
    act(() => handlers.get("selected-action://state")!({ payload: { ...metadata, state: "listening" } }));
    if (edited) fireEvent.change(input, { target: { value: "Z🙂B🙂C" } });
    input.setSelectionRange(start, end); fireEvent.select(input);
    if (!focused) input.blur();
    // A duplicate lifecycle event must not rebase the same recording onto drift.
    act(() => handlers.get("selected-action://state")!({ payload: { ...metadata, state: "listening" } }));
    act(() => handlers.get("selected-action://onboarding-result")!({ payload: { ...metadata, final_text: "OK" } }));
    const unchanged = edited ? "Z🙂B🙂C" : "A🙂B🙂C";
    expect(input).toHaveValue(replaced ? "AOKB🙂C" : unchanged);
    if (replaced) {
      expect(input.selectionStart).toBe(3); expect(input.selectionEnd).toBe(3);
    } else {
      expect(screen.getByLabelText("可恢复的试用结果")).toHaveValue("OK");
      expect(input.selectionStart).toBe(start); expect(input.selectionEnd).toBe(end);
      if (!focused) expect(document.activeElement).not.toBe(input);
      // Duplicate result cannot later apply the captured old range.
      input.setSelectionRange(1, 3);
      act(() => handlers.get("selected-action://onboarding-result")!({ payload: { ...metadata, final_text: "DUPLICATE" } }));
      expect(input).toHaveValue(unchanged);
    }
  });

  it.each([false, true])("keeps a later focused selection/current recording after committed result work (new recording=%s)", async (newRecording) => {
    render(<TryItStep trial="selected_action" recording={false} processing={false} hotkeyDisplay="key" selectedActionHotkeyDisplay="other" activationMode="tap" trialReady trialLease={{ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42" }} />);
    const input = screen.getByLabelText("选中文本操作试用框") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "A🙂B🙂C" } });
    input.focus(); input.setSelectionRange(1, 3); fireEvent.select(input);
    await waitFor(() => expect(handlers.has("selected-action://onboarding-result")).toBe(true));
    const metadata = { trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "42", trigger_source: "native_hotkey", trial_current: true };
    const frames: FrameRequestCallback[] = [];
    const animation = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => { frames.push(callback); return frames.length; });
    try {
      act(() => handlers.get("selected-action://state")!({ payload: { ...metadata, recording_generation: "7", state: "listening" } }));
      act(() => handlers.get("selected-action://onboarding-result")!({ payload: { ...metadata, recording_generation: "7", final_text: "OK" } }));
      // Actual DOM + React text commit before the intervening user action,
      // not a fabricated guard result. Ordinary caret success is covered above.
      expect(input).toHaveValue("AOKB🙂C");
      expect(document.activeElement).toBe(input);
      input.setSelectionRange(4, 6); fireEvent.select(input);
      if (newRecording) {
        act(() => handlers.get("selected-action://state")!({ payload: { ...metadata, recording_generation: "8", state: "listening" } }));
      }
      // Drain only callbacks genuinely queued before this boundary. In06 the
      // old result7 caret callback would still overwrite this same focused text.
      const queued = frames.splice(0);
      act(() => { for (const callback of queued) callback(16); });
      expect(input).toHaveValue("AOKB🙂C");
      expect(input.selectionStart).toBe(4); expect(input.selectionEnd).toBe(6);
      expect(document.activeElement).toBe(input);
      if (newRecording) {
        act(() => handlers.get("selected-action://onboarding-result")!({ payload: { ...metadata, recording_generation: "8", final_text: "NEW" } }));
        expect(input).toHaveValue("AOKBNEWC");
        expect(input.selectionStart).toBe(7); expect(input.selectionEnd).toBe(7);
        expect(screen.queryByLabelText("可恢复的试用结果")).not.toBeInTheDocument();
      }
    } finally { animation.mockRestore(); }
  });

});
