import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(), listen: listenMock }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: () => Promise.resolve(() => undefined),
  }),
}));

const invokeMock = vi.mocked(invoke);

const settings = {
  schema_version: 4,
  api_key_configured: true,
  api_key_hint: "gsk_…abcd",
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "openai/gpt-oss-20b",
  language: "auto",
  ui_language: "zh" as const,
  theme: "system" as const,
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
};

describe("settings navigation", () => {
  beforeEach(() => {
    listenMock.mockImplementation((_event: string, _handler: unknown) => Promise.resolve(vi.fn()));
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (command === "check_permissions") return { microphone: false, microphone_status: "denied", accessibility: false };
      if (command === "get_audio_input_devices") return [{ name: "MacBook Pro Microphone", is_default: true }];
      if (command === "get_context_snapshot") return { profile: { id: "native.general", family: "general", app_label: "VoiceFlow", icon_key: "app", source: "fallback", confidence: 0.4 }, browser_access_status: "not_applicable" };
      if (command === "get_context_mappings") return [];
      if (command === "get_context_override") return null;
      if (command === "get_available_applications") return [];
      return undefined;
    });
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("opens the dedicated permissions page from the system navigation group", async () => {
    render(<App />);

    const permissionsNav = await screen.findByRole("button", { name: "系统权限" });
    expect(permissionsNav).not.toHaveAttribute("aria-current");

    fireEvent.click(permissionsNav);

    await waitFor(() => expect(screen.getByRole("heading", { name: "系统权限" })).toBeInTheDocument());
    expect(permissionsNav).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("麦克风")).toBeInTheDocument();
    expect(screen.getByText("自动粘贴")).toBeInTheDocument();
  });

  it("splits smart formatting and tone into separate settings pages", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("button", { name: "智能整理" }));
    expect(await screen.findByRole("heading", { name: "智能整理" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "语气" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "语气" }));
    expect(await screen.findByRole("heading", { name: "语气" })).toBeInTheDocument();
  });

  it("shows and lets the user choose the input device in system settings", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("button", { name: "系统设置" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "系统设置" })).toBeInTheDocument());

    const input = await screen.findByRole("combobox", { name: "输入设备" });
    expect(screen.getByRole("option", { name: "默认（跟随系统） · MacBook Pro Microphone" })).toBeInTheDocument();
    fireEvent.change(input, { target: { value: "MacBook Pro Microphone" } });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
      patch: { input_device: "MacBook Pro Microphone" },
    }));
  });

  it("puts automatic context and output formatting on the smart formatting page", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("button", { name: "智能整理" }));

    await waitFor(() => expect(screen.getByRole("heading", { name: "智能整理" })).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "上下文" })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "输出模式" })).toHaveValue("auto");
    expect(screen.getByRole("switch", { name: "App 上下文适配" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "语气 Prompt" })).not.toBeInTheDocument();
    expect(screen.queryByText("App / 网站映射")).not.toBeInTheDocument();
    expect(screen.getByText("自动根据当前 App、输入框和你说的内容选择整理方式；手动模式会覆盖自动判断。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "语气" }));
    expect(await screen.findByRole("heading", { name: "语气" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "语气 Prompt" })).toBeInTheDocument();
    expect(screen.getByText("App / 网站映射")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "录音与输出" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "录音与输出" })).toBeInTheDocument());
    expect(screen.queryByRole("combobox", { name: "输出模式" })).not.toBeInTheDocument();
  });

  it("moves theme and language into system settings", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "录音与输出" })).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "主题" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "语言" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "系统设置" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "系统设置" })).toBeInTheDocument());
    expect(screen.getByRole("combobox", { name: "主题" })).toHaveValue("system");
    expect(screen.getByRole("combobox", { name: "语言" })).toHaveValue("zh");
  });

  it("moves the menu bar icon setting into system settings", async () => {
    render(<App />);

    expect(await screen.findByRole("button", { name: "系统设置" })).toBeInTheDocument();
    expect(screen.queryByText("菜单栏图标")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "系统设置" }));

    await waitFor(() => expect(screen.getByRole("heading", { name: "系统设置" })).toBeInTheDocument());
    expect(screen.getByText("菜单栏图标")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "显示菜单栏图标" })).toHaveAttribute("aria-checked", "true");
  });

  it("shows selected-text actions as an independent opt-in setting", async () => {
    render(<App />);

    expect(await screen.findByText("选中文本操作")).toBeInTheDocument();
    expect(screen.getByText("选中文本快捷键")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "启用选中文本操作" })).toHaveAttribute("aria-checked", "false");
  });

  it("keeps selected-text actions enabled until a hotkey is assigned", async () => {
    render(<App />);

    const toggle = await screen.findByRole("switch", { name: "启用选中文本操作" });
    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("status")).toHaveTextContent("请先设置快捷键后才能触发");
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
      patch: { selected_actions_enabled: true },
    }));
  });

  it("opens the selected-text preview dialog from the native event", async () => {
    let previewHandler: ((event: { payload: { selected_text: string; transcript: string; final_text: string } }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: { selected_text: string; transcript: string; final_text: string } }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());

    previewHandler?.({
      payload: {
        selected_text: "原文",
        transcript: "请整理",
        final_text: "整理后的文本",
      },
    });

    expect(await screen.findByRole("dialog", { name: "预览选中文本操作" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("整理后的文本");
  });

  it("refreshes dictionary when backend settings change", async () => {
    let settingsHandler: ((event: { payload: typeof settings & { dictionary: string[] } }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: typeof settings & { dictionary: string[] } }) => void) => {
      if (event === "settings://changed") settingsHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "个人词典" }));
    expect(await screen.findByText(/还没有学到替换/)).toBeInTheDocument();
    await waitFor(() => expect(settingsHandler).toBeDefined());

    settingsHandler?.({ payload: { ...settings, dictionary: ["知乎"] } as typeof settings & { dictionary: string[] } });
    expect(await screen.findByText("知乎")).toBeInTheDocument();
  });

  it("lets the user choose activation mode and does not persist empty chunk values", async () => {
    render(<App />);

    expect(await screen.findByText("按一下切换")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /按一下切换/ })).toBeChecked();

    invokeMock.mockClear();
    const threshold = screen.getByRole("spinbutton", { name: "开始分段（秒）" });
    fireEvent.change(threshold, { target: { value: "" } });
    fireEvent.blur(threshold);
    expect(invokeMock).not.toHaveBeenCalledWith("update_settings_patch", expect.anything());

    fireEvent.change(threshold, { target: { value: "8" } });
    fireEvent.blur(threshold);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
      patch: { chunk_threshold_secs: 8 },
    }));
  });

  it("reloads history when dictation completes while the page stays open", async () => {
    let stateHandler: ((event: { payload: { state: string } }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: { state: string } }) => void) => {
      if (event === "dictation://state") stateHandler = handler;
      return Promise.resolve(vi.fn());
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (command === "check_permissions") return { microphone: false, microphone_status: "denied", accessibility: false };
      if (command === "get_history") return { items: [], has_more: false };
      return undefined;
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "历史记录" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "历史" })).toBeInTheDocument());
    await waitFor(() => expect(stateHandler).toBeDefined());

    const historyCallsBeforeCompletion = invokeMock.mock.calls.filter(([command]) => command === "get_history").length;
    stateHandler?.({ payload: { state: "copied" } });

    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "get_history")).toHaveLength(historyCallsBeforeCompletion + 1));
  });
});
