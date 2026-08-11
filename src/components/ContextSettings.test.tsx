import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ContextSettings } from "./ContextSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const openMock = vi.mocked(open);

describe("ContextSettings", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
    openMock.mockReset();
    openMock.mockResolvedValue(null);
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command) => {
      switch (command) {
        case "get_context_snapshot":
          return { profile: { id: "native.general", family: "general", app_label: "Cursor", icon_key: "app", source: "fallback", confidence: 0.4 }, browser_access_status: "not_applicable" };
        case "get_context_mappings":
          return [];
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

  it("lets users choose an app without exposing native identifiers", async () => {
    render(<ContextSettings />);

    expect(await screen.findByRole("option", { name: "Cursor" })).toBeInTheDocument();
    expect(screen.queryByLabelText("应用映射 ID")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Bundle ID")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("可执行文件名")).not.toBeInTheDocument();
  });

  it("saves the selected app and writing mode using generated selectors", async () => {
    render(<ContextSettings />);

    const applicationSelect = await screen.findByRole("combobox", { name: "选择 App" });
    fireEvent.change(applicationSelect, { target: { value: "com.todesktop.230313mzl4w4u92" } });
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射写作模式" }), { target: { value: "prompt_or_code" } });
    fireEvent.click(screen.getByRole("button", { name: "保存 App 设置" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: {
        id: "com.todesktop.230313mzl4w4u92",
        label: "Cursor",
        family: "prompt_or_code",
        mode_id: "prompt_or_code",
        bundle_id: "com.todesktop.230313mzl4w4u92",
        executable: null,
        browser_host: null,
        enabled: true,
      },
    }));
  });

  it("uses a custom writing mode in an app mapping", async () => {
    render(<ContextSettings writingModes={[
      { id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true },
      { id: "custom.reply", label: "客服回复", family: "general", prompt: "保持礼貌，但不要添加承诺。", builtin: false },
    ]} />);

    fireEvent.change(await screen.findByRole("combobox", { name: "选择 App" }), { target: { value: "com.todesktop.230313mzl4w4u92" } });
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射写作模式" }), { target: { value: "custom.reply" } });
    fireEvent.click(screen.getByRole("button", { name: "保存 App 设置" }));

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

    const prompt = await screen.findByRole("textbox", { name: "写作模式 Prompt" });
    fireEvent.change(prompt, { target: { value: "保留我的语气，只修正明显的语病。" } });
    fireEvent.click(screen.getByRole("button", { name: "保存模式" }));

    expect(onWritingModesChange).toHaveBeenCalledWith(expect.arrayContaining([
      expect.objectContaining({ id: "general", prompt: "保留我的语气，只修正明显的语病。" }),
    ]));

    fireEvent.change(screen.getByRole("combobox", { name: "编辑写作模式" }), { target: { value: "__add_custom_mode__" } });
    expect(onWritingModesChange).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "保存模式" }));
    expect(onWritingModesChange).toHaveBeenLastCalledWith(expect.arrayContaining([
      expect.objectContaining({ label: "自定义模式", builtin: false }),
    ]));
  });

  it("adds an app selected from the Applications folder", async () => {
    openMock.mockResolvedValue("/Applications/Obsidian.app");
    render(<ContextSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "从应用程序中选择" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_application_from_path", {
      path: "/Applications/Obsidian.app",
    }));
    expect(await screen.findByRole("option", { name: "Obsidian" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "选择 App" })).toHaveValue("md.obsidian");
  });

  it("keeps browser website mappings user-friendly", async () => {
    render(<ContextSettings />);

    fireEvent.change(await screen.findByRole("combobox", { name: "选择 App" }), { target: { value: "com.google.Chrome" } });
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射网站" }), { target: { value: "mail.google.com" } });
    fireEvent.change(screen.getByRole("combobox", { name: "应用映射写作模式" }), { target: { value: "email" } });
    fireEvent.click(screen.getByRole("button", { name: "保存 App 设置" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("save_context_mapping", {
      mapping: {
        id: "com.google.Chrome:mail.google.com",
        label: "Google Chrome · Gmail",
        family: "email",
        mode_id: "email",
        bundle_id: null,
        executable: null,
        browser_host: "mail.google.com",
        enabled: true,
      },
    }));
  });
});
