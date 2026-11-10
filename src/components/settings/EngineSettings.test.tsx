import { closeSelect, openSelect, selectOption } from "../../test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useState } from "react";
import type { LocalCleanupModelStatus, MlxCleanupModelStatus, Settings } from "../../types/settings";
import type { JsonObject } from "../../lib/customCleanupParameters";
import { I18nProvider, useI18n } from "../../lib/i18n";
import { PROVIDERS } from "../../lib/providers";
import { EngineSettings, type EnginePage } from "./EngineSettings";

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

const customJsonSaveCases: Array<{ label: string; parameters: JsonObject; source: string }> = [
  { label: "an empty object", parameters: {}, source: "{}" },
  {
    label: "the same values with reordered keys",
    parameters: { temperature: 0.2, max_tokens: 2048 },
    source: '{\n  "max_tokens": 2048,\n  "temperature": 0.2\n}',
  },
];

const localCleanupCatalog: LocalCleanupModelStatus[] = [
  {
    id: "qwen3.5-2b", label: "Qwen3.5 2B · Q4_K_M", repository: "lmstudio-community/Qwen3.5-2B-GGUF",
    revision: "rev-qwen2b", sha256: "sha-qwen2b", bytes: 1_270_808_032, state: "missing", operation: null,
    bytes_done: 0, bytes_total: 0, error: null, last_operation: null, last_operation_error: null,
    managed: false, verified: false, compatible: true, compatibility_error: null,
  },
  {
    id: "qwen3.5-4b", label: "Qwen3.5 4B · Q4_K_M", repository: "lmstudio-community/Qwen3.5-4B-GGUF",
    revision: "rev-qwen4b", sha256: "sha-qwen4b", bytes: 2_707_513_696, state: "missing", operation: null,
    bytes_done: 0, bytes_total: 0, error: null, last_operation: null, last_operation_error: null,
    managed: false, verified: false, compatible: true, compatibility_error: null,
  },
  {
    id: "gemma-4-e4b-it", label: "Gemma 4 E4B IT · Q4_0", repository: "ggml-org/gemma-4-E4B-it-GGUF",
    revision: "rev-gemma", sha256: "sha-gemma", bytes: 4_590_807_392, state: "missing", operation: null,
    bytes_done: 0, bytes_total: 0, error: null, last_operation: null, last_operation_error: null,
    managed: false, verified: false, compatible: true, compatibility_error: null,
  },
];

function localCleanupModels(overrides: Partial<LocalCleanupModelStatus>[] = []) {
  return localCleanupCatalog.map((model, index) => ({ ...model, ...overrides[index] }));
}

const mlxCleanupCatalog: MlxCleanupModelStatus[] = [
  {
    id: "qwen3_5_2b_mlx_4bit", label: "Qwen3.5 2B · MLX 4-bit", repository: "mlx-community/Qwen3.5-2B-4bit",
    revision: "rev-qwen-mlx-2b", bytes: 2_000_000_000,
    language: "Multilingual; product language coverage pending verification",
    state: "missing", operation: null, bytes_done: 0, bytes_total: 0, error: null,
    last_operation: null, last_operation_error: null, managed: false, partial_managed: false,
    verified: false, qualified: false, download_available: true, downloadable: true, selectable: false, compatible: true, compatibility_error: null,
  },
];

function mlxCleanupModels(overrides: Partial<MlxCleanupModelStatus>[] = []) {
  return mlxCleanupCatalog.map((model, index) => ({ ...model, ...overrides[index] }));
}

const enginePages: ReadonlyArray<{ id: EnginePage; label: "服务配置" | "API Key" | "本地模型" | "高级设置" }> = [
  { id: "service", label: "服务配置" },
  { id: "api_key", label: "API Key" },
  { id: "local_models", label: "本地模型" },
  { id: "advanced", label: "高级设置" },
];

function EngineSettingsTestHarness({ props }: { props: Parameters<typeof EngineSettings>[0] }) {
  const [page, setPage] = useState<EnginePage>("service");
  const { t } = useI18n();
  return (
    <>
      <nav aria-label={t("模型配置")}>
        {enginePages.map((item) => <button key={item.id} type="button" aria-current={page === item.id ? "page" : undefined} onClick={() => setPage(item.id)}>{t(item.label)}</button>)}
      </nav>
      <EngineSettings {...props} page={page} onPageChange={setPage} />
    </>
  );
}

function engineTestElement(props: Parameters<typeof EngineSettings>[0], language: "zh" | "en" = "zh") {
  return (
    <I18nProvider initialLanguage={language}>
      <EngineSettingsTestHarness props={props} />
    </I18nProvider>
  );
}

function renderEngineProps(props: Parameters<typeof EngineSettings>[0], language: "zh" | "en" = "zh") {
  return render(engineTestElement(props, language));
}

function renderEngine(overrides: Partial<Settings> = {}, handlers: Partial<Parameters<typeof EngineSettings>[0]> = {}, language: "zh" | "en" = "zh") {
  const previousInvoke = vi.mocked(invoke).getMockImplementation();
  if (previousInvoke) {
    vi.mocked(invoke).mockImplementation(async (command, args, options) => {
      const result = await previousInvoke(command, args, options);
      if (command === "list_on_device_models") return Array.isArray(result) ? result : [];
      if (command === "get_cleanup_model_store_status"
        && (typeof result !== "object" || result === null || typeof (result as { revision?: unknown }).revision !== "string")) {
        return undefined;
      }
      return result;
    });
  }
  const props: Parameters<typeof EngineSettings>[0] = {
    settings: { ...settings, ...overrides },
    commitEngine: handlers.commitEngine ?? vi.fn(),
    removeProviderKey: handlers.removeProviderKey ?? vi.fn(),
    ...handlers,
    page: "service",
    onPageChange: vi.fn(),
  };
  return renderEngineProps(props, language);
}

function activatePage(
  label: "服务配置" | "API Key" | "本地模型" | "Local models" | "高级设置",
  options: { expandCredentialRows?: boolean } = {},
) {
  const navigationItem = screen.getByRole("button", { name: label });
  fireEvent.click(navigationItem);
  if (label === "API Key" && options.expandCredentialRows !== false) {
    credentialPanel().querySelectorAll<HTMLElement>("[data-provider]").forEach((row) => {
      const expand = row.querySelector<HTMLButtonElement>('button[aria-expanded="false"]');
      if (expand) fireEvent.click(expand);
    });
  }
  return navigationItem;
}

function servicePanel() {
  return screen.getByRole("region", { name: "服务配置" });
}

function credentialPanel() {
  return screen.getByRole("region", { name: "API Key" });
}

function advancedPanel() {
  return screen.getByRole("region", { name: "高级设置" });
}

function modelsPanel() {
  return screen.getByRole("region", { name: /^(本地模型|Local models)$/ });
}

function openAdvancedConnections() {
  const summary = within(advancedPanel()).getByText("连接地址与高级请求参数");
  const disclosure = summary.closest("details");
  if (disclosure && !disclosure.open) fireEvent.click(summary);
  return within(advancedPanel());
}

function openAdvancedPresets() {
  const panel = openAdvancedConnections();
  const summary = panel.getByText("服务预设");
  const disclosure = summary.closest("details");
  if (disclosure && !disclosure.open) fireEvent.click(summary);
  return panel;
}

describe("EngineSettings form", () => {
  afterEach(() => {
    cleanup();
    vi.mocked(invoke).mockReset();
    vi.mocked(listen).mockReset().mockResolvedValue(() => undefined);
    vi.useRealTimers();
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
    expect(within(cleanupMenu).getByRole("option", { name: "VoiceFlow llama.cpp" })).toHaveAttribute("aria-disabled", "true");
    expect(within(cleanupMenu).queryByRole("option", { name: "LM Studio" })).not.toBeInTheDocument();
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
        return [{
          id: "sensevoice-small", label: "SenseVoice Small", bytes: 163_002_883, state: "ready",
          inference_ready: false, downloaded_bytes: 163_002_883, error: null, revision: null, file_count: 2,
          platform_supported: true, runtime_status: "legacy_no_runtime", loaded: false,
        }];
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
    await waitFor(() => {
      expect(screen.getByText("旧版模型文件没有 MLX 推理支持")).toBeVisible();
    });
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    activatePage("本地模型");
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).getByText("155.5 MB · 已下载")).toBeVisible();
    expect(within(modelsPanel()).getByText("旧版文件管理")).toBeVisible();
    expect(within(modelsPanel()).getByText("服务配置仍选用了旧版 SenseVoice；此模型不能用于本机听写。" )).toBeVisible();
    expect(within(modelsPanel()).queryByRole("button", { name: "转到转写模型选择" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).queryByRole("button", { name: "转到整理模型选择" })).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("lists every key-capable provider in one collapsed editor row", () => {
    renderEngine();
    activatePage("API Key", { expandCredentialRows: false });
    const rows = Array.from(credentialPanel().querySelectorAll<HTMLElement>("[data-provider]"));
    const keyCapableProviders = PROVIDERS.filter(({ id }) => !["on_device", "on_device_mlx", "on_device_llama_cpp"].includes(id));
    expect(rows.map((row) => row.dataset.provider)).toEqual(expect.arrayContaining(keyCapableProviders.map(({ id }) => id)));
    expect(rows).toHaveLength(keyCapableProviders.length);
    expect(new Set(rows.map((row) => row.dataset.provider)).size).toBe(rows.length);
    for (const provider of keyCapableProviders) {
      const row = credentialPanel().querySelector<HTMLElement>(`[data-provider="${provider.id}"]`)!;
      expect(row.querySelector<HTMLButtonElement>("button[aria-expanded]" )).toHaveAttribute("aria-expanded", "false");
      expect(row.querySelector<HTMLElement>(`#provider-key-${provider.id}`)).not.toBeVisible();
    }
    expect(credentialPanel().querySelector("[data-provider=ollama]")).toBeInTheDocument();
    expect(credentialPanel().querySelector("[data-provider=local_whisper]")).toBeInTheDocument();
    expect(credentialPanel().querySelector("[data-provider=custom]")).toBeInTheDocument();
    expect(credentialPanel().querySelector("[data-provider=on_device]")).not.toBeInTheDocument();
    expect(credentialPanel().querySelector("[data-provider=on_device_mlx]")).not.toBeInTheDocument();
    expect(credentialPanel().querySelector("[data-provider=on_device_llama_cpp]")).not.toBeInTheDocument();
    expect(within(credentialPanel()).queryByText("管理其他服务")).not.toBeInTheDocument();
  });

  it("keeps the overview and disclosure focus on entry, then guides a changed service to its missing key", async () => {
    renderEngine({ api_key_configured: false, api_key_hint: null, provider_keys: {} });
    activatePage("API Key", { expandCredentialRows: false });
    expect(screen.getByLabelText("Groq API Key")).not.toHaveFocus();
    const groqExpand = screen.getByRole("button", { name: /添加 Groq/ });
    groqExpand.focus();
    fireEvent.click(groqExpand);
    expect(groqExpand).toHaveFocus();
    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    expect(screen.getByRole("button", { name: "API Key" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();
    expect(screen.getByText("请先添加当前转写服务的 API Key。")).toBeVisible();
    await waitFor(() => expect(screen.getByLabelText("Deepgram API Key")).toHaveFocus());
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("retains a single key editor and its draft while moving a provider into and out of the active configuration", () => {
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    activatePage("API Key", { expandCredentialRows: false });
    const row = document.querySelector<HTMLElement>('[data-provider="deepgram"]')!;
    fireEvent.click(within(row).getByRole("button", { name: "添加 Deepgram" }));
    const key = screen.getByLabelText("Deepgram API Key");
    fireEvent.change(key, { target: { value: "draft-deepgram-key" } });
    fireEvent.click(within(row).getByRole("button", { name: "收起 Deepgram" }));
    expect(key).not.toBeVisible();
    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    activatePage("API Key");
    expect(screen.getAllByLabelText("Deepgram API Key")).toHaveLength(1);
    expect(screen.getByLabelText("Deepgram API Key")).toBe(key);
    expect(key).toBeVisible();
    expect(key).toHaveValue("draft-deepgram-key");
    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "groq");
    activatePage("API Key");
    expect(screen.getByLabelText("Deepgram API Key")).toBe(key);
    expect(key).toHaveValue("draft-deepgram-key");
    expect(commitEngine).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("fills Beijing DashScope Qwen3-ASR from the Advanced preset", () => {
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "custom");
    activatePage("高级设置");
    fireEvent.click(openAdvancedPresets().getByRole("button", { name: "阿里云百炼 Qwen3-ASR" }));
    activatePage("服务配置");
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("custom");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("qwen3-asr-flash");
    expect(screen.getByLabelText("AI 文字整理模型")).toHaveValue("gpt-4o-mini");
    activatePage("高级设置");
    expect(openAdvancedConnections().getByLabelText("自定义服务地址")).toHaveValue("https://dashscope.aliyuncs.com/compatible-mode/v1");
  });

  it("keeps the Custom cleanup model unchanged when applying an ASR preset", () => {
    renderEngine({
      asr_provider: "custom",
      cleanup_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      cleanup_model: "cleanup-model",
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    });
    activatePage("高级设置");
    fireEvent.click(openAdvancedPresets().getByRole("button", { name: "国际站 Qwen3-ASR" }));
    activatePage("服务配置");
    expect(screen.getByLabelText("AI 文字整理模型")).toHaveValue("cleanup-model");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("qwen3-asr-flash");
  });

  it("fills a local mlx-qwen3-asr sidecar from the Advanced preset", () => {
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "custom");
    activatePage("高级设置");
    fireEvent.click(openAdvancedPresets().getByRole("button", { name: "本机 Qwen3-ASR (MLX)" }));
    activatePage("服务配置");
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("custom");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("Qwen/Qwen3-ASR-0.6B");
    activatePage("高级设置");
    expect(openAdvancedConnections().getByLabelText("自定义服务地址")).toHaveValue("http://127.0.0.1:8765/v1");
  });

  it("fills Handy-compatible Whisper.cpp on the local Whisper provider", () => {
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "custom");
    activatePage("高级设置");
    fireEvent.click(openAdvancedPresets().getByRole("button", { name: "本机 Whisper.cpp" }));
    activatePage("服务配置");
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("local_whisper");
    expect(screen.getByLabelText("ASR 模型")).toHaveValue("ggml-large-v3-turbo.bin");
    activatePage("高级设置");
    expect(openAdvancedConnections().getByLabelText("本机 Whisper 地址")).toHaveValue("http://127.0.0.1:9000/v1");
  });

  it("explains that local Whisper uses an independently managed HTTP sidecar", () => {
    renderEngine({ asr_provider: "local_whisper", asr_model: "whisper-large-v3-turbo" });
    expect(screen.getByText("本机 Whisper 走本机 HTTP；先自行运行 whisper.cpp 或 speaches。"))
      .toBeInTheDocument();
  });

  it("labels the Custom route by its purpose in each service selector", async () => {
    renderEngine();
    const asrMenu = await openSelect(screen.getByRole("combobox", { name: "转写服务" }));
    expect(within(asrMenu).getByRole("option", { name: "自定义语音转写" })).toBeInTheDocument();
    closeSelect(asrMenu);
    const cleanupMenu = await openSelect(screen.getByRole("combobox", { name: "整理服务" }));
    expect(within(cleanupMenu).getByRole("option", { name: "自定义 AI 整理" })).toBeInTheDocument();
    closeSelect(cleanupMenu);
  });

  it("keeps Custom available in both selectors when saved purpose flags are false", () => {
    renderEngine({ custom_asr: false, custom_llm: false });
    const asr = screen.getByRole("combobox", { name: "转写服务" });
    selectOption(asr, "custom");
    expect(asr).toHaveValue("custom");
    activatePage("服务配置");
    const cleanup = screen.getByRole("combobox", { name: "整理服务" });
    selectOption(cleanup, "custom");
    expect(cleanup).toHaveValue("custom");
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

  it("separates route configuration, credentials, and the shared application action", () => {
    renderEngine();
    const service = servicePanel();
    const asrTitle = within(service).getByText("转写", { selector: "p.text-sm" });
    const asrSelect = within(service).getByRole("combobox", { name: "转写服务" });
    expect(asrTitle.compareDocumentPosition(asrSelect) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    activatePage("API Key");
    const keys = credentialPanel();
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(keys).toContainElement(within(groqRow).getByRole("button", { name: "测试转写" }));
    expect(keys).toContainElement(within(groqRow).getByRole("button", { name: "测试整理" }));
    expect(service).not.toContainElement(groqRow);
    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    const saveButton = screen.getByRole("button", { name: "保存并使用" });
    expect(saveButton.parentElement?.className).not.toMatch(/border/);
  });

  it("uses standard shell spacing on clean pages and flush sizing only with the save rail", () => {
    renderEngine();
    const shell = document.querySelector<HTMLElement>(".vf-settings-shell")!;
    const engine = document.querySelector<HTMLElement>(".vf-engine-settings")!;

    for (const page of enginePages) {
      activatePage(page.label);
      expect(shell).not.toHaveClass("vf-settings-shell--flush-bottom");
      expect(engine).not.toHaveClass("vf-engine-settings--with-footer");
    }

    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    expect(shell).toHaveClass("vf-settings-shell--flush-bottom");
    expect(engine).toHaveClass("vf-engine-settings--with-footer");
  });

  it("does not equate configured credentials with verified readiness", () => {
    renderEngine();
    const service = servicePanel();
    expect(within(service).getAllByText("凭据已保存 · 本次尚未检查")).toHaveLength(2);
    expect(within(service).queryByText("已就绪")).not.toBeInTheDocument();
    activatePage("API Key", { expandCredentialRows: false });
    const keys = credentialPanel();
    expect(within(keys).getAllByText("使用中")).toHaveLength(2);
    expect(within(credentialPanel()).getAllByText("未添加").length).toBeGreaterThan(0);
  });

  it("keeps model changes pending and the saved route visible until explicit application", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true } });
    renderEngine({}, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    expect(commitEngine).not.toHaveBeenCalled();
    expect(within(servicePanel()).getByLabelText("当前生效路线")).toHaveTextContent("whisper-large-v3-turbo");
    expect(screen.getByText("有待应用的更改；当前听写仍使用已保存路线。")).toBeInTheDocument();
    activatePage("API Key");
    fireEvent.click(within(document.querySelector('[data-provider="groq"]')!).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "asr", asr_model: "whisper-large-v3" }),
    })));
    expect(commitEngine).not.toHaveBeenCalled();
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ asr_model: "whisper-large-v3" })));
  });

  it("does not call a cleanup provider error an invalid key", async () => {
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: true },
      cleanup: { ok: false, skipped: false, error_kind: "provider", message: "HTTP status 503" },
    });
    renderEngine({
      cleanup_provider: "custom",
      custom_base_url: "https://router.example/v1",
      cleanup_model: "provider/model",
      provider_keys: {
        groq: { configured: true, hint: "••••yabcd" },
        custom: { configured: true, hint: "••••key" },
      },
    });
    activatePage("服务配置");
    fireEvent.click(within(servicePanel()).getByRole("button", { name: "测试整理" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("OpenAI Compatible：服务返回错误"));
    expect(screen.getByText("AI 文字整理").closest("div")).toHaveTextContent("服务返回错误");
    expect(screen.queryByText("密钥无效")).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "cleanup", cleanup_provider: "custom" }),
    }));
  });

  it("does not persist when the probe fails", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "model", message: "nope" },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({}, { commitEngine });
    activatePage("API Key");
    fireEvent.click(within(document.querySelector('[data-provider="groq"]')!).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("模型名不被这个接口接受"));
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("ignores an old probe result after the saved route changes away and back", async () => {
    type StageResult = { ok: boolean; skipped: boolean; error_kind?: string | null };
    type PendingProbeResult = { asr: StageResult; cleanup: StageResult };
    let resolveProbe!: (result: PendingProbeResult) => void;
    const pendingProbe = new Promise<PendingProbeResult>((resolve) => {
      resolveProbe = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "probe_engine_draft") return pendingProbe;
      if (command === "list_on_device_models") return [];
      return undefined;
    });
    const commitEngine = vi.fn();
    const initialProps: Parameters<typeof EngineSettings>[0] = {
      settings,
      removeProviderKey: vi.fn(),
      commitEngine,
      page: "service",
      onPageChange: vi.fn(),
    };
    const view = (snapshot: Settings) => engineTestElement({ ...initialProps, settings: snapshot });
    const result = render(view(settings));
    activatePage("API Key");
    fireEvent.click(within(document.querySelector('[data-provider="groq"]')!).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.anything()));

    activatePage("服务配置");
    result.rerender(view({ ...settings, asr_model: "whisper-large-v3" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3"));
    result.rerender(view(settings));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3-turbo"));

    await act(async () => resolveProbe({
      asr: { ok: false, skipped: false, error_kind: "key" },
      cleanup: { ok: true, skipped: true },
    }));
    expect(screen.queryByText("密钥无效")).not.toBeInTheDocument();
    expect(within(servicePanel()).getAllByText("凭据已保存 · 本次尚未检查")).toHaveLength(2);
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("keeps generic endpoint errors actionable when the model may be missing", async () => {
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: false, error_kind: "path", message: "HTTP status 404" },
      cleanup: { ok: true, skipped: true },
    });
    renderEngine();
    activatePage("API Key");
    fireEvent.click(within(document.querySelector('[data-provider="groq"]')!).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("请检查接口地址和模型 ID"));
  });

  it("tests a custom ASR draft without saving it, then applies it explicitly", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: true },
    });
    renderEngine({
      cleanup_provider: "groq",
      cleanup_model: "openai/gpt-oss-20b",
      provider_keys: { groq: { configured: true, hint: "••••yabcd" } },
    }, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "custom");
    activatePage("高级设置");
    fireEvent.change(openAdvancedConnections().getByLabelText("自定义服务地址"), { target: { value: "http://127.0.0.1:8000/v1" } });
    activatePage("服务配置");
    fireEvent.change(screen.getByLabelText("ASR 模型"), { target: { value: "whisper-1" } });
    activatePage("API Key");
    fireEvent.change(screen.getByLabelText("自定义服务 API Key"), { target: { value: "local-key" } });
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "asr", asr_provider: "custom", cleanup_enabled: false }),
    })));
    expect(commitEngine).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledTimes(1));
    expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      provider_keys: { custom: "local-key" },
    }));
  });

  it("tests custom cleanup independently while keeping its endpoint and key on separate pages", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: false, skipped: true },
      cleanup: { ok: true, skipped: false },
    });
    renderEngine({
      asr_provider: "openai",
      cleanup_provider: "custom",
      custom_base_url: "https://router.example/v1",
      cleanup_model: "provider/model",
      custom_cleanup_parameters: { temperature: 0.2 },
      provider_keys: {
        openai: { configured: false, hint: null },
        custom: { configured: true, hint: "••••key" },
      },
    }, { commitEngine });

    activatePage("API Key");
    const key = screen.getByLabelText("自定义服务 API Key");
    expect(key).toBeVisible();
    expect(screen.queryByLabelText("自定义服务地址")).not.toBeVisible();
    expect(screen.queryByLabelText("AI 文字整理模型")).not.toBeVisible();
    expect(within(credentialPanel()).queryByText("仅转写预设")).not.toBeInTheDocument();
    expect(within(credentialPanel()).queryByText("转写和 AI 整理共用此地址与 API Key；两个阶段分别使用各自的模型。")).not.toBeInTheDocument();
    activatePage("高级设置");
    expect(openAdvancedConnections().getByLabelText("自定义服务地址")).toHaveValue("https://router.example/v1");
    expect(within(advancedPanel()).getByText("转写和 AI 整理共用此地址与 API Key；两个阶段分别使用各自的模型。")).toBeInTheDocument();
    expect(screen.queryByLabelText("自定义服务 API Key")).not.toBeVisible();

    activatePage("服务配置");
    expect(screen.getByLabelText("AI 文字整理模型")).toHaveValue("provider/model");
    fireEvent.click(screen.getByRole("button", { name: "测试整理" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({
        probe_stage: "cleanup",
        cleanup_provider: "custom",
        custom_cleanup_parameters: { temperature: 0.2 },
      }),
    })));
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("does not show the shared save rail while testing a pristine saved provider", async () => {
    let finish!: (result: {
      asr: { ok: boolean; skipped: boolean };
      cleanup: { ok: boolean; skipped: boolean };
    }) => void;
    vi.mocked(invoke).mockReturnValue(new Promise((resolve) => { finish = resolve; }) as never);
    renderEngine();

    const testButton = screen.getByRole("button", { name: "测试转写" });
    fireEvent.click(testButton);
    await waitFor(() => expect(testButton).toHaveAttribute("aria-busy", "true"));
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();

    finish({ asr: { ok: true, skipped: false }, cleanup: { ok: true, skipped: true } });
    await waitFor(() => expect(testButton).toHaveAttribute("aria-busy", "false"));
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
  });

  it("shows busy state only on the requested Custom stage while the other stays disabled", async () => {
    let resolveProbe!: (value: {
      asr: { ok: boolean; skipped: boolean };
      cleanup: { ok: boolean; skipped: boolean };
    }) => void;
    const pendingProbe = new Promise<{
      asr: { ok: boolean; skipped: boolean };
      cleanup: { ok: boolean; skipped: boolean };
    }>((resolve) => { resolveProbe = resolve; });
    vi.mocked(invoke).mockReturnValue(pendingProbe as never);
    renderEngine({
      asr_provider: "custom",
      cleanup_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      cleanup_model: "provider/model",
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    });

    const asrButton = screen.getByRole("button", { name: "测试转写" });
    const cleanupButton = screen.getByRole("button", { name: "测试整理" });
    fireEvent.click(asrButton);
    await waitFor(() => expect(asrButton).toHaveAttribute("aria-busy", "true"));
    expect(asrButton).toHaveTextContent("测试中…");
    expect(cleanupButton).toHaveAttribute("aria-busy", "false");
    expect(cleanupButton).toHaveTextContent("测试整理");
    expect(cleanupButton).toBeDisabled();

    resolveProbe({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: true },
    });
    await waitFor(() => expect(asrButton).toHaveAttribute("aria-busy", "false"));
  });

  it("keeps ASR testing available with invalid cleanup JSON but blocks cleanup test and save", async () => {
    vi.mocked(invoke).mockResolvedValue({
      asr: { ok: true, skipped: false },
      cleanup: { ok: true, skipped: true },
    });
    renderEngine({
      asr_provider: "custom",
      cleanup_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      asr_model: "whisper-1",
      cleanup_model: "cleanup-model",
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    });
    activatePage("高级设置");
    openAdvancedConnections();
    fireEvent.change(screen.getByLabelText("高级请求参数 JSON"), { target: { value: "{ bad" } });
    activatePage("服务配置");
    expect(screen.getByRole("button", { name: "测试整理" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "测试转写" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "asr", asr_provider: "custom" }),
    })));
  });

  it("does not let invalid inactive Custom JSON block save and preserves the last valid parameters", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    const lastValidParameters = { temperature: 0.2 };
    renderEngine({
      cleanup_provider: "custom",
      custom_base_url: "http://127.0.0.1:8000/v1",
      cleanup_model: "cleanup-model",
      custom_cleanup_parameters: lastValidParameters,
      provider_keys: {
        groq: { configured: true, hint: "••••yabcd" },
        custom: { configured: true, hint: "••••key" },
      },
    }, { commitEngine });

    activatePage("高级设置");
    openAdvancedConnections();
    const parameters = screen.getByLabelText("高级请求参数 JSON");
    fireEvent.change(parameters, { target: { value: "{ bad" } });
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();

    activatePage("服务配置");
    const cleanupService = screen.getByRole("combobox", { name: "整理服务" });
    selectOption(cleanupService, "groq");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();

    selectOption(cleanupService, "custom");
    activatePage("高级设置");
    openAdvancedConnections();
    expect(screen.getByLabelText("高级请求参数 JSON")).toHaveValue("{ bad");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();

    activatePage("服务配置");
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      cleanup_enabled: false,
      custom_cleanup_parameters: lastValidParameters,
    })));
  });

  it.each(customJsonSaveCases)("clears valid custom JSON formatting from pending state after saving $label", async ({ parameters, source }) => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    const initial: Settings = {
      ...settings,
      cleanup_provider: "custom",
      custom_base_url: "https://router.example/v1",
      cleanup_model: "provider/model",
      custom_cleanup_parameters: parameters,
      provider_keys: {
        ...settings.provider_keys,
        custom: { configured: true, hint: "••••key" },
      },
    };
    const view = renderEngine(initial, { commitEngine });

    activatePage("高级设置");
    openAdvancedConnections();
    fireEvent.change(screen.getByLabelText("高级请求参数 JSON"), { target: { value: source } });
    activatePage("服务配置");
    fireEvent.change(screen.getByLabelText("AI 文字整理模型"), { target: { value: "provider/model-after-save" } });
    expect(screen.getByText("有待应用的更改；当前听写仍使用已保存路线。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      cleanup_model: "provider/model-after-save",
      custom_cleanup_parameters: parameters,
    })));

    view.rerender(engineTestElement({
      settings: { ...initial, cleanup_model: "provider/model-after-save", custom_cleanup_parameters: parameters },
      removeProviderKey: vi.fn(),
      commitEngine,
      page: "service",
      onPageChange: vi.fn(),
    }));
    await waitFor(() => expect(screen.queryByText("有待应用的更改；当前听写仍使用已保存路线。")).not.toBeInTheDocument());
  });

  it("shows a saved route's missing key in its provider row without an idle save footer", () => {
    renderEngine({
      cleanup_provider: "openai",
      provider_keys: {
        groq: { configured: true, hint: "••••yabcd" },
        openai: { configured: false, hint: null },
      },
    });
    expect(screen.getAllByText("未配置密钥").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    activatePage("API Key", { expandCredentialRows: false });
    const row = document.querySelector<HTMLElement>('[data-provider="openai"]')!;
    expect(within(row).getByRole("button", { name: "添加 OpenAI" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "添加 OpenAI" }));
    expect(screen.getByLabelText("OpenAI API Key")).toBeVisible();
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
  });

  it("keeps a saved Custom endpoint prerequisite inline without an idle save footer", () => {
    renderEngine({
      asr_provider: "custom",
      asr_model: "whisper-1",
      custom_base_url: "",
      cleanup_enabled: false,
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    });
    activatePage("高级设置");

    expect(within(advancedPanel()).getByText("请先填写自定义服务地址。")).toBeVisible();
    expect(within(advancedPanel()).getByRole("button", { name: "管理连接地址" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
  });
  it("marks a newly selected provider pending while retaining actual use and key access", () => {
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    expect(screen.getByRole("button", { name: "API Key" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByLabelText("Deepgram API Key")).toBeVisible();
    activatePage("服务配置");
    const selectedRow = screen.getByText("Deepgram", { selector: "p.text-sm" }).closest("div")?.parentElement;
    expect(selectedRow).toHaveTextContent("待应用");
    expect(selectedRow).not.toHaveTextContent("使用中");
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("Groq");
    activatePage("API Key");
    expect(screen.getByLabelText("Deepgram API Key")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "API Key" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByLabelText("Deepgram API Key")).toBeVisible();
  });

  it.each(["assemblyai", "dashscope"])("tests %s without saving its key or applying its draft route", async (provider) => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true, skipped: true } });
    renderEngine({}, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), provider);
    activatePage("API Key");
    const key = screen.getByLabelText(provider === "assemblyai" ? "AssemblyAI API Key" : "DashScope · Qwen Audio API Key");
    fireEvent.change(key, { target: { value: "new-service-key" } });
    const row = key.closest("fieldset")!;
    fireEvent.click(within(row).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ asr_provider: provider, probe_stage: "asr", provider_keys: { [provider]: "new-service-key" } }),
    })));
    expect(commitEngine).not.toHaveBeenCalled();
    activatePage("服务配置");
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("Groq");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: provider,
      provider_keys: { [provider]: "new-service-key" },
    })));
  });

  it("saves Soniox credentials through the shared action without a realtime settings probe", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    renderEngine({ asr_provider: "soniox", asr_model: "stt-rt-v4", cleanup_enabled: false, provider_keys: { soniox: { configured: true, hint: "saved-key" } } }, { commitEngine });
    activatePage("API Key");
    fireEvent.change(screen.getByLabelText("Soniox API Key"), { target: { value: "new-soniox-key" } });
    expect(within(credentialPanel()).getByText("实时连接在听写时建立；API Key 通过下方共用操作保存。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "保存密钥" })).not.toBeInTheDocument();
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      asr_provider: "soniox",
      cleanup_enabled: false,
      provider_keys: { soniox: "new-soniox-key" },
    })));
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
    expect(within(servicePanel()).getByRole("status")).toHaveTextContent("设置已保存");
  });

  it("keeps AI off and the local cleanup preset pending until the common apply action", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: true }, cleanup: { ok: true } });
    renderEngine({}, { commitEngine });
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    expect(commitEngine).not.toHaveBeenCalled();
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("OpenAI");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: false })));
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "ollama");
    activatePage("高级设置");
    fireEvent.click(openAdvancedConnections().getByRole("button", { name: "使用本机 Qwen3.5:4b 整理" }));
    expect(commitEngine).toHaveBeenCalledTimes(1);
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenLastCalledWith(expect.objectContaining({ cleanup_provider: "ollama", cleanup_model: "qwen3.5:4b", cleanup_enabled: true })));
  });

  it("shows storage guidance and retains download retry after an out-of-disk failure", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [{
        id: "qwen3-asr-0.6b", label: "Qwen3 ASR", state: "missing", inference_ready: false,
        error: "write failed: No space left on device", error_category: "disk_full",
        downloaded_bytes: 0, bytes: 100, sha256: null, revision: null, file_count: 1,
        platform_supported: true, runtime_status: "runtime_ready", loaded: false,
      }];
      return undefined;
    });
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" }, {}, "en");
    activatePage("Local models");
    expect(await within(modelsPanel()).findByText("Check available storage, then retry the download.")).toBeInTheDocument();
    expect(within(modelsPanel()).getByText("write failed: No space left on device")).toBeInTheDocument();
    const retry = screen.getByRole("button", { name: "Download model" });
    expect(retry).toBeEnabled();
    fireEvent.click(retry);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("download_on_device_model", { id: "qwen3-asr-0.6b" }));
  });

  it("explains local MLX cleanup and puts download before import for an unmanaged model", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "missing",
          operation: null,
          source_kind: null,
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: false,
          partial_managed: false,
          platform_supported: true,
        };
      }
      return undefined;
    });
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" }, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    expect(screen.getByRole("combobox", { name: "AI 文字整理模型" })).toHaveValue("qwen3_8_27b_mlx_6bit");
    expect(screen.queryByLabelText("VoiceFlow MLX API Key")).not.toBeInTheDocument();
    activatePage("本地模型");
    expect(await within(modelsPanel()).findByText(/未下载/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    const cleanupGroup = Array.from(modelsPanel().querySelectorAll<HTMLElement>(":scope > .vf-settings-group"))[1];
    expect(within(cleanupGroup).getByRole("heading", { name: "本机文字整理" })).toBeInTheDocument();
    expect(within(cleanupGroup).getByText("在这台 Mac 上调整转写文字和排版。")).toBeInTheDocument();
    expect(Array.from(cleanupGroup.querySelectorAll("button")).map((button) => button.textContent)).toContain("下载模型");
    expect(within(cleanupGroup).getByText("导入已有模型")).toBeVisible();
    expect(within(cleanupGroup).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    fireEvent.click(within(cleanupGroup).getByText("导入已有模型"));
    fireEvent.click(screen.getByRole("button", { name: "选择文件夹并导入" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("import_cleanup_model"));
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("offers model refresh when the initial status cannot be read, then shows supported actions", async () => {
    let statusReads = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        statusReads += 1;
        if (statusReads === 1) throw new Error("status read failed");
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "missing",
          operation: null,
          source_kind: null,
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: false,
          partial_managed: false,
          platform_supported: true,
        };
      }
      return undefined;
    });
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" });
    activatePage("本地模型");
    expect(await within(modelsPanel()).findByText("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。")).toHaveAttribute("role", "alert");
    expect(within(modelsPanel()).getByRole("button", { name: "刷新模型状态" })).toBeEnabled();
    expect(within(modelsPanel()).queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).queryByRole("button", { name: "选择文件夹并导入" })).not.toBeInTheDocument();

    fireEvent.click(within(modelsPanel()).getByRole("button", { name: "刷新模型状态" }));
    expect(await within(modelsPanel()).findByText(/未下载/)).toBeInTheDocument();
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).getByRole("button", { name: "下载模型" })).toBeEnabled();
    fireEvent.click(within(modelsPanel()).getByText("导入已有模型"));
    expect(within(modelsPanel()).getByRole("button", { name: "选择文件夹并导入" })).toBeEnabled();
  });

  it("keeps ASR-only probe and apply available when an unsupported native cleanup preference is unused", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "missing",
          operation: null,
          source_kind: null,
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: false,
          partial_managed: false,
          platform_supported: false,
        };
      }
      if (command === "probe_engine_draft") {
        return {
          asr: { ok: true, skipped: false },
          cleanup: { ok: true, skipped: true },
        };
      }
      return undefined;
    });
    renderEngine({
      cleanup_provider: "on_device_mlx",
      cleanup_model: "qwen3_8_27b_mlx_6bit",
      cleanup_enabled: false,
    }, { commitEngine });

    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "asr", cleanup_enabled: false }),
    })));
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "cleanup" }),
    }));

    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: false })));
  });

  it("keeps an existing cloud cleanup route ready when the VoiceFlow MLX platform is unsupported", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return { platform_supported: false };
      return undefined;
    });
    renderEngine({}, { commitEngine });

    expect(screen.getByRole("combobox", { name: "整理服务" })).toHaveValue("openai");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: true })));
  });

  it("keeps an existing Ollama cleanup probe and apply available on an unsupported VoiceFlow MLX platform", async () => {
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return { platform_supported: false };
      if (command === "probe_engine_draft") {
        return {
          asr: { ok: true, skipped: true },
          cleanup: { ok: true, skipped: false },
        };
      }
      return undefined;
    });
    renderEngine({
      cleanup_provider: "ollama",
      cleanup_model: "qwen3.5:4b",
      ollama_base_url: "http://127.0.0.1:11434/v1",
    }, { commitEngine });

    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3");
    fireEvent.click(within(servicePanel()).getByRole("button", { name: "测试整理" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ probe_stage: "cleanup", cleanup_provider: "ollama" }),
    })));
    expect(screen.getByRole("combobox", { name: "整理服务" })).toHaveValue("ollama");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({ cleanup_enabled: true })));
  });

  it("allows apply for an installed managed model and offers only model deletion", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "installed",
          operation: null,
          source_kind: null,
          bytes_done: 22_804_836_386,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: true,
          partial_managed: false,
          platform_supported: true,
        };
      }
      return undefined;
    });
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");

    expect(await screen.findByText("已安装；首次使用时会重新校验")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    activatePage("本地模型");
    const cleanupGroup = Array.from(modelsPanel().querySelectorAll<HTMLElement>(":scope > .vf-settings-group"))[1];
    expect(within(cleanupGroup).getByRole("button", { name: "删除模型: Qwen 3.8 27B" })).toBeEnabled();
    expect(within(cleanupGroup).queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    expect(within(cleanupGroup).queryByRole("button", { name: "选择文件夹并导入" })).not.toBeInTheDocument();
    expect(within(cleanupGroup).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
  });

  it.each([
    ["unsupported", false, "VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。"],
    ["unknown", null, "无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"],
    ["missing", undefined, "无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"],
  ] as const)("fails closed for %s VoiceFlow MLX platform status", async (_caseName, platformSupported, explanation) => {
    const status = {
      id: "qwen3_8_27b_mlx_6bit",
      label: "Qwen 3.8 27B · MLX 6-bit",
      repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
      revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
      state: "missing",
      operation: null,
      source_kind: null,
      bytes_done: 0,
      bytes_total: 22_804_836_386,
      error: null,
      last_operation: null,
      last_operation_error: null,
      managed: false,
      partial_managed: false,
      platform_supported: platformSupported,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return status;
      return undefined;
    });
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    activatePage("本地模型");

    expect(await within(modelsPanel()).findByText(explanation)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();
    fireEvent.click(within(modelsPanel()).getByText("导入已有模型"));
    expect(screen.getByRole("button", { name: "选择文件夹并导入" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下载模型" })).toBeDisabled();
    if (platformSupported === false) {
      expect(screen.queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    } else {
      expect(screen.getByRole("button", { name: "刷新模型状态" })).toBeEnabled();
    }

    fireEvent.click(screen.getByRole("button", { name: "选择文件夹并导入" }));
    fireEvent.click(screen.getByRole("button", { name: "下载模型" }));
    expect(invoke).not.toHaveBeenCalledWith("import_cleanup_model");
    expect(invoke).not.toHaveBeenCalledWith("download_cleanup_model");
  });

  it.each([
    ["unsupported", false, "VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。"],
    ["unknown", null, "无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"],
    ["missing", undefined, "无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"],
  ] as const)("keeps an existing model deletable but blocks route apply for %s platform capability", async (_caseName, platformSupported, explanation) => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "ready",
          operation: null,
          source_kind: null,
          bytes_done: 22_804_836_386,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: true,
          partial_managed: false,
          platform_supported: platformSupported,
        };
      }
      return undefined;
    });
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    activatePage("本地模型");

    expect(await within(modelsPanel()).findByText(explanation)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除模型: Qwen 3.8 27B" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "选择文件夹并导入" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
  });

  it("keeps model-operation cancellation available when the platform is unsupported", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "downloading",
          operation: "download",
          source_kind: "download",
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: false,
          partial_managed: false,
          platform_supported: false,
        };
      }
      return undefined;
    });
    renderEngine();
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    activatePage("本地模型");
    const cancel = await screen.findByRole("button", { name: "取消模型操作" });
    expect(cancel).toBeEnabled();
    fireEvent.click(cancel);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("cancel_cleanup_model_store_operation"));
  });

  it("preserves model action errors across status refreshes and clears only read errors", async () => {
    let statusReads = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        statusReads += 1;
        if (statusReads === 2) throw new Error("status read failed");
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state: "missing",
          operation: null,
          source_kind: null,
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: null,
          last_operation_error: null,
          managed: false,
          partial_managed: false,
          platform_supported: true,
        };
      }
      if (command === "import_cleanup_model") throw new Error("import failed");
      return undefined;
    });
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" });
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    activatePage("本地模型");
    await within(modelsPanel()).findByText(/未下载/);
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();

    fireEvent.click(within(modelsPanel()).getByText("导入已有模型"));
    fireEvent.click(screen.getByRole("button", { name: "选择文件夹并导入" }));
    expect(await within(modelsPanel()).findByText("本机 MLX 模型操作失败，请重试。")).toHaveAttribute("role", "alert");
    expect(await within(modelsPanel()).findByText("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。")).toHaveAttribute("role", "alert");
    expect(within(modelsPanel()).getByRole("button", { name: "刷新模型状态" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "刷新模型状态" }));
    await waitFor(() => expect(within(modelsPanel()).queryByText("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。")).not.toBeInTheDocument());
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).getByText("本机 MLX 模型操作失败，请重试。")).toHaveAttribute("role", "alert");
  });

  it("keeps model-status polling alive after a transient read failure and exposes recovery", async () => {
    vi.useFakeTimers();
    let statusReads = 0;
    const status = {
      id: "qwen3_8_27b_mlx_6bit",
      label: "Qwen 3.8 27B · MLX 6-bit",
      repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
      revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
      state: "downloading",
      operation: "download",
      source_kind: "download",
      bytes_done: 10,
      bytes_total: 22_804_836_386,
      error: null,
      last_operation: null,
      last_operation_error: null,
      managed: false,
      partial_managed: false,
      platform_supported: true,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        statusReads += 1;
        if (statusReads === 2) throw new Error("status read failed");
        return status;
      }
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(statusReads).toBe(1);
    expect(within(modelsPanel()).getByRole("button", { name: "取消模型操作" })).toBeEnabled();
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(900);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(statusReads).toBe(2);
    expect(within(modelsPanel()).getByRole("button", { name: "刷新模型状态" })).toBeEnabled();
    expect(within(modelsPanel()).getByRole("button", { name: "取消模型操作" })).toBeEnabled();

    fireEvent.click(within(modelsPanel()).getByRole("button", { name: "刷新模型状态" }));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(statusReads).toBe(3);
    expect(within(modelsPanel()).queryByRole("button", { name: "刷新模型状态" })).not.toBeInTheDocument();
    expect(within(modelsPanel()).getByRole("button", { name: "取消模型操作" })).toBeEnabled();
  });

  it.each([
    ["partial", "download", "21.2 GB · 下载未完成"],
    ["missing", "download", "21.2 GB · 未下载 · 已取消"],
    ["missing", "import", "21.2 GB · 未下载 · 已取消"],
  ] as const)("shows terminal copy for %s model operations", async (state, lastOperation, summary) => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") {
        return {
          id: "qwen3_8_27b_mlx_6bit",
          label: "Qwen 3.8 27B · MLX 6-bit",
          repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
          revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
          state,
          operation: null,
          source_kind: null,
          bytes_done: 0,
          bytes_total: 22_804_836_386,
          error: null,
          last_operation: lastOperation,
          last_operation_error: state === "missing" ? `${lastOperation} cancelled` : null,
          managed: false,
          partial_managed: state === "partial",
          platform_supported: true,
        };
      }
      return undefined;
    });
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" });
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    activatePage("本地模型");
    expect(await within(modelsPanel()).findByText(summary, { exact: true })).toBeVisible();
    if (lastOperation && state === "missing") {
      expect(within(modelsPanel()).queryByRole("alert")).not.toBeInTheDocument();
      expect(within(modelsPanel()).queryByText("上次模型操作失败；查看技术信息与错误。")).not.toBeInTheDocument();
    }
    expect(screen.queryByText("正在读取模型状态")).not.toBeInTheDocument();
    if (state === "partial") {
      expect(within(modelsPanel()).getByRole("button", { name: "删除模型: Qwen 3.8 27B" })).toBeEnabled();
      expect(within(modelsPanel()).getByRole("button", { name: "下载模型" })).toBeEnabled();
      fireEvent.click(within(modelsPanel()).getByText("导入已有模型"));
      expect(within(modelsPanel()).getByRole("button", { name: "选择文件夹并导入" })).toBeEnabled();
    }
  });

  it("leaves cloud routes blocked in strict offline mode while preserving the immediate safety toggle", async () => {
    const commitEngine = vi.fn().mockResolvedValue(undefined);
    renderEngine({ strict_offline_enabled: true }, { commitEngine });
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    expect(within(servicePanel()).getByText("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用")).toBeVisible();
    activatePage("高级设置");
    fireEvent.click(screen.getByRole("switch", { name: "严格离线模式" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith({ strict_offline_enabled: false }));
    expect(invoke).not.toHaveBeenCalledWith("probe_engine_draft", expect.anything());
  });

  it("can discard routing and credential drafts without changing saved settings", () => {
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "deepgram");
    activatePage("API Key");
    fireEvent.change(screen.getByLabelText("Deepgram API Key"), { target: { value: "pending-key" } });
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));
    fireEvent.click(screen.getByRole("button", { name: "放弃更改" }));
    expect(screen.getByRole("combobox", { name: "转写服务" })).toHaveValue("groq");
    expect(screen.getByRole("switch", { name: "AI 文字整理" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByRole("button", { name: "放弃更改" })).not.toBeInTheDocument();
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("refreshes an untouched draft when actual settings change without replacing a user's pending route", () => {
    const commitEngine = vi.fn();
    const initialProps: Parameters<typeof EngineSettings>[0] = {
      settings,
      removeProviderKey: vi.fn(),
      commitEngine,
      page: "service",
      onPageChange: vi.fn(),
    };
    const result = renderEngineProps(initialProps);
    result.rerender(engineTestElement({ ...initialProps, settings: { ...settings, asr_model: "whisper-large-v3" } }));
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "whisper-large-v3-turbo");
    result.rerender(engineTestElement({ ...initialProps, settings: { ...settings, asr_model: "whisper-large-v3", cleanup_enabled: false } }));
    expect(screen.getByRole("combobox", { name: "ASR 模型" })).toHaveValue("whisper-large-v3-turbo");
    expect(screen.getByRole("switch", { name: "AI 文字整理" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByLabelText("当前生效路线")).toHaveTextContent("whisper-large-v3");
    expect(screen.getByText("有待应用的更改；当前听写仍使用已保存路线。")).toBeInTheDocument();
  });

  it("clears a Custom key candidate when an external settings snapshot changes origin", async () => {
    const initialSettings = {
      ...settings,
      asr_provider: "custom" as const,
      custom_base_url: "https://router.example.test/v1",
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    };
    const props: Parameters<typeof EngineSettings>[0] = {
      settings: initialSettings,
      removeProviderKey: vi.fn(),
      commitEngine: vi.fn(),
      page: "service",
      onPageChange: vi.fn(),
    };
    const result = renderEngineProps(props);
    activatePage("API Key");
    const key = screen.getByLabelText("自定义服务 API Key");
    fireEvent.change(key, { target: { value: "candidate-for-old-origin" } });
    const sameOriginSnapshot = {
      ...initialSettings,
      custom_base_url: "https://router.example.test/compatible/v1",
    };
    result.rerender(engineTestElement({ ...props, settings: sameOriginSnapshot }));
    expect(key).toHaveValue("candidate-for-old-origin");
    const newOriginSnapshot = {
      ...sameOriginSnapshot,
      custom_base_url: "https://new-router.example.test/v1",
      asr_api_key_configured: false,
      asr_api_key_hint: null,
      cleanup_api_key_configured: false,
      cleanup_api_key_hint: null,
      provider_keys: { custom: { configured: false, hint: null } },
    };
    result.rerender(engineTestElement({ ...props, settings: newOriginSnapshot }));
    await waitFor(() => expect(key).toHaveValue(""));
    activatePage("高级设置");
    expect(openAdvancedConnections().getByLabelText("自定义服务地址")).toHaveValue("https://new-router.example.test/v1");
    result.rerender(engineTestElement({
      ...props,
      settings: {
        ...newOriginSnapshot,
        asr_api_key_configured: true,
        asr_api_key_hint: "••••new",
        provider_keys: { custom: { configured: true, hint: "••••new" } },
      },
    }));
    activatePage("API Key");
    expect(key).toHaveValue("••••new");
  });

  it("preserves a Custom key candidate for same-origin path edits and clears it when editing a new origin", () => {
    renderEngine({
      asr_provider: "custom",
      custom_base_url: "https://router.example.test/v1",
      provider_keys: { custom: { configured: true, hint: "••••key" } },
    });
    activatePage("API Key");
    const key = screen.getByLabelText("自定义服务 API Key");
    fireEvent.change(key, { target: { value: "candidate-key" } });
    activatePage("高级设置");
    const endpoint = openAdvancedConnections().getByLabelText("自定义服务地址");
    fireEvent.change(endpoint, { target: { value: "https://router.example.test/compatible/v1" } });
    activatePage("API Key");
    expect(key).toHaveValue("candidate-key");
    activatePage("高级设置");
    fireEvent.change(endpoint, { target: { value: "https://new-router.example.test/v1" } });
    activatePage("API Key");
    expect(key).toHaveValue("");
  });

  it("keeps an unused provider's connection failure separate from the current route", async () => {
    vi.mocked(invoke).mockResolvedValue({ asr: { ok: false, error_kind: "key" }, cleanup: { ok: true } });
    renderEngine();
    activatePage("API Key", { expandCredentialRows: false });
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    fireEvent.click(within(groqRow).getByRole("button", { name: /编辑 Groq/ }));
    fireEvent.change(screen.getByLabelText("Groq API Key"), { target: { value: "inactive-groq-draft" } });
    const deepgramRow = document.querySelector<HTMLElement>('[data-provider="deepgram"]')!;
    fireEvent.click(within(deepgramRow).getByRole("button", { name: "添加 Deepgram" }));
    const key = screen.getByLabelText("Deepgram API Key");
    fireEvent.change(key, { target: { value: "invalid-key" } });
    fireEvent.click(within(key.closest("fieldset")!).getByRole("button", { name: "测试转写" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Deepgram"));
    expect(invoke).toHaveBeenCalledWith("probe_engine_draft", expect.objectContaining({
      draft: expect.objectContaining({ provider_keys: { deepgram: "invalid-key" } }),
    }));
    activatePage("服务配置");
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).toHaveTextContent("密钥待保存");
    expect(screen.getByText("转写", { selector: "p.text-sm" }).closest("div")).not.toHaveTextContent("密钥无效");
  });

  it("distinguishes a typed key from a saved credential and keeps deletion unavailable until saved", () => {
    renderEngine({ api_key_configured: false, api_key_hint: null, provider_keys: {} });
    activatePage("API Key");
    fireEvent.change(screen.getByLabelText("Groq API Key"), { target: { value: "not-saved-yet" } });
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(within(groqRow).getByText("密钥待保存")).toBeInTheDocument();
    expect(within(groqRow).queryByText("凭据已保存 · 本次尚未检查")).not.toBeInTheDocument();
    const row = screen.getByLabelText("Groq API Key").closest("fieldset")!;
    expect(within(row).queryByRole("button", { name: "删除密钥" })).not.toBeInTheDocument();
  });

  it("also labels a replacement for an existing key as pending", () => {
    renderEngine();
    activatePage("API Key");
    fireEvent.change(screen.getByLabelText("Groq API Key"), { target: { value: "replacement-draft" } });
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(within(groqRow).getByText("使用中")).toBeInTheDocument();
    expect(within(groqRow).getByText("密钥待保存")).toBeInTheDocument();
  });

  it("prevents editing, testing, or applying while a key deletion is pending", async () => {
    let finish!: () => void;
    const removeProviderKey = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    renderEngine({}, { removeProviderKey });
    activatePage("API Key");
    const key = screen.getByLabelText("Groq API Key");
    const row = key.closest("fieldset")!;
    fireEvent.click(within(row).getByRole("button", { name: "删除密钥" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "删除密钥" }));
    expect(key).toBeDisabled();
    expect(within(row).getByRole("button", { name: "测试转写" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    finish();
    await waitFor(() => expect(key).toBeEnabled());
  });

  it("shows a failed key deletion beside its service and retains the user's key draft", async () => {
    const removeProviderKey = vi.fn().mockRejectedValue(new Error("Keychain unavailable"));
    renderEngine({}, { removeProviderKey });
    activatePage("API Key");
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

  it("cancels a pending foreground Groq keychain read when Settings unmounts", async () => {
    let finish!: (result: { status: "cancelled"; settings: null }) => void;
    const readProviderCredential = vi.fn((_provider: string, _requestId: string) =>
      new Promise<{ status: "cancelled"; settings: null }>((resolve) => { finish = resolve; }));
    const cancelProviderCredentialRead = vi.fn().mockResolvedValue(true);
    const view = renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential, cancelProviderCredentialRead },
    );
    activatePage("API Key", { expandCredentialRows: false });
    const action = screen.getByRole("button", { name: "从钥匙串重新读取" });

    fireEvent.click(action);
    await waitFor(() => expect(readProviderCredential).toHaveBeenCalledTimes(1));
    const requestId = readProviderCredential.mock.calls[0][1];
    expect(readProviderCredential).toHaveBeenCalledWith("groq", requestId);
    expect(action).toBeDisabled();
    expect(action).toHaveAttribute("aria-busy", "true");

    view.unmount();
    expect(cancelProviderCredentialRead).toHaveBeenCalledWith(requestId);
    finish({ status: "cancelled", settings: null });
  });

  it("cancels a pending Groq read when the provider draft changes", async () => {
    const readProviderCredential = vi.fn((_provider: string, _requestId: string) =>
      new Promise<{ status: "cancelled"; settings: null }>(() => undefined));
    const cancelProviderCredentialRead = vi.fn().mockResolvedValue(true);
    renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential, cancelProviderCredentialRead },
    );
    activatePage("API Key");
    fireEvent.click(screen.getByRole("button", { name: "从钥匙串重新读取" }));
    await waitFor(() => expect(readProviderCredential).toHaveBeenCalledTimes(1));
    const requestId = readProviderCredential.mock.calls[0][1];

    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "转写服务" }), "assemblyai");

    await waitFor(() => expect(cancelProviderCredentialRead).toHaveBeenCalledWith(requestId));
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(await within(groqRow).findByText("离开 API Key 页面后，读取请求已取消；请重新发起读取。")).toBeInTheDocument();
  });

  it("keeps an unchecked Groq credential neutral until Keychain confirms it is missing", async () => {
    const readProviderCredential = vi.fn()
      .mockResolvedValue({ status: "unavailable", settings: null });
    renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential },
    );
    activatePage("API Key");
    const row = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(within(row).getByText("未读取")).toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: "从钥匙串重新读取" }));
    expect(await within(row).findByText("待确认")).toBeInTheDocument();
    expect(await screen.findByText("钥匙串暂时不可读取；当前设置未更改。")).toBeInTheDocument();
    expect(within(row).queryByText("未添加")).not.toBeInTheDocument();
  });

  it("only labels Groq missing after an explicit Keychain missing result", async () => {
    const readProviderCredential = vi.fn()
      .mockResolvedValue({ status: "missing", settings: null });
    renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential },
    );
    activatePage("API Key");
    const row = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(within(row).getByText("未读取")).toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "从钥匙串重新读取" }));
    expect(await within(row).findByText("未添加")).toBeInTheDocument();
  });

  it("keeps the Groq re-read action visible without forcing its disclosure open", () => {
    renderEngine(
      {
        asr_provider: "assemblyai",
        cleanup_enabled: false,
        api_key_configured: false,
        api_key_hint: null,
        provider_keys: {},
      },
      { readProviderCredential: vi.fn().mockResolvedValue({ status: "stale", settings: null }) },
    );
    activatePage("API Key", { expandCredentialRows: false });
    const row = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    const disclosure = row.querySelector<HTMLButtonElement>("button[aria-expanded]")!;
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(within(row).getByRole("button", { name: "从钥匙串重新读取" })).toBeInTheDocument();
    fireEvent.click(disclosure);
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(disclosure);
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
  });

  it("reports a Keychain read as session-only", async () => {
    const readProviderCredential = vi.fn()
      .mockResolvedValue({ status: "session_configured", settings: null });
    renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential },
    );
    activatePage("API Key");

    fireEvent.click(screen.getByRole("button", { name: "从钥匙串重新读取" }));
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(await within(groqRow).findByText("钥匙串密钥已在本次打开期间读取；重启后可能需要再次授权。")).toBeInTheDocument();
    expect(screen.queryByText(/session-key|gsk_/i)).not.toBeInTheDocument();
  });

  it("offers a Custom reread for an unavailable saved route and keeps the credential out of UI text", async () => {
    const readProviderCredential = vi.fn().mockResolvedValue({
      status: "session_configured",
      settings: null,
    });
    renderEngine({
      asr_provider: "custom",
      custom_base_url: "https://router.example.test/v1",
      api_key_configured: false,
      provider_keys: { custom: { configured: true, keychain_state: "unavailable" } },
    }, { readProviderCredential });
    activatePage("API Key");
    const row = document.querySelector<HTMLElement>('[data-provider="custom"]')!;
    expect(within(row).getByText("本次会话有凭据 · 钥匙串暂时不可读取")).toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "从钥匙串重新读取" }));
    await waitFor(() => expect(readProviderCredential).toHaveBeenCalledTimes(1));
    const requestId = readProviderCredential.mock.calls[0][1];
    expect(readProviderCredential).toHaveBeenCalledWith("custom", requestId, "asr");
    expect(await within(row).findByRole("status")).toHaveTextContent(
      "自定义服务密钥已在本次打开期间读取；重启后可能需要再次授权。",
    );
    expect(document.body).not.toHaveTextContent("synthetic-session-only-key");
  });

  it("binds cleanup-only Custom recovery to cleanup and cancels it on unmount", async () => {
    const readProviderCredential = vi.fn((_provider: string, _requestId: string, _purpose?: string) =>
      new Promise<{ status: "cancelled"; settings: null }>(() => undefined));
    const cancelProviderCredentialRead = vi.fn().mockResolvedValue(true);
    const view = renderEngine({
      asr_provider: "groq",
      cleanup_enabled: true,
      cleanup_provider: "custom",
      custom_base_url: "https://router.example.test/v1",
      provider_keys: {
        groq: { configured: true },
        custom: { configured: false, keychain_state: "missing" },
      },
    }, { readProviderCredential, cancelProviderCredentialRead });
    activatePage("API Key");
    const row = document.querySelector<HTMLElement>('[data-provider="custom"]')!;
    const action = within(row).getByRole("button", { name: "从钥匙串重新读取" });
    fireEvent.click(action);
    await waitFor(() => expect(readProviderCredential).toHaveBeenCalledTimes(1));
    const requestId = readProviderCredential.mock.calls[0][1];
    expect(readProviderCredential).toHaveBeenCalledWith("custom", requestId, "cleanup");
    expect(action).toBeDisabled();
    expect(action).toHaveAttribute("aria-busy", "true");
    view.unmount();
    expect(cancelProviderCredentialRead).toHaveBeenCalledWith(requestId);
  });

  it("keeps a usable session credential when a later Custom Keychain read is unavailable", async () => {
    const readProviderCredential = vi.fn().mockResolvedValue({
      status: "unavailable",
      settings: null,
    });
    renderEngine({
      asr_provider: "custom",
      custom_base_url: "https://router.example.test/v1",
      api_key_configured: false,
      provider_keys: { custom: { configured: true, keychain_state: "unavailable" } },
    }, { readProviderCredential });
    activatePage("API Key");
    const row = document.querySelector<HTMLElement>('[data-provider="custom"]')!;
    fireEvent.click(within(row).getByRole("button", { name: "从钥匙串重新读取" }));
    expect(await within(row).findByText("本次会话有凭据 · 钥匙串暂时不可读取")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent("当前设置未更改");
  });

  it("does not describe a cancelled Keychain authorization as saved", async () => {
    const readProviderCredential = vi.fn().mockResolvedValue({ status: "cancelled", settings: null });
    renderEngine(
      { api_key_configured: false, api_key_hint: null, provider_keys: {} },
      { readProviderCredential },
    );
    activatePage("API Key");
    fireEvent.click(screen.getByRole("button", { name: "从钥匙串重新读取" }));
    const groqRow = document.querySelector<HTMLElement>('[data-provider="groq"]')!;
    expect(await within(groqRow).findByText("已取消钥匙串授权；当前设置未更改。")).toBeInTheDocument();
    expect(within(groqRow).getByText("待确认")).toBeInTheDocument();
    expect(within(groqRow).queryByText(/已保存|可持久使用/)).not.toBeInTheDocument();
  });

  it("marks saved cloud routes as blocked while strict offline mode is enabled", () => {
    renderEngine({ strict_offline_enabled: true });
    expect(within(screen.getByLabelText("当前生效路线")).getAllByText("严格离线模式已阻止此路线")).toHaveLength(2);
  });

  it.each([
    ["qwen3-asr-1.7b", "ready", "runtime_ready", false],
    ["qwen3-asr-1.7b", "ready", "loaded", true],
    ["cohere-transcribe-2b", "ready", "runtime_ready", false],
    ["cohere-transcribe-2b", "ready", "loaded", true],
    ["cohere-transcribe-2b", "corrupt", "runtime_ready", false],
  ] as const)("keeps existing %s files (%s/%s) deletable without a license warning", async (id, state, runtime_status, loaded) => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_on_device_models" ? [{
      id, label: id, state, inference_ready: state === "ready", platform_supported: true,
      runtime_status, loaded, downloaded_bytes: state === "ready" ? 100 : 0, bytes: 100,
      download_available: id === "qwen3-asr-1.7b",
      sha256: null, revision: null, file_count: 1,
    }] : undefined);
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" }, {}, "en");
    activatePage("Local models");
    await within(modelsPanel()).findByText(id);
    const row = within(modelsPanel()).getByText(id).closest<HTMLElement>("[data-model-row]");
    expect(row).not.toBeNull();
    expect(within(row!).queryByText("License unreviewed")).not.toBeInTheDocument();
    expect(within(row!).queryByText("The official model card declares Apache-2.0; the license for this fixed MLX conversion remains unreviewed.")).not.toBeInTheDocument();
    expect(within(row!).getByRole("button", { name: `Delete model: ${id}` })).toBeEnabled();
    expect(within(row!).queryByRole("button", { name: /Download model|Download again/ })).not.toBeInTheDocument();
    if (id === "cohere-transcribe-2b") {
      expect(within(row!).getByText("Choose Chinese or English. Automatic language detection is not supported.")).toBeVisible();
      expect(within(row!).getAllByText("Choose Chinese or English. Automatic language detection is not supported.")).toHaveLength(1);
    }
    fireEvent.click(within(row!).getByText("Technical details"));
    expect(within(row!).queryByText("The official model card declares Apache-2.0; the license for this fixed MLX conversion remains unreviewed.")).not.toBeInTheDocument();
  });

  it("offers Qwen3-ASR 1.7B for download and hides retired local models with no files", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_on_device_models" ? [
      { id: "qwen3-asr-0.6b", label: "Qwen3 ASR 0.6B", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 100, bytes: 100 },
      { id: "qwen3-asr-1.7b", label: "Qwen3 ASR 1.7B", state: "missing", inference_ready: false, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 0, bytes: 100, download_available: true },
      { id: "cohere-transcribe-2b", label: "Cohere Transcribe 2B", state: "missing", inference_ready: false, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 0, bytes: 100, download_available: false },
    ] : undefined);
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" }, {}, "en");
    activatePage("Local models");
    await within(modelsPanel()).findByText("Qwen3 ASR 0.6B");
    const qwen17 = within(modelsPanel()).getByText("Qwen3 ASR 1.7B").closest<HTMLElement>("[data-model-row]")!;
    expect(within(qwen17).getByRole("button", { name: "Download model" })).toBeEnabled();
    expect(within(modelsPanel()).queryByText("Cohere Transcribe 2B")).not.toBeInTheDocument();
    expect(within(modelsPanel()).queryByText("License unreviewed")).not.toBeInTheDocument();
  });

  it("distinguishes the saved local ASR model from a pending model choice", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_on_device_models" ? [
      { id: "qwen3-asr-0.6b", label: "Qwen3 ASR 0.6B", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 100, bytes: 100 },
      { id: "qwen3-asr-1.7b", label: "Qwen3 ASR 1.7B", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 200, bytes: 200 },
    ] : undefined);
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-1.7b", cleanup_enabled: false });
    activatePage("本地模型");

    const panel = modelsPanel();
    await within(panel).findByText("Qwen3 ASR 1.7B");
    const savedRow = within(panel).getByText("Qwen3 ASR 1.7B").closest<HTMLElement>("[data-model-row]");
    expect(savedRow).not.toBeNull();
    expect(within(savedRow!).getByText("服务配置已选")).toBeVisible();
    expect(within(panel).queryByText(/当前草稿|未选择/)).not.toBeInTheDocument();

    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "ASR 模型" }), "qwen3-asr-0.6b");
    activatePage("本地模型");
    const pendingRow = within(panel).getByText("Qwen3 ASR 0.6B").closest<HTMLElement>("[data-model-row]");
    expect(pendingRow).not.toBeNull();
    expect(within(pendingRow!).getByText("待应用")).toBeVisible();
    expect(within(savedRow!).getByText("服务配置已选")).toBeVisible();
  });

  it("keeps a failed cleanup operation visible when its last-operation error is the only failure", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return {
        id: "qwen3_8_27b_mlx_6bit", label: "Qwen 3.8 27B · MLX 6-bit",
        repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit", revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
        state: "missing", operation: null, source_kind: null, bytes_done: 0, bytes_total: 22_804_836_386,
        error: null, last_operation: "delete", last_operation_error: "delete failed",
        managed: false, partial_managed: false, platform_supported: true,
      };
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    expect(await within(modelsPanel()).findByRole("alert")).toHaveTextContent("上次模型操作失败；查看技术信息与错误。");
    fireEvent.click(within(modelsPanel()).getByText("查看技术信息与错误"));
    expect(within(modelsPanel()).getByText("delete failed")).toBeVisible();
  });

  it("shows a current cleanup validation error instead of a stale cancellation", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return {
        id: "qwen3_8_27b_mlx_6bit", label: "Qwen 3.8 27B · MLX 6-bit",
        repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit", revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
        state: "invalid", operation: null, source_kind: null, bytes_done: 0, bytes_total: 22_804_836_386,
        error: "model inventory changed", last_operation: "download", last_operation_error: "download cancelled",
        managed: false, partial_managed: false, platform_supported: true,
      };
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    expect(await within(modelsPanel()).findByRole("alert")).toHaveTextContent("模型状态有错误；查看技术信息与错误。");
    const row = within(modelsPanel()).getByText("Qwen 3.8 27B").closest<HTMLElement>(".vf-settings-row");
    expect(row).not.toBeNull();
    expect(within(row!).getByText("21.2 GB · 文件校验失败")).toBeVisible();
    expect(within(row!).queryByText(/已取消/)).not.toBeInTheDocument();
    fireEvent.click(within(row!).getByText("查看技术信息与错误"));
    expect(within(row!).getByText("model inventory changed")).toBeVisible();
    expect(within(row!).queryByText("download cancelled")).not.toBeInTheDocument();
  });

  it("shows local model management in two peer settings surfaces with flat model rows", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [{
        id: "sensevoice-small", label: "SenseVoice Small", state: "ready", inference_ready: false,
        runtime_status: "legacy_no_runtime", loaded: false, downloaded_bytes: 100, bytes: 100,
      }];
      if (command === "get_cleanup_model_store_status") return {
        id: "qwen3_8_27b_mlx_6bit", label: "Qwen 3.8 27B · MLX 6-bit",
        repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit", revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
        state: "missing", operation: null, source_kind: null, bytes_done: 0, bytes_total: 22_804_836_386,
        error: null, last_operation: null, last_operation_error: null, managed: false, partial_managed: false,
        platform_supported: true,
      };
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    const panel = modelsPanel();
    await within(panel).findByText("Qwen 3.8 27B");
    const groups = Array.from(panel.querySelectorAll<HTMLElement>(":scope > .vf-settings-group"));
    expect(groups).toHaveLength(2);
    expect(groups.map((group) => group.querySelector("h2")?.textContent)).toEqual(["本机转写", "本机文字整理"]);
    expect(within(groups[0]).getByText("把录音变成文字。")).toBeInTheDocument();
    expect(within(groups[1]).getByText("在这台 Mac 上调整转写文字和排版。")).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "转到转写模型选择" })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "转到整理模型选择" })).not.toBeInTheDocument();
    expect(within(panel).getByText("旧版文件管理")).toBeVisible();
    expect(within(panel).getByText("SenseVoice Small")).not.toBeVisible();
    expect(within(panel).queryByText(/当前草稿|未选择/)).not.toBeInTheDocument();
    fireEvent.click(within(panel).getByText("旧版文件管理"));
    expect(within(panel).getByText("SenseVoice Small")).toBeVisible();
    const legacyRow = within(panel).getByText("SenseVoice Small").closest<HTMLElement>("[data-model-row]")!;
    expect(within(legacyRow).getByRole("button", { name: "删除模型: SenseVoice Small" })).toBeEnabled();
    expect(within(legacyRow).queryByRole("button", { name: /下载模型|重新下载/ })).not.toBeInTheDocument();
    expect(await within(panel).findByText("Qwen 3.8 27B")).toBeVisible();
    for (const group of groups) {
      expect(group.querySelectorAll(".vf-settings-group-body")).toHaveLength(1);
      expect(group.querySelector(".vf-settings-group-body section")).not.toBeInTheDocument();
      expect(group.querySelector(".vf-settings-group-body div.rounded-md.border, .vf-settings-group-body div.rounded-xl.border, .vf-settings-group-body div.rounded-lg.bg-elevated, .vf-settings-group-body div.bg-card\\/35")).not.toBeInTheDocument();
    }
  });

  it("hides legacy SenseVoice management once no files remain and it is not selected", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_on_device_models" ? [
      { id: "qwen3-asr-0.6b", label: "Qwen3 ASR 0.6B", state: "ready", inference_ready: true, platform_supported: true, runtime_status: "runtime_ready", loaded: false, downloaded_bytes: 100, bytes: 100 },
      { id: "sensevoice-small", label: "SenseVoice Small", state: "missing", inference_ready: false, runtime_status: "legacy_no_runtime", loaded: false, downloaded_bytes: 0, bytes: 100 },
    ] : undefined);
    renderEngine({ asr_provider: "on_device", asr_model: "qwen3-asr-0.6b" });
    activatePage("本地模型");
    await within(modelsPanel()).findByText("Qwen3 ASR 0.6B");
    expect(within(modelsPanel()).queryByText("旧版文件管理")).not.toBeInTheDocument();
    expect(within(modelsPanel()).queryByText("SenseVoice Small")).not.toBeInTheDocument();
  });

  it("lists installed local cleanup models with their pinned sizes and compatibility reasons", async () => {
    const models = localCleanupModels([
      { state: "installed", managed: true, verified: true, compatible: false, compatibility_error: "uncalibrated_memory" },
      { state: "installed", managed: true, verified: true, compatible: true },
      { state: "installed", managed: true, verified: true, compatible: true },
    ]);
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_local_cleanup_models"
      ? models
      : command === "list_on_device_models" ? [] : undefined);
    renderEngine();
    activatePage("本地模型");

    const panel = modelsPanel();
    const qwen2 = await within(panel).findByText("Qwen3.5 2B · Q4_K_M");
    const qwen4 = within(panel).getByText("Qwen3.5 4B · Q4_K_M");
    const gemma = within(panel).getByText("Gemma 4 E4B IT · Q4_0");
    const qwen2Row = qwen2.closest<HTMLElement>("[data-model-row]")!;
    const qwen4Row = qwen4.closest<HTMLElement>("[data-model-row]")!;
    const gemmaRow = gemma.closest<HTMLElement>("[data-model-row]")!;

    expect(within(qwen2Row).getByText("1.2 GB · 已下载并校验")).toBeVisible();
    expect(within(qwen2Row).getByText("此模型的最低内存档尚未完成验证。")).toBeVisible();
    expect(within(qwen4Row).getByText("2.5 GB · 已下载并校验")).toBeVisible();
    expect(within(gemmaRow).getByText("4.3 GB · 已下载并校验")).toBeVisible();
    expect(within(gemmaRow).queryByText("通用整理 · 适用语言和模式按模型分别验收。")).not.toBeInTheDocument();
    fireEvent.click(within(gemmaRow).getByText("技术信息"));
    expect(within(gemmaRow).queryByText("逐款验收中；当前尚无已验证语言或模式列表。")).not.toBeInTheDocument();
    expect(within(gemmaRow).getByText(/Apple M1 · 16 GB；待实机验证/)).toBeVisible();
    expect(within(gemmaRow).getByText(/该模型最低支持档尚待实机验收；兼容状态按本机检查结果显示。/)).toBeVisible();
    for (const row of [qwen2Row, qwen4Row, gemmaRow]) {
      expect(within(row).queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    }
  });

  it("keeps download progress and cancellation scoped to one model without selecting it", async () => {
    let currentModels = localCleanupModels();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_local_cleanup_models") return currentModels;
      if (command === "download_local_cleanup_model") {
        expect(args).toEqual({ id: "qwen3.5-2b" });
        currentModels = currentModels.map((model) => model.id === "qwen3.5-2b"
          ? { ...model, state: "downloading", operation: "download", managed: true, partial_managed: true, bytes_done: 65_536, bytes_total: model.bytes }
          : model);
        return undefined;
      }
      if (command === "cancel_local_cleanup_model_download") {
        expect(args).toEqual({ id: "qwen3.5-2b" });
        currentModels = currentModels.map((model) => model.id === "qwen3.5-2b"
          ? { ...model, state: "missing", operation: null, partial_managed: false, bytes_done: 0, bytes_total: 0, last_operation: "download", last_operation_error: "download cancelled" }
          : model);
        return true;
      }
      return undefined;
    });
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    activatePage("本地模型");

    const panel = modelsPanel();
    const qwen2Row = (await within(panel).findByText("Qwen3.5 2B · Q4_K_M")).closest<HTMLElement>("[data-model-row]")!;
    fireEvent.click(within(qwen2Row).getByRole("button", { name: "下载模型" }));
    expect(await within(qwen2Row).findByRole("progressbar", { name: "模型下载进度: Qwen3.5 2B · Q4_K_M" })).toBeVisible();
    expect(within(qwen2Row).getByText("64.0 KB / 1.2 GB")).toBeVisible();
    expect(within(qwen2Row).getByRole("button", { name: "取消下载" })).toBeEnabled();
    expect(within(qwen2Row).queryByRole("button", { name: /删除模型|清理未完成文件/ })).not.toBeInTheDocument();
    expect(within(panel).getByText("Qwen3.5 4B · Q4_K_M").closest("[data-model-row]")).toHaveTextContent("2.5 GB · 尚未下载");

    fireEvent.click(within(qwen2Row).getByRole("button", { name: "取消下载" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("cancel_local_cleanup_model_download", { id: "qwen3.5-2b" }));
    await waitFor(() => expect(within(qwen2Row).getByText("1.2 GB · 尚未下载 · 已取消")).toBeVisible());
    activatePage("服务配置");
    expect(screen.getByRole("combobox", { name: "整理服务" })).toHaveValue("openai");
    expect(screen.queryByRole("button", { name: "保存并使用" })).not.toBeInTheDocument();
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("offers only installed verified compatible llama.cpp cleanup models and saves an explicit choice", async () => {
    const models = localCleanupModels([
      { state: "installed", managed: true, verified: true, compatible: true, bytes_total: localCleanupCatalog[0].bytes },
      { state: "installed", managed: true, verified: false, compatible: true, bytes_total: localCleanupCatalog[1].bytes },
      { state: "missing", managed: false, partial_managed: true, verified: false, compatible: true, bytes_done: 262_144, bytes_total: localCleanupCatalog[2].bytes },
    ]);
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_local_cleanup_models"
      ? models
      : command === "list_on_device_models" ? [] : undefined);
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_local_cleanup_models"));
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_llama_cpp");

    const cleanupActions = screen.getByRole("button", { name: "管理模型" }).parentElement!;
    expect(within(cleanupActions).queryByRole("button", { name: "管理凭据" })).not.toBeInTheDocument();
    expect(within(cleanupActions).getByRole("button", { name: "管理模型" })).toBeVisible();
    const modelSelector = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    const menu = await openSelect(modelSelector);
    expect(within(menu).getByRole("option", { name: "Qwen3.5 2B · Q4_K_M · 1.2 GB" })).toBeInTheDocument();
    expect(within(menu).queryByRole("option", { name: /Qwen3.5 4B/ })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("option", { name: /Gemma 4 E4B/ })).not.toBeInTheDocument();
    closeSelect(menu);
    selectOption(modelSelector, "qwen3.5-2b");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      cleanup_provider: "on_device_llama_cpp",
      cleanup_model: "qwen3.5-2b",
    })));
  });

  it("requires an explicit model choice when only one verified local cleanup model is available", async () => {
    const models = localCleanupModels([
      { state: "installed", managed: true, verified: true, compatible: true, bytes_total: localCleanupCatalog[0].bytes },
      {},
      {},
    ]);
    vi.mocked(invoke).mockImplementation(async (command) => command === "list_local_cleanup_models"
      ? models
      : command === "list_on_device_models" ? [] : undefined);
    const commitEngine = vi.fn();
    renderEngine({}, { commitEngine });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_local_cleanup_models"));
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_llama_cpp");

    const modelSelector = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    expect(modelSelector).toHaveTextContent("请选择已下载并验证的模型");
    const menu = await openSelect(modelSelector);
    expect(within(menu).getAllByRole("option", { name: "Qwen3.5 2B · Q4_K_M · 1.2 GB" })).toHaveLength(1);
    closeSelect(menu);

    selectOption(modelSelector, "qwen3.5-2b");
    expect(modelSelector).toHaveTextContent("Qwen3.5 2B · Q4_K_M · 1.2 GB");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      cleanup_provider: "on_device_llama_cpp",
      cleanup_model: "qwen3.5-2b",
    })));
  });

  it("shows partial download bytes and lets the user clean interrupted files", async () => {
    let currentModels = localCleanupModels([
      {},
      {},
      { partial_managed: true, bytes_done: 262_144, bytes_total: localCleanupCatalog[2].bytes },
    ]);
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_local_cleanup_models") return currentModels;
      if (command === "delete_local_cleanup_model") {
        expect(args).toEqual({ id: "gemma-4-e4b-it" });
        currentModels = currentModels.map((model) => model.id === "gemma-4-e4b-it"
          ? { ...model, partial_managed: false, bytes_done: 0 }
          : model);
      }
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    const panel = modelsPanel();
    const row = (await within(panel).findByText("Gemma 4 E4B IT · Q4_0")).closest<HTMLElement>("[data-model-row]")!;
    expect(within(row).getByText("256.0 KB / 4.3 GB · 下载未完成")).toBeVisible();
    expect(within(row).getByText("下载已中断；可以重新下载或清理部分文件。")).toBeVisible();
    expect(within(row).getByRole("button", { name: "下载模型" })).toBeEnabled();
    fireEvent.click(within(row).getByRole("button", { name: "清理未完成文件: Gemma 4 E4B IT · Q4_0" }));

    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "清理未完成模型文件" })).toBeVisible();
    expect(within(dialog).getByRole("button", { name: "清理文件" })).toBeVisible();
    fireEvent.click(within(dialog).getByRole("button", { name: "清理文件" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("delete_local_cleanup_model", { id: "gemma-4-e4b-it" }));
    await waitFor(() => expect(within(row).queryByRole("button", { name: /清理未完成文件/ })).not.toBeInTheDocument());
  });

  it("prevents deleting a selected model and keeps deletion errors on the affected model", async () => {
    const models = localCleanupModels([
      { state: "installed", managed: true, verified: true, compatible: true, bytes_total: localCleanupCatalog[0].bytes },
      { state: "installed", managed: true, verified: true, compatible: true, bytes_total: localCleanupCatalog[1].bytes },
      {},
    ]);
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_local_cleanup_models") return models;
      if (command === "delete_local_cleanup_model") {
        if ((args as { id: string }).id === "qwen3.5-4b") throw new Error("disk deletion failed");
      }
      return undefined;
    });
    renderEngine({ cleanup_provider: "on_device_llama_cpp", cleanup_model: "qwen3.5-2b" });
    activatePage("本地模型");

    const panel = modelsPanel();
    const qwen2Row = (await within(panel).findByText("Qwen3.5 2B · Q4_K_M")).closest<HTMLElement>("[data-model-row]")!;
    const qwen4Row = within(panel).getByText("Qwen3.5 4B · Q4_K_M").closest<HTMLElement>("[data-model-row]")!;
    expect(within(qwen2Row).getByRole("button", { name: "删除模型: Qwen3.5 2B · Q4_K_M" })).toBeDisabled();

    fireEvent.click(within(qwen4Row).getByRole("button", { name: "删除模型: Qwen3.5 4B · Q4_K_M" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "删除模型" }));
    expect(await within(qwen4Row).findByRole("alert")).toHaveTextContent("删除模型失败: disk deletion failed");
    expect(within(qwen2Row).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps native MLX models separate from GGUF and follows backend download eligibility", async () => {
    let currentModels = mlxCleanupModels();
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_local_cleanup_models") return localCleanupModels([
        { state: "installed", managed: true, verified: true, compatible: true },
      ]);
      if (command === "list_mlx_cleanup_models") return currentModels;
      if (command === "get_cleanup_model_store_status") return {
        id: "qwen3_8_27b_mlx_6bit", label: "Qwen 3.8 27B · MLX 6-bit", repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit",
        revision: "rev-27b", state: "missing", operation: null, source_kind: null,
        bytes_done: 0, bytes_total: 22_804_836_386, error: null, last_operation: null, last_operation_error: null,
        managed: false, partial_managed: false, platform_supported: true,
      };
      if (command === "download_mlx_cleanup_model") {
        expect(args).toEqual({ id: "qwen3_5_2b_mlx_4bit" });
        currentModels = currentModels.map((model) => model.id === "qwen3_5_2b_mlx_4bit"
          ? { ...model, state: "downloading", operation: "download", managed: true, partial_managed: true, bytes_done: 65_536, bytes_total: model.bytes }
          : model);
      }
      if (command === "cancel_mlx_cleanup_model_download") {
        expect(args).toEqual({ id: "qwen3_5_2b_mlx_4bit" });
        currentModels = currentModels.map((model) => model.id === "qwen3_5_2b_mlx_4bit"
          ? { ...model, state: "missing", operation: null, partial_managed: false, bytes_done: 0, bytes_total: 0, last_operation: "download", last_operation_error: "download cancelled" }
          : model);
        return true;
      }
      return undefined;
    });
    renderEngine({}, { commitEngine });
    activatePage("本地模型");

    const panel = modelsPanel();
    expect(within(panel).getByRole("heading", { name: "本机文字整理" })).toBeVisible();
    await within(panel).findByText("Qwen3.5 2B · Q4_K_M");
    expect(within(panel).getByText("Qwen3.5 2B · Q4_K_M")).toBeVisible();
    expect(await within(panel).findByText("Qwen 3.8 27B")).toBeVisible();
    const ggufRow = within(panel).getByText("Qwen3.5 2B · Q4_K_M").closest<HTMLElement>("[data-model-row]")!;
    expect(within(ggufRow).queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    const row = within(panel).getByText("Qwen3.5 2B · MLX 4-bit").closest<HTMLElement>("[data-model-row]")!;
    expect(row).not.toHaveTextContent("多语言；产品语言覆盖尚待验证");
    expect(row).not.toHaveTextContent("尚未通过发布验收");
    expect(row).not.toHaveTextContent("此模型尚未完成发布验收；当前状态不代表已验证的整理质量。");
    expect(row).not.toHaveTextContent("此模型尚未完成完整整理质量评测；文件校验不代表质量评测通过。");
    expect(within(row).getByRole("button", { name: "下载模型" })).toBeEnabled();

    fireEvent.click(within(row).getByRole("button", { name: "下载模型" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("download_mlx_cleanup_model", { id: "qwen3_5_2b_mlx_4bit" }));
    expect(await within(row).findByRole("progressbar", { name: "模型下载进度: Qwen3.5 2B · MLX 4-bit" })).toBeVisible();
    expect(within(row).getByText("64.0 KB / 1.9 GB")).toBeVisible();
    fireEvent.click(within(row).getByRole("button", { name: "取消下载" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("cancel_mlx_cleanup_model_download", { id: "qwen3_5_2b_mlx_4bit" }));
    await waitFor(() => expect(within(row).getByText("1.9 GB · 尚未下载 · 已取消")).toBeVisible());
    activatePage("服务配置");
    expect(screen.getByRole("combobox", { name: "整理服务" })).toHaveValue("openai");
    expect(commitEngine).not.toHaveBeenCalled();
  });

  it("updates the matching native MLX row from the status event", async () => {
    let statusHandler: ((event: { payload: MlxCleanupModelStatus }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "mlx-cleanup-model://status") {
        statusHandler = handler as unknown as (event: { payload: MlxCleanupModelStatus }) => void;
      }
      return () => undefined;
    });
    const model = mlxCleanupModels()[0];
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return [model];
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    const row = (await within(modelsPanel()).findByText(model.label)).closest<HTMLElement>("[data-model-row]")!;
    expect(statusHandler).toBeDefined();
    act(() => statusHandler?.({ payload: {
      ...model, state: "downloading", operation: "download", managed: true, partial_managed: true,
      bytes_done: 65_536, bytes_total: model.bytes,
    } }));
    expect(await within(row).findByRole("progressbar", { name: `模型下载进度: ${model.label}` })).toBeVisible();
    expect(within(row).getByRole("button", { name: "取消下载" })).toBeEnabled();
  });

  it("verifies installed MLX files with indeterminate progress and applies completed status events", async () => {
    let statusHandler: ((event: { payload: MlxCleanupModelStatus }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "mlx-cleanup-model://status") {
        statusHandler = handler as unknown as (event: { payload: MlxCleanupModelStatus }) => void;
      }
      return () => undefined;
    });
    const failedModel: MlxCleanupModelStatus = {
      ...mlxCleanupCatalog[0], id: "qwen3_5_4b_mlx_4bit", label: "Qwen3.5 4B · MLX 4-bit",
      state: "installed", managed: true, partial_managed: false, downloadable: false, verified: false, selectable: false,
    };
    let currentModels = [
      { ...mlxCleanupCatalog[0], state: "installed", managed: true, partial_managed: false, downloadable: false },
      failedModel,
    ];
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return currentModels;
      if (command === "verify_mlx_cleanup_model") {
        expect(args).toEqual({ id: "qwen3_5_2b_mlx_4bit" });
        currentModels = currentModels.map((model) => model.id === "qwen3_5_2b_mlx_4bit"
          ? { ...model, state: "installed", operation: "verify", bytes_done: 0, bytes_total: model.bytes }
          : model);
      }
      return undefined;
    });
    renderEngine();
    activatePage("本地模型");

    const panel = modelsPanel();
    const row = (await within(panel).findByText("Qwen3.5 2B · MLX 4-bit")).closest<HTMLElement>("[data-model-row]")!;
    expect(within(row).getByRole("button", { name: "校验模型" })).toBeEnabled();
    expect(within(row).queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "校验模型" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("verify_mlx_cleanup_model", { id: "qwen3_5_2b_mlx_4bit" }));
    const verificationProgress = await within(row).findByRole("progressbar", { name: "模型校验进度: Qwen3.5 2B · MLX 4-bit" });
    expect(verificationProgress).toBeVisible();
    expect(verificationProgress).not.toHaveAttribute("max");
    expect(verificationProgress).not.toHaveAttribute("value");
    expect(within(row).getByRole("button", { name: "正在校验…" })).toBeDisabled();

    expect(statusHandler).toBeDefined();
    act(() => statusHandler?.({ payload: {
      ...currentModels[0], state: "ready", operation: null, verified: true, selectable: true,
      bytes_done: currentModels[0].bytes, bytes_total: currentModels[0].bytes,
    } }));
    expect(await within(row).findByText("可用于服务配置")).toBeVisible();
    expect(within(row).queryByRole("button", { name: "校验模型" })).not.toBeInTheDocument();

    const failedRow = within(panel).getByText(failedModel.label).closest<HTMLElement>("[data-model-row]")!;
    act(() => statusHandler?.({ payload: {
      ...failedModel, state: "corrupt", operation: null, error: "pinned file hash mismatch", verified: false, selectable: false,
    } }));
    await waitFor(() => expect(failedRow).toHaveTextContent("文件校验失败"));
    expect(within(failedRow).getByRole("alert")).toHaveTextContent("pinned file hash mismatch");

    activatePage("服务配置");
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");
    const modelSelector = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    const menu = await openSelect(modelSelector);
    expect(within(menu).getByRole("option", { name: "Qwen3.5 2B · MLX 4-bit · 1.9 GB" })).toBeInTheDocument();
    expect(within(menu).queryByRole("option", { name: /Qwen3\.5 4B · MLX/ })).not.toBeInTheDocument();
  });

  it("offers a backend-selectable developer MLX model and saves it without release qualification", async () => {
    const models = mlxCleanupModels([{
      state: "ready", managed: true, verified: true, qualified: false, downloadable: false,
      selectable: true, compatible: true, bytes_done: mlxCleanupCatalog[0].bytes, bytes_total: mlxCleanupCatalog[0].bytes,
    }]);
    const commitEngine = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return models;
      if (command === "get_cleanup_model_store_status") return { platform_supported: false, managed: false, state: "missing", operation: null };
      return undefined;
    });
    renderEngine({}, { commitEngine });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_mlx_cleanup_models"));
    selectOption(screen.getByRole("combobox", { name: "整理服务" }), "on_device_mlx");

    const modelSelector = screen.getByRole("combobox", { name: "AI 文字整理模型" });
    const menu = await openSelect(modelSelector);
    expect(within(menu).getByRole("option", { name: "Qwen3.5 2B · MLX 4-bit · 1.9 GB" })).toBeInTheDocument();
    closeSelect(menu);
    selectOption(modelSelector, "qwen3_5_2b_mlx_4bit");
    expect(screen.getByRole("button", { name: "保存并使用" })).toBeEnabled();
    activatePage("本地模型");
    const row = within(modelsPanel()).getByText("Qwen3.5 2B · MLX 4-bit").closest<HTMLElement>("[data-model-row]")!;
    expect(within(row).queryByText("尚未通过发布验收")).not.toBeInTheDocument();
    expect(within(row).getByText("可用于服务配置")).toBeVisible();
    expect(within(row).queryByText("此模型尚未完成发布验收；当前状态不代表已验证的整理质量。")).not.toBeInTheDocument();
    activatePage("服务配置");
    fireEvent.click(screen.getByRole("button", { name: "保存并使用" }));
    await waitFor(() => expect(commitEngine).toHaveBeenCalledWith(expect.objectContaining({
      cleanup_provider: "on_device_mlx",
      cleanup_model: "qwen3_5_2b_mlx_4bit",
    })));
  });

  it("focuses the selected MLX candidate from Manage and the blocked Save action", async () => {
    const model = mlxCleanupModels([{
      state: "installed", managed: true, verified: false, qualified: false, downloadable: false,
      selectable: false, compatible: true, bytes_done: 0, bytes_total: mlxCleanupCatalog[0].bytes,
    }]);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return model;
      return undefined;
    });
    renderEngine({
      cleanup_enabled: false,
      cleanup_provider: "on_device_mlx",
      cleanup_model: "qwen3_5_2b_mlx_4bit",
    });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_mlx_cleanup_models"));
    fireEvent.click(screen.getByRole("switch", { name: "AI 文字整理" }));

    const blockedSave = screen.getByText("请先安装并验证 VoiceFlow MLX 整理模型。").parentElement!;
    fireEvent.click(within(blockedSave).getByRole("button", { name: "管理模型" }));
    await waitFor(() => expect(document.activeElement).toHaveAttribute("id", "mlx-cleanup-model-qwen3_5_2b_mlx_4bit"));
  });

  it("focuses the MLX catalog fallback when the saved model is absent", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return [];
      return undefined;
    });
    renderEngine({ cleanup_provider: "on_device_mlx", cleanup_model: "qwen3_5_2b_mlx_4bit" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_mlx_cleanup_models"));
    fireEvent.click(within(servicePanel()).getAllByRole("button", { name: "管理模型" })[0]);
    await waitFor(() => expect(document.activeElement).toHaveAttribute("id", "local-mlx-cleanup-models"));
  });

  it("keeps the legacy 27B Manage model focus target", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "list_on_device_models") return [];
      if (command === "get_cleanup_model_store_status") return {
        id: "qwen3_8_27b_mlx_6bit", label: "Qwen 3.8 27B · MLX 6-bit",
        repository: "lmstudio-community/Qwen3.8-27B-MLX-6bit", revision: "rev-27b",
        state: "missing", operation: null, source_kind: null, bytes_done: 0, bytes_total: 22_804_836_386,
        error: null, last_operation: null, last_operation_error: null, managed: false, partial_managed: false,
        platform_supported: true,
      };
      return undefined;
    });
    renderEngine({ cleanup_provider: "on_device_mlx", cleanup_model: "qwen3_8_27b_mlx_6bit" });
    fireEvent.click(within(servicePanel()).getAllByRole("button", { name: "管理模型" })[0]);
    await waitFor(() => expect(document.activeElement).toHaveAttribute("id", "local-cleanup-model"));
  });

  it("protects deletion only for the saved active native MLX model", async () => {
    const secondModel: MlxCleanupModelStatus = {
      ...mlxCleanupCatalog[0], id: "qwen3_5_4b_mlx_4bit", label: "Qwen3.5 4B · MLX 4-bit", bytes: 3_000_000_000,
      state: "installed", operation: null, managed: true, verified: true, qualified: false,
      downloadable: false, selectable: true, bytes_done: 3_000_000_000, bytes_total: 3_000_000_000,
    };
    let currentModels = [
      { ...mlxCleanupCatalog[0], state: "installed", managed: true, verified: true, selectable: true, downloadable: false, bytes_done: mlxCleanupCatalog[0].bytes, bytes_total: mlxCleanupCatalog[0].bytes },
      secondModel,
    ];
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_on_device_models") return [];
      if (command === "list_mlx_cleanup_models") return currentModels;
      if (command === "delete_mlx_cleanup_model") {
        expect(args).toEqual({ id: "qwen3_5_4b_mlx_4bit" });
        currentModels = currentModels.filter((model) => model.id !== "qwen3_5_4b_mlx_4bit");
      }
      return undefined;
    });
    renderEngine({ cleanup_provider: "on_device_mlx", cleanup_model: "qwen3_5_2b_mlx_4bit" });
    activatePage("本地模型");

    const panel = modelsPanel();
    const activeRow = (await within(panel).findByText("Qwen3.5 2B · MLX 4-bit")).closest<HTMLElement>("[data-model-row]")!;
    const otherRow = within(panel).getByText("Qwen3.5 4B · MLX 4-bit").closest<HTMLElement>("[data-model-row]")!;
    expect(within(activeRow).getByRole("button", { name: "删除模型: Qwen3.5 2B · MLX 4-bit" })).toBeDisabled();
    expect(within(otherRow).getByRole("button", { name: "删除模型: Qwen3.5 4B · MLX 4-bit" })).toBeEnabled();

    fireEvent.click(within(otherRow).getByRole("button", { name: "删除模型: Qwen3.5 4B · MLX 4-bit" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "删除模型" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("delete_mlx_cleanup_model", { id: "qwen3_5_4b_mlx_4bit" }));
    await waitFor(() => expect(within(panel).queryByText("Qwen3.5 4B · MLX 4-bit")).not.toBeInTheDocument());
    expect(within(panel).getByText("Qwen3.5 2B · MLX 4-bit")).toBeVisible();
  });

  it("does not mark the saved VoiceFlow MLX cleanup route blocked in strict offline mode", () => {
    renderEngine({
      asr_provider: "on_device",
      asr_model: "qwen3-asr-0.6b",
      cleanup_provider: "on_device_mlx",
      cleanup_model: "qwen3_8_27b_mlx_6bit",
      strict_offline_enabled: true,
    });
    const currentRoutes = screen.getByLabelText("当前生效路线");
    expect(currentRoutes).toHaveTextContent(/当前使用 · VoiceFlow MLX/);
    expect(within(currentRoutes).queryByText("严格离线模式已阻止此路线")).not.toBeInTheDocument();
  });
});
