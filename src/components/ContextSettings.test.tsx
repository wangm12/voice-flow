import { closeSelect, openSelect, selectOption } from "../test/selectOption";
import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { I18nProvider } from "../lib/i18n";
import type { ProviderId } from "../lib/providers";
import { ContextSettings, type WritingMode } from "./ContextSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const openMock = vi.mocked(open);
let contextMappingsResponse: unknown[] = [];
let contextSnapshotResponse: unknown = null;

describe("ContextSettings", () => {
  const modes: WritingMode[] = [
    { id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true },
    { id: "custom.reply", label: "回复", family: "general", prompt: "礼貌回复。", builtin: false },
  ];

  it("retains an unsaved Prompt across equivalent and changed background snapshots", async () => {
    const onChange = vi.fn();
    const view = render(<ContextSettings writingModes={modes} onWritingModesChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "我的未保存草稿" } });
    view.rerender(<ContextSettings writingModes={modes.map((mode) => ({ ...mode }))} onWritingModesChange={onChange} />);
    expect(prompt).toHaveValue("我的未保存草稿");
    const updated = modes.map((mode) => ({ ...mode, prompt: mode.id === "general" ? "后台更新当前语气" : "后台更新另一语气" }));
    view.rerender(<ContextSettings writingModes={updated} onWritingModesChange={onChange} />);
    expect(prompt).toHaveValue("我的未保存草稿");
    expect(screen.getByText("有未保存的更改")).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(within(prompt.closest("section")!).getByRole("button", { name: "保存语气" }));
    expect(onChange).toHaveBeenCalledWith([
      { ...modes[0], prompt: "我的未保存草稿" },
      { ...modes[1], prompt: "后台更新另一语气" },
    ]);
  });

  it("keeps a new custom tone and its name across a background settings snapshot", async () => {
    const onChange = vi.fn();
    const view = render(<ContextSettings writingModes={modes} onWritingModesChange={onChange} />);
    const selector = screen.getByRole("combobox", { name: "编辑哪种语气" });
    selectOption(selector, "__add_custom_mode__");
    const draftId = (selector as HTMLButtonElement).value;
    fireEvent.change(screen.getByRole("textbox", { name: "自定义语气名称" }), { target: { value: "正在编辑的新语气" } });
    view.rerender(<ContextSettings writingModes={modes.map((mode) => ({ ...mode }))} onWritingModesChange={onChange} />);
    expect(selector).toHaveValue(draftId);
    expect(screen.getByRole("textbox", { name: "自定义语气名称" })).toHaveValue("正在编辑的新语气");
    expect(onChange).not.toHaveBeenCalled();
    const appMenu = await openSelect(screen.getByRole("combobox", { name: "选择 App" }));
    await screen.findByRole("option", { name: "Cursor" });
    closeSelect(appMenu);
  });

  it("updates a clean editor from a changed snapshot and restores the latest saved Prompt on discard", async () => {
    const view = render(<ContextSettings writingModes={modes} />);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    const updated = [{ ...modes[0], prompt: "新的已保存配置" }, modes[1]];
    view.rerender(<ContextSettings writingModes={updated} />);
    expect(prompt).toHaveValue("新的已保存配置");
    fireEvent.change(prompt, { target: { value: "不要保留的草稿" } });
    const latest = [{ ...modes[0], prompt: "最终已保存配置" }, modes[1]];
    view.rerender(<ContextSettings writingModes={latest} />);
    const selector = screen.getByRole("combobox", { name: "编辑哪种语气" });
    selector.focus();
    selectOption(selector, "custom.reply");
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }));
    await waitFor(() => expect(selector).toHaveFocus());
    selectOption(selector, "general");
    expect(prompt).toHaveValue("最终已保存配置");
  });

  it("keeps an unrelated toggle failure beside its row without reopening the advanced group", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args, options) => command === "set_context_enabled"
      ? Promise.reject(new Error("上下文开关失败")) : base(command, args, options));
    render(<ContextSettings automationOnly />);
    await screen.findByText("Cursor · 通用");
    const advanced = screen.getByRole("button", { name: /高级整理设置/ });
    fireEvent.click(advanced);
    selectOption(screen.getByRole("combobox", { name: "临时覆盖" }), "email");
    await waitFor(() => expect(screen.getByRole("switch", { name: "App 上下文适配" })).not.toBeDisabled());
    fireEvent.click(advanced);
    fireEvent.click(screen.getByRole("switch", { name: "App 上下文适配" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("上下文开关失败");
    expect(alert.closest(".vf-settings-row")).toContainElement(screen.getByRole("switch", { name: "App 上下文适配" }));
    expect(advanced).toHaveAttribute("aria-expanded", "false");
    expect(advanced).toHaveAttribute("aria-disabled", "false");
  });

  it("uses the latest saved modes when a background snapshot arrives during a discard confirmation", async () => {
    const onChange = vi.fn();
    const view = render(<ContextSettings writingModes={modes} onWritingModesChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "即将放弃的草稿" } });
    selectOption(screen.getByRole("combobox", { name: "编辑哪种语气" }), "custom.reply");
    const latest = modes.map((mode) => ({ ...mode, prompt: `最新保存的 ${mode.id}` }));
    view.rerender(<ContextSettings writingModes={latest} onWritingModesChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }));
    expect(prompt).toHaveValue("最新保存的 custom.reply");
    fireEvent.click(within(prompt.closest("section")!).getByRole("button", { name: "保存语气" }));
    expect(onChange).toHaveBeenCalledWith(latest);
    const appMenu = await openSelect(screen.getByRole("combobox", { name: "选择 App" }));
    await screen.findByRole("option", { name: "Cursor" });
    closeSelect(appMenu);
  });

  it("retains other background updates when deleting a tone after its confirmation opened", async () => {
    const onChange = vi.fn();
    const view = render(<ContextSettings writingModes={modes} onWritingModesChange={onChange} />);
    selectOption(screen.getByRole("combobox", { name: "编辑哪种语气" }), "custom.reply");
    fireEvent.click(screen.getByRole("button", { name: "删除自定义语气" }));
    const latest = modes.map((mode) => ({ ...mode, prompt: `后台更新 ${mode.id}` }));
    view.rerender(<ContextSettings writingModes={latest} onWritingModesChange={onChange} />);
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "删除自定义语气" }));
    expect(onChange).toHaveBeenCalledWith([latest[0]]);
    const appMenu = await openSelect(screen.getByRole("combobox", { name: "选择 App" }));
    await screen.findByRole("option", { name: "Cursor" });
    closeSelect(appMenu);
  });

  it("renders Prompt validation beside the save control and persistence errors inside their advanced group", async () => {
    const view = render(<ContextSettings />);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "" } });
    fireEvent.click(within(prompt.closest("section")!).getByRole("button", { name: "保存语气" }));
    expect(within(prompt.closest("section")!).getByRole("alert")).toHaveTextContent("请填写 Prompt");
    view.unmount();
    render(<ContextSettings automationOnly saveFailure={{ fields: ["accurate_asr_model"], message: "模型保存失败" }} />);
    const advanced = screen.getByRole("button", { name: /高级整理设置/ });
    expect(advanced).toHaveAttribute("aria-expanded", "true");
    expect(within(advanced.closest("section")!).getByRole("alert")).toHaveTextContent("模型保存失败");
    await screen.findByText("Cursor · 通用");
  });

  it("keeps one manual preview mounted and pending while the Prompt group folds", async () => {
    const base = invokeMock.getMockImplementation()!;
    let resolvePreview!: (value: unknown) => void;
    invokeMock.mockImplementation((command, args, options) => command === "preview_writing_mode"
      ? new Promise((resolve) => { resolvePreview = resolve; })
      : base(command, args, options));
    render(<ContextSettings />);
    await screen.findByRole("combobox", { name: "编辑哪种语气" });
    const sample = screen.getByRole("textbox", { name: "试跑文本" });
    fireEvent.change(sample, { target: { value: "保留这段试跑草稿" } });
    const trigger = screen.getByRole("button", { name: /编辑 Prompt/ });
    fireEvent.click(trigger);
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "只修正标点，保留事实。" } });
    fireEvent.click(screen.getByRole("button", { name: "试跑并对比" }));
    expect(invokeMock).toHaveBeenCalledWith("preview_writing_mode", expect.objectContaining({
      request: expect.objectContaining({ text: "保留这段试跑草稿", compare_saved: true }),
    }));
    fireEvent.click(trigger);
    expect(prompt).not.toBeVisible();
    expect(screen.getByRole("textbox", { name: "试跑文本" })).toBe(sample);
    expect(sample).toBeVisible();
    expect(screen.getByRole("button", { name: "正在试跑…" })).toBeDisabled();
    fireEvent.click(trigger);
    expect(screen.getByRole("textbox", { name: "语气 Prompt" })).toBe(prompt);
    expect(prompt).toHaveValue("只修正标点，保留事实。");
    expect(invokeMock.mock.calls.filter(([command]) => command === "preview_writing_mode")).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === "cancel_writing_preview")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", expect.anything());
    await act(async () => resolvePreview({ saved: null, draft: { text: "过期结果", status: "model", elapsed_ms: 1 } }));
    expect(screen.queryByText("过期结果")).not.toBeInTheDocument();
  });

  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
    openMock.mockReset();
    openMock.mockResolvedValue(null);
    contextMappingsResponse = [];
    contextSnapshotResponse = { profile: { id: "native.general", family: "general", app_label: "Cursor", icon_key: "app", source: "fallback", confidence: 0.4 }, browser_access_status: "not_applicable" };
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command) => {
      switch (command) {
        case "get_context_snapshot":
          return contextSnapshotResponse;
        case "get_context_mappings":
          return contextMappingsResponse;
        case "get_settings":
          return { context_enabled: true };
        case "get_context_override":
          return null;
        case "get_available_applications":
          return [
            { bundle_id: "com.todesktop.230313mzl4w4u92", label: "Cursor" },
            { bundle_id: "com.google.Chrome", label: "Google Chrome" },
            ];
        case "get_application_from_path":
          return { bundle_id: "md.obsidian", label: "Obsidian" };
        case "save_context_mapping":
          return [{ id: "com.todesktop.230313mzl4w4u92", label: "Cursor", family: "prompt_or_code", bundle_id: "com.todesktop.230313mzl4w4u92", executable: null, browser_host: null, enabled: true }];
        default:
          return undefined;
      }
    });
  });

  it("keeps basic mapping choices visible and advanced fields collapsed without hiding grants", async () => {
    render(<ContextSettings />);
    expect(await screen.findByRole("combobox", { name: "选择 App" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "应用映射语气" }).closest("details")).toBeNull();
    const disclosure = screen.getByText("高级映射选项").closest("details");
    expect(disclosure).not.toHaveAttribute("open");
    expect(screen.getByLabelText("可执行文件名").closest("details")).toBe(disclosure);
    expect(screen.getByLabelText("App 风格示例输入").closest("details")).toBe(disclosure);
    for (const name of ["允许读取辅助功能文字", "允许本机 OCR", "允许自动云端视觉", "允许文字发送给服务商", "允许风格示例发送给整理服务商"]) {
      const grant = screen.getByRole("switch", { name });
      expect(grant.closest("details")).toBeNull();
      expect(grant).toHaveAttribute("aria-checked", "false");
    }
    fireEvent.click(screen.getByText("高级映射选项"));
    expect(disclosure).toHaveAttribute("open");
    fireEvent.change(screen.getByLabelText("可执行文件名"), { target: { value: "Cursor" } });
    fireEvent.click(screen.getByText("高级映射选项 · 已设置"));
    expect(disclosure).not.toHaveAttribute("open");
    expect(screen.getByLabelText("可执行文件名")).toHaveValue("Cursor");
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");
  });

  it("uses WeChat-style help for mapping examples", async () => {
    render(<ContextSettings />);

    expect(await screen.findByPlaceholderText("贴一条你平时微信怎么打")).toBeInTheDocument();
  });

  it("translates WeChat-style mapping help", async () => {
    render(
      <I18nProvider initialLanguage="en">
        <ContextSettings />
      </I18nProvider>,
    );

    expect(await screen.findByPlaceholderText("Paste a typical WeChat message")).toBeInTheDocument();
  });

  it("leaves the look-at-screen vision model unset", async () => {
    render(
      <ContextSettings
        automationOnly
        visionProvider=""
        visionModel=""
        onVisionProviderChange={vi.fn()}
        onVisionModelChange={vi.fn()}
      />,
    );

    expect(await screen.findByLabelText("视觉模型")).toHaveValue("");
    expect(screen.getByLabelText("视觉服务商")).toHaveValue("");
    expect(screen.queryByRole("option", { name: "Anthropic" })).not.toBeInTheDocument();
  });

  it("exposes accurate ASR fields that default empty", async () => {
    render(
      <ContextSettings
        automationOnly
        accurateAsrProvider="groq"
        accurateAsrModel=""
        onAccurateAsrProviderChange={vi.fn()}
        onAccurateAsrModelChange={vi.fn()}
      />,
    );

    expect(await screen.findByLabelText("精确转写模型")).toHaveValue("");
    expect(screen.getByLabelText("精确转写服务商")).toHaveValue("groq");
    expect(screen.getByText("中英混合、人名多、主转写失败或置信度低时才打第二枪；留空仍关闭。")).toBeInTheDocument();
    expect(screen.getByText("第二枪需要自定义 / 兼容接口上已填的百炼密钥。")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("whisper-large-v3-turbo")).not.toBeInTheDocument();
  });

  it("fills Accurate ASR with DashScope Qwen3-ASR on one click", async () => {
    function Harness() {
      const [provider, setProvider] = useState<ProviderId>("groq");
      const [model, setModel] = useState("");
      const [url, setUrl] = useState("");
      return (
        <ContextSettings
          automationOnly
          accurateAsrProvider={provider}
          accurateAsrModel={model}
          accurateAsrBaseUrl={url}
          onAccurateAsrProviderChange={setProvider}
          onAccurateAsrModelChange={setModel}
          onAccurateAsrBaseUrlChange={setUrl}
        />
      );
    }

    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: /高级整理设置/ }));
    fireEvent.click(await screen.findByRole("button", { name: "用百炼 Qwen3-ASR 补一枪" }));

    expect(screen.getByLabelText("精确转写服务商")).toHaveValue("custom");
    expect(screen.getByLabelText("精确转写模型")).toHaveValue("qwen3-asr-flash");
    expect(screen.getByLabelText("精确转写模型")).not.toHaveValue("");
    expect(screen.getByLabelText("精确转写模型")).not.toHaveValue("whisper-large-v3-turbo");
    expect(screen.getByLabelText("精确转写地址")).toHaveValue("https://dashscope.aliyuncs.com/compatible-mode/v1");
  });

  it("no-ops the Accurate Qwen fill when change handlers are missing", async () => {
    render(<ContextSettings automationOnly />);
    fireEvent.click(screen.getByRole("button", { name: /高级整理设置/ }));
    fireEvent.click(await screen.findByRole("button", { name: "用百炼 Qwen3-ASR 补一枪" }));
    expect(screen.getByLabelText("精确转写服务商")).toHaveValue("groq");
    expect(screen.getByLabelText("精确转写模型")).toHaveValue("");
    expect(screen.queryByLabelText("精确转写地址")).not.toBeInTheDocument();
  });

  it("exposes a window OCR opt-in that defaults off", async () => {
    render(
      <ContextSettings
        automationOnly
        windowOcrEnabled={false}
        onWindowOcrEnabledChange={vi.fn()}
      />,
    );

    expect(await screen.findByRole("switch", { name: "窗口文字识别" })).toHaveAttribute("aria-checked", "false");
  });

  it("exposes per-app cleanup effort and learning controls", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    expect(screen.getByRole("combobox", { name: "整理强度" })).toHaveValue("inherit");
    expect(screen.getByRole("switch", { name: "这个 App 使用 AI 整理" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "在这个 App 学习词条" })).toHaveAttribute("aria-checked", "true");
  });

  it("lets users choose an app without exposing native identifiers", async () => {
    render(<ContextSettings />);

    const appMenu = await openSelect(screen.getByRole("combobox", { name: "选择 App" }));
    expect(await screen.findByRole("option", { name: "Cursor" })).toBeInTheDocument();
    closeSelect(appMenu);
    expect(screen.queryByLabelText("应用映射 ID")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Bundle ID")).not.toBeInTheDocument();
    expect(screen.getByLabelText("可执行文件名")).toBeInTheDocument();
  });

  it("shows the current app and family from the live snapshot", async () => {
    render(<ContextSettings combined />);

    expect(await screen.findByText("Cursor · 通用")).toBeInTheDocument();
    expect(screen.getByText("来源: 自动回退")).toBeInTheDocument();
  });

  it("shows the matched user rule and its satisfied selectors", async () => {
    contextSnapshotResponse = {
      profile: { id: "user.chrome:gmail", family: "email", app_label: "Google Chrome", icon_key: "browser", source: "user_mapping", confidence: 1 },
      browser_access_status: "granted",
    };
    contextMappingsResponse = [{
      id: "chrome:gmail",
      label: "Chrome · Gmail work mail",
      family: "email",
      bundle_id: "com.google.Chrome",
      executable: null,
      browser_host: "mail.google.com",
      browser_path_prefix: "/mail",
      focused_field: "email",
      source_permissions: {},
      enabled: true,
    }];
    render(<ContextSettings combined />);

    expect(await screen.findByText("Chrome · Gmail work mail · 邮件")).toBeInTheDocument();
    expect(screen.getByText("来源: 用户映射")).toBeInTheDocument();
    expect(screen.getByText("匹配条件: App: Google Chrome · 网站: Gmail · 路径前缀: /mail · 输入框: 邮件输入框")).toBeInTheDocument();
  });

  it("saves the selected app and writing mode using generated selectors", async () => {
    render(<ContextSettings />);

    const applicationSelect = await screen.findByRole("combobox", { name: "选择 App" });
    selectOption(applicationSelect, "com.todesktop.230313mzl4w4u92");
    selectOption(screen.getByRole("combobox", { name: "应用映射语气" }), "prompt_or_code");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: {
        id: "com.todesktop.230313mzl4w4u92",
        label: "Cursor",
        family: "prompt_or_code",
        mode_id: "prompt_or_code",
        bundle_id: "com.todesktop.230313mzl4w4u92",
        executable: null,
        browser_host: null,
        browser_path_prefix: null,
        focused_field: null,
        source_permissions: {
          ax_text: false,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: false,
        },
        style_examples_approved: false,
        enabled: true,
        cleanup_effort: null,
        cleanup_intensity: null,
        cleanup_enabled: true,
        dictionary_learn_enabled: true,
      },
    }));
  });

  it("persists explicit scene Auto separately from inheriting the global intensity", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    const intensity = screen.getByRole("combobox", { name: "整理强度" });
    expect(intensity).toHaveValue("inherit");
    selectOption(intensity, "auto");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({ cleanup_intensity: "auto" }),
    }));
  });

  it("defaults global cleanup intensity to auto and can save light", async () => {
    const onCleanupIntensityChange = vi.fn();
    render(
      <ContextSettings
        automationOnly
        cleanupIntensity="auto"
        onCleanupIntensityChange={onCleanupIntensityChange}
      />,
    );

    const control = await screen.findByRole("combobox", { name: "整理强度" });
    expect(control).toHaveValue("auto");
    selectOption(control, "light");
    expect(onCleanupIntensityChange).toHaveBeenCalledWith("light");
  });

  it("uses a custom writing mode in an app mapping", async () => {
    render(<ContextSettings writingModes={[
      { id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true },
      { id: "custom.reply", label: "客服回复", family: "general", prompt: "保持礼貌，但不要添加承诺。", builtin: false },
    ]} />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    selectOption(screen.getByRole("combobox", { name: "应用映射语气" }), "custom.reply");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        family: "general",
        mode_id: "custom.reply",
      }),
    }));
  });

  it("edits a built-in prompt and adds a custom mode", async () => {
    const onWritingModesChange = vi.fn();
    render(<ContextSettings onWritingModesChange={onWritingModesChange} />);

    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = await screen.findByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "保留我的语气，只修正明显的语病。" } });
    fireEvent.click(within(prompt.closest("section")!).getByRole("button", { name: "保存语气" }));

    expect(onWritingModesChange).toHaveBeenCalledWith(expect.arrayContaining([
      expect.objectContaining({ id: "general", prompt: "保留我的语气，只修正明显的语病。" }),
    ]));

    selectOption(screen.getByRole("combobox", { name: "编辑哪种语气" }), "__add_custom_mode__");
    expect(onWritingModesChange).toHaveBeenCalledTimes(1);
    fireEvent.click(within(screen.getByRole("combobox", { name: "编辑哪种语气" }).closest("section")!).getByRole("button", { name: "保存语气" }));
    expect(onWritingModesChange).toHaveBeenLastCalledWith(expect.arrayContaining([
      expect.objectContaining({ label: "自定义语气", builtin: false }),
    ]));
  });

  it("adds an app selected from the Applications folder", async () => {
    openMock.mockResolvedValue("/Applications/Obsidian.app");
    render(<ContextSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "从应用程序中选择" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_application_from_path", {
      path: "/Applications/Obsidian.app",
    }));
    const appMenu = await openSelect(screen.getByRole("combobox", { name: "选择 App" }));
    expect(await screen.findByRole("option", { name: "Obsidian" })).toBeInTheDocument();
    closeSelect(appMenu);
    expect(screen.getByRole("combobox", { name: "选择 App" })).toHaveValue("md.obsidian");
  });

  it("keeps browser website mappings user-friendly", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.google.Chrome");
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射网站" }), { target: { value: "mail.google.com" } });
    selectOption(screen.getByRole("combobox", { name: "应用映射语气" }), "email");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: {
        id: "com.google.Chrome:mail.google.com",
        label: "Google Chrome",
        family: "email",
        mode_id: "email",
        bundle_id: "com.google.Chrome",
        executable: null,
        browser_host: "mail.google.com",
        browser_path_prefix: null,
        focused_field: null,
        source_permissions: {
          ax_text: false,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: false,
        },
        style_examples_approved: false,
        enabled: true,
        cleanup_effort: null,
        cleanup_intensity: null,
        cleanup_enabled: true,
        dictionary_learn_enabled: true,
      },
    }));
  });

  it("saves path, coding prompt selector, and explicit source grants on an app and website rule", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.google.Chrome");
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射网站" }), { target: { value: "mail.google.com" } });
    fireEvent.change(screen.getByRole("textbox", { name: "网站路径前缀" }), { target: { value: "/issues" } });
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "coding_prompt");
    fireEvent.click(screen.getByRole("switch", { name: "允许读取辅助功能文字" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许本机 OCR" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许自动云端视觉" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许文字发送给服务商" }));
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        bundle_id: "com.google.Chrome",
        browser_host: "mail.google.com",
        browser_path_prefix: "/issues",
        focused_field: "coding_prompt",
        source_permissions: {
          ax_text: true,
          local_ocr: true,
          cloud_vision: true,
          context_text_to_providers: true,
        },
      }),
    }));
  });

  it("saves a host-only rule with normalized host, path, and focused-field conditions", async () => {
    render(<ContextSettings />);

    fireEvent.change(await screen.findByRole("combobox", { name: "应用映射网站" }), { target: { value: "MAIL.Google.com" } });
    fireEvent.change(screen.getByRole("textbox", { name: "网站路径前缀" }), { target: { value: "/mail" } });
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "email");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        bundle_id: null,
        executable: null,
        browser_host: "mail.google.com",
        browser_path_prefix: "/mail",
        focused_field: "email",
        source_permissions: {
          ax_text: false,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: false,
        },
      }),
    }));
  });

  it("saves an executable-only rule and rejects full URLs in the host selector", async () => {
    render(<ContextSettings />);

    fireEvent.change(await screen.findByRole("textbox", { name: "可执行文件名" }), { target: { value: "Cursor" } });
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({ bundle_id: null, executable: "Cursor", browser_host: null }),
    }));

    invokeMock.mockClear();
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射网站" }), { target: { value: "https://user:secret@mail.google.com/mail?token=private#draft" } });
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    expect(await screen.findByText("请输入主机名，不要粘贴网址、路径或查询参数")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
  });

  it("shows and lets the user clear an ineffective legacy host-only cloud grant", async () => {
    contextMappingsResponse = [{
      id: "legacy.gmail",
      label: "Gmail rule",
      family: "email",
      bundle_id: null,
      executable: null,
      browser_host: "mail.google.com",
      source_permissions: { cloud_vision: true },
      enabled: true,
    }];
    render(<ContextSettings />);

    fireEvent.change(await screen.findByRole("combobox", { name: "应用映射网站" }), { target: { value: "mail.google.com" } });
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    expect(await screen.findByText("此目标已有保存规则；请先检查其来源权限，再明确更新。")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("此规则没有具体原生 App 或可执行文件选择器，云端视觉权限不会生效；关闭权限后才能保存。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("switch", { name: "允许自动云端视觉" }));
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        id: "legacy.gmail",
        bundle_id: null,
        browser_host: "mail.google.com",
        source_permissions: expect.objectContaining({ cloud_vision: false }),
      }),
    }));
  });

  it("defaults every missing legacy source permission grant to false", async () => {
    contextMappingsResponse = [{
      id: "com.todesktop.230313mzl4w4u92",
      label: "Cursor",
      family: "general",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      enabled: true,
    }];
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");

    expect(screen.getByRole("switch", { name: "允许读取辅助功能文字" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许本机 OCR" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText("启用后，辅助功能或 OCR 派生的文字可能离开这台 Mac，发送给已配置的转写和整理服务商。关闭后，这些文字不会进入服务商请求。")).toBeInTheDocument();
  });

  it("defaults legacy style-example approval to false independently of the text provider grant and retains examples", async () => {
    contextMappingsResponse = [{
      id: "com.todesktop.230313mzl4w4u92",
      label: "Cursor",
      family: "general",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      source_permissions: { context_text_to_providers: true },
      style_example_input: "could you fix this",
      style_example_output: "Could you fix this?",
      style_example_pairs: [{ input: "thanks!", output: "Thank you!" }],
      enabled: true,
    }];
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");

    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("textbox", { name: "App 风格示例输入" })).toHaveValue("could you fix this");
    expect(screen.getByRole("textbox", { name: "App 风格期望输出" })).toHaveValue("Could you fix this?");
    expect(screen.getByText("风格示例会保留，但在你单独批准前不会进入整理服务请求。")).toBeInTheDocument();
    fireEvent.click(screen.getByText("查看已保存的风格配对 (1)"));
    expect(screen.getByText("thanks!")).toBeInTheDocument();
    expect(screen.getByText("Thank you!")).toBeInTheDocument();
    expect(screen.getByText("启用后，此映射保留的输入、期望输出和已学习示例对可能随整理请求发送给已配置的整理服务商。关闭时仍保留这些示例，但不发送。此权限独立于“允许文字发送给服务商”。")).toBeInTheDocument();
    expect(screen.getByText("风格示例会保留，但在你单独批准前不会进入整理服务请求。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        style_examples_approved: false,
        style_example_input: "could you fix this",
        style_example_output: "Could you fix this?",
        style_example_pairs: [{ input: "thanks!", output: "Thank you!" }],
        source_permissions: expect.objectContaining({ context_text_to_providers: true }),
      }),
    }));
  });

  it("saves explicit style-example approval without coupling it to the text provider grant", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.change(screen.getByRole("textbox", { name: "App 风格示例输入" }), { target: { value: "thanks!" } });
    fireEvent.change(screen.getByRole("textbox", { name: "App 风格期望输出" }), { target: { value: "Thank you!" } });
    fireEvent.click(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" }));

    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: expect.objectContaining({
        style_examples_approved: true,
        style_example_input: "thanks!",
        style_example_output: "Thank you!",
        source_permissions: expect.objectContaining({ context_text_to_providers: false }),
      }),
    }));
  });
});
