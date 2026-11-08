import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { PermissionsSettings } from "./PermissionsSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("PermissionsSettings", () => {
  afterEach(async () => {
    cleanup();
    await act(async () => {});
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

  it("opens a matching native AX guide without inferring permission from window or drag availability", async () => {
    invokeMock.mockImplementation(async (command, args) => {
      if (command !== "show_permission_guide") return undefined;
      const request = args as { guideId: string; permission: string };
      return { guide_id: request.guideId, permission: request.permission, bundle_path: "/Applications/Fixture.app", bundle_identifier: "test.fixture", signature_status: "valid", drag_available: true, permission_allowed: false, window_open: true, placement: "independent_movable", detail: null };
    });
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: true, microphone_status: "authorized", accessibility: false }} onRefresh={onRefresh} />);
    fireEvent.click(screen.getByRole("button", { name: "开启自动粘贴" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("show_permission_guide", { permission: "accessibility", guideId: expect.stringMatching(/^[0-9a-f-]{36}$/) }));
    expect(await screen.findByText(/拖拽完成不代表已授权/)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("等待本机权限检查");
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("closes the same guide ID only after native permission readback grants access", async () => {
    let guideId = "";
    invokeMock.mockImplementation(async (command, args) => {
      if (command !== "show_permission_guide") return undefined;
      const request = args as { guideId: string; permission: string };
      guideId = request.guideId;
      return { guide_id: guideId, permission: request.permission, bundle_path: null, bundle_identifier: null, signature_status: "unknown", drag_available: false, permission_allowed: false, window_open: true, placement: "independent_movable", detail: "development" };
    });
    const view = render(<PermissionsSettings permissions={{ microphone: true, accessibility: false }} onRefresh={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "开启自动粘贴" }));
    await waitFor(() => expect(guideId).not.toBe(""));
    view.rerender(<PermissionsSettings permissions={{ microphone: true, accessibility: true }} onRefresh={vi.fn()} />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("close_permission_guide", { guideId }));
    expect(screen.getByRole("status")).toHaveTextContent("本机权限检查已通过");
  });

  it("opens a screen guide only after explicit action and never makes it required for dictation", async () => {
    invokeMock.mockImplementation(async (command, args) => {
      if (command !== "show_permission_guide") return undefined;
      const request = args as { guideId: string; permission: string };
      return { guide_id: request.guideId, permission: request.permission, bundle_path: null, bundle_identifier: null, signature_status: "unknown", drag_available: false, permission_allowed: false, window_open: true, placement: "independent_movable", detail: null };
    });
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<PermissionsSettings permissions={{ microphone: true, accessibility: true, screen_recording: false }} onRefresh={onRefresh} />);
    expect(invokeMock).not.toHaveBeenCalledWith("show_permission_guide", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "打开屏幕录制设置" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("show_permission_guide", { permission: "screen", guideId: expect.any(String) }));
    expect(onRefresh).toHaveBeenCalledOnce();
    expect(screen.getByText(/普通听写不截图/)).toBeInTheDocument();
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
