import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { Onboarding } from "./Onboarding";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const invokeMock = vi.mocked(invoke);
const baseSettings = {
  api_key_configured: false,
  hotkey: "CmdOrControl+Shift+Space",
  activation_mode: "tap",
  selected_action_hotkey: "CmdOrControl+Shift+Slash",
  ui_language: "en" as const,
  onboarded: false,
};

describe("Onboarding", () => {
  beforeEach(() => {
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      if (command === "get_settings") return { ...baseSettings, api_key_configured: true, onboarded: true };
      return undefined;
    });
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("shows a keychain error when the API key cannot be saved", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "validate_api_key") return "valid";
      if (command === "update_settings_patch") {
        throw new Error("credential_storage: failed to store API key securely: keychain write timed out");
      }
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.change(await screen.findByLabelText("Groq API Key"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByText("访问密钥有效")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("无法保存到这台 Mac 的钥匙串"));
    expect(screen.getByRole("heading", { name: "连接语音服务" })).toBeInTheDocument();
  });

  it("shows the engine validation error when leftover custom ASR blocks the save", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "validate_api_key") return "valid";
      if (command === "update_settings_patch") {
        throw new Error("自定义 ASR 地址需要填写 ASR 密钥。");
      }
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.change(await screen.findByLabelText("Groq API Key"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByText("访问密钥有效")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("自定义 ASR 地址需要填写 ASR 密钥"));
    expect(screen.getByRole("heading", { name: "连接语音服务" })).toBeInTheDocument();
  });

  it("does not allow onboarding to finish without an API key", async () => {
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    const permissionsContinue = await screen.findByRole("button", { name: "继续" });
    await waitFor(() => expect(permissionsContinue).toBeEnabled());
    fireEvent.click(permissionsContinue);

    expect(await screen.findByRole("heading", { name: "连接语音服务" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "开始使用" })).not.toBeInTheDocument();
  });

  it("uses the full main pane on the voice input trial", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "validate_configured_api_key") return "valid";
      return undefined;
    });
    render(
      <Onboarding
        settings={{ ...baseSettings, api_key_configured: true }}
        onFinish={vi.fn()}
        onSkipToSettings={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_configured_api_key"));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });

    const pane = screen.getByRole("heading", { name: "设置语音输入快捷键" }).closest(".overflow-y-auto");
    expect(pane).toBeTruthy();
    expect(pane?.className).not.toMatch(/max-w-\[480px\]/);
    expect(pane?.className).toMatch(/max-w-none|max-w-3xl|max-w-\[7/);
    expect(pane?.className).not.toMatch(/px-8/);
  });

  it("uses the settings skip callback without finishing onboarding", () => {
    const onFinish = vi.fn();
    const onSkipToSettings = vi.fn();
    render(<Onboarding settings={baseSettings} onFinish={onFinish} onSkipToSettings={onSkipToSettings} />);

    fireEvent.click(screen.getByRole("button", { name: "稍后设置" }));

    expect(onSkipToSettings).toHaveBeenCalledOnce();
    expect(onFinish).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "update_settings_patch",
      expect.objectContaining({ patch: expect.objectContaining({ onboarded: true }) }),
    );
  });

  it("gates the permissions step until microphone access is available", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: false, microphone_status: "denied", accessibility: false };
      }
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "继续" }));

    expect(await screen.findByRole("heading", { name: "先确认必要权限" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    expect(screen.queryByRole("heading", { name: "连接语音服务" })).not.toBeInTheDocument();
  });

  it("preserves the current UI language when finishing", async () => {
    const finishedSettings = { ...baseSettings, api_key_configured: true, onboarded: true };
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "validate_configured_api_key") return "valid";
      if (command === "get_settings") return finishedSettings;
      return undefined;
    });
    const onFinish = vi.fn();
    render(
      <Onboarding
        settings={{ ...baseSettings, api_key_configured: true }}
        onFinish={onFinish}
        onSkipToSettings={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_configured_api_key"));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置选中文本操作快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "准备好了" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));

    await waitFor(() => expect(onFinish).toHaveBeenCalledWith(finishedSettings));
    const finishCall = invokeMock.mock.calls.find(
      ([command, args]) => command === "update_settings_patch"
        && (args as { patch?: { onboarded?: boolean } } | undefined)?.patch?.onboarded === true,
    );
    expect(finishCall?.[1]).toEqual({
      patch: expect.objectContaining({ onboarded: true, ui_language: "en" }),
    });
    expect(finishCall?.[1]).not.toEqual({
      patch: expect.objectContaining({ ui_language: "system" }),
    });
  });
});
