import { closeSelect, openSelect, selectOption } from "./test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { SelectedActionPreview, TextActionOutcome } from "./components/SelectedPreviewDialog";
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
let selectedActionOutcome: TextActionOutcome = "replaced";
const selectedPreviewPayload: SelectedActionPreview = {
  transaction_id: "tx-selected-1",
  action_sequence: 1,
  kind: "selected",
  operation: "rewrite",
  target_kind: "selection",
  target_label: "current_field",
  source_text: "原文",
  instruction: "请整理",
  delivery_mode: "replace_or_copy",
  delivery_notice: "copy_if_target_changed",
  selected_text: "原文",
  transcript: "请整理",
  final_text: "整理后的文本",
  replace_allowed: true,
};

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

function captureNativeEvents() {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  listenMock.mockImplementation((event: string, handler: (event: { payload: unknown }) => void) => {
    handlers.set(event, handler);
    return Promise.resolve(vi.fn());
  });
  return {
    emit: (event: string, payload: unknown) => act(() => handlers.get(event)?.({ payload })),
    has: (event: string) => handlers.has(event),
  };
}

describe("settings navigation", () => {
  beforeEach(() => {
    selectedActionOutcome = "replaced";
    listenMock.mockImplementation((_event: string, _handler: unknown) => Promise.resolve(vi.fn()));
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (command === "check_permissions") return { microphone: false, microphone_status: "denied", accessibility: false };
      if (command === "get_audio_input_devices") return [{ name: "MacBook Pro Microphone", is_default: true }];
      if (command === "get_context_snapshot") return { profile: { id: "native.general", family: "general", app_label: "VoiceFlow", icon_key: "app", source: "fallback", confidence: 0.4 }, browser_access_status: "not_applicable" };
      if (command === "get_context_mappings") return [];
      if (command === "get_context_override") return null;
      if (command === "get_available_applications") return [];
      if (command === "confirm_selected_action_preview" || command === "confirm_screen_action_preview") return selectedActionOutcome;
      if (command === "copy_selected_action_preview" || command === "copy_screen_action_preview") return "copied";
      return undefined;
    });
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("shows actual OS login state when registration differs from stored settings", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "get_settings") return { ...settings, autostart_enabled: true };
      if (command === "get_autostart_status") return { enabled: false, error: null };
      return base(command, args, options);
    });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "系统设置" }));
    expect(await screen.findByText("系统登录启动状态与设置不同，请重新切换开关。")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "开机启动" })).toHaveAttribute("aria-checked", "false");
  });

  it("loads settings when optional release notes fail", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "get_whats_new_status") throw new Error("notes unavailable");
      return base(command, args, options);
    });
    render(<App />);
    expect(await screen.findByRole("button", { name: "系统设置" })).toBeInTheDocument();
    expect(screen.queryByText("notes unavailable")).not.toBeInTheDocument();
  });

  it("keeps a failed learning undo visible and retries without claiming success", async () => {
    const native = captureNativeEvents();
    const base = invokeMock.getMockImplementation()!;
    let failed = true;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "undo_learn_pair") {
        if (failed) throw new Error("database unavailable");
        return settings;
      }
      return base(command, args, options);
    });
    render(<App />);
    await screen.findByRole("button", { name: "系统设置" });
    native.emit("learn_pairs://promoted", { pair_key: "pair", before: "old", after: "new" });
    fireEvent.click(screen.getByRole("button", { name: "撤销" }));
    expect(await screen.findByText("撤销未完成，请重试。")).toBeInTheDocument();
    failed = false;
    fireEvent.click(screen.getByRole("button", { name: "重试撤销" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "重试撤销" })).not.toBeInTheDocument());
    expect(invokeMock.mock.calls.filter(([command]) => command === "undo_learn_pair")).toHaveLength(2);
  });

  it("ignores the debug shortcut in editing, composing and repeated key events", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "系统设置" });
    const input = document.createElement("input");
    document.body.append(input);
    fireEvent.keyDown(input, { key: "D", metaKey: true, shiftKey: true });
    input.remove();
    fireEvent.keyDown(window, { key: "D", metaKey: true, shiftKey: true, repeat: true });
    fireEvent.keyDown(window, { key: "D", metaKey: true, shiftKey: true, isComposing: true });
    expect(screen.queryByRole("heading", { name: "调试设置" })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "D", metaKey: true, shiftKey: true });
    expect(await screen.findByRole("heading", { name: "调试设置" })).toBeInTheDocument();
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
    const devices = await openSelect(input);
    expect(screen.getByRole("option", { name: "默认（跟随系统）" })).toBeInTheDocument();
    closeSelect(devices);
    expect(screen.getByText("当前麦克风 · MacBook Pro Microphone")).toBeInTheDocument();
    selectOption(input, "MacBook Pro Microphone");

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
      patch: { input_device: "MacBook Pro Microphone" },
    }));
    expect(screen.getByText("选定麦克风 · MacBook Pro Microphone")).toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
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
    fireEvent.click(screen.getByRole("button", { name: /更多快捷键/ }));
    expect(screen.getByRole("switch", { name: "启用选中文本操作" })).toHaveAttribute("aria-checked", "false");
  });

  it("keeps selected-text actions enabled until a hotkey is assigned", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("button", { name: /更多快捷键/ }));
    const toggle = screen.getByRole("switch", { name: "启用选中文本操作" });
    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("status")).toHaveTextContent("请先设置快捷键后才能触发");
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
      patch: { selected_actions_enabled: true },
    }));
  });

  it("opens the selected-text preview dialog from the native event", async () => {
    let previewHandler: ((event: { payload: SelectedActionPreview }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: SelectedActionPreview }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());

    previewHandler?.({ payload: selectedPreviewPayload });

    expect(await screen.findByRole("dialog", { name: "文字操作预览" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("整理后的文本");
  });

  it("sends a manually edited preview with its transaction ID and reports the backend outcome", async () => {
    let previewHandler: ((event: { payload: SelectedActionPreview }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: SelectedActionPreview }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());
    previewHandler?.({ payload: selectedPreviewPayload });

    fireEvent.change(await screen.findByRole("textbox", { name: "VoiceFlow 生成结果" }), {
      target: { value: "我手动修订后的内容" },
    });
    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    fireEvent.click(screen.getByRole("button", { name: "确认" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("confirm_selected_action_preview", {
      transaction_id: "tx-selected-1",
      final_text: "我手动修订后的内容",
    }));
    expect(invokeMock.mock.calls.filter(([command]) => command === "confirm_selected_action_preview")).toHaveLength(1);
    expect(await screen.findByRole("status")).toHaveTextContent("结果已替换并验证");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("copies explicitly and prevents a late preview event from reviving a canceled transaction", async () => {
    let previewHandler: ((event: { payload: SelectedActionPreview }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: SelectedActionPreview }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());
    previewHandler?.({ payload: selectedPreviewPayload });
    fireEvent.click(await screen.findByRole("button", { name: "只复制" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("copy_selected_action_preview", {
      transaction_id: "tx-selected-1",
      final_text: "整理后的文本",
    }));
    expect(await screen.findByRole("status")).toHaveTextContent("结果已复制");

    previewHandler?.({ payload: selectedPreviewPayload });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("cancels only the displayed transaction and suppresses its late preview event", async () => {
    let previewHandler: ((event: { payload: SelectedActionPreview }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: SelectedActionPreview }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());
    previewHandler?.({ payload: selectedPreviewPayload });
    fireEvent.click(await screen.findByRole("button", { name: "取消" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("cancel_selected_action_preview", {
      transaction_id: "tx-selected-1",
    }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    previewHandler?.({ payload: selectedPreviewPayload });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("orders previews by action sequence and tombstones a terminal same-sequence preview", async () => {
    const events = captureNativeEvents();
    render(<App />);
    await waitFor(() => {
      expect(events.has("selected-action://preview")).toBe(true);
      expect(events.has("selected-action://lifecycle")).toBe(true);
    });

    events.emit("selected-action://preview", selectedPreviewPayload);
    expect(await screen.findByRole("dialog", { name: "文字操作预览" })).toBeInTheDocument();
    const latestPreview = {
      ...selectedPreviewPayload,
      transaction_id: "tx-selected-2",
      action_sequence: 2,
      final_text: "最新结果",
    };
    events.emit("selected-action://lifecycle", {
      action_sequence: 2,
      transaction_id: "tx-selected-2",
      state: "started",
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    events.emit("selected-action://preview", selectedPreviewPayload);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    events.emit("selected-action://preview", latestPreview);
    expect(await screen.findByRole("dialog", { name: "文字操作预览" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("最新结果");
    events.emit("selected-action://lifecycle", {
      action_sequence: 1,
      transaction_id: "tx-selected-1",
      state: "cancelled",
    });
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    events.emit("selected-action://lifecycle", {
      action_sequence: 2,
      transaction_id: "tx-selected-2",
      state: "completed",
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    events.emit("selected-action://preview", latestPreview);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("cancels a matching sequence terminally so its late preview cannot reopen", async () => {
    const events = captureNativeEvents();
    render(<App />);
    await waitFor(() => expect(events.has("selected-action://preview")).toBe(true));
    events.emit("selected-action://preview", selectedPreviewPayload);
    expect(await screen.findByRole("dialog", { name: "文字操作预览" })).toBeInTheDocument();

    events.emit("selected-action://lifecycle", {
      action_sequence: 1,
      transaction_id: "tx-selected-1",
      state: "cancelled",
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    events.emit("selected-action://preview", selectedPreviewPayload);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("localizes only the safe action error code and blocks its late preview", async () => {
    const events = captureNativeEvents();
    render(<App />);
    await waitFor(() => expect(events.has("selected-action://error")).toBe(true));
    events.emit("selected-action://error", {
      action_sequence: 3,
      transaction_id: "tx-denied",
      code: "permission_required",
      provider_error: "private provider response",
    });

    expect(await screen.findByRole("alert")).toHaveTextContent("缺少执行此操作所需的权限。");
    expect(screen.queryByText("private provider response")).not.toBeInTheDocument();
    events.emit("selected-action://lifecycle", {
      action_sequence: 3,
      transaction_id: "tx-denied",
      state: "failed",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("缺少执行此操作所需的权限。");
    events.emit("selected-action://preview", {
      ...selectedPreviewPayload,
      transaction_id: "tx-denied",
      action_sequence: 3,
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("replaces a generic failed-lifecycle notice with the safe action error when it arrives later", async () => {
    const events = captureNativeEvents();
    render(<App />);
    await waitFor(() => expect(events.has("selected-action://error")).toBe(true));
    events.emit("selected-action://lifecycle", {
      action_sequence: 5,
      transaction_id: "tx-failed",
      state: "failed",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("操作未能完成。");

    events.emit("selected-action://error", {
      action_sequence: 5,
      transaction_id: "tx-failed",
      code: "provider_failed",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("文字服务暂时无法完成操作。");
  });

  it("reports an unverified backend completion without claiming replacement", async () => {
    selectedActionOutcome = "unverified";
    let previewHandler: ((event: { payload: SelectedActionPreview }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: SelectedActionPreview }) => void) => {
      if (event === "selected-action://preview") previewHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    await waitFor(() => expect(previewHandler).toBeDefined());
    previewHandler?.({ payload: selectedPreviewPayload });
    fireEvent.click(await screen.findByRole("button", { name: "确认" }));

    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("结果未能验证，请检查目标。"));
    expect(screen.queryByText("结果已替换并验证")).not.toBeInTheDocument();
  });

  it("refreshes dictionary when backend settings change", async () => {
    let settingsHandler: ((event: { payload: typeof settings & { dictionary: string[] } }) => void) | undefined;
    listenMock.mockImplementation((event: string, handler: (event: { payload: typeof settings & { dictionary: string[] } }) => void) => {
      if (event === "settings://changed") settingsHandler = handler;
      return Promise.resolve(vi.fn());
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "个人词典" }));
    expect(await screen.findByText(/还没有已生效替换/)).toBeInTheDocument();
    await waitFor(() => expect(settingsHandler).toBeDefined());

    settingsHandler?.({ payload: { ...settings, dictionary: ["知乎"] } as typeof settings & { dictionary: string[] } });
    expect(await screen.findByText("知乎")).toBeInTheDocument();
  });

  it("reopens the affected recording group after a debounced blur save fails", async () => {
    const base = invokeMock.getMockImplementation()!;
    let failSave = true;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "update_settings_patch" && failSave) throw new Error("disk unavailable");
      return base(command, args, options);
    });
    render(<App />);
    const trigger = await screen.findByRole("button", { name: /高级录音设置/ });
    fireEvent.click(trigger);
    const threshold = screen.getByRole("spinbutton", { name: "开始分段（秒）" });
    fireEvent.change(threshold, { target: { value: "8" } });
    threshold.focus();
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(await screen.findByRole("alert")).toHaveTextContent("disk unavailable");
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(threshold).toBeVisible();
    expect(threshold).toHaveValue(8);
    expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", { patch: { chunk_threshold_secs: 8 } });
    failSave = false;
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("lets the user choose activation mode and does not persist empty chunk values", async () => {
    render(<App />);

    expect(await screen.findByText("点按切换")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /点按切换/ })).toBeChecked();

    invokeMock.mockClear();
    fireEvent.click(screen.getByRole("button", { name: /高级录音设置/ }));
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
    await waitFor(() => expect(screen.getByRole("heading", { name: "历史记录" })).toBeInTheDocument());
    await waitFor(() => expect(stateHandler).toBeDefined());

    const historyCallsBeforeCompletion = invokeMock.mock.calls.filter(([command]) => command === "get_history").length;
    stateHandler?.({ payload: { state: "copied" } });

    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "get_history")).toHaveLength(historyCallsBeforeCompletion + 1));
  });
});
