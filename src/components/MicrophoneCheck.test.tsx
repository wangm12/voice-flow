import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MicrophoneCheck, type MicrophoneStatus } from "./MicrophoneCheck";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
let handler: (event: { payload: MicrophoneStatus }) => void;
const unlisten = vi.fn();
const snapshot = (session_id: string, patch: Partial<MicrophoneStatus> = {}): MicrophoneStatus => ({
  session_id, device_name: "USB microphone", state: "running", elapsed_secs: 0,
  input_gain: 1.5, level: 0, peak: 0, received_frames: false,
  signal_detected: false, clipping_detected: false, error: null, ...patch,
});
function sessionId() {
  return (invokeMock.mock.calls.find(([command]) => command === "start_microphone_check")?.[1] as { sessionId: string }).sessionId;
}
beforeEach(() => {
  invokeMock.mockReset(); listenMock.mockReset(); unlisten.mockReset();
  listenMock.mockImplementation(async (_event, callback) => { handler = callback as typeof handler; return unlisten; });
  invokeMock.mockImplementation(async (command, args) => command === "start_microphone_check" ? snapshot((args as { sessionId: string }).sessionId) : undefined);
});
afterEach(cleanup);

describe("MicrophoneCheck", () => {
  it("opens only on request and shows the actual selected device and input metrics", async () => {
    render(<MicrophoneCheck />);
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await screen.findByText(/USB microphone/);
    act(() => handler({ payload: snapshot(sessionId(), { received_frames: true, signal_detected: true, level: 0.4, elapsed_secs: 3 }) }));
    expect(screen.getByRole("meter", { name: "麦克风输入音量" })).toHaveAttribute("value", "0.4");
    expect(screen.getByText(/已收到输入信号/)).toBeInTheDocument();
    expect(screen.getByText(/1.5× · 3\/30/)).toBeInTheDocument();
    expect(invokeMock.mock.calls.every(([command]) => command === "start_microphone_check")).toBe(true);
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

  it("cancels opening when settings change and ignores the late start result", async () => {
    let resolve!: (status: MicrophoneStatus) => void;
    invokeMock.mockImplementation(async (command) => command === "start_microphone_check" ? new Promise<MicrophoneStatus>((done) => { resolve = done; }) : undefined);
    const view = render(<MicrophoneCheck configurationKey="USB|1" />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("start_microphone_check", expect.anything()));
    expect(screen.getByRole("button", { name: "停止测试" })).toHaveAttribute("aria-busy", "true");
    const id = sessionId();
    view.rerender(<MicrophoneCheck configurationKey="Built-in|2" />);
    expect(invokeMock).toHaveBeenCalledWith("stop_microphone_check", { sessionId: id });
    await act(async () => resolve(snapshot(id)));
    expect(screen.queryByText(/USB microphone/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toHaveAttribute("aria-busy", "false");
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
    expect(unlisten).toHaveBeenCalledOnce();
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

  it.each([
    ["microphone_check_busy", "请先结束当前听写"],
    ["microphone_check_permission", "请先在权限设置中"],
  ])("offers a recoverable explanation for %s", async (failure, message) => {
    invokeMock.mockRejectedValueOnce(new Error(failure));
    render(<MicrophoneCheck />);
    fireEvent.click(screen.getByRole("button", { name: "开始麦克风测试" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "开始麦克风测试" })).toBeEnabled();
  });
});
