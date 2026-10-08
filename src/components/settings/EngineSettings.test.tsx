import { closeSelect, openSelect, selectOption } from "../../test/selectOption";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../../types/settings";
import { EngineSettings } from "./EngineSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));

const settings: Settings = {
  schema_version: 17,
  api_key_configured: true,
  api_key_hint: "••••yabcd",
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "gpt-4o-mini",
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
  activation_mode: "tap",
  context_enabled: true,
  browser_access_enabled: false,
  context_mappings: [],
  writing_modes: [{ id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true }],
  snippets: [],
  output_mode: "auto",
  translation_target_language: "en",
  input_device: "",
  asr_provider: "groq",
  cleanup_provider: "openai",
  asr_api_key_configured: false,
  asr_api_key_hint: null,
  provider_keys: {
    groq: { configured: true, hint: "••••yabcd" },
    openai: { configured: true, hint: "••••i-key" },
  },
};

const unused = {
  saveApiKey: vi.fn(),
  removeApiKey: vi.fn(),
  saveAsrApiKey: vi.fn(),
  removeAsrApiKey: vi.fn(),
  removeCleanupApiKey: vi.fn(),
  removeProviderKey: vi.fn(),
};

function renderEngine(overrides: Partial<Settings> = {}, handlers: Partial<Parameters<typeof EngineSettings>[0]> = {}) {
  return render(
    <EngineSettings
      settings={{ ...settings, ...overrides }}
      save={handlers.save ?? vi.fn()}
      commitEngine={handlers.commitEngine ?? vi.fn()}
      {...unused}
      {...handlers}
    />,
  );
}

describe("EngineSettings form", () => {
  afterEach(() => {
    cleanup();
    vi.mocked(invoke).mockReset();
  });

  it("keeps ASR and cleanup dropdowns filtered by capability", async () => {
    renderEngine();
    const asr = screen.getByRole("combobox", { name: "转写服务" });
    const cleanup = screen.getByRole("combobox", { name: "整理服务" });
    expect(asr).toHaveValue("groq");
    expect(cleanup).toHaveValue("openai");
    const asrMenu = await openSelect(asr);
    expect(within(asrMenu).queryByRole("option", { name: "DeepSeek" })).not.toBeInTheDocument();
    expect(within(asrMenu).queryByRole("option", { name: "Anthropic" })).not.toBeInTheDocument();
    expect(within(asrMenu).getByRole("option", { name: "Deepgram" })).toBeInTheDocument();
    expect(within(asrMenu).getByRole("option", { name: "本机模型" })).toBeInTheDocument();
    closeSelect(asrMenu);
    const cleanupMenu = await openSelect(cleanup);
    expect(within(cleanupMenu).queryByRole("option", { name: "Deepgram" })).not.toBeInTheDocument();
    expect(within(cleanupMenu).queryByRole("option", { name: "Local Whisper" })).not.toBeInTheDocument();
    expect(within(cleanupMenu).getByRole("option", { name: "DeepSeek" })).toBeInTheDocument();
    expect(within(cleanupMenu).queryByRole("option", { name: "本机模型" })).not.toBeInTheDocument();
    closeSelect(cleanupMenu);
  });

  it("hides the API key field when OnDevice is selected", () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") {
        return [{ id: "sensevoice-small", state: "missing" }];
      }
      return undefined;
    });
    renderEngine({
      asr_provider: "on_device",
      asr_model: "sensevoice-small",
      cleanup_enabled: false,
      api_key_configured: false,
      provider_keys: {},
    });
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("on_device");
    expect(screen.queryByLabelText("本机模型 API Key")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("On Device API Key")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Groq API Key")).not.toBeVisible();
  });

  it("reports OnDevice model files separately from unavailable inference", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") {
        return [{ id: "sensevoice-small", state: "ready", inference_ready: false, runtime_status: "legacy_no_runtime" }];
      }
      if (command === "probe_engine_draft") {
        return {
          asr: { ok: false, skipped: false, error_kind: "on_device_inference_unavailable", message: "missing" },
          cleanup: { ok: true, skipped: true },
        };
      }
      return undefined;
    });
    renderEngine({
      asr_provider: "on_device",
      asr_model: "sensevoice-small",
      cleanup_enabled: false,
      api_key_configured: false,
      provider_keys: {},
    });
    const testButton = screen.getByRole("button", { name: "测试并应用" });
    await waitFor(() => {
      const runtimeStatus = screen.getByText("旧版模型文件没有 MLX 推理支持", { selector: "p" });
      expect(runtimeStatus.parentElement).toHaveTextContent("模型文件已下载并校验");
    });
    expect(testButton).toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("shows one key field each for Groq and OpenAI", () => {
    renderEngine();
    expect(screen.getByLabelText("Groq API Key")).toHaveValue("••••yabcd");
    expect(screen.getByLabelText("OpenAI API Key")).toHaveValue("••••i-key");
    expect(screen.getByLabelText("Deepgram API Key")).not.toBeVisible();
    expect(screen.queryByRole("button", { name: "删除本机密钥" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "删除 ASR 密钥" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "删除密钥" })).toHaveLength(2);
  });

  it("keeps the overview and disclosure focus on entry, then guides a changed service to its missing key", () => {
    renderEngine({ api_key_configured: false, api_key_hint: null, provider_keys: {} });
    expect(screen.getByLabelText("Groq API Key")).not.toHaveFocus();
    const management = screen.getByRole("button", { name: /管理其他服务/ });
    management.focus();
    fireEvent.click(management);
    expect(management).toHaveFocus();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    expect(screen.getByLabelText("Deepgram API Key")).toHaveFocus();
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("retains a single key editor and its draft while moving a provider into and out of the active configuration", () => {
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("Deepgram", { selector: "p.text-sm" }));
    const key = screen.getByLabelText("Deepgram API Key");
    fireEvent.change(key, { target: { value: "draft-deepgram-key" } });
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    expect(key).not.toBeVisible();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    expect(screen.getAllByLabelText("Deepgram API Key")).toHaveLength(1);
    expect(screen.getByLabelText("Deepgram API Key")).toBe(key);
    expect(key).toBeVisible();
    expect(key).toHaveValue("draft-deepgram-key");
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "groq");
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    expect(screen.getByLabelText("Deepgram API Key")).toBe(key);
    expect(key).toHaveValue("draft-deepgram-key");
    expect(commitEngine).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("fills Beijing DashScope Qwen3-ASR from the Chinese preset", () => {
    renderEngine();
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("兼容接口", { selector: "p.text-sm" }));
    fireEvent.click(screen.getByRole("button", { name: "阿里云百炼 Qwen3-ASR" }));
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("custom");
    expect(screen.getByLabelText("兼容地址")).toHaveValue("https://dashscope.aliyuncs.com/compatible-mode/v1");
    expect(screen.getByRole("textbox", { name: "ASR 模型" })).toHaveValue("qwen3-asr-flash");
  });

  it("fills a local mlx-qwen3-asr sidecar from the Chinese preset", () => {
    renderEngine();
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("兼容接口", { selector: "p.text-sm" }));
    fireEvent.click(screen.getByRole("button", { name: "本机 Qwen3-ASR (MLX)" }));
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("custom");
    expect(screen.getByLabelText("兼容地址")).toHaveValue("http://127.0.0.1:8765/v1");
    expect(screen.getByRole("textbox", { name: "ASR 模型" })).toHaveValue("Qwen/Qwen3-ASR-0.6B");
  });

  it("fills Handy-compatible Whisper.cpp on the local Whisper provider", () => {
    renderEngine();
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("兼容接口", { selector: "p.text-sm" }));
    fireEvent.click(screen.getByRole("button", { name: "本机 Whisper.cpp" }));
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("local_whisper");
    expect(screen.getByLabelText("Local Whisper 本机地址")).toHaveValue("http://127.0.0.1:9000/v1");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("ggml-large-v3-turbo.bin");
  });

  it("explains that local Whisper is a sidecar, not an in-app Handy download", () => {
    renderEngine({ asr_provider: "local_whisper", asr_model: "whisper-large-v3-turbo" });
    expect(
      screen.getByText(
        "本机 Whisper 走本机 HTTP，不会像 Handy 那样把 ggml 下进 App。先自己跑 whisper.cpp 或 speaches，模型名用对方接口要的名字。",
      ),
    ).toBeInTheDocument();
  });

  it("explains that the Qwen path uses chat completions", () => {
    renderEngine();
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("兼容接口", { selector: "p.text-sm" }));
    expect(
      screen.getByText("这条 Qwen 路径走 chat completions，需要带 ASR 权限的百炼密钥，不是 Groq Whisper。"),
    ).toBeInTheDocument();
  });

  it("does not offer retired Groq models to new configurations but retains an existing choice", async () => {
    renderEngine({ cleanup_provider: "groq", cleanup_model: "openai/gpt-oss-20b" });
    const newConfig = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    expect(newConfig).toHaveValue("openai/gpt-oss-20b");
    const retiredMenu = await openSelect(newConfig);
    expect(within(retiredMenu).getByRole("option", { name: /Llama 3\.1 8B Instant/ })).toHaveAttribute("data-disabled");
    closeSelect(retiredMenu);

    cleanup();
    renderEngine({ cleanup_provider: "groq", cleanup_model: "llama-3.1-8b-instant" });
    const savedConfig = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    expect(savedConfig).toHaveValue("llama-3.1-8b-instant");
    const savedMenu = await openSelect(savedConfig);
    expect(within(savedMenu).getByRole("option", { name: /Llama 3\.1 8B Instant/ })).not.toHaveAttribute("data-disabled");
    expect(within(savedMenu).getByRole("option", {
      name: /Llama 3\.1 8B Instant · 免费和 Developer 账户已于 2026-08-16 停止/,
    })).toBeInTheDocument();
    closeSelect(savedMenu);
  });

  it("keeps provider controls, credentials, and application in the same configuration surface", () => {
    renderEngine();
    const asrTitle = screen.getByText("转写", { selector: "p.text-sm" });
    const asrSelect = screen.getByRole("combobox", { name: "转写服务" });
    expect(asrTitle.compareDocumentPosition(asrSelect) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const testButton = screen.getByRole("button", { name: "测试并应用" });
    expect(screen.getByText("服务配置").closest("section")).toContainElement(testButton);
    expect(testButton.parentElement?.className).not.toMatch(/border/);
  });

  it("does not equate configured credentials with verified readiness", () => {
    renderEngine();
    expect(screen.getAllByText("凭据已保存 · 本次尚未检查")).toHaveLength(2);
    expect(screen.queryByText("已就绪")).not.toBeInTheDocument();
    expect(screen.getAllByText("使用中")).toHaveLength(2);
    expect(screen.getAllByText("未添加").every((element) => element.closest("[hidden]"))).toBe(true);
    const disclosure = screen.getByRole("button", { name: /管理其他服务/ });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(disclosure);
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByText("未添加").length).toBeGreaterThan(0);
  });

  it("keeps model changes pending and the saved route visible until explicit application", async () => {
    const save = vi.fn();
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true } });
    renderEngine({}, { save, commitEngine });
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    expect(save).not.toHaveBeenCalled();
    expect(commitEngine).not.toHaveBeenCalled();
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("whisper-large-v3-turbo");
    expect(screen.getByText("有待应用的更改；当前听写仍使用上方路线。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ asr_model: "whisper-large-v3" })));
  });

  it("does not call a cleanup provider error an invalid key", async () => {
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: false, skipped: false, error_kind: "provider", message: "HTTP status 503" },
    });
    renderEngine({
      cleanup_provider: "groq",
      cleanup_model: "openai/gpt-oss-120b",
      provider_keys: { groq: { configured: true, hint: "••••yabcd" } },
    });
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("文字整理：服务返回错误"));
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).toHaveTextContent("服务检查通过");
    expect(screen.getByText("AI 文字整理").closest("div")).toHaveTextContent("服务返回错误");
    expect(screen.queryByText("密钥无效")).not.toBeInTheDocument();
  });

  it("does not persist when the probe fails", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "model", message: "nope" },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({}, { commitEngine });
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("模型名不被这个接口接受"));
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("persists the draft once after a successful probe", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({
      cleanup_provider: "groq",
      cleanup_model: "openai/gpt-oss-20b",
      provider_keys: { groq: { configured: true, hint: "••••yabcd" } },
    }, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "custom");
    fireEvent.change(screen.getByLabelText("兼容地址"), { target: { value: "http://127.0.0.1:8000/v1" } });
    fireEvent.change(screen.getByRole("textbox", { name: "ASR 模型" }), { target: { value: "whisper-1" } });
    fireEvent.change(screen.getByLabelText("OpenAI Compatible API Key"), { target: { value: "local-key" } });
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("status")).toHaveTextContent("测试通过，设置已保存");
    expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      provider_keys: { custom: "local-key" },
    }));
  });

  it("warns and disables the primary action when a routed provider has no key", () => {
    renderEngine({
      cleanup_provider: "openai",
      provider_keys: {
        groq: { configured: true, hint: "••••yabcd" },
        openai: { configured: false, hint: null },
      },
    });
    expect(screen.getAllByText("未配置密钥").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "测试并应用" })).toBeDisabled();
    expect(screen.getByLabelText("OpenAI API Key")).toBeInTheDocument();
  });
  it("marks a newly selected provider pending while retaining actual use and key access", () => {
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    const selectedRow = screen.getByText("Deepgram", { selector: "p.text-sm" }).closest("div")?.parentElement;
    expect(selectedRow).toHaveTextContent("待应用");
    expect(selectedRow).not.toHaveTextContent("使用中");
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("Groq");
    expect(screen.getByLabelText("Deepgram API Key")).toBeInTheDocument();
    expect(screen.getByLabelText("Groq API Key")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /管理其他服务/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByLabelText("Deepgram API Key")).toBeVisible();
  });

  it.each(["assemblyai", "dashscope"])("tests and saves %s credentials without applying its draft route", async (provider) => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true, skipped: true } });
    renderEngine({}, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), provider);
    const key = screen.getByLabelText(provider === "assemblyai" ? "AssemblyAI API Key" : "DashScope · Qwen Audio API Key");
    fireEvent.change(key, { target: { value: "new-service-key" } });
    const row = key.closest("fieldset")!;
    fireEvent.click(within(row).getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith({ provider_keys: { [provider]: "new-service-key" } }));
    expect(commitEngine).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("Groq");
  });

  it("applies Soniox explicitly without pretending to probe a realtime connection", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    renderEngine({ asr_provider: "soniox", asr_model: "stt-rt-v4", cleanup_enabled: false, provider_keys: { soniox: { configured: true, hint: "saved-key" } } }, { commitEngine });
    fireEvent.click(screen.getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ asr_provider: "soniox", cleanup_enabled: false })));
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
    expect(screen.getByRole("status")).toHaveTextContent("实时连接尚未测试");
  });

  it("keeps AI off and the local cleanup preset pending until the common apply action", async () => {
    const save = vi.fn();
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true } });
    renderEngine({}, { save, commitEngine });
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    expect(save).not.toHaveBeenCalled();
    expect(commitEngine).not.toHaveBeenCalled();
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("OpenAI");
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: false })));
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "ollama");
    fireEvent.click(screen.getByRole("button", { name: "使用本机 Qwen3.5:4b 整理" }));
    expect(save).not.toHaveBeenCalled();
    expect(commitEngine).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "测试并应用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenLastCalledWith(expect.objectContaining({ cleanup_provider: "ollama", cleanup_model: "qwen3.5:4b", cleanup_enabled: true })));
  });

  it("leaves cloud routes blocked in strict offline mode while preserving the immediate safety toggle", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    renderEngine({ strict_offline_enabled: true }, { commitEngine });
    expect(screen.getByRole("button", { name: "测试并应用" })).toBeDisabled();
    fireEvent.click(screen.getByRole("switch", { name: "严格离线模式" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith({ strict_offline_enabled: false }));
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("can discard routing and credential drafts without changing saved settings", () => {
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    fireEvent.change(screen.getByLabelText("Deepgram API Key"), { target: { value: "pending-key" } });
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    fireEvent.click(screen.getByRole("button", { name: "放弃更改" }));
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("groq");
    expect(screen.getByRole("switch", { name: "AI 文字整理" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByRole("button", { name: "放弃更改" })).not.toBeInTheDocument();
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("refreshes an untouched draft when actual settings change without replacing a user's pending route", () => {
    const commitEngine = vi.fn();
    const initialProps = { ...unused, settings, save: vi.fn(), commitEngine };
    const result = render(<EngineSettings {...initialProps} />);
    result.rerender(<EngineSettings {...initialProps} settings={{ ...settings, asr_model: "whisper-large-v3" }} />);
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3-turbo");
    result.rerender(<EngineSettings {...initialProps} settings={{ ...settings, asr_model: "whisper-large-v3", cleanup_enabled: false }} />);
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3-turbo");
    expect(screen.getByRole("switch", { name: "AI 文字整理" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("whisper-large-v3");
    expect(screen.getByText("有待应用的更改；当前听写仍使用上方路线。")).toBeInTheDocument();
  });

  it("keeps an unused provider's connection failure separate from the current route", async () => {
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: false, error_kind: "key" }, cleanup: { ok: true } });
    renderEngine();
    fireEvent.click(screen.getByRole("button", { name: /管理其他服务/ }));
    fireEvent.click(screen.getByText("Deepgram", { selector: "p.text-sm" }));
    const key = screen.getByLabelText("Deepgram API Key");
    fireEvent.change(key, { target: { value: "invalid-key" } });
    fireEvent.click(within(key.closest("fieldset")!).getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Deepgram"));
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).toHaveTextContent("凭据已保存 · 本次尚未检查");
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).not.toHaveTextContent("密钥无效");
  });

  it("distinguishes a typed key from a saved credential and keeps deletion unavailable until saved", () => {
    renderEngine({ api_key_configured: false, api_key_hint: null, provider_keys: {} });
    fireEvent.change(screen.getByLabelText("Groq API Key"), { target: { value: "not-saved-yet" } });
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).toHaveTextContent("密钥待保存");
    expect(screen.queryByText("凭据已保存 · 本次尚未检查")).not.toBeInTheDocument();
    const row = screen.getByLabelText("Groq API Key").closest("fieldset")!;
    expect(within(row).queryByRole("button", { name: "删除密钥" })).not.toBeInTheDocument();
  });

  it("also labels a replacement for an existing key as pending", () => {
    renderEngine();
    fireEvent.change(screen.getByLabelText("Groq API Key"), { target: { value: "replacement-draft" } });
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).toHaveTextContent("密钥待保存");
  });

  it("prevents editing, testing, or applying while a key deletion is pending", async () => {
    let finish!: () => void;
    const removeProviderKey = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    renderEngine({}, { removeProviderKey });
    const key = screen.getByLabelText("Groq API Key");
    const row = key.closest("fieldset")!;
    fireEvent.click(within(row).getByRole("button", { name: "删除密钥" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "删除密钥" }));
    expect(key).toBeDisabled();
    expect(within(row).getByRole("button", { name: "测试连接" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "测试并应用" })).toBeDisabled();
    finish();
    await waitFor(() => expect(key).toBeEnabled());
  });

  it("shows a failed key deletion beside its service and retains the user's key draft", async () => {
    const removeProviderKey = vi.fn().mockRejectedValue(new Error("Keychain unavailable"));
    renderEngine({}, { removeProviderKey });
    const key = screen.getByLabelText("Groq API Key");
    const row = key.closest("fieldset")!;
    fireEvent.change(key, { target: { value: "preserve-my-draft" } });
    fireEvent.click(within(row).getByRole("button", { name: "删除密钥" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "删除密钥" }));
    expect(await within(row).findByRole("alert")).toHaveTextContent("密钥删除失败");
    expect(removeProviderKey).toHaveBeenCalledWith("groq");
    expect(key).toHaveValue("preserve-my-draft");
    expect(within(row).getByRole("button", { name: "删除密钥" })).toBeEnabled();
  });

  it("marks saved cloud routes as blocked while strict offline mode is enabled", () => {
    renderEngine({ strict_offline_enabled: true });
    expect(within(screen.getByLabelText("当前生效路线")).getAllByText("严格离线模式已阻止此路线")).toHaveLength(2);
  });
});
