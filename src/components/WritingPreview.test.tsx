import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { WritingPreview } from "./WritingPreview";
import type { WritingMode } from "./ContextSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const mode: WritingMode = { id: "general", label: "通用", family: "general", prompt: "简洁，保留事实。", builtin: true };

describe("WritingPreview", () => {
  afterEach(cleanup);
  beforeEach(() => { invokeMock.mockReset(); });

  it("sends only an explicit sample and draft, and identifies model versus fallback results", async () => {
    invokeMock.mockResolvedValue({ saved: { text: "已保存输出", status: "model", elapsed_ms: 420 }, draft: { text: "保留 API", status: "guard_fallback", elapsed_ms: 100 } });
    render(<WritingPreview mode={mode} compareSaved />);
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: "保留 API" } });
    fireEvent.click(screen.getByRole("button", { name: "试跑并对比" }));
    expect(await screen.findByText("已保存输出")).toBeInTheDocument();
    expect(screen.getByText("模型结果 · 0.4s")).toBeInTheDocument();
    expect(screen.getByText("事实保护规则触发，已回退 · 0.1s")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("preview_writing_mode", { request: { request_id: expect.any(String), text: "保留 API", mode, compare_saved: true } });
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["preview_writing_mode"]);
  });

  it("cancels an edited draft and ignores its late result", async () => {
    let resolve: (value: unknown) => void = () => {};
    invokeMock.mockImplementation((command) => command === "preview_writing_mode" ? new Promise((done) => { resolve = done; }) : Promise.resolve(undefined));
    const { rerender } = render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    const request = invokeMock.mock.calls[0][1] as { request: { request_id: string } };
    rerender(<WritingPreview mode={{ ...mode, prompt: "用短句。" }} compareSaved />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", { requestId: request.request.request_id }));
    resolve({ saved: null, draft: { text: "过期结果", status: "model", elapsed_ms: 10 } });
    await waitFor(() => expect(screen.getByRole("button", { name: "试跑并对比" })).toBeEnabled());
    expect(screen.queryByText("过期结果")).not.toBeInTheDocument();
  });

  it("shows local-only status honestly and never renders a provider error payload", async () => {
    invokeMock.mockResolvedValueOnce({ saved: null, draft: { text: "本地输出", status: "local_only", elapsed_ms: 1 } }).mockRejectedValueOnce("private provider detail");
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/本地处理，未调用模型/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("试跑未完成");
    expect(screen.queryByText("private provider detail")).not.toBeInTheDocument();
  });

  it("blocks empty samples and cancels when closed", async () => {
    invokeMock.mockImplementation(() => new Promise(() => {}));
    const { unmount } = render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: " " } });
    expect(screen.getByRole("button", { name: "试跑当前语气" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "试跑当前语气" })).toHaveAccessibleDescription("请先填写试跑文本。");
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: "样例" } });
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    unmount();
    expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", expect.any(Object));
  });

  it("returns focus to the run button after a manual cancellation", async () => {
    invokeMock.mockImplementation((command) => command === "preview_writing_mode" ? new Promise(() => {}) : Promise.resolve(undefined));
    render(<WritingPreview mode={mode} compareSaved={false} />);
    const run = screen.getByRole("button", { name: "试跑当前语气" });
    fireEvent.click(run);
    const cancel = screen.getByRole("button", { name: "取消" });
    cancel.focus();
    fireEvent.click(cancel);
    expect(run).toBeEnabled();
    expect(run).toHaveFocus();
    expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", expect.any(Object));
  });
});
