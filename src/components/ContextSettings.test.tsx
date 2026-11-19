import { SessionDraftProvider } from "../lib/sessionDraft";
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
function revealPermissions() {
  const control = screen.getByRole("button", { name: /^上下文与样例授权/ });
  if (control.getAttribute("aria-expanded") !== "true") fireEvent.click(control);
}


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
    expect(screen.getAllByText("有未保存的更改").length).toBeGreaterThan(0);
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

  it("keeps basic mapping choices visible and discloses independent grants without changing them", async () => {
    render(<ContextSettings />);
    expect(await screen.findByRole("combobox", { name: "选择 App" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "应用映射语气" }).closest("details")).toBeNull();
    const disclosure = screen.getByText("高级映射选项").closest("details");
    expect(disclosure).not.toHaveAttribute("open");
    expect(screen.getByLabelText("可执行文件名").closest("details")).toBe(disclosure);
    expect(screen.getByLabelText("App 风格示例输入").closest("details")).toBe(disclosure);
    expect(screen.queryByRole("switch", { name: "允许读取辅助功能文字" })).not.toBeInTheDocument();
    revealPermissions();
    for (const name of ["允许读取辅助功能文字", "允许本机 OCR", "允许自动云端视觉", "允许文字发送给服务商", "允许风格示例发送给整理服务商", "允许结构化偏好发送给云端整理服务商"]) {
      const grant = screen.getByRole("switch", { name });
      expect(grant.closest("details")).toBeNull();
      expect(grant).toHaveAttribute("aria-checked", "false");
    }
    fireEvent.click(screen.getByText(/高级映射选项/));
    expect(disclosure).toHaveAttribute("open");
    fireEvent.change(screen.getByLabelText("可执行文件名"), { target: { value: "Cursor" } });
    fireEvent.click(screen.getByText("高级映射选项 · 已设置"));
    expect(disclosure).not.toHaveAttribute("open");
    expect(screen.getByLabelText("可执行文件名")).toHaveValue("Cursor");
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");
  });

  it("keeps the translation target editable from Writing while output mode is Auto", async () => {
    const onTargetLanguageChange = vi.fn();
    render(
      <ContextSettings
        combined
        outputMode="auto"
        translationTargetLanguage="en"
        onTranslationTargetLanguageChange={onTargetLanguageChange}
      />,
    );

    const targetLanguage = await screen.findByRole("combobox", { name: "翻译目标语言" });
    expect(targetLanguage).toHaveValue("en");
    selectOption(targetLanguage, "ja");
    expect(onTargetLanguageChange).toHaveBeenCalledWith("ja");
    expect(screen.getByRole("combobox", { name: "输出模式" })).toHaveValue("auto");
  });

  it("applies the three context privacy presets to the draft and saves the shared provider grant explicitly", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args, options) => command === "save_context_mapping"
      ? Promise.resolve([(args as { mapping: unknown }).mapping])
      : base(command, args, options));

    render(<ContextSettings combined />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    revealPermissions();
    const off = screen.getByRole("radio", { name: /关闭上下文读取/ });
    const local = screen.getByRole("radio", { name: /仅本机使用/ });
    const providers = screen.getByRole("radio", { name: /允许文字发送给转写与整理服务/ });

    fireEvent.click(off);
    expect(off).toBeChecked();
    expect(screen.getByRole("switch", { name: "允许读取辅助功能文字" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");

    fireEvent.click(local);
    expect(local).toBeChecked();
    expect(screen.getByRole("switch", { name: "允许读取辅助功能文字" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许本机 OCR" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "false");

    fireEvent.click(providers);
    expect(providers).toBeChecked();
    expect(screen.getByRole("switch", { name: "允许读取辅助功能文字" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许本机 OCR" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许自动云端视觉" })).toHaveAttribute("aria-checked", "false");
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        source_permissions: {
          ax_text: true,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: true,
        },
        style_examples_approved: false,
        preference_guidance_provider_approved: false,
      }),
    })));
  });

  it("allows task bundle-only binding for a known native app and rejects unknown Chromium IDs", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args, options) => {
      if (command === "get_available_applications") {
        return Promise.resolve([{ bundle_id: "COM.TINYSPECK.SLACKMACGAP", label: "Slack" }]);
      }
      if (command === "save_context_mapping") return Promise.resolve([(args as { mapping: unknown }).mapping]);
      return base(command, args, options);
    });

    const view = render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "COM.TINYSPECK.SLACKMACGAP");
    fireEvent.click(screen.getByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "写作任务 ID" }), "work_chat");
    fireEvent.click(screen.getByText("高级映射选项"));
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "secure");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    expect(await screen.findByText("指定写作任务需要认可的原生 App、可信输入框，或精确网站路径")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        bundle_id: "COM.TINYSPECK.SLACKMACGAP",
        writing_task_id: "work_chat",
      }),
    })));

    view.unmount();
    invokeMock.mockClear();
    invokeMock.mockImplementation((command, args, options) => {
      if (command === "get_available_applications") {
        return Promise.resolve([{ bundle_id: "org.chromium.Chromium", label: "Chromium" }]);
      }
      return base(command, args, options);
    });
    render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "org.chromium.Chromium");
    fireEvent.click(screen.getByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "写作任务 ID" }), "work_chat");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    expect(await screen.findByText("指定写作任务需要认可的原生 App、可信输入框，或精确网站路径")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
  });

  it("saves task-scoped preferences separately from style-sample approval and supports pause and forget", async () => {
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args, options) => command === "save_context_mapping"
      ? Promise.resolve([(args as { mapping: unknown }).mapping])
      : base(command, args, options));

    render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.click(screen.getByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "写作任务 ID" }), "project_update");
    selectOption(screen.getByRole("combobox", { name: "列表结构" }), "bullets");
    fireEvent.click(screen.getByText("高级映射选项"));
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "document");
    revealPermissions();
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        writing_task_id: "project_update",
        writing_preferences: [{ task_id: "project_update", dimension: "list_structure", value: "bullets", status: "active", version: 1 }],
        preference_guidance_provider_approved: false,
        style_examples_approved: false,
      }),
    })));
    expect(invokeMock.mock.calls.some(([command]) => command === "preview_writing_mode")).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "编辑映射" }));
    selectOption(screen.getByRole("combobox", { name: "列表结构 · 偏好状态" }), "paused");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        writing_preferences: [{ task_id: "project_update", dimension: "list_structure", value: "bullets", status: "paused", version: 1 }],
        preference_guidance_provider_approved: false,
      }),
    })));

    fireEvent.click(screen.getByRole("button", { name: "编辑映射" }));
    selectOption(screen.getByRole("combobox", { name: "列表结构 · 偏好状态" }), "forgotten");
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "忘记偏好" }));
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        writing_preferences: [{ task_id: "project_update", dimension: "list_structure", value: "bullets", status: "forgotten", version: 1 }],
        preference_guidance_provider_approved: false,
      }),
    })));
  });

  it("does not accept a focused-field-only task selector without an App or site anchor", async () => {
    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "写作任务 ID" }), "project_update");
    fireEvent.click(screen.getByText("高级映射选项"));
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "document");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    expect(await screen.findByText("指定写作任务需要认可的原生 App、可信输入框，或精确网站路径")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
  });

  it("shows fixed greeting and punctuation examples without calling a provider", async () => {
    render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.click(screen.getByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "写作任务 ID" }), "work_chat");
    selectOption(screen.getByRole("combobox", { name: "短称呼形式" }), "respectful_greeting");
    selectOption(screen.getByRole("combobox", { name: "陈述句句末标点" }), "no_terminal_period");

    expect(screen.getByText("你好，Alex。 → 您好，Alex。")).toBeInTheDocument();
    expect(screen.getByText("好的。 → 好的")).toBeInTheDocument();
    expect(screen.getAllByText("固定示例，不会调用服务商或修改原文")).toHaveLength(2);
    expect(invokeMock.mock.calls.some(([command]) => command === "preview_writing_mode")).toBe(false);
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
    expect(screen.getByText("第二枪需要 Groq 上已填的密钥。")).toBeInTheDocument();
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
    expect(screen.getByText("第二枪需要 自定义 / 兼容接口 上已填的密钥。")).toBeInTheDocument();
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

  it("keeps draft preferences and independent source grants while reopening contextual teaching", async () => {
    const onChange = vi.fn();
    const onOpenPermissions = vi.fn();
    render(<ContextSettings writingModes={modes} onWritingModesChange={onChange} onOpenPermissions={onOpenPermissions} />);
    revealPermissions();
    await screen.findByRole("combobox", { name: "选择 App" });
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "我的未保存偏好" } });
    fireEvent.click(screen.getByRole("button", { name: "语气与个人偏好：草稿、保存和批准" }));
    expect(screen.getByText(/草稿不会切换当前听写语气/)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "收起说明" }));
    fireEvent.click(screen.getByRole("button", { name: "语气与个人偏好：草稿、保存和批准" }));
    fireEvent.click(screen.getByRole("button", { name: "上下文与看屏幕：分别选择来源和用途" }));
    expect(prompt).toHaveValue("我的未保存偏好");
    for (const name of ["允许读取辅助功能文字", "允许本机 OCR", "允许自动云端视觉", "允许文字发送给服务商", "允许风格示例发送给整理服务商", "允许结构化偏好发送给云端整理服务商"]) {
      expect(screen.getByRole("switch", { name })).toHaveAttribute("aria-checked", "false");
    }
    expect(onChange).not.toHaveBeenCalled();
    expect(onOpenPermissions).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => ["preview_writing_mode", "save_context_mapping", "show_permission_guide", "acknowledge_guide"].includes(command as string))).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "查看系统权限（不自动申请）" }));
    expect(onOpenPermissions).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(prompt).toHaveValue("我的未保存偏好");
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
    render(<ContextSettings automationOnly />);

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
    render(<ContextSettings automationOnly />);

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

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
      mapping: {
        id: expect.stringMatching(/^rule:/),
        label: "Cursor",
        family: "prompt_or_code",
        mode_id: "prompt_or_code",
        bundle_id: "com.todesktop.230313mzl4w4u92",
        executable: null,
        browser_host: null,
        browser_path_prefix: null,
        focused_field: null,
        writing_task_id: null,
        writing_preferences: [],
        preference_guidance_provider_approved: false,
        preference_revision: 0,
        source_permissions: {
          ax_text: false,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: false,
        },
        style_examples_approved: false,
        style_example_input: null,
        style_example_output: null,
        style_example_pairs: [],
        enabled: true,
        cleanup_effort: null,
        cleanup_intensity: null,
        cleanup_enabled: true,
        dictionary_learn_enabled: true,
      },
    }));
  });

  it("clears preferences on a saved scope change and requires fresh entry for the new scope", async () => {
    contextMappingsResponse = [{
      id: "rule:cursor-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [{
        task_id: "work_chat",
        dimension: "greeting_tone",
        value: "respectful_greeting",
        status: "active",
        version: 1,
      }],
      preference_guidance_provider_approved: true,
      preference_revision: 4,
      source_permissions: {},
      style_example_input: "private sample input",
      style_example_output: "Private sample output",
      style_example_pairs: [{ input: "private sample input", output: "Private sample output" }],
      style_examples_approved: true,
      enabled: true,
    }];
    const base = invokeMock.getMockImplementation()!;
    let mappingSaveCount = 0;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "save_context_mapping") {
        const incoming = (args as { mapping: Record<string, unknown> }).mapping;
        mappingSaveCount += 1;
        const saved = mappingSaveCount === 1
          ? { ...incoming, writing_preferences: [], preference_guidance_provider_approved: false, preference_revision: 5, style_examples_approved: false }
          : { ...incoming, preference_revision: 6 };
        contextMappingsResponse = [saved];
        return [saved];
      }
      return base(command, args, options);
    });
    render(<ContextSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    revealPermissions();
    const cloudPreferenceToggle = screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" });
    const cloudStyleExamplesToggle = screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" });
    expect(cloudPreferenceToggle).toBeChecked();
    expect(cloudStyleExamplesToggle).toBeChecked();

    selectOption(screen.getByRole("combobox", { name: "选择 App" }), "com.google.Chrome");
    await waitFor(() => {
      expect(cloudPreferenceToggle).not.toBeChecked();
      expect(cloudStyleExamplesToggle).not.toBeChecked();
    });
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      replaceStyleExamples: true,
      mapping: expect.objectContaining({
        id: "rule:cursor-chat",
        bundle_id: "com.google.Chrome",
        preference_guidance_provider_approved: false,
        preference_revision: 4,
        style_examples_approved: false,
        style_example_pairs: [{ input: "private sample input", output: "Private sample output" }],
      }),
    }));
    expect(await screen.findByText(/本次填写的偏好没有保存/)).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).not.toBeChecked();

    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    expect(screen.getByRole("combobox", { name: "短称呼形式" })).toHaveValue("");
    selectOption(screen.getByRole("combobox", { name: "短称呼形式" }), "conversational_greeting");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith("save_context_mapping", {
      replaceStyleExamples: true,
      mapping: expect.objectContaining({
        id: "rule:cursor-chat",
        bundle_id: "com.google.Chrome",
        writing_preferences: [{ task_id: "work_chat", dimension: "greeting_tone", value: "conversational_greeting", status: "active", version: 1 }],
        preference_guidance_provider_approved: false,
        preference_revision: 5,
      }),
    }));
  });

  it("clears the editor after deleting its mapping so a later save cannot replay its preferences", async () => {
    const base = invokeMock.getMockImplementation()!;
    contextMappingsResponse = [{
      id: "rule:cursor-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [{
        task_id: "work_chat",
        dimension: "greeting_tone",
        value: "respectful_greeting",
        status: "active",
        version: 1,
      }],
      preference_guidance_provider_approved: true,
      preference_revision: 4,
      source_permissions: {},
      style_example_pairs: [],
      enabled: true,
    }];
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "delete_context_mapping") {
        contextMappingsResponse = [];
        return [];
      }
      return base(command, args, options);
    });

    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    revealPermissions();
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "删除应用映射 Cursor" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "删除映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("delete_context_mapping", { id: "rule:cursor-chat" }));
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).not.toBeChecked();

    selectOption(screen.getByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      replaceStyleExamples: true,
      mapping: expect.objectContaining({
        id: expect.stringMatching(/^rule:/),
        writing_task_id: null,
        writing_preferences: [],
        preference_guidance_provider_approved: false,
        preference_revision: 0,
      }),
    }));
  });

  it("rejects a restored edit draft when its saved mapping owner has disappeared", async () => {
    contextMappingsResponse = [{
      id: "rule:cursor-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [{
        task_id: "work_chat",
        dimension: "greeting_tone",
        value: "respectful_greeting",
        status: "active",
        version: 1,
      }],
      preference_guidance_provider_approved: true,
      preference_revision: 4,
      source_permissions: {},
      style_example_pairs: [],
      enabled: true,
    }];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "save_context_mapping") {
        const mapping = (args as { mapping: Record<string, unknown> }).mapping;
        contextMappingsResponse = [mapping];
        return [mapping];
      }
      return base(command, args, options);
    });
    const view = render(<SessionDraftProvider><ContextSettings /></SessionDraftProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "更新映射" })).toBeEnabled());

    contextMappingsResponse = [];
    view.rerender(<SessionDraftProvider>{null}</SessionDraftProvider>);
    view.rerender(<SessionDraftProvider><ContextSettings /></SessionDraftProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "更新映射" }));

    expect(await screen.findByText("此映射已删除或不再可用，请重新选择规则后再保存。")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "放弃草稿并新建规则" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "选择 App" })).toBeEnabled());
    selectOption(screen.getByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    expect(screen.getByRole("combobox", { name: "写作任务 ID" })).toHaveValue("");
    expect(screen.getByRole("combobox", { name: "短称呼形式" })).toHaveValue("");
    revealPermissions();
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        id: expect.stringMatching(/^rule:/),
        bundle_id: "com.todesktop.230313mzl4w4u92",
        writing_task_id: null,
        writing_preferences: [],
        preference_guidance_provider_approved: false,
      }),
    })));
    const savedMapping = (invokeMock.mock.lastCall?.[1] as { mapping: { id: string } }).mapping;
    expect(savedMapping.id).not.toBe("rule:cursor-chat");
  });

  it("persists explicit scene Auto separately from inheriting the global intensity", async () => {
    render(<ContextSettings />);

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    const intensity = screen.getByRole("combobox", { name: "整理强度" });
    expect(intensity).toHaveValue("inherit");
    selectOption(intensity, "auto");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
      mapping: {
        id: expect.stringMatching(/^rule:/),
        label: "Google Chrome",
        family: "email",
        mode_id: "email",
        bundle_id: "com.google.Chrome",
        executable: null,
        browser_host: "mail.google.com",
        browser_path_prefix: null,
        focused_field: null,
        writing_task_id: null,
        writing_preferences: [],
        preference_guidance_provider_approved: false,
        preference_revision: 0,
        source_permissions: {
          ax_text: false,
          local_ocr: false,
          cloud_vision: false,
          context_text_to_providers: false,
        },
        style_examples_approved: false,
        style_example_input: null,
        style_example_output: null,
        style_example_pairs: [],
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
    revealPermissions();

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.google.Chrome");
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射网站" }), { target: { value: "mail.google.com" } });
    fireEvent.change(screen.getByRole("textbox", { name: "网站路径前缀" }), { target: { value: "/issues" } });
    selectOption(screen.getByRole("combobox", { name: "输入框类型" }), "coding_prompt");
    fireEvent.click(screen.getByRole("switch", { name: "允许读取辅助功能文字" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许本机 OCR" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许自动云端视觉" }));
    fireEvent.click(screen.getByRole("switch", { name: "允许文字发送给服务商" }));
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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
    revealPermissions();

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
    revealPermissions();

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
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
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
    revealPermissions();

    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.change(screen.getByRole("textbox", { name: "App 风格示例输入" }), { target: { value: "thanks!" } });
    fireEvent.change(screen.getByRole("textbox", { name: "App 风格期望输出" }), { target: { value: "Thank you!" } });
    fireEvent.click(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" }));

    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "允许文字发送给服务商" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("button", { name: "保存映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true,
      mapping: expect.objectContaining({
        style_examples_approved: true,
        style_example_input: "thanks!",
        style_example_output: "Thank you!",
        source_permissions: expect.objectContaining({ context_text_to_providers: false }),
      }),
    }));
  });

  it("revokes the separate cloud sample grant when an approved example changes", async () => {
    contextMappingsResponse = [{
      id: "cursor",
      label: "Cursor",
      family: "general",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      style_example_input: "old input",
      style_example_output: "old output",
      style_example_pairs: [{ input: "old input", output: "old output" }],
      style_examples_approved: true,
      enabled: true,
    }];
    render(<ContextSettings />);
    revealPermissions();
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");

    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "true");
    fireEvent.change(screen.getByRole("textbox", { name: "App 风格示例输入" }), { target: { value: "new input" } });
    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      replaceStyleExamples: true,
      mapping: expect.objectContaining({
        style_example_input: "new input",
        style_examples_approved: false,
        style_example_pairs: [{ input: "old input", output: "old output" }],
      }),
    }));
  });

  it("does not display a list-only preference grant as cloud guidance approval", async () => {
    contextMappingsResponse = [{
      id: "editor",
      label: "Editor",
      family: "work_chat",
      bundle_id: "com.example.editor",
      writing_task_id: "work_chat",
      writing_preferences: [{ task_id: "work_chat", dimension: "list_structure", value: "bullets", status: "active", version: 1 }],
      preference_guidance_provider_approved: true,
      enabled: true,
    }];
    render(<ContextSettings />);
    revealPermissions();
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.example.editor");

    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toHaveAttribute("aria-checked", "false");
  });

  it("reviews a structured candidate, refreshes its mapping revision, preserves other drafts, and keeps cloud/sample grants separate", async () => {
    const mapping = {
      id: "rule:cursor-work-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [{ task_id: "work_chat", dimension: "greeting_tone", value: "respectful_greeting", status: "active", version: 1 }],
      preference_guidance_provider_approved: true,
      preference_revision: 2,
      source_permissions: {},
      style_example_input: "private sample input",
      style_example_output: "private sample output",
      style_example_pairs: [{ input: "private sample input", output: "private sample output" }],
      style_examples_approved: true,
      enabled: true,
      cleanup_intensity: "standard",
    };
    const candidate = {
      candidate_id: "candidate:cursor-chat-punctuation",
      candidate_generation: 1,
      mapping_id: mapping.id,
      mapping_revision: 2,
      app_label: "Cursor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedMapping = {
      ...mapping,
      writing_preferences: [
        ...mapping.writing_preferences,
        { task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 },
      ],
      preference_guidance_provider_approved: false,
      preference_revision: 3,
    };
    contextMappingsResponse = [mapping];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") return [candidate];
      if (command === "review_writing_preference_drafts") {
        contextMappingsResponse = [updatedMapping];
        return [{
          candidate_id: candidate.candidate_id,
          mapping_id: mapping.id,
          decision: "approve",
          status: "approved",
          new_mapping_revision: 3,
        }];
      }
      if (command === "save_context_mapping") {
        contextMappingsResponse = [(args as { mapping: unknown }).mapping];
        return contextMappingsResponse;
      }
      return base(command, args, options);
    });

    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    fireEvent.click(screen.getByText(/高级映射选项/));
    fireEvent.click(screen.getByRole("button", { name: /任务级写作偏好/ }));
    selectOption(screen.getByRole("combobox", { name: "整理强度" }), "light");
    revealPermissions();
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toBeChecked();

    expect(invokeMock).not.toHaveBeenCalledWith("list_writing_preference_drafts");
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*陈述句句末标点/ });
    expect(screen.getByText(/App: Cursor \(com\.todesktop\.230313mzl4w4u92\).*聊天输入框/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("review_writing_preference_drafts", {
      decisions: [{ candidate_id: candidate.candidate_id, candidate_generation: 1, expected_mapping_revision: 2, decision: "approve" }],
    }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "get_context_mappings")).toHaveLength(3));
    expect(screen.getByRole("combobox", { name: "整理强度" })).toHaveValue("light");
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("no_terminal_period");
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", expect.objectContaining({
      mapping: expect.objectContaining({
        id: mapping.id,
        preference_revision: 3,
        writing_preferences: updatedMapping.writing_preferences,
        preference_guidance_provider_approved: false,
        style_examples_approved: true,
        style_example_input: "private sample input",
        cleanup_intensity: "light",
      }),
    })));
  });

  it("treats a family-only editor change during preference review as a scope conflict", async () => {
    const mapping = {
      id: "rule:family-review-race",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [],
      preference_guidance_provider_approved: false,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const candidate = {
      candidate_id: "candidate:family-review-race",
      candidate_generation: 1,
      mapping_id: mapping.id,
      mapping_revision: 2,
      app_label: "Cursor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedMapping = {
      ...mapping,
      writing_preferences: [{ task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 }],
      preference_revision: 3,
    };
    let resolveReview!: (outcomes: unknown[]) => void;
    contextMappingsResponse = [mapping];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args, options) => {
      if (command === "list_writing_preference_drafts") return Promise.resolve([candidate]);
      if (command === "review_writing_preference_drafts") {
        return new Promise((resolve) => { resolveReview = resolve; });
      }
      if (command === "save_context_mapping") return Promise.resolve([(args as { mapping: unknown }).mapping]);
      return base(command, args, options);
    });

    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*陈述句句末标点/ });
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));
    await waitFor(() => expect(resolveReview).toBeDefined());

    selectOption(screen.getByRole("combobox", { name: "应用映射语气" }), "email");
    contextMappingsResponse = [updatedMapping];
    await act(async () => resolveReview([{
      candidate_id: candidate.candidate_id,
      mapping_id: mapping.id,
      decision: "approve",
      status: "approved",
      new_mapping_revision: 3,
    }]));

    expect(await screen.findByText(/批量复核期间此 App 规则的范围或任务已改变/)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "应用映射语气" })).toHaveValue("email");
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    expect(await screen.findByText("批量复核期间此 App 规则的偏好或授权草稿发生冲突。草稿仍保留；请重新加载获批规则后再保存。")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());
  });

  it("completes approval sync when one mapping was deleted and keeps the remaining mapping update", async () => {
    const mappingA = {
      id: "rule:deleted-during-review",
      label: "Google Chrome",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.google.Chrome",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [],
      preference_guidance_provider_approved: false,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const mappingB = {
      ...mappingA,
      id: "rule:still-present-during-review",
      label: "Cursor",
      bundle_id: "com.todesktop.230313mzl4w4u92",
    };
    const candidateA = {
      candidate_id: "candidate:deleted-during-review",
      candidate_generation: 1,
      mapping_id: mappingA.id,
      mapping_revision: 2,
      app_label: mappingA.label,
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false as const,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const candidateB = {
      ...candidateA,
      candidate_id: "candidate:still-present-during-review",
      mapping_id: mappingB.id,
      app_label: mappingB.label,
      dimension: "greeting_tone" as const,
      value: "respectful_greeting" as const,
    };
    const candidateAfterReload = {
      ...candidateA,
      candidate_id: "candidate:after-owner-deletion",
      mapping_id: mappingB.id,
      app_label: mappingB.label,
      mapping_revision: 3,
      dimension: "list_structure" as const,
      value: "bullets" as const,
    };
    const updatedMappingB = {
      ...mappingB,
      writing_preferences: [{ task_id: "work_chat", dimension: "greeting_tone", value: "respectful_greeting", status: "active", version: 1 }],
      preference_revision: 3,
    };
    let candidateLoadCount = 0;
    contextMappingsResponse = [mappingA, mappingB];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") {
        candidateLoadCount += 1;
        return candidateLoadCount === 1 ? [candidateA, candidateB] : [candidateAfterReload];
      }
      if (command === "review_writing_preference_drafts") {
        contextMappingsResponse = [updatedMappingB];
        return [candidateA, candidateB].map((candidate) => ({
          candidate_id: candidate.candidate_id,
          mapping_id: candidate.mapping_id,
          decision: "approve",
          status: "approved",
          new_mapping_revision: 3,
        }));
      }
      if (command === "save_context_mapping") return [(args as { mapping: unknown }).mapping];
      return base(command, args, options);
    });

    render(<SessionDraftProvider><ContextSettings /></SessionDraftProvider>);
    const editButtons = await screen.findAllByRole("button", { name: "编辑映射" });
    fireEvent.click(editButtons[1]);
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Google Chrome.*工作聊天.*陈述句句末标点/ });
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*短称呼形式/ });
    fireEvent.click(screen.getByRole("checkbox", { name: /Google Chrome.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*短称呼形式/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    expect(await screen.findByText(/处理结果：批准 2 项/)).toBeInTheDocument();
    expect(screen.queryByText(/偏好已批准，但 App 规则未能同步/)).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "短称呼形式" })).toHaveValue("respectful_greeting");
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "刷新候选列表" }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*列表结构/ });
    const candidateAfterDeletionCheckbox = screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*列表结构/ });
    fireEvent.click(candidateAfterDeletionCheckbox);
    expect(screen.getByRole("button", { name: /批准所选偏好/ })).toBeEnabled();
  });

  it("joins equal App/task candidates by stable mapping ID and never merges A into an editor switched to B", async () => {
    const mappingA = {
      id: "rule:shared-chat",
      label: "Shared Editor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.example.shared-editor",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [{ task_id: "work_chat", dimension: "greeting_tone", value: "respectful_greeting", status: "active", version: 1 }],
      preference_guidance_provider_approved: true,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const mappingB = {
      ...mappingA,
      id: "rule:shared-document",
      focused_field: "document",
      writing_preferences: [{ task_id: "work_chat", dimension: "terminal_punctuation", value: "terminal_period", status: "active", version: 1 }],
      preference_guidance_provider_approved: false,
      preference_revision: 4,
    };
    const candidate = {
      candidate_id: "candidate:shared-chat-punctuation",
      candidate_generation: 1,
      mapping_id: mappingA.id,
      mapping_revision: 2,
      app_label: "Shared Editor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedA = {
      ...mappingA,
      writing_preferences: [
        ...mappingA.writing_preferences,
        { task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 },
      ],
      preference_guidance_provider_approved: false,
      preference_revision: 3,
    };
    let mappingReadCount = 0;
    let resolveMappingRefresh: ((mappings: unknown[]) => void) | undefined;
    contextMappingsResponse = [mappingA, mappingB];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") return [candidate];
      if (command === "review_writing_preference_drafts") return [{
        candidate_id: candidate.candidate_id,
        mapping_id: mappingA.id,
        decision: "approve",
        status: "approved",
        new_mapping_revision: 3,
      }];
      if (command === "get_context_mappings") {
        mappingReadCount += 1;
        if (mappingReadCount <= 2) return [mappingA, mappingB];
        return new Promise((resolve) => { resolveMappingRefresh = resolve; });
      }
      return base(command, args, options);
    });

    render(<ContextSettings />);
    const editButtons = await screen.findAllByRole("button", { name: "编辑映射" });
    fireEvent.click(editButtons[0]);
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    expect(await screen.findByText(/App: Shared Editor \(com\.example\.shared-editor\) · 输入框: 聊天输入框/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /Shared Editor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));
    await waitFor(() => expect(mappingReadCount).toBe(3));
    await waitFor(() => expect(resolveMappingRefresh).toBeDefined());

    fireEvent.click(screen.getAllByRole("button", { name: "编辑映射" })[1]);
    const refreshedTaskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (refreshedTaskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(refreshedTaskPreferences);
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("terminal_period");
    await act(async () => resolveMappingRefresh?.([updatedA, mappingB]));

    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("terminal_period");
    expect(screen.queryByText(/此偏好维度有未保存草稿/)).not.toBeInTheDocument();
  });

  it("preserves a dirty preference and cloud grant draft until the user reloads the reviewed mapping", async () => {
    const mapping = {
      id: "rule:cursor-chat-dirty",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [],
      preference_guidance_provider_approved: false,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const candidate = {
      candidate_id: "candidate:cursor-chat-punctuation-dirty",
      candidate_generation: 1,
      mapping_id: mapping.id,
      mapping_revision: 2,
      app_label: "Cursor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedMapping = {
      ...mapping,
      writing_preferences: [{ task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 }],
      preference_guidance_provider_approved: false,
      preference_revision: 3,
    };
    contextMappingsResponse = [mapping];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") return [candidate];
      if (command === "review_writing_preference_drafts") {
        contextMappingsResponse = [updatedMapping];
        return [{
          candidate_id: candidate.candidate_id,
          mapping_id: mapping.id,
          decision: "approve",
          status: "approved",
          new_mapping_revision: 3,
        }];
      }
      if (command === "save_context_mapping") return [(args as { mapping: unknown }).mapping];
      return base(command, args, options);
    });

    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    selectOption(screen.getByRole("combobox", { name: "陈述句句末标点" }), "terminal_period");
    revealPermissions();
    fireEvent.click(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" }));

    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*陈述句句末标点/ });
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_context_mappings"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("terminal_period"));
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).toBeChecked();
    expect(screen.getByText(/此偏好维度有未保存草稿/)).toBeInTheDocument();
    expect(screen.getByText(/云端偏好授权有未保存草稿/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getAllByRole("button", { name: "重新加载获批的 App 规则" })[0]);
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("no_terminal_period");
    expect(screen.getByRole("switch", { name: "允许结构化偏好发送给云端整理服务商" })).not.toBeChecked();
    expect(screen.queryByText(/此偏好维度有未保存草稿/)).not.toBeInTheDocument();
  });

  it("keeps the opened mapping revision across remount and reloads a late approval before save", async () => {
    const mapping = {
      id: "rule:remount-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [],
      preference_guidance_provider_approved: false,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const candidate = {
      candidate_id: "candidate:remount-chat-punctuation",
      candidate_generation: 1,
      mapping_id: mapping.id,
      mapping_revision: 2,
      app_label: "Cursor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedMapping = {
      ...mapping,
      writing_preferences: [{ task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 }],
      preference_revision: 3,
    };
    let mappingReadCount = 0;
    let resolveMappingRefresh: ((mappings: unknown[]) => void) | undefined;
    contextMappingsResponse = [mapping];
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") return [candidate];
      if (command === "review_writing_preference_drafts") {
        contextMappingsResponse = [updatedMapping];
        return [{ candidate_id: candidate.candidate_id, mapping_id: mapping.id, decision: "approve", status: "approved", new_mapping_revision: 3 }];
      }
      if (command === "get_context_mappings") {
        mappingReadCount += 1;
        if (mappingReadCount <= 2) return [mapping];
        if (mappingReadCount === 3) return new Promise((resolve) => { resolveMappingRefresh = resolve; });
        return contextMappingsResponse;
      }
      return base(command, args, options);
    });

    const view = render(<SessionDraftProvider><ContextSettings /></SessionDraftProvider>);
    fireEvent.click((await screen.findAllByRole("button", { name: "编辑映射" }))[0]);
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*陈述句句末标点/ });
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));
    await waitFor(() => expect(mappingReadCount).toBe(3));
    await waitFor(() => expect(resolveMappingRefresh).toBeDefined());

    view.rerender(<SessionDraftProvider><div /></SessionDraftProvider>);
    await act(async () => resolveMappingRefresh?.([updatedMapping]));
    view.rerender(<SessionDraftProvider><ContextSettings /></SessionDraftProvider>);
    expect(await screen.findByText(/此 App 规则在当前草稿建立后已更新/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "重新加载获批的 App 规则" }));
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("no_terminal_period");
    expect(screen.queryByText(/此 App 规则在当前草稿建立后已更新/)).not.toBeInTheDocument();
  });

  it("keeps a stale form blocked when review IPC is lost after commit and a candidate refresh reads the new revision", async () => {
    const mapping = {
      id: "rule:lost-ipc-chat",
      label: "Cursor",
      family: "work_chat",
      mode_id: "work_chat",
      bundle_id: "com.todesktop.230313mzl4w4u92",
      executable: null,
      browser_host: null,
      browser_path_prefix: null,
      focused_field: "chat",
      writing_task_id: "work_chat",
      writing_preferences: [],
      preference_guidance_provider_approved: false,
      preference_revision: 2,
      source_permissions: {},
      style_example_pairs: [],
      style_examples_approved: false,
      enabled: true,
    };
    const candidate = {
      candidate_id: "candidate:lost-ipc-chat-punctuation",
      candidate_generation: 1,
      mapping_id: mapping.id,
      mapping_revision: 2,
      app_label: "Cursor",
      task_id: "work_chat",
      dimension: "terminal_punctuation",
      value: "no_terminal_period",
      has_text_sample: false,
      distinct_origins: 3,
      distinct_natural_days: 2,
      distinct_sessions: 2,
      qualified_at_ms: 1_800_000_001_000,
    };
    const updatedMapping = {
      ...mapping,
      writing_preferences: [{ task_id: "work_chat", dimension: "terminal_punctuation", value: "no_terminal_period", status: "active", version: 1 }],
      preference_revision: 3,
    };
    contextMappingsResponse = [mapping];
    let reviewAttempted = false;
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args, options) => {
      if (command === "list_writing_preference_drafts") return reviewAttempted ? [] : [candidate];
      if (command === "review_writing_preference_drafts") {
        reviewAttempted = true;
        contextMappingsResponse = [updatedMapping];
        throw new Error("IPC response lost after commit");
      }
      return base(command, args, options);
    });

    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑映射" }));
    fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
    await screen.findByRole("group", { name: /Cursor.*工作聊天.*陈述句句末标点/ });
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*工作聊天.*陈述句句末标点/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));
    const uncertainAlert = await screen.findByRole("alert");
    fireEvent.click(within(uncertainAlert).getByRole("button", { name: "刷新候选列表" }));

    expect(await screen.findByText(/此 App 规则在当前草稿建立后已更新/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    expect(invokeMock).not.toHaveBeenCalledWith("save_context_mapping", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "重新加载获批的 App 规则" }));
    const taskPreferences = screen.getByRole("button", { name: /任务级写作偏好/ });
    if (taskPreferences.getAttribute("aria-expanded") !== "true") fireEvent.click(taskPreferences);
    expect(screen.getByRole("combobox", { name: "陈述句句末标点" })).toHaveValue("no_terminal_period");
  });

  it("keeps a dirty Prompt through unmount while reconciling newer saved modes", async () => {
    const onChange = vi.fn();
    const view = render(<SessionDraftProvider><ContextSettings writingModes={modes} onWritingModesChange={onChange} /></SessionDraftProvider>);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "语气 Prompt" }), { target: { value: "保留我的草稿" } });
    view.rerender(<SessionDraftProvider>{null}</SessionDraftProvider>);
    const updated = modes.map((mode) => ({ ...mode, prompt: `后台的 ${mode.id}` }));
    view.rerender(<SessionDraftProvider><ContextSettings writingModes={updated} onWritingModesChange={onChange} /></SessionDraftProvider>);
    fireEvent.click(screen.getByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    expect(prompt).toHaveValue("保留我的草稿");
    fireEvent.click(within(prompt.closest("section")!).getByRole("button", { name: "保存语气" }));
    expect(onChange).toHaveBeenCalledWith([{ ...modes[0], prompt: "保留我的草稿" }, updated[1]]);
  });

  it("removes cleared manual examples instead of restoring the old text", async () => {
    contextMappingsResponse = [{ id: "cursor", label: "Cursor", family: "general", bundle_id: "com.todesktop.230313mzl4w4u92", enabled: true, style_example_input: "old input", style_example_output: "old output", style_example_pairs: [{ input: "retained", output: "Retained" }] }];
    render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    fireEvent.change(screen.getByLabelText("App 风格示例输入"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("App 风格期望输出"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true, mapping: expect.objectContaining({ style_example_input: null, style_example_output: null, style_example_pairs: [{ input: "retained", output: "Retained" }] }) }));
  });

  it("explicitly removes all retained example pairs and revokes their separate approval", async () => {
    contextMappingsResponse = [{ id: "cursor", label: "Cursor", family: "general", bundle_id: "com.todesktop.230313mzl4w4u92", enabled: true, style_examples_approved: true, style_example_pairs: [{ input: "retained", output: "Retained" }] }];
    render(<ContextSettings />);
    selectOption(await screen.findByRole("combobox", { name: "选择 App" }), "com.todesktop.230313mzl4w4u92");
    revealPermissions();
    fireEvent.click(screen.getByRole("button", { name: "移除全部样例" }));
    expect(screen.getByRole("switch", { name: "允许风格示例发送给整理服务商" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("button", { name: "更新映射" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", { replaceStyleExamples: true, mapping: expect.objectContaining({ style_example_input: null, style_example_output: null, style_example_pairs: [], style_examples_approved: false }) }));
  });

  it("returns keyboard focus to the Prompt after cancelling its changes", async () => {
    render(<ContextSettings />);
    fireEvent.click(await screen.findByRole("button", { name: /编辑 Prompt/ }));
    const prompt = screen.getByRole("textbox", { name: "语气 Prompt" });
    fireEvent.change(prompt, { target: { value: "Unsaved prompt" } });
    const cancelButtons = screen.getAllByRole("button", { name: "取消修改" });
    const cancel = cancelButtons[cancelButtons.length - 1];
    cancel.focus(); fireEvent.click(cancel);
    expect(prompt).toHaveFocus();
    expect(screen.queryByRole("button", { name: "取消修改" })).not.toBeInTheDocument();
  });

  it("displays the selected accurate ASR provider label dynamically instead of hardcoding DashScope", async () => {
    render(<ContextSettings automationOnly accurateAsrProvider="custom" />);
    fireEvent.click(screen.getByRole("button", { name: /高级整理设置/ }));
    expect(screen.getByText("第二枪需要 自定义 / 兼容接口 上已填的密钥。")).toBeInTheDocument();
  });

});
