import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { PermissionsSettings } from "./PermissionsSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("PermissionsSettings", () => {
  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
  });

  it("keeps microphone and auto-paste as separate permission rows", () => {
    render(<PermissionsSettings permissions={{ microphone: true, microphone_status: "authorized", accessibility: false }} onRefresh={vi.fn()} />);

    expect(screen.getByText("麦克风")).toBeInTheDocument();
    expect(screen.getByText("自动粘贴")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开启自动粘贴" })).toBeInTheDocument();
    expect(screen.getByText("已允许")).toBeInTheDocument();
  });

  it("opens the right macOS entry for a denied microphone permission", async () => {
    invokeMock.mockResolvedValue(undefined);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: false, microphone_status: "denied", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "打开麦克风设置" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "microphone" });
      expect(invokeMock).not.toHaveBeenCalledWith("request_microphone_permission");
      expect(onRefresh).toHaveBeenCalledOnce();
    });
  });

  it("only requests microphone access when permission is undecided", async () => {
    invokeMock.mockResolvedValue(undefined);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: false, microphone_status: "not_determined", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "允许麦克风" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("request_microphone_permission");
      expect(invokeMock).not.toHaveBeenCalledWith("open_privacy_settings", { pane: "microphone" });
    });
  });

  it("does not open settings after the native prompt grants access", async () => {
    invokeMock.mockResolvedValueOnce(true);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: false, microphone_status: "not_determined", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "允许麦克风" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("request_microphone_permission");
      expect(invokeMock).not.toHaveBeenCalledWith("open_privacy_settings", { pane: "microphone" });
      expect(onRefresh).toHaveBeenCalledOnce();
    });
  });

  it("prompts for accessibility before opening System Settings", async () => {
    invokeMock.mockResolvedValueOnce(false);
    invokeMock.mockResolvedValueOnce(undefined);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: true, microphone_status: "authorized", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "开启自动粘贴" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("request_accessibility_permission");
      expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "accessibility" });
      expect(onRefresh).toHaveBeenCalledOnce();
    });
  });

  it("does not open System Settings when accessibility is granted by the native prompt", async () => {
    invokeMock.mockResolvedValueOnce(true);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: true, microphone_status: "authorized", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "开启自动粘贴" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("request_accessibility_permission");
      expect(invokeMock).not.toHaveBeenCalledWith("open_privacy_settings", { pane: "accessibility" });
      expect(onRefresh).toHaveBeenCalledOnce();
    });
  });

  it("opens Screen Recording settings without treating it as required for dictation", async () => {
    invokeMock.mockResolvedValue(undefined);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(
      <PermissionsSettings
        permissions={{ microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: false }}
        onRefresh={onRefresh}
      />,
    );

    expect(screen.getByText("屏幕录制")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "打开屏幕录制设置" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("open_privacy_settings", { pane: "screen" });
      expect(onRefresh).toHaveBeenCalledOnce();
    });
  });

  it("exposes a manual recheck action", async () => {
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={null} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "重新检测" }));

    await waitFor(() => expect(onRefresh).toHaveBeenCalledOnce());
  });

  it("prevents repeated requests while busy and shows the actual error before allowing a retry", async () => {
    let rejectRequest!: (reason: Error) => void;
    invokeMock.mockImplementationOnce(() => new Promise((_, reject) => { rejectRequest = reject; }));
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: false, microphone_status: "not_determined", accessibility: false }} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "允许麦克风" }));
    const busy = screen.getByRole("button", { name: "处理中…" });
    expect(busy).toBeDisabled();
    expect(busy).toHaveAttribute("aria-busy", "true");
    fireEvent.click(busy);
    expect(invokeMock).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "重新检测" })).toBeDisabled();

    rejectRequest(new Error("无法打开麦克风授权"));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法打开麦克风授权");
    expect(screen.getByRole("button", { name: "允许麦克风" })).toBeEnabled();
    expect(onRefresh).not.toHaveBeenCalled();
  });
});
