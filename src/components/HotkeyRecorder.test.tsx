import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { HotkeyRecorder } from "./HotkeyRecorder";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("HotkeyRecorder", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    invokeMock.mockReset();
  });

  it("starts a capture session from the button", async () => {
    invokeMock.mockResolvedValue(undefined);
    render(<HotkeyRecorder value="" onChange={vi.fn()} />);

    fireEvent.pointerDown(
      screen.getByRole("button", { name: /点击后按快捷键/ }),
      { button: 0 },
    );

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_hotkeys_suspended", {
        suspended: true,
        capturedHotkey: null,
        capturedActivationMode: null,
        captureTarget: "dictation",
      });
    });
    expect(screen.getByText("组合键按一下，或双击功能键（Esc 取消）")).toBeInTheDocument();
  });

  it("cancels an active capture when Escape is pressed", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue(undefined);
    const onChange = vi.fn();
    render(<HotkeyRecorder value="" onChange={onChange} />);

    fireEvent.pointerDown(
      screen.getByRole("button", { name: /点击后按快捷键/ }),
      { button: 0 },
    );
    await act(async () => {
      await Promise.resolve();
    });
    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });

    await act(async () => {
      vi.advanceTimersByTime(400);
      await Promise.resolve();
    });

    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", {
      suspended: false,
      capturedHotkey: null,
      capturedActivationMode: null,
      captureTarget: "dictation",
    });
    expect(onChange).not.toHaveBeenCalled();
  });
});
