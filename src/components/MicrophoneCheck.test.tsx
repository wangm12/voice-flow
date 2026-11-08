import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { I18nProvider } from "../lib/i18n";
import { MicrophoneCheck, type MicrophoneStatus } from "./MicrophoneCheck";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
let handler: (event: { payload: MicrophoneStatus }) => void;
const unlisten = vi.fn();
const stopReceipt = (session_id: string) => ({ session_id, state: "stopped" });
const snapshot = (session_id: string, patch: Partial<MicrophoneStatus> = {}): MicrophoneStatus => ({
  session_id, device_name: "USB microphone", state: "running", elapsed_secs: 0,
  input_gain: 1.5, level: 0, peak: 0, received_frames: false,
  frame_fresh: false, signal_detected: false, clipping_detected: false, error: null,
  stream_released: patch.state !== undefined && patch.state !== "running", ...patch,
});
function sessionId() {
  return (invokeMock.mock.calls.find(([command]) => command === "start_microphone_check")?.[1] as { sessionId: string }).sessionId;
}
beforeEach(() => {
  invokeMock.mockReset(); listenMock.mockReset(); unlisten.mockReset();
  listenMock.mockImplementation(async (_event, callback) => { handler = callback as typeof handler; return unlisten; });
  invokeMock.mockImplementation(async (command, args) => command === "start_microphone_check" ? snapshot((args as { sessionId: string }).sessionId) : stopReceipt((args as { sessionId: string }).sessionId));
});
afterEach(async () => { cleanup(); await act(async () => {}); });

describe("MicrophoneCheck", () => {
  it("opens only on request and shows the actual selected device and input metrics", async () => {
    render(<MicrophoneCheck />);
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    act(() => handler({ payload: snapshot(sessionId(), { received_frames: true, frame_fresh: true, signal_detected: true, level: 0.4, elapsed_secs: 3 }) }));
    expect(screen.getByRole("meter", { name: "麦克风输入音量" })).toHaveAttribute("value", "0.4");
    expect(screen.getByText(/已收到输入信号/)).toBeInTheDocument();
    expect(screen.getByText(/1.5× · 3\/30/)).toBeInTheDocument();
    expect(invokeMock.mock.calls.every(([command]) => command === "start_microphone_check")).toBe(true);
  });

  it("expires the live meter when frames stall but keeps cumulative signal evidence", async () => {
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    act(() => handler({ payload: snapshot(id, {
      received_frames: true,
      frame_fresh: true,
      signal_detected: true,
      level: 0.4,
    }) }));
    expect(screen.getByRole("meter", { name: "麦克风输入音量" })).toHaveAttribute("value", "0.4");
    expect(screen.getByText(/已收到输入信号/)).toBeInTheDocument();

    act(() => handler({ payload: snapshot(id, {
      received_frames: true,
      frame_fresh: false,
      signal_detected: true,
      level: 0,
    }) }));
    expect(screen.getByRole("meter", { name: "麦克风输入音量" })).toHaveAttribute("value", "0");
    expect(screen.getByText(/暂未收到新的音频帧/)).toBeInTheDocument();
  });

  it("keeps clipping feedback after completion and releases its subscription", async () => {
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    act(() => handler({ payload: snapshot(sessionId(), { state: "completed", elapsed_secs: 30, received_frames: true, signal_detected: true, clipping_detected: true, level: 0.8 }) }));
    expect(screen.getByText(/输入峰值过高/)).toBeInTheDocument();
    expect(screen.getByRole("meter")).toHaveAttribute("value", "0");
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeEnabled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("cancels opening when settings change and retires a confirmed lease before reentry", async () => {
    let resolve!: (status: MicrophoneStatus) => void;
    invokeMock.mockImplementationOnce(async (command, args) => command === "start_microphone_check"
      ? new Promise<MicrophoneStatus>((done) => { resolve = done; })
      : stopReceipt((args as { sessionId: string }).sessionId));
    const view = render(<MicrophoneCheck configurationKey="USB|1" />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("start_microphone_check", expect.anything()));
    expect(screen.getByRole("button", { name: "停止测试" })).toHaveAttribute("aria-busy", "true");
    const id = sessionId();
    view.rerender(<MicrophoneCheck configurationKey="Built-in|2" />);
    expect(invokeMock).toHaveBeenCalledWith("stop_microphone_check", { sessionId: id });
    // Let the late start reply race the already-confirmed stop before the
    // original release continuation clears its component-owned reference.
    await act(async () => resolve(snapshot(id)));
    expect(screen.queryByText(/USB microphone/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toHaveAttribute("aria-busy", "false");

    view.unmount();
    render(<MicrophoneCheck />);
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "start_microphone_check")).toHaveLength(2));
  });

  it("does not start native capture when microphone settings become pending during listener setup", async () => {
    let finishListen!: (value: typeof unlisten) => void;
    listenMock.mockImplementationOnce(async (_event, callback) => {
      handler = callback as typeof handler;
      return new Promise((resolve) => { finishListen = resolve; });
    });
    const view = render(<MicrophoneCheck configurationKey="USB|1" />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await waitFor(() => expect(listenMock).toHaveBeenCalledOnce());

    view.rerender(<MicrophoneCheck configurationKey="USB|1" disabled />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("stop_microphone_check", expect.anything()));
    await act(async () => finishListen(unlisten));

    expect(invokeMock).not.toHaveBeenCalledWith("start_microphone_check", expect.anything());
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeDisabled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("stops on unmount and does not accept a result from another session", async () => {
    const view = render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    act(() => handler({ payload: snapshot("old-session", { device_name: "Wrong microphone", state: "completed" }) }));
    expect(screen.queryByText(/Wrong microphone/)).not.toBeInTheDocument();
    const id = sessionId();
    view.unmount();
    expect(invokeMock).toHaveBeenCalledWith("stop_microphone_check", { sessionId: id });
    await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
  });

  it("ends automatically when dictation takes the microphone and ignores late snapshots", async () => {
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    act(() => handler({ payload: snapshot(id, { state: "interrupted" }) }));
    act(() => handler({ payload: snapshot(id, { device_name: "Late result" }) }));
    expect(screen.getByText(/正式听写已开始/)).toBeInTheDocument();
    expect(screen.queryByText(/Late result/)).not.toBeInTheDocument();
  });

  it("retains uncertain stop ownership and retries closing before another check", async () => {
    let failStop = true;
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "start_microphone_check") return snapshot((args as { sessionId: string }).sessionId);
      if (command === "stop_microphone_check" && failStop) { failStop = false; throw new Error("stop_transport_failed"); }
      return stopReceipt((args as { sessionId: string }).sessionId);
    });
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    fireEvent.click(screen.getByRole("button", { name: "停止测试" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("关闭尚未确认");
    expect(screen.queryByText("测试已结束，麦克风自检已关闭。")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "停止测试" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeEnabled());
    const stoppedIds = invokeMock.mock.calls.filter(([command]) => command === "stop_microphone_check").map(([, args]) => (args as { sessionId: string }).sessionId);
    expect(stoppedIds).toEqual([id, id]);
  });

  it.each([
    ["microphone_check_busy", "请先结束当前听写"],
    ["microphone_check_permission", "请先在权限设置中"],
  ])("offers a recoverable explanation for %s", async (failure, message) => {
    invokeMock.mockRejectedValueOnce(new Error(failure));
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    await waitFor(() => expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeEnabled());
  });
  it.each([
    ["zh", "合上显示屏时内置麦克风不可用，请打开显示屏或选择外接麦克风。"],
    ["en", "The built-in microphone is unavailable while the display is closed. Open the display or choose an external microphone."],
  ] as const)("shows the closed-lid recovery guidance in %s", async (language, message) => {
    invokeMock.mockRejectedValueOnce(new Error(
      "microphone device error: selected input device is unavailable while the lid is closed; open the display or choose an external microphone",
    ));
    render(
      <I18nProvider initialLanguage={language}>
        <MicrophoneCheck />
      </I18nProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: language === "en" ? "Test microphone" : "开始麦克风测试" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
  });
  it("keeps its lease and listener until a matching audio-thread stop receipt", async () => {
    let acknowledge!: (value: unknown) => void;
    invokeMock.mockImplementation(async (command, args) => command === "start_microphone_check"
      ? snapshot((args as { sessionId: string }).sessionId)
      : new Promise((resolve) => { acknowledge = resolve; }));
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    fireEvent.click(screen.getByRole("button", { name: "停止测试" }));
    expect(unlisten).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "停止测试" })).toBeDisabled();
    expect(screen.queryByText("测试已结束，麦克风自检已关闭。")).not.toBeInTheDocument();
    act(() => handler({ payload: snapshot(id, { level: 0.8 }) }));
    expect(screen.getByRole("meter")).toHaveAttribute("value", "0");
    await act(async () => acknowledge(stopReceipt(id)));
    expect(unlisten).toHaveBeenCalledOnce();
    expect(screen.getByText("测试已结束，麦克风自检已关闭。")).toBeInTheDocument();
  });

  it.each(["void", "foreign_session", "nonterminal"] as const)("rejects %s IPC success without a release fact and retains retry ownership", async (invalid) => {
    let attempts = 0;
    invokeMock.mockImplementation(async (command, args) => {
      const id = (args as { sessionId: string }).sessionId;
      if (command === "start_microphone_check") return snapshot(id);
      if (++attempts !== 1) return stopReceipt(id);
      return invalid === "void" ? undefined : invalid === "foreign_session"
        ? stopReceipt("another-owned-session") : { session_id: id, state: "running" };
    });
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    fireEvent.click(screen.getByRole("button", { name: "停止测试" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("关闭尚未确认");
    expect(unlisten).not.toHaveBeenCalled();
    expect(screen.queryByText("测试已结束，麦克风自检已关闭。")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "停止测试" }));
    await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
    expect(invokeMock.mock.calls.filter(([c]) => c === "stop_microphone_check").map(([, a]) => (a as { sessionId: string }).sessionId)).toEqual([id, id]);
  });

  it("does not confuse a pre-drop terminal label with release; a real terminal wins a later transport error", async () => {
    let reject!: (reason: Error) => void;
    invokeMock.mockImplementation(async (command, args) => command === "start_microphone_check"
      ? snapshot((args as { sessionId: string }).sessionId)
      : new Promise((_resolve, fail) => { reject = fail; }));
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    const id = sessionId();
    act(() => handler({ payload: snapshot(id, { state: "stopped", stream_released: false }) }));
    expect(unlisten).not.toHaveBeenCalled();
    expect(screen.queryByText("测试已结束，麦克风自检已关闭。")).not.toBeInTheDocument();
    act(() => handler({ payload: snapshot(id, { state: "stopped", stream_released: true }) }));
    await act(async () => reject(new Error("reply_transport_lost_after_terminal")));
    expect(unlisten).toHaveBeenCalledOnce();
    expect(screen.getByText("测试已结束，麦克风自检已关闭。")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

});
