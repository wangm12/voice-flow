import { selectOption } from "../../test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
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

let fixtureSettings: Record<string, unknown>;
let fixtureGeneration: bigint;
let activeTrialId: string | null;

function mockBackend(handler: (command: string, args?: unknown) => Promise<unknown>) {
  invokeMock.mockImplementation(async (command, args) => {
    if (command === "get_onboarding_configuration_generation") return String(fixtureGeneration);
    if (command === "set_onboarding_test_mode") {
      const request = args as { enabled: boolean; trialId: string };
      if (request.enabled) activeTrialId = request.trialId;
      else if (activeTrialId === request.trialId) activeTrialId = null;
      return { trial_id: request.trialId, config_generation: String(fixtureGeneration), enabled: request.enabled };
    }
    const result = await handler(command, args);
    if (command === "update_settings_patch") {
      const patch = (args as { patch: Record<string, unknown> }).patch;
      if (Object.entries(patch).some(([key, value]) => fixtureSettings[key] !== value)) fixtureGeneration += 1n;
      fixtureSettings = { ...fixtureSettings, ...patch, ...(patch.api_key ? { api_key_configured: true } : {}) };
    }
    if (command === "get_settings") return result && typeof result === "object" ? { ...fixtureSettings, ...result } : fixtureSettings;
    if (command === "get_audio_input_devices" && result === undefined) return [{ name: "Fixture microphone", is_default: true }];
    return result;
  });
}

describe("Onboarding", () => {
  beforeEach(() => {
    fixtureSettings = { ...baseSettings }; fixtureGeneration = 1n; activeTrialId = null;
    listenMock.mockResolvedValue(vi.fn());
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
      if (command === "check_permissions") {
        return { microphone: true, microphone_status: "authorized", accessibility: false };
      }
      if (command === "set_onboarding_test_mode" || command === "update_settings_patch") return undefined;
      if (command === "get_settings") return { ...baseSettings, api_key_configured: true, onboarded: true };
      return undefined;
    });
  });

  afterEach(async () => {
    cleanup();
    await act(async () => {});
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("finishes configured cloud setup without automatically probing or starting a recording", async () => {
    const onFinish = vi.fn();
    render(<Onboarding settings={{ ...baseSettings, api_key_configured: true }} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "连接语音服务" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    expect(screen.getByText("外部插入确认").nextElementSibling).toHaveTextContent("尚未测试");
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));
    await waitFor(() => expect(onFinish).toHaveBeenCalledOnce());
    const called = invokeMock.mock.calls.map(([command]) => command);
    expect(called).not.toContain("validate_api_key");
    expect(called).not.toContain("validate_configured_api_key");
    expect(called).not.toContain("probe_engine_draft");
    expect(called).not.toContain("start_microphone_check");
  });


  it("keeps only a native-prepared external focus continuation and invalidates it on actual unknown readback", async () => {
    let known = true;
    mockBackend(async (command, args) => {
      if (command === "get_settings") return { ...baseSettings, api_key_configured: true, onboarded: true };
      if (command === "check_permissions") return known ? { microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: false } : null;
      if (command === "prepare_onboarding_external") {
        const lease = { trial_id: (args as { trialId: string }).trialId, config_generation: String(fixtureGeneration) };
        return { lease, target_app: "text_edit", before: "prefix[REPLACE ONLY THIS]", selected: "REPLACE ONLY THIS", inserted: "once", after: "prefix[once]",
          selection_start_utf16: 7, preparation_ms: 180000, switch_ms: 5000 };
      }
      return undefined;
    });
    render(<Onboarding settings={{ ...baseSettings, api_key_configured: true }} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "连接语音服务" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("button", { name: "开始受控外部测试（可选）" });
    fireEvent.click(screen.getByRole("button", { name: "开始受控外部测试（可选）" }));
    await screen.findByRole("button", { name: "复制固定测试文字" });
    const before = activeTrialId;
    fireEvent.blur(window);
    await act(async () => {});
    expect(activeTrialId).toBe(before);
    known = false;
    fireEvent.focus(window);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_onboarding_test_mode", { enabled: false, trialId: before }));
    expect(screen.queryByRole("button", { name: "五秒后测试：现在切换到固定 TextEdit 文档" })).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.map(([command]) => command)).not.toContain("run_onboarding_external");
  });

  async function openEngineStep(language: "zh" | "en" = "zh") {
    const labels = language === "en"
      ? { continue: "Continue", permissions: "Confirm the required permissions", engine: "Connect voice service" }
      : { continue: "继续", permissions: "先确认必要权限", engine: "连接语音服务" };
    fireEvent.click(screen.getByRole("button", { name: labels.continue }));
    await screen.findByRole("heading", { name: labels.permissions });
    await waitFor(() => expect(screen.getByRole("button", { name: labels.continue })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: labels.continue }));
    await screen.findByRole("heading", { name: labels.engine });
  }

  async function completeFromEngine(onFinish: ReturnType<typeof vi.fn>) {
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));
    await waitFor(() => expect(onFinish).toHaveBeenCalledOnce());
  }

  function localSetup(onboarded: boolean, cleanupProvider: "groq" | "ollama") {
    const selected = { ...baseSettings, onboarded, asr_provider: "on_device", asr_model: "qwen3-asr-0.6b", language: "auto", cleanup_enabled: true, cleanup_intensity: "standard", cleanup_provider: cleanupProvider, cleanup_model: cleanupProvider === "ollama" ? "qwen3.5:4b" : "openai/gpt-oss-20b", strict_offline_enabled: cleanupProvider === "ollama" };
    fixtureSettings = selected;
    mockBackend(async (command) => {
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "list_on_device_models") return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready" }];
      if (command === "preview_writing_mode") return { saved: null, draft: { text: "Fixture preview", status: "local_only", elapsed_ms: 1 } };
      return undefined;
    });
    return selected;
  }

  it.each(["ollama", "groq"] as const)("preserves saved %s cleanup when rechecking the existing local ASR route", async (provider) => {
    const selected = localSetup(true, provider);
    const onFinish = vi.fn();
    render(<Onboarding settings={selected} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    fireEvent.click(screen.getByText("整理强度与保真"));
    expect(screen.getByLabelText("AI 整理强度")).toHaveTextContent("Standard");
    expect(screen.getByText(/已配置整理服务/)).toHaveTextContent(provider);
    expect(screen.queryByText(/新选择本机路线默认关闭/)).not.toBeInTheDocument();
    await completeFromEngine(onFinish);
    expect(onFinish).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: true, cleanup_provider: provider, cleanup_intensity: "standard" }));
    const patches = invokeMock.mock.calls.filter(([command]) => command === "update_settings_patch").map(([, args]) => (args as { patch: Record<string, unknown> }).patch);
    expect(patches.some((patch) => "cleanup_enabled" in patch)).toBe(false);
    expect(invokeMock.mock.calls.some(([command]) => command === "probe_engine_draft" || command === "validate_api_key")).toBe(false);
  });

  it("applies Off during initial local setup without an automatic cleanup preview", async () => {
    const selected = { ...localSetup(false, "groq"), writing_modes: [{ id: "fixture", label: "Fixture mode", family: "email", prompt: "Keep the fixture meaning.", builtin: false }] };
    fixtureSettings = selected;
    const onFinish = vi.fn();
    render(<Onboarding settings={selected} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    fireEvent.click(screen.getByText("整理强度与保真"));
    expect(screen.getByLabelText("AI 整理强度")).toHaveTextContent("Off");
    expect(screen.getByText(/默认选择尚未保存/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "试跑当前语气" })).not.toBeInTheDocument();
    await completeFromEngine(onFinish);
    expect(onFinish).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: false, cleanup_provider: "groq" }));
    expect(invokeMock.mock.calls.some(([command]) => command === "preview_writing_mode")).toBe(false);
  });

  it("honors an explicit Off selection on a saved local route", async () => {
    const selected = localSetup(true, "ollama");
    const onFinish = vi.fn();
    render(<Onboarding settings={selected} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    fireEvent.click(screen.getByText("整理强度与保真"));
    selectOption(screen.getByLabelText("AI 整理强度"), "off");
    await waitFor(() => expect(screen.getByLabelText("AI 整理强度")).toBeEnabled());
    await completeFromEngine(onFinish);
    expect(onFinish).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: false, cleanup_intensity: "off", cleanup_provider: "ollama" }));
  });

  it("lets an explicit new cleanup choice override the first local default", async () => {
    const mode = { id: "fixture", label: "Fixture mode", family: "email", prompt: "Keep the fixture meaning.", builtin: false };
    const selected = { ...localSetup(false, "groq"), writing_modes: [mode] };
    fixtureSettings = selected;
    const onFinish = vi.fn();
    render(<Onboarding settings={selected} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    fireEvent.click(screen.getByText("整理强度与保真"));
    selectOption(screen.getByLabelText("AI 整理强度"), "light");
    await waitFor(() => expect(screen.getByLabelText("AI 整理强度")).toBeEnabled());
    fireEvent.change(await screen.findByLabelText("试跑文本"), { target: { value: "Do not change API version 2.0." } });
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("preview_writing_mode", { request: expect.objectContaining({ mode, text: "Do not change API version 2.0.", compare_saved: false }) }));
    await completeFromEngine(onFinish);
    expect(onFinish).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: true, cleanup_intensity: "light", cleanup_provider: "groq" }));
  });

  it("presents and applies the declared Off default when explicitly changing to the local route", async () => {
    const selected = { ...localSetup(true, "groq"), asr_provider: "groq", asr_model: "whisper-large-v3-turbo", api_key_configured: true };
    fixtureSettings = selected;
    const onFinish = vi.fn();
    render(<Onboarding settings={selected} onFinish={onFinish} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    fireEvent.click(screen.getByRole("button", { name: /本机路线/ }));
    fireEvent.click(screen.getByText("整理强度与保真"));
    expect(screen.getByLabelText("AI 整理强度")).toHaveTextContent("Off");
    expect(screen.getByText(/新选择本机路线默认关闭/)).toBeInTheDocument();
    await completeFromEngine(onFinish);
    expect(onFinish).toHaveBeenCalledWith(expect.objectContaining({ asr_provider: "on_device", cleanup_enabled: false }));
  });

  it("shows a keychain error when the API key cannot be saved", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByText("访问密钥有效")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("无法保存到这台 Mac 的钥匙串"));
    expect(screen.getByRole("heading", { name: "连接语音服务" })).toBeInTheDocument();
  });

  it("keeps the tested provider and credentials stable while validation is pending", async () => {
    let resolve!: (result: string) => void;
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "validate_api_key") return new Promise<string>((done) => { resolve = done; });
      return undefined;
    });
    render(<Onboarding settings={baseSettings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    fireEvent.change(await screen.findByLabelText("Groq API Key"), { target: { value: "gsk_test" } });
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
    expect(screen.getByLabelText("Groq API Key")).toBeDisabled();
    expect(screen.getByLabelText("转写服务")).toBeDisabled();
    expect(screen.getByRole("button", { name: "继续" })).toBeDisabled();
    resolve("valid");
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    expect(screen.getByLabelText("Groq API Key")).toBeEnabled();
  });

  it("shows the engine validation error when leftover custom ASR blocks the save", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByText("访问密钥有效")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("自定义 ASR 地址需要填写 ASR 密钥"));
    expect(screen.getByRole("heading", { name: "连接语音服务" })).toBeInTheDocument();
  });

  it.each([
    ["qwen3-asr-1.7b", "missing", "runtime_ready", false],
    ["qwen3-asr-1.7b", "ready", "runtime_ready", false],
    ["qwen3-asr-1.7b", "ready", "loaded", true],
    ["cohere-transcribe-2b", "missing", "runtime_ready", false],
    ["cohere-transcribe-2b", "ready", "runtime_ready", false],
    ["cohere-transcribe-2b", "ready", "loaded", true],
  ] as const)("shows the unreviewed conversion notice on the configured %s card in %s/%s", async (id, state, runtime_status, loaded) => {
    const settings = { ...baseSettings, asr_provider: "on_device", asr_model: id, language: id === "cohere-transcribe-2b" ? "en" : "auto" };
    fixtureSettings = settings;
    mockBackend(async (command) => {
      if (command === "list_on_device_models") return [{ id, label: id, state, inference_ready: state === "ready", platform_supported: true, runtime_status, loaded, downloaded_bytes: state === "ready" ? 100 : 0, bytes: 100 }];
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      return undefined;
    });
    render(<I18nProvider initialLanguage="en"><Onboarding settings={settings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} /></I18nProvider>);
    await openEngineStep("en");
    const notice = await screen.findByRole("note");
    expect(within(notice).getByText("License unreviewed")).toBeInTheDocument();
    expect(within(notice).getByText("The official model card declares Apache-2.0; the license for this fixed MLX conversion remains unreviewed.")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("download_on_device_model", expect.anything());
  });

  it("shows no target notice on the configured 0.6B model card", async () => {
    const settings = { ...baseSettings, asr_provider: "on_device", asr_model: "qwen3-asr-0.6b", language: "auto" };
    fixtureSettings = settings;
    mockBackend(async (command) => {
      if (command === "list_on_device_models") return [{ id: "qwen3-asr-0.6b", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready", loaded: false }];
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      return undefined;
    });
    render(<I18nProvider initialLanguage="en"><Onboarding settings={settings} onFinish={vi.fn()} onSkipToSettings={vi.fn()} /></I18nProvider>);
    await openEngineStep("en");
    expect(screen.queryByText("License unreviewed")).not.toBeInTheDocument();
  });

  it("blocks onboarding when OnDevice model files exist but inference is unavailable", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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

  it("lets onboarding manually retry the selected model after a storage failure", async () => {
    const download = vi.fn();
    fixtureSettings = { ...baseSettings, asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" };
    mockBackend(async (command) => {
      if (command === "list_on_device_models") return [{
        id: "qwen3-asr-0.6b", label: "Qwen3 ASR", state: "missing", inference_ready: false,
        error: "write failed: No space left on device", error_category: "disk_full",
        downloaded_bytes: 0, bytes: 100, sha256: null, revision: null, file_count: 1,
        platform_supported: true, runtime_status: "runtime_ready", loaded: false,
      }];
      if (command === "download_on_device_model") download();
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "get_settings") return fixtureSettings;
      return undefined;
    });
    render(<Onboarding settings={{ ...baseSettings, asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" }} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    await openEngineStep();
    expect(await screen.findByText("检查可用空间后重试下载。")).toBeInTheDocument();
    expect(screen.getByText("write failed: No space left on device")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "下载模型" }));
    await waitFor(() => expect(download).toHaveBeenCalledOnce());
    expect(invokeMock).toHaveBeenCalledWith("download_on_device_model", { id: "qwen3-asr-0.6b" });
  });

  it("does not persist OnDevice before the hotkey trial without an inference runtime", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
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
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
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

  it("uses the settings skip callback without finishing onboarding", async () => {
    const onFinish = vi.fn();
    const onSkipToSettings = vi.fn();
    render(<Onboarding settings={baseSettings} onFinish={onFinish} onSkipToSettings={onSkipToSettings} />);

    fireEvent.click(screen.getByRole("button", { name: "稍后设置" }));

    await waitFor(() => expect(onSkipToSettings).toHaveBeenCalledOnce());
    expect(onFinish).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "update_settings_patch",
      expect.objectContaining({ patch: expect.objectContaining({ onboarded: true }) }),
    );
  });

  it("gates the permissions step until microphone access is available", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_configured_api_key"));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());

    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));

    await waitFor(() => expect(onFinish).toHaveBeenCalledWith(expect.objectContaining(finishedSettings)));
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
    let finishSaved = false;
    const finishedSettings = { ...baseSettings, api_key_configured: true, onboarded: true };
    mockBackend(async (command, args) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
      if (command === "check_permissions") return { microphone: true, microphone_status: "authorized", accessibility: false };
      if (command === "validate_configured_api_key") return "valid";
      const isFinishSave = command === "update_settings_patch"
        && (args as { patch?: { onboarded?: boolean } } | undefined)?.patch?.onboarded;
      if (isFinishSave) finishSaved = true;
      if ((pending === "save" && isFinishSave) || (pending === "read" && finishSaved && command === "get_settings")) {
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
    fireEvent.click(await screen.findByRole("button", { name: "验证 API Key" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "继续" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置语音输入快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));
    await waitFor(() => expect(resolve).toBeDefined());
    const optionalTrial = screen.getByRole("button", { name: "试用选中文本操作（可选）" });
    expect(optionalTrial).toBeDisabled();
    fireEvent.click(optionalTrial);
    expect(screen.queryByRole("heading", { name: "设置选中文本操作快捷键" })).not.toBeInTheDocument();
    expect(onFinish).not.toHaveBeenCalled();
    await act(async () => { resolve(pending === "read" ? finishedSettings : undefined); });
    await waitFor(() => expect(onFinish).toHaveBeenCalledExactlyOnceWith(expect.objectContaining(finishedSettings)));
  });

  it("keeps cloud choices advanced and saves the ready on-device route without cloud cleanup", async () => {
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    mockBackend(async (command) => {
      if (command === "get_audio_input_devices") return [{ name: "Fixture microphone", is_default: true }];
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
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    fireEvent.click(screen.getByRole("button", { name: "试用选中文本操作（可选）" }));
    await screen.findByRole("heading", { name: "设置选中文本操作快捷键" });
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
    fireEvent.click(screen.getByRole("button", { name: "开始使用" }));

    await waitFor(() => expect(onFinish).toHaveBeenCalledWith(expect.objectContaining(finishedSettings)));
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
  it("invalidates actual Finish facts on revoke/unknown/reentry and refuses late old trial receipts after regrant", async () => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    listenMock.mockImplementation(async (event, handler) => { handlers.set(event, handler as (event: { payload: unknown }) => void); return vi.fn(); });
    let permissions: unknown = { microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: true };
    fixtureSettings = { ...baseSettings, api_key_configured: true };
    const permissionReadbacks: { sequence: number; value: unknown }[] = [];
    mockBackend(async (command) => {
      if (command !== "check_permissions") return undefined;
      const value = permissions;
      permissionReadbacks.push({ sequence: permissionReadbacks.length + 1, value });
      return value;
    });
    // Drive all *active* interval registrations through their real lifecycle.
    // A saved callback/constant ID cannot establish that its owner is current,
    // nor await the production () => void poll() wrapper's async readback.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const permissionTick = async () => {
      const before = permissionReadbacks.length;
      const callBoundary = invokeMock.mock.calls.length;
      const expected = permissions;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
        const requests = invokeMock.mock.calls.flatMap(([command], index) =>
          command === "check_permissions" && index >= callBoundary ? [index] : []);
        expect(requests.length, "current timer tick must issue an actual permission read").toBeGreaterThan(0);
        // Await the actual mocked IPC promise, not the void timer wrapper.
        for (const index of requests) expect(await invokeMock.mock.results[index].value).toBe(expected);
      });
      expect(permissionReadbacks.length).toBeGreaterThan(before);
      expect(permissionReadbacks[permissionReadbacks.length - 1]?.value).toBe(expected);
    };
    const view = render(<Onboarding settings={{ ...baseSettings, api_key_configured: true }} onFinish={vi.fn()} onSkipToSettings={vi.fn()} />);
    const result = (id: string, text: string) => ({ trial_id: id, config_generation: String(fixtureGeneration), recording_generation: "7", trigger_source: "native_hotkey", trial_current: true, final_text: text });
    const finishFact = () => screen.getByText("应用内听写结果").nextElementSibling;
    try {
      await openEngineStep();
      fireEvent.click(screen.getByRole("button", { name: "继续" }));
      await screen.findByRole("heading", { name: "设置语音输入快捷键" });
      await waitFor(() => expect(activeTrialId).not.toBeNull());
      await waitFor(() => expect(screen.queryByText("试用正在准备；完成后再按快捷键。")).not.toBeInTheDocument());
      const oldId = activeTrialId!;
      act(() => handlers.get("onboarding://error")!({ payload: {
        trial_id: oldId, config_generation: String(fixtureGeneration), recording_generation: "7",
        trigger_source: "native_hotkey", trial_current: true, code: "no_speech", message: "provider raw failure",
      } }));
      expect(screen.getByRole("alert")).toHaveTextContent("没有检测到语音；请检查麦克风后重试。");
      expect(screen.getByText("provider raw failure")).toBeInTheDocument();
      const oldResult = result(oldId, "first actual fixture");
      act(() => handlers.get("dictation://onboarding-result")!({ payload: oldResult }));
      expect(screen.getByLabelText("语音输入试用框")).toHaveValue("first actual fixture");
      fireEvent.click(screen.getByRole("button", { name: "继续" }));
      await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
      expect(finishFact()).toHaveTextContent("本次检查通过");

      permissions = { microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: false };
      await permissionTick();
      expect(finishFact()).toHaveTextContent("无法确认");
      permissions = { microphone: false, microphone_status: "denied", accessibility: true, screen_recording: false };
      await permissionTick();
      expect(finishFact()).toHaveTextContent("无法确认");
      // Regrant must not revive the old fact or the old trial ID.
      permissions = { microphone: true, microphone_status: "authorized", accessibility: true };
      await permissionTick();
      expect(finishFact()).toHaveTextContent("无法确认");
      fireEvent.click(screen.getByRole("button", { name: "重新试用听写" }));
      await waitFor(() => expect(activeTrialId && activeTrialId !== oldId).toBeTruthy());
      await waitFor(() => expect(screen.queryByText("试用正在准备；完成后再按快捷键。")).not.toBeInTheDocument());
      const freshId = activeTrialId!;
      act(() => handlers.get("dictation://onboarding-result")!({ payload: { ...oldResult, recording_generation: "8" } }));
      expect(screen.getByLabelText("语音输入试用框")).toHaveValue("");
      // Invalidate while this exact lease is live, not only after step exit.
      permissions = undefined;
      await permissionTick();
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_onboarding_test_mode", { enabled: false, trialId: freshId }));
      await waitFor(() => expect(activeTrialId, `unknown readbacks observed: ${permissionReadbacks.length}`).toBeNull());
      expect(permissionReadbacks[permissionReadbacks.length - 1]?.value).toBeUndefined();
      act(() => handlers.get("dictation://onboarding-result")!({ payload: { ...result(freshId, "late while unknown"), recording_generation: "8" } }));
      expect(screen.getByLabelText("语音输入试用框")).toHaveValue("");
      permissions = { microphone: true, microphone_status: "authorized", accessibility: true };
      await permissionTick();
      await waitFor(() => expect(activeTrialId && activeTrialId !== freshId).toBeTruthy());
      await waitFor(() => expect(screen.queryByText("试用正在准备；完成后再按快捷键。")).not.toBeInTheDocument());
      const currentId = activeTrialId!;
      act(() => handlers.get("dictation://onboarding-result")!({ payload: { ...result(freshId, "late after regrant"), recording_generation: "8" } }));
      expect(screen.getByLabelText("语音输入试用框")).toHaveValue("");
      act(() => handlers.get("dictation://onboarding-result")!({ payload: result(currentId, "new actual fixture") }));
      expect(screen.getByLabelText("语音输入试用框")).toHaveValue("new actual fixture");
      fireEvent.click(screen.getByRole("button", { name: "继续" }));
      await screen.findByRole("heading", { name: "设置已完成，随时可以重测" });
      expect(finishFact()).toHaveTextContent("本次检查通过");

      permissions = { microphone: false, microphone_status: "unknown", accessibility: true };
      await permissionTick();
      expect(finishFact()).toHaveTextContent("无法确认");
      permissions = { microphone: true, microphone_status: "authorized", accessibility: true };
      await permissionTick();
      expect(finishFact()).toHaveTextContent("无法确认");
      // Window reentry invalidates even an unchanged permission readback and
      // cannot rewrite native configuration or credential ownership.
      const nativeGeneration = fixtureGeneration;
      act(() => window.dispatchEvent(new Event("blur")));
      await act(async () => window.dispatchEvent(new Event("focus")));
      expect(finishFact()).toHaveTextContent("无法确认");
      expect(fixtureGeneration).toBe(nativeGeneration);
      expect(screen.getByText("凭据配置").nextElementSibling).toHaveTextContent("本次检查通过");
    } finally { view.unmount(); vi.useRealTimers(); }
  });

  it.each([
    { language: "zh" as const, selection: "请先在试用输入框中选中要修改的文字。", fallback: "语音试用未完成；请检查设备或语音服务后重试。" },
    { language: "en" as const, selection: "Select the text to change in the trial input first.", fallback: "The voice trial did not finish. Check your device or speech service and try again." },
  ])("keeps trial error details with their current error in $language", async ({ language, selection, fallback }) => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    listenMock.mockImplementation(async (event, handler) => { handlers.set(event, handler as (event: { payload: unknown }) => void); return vi.fn(); });
    fixtureSettings = { ...baseSettings, api_key_configured: true };
    mockBackend(async (command) => command === "check_permissions"
      ? { microphone: true, microphone_status: "authorized", accessibility: false }
      : undefined);
    render(<I18nProvider initialLanguage={language}><Onboarding settings={{ ...baseSettings, api_key_configured: true }} onFinish={vi.fn()} onSkipToSettings={vi.fn()} /></I18nProvider>);
    await openEngineStep(language);
    fireEvent.click(screen.getByRole("button", { name: language === "en" ? "Continue" : "继续" }));
    await screen.findByRole("heading", { name: language === "en" ? "Set the voice input hotkey" : "设置语音输入快捷键" });
    await waitFor(() => expect(activeTrialId).not.toBeNull());
    await waitFor(() => expect(handlers.has("onboarding://error")).toBe(true));
    const error = (code: string, message?: string, trialCurrent = true) => ({
      trial_id: activeTrialId, config_generation: String(fixtureGeneration), recording_generation: "7",
      trigger_source: "native_hotkey", trial_current: trialCurrent, code, ...(message ? { message } : {}),
    });
    act(() => handlers.get("onboarding://error")!({ payload: error("no_speech", "raw from first error") }));
    expect(screen.getByRole("alert")).toHaveTextContent(language === "en" ? "No speech was detected" : "没有检测到语音");
    expect(screen.getByText("raw from first error")).toBeInTheDocument();

    act(() => handlers.get("onboarding://error")!({ payload: error("onboarding_selection_missing") }));
    expect(screen.getByRole("alert")).toHaveTextContent(selection);
    expect(screen.queryByText("raw from first error")).not.toBeInTheDocument();

    act(() => handlers.get("onboarding://error")!({ payload: error("future_code", "raw from replaced error") }));
    expect(screen.getByRole("alert")).toHaveTextContent(fallback);
    expect(screen.getByText("raw from replaced error")).toBeInTheDocument();
    act(() => handlers.get("dictation://state")!({ payload: { ...error("future_code", "", false), state: "error" } }));
    expect(screen.getByRole("alert")).toHaveTextContent(language === "en" ? "The trial expired" : "试用已失效");
    expect(screen.queryByText("raw from replaced error")).not.toBeInTheDocument();
  });

});
