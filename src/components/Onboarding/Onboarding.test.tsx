import { selectOption } from "../../test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

  it("keeps the tested provider and credentials stable while validation is pending", async () => {
    let resolve!: (result: string) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "validate_api_key") return new Promise<string>((done) => { resolve = done; });
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.change(await screen.findByLabelText("Groq API Key"), { target: { value: "gsk_test" } });
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    expect(screen.getByLabelText("Groq API Key")).toBeDisabled();
    expect(screen.getByLabelText("转写服务")).toBeDisabled();
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    resolve("valid");
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    expect(screen.getByLabelText("Groq API Key")).toBeEnabled();
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

  it("blocks onboarding when OnDevice model files exist but inference is unavailable", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "list_on_device_models") {
        return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: false, platform_supported: true, runtime_status: "not_checked" }];
      }
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      if (command === "get_settings") {
        return { ...baseSettings, asr_provider: "on_device", onboarded: true, cleanup_enabled: false };
      }
      return undefined;
    });
    const onFinish = vi.fn();
    render(<Onboarding settings={baseSettings} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    selectOption(await screen.findByLabelText("转写服务"), "on_device");
    expect(screen.queryByLabelText("Groq API Key")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("模型文件已下载并校验 · 本机运行时尚未检查")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    expect(screen.queryByRole("heading", { name: "设置语音输入快捷键" })).not.toBeInTheDocument();
    expect(onFinish).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => command === "update_settings_patch")).toBe(false);
  });

  it("does not persist OnDevice before the hotkey trial without an inference runtime", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "list_on_device_models") {
        return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: false, platform_supported: true, runtime_status: "not_checked" }];
      }
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    selectOption(await screen.findByLabelText("转写服务"), "on_device");
    await waitFor(() => expect(screen.getByText("模型文件已下载并校验 · 本机运行时尚未检查")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    expect(screen.queryByRole("heading", { name: "设置语音输入快捷键" })).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.some(([command]) => command === "update_settings_patch")).toBe(false);
  });

  it("does not keep a typed Groq key after the user switches to OnDevice", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "list_on_device_models") {
        return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: false, platform_supported: true, runtime_status: "not_checked" }];
      }
      if (command === "validate_api_key") return "valid";
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.change(await screen.findByLabelText("Groq API Key"), { target: { value: "gsk_typed_then_switched" } });
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByText("访问密钥有效")).toBeInTheDocument());
    selectOption(screen.getByLabelText("转写服务"), "on_device");
    await waitFor(() => expect(screen.getByText("模型文件已下载并校验 · 本机运行时尚未检查")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    const patches = invokeMock.mock.calls
      .filter(([command]) => command === "update_settings_patch")
      .map(([, args]) => (args as { patch?: Record<string, unknown> } | undefined)?.patch ?? {});
    expect(patches.some((patch) => patch.asr_provider === "on_device")).toBe(false);
    expect(patches.some((patch) => typeof patch.api_key === "string")).toBe(false);
  });

  it("does not allow a hotkey trial with OnDevice while inference is unavailable", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "list_on_device_models") {
        return [{ id: "qwen3-asr-0.6b", state: "missing", platform_supported: true, runtime_status: "not_checked" }];
      }
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    selectOption(await screen.findByLabelText("转写服务"), "on_device");
    expect(screen.getByText("模型文件未下载 · 本机运行时尚未检查")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    expect(screen.queryByRole("heading", { name: "设置语音输入快捷键" })).not.toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_configured_api_key"));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });

    const pane = screen.getByRole("heading", { name: "设置语音输入快捷键" }).closest(".overflow-y-auto");
    expect(pane).toBeTruthy();
    expect(pane).toHaveClass("vf-onboarding-content");
    expect(pane).not.toHaveClass("vf-onboarding-content--welcome");
    expect(document.querySelector(".vf-onboarding-footer-inner")).not.toHaveClass("vf-onboarding-footer-inner--welcome");
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
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_configured_api_key"));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
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
    expect((finishCall?.[1] as { patch: Record<string, unknown> }).patch).not.toHaveProperty("selected_actions_enabled");
    expect((finishCall?.[1] as { patch: Record<string, unknown> }).patch).not.toHaveProperty("selected_action_hotkey");
  });

  it.each(["save", "read"])("blocks optional trial navigation while finish %s is pending", async (pending) => {
    let resolve!: (value: unknown) => void;
    const finishedSettings = { ...baseSettings, api_key_configured: true, onboarded: true };
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "validate_configured_api_key") return "valid";
      const isFinishSave = command === "update_settings_patch"
        && (args as { patch?: { onboarded?: boolean } } | undefined)?.patch?.onboarded;
      if ((pending === "save" && isFinishSave) || (pending === "read" && command === "get_settings")) {
        return new Promise<unknown>((done) => { resolve = done; });
      }
      if (command === "get_settings") return finishedSettings;
      return undefined;
    });
    const onFinish = vi.fn();
    render(<Onboarding settings={{ ...baseSettings, api_key_configured: true }} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.click(screen.getByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "准备好了" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));
    await waitFor(() => expect(resolve).toBeDefined());
    const optionalTrial = screen.getByRole("button", { name: "试用选中文本操作（可选）" });
    expect(optionalTrial).toBeDisabled();
    fireEvent.click(optionalTrial);
    expect(screen.queryByRole("heading", { name: "设置选中文本操作快捷键" })).not.toBeInTheDocument();
    expect(onFinish).not.toHaveBeenCalled();
    await act(async () => { resolve(pending === "read" ? finishedSettings : undefined); });
    await waitFor(() => expect(onFinish).toHaveBeenCalledExactlyOnceWith(finishedSettings));
  });

  it("keeps cloud choices advanced and saves the ready on-device route without cloud cleanup", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "list_on_device_models") return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready" }];
      return undefined;
    });
    render(<Onboarding settings={{ ...baseSettings, cleanup_enabled: true }} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByLabelText("Groq API Key");
    expect(screen.getByText(/^高级配置/).closest("details")).not.toHaveAttribute("open");
    fireEvent.click(screen.getByRole("button", { name: /本机路线/ }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", { patch: expect.objectContaining({ asr_provider: "on_device", cleanup_enabled: false }) });
    expect(invokeMock.mock.calls.some(([command]) => command === "validate_api_key" || command === "probe_engine_draft")).toBe(false);
  });

  it("finishes setup with an already selected non-Groq provider after checking it", async () => {
    const selectedProviderSettings = {
      ...baseSettings,
      asr_provider: "openai" as const,
      asr_model: "gpt-transcribe",
      cleanup_enabled: false,
      provider_keys: { openai: { configured: true, hint: "••••test" } },
    };
    const finishedSettings = { ...selectedProviderSettings, onboarded: true };
    invokeMock.mockImplementation(async (command) => {
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "probe_engine_draft") {
        return { asr: { ok: true, skipped: false }, cleanup: { ok: true, skipped: true } };
      }
      if (command === "get_settings") return finishedSettings;
      return undefined;
    });
    const onFinish = vi.fn();
    render(
      <Onboarding
        settings={selectedProviderSettings}
        onFinish={onFinish}
        onSkipToSettings={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "连接语音服务" });
    fireEvent.click(await screen.findByRole("button", { name: "测试转写服务" }));
    await screen.findByText("服务检查通过");
    expect(invokeMock).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ asr_provider: "openai", cleanup_enabled: false }),
    }));

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "准备好了" });
    fireEvent.click(screen.getByRole("button", { name: "试用选中文本操作（可选）" }));
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
      patch: expect.objectContaining({ onboarded: true, asr_provider: "openai" }),
    });
    expect(finishCall?.[1]).not.toEqual({
      patch: expect.objectContaining({ api_key: expect.any(String) }),
    });
    expect(invokeMock).not.toHaveBeenCalledWith("validate_api_key", expect.anything());
  });
});
